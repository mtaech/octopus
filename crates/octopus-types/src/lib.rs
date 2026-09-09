//! Octopus 共享类型（#04 意图 / #17 演出流事件 / #06 状态投影 / #24 存档）。
//!
//! 单一事实来源：前端 `frontend/src/types/index.ts` 应由此处生成（ts-rs 待接入，
//! 见 #20 ③）。当前里程碑先用 serde 手工对齐，字段名与前端一致。

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Seq = u64;

// ============================================================
// 演出流事件（#17 包络 + 类型表）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStage {
    Idle,
    StoryThinking,
    CharacterThinking,
    Resolving,
    WaitingConfirm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuccessLevel {
    Great,
    Success,
    Barely,
    Fail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SystemLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoundChannel {
    Character,
    Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundInput {
    pub channel: RoundChannel,
    pub text: String,
}

/// 共享状态增量（#17）：resolution 与 state_update 共用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub domain: DeltaDomain,
    pub entity_id: String,
    pub field: String,
    pub op: DeltaOp,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeltaDomain {
    Character,
    Goal,
    Beat,
    Location,
    Relationship,
    Resource,
    Flag,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeltaOp {
    Set,
    Add,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenePayload {
    pub scene_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub present: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarratePayload {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialoguePayload {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotePayload {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Ok,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateUpdatePayload {
    pub changes: Vec<StateDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhasePayload {
    pub stage: PhaseStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundStartPayload {
    pub input: RoundInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundEndPayload {
    pub round: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPayload {
    pub level: SystemLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub text: String,
}

// ============================================================
// 意图（#04 动作协议，v1 最小集）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentEnvelope {
    pub intent_id: String,
    #[serde(flatten)]
    pub intent: Intent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    Narrate { content: String },
    Speak { content: String, #[serde(default)] tone: Option<String> },
    Emote { content: String, #[serde(default)] emotion: Option<String> },
    Move { destination_id: String },
    UseSkill { skill_id: String, #[serde(default)] target_id: Option<String> },
    UseItem { item_id: String, #[serde(default)] target_id: Option<String> },
    Check { attribute: String, #[serde(default)] difficulty: Option<i64> },
    QueryWorld { query: String },
    AdvanceScene { #[serde(default)] target_scene_id: Option<String>, #[serde(default)] abandon: bool },
    Intervene { content: String },
    FinishTurn,
}

/// 六种驳回码（#04 校验阶段）+ 玩家取消。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    ActorNotFound,
    ActorNotControlled,
    TargetInvalid,
    InsufficientResource,
    CooldownActive,
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
            Self::RuleViolation => "rule_violation",
            Self::Cancelled => "cancelled",
        }
    }
}

// ============================================================
// 状态投影（#06 ① / #17 GET /state）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterInstance {
    pub instance_id: String,
    pub template_id: String,
    pub name: String,
    pub kind: String,
    pub attributes: serde_json::Map<String, Value>,
    pub resources: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    pub present: bool,
    pub statuses: Vec<StatusInstance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInstance {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns_left: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenes_left: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkeletonProgress {
    pub goals: serde_json::Map<String, Value>,
    pub beats: serde_json::Map<String, Value>,
    pub abandoned_scenes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionMeta {
    pub save_id: String,
    pub save_title: String,
    pub storybook_title: String,
    pub revision: u32,
    pub needs_upgrade: bool,
    pub auto_confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldProjection {
    pub seq: Seq,
    pub scene_id: String,
    pub scene_title: String,
    pub characters: serde_json::Map<String, Value>,
    pub controlled: Vec<String>,
    pub flags: serde_json::Map<String, Value>,
    pub progress: SkeletonProgress,
    pub locations: Vec<Value>,
    pub meta: ProjectionMeta,
}

// ============================================================
// 存档（#24 / #21）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub created_at: String,
    pub updated_at: String,
    pub last_played_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveDetail {
    #[serde(flatten)]
    pub item: SaveListItem,
    /// 内嵌冻结故事书（#14）；当前里程碑以 JSON 直存。
    pub storybook: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    pub events: Vec<EventEnvelope>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveSettings {
    pub auto_confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceRow {
    pub at: String,
    pub op: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSaveRequest {
    pub storybook_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub controlled_character_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitRoundRequest {
    pub channel: RoundChannel,
    pub text: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmRequest {
    pub action_id: String,
    pub decision: ConfirmDecision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmDecision {
    Confirm,
    Cancel,
}

// ============================================================
// 统一错误信封（#23）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}
