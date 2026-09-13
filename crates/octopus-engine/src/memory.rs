//! 记忆索引重建（#05/#27 M2）：从权威命令日志重建派生向量索引。
//!
//! 重建是唯一能补齐被删 / 被重建索引的入口；换 embedding 模型或维度后也应调用它。
//! 这里只做「读权威日志 → 交给 VectorIndex 覆盖式重建」；文本的 embedding 由索引
//! 实现通过注入的 EmbeddingBackend 现场完成（所以调用方无需预先算向量）。

use std::sync::Arc;

use crate::{
    error::EngineError,
    ports::{EmbeddingBackend, MemoryHit, MemoryRetriever, VectorIndex},
    storage::SqliteStore,
};

/// 记忆索引协调器：把权威命令日志喂给派生向量索引。
pub struct MemoryIndexer {
    store: Arc<SqliteStore>,
    index: Arc<dyn VectorIndex>,
}

impl MemoryIndexer {
    pub fn new(store: Arc<SqliteStore>, index: Arc<dyn VectorIndex>) -> Self {
        Self { store, index }
    }

    /// 重建某存档的向量索引，返回写入的叙事事件条数。
    ///
    /// 派生数据：失败不改权威状态，调用方可随时重试。索引若被删/换维度后为空，
    /// 也由本方法重新填满。
    pub async fn rebuild_memory_index(&self, save_id: &str) -> Result<usize, EngineError> {
        let rows = self.store.load_narrative_events(save_id).await?;
        let count = rows.len();
        self.index.rebuild_from(save_id, &rows).await?;
        Ok(count)
    }
}

/// 混合检索器（#05 §3.4）：向量 top-K ∪ FTS top-K，按 seq 去重（向量优先），取前 K。
///
/// 降级链：无向量库 / embedding 失败 / 维度不符 → 只用 FTS；FTS 也没结果 → 返回空。
/// 任何一步失败都只记 warn，不向上抛（权威回合与本检索无关）。
pub struct HybridMemoryRetriever {
    store: Arc<SqliteStore>,
    embedding: Arc<dyn EmbeddingBackend>,
    index: Option<Arc<dyn VectorIndex>>,
}

impl HybridMemoryRetriever {
    pub fn new(
        store: Arc<SqliteStore>,
        embedding: Arc<dyn EmbeddingBackend>,
        index: Option<Arc<dyn VectorIndex>>,
    ) -> Self {
        Self { store, embedding, index }
    }
}

#[async_trait::async_trait]
impl MemoryRetriever for HybridMemoryRetriever {
    async fn retrieve(
        &self,
        save_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryHit>, EngineError> {
        if k == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // 命中的 seq 顺序即最终顺序：向量先入（优先），FTS 只补未命中的。
        let mut ordered: Vec<i64> = Vec::new();
        let mut scores: std::collections::HashMap<i64, f32> = std::collections::HashMap::new();

        // ① 向量检索：失败只 warn，继续走 FTS。
        if let Some(index) = &self.index {
            match self.embedding.embed(query).await {
                Ok(query_vec) => match index.search(save_id, &query_vec, k).await {
                    Ok(hits) => {
                        for (seq, score) in hits {
                            // 只收新 seq；vector 命中优先，后续 FTS 不覆盖。
                            if !scores.contains_key(&seq) {
                                scores.insert(seq, score);
                                ordered.push(seq);
                            }
                        }
                    }
                    Err(e) => tracing::warn!(save_id = %save_id, error = %e, "向量检索失败，降级为仅 FTS"),
                },
                Err(e) => tracing::warn!(save_id = %save_id, error = %e, "查询向量生成失败，降级为仅 FTS"),
            }
        }

        // ② FTS5 关键词兜底：bm25 越小越相关，取负统一成「越大越相关」。
        match self.store.search_events_fts(save_id, query, k).await {
            Ok(hits) => {
                for (seq, bm25) in hits {
                    // FTS 只补向量没召回的 seq（向量胜出）；bm25 越小越相关，取负统一语义。
                    if !scores.contains_key(&seq) {
                        scores.insert(seq, -bm25);
                        ordered.push(seq);
                    }
                }
            }
            Err(e) => tracing::warn!(save_id = %save_id, error = %e, "FTS 检索失败，本次不注入相关往事"),
        }

        if ordered.is_empty() {
            return Ok(Vec::new());
        }
        // 命中的只是 seq：原文 / round / kind 从权威日志回填，保证注入的是原文。
        let rows = self.store.load_narrative_events_by_seqs(save_id, &ordered).await?;
        let mut by_seq: std::collections::HashMap<i64, (u32, String, String)> =
            std::collections::HashMap::with_capacity(rows.len());
        for (seq, round, kind, text) in rows {
            by_seq.insert(seq, (round, kind, text));
        }
        let mut out = Vec::with_capacity(k);
        for seq in ordered {
            if out.len() >= k {
                break;
            }
            let Some((round, kind, text)) = by_seq.remove(&seq) else {
                continue;
            };
            out.push(MemoryHit {
                seq,
                round,
                kind,
                text,
                score: scores.get(&seq).copied().unwrap_or(0.0),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_types::{EventEnvelope, NarratePayload, PlayEvent};

    struct FakeEmbedding;

    #[async_trait::async_trait]
    impl EmbeddingBackend for FakeEmbedding {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, EngineError> {
            Ok(vec![1.0, 0.0, 0.0])
        }
        fn dimension(&self) -> usize {
            3
        }
    }

    /// 可编程的假向量索引：返回预设命中，或模拟失败以验证降级。
    struct FakeIndex {
        hits: Vec<(i64, f32)>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl VectorIndex for FakeIndex {
        fn indexed_count(&self, _save_id: &str) -> Result<usize, EngineError> {
            Ok(self.hits.len())
        }
        async fn upsert(
            &self,
            _save_id: &str,
            _seq: i64,
            _round: u32,
            _kind: &str,
            _text: &str,
            _embedding: &[f32],
        ) -> Result<(), EngineError> {
            Ok(())
        }
        async fn search(&self, _save_id: &str, _q: &[f32], _k: usize) -> Result<Vec<(i64, f32)>, EngineError> {
            if self.fail {
                Err(EngineError::Internal("向量库不可用".into()))
            } else {
                Ok(self.hits.clone())
            }
        }
        async fn dimension(&self) -> Result<usize, EngineError> {
            Ok(3)
        }
        async fn rebuild_from(
            &self,
            _save_id: &str,
            _rows: &[(i64, u32, String, String)],
        ) -> Result<(), EngineError> {
            Ok(())
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
            event: PlayEvent::Narrate(NarratePayload { content: text.into(), scene_ref: None }),
        }
    }

    async fn store_with_events() -> Arc<SqliteStore> {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        store.append_event("sv-1", &narrate(1, "月光下的古堡")).await.unwrap();
        store.append_event("sv-1", &narrate(2, "森林中的小屋")).await.unwrap();
        store.append_event("sv-2", &narrate(1, "别处的古堡")).await.unwrap();
        store
    }

    /// 向量 ∪ FTS：向量命中优先且去重，FTS 只补未命中的，原文从权威日志回填。
    #[tokio::test]
    async fn merges_vector_and_fts_and_dedupes_by_seq() {
        let store = store_with_events().await;
        let index = Arc::new(FakeIndex { hits: vec![(1, 0.9)], fail: false });
        let r = HybridMemoryRetriever::new(store, Arc::new(FakeEmbedding), Some(index as Arc<dyn VectorIndex>));
        // 查询「森林」：向量命中 seq1，FTS 命中 seq2 → [1, 2]。
        let hits = r.retrieve("sv-1", "森林", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(hits[0].text, "月光下的古堡");
        assert_eq!(hits[1].text, "森林中的小屋");
        assert!((hits[0].score - 0.9).abs() < 1e-6);
        // 查询「古堡」：向量与 FTS 都命中 seq1 → 去重后只 1 条，向量分胜出。
        let hits = r.retrieve("sv-1", "古堡", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1]);
        assert!((hits[0].score - 0.9).abs() < 1e-6, "向量分应覆盖 FTS 分");
        // save_id 隔离：sv-2 只回自己的 seq1。
        let hits = r.retrieve("sv-2", "古堡", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1]);
        assert_eq!(hits[0].text, "别处的古堡");
    }

    /// 向量失败 → 静默降级为仅 FTS（不报错）。
    #[tokio::test]
    async fn vector_failure_falls_back_to_fts_only() {
        let store = store_with_events().await;
        let index = Arc::new(FakeIndex { hits: vec![(1, 0.9)], fail: true });
        let r = HybridMemoryRetriever::new(store, Arc::new(FakeEmbedding), Some(index as Arc<dyn VectorIndex>));
        let hits = r.retrieve("sv-1", "古堡", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1]);
        assert!(hits[0].score > 0.0, "FTS 分取负后应大于 0");
    }

    /// 无索引 → 只用 FTS。
    #[tokio::test]
    async fn without_index_uses_fts_only() {
        let store = store_with_events().await;
        let r = HybridMemoryRetriever::new(store, Arc::new(FakeEmbedding), None);
        let hits = r.retrieve("sv-1", "古堡", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1]);
    }

    /// 保留前 K 条；空查询 / k=0 直接返回空。
    #[tokio::test]
    async fn respects_top_k_and_empty_query() {
        let store = store_with_events().await;
        let r = HybridMemoryRetriever::new(store, Arc::new(FakeEmbedding), None);
        assert!(r.retrieve("sv-1", "古堡", 0).await.unwrap().is_empty());
        assert!(r.retrieve("sv-1", " 。！", 5).await.unwrap().is_empty());
        // 「的」同时命中 sv-1 的 seq1 与 seq2：k=1 截断，k=5 全给。
        assert_eq!(r.retrieve("sv-1", "的", 1).await.unwrap().len(), 1);
        assert_eq!(r.retrieve("sv-1", "的", 5).await.unwrap().len(), 2);
    }

    /// #05 §3.2/§3.3：摘要也进 FTS5，现有混合检索器**无需特判**即可召回。
    #[tokio::test]
    async fn summaries_are_reachable_through_fts_retrieval() {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        let save = "sv-sum";
        store
            .upsert_round_summary(save, 3, "在废弃矿洞里发现了一块会发光的矿石")
            .await
            .unwrap();
        store
            .upsert_scene_summary(save, "sc-2", 4, "矿洞深处遭遇了守卫石像")
            .await
            .unwrap();

        // 无向量库 → 纯 FTS 路径；检索器代码没有任何摘要特判。
        let r = HybridMemoryRetriever::new(store.clone(), Arc::new(FakeEmbedding), None);
        let hits = r.retrieve(save, "矿石", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "summary", "摘要要有可区分 kind");
        assert!(hits[0].text.contains("发光"));
        assert_eq!(hits[0].round, 3);

        let hits = r.retrieve(save, "守卫", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "scene_summary");
        assert_eq!(hits[0].round, 4);

        // 重建路径（load_narrative_events）也要纳入摘要，否则换 embedding 后摘要会丢。
        let rows = store.load_narrative_events(save).await.unwrap();
        assert_eq!(rows.len(), 2, "摘要应参与索引重建: {rows:?}");
    }
}
