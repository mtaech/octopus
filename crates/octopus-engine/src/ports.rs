//! 端口层（#20 ②）：engine 只依赖这些 trait，实现由 ai / api 注入。

use async_trait::async_trait;
use octopus_types::{ActorRef, EncounterView, FocusEntity, Intent, QuestView, RoundChannel};

use crate::error::EngineError;
use crate::protocol::ProtocolSpec;

/// 可推进场景的摘要：主线 AI 用它挑 `advance_scene {target_scene_id}` 的合法目标。
#[derive(Debug, Clone)]
pub struct SceneBrief {
    pub id: String,
    pub title: String,
    pub chapter: String,
}

/// 本回合要用的模型（供应商 + 模型 id）。None = 用角色默认。
#[derive(Debug, Clone)]
pub struct ModelRef {
    pub provider_id: String,
    pub model: String,
    /// 思考强度（reasoning_effort）；None = 用角色 / 供应商默认。
    pub reasoning_effort: Option<String>,
}

/// 人物设定：从故事书人物模板抽出、注入提示词的「人格档案」。
///
/// 只含发给 AI 的字段；人物模板上的「作者注释（notes）」是给创作者的，
/// 永不进入这里（见 CONTEXT「人物」）。
#[derive(Debug, Clone, PartialEq)]
pub struct PersonaView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub background: String,
    pub personality: String,
    pub appearance: String,
    /// 对话示例：AI 模仿语气与句式的 few-shot 样板。
    pub example_dialogues: String,
}

/// 叙述段：故事书声明的「怎么讲」（文风 / 行为约束 / 收尾），按槽位注入提示词。
///
/// 与「世界词条 (Lore)」分工：Lore 讲「世界有什么」（事实），叙述段讲「怎么讲」（指令）。
#[derive(Debug, Clone, PartialEq)]
pub struct NarrativeView {
    pub id: String,
    /// 槽位：world | style | behavior | closing（引擎固定顺序）。
    pub slot: String,
    /// 作用范围：story | character | both | character:<模板id>。
    pub scope: String,
    pub text: String,
}

/// 世界词条（关键词触发注入）：命中触发词才把内容交给 AI，省上下文。
///
/// 与「剧情触发点 (Trigger)」不同：Trigger 由引擎求值并提示 AI，可改进度；
/// Lore 只是条件性的上下文注入，不改世界状态。
#[derive(Debug, Clone, PartialEq)]
pub struct LoreView {
    pub id: String,
    pub title: String,
    pub content: String,
    pub priority: i64,
}

impl PersonaView {
    /// 是否存在任何值得注入的人格内容（全空则不必占用上下文）。
    pub fn has_content(&self) -> bool {
        !self.background.trim().is_empty()
            || !self.personality.trim().is_empty()
            || !self.appearance.trim().is_empty()
            || !self.example_dialogues.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct TurnContext {
    pub save_id: String,
    pub round: u32,
    pub scene_id: String,
    pub scene_title: String,
    /// 当前场景描述（骨架 scenes[].description）：给 AI 场景基调，避免它自由发挥到别处。
    pub scene_description: Option<String>,
    pub controlled: String,
    pub player_text: String,
    pub channel: RoundChannel,
    pub characters: Vec<ActorRef>,
    /// 在场人物的人格档案（按需注入；不含作者注释）。
    pub personas: Vec<PersonaView>,
    /// 世界前提（storybook.world.premise）：每回合随世界槽位注入。
    pub premise: Option<String>,
    /// 故事书声明的叙述段（已按 enabled 过滤；scope 由提示词按渠道再筛）。
    pub narrative: Vec<NarrativeView>,
    /// 本回合命中的世界词条（关键词触发注入）。
    pub lore: Vec<LoreView>,
    /// 每回合 token 预算（0 = 不限）：用于裁剪 lore / 人物设定等可选注入。
    pub token_budget: usize,
    /// 主线 AI 本回合已叙述的文本（旁白/台词/神态）：只给角色 AI，用于接着演、避免重描同一场景。
    pub story_narration: Option<String>,
    /// 玩家显式引用的实体（完整定义）：AI 应据此聚焦本次演绎。
    pub focus: Vec<FocusEntity>,
    /// 导演（人）已裁定的事实：AI 必须当作既定前提，不得推翻。
    pub canon: Vec<String>,
    /// 当前任务（含骨架目标与导演新增），供 AI 推进与闭环。
    pub quests: Vec<QuestView>,
    /// 正在进行的结构化遭遇（导演创建）。
    pub encounters: Vec<EncounterView>,
    /// 骨架里的全部场景（含当前场景）：给 AI 合法可切的 advance_scene 目标。
    pub scenes: Vec<SceneBrief>,
    /// 本存档指定的模型（覆盖角色默认）。
    pub model: Option<ModelRef>,
    /// 故事书声明的输出协议（叙事契约 P2）；None = 引擎默认协议。
    pub protocol: Option<ProtocolSpec>,
}

/// 可热替换的 AI provider 槽：改配置后，后续回合立即用新模型，无需重启进程或重建会话。
pub type AiSlot = std::sync::Arc<std::sync::RwLock<std::sync::Arc<dyn AiProvider>>>;

/// 一次 AI 调用的产物：意图 + 可选的思考链文本（reasoning_content）。
#[derive(Debug, Clone, Default)]
pub struct AiOutput {
    pub intents: Vec<Intent>,
    /// 供应商返回的思考链；None 表示该模型 / 供应商没给。
    pub reasoning: Option<String>,
    /// 协议适配器产生的警告（如 declarative 白名单过滤掉的意图）；引擎落 System 事件。
    pub intent_warnings: Vec<String>,
}

/// AI 只产生「意图」，引擎负责结算（#03/#04）。
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// 主线 AI：旁白 / 场景推进 / 世界响应。
    async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError>;
    /// 角色 AI：相关角色言行（引擎可并行调用多个）。
    async fn character_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError>;
}

/// 演出流出口（#17）：结算即推。api 层实现为 SSE。
#[async_trait]
pub trait EventSink: Send + Sync {
    fn emit(&self, event: octopus_types::EventEnvelope);

    /// 等待此前 emit 的事件全部落库。回滚等需要读写一致性的维护操作先 await 它，
    /// 否则写队列里未落库的旧事件可能在归档之后又被写回。
    async fn flush(&self) {}
}

/// 向量化（#15/#27）：默认本地 bge-small-zh-v1.5，512 维。
/// `embed` 为异步，以支持 rig 等网络 provider。
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError>;
    fn dimension(&self) -> usize;
}
