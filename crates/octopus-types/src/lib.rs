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
    CharacterThinking,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<EffectDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lua: Option<String>,
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DeltaOp {
    Set,
    Add,
    Remove,
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
    pub intent_id: String,
    #[serde(flatten)]
    pub intent: Intent,
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
    Move { destination_id: String },
    UseSkill { skill_id: String, #[serde(default)] target_id: Option<String> },
    UseItem { item_id: String, #[serde(default)] target_id: Option<String> },
    Check {
        attribute: String,
        #[serde(default)]
        difficulty: Option<i64>,
        #[serde(default)]
        actor_id: Option<String>,
    },
    QueryWorld { query: String },
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

/// 遭遇里的敌方单位（3a：只有名字与 HP；先攻/回合见后续）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnemyView {
    /// 遭遇内寻址 id（e1、e2…）：玩家/引擎引用某个敌人时用它。
    pub id: String,
    pub name: String,
    pub hp: i64,
    pub max: i64,
    /// 防御值：攻击判定的难度就是它（缺省 12）。
    pub ac: i64,
}

/// 结构化遭遇：导演创建，投影给前端与提示词。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EncounterView {
    pub id: String,
    pub name: String,
    pub enemies: Vec<EnemyView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub active: bool,
}

/// 运行时任务（骨架目标 + 导演新增），投影给前端与提示词。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuestView {
    pub id: String,
    pub text: String,
    pub done: bool,
    /// skeleton = 故事书骨架；gm = 导演运行时新增
    pub source: String,
    pub hidden: bool,
    pub primary: bool,
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
