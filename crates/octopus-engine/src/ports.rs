//! 端口层（#20 ②）：engine 只依赖这些 trait，实现由 ai / api 注入。

use async_trait::async_trait;
use octopus_types::{ActorRef, EncounterView, FocusEntity, Intent, IntentEnvelope, QuestView, RoundChannel};

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
    /// 本回合检索到的相关往事（#05 §3.4）：只用于主线 AI 提示词；为空则整段不注入。
    pub memories: Vec<MemoryHit>,
    /// 每回合 token 预算（0 = 不限）：用于裁剪 lore / 人物设定等可选注入。
    pub token_budget: usize,
    /// 玩家显式引用的实体（完整定义）：AI 应据此聚焦本次演绎。
    pub focus: Vec<FocusEntity>,
    /// 导演（人）已裁定的事实：AI 必须当作既定前提，不得推翻。
    pub canon: Vec<String>,
    /// #04 ⑦ 回合内续轮时回喂的「新引擎信息」：模型自己在本回合触发的
    /// query_world / check / interact 结果。
    ///
    /// 只装这些结果本身（查询答案已按关键词收窄），不重发世界全量。首轮恒为空，
    /// 所以单轮路径的提示词与 turn_feedback 加入前逐字一致。
    pub turn_feedback: Vec<String>,
    /// 当前任务（含骨架目标与导演新增），供 AI 推进与闭环。
    pub quests: Vec<QuestView>,
    /// 正在进行的结构化遭遇（导演创建）。
    pub encounters: Vec<EncounterView>,
    /// 骨架里的全部场景（含当前场景）：给 AI 合法可切的 advance_scene 目标。
    pub scenes: Vec<SceneBrief>,
    /// 故事书声明的合法判定属性 key（attribute_dimensions + world.check.attributes）：
    /// check 意图的 attribute 只能填这些，必须列进提示词，避免模型拿英文别名瞎猜。
    pub attributes: Vec<String>,
    /// 本存档指定的单一模型（覆盖全局默认）；None = 用全局默认。
    pub model: Option<ModelRef>,
    /// 故事书声明的输出协议（叙事契约 P2）；None = 引擎默认协议。
    pub protocol: Option<ProtocolSpec>,
}

/// 可热替换的 AI provider 槽：改配置后，后续回合立即用新模型，无需重启进程或重建会话。
pub type AiSlot = std::sync::Arc<std::sync::RwLock<std::sync::Arc<dyn AiProvider>>>;

/// 一次自动上下文压缩的记账（进游玩页日志，不进对话）。
///
/// 压缩只重写**派生 surface**（模型会话）；权威命令日志不动，所以玩家看到的叙事、
/// 回放与存档都不受影响。语义对齐 DSH `compaction` 的 `CompactionResult`。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompactionReport {
    /// 触发原因："pressure"（token 压力）| "context_overflow"（供应商报超限后的兜底）。
    pub trigger: String,
    /// 被摘要遮蔽掉的回合数。
    pub shadowed_rounds: u32,
    /// 遮蔽前后的字符数（摘要必须更短才算成功）。
    pub chars_before: u64,
    pub chars_after: u64,
    /// 摘要调用本身的用量（缓存命中是关键指标）。
    pub usage: octopus_types::AiCallUsage,
    pub latency_ms: u64,
}

/// 一次 AI 调用的产物：意图包络 + 可选的思考链文本（reasoning_content）。
#[derive(Debug, Clone, Default)]
pub struct AiOutput {
    /// 意图包络（#04 ⑨）：每个元素带可选的 intent_id，供回合内幂等去重。
    pub intents: Vec<IntentEnvelope>,
    /// 供应商返回的思考链；None 表示该模型 / 供应商没给。
    pub reasoning: Option<String>,
    /// 协议适配器产生的警告（如 declarative 白名单过滤掉的意图）；引擎落 System 事件。
    pub intent_warnings: Vec<String>,
    /// 本次调用的完整轨迹（pi 式 span）：请求上下文 + 用量 + 延迟 + 状态。
    /// None = 该 provider 不采集（ScriptedProvider 等确定性实现）。
    pub trace: Option<octopus_types::AiCallPayload>,
    /// 本次调用之前做过一次自动上下文压缩时的记账（None = 没压）。
    pub compaction: Option<CompactionReport>,
}

impl AiOutput {
    /// 由纯意图列表构造（每个包一层无幂等 id 的包络）：脚本化 / 测试 provider 常用。
    /// reasoning / intent_warnings 取缺省；需要时再就地覆盖字段。
    pub fn from_intents(intents: Vec<Intent>) -> Self {
        Self {
            intents: intents.into_iter().map(IntentEnvelope::from).collect(),
            reasoning: None,
            intent_warnings: Vec::new(),
            trace: None,
            compaction: None,
        }
    }
}

/// 追加式模型会话里的一条消息（持久化 / 重建用）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConvRecord {
    pub round: u32,
    /// "user" | "assistant"
    pub role: String,
    pub content: String,
}

/// 每存档一行的模型会话持久化端口：让追加式会话（缓存前缀）跨重启稳定。
///
/// 派生数据：读 / 写失败都只影响缓存命中与跨重启连续感，调用方必须静默降级，
/// 绝不因此让回合失败；权威回合与 replay 都不依赖它。
#[async_trait]
pub trait ConversationStore: Send + Sync {
    /// 读回某存档的会话；无记录返回空。
    async fn load(&self, save_id: &str) -> Result<Vec<ConvRecord>, EngineError>;
    /// 覆盖写入某存档的会话快照。
    async fn save(&self, save_id: &str, records: &[ConvRecord]) -> Result<(), EngineError>;
    /// 删除某存档的会话。
    async fn clear(&self, save_id: &str) -> Result<(), EngineError>;
}

/// AI 只产生「意图」，引擎负责结算（#03/#04）。
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// 单一 AI：负责旁白 / 场景推进 / 世界响应，并扮演所有非玩家角色。
    async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError>;

    /// 请求取消某存档**正在进行**的 AI 调用（没有在途调用时是空操作）。
    ///
    /// 实现应立即让在途请求返回 `Err(EngineError::Cancelled)`；引擎把这一回合当作
    /// 「玩家主动停止」干净收尾（System 事件 + RoundEnd，不产生任何叙事）。
    fn cancel(&self, _save_id: &str) {}

    /// 注入会话持久化端口（仅 RigProvider 会用；其它实现忽略即可）。
    fn set_conversation_store(&self, _store: std::sync::Arc<dyn ConversationStore>) {}

    /// 丢弃某存档的追加式模型会话（内存 + 持久层）。
    ///
    /// 会话被重建时调用（新原点 / 升级 / 导入 / 删除）：不清理的话，重建后的会话会把
    /// 已归档的旧对话继续当上下文发出去，缓存前缀也会包含已丢弃内容。
    async fn clear_conversation(&self, _save_id: &str) {}

    /// 把一段文本压缩成更短的摘要（#05 §3.3 场景压缩）。
    ///
    /// 默认返回 `Ok(None)` = 不做 AI 压缩，调用方退化为确定性拼接：这样既有实现与
    /// 测试 mock 无需改动即可编译，离线 / 默认行为也可复现。真实实现走便宜角色（pair）；
    /// 返回 None / 出错都由调用方回退，绝不把摘要失败传导给权威回合。
    async fn summarize(&self, _text: &str) -> Result<Option<String>, EngineError> {
        Ok(None)
    }
}

/// 演出流出口（#17）：结算即推。api 层实现为 SSE。
#[async_trait]
pub trait EventSink: Send + Sync {
    fn emit(&self, event: octopus_types::EventEnvelope);

    /// 等待此前 emit 的事件全部落库。回滚等需要读写一致性的维护操作先 await 它，
    /// 否则写队列里未落库的旧事件可能在归档之后又被写回。
    async fn flush(&self) {}
}

/// 一条被检索到的往事（#05 §3.4）：派生视图——文本来自权威日志 / 派生摘要表，命中来自派生索引。
///
/// 注入提示词前引擎不再回读状态；`text` 就是当时的叙事原文或摘要文本。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    /// 定位键（同存档内唯一，用于去重与定位原文）：叙事事件为正的命令日志 seq，
    /// 派生摘要为负数编号（见 octopus-engine::storage 的 seq 编码）。
    pub seq: i64,
    pub round: u32,
    pub kind: String,
    pub text: String,
    /// 相关性分数：FTS 命中为 -bm25（越大越相关）。
    pub score: f32,
}

/// 相关往事检索（#05 §3.4）：组合根注入；None = 不检索，行为与今天一致。
///
/// 这是**派生数据**端口：实现可以失败（FTS 索引不可用 / 派生库损坏），
/// 调用方（Session）必须静默降级，绝不因此让回合失败。
#[async_trait]
pub trait MemoryRetriever: Send + Sync {
    /// 以 `query` 召回该存档至多 `k` 条相关往事（按相关性排序，越靠前越相关）。
    async fn retrieve(
        &self,
        save_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryHit>, EngineError>;
}

/// 摘要持久化端口（#05 §3.2/§3.3）：回合微摘要与场景摘要都写派生表。
///
/// 实现负责落库 + best-effort 同步 FTS5；调用方（Session）把任何失败
/// 只当 warn——摘要是派生数据，丢了 / 失败了都不该影响权威回合与 `replay()`。
#[async_trait]
pub trait SummaryStore: Send + Sync {
    /// 写入某回合的微摘要（同存档同回合覆盖）。
    async fn put_round_summary(&self, save_id: &str, round: u32, text: &str) -> Result<(), EngineError>;
    /// 读回 `round > after_round` 的微摘要（场景压缩输入），按 round 升序。
    async fn round_summaries_after(
        &self,
        save_id: &str,
        after_round: u32,
    ) -> Result<Vec<(u32, String)>, EngineError>;
    /// 写入某场景的压缩摘要（同存档同场景覆盖）。
    async fn put_scene_summary(
        &self,
        save_id: &str,
        scene_id: &str,
        round: u32,
        text: &str,
    ) -> Result<(), EngineError>;
}
