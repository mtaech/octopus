//! octopus-engine：状态权威 / 命令管线 / 结算 / 事件发射 / 存储（#06/#17/#20/#27）。
//!
//! 依赖纪律（#20 ②）：`state`、`rng` 为叶子；`command`/`session` 为编排；
//! `storage` 为适配层；外部 IO 与 LLM 一律走 `ports`。

pub mod assets;
pub mod command;
pub mod conditions;
pub mod derived;
pub mod effects;
pub mod entities;
pub mod error;
pub mod lua_host;
pub mod lua_lint;
pub mod modifiers;
pub mod ports;
pub mod protocol;
pub mod recovery;
pub mod resolve;
pub mod rng;
pub mod seed;
pub mod session;
pub mod state;
pub mod storage;
pub mod upcast;
pub mod validate;

pub use assets::{
    collect_asset_refs, content_type_of, pack_bundle, unpack_bundle, AssetStore, StoredAsset,
};
pub use derived::{check_formula, eval_formula};
pub use conditions::{
    eval_cond, evaluate_skeleton, goal_delta, trigger_delta, EvalContext,
};
pub use command::{
    execute_declarative_skill, execute_item_skill, execute_skill, insufficient_cost,
    insufficient_item, CommandContext, CommandOutcome,
};
pub use effects::{
    build_status, build_status_instance, build_status_ref, eval_amount, resolve_effect,
    resolve_immediate, status_delta, EffectResolution, DEFAULT_VITAL_RESOURCE,
};
pub use error::EngineError;
pub use lua_lint::{lint_script, lint_storybook, new_lint_state, LuaIssue};
pub use lua_host::{
    LuaCheckOutcome, LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest, MountedScript,
    SandboxLimits,
};
pub use modifiers::{attribute_modifiers, AttrModifier};
pub use ports::{AiOutput, AiProvider, AiSlot, EmbeddingBackend, EventSink, LoreView, ModelRef, NarrativeView, PersonaView, SceneBrief, TurnContext};
pub use protocol::{
    build_protocol_adapter, check_protocol_conformance, intent_kind, is_known_intent,
    parse_intents, protocol_sandbox_limits, DeclarativeProtocol, DefaultProtocol, LuaProtocol,
    ProtocolAdapter, ProtocolMode, ProtocolRole, ProtocolSpec, CHARACTER_PREAMBLE, KNOWN_INTENTS,
    NARRATIVE_INTENTS, STORY_PREAMBLE,
};
pub use recovery::{plan_rest, rest_deltas, RecoveryTrigger, RestKind};
pub use resolve::{
    degree_thresholds, level_for_margin, modifier_for, resolve_checker, resolve_declarative_check,
    resolve_lua_check, roll_dice, DiceRoll, ModifierProfile, ResolvedCheck,
    DEFAULT_DEGREE_THRESHOLDS,
};
pub use rng::DeterministicRng;
pub use session::{RewindPlan, Session};
pub use state::WorldState;
pub use storage::{
    NewPairMessage, PairMessageRow, PairThreadRow, PersistedEvent, SqliteStore, StorybookRow,
};
pub use upcast::{ensure_skeleton_ids, upcast_event, upcast_storybook, STORYBOOK_SCHEMA_VERSION};
pub use validate::{validate_storybook, validate_storybook_result};
