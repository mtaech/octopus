//! Octopus 共享类型（#04 意图 / #17 演出流事件 / #06 状态投影 / #24 存档）。
//!
//! 单一事实来源：前端 `frontend/src/types/index.ts` 应由此处生成（ts-rs 待接入，
//! 见 #20 ③）。当前里程碑先用 serde 手工对齐，字段名与前端一致。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub type Seq = u64;

// ============================================================
// 演出流事件（#17 包络 + 类型表）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ActorRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStage {
    Idle,
    StoryThinking,
    Resolving,
    WaitingConfirm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SuccessLevel {
    Great,
    Success,
    Barely,
    Fail,
}

/// 判定比较模式（#12）：≥难度 / ≤目标值 / 对抗。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CheckMode {
    Gte,
    Lte,
    Opposed,
}

/// 判定种类（#3）：谁掷骰 / 是否掷骰 / 比较对象是什么。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// 主动属性检定：本人掷骰 + 属性修正 vs 难度。
    Attribute,
    /// 攻击检定：本人掷骰 + 攻击加值 vs 目标防御（AC）。
    Attack,
    /// 豁免：目标掷骰 + 豁免加值 vs 施法者 DC；失败才结算效果。
    Save,
    /// 被动值：不掷骰，以 passive_base + 修正 vs 难度。
    Passive,
}

/// 判定器声明（#12）：参数化配置，引擎不预设骰系；lua 为自定义判定脚本（归一化 total / margin）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckerDef {
    /// 骰子表达式 1d20 / 1d100 / 3d6；None = 无骰（由 AI 依属性叙事裁决）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<CheckMode>,
    /// 属性 → 固定修正映射（键为维度 key）；命中则覆盖中心偏移公式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute_modifier: Option<std::collections::BTreeMap<String, i64>>,
    /// 中心偏移公式（默认 (v - 50) / 5）。v1 仅支持默认公式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_formula: Option<String>,
    /// 成功度分档阈值（按差值），默认 [+10, 0, -10]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degree_thresholds: Option<Vec<i64>>,
    /// Lua 判定脚本源码（归一化输出 total / margin）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lua: Option<String>,
    /// 判定种类（#3）：缺省 attribute。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CheckKind>,
    /// 被动判定的基数（缺省 10；D&D 被动察觉 = 10 + 加值）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passive_base: Option<i64>,
    /// 判定属性维度 key（判定 C1）：技能未声明时用它，再缺省回落 'str'。
    /// 必须命中故事书 attribute_dimensions 的 key，否则校验报 Error（不静默 0 分）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    /// 对抗判定时对手所用的属性维度 key（判定 C1）；缺省同 attribute。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opposed_attribute: Option<String>,
    /// 兼容别名：故事书常写 type: "d20" / type: "d100" 表示骰子家族；
    /// 未显式给 dice 时据此推导（d20 → 1d20）。非骰式类型名（如 attribute）忽略。
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// 合法判定属性 key 白名单；声明后意图的 attribute 必须命中（否则驳回，不再静默 0 分）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Vec<String>>,
    /// 判定难度缺省值（意图未给 difficulty 时用它）；再缺省回落 12。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_dc: Option<i64>,
}

/// 条件表达式（#13）：结构化 JSON 表达式树 + Lua 兜底（与 #12「声明式核心 + Lua 钩子」同构）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CondExpr {
    AllOf { children: Vec<CondExpr> },
    AnyOf { children: Vec<CondExpr> },
    Not { child: Box<CondExpr> },
    TriggerFired { trigger_id: String },
    FlagSet { flag: String },
    AtLocation { location_id: String },
    AttributeGe { attribute: String, value: f64 },
    RelationshipGe {
        from: String,
        to: String,
        #[serde(rename = "type")]
        r#type: String,
        value: f64,
    },
    /// 当前场景的遭遇已清空（地图 P5 §6.5）：该场景的遭遇全部结束，或敌人全灭。
    /// 让「清剿」有自然判据，不必绕道 flag_set 让导演记得设标记。
    EncounterCleared {},
    Lua { script: String },
}

/// 技能 / 物品的资源消耗（#12 ②）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ResourceCost {
    pub resource: String,
    pub amount: i64,
}

/// 技能冷却（#01）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Cooldown {
    pub turns: i64,
}

/// 即时效果（#12 ③）。数量字段支持骰子表达式（如 2d6+3）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImmediateEffect {
    /// 伤害：作用于目标的资源（缺省 hp）。
    Damage {
        amount: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource: Option<String>,
    },
    /// 治疗：作用于目标的资源（缺省 hp）。
    Heal {
        amount: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource: Option<String>,
    },
    /// 改资源：正负由 amount 符号决定。
    ModifyResource { resource: String, amount: String },
    /// 设标记。
    SetFlag { flag: String },
}

/// 状态持续单位。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StatusUnit {
    Turns,
    Scenes,
}

/// 同名状态叠加策略。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StatusStack {
    Replace,
    Add,
    Max,
}

fn default_status_duration() -> i64 {
    1
}

fn default_status_unit() -> StatusUnit {
    StatusUnit::Turns
}

/// 持续状态声明。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StatusDef {
    pub id: String,
    pub name: String,
    /// 状态说明（可选，供编辑器与 AI 参考）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_status_duration")]
    pub duration: i64,
    #[serde(default = "default_status_unit")]
    pub unit: StatusUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<StatusStack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Vec<ImmediateEffect>>,
}

/// 条件触发（事件 + 条件 + 效果）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct EffectTrigger {
    pub id: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<CondExpr>,
    #[serde(default)]
    pub effects: Vec<ImmediateEffect>,
}

/// 静态属性修正（同名取 max）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct AttributeModifier {
    pub attribute: String,
    pub value: i64,
}

/// 效果声明（#12 ③）：声明式四层 + Lua 兜底。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct EffectDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediate: Option<Vec<ImmediateEffect>>,
    /// 施加的持续状态：引用故事书顶层「statuses」声明的 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Vec<EffectTrigger>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<Vec<AttributeModifier>>,
}

/// 技能判定：引用全局判定器（字符串）或内联声明（配置 / Lua）。
///
/// 这里 derive `TS`（而非在字段上用字符串 override）是为了让 ts-rs 正确生成
/// `string | CheckerDef` 联合类型并补上 `CheckerDef` 的 import——手写 override
/// 不会带上依赖 import，生成的 TS 会缺名。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(untagged)]
pub enum SkillCheck {
    Ref(String),
    Def(CheckerDef),
}

/// 技能 / 法术（#01）：声明式定义 + 可选 Lua 钩子。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct SkillDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cost: Vec<ResourceCost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown: Option<Cooldown>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<SkillCheck>,
    /// 判定属性维度 key（判定 C1）：声明后本技能用它掷骰（如匕首用 dex）。
    /// 优先级：技能 → 判定器 → 全局 → 'str'；必须命中 attribute_dimensions。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<EffectDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lua: Option<String>,
}

/// 规则集插件：故事书声明的挂载点脚本（「规则集走 Lua」①）。
///
/// 引擎只提供**挂载时机**与**通用原语**（掷两次取高/低 · 加值 · 改难度 · 施加即时效果 ·
/// 加减资源）；脚本里写的才是规则集自己的词汇（何时算优势、豁免怎么算、熟练给几点）。
/// 一组具名脚本 = 一个规则集，随故事书发布、随存档冻结、可被静态校验与单独禁用。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct LuaMountDef {
    /// 脚本 id（同一故事书内唯一）：per-script 存储区隔离 + 错误回溯用。
    pub id: String,
    /// 挂载时机：check_pre_roll / check_post_roll / check / pre_resolve / post_resolve /
    /// event / condition / status_tick / turn_end / scene_end
    /// （未知值发布门报错，不静默回落；protocol 走独立的 narrative.protocol 声明，不在列）。
    pub mount: String,
    /// 源文本；发布门走 lua_lint 静态预检（语法 + scoped env 白名单）。
    pub source: String,
    /// 可选：仅在条件成立时执行（复用 CondExpr）；缺省恒执行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<CondExpr>,
    /// 可选：false = 禁用该脚本（缺省启用）。禁用仍参与静态校验。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SystemLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RoundChannel {
    Character,
    Meta,
    /// 导演通道：人代替 GM 推进剧情；事件归属「故事本身」而非任何角色。
    Gm,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoundInput {
    pub channel: RoundChannel,
    pub text: String,
    /// 玩家在输入框里显式引用的故事书实体（随回合落进命令日志，供展示/重放）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<EntityRef>,
}

/// 共享状态增量（#17）：resolution 与 state_update 共用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StateDelta {
    pub domain: DeltaDomain,
    pub entity_id: String,
    pub field: String,
    pub op: DeltaOp,
    #[ts(type = "unknown")]
    pub value: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DeltaDomain {
    Character,
    Goal,
    /// 结构化遭遇（导演创建 / 更新 / 结束）。
    Encounter,
    /// 骨架进度域。旧格式写作 beat，靠 alias 升格读取。
    #[serde(alias = "beat")]
    Trigger,
    Location,
    Relationship,
    Resource,
    Flag,
    /// 全量状态检查点（#14 新原点 / 升级基座）：value = 序列化的世界状态。
    /// 唯一用途是让「日志被归档后的重放」与「故事书换版后的重放」有确定基线；
    /// 历史日志里的旧 delta 仍照常逐条应用。
    Origin,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DeltaOp {
    Set,
    Add,
    Remove,
}

/// 场景切换事件的规范名（决策 #11：引擎内部事件与演出流事件 scene 统一）。
///
/// 旧故事书把场景切换事件写作 scene_change（#13 原文），为不静默丢引用，
/// 读写两侧统一经此函数归一：声明、触发匹配、校验都认两种写法，新数据只写 scene。
pub fn normalize_event_name(name: &str) -> &str {
    match name {
        "scene_change" => "scene",
        other => other,
    }
}

/// 关系边端点 id：规范字段 from / to（#01），兼容旧字段 from_id / to_id（决策 #11）。
///
/// 关系边在故事书里是 raw JSON（没有强类型结构），读写两侧都经此取值，
/// 保证用旧字段写的历史故事书不会静默失效；新数据只写规范字段。
pub fn relationship_endpoint<'a>(
    edge: &'a Value,
    canonical: &str,
    legacy: &str,
) -> Option<&'a str> {
    edge.get(canonical)
        .or_else(|| edge.get(legacy))
        .and_then(Value::as_str)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PlayEvent {
    Scene(ScenePayload),
    Narrate(NarratePayload),
    Dialogue(DialoguePayload),
    Emote(EmotePayload),
    Pending(PendingPayload),
    CheckResult(CheckResultPayload),
    Resolution(ResolutionPayload),
    StateUpdate(StateUpdatePayload),
    Phase(PhasePayload),
    RoundStart(RoundStartPayload),
    RoundEnd(RoundEndPayload),
    System(SystemPayload),
    /// AI 思考链（reasoning_content）：给玩家作参考，不改变世界状态。
    Reasoning(ReasoningPayload),
    /// 一次 AI 调用（主线 / 角色）的完整轨迹（pi 式 span）：请求上下文 + 用量 + 延迟 + 状态。
    /// 与 reasoning 的区别：保留发给模型的**完整上下文**（system 提示词 + 会话历史），
    /// 供游玩页「日志」tab 复盘；数据量较大，只在日志视图展示。
    AiCall(AiCallPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EventEnvelope {
    pub id: String,
    pub seq: Seq,
    pub round: u32,
    pub ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActorRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    #[serde(flatten)]
    pub event: PlayEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScenePayload {
    pub scene_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub present: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NarratePayload {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_ref: Option<String>,
}

/// AI 思考链：某次 AI 调用（主线 / 角色）产出的 reasoning_content 纯文本。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReasoningPayload {
    /// 思考阶段：story_thinking / character_thinking（think 意图为 model_draft）
    pub stage: String,
    /// 完整思考链文本。
    pub text: String,
    /// 来源：provider = 供应商 reasoning_content；model = 模型写在正文里的 think 意图。
    /// serde 缺省 provider，兼容 P3 之前的既有事件数据。
    #[serde(default = "default_reasoning_source")]
    pub source: String,
}

fn default_reasoning_source() -> String {
    "provider".to_string()
}

/// 一次 AI 调用的结果状态（pi 式 span 的 status）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AiCallStatus {
    Ok,
    Error,
}

/// 一次 AI 调用的 token 用量（供应商不回传时对应字段为 0）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct AiCallUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    /// 命中缓存的输入 token（cached 是检验「缓存是否吃满」的关键指标）。
    pub cached_input_tokens: u64,
    /// 写入缓存的输入 token。
    pub cache_creation_input_tokens: u64,
}

/// 发给模型的上下文里的一条消息（日志视图用；只保留可读正文，工具调用等折叠为文本）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct AiCallMessage {
    /// system / user / assistant
    pub role: String,
    pub content: String,
}

/// 一次 AI 调用（主线 / 角色）的完整轨迹：pi 式 span 的「请求属性 → 事件 → 状态」。
///
/// - 请求属性：provider / model / temperature / max_tokens / messages（含 system 提示词与
///   会话历史的**完整原文**，供游玩页「日志」tab 复盘）。
/// - 事件：思考链（reasoning）、解析出的意图、协议警告。
/// - 结束属性：用量、延迟、状态（ok / error）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AiCallPayload {
    /// 思考阶段：story_thinking（角色 AI 接入后为 character_thinking）。
    pub stage: String,
    /// 供应商 id（config.json providers[].id）。
    pub provider: String,
    /// 实际调用的模型 id。
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u64,
    /// 发给模型的完整上下文：首条为 system 提示词，其后为会话历史 + 本次输入。
    pub messages: Vec<AiCallMessage>,
    /// 供应商返回的思考链全文（同 reasoning 事件，这里保留完整副本）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub usage: AiCallUsage,
    /// 本次补全的端到端耗时（毫秒，含网络往返）。
    pub latency_ms: u64,
    /// 该回合这一轮 AI 调用的总尝试次数（含首次）。
    /// >1 表示发生过「意图解析失败 → 回喂纠正 → 重试」（pi 式 agent 循环的 retry）。
    #[serde(default)]
    pub attempts: u32,
    pub status: AiCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 解析出的意图名摘要（narrate / speak / check …），一眼可见模型要做什么。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<String>,
    /// 协议适配器警告（白名单过滤等）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DialoguePayload {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EmotePayload {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingPayload {
    pub action_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    pub actor: ActorRef,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckResultPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    pub actor: ActorRef,
    pub attribute: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolls: Option<Vec<i64>>,
    pub r#mod: i64,
    pub total: i64,
    pub target: i64,
    pub margin: i64,
    pub result: bool,
    pub level: SuccessLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opponent: Option<ActorRef>,
    /// 判定种类（#3）：被动 / 豁免 / 攻击 / 属性。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CheckKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResolutionPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    pub status: ResolutionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggered_events: Option<Vec<String>>,
    pub state_changes: Vec<StateDelta>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Ok,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StateUpdatePayload {
    pub changes: Vec<StateDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PhasePayload {
    pub stage: PhaseStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoundStartPayload {
    pub input: RoundInput,
    /// 提交该回合时客户端携带的幂等请求 id（重启后据此去重，避免重复重放）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoundEndPayload {
    pub round: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SystemPayload {
    pub level: SystemLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub text: String,
}

// ============================================================
// 意图（#04 动作协议，v1 最小集）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IntentEnvelope {
    /// 意图幂等 id（#04 ⑨）：同一回合内重复的 id 只结算一次。
    /// 缺省 = 旧协议 / 老模型没给 id，此时保持既有行为（不去重）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    #[serde(flatten)]
    pub intent: Intent,
}

impl IntentEnvelope {
    /// 包一层无幂等 id 的意图（引擎内部与测试构造用；不改变任何既有语义）。
    pub fn new(intent: Intent) -> Self {
        Self { intent_id: None, intent }
    }
}

impl From<Intent> for IntentEnvelope {
    fn from(intent: Intent) -> Self {
        Self::new(intent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    /// 模型写在正文里的思考草稿（P3）：引擎只落一条「思考」事件，
    /// 不进叙事条目、不改世界状态；展示策略由故事书 narrative.display.draft 决定。
    Think { content: String },
    /// actor_id：说话/动作主体的人物 id（用在场角色名单里的 id）；纯旁白可省略。
    Narrate { content: String, #[serde(default)] actor_id: Option<String> },
    Speak {
        content: String,
        #[serde(default)]
        tone: Option<String>,
        #[serde(default)]
        actor_id: Option<String>,
    },
    Emote {
        content: String,
        #[serde(default)]
        emotion: Option<String>,
        #[serde(default)]
        actor_id: Option<String>,
    },
    /// 移动。`character_id` 指名「移动的是谁」（图鉴 M2 补丁）：位置链路 P2 起 NPC /
    /// 怪物也在实例表里，只有受控角色写得动已经不够用。缺省 = 意图归属的角色 → 受控角色
    /// （旧协议 / 老模型不带该字段时行为逐字不变）。
    Move {
        destination_id: String,
        #[serde(default)]
        character_id: Option<String>,
    },
    UseSkill { skill_id: String, #[serde(default)] target_id: Option<String> },
    UseItem { item_id: String, #[serde(default)] target_id: Option<String> },
    /// 与场景物件交互（#04 / #01 objects）：object_id 引用故事书 objects 的 id，
    /// action 必须匹配该物件声明的动作 key；两者都在引擎侧校验。
    Interact { object_id: String, action: String },
    Check {
        attribute: String,
        #[serde(default)]
        difficulty: Option<i64>,
        #[serde(default)]
        actor_id: Option<String>,
        /// 对抗判定的对手（判定 C3）：角色实例（模板 id / 名字 / 实例键都能命中）或
        /// 静态被动值的标识。给了对手 = 双方各掷一次比大小（对手命中不了实例时，
        /// 用它的静态被动值）；判定器声明 mode = opposed 却不给对手 → 意图被驳回。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opponent_id: Option<String>,
    },
    QueryWorld { query: String },
    /// 查询某个角色的私有资料（属性 / 资源 / 状态 / 物品 / 位置；#04 Query 类）。
    /// 缺省 `character_id` = 查询发起者自己；角色 AI 只能查到自己的数据（#16 认知边界）。
    QueryCharacter { #[serde(default)] character_id: Option<String> },
    /// 查询与某实体相关的关系边（#04 Query 类）。
    /// 角色 AI 只得到触及自己的边；缺省 `entity_id` = 自己（#04 §6）。
    QueryRelationships { #[serde(default)] entity_id: Option<String> },
    AdvanceScene { #[serde(default)] target_scene_id: Option<String>, #[serde(default)] abandon: bool },
    Intervene { content: String },
    /// 导演专属：新增一个运行时任务（来源=故事，不属于骨架）。
    Quest {
        text: String,
        #[serde(default)]
        hidden: bool,
        #[serde(default)]
        primary: bool,
    },
    /// 导演专属：调整某角色的资源（amount 可正可负）。
    Adjust {
        character_id: String,
        resource: String,
        amount: i64,
    },
    /// 攻击遭遇内的某个敌人：命中与伤害**由引擎结算**，AI 不自己编结果。
    Strike {
        enemy_id: String,
        #[serde(default)]
        skill_id: Option<String>,
    },
    /// 遭遇内的敌人攻击某个角色（图鉴 M2 §4.2）：与 strike **同一个内核**的另一个方向。
    ///
    /// 需要独立意图而非复用 strike：strike 的语义写死了「受控角色打遭遇里的敌人」，
    /// 反向复用会让「谁是攻方」含糊，也表达不了「3 只地精各打一次」。
    /// target_id 缺省 = 受控角色（玩家）；skill_id 缺省 = 图鉴条目的第一个攻击技能。
    EnemyStrike {
        enemy_id: String,
        #[serde(default)]
        target_id: Option<String>,
        #[serde(default)]
        skill_id: Option<String>,
    },
    /// 导演专属：创建一个结构化遭遇（敌方单位 + HP）。
    Encounter {
        name: String,
        #[serde(default)]
        enemies: Vec<EnemySpec>,
        #[serde(default)]
        note: Option<String>,
    },
    /// 导演专属：施加 / 移除状态。
    Status {
        character_id: String,
        status_id: String,
        #[serde(default)]
        remove: bool,
    },
    /// 本回合微摘要（#05 §3.2）：主线 AI 在同一轮顺手产出的一两句 recap。
    ///
    /// 派生数据：引擎**不产生叙事事件、不改世界状态**，只写派生表 round_summaries；
    /// 重放忽略它，删掉也能从回合重建（最坏退化为空）。提示词里属可选但鼓励。
    Summary { text: String },
    FinishTurn,
}

/// 遭遇敌方单位的输入形态（hp 缺省由引擎给一个合理初始值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnemySpec {
    pub name: String,
    #[serde(default)]
    pub hp: Option<i64>,
    /// 防御值（缺省 12：越高越难打中）
    #[serde(default)]
    pub ac: Option<i64>,
    /// 图鉴引用（图鉴 M1）：命中 storybook.characters[] 中 kind='monster' 的条目 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    /// 展开数量（默认 1）：3 只地精 = 1 条 template_id + count=3。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// 覆盖攻击技能（缺省用图鉴条目的第一个攻击技能）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
}

/// 触发点预置的遭遇（地图 P5 §6.3）：作者声明，不再靠导演 AI 即兴 spawn。
///
/// 引擎在触发点被标记 fired 之后按它自动建遭遇，走 Intent::Encounter 的**同一条创建路径**
/// （图鉴实例克隆 + 地点继承）。掷表遭遇**不复用也不新增类型**——表是内容 / 规则集语义，
/// 由 Lua 掷骰（engine_rng）→ set_flag → 触发点 condition: flag_set + 本预置表达
/// （见 docs/rules-via-lua.md §6）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EncounterPreset {
    /// 遭遇名；缺省回落触发点标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 遭遇地点；缺省继承触发时所在场景的 location_id（与 Intent::Encounter 同一口径）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    /// 引用的图鉴条目（kind = "monster"）与展开数量。
    #[serde(default)]
    pub enemies: Vec<EncounterPresetEnemy>,
}

/// 预置遭遇里的一条怪物模板引用（template_id 必须命中 kind = "monster" 的图鉴条目）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EncounterPresetEnemy {
    pub template_id: String,
    /// 展开数量（缺省 1）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// 六种驳回码（#04 校验阶段）+ 玩家取消。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    ActorNotFound,
    ActorNotControlled,
    TargetInvalid,
    InsufficientResource,
    CooldownActive,
    /// 使用物品时未持有该物品（#01 物品栏）。
    ItemNotOwned,
    RuleViolation,
    Cancelled,
}

impl RejectionCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActorNotFound => "actor_not_found",
            Self::ActorNotControlled => "actor_not_controlled",
            Self::TargetInvalid => "target_invalid",
            Self::InsufficientResource => "insufficient_resource",
            Self::CooldownActive => "cooldown_active",
            Self::ItemNotOwned => "item_not_owned",
            Self::RuleViolation => "rule_violation",
            Self::Cancelled => "cancelled",
        }
    }
}

// ============================================================
// 状态投影（#06 ① / #17 GET /state）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CharacterInstance {
    pub instance_id: String,
    pub template_id: String,
    pub name: String,
    pub kind: String,
    #[ts(type = "Record<string, unknown>")]
    pub attributes: serde_json::Map<String, Value>,
    #[ts(type = "Record<string, unknown>")]
    pub resources: serde_json::Map<String, Value>,
    /// 物品栏（#01）：item id → 数量（运行时状态）。
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub inventory: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    pub present: bool,
    pub statuses: Vec<StatusInstance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StatusInstance {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns_left: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenes_left: Option<i32>,
}

/// 遭遇敌人可用的攻击（图鉴 M2 §4.6 的数据卡摘要）：技能名 + 伤害骰。
///
/// 展示型投影：提示词据此告诉 AI「这只怪能怎么打」，引擎的缺省攻击技能也取第一条
/// （EnemySpec.skill_id 覆盖时它就是唯一一条）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnemyAttack {
    /// 技能 id（命中 storybook.skills）。
    pub skill_id: String,
    pub name: String,
    /// 伤害骰表达式（如 1d6+2）；未声明时为空串。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub damage: String,
}

/// 遭遇里的敌方单位（3a：只有名字与 HP；先攻/回合见后续）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnemyView {
    /// 遭遇内寻址 id（e1、e2…）：玩家/引擎引用某个敌人时用它。
    pub id: String,
    pub name: String,
    pub hp: i64,
    pub max: i64,
    /// 防御值：攻击判定的难度就是它（缺省 12）。
    pub ac: i64,
    /// 背后的怪物实例键（图鉴 M1）；缺省 = 临时敌人（AI 现编 / 旧日志）。
    /// 存在时 hp / ac 是该实例的投影；不存在时条目自身是权威（旧行为）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// 图鉴模板 id（图鉴 M1）：指向 storybook.characters[] 中 kind='monster' 的条目。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    /// 遭遇创建时快照的场景 id（地图 P5）；缺省 = 未记录（旧日志）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_id: Option<String>,
    /// 遭遇发生地点（地图 P5）：怪物实例继承它，地图按它归位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    /// 可用攻击（图鉴 M2 数据卡摘要）：名字 + 伤害骰，供提示词与缺省攻击技能。
    /// 缺省为空 = 临时敌人 / 旧日志（AI 现编的敌人没有图鉴攻击）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attacks: Vec<EnemyAttack>,
}

/// 结构化遭遇：导演创建 / 触发点预置（§6.3），投影给前端与提示词。
///
/// 叙事与空间锚（scene / location / goal / template_ids）都是**创建时的快照**：
/// 运行时事件必须可重放，查询「此刻的场景」在重放时会得到错误答案（§6.2）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EncounterView {
    pub id: String,
    pub name: String,
    pub enemies: Vec<EnemyView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub active: bool,
    /// 创建时的场景（地图 P5 §6.4）：地图与提示词据此说明「这一战发生在哪一幕」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_id: Option<String>,
    /// 创建时的地点（缺省继承场景）；怪物实例也按它归位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    /// 关联目标 id（可选）：创建时按所在场景推导后快照，任务面板据它显示「清剿中」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    /// 涉及的图鉴模板 id（去重、按创建顺序）：临时敌人不计入。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_ids: Vec<String>,
}

/// 运行时任务（骨架目标 + 导演新增），投影给前端与提示词。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuestView {
    pub id: String,
    pub text: String,
    pub done: bool,
    /// skeleton = 故事书骨架；gm = 导演运行时新增
    pub source: String,
    pub hidden: bool,
    pub primary: bool,
    /// 所属场景的地点（地图 P5 §6.2）：投影时从场景**推导**，不落库、不存字段。
    /// 导演运行时新增的任务不属于任何场景 → None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkeletonProgress {
    #[ts(type = "Record<string, unknown>")]
    pub goals: serde_json::Map<String, Value>,
    /// 已触发的剧情触发点（trigger id → 是否已触发）
    #[ts(type = "Record<string, unknown>")]
    pub triggers: serde_json::Map<String, Value>,
    pub abandoned_scenes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectionMeta {
    pub save_id: String,
    pub save_title: String,
    pub storybook_title: String,
    pub revision: u32,
    pub needs_upgrade: bool,
    pub auto_confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorldProjection {
    pub seq: Seq,
    pub scene_id: String,
    pub scene_title: String,
    #[ts(type = "Record<string, unknown>")]
    pub characters: serde_json::Map<String, Value>,
    pub controlled: Vec<String>,
    #[ts(type = "Record<string, unknown>")]
    pub flags: serde_json::Map<String, Value>,
    pub progress: SkeletonProgress,
    /// 运行时任务（骨架目标里带 text 的条目 + 导演新增），供 UI 与提示词使用。
    pub quests: Vec<QuestView>,
    /// 结构化遭遇（导演创建）。
    pub encounters: Vec<EncounterView>,
    #[ts(type = "Array<unknown>")]
    pub locations: Vec<Value>,
    pub meta: ProjectionMeta,
}

// ============================================================
// 存档（#24 / #21）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct SaveListItem {
    pub id: String,
    pub title: String,
    pub storybook_id: String,
    pub storybook_title: String,
    pub embedded_revision: u32,
    pub latest_revision: u32,
    pub needs_upgrade: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_sandbox: Option<bool>,
    pub created_at: String,
    pub updated_at: String,
    pub last_played_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct SaveDetail {
    #[serde(flatten)]
    pub item: SaveListItem,
    /// 内嵌冻结故事书（#14）；当前里程碑以 JSON 直存。
    #[ts(type = "unknown")]
    pub storybook: Value,
    /// 存档遗留区（#14）：版次升级时被删除、但玩家选择「遗留冻结」的旧定义。
    /// 空时不序列化，旧存档包 / 旧客户端不受影响。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy: Vec<LegacyDefinition>,
}

/// 遗留区条目（#14 / CONTEXT.md「遗留区」）：旧故事书里已被删除的实体定义，
/// 只读保留在存档内，让运行时实例（角色 / 物品引用）在升级后仍可解释。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct LegacyDefinition {
    /// 实体种类：character / skill / item / faction / location / status
    pub kind: String,
    /// 实体 id（编辑器纪律：一经发布 id 不可变，所以 id 足以定位）
    pub id: String,
    pub name: String,
    /// 原定义本体（故事书内该实体的完整 JSON）
    #[ts(type = "unknown")]
    pub definition: Value,
    /// 冻结时的内嵌版次（可追溯来源）
    pub frozen_at_revision: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HistoryPage {
    pub events: Vec<EventEnvelope>,
    #[serde(rename = "hasMore", alias = "has_more")]
    pub has_more: bool,
}

/// 存档级叙述段玩家偏好：叙述段 id → 开关(bool) 或变体 key(string)。
///
/// 只对故事书中 `playerEditable: true` 的段生效（引擎侧校验，防止伪造偏好篡改非可编辑段）；
/// 只影响之后的回合，绝不回写命令日志 / 历史（见 docs/narrative-contract.md §4.3）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(untagged)]
pub enum NarrativeOverride {
    Enabled(bool),
    Variant(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SaveSettings {
    pub auto_confirm: bool,
    /// 本存档使用的模型；None = 用全局角色默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 本存档的思考强度（reasoning_effort）；None = 用角色 / 供应商默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// 叙述段玩家偏好（section id → 开关 | 变体 key）；None = 全部用故事书默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<std::collections::BTreeMap<String, NarrativeOverride>>,
}

/// 故事书实体的结构化引用（#23 ④ 精准指向）：creator 在结对里显式指定「要改这个」。
/// 单例 meta / world 无 id；声明区（flag/event/relationship_type/target_type）id = key；
/// 嵌套 scene / goal / trigger 带 parent_id。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct EntityRef {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub name: String,
}

/// 被引用实体的**完整定义**（前端解析后随请求带上，供 AI 提示注入）。
/// 结对与游玩共用：结对随消息、游玩随回合，都只做本轮请求的临时上下文。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct FocusEntity {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[ts(type = "unknown")]
    pub entity: Value,
}

/// 编辑器 C 范式的一条结对会话线程（一本故事书可有多条，按主题隔离上下文）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct PairThreadRecord {
    pub id: String,
    pub storybook_id: String,
    pub title: String,
    pub message_count: i64,
    pub created_at: String,
    pub updated_at: String,
    /// 该会话尚未处理的「待审查改动」（前端形状的 JSON 数组，后端只负责存取）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub pending_suggestions: Option<Value>,
}

/// 编辑器 C 范式结对会话的一条消息（#23 ④）。建议「待审查」不入此表——它是临时态，
/// 采纳后才经编辑操作落入草稿，避免刷新后重复采纳。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct PairMessageRecord {
    pub seq: i64,
    pub role: String,
    pub content: String,
    /// 仅 assistant：思考流（reasoning_content）正文（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub tools: Option<Value>,
    /// 该消息显式引用的故事书实体（用户消息）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<EntityRef>>,
    /// 用户消息的附件（JSON 数组：name + 文本正文）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub attachments: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct MaintenanceRow {
    pub at: String,
    pub op: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateSaveRequest {
    pub storybook_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub controlled_character_id: Option<String>,
    #[serde(default)]
    pub is_sandbox: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlaytestRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub controlled_character_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SubmitRoundRequest {
    pub channel: RoundChannel,
    pub text: String,
    #[serde(default)]
    pub request_id: Option<String>,
    /// 引用的实体（轻量，随回合落库）。
    #[serde(default)]
    pub refs: Option<Vec<EntityRef>>,
    /// 引用的实体（完整定义，仅本轮请求透传给 AI，不落库）。
    #[serde(default)]
    pub focus: Option<Vec<FocusEntity>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfirmRequest {
    pub action_id: String,
    pub decision: ConfirmDecision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmDecision {
    Confirm,
    Cancel,
}

// ============================================================
// 统一错误信封（#23）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "Record<string, unknown> | null")]
    pub detail: Option<Value>,
}

// ============================================================
// 账户与登录（多账户；系统侧「账号」，区别于领域里的「玩家」）
// ============================================================

/// 账户的公开投影：任何响应里都不出现口令哈希。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    /// 仍在使用初始口令（默认管理员）；前端据此常驻提醒改密。
    pub must_change_password: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RegisterRequest {
    pub username: String,
    /// 留空则回落为用户名。
    #[serde(default)]
    #[ts(optional)]
    pub display_name: Option<String>,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginResponse {
    /// 不透明登录令牌：前端以 `Authorization: Bearer <token>` 回传。
    pub token: String,
    pub expires_at: String,
    pub account: Account,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

// ---- 管理后台（只有 is_admin 账户可达；见 docs/auth-accounts.md §9） ----

/// 管理后台概览：一眼看清这台实例有多大、有多少人。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct AdminOverview {
    // 计数在 JSON 里就是普通数字。ts-rs 默认把 u64 映射成 bigint，
    // 而前端要拿它们做算术 / 格式化，所以显式纠正成 number（与真实载荷一致）。
    #[ts(type = "number")]
    pub users: u64,
    #[ts(type = "number")]
    pub admins: u64,
    #[ts(type = "number")]
    pub saves: u64,
    #[ts(type = "number")]
    pub storybooks: u64,
    /// 未过期的登录态条数（≈ 当前有登录的设备数）。
    #[ts(type = "number")]
    pub active_sessions: u64,
    /// 数据库文件路径（内存库为 ":memory:"）。
    pub db_path: String,
    #[ts(type = "number")]
    pub db_bytes: u64,
    pub version: String,
}

/// 管理后台：账户行（含内容统计）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct AdminUserRow {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub must_change_password: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
    #[ts(type = "number")]
    pub saves: u64,
    #[ts(type = "number")]
    pub storybooks: u64,
    #[ts(type = "number")]
    pub active_sessions: u64,
    /// 就是当前请求者自己：前端据此禁用「删除」（后端也会拒）。
    pub is_self: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AdminCreateUserRequest {
    pub username: String,
    pub password: String,
    /// 留空则回落为用户名。
    #[serde(default)]
    #[ts(optional)]
    pub display_name: Option<String>,
    /// 直接建成管理员（默认普通账户）。
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AdminUpdateUserRequest {
    /// 省略 = 不改；空串 = 回落为用户名。
    #[serde(default)]
    #[ts(optional)]
    pub display_name: Option<String>,
    /// 省略 = 不改。
    #[serde(default)]
    #[ts(optional)]
    pub is_admin: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AdminResetPasswordRequest {
    pub new_password: String,
}

// ============================================================
// 校验与故事书 API（#01 / #23）
// ============================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ValidateResult {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateStorybookRequest {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SaveDraftRequest {
    #[ts(type = "unknown")]
    pub draft: Value,
    pub base_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishRequest {
    pub base_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StorybookDocument {
    pub id: String,
    pub revision: u32,
    pub draft_version: u32,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    pub published: bool,
    #[ts(type = "unknown")]
    pub draft: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub released: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StorybookListItem {
    pub id: String,
    pub title: String,
    pub revision: u32,
    pub draft_version: u32,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    pub published: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 故事书封面（#28）：取自「已发布版次优先、否则草稿」的 meta.cover
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub cover: Option<Value>,
    /// 内容评级（P3，meta.rating）：仅列表徽标 / 筛选用，引擎与提示词都不读
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<String>,
}

// ============================================================
// 通用自包含存档包（#27 / 跨数据库导出与导入）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct CommandRecord {
    pub seq: i64,
    pub round: i64,
    pub kind: String,
    #[ts(type = "unknown")]
    pub payload: Value,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ArchivedCommandRecord {
    pub origin_seq: i64,
    pub seq: i64,
    pub round: i64,
    pub kind: String,
    #[ts(type = "unknown")]
    pub payload: Value,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct SavePackage {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub save: SaveDetail,
    pub commands: Vec<CommandRecord>,
    pub archived_commands: Vec<ArchivedCommandRecord>,
    pub maintenance: Vec<MaintenanceRow>,
}

// ============================================================
// 通用自包含故事书包（导出 / 导入）
// ============================================================

/// 故事书包里的故事书记录：草稿全文 + 已发布版次快照。
/// 与 `StorybookDocument` 的区别：这是**可移植的交付物**——导入方按它重建一条本地记录，
/// 因此不带本地并发令牌（`draft_version` 由导入方从 1 起算）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StorybookExportRecord {
    pub id: String,
    pub title: String,
    pub revision: u32,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    pub published: bool,
    #[ts(type = "unknown")]
    pub draft: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub released: Option<Value>,
}

/// 故事书包的格式标识（清单 `format` 字段；不匹配即拒收）
pub const STORYBOOK_PACKAGE_FORMAT: &str = "octopus-storybook-package";

/// 自包含故事书包：zip 容器 `.octopus-book.zip`，内含 `storybook.json`（本清单）+ `assets/`。
/// 与存档包同构（zip + 清单 + 内容寻址资产），但按故事书粒度交付：不含命令日志与存档状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StorybookPackage {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub storybook: StorybookExportRecord,
}

// ============================================================
// 存档版次迁移（#14 ②）：dry-run 报告 → 逐项裁决 → 执行
// ============================================================

/// 人物消失时的处置方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeDisposition {
    /// 遗留冻结：旧定义挪入存档遗留区，实例继续可用。
    Freeze,
    /// 叙事离场：实例标记已离场，由主线 AI 在后续叙事中交代。
    Departure,
}

/// dry-run 报告里一条自动处理的结构变更（技能 / 物品 / 势力 / 地点消失或定义改变）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct UpgradeChange {
    /// 实体种类：skill / item / faction / location / status / character（定义变化）
    pub kind: String,
    pub id: String,
    pub label: String,
    /// 引擎将采取的动作（人类可读；人物裁决不属于此类）
    pub action: String,
}

/// dry-run 报告里一个「新版故事书已删除」的人物：必须逐项裁决后才能升级。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct UpgradeGoneCharacter {
    pub character_id: String,
    pub name: String,
    /// 消失原因的提示（供 UI 展示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct UpgradeReportGroup {
    /// 自动处理项（参考信息）
    pub changes: Vec<UpgradeChange>,
    /// 需逐项裁决的人物消失
    pub gone_characters: Vec<UpgradeGoneCharacter>,
}

/// 两段式升级的第一段产物：纯读、无副作用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct UpgradeReport {
    pub from_revision: u32,
    pub to_revision: u32,
    pub groups: UpgradeReportGroup,
}

/// 执行升级时的一项人物裁决。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct DispositionItem {
    pub character_id: String,
    pub disposition: UpgradeDisposition,
}

/// 执行升级请求体（缺省空数组 = 无人物消失；有人物消失时由引擎严格校验必须齐全）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpgradeRequest {
    #[serde(default)]
    pub dispositions: Vec<DispositionItem>,
}

/// 执行升级结果：升级后的存档 + 自动备份包的磁盘文件名。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpgradeResult {
    pub detail: SaveDetail,
    /// 自动备份文件名（同数据目录 backups/）；幂等无操作时为空串。
    pub backup_name: String,
}

/// 新原点结果（#14 ④ / 决策 3）：旧日志已移入库内归档表。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NewOriginResult {
    pub ok: bool,
    /// 移入归档表的命令条数
    pub archived_count: u64,
    /// 新原点的 seq（检查点所在序号；无事件时为 0）
    pub origin_seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 图鉴 M1 / 地图 P1 的字段扩展必须纯加性：旧命令日志（无新字段）反序列化后
    /// 新字段全为 None，再序列化回去**逐字不变**（skip_serializing_if 生效）。
    #[test]
    fn enemy_view_old_log_shape_roundtrips_unchanged() {
        let old = json!({ "id": "e1", "name": "灰狼", "hp": 7, "max": 11, "ac": 12 });
        let view: EnemyView = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(view.instance_id, None);
        assert_eq!(view.template_id, None);
        assert_eq!(view.scene_id, None);
        assert_eq!(view.location_id, None);
        assert!(view.attacks.is_empty(), "旧数据没有攻击摘要");
        assert_eq!(serde_json::to_value(&view).unwrap(), old);
    }

    #[test]
    fn enemy_view_new_fields_roundtrip() {
        let view = EnemyView {
            id: "e1".into(),
            name: "地精".into(),
            hp: 7,
            max: 7,
            ac: 15,
            instance_id: Some("enc-x:goblin#1".into()),
            template_id: Some("char-goblin".into()),
            scene_id: Some("sc-cave".into()),
            location_id: Some("loc-cave".into()),
            attacks: vec![EnemyAttack {
                skill_id: "sk-goblin-scimitar".into(),
                name: "弯刀".into(),
                damage: "1d6+2".into(),
            }],
        };
        let back: EnemyView = serde_json::from_value(serde_json::to_value(&view).unwrap()).unwrap();
        assert_eq!(back, view);
        assert_eq!(back.attacks[0].skill_id, "sk-goblin-scimitar");
    }

    #[test]
    fn enemy_spec_old_intent_shape_roundtrips_unchanged() {
        let old = json!({ "name": "灰狼", "hp": 11, "ac": 12 });
        let spec: EnemySpec = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(spec.template_id, None);
        assert_eq!(spec.count, None);
        assert_eq!(spec.skill_id, None);
        assert_eq!(serde_json::to_value(&spec).unwrap(), old);

        // 最旧形态：只有 name。hp / ac 是既有字段（本来就序列化成 null）；
        // 本次新增的图鉴字段必须完全不出现。
        let oldest = json!({ "name": "灰狼" });
        let spec: EnemySpec = serde_json::from_value(oldest).unwrap();
        let out = serde_json::to_value(&spec).unwrap();
        assert!(out.get("template_id").is_none(), "新增字段不得出现在旧数据里");
        assert!(out.get("count").is_none(), "新增字段不得出现在旧数据里");
        assert!(out.get("skill_id").is_none(), "新增字段不得出现在旧数据里");
    }

    /// 图鉴 M2 补丁：Intent::Move 新增 character_id（可选），旧协议日志逐字往返。
    #[test]
    fn move_intent_old_shape_roundtrips_unchanged() {
        let old = json!({ "type": "move", "destination_id": "loc-tavern" });
        let intent: Intent = serde_json::from_value(old).unwrap();
        match &intent {
            Intent::Move { destination_id, character_id } => {
                assert_eq!(destination_id, "loc-tavern");
                assert_eq!(*character_id, None);
            }
            other => panic!("应解析为 move：{other:?}"),
        }
        // 新字段是**可选加性**：旧日志（不带它）照常解析；新写入多一个 null，
        // 与 actor_id / tone 等既有可选字段同口径（不 skip_serializing_if）。
        let out = serde_json::to_value(&intent).unwrap();
        assert_eq!(out.get("type").and_then(Value::as_str), Some("move"));
        assert_eq!(out.get("destination_id").and_then(Value::as_str), Some("loc-tavern"));
        assert!(out.get("character_id").is_some_and(Value::is_null));
    }

    /// 图鉴 M2：enemy_strike 意图（可选字段）往返，并只带 type 标记。
    #[test]
    fn enemy_strike_intent_roundtrips() {
        let value = json!({ "type": "enemy_strike", "enemy_id": "e1" });
        let intent: Intent = serde_json::from_value(value).unwrap();
        match &intent {
            Intent::EnemyStrike { enemy_id, target_id, skill_id } => {
                assert_eq!(enemy_id, "e1");
                assert_eq!(*target_id, None);
                assert_eq!(*skill_id, None);
            }
            other => panic!("应解析为 enemy_strike：{other:?}"),
        }
    }

    /// 判定 C3：Intent::Check 的 opponent_id 是纯加性可选字段——旧的 check 意图
    /// （不带对手）解析后再序列化逐字不变；带对手时能被读出来。
    #[test]
    fn check_intent_opponent_is_additive() {
        let old = json!({
            "type": "check", "attribute": "dex", "difficulty": 12, "actor_id": null
        });
        let intent: Intent = serde_json::from_value(old.clone()).unwrap();
        match &intent {
            Intent::Check { opponent_id, .. } => assert_eq!(*opponent_id, None),
            other => panic!("应解析为 check：{other:?}"),
        }
        assert_eq!(serde_json::to_value(&intent).unwrap(), old, "没给对手时不得多出字段");

        let with = json!({ "type": "check", "attribute": "dex", "opponent_id": "char-goblin" });
        let intent: Intent = serde_json::from_value(with).unwrap();
        match &intent {
            Intent::Check { opponent_id, .. } => {
                assert_eq!(opponent_id.as_deref(), Some("char-goblin"))
            }
            other => panic!("应解析为 check：{other:?}"),
        }
    }

    /// 判定 C1：旧判定器 / 旧技能日志不带 attribute，序列化结果不变。
    #[test]
    fn checker_and_skill_old_shape_roundtrips_unchanged() {
        let checker_json = json!({ "dice": "1d20", "kind": "attack" });
        let checker: CheckerDef = serde_json::from_value(checker_json.clone()).unwrap();
        assert_eq!(checker.attribute, None);
        assert_eq!(checker.opposed_attribute, None);
        assert_eq!(serde_json::to_value(&checker).unwrap(), checker_json);

        let skill_json = json!({ "id": "sk-bite", "name": "啃咬" });
        let skill: SkillDef = serde_json::from_value(skill_json).unwrap();
        assert_eq!(skill.attribute, None);
        let out = serde_json::to_value(&skill).unwrap();
        assert!(out.get("attribute").is_none(), "新增字段不得出现在旧数据里");
    }

    /// 遭遇整体（旧日志里 EncounterView 内嵌 enemies）也必须逐字往返。
    #[test]
    fn encounter_view_old_log_shape_roundtrips_unchanged() {
        // 旧日志里 note 为 None 时本就被 skip（既有行为），故旧形态不含 note。
        let old = json!({
            "id": "enc-1",
            "name": "狼群合围",
            "enemies": [{ "id": "e1", "name": "灰狼", "hp": 7, "max": 11, "ac": 12 }],
            "active": true
        });
        let view: EncounterView = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(view.enemies[0].instance_id, None);
        assert_eq!(serde_json::to_value(&view).unwrap(), old);
        // 地图 P5 的叙事锚同样是纯加性：旧日志解析后全为 None / 空，往返逐字不变。
        assert_eq!(view.scene_id, None);
        assert_eq!(view.location_id, None);
        assert_eq!(view.goal_id, None);
        assert!(view.template_ids.is_empty());
    }

    /// 地图 P5 §6.4：遭遇的叙事 / 空间锚（创建时快照）往返。
    #[test]
    fn encounter_view_anchor_fields_roundtrip() {
        let view = EncounterView {
            id: "enc-1".into(),
            name: "游荡的地精".into(),
            enemies: vec![],
            note: Some("触发点预置".into()),
            active: true,
            scene_id: Some("sc-1".into()),
            location_id: Some("loc-cave".into()),
            goal_id: Some("g-clear".into()),
            template_ids: vec!["mon-goblin".into(), "mon-wolf".into()],
        };
        let back: EncounterView = serde_json::from_value(serde_json::to_value(&view).unwrap()).unwrap();
        assert_eq!(back, view);
    }

    /// 地图 P5 §6.2：QuestView 的地点是从场景推导的加性字段；旧投影逐字往返。
    #[test]
    fn quest_view_location_is_additive() {
        let old = json!({
            "id": "g1", "text": "打倒地精", "done": false,
            "source": "skeleton", "hidden": false, "primary": true
        });
        let quest: QuestView = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(quest.location_id, None);
        assert_eq!(serde_json::to_value(&quest).unwrap(), old);

        let with_loc = QuestView { location_id: Some("loc-cave".into()), ..quest };
        let back: QuestView =
            serde_json::from_value(serde_json::to_value(&with_loc).unwrap()).unwrap();
        assert_eq!(back.location_id.as_deref(), Some("loc-cave"));
    }

    /// 地图 P5 §6.5：encounter_cleared 是零参数 op，展开形态就是 { "op": "encounter_cleared" }。
    #[test]
    fn encounter_cleared_cond_roundtrips() {
        let raw = json!({ "op": "encounter_cleared" });
        let cond: CondExpr = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(cond, CondExpr::EncounterCleared {});
        assert_eq!(serde_json::to_value(&cond).unwrap(), raw);
    }

    /// 地图 P5 §6.3：触发点预置遭遇的解析（旧的「没有 encounter」形状不受影响）。
    #[test]
    fn encounter_preset_parses_optional_fields() {
        let raw = json!({
            "name": "游荡怪物",
            "enemies": [{ "template_id": "mon-goblin", "count": 2 }, { "template_id": "mon-wolf" }]
        });
        let preset: EncounterPreset = serde_json::from_value(raw).unwrap();
        assert_eq!(preset.name.as_deref(), Some("游荡怪物"));
        assert_eq!(preset.note, None);
        assert_eq!(preset.location_id, None);
        assert_eq!(preset.enemies.len(), 2);
        assert_eq!(preset.enemies[0].count, Some(2));
        assert_eq!(preset.enemies[1].count, None);
    }
}
