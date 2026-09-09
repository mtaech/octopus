//! 端口层（#20 ②）：engine 只依赖这些 trait，实现由 ai / api 注入。

use async_trait::async_trait;
use octopus_types::{ActorRef, Intent, RoundChannel};

use crate::error::EngineError;

#[derive(Debug, Clone)]
pub struct TurnContext {
    pub save_id: String,
    pub round: u32,
    pub scene_title: String,
    pub controlled: String,
    pub player_text: String,
    pub channel: RoundChannel,
    pub characters: Vec<ActorRef>,
}

/// AI 只产生「意图」，引擎负责结算（#03/#04）。
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// 主线 AI：旁白 / 场景推进 / 世界响应。
    async fn story_intents(&self, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError>;
    /// 角色 AI：相关角色言行（引擎可并行调用多个）。
    async fn character_intents(&self, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError>;
}

/// 演出流出口（#17）：结算即推。api 层实现为 SSE。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: octopus_types::EventEnvelope);
}

/// 向量化（#15/#27）：默认本地 bge-small-zh-v1.5，512 维。
pub trait EmbeddingBackend: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError>;
    fn dimension(&self) -> usize;
}
