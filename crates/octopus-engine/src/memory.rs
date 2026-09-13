//! 记忆检索（#05 §3.4）：用权威单库的 FTS5 关键词索引召回相关往事。
//!
//! 检索是**派生路径**：任何失败都只记 warn / 返回空，绝不把失败传导给权威回合与
//! `replay()`。命中的只是 seq，正文一律从权威命令日志 / 派生摘要表回填。

use std::sync::Arc;

use crate::{
    error::EngineError,
    ports::{MemoryHit, MemoryRetriever},
    storage::SqliteStore,
};

/// FTS5 关键词检索器：bm25 排序取 top-K，按 seq 去重后回填原文。
///
/// 关键词检索对短查询命中好；整段连续中文会合成短语，召回偏窄（见 `text_index`）。
pub struct FtsMemoryRetriever {
    store: Arc<SqliteStore>,
}

impl FtsMemoryRetriever {
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl MemoryRetriever for FtsMemoryRetriever {
    async fn retrieve(
        &self,
        save_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryHit>, EngineError> {
        if k == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // bm25 越小越相关，取负统一成「越大越相关」；命中的 seq 顺序即相关性顺序。
        let mut ordered: Vec<i64> = Vec::new();
        let mut scores: std::collections::HashMap<i64, f32> = std::collections::HashMap::new();
        match self.store.search_events_fts(save_id, query, k).await {
            Ok(hits) => {
                for (seq, bm25) in hits {
                    // 同一 seq 只收一次（重复命中时保留首见的分数）。
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

/// 记忆索引协调器（#05 M2 遗留入口）：当前是**空实现**。
///
/// 向量索引相关实现已整体移除；这里只保留类型与入口，方便将来接回，重建不做任何事、
/// 直接返回写入条数 0。FTS5 关键词索引由 storage 在写路径同事务维护，不经过这里。
///
/// 调用方（组合根 / 后台任务）目前**直接跳过这一步**：既不入队，也不在启动期重灌。
pub struct MemoryIndexer;

impl Default for MemoryIndexer {
    fn default() -> Self {
        Self
    }
}

impl MemoryIndexer {
    pub fn new() -> Self {
        Self
    }

    /// 空实现：不建立任何索引，直接返回 0（没有写入任何条目）。
    pub async fn rebuild_memory_index(&self, _save_id: &str) -> Result<usize, EngineError> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_types::{EventEnvelope, NarratePayload, PlayEvent};

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

    /// 关键词命中：原文从权威日志回填，且按 save_id 隔离。
    #[tokio::test]
    async fn retrieves_by_keyword_within_the_save() {
        let store = store_with_events().await;
        let r = FtsMemoryRetriever::new(store);
        let hits = r.retrieve("sv-1", "古堡", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1]);
        assert_eq!(hits[0].text, "月光下的古堡");
        assert!(hits[0].score > 0.0, "bm25 取负后应大于 0");
        // save_id 隔离：sv-2 只回自己的 seq1。
        let hits = r.retrieve("sv-2", "古堡", 5).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.seq).collect::<Vec<_>>(), vec![1]);
        assert_eq!(hits[0].text, "别处的古堡");
    }

    /// 保留前 K 条；空查询 / k=0 直接返回空。
    #[tokio::test]
    async fn respects_top_k_and_empty_query() {
        let store = store_with_events().await;
        let r = FtsMemoryRetriever::new(store);
        assert!(r.retrieve("sv-1", "古堡", 0).await.unwrap().is_empty());
        assert!(r.retrieve("sv-1", " 。！", 5).await.unwrap().is_empty());
        // 「的」同时命中 sv-1 的 seq1 与 seq2：k=1 截断，k=5 全给。
        assert_eq!(r.retrieve("sv-1", "的", 1).await.unwrap().len(), 1);
        assert_eq!(r.retrieve("sv-1", "的", 5).await.unwrap().len(), 2);
    }

    /// #05 §3.2/§3.3：摘要也进 FTS5，检索器**无需特判**即可召回。
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

        let r = FtsMemoryRetriever::new(store.clone());
        let hits = r.retrieve(save, "矿石", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "summary", "摘要要有可区分 kind");
        assert!(hits[0].text.contains("发光"));
        assert_eq!(hits[0].round, 3);

        let hits = r.retrieve(save, "守卫", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "scene_summary");
        assert_eq!(hits[0].round, 4);
    }
}
