//! octopus-index：派生向量索引（#27/#05 M2）。
//!
//! 只做 DuckDB 向量库；不参与权威（replay 不读），可整体重建。
//! 所有失败都返回清晰的 EngineError；组合根可 best-effort 忽略它，应用照常启动。

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use duckdb::{params, Connection};
use octopus_engine::{EmbeddingBackend, EngineError, VectorIndex};

/// DuckDB 向量库：文件 `octopus-vectors.duckdb` 与权威 SQLite 同级，派生可重建。
pub struct DuckDbVectorIndex {
    conn: Mutex<Connection>,
    dim: usize,
    /// 重建时现场生成向量；写路径（upsert）由调用方传入已算好的向量。
    embedding: Arc<dyn EmbeddingBackend>,
    /// 本次打开是否**丢弃了已有表**（指纹/维度变化）。API 据此在启动期重灌——
    /// 启动时还没有任何会话/写队列，是唯一无竞态的重建时机。
    reset: bool,
}

impl DuckDbVectorIndex {
    /// 打开 / 创建向量库，并按当前 embedding 维度校验 schema。
    ///
    /// 若 `meta.dim` 与当前维度不一致：丢弃旧表重建（派生数据不需要迁移）。
    pub fn open(path: &str, embedding: Arc<dyn EmbeddingBackend>) -> Result<Self, EngineError> {
        let conn = Connection::open(path).map_err(|e| err("打开", e))?;
        Self::from_conn(conn, embedding)
    }

    /// 仅内存（测试用）。
    pub fn open_in_memory(embedding: Arc<dyn EmbeddingBackend>) -> Result<Self, EngineError> {
        let conn = Connection::open_in_memory().map_err(|e| err("打开内存库", e))?;
        Self::from_conn(conn, embedding)
    }

    fn from_conn(conn: Connection, embedding: Arc<dyn EmbeddingBackend>) -> Result<Self, EngineError> {
        let dim = embedding.dimension();
        if dim == 0 {
            return Err(EngineError::Internal("embedding 维度为 0，无法建立向量索引".into()));
        }
        // 指纹 = 后端 + 模型 + 维度。只比维度是不够的：Stub(512) 与 bge-small(512) 维度相同，
        // 但向量语义完全不同；不比指纹就会把两批向量混在一起检索。
        let fingerprint = embedding.fingerprint();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (k VARCHAR PRIMARY KEY, v VARCHAR)")
            .map_err(|e| err("创建 meta 表", e))?;
        let old_dim = read_meta(&conn, "dim")?.and_then(|s| s.parse::<usize>().ok());
        let old_fp = read_meta(&conn, "fingerprint")?;
        let reusable = old_dim == Some(dim) && old_fp.as_deref() == Some(fingerprint.as_str());
        let mut reset = false;
        if reusable {
            create_event_vectors(&conn, dim)?;
        } else {
            reset = old_dim.is_some();
            // 首次创建（旧维度为空）不算变化；其余情况说明换了后端/模型/维度。
            if old_dim.is_some() {
                tracing::warn!(
                    old_dim = ?old_dim,
                    old_fingerprint = ?old_fp,
                    new_fingerprint = %fingerprint,
                    "向量库指纹或维度变化，丢弃旧索引（派生数据可重建；旧向量需 rebuild_memory_index 补回）"
                );
            }
            conn.execute_batch("DROP TABLE IF EXISTS event_vectors")
                .map_err(|e| err("丢弃旧向量表", e))?;
            create_event_vectors(&conn, dim)?;
            write_meta(&conn, "dim", &dim.to_string())?;
            write_meta(&conn, "fingerprint", &fingerprint)?;
        }
        Ok(Self { conn: Mutex::new(conn), dim, embedding, reset })
    }

    /// 本次打开是否丢弃了旧表（调用方据此决定是否重灌）。
    pub fn was_reset(&self) -> bool {
        self.reset
    }

    /// 某存档当前向量条数（测试 / 运维用；不在 VectorIndex 端口里）。
    pub fn count(&self, save_id: &str) -> Result<i64, EngineError> {
        self.lock()?
            .query_row(
                "SELECT COUNT(*) FROM event_vectors WHERE save_id = ?",
                params![save_id],
                |r| r.get(0),
            )
            .map_err(|e| err("统计条数", e))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, EngineError> {
        self.conn
            .lock()
            .map_err(|_| EngineError::Internal("向量库锁中毒".into()))
    }

    /// 校验向量维度，避免不同模型 / 脏数据写进固定维度列。
    fn check_dim(&self, embedding: &[f32]) -> Result<(), EngineError> {
        if embedding.len() != self.dim {
            return Err(EngineError::Storage(format!(
                "向量维度不符：期望 {}，实际 {}",
                self.dim,
                embedding.len()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl VectorIndex for DuckDbVectorIndex {
    async fn upsert(
        &self,
        save_id: &str,
        seq: i64,
        round: u32,
        kind: &str,
        text: &str,
        embedding: &[f32],
    ) -> Result<(), EngineError> {
        self.check_dim(embedding)?;
        let lit = vector_literal(embedding)?;
        let sql = format!(
            "INSERT OR REPLACE INTO event_vectors (save_id, seq, round, kind, text, embedding) \
             VALUES (?, ?, ?, ?, ?, {lit}::FLOAT[{}])",
            self.dim
        );
        self.lock()?
            .execute(&sql, params![save_id, seq, round as i32, kind, text])
            .map_err(|e| err("写入向量", e))?;
        Ok(())
    }

    async fn search(
        &self,
        save_id: &str,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>, EngineError> {
        self.check_dim(query)?;
        if k == 0 {
            return Ok(Vec::new());
        }
        let lit = vector_literal(query)?;
        let sql = format!(
            "SELECT seq, CAST(array_cosine_similarity(embedding, {lit}::FLOAT[{}]) AS DOUBLE) AS score \
             FROM event_vectors WHERE save_id = ? ORDER BY score DESC LIMIT ?",
            self.dim
        );
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql).map_err(|e| err("准备检索", e))?;
        let rows = stmt
            .query_map(params![save_id, k as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)? as f32))
            })
            .map_err(|e| err("执行检索", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| err("读取检索结果", e))?);
        }
        Ok(out)
    }

    fn indexed_count(&self, save_id: &str) -> Result<usize, EngineError> {
        self.count(save_id).map(|n| n.max(0) as usize)
    }

    async fn dimension(&self) -> Result<usize, EngineError> {
        Ok(self.dim)
    }

    async fn rebuild_from(
        &self,
        save_id: &str,
        rows: &[(i64, u32, String, String)],
    ) -> Result<(), EngineError> {
        // 先在锁外算好全部向量：embedding 可能较慢（首次还会加载模型），避免长时间持锁。
        let mut prepared: Vec<(i64, i32, String, String, String)> = Vec::with_capacity(rows.len());
        for (seq, round, kind, text) in rows {
            if text.trim().is_empty() {
                continue;
            }
            let emb = self.embedding.embed(text).await?;
            self.check_dim(&emb)?;
            prepared.push((*seq, *round as i32, kind.clone(), text.clone(), vector_literal(&emb)?));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(|e| err("开启重建事务", e))?;
        tx.execute("DELETE FROM event_vectors WHERE save_id = ?", params![save_id])
            .map_err(|e| err("清空旧向量", e))?;
        for (seq, round, kind, text, lit) in &prepared {
            let sql = format!(
                "INSERT INTO event_vectors (save_id, seq, round, kind, text, embedding) \
                 VALUES (?, ?, ?, ?, ?, {lit}::FLOAT[{}])",
                self.dim
            );
            tx.execute(&sql, params![save_id, seq, round, kind, text])
                .map_err(|e| err("重建写入向量", e))?;
        }
        tx.commit().map_err(|e| err("提交重建事务", e))?;
        Ok(())
    }
}

fn err(ctx: &str, e: duckdb::Error) -> EngineError {
    EngineError::Storage(format!("向量库{ctx}失败: {e}"))
}

fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>, EngineError> {
    let mut stmt = conn
        .prepare("SELECT v FROM meta WHERE k = ?")
        .map_err(|e| err("读取元数据", e))?;
    let mut rows = stmt.query(params![key]).map_err(|e| err("查询元数据", e))?;
    match rows.next().map_err(|e| err("读取元数据行", e))? {
        Some(row) => {
            let v: String = row.get(0).map_err(|e| err("解析元数据列", e))?;
            Ok(Some(v))
        }
        None => Ok(None),
    }
}

fn write_meta(conn: &Connection, key: &str, value: &str) -> Result<(), EngineError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (k, v) VALUES (?, ?)",
        params![key, value],
    )
    .map_err(|e| err("记录元数据", e))?;
    Ok(())
}

fn create_event_vectors(conn: &Connection, dim: usize) -> Result<(), EngineError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS event_vectors (\
         save_id VARCHAR, seq BIGINT, round INTEGER, kind VARCHAR, text VARCHAR, \
         embedding FLOAT[{dim}], PRIMARY KEY (save_id, seq))"
    ))
    .map_err(|e| err("创建向量表", e))
}

/// 把 f32 向量格式化成 DuckDB ARRAY 字面量 `[1.0,2.0]`。
///
/// duckdb-rs 尚不支持绑定 Array 参数（Value::Array 会被判为 unsupported），
/// 故内联为纯数字字面量——只含有限浮点数，不存在 SQL 注入面。
fn vector_literal(v: &[f32]) -> Result<String, EngineError> {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if !x.is_finite() {
            return Err(EngineError::Storage(format!("向量含非有限值：{x}")));
        }
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use octopus_engine::MemoryIndexer;
    use octopus_engine::SqliteStore;
    use octopus_types::{EventEnvelope, NarratePayload, PlayEvent};

    /// 确定性伪向量：把字节散到 dim 维后归一化，足以验证存取 / 排序 / 重建条数。
    struct TestEmbedding {
        dim: usize,
    }

    impl TestEmbedding {
        fn new(dim: usize) -> Self {
            Self { dim }
        }
    }

    #[async_trait]
    impl EmbeddingBackend for TestEmbedding {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError> {
            let mut v = vec![0f32; self.dim];
            for (i, b) in text.bytes().enumerate() {
                v[i % self.dim] += (b as f32) / 255.0;
            }
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
            for x in &mut v {
                *x /= norm;
            }
            Ok(v)
        }

        fn dimension(&self) -> usize {
            self.dim
        }
    }

    fn narrate(seq: u64, text: &str) -> EventEnvelope {
        EventEnvelope {
            id: format!("ev-{seq}"),
            seq,
            round: 1,
            ts: "t".into(),
            actor: None,
            intent_id: None,
            event: PlayEvent::Narrate(NarratePayload {
                content: text.to_string(),
                scene_ref: None,
            }),
        }
    }

    /// #05/#27 M2：upsert → search 往返，且按 save_id 隔离、按相似度降序。
    #[tokio::test]
    async fn upsert_then_search_round_trips() {
        let index = DuckDbVectorIndex::open_in_memory(Arc::new(TestEmbedding::new(4))).unwrap();
        index.upsert("sv-1", 1, 1, "narrate", "a", &[1.0, 0.0, 0.0, 0.0]).await.unwrap();
        index.upsert("sv-1", 2, 1, "narrate", "b", &[0.0, 1.0, 0.0, 0.0]).await.unwrap();
        index.upsert("sv-2", 1, 1, "narrate", "c", &[0.0, 0.0, 1.0, 0.0]).await.unwrap();

        let hits = index.search("sv-1", &[1.0, 0.0, 0.0, 0.0], 5).await.unwrap();
        assert_eq!(hits.len(), 2, "只返回该存档的两条");
        assert_eq!(hits[0].0, 1);
        assert!((hits[0].1 - 1.0).abs() < 1e-5, "相同向量余弦≈1: {}", hits[0].1);
        assert!(hits[0].1 > hits[1].1, "按相似度降序");
        assert_eq!(index.search("sv-2", &[1.0, 0.0, 0.0, 0.0], 5).await.unwrap().len(), 1);

        // upsert 覆盖同 (save_id, seq)：不会产生重复行。
        index.upsert("sv-1", 1, 2, "narrate", "a2", &[0.0, 0.0, 0.0, 1.0]).await.unwrap();
        assert_eq!(index.count("sv-1").unwrap(), 2);
        assert_eq!(index.search("sv-1", &[0.0, 0.0, 0.0, 1.0], 5).await.unwrap()[0].0, 1);

        // k=0 返回空，不报错。
        assert!(index.search("sv-1", &[1.0, 0.0, 0.0, 0.0], 0).await.unwrap().is_empty());
    }

    /// 维度不符 → 打开时丢弃旧表重建；同维度重开保留数据。
    #[tokio::test]
    async fn dimension_mismatch_drops_and_recreates_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("octopus-vectors.duckdb").to_string_lossy().into_owned();
        {
            let index = DuckDbVectorIndex::open(&path, Arc::new(TestEmbedding::new(3))).unwrap();
            index.upsert("sv-1", 1, 1, "narrate", "x", &[1.0, 0.0, 0.0]).await.unwrap();
            index.upsert("sv-1", 2, 1, "narrate", "y", &[0.0, 1.0, 0.0]).await.unwrap();
            assert_eq!(index.count("sv-1").unwrap(), 2);
        }
        // 换维度（新模型）：旧向量清空，表按新维度重建。
        let index2 = DuckDbVectorIndex::open(&path, Arc::new(TestEmbedding::new(4))).unwrap();
        assert_eq!(index2.count("sv-1").unwrap(), 0, "维度变更后旧索引应被丢弃");
        index2.upsert("sv-1", 1, 1, "narrate", "x", &[1.0, 0.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(index2.count("sv-1").unwrap(), 1);
        assert_eq!(index2.search("sv-1", &[1.0, 0.0, 0.0, 0.0], 5).await.unwrap()[0].0, 1);
        drop(index2);
        // 同维度重开：数据保留。
        let index3 = DuckDbVectorIndex::open(&path, Arc::new(TestEmbedding::new(4))).unwrap();
        assert_eq!(index3.count("sv-1").unwrap(), 1, "同维度重开数据应保留");
        assert_eq!(index3.dimension().await.unwrap(), 4);
    }

    /// 重建入口产出的条数与增量 upsert 一致；重复重建幂等（覆盖而非追加）。
    #[tokio::test]
    async fn rebuild_memory_index_matches_incremental_row_count() {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        let save_id = "sv-rebuild";
        for i in 0..5u64 {
            store.append_event(save_id, &narrate(i + 1, &format!("事件 {i}"))).await.unwrap();
        }
        // 非叙事事件不参与重建。
        store
            .append_event(
                save_id,
                &EventEnvelope {
                    id: "r".into(),
                    seq: 99,
                    round: 1,
                    ts: "t".into(),
                    actor: None,
                    intent_id: None,
                    event: PlayEvent::Reasoning(octopus_types::ReasoningPayload {
                        stage: "s".into(),
                        text: "x".into(),
                        source: "provider".into(),
                    }),
                },
            )
            .await
            .unwrap();

        let emb: Arc<dyn EmbeddingBackend> = Arc::new(TestEmbedding::new(4));
        // 增量路径：调用方算好向量逐条 upsert。
        let incremental = Arc::new(DuckDbVectorIndex::open_in_memory(emb.clone()).unwrap());
        let rows = store.load_narrative_events(save_id).await.unwrap();
        for (seq, round, kind, text) in &rows {
            let v = emb.embed(text).await.unwrap();
            incremental.upsert(save_id, *seq, *round, kind, text, &v).await.unwrap();
        }
        let incremental_count = incremental.count(save_id).unwrap();
        assert_eq!(incremental_count, 5);

        // 重建路径：MemoryIndexer 从权威命令日志覆盖式重建。
        let rebuilt = Arc::new(DuckDbVectorIndex::open_in_memory(emb.clone()).unwrap());
        let indexer = MemoryIndexer::new(store.clone(), rebuilt.clone());
        assert_eq!(indexer.rebuild_memory_index(save_id).await.unwrap(), 5);
        assert_eq!(rebuilt.count(save_id).unwrap(), incremental_count);

        // 再重建一次：覆盖而非追加。
        assert_eq!(indexer.rebuild_memory_index(save_id).await.unwrap(), 5);
        assert_eq!(rebuilt.count(save_id).unwrap(), 5);
    }

    /// 回归：换 embedding 后端（维度相同、语义不同）时必须丢弃旧向量表重建。
    /// Stub(512) → bge-small(512) 正是「维度相同但向量绝不能混用」的情形。
    #[tokio::test]
    async fn fingerprint_change_drops_and_recreates_the_index() {
        struct NamedEmbedding {
            dim: usize,
            name: &'static str,
        }

        #[async_trait]
        impl EmbeddingBackend for NamedEmbedding {
            async fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError> {
                let mut v = vec![0f32; self.dim];
                for (i, b) in text.bytes().enumerate() {
                    v[i % self.dim] += (b as f32) / 255.0;
                }
                let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                for x in &mut v {
                    *x /= n;
                }
                Ok(v)
            }

            fn dimension(&self) -> usize {
                self.dim
            }

            fn backend_name(&self) -> &'static str {
                self.name
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.duckdb");
        let p = path.to_str().unwrap();

        let idx =
            DuckDbVectorIndex::open(p, Arc::new(NamedEmbedding { dim: 4, name: "stub" })).unwrap();
        idx.upsert("sv-1", 1, 1, "narrate", "a", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();
        assert_eq!(idx.count("sv-1").unwrap(), 1);
        drop(idx);

        let idx2 = DuckDbVectorIndex::open(p, Arc::new(NamedEmbedding { dim: 4, name: "fastembed" }))
            .unwrap();
        assert_eq!(idx2.count("sv-1").unwrap(), 0, "换后端后旧向量必须被丢弃");
    }
}
