//! 会话（#03 核心循环 / #17 事件发射 / #24 回合并发与确认门）。
//!
//! 一个存档一个 Session：内存权威状态 + 确定性 RNG + 演出流出口 + AI 端口。
//! 回合串行（#24 ④）：非 idle 提交返回 `RoundInProgress`。

use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

use octopus_types::{
    ActorRef, CheckKind, CheckMode, CheckResultPayload, CheckerDef, CondExpr, ConfirmDecision, DeltaDomain, DeltaOp,
    DialoguePayload,
    EmotePayload, EventEnvelope, FocusEntity, HistoryPage, Intent, NarrativeOverride, NarratePayload, PendingPayload, PhasePayload,
    normalize_event_name,
    relationship_endpoint,
    PhaseStage, PlayEvent, RejectionCode, ResolutionPayload, ResolutionStatus, RoundChannel,
    CharacterInstance, EncounterView, EnemyAttack, EnemyView, QuestView, RoundEndPayload, RoundInput,
    RoundStartPayload, ScenePayload, Seq,
    EnemySpec, SkillDef, StateDelta, StateUpdatePayload, StatusUnit,
    EffectDef, EffectTrigger, ImmediateEffect, SkillCheck, StatusDef, StatusInstance, SystemLevel, SystemPayload,
    AiCallPayload, AiCallStatus, AiCallUsage, ReasoningPayload,
    WorldProjection,
};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

use crate::{
    command::{execute_item_skill, execute_skill, CommandContext, CommandOutcome},
    derived::compute_derived,
    conditions::{
        chapter_locations, eval_cond, evaluate_skeleton_full, goal_delta, goal_id_of,
        scene_encounter_goal_id, trigger_encounter_preset, trigger_progress_delta,
        ChapterLocations, EvalContext, TriggerEncounterPlan,
    },
    effects::{build_status_instance, resolve_effect, resolve_immediate, status_delta},
    error::EngineError,
    lua_host::{
        LuaCheckContext, LuaEventContext, LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest,
        LuaStatusContext, MountEnv, SandboxLimits,
    },
    modifiers::AttrModifier,
    ports::{
        AiSlot, EventSink, LoreView, MemoryHit, MemoryRetriever, ModelRef, NarrativeView, PersonaView,
        SceneBrief, SummaryStore, TurnContext,
    },
    protocol::ProtocolSpec,
    resolve::{level_for_margin, ModifierProfile, ResolvedCheck, DEFAULT_DEGREE_THRESHOLDS},
    rng::DeterministicRng,
    state::WorldState,
    storage::{now_iso, PersistedEvent},
};

struct Pending {
    action_id: String,
    tx: oneshot::Sender<ConfirmDecision>,
}

/// 全量状态检查点（#14）：世界状态 + 场景压缩窗口左界。
///
/// 场景压缩左界是派生态（不进 WorldState），但删掉它会让新原点后的场景摘要
/// 从错误窗口重压；随检查点一起携带即可让重放继续可复现。
#[derive(serde::Serialize, serde::Deserialize)]
struct OriginCheckpoint {
    state: WorldState,
    #[serde(default)]
    scene_start_round: u32,
}

/// 回合内 intent_id 幂等登记（#04 ⑨）：非空且首次出现返回 true 可结算；
/// 重复 id 返回 false 跳过；缺省 / 空 id 恒 true，保持旧模型行为。
fn register_intent_id(seen: &mut std::collections::HashSet<String>, id: &Option<String>) -> bool {
    match id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => seen.insert(id.to_string()),
        None => true,
    }
}

/// 把 map（serde_json Map 或 BTreeMap）渲染成紧凑的 `k=v` 文本（只读查询答案用）。
fn compact_map<'a, I: IntoIterator<Item = (&'a String, &'a Value)>>(entries: I) -> String {
    entries
        .into_iter()
        .map(|(k, v)| format!("{k}={}", compact_value(v)))
        .collect::<Vec<_>>()
        .join("，")
}

fn compact_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// 单回合最多注入多少个人物的人格档案（防止上下文被设定撑爆）。
const PERSONA_LIMIT: usize = 6;

/// 一条 EnemySpec 最多展开多少只（图鉴 M2）：护栏，防止模型写 count: 100000 撑爆实例表。
const MAX_ENCOUNTER_UNITS: u32 = 20;

/// 单回合最多注入多少条「相关往事」（#05 §3.4 的 K=5）。
const MEMORY_TOP_K: usize = 5;

/// 无归属旁白认人时只看开头多少字（主语位窗口）。
/// 旁白常被模型用来写角色动作（「露西靠在他身侧…」），要在主语位认出来；
/// 但环境描写里顺带提及的名字（「露西家的灯还亮着」）不该被当成行动者。
const NARRATE_ACTOR_HEAD_CHARS: usize = 6;

/// 一个玩家回合内主线 AI 最多调用几次（#04 ⑦：首轮 + 最多 2 轮续写 = 3）。
/// 达到上限即收束，防止「查询—再查询」无限循环；不产出引擎可见信息的模型
/// 第一轮后自然停止，行为与单轮路径一致。
const MAX_STORY_AI_ROUNDS: usize = 3;

/// token 预算 → lore 注入的字符预算（0 = 不限）。中文约 1.5–2 字符/token，取 2 保守估计。
fn lore_budget_chars(token_budget: usize) -> usize {
    if token_budget == 0 {
        0
    } else {
        token_budget.saturating_mul(2)
    }
}

/// 叙述段变体的 key（去空白后非空才算有效）。
fn narrative_variant_key(variant: &Value) -> Option<&str> {
    variant
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 叙述段 `when` 条件求值：缺省无条件视为命中；形状非法 / 求值出错一律不注入。
///
/// 形状问题在发布校验（validate.rs）已拦成 Error；运行期再遇异常时宁可少注入一段，
/// 也不让整回合失败，或注入一个不确定的内容（确定性契约：同一输入必得同一装配）。
fn narrative_when_matches(section: &Value, ctx: &EvalContext<'_>) -> bool {
    match section.get("when") {
        None | Some(Value::Null) => true,
        Some(v) => match serde_json::from_value::<CondExpr>(v.clone()) {
            Ok(cond) => eval_cond(&cond, ctx).unwrap_or(false),
            Err(_) => false,
        },
    }
}

/// 叙述段文本解析：变体组取「存档选择 → defaultVariant → 首个变体」，否则取 `text`。
fn resolve_narrative_text(section: &Value, ov: Option<&NarrativeOverride>) -> String {
    if let Some(list) = section
        .get("variants")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    {
        let default_key = section
            .get("defaultVariant")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let chosen = match ov {
            Some(NarrativeOverride::Variant(k)) => {
                list.iter().find(|x| narrative_variant_key(x) == Some(k.as_str()))
            }
            _ => None,
        }
        .or_else(|| {
            default_key
                .and_then(|d| list.iter().find(|x| narrative_variant_key(x) == Some(d)))
        })
        .or_else(|| list.first());
        return chosen
            .and_then(|x| x.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
    section.get("text").and_then(Value::as_str).unwrap_or("").to_string()
}

/// 回合结束后自动清 busy（无 finally 语义，用 Drop 保证）。
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 会话规则集：从内嵌冻结故事书解析出的技能定义与全局判定器。
#[derive(Debug, Clone)]
pub struct SessionRules {
    storybook: Value,
    skills: std::collections::HashMap<String, SkillDef>,
    /// 顶层声明的持续状态定义（storybook.statuses），技能效果按 id 引用。
    status_defs: std::collections::HashMap<String, StatusDef>,
    /// 各属性维度的修正配置（基线 / 范围 / 步长）。
    profiles: std::collections::HashMap<String, ModifierProfile>,
    /// 角色模板 → 属性修正（来自挂接定义与已装备物品）。
    attribute_bonuses: std::collections::HashMap<String, std::collections::HashMap<String, AttrModifier>>,
    /// 资源边界（#12 ③）：资源 id → (min, max)。只含**声明过** \`default_max\` / \`min\` 的资源，
    /// 未声明的资源不参与夹取（对旧故事书零影响）。构造时算一次，热路径不解析 JSON。
    resource_bounds: std::collections::HashMap<String, (Option<i64>, Option<i64>)>,
    /// 时序声明（#GAP-I，解析自 `world.turn`）：None = 未声明 → 时序整体关闭（旧行为逐字不变）。
    turn: Option<TurnRules>,
}

/// 时序声明（#GAP-I）。刻意**不进 octopus-types**：与 `world.resources` 同口径——
/// 引擎从原始 JSON 读它、发布门校验它，但它不是引擎对外承诺的数据契约。
#[derive(Debug, Clone, Default)]
pub struct TurnRules {
    /// 顺序来源：none（只做预算）/ initiative（掷骰）/ fixed（查表）。
    pub order: TurnOrderKind,
    /// 先攻骰式（缺省 1d20）。
    pub initiative_dice: String,
    /// 先攻加值取哪个属性维度（走既有 modifier 口径）。
    pub initiative_attribute: Option<String>,
    /// 模板 id → 固定先攻值（order = fixed，或个别单位特例）。
    pub initiative_fixed: std::collections::HashMap<String, i64>,
    /// 声明的预算（id, 每回合额度），保持声明顺序。
    pub budgets: Vec<(String, i64)>,
    /// true = 每轮重置全员预算；false（缺省）= 轮到自己时重置自己的。
    pub reset_per_round: bool,
    /// 机械意图类型 → 缺省消耗的预算 id（作者显式声明；缺省 = 不扣）。
    pub intent_budget: std::collections::HashMap<String, String>,
}

/// 顺序来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnOrderKind {
    /// 不定顺序（只按玩家回合做预算限制）。
    #[default]
    None,
    /// 掷先攻排序。
    Initiative,
    /// 查 `initiative.fixed` 表排序（不掷骰、不消耗 RNG）。
    Fixed,
}

impl TurnRules {
    /// 是否真的启用时序（声明了顺序来源或预算）。整块声明但两者皆空 = 等于没声明。
    pub fn enabled(&self) -> bool {
        self.order != TurnOrderKind::None || !self.budgets.is_empty()
    }
}

/// 解析 `world.turn`；缺省 / 非对象 → None（时序整体关闭）。
///
/// 形状非法的条目在这里**静默丢弃**（发布门 validate 负责报错）：运行期不 panic，
/// 也不因为一条坏声明就让整个回合失败。
fn turn_rules_of(storybook: &Value) -> Option<TurnRules> {
    let t = storybook.pointer("/world/turn")?;
    if !t.is_object() {
        return None;
    }
    let order = match t.get("order").and_then(Value::as_str).map(str::trim) {
        Some("initiative") => TurnOrderKind::Initiative,
        Some("fixed") => TurnOrderKind::Fixed,
        _ => TurnOrderKind::None,
    };
    let init = t.get("initiative");
    let initiative_dice = init
        .and_then(|i| i.get("dice"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("1d20")
        .to_string();
    let initiative_attribute = init
        .and_then(|i| i.get("attribute"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut initiative_fixed = std::collections::HashMap::new();
    if let Some(map) = init.and_then(|i| i.get("fixed")).and_then(Value::as_object) {
        for (k, v) in map {
            if let Some(n) = v.as_i64() {
                initiative_fixed.insert(k.clone(), n);
            }
        }
    }
    let mut budgets: Vec<(String, i64)> = Vec::new();
    if let Some(arr) = t.get("budgets").and_then(Value::as_array) {
        for b in arr {
            let Some(id) = b
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let Some(amount) = b.get("amount").and_then(Value::as_i64) else { continue };
            if amount <= 0 || budgets.iter().any(|(x, _)| x == id) {
                continue;
            }
            budgets.push((id.to_string(), amount));
        }
    }
    let reset_per_round =
        t.get("reset").and_then(Value::as_str).map(str::trim) == Some("per_round");
    let mut intent_budget = std::collections::HashMap::new();
    if let Some(map) = t.get("intent_budget").and_then(Value::as_object) {
        for (k, v) in map {
            if let Some(id) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                intent_budget.insert(k.clone(), id.to_string());
            }
        }
    }
    Some(TurnRules {
        order,
        initiative_dice,
        initiative_attribute,
        initiative_fixed,
        budgets,
        reset_per_round,
        intent_budget,
    })
}

/// 一条时序 delta（#GAP-I）：`DeltaDomain::Turn` + 约定字段名（见该枚举的文档）。
fn turn_delta(field: &str, op: DeltaOp, value: Value) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Turn,
        entity_id: String::new(),
        field: field.to_string(),
        op,
        value,
    }
}

/// 机械意图的类型名：时序闸门只约束这些（会发生规则结算 / 改变世界状态）。
///
/// 叙事意图（narrate / speak / emote / think）**任何回合都能发**——否则「失去回合」
/// 会连台词都说不出来，那不是规则语义，是 bug。导演类意图（quest / encounter / adjust…）
/// 也不受约束：它们是人替 GM 推进剧情，不属于某个角色的行动经济。
fn mechanical_intent_kind(intent: &Intent) -> Option<&'static str> {
    match intent {
        Intent::Check { .. } => Some("check"),
        Intent::Move { .. } => Some("move"),
        Intent::UseSkill { .. } => Some("use_skill"),
        Intent::UseItem { .. } => Some("use_item"),
        Intent::Interact { .. } => Some("interact"),
        Intent::Strike { .. } => Some("strike"),
        Intent::EnemyStrike { .. } => Some("enemy_strike"),
        _ => None,
    }
}

/// 解析故事书声明的资源边界（\`world.resources[].default_max\` / \`min\`）。
fn resource_bounds_of(
    storybook: &Value,
) -> std::collections::HashMap<String, (Option<i64>, Option<i64>)> {
    let mut out = std::collections::HashMap::new();
    let Some(arr) = storybook.pointer("/world/resources").and_then(Value::as_array) else {
        return out;
    };
    for def in arr {
        let Some(id) = def.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            continue;
        };
        let max = def.get("default_max").and_then(Value::as_i64);
        let min = def.get("min").and_then(Value::as_i64);
        if max.is_none() && min.is_none() {
            continue;
        }
        out.insert(id.to_string(), (min, max));
    }
    out
}

/// Lua 只读开放内容快照（GAP-C）：只截取故事书里「人物模板的挂接」与「definitions」两段原文。
///
/// 引擎**不理解**这些数据的语义（不认识 kind，也不解释 fields / modifiers / mechanics）；
/// 它只负责把作者写下的开放内容交给脚本，由规则包自己解释。
fn lua_read_data(storybook: &Value) -> Value {
    let section = |key: &str| storybook.get(key).cloned().unwrap_or(Value::Array(Vec::new()));
    serde_json::json!({
        "characters": section("characters"),
        "definitions": section("definitions"),
    })
}

/// 遭遇里的敌人是否**全灭**（通用判据，不预设任何规则集语义）。
fn enemies_all_down(enemies: &[Value]) -> bool {
    !enemies.is_empty()
        && enemies
            .iter()
            .all(|e| e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0)
}

/// 遭遇的叙事 / 空间锚（创建时快照）→ 事件上下文里的 encounter 事实。
fn encounter_facts(enc: &Value, enc_id: &str) -> Value {
    serde_json::json!({
        "id": enc.get("id").cloned().unwrap_or_else(|| Value::from(enc_id)),
        "name": enc.get("name").cloned().unwrap_or(Value::Null),
        "scene_id": enc.get("scene_id").cloned().unwrap_or(Value::Null),
        "location_id": enc.get("location_id").cloned().unwrap_or(Value::Null),
    })
}

impl SessionRules {
    pub fn from_storybook(storybook: Value) -> Self {
        let mut skills = std::collections::HashMap::new();
        if let Some(arr) = storybook.get("skills").and_then(Value::as_array) {
            for s in arr {
                if let Ok(def) = serde_json::from_value::<SkillDef>(s.clone()) {
                    skills.insert(def.id.clone(), def);
                }
            }
        }
        let mut status_defs = std::collections::HashMap::new();
        if let Some(arr) = storybook.get("statuses").and_then(Value::as_array) {
            for s in arr {
                if let Ok(def) = serde_json::from_value::<StatusDef>(s.clone()) {
                    status_defs.insert(def.id.clone(), def);
                }
            }
        }
        let mut profiles = std::collections::HashMap::new();
        if let Some(arr) = storybook.get("attribute_dimensions").and_then(Value::as_array) {
            for d in arr {
                if let Some(k) = d.get("key").and_then(|v| v.as_str()) {
                    profiles.insert(k.to_string(), ModifierProfile::for_storybook(&storybook, k));
                }
            }
        }
        let attribute_bonuses = crate::modifiers::attribute_modifiers(&storybook);
        let resource_bounds = resource_bounds_of(&storybook);
        let turn = turn_rules_of(&storybook);
        Self { storybook, skills, status_defs, profiles, attribute_bonuses, resource_bounds, turn }
    }

    /// 角色模板 → 属性修正（挂接定义 + 已装备物品）。
    pub fn attribute_bonuses(
        &self,
    ) -> &std::collections::HashMap<String, std::collections::HashMap<String, AttrModifier>> {
        &self.attribute_bonuses
    }

    /// 属性维度 → 修正配置（供判定中心偏移使用）。
    pub fn profiles(&self) -> &std::collections::HashMap<String, ModifierProfile> {
        &self.profiles
    }

    /// 资源边界（资源 id → (min, max)）：只含声明过边界的资源。
    pub fn resource_bounds(
        &self,
    ) -> &std::collections::HashMap<String, (Option<i64>, Option<i64>)> {
        &self.resource_bounds
    }

    /// 时序声明（`world.turn`）；None = 未声明。
    pub fn turn(&self) -> Option<&TurnRules> {
        self.turn.as_ref()
    }

    pub fn skill(&self, id: &str) -> Option<&SkillDef> {
        self.skills.get(id)
    }

    /// 故事书 derived[] 的 (key, formula) 声明，顺序即求值顺序（后者可引用前者）。
    ///
    /// 空 key / 空公式的条目直接丢弃：它们是发布门该拦的坏声明，运行期不猜。
    pub fn derived_defs(&self) -> Vec<(String, String)> {
        self.storybook
            .get("derived")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| {
                        let key = d.get("key").and_then(Value::as_str)?.trim().to_string();
                        let formula = d.get("formula").and_then(Value::as_str)?.trim().to_string();
                        (!key.is_empty() && !formula.is_empty()).then_some((key, formula))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 图鉴模板（图鉴 M2）：storybook.characters[] 里 kind = "monster" 的条目。
    ///
    /// 命中不了（id 不存在 / 那条不是怪物）返回 None——调用方回落到「临时敌人」路径，
    /// 旧故事书与 AI 现编的敌人行为逐字不变。
    pub fn monster_template(&self, id: &str) -> Option<Value> {
        self.storybook
            .get("characters")
            .and_then(Value::as_array)?
            .iter()
            .find(|c| {
                c.get("id").and_then(Value::as_str) == Some(id)
                    && c.get("kind").and_then(Value::as_str) == Some("monster")
            })
            .cloned()
    }

    pub fn global_checker(&self) -> Option<CheckerDef> {
        self.storybook
            .pointer("/world/check")
            .and_then(|v| serde_json::from_value::<CheckerDef>(v.clone()).ok())
    }

    /// 故事书人物声明的属性值（判定 C3：对抗的静态被动值用）。
    ///
    /// 先按 id 命中，再按 name；返回（显示名，属性值）。查不到返回 None——调用方按
    /// 「无修正」处理，绝不猜一个值出来。
    pub fn character_attribute(&self, id: &str, attribute: &str) -> Option<(String, f64)> {
        let chars = self.storybook.get("characters").and_then(Value::as_array)?;
        let hit = chars
            .iter()
            .find(|c| c.get("id").and_then(Value::as_str) == Some(id))
            .or_else(|| {
                chars
                    .iter()
                    .find(|c| c.get("name").and_then(Value::as_str) == Some(id))
            })?;
        let raw = hit.get("attributes").and_then(|a| a.get(attribute))?;
        let value = raw.as_f64().or_else(|| raw.as_i64().map(|i| i as f64))?;
        let name = hit.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
        Some((name, value))
    }

    pub fn skeleton(&self) -> Option<&Value> {
        self.storybook.get("skeleton")
    }

    /// 关键词触发的世界词条：命中 `haystack` 的条目按 priority 降序返回。
    ///
    /// - `constant: true` 的条目永远注入（核心世界规则）。
    /// - `recursive: true` 的命中条目，其内容继续参与扫描，形成连锁命中（最多 3 轮）。
    /// - `max_chars > 0` 时按内容长度截断：装不下的条目整条跳过（不半截注入），
    ///   但最高优先级的一条总会注入，避免预算过小导致什么都不给。
    pub fn match_lore(&self, haystack: &str, max_chars: usize) -> Vec<LoreView> {
        let Some(arr) = self.storybook.get("lore").and_then(Value::as_array) else {
            return Vec::new();
        };
        let enabled = |e: &Value| e.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let mut scan = haystack.to_lowercase();
        let mut matched: Vec<&Value> = Vec::new();
        let mut matched_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 常驻条目
        for e in arr.iter().filter(|e| enabled(e)) {
            if e.get("constant").and_then(Value::as_bool).unwrap_or(false) {
                if let Some(id) = e.get("id").and_then(Value::as_str) {
                    if matched_ids.insert(id.to_string()) {
                        matched.push(e);
                    }
                }
            }
        }
        // 关键词触发 + 递归扫描
        for _ in 0..3 {
            let mut added = false;
            for e in arr.iter().filter(|e| enabled(e)) {
                let id = match e.get("id").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if matched_ids.contains(&id) {
                    continue;
                }
                let keys: Vec<String> = e
                    .get("keys")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(|s| s.to_lowercase())
                            .filter(|s| !s.trim().is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                if keys.is_empty() {
                    continue;
                }
                if keys.iter().any(|k| scan.contains(k)) {
                    if e.get("recursive").and_then(Value::as_bool).unwrap_or(false) {
                        if let Some(c) = e.get("content").and_then(Value::as_str) {
                            scan.push_str(&c.to_lowercase());
                        }
                    }
                    matched_ids.insert(id);
                    matched.push(e);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        // priority 降序；sort_by 稳定，同优先级保持故事书声明顺序
        matched.sort_by(|a, b| {
            let pa = a.get("priority").and_then(Value::as_i64).unwrap_or(0);
            let pb = b.get("priority").and_then(Value::as_i64).unwrap_or(0);
            pb.cmp(&pa)
        });
        let mut out = Vec::new();
        let mut used = 0usize;
        for e in matched {
            let content = e.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if content.is_empty() {
                continue;
            }
            let len = content.chars().count();
            if max_chars > 0 && used + len > max_chars && !out.is_empty() {
                continue;
            }
            used += len;
            out.push(LoreView {
                id: e.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                title: e.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
                content,
                priority: e.get("priority").and_then(Value::as_i64).unwrap_or(0),
            });
            if max_chars > 0 && used >= max_chars {
                break;
            }
        }
        out
    }

    /// 故事书声明的输出协议（storybook.narrative.protocol）；None = 引擎默认协议。
    ///
    /// 纯解析：非法 mode 回落 Default，运行期不 panic；发布校验负责拦截非法声明。
    pub fn protocol_spec(&self) -> Option<ProtocolSpec> {
        ProtocolSpec::from_storybook(&self.storybook)
    }

    /// 世界前提（storybook.world.premise）：非空才注入。
    pub fn premise(&self) -> Option<String> {
        self.storybook
            .pointer("/world/premise")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// 故事书的叙述段（storybook.narrative.sections）逐回合解析为最终注入文本。
    ///
    /// 纯函数：输入 = 冻结故事书 + 当回合世界状态（`ctx`）+ 存档偏好（`overrides`）。
    /// 顺序：
    /// 1. `enabled: false` 的段直接丢弃；
    /// 2. 带 `when` 的段按当前世界状态求值（复用骨架条件系统），不成立即丢弃；
    /// 3. 玩家偏好只对 `playerEditable: true` 的段生效：`false` 关闭、变体 key 选择；
    ///    非可编辑段的偏好一律忽略（防止伪造设置篡改故事书声明的内容）；
    /// 4. 变体组解析出唯一文本：存档选择 → `defaultVariant` → 首个变体；无变体组用 `text`。
    ///
    /// scope 归一为 story | character | both | character:<模板id>，由提示词按渠道再筛。
    /// 本函数不写任何状态 / 日志——偏好只影响之后的回合。
    pub fn narrative(
        &self,
        ctx: &EvalContext<'_>,
        overrides: &std::collections::BTreeMap<String, NarrativeOverride>,
    ) -> Vec<NarrativeView> {
        let Some(arr) = self.storybook.pointer("/narrative/sections").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for s in arr {
            if s.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let id = s.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let player_editable = s.get("playerEditable").and_then(Value::as_bool).unwrap_or(false);
            let ov = if player_editable { overrides.get(&id) } else { None };
            if matches!(ov, Some(NarrativeOverride::Enabled(false))) {
                continue;
            }
            if !narrative_when_matches(s, ctx) {
                continue;
            }
            let text = resolve_narrative_text(s, ov);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let slot = s.get("slot").and_then(Value::as_str).unwrap_or("style").trim().to_lowercase();
            let scope = match s.get("scope") {
                Some(Value::String(v)) => v.trim().to_string(),
                Some(Value::Object(o)) => o
                    .get("characterId")
                    .and_then(Value::as_str)
                    .map(|id| format!("character:{id}"))
                    .unwrap_or_else(|| "character".to_string()),
                _ => "both".to_string(),
            };
            out.push(NarrativeView { id, slot, scope, text: text.to_string() });
        }
        out
    }

    /// 按模板 id 抽取人物的人格档案（注入提示词）。
    ///
    /// 只取发给 AI 的字段：background / personality / appearance / example_dialogues。
    /// 刻意不读 notes——它是给创作者的备注，永不交给 AI。
    /// 全空的人物跳过（不占上下文）；`limit` 之外的也跳过。
    pub fn personas(&self, ids: &[String], limit: usize) -> Vec<PersonaView> {
        let Some(arr) = self.storybook.get("characters").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for id in ids {
            if out.len() >= limit {
                break;
            }
            let Some(c) = arr
                .iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(id.as_str()))
            else {
                continue;
            };
            // 图鉴 M2 §4.6：怪物没有对话示例，不参与扮演，也不该占人格预算。
            if c.get("kind").and_then(Value::as_str) == Some("monster") {
                continue;
            }
            let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let p = PersonaView {
                id: id.clone(),
                name: s("name"),
                kind: s("kind"),
                background: s("background"),
                personality: s("personality"),
                appearance: s("appearance"),
                example_dialogues: s("example_dialogues"),
            };
            if p.has_content() {
                out.push(p);
            }
        }
        out
    }

    pub fn relationships(&self) -> Vec<Value> {
        self.storybook
            .get("relationships")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// 全部持续状态定义（故事书顶层 statuses；技能 effect.status 只存 id 引用）。
    pub fn status_defs(&self) -> &std::collections::HashMap<String, StatusDef> {
        &self.status_defs
    }

    /// 全部技能声明的效果触发点（#12 ③：event + condition + effects）。
    pub fn effect_triggers(&self) -> Vec<(String, EffectTrigger)> {
        let mut out = Vec::new();
        for skill in self.skills.values() {
            if let Some(effect) = &skill.effect {
                if let Some(triggers) = &effect.triggers {
                    for trigger in triggers {
                        out.push((skill.id.clone(), trigger.clone()));
                    }
                }
            }
        }
        out
    }

    /// 物品引用的技能 id（#01 物品技能）。
    pub fn item_skill_ids(&self, item_id: &str) -> Vec<String> {
        self.storybook
            .get("items")
            .and_then(Value::as_array)
            .and_then(|arr| arr.iter().find(|i| i.get("id").and_then(Value::as_str) == Some(item_id)))
            .and_then(|i| i.get("skills").and_then(Value::as_array))
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
}

/// 「重跑本轮」的回滚计划：最后一个玩家回合的起点与输入。
#[derive(Debug, Clone)]
pub struct RewindPlan {
    /// 该回合的回合号（重跑会复用它）。
    pub round: u32,
    /// 该回合 round_start 事件的 seq：保留 seq < from_seq 的历史。
    pub from_seq: Seq,
    /// 该回合的原始输入（文本 / 渠道 / 引用），重跑直接复用。
    pub input: RoundInput,
}

/// 全量快照基座（#06 ② 启动缓存）：以 seq 处的世界状态替代「从零重放」。
///
/// 快照永远是派生缓存——它必须与「全量重放同一日志」逐位等价；载入失败或过期一律回退
/// 全量重放。rng_position 随 state 一起携带，保证骰序也接得上。
#[derive(Debug, Clone)]
pub struct SnapshotBase {
    pub seq: Seq,
    pub state: WorldState,
    pub scene_start_round: u32,
}

/// 挂载点 `when` 条件求值的只读世界快照（owned 版本，供 EvalContext 借用）。
struct MountWorld {
    flags: std::collections::BTreeMap<String, Value>,
    goals: serde_json::Map<String, Value>,
    triggers: serde_json::Map<String, Value>,
    actor_location: Option<String>,
    actor_attributes: Option<serde_json::Map<String, Value>>,
    relationships: Vec<Value>,
    /// 当前场景与遭遇快照（地图 P5）：encounter_cleared 作为挂载点 when 的判据。
    scene_id: Option<String>,
    encounters: Vec<Value>,
}

pub struct Session {
    pub save_id: String,
    state: Mutex<WorldState>,
    /// 重放基线（开档初值或快照状态）：回滚时以它为基线重放保留的事件。
    base_state: Mutex<WorldState>,
    /// 基线覆盖到的命令 seq：回滚只重放 seq 之后的保留事件（快照之前的已在基线里）。
    base_seq: AtomicU64,
    rng: Arc<Mutex<DeterministicRng>>,
    /// 已经写进命令日志的 RNG 消耗条数（水位线）：emit 前把新增的消耗补成 rng_consume
    /// 事件，保证日志与 RNG 位置不漂移，重放可精确恢复（#06 ②）。
    rng_recorded: AtomicUsize,
    rules: SessionRules,
    lua: LuaHost,
    lua_registry: Mutex<LuaRegistry>,
    sink: Arc<dyn EventSink>,
    ai: AiSlot,
    seq: AtomicU64,
    round: AtomicU32,
    busy: AtomicBool,
    auto_confirm: AtomicBool,
    pending: Mutex<Option<Pending>>,
    event_log: Mutex<Vec<EventEnvelope>>,
    request_ids: Mutex<Vec<String>>,
    /// 本存档指定的单一模型；None = 用全局默认。
    model: Mutex<Option<ModelRef>>,
    /// 每回合 token 预算（0 = 不限）：裁剪 lore 等可选注入。
    token_budget: AtomicU32,
    /// 存档级叙述段玩家偏好（section id → 开关 | 变体 key）；只作用于之后的回合。
    narrative_overrides: Mutex<std::collections::BTreeMap<String, NarrativeOverride>>,
    /// 相关往事检索器（#05 §3.4）：None = 不检索，行为与今天一致。
    memory: Mutex<Option<Arc<dyn MemoryRetriever>>>,
    /// 摘要落库端口（#05 §3.2/§3.3）：None = 不写派生摘要（离线 / 测试默认）。
    summary_store: Mutex<Option<Arc<dyn SummaryStore>>>,
    /// 当前场景起始回合（场景压缩窗口的左界）：replay 时按最后的 advance_scene 事件重建。
    scene_start_round: AtomicU32,
    confirmation_timeout_ms: u64,
    /// 事件广播递归深度（防止 Lua 事件脚本互相触发形成死循环）。
    dispatch_depth: AtomicU32,
    /// 最近一次被驳回的意图原因 + 累计次数（#04 续轮回喂）。
    ///
    /// 驳回**不进叙事**（前端把它当引擎 QA 信息过滤），所以模型本来完全看不到
    /// 「技能在冷却 / 资源不足」这类事实，会照着自己的想象把失败写成成功。
    /// 回合内把原因回喂给模型，是「引擎真的拦」与「模型知道为什么」之间的唯一桥。
    last_rejection: Mutex<Option<String>>,
    rejection_count: AtomicU64,
}

impl Session {
    pub fn new(
        save_id: String,
        mut state: WorldState,
        sink: Arc<dyn EventSink>,
        ai: AiSlot,
        auto_confirm: bool,
        storybook: Value,
    ) -> Self {
        // 存档级设置先落地到投影，重放再以日志中的 confirm_toggle 事件为准修正。
        state.meta.auto_confirm = auto_confirm;
        let seed = state.rng_seed;
        let initial_position = state.rng_position as usize;
        // 快照基线可能自带 RNG 位置（#06 ②）：构造时先拨到位，replay 再在其上推进。
        let mut initial_rng = DeterministicRng::new(seed);
        initial_rng.restore(initial_position);
        let rng = Arc::new(Mutex::new(initial_rng));
        // Lua 宿主与引擎共享同一 RNG 序列（#12 ④：engine_rng 走确定性序列）。
        let lua = LuaHost::with_rng(rng.clone(), SandboxLimits::default())
            .expect("初始化 Lua 沙箱宿主失败");
        // 只读开放内容快照（GAP-C）：把冻结故事书的 characters / definitions 原文交给脚本，
        // 让规则由开放内容驱动。引擎不理解其语义（不认识 kind，也不解释 fields / modifiers）。
        lua.set_read_data(lua_read_data(&storybook));
        // 规则集挂载点：随故事书声明装载；故事书随存档冻结，因此规则集也随存档冻结。
        let lua_registry = LuaRegistry::from_storybook(&storybook);
        let base_state = state.clone();
        Self {
            save_id,
            state: Mutex::new(state),
            base_state: Mutex::new(base_state),
            base_seq: AtomicU64::new(0),
            rng,
            rng_recorded: AtomicUsize::new(initial_position),
            rules: SessionRules::from_storybook(storybook),
            lua,
            lua_registry: Mutex::new(lua_registry),
            sink,
            ai,
            seq: AtomicU64::new(0),
            round: AtomicU32::new(0),
            busy: AtomicBool::new(false),
            auto_confirm: AtomicBool::new(auto_confirm),
            pending: Mutex::new(None),
            event_log: Mutex::new(Vec::new()),
            request_ids: Mutex::new(Vec::new()),
            model: Mutex::new(None),
            token_budget: AtomicU32::new(0),
            narrative_overrides: Mutex::new(std::collections::BTreeMap::new()),
            memory: Mutex::new(None),
            summary_store: Mutex::new(None),
            scene_start_round: AtomicU32::new(0),
            confirmation_timeout_ms: 20_000,
            dispatch_depth: AtomicU32::new(0),
            last_rejection: Mutex::new(None),
            rejection_count: AtomicU64::new(0),
        }
    }

    /// 设置每回合 token 预算（0 = 不限）。由组合根在构造后按配置注入。
    pub fn set_token_budget(&self, budget: u32) {
        self.token_budget.store(budget, Ordering::SeqCst);
    }

    /// 应用存档级叙述段玩家偏好（叙事契约 P1）：只影响之后的回合，绝不回写历史。
    pub fn set_narrative_overrides(
        &self,
        overrides: std::collections::BTreeMap<String, NarrativeOverride>,
    ) {
        *self
            .narrative_overrides
            .lock()
            .expect("narrative overrides poisoned") = overrides;
    }

    /// 注入相关往事检索器（#05 §3.4）：由组合根在会话建立后调用。
    ///
    /// None = 不检索——直接 `Session::new` 构造的测试 / 离线场景保持与今天逐字一致。
    pub fn set_memory_retriever(&self, retriever: Option<Arc<dyn MemoryRetriever>>) {
        *self.memory.lock().expect("memory retriever poisoned") = retriever;
    }

    /// 注入摘要落库端口（#05 §3.2/§3.3）：由组合根在会话建立后调用。
    ///
    /// None = 不写摘要：直接 Session::new 构造的测试 / 离线场景保持与今天逐字一致
    /// （摘要是派生数据，缺失只少一段记忆，绝不影响回合与重放）。
    pub fn set_summary_store(&self, store: Option<Arc<dyn SummaryStore>>) {
        *self.summary_store.lock().expect("summary store poisoned") = store;
    }

    /// 检索本回合相关往事：query = 玩家输入 + 当前场景标题，取 top-K。
    ///
    /// 派生数据：检索器内部已做「FTS 失败 → 空」的降级；这里再兜一层，
    /// 任何错误只 warn 并返回空，绝不把失败传导给权威回合。
    async fn retrieve_memories(&self, query: &str) -> Vec<MemoryHit> {
        // 先把 Arc 克隆出来，避免跨 await 持有锁。
        let retriever = self
            .memory
            .lock()
            .expect("memory retriever poisoned")
            .clone();
        let Some(retriever) = retriever else {
            return Vec::new();
        };
        match retriever.retrieve(&self.save_id, query, MEMORY_TOP_K).await {
            Ok(hits) => hits,
            Err(e) => {
                tracing::warn!(
                    save_id = %self.save_id,
                    error = %e,
                    "相关往事检索失败，本回合不注入（回合照常）"
                );
                Vec::new()
            }
        }
    }

    /// 场景压缩（#05 §3.3）：把当前场景期间的回合微摘要合成一条场景摘要。
    ///
    /// 派生数据：读摘要 / AI 压缩 / 写摘要任何一步失败都只 warn，本回合照常结算。
    /// AI 用便宜角色（AiProvider::summarize）；默认实现返回 None → 退化为确定性拼接，
    /// 保证默认配置与测试可复现。
    async fn compress_scene(&self, leaving_scene_id: &str, round: u32) {
        let store = self
            .summary_store
            .lock()
            .expect("summary store poisoned")
            .clone();
        let Some(store) = store else { return };
        // 窗口左界是当前场景起始回合：只压缩本场景期间产生的微摘要。
        let start = self.scene_start_round.load(Ordering::SeqCst);
        let rounds = match store.round_summaries_after(&self.save_id, start).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(save_id = %self.save_id, error = %e, "读取回合微摘要失败，跳过场景压缩");
                return;
            }
        };
        let joined = rounds
            .iter()
            .map(|(_, t)| t.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if joined.is_empty() {
            return;
        }
        // AI 压缩失败 / 返回空 → 直接拼接（确定性兜底），绝不因此让回合失败。
        let ai = self.ai.read().expect("ai poisoned").clone();
        let text = match ai.summarize(&joined).await {
            Ok(Some(s)) if !s.trim().is_empty() => s,
            Ok(_) => joined,
            Err(e) => {
                tracing::warn!(save_id = %self.save_id, error = %e, "场景摘要 AI 压缩失败，退化为拼接");
                joined
            }
        };
        if let Err(e) = store
            .put_scene_summary(&self.save_id, leaving_scene_id, round, &text)
            .await
        {
            tracing::warn!(save_id = %self.save_id, error = %e, "场景摘要写入失败（派生数据，忽略）");
        }
    }

    // ---------- 事件 ----------

    /// 发事件的统一入口：先把自上次记录以来新增的 RNG 消耗补成 rng_consume 事件
    /// （#06 ②），再发本事件。这样任何掷骰都必然落在其后的权威日志里。
    fn emit(&self, event: PlayEvent, actor: Option<ActorRef>, intent_id: Option<String>) -> EventEnvelope {
        self.flush_rng_consumption();
        self.emit_raw(event, actor, intent_id)
    }

    /// 把 RNG 自水位线以来新消耗的原始输出写成一条 rng_consume 事件。
    ///
    /// 事件只承载「消耗了什么」，不改变世界状态（apply_event 只累计 rng_position）。
    /// 重放时据此把 RNG 复位到同一位置，未来的骰序与不重启一致。
    fn flush_rng_consumption(&self) {
        let values = {
            let rng = self.rng.lock().expect("rng poisoned");
            let total = rng.consumed.len();
            let recorded = self.rng_recorded.load(Ordering::SeqCst);
            if total <= recorded {
                return;
            }
            let values = rng.consumed[recorded..].to_vec();
            // 先推进水位线：emit_raw 不会再触发 flush，不会递归。
            self.rng_recorded.store(total, Ordering::SeqCst);
            values
        };
        self.emit_raw(
            PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: Some("rng_consume".into()),
                text: serde_json::to_string(&values).unwrap_or_default(),
            }),
            None,
            None,
        );
    }

    /// 真正构造并广播事件（不做 RNG 补记，供 flush 自身调用以防递归）。
    fn emit_raw(&self, mut event: PlayEvent, actor: Option<ActorRef>, intent_id: Option<String>) -> EventEnvelope {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        // 唯一的状态变更入口：实时与重放走同一条路径，保证二者结果一致。
        // 资源边界夹取必须在 **delta 进日志之前**发生——否则重放会拿未夹取的 delta
        // 再算一遍，实时与重放就分叉了（夹取后的 Set 进日志，重放逐字应用同一个值）。
        if let Ok(mut st) = self.state.lock() {
            self.clamp_event_resources(&st, &mut event);
            apply_event(&mut st, &event);
        }
        let env = EventEnvelope {
            id: uuid::Uuid::new_v4().to_string(),
            seq,
            round: self.round.load(Ordering::SeqCst),
            ts: now_iso(),
            actor,
            intent_id,
            event,
        };
        if let Ok(mut log) = self.event_log.lock() {
            log.push(env.clone());
        }
        self.sink.emit(env.clone());
        env
    }

    /// 资源边界夹取（#12 ③）：把「越过声明边界」的资源增量改写为夹取后的 Set。
    ///
    /// 语义（不预设规则体系；对未声明边界的资源零影响）：
    /// - 只有故事书为该资源声明了 \`default_max\` / \`min\` 才夹；
    /// - 只夹**运动方向上越过边界**的那一步：\`6/10 补 5\` 夹到 \`10\`；
    ///   已经超过上限的值（如满血 30 而 default_max 是 8 的图鉴数据）**不会被拉回来**，
    ///   避免把作者声明的初始值悄悄改小；
    /// - **不设隐式下限**：没声明 \`min\` 的资源可以被扣成负数（D&D 的死亡豁免 / 溢伤
    ///   需要「低于 0 多少」这个事实）；想要 0 下限的作者显式声明 \`min: 0\` 即可；
    /// - 未实际越界时**不改写** delta，历史日志的形状（Add / 数值）逐字不变。
    fn clamp_event_resources(&self, st: &WorldState, event: &mut PlayEvent) {
        let bounds = self.rules.resource_bounds();
        if bounds.is_empty() {
            return;
        }
        let changes: &mut Vec<StateDelta> = match event {
            PlayEvent::Resolution(p) => &mut p.state_changes,
            PlayEvent::StateUpdate(p) => &mut p.changes,
            _ => return,
        };
        // 同一事件内的多个 delta 按声明顺序折叠：后一条以「前一条之后的值」为基座。
        let mut working: std::collections::HashMap<(String, String), i64> =
            std::collections::HashMap::new();
        for d in changes.iter_mut() {
            if d.domain != DeltaDomain::Character {
                continue;
            }
            let Some(resource) = d.field.strip_prefix("resources.") else { continue };
            let Some((min, max)) = bounds.get(resource) else { continue };
            let key = (d.entity_id.clone(), resource.to_string());
            let cur = working.get(&key).copied().or_else(|| {
                st.characters
                    .get(&d.entity_id)
                    .and_then(|c| c.resources.get(resource))
                    .and_then(resource_num)
            });
            let Some(cur) = cur else { continue };
            // Set 是显式赋值：等价增量 = 目标值 - 当前值；其余按 Add 处理。
            let amount = match d.op {
                DeltaOp::Set => d.value.as_i64().unwrap_or(cur).saturating_sub(cur),
                _ => d.value.as_i64().unwrap_or(0),
            };
            let raw = cur.saturating_add(amount);
            let mut clamped = raw;
            if let Some(max) = max {
                if clamped > *max {
                    clamped = (*max).max(cur);
                }
            }
            if let Some(min) = min {
                if clamped < *min {
                    clamped = (*min).min(cur);
                }
            }
            working.insert(key, clamped);
            if clamped != raw {
                d.op = DeltaOp::Set;
                d.value = Value::from(clamped);
            }
        }
    }

    fn emit_simple(&self, event: PlayEvent) -> EventEnvelope {
        self.emit(event, None, None)
    }

    /// 推一条 AI 思考链事件（reasoning_content）：给玩家作参考，不改世界状态。
    fn emit_reasoning(&self, stage: &str, text: String) {
        self.emit_simple(PlayEvent::Reasoning(ReasoningPayload {
            stage: stage.to_string(),
            text,
            source: "provider".to_string(),
        }));
    }

    /// 协议适配器过滤掉意图时落 System 警告（code = intent_not_allowed），便于创作者排查。
    fn emit_intent_warnings(&self, warnings: &[String]) {
        for w in warnings {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Warn,
                code: Some("intent_not_allowed".into()),
                text: w.clone(),
            }));
        }
    }

    fn phase(&self, stage: PhaseStage, detail: Option<String>) {
        self.emit_simple(PlayEvent::Phase(PhasePayload { stage, detail }));
    }

    // ---------- 查询 ----------

    pub fn projection(&self) -> WorldProjection {
        let (mut proj, scene_id, progress_goals) = {
            let mut st = self.state.lock().expect("state poisoned");
            st.seq = self.seq.load(Ordering::SeqCst);
            (st.projection(), st.scene_id.clone(), st.progress.goals.clone())
        };
        // 当前场景的骨架目标随投影一起下发。UI 左栏是直接读故事书骨架画出来的，
        // 但 AI 提示词只认 projection.quests —— 不在此合成，AI 就永远看不到「当前任务」。
        let mut quests = self.skeleton_quests(&scene_id, &progress_goals);
        if !quests.is_empty() {
            quests.append(&mut proj.quests);
            proj.quests = quests;
        }
        proj
    }

    /// 当前场景的骨架目标 → QuestView（source = skeleton）。id 缺失时按「场景 id + 序号」
    /// 合成稳定 id：导入的故事书常见无 id，仍要保证提示词与进度键可复现。
    fn skeleton_quests(
        &self,
        scene_id: &str,
        progress: &serde_json::Map<String, Value>,
    ) -> Vec<QuestView> {
        let mut out = Vec::new();
        let Some(chapters) = self.rules.skeleton().and_then(Value::as_array) else {
            return out;
        };
        for chapter in chapters {
            let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
                continue;
            };
            for scene in scenes {
                if scene.get("id").and_then(Value::as_str) != Some(scene_id) {
                    continue;
                }
                let Some(goals) = scene.get("goals").and_then(Value::as_array) else {
                    continue;
                };
                // 任务的地点（地图 P5 §6.2）：继承**所属场景**，投影时推导、不落库。
                let location_id = scene
                    .get("location_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                for (idx, goal) in goals.iter().enumerate() {
                    let text = goal.get("text").and_then(Value::as_str).unwrap_or("").trim();
                    if text.is_empty() {
                        continue;
                    }
                    let id = goal_id_of(scene_id, idx, goal);
                    let done = progress.get(&id).and_then(Value::as_bool).unwrap_or(false);
                    out.push(QuestView {
                        id,
                        text: text.to_string(),
                        done,
                        source: "skeleton".into(),
                        hidden: goal.get("hidden").and_then(Value::as_bool).unwrap_or(false),
                        primary: goal.get("primary").and_then(Value::as_bool).unwrap_or(false),
                        location_id: location_id.clone(),
                    });
                }
            }
        }
        out
    }

    /// 当前权威日志的最大 seq（写检查点 / 新原点时据此分配新序号）。
    pub fn current_seq(&self) -> Seq {
        self.seq.load(Ordering::SeqCst)
    }

    /// 当前回合号（检查点事件沿用它，保证 history 展示的回合归属连续）。
    pub fn current_round(&self) -> u32 {
        self.round.load(Ordering::SeqCst)
    }

    /// 取出检查点两部分：世界状态（含已进日志的 RNG 位置）+ 场景压缩左界。
    ///
    /// RNG 位置以水位线为准（已进日志的消耗）：快照是日志的缓存，绝不能包含未落日志的
    /// 掷骰，否则「快照 + 其后命令」会与全量重放分叉（#06 ②）。
    fn checkpoint_parts(&self) -> (WorldState, u32) {
        let mut st = self.state.lock().expect("state poisoned");
        st.rng_position = self.rng_recorded.load(Ordering::SeqCst) as u64;
        (st.clone(), self.scene_start_round.load(Ordering::SeqCst))
    }

    /// 把当前世界状态序列化为全量检查点 JSON（#14）。
    ///
    /// 供「新原点」把状态写成新起点、供「版次升级」在换故事书前钉住运行时状态——
    /// 否则重放会用新故事书的初值重建，已有的属性 / 位置 / 物品栏会漂移。
    pub fn snapshot_value(&self) -> Value {
        let (state, scene_start_round) = self.checkpoint_parts();
        serde_json::to_value(OriginCheckpoint { state, scene_start_round })
            .unwrap_or(Value::Null)
    }

    /// 当前状态的全量快照基座（#06 ② 启动缓存），seq 为当前权威日志最大序号。
    pub fn snapshot_base(&self) -> SnapshotBase {
        let (state, scene_start_round) = self.checkpoint_parts();
        SnapshotBase { seq: self.seq.load(Ordering::SeqCst), state, scene_start_round }
    }

    /// 维护操作（升级 / 新原点）抢占会话：成功即占住 busy，并发的 run_round
    /// 与 submit_round（经 is_idle）都会被挡住；失败表示已有回合在跑。
    pub fn try_begin_maintenance(&self) -> bool {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// 释放维护占用。维护路径无论成败都必须调用（见 API 层的 Drop guard）。
    pub fn end_maintenance(&self) {
        self.busy.store(false, Ordering::SeqCst);
    }

    pub fn history(&self, before_seq: Option<Seq>, limit: usize) -> HistoryPage {
        let log = self.event_log.lock().expect("event log poisoned");
        let all: Vec<EventEnvelope> = log
            .iter()
            .filter(|e| before_seq.is_none_or(|b| e.seq < b))
            .cloned()
            .collect();
        let start = all.len().saturating_sub(limit);
        HistoryPage { events: all[start..].to_vec(), has_more: start > 0 }
    }

    /// 从命令日志重放：纯投影重建世界状态与事件日志，**不调用 AI、不重新结算**。
    /// 必须在任何 `emit` 之前调用一次。
    pub fn replay(&self, persisted: &[PersistedEvent]) {
        self.replay_from(persisted, None)
    }

    /// 从命令日志重放，可选以一份全量快照为基线（#06 ② 启动缓存）。
    ///
    /// 快照只是缓存：seq <= base.seq 的命令不再重新投影（状态已物化在快照里），
    /// 但**全部**命令仍进内存 event_log（history 分页照旧）。传 None 等价于全量重放。
    /// 重放末尾把 RNG 拨回日志 / 快照累计的消耗位置，保证跨重启骰序连续。
    pub fn replay_from(&self, persisted: &[PersistedEvent], base: Option<SnapshotBase>) {
        let mut max_seq: Seq = 0;
        let mut max_round: u32 = 0;
        // 场景压缩窗口左界：从权威日志里最后一次 advance_scene 结算重建，
        // 保证重启后不会把旧场景的微摘要重复压进新场景摘要（派生数据也要可复现）。
        let mut scene_start_round: u32 =
            base.as_ref().map(|b| b.scene_start_round).unwrap_or(0);
        {
            let mut st = self.state.lock().expect("state poisoned");
            let mut log = self.event_log.lock().expect("event log poisoned");
            let mut reqs = self.request_ids.lock().expect("request ids poisoned");
            // 快照基线：直接放回物化状态，只有其后的命令需要重新投影。
            let apply_after = match &base {
                Some(b) => {
                    *st = b.state.clone();
                    Some(b.seq)
                }
                None => None,
            };
            for p in persisted {
                let env = &p.envelope;
                // 快照之前的命令不再投影（状态已在快照里），但仍进日志供 history 回读。
                if apply_after.is_none_or(|seq| env.seq > seq) {
                    apply_event(&mut st, &env.event);
                }
                if let PlayEvent::Resolution(r) = &env.event {
                    if r.outcome.as_deref() == Some("advance_scene") {
                        scene_start_round = env.round;
                    }
                }
                // 新原点 / 升级检查点携带场景压缩左界：从权威日志恢复派生态基线。
                if let PlayEvent::StateUpdate(p) = &env.event {
                    for d in &p.changes {
                        if d.domain == DeltaDomain::Origin {
                            if let Ok(cp) =
                                serde_json::from_value::<OriginCheckpoint>(d.value.clone())
                            {
                                scene_start_round = cp.scene_start_round;
                            }
                        }
                    }
                }
                max_seq = max_seq.max(env.seq);
                max_round = max_round.max(env.round);
                if let Some(rid) = &p.request_id {
                    if !reqs.iter().any(|x| x == rid) {
                        reqs.push(rid.clone());
                    }
                }
                log.push(env.clone());
            }
            st.seq = max_seq;
            // #06 ①：重放不做结算，必须显式把 RNG 拨回日志记录的消耗位置。
            let position = st.rng_position as usize;
            self.rng.lock().expect("rng poisoned").restore(position);
            self.rng_recorded.store(position, Ordering::SeqCst);
        }
        self.seq.store(max_seq, Ordering::SeqCst);
        self.round.store(max_round, Ordering::SeqCst);
        self.scene_start_round.store(scene_start_round, Ordering::SeqCst);
        // 快照基线同时成为回滚基线：快照之前的命令不再重复叠加。
        if let Some(b) = base {
            *self.base_state.lock().expect("base state poisoned") = b.state;
            self.base_seq.store(b.seq, Ordering::SeqCst);
        }
    }

    /// 等待此前 emit 的事件全部落库（回滚前保证读写一致）。
    pub async fn flush_events(&self) {
        // 先把尚未记录的 RNG 消耗补进日志，再等屏障：屏障之后读到的都是持久化事实。
        self.flush_rng_consumption();
        self.sink.flush().await;
    }

    /// 是否空闲到可以接受新回合：没有进行中的回合，也没有待确认动作。
    ///
    /// 设计决策 #6：非 idle（thinking / resolving / waiting_confirm）提交新回合应当被拒，
    /// 而不是排进队列。API 层据此在 `tokio::spawn` **之前**返回 409。
    pub fn is_idle(&self) -> bool {
        !self.busy.load(Ordering::SeqCst)
            && self.pending.lock().map(|p| p.is_none()).unwrap_or(false)
    }

    /// 是否空闲到可以回滚（与「可接受新回合」同一条判据）。
    pub fn can_rewind(&self) -> bool {
        self.is_idle()
    }

    /// 该 request_id 是否已受理过（#24 幂等去重）：重复提交放行，新工作在被拒时返回 409。
    pub fn has_seen_request(&self, request_id: &str) -> bool {
        self.request_ids
            .lock()
            .map(|ids| ids.iter().any(|x| x == request_id))
            .unwrap_or(false)
    }

    /// 最后一个玩家回合的回滚计划；没有任何回合时返回 None。
    pub fn rewind_plan(&self) -> Option<RewindPlan> {
        let log = self.event_log.lock().expect("event log poisoned");
        log.iter().rev().find_map(|env| match &env.event {
            PlayEvent::RoundStart(p) => Some(RewindPlan {
                round: env.round,
                from_seq: env.seq,
                input: p.input.clone(),
            }),
            _ => None,
        })
    }

    /// 回滚到 from_seq 之前：世界状态以重放基线重建保留的事件，事件日志就地截断。
    /// seq 计数器**不复位**（重跑事件用新 seq，避免与旧广播 / 归档冲突）；
    /// round 回退到 round-1，让重跑复用同一回合号。
    pub fn apply_rewind(&self, from_seq: Seq, round: u32) {
        {
            let mut st = self.state.lock().expect("state poisoned");
            *st = self.base_state.lock().expect("base state poisoned").clone();
            let base_seq = self.base_seq.load(Ordering::SeqCst);
            let mut log = self.event_log.lock().expect("event log poisoned");
            log.retain(|e| e.seq < from_seq);
            // 基线覆盖到的命令不重复投影（有快照时它们已物化在基线里）。
            for env in log.iter().filter(|e| e.seq > base_seq) {
                apply_event(&mut st, &env.event);
            }
            st.seq = self.seq.load(Ordering::SeqCst);
            // #06 ①：回滚截断了事件，RNG 也必须退回保留命令记录的消耗位置。
            let position = st.rng_position as usize;
            self.rng.lock().expect("rng poisoned").restore(position);
            self.rng_recorded.store(position, Ordering::SeqCst);
        }
        self.round.store(round.saturating_sub(1), Ordering::SeqCst);
    }

    pub fn auto_confirm(&self) -> bool {
        self.auto_confirm.load(Ordering::SeqCst)
    }

    // ---------- 玩家动作 ----------

    pub fn confirm(&self, action_id: &str, decision: ConfirmDecision) -> Result<(), EngineError> {
        let mut slot = self.pending.lock().expect("pending poisoned");
        match slot.take() {
            Some(pending) if pending.action_id == action_id => {
                let _ = pending.tx.send(decision);
                Ok(())
            }
            other => {
                *slot = other;
                Err(EngineError::Conflict("expired".into()))
            }
        }
    }

    /// 本存档指定的单一模型；None = 用全局默认。
    pub fn model(&self) -> Option<ModelRef> {
        self.model.lock().ok().and_then(|m| m.clone())
    }

    /// 设置本存档的单一模型；只影响之后的回合，不重写历史。
    pub fn set_model(&self, model: Option<ModelRef>) {
        if let Ok(mut slot) = self.model.lock() {
            *slot = model;
        }
    }

    pub fn set_auto_confirm(&self, v: bool) {
        // 只在真的改变时落事件：存档设置端点每次都会回写 auto_confirm（改模型 / 叙述偏好等），
        // 无条件 emit 会让「每写一次设置就多一条 confirm_toggle」刷屏并污染命令日志。
        let prev = self.auto_confirm.swap(v, Ordering::SeqCst);
        if prev == v {
            return;
        }
        self.emit_simple(PlayEvent::System(SystemPayload {
            level: SystemLevel::Info,
            code: Some("confirm_toggle".into()),
            text: format!("免确认模式已{}", if v { "开启" } else { "关闭" }),
        }));
    }

    /// 开档开场白：新游戏默认输出世界观里的「故事开头」（缺省回落世界前提），
    /// 作为第一条叙事落进事件日志 —— 历史回放天然带上它，任何客户端都看得到，
    /// 也不会因为后续切角色等系统事件而消失。
    pub fn emit_opening(&self) -> bool {
        let world = self.rules.storybook.get("world");
        let pick = |k: &str| -> Option<String> {
            world
                .and_then(|w| w.get(k))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let Some(opening) = pick("opening").or_else(|| pick("premise")) else {
            return false;
        };
        let (scene_id, scene_title, scene_description, present) = {
            let st = self.state.lock().expect("state poisoned");
            // 在场名单＝骨架初始场景的 present_char_ids（受控角色恒在场），
            // 不再把全书角色都算进来。
            let present: Vec<String> = st
                .characters
                .iter()
                .filter(|(id, c)| c.present || st.controlled.contains(id))
                .map(|(id, _)| id.clone())
                .collect();
            (
                st.scene_id.clone(),
                st.scene_title.clone(),
                st.scene_description.clone(),
                present,
            )
        };
        self.emit_simple(PlayEvent::Scene(ScenePayload {
            scene_id,
            title: scene_title,
            description: scene_description,
            present,
        }));
        self.emit_simple(PlayEvent::Narrate(NarratePayload { content: opening, scene_ref: None }));
        true
    }

    /// 切换受控角色。存档内的寻址键是**角色实例 id**；为兼容外部传入的模板 id，
    /// 这里两种 id 都认，最终一律落到实例键上。
    pub fn switch_character(&self, character_id: &str) -> Result<(), EngineError> {
        let found = {
            let st = self.state.lock().expect("state poisoned");
            let key = if st.characters.contains_key(character_id) {
                Some(character_id.to_string())
            } else {
                st.characters
                    .iter()
                    .find(|(_, c)| c.template_id == character_id || c.instance_id == character_id)
                    .map(|(k, _)| k.clone())
            };
            key.and_then(|k| st.characters.get(&k).map(|c| (k, c.name.clone(), c.kind.clone())))
        };
        match found {
            Some((key, name, kind)) if kind == "pc" => {
                self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
                    changes: vec![StateDelta {
                        domain: DeltaDomain::Character,
                        entity_id: key,
                        field: "controlled".into(),
                        op: DeltaOp::Set,
                        value: Value::Bool(true),
                    }],
                }));
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("switch_character".into()),
                    text: format!("已切换受控角色：{name}"),
                }));
                Ok(())
            }
            Some(_) => Err(EngineError::Conflict("not_controllable".into())),
            None => Err(EngineError::Conflict("character_not_found".into())),
        }
    }

    /// 一次休息（#4 短休 / 长休）：按 world.resources 的 natural_recovery 恢复受控角色的资源。
    pub fn rest(&self, kind: crate::recovery::RestKind) -> Result<Vec<StateDelta>, EngineError> {
        let (actor_id, world_resources, actor_resources) = {
            let st = self.state.lock().expect("state poisoned");
            let actor_id = st.controlled.first().cloned().unwrap_or_default();
            let actor_resources = st
                .characters
                .get(&actor_id)
                .map(|c| Value::Object(c.resources.clone()))
                .unwrap_or(Value::Null);
            let world_resources = self
                .rules
                .storybook
                .get("world")
                .and_then(|w| w.get("resources"))
                .cloned()
                .unwrap_or(Value::Null);
            (actor_id, world_resources, actor_resources)
        };
        if actor_id.is_empty() {
            return Err(EngineError::Conflict("no_controlled_character".into()));
        }
        let changes =
            crate::recovery::rest_deltas(&actor_id, &world_resources, &actor_resources, kind);
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes: changes.clone() }));
        }
        let label = match kind {
            crate::recovery::RestKind::Short => "短休",
            crate::recovery::RestKind::Long => "长休",
        };
        self.emit_simple(PlayEvent::System(SystemPayload {
            level: SystemLevel::Info,
            code: Some("rest".into()),
            text: format!("{label}完成，恢复 {} 项资源", changes.len()),
        }));
        Ok(changes)
    }

    // ---------- 时序与行动经济（#GAP-I） ----------

    /// 生效的时序声明：声明了 \`world.turn\` 且真的启用（有顺序来源或预算）。
    fn turn_rules(&self) -> Option<&TurnRules> {
        self.rules.turn().filter(|r| r.enabled())
    }

    /// Lua 只读时序快照（#GAP-I）：脚本的 \`host.turn\`。不在时序中时 None（读到 nil）。
    ///
    /// 只给事实（轮次 / 顺序 / 当前行动者 / 剩余预算），**不给任何规则集语义**——
    /// 「谁被突袭」「什么时候该失去回合」由规则包 Lua 自己判断。
    fn turn_snapshot(&self) -> Option<Value> {
        let st = self.state.lock().ok()?;
        if st.turn.order.is_empty() {
            return None;
        }
        let current = st.turn.order.get(st.turn.index).cloned();
        let budgets = current
            .as_ref()
            .and_then(|k| st.turn.budgets.get(k))
            .map(|m| serde_json::json!(m))
            .unwrap_or_else(|| serde_json::json!({}));
        Some(serde_json::json!({
            "round": st.turn.round,
            "order": st.turn.order,
            "index": st.turn.index,
            "current": current,
            "budgets": budgets,
            "combat": self.combat_active(&st),
        }))
    }

    /// 当前场景是否有**未结束且未全灭**的遭遇——时序的激活判据。
    ///
    /// 与 \`CondExpr::EncounterActive\` 同一口径（两处必须同时改）：场景归属看 \`scene_id\`，
    /// 已结束（active=false）或敌人全灭都不算「在战斗中」。
    fn combat_active(&self, st: &WorldState) -> bool {
        st.encounters.values().any(|enc| {
            let belongs = match enc.get("scene_id").and_then(Value::as_str) {
                Some(s) => s == st.scene_id,
                None => true,
            };
            if !belongs {
                return false;
            }
            if !enc.get("active").and_then(Value::as_bool).unwrap_or(true) {
                return false;
            }
            !enc
                .get("enemies")
                .and_then(Value::as_array)
                .is_some_and(|l| {
                    l.iter()
                        .all(|e| e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0)
                })
        })
    }

    /// 时序激活 / 退出（#GAP-I）：在「遭遇刚建好」与「回合末」两处调用。
    ///
    /// 未声明 \`world.turn\` 时完全静默——旧故事书的行为逐字不变。
    fn sync_turn_combat(&self) {
        let Some(rules) = self.turn_rules().cloned() else { return };
        let (active, has_order) = {
            let st = self.state.lock().expect("state poisoned");
            (self.combat_active(&st), !st.turn.order.is_empty())
        };
        if active && !has_order {
            self.begin_combat(&rules);
        } else if !active && has_order {
            // 战斗结束：清空顺序即退出时序（预算残留无害，下次开战重建）。
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
                changes: vec![turn_delta("turn/order", DeltaOp::Set, serde_json::json!([]))],
            }));
        }
    }

    /// 开始一场战斗的时序：排定先攻顺序、建立全员预算（#GAP-I）。
    ///
    /// 先攻掷骰走 engine RNG → 自动落 \`rng_consume\`，重放时骰序连续；
    /// 同分按实例键排序，「同一日志 → 同一顺序」是确定性的。
    fn begin_combat(&self, rules: &TurnRules) {
        // 参战者 = 在场的角色实例 + 当前场景遭遇里还活着的敌人实例。
        let (participants, global_checker) = {
            let st = self.state.lock().expect("state poisoned");
            let mut keys: Vec<String> = st
                .characters
                .iter()
                .filter(|(_, c)| c.present)
                .map(|(k, _)| k.clone())
                .collect();
            for enc in st.encounters.values() {
                let belongs = match enc.get("scene_id").and_then(Value::as_str) {
                    Some(s) => s == st.scene_id,
                    None => true,
                };
                if !belongs || !enc.get("active").and_then(Value::as_bool).unwrap_or(true) {
                    continue;
                }
                if let Some(list) = enc.get("enemies").and_then(Value::as_array) {
                    for e in list {
                        if e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0 {
                            continue;
                        }
                        if let Some(k) = e.get("instance_id").and_then(Value::as_str) {
                            if !keys.iter().any(|x| x == k) {
                                keys.push(k.to_string());
                            }
                        }
                    }
                }
            }
            let info: Vec<(String, String, f64)> = keys
                .iter()
                .map(|k| {
                    let c = st.characters.get(k);
                    let template = c.map(|c| c.template_id.clone()).unwrap_or_default();
                    let value = rules
                        .initiative_attribute
                        .as_ref()
                        .and_then(|a| c.and_then(|c| c.attributes.get(a)))
                        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
                        .unwrap_or(0.0);
                    (k.clone(), template, value)
                })
                .collect();
            (info, self.rules.global_checker())
        };
        let mut scored: Vec<(String, i64)> = Vec::new();
        for (key, template_id, attr_value) in participants {
            let score = if let Some(fixed) = rules.initiative_fixed.get(&template_id) {
                *fixed
            } else if rules.order == TurnOrderKind::Initiative {
                let rolled = {
                    let mut rng = self.rng.lock().expect("rng poisoned");
                    crate::resolve::roll_dice(&rules.initiative_dice, &mut rng)
                        .map(|d| d.total)
                        .unwrap_or(0)
                };
                let modifier = match (&rules.initiative_attribute, global_checker.as_ref()) {
                    (Some(attr), Some(checker)) => {
                        let profile =
                            self.rules.profiles().get(attr).cloned().unwrap_or_default();
                        crate::resolve::modifier_for(checker, attr, attr_value, profile)
                    }
                    _ => 0,
                };
                rolled + modifier
            } else {
                0
            };
            scored.push((key, score));
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let order: Vec<String> = scored.into_iter().map(|(k, _)| k).collect();
        let names: Vec<String> = {
            let st = self.state.lock().expect("state poisoned");
            order
                .iter()
                .map(|k| {
                    st.characters
                        .get(k)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| k.clone())
                })
                .collect()
        };
        let mut changes = vec![
            turn_delta("turn/order", DeltaOp::Set, serde_json::json!(order)),
            turn_delta("turn/index", DeltaOp::Set, serde_json::json!(0)),
            turn_delta("turn/round", DeltaOp::Set, serde_json::json!(1)),
        ];
        for key in &order {
            for (id, amount) in &rules.budgets {
                changes.push(turn_delta(
                    &format!("turn/budget.{key}.{id}"),
                    DeltaOp::Set,
                    serde_json::json!(amount),
                ));
            }
        }
        self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        self.emit_simple(PlayEvent::System(SystemPayload {
            level: SystemLevel::Info,
            code: Some("turn_order".into()),
            text: format!("战斗开始，先攻顺序：{}", names.join(" → ")),
        }));
    }

    /// 一次机械意图要花掉哪些预算（#GAP-I）。
    ///
    /// 口径（**缺省不扣**，作者必须显式）：技能声明的 \`budget\` 优先（use_skill / use_item /
    /// 带 skill_id 的 strike）；否则查 \`world.turn.intent_budget\`（amount 视为 1）；都没有 = 不消耗。
    fn turn_costs(&self, rules: &TurnRules, intent: &Intent) -> Vec<(String, i64)> {
        let mut out: Vec<(String, i64)> = Vec::new();
        // 物品技能与技能同口径：物品引用的第一个技能声明什么就扣什么（不 leak、不复制语义）。
        let skill_id: Option<String> = match intent {
            Intent::UseSkill { skill_id, .. } => Some(skill_id.clone()),
            Intent::UseItem { item_id, .. } => self.rules.item_skill_ids(item_id).into_iter().next(),
            Intent::Strike { skill_id, .. } => skill_id.clone(),
            _ => None,
        };
        if let Some(sid) = skill_id.as_deref() {
            if let Some(skill) = self.rules.skill(sid) {
                for b in &skill.budget {
                    if b.amount > 0 {
                        out.push((b.budget.clone(), b.amount));
                    }
                }
            }
        }
        if out.is_empty() {
            if let Some(kind) = mechanical_intent_kind(intent) {
                if let Some(bid) = rules.intent_budget.get(kind) {
                    out.push((bid.clone(), 1));
                }
            }
        }
        // 同一预算声明多次 → 合并，避免重复检查 / 重复扣。
        let mut merged: Vec<(String, i64)> = Vec::new();
        for (id, amount) in out {
            match merged.iter_mut().find(|(x, _)| *x == id) {
                Some(slot) => slot.1 += amount,
                None => merged.push((id, amount)),
            }
        }
        merged
    }

    /// 时序闸门（#GAP-I）：机械意图必须来自当前行动者，且预算足够；通过则**就地扣除**预算。
    ///
    /// 未声明 \`world.turn\` / 不在战斗中 / 非机械意图 → 恒放行（旧行为逐字不变）。
    /// 「宣告即消耗」与规则书一致：即使随后被别的闸门（冷却 / 资源）驳回，这次行动也已经花掉了。
    fn turn_gate(
        &self,
        intent: &Intent,
        actor: Option<&ActorRef>,
    ) -> Result<(), (RejectionCode, String)> {
        let Some(rules) = self.turn_rules().cloned() else { return Ok(()) };
        if mechanical_intent_kind(intent).is_none() {
            return Ok(());
        }
        let actor_id = actor.map(|a| a.id.clone()).unwrap_or_default();
        let (active, current, cur_name, skipped, budgets) = {
            let st = self.state.lock().expect("state poisoned");
            let current = st.turn.order.get(st.turn.index).cloned();
            let name_of = |k: &str| {
                st.characters
                    .get(k)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| k.to_string())
            };
            let cur_name = current.as_deref().map(name_of).unwrap_or_default();
            let skipped = current
                .as_ref()
                .map(|k| st.turn.skip.get(k).copied().unwrap_or(0) > 0)
                .unwrap_or(false);
            let budgets = current
                .as_ref()
                .and_then(|k| st.turn.budgets.get(k).cloned())
                .unwrap_or_default();
            (self.combat_active(&st), current, cur_name, skipped, budgets)
        };
        // 不在战斗中 / 还没排顺序 → 不约束（时序尚未建立）。
        if !active {
            return Ok(());
        }
        let Some(current) = current else { return Ok(()) };
        let actor_key = {
            let st = self.state.lock().expect("state poisoned");
            Self::find_character(&st.characters, &actor_id)
                .map(|(k, _)| k.clone())
                .unwrap_or(actor_id)
        };
        if actor_key.is_empty() {
            // 归不到具体角色（意图没署名、也没有受控角色）：无法判定归属 → 保持旧行为放行，
            // 绝不因为「认不出是谁」就驳回一条本来合法的意图。
            return Ok(());
        }
        if actor_key != current {
            return Err((
                RejectionCode::NotYourTurn,
                format!("现在不是你的时序回合（当前行动者：{cur_name}）"),
            ));
        }
        if skipped {
            return Err((
                RejectionCode::NotYourTurn,
                "你本轮失去了自己的时序回合".to_string(),
            ));
        }
        let costs = self.turn_costs(&rules, intent);
        for (id, amount) in &costs {
            let left = budgets.get(id).copied().unwrap_or(0);
            if left < *amount {
                return Err((
                    RejectionCode::InsufficientBudget,
                    format!("行动预算「{id}」不足（剩余 {left}，需要 {amount}）"),
                ));
            }
        }
        if !costs.is_empty() {
            let changes: Vec<StateDelta> = costs
                .iter()
                .map(|(id, amount)| {
                    turn_delta(
                        &format!("turn/budget.{actor_key}.{id}"),
                        DeltaOp::Add,
                        serde_json::json!(-*amount),
                    )
                })
                .collect();
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
        Ok(())
    }

    /// 结束当前行动者的时序回合（#GAP-I）：跳过计数递减、指针推进、按声明重置预算。
    fn end_turn(&self) {
        let Some(rules) = self.turn_rules().cloned() else { return };
        let changes = {
            let st = self.state.lock().expect("state poisoned");
            if !self.combat_active(&st) || st.turn.order.is_empty() {
                return;
            }
            let n = st.turn.order.len();
            let mut index = st.turn.index;
            let mut round = st.turn.round.max(1);
            let mut skip_now = st.turn.skip.clone();
            let mut changes: Vec<StateDelta> = Vec::new();
            // 找下一个没被跳过的人；跳过者本轮消耗一次 skip 计数。
            // guard 兜底：全员都被跳过时最多绕两圈就停下，绝不死循环。
            let mut guard = 0usize;
            loop {
                guard += 1;
                if guard > n * 3 + 8 {
                    break;
                }
                index += 1;
                if index >= n {
                    index = 0;
                    round += 1;
                }
                let key = st.turn.order[index].clone();
                match skip_now.get(&key).copied().unwrap_or(0) {
                    0 => break,
                    left => {
                        let next = left - 1;
                        if next == 0 {
                            skip_now.remove(&key);
                            changes.push(turn_delta(
                                &format!("turn/skip.{key}"),
                                DeltaOp::Remove,
                                Value::Null,
                            ));
                        } else {
                            skip_now.insert(key.clone(), next);
                            changes.push(turn_delta(
                                &format!("turn/skip.{key}"),
                                DeltaOp::Set,
                                serde_json::json!(next),
                            ));
                        }
                    }
                }
            }
            changes.push(turn_delta("turn/index", DeltaOp::Set, serde_json::json!(index)));
            changes.push(turn_delta("turn/round", DeltaOp::Set, serde_json::json!(round)));
            // 预算重置：缺省「轮到自己时重置自己的」；reset = per_round 则每轮重置全员。
            let keys: Vec<String> = if rules.reset_per_round {
                st.turn.order.clone()
            } else {
                vec![st.turn.order[index].clone()]
            };
            for key in keys {
                for (id, amount) in &rules.budgets {
                    changes.push(turn_delta(
                        &format!("turn/budget.{key}.{id}"),
                        DeltaOp::Set,
                        serde_json::json!(amount),
                    ));
                }
            }
            changes
        };
        if changes.is_empty() {
            return;
        }
        self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
    }

    // ---------- 回合管线（#03） ----------

    pub async fn run_round(
        &self,
        input: RoundInput,
        request_id: Option<String>,
        focus: Vec<FocusEntity>,
    ) -> Result<(), EngineError> {
        if input.text.trim().is_empty() {
            return Err(EngineError::EmptyInput);
        }
        if let Some(rid) = &request_id {
            let mut ids = self.request_ids.lock().expect("request ids poisoned");
            if ids.iter().any(|x| x == rid) {
                return Ok(());
            }
            ids.push(rid.clone());
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(EngineError::RoundInProgress);
        }
        let _guard = BusyGuard(&self.busy);

        let round = self.round.fetch_add(1, Ordering::SeqCst) + 1;
        // 回合开始：turns 单位的持续状态 tick（#12 ④）
        self.tick_statuses_turn();

        self.emit_simple(PlayEvent::RoundStart(RoundStartPayload {
            input: input.clone(),
            request_id: request_id.clone(),
        }));

        if input.channel == RoundChannel::Meta {
            self.handle_meta(&input.text);
            self.emit_simple(PlayEvent::RoundEnd(RoundEndPayload { round }));
            return Ok(());
        }

        let (scene_id, scene_title, scene_description, controlled, chars) = {
            let st = self.state.lock().expect("state poisoned");
            // 提示词里用「名字(模板 id)」表示受控角色，和在场角色名单同格式。
            let controlled = st
                .controlled
                .first()
                .and_then(|id| st.characters.get(id))
                .map(|c| format!("{}({})", c.name, c.template_id))
                .unwrap_or_default();
            (
                st.scene_id.clone(),
                st.scene_title.clone(),
                st.scene_description.clone(),
                controlled,
                present_actors(&st),
            )
        };
        // 检索相关往事（#05 §3.4）：query = 玩家输入 + 当前场景标题。
        // 检索是派生数据，失败已在 retrieve_memories 内吞掉，回合照常。
        let memories = self
            .retrieve_memories(&format!("{} {}", input.text, scene_title))
            .await;
        // 在场人物的人格档案（背景/性格/外观/对话示例）注入提示词；作者注释不在此列。
        let persona_ids: Vec<String> = chars.iter().map(|c| c.id.clone()).collect();
        let personas = self.rules.personas(&persona_ids, PERSONA_LIMIT);
        // 世界词条：扫玩家输入 + 当前场景标题/描述，命中才注入（按 token 预算裁剪）。
        let token_budget = self.token_budget.load(Ordering::SeqCst) as usize;
        let lore_haystack = format!(
            "{} {} {}",
            input.text,
            scene_title,
            scene_description.as_deref().unwrap_or("")
        );
        let lore = self.rules.match_lore(&lore_haystack, lore_budget_chars(token_budget));
        // 叙事契约（P1）：本回合解析出最终叙述段——enabled / when / 玩家偏好 / 变体组。
        // 纯函数：世界状态取当回合快照、偏好取存档设置；不写状态、不改历史。
        let narrative = {
            let overrides = self
                .narrative_overrides
                .lock()
                .expect("narrative overrides poisoned")
                .clone();
            self.with_eval_context("narrative", |eval| self.rules.narrative(eval, &overrides))
        };
        let live = self.projection();
        // 当前地点名（位置链路 P2，设计 §6.6）：只进回合用户提示词，绝不进系统 preamble
        // ——AGENTS.md 上下文缓存不变量：系统层必须逐回合稳定。
        let location = self.current_location(&scene_id);
        let ctx = TurnContext {
            save_id: self.save_id.clone(),
            round,
            scene_id,
            scene_title,
            scene_description,
            location,
            controlled,
            player_text: input.text.clone(),
            channel: input.channel,
            characters: chars,
            personas,
            premise: self.rules.premise(),
            narrative,
            lore,
            memories,
            token_budget,
            focus,
            canon: self.canon_lines(8),
            // 首轮恒为空：续轮结果由循环就地填入（#04 ⑦）。
            turn_feedback: Vec::new(),
            quests: live.quests,
            encounters: live.encounters,
            scenes: self.scene_briefs(),
            attributes: self.attribute_keys(),
            model: self.model(),
            protocol: self.rules.protocol_spec(),
        };

        self.phase(PhaseStage::StoryThinking, None);
        // 每回合取一次当前 provider：配置热替换后下一个回合立即生效。
        let ai = self.ai.read().expect("ai poisoned").clone();
        // 导演通道：人代替 GM 推进剧情，事件归「故事本身」；本回合不额外扮演 NPC。
        let is_gm = ctx.channel == RoundChannel::Gm;
        let story_fallback = if is_gm { Some(story_actor()) } else { self.controlled_actor() };
        // 单一 AI 同时扮演 NPC：未署名 actor_id 的 speak / emote 回落到首个在场 NPC，
        // 而不是玩家角色（玩家的话由玩家自己输入）。narrate 等仍回落到阶段默认。
        let npc_fallback = self.present_npcs().into_iter().next();
        // 输入里点名的武器（伤害/命中加值）：AI 漏给 skill_id 时也要用上
        let actor_key = self.controlled_actor().map(|a| a.id.clone()).unwrap_or_default();
        let text_choice = self.attack_choice_in_text(&input.text, &actor_key);
        let mut struck = false;
        // #04 ⑨ 回合内 intent_id 幂等集合：主线 + 角色两个阶段的重复意图都只结算一次。
        // 缺省 / 空 intent_id 不入集合，保持旧模型「同一意图出现两次照常结算」的行为。
        let mut seen_intent_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        // #04 ⑦：回合内最多 MAX_STORY_AI_ROUNDS 轮主线 AI 工具调用。
        // 第一轮 `turn_feedback` 为空、上下文与旧单轮路径逐字一致；只有模型自己触发了
        // query_world / check / interact 且引擎产出了新结果，才追加一轮把结果回喂。
        // 模型输出 finish_turn、没有新结果可回喂、或达到轮次上限时收束。
        let mut turn_feedback: Option<Vec<String>> = None;
        for _story_round in 0..MAX_STORY_AI_ROUNDS {
            // 后续轮只替换「本轮工具结果」字段；其余上下文照旧，模型才能据实续写合法意图。
            let call_ctx = match turn_feedback.take() {
                Some(feedback) => {
                    let mut follow = ctx.clone();
                    follow.turn_feedback = feedback;
                    follow
                }
                None => ctx.clone(),
            };
            // 主线 AI 调用：成功带完整轨迹（ai_call 事件，含发给模型的完整上下文）；
            // 失败也发一条 error 状态的 ai_call 留痕（pi 式 span 的 status），再向上抛错。
            let out = match ai.story_intents(&call_ctx).await {
                Ok(out) => out,
                // 玩家按了停止：这一回合干净收尾（System 事件 + RoundEnd），不产生任何叙事，
                // 也不把「取消」当失败往上抛——否则前端会一直卡在「思考中」。
                Err(EngineError::Cancelled) => {
                    tracing::info!(save_id = %ctx.save_id, round = ctx.round, "本回合的 AI 调用已被玩家取消");
                    self.phase(PhaseStage::Idle, None);
                    self.emit_simple(PlayEvent::System(SystemPayload {
                        level: SystemLevel::Info,
                        code: Some("round_cancelled".to_string()),
                        text: "已停止本回合的 AI 推理（未产生叙事，可以重新发送）".to_string(),
                    }));
                    self.emit_simple(PlayEvent::RoundEnd(RoundEndPayload { round }));
                    return Ok(());
                }
                Err(e) => {
                    let model = call_ctx.model.as_ref();
                    self.emit_simple(PlayEvent::AiCall(AiCallPayload {
                        stage: "story_thinking".to_string(),
                        provider: model
                            .map(|m| m.provider_id.clone())
                            .unwrap_or_default(),
                        model: model.map(|m| m.model.clone()).unwrap_or_default(),
                        temperature: 0.0,
                        max_tokens: 0,
                        messages: Vec::new(),
                        reasoning: None,
                        usage: AiCallUsage::default(),
                        latency_ms: 0,
                        attempts: 1,
                        status: AiCallStatus::Error,
                        error: Some(e.to_string()),
                        intents: Vec::new(),
                        warnings: Vec::new(),
                    }));
                    return Err(e);
                }
            };
            if let Some(trace) = out.trace.clone() {
                self.emit_simple(PlayEvent::AiCall(trace));
            }
            // 自动上下文压缩只重写派生 surface：给日志留一条痕，玩家看得到发生过什么。
            if let Some(report) = out.compaction.clone() {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("context_compacted".to_string()),
                    text: format!(
                        "上下文已压缩（{}）：把最早 {} 个回合的原文换成摘要（{} → {} 字符）；叙事与存档不受影响",
                        report.trigger, report.shadowed_rounds, report.chars_before, report.chars_after
                    ),
                }));
            }
            if let Some(reasoning) = out.reasoning.clone() {
                self.emit_reasoning("story_thinking", reasoning);
            }
            self.emit_intent_warnings(&out.intent_warnings);
            let mut finished = false;
            let mut next_feedback: Vec<String> = Vec::new();
            for envelope in out.intents {
                if !register_intent_id(&mut seen_intent_ids, &envelope.intent_id) {
                    continue;
                }
                let intent = envelope.intent;
                // finish_turn 是循环终止信号：不算意图、不校验、不进日志（#04 修订）。
                if matches!(intent, Intent::FinishTurn) {
                    finished = true;
                    continue;
                }
                // #04 ④：显式 actor_id 必须指向在场且合法的角色；否则驳回，绝不静默改判给他人。
                if let Some((bad, code)) = self.invalid_explicit_actor(&intent) {
                    let message = match code {
                        RejectionCode::ActorNotControlled => {
                            format!("不能以玩家受控角色「{bad}」的身份代说台词")
                        }
                        _ => format!("意图指定的行动者不在场或不存在：{bad}"),
                    };
                    self.reject(message, code);
                    continue;
                }
                // 归属优先级：意图自带的 actor_id → 台词里提到的角色 → 阶段默认。
                // narrate 等默认归玩家角色（是你在行动）；导演回合归故事本身；
                // 未署名的 speak / emote 归首个在场 NPC（AI 扮演的才是 NPC）。
                let actor = self.resolve_intent_actor(&intent).or_else(|| match &intent {
                    Intent::Speak { .. } | Intent::Emote { .. } => npc_fallback.clone(),
                    _ => story_fallback.clone(),
                });
                let rejections_before = self.rejection_count.load(Ordering::SeqCst);
                // 时序闸门（#GAP-I）：机械意图的归属与预算。放在最前，被驳回也进回喂。
                if let Err((code, message)) = self.turn_gate(&intent, actor.as_ref()) {
                    self.reject(message, code);
                } else if let Intent::Strike { enemy_id, skill_id } = intent {
                    struck = true;
                    let mut choice = text_choice.clone();
                    if skill_id.is_some() {
                        choice.skill_id = skill_id;
                    }
                    self.strike_enemy(enemy_id, choice, actor).await;
                } else if let Some(info) = self.handle_intent(intent, actor).await {
                    // 只有 query_world / check / interact 会返回「新引擎信息」供下一轮回喂。
                    next_feedback.push(info);
                }
                // 驳回同样要回喂：驳回不进叙事（前端当引擎 QA 信息过滤），模型本来完全
                // 看不到「技能在冷却 / 资源不足」，会照自己的想象把失败写成成功。
                if self.rejection_count.load(Ordering::SeqCst) > rejections_before {
                    if let Some(reason) =
                        self.last_rejection.lock().expect("rejection poisoned").clone()
                    {
                        next_feedback.push(format!("引擎驳回了一条意图：{reason}"));
                    }
                }
            }
            if finished || next_feedback.is_empty() {
                break;
            }
            turn_feedback = Some(next_feedback);
        }
        // 兜底：玩家明确攻击了遭遇中的敌人，但 AI 没给 strike → 引擎自己结算一次。
        // 战斗必须有权威结果，不能取决于模型这次记不记得用 strike。
        if !is_gm && !struck {
            if let Some(enemy_id) = self.enemy_attacked_in(&input.text) {
                let actor = self.controlled_actor();
                let key = actor.as_ref().map(|a| a.id.clone()).unwrap_or_default();
                let choice = self.attack_choice_in_text(&input.text, &key);
                self.strike_enemy(enemy_id, choice, actor).await;
            }
        }

        if is_gm {
            self.evaluate_turn_end();
            self.emit_simple(PlayEvent::RoundEnd(RoundEndPayload { round }));
            return Ok(());
        }

        // 回合末骨架求值（#13）：标记新达成目标 / 新触发触发点，提示主线 AI，不自动切场景。
        self.evaluate_turn_end();

        self.emit_simple(PlayEvent::RoundEnd(RoundEndPayload { round }));
        Ok(())
    }

    /// 构造条件求值的只读世界快照（与回合末骨架求值同一范式）：
    /// 受控角色视角的 flags / 目标 / 触发点 / 属性 / 位置 / 关系 + Lua 宿主。
    /// `script_id` 区分调用来源（turn_end / narrative），供 Lua 上下文标识。
    fn with_eval_context<R>(&self, script_id: &str, f: impl FnOnce(&EvalContext<'_>) -> R) -> R {
        let (flags, goals, triggers, actor_attrs, actor_loc, scene_id, encounters) = {
            let st = self.state.lock().expect("state poisoned");
            // 条件求值里的 Lua 也用同一批只读事实（GAP-A）：跑之前刷新，避免读到旧快照。
            self.refresh_lua_world_facts(&st);
            let actor_id = st.controlled.first().cloned().unwrap_or_default();
            let actor = st.characters.get(&actor_id);
            (
                st.flags.clone(),
                st.progress.goals.clone(),
                st.progress.triggers.clone(),
                actor.map(|c| c.attributes.clone()),
                actor.and_then(|c| c.location_id.clone()),
                st.scene_id.clone(),
                st.encounters.values().cloned().collect::<Vec<Value>>(),
            )
        };
        let relationships = self.rules.relationships();

        let mut actor_obj = serde_json::Map::new();
        if let Some(attrs) = &actor_attrs {
            actor_obj.insert("attributes".to_string(), Value::Object(attrs.clone()));
        }
        if let Some(loc) = &actor_loc {
            actor_obj.insert("location_id".to_string(), Value::String(loc.clone()));
        }
        let lua_ctx = LuaHostContext {
            script_id: script_id.into(),
            actor: Value::Object(actor_obj),
            round: self.round.load(Ordering::SeqCst),
            relationships: relationships.clone(),
            ..Default::default()
        };

        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: actor_loc.as_deref(),
            actor_attributes: actor_attrs.as_ref(),
            relationships: &relationships,
            scene_id: Some(scene_id.as_str()),
            encounters: &encounters,
            lua: Some((&self.lua, &lua_ctx)),
        };

        f(&ctx)
    }

    /// 回合末对全部 goal / trigger 求值，把新进展落成权威 StateUpdate 事件。
    ///
    /// 触发点进度写的是**完整变化集**（`trigger_progress`）：一次性触发与历史一样落
    /// `true`，可重复触发落 `{fired, active}`——后者带上了「上一次条件值」，边沿检测
    /// 才能在重放（同一条日志 → 同一份进度）下得出同一个结论。
    fn evaluate_turn_end(&self) {
        // 时序激活 / 退出（#GAP-I）：放在最前，保证即使骨架没有任何进展也会同步。
        self.sync_turn_combat();
        let skeleton = self.rules.skeleton().cloned().unwrap_or(Value::Null);
        match self.with_eval_context("turn_end", |ctx| evaluate_skeleton_full(&skeleton, ctx)) {
            Ok(out) => {
                if out.goals.is_empty() && out.trigger_progress.is_empty() {
                    return;
                }
                let mut changes = Vec::new();
                for id in &out.goals {
                    changes.push(goal_delta(id));
                }
                for (id, value) in &out.trigger_progress {
                    changes.push(trigger_progress_delta(id, value.clone()));
                }
                self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
                // 触发点预置遭遇（地图 P5 §6.3）：标记 fired 之后自动建遭遇，
                // 走 Intent::Encounter 的同一条创建路径（含图鉴实例克隆与地点继承）。
                // 可重复触发在每次边沿都会重新走这条路（遭遇可再次出现）。
                self.spawn_trigger_encounters(&out.triggers);
                let mut parts = Vec::new();
                if !out.goals.is_empty() {
                    parts.push(format!("目标达成：{}", out.goals.join("、")));
                }
                if !out.triggers.is_empty() {
                    parts.push(format!("剧情触发：{}", out.triggers.join("、")));
                }
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("skeleton_progress".into()),
                    text: parts.join("；"),
                }));
            }
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("condition_error".into()),
                    text: e.to_string(),
                }));
            }
        }
    }

    /// 建一场遭遇：导演即兴（Intent::Encounter）与触发点预置遭遇（地图 P5 §6.3）
    /// **同一条创建路径**——图鉴实例克隆、地点继承、叙事锚快照都在这里。
    ///
    /// location_override：预置显式声明的地点；None = 继承当前场景的 location_id。
    fn create_encounter(
        &self,
        name: String,
        enemies: Vec<EnemySpec>,
        note: Option<String>,
        location_override: Option<String>,
    ) {
        // 3a：只建结构与 HP；先攻与回合限制见后续。
        // 图鉴 M2 §4.1：带 template_id 的敌人按模板**克隆实例**（同一模板的多场
        // 遭遇天然隔离：实例键带遭遇 id）；命中不了模板就保持临时敌人（现状）。
        let id = format!("enc-{}", uuid::Uuid::new_v4().simple());
        let scene_id = self.state.lock().expect("state poisoned").scene_id.clone();
        // 遭遇地点（地图 P2）：预置地点优先，缺省继承当前场景声明的地点；
        // 怪物实例也按它归位。
        let location_id = location_override
            .or_else(|| self.scene_def(&scene_id).and_then(|s| s.location_id));
        // 叙事锚（地图 P5 §6.4）：**创建时快照**，绝不查询时推导——运行时事件必须可重放，
        // 「此刻的场景」在重放时会得到错误答案。
        let goal_id = self
            .rules
            .skeleton()
            .and_then(|skeleton| scene_encounter_goal_id(skeleton, &scene_id));
        let mut list: Vec<EnemyView> = Vec::new();
        let mut template_ids: Vec<String> = Vec::new();
        let mut changes: Vec<StateDelta> = Vec::new();
        for spec in enemies {
            let Some(template) = spec
                .template_id
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .and_then(|t| self.rules.monster_template(t))
            else {
                // 临时敌人（AI 现编 / 旧日志 / 模板失配）：行为与改动前逐字一致。
                let hp = spec.hp.filter(|h| *h > 0).unwrap_or(10);
                let ac = spec.ac.filter(|a| *a > 0).unwrap_or(12);
                list.push(EnemyView {
                    id: format!("e{}", list.len() + 1),
                    name: spec.name,
                    hp,
                    max: hp,
                    ac,
                    ..Default::default()
                });
                continue;
            };
            let template_id = template
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = if spec.name.trim().is_empty() {
                template
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&template_id)
                    .to_string()
            } else {
                spec.name.clone()
            };
            let attributes = template
                .get("attributes")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut resources = template
                .get("resources")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            // 快照满值：spec.hp 显式给值时才覆盖模板的生命值。
            let vital = vital_resource_key(&resources).unwrap_or_else(|| "hp".to_string());
            let hp = spec
                .hp
                .filter(|h| *h > 0)
                .or_else(|| resources.get(&vital).and_then(Value::as_i64))
                .unwrap_or(10);
            resources.insert(vital.clone(), Value::from(hp));
            // AC（图鉴 M2 §4.4）：派生值 derived.ac（LMoP 是 10 + dex_mod + 挂接护甲）。
            let probe = CharacterInstance {
                instance_id: String::new(),
                template_id: template_id.clone(),
                name: name.clone(),
                kind: "monster".into(),
                attributes: attributes.clone(),
                resources: resources.clone(),
                inventory: Default::default(),
                location_id: location_id.clone(),
                present: true,
                statuses: vec![],
            };
            let ac = self
                .derived_ac(&probe)
                .or_else(|| spec.ac.filter(|a| *a > 0))
                .unwrap_or(12);
            let attacks = self.enemy_attacks(&template, spec.skill_id.as_deref());
            // 涉及的图鉴条目（§6.4）：去重、按创建顺序；临时敌人不计入。
            if !template_ids.contains(&template_id) {
                template_ids.push(template_id.clone());
            }
            // 护栏：单条 spec 最多展开 MAX_ENCOUNTER_UNITS 只。
            let count = spec.count.unwrap_or(1).clamp(1, MAX_ENCOUNTER_UNITS);
            for n in 1..=count {
                let key = format!("enc-{id}:{template_id}#{n}");
                let instance = CharacterInstance {
                    instance_id: key.clone(),
                    template_id: template_id.clone(),
                    name: name.clone(),
                    kind: "monster".into(),
                    attributes: attributes.clone(),
                    resources: resources.clone(),
                    inventory: Default::default(),
                    location_id: location_id.clone(),
                    present: true,
                    statuses: vec![],
                };
                changes.push(StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: key.clone(),
                    field: "instance".into(),
                    op: DeltaOp::Add,
                    value: serde_json::to_value(&instance).unwrap_or(Value::Null),
                });
                list.push(EnemyView {
                    id: format!("e{}", list.len() + 1),
                    name: name.clone(),
                    hp,
                    max: hp,
                    ac,
                    instance_id: Some(key),
                    template_id: Some(template_id.clone()),
                    scene_id: Some(scene_id.clone()),
                    location_id: location_id.clone(),
                    attacks: attacks.clone(),
                });
            }
        }
        let view = EncounterView {
            id: id.clone(),
            name,
            enemies: list,
            note,
            active: true,
            scene_id: Some(scene_id),
            location_id,
            goal_id,
            template_ids,
        };
        changes.push(StateDelta {
            domain: DeltaDomain::Encounter,
            entity_id: id,
            field: "encounter".into(),
            op: DeltaOp::Add,
            value: serde_json::to_value(view).unwrap_or(Value::Null),
        });
        self.emit(
            PlayEvent::StateUpdate(StateUpdatePayload { changes }),
            Some(story_actor()),
            None,
        );
        // 遭遇建好即进入时序（#GAP-I）：排先攻、建预算。未声明 world.turn 时静默。
        self.sync_turn_combat();
    }

    /// 触发点被标记 fired 之后自动建遭遇（地图 P5 §6.3）。
    ///
    /// 预置遭遇的创建路径与 Intent::Encounter **完全同一条**（见 create_encounter）；
    /// 引擎在这里只读触发点的 condition 结果与 encounter 预置。
    ///
    /// 掷表遭遇不在这里：表是内容 / 规则集语义，由 Lua 掷骰（engine_rng）→ set_flag →
    /// 触发点 condition: flag_set + encounter 预置表达，引擎不认识「遭遇表」这个东西
    /// （docs/rules-via-lua.md §6）。
    fn spawn_trigger_encounters(&self, trigger_ids: &[String]) {
        if trigger_ids.is_empty() {
            return;
        }
        let skeleton = self.rules.skeleton().cloned().unwrap_or(Value::Null);
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for trigger_id in trigger_ids {
            // 同一 id 写进多个场景时只建一场（求值侧按 id 去重不了）。
            if !seen.insert(trigger_id.as_str()) {
                continue;
            }
            let Some(plan) = trigger_encounter_preset(&skeleton, trigger_id) else {
                continue;
            };
            let TriggerEncounterPlan { preset, title } = plan;
            if preset.enemies.is_empty() {
                continue;
            }
            let name = preset
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or(title)
                .unwrap_or_else(|| "遭遇".to_string());
            let location_override = preset
                .location_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let enemies: Vec<EnemySpec> = preset
                .enemies
                .iter()
                .map(|e| EnemySpec {
                    // 空名 = 用图鉴条目的名字（与 AI 只给 template_id 时同一口径）。
                    name: String::new(),
                    hp: None,
                    ac: None,
                    template_id: Some(e.template_id.trim().to_string()),
                    count: e.count,
                    skill_id: None,
                })
                .collect();
            self.create_encounter(name, enemies, preset.note, location_override);
        }
    }

    /// 章节的空间范围（地图 P5 §6.2 / §6.7）：其 scenes 的 location_id **并集**。
    ///
    /// **推导，不存冗余字段**——存一份就一定会与场景定义不同步；地图按它高亮「本章范围」。
    pub fn chapter_locations(&self) -> Vec<ChapterLocations> {
        self.rules
            .skeleton()
            .map(chapter_locations)
            .unwrap_or_default()
    }

    /// 切换到骨架里的目标场景：按**三层在场优先级**重算在场名单
    /// （① 作者点名 present_char_ids > ② 实例 location_id == 场景 location_id >
    /// ③ 两者都未声明则全员在场；受控角色恒在场），并以 Scene 事件记录场景元信息。
    ///
    /// 判据与开档共用 `scene_presence`：这里修掉了旧行为的不一致——旧代码把
    /// 「未声明 present_char_ids」直接当成空名单，一切场就把所有人清空（开档却是全员在场）。
    fn switch_scene(&self, scene_id: &str) {
        let Some(def) = self.scene_def(scene_id) else {
            return;
        };
        let (present, deltas) = {
            let st = self.state.lock().expect("state poisoned");
            let mut present = Vec::new();
            let mut deltas = Vec::new();
            for (key, c) in &st.characters {
                let controlled =
                    st.controlled.contains(key) || st.controlled.contains(&c.instance_id);
                let on = scene_presence(
                    def.present_char_ids.as_ref(),
                    def.location_id.as_deref(),
                    &c.template_id,
                    c.location_id.as_deref(),
                    controlled,
                );
                if on {
                    present.push(key.clone());
                }
                if c.present != on {
                    deltas.push(StateDelta {
                        domain: DeltaDomain::Character,
                        entity_id: key.clone(),
                        field: "present".into(),
                        op: DeltaOp::Set,
                        value: Value::Bool(on),
                    });
                }
            }
            (present, deltas)
        };
        if !deltas.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes: deltas }));
        }
        self.emit_simple(PlayEvent::Scene(ScenePayload {
            scene_id: scene_id.to_string(),
            title: def.title,
            description: def.description,
            present,
        }));
    }

    /// 从骨架取一个场景定义（标题 / 描述 / 在场名单 / 地点）——切场判据与提示词共用。
    ///
    /// present_char_ids 与 location_id 都保留 Option：**未声明**（None）与
    /// **显式空名单**（Some(空集)）在 scene_presence 里语义不同，不可合并。
    fn scene_def(&self, scene_id: &str) -> Option<SceneDef> {
        let chapters = self.rules.skeleton()?.as_array()?;
        for chapter in chapters {
            let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
                continue;
            };
            for scene in scenes {
                if scene.get("id").and_then(Value::as_str) != Some(scene_id) {
                    continue;
                }
                let title = scene
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(scene_id)
                    .to_string();
                let description = scene
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let present_char_ids =
                    scene.get("present_char_ids").and_then(Value::as_array).map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<std::collections::HashSet<String>>()
                    });
                let location_id = scene
                    .get("location_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                return Some(SceneDef { title, description, present_char_ids, location_id });
            }
        }
        None
    }

    /// 当前场景所在地点的**显示名**（骨架 scenes[].location_id → world.locations[].name）：
    /// 只喂回合提示词的 TurnContext.location（用户消息），不进系统 preamble。
    /// 场景没声明地点、或场景不在骨架里 → None；地点表里查不到该 id 时回落 id 本身
    /// （宁肯告诉模型一个未登记的地点，也不要让它以为自己在虚空里）。
    fn current_location(&self, scene_id: &str) -> Option<String> {
        let location_id = self.scene_def(scene_id)?.location_id?;
        let st = self.state.lock().ok()?;
        let named = st.locations.iter().find_map(|l| {
            if l.get("id").and_then(Value::as_str) != Some(location_id.as_str()) {
                return None;
            }
            l.get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
        Some(named.unwrap_or(location_id))
    }

    /// 合法判定属性 key：属性维度 + world.check.attributes（去重、稳定排序）。
    fn attribute_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.rules.profiles().keys().cloned().collect();
        if let Some(list) = self.rules.global_checker().and_then(|c| c.attributes) {
            for k in list {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        keys.sort();
        keys
    }

    /// 骨架里全部场景摘要（含当前场景）：主线 AI 只能在这些 id 里选 advance_scene 目标。
    fn scene_briefs(&self) -> Vec<SceneBrief> {
        let mut out = Vec::new();
        let Some(chapters) = self.rules.skeleton().and_then(Value::as_array) else {
            return out;
        };
        for chapter in chapters {
            let chapter_title = chapter.get("title").and_then(Value::as_str).unwrap_or("");
            let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
                continue;
            };
            for scene in scenes {
                let Some(id) = scene.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                    continue;
                };
                out.push(SceneBrief {
                    id: id.to_string(),
                    title: scene
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    chapter: chapter_title.to_string(),
                });
            }
        }
        out
    }

    fn handle_meta(&self, text: &str) {
        if text.contains("免确认") {
            let v = !self.auto_confirm.load(Ordering::SeqCst);
            self.set_auto_confirm(v);
        } else if text.contains("帮助") || text.contains("help") {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: None,
                text: "输入你想做的事；元指令：/存档 /免确认 /帮助".into(),
            }));
        } else if text.contains("存档") {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: Some("saved".into()),
                text: "已手动存档".into(),
            }));
        } else {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Warn,
                code: None,
                text: format!("未知元指令：{text}"),
            }));
        }
    }

    /// 玩家文本里是否明确攻击了遭遇中的某个敌人（AI 漏给 strike 时兜底）。
    /// 需要同时满足：出现攻击动词 + 出现某个存活敌人的名字（允许简称包含）。
    fn enemy_attacked_in(&self, text: &str) -> Option<String> {
        const VERBS: [&str; 12] = [
            "攻击", "砍", "刺", "射", "打", "劈", "戳", "砸", "杀", "袭", "冲向", "斩",
        ];
        if !VERBS.iter().any(|v| text.contains(v)) {
            return None;
        }
        let st = self.state.lock().ok()?;
        let mut best: Option<(usize, String)> = None;
        for enc in st.encounters.values() {
            if !enc.get("active").and_then(Value::as_bool).unwrap_or(true) {
                continue;
            }
            let Some(list) = enc.get("enemies").and_then(Value::as_array) else { continue };
            for e in list {
                if e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0 {
                    continue;
                }
                let name = e.get("name").and_then(Value::as_str).unwrap_or("");
                let id = e.get("id").and_then(Value::as_str).unwrap_or("");
                if name.is_empty() || id.is_empty() {
                    continue;
                }
                // 全长匹配，或名字的某个子串出现在输入里（「砍头狼」↔「灰狼头狼」）
                let chars: Vec<char> = name.chars().collect();
                let mut hit = 0usize;
                for start in 0..chars.len() {
                    for end in (start + 2)..=chars.len() {
                        let piece: String = chars[start..end].iter().collect();
                        if text.contains(&piece) && piece.chars().count() > hit {
                            hit = piece.chars().count();
                        }
                    }
                }
                if hit > 0 && best.as_ref().is_none_or(|(l, _)| hit > *l) {
                    best = Some((hit, id.to_string()));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// 输入里提到的武器/技能（取最长匹配）——兜底攻击时用它，避免「抡长棍」被当成徒手。
    /// 优先玩家**手里**的武器（物品名 → 它挂的技能），其次同名技能。
    fn attack_choice_in_text(&self, text: &str, actor_key: &str) -> AttackChoice {
        let sb = &self.rules.storybook;
        let skills = sb.get("skills").and_then(Value::as_array).cloned().unwrap_or_default();
        let items = sb.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        let inventory: Vec<String> = {
            let Ok(st) = self.state.lock() else { return AttackChoice::default() };
            st.characters
                .get(actor_key)
                .or_else(|| st.characters.values().find(|c| c.template_id == actor_key))
                .map(|c| c.inventory.keys().cloned().collect())
                .unwrap_or_default()
        };
        // 名字出现在输入里的最长片段长度（≥1 字）
        let match_len = |name: &str| -> usize {
            let chars: Vec<char> = name.chars().collect();
            let mut best = 0usize;
            for start in 0..chars.len() {
                for end in (start + 1)..=chars.len() {
                    let piece: String = chars[start..end].iter().collect();
                    if text.contains(&piece) {
                        best = best.max(piece.chars().count());
                    }
                }
            }
            best
        };
        let mut best: Option<(usize, AttackChoice)> = None;
        // ① 手里的武器：优先带技能的；没有技能就用物品自带的伤害骰与命中加值
        for it in &items {
            let id = it.get("id").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() || !inventory.iter().any(|x| x == id) {
                continue;
            }
            let len = match_len(it.get("name").and_then(Value::as_str).unwrap_or(""));
            if len == 0 {
                continue;
            }
            let skill_id = it
                .get("skills")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .map(str::to_string);
            let damage = it.get("damage").and_then(Value::as_str).map(str::to_string);
            let bonus: i64 = it
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter(|m| {
                            m.get("target").and_then(Value::as_str) == Some("attack")
                                && m.get("op").and_then(Value::as_str).unwrap_or("add") == "add"
                        })
                        .filter_map(|m| m.get("value").and_then(Value::as_i64))
                        .sum()
                })
                .unwrap_or(0);
            if skill_id.is_none() && damage.is_none() && bonus == 0 {
                continue;
            }
            let label = it.get("name").and_then(Value::as_str).map(str::to_string);
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((len, AttackChoice { skill_id, damage, bonus, label }));
            }
        }
        // ② 技能名本身
        for s in &skills {
            let id = s.get("id").and_then(Value::as_str).unwrap_or("");
            let name = s.get("name").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let len = match_len(name);
            if len > 0 && best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((
                    len,
                    AttackChoice {
                        skill_id: Some(id.to_string()),
                        damage: None,
                        bonus: 0,
                        label: None,
                    },
                ));
            }
        }
        best.map(|(_, c)| c).unwrap_or_default()
    }

    /// 结算一次对遭遇内敌人的攻击：**命中与伤害全由引擎算**，AI 只叙事。
    ///
    /// 判定与效果走 `command::execute_skill`（与 use_skill / use_item 同一内核，判定 C2）：
    /// - 判定器：技能内联 → 引用全局 world.check → 合成徒手 1d20（修 D3，不再静默换骰）
    /// - Lua 判定器与 Lua 挂载点走同一份实现（修 D2）
    /// - 属性修正来源 = 挂接定义 + 已装备物品（修 D5，与技能同口径）
    /// - 难度：目标 AC → world.check.default_dc → 12（修 D6）
    /// - 命中判据用内核的 resolved.result（尊重 mode），不再丢弃档位
    async fn strike_enemy(&self, enemy_id: String, choice: AttackChoice, actor: Option<ActorRef>) {
        let actor = actor
            .or_else(|| self.controlled_actor())
            .unwrap_or(ActorRef { id: String::new(), name: "未知角色".into() });

        // 1) 找遭遇与敌人（enemy_id 认条目 id、名字，也认背后的实例键）
        let Some((enc_id, enc, idx)) = self.find_encounter_enemy(&enemy_id) else {
            self.reject(
                format!("当前遭遇里没有这个敌人：{enemy_id}"),
                RejectionCode::TargetInvalid,
            );
            return;
        };
        let enemy_name = enc
            .pointer(&format!("/enemies/{idx}/name"))
            .and_then(Value::as_str)
            .unwrap_or("敌人")
            .to_string();
        let enemy_json = enc.pointer(&format!("/enemies/{idx}")).cloned().unwrap_or(Value::Null);
        let global_checker = self.rules.global_checker();

        // 2) 守方（图鉴 M2 §4.1/§4.3）：有 instance_id 的图鉴怪 → 真实例（权威 HP + 派生 AC）；
        //    没有的临时敌人 → 条目自身是权威，键沿用调用方给的 id（旧路径逐字不变）。
        let instance = self.enemy_instance(&enc, idx);
        let instance_backed = instance.is_some();
        let (defender_key, defender_json, vital, hp_before, derived_ac) = match instance {
            Some((key, c)) => {
                let vital = vital_resource_key(&c.resources).unwrap_or_else(|| "hp".to_string());
                let entry_hp = enc
                    .pointer(&format!("/enemies/{idx}/hp"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let hp = c.resources.get(&vital).and_then(resource_num).unwrap_or(entry_hp);
                let ac = self.derived_ac(&c);
                (
                    key,
                    serde_json::to_value(&c).unwrap_or(Value::Null),
                    vital,
                    hp,
                    ac,
                )
            }
            None => (
                enemy_id.clone(),
                enemy_json.clone(),
                "hp".to_string(),
                enc.pointer(&format!("/enemies/{idx}/hp"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                None,
            ),
        };
        // 难度（修 D6 + 图鉴 M2 §4.4）：实例派生 ac → 条目 ac → world.check.default_dc → 12。
        let difficulty = derived_ac
            .or_else(|| {
                enc.pointer(&format!("/enemies/{idx}/ac"))
                    .and_then(Value::as_i64)
                    .filter(|a| *a > 0)
            })
            .or_else(|| global_checker.as_ref().and_then(|c| c.default_dc))
            .unwrap_or(12)
            .max(0);

        // 3) 组装本次攻击的技能：AI 指明 / 文本命中的技能优先，缺省合成徒手攻击。
        let declared = choice.skill_id.as_deref().and_then(|id| self.rules.skill(id)).cloned();
        let skill_name = declared
            .as_ref()
            .map(|s| s.name.clone())
            .or_else(|| choice.label.clone())
            .unwrap_or_else(|| "徒手攻击".into());
        let damage_expr = {
            let declared_expr = attack_damage_expr(declared.as_ref());
            if declared_expr.is_empty() {
                choice.damage.clone().unwrap_or_else(|| "1d6".to_string())
            } else {
                declared_expr
            }
        };
        // 只有「生命资源不叫 hp」的图鉴怪才显式指定资源：临时敌人传 None，
        // 合成的技能与改动前逐字一致（effects 缺省回落 hp）。
        let damage_resource = (instance_backed && vital != "hp").then(|| vital.as_str());
        let skill = attack_skill(
            declared.as_ref(),
            global_checker.as_ref(),
            &skill_name,
            &damage_expr,
            damage_resource,
        );

        // 3b) 攻方实例与场景：任意 id 形态（实例键 / 模板 id / 名字 / instance_id）都能定位。
        let (actor_key, actor_json, scene_id) = {
            let st = self.state.lock().expect("state poisoned");
            let scene = st.scene_id.clone();
            let hit = if actor.id.is_empty() {
                None
            } else {
                Self::find_character(&st.characters, &actor.id)
            };
            match hit {
                Some((key, c)) => (key.clone(), serde_json::to_value(c).unwrap_or(Value::Null), scene),
                None => (actor.id.clone(), Value::Null, scene),
            }
        };

        // 冷却门（#01）：攻击技能同样受 cooldown 约束，与 use_skill 同口径。
        if let Some(remaining) = self.cooldown_block(&actor_key, &skill) {
            self.reject(
                format!("「{skill_name}」还在冷却中（剩余 {remaining} 回合）"),
                RejectionCode::CooldownActive,
            );
            return;
        }

        // 4) 交给内核结算（对称内核：与 enemy_strike 同一份实现）。
        let lua_ctx = LuaHostContext {
            script_id: format!("strike:{}", skill.id),
            actor_id: actor_key.clone(),
            actor: actor_json.clone(),
            target_id: Some(defender_key.clone()),
            target: Some(defender_json.clone()),
            skill: serde_json::to_value(&skill).ok(),
            scene_id,
            round: self.round.load(Ordering::SeqCst),
            difficulty: Some(difficulty),
            // 时序只读快照（#GAP-I）：规则包在挂载点读 host.turn 判断「何时该失去回合」。
            turn: self.turn_snapshot(),
            relationships: vec![],
            present: vec![],
            controlled: String::new(),
        };
        let settled = self.resolve_attack(AttackSetup {
            attacker_key: &actor_key,
            attacker_json: &actor_json,
            target_id: Some(defender_key.as_str()),
            target_json: Some(&defender_json),
            skill: &skill,
            difficulty,
            attribute: None,
            extra_bonus: choice.bonus,
            global_checker: global_checker.clone(),
            lua_ctx,
        });
        let outcome = match settled {
            Ok(o) => o,
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("lua_error".into()),
                    text: e.to_string(),
                }));
                return;
            }
        };
        if let Some(code) = outcome.rejection {
            self.reject(format!("攻击「{skill_name}」被驳回"), code);
            return;
        }

        // 5) 命中与伤害：命中判据取内核的 resolved.result（尊重 mode，不再丢弃档位）。
        let hit = outcome.check.as_ref().map(|c| c.result).unwrap_or(true);
        let (die, total, r#mod) = match outcome.check.as_ref() {
            Some(c) => (c.rolls.first().copied().unwrap_or(0), c.total, c.r#mod),
            None => (0, 0, 0),
        };
        let hp_field = format!("resources.{vital}");
        let harm = outcome
            .effects
            .deltas
            .iter()
            .filter(|d| {
                d.domain == DeltaDomain::Character
                    && d.entity_id == defender_key
                    && d.field == hp_field
            })
            .map(|d| -d.value.as_i64().unwrap_or(0))
            .sum::<i64>()
            .max(0);
        let harm = if hit { harm.max(1) } else { 0 };
        let hp_after = (hp_before - harm).max(0);

        // 6) 写回敌人 HP（遭遇条目 = 实例的投影）；非敌人 HP 的效果 delta（消耗等）原样进事件。
        let mut list = enc
            .get("enemies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // 击杀 / 遭遇结束的判据取「这一击**之前**」的事实：对已经倒下的敌人再补刀
        // 不会重复派发事件（每个事件恰好一次），重放同一日志得到同一结论。
        let enemy_down_before = hp_before <= 0;
        let encounter_over_before = enc.get("active").and_then(Value::as_bool) == Some(false)
            || enemies_all_down(&list);
        if let Some(entry) = list.get_mut(idx).and_then(Value::as_object_mut) {
            entry.insert("hp".into(), Value::from(hp_after));
        }
        let all_down = list
            .iter()
            .all(|e| e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0);
        // 通用事实（引擎不解释用途，只把「谁倒下了 / 哪场遭遇结束了」交给脚本）。
        let killed = !enemy_down_before && hp_after <= 0;
        let encounter_over_after = enc.get("active").and_then(Value::as_bool) == Some(false)
            || enemies_all_down(&list);
        let encounter_ended = !encounter_over_before && encounter_over_after;

        let mut state_changes: Vec<StateDelta> = outcome
            .effects
            .deltas
            .iter()
            .filter(|d| {
                !(d.domain == DeltaDomain::Character
                    && d.entity_id == defender_key
                    && d.field == hp_field)
            })
            .cloned()
            .collect();
        if instance_backed {
            // 图鉴怪物的 HP 落在**实例**上（图鉴 M2）：扣血压成一条钳制后的 Set，
            // 与遭遇条目的投影一致，也不会把生命值打成负数。临时敌人没有实例，
            // 旧路径原样剔除（只有遭遇条目被改写）。
            state_changes.push(StateDelta {
                domain: DeltaDomain::Character,
                entity_id: defender_key.clone(),
                field: hp_field,
                op: DeltaOp::Set,
                value: Value::from(hp_after),
            });
        }
        state_changes.push(StateDelta {
            domain: DeltaDomain::Encounter,
            entity_id: enc_id.clone(),
            field: "encounter".into(),
            op: DeltaOp::Set,
            value: serde_json::json!({ "enemies": list }),
        });
        let verdict = if outcome.check.as_ref().is_some_and(|c| c.rolls.is_empty()) {
            format!("命中判定 {total}")
        } else {
            format!("命中判定 {total}（骰 {die} + 修正 {m}）", m = r#mod)
        };
        let narrative = if harm > 0 {
            format!(
                "{actor} 用{skill_name}命中{enemy_name}：{verdict} ≥ {difficulty}，造成 {harm} 点伤害（{before} → {after}）",
                actor = actor.name,
                verdict = verdict,
                difficulty = difficulty,
                before = hp_before,
                after = hp_after,
            )
        } else {
            format!(
                "{actor} 的{skill_name}未命中{enemy_name}：{verdict} < {difficulty}",
                actor = actor.name,
                verdict = verdict,
                difficulty = difficulty,
            )
        };
        // 击杀者事实（供后续事件上下文用；actor 会在 emit 时被移走，先留一份）。
        let killer = ActorRef { id: actor_key.clone(), name: actor.name.clone() };
        // 攻击判定的可观测（判定 C5）：与 use_skill / Intent::Check 同一条 CheckResult
        // 事件——玩家在 UI 看得到骰面 / 修正 / 总值 / 难度，前端 CheckCard 直接复用。
        // 发射顺序与技能判定一致：先 CheckResult，后 Resolution（叙事文案逐字不变）。
        if let Some(check) = &outcome.check {
            self.emit(
                PlayEvent::CheckResult(CheckResultPayload {
                    intent_id: None,
                    actor: actor.clone(),
                    attribute: check.attribute.clone(),
                    expr: check.expr.clone(),
                    rolls: if check.rolls.is_empty() { None } else { Some(check.rolls.clone()) },
                    r#mod: check.r#mod,
                    total: check.total,
                    target: check.target,
                    margin: check.margin,
                    result: check.result,
                    level: check.level,
                    opponent: None,
                    kind: Some(check.kind),
                }),
                Some(actor.clone()),
                None,
            );
        }
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(narrative),
                outcome: Some("strike".into()),
                triggered_events: None,
                state_changes,
            }),
            Some(actor),
            None,
        );
        if all_down {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: Some("encounter_cleared".into()),
                text: "遭遇结束：敌人已全部被击倒。".into(),
            }));
        }
        // Lua 判定器 / 挂载点的写请求（消耗 / 状态 / 事件）真正落到世界状态。
        self.apply_lua_requests(&outcome.requests, &actor_key);

        // 把「敌人被击败 / 遭遇结束」作为**通用事实**派发给 Lua（Event 挂载点）。
        // 放在所有既有事件之后：既有事件的相对顺序逐字不变，这里只追加；
        // 没有声明 event 脚本时 dispatch_lua_event_with 直接返回（零开销、零行为变化）。
        if killed {
            let enemy = enc.pointer(&format!("/enemies/{idx}")).cloned().unwrap_or(Value::Null);
            let mut facts = serde_json::Map::new();
            facts.insert(
                "enemy".into(),
                serde_json::json!({
                    "id": enemy.get("id").cloned().unwrap_or(Value::Null),
                    "name": enemy.get("name").cloned().unwrap_or(Value::Null),
                    "template_id": enemy.get("template_id").cloned().unwrap_or(Value::Null),
                    "instance_id": enemy.get("instance_id").cloned().unwrap_or(Value::Null),
                }),
            );
            facts.insert("encounter".into(), encounter_facts(&enc, &enc_id));
            // 击杀者可选（若有）：无署名时不塞一个空对象，脚本读到的就是 nil。
            if !killer.id.is_empty() || !killer.name.is_empty() {
                facts.insert(
                    "killer".into(),
                    serde_json::json!({ "id": killer.id, "name": killer.name }),
                );
            }
            self.dispatch_lua_event_with("enemy_defeated", Some(&Value::Object(facts)));
        }
        if encounter_ended {
            let facts = serde_json::json!({
                "encounter": encounter_facts(&enc, &enc_id),
                "enemies": list.clone(),
                "reason": if all_down { "all_down" } else { "inactive" },
            });
            self.dispatch_lua_event_with("encounter_cleared", Some(&facts));
        }
    }

    /// 结算一次「遭遇里的敌人攻击某个角色」（图鉴 M2 §4.2 / §4.3）。
    ///
    /// 与 `strike_enemy` 是**同一个内核的两个方向**——两者都经 `resolve_attack` 调用
    /// `command::execute_skill`，不是第 4 个平行实现：
    /// - 命中：怪物自身属性 + 它自己的攻击技能（缺省 = 图鉴条目的第一个攻击技能）；
    /// - 难度：目标（玩家）的**派生 AC**（derived.ac），缺失回落 default_dc → 12；
    /// - 伤害：落到目标实例的 `resources.hp`（走效果 delta，与 use_skill 同一条路径）。
    async fn enemy_strike(
        &self,
        enemy_id: String,
        target_id: Option<String>,
        skill_id: Option<String>,
    ) {
        // 1) 攻方：遭遇条目 → 背后的怪物实例；临时敌人走条目适配器（属性为空）。
        let Some((_enc_id, enc, idx)) = self.find_encounter_enemy(&enemy_id) else {
            self.reject(
                format!("当前遭遇里没有这个敌人：{enemy_id}"),
                RejectionCode::TargetInvalid,
            );
            return;
        };
        let enemy_name = enc
            .pointer(&format!("/enemies/{idx}/name"))
            .and_then(Value::as_str)
            .unwrap_or("敌人")
            .to_string();
        let enemy_json = enc.pointer(&format!("/enemies/{idx}")).cloned().unwrap_or(Value::Null);
        let (attacker_key, attacker_json, attacker_template) = match self.enemy_instance(&enc, idx) {
            Some((key, c)) => {
                let template = c.template_id.clone();
                (key, serde_json::to_value(&c).unwrap_or(Value::Null), template)
            }
            None => (enemy_id.clone(), enemy_json.clone(), String::new()),
        };
        let attacker_ref = ActorRef {
            id: if attacker_template.is_empty() { enemy_id.clone() } else { attacker_template },
            name: enemy_name.clone(),
        };

        // 2) 守方：显式 target_id（任意 id 形态）→ 受控角色。
        let (target_key, target_instance) = {
            let st = self.state.lock().expect("state poisoned");
            let wanted = target_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| st.controlled.first().cloned());
            match wanted.as_deref().and_then(|id| Self::find_character(&st.characters, id)) {
                Some((key, c)) => (key.clone(), c.clone()),
                None => {
                    drop(st);
                    self.reject(
                        format!("找不到要攻击的角色：{}", target_id.unwrap_or_default()),
                        RejectionCode::ActorNotFound,
                    );
                    return;
                }
            }
        };
        let target_json = serde_json::to_value(&target_instance).unwrap_or(Value::Null);
        let target_ref = ActorRef {
            id: target_instance.template_id.clone(),
            name: target_instance.name.clone(),
        };

        // 3) 技能：显式 skill_id → 数据卡第一条攻击（EnemySpec.skill_id 覆盖时它就是唯一一条）
        //    → 合成徒手攻击；判定器 / 属性沿用技能声明，与 use_skill 同口径。
        let global_checker = self.rules.global_checker();
        let default_skill_id = enc
            .pointer(&format!("/enemies/{idx}/attacks/0/skill_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let declared = skill_id
            .as_deref()
            .or(default_skill_id.as_deref())
            .and_then(|id| self.rules.skill(id))
            .cloned();
        let skill_name = declared
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "徒手攻击".into());
        let damage_expr = {
            let declared_expr = attack_damage_expr(declared.as_ref());
            if declared_expr.is_empty() { "1d6".to_string() } else { declared_expr }
        };
        // 伤害落到目标的**生命资源**（图鉴 M2）：目标叫 res-hp 就打到 res-hp，
        // 缺省 hp 与既有的临时敌人路径一致。
        let vital = vital_resource_key(&target_instance.resources).unwrap_or_else(|| "hp".to_string());
        let skill = attack_skill(
            declared.as_ref(),
            global_checker.as_ref(),
            &skill_name,
            &damage_expr,
            Some(&vital),
        );
        let hp_before = target_instance
            .resources
            .get(&vital)
            .and_then(resource_num)
            .unwrap_or(0);
        // 难度（图鉴 M2 §4.4）：目标的派生 AC，缺失 → world.check.default_dc → 12。
        let difficulty = self
            .derived_ac(&target_instance)
            .or_else(|| global_checker.as_ref().and_then(|c| c.default_dc))
            .unwrap_or(12)
            .max(0);

        // 4) 同一内核：Lua 挂载点 → 判定（含 Lua 判定器）→ 命中门 → 效果。
        let scene_id = self.state.lock().expect("state poisoned").scene_id.clone();
        let lua_ctx = LuaHostContext {
            script_id: format!("enemy_strike:{}", skill.id),
            actor_id: attacker_key.clone(),
            actor: attacker_json.clone(),
            target_id: Some(target_key.clone()),
            target: Some(target_json.clone()),
            skill: serde_json::to_value(&skill).ok(),
            scene_id,
            round: self.round.load(Ordering::SeqCst),
            difficulty: Some(difficulty),
            // 时序只读快照（#GAP-I）：规则包在挂载点读 host.turn 判断「何时该失去回合」。
            turn: self.turn_snapshot(),
            relationships: vec![],
            present: vec![],
            controlled: String::new(),
        };
        // 冷却门（#01）：敌人的攻击技能同样受 cooldown 约束（怪物能力也该被量化）。
        if let Some(remaining) = self.cooldown_block(&attacker_key, &skill) {
            self.reject(
                format!("「{skill_name}」还在冷却中（剩余 {remaining} 回合）"),
                RejectionCode::CooldownActive,
            );
            return;
        }
        let settled = self.resolve_attack(AttackSetup {
            attacker_key: &attacker_key,
            attacker_json: &attacker_json,
            target_id: Some(target_key.as_str()),
            target_json: Some(&target_json),
            skill: &skill,
            difficulty,
            attribute: None,
            extra_bonus: 0,
            global_checker: global_checker.clone(),
            lua_ctx,
        });
        let outcome = match settled {
            Ok(o) => o,
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("lua_error".into()),
                    text: e.to_string(),
                }));
                return;
            }
        };
        if let Some(code) = outcome.rejection {
            self.reject(format!("攻击「{skill_name}」被驳回"), code);
            return;
        }

        // 5) 命中与伤害：伤害落到目标实例的 resources.hp（效果 delta 原样进事件）。
        let hit = outcome.check.as_ref().map(|c| c.result).unwrap_or(true);
        let (die, total, r#mod) = match outcome.check.as_ref() {
            Some(c) => (c.rolls.first().copied().unwrap_or(0), c.total, c.r#mod),
            None => (0, 0, 0),
        };
        let hp_field = format!("resources.{vital}");
        let harm = outcome
            .effects
            .deltas
            .iter()
            .filter(|d| {
                d.domain == DeltaDomain::Character && d.entity_id == target_key && d.field == hp_field
            })
            .map(|d| -d.value.as_i64().unwrap_or(0))
            .sum::<i64>()
            .max(0);
        let harm = if hit { harm.max(1) } else { 0 };
        let hp_after = hp_before - harm;
        let verdict = if outcome.check.as_ref().is_some_and(|c| c.rolls.is_empty()) {
            format!("命中判定 {total}")
        } else {
            format!("命中判定 {total}（骰 {die} + 修正 {m}）", m = r#mod)
        };
        let narrative = if harm > 0 {
            format!(
                "{attacker} 的{skill_name}命中{target}：{verdict} ≥ {difficulty}，造成 {harm} 点伤害（{before} → {after}）",
                attacker = enemy_name,
                target = target_ref.name,
                verdict = verdict,
                difficulty = difficulty,
                before = hp_before,
                after = hp_after,
            )
        } else {
            format!(
                "{attacker} 的{skill_name}未命中{target}：{verdict} < {difficulty}",
                attacker = enemy_name,
                target = target_ref.name,
                verdict = verdict,
                difficulty = difficulty,
            )
        };
        // 怪物攻击同样发 CheckResult（判定 C5）：攻守双方共用一条可观测路径，
        // 玩家在防御时也看得到敌人的骰面；署名是攻方（敌人）实例。
        if let Some(check) = &outcome.check {
            self.emit(
                PlayEvent::CheckResult(CheckResultPayload {
                    intent_id: None,
                    actor: attacker_ref.clone(),
                    attribute: check.attribute.clone(),
                    expr: check.expr.clone(),
                    rolls: if check.rolls.is_empty() { None } else { Some(check.rolls.clone()) },
                    r#mod: check.r#mod,
                    total: check.total,
                    target: check.target,
                    margin: check.margin,
                    result: check.result,
                    level: check.level,
                    opponent: None,
                    kind: Some(check.kind),
                }),
                Some(attacker_ref.clone()),
                None,
            );
        }
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(narrative),
                outcome: Some("enemy_strike".into()),
                triggered_events: None,
                // 与 use_skill 同一条路径：效果 delta 原样落状态（damage → resources.hp）。
                state_changes: outcome.effects.deltas.clone(),
            }),
            Some(attacker_ref),
            None,
        );
        // Lua 判定器 / 挂载点的写请求（消耗 / 状态 / 事件）真正落到世界状态。
        self.apply_lua_requests(&outcome.requests, &attacker_key);
    }

    /// 遭遇里的敌人：按条目 id / 名字 / 背后的实例键定位（图鉴 M2 起实例键也能寻址）。
    fn find_encounter_enemy(&self, enemy_id: &str) -> Option<(String, Value, usize)> {
        let st = self.state.lock().expect("state poisoned");
        for (enc_id, enc) in st.encounters.iter() {
            if !enc.get("active").and_then(Value::as_bool).unwrap_or(true) {
                continue;
            }
            let Some(list) = enc.get("enemies").and_then(Value::as_array) else { continue };
            for (i, e) in list.iter().enumerate() {
                let eid = e.get("id").and_then(Value::as_str).unwrap_or("");
                let name = e.get("name").and_then(Value::as_str).unwrap_or("");
                let inst = e.get("instance_id").and_then(Value::as_str).unwrap_or("");
                if eid == enemy_id
                    || (!enemy_id.is_empty() && name == enemy_id)
                    || (!inst.is_empty() && inst == enemy_id)
                {
                    return Some((enc_id.clone(), enc.clone(), i));
                }
            }
        }
        None
    }

    /// 敌人条目背后的图鉴怪物实例（图鉴 M2）；临时敌人（无 instance_id / 实例已不在）返回 None。
    fn enemy_instance(&self, enc: &Value, idx: usize) -> Option<(String, CharacterInstance)> {
        let key = enc
            .pointer(&format!("/enemies/{idx}/instance_id"))
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())?
            .to_string();
        let st = self.state.lock().expect("state poisoned");
        st.characters.get(&key).map(|c| (key.clone(), c.clone()))
    }

    /// 一次攻击的对称结算（图鉴 M2 §4.3）：攻守双方都是角色实例。
    ///
    /// **不是新写的第四个实现**——它就是 `command::execute_skill` 的一次调用：
    /// 判定器（内联 / 全局 / 合成 1d20）、Lua 挂载点、属性修正、消耗扣减、效果结算
    /// 全部沿用 use_skill / use_item 的同一条路径；玩家的 strike 与怪物的 enemy_strike
    /// 只是「谁是攻方、谁是守方」这一件事不同。
    fn resolve_attack(&self, setup: AttackSetup<'_>) -> Result<CommandOutcome, EngineError> {
        // 冷却起点用的两个字段先复制出来：闭包会借用 setup 的其余字段。
        let attacker_key = setup.attacker_key;
        let skill = setup.skill;
        // 注册表锁只在结算期间持有：Lua 写请求（trigger_event 等）在锁释放后才落状态，
        // 否则事件回到 dispatch_lua_event 再取同一把锁即死锁。
        let outcome = self.with_mount_gate(&setup.lua_ctx, |gate| {
            let registry = self.lua_registry.lock().expect("lua registry poisoned");
            let mut ctx = CommandContext {
                actor_id: setup.attacker_key,
                actor: setup.attacker_json,
                target_id: setup.target_id,
                target: setup.target_json,
                difficulty: setup.difficulty,
                attribute: setup.attribute.clone(),
                global_checker: setup.global_checker.as_ref(),
                rng: &self.rng,
                lua: Some((&self.lua, &setup.lua_ctx)),
                registry: Some(&*registry),
                status_defs: Some(self.rules.status_defs()),
                profiles: Some(self.rules.profiles()),
                attribute_bonuses: Some(self.rules.attribute_bonuses()),
                extra_bonus: setup.extra_bonus,
                effect_requires_success: true,
                mount_gate: gate,
            };
            execute_skill(setup.skill, &mut ctx)
        })?;
        // 攻击也是一种「使用技能」：结算成功（非驳回）即记冷却起点（#01），两种入口同口径。
        if outcome.rejection.is_none() {
            self.record_cooldown(attacker_key, skill);
        }
        Ok(outcome)
    }

    /// 按故事书 derived[] 顺序求值派生值（图鉴 M2 §4.4）：变量 = 实例属性（并入挂接定义 /
    /// 已装备物品的修正）+ 之前已算出的派生值。
    ///
    /// **纯计算，绝不写进 CharacterInstance**：属性被状态或装备改动后 AC 自动跟随，
    /// 没有陈旧值、没有新的不变量。与前端 computeDerived 同语法同口径。
    fn derived_values(
        &self,
        inst: &CharacterInstance,
    ) -> std::collections::HashMap<String, f64> {
        let attrs: std::collections::HashMap<String, f64> = inst
            .attributes
            .iter()
            .filter_map(|(k, v)| {
                v.as_f64()
                    .or_else(|| v.as_i64().map(|i| i as f64))
                    .map(|n| (k.clone(), n))
            })
            .collect();
        let empty = std::collections::HashMap::new();
        let modifiers = self
            .rules
            .attribute_bonuses()
            .get(&inst.template_id)
            .unwrap_or(&empty);
        compute_derived(&attrs, &self.rules.derived_defs(), modifiers)
    }

    /// 实例的派生 AC（图鉴 M2 §4.4）：故事书 derived[] 里 key = "ac" 的值。
    /// 缺失 / 求值失败 / 非正数 → None（调用方回落 default_dc → 12）。
    fn derived_ac(&self, inst: &CharacterInstance) -> Option<i64> {
        let v = *self.derived_values(inst).get("ac")?;
        (v.is_finite() && v > 0.0).then(|| v.round() as i64)
    }

    /// 图鉴条目的可用攻击（图鉴 M2 §4.6）：技能名 + 伤害骰。
    ///
    /// EnemySpec.skill_id 覆盖时它就是唯一一条（也是缺省的攻击技能）。只保留攻击类技能
    /// （check.kind = attack）；一个都没有时退回全部，别把数据卡清空。
    fn enemy_attacks(&self, template: &Value, override_skill: Option<&str>) -> Vec<EnemyAttack> {
        let ids: Vec<String> = match override_skill.map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => vec![id.to_string()],
            None => template
                .get("skills")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        };
        let all: Vec<EnemyAttack> = ids
            .iter()
            .filter_map(|id| self.rules.skill(id))
            .map(|skill| EnemyAttack {
                skill_id: skill.id.clone(),
                name: skill.name.clone(),
                damage: attack_damage_expr(Some(skill)),
            })
            .collect();
        let attacks: Vec<EnemyAttack> = all
            .iter()
            .filter(|a| is_attack_skill(self.rules.skill(&a.skill_id)))
            .cloned()
            .collect();
        if attacks.is_empty() { all } else { attacks }
    }


    /// 最近由「故事/导演」裁定的叙事事实（供提示词，确保 AI 不推翻既定前提）。
    fn canon_lines(&self, limit: usize) -> Vec<String> {
        let log = self.event_log.lock().expect("event log poisoned");
        let mut out: Vec<String> = log
            .iter()
            .filter(|e| e.actor.as_ref().is_some_and(|a| a.id == STORY_ACTOR_ID))
            .filter_map(|e| match &e.event {
                PlayEvent::Narrate(p) => Some(p.content.clone()),
                PlayEvent::Dialogue(p) => Some(p.content.clone()),
                PlayEvent::Emote(p) => Some(p.content.clone()),
                // 只读查询（query_world）的答案作为权威世界事实回填上下文（#04 查询类）：
                // 多轮工具调用循环未实现前，用 canon 通道让后续回合的 AI 读到查询结果。
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("query_world") => {
                    p.narrative.clone()
                }
                _ => None,
            })
            .collect();
        if out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        out
    }

    /// 当前受控角色（玩家）→ ActorRef。
    fn controlled_actor(&self) -> Option<ActorRef> {
        let st = self.state.lock().ok()?;
        let id = st.controlled.first()?.clone();
        let c = st.characters.get(&id)?;
        Some(ActorRef { id: c.template_id.clone(), name: c.name.clone() })
    }

    /// 模型回填 actor_id 的容错候选：原串，以及 `名字(id)` / `名字（id）` 括号内的 id。
    ///
    /// 提示词把在场角色渲染为 `名字(id)`（见 `present_actors`），并说明 actor_id
    /// 「形如 名字(id)」，模型常把整个 `名字(id)` 当作 id 回填。除原串外补上括号内的 id，
    /// 显式归属仍能命中本人，而不是被误判成「不存在」而驳回。
    fn actor_id_candidates(id: &str) -> Vec<&str> {
        let mut out = vec![id];
        for (open, close) in [('(', ')'), ('（', '）')] {
            let Some(start) = id.rfind(open) else { continue };
            let after = start + open.len_utf8();
            let Some(rel) = id[after..].find(close) else { continue };
            let inner = id[after..after + rel].trim();
            if !inner.is_empty() && inner != id {
                out.push(inner);
            }
        }
        out
    }

    /// 在角色表里按任意一种 id（实例键 / 模板 id / 角色名 / 实例 id）查人。
    /// 兼容模型把 `名字(id)` 整串当 id 回填的形式。
    fn find_character<'a>(
        chars: &'a std::collections::BTreeMap<String, octopus_types::CharacterInstance>,
        id: &str,
    ) -> Option<(&'a String, &'a octopus_types::CharacterInstance)> {
        Self::actor_id_candidates(id).into_iter().find_map(|cand| {
            chars.iter().find(|(key, c)| {
                c.template_id == cand
                    || c.name == cand
                    || c.instance_id == cand
                    || key.as_str() == cand
            })
        })
    }

    /// 按任意一种 id（实例键 / 模板 id / 角色名 / 实例 id）找角色。
    fn actor_by_id(&self, id: &str) -> Option<ActorRef> {
        let st = self.state.lock().ok()?;
        Self::find_character(&st.characters, id)
            .map(|(_, c)| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
    }

    /// 本场在演的非玩家角色（未署名 speak / emote 的回落对象）。
    ///
    /// 按模板 id 排序：并行调用完成后按此顺序结算，事件顺序与完成顺序无关。
    fn present_npcs(&self) -> Vec<ActorRef> {
        let Ok(st) = self.state.lock() else {
            return Vec::new();
        };
        let mut out: Vec<ActorRef> = st
            .characters
            .values()
            // 图鉴 M2 §4.6：怪物不参与对话扮演（它们出现在【当前遭遇】块里）。
            .filter(|c| c.kind != "pc" && c.kind != "monster" && c.present)
            .map(|c| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 角色作用域下的「本人标识集合」：模板 id / 名字 / 实例键 / instance_id。
    /// 关系边端点通常写故事书人物 id（= 模板 id），这里一并带上实例标识，
    /// 防止同一角色在故事书与存档用不同 id 时丢边（#04 §6）。
    fn character_identifiers(&self, actor: &ActorRef) -> Vec<String> {
        let mut ids = vec![actor.id.clone(), actor.name.clone()];
        if let Ok(st) = self.state.lock() {
            if let Some((key, c)) = st.characters.iter().find(|(key, c)| {
                c.template_id == actor.id || c.name == actor.id || key.as_str() == actor.id
            }) {
                ids.push(key.clone());
                ids.push(c.instance_id.clone());
            }
        }
        ids.retain(|s| !s.trim().is_empty());
        ids.dedup();
        ids
    }

    /// 单个角色的私有资料卡（#04 query_character）：属性 / 资源 / 状态 / 物品 / 位置。
    /// 只读、不产生状态变更；调用方负责保证只有本人（或主线 AI）能取到。
    fn character_sheet(&self, key: &str) -> Option<String> {
        let st = self.state.lock().ok()?;
        let c = st.characters.get(key)?;
        let mut parts = vec![format!("{}（{}）", c.name, c.template_id)];
        if !c.attributes.is_empty() {
            parts.push(format!("属性：{}", compact_map(&c.attributes)));
        }
        if !c.resources.is_empty() {
            parts.push(format!("资源：{}", compact_map(&c.resources)));
        }
        if !c.statuses.is_empty() {
            let list: Vec<String> = c
                .statuses
                .iter()
                .map(|s| {
                    let left = s
                        .turns_left
                        .map(|t| format!("（剩 {t} 回合）"))
                        .or_else(|| s.scenes_left.map(|t| format!("（剩 {t} 场景）")))
                        .unwrap_or_default();
                    format!("{}{}", if s.name.is_empty() { &s.id } else { &s.name }, left)
                })
                .collect();
            parts.push(format!("状态：{}", list.join("、")));
        }
        if !c.inventory.is_empty() {
            parts.push(format!("物品：{}", compact_map(&c.inventory)));
        }
        if let Some(loc) = &c.location_id {
            parts.push(format!("位置：{loc}"));
        }
        Some(parts.join("\n"))
    }

    /// 从台词/神态文本里认人：取**最长**的名字匹配，避免「诺德罗」被更短的别名抢先。
    fn actor_in_text(&self, text: &str) -> Option<ActorRef> {
        self.longest_npc_in(text)
    }

    /// 只认**句首主语位**的名字：旁白里顺带提到的名字不算行动者。
    /// 「Lucy靠在他身侧」是她的动作；「Lucy家的灯还亮着」只是环境描写。
    fn actor_leading_in_text(&self, text: &str, chars: usize) -> Option<ActorRef> {
        let head: String = text.trim_start().chars().take(chars).collect();
        self.longest_npc_in(&head)
    }

    /// 在一段文本里找**最长**的非玩家角色名。
    /// 玩家角色（PC）不由 AI 代说：文本里出现的「你 / PC 名」是**称呼**，不是行动者，
    /// 否则 PC 名恰好是「你」时，「你带伞了吗？」这类 NPC 台词会被判给玩家。
    fn longest_npc_in(&self, hay: &str) -> Option<ActorRef> {
        if hay.is_empty() { return None }
        let st = self.state.lock().ok()?;
        let mut best: Option<(usize, ActorRef)> = None;
        for c in st.characters.values() {
            if c.name.is_empty() || !hay.contains(&c.name) { continue }
            if c.kind == "pc" { continue }
            let len = c.name.chars().count();
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((len, ActorRef { id: c.template_id.clone(), name: c.name.clone() }));
            }
        }
        best.map(|(_, a)| a)
    }

    /// 意图归属：① 意图自带的 actor_id；② 文本里提到的角色名；③ None（调用方回落）。
    ///
    /// 旁白也参与推断（#17：叙事事件必带归属），但**只认句首主语位**的名字：
    /// 角色动作常被模型写成无归属旁白（「Lucy靠在他身侧…」），这类要认出来；
    /// 而环境描写里顺带提及的名字（「Lucy家的灯还亮着」）不该被当成行动者。
    fn resolve_intent_actor(&self, intent: &Intent) -> Option<ActorRef> {
        let (id, content, leading_only) = match intent {
            Intent::Speak { actor_id, content, .. } => (actor_id.clone(), content.as_str(), false),
            Intent::Emote { actor_id, content, .. } => (actor_id.clone(), content.as_str(), false),
            Intent::Check { actor_id, .. } => (actor_id.clone(), "", false),
            Intent::Narrate { actor_id, content } => (actor_id.clone(), content.as_str(), true),
            _ => (None, "", false),
        };
        if let Some(a) = id.and_then(|i| self.actor_by_id(&i)) {
            return Some(a);
        }
        if leading_only {
            return self.actor_leading_in_text(content, NARRATE_ACTOR_HEAD_CHARS);
        }
        self.actor_in_text(content)
    }

    /// 显式 actor_id 的合法性校验（#04 ④）：
    /// - None / 空串 = 未提供 → 合法（保持既有的文本推断与阶段回落）；
    /// - 指向不存在 / 不在场的角色 → ActorNotFound，调用方驳回，绝不静默改判给他人；
    /// - 指向玩家受控角色（PC）且意图是 speak → ActorNotControlled：
    ///   玩家的**台词**由玩家自己输入，AI 不代说。
    ///   注意 emote / narrate 不在此列：单一 AI 描写玩家角色的动作 / 环境属正常旁白，
    ///   早先「PC 一律禁止说演」过严，会把这类叙述整条丢掉并弹出驳回卡片。
    fn invalid_explicit_actor(&self, intent: &Intent) -> Option<(String, RejectionCode)> {
        let id = match intent {
            Intent::Speak { actor_id, .. }
            | Intent::Emote { actor_id, .. }
            | Intent::Check { actor_id, .. }
            | Intent::Narrate { actor_id, .. } => actor_id.as_deref()?,
            _ => return None,
        };
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let st = self.state.lock().ok()?;
        let Some((key, c)) = Self::find_character(&st.characters, id) else {
            return Some((id.to_string(), RejectionCode::ActorNotFound));
        };
        // 在场判定与 present_actors 保持一致：受控角色即便 present=false 也算在场。
        let controlled = st.controlled.contains(key)
            || st.controlled.contains(&c.instance_id)
            || st.controlled.contains(&c.template_id);
        if !c.present && !controlled {
            return Some((id.to_string(), RejectionCode::ActorNotFound));
        }
        if controlled && matches!(intent, Intent::Speak { .. }) {
            return Some((id.to_string(), RejectionCode::ActorNotControlled));
        }
        None
    }

    /// 取某角色实例的属性值（判定修正用）；找不到人物或维度时回落中心基线。
    fn actor_attribute_value(&self, actor: &ActorRef, attribute: &str) -> f64 {
        let Ok(st) = self.state.lock() else {
            return crate::resolve::DEFAULT_BASELINE;
        };
        st.characters
            .values()
            .find(|c| c.template_id == actor.id || c.name == actor.id || c.instance_id == actor.id)
            .and_then(|c| c.attributes.get(attribute))
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
            .unwrap_or(crate::resolve::DEFAULT_BASELINE)
    }

    /// 取某角色实例的 JSON（Lua 判定上下文用）；找不到时 Null。
    fn actor_json_of(&self, actor: &ActorRef) -> Value {
        let Ok(st) = self.state.lock() else {
            return Value::Null;
        };
        st.characters
            .values()
            .find(|c| c.template_id == actor.id || c.name == actor.id || c.instance_id == actor.id)
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .unwrap_or(Value::Null)
    }

    /// 只读查询（#04 Query 类）：对权威世界状态做只读汇总，供回喂模型。
    /// 按查询关键词收窄；无法识别时给一份紧凑快照，绝不返回空。
    fn answer_query(&self, query: &str, scope_ids: Option<&[String]>) -> String {
        let q = query.trim().to_lowercase();
        let wants = |keys: &[&str]| q.is_empty() || keys.iter().any(|k| q.contains(k));
        let st = self.state.lock().expect("state poisoned");
        // 角色 AI 作用域（#04 §6 / #16 认知边界）：scope_ids = 发起查询的角色本人标识，
        // 只返回它自己的私有数据；None = 主线 AI 全量。在场名单是公开信息，恒返回。
        let sees_all = scope_ids.is_none();
        let is_self = |key: &str, c: &octopus_types::CharacterInstance| {
            scope_ids.is_some_and(|ids| {
                ids.iter().any(|id| {
                    id == key || id == &c.template_id || id == &c.instance_id || id == &c.name
                })
            })
        };
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("场景：{}（{}）", st.scene_title, st.scene_id));
        if let Some(d) = st
            .scene_description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            lines.push(format!("场景描述：{d}"));
        }
        // 在场角色恒返回：「有谁 / 谁在」是最常见的世界查询。
        let present: Vec<String> = st
            .characters
            .iter()
            .filter(|(key, c)| {
                c.present || st.controlled.contains(key) || st.controlled.contains(&c.instance_id)
            })
            .map(|(_, c)| format!("{}（{}）", c.name, c.template_id))
            .collect();
        lines.push(if present.is_empty() {
            "在场角色：无".to_string()
        } else {
            format!("在场角色：{}", present.join("、"))
        });
        if wants(&["属性", "数值", "资源", "血量", "法力", "attribute", "resource", "stat", "hp", "mana"]) {
            // 只报在场（含受控）角色：减少对未出场角色的信息泄露；
            // 角色 AI 作用域再收窄到本人（#16 认知边界）。
            for (_key, c) in st.characters.iter().filter(|(key, c)| {
                (c.present || st.controlled.contains(key) || st.controlled.contains(&c.instance_id))
                    && (sees_all || is_self(key, c))
            }) {
                if !c.attributes.is_empty() {
                    lines.push(format!("{} 属性：{}", c.name, compact_map(&c.attributes)));
                }
                if !c.resources.is_empty() {
                    lines.push(format!("{} 资源：{}", c.name, compact_map(&c.resources)));
                }
            }
        }
        if sees_all && wants(&["标记", "旗", "flag"]) {
            lines.push(format!(
                "世界标记：{}",
                if st.flags.is_empty() { "无".to_string() } else { compact_map(&st.flags) }
            ));
        }
        if sees_all && wants(&["任务", "目标", "quest", "goal"]) {
            let quests: Vec<String> = st
                .progress
                .goals
                .iter()
                .filter(|(_, v)| !v.get("hidden").and_then(Value::as_bool).unwrap_or(false))
                .map(|(id, v)| {
                    let text = v.get("text").and_then(Value::as_str).unwrap_or(id);
                    let done = v.get("done").and_then(Value::as_bool).unwrap_or(false);
                    format!("{}（{}）", text, if done { "已达成" } else { "进行中" })
                })
                .collect();
            lines.push(format!(
                "任务/目标：{}",
                if quests.is_empty() { "无".to_string() } else { quests.join("；") }
            ));
        }
        if sees_all && wants(&["遭遇", "敌人", "战斗", "encounter", "enemy", "combat"]) {
            let enc: Vec<String> = st
                .encounters
                .values()
                .map(|v| v.get("name").and_then(Value::as_str).unwrap_or("遭遇").to_string())
                .collect();
            lines.push(format!(
                "遭遇：{}",
                if enc.is_empty() { "无".to_string() } else { enc.join("；") }
            ));
        }
        if sees_all && wants(&["地点", "位置", "场景", "location", "scene"]) && !st.locations.is_empty() {
            let locs: Vec<String> = st
                .locations
                .iter()
                .map(|l| {
                    l.get("name")
                        .and_then(Value::as_str)
                        .or_else(|| l.get("id").and_then(Value::as_str))
                        .unwrap_or("?")
                        .to_string()
                })
                .collect();
            lines.push(format!("已知地点：{}", locs.join("、")));
        }
        if wants(&["关系", "relationship"]) {
            // 角色作用域只给触及自己的边（#04 §6 query_relationships 语义）。
            let rels = self.rules.relationships();
            let rels: Vec<Value> = match scope_ids {
                Some(ids) => rels.into_iter().filter(|r| edge_touches(r, ids)).collect(),
                None => rels,
            };
            let text = if rels.is_empty() {
                "无".to_string()
            } else {
                serde_json::to_string(&rels).unwrap_or_default()
            };
            lines.push(format!("关系：{text}"));
        }
        if wants(&["物件", "object", "物品", "item", "背包", "inventory"]) {
            if let Some(objects) = self.rules.storybook.get("objects").and_then(Value::as_array) {
                let names: Vec<String> = objects
                    .iter()
                    .map(|o| o.get("name").and_then(Value::as_str).unwrap_or("?").to_string())
                    .collect();
                lines.push(format!(
                    "场景物件：{}",
                    if names.is_empty() { "无".to_string() } else { names.join("、") }
                ));
            }
            for (_key, c) in st.characters.iter().filter(|(key, c)| sees_all || is_self(key, c)) {
                if !c.inventory.is_empty() {
                    lines.push(format!("{} 物品栏：{}", c.name, compact_map(&c.inventory)));
                }
            }
        }
        lines.join("\n")
    }

    /// 与场景物件交互（#04 / #01 objects）：校验物件与动作后落一条 Resolution。
    /// 故事书未声明 objects 时优雅降级（明确说明，不报错、不改状态）。
    /// 成功（含降级）返回该 Resolution 的文本，供回合内续轮回喂模型（#04 ⑦）。
    fn handle_interact(&self, object_id: &str, action: &str, actor: Option<ActorRef>) -> Option<String> {
        let Some(objects) = self.rules.storybook.get("objects").and_then(Value::as_array) else {
            let narrative = "这本故事书未声明「物件（objects）」，交互请求已忽略。".to_string();
            self.emit(
                PlayEvent::Resolution(ResolutionPayload {
                    intent_id: None,
                    status: ResolutionStatus::Ok,
                    rejection_code: None,
                    narrative: Some(narrative.clone()),
                    outcome: Some("interact".into()),
                    triggered_events: None,
                    state_changes: vec![],
                }),
                actor,
                None,
            );
            return Some(narrative);
        };
        let Some(object) = objects
            .iter()
            .find(|o| o.get("id").and_then(Value::as_str) == Some(object_id))
        else {
            self.reject(format!("找不到物件：{object_id}"), RejectionCode::TargetInvalid);
            return None;
        };
        let name = object.get("name").and_then(Value::as_str).unwrap_or(object_id);
        let mut label = action.to_string();
        // 物件声明了动作列表时，action 必须命中；未声明动作列表则宽容放行（降级）。
        if let Some(actions) = object
            .get("actions")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
        {
            match actions.iter().find(|a| {
                a.get("key").and_then(Value::as_str) == Some(action) || a.as_str() == Some(action)
            }) {
                Some(a) => {
                    if let Some(l) = a.get("label").and_then(Value::as_str).filter(|l| !l.trim().is_empty()) {
                        label = l.to_string();
                    }
                }
                None => {
                    self.reject(
                        format!("物件「{name}」不支持动作：{action}"),
                        RejectionCode::TargetInvalid,
                    );
                    return None;
                }
            }
        }
        let narrative = match object.get("description").and_then(Value::as_str).map(str::trim) {
            Some(d) if !d.is_empty() => format!("与「{name}」交互（{label}）：{d}"),
            _ => format!("与「{name}」交互：{label}"),
        };
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(narrative.clone()),
                outcome: Some("interact".into()),
                triggered_events: None,
                state_changes: vec![],
            }),
            actor,
            None,
        );
        Some(narrative)
    }

    /// #04 Query 类：查询某角色的私有资料（属性 / 资源 / 状态 / 物品 / 位置）。
    ///
    /// 作用域规则（#16 认知边界）：角色 AI（scope = Some）只能查自己——缺省查自己，
    /// 显式查他人一律驳回，绝不泄露他人私有数据；主线 AI（scope = None）可查任意角色，
    /// 缺省查受控角色。答案落一条 Resolution，并回喂调用方（续轮 / 事件流）。
    fn handle_query_character(
        &self,
        character_id: Option<String>,
        actor: Option<ActorRef>,
        scope: Option<&ActorRef>,
    ) -> Option<String> {
        let requested = character_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // 锁内只做定位 / 判定，锁外再落事件，避免与 character_sheet 的加锁嵌套。
        let (target, denied) = {
            let st = self.state.lock().expect("state poisoned");
            match scope {
                Some(myself) => {
                    let self_key = character_key_in(&st, &myself.id)
                        .or_else(|| character_key_in(&st, &myself.name));
                    match &requested {
                        None => (self_key, None),
                        Some(id) => {
                            if self_key.is_some()
                                && character_key_in(&st, id).as_deref() == self_key.as_deref()
                            {
                                (self_key, None)
                            } else {
                                (None, Some(id.clone()))
                            }
                        }
                    }
                }
                None => {
                    let target = requested
                        .as_deref()
                        .and_then(|id| character_key_in(&st, id))
                        .or_else(|| st.controlled.first().cloned())
                        .or_else(|| st.characters.keys().next().cloned());
                    (target, None)
                }
            }
        };
        if let Some(id) = denied {
            self.reject(
                format!("角色 AI 只能查询自身资料，不能查询「{id}」"),
                RejectionCode::TargetInvalid,
            );
            return None;
        }
        let Some(key) = target else {
            self.reject(
                format!("查询角色不存在：{}", requested.as_deref().unwrap_or("")),
                RejectionCode::TargetInvalid,
            );
            return None;
        };
        let answer = self
            .character_sheet(&key)
            .unwrap_or_else(|| "（无此人资料）".to_string());
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(answer.clone()),
                outcome: Some("query_character".into()),
                triggered_events: None,
                state_changes: vec![],
            }),
            actor.or_else(|| scope.cloned()).or_else(|| Some(story_actor())),
            None,
        );
        Some(answer)
    }

    /// #04 Query 类：查询与某实体相关的关系边。
    ///
    /// 作用域规则（#04 §6）：角色 AI（scope = Some）只得到触及自己的边；主线 AI
    /// （scope = None）给全集，显式 `entity_id` 时收窄到该实体。只读、不改状态。
    fn handle_query_relationships(
        &self,
        entity_id: Option<String>,
        actor: Option<ActorRef>,
        scope: Option<&ActorRef>,
    ) -> Option<String> {
        let requested = entity_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let ids: Option<Vec<String>> = match scope {
            // 角色 AI：无论它请求谁的关系，都只返回触及自己的边。
            Some(myself) => Some(self.character_identifiers(myself)),
            None => requested.as_deref().map(|id| {
                self.character_identifiers(&ActorRef { id: id.to_string(), name: id.to_string() })
            }),
        };
        let rels = self.rules.relationships();
        let rels: Vec<Value> = match ids.as_deref() {
            Some(ids) => rels.into_iter().filter(|r| edge_touches(r, ids)).collect(),
            None => rels,
        };
        let answer = if rels.is_empty() {
            "（无相关关系）".to_string()
        } else {
            serde_json::to_string(&rels).unwrap_or_default()
        };
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(answer.clone()),
                outcome: Some("query_relationships".into()),
                triggered_events: None,
                state_changes: vec![],
            }),
            actor.or_else(|| scope.cloned()).or_else(|| Some(story_actor())),
            None,
        );
        Some(answer)
    }

    /// 结算一个意图。返回值 = 本意图新产生的、模型应据以续写的引擎信息
    /// （#04 ⑦）：只有 query_world / check / interact 这类「模型自己发起的读取」才返回，
    /// 其余意图的结算结果不需要模型再反应，返回 None。
    async fn handle_intent(&self, intent: Intent, actor: Option<ActorRef>) -> Option<String> {
        self.handle_intent_scoped(intent, actor, None).await
    }

    /// 结算一个意图，并按发起者作用域过滤只读查询（#04 §6 / #16 认知边界）。
    /// `scope` = 角色 AI 本次只扮演 / 只代表的那一个角色；None = 主线 AI（全量视野）。
    async fn handle_intent_scoped(
        &self,
        intent: Intent,
        actor: Option<ActorRef>,
        scope: Option<&ActorRef>,
    ) -> Option<String> {
        match intent {
            // 模型写在正文里的思考（P3）：只落一条思考事件（source=model），
            // 不进叙事条目、不改世界状态；前端沿用同一个「思考」折叠组件。
            Intent::Think { content } => {
                self.emit_simple(PlayEvent::Reasoning(ReasoningPayload {
                    stage: "model_draft".into(),
                    text: content,
                    source: "model".into(),
                }));
            }
            Intent::Narrate { content, .. } => {
                // 旁白也带归属：导演回合 → 故事（前端用「导演裁定」样式渲染，
                // 并进入提示词的「已裁定事实」）；普通回合 → 玩家角色（渲染仍是散文）。
                self.emit(PlayEvent::Narrate(NarratePayload { content, scene_ref: None }), actor, None);
            }
            Intent::Speak { content, .. } => {
                self.emit(PlayEvent::Dialogue(DialoguePayload { content, audience: None }), actor, None);
            }
            Intent::Emote { content, emotion, .. } => {
                self.emit(PlayEvent::Emote(EmotePayload { content, emotion, gesture: None }), actor, None);
            }
            Intent::Check { attribute, difficulty, opponent_id, .. } => {
                return self.run_check(attribute, difficulty, opponent_id, actor).await;
            }
            Intent::Quest { text, hidden, primary } => {
                let id = format!("quest-{}", uuid::Uuid::new_v4().simple());
                self.emit(
                    PlayEvent::StateUpdate(StateUpdatePayload {
                        changes: vec![StateDelta {
                            domain: DeltaDomain::Goal,
                            entity_id: id,
                            field: "quest".into(),
                            op: DeltaOp::Add,
                            value: serde_json::json!({
                                "text": text, "done": false, "source": "gm",
                                "hidden": hidden, "primary": primary,
                            }),
                        }],
                    }),
                    Some(story_actor()),
                    None,
                );
            }
            Intent::Adjust { character_id, resource, amount } => {
                // 护栏：GM 只能把数值改到合法区间（二进制 0..1；数值 0..default_max）。
                // 落库的是**钳制后的绝对值**（Set），所以事件日志里就是最终值，重放一致。
                let def = self
                    .rules
                    .storybook
                    .pointer("/world/resources")
                    .and_then(Value::as_array)
                    .and_then(|arr| {
                        arr.iter().find(|r| {
                            r.get("id").and_then(Value::as_str) == Some(resource.as_str())
                        })
                    });
                let binary =
                    def.and_then(|d| d.get("type")).and_then(Value::as_str) == Some("binary");
                let max = def.and_then(|d| d.get("default_max")).and_then(Value::as_f64);
                let cur = {
                    let st = self.state.lock().expect("state poisoned");
                    st.characters
                        .get(&character_id)
                        .or_else(|| {
                            st.characters.values().find(|c| c.template_id == character_id)
                        })
                        .and_then(|c| c.resources.get(&resource))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                };
                let mut next = cur + amount as f64;
                if binary {
                    next = next.clamp(0.0, 1.0);
                } else {
                    next = next.max(0.0);
                    if let Some(m) = max {
                        next = next.min(m);
                    }
                }
                let value = if next.fract() == 0.0 {
                    Value::from(next as i64)
                } else {
                    Value::from(next)
                };
                self.emit(
                    PlayEvent::StateUpdate(StateUpdatePayload {
                        changes: vec![StateDelta {
                            domain: DeltaDomain::Character,
                            entity_id: character_id,
                            field: format!("resources.{resource}"),
                            op: DeltaOp::Set,
                            value,
                        }],
                    }),
                    Some(story_actor()),
                    None,
                );
            }
            Intent::Strike { enemy_id, skill_id } => {
                let choice = AttackChoice { skill_id, ..Default::default() };
                self.strike_enemy(enemy_id, choice, actor).await;
            }
            Intent::EnemyStrike { enemy_id, target_id, skill_id } => {
                // 图鉴 M2 §4.2：敌人打角色——与 strike 同一个内核的另一个方向。
                self.enemy_strike(enemy_id, target_id, skill_id).await;
            }
            Intent::Encounter { name, enemies, note } => {
                // 导演即兴遭遇：与触发点预置遭遇（地图 P5 §6.3）共用同一条创建路径
                // （见 create_encounter：图鉴实例克隆 / 地点继承 / 叙事锚快照）。
                self.create_encounter(name, enemies, note, None);
            }
            Intent::Status { character_id, status_id, remove } => {
                let (op, value) = if remove {
                    (DeltaOp::Remove, Value::String(status_id))
                } else {
                    let defs = self.rules.status_defs();
                    let Some(def) = defs.get(&status_id) else {
                        self.reject(
                            format!("找不到状态：{status_id}"),
                            RejectionCode::RuleViolation,
                        );
                        return None;
                    };
                    let unit = if def.unit == StatusUnit::Scenes { "scenes" } else { "turns" };
                    let inst =
                        crate::effects::build_status_instance(&status_id, def.duration, unit, defs);
                    // 叠加策略（#12 ③）：显式施加状态与技能效果走同一套 add / max 合并，
                    // 否则「声明了 stack 却只有技能路径生效」会变成第二处声明与实现不一致。
                    let existing = {
                        let st = self.state.lock().expect("state poisoned");
                        Self::find_character(&st.characters, &character_id)
                            .and_then(|(_, c)| c.statuses.iter().find(|s| s.id == status_id).cloned())
                    };
                    let inst = crate::effects::merge_status(existing.as_ref(), inst, def.stack);
                    (DeltaOp::Set, serde_json::to_value(inst).unwrap_or(Value::Null))
                };
                self.emit(
                    PlayEvent::StateUpdate(StateUpdatePayload {
                        changes: vec![StateDelta {
                            domain: DeltaDomain::Character,
                            entity_id: character_id,
                            field: "status".into(),
                            op,
                            value,
                        }],
                    }),
                    Some(story_actor()),
                    None,
                );
            }
            Intent::Summary { text } => {
                // 回合微摘要（#05 §3.2）：派生数据，不落叙事事件、不改世界状态。
                // 端口缺失 / 写失败都只 warn，本回合照常。
                let text = text.trim();
                if text.is_empty() {
                    return None;
                }
                let store = self
                    .summary_store
                    .lock()
                    .expect("summary store poisoned")
                    .clone();
                let Some(store) = store else { return None };
                let round = self.round.load(Ordering::SeqCst);
                if let Err(e) = store.put_round_summary(&self.save_id, round, text).await {
                    tracing::warn!(save_id = %self.save_id, round, error = %e, "回合微摘要写入失败（派生数据，忽略）");
                }
            }
            Intent::Intervene { content } => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("intervene".into()),
                    text: format!("玩家干预：{content}"),
                }));
            }
            Intent::Move { destination_id, character_id } => {
                // 目标角色：显式 character_id（图鉴 M2 补丁）→ 意图归属的角色（角色 AI 的
                // move 就是**本人**移动，NPC / 怪物都算）→ 受控角色——未指名时旧的
                // 「只动受控角色」行为逐字不变。设计 §4.2「Move 泛化」：位置不再只有
                // 玩家一个人写得动。
                let explicit = character_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let (target, missing) = {
                    let st = self.state.lock().expect("state poisoned");
                    match explicit {
                        // 显式指名却找不到 → 驳回，绝不静默改判给受控角色。
                        Some(id) => match character_key_in(&st, id) {
                            Some(key) => (Some(key), None),
                            None => (None, Some(id.to_string())),
                        },
                        None => (
                            actor
                                .as_ref()
                                .and_then(|a| character_key_in(&st, &a.id))
                                .or_else(|| st.controlled.first().cloned()),
                            None,
                        ),
                    }
                };
                if let Some(id) = missing {
                    self.reject(
                        format!("找不到要移动的角色：{id}"),
                        RejectionCode::ActorNotFound,
                    );
                    return None;
                }
                if let Some(c) = target {
                    self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
                        changes: vec![StateDelta {
                            domain: DeltaDomain::Character,
                            entity_id: c,
                            field: "location_id".into(),
                            op: DeltaOp::Set,
                            value: Value::String(destination_id.clone()),
                        }],
                    }));
                }
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("moved".into()),
                    text: format!("移动到 {destination_id}"),
                }));
            }
            Intent::AdvanceScene { target_scene_id, abandon } => {
                // 离开的是**切换前**的场景：场景压缩按它归档本场景期间的微摘要。
                let leaving_scene = self.state.lock().expect("state poisoned").scene_id.clone();
                let round = self.round.load(Ordering::SeqCst);
                // 场景切换：先落「在场名单」增量，再落权威 Scene 事件（更新场景元信息）。
                if let Some(target) = target_scene_id.as_deref() {
                    self.switch_scene(target);
                }
                self.emit_simple(PlayEvent::Resolution(ResolutionPayload {
                    intent_id: None,
                    status: ResolutionStatus::Ok,
                    rejection_code: None,
                    narrative: Some(if abandon { "你们离开了这里。".into() } else { "场景推进。".into() }),
                    outcome: Some("advance_scene".into()),
                    triggered_events: target_scene_id.map(|t| vec![t]),
                    state_changes: vec![],
                }));
                // 场景切换：scenes 单位的持续状态 tick（#12 ④）
                self.tick_statuses_scene();
                // 事件广播：declarative triggers + Lua Event 挂载点。
                // 规范事件名 scene（决策 #11，与演出流事件统一）；旧名 scene_change 由别名匹配兜底。
                self.dispatch_event("scene");
                // 场景压缩（#05 §3.3）：派生数据，任何失败都只 warn，绝不阻断本回合。
                self.compress_scene(&leaving_scene, round).await;
                // 新场景从本回合起：下次压缩窗口左界。
                self.scene_start_round.store(round, Ordering::SeqCst);
            }
            Intent::QueryWorld { query } => {
                // #04 Query 类：只读、不产生状态变更；答案作为 Resolution 落事件流，
                // 既供 canon_lines 注入后续回合，也作为本回合续写的工具结果回喂（#04 ⑦）。
                // 角色 AI 作用域：按该角色收窄，不泄露他人私有数据（#04 §6 / #16）。
                let scope_ids = scope.map(|a| self.character_identifiers(a));
                let answer = self.answer_query(&query, scope_ids.as_deref());
                self.emit(
                    PlayEvent::Resolution(ResolutionPayload {
                        intent_id: None,
                        status: ResolutionStatus::Ok,
                        rejection_code: None,
                        narrative: Some(answer.clone()),
                        outcome: Some("query_world".into()),
                        triggered_events: None,
                        state_changes: vec![],
                    }),
                    actor.clone().or_else(|| scope.cloned()).or_else(|| Some(story_actor())),
                    None,
                );
                return Some(answer);
            }
            Intent::QueryCharacter { character_id } => {
                return self.handle_query_character(character_id, actor, scope);
            }
            Intent::QueryRelationships { entity_id } => {
                return self.handle_query_relationships(entity_id, actor, scope);
            }
            Intent::Interact { object_id, action } => {
                return self.handle_interact(&object_id, &action, actor);
            }
            Intent::UseSkill { skill_id, target_id } => {
                self.handle_use_skill(&skill_id, target_id.as_deref(), actor);
            }
            Intent::UseItem { item_id, target_id } => {
                self.handle_use_item(&item_id, target_id.as_deref(), actor);
            }
            Intent::EndTurn => {
                // 时序回合结束（#GAP-I）：推进指针、重置预算。非时序状态下是 no-op。
                self.end_turn();
            }
            Intent::FinishTurn => {}
        }
        // 其余意图只落叙事 / 状态，不需要模型据新信息续写。
        None
    }

    // ---------- 机制结算（#04 四阶段 / #12 判定与效果） ----------

    fn handle_use_skill(&self, skill_id: &str, target_id: Option<&str>, actor: Option<ActorRef>) {
        let Some(skill) = self.rules.skill(skill_id).cloned() else {
            self.reject(format!("找不到技能：{skill_id}"), RejectionCode::RuleViolation);
            return;
        };
        self.resolve_skill(None, &skill, target_id, actor);
    }

    fn handle_use_item(&self, item_id: &str, target_id: Option<&str>, actor: Option<ActorRef>) {
        let skill_ids = self.rules.item_skill_ids(item_id);
        let Some(skill) = skill_ids.iter().find_map(|id| self.rules.skill(id)).cloned() else {
            self.reject(format!("物品没有可用技能：{item_id}"), RejectionCode::RuleViolation);
            return;
        };
        self.resolve_skill(Some(item_id), &skill, target_id, actor);
    }

    fn reject(&self, narrative: String, code: RejectionCode) {
        // 驳回属于「引擎内部 QA 信息」：不渲染成玩家卡片（前端过滤），但一律进服务端日志，
        // 并照样落一条 Resolution 事件——按 save_id 查命令日志就能复盘。
        tracing::warn!(
            save_id = %self.save_id,
            code = code.as_str(),
            reason = %narrative,
            "AI 意图被驳回"
        );
        // 记账供回合内回喂（见 run_round 的 next_feedback）：驳回不进叙事，模型看不到。
        self.last_rejection
            .lock()
            .expect("rejection poisoned")
            .replace(format!("[{}] {narrative}", code.as_str()));
        self.rejection_count.fetch_add(1, Ordering::SeqCst);
        self.emit_simple(PlayEvent::Resolution(ResolutionPayload {
            intent_id: None,
            status: ResolutionStatus::Rejected,
            rejection_code: Some(code.as_str().into()),
            narrative: Some(narrative),
            outcome: None,
            triggered_events: None,
            state_changes: vec![],
        }));
    }

    /// 冷却剩余回合（#01）：技能声明了 \`cooldown.turns\` 且仍在冷却期内 → 返回剩余回合数。
    ///
    /// 未声明冷却 / 冷却 <= 0 / 从未用过 → None（可以使用）。记账读 \`WorldState.cooldowns\`
    /// （随命令日志重放），所以重启后冷却不会凭空消失。
    fn cooldown_block(&self, actor_key: &str, skill: &SkillDef) -> Option<i64> {
        let turns = skill.cooldown.map(|c| c.turns).unwrap_or(0);
        if turns <= 0 || skill.id.is_empty() || actor_key.is_empty() {
            return None;
        }
        let last = {
            let st = self.state.lock().expect("state poisoned");
            st.cooldowns
                .get(actor_key)
                .and_then(|m| m.get(&skill.id))
                .copied()?
        };
        let now = self.round.load(Ordering::SeqCst);
        let elapsed = now.saturating_sub(last) as i64;
        (elapsed < turns).then(|| turns - elapsed)
    }

    /// 记一次技能使用起点（#01）：只有声明了冷却的技能才写，避免给无冷却技能刷无用状态。
    ///
    /// 走 Character 域的 \`cooldown.<skill_id>\` delta（\`apply_delta\` 是唯一变更路径），
    /// 因此与其它状态一样可重放、可审计；不产生叙事、也不改其它世界状态。
    fn record_cooldown(&self, actor_key: &str, skill: &SkillDef) {
        let turns = skill.cooldown.map(|c| c.turns).unwrap_or(0);
        if turns <= 0 || skill.id.is_empty() || actor_key.is_empty() {
            return;
        }
        self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Character,
                entity_id: actor_key.to_string(),
                field: format!("cooldown.{}", skill.id),
                op: DeltaOp::Set,
                value: Value::from(self.round.load(Ordering::SeqCst)),
            }],
        }));
    }

    /// 走 command 管线结算一次技能/物品使用，并把结果落成权威事件。
    fn resolve_skill(
        &self,
        item_id: Option<&str>,
        skill: &SkillDef,
        target_id: Option<&str>,
        actor: Option<ActorRef>,
    ) {
        // 结算主体（修 D7）：调用方指定（意图 / 参数）优先，其次受控角色；两者都按
        // 任意 id 形态（实例键 / 模板 id / 名字 / instance_id）解析，不再恒取 controlled。
        let (actor_key, actor_json, actor_ref, scene_id) = {
            let st = self.state.lock().expect("state poisoned");
            let scene = st.scene_id.clone();
            let found = match actor.as_ref().filter(|a| !a.id.is_empty()) {
                Some(a) => Self::find_character(&st.characters, &a.id),
                None => None,
            }
            .or_else(|| {
                st.controlled
                    .first()
                    .and_then(|c| Self::find_character(&st.characters, c))
            });
            match found {
                Some((key, c)) => (
                    key.clone(),
                    serde_json::to_value(c).unwrap_or(Value::Null),
                    ActorRef { id: c.template_id.clone(), name: c.name.clone() },
                    scene,
                ),
                None => (
                    String::new(),
                    Value::Null,
                    ActorRef { id: String::new(), name: String::new() },
                    scene,
                ),
            }
        };
        if actor_key.is_empty() {
            self.reject("找不到可结算的角色".into(), RejectionCode::ActorNotFound);
            return;
        }
        // 冷却门（#01）：声明了 cooldown.turns 的技能在冷却期内一律驳回。
        // 提示词一直在告诉 AI「技能有冷却」，引擎就必须真的检查——否则那是幻觉规则的源头。
        if let Some(remaining) = self.cooldown_block(&actor_key, skill) {
            self.reject(
                format!("「{}」还在冷却中（剩余 {remaining} 回合）", skill.name),
                RejectionCode::CooldownActive,
            );
            return;
        }
        // 目标（修 D8）：模板 id / 角色名 / 实例键 / instance_id 都要能命中；
        // 解析不到时保留原始串（可能是非角色实体），行为与旧版一致。
        let (target_key, target_json) = match target_id {
            Some(t) => {
                let st = self.state.lock().expect("state poisoned");
                match Self::find_character(&st.characters, t) {
                    Some((key, c)) => (Some(key.clone()), serde_json::to_value(c).ok()),
                    None => (Some(t.to_string()), None),
                }
            }
            None => (None, None),
        };
        let global_checker = self.rules.global_checker();
        // 难度（修 D6）：world.check.default_dc → 12（不再硬编码 12）。
        let difficulty = global_checker.as_ref().and_then(|c| c.default_dc).unwrap_or(12);
        let lua_ctx = LuaHostContext {
            script_id: format!("skill:{}", skill.id),
            actor_id: actor_key.clone(),
            actor: actor_json.clone(),
            target_id: target_key.clone(),
            target: target_json.clone(),
            skill: serde_json::to_value(skill).ok(),
            scene_id,
            round: self.round.load(Ordering::SeqCst),
            difficulty: Some(difficulty),
            // 时序只读快照（#GAP-I）：规则包在挂载点读 host.turn 判断「何时该失去回合」。
            turn: self.turn_snapshot(),
            relationships: vec![],
            present: vec![],
            controlled: String::new(),
        };
        // 注册表锁只在结算期间持有：Lua 写请求（trigger_event 等）在锁释放后才落状态，
        // 否则事件回到 dispatch_lua_event 再取同一把锁即死锁。
        let settled = self.with_mount_gate(&lua_ctx, |gate| {
            let registry = self.lua_registry.lock().expect("lua registry poisoned");
            let mut ctx = CommandContext {
                actor_id: &actor_key,
                actor: &actor_json,
                target_id: target_key.as_deref(),
                target: target_json.as_ref(),
                difficulty,
                // 判定属性交给内核按优先级解析（修 D1）：技能声明 → 判定器 → 全局 → 'str'。
                attribute: None,
                global_checker: global_checker.as_ref(),
                rng: &self.rng,
                lua: Some((&self.lua, &lua_ctx)),
                registry: Some(&*registry),
                status_defs: Some(self.rules.status_defs()),
                profiles: Some(self.rules.profiles()),
                attribute_bonuses: Some(self.rules.attribute_bonuses()),
                extra_bonus: 0,
                effect_requires_success: false,
                mount_gate: gate,
            };
            match item_id {
                Some(id) => execute_item_skill(id, skill, &mut ctx),
                None => execute_skill(skill, &mut ctx),
            }
        });
        let outcome = match settled {
            Ok(o) => o,
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("lua_error".into()),
                    text: e.to_string(),
                }));
                return;
            }
        };
        if let Some(code) = outcome.rejection {
            self.reject(format!("使用「{}」被驳回", skill.name), code);
            return;
        }
        // RNG 消耗不再在此单独落条：emit 统一在发事件前把新增消耗补成 rng_consume
        // 事件（#06 ②），技能 / 判定 / 状态 tick / Lua 等所有路径一视同仁、不漏不重。
        // 结算主体与 CheckResult / Resolution 的署名一致（修 D7：不再「算在受控角色头上、
        // 叙事却署名别人」）。
        if let Some(check) = &outcome.check {
            self.emit(
                PlayEvent::CheckResult(CheckResultPayload {
                    intent_id: None,
                    actor: actor_ref.clone(),
                    attribute: check.attribute.clone(),
                    expr: check.expr.clone(),
                    rolls: if check.rolls.is_empty() { None } else { Some(check.rolls.clone()) },
                    r#mod: check.r#mod,
                    total: check.total,
                    target: check.target,
                    margin: check.margin,
                    result: check.result,
                    level: check.level,
                    opponent: None,
                    kind: Some(check.kind),
                }),
                Some(actor_ref.clone()),
                None,
            );
        }
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: outcome.narrative.clone(),
                outcome: outcome.outcome.clone(),
                triggered_events: None,
                state_changes: outcome.effects.deltas.clone(),
            }),
            Some(actor_ref),
            None,
        );
        // 结算成功才记冷却起点（#01）：驳回（含冷却未好）不算「用过」。
        self.record_cooldown(&actor_key, skill);
        // Lua 钩子的写请求（消耗 / 施加与移除状态 / 触发事件）真正落到世界状态。
        self.apply_lua_requests(&outcome.requests, &actor_key);
    }

    /// 回合边界 tick：turns 单位的持续状态减 1，归零移除（#12 ④）。
    ///
    /// 同时是 Lua 时机（「规则集走 Lua」⑤ 时机原语）：结算**前**逐 (角色, 状态) 派发
    /// `status_tick`，结算**后**派发一次 `turn_end`。
    /// 边界放在结算后，规则包在此新加的状态才不会被同一次 tick 立刻递减掉。
    fn tick_statuses_turn(&self) {
        self.tick_statuses(true);
        self.tick_natural_recovery(crate::recovery::TickKind::Turn);
        self.dispatch_tick_boundary(LuaMount::TurnEnd);
    }

    /// 场景切换 tick：scenes 单位的持续状态减 1；Lua 时机为 `status_tick` + `scene_end`。
    fn tick_statuses_scene(&self) {
        self.tick_statuses(false);
        self.tick_natural_recovery(crate::recovery::TickKind::Scene);
        self.dispatch_tick_boundary(LuaMount::SceneEnd);
    }

    /// 边界 tick 的自然恢复（#12 ④）：\`natural_recovery.trigger = per_turn / per_scene\`
    /// 在对应边界给**每个角色实例**补量，上限取资源声明的 \`default_max\`。
    ///
    /// 未声明这两类触发的故事书在这里什么都不做、也不发事件——与旧行为逐字一致。
    /// 先在锁内快照 (实例键, 资源)，锁外结算：\`emit\` 要再取同一把锁。
    fn tick_natural_recovery(&self, tick: crate::recovery::TickKind) {
        let world_resources = self
            .rules
            .storybook
            .get("world")
            .and_then(|w| w.get("resources"))
            .cloned()
            .unwrap_or(Value::Null);
        let actors: Vec<(String, Value)> = {
            let st = self.state.lock().expect("state poisoned");
            st.characters
                .iter()
                .map(|(k, c)| (k.clone(), Value::Object(c.resources.clone())))
                .collect()
        };
        let mut changes: Vec<StateDelta> = Vec::new();
        for (actor_id, resources) in actors {
            changes.extend(crate::recovery::tick_deltas(
                &actor_id,
                &world_resources,
                &resources,
                tick,
            ));
        }
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
    }

    /// 边界时机（`turn_end` / `scene_end`）：每个边界派发一次，actor = 受控角色。
    ///
    /// 引擎只提供时机与事实；要不要在此做事、做什么，全由规则包的 Lua 决定
    ///（引擎不认识任何规则集语义，也不会替规则包结算任何东西）。
    fn dispatch_tick_boundary(&self, mount: LuaMount) {
        if !self.has_mount(mount) {
            return;
        }
        let controlled = self
            .state
            .lock()
            .map(|st| st.controlled.first().cloned().unwrap_or_default())
            .unwrap_or_default();
        let ctx = self.tick_lua_context(format!("{}:{}", mount.as_str(), controlled), &controlled);
        match self.run_mount_chain(mount, &ctx, None, None) {
            Ok(requests) => self.apply_lua_requests(&requests, &controlled),
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some(format!("{}_lua_error", mount.as_str())),
                    text: format!("{} 挂载点脚本执行失败：{e}", mount.as_str()),
                }));
            }
        }
    }

    /// 单个 (角色, 状态) 的结算时机：把状态实例快照交给 `status_tick` 脚本。
    ///
    /// 引擎只派发「谁、哪个状态、还剩多久」；脚本要不要在此结束这个状态、
    /// 用什么方式决定，都是规则包自己的事。
    fn dispatch_status_tick(&self, char_key: &str, status: &StatusInstance, left: i32, turn: bool) {
        let ctx = self.tick_lua_context(format!("status_tick:{char_key}:{}", status.id), char_key);
        let snapshot = LuaStatusContext {
            id: status.id.clone(),
            name: status.name.clone(),
            turns_left: status.turns_left,
            scenes_left: status.scenes_left,
            unit: if turn { "turns" } else { "scenes" },
            remaining: left - 1,
        };
        match self.run_mount_chain(LuaMount::StatusTick, &ctx, None, Some(&snapshot)) {
            Ok(requests) => self.apply_lua_requests(&requests, char_key),
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("status_tick_lua_error".into()),
                    text: format!("status_tick 挂载点脚本执行失败：{e}"),
                }));
            }
        }
    }

    /// 状态 tick / 边界时机的 Lua 上下文：actor = 被结算的角色实例（含其状态与资源）。
    fn tick_lua_context(&self, script_id: String, char_key: &str) -> LuaHostContext {
        let (scene_id, actor) = {
            let st = self.state.lock().expect("state poisoned");
            (
                st.scene_id.clone(),
                st.characters
                    .get(char_key)
                    .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null),
            )
        };
        LuaHostContext {
            script_id,
            actor_id: char_key.to_string(),
            actor,
            scene_id,
            round: self.round.load(Ordering::SeqCst),
            // 时序快照（#GAP-I）：turn_end 是规则包判断「何时失去回合」的主要时机。
            turn: self.turn_snapshot(),
            relationships: self.rules.relationships(),
            ..Default::default()
        }
    }

    /// 某角色身上某状态在本 tick 单位下的剩余量；
    /// 状态已被脚本移除（或不再按这个单位计时）时返回 None。
    fn status_left(&self, char_key: &str, status_id: &str, turn: bool) -> Option<i32> {
        let st = self.state.lock().expect("state poisoned");
        let c = st.characters.get(char_key)?;
        c.statuses
            .iter()
            .find(|s| s.id == status_id)
            .and_then(|s| if turn { s.turns_left } else { s.scenes_left })
    }

    /// 某挂载点是否注册了脚本（没有 → 整条路径零额外开销、零行为变化）。
    fn has_mount(&self, mount: LuaMount) -> bool {
        self.lua_registry
            .lock()
            .map(|registry| registry.for_mount(mount).next().is_some())
            .unwrap_or(false)
    }

    fn tick_statuses(&self, turn: bool) {
        let status_defs = self.rules.status_defs();
        // 先快照本次要结算的 (角色, 状态) 列表，再逐个结算：派发 Lua 时不能持有状态锁
        //（脚本的写请求要落状态，持锁即死锁）。快照不改变结算顺序，也不改变骰序。
        let pending: Vec<(String, StatusInstance)> = {
            let st = self.state.lock().expect("state poisoned");
            st.characters
                .iter()
                .flat_map(|(char_id, c)| {
                    c.statuses
                        .iter()
                        .filter(|s| if turn { s.turns_left.is_some() } else { s.scenes_left.is_some() })
                        .map(|s| (char_id.clone(), s.clone()))
                })
                .collect()
        };
        // 没有 status_tick 脚本 → 与旧行为逐字一致：不构造上下文、不派发、不多掷一颗骰。
        let has_tick_scripts = self.has_mount(LuaMount::StatusTick);
        let mut changes: Vec<StateDelta> = Vec::new();
        for (char_id, status) in pending {
            let Some(mut left) = (if turn { status.turns_left } else { status.scenes_left }) else {
                continue;
            };
            if has_tick_scripts {
                self.dispatch_status_tick(&char_id, &status, left, turn);
                // 脚本可能已移除该状态：以最新状态为准，既不重复结算，
                // 也不让引擎的递减 delta 把它写回来。
                match self.status_left(&char_id, &status.id, turn) {
                    Some(current) => left = current,
                    None => continue,
                }
            }
            // 先结算状态自身的持续效果（#12 ③）：每经过一个 duration 单位结算一次。
            if let Some(def) = status_defs.get(&status.id) {
                if let Some(effects) = def.effect.as_ref().filter(|e| !e.is_empty()) {
                    let resolved = {
                        let mut rng = self.rng.lock().expect("rng poisoned");
                        resolve_immediate(effects, &char_id, &mut rng)
                    };
                    match resolved {
                        Ok(mut deltas) => changes.append(&mut deltas),
                        Err(e) => {
                            self.emit_simple(PlayEvent::System(SystemPayload {
                                level: SystemLevel::Error,
                                code: Some("status_effect_error".into()),
                                text: e.to_string(),
                            }));
                        }
                    }
                }
            }
            if left <= 1 {
                changes.push(StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: char_id,
                    field: "status".into(),
                    op: DeltaOp::Remove,
                    value: Value::String(status.id.clone()),
                });
            } else {
                let mut next = status.clone();
                if turn {
                    next.turns_left = Some(left - 1);
                } else {
                    next.scenes_left = Some(left - 1);
                }
                changes.push(StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: char_id,
                    field: "status".into(),
                    op: DeltaOp::Set,
                    value: serde_json::to_value(next).unwrap_or(Value::Null),
                });
            }
        }
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
        // 状态 tick 也可能掷骰（每 tick 效果 / 脚本次骰）：即使没有状态变更也要补记消耗（#06 ②）。
        self.flush_rng_consumption();
    }

    /// 把只读世界事实（GAP-A：角色实例 / 标记 / 遭遇）注入 Lua 宿主，供脚本按 id 查表。
    ///
    /// 引擎**不理解**这些数据的语义，只做搬运：把存档里的就绪事实原样转成 JSON 快照，
    /// 由 `host.get_character` / `get_flag` / `list_flags` / `get_encounter` /
    /// `list_encounters` 查表。**只读**：不产生任何 LuaRequest、不改世界状态。
    ///
    /// 注册表为空 = 没有任何脚本会读它 → 不构造、不注入（零开销、零行为变化）。
    /// 与 `LuaHostContext` 无关（那是跨 crate 字面量构造契约），走 `set_world_facts` 独立注入。
    fn refresh_lua_world_facts(&self, st: &WorldState) {
        let has_scripts = self
            .lua_registry
            .lock()
            .map(|registry| !registry.is_empty())
            .unwrap_or(false);
        if !has_scripts {
            return;
        }
        let facts = serde_json::json!({
            "characters": serde_json::to_value(&st.characters).unwrap_or(Value::Null),
            "flags": serde_json::to_value(&st.flags).unwrap_or(Value::Null),
            "encounters": serde_json::to_value(&st.encounters).unwrap_or(Value::Null),
        });
        self.lua.set_world_facts(facts);
    }

    /// 挂载点 `when` 条件求值的只读世界快照。
    fn mount_world(&self, actor_key: &str) -> MountWorld {
        let (flags, goals, triggers, actor_location, actor_attributes, scene_id, encounters) = {
            let st = self.state.lock().expect("state poisoned");
            // 跑脚本前刷新只读世界事实（GAP-A）：没有挂载点脚本时不构造（零开销）。
            self.refresh_lua_world_facts(&st);
            let hit = if actor_key.is_empty() {
                None
            } else {
                Self::find_character(&st.characters, actor_key)
            };
            (
                st.flags.clone(),
                st.progress.goals.clone(),
                st.progress.triggers.clone(),
                hit.and_then(|(_, c)| c.location_id.clone()),
                hit.map(|(_, c)| c.attributes.clone()),
                st.scene_id.clone(),
                st.encounters.values().cloned().collect::<Vec<Value>>(),
            )
        };
        MountWorld {
            flags,
            goals,
            triggers,
            actor_location,
            actor_attributes,
            relationships: self.rules.relationships(),
            scene_id: Some(scene_id),
            encounters,
        }
    }

    /// 用当前世界快照构造挂载点 `when` 闸门，并在闸门存活期内执行回调。
    ///
    /// 引擎侧只做「条件成立才跑」这一件事；条件本身是作者的声明（复用 CondExpr）。
    fn with_mount_gate<R>(
        &self,
        lua_ctx: &LuaHostContext,
        f: impl FnOnce(Option<&dyn Fn(&CondExpr) -> bool>) -> R,
    ) -> R {
        let world = self.mount_world(&lua_ctx.actor_id);
        let eval = EvalContext {
            flags: &world.flags,
            goals: &world.goals,
            triggers: &world.triggers,
            actor_location: world.actor_location.as_deref(),
            actor_attributes: world.actor_attributes.as_ref(),
            relationships: &world.relationships,
            scene_id: world.scene_id.as_deref(),
            encounters: &world.encounters,
            lua: Some((&self.lua, lua_ctx)),
        };
        // 条件求值出错按「不成立」处理：坏条件不该中断整轮结算（与骨架求值同口径）。
        let gate = |cond: &CondExpr| eval_cond(cond, &eval).unwrap_or(false);
        f(Some(&gate))
    }

    /// 跑一个判定挂载点链并取回写请求（不经 command 的入口——Intent::Check——用）。
    ///
    /// 注册表锁只在链执行期间持有：链里的 trigger_event 请求会在锁释放后才派发，
    /// 否则事件回到 dispatch_lua_event 再取同一把锁即死锁。
    fn run_mount_chain(
        &self,
        mount: LuaMount,
        lua_ctx: &LuaHostContext,
        check: Option<&LuaCheckContext>,
        status: Option<&LuaStatusContext>,
    ) -> Result<Vec<LuaRequest>, EngineError> {
        // 没有该挂载点的脚本 → 连世界快照都不构造（旧故事书零额外开销、零行为变化）。
        if !self.has_mount(mount) {
            return Ok(Vec::new());
        }
        self.with_mount_gate(lua_ctx, |gate| {
            let registry = self.lua_registry.lock().expect("lua registry poisoned");
            let env = MountEnv { gate, check, event: None, resolved_effects: None };
            match registry.run_chain_status(&self.lua, mount, lua_ctx, &env, status) {
                Ok(_) => Ok(self.lua.drain_requests()),
                Err(e) => {
                    let _ = self.lua.drain_requests();
                    Err(e)
                }
            }
        })
    }

    /// 注册 Lua 挂载点脚本（供 API / 测试接线）。
    pub fn register_lua_hook(&self, id: &str, mount: LuaMount, source: &str) {
        if let Ok(mut registry) = self.lua_registry.lock() {
            registry.register(id, mount, source);
        }
    }

    /// 事件广播（#04 Commit 后）：声明式 effect.triggers → 条件 → 立即效果；
    /// 以及 Lua Event 挂载点脚本链（#02 ③）。
    pub fn dispatch_event(&self, event: &str) {
        let depth = self.dispatch_depth.fetch_add(1, Ordering::SeqCst);
        if depth >= 8 {
            self.dispatch_depth.fetch_sub(1, Ordering::SeqCst);
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Warn,
                code: Some("event_recursion_limit".into()),
                text: format!("事件递归过深，已停止广播：{event}"),
            }));
            return;
        }
        self.dispatch_declarative_triggers(event);
        self.dispatch_lua_event(event);
        self.dispatch_depth.fetch_sub(1, Ordering::SeqCst);
    }

    fn dispatch_declarative_triggers(&self, event: &str) {
        let triggers: Vec<(String, EffectTrigger)> = self
            .rules
            .effect_triggers()
            .into_iter()
            // 事件名归一：规范 scene 与旧 scene_change 互为别名（决策 #11），
            // 无论派发名还是故事书里写的是哪个，都能命中。
            .filter(|(_, trigger)| normalize_event_name(&trigger.event) == normalize_event_name(event))
            .collect();
        if triggers.is_empty() {
            return;
        }

        let (flags, goals, prog_triggers, actor_id, actor_attrs, actor_loc, scene_id, encounters) = {
            let st = self.state.lock().expect("state poisoned");
            // 触发点条件里的 Lua 也用同一批只读事实（GAP-A）：跑之前刷新。
            self.refresh_lua_world_facts(&st);
            let actor_id = st.controlled.first().cloned().unwrap_or_default();
            let actor = st.characters.get(&actor_id);
            (
                st.flags.clone(),
                st.progress.goals.clone(),
                st.progress.triggers.clone(),
                actor_id,
                actor.map(|c| c.attributes.clone()),
                actor.and_then(|c| c.location_id.clone()),
                st.scene_id.clone(),
                st.encounters.values().cloned().collect::<Vec<Value>>(),
            )
        };
        let relationships = self.rules.relationships();
        let mut actor_obj = serde_json::Map::new();
        if let Some(attrs) = &actor_attrs {
            actor_obj.insert("attributes".to_string(), Value::Object(attrs.clone()));
        }
        let lua_ctx = LuaHostContext {
            script_id: format!("trigger:{event}"),
            actor: Value::Object(actor_obj),
            round: self.round.load(Ordering::SeqCst),
            turn: self.turn_snapshot(),
            relationships: relationships.clone(),
            ..Default::default()
        };
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &prog_triggers,
            actor_location: actor_loc.as_deref(),
            actor_attributes: actor_attrs.as_ref(),
            relationships: &relationships,
            scene_id: Some(scene_id.as_str()),
            encounters: &encounters,
            lua: Some((&self.lua, &lua_ctx)),
        };

        let mut changes: Vec<StateDelta> = Vec::new();
        for (_skill_id, trigger) in triggers {
            let passed = match &trigger.condition {
                Some(cond) => eval_cond(cond, &ctx).unwrap_or(false),
                None => true,
            };
            if !passed {
                continue;
            }
            let resolved = {
                let mut rng = self.rng.lock().expect("rng poisoned");
                resolve_immediate(&trigger.effects, &actor_id, &mut rng)
            };
            match resolved {
                Ok(mut deltas) => changes.append(&mut deltas),
                Err(e) => {
                    self.emit_simple(PlayEvent::System(SystemPayload {
                        level: SystemLevel::Error,
                        code: Some("trigger_error".into()),
                        text: e.to_string(),
                    }));
                }
            }
        }
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
        // 触发效果也可能掷骰：没有状态变更时同样要补记 RNG 消耗（#06 ②）。
        self.flush_rng_consumption();
    }

    fn dispatch_lua_event(&self, event: &str) {
        self.dispatch_lua_event_with(event, None);
    }

    /// 派发一个带**只读事实快照**的 Lua 事件（Event 挂载点）。
    ///
    /// 引擎只把「事件名 + 相关事实」原样交给脚本（`host.event_name` / `host.event_data`），
    /// 不解释数据内容；没有声明 event 脚本时直接返回（零开销、零行为变化）。
    fn dispatch_lua_event_with(&self, event: &str, data: Option<&Value>) {
        if !self.has_mount(LuaMount::Event) {
            return;
        }
        let (scene_id, controlled) = self
            .state
            .lock()
            .map(|st| (st.scene_id.clone(), st.controlled.first().cloned().unwrap_or_default()))
            .unwrap_or_default();
        let lua_ctx = LuaHostContext {
            script_id: format!("event:{event}"),
            actor_id: controlled.clone(),
            actor: Value::Null,
            scene_id,
            round: self.round.load(Ordering::SeqCst),
            // 时序快照（#GAP-I）：突袭等战斗开局逻辑在事件挂载点里读它。
            turn: self.turn_snapshot(),
            ..Default::default()
        };
        // 走统一入口：注册表锁只在链执行期间持有（写请求在锁释放后落状态），
        // 且 `when` 条件与世界快照一并生效。
        match self.run_event_chain(&lua_ctx, event, data) {
            // Lua 事件脚本的写请求同样落状态（此前被直接丢弃）。
            Ok(requests) => self.apply_lua_requests(&requests, &controlled),
            Err(e) => {
                // fail-fast：报错即不落该链的任何请求（与命令侧一致）。
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("event_lua_error".into()),
                    text: e.to_string(),
                }));
            }
        }
    }

    /// 事件挂载点链（带通用事件上下文）。
    ///
    /// 与 `run_mount_chain` 同一条执行路径，区别只是把事件名与事实快照放进 `MountEnv`。
    /// 注册表锁只在链执行期间持有：链里的 trigger_event 请求会在锁释放后才派发，
    /// 否则事件回到 dispatch_lua_event 再取同一把锁即死锁。
    fn run_event_chain(
        &self,
        lua_ctx: &LuaHostContext,
        name: &str,
        data: Option<&Value>,
    ) -> Result<Vec<LuaRequest>, EngineError> {
        if !self.has_mount(LuaMount::Event) {
            return Ok(Vec::new());
        }
        self.with_mount_gate(lua_ctx, |gate| {
            let registry = self.lua_registry.lock().expect("lua registry poisoned");
            let env = MountEnv {
                gate,
                check: None,
                event: Some(LuaEventContext { name, data }),
                resolved_effects: None,
            };
            match registry.run_chain_with(&self.lua, LuaMount::Event, lua_ctx, &env) {
                Ok(_) => Ok(self.lua.drain_requests()),
                Err(e) => {
                    let _ = self.lua.drain_requests();
                    Err(e)
                }
            }
        })
    }

    /// 把 Lua 写请求落到世界状态（请求-校验-执行；状态变更走 StateUpdate 权威事件）。
    /// `default_actor` 用于脚本未显式给 target 的请求（技能钩子 = 施法者）。
    fn apply_lua_requests(&self, requests: &[LuaRequest], default_actor: &str) {
        if requests.is_empty() {
            return;
        }
        let mut changes: Vec<StateDelta> = Vec::new();
        let mut events: Vec<String> = Vec::new();
        for req in requests {
            match req {
                LuaRequest::Cost { resource, amount } => {
                    if !default_actor.is_empty() && *amount != 0 {
                        changes.push(StateDelta {
                            domain: DeltaDomain::Character,
                            entity_id: default_actor.to_string(),
                            field: format!("resources.{resource}"),
                            op: DeltaOp::Add,
                            value: Value::from(-*amount),
                        });
                    }
                }
                LuaRequest::ApplyStatus { target, status, duration, unit } => {
                    let entity = if target.is_empty() { default_actor } else { target.as_str() };
                    if entity.is_empty() {
                        continue;
                    }
                    let instance =
                        build_status_instance(status, *duration, unit, self.rules.status_defs());
                    changes.push(status_delta(entity, &instance));
                }
                LuaRequest::RemoveStatus { target, status } => {
                    let entity = if target.is_empty() { default_actor } else { target.as_str() };
                    if entity.is_empty() {
                        continue;
                    }
                    changes.push(StateDelta {
                        domain: DeltaDomain::Character,
                        entity_id: entity.to_string(),
                        field: "status".into(),
                        op: DeltaOp::Remove,
                        value: Value::String(status.clone()),
                    });
                }
                LuaRequest::TriggerEvent { event, .. } => events.push(event.clone()),
                // query_world 交由引擎解释，v1 暂为 no-op。
                LuaRequest::QueryWorld { .. } => {}
                // 施加即时效果：与声明式 ImmediateEffect 走同一条 resolve_effect 路径
                //（骰子数量在这里消耗引擎 RNG，随后的 emit 会补记 rng_consume）。
                LuaRequest::ApplyEffect { target, effect } => {
                    let immediate = match serde_json::from_value::<ImmediateEffect>(effect.clone())
                    {
                        Ok(e) => e,
                        Err(e) => {
                            self.emit_simple(PlayEvent::System(SystemPayload {
                                level: SystemLevel::Warn,
                                code: Some("lua_effect_invalid".into()),
                                text: format!("apply_effect 形状非法，已忽略：{e}"),
                            }));
                            continue;
                        }
                    };
                    // 标记是**世界级键**（delta 的 entity_id = flag 名），与角色实体无关：
                    // 目标解析不到也照常落 delta（与声明式 resolve_immediate 同口径）；
                    // 其余效果仍要求解析出实体（保持旧行为）。
                    let entity = self.resolve_lua_target(target, default_actor);
                    if entity.is_empty() && !matches!(&immediate, ImmediateEffect::SetFlag { .. }) {
                        continue;
                    }
                    let def = EffectDef {
                        immediate: Some(vec![immediate]),
                        ..Default::default()
                    };
                    // 叠加策略基座：目标当前的同名状态（#12 ③）；读不到即空表。
                    let current_statuses: Vec<StatusInstance> = {
                        let st = self.state.lock().expect("state poisoned");
                        st.characters
                            .get(&entity)
                            .map(|c| c.statuses.clone())
                            .unwrap_or_default()
                    };
                    let resolved = {
                        let mut rng = self.rng.lock().expect("rng poisoned");
                        resolve_effect(
                            &def,
                            &entity,
                            &entity,
                            &mut rng,
                            self.rules.status_defs(),
                            &current_statuses,
                        )
                    };
                    match resolved {
                        Ok(out) => changes.extend(out.deltas),
                        Err(e) => {
                            self.emit_simple(PlayEvent::System(SystemPayload {
                                level: SystemLevel::Warn,
                                code: Some("lua_effect_error".into()),
                                text: format!("apply_effect 结算失败，已忽略：{e}"),
                            }));
                        }
                    }
                }
                // 加减资源：可正可负、可指定任意目标（不受「当前 actor 消耗」限制）。
                LuaRequest::ModifyResource { target, resource, amount } => {
                    let entity = self.resolve_lua_target(target, default_actor);
                    if entity.is_empty() || *amount == 0 {
                        continue;
                    }
                    changes.push(StateDelta {
                        domain: DeltaDomain::Character,
                        entity_id: entity,
                        field: format!("resources.{resource}"),
                        op: DeltaOp::Add,
                        value: Value::from(*amount),
                    });
                }
                // 时序原语（#GAP-I）：让目标跳过 N 个时序回合（突袭 / 定身… 由规则包判断）。
                LuaRequest::SkipTurn { target, turns } => {
                    let entity = self.resolve_lua_target(target, default_actor);
                    if entity.is_empty() || *turns == 0 {
                        continue;
                    }
                    // 与既有跳过数**累加**：两条脚本各跳 1 回合 = 跳 2 回合，不互相覆盖。
                    let base = self
                        .state
                        .lock()
                        .expect("state poisoned")
                        .turn
                        .skip
                        .get(&entity)
                        .copied()
                        .unwrap_or(0);
                    changes.push(turn_delta(
                        &format!("turn/skip.{entity}"),
                        DeltaOp::Set,
                        serde_json::json!(base.saturating_add(*turns)),
                    ));
                }
                // 时序原语（#GAP-I）：改某个预算槽（0 = 本轮不能再花这个预算）。
                LuaRequest::SetBudget { target, budget, amount } => {
                    let entity = self.resolve_lua_target(target, default_actor);
                    let budget = budget.trim();
                    if entity.is_empty() || budget.is_empty() {
                        continue;
                    }
                    changes.push(turn_delta(
                        &format!("turn/budget.{entity}.{budget}"),
                        DeltaOp::Set,
                        serde_json::json!((*amount).max(0)),
                    ));
                }
                // 判定修正只在判定挂载点（check_pre_roll / check_post_roll）被消费；
                // 其他时机抛出这类请求没有判定可改，忽略。
                LuaRequest::ModifyCheck { .. } => {}
                // 效果缩放 / 效果门都在技能结算路径（command::execute_skill）被消费；
                // 判定意图（Intent::Check）没有效果可结算，这里忽略。
                LuaRequest::ScaleEffect { .. } => {}
                LuaRequest::ForceEffect => {}
            }
        }
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
        for event in events {
            self.dispatch_event(&event);
        }
        // 效果可能掷骰：没有状态变更时也要把消耗记进日志（与状态 tick 同口径）。
        self.flush_rng_consumption();
    }

    /// Lua 请求的实体寻址：任意 id 形态（实例键 / 模板 id / 名字 / instance_id）→ 存档键；
    /// 解析不到时保留原串（可能是敌人 / 非角色实体），与既有 ApplyStatus 的行为一致。
    fn resolve_lua_target(&self, target: &str, default_actor: &str) -> String {
        let raw = if target.trim().is_empty() { default_actor } else { target.trim() };
        if raw.is_empty() {
            return String::new();
        }
        let st = self.state.lock().expect("state poisoned");
        Self::find_character(&st.characters, raw)
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| raw.to_string())
    }

    /// 对抗判定的对手值（判定 C3）：返回（对手署名，对手 total）。
    ///
    /// 对手的两种形态——LMoP 的三种组合由「主动方 kind × 对手形态」组合出来：
    /// - **角色实例**（模板 id / 名字 / 实例键 / 实例 id 都能命中）→ 对手也掷一次骰：
    ///   同一骰式、`opposed_attribute`（缺省 = 主动方属性）、它自己的属性值 + 修正来源。
    ///   主动方 kind = passive 时即「被动 vs 掷」。
    /// - **静态被动值**（命中不了实例）→ 不掷骰：`passive_base`（缺省 10）+ 属性修正
    ///   （属性值取故事书 characters[] 里同名条目声明的值；查不到按 0 修正）。即「掷 vs 被动」。
    ///
    /// 引擎不认识「被动察觉」这类规则词——它只知道这一侧掷不掷骰。
    fn opponent_total(
        &self,
        opponent_id: &str,
        attribute: &str,
        checker: Option<&CheckerDef>,
        difficulty: i64,
    ) -> Result<(ActorRef, i64), EngineError> {
        // 对手属性：判定器声明的 opposed_attribute；缺省与主动方同属性。
        let opposed_attribute = checker
            .and_then(|c| c.opposed_attribute.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| attribute.to_string());
        let mut opponent_checker = checker.cloned().unwrap_or_else(|| CheckerDef {
            dice: Some("1d20".into()),
            ..Default::default()
        });
        let instance = {
            let st = self.state.lock().expect("state poisoned");
            Self::find_character(&st.characters, opponent_id).map(|(_, c)| c.clone())
        };
        let Some(instance) = instance else {
            // 静态被动值：没有骰子，只有基数 + 属性修正。
            let base = opponent_checker.passive_base.unwrap_or(10);
            let declared = self.rules.character_attribute(opponent_id, &opposed_attribute);
            let name = declared
                .as_ref()
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| opponent_id.to_string());
            let bonus = declared
                .map(|(_, v)| {
                    let profile = self
                        .rules
                        .profiles()
                        .get(&opposed_attribute)
                        .copied()
                        .unwrap_or_default();
                    crate::resolve::modifier_for(&opponent_checker, &opposed_attribute, v, profile)
                })
                .unwrap_or(0);
            return Ok((ActorRef { id: opponent_id.to_string(), name }, base + bonus));
        };
        let sign = ActorRef { id: instance.template_id.clone(), name: instance.name.clone() };
        let base = instance
            .attributes
            .get(&opposed_attribute)
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
            .unwrap_or(crate::resolve::DEFAULT_BASELINE);
        let value = self
            .rules
            .attribute_bonuses()
            .get(&instance.template_id)
            .and_then(|m| m.get(&opposed_attribute))
            .map(|m| m.apply(base))
            .unwrap_or(base);
        let profile = self
            .rules
            .profiles()
            .get(&opposed_attribute)
            .copied()
            .unwrap_or_default();
        // 对手侧固定掷骰（它的被动形态走上面那条静态被动值）；主动方的 lua 判定器上下文
        // 属于主动方，不套到对手头上——否则脚本会以对手的身份读到主动方的上下文。
        opponent_checker.kind = Some(CheckKind::Attribute);
        opponent_checker.mode = Some(CheckMode::Gte);
        opponent_checker.lua = None;
        let resolved = crate::command::run_check(
            &opponent_checker,
            &opposed_attribute,
            value,
            difficulty,
            profile,
            &self.rng,
            None,
            crate::command::RollPolicy::Single,
        )?;
        Ok((sign, resolved.total))
    }

    /// 结算一次判定。成功结算时返回结果摘要，供回合内续轮回喂模型（#04 ⑦）；
    /// 取消 / 出错没有可续写的信息，返回 None。
    ///
    /// `opponent_id`（判定 C3）：给了对手就是**对抗判定**——双方各得一次值比大小，
    /// 不再与静态难度比。对手是什么见 [`Session::opponent_total`]。
    async fn run_check(
        &self,
        attribute: String,
        difficulty: Option<i64>,
        opponent_id: Option<String>,
        actor: Option<ActorRef>,
    ) -> Option<String> {
        // 判定归属：意图指定 → 受控角色（是玩家在掷骰）。
        // 绝不回落到种子/测试角色；真找不到人时用中性占位。
        let actor = actor
            .or_else(|| self.controlled_actor())
            .unwrap_or(ActorRef { id: String::new(), name: "未知角色".into() });
        // 判定器来源（#12）：优先故事书 world.check；未声明时保持 1d20 默认（纯骰、无修正）。
        let declared = self.rules.global_checker();
        // #12 ④：属性必须在故事书声明的判定属性白名单里，否则明确驳回——绝不静默 0 分必失败。
        if let Some(list) = declared
            .as_ref()
            .and_then(|c| c.attributes.as_ref())
            .filter(|l| !l.is_empty())
        {
            if !list.iter().any(|k| k == &attribute) {
                self.reject(
                    format!(
                        "判定属性「{attribute}」不在故事书声明的判定属性里（可用：{}）",
                        list.join("、")
                    ),
                    RejectionCode::RuleViolation,
                );
                return None;
            }
        }
        // 判定 C3：对抗判定必须有对手。mode = opposed 却没收对手 → 显式驳回，
        // 不再静默降级成 gte（给作者一个错误，而不是一个看起来正常的错误结果）。
        let declared_mode = declared.as_ref().and_then(|c| c.mode).unwrap_or(CheckMode::Gte);
        let opponent_id = opponent_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if declared_mode == CheckMode::Opposed && opponent_id.is_none() {
            self.reject(
                "判定模式为对抗（opposed），但意图没有给出对手 opponent_id".to_string(),
                RejectionCode::RuleViolation,
            );
            return None;
        }
        if !self.auto_confirm.load(Ordering::SeqCst) {
            self.phase(PhaseStage::WaitingConfirm, None);
            let action_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            *self.pending.lock().expect("pending poisoned") = Some(Pending { action_id: action_id.clone(), tx });
            self.emit(
                PlayEvent::Pending(PendingPayload {
                    action_id: action_id.clone(),
                    intent_id: None,
                    actor: actor.clone(),
                    // 对抗判定把对手一并写进确认描述；非对抗的文案逐字不变。
                    description: match opponent_id.as_deref() {
                        Some(id) => format!("用{attribute}进行一次对抗判定（对手：{id}）"),
                        None => format!("用{attribute}进行一次判定"),
                    },
                    impact: Some("判定会立即结算，成败写入世界状态".into()),
                    timeout_ms: self.confirmation_timeout_ms,
                }),
                Some(actor.clone()),
                None,
            );
            let decision = match timeout(Duration::from_millis(self.confirmation_timeout_ms), rx).await {
                Ok(Ok(d)) => d,
                _ => {
                    self.pending.lock().expect("pending poisoned").take();
                    ConfirmDecision::Cancel
                }
            };
            if decision == ConfirmDecision::Cancel {
                self.emit(
                    PlayEvent::Resolution(ResolutionPayload {
                        intent_id: None,
                        status: ResolutionStatus::Rejected,
                        rejection_code: Some(RejectionCode::Cancelled.as_str().into()),
                        narrative: Some("你收回了动作。".into()),
                        outcome: None,
                        triggered_events: None,
                        state_changes: vec![],
                    }),
                    Some(actor),
                    None,
                );
                return None;
            }
            self.phase(PhaseStage::Resolving, None);
        }

        // 难度：意图给的优先 → 故事书 default_dc → 12。
        let mut target = difficulty
            .or_else(|| declared.as_ref().and_then(|c| c.default_dc))
            .unwrap_or(12);
        // 判定归属的存档键：挂载点写请求（状态 / 资源 / 即时效果）默认落到它头上。
        let actor_key = {
            let st = self.state.lock().expect("state poisoned");
            Self::find_character(&st.characters, &actor.id)
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| actor.id.clone())
        };
        // 判定挂载点上下文：判定前后都要用（前改骰数/难度，后改 total/margin/档位）。
        let mut mount_ctx = LuaHostContext {
            script_id: format!("check:{attribute}"),
            actor_id: actor.id.clone(),
            actor: self.actor_json_of(&actor),
            scene_id: self.state.lock().map(|st| st.scene_id.clone()).unwrap_or_default(),
            round: self.round.load(Ordering::SeqCst),
            difficulty: Some(target),
            ..Default::default()
        };
        // 判定种类在掷骰前就确定（缺省 = 属性检定）：check_pre_roll 的签名要用它。
        let signature_kind = declared
            .as_ref()
            .and_then(|c| c.kind)
            .unwrap_or(CheckKind::Attribute);
        // 签名（属性 / 种类 / 难度）在掷骰前已经确定，一并交给钩子——
        // 「优势 / 劣势只有掷骰前有意义」由此才表达得出来。
        let signature = crate::command::check_signature(&attribute, signature_kind, target);
        let mut adjustments = crate::command::CheckAdjustments::default();
        let mut lua_requests: Vec<LuaRequest> = Vec::new();
        // check_pre_roll：收集 → **掷骰前**生效（取高/取低、改难度、加值）。
        match self.run_mount_chain(LuaMount::CheckPreRoll, &mount_ctx, Some(&signature), None) {
            Ok(reqs) => lua_requests.extend(adjustments.absorb(reqs)),
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("check_lua_error".into()),
                    text: e.to_string(),
                }));
                return None;
            }
        }
        target += adjustments.dc;
        mount_ctx.difficulty = Some(target);
        let policy = adjustments.roll_policy();
        let mut resolved = match &declared {
            Some(checker) => {
                let value = self.actor_attribute_value(&actor, &attribute);
                let profile = self
                    .rules
                    .profiles()
                    .get(&attribute)
                    .copied()
                    .unwrap_or_default();
                let lua_ctx = LuaHostContext {
                    script_id: format!("check:{attribute}"),
                    actor_id: actor.id.clone(),
                    actor: self.actor_json_of(&actor),
                    scene_id: mount_ctx.scene_id.clone(),
                    round: mount_ctx.round,
                    difficulty: Some(target),
                    ..Default::default()
                };
                let lua = if checker
                    .lua
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty())
                {
                    Some((&self.lua, &lua_ctx))
                } else {
                    None
                };
                match crate::command::run_check(
                    checker,
                    &attribute,
                    value,
                    target,
                    profile,
                    &self.rng,
                    lua,
                    policy,
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        self.emit_simple(PlayEvent::System(SystemPayload {
                            level: SystemLevel::Error,
                            code: Some("check_error".into()),
                            text: e.to_string(),
                        }));
                        return None;
                    }
                }
            }
            None => {
                // 缺省路径与历史逐字一致（单次时只掷一颗 1d20，无修正，默认阈值）；
                // 取高/取低的通用策略下掷两次取优（RNG 消耗 = 2）。
                let rolls = {
                    let mut rng = self.rng.lock().expect("rng poisoned");
                    let times = if policy == crate::command::RollPolicy::Single { 1 } else { 2 };
                    (0..times).map(|_| rng.range_inclusive(1, 20)).collect::<Vec<i64>>()
                };
                let roll = match policy {
                    crate::command::RollPolicy::KeepHigh => rolls.iter().copied().max().unwrap_or(0),
                    crate::command::RollPolicy::KeepLow => rolls.iter().copied().min().unwrap_or(0),
                    crate::command::RollPolicy::Single => rolls.first().copied().unwrap_or(0),
                };
                let margin = roll - target;
                ResolvedCheck {
                    attribute: attribute.clone(),
                    expr: Some("1d20".into()),
                    rolls: vec![roll],
                    r#mod: 0,
                    total: roll,
                    target,
                    margin,
                    result: roll >= target,
                    level: level_for_margin(margin, &DEFAULT_DEGREE_THRESHOLDS),
                    rolled: true,
                    kind: CheckKind::Attribute,
                }
            }
        };

        // 判定 C3：有对手就是对抗——对手也得过一次值（角色实例掷一次 / 静态被动值算一次），
        // 它的 total 成为本判定的 target（不是静态难度）。RNG 顺序：主动方先掷，对手后掷。
        let mut opponent: Option<(ActorRef, i64)> = None;
        if let Some(oid) = opponent_id.as_deref() {
            match self.opponent_total(oid, &attribute, declared.as_ref(), target) {
                Ok(o) => opponent = Some(o),
                Err(e) => {
                    self.emit_simple(PlayEvent::System(SystemPayload {
                        level: SystemLevel::Error,
                        code: Some("check_error".into()),
                        text: e.to_string(),
                    }));
                    return None;
                }
            }
        }
        let mode = if opponent.is_some() {
            CheckMode::Opposed
        } else {
            declared_mode
        };
        let thresholds: &[i64] = match declared.as_ref() {
            Some(checker) => crate::resolve::degree_thresholds(checker),
            None => &DEFAULT_DEGREE_THRESHOLDS,
        };
        if let Some((_, opponent_total)) = &opponent {
            crate::resolve::apply_opposed_target(&mut resolved, *opponent_total, thresholds);
        }
        // 判定前挂载点的结果覆盖（判定 C4）：与技能路径同口径，在快照之前兑现——
        // 后置挂载点读到的是覆盖之后的判定（骰面 / 总值 / 难度都不变）。
        if let Some(success) = adjustments.force {
            crate::resolve::apply_forced_result(&mut resolved, success);
        }

        // check_post_roll：收集 → **掷骰后**生效（改 total / margin / 成功度分档 / 覆盖结果）。
        let mut post = crate::command::CheckAdjustments::default();
        let check_snapshot = crate::command::check_context(&resolved);
        match self.run_mount_chain(LuaMount::CheckPostRoll, &mount_ctx, Some(&check_snapshot), None) {
            Ok(reqs) => lua_requests.extend(post.absorb(reqs)),
            Err(e) => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("check_lua_error".into()),
                    text: e.to_string(),
                }));
                return None;
            }
        }
        crate::command::apply_post_roll_adjustments(
            &mut resolved,
            adjustments.add + post.add,
            post.dc,
            mode,
            thresholds,
            post.force,
        );

        self.emit(
            PlayEvent::CheckResult(CheckResultPayload {
                intent_id: None,
                actor: actor.clone(),
                attribute: resolved.attribute.clone(),
                expr: resolved.expr.clone(),
                rolls: if resolved.rolls.is_empty() { None } else { Some(resolved.rolls.clone()) },
                r#mod: resolved.r#mod,
                total: resolved.total,
                target: resolved.target,
                margin: resolved.margin,
                result: resolved.result,
                level: resolved.level,
                // 判定 C3：对手署名落 payload（前端 CheckCard 早已能渲染）。
                opponent: opponent.as_ref().map(|(o, _)| o.clone()),
                kind: Some(resolved.kind),
            }),
            Some(actor.clone()),
            None,
        );
        // 判定结果只进 CheckResult 与 Resolution；不再硬编码写 demo flag（mine_foreshadow），
        // 结果如何演绎交给叙事层/AI（#04/#12 缺口 2）。
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(if resolved.result { "判定成功。".into() } else { "判定失败。".into() }),
                outcome: Some("check".into()),
                triggered_events: None,
                state_changes: vec![],
            }),
            Some(actor),
            None,
        );
        // 挂载点的写请求（状态 / 资源 / 即时效果 / 事件）在判定事件之后落状态。
        self.apply_lua_requests(&lua_requests, &actor_key);
        // #04 ⑦：把判定结果摘要回喂模型，让它据此续写（失败时改换策略、成功时顺势推进）。
        let dice = if resolved.rolls.is_empty() {
            resolved.expr.clone().unwrap_or_else(|| "无骰".to_string())
        } else {
            resolved.rolls.iter().map(i64::to_string).collect::<Vec<_>>().join("+")
        };
        let level = match resolved.level {
            octopus_types::SuccessLevel::Great => "大成功",
            octopus_types::SuccessLevel::Success => "成功",
            octopus_types::SuccessLevel::Barely => "险胜",
            octopus_types::SuccessLevel::Fail => "失败",
        };
        // 非对抗的摘要逐字不变（旧行为）；对抗把「难度」换成对手署名 + 对手总值。
        Some(match &opponent {
            Some((o, _)) => format!(
                "判定「{}」：{level}（骰 {dice}，修正 {}，总值 {}，对抗 {} {}）",
                resolved.attribute, resolved.r#mod, resolved.total, o.name, resolved.target
            ),
            None => format!(
                "判定「{}」：{level}（骰 {dice}，修正 {}，总值 {}，难度 {}）",
                resolved.attribute, resolved.r#mod, resolved.total, resolved.target
            ),
        })
    }
}

/// 兜底攻击选中的武器/技能：伤害骰、命中加值、可选技能。
#[derive(Debug, Clone, Default)]
struct AttackChoice {
    skill_id: Option<String>,
    damage: Option<String>,
    bonus: i64,
    /// 叙事里用的名字（武器名优先于「徒手攻击」）
    label: Option<String>,
}

/// 一次攻击结算的输入（图鉴 M2 §4.3 对称内核）：攻守双方都是角色实例。
///
/// 临时敌人（无图鉴实例）由 `strike_enemy` 传条目 JSON + 条目 id：属性为空、
/// 生命值取条目，Lua 与效果看到的 target 与改动前逐字一致。
struct AttackSetup<'a> {
    attacker_key: &'a str,
    attacker_json: &'a Value,
    target_id: Option<&'a str>,
    target_json: Option<&'a Value>,
    skill: &'a SkillDef,
    difficulty: i64,
    /// 判定属性（None = 交给内核按「技能 → 判定器 → 全局 → str」解析）。
    attribute: Option<String>,
    /// 调用方追加的固定判定修正（武器命中加值）。
    extra_bonus: i64,
    global_checker: Option<CheckerDef>,
    lua_ctx: LuaHostContext,
}

/// 合成徒手攻击的技能 id（技能未声明时用它，供 rng / 事件记账定位）。
const STRIKE_SKILL_ID: &str = "__strike__";

/// 为一次攻击组装「判定 + 伤害」的技能声明（判定 C2：攻击就是一次 execute_skill）。
///
/// - 判定器：技能内联声明原样沿用；`check: "world"` 取全局 world.check 的骰式 / 修正 / 阈值
///   （修 D3：不再静默换成裸 1d20）；两者都缺省时合成 1d20（与历史徒手默认一致）。
/// - 属性：沿用技能声明，交给内核按「技能 → 判定器 → 全局 → 'str'」解析（修 D1）。
/// - 消耗 / lua 钩子：原样保留，攻击与技能同口径。
/// - 效果：收敛为本次攻击的伤害骰（历史 strike 的结算范围），静态修正原样保留。
/// - `damage_resource`：伤害落到哪个资源（图鉴 M2）。None = 历史默认 `hp`；
///   图鉴怪物的生命资源可能叫别的（LMoP 是 `res-hp`），传它才能真的扣到血。
fn attack_skill(
    declared: Option<&SkillDef>,
    global: Option<&CheckerDef>,
    name: &str,
    damage_expr: &str,
    damage_resource: Option<&str>,
) -> SkillDef {
    let mut skill = declared.cloned().unwrap_or_default();
    if skill.id.is_empty() {
        skill.id = STRIKE_SKILL_ID.to_string();
    }
    if skill.name.is_empty() {
        skill.name = name.to_string();
    }
    let mut checker = match skill.check.as_ref() {
        Some(SkillCheck::Def(def)) => Some(def.clone()),
        Some(SkillCheck::Ref(_)) => global.cloned(),
        None => None,
    }
    .unwrap_or_else(|| CheckerDef { dice: Some("1d20".into()), ..Default::default() });
    // 本入口就是一次攻击判定：种类固定，避免继承其它判定种类的掷骰方语义。
    checker.kind = Some(CheckKind::Attack);
    skill.check = Some(SkillCheck::Def(checker));
    let modifiers = skill.effect.as_ref().and_then(|e| e.modifiers.clone());
    skill.effect = Some(EffectDef {
        immediate: Some(vec![ImmediateEffect::Damage {
            amount: damage_expr.to_string(),
            resource: damage_resource.map(str::to_string),
        }]),
        modifiers,
        ..Default::default()
    });
    skill
}

/// 实例的生命资源键（图鉴 M2）。
///
/// 约定 `hp` 优先；否则取 id 以 `-hp` / `_hp` 结尾的那一个（LMoP 导入的图鉴用 `res-hp`）。
/// 只用于读取 / 投影，**绝不写进实例**——与派生值同一条纪律：不制造第二份真相。
fn vital_resource_key(resources: &serde_json::Map<String, Value>) -> Option<String> {
    if resources.contains_key("hp") {
        return Some("hp".to_string());
    }
    let mut keys: Vec<&String> = resources.keys().collect();
    keys.sort();
    keys.iter()
        .find(|k| k.ends_with("-hp") || k.ends_with("_hp"))
        .map(|k| (*k).clone())
}

/// 读资源的数值：命中 delta 的 Add 会把整数写成浮点（27 → 27.0），读取必须两种都认。
fn resource_num(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// 技能声明的伤害骰（effect.immediate 里第一条 damage）；未声明时为空串。
fn attack_damage_expr(skill: Option<&SkillDef>) -> String {
    skill
        .and_then(|s| s.effect.as_ref())
        .and_then(|e| e.immediate.as_ref())
        .and_then(|list| {
            list.iter().find_map(|f| match f {
                ImmediateEffect::Damage { amount, .. } => Some(amount.clone()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

/// 是否攻击类技能（check.kind = attack）：图鉴数据卡的「可用攻击」只列这些。
fn is_attack_skill(skill: Option<&SkillDef>) -> bool {
    matches!(skill.and_then(|s| s.check.as_ref()), Some(SkillCheck::Def(c)) if c.kind == Some(CheckKind::Attack))
}

/// 「故事本身」的保留身份：导演通道的事件都归它，永不指向真实角色。
const STORY_ACTOR_ID: &str = "__story__";

fn story_actor() -> ActorRef {
    ActorRef { id: STORY_ACTOR_ID.into(), name: "故事".into() }
}

/// 按任意一种 id（实例键 / 模板 id / 名字 / instance_id）在状态里定位角色实例键。
/// 关系边端点用模板 id，实例状态用实例键；查询两类都得能互相解析。
fn character_key_in(st: &WorldState, id: &str) -> Option<String> {
    st.characters
        .iter()
        .find(|(key, c)| {
            key.as_str() == id || c.template_id == id || c.name == id || c.instance_id == id
        })
        .map(|(key, _)| key.clone())
}

/// 在场判据：位置驱动 + 显式覆盖（docs/map-and-presence-design.md §4.1 三层优先级）。
///
/// ① 作者点名（场景 present_char_ids 命中该模板 id）→ 在场；
/// ② 位置匹配（角色实例 location_id == 场景 location_id）→ 在场；
/// ③ 场景**既未声明** present_char_ids、**也未声明** location_id → 全员在场
///    （沿用开档的既有回落，避免无名单的旧故事书一切场就空场）；
/// 受控角色（玩家）恒在场。
///
/// named 必须是 Option：None = 场景没写这个字段（未声明），
/// Some(空集) = 作者显式声明「这一幕没人」。两者语义不同，不可合并——
/// 合并正是旧代码「缺省名单即清场」这个不一致的根源。
///
/// 开档（octopus-api 的 is_initially_present）与切场（Session::switch_scene）
/// 共用这一个函数，两处判据永远同源。
pub fn scene_presence(
    named: Option<&std::collections::HashSet<String>>,
    scene_location_id: Option<&str>,
    template_id: &str,
    character_location_id: Option<&str>,
    controlled: bool,
) -> bool {
    if controlled {
        return true;
    }
    if named.is_some_and(|list| list.contains(template_id)) {
        return true;
    }
    // 空串视为「未声明」：校验层会报错，运行期不因一个空值把全场清空。
    let scene_location_id = scene_location_id.filter(|s| !s.is_empty());
    let character_location_id = character_location_id.filter(|s| !s.is_empty());
    if let (Some(scene), Some(character)) = (scene_location_id, character_location_id) {
        if scene == character {
            return true;
        }
    }
    named.is_none() && scene_location_id.is_none()
}

/// 骨架里的一个场景定义：切场在场判据与「当前地点」提示词共用同一份解析。
struct SceneDef {
    title: String,
    description: Option<String>,
    /// 作者点名的在场名单；None = 场景**未声明**该字段（≠ 显式空名单）。
    present_char_ids: Option<std::collections::HashSet<String>>,
    /// 场景所在地点 id；None = **未声明**（空串同样视为未声明）。
    location_id: Option<String>,
}

/// 关系边是否触及给定的一组实体 id（端点兼容 from/to 与旧 from_id/to_id）。
fn edge_touches(edge: &Value, ids: &[String]) -> bool {
    ["from", "to"].iter().any(|canonical| {
        let legacy = if *canonical == "from" { "from_id" } else { "to_id" };
        relationship_endpoint(edge, canonical, legacy).is_some_and(|e| ids.iter().any(|id| id == e))
    })
}

/// 提示词里的「在场角色」：只列本场在场者（受控角色恒在场）。
/// 之前把全书角色都塞进提示词，AI 会把终局 Boss 当同场同伴演。
fn present_actors(st: &WorldState) -> Vec<ActorRef> {
    st.characters
        .iter()
        // 图鉴 M2 §4.6：怪物不进「在场角色」名单——它们出现在【当前遭遇】数据卡里。
        .filter(|(_, c)| c.kind != "monster")
        .filter(|(key, c)| {
            c.present || st.controlled.contains(key) || st.controlled.contains(&c.instance_id)
        })
        .map(|(_, c)| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
        .collect()
}

// ============================================================
// 事件 → 世界状态（实时与重放共用的唯一变更路径）
// ============================================================

/// 把一个事件投影到世界状态上。实时（`emit`）与重放（`replay`）都只经由这里，
/// 因此二者必然得到一致结果。承载状态变更的事件只有 `Scene` / `Resolution` /
/// `StateUpdate` / `System(confirm_toggle)`。
fn apply_event(state: &mut WorldState, event: &PlayEvent) {
    match event {
        PlayEvent::Scene(p) => {
            state.scene_id = p.scene_id.clone();
            state.scene_title = p.title.clone();
            state.scene_description = p.description.clone();
        }
        PlayEvent::Resolution(p) => {
            for d in &p.state_changes {
                apply_delta(state, d);
            }
        }
        PlayEvent::StateUpdate(p) => {
            for d in &p.changes {
                apply_delta(state, d);
            }
        }
        PlayEvent::System(p) => {
            if p.code.as_deref() == Some("confirm_toggle") {
                state.meta.auto_confirm = p.text.contains("开启");
            }
            // rng_consume 不改世界事实，只累加「已消耗的掷骰次数」（#06 ②）：
            // 重放据此把 RNG 拨回日志记录的位置，未来骰序与不重启一致。
            if p.code.as_deref() == Some("rng_consume") {
                if let Ok(vals) = serde_json::from_str::<Vec<u64>>(&p.text) {
                    state.rng_position = state.rng_position.saturating_add(vals.len() as u64);
                }
            }
        }
        _ => {}
    }
}

/// 落不到任何角色实例的 Character 域 delta：**只告警，不改行为**。
///
/// 这是本设计自报的主要风险形态——「不报错，只静默给出错误结论」。这类 delta
/// （最常见的来源：AI 把 entity_id 写成了不存在的键）以前被直接丢弃、不留任何痕迹，
/// 只能靠事后比对 `Resolution.state_changes` 才能发现。这里补一条 WARN，带上
/// 域 / 实体键 / 字段，让「丢弃」在日志里可见。
///
/// **不落事件**：给丢弃补一条 System 事件会让命令日志多出内容，重放路径随之产生
/// 新事件、重放一致性被破坏。所以这里只有日志，投影结果逐字不变。
///
/// **去重（重放不刷屏）**：实时（`emit`）与重放（`replay`）走的是同一条 `apply_delta`，
/// 同一批 delta 会被原样再跑一遍。按 `(域, 实体键, 字段)` 去重后，同一种丢弃在
/// **进程生命周期内只告警一次**，重放不会把它再刷一遍。表有上限，防止病态输入
/// 把它撑大；到上限后重复形态仍被识别，新形态则每次都告警。
fn warn_dropped_character_delta(d: &StateDelta) {
    /// 去重表上限：只记「形态」，不记次数，正常内容远到不了这里。
    const CAP: usize = 256;
    static SEEN: std::sync::OnceLock<Mutex<std::collections::HashSet<(String, String, String)>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let shape = (format!("{:?}", d.domain), d.entity_id.clone(), d.field.clone());
    let first_time = match seen.lock() {
        Ok(mut set) => {
            if set.contains(&shape) {
                false
            } else {
                if set.len() < CAP {
                    set.insert(shape);
                }
                true
            }
        }
        // 锁中毒不该让投影路径 panic：退化为每次都告警。
        Err(_) => true,
    };
    if !first_time {
        return;
    }
    tracing::warn!(
        domain = ?d.domain,
        entity_id = %d.entity_id,
        field = %d.field,
        op = ?d.op,
        "状态增量落不到任何角色实例，已丢弃（投影逐字不变）"
    );
}

fn apply_delta(state: &mut WorldState, d: &StateDelta) {
    match d.domain {
        DeltaDomain::Flag => match d.op {
            DeltaOp::Remove => {
                state.flags.remove(&d.entity_id);
            }
            _ => {
                state.flags.insert(d.entity_id.clone(), d.value.clone());
            }
        },
        DeltaDomain::Goal => match d.op {
            DeltaOp::Remove => {
                state.progress.goals.remove(&d.entity_id);
            }
            _ => {
                state.progress.goals.insert(d.entity_id.clone(), d.value.clone());
            }
        },
        DeltaDomain::Encounter => match d.op {
            DeltaOp::Remove => {
                state.encounters.remove(&d.entity_id);
            }
            DeltaOp::Set => {
                if let Some(e) = state.encounters.get_mut(&d.entity_id) {
                    if let Some(obj) = d.value.as_object() {
                        for (k, v) in obj {
                            e[k] = v.clone();
                        }
                    }
                }
            }
            DeltaOp::Add => {
                state.encounters.insert(d.entity_id.clone(), d.value.clone());
            }
        },
        DeltaDomain::Trigger => match d.op {
            DeltaOp::Remove => {
                state.progress.triggers.remove(&d.entity_id);
            }
            _ => {
                state.progress.triggers.insert(d.entity_id.clone(), d.value.clone());
            }
        },
        DeltaDomain::Character => {
            // 图鉴 M2：field="instance" + op=Add 插入**完整实例**（怪物克隆）。
            // 必须先于「解析已有实例键」那条路：刚插入的实例此刻还不存在，
            // 而 entity_id 就是新实例键本身。实时（emit）与重放（replay）都只经由
            // apply_delta 这一条唯一变更路径，所以二者必然一致。
            if d.field == "instance" && d.op == DeltaOp::Add {
                if let Ok(inst) = serde_json::from_value::<CharacterInstance>(d.value.clone()) {
                    state.upsert_instance(&d.entity_id, inst);
                }
                return;
            }
            // entity_id 应为存档内的实例键；历史日志里存的是模板 id，这里统一解析，
            // 保证老存档回放后 controlled / 属性 / 资源 仍指向同一个角色。
            let key = if state.characters.contains_key(&d.entity_id) {
                d.entity_id.clone()
            } else {
                state
                    .characters
                    .iter()
                    .find(|(_, c)| c.template_id == d.entity_id || c.instance_id == d.entity_id)
                    .map(|(k, _)| k.clone())
                    .unwrap_or_else(|| d.entity_id.clone())
            };
            if d.field == "controlled" {
                state.controlled = vec![key];
                return;
            }
            // 技能冷却（#01）：与角色实例字段无关，单独处理以免与下面的可变借用冲突。
            if let Some(skill_id) = d.field.strip_prefix("cooldown.") {
                match d.op {
                    DeltaOp::Remove => {
                        state.cooldowns.entry(key).or_default().remove(skill_id);
                    }
                    _ => {
                        if let Some(round) = d.value.as_u64() {
                            state
                                .cooldowns
                                .entry(key)
                                .or_default()
                                .insert(skill_id.to_string(), round as u32);
                        }
                    }
                }
                return;
            }
            let Some(c) = state.characters.get_mut(&key) else {
                // 丢弃可见化（只写日志，不落事件、不改状态）：见 warn_dropped_character_delta。
                warn_dropped_character_delta(d);
                return;
            };
            if d.field == "present" {
                c.present = d.value.as_bool().unwrap_or(false);
            } else if let Some(k) = d.field.strip_prefix("attributes.") {
                apply_map_value(&mut c.attributes, k, d);
            } else if let Some(k) = d.field.strip_prefix("resources.") {
                apply_map_value(&mut c.resources, k, d);
            } else if d.field == "location_id" {
                c.location_id = if d.value.is_null() {
                    None
                } else {
                    d.value.as_str().map(str::to_string)
                };
            } else if d.field == "status" {
                match d.op {
                    DeltaOp::Remove => {
                        let id = d.value.as_str().unwrap_or("");
                        c.statuses.retain(|s| s.id != id);
                    }
                    _ => {
                        if let Ok(status) = serde_json::from_value::<StatusInstance>(d.value.clone()) {
                            // 默认叠加策略 = replace（同名替换；add / max 待状态定义随 delta 携带）
                            if let Some(existing) = c.statuses.iter_mut().find(|s| s.id == status.id) {
                                *existing = status;
                            } else {
                                c.statuses.push(status);
                            }
                        }
                    }
                }
            }
        }
        DeltaDomain::Origin => {
            // 全量检查点：整体替换状态，但保留重放推进的 seq 与当前存档元信息
            // （revision / needs_upgrade 以存档列为准，不回到检查点时的旧值）。
            if let Ok(cp) = serde_json::from_value::<OriginCheckpoint>(d.value.clone()) {
                let seq = state.seq;
                let meta = state.meta.clone();
                *state = cp.state;
                state.seq = seq;
                state.meta = meta;
            }
        }
        // 时序域（#GAP-I）：field 约定见 DeltaDomain::Turn 的文档。
        DeltaDomain::Turn => match d.field.as_str() {
            "turn/round" => {
                if let Some(v) = d.value.as_u64() {
                    state.turn.round = v as u32;
                }
            }
            "turn/index" => {
                if let Some(v) = d.value.as_u64() {
                    state.turn.index = v as usize;
                }
            }
            "turn/order" => {
                if let Ok(v) = serde_json::from_value::<Vec<String>>(d.value.clone()) {
                    state.turn.order = v;
                    // 顺序变短后指针可能越界：夹回合法下標，绝不让它指向不存在的人。
                    if state.turn.index >= state.turn.order.len() {
                        state.turn.index = 0;
                    }
                }
            }
            other => {
                if let Some(rest) = other.strip_prefix("turn/budget.") {
                    // rest = "<实例键>.<预算id>"：预算 id 不含点（发布门校验），
                    // 所以从**最后一个点**切分永远切在真正的分隔符上。
                    if let Some((key, id)) = rest.rsplit_once('.') {
                        match d.op {
                            DeltaOp::Remove => {
                                if let Some(m) = state.turn.budgets.get_mut(key) {
                                    m.remove(id);
                                }
                            }
                            DeltaOp::Add => {
                                if let Some(v) = d.value.as_i64() {
                                    let slot = state
                                        .turn
                                        .budgets
                                        .entry(key.to_string())
                                        .or_default()
                                        .entry(id.to_string())
                                        .or_insert(0);
                                    *slot = slot.saturating_add(v);
                                }
                            }
                            DeltaOp::Set => {
                                if let Some(v) = d.value.as_i64() {
                                    state
                                        .turn
                                        .budgets
                                        .entry(key.to_string())
                                        .or_default()
                                        .insert(id.to_string(), v);
                                }
                            }
                        }
                    }
                } else if let Some(key) = other.strip_prefix("turn/skip.") {
                    match d.op {
                        DeltaOp::Remove => {
                            state.turn.skip.remove(key);
                        }
                        _ => {
                            if let Some(v) = d.value.as_u64() {
                                state.turn.skip.insert(key.to_string(), v as u32);
                            }
                        }
                    }
                }
            }
        },
        // v1 游玩页没有地点 / 关系 / 资源面板，保持 no-op。
        DeltaDomain::Location | DeltaDomain::Relationship | DeltaDomain::Resource => {}
    }
}

fn apply_map_value(map: &mut serde_json::Map<String, Value>, key: &str, d: &StateDelta) {
    match d.op {
        DeltaOp::Remove => {
            map.remove(key);
        }
        DeltaOp::Add => {
            let cur = map.get(key).cloned().unwrap_or_else(|| Value::from(0));
            map.insert(key.to_string(), add_numbers(&cur, &d.value));
        }
        DeltaOp::Set => {
            map.insert(key.to_string(), d.value.clone());
        }
    }
}

/// 数值相加：**两个操作数都是整数值时结果保持 JSON 整数**，只有真的带小数才落浮点。
///
/// 数据形状必须一致：资源 / 属性是整数口径的字段，一旦被加成 21.0，下游
/// Value::as_i64() 就会返回 None，按整数读的地方（伤害扣血、阈值比较、协议归一化）
/// 会静默拿不到值。这里只按数值本身判定，不引入任何规则集语义。
fn add_numbers(cur: &Value, add: &Value) -> Value {
    if let (Some(a), Some(b)) = (integral_number(cur), integral_number(add)) {
        if let Some(sum) = a.checked_add(b) {
            return Value::from(sum);
        }
    }
    let a = cur.as_f64().unwrap_or(0.0);
    let b = add.as_f64().unwrap_or(0.0);
    Value::from(a + b)
}

/// 把 JSON 数值读成整数：整数原样；浮点仅在**小数部分为 0** 且落在 i64 范围内时按整数处理；
/// 其余（真的带小数 / 非数值 / 越界）返回 None，交给浮点路径。
fn integral_number(v: &Value) -> Option<i64> {
    if v.is_boolean() || v.is_string() {
        return None;
    }
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(u) = v.as_u64() {
        return i64::try_from(u).ok();
    }
    let f = v.as_f64()?;
    if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        Some(f as i64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AiOutput, AiProvider, MemoryHit, MemoryRetriever, ModelRef};
    use octopus_types::{CharacterInstance, IntentEnvelope, ProjectionMeta, SuccessLevel};
    use serde_json::json;
    use std::sync::Mutex as StdMutex;
    use std::sync::RwLock;

    struct CaptureSink(StdMutex<Vec<EventEnvelope>>);

    impl EventSink for CaptureSink {
        fn emit(&self, event: EventEnvelope) {
            self.0.lock().unwrap().push(event);
        }
    }

    struct DummyAi;

    #[async_trait::async_trait]
    impl AiProvider for DummyAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::default())
        }
        
    }

    fn state_with_pc() -> WorldState {
        state_with_inventory(json!({}))
    }

    fn state_with_inventory(inventory: Value) -> WorldState {
        let mut chars = std::collections::BTreeMap::new();
        chars.insert(
            "char-a".to_string(),
            CharacterInstance {
                instance_id: "inst-char-a".into(),
                template_id: "char-a".into(),
                name: "米拉".into(),
                kind: "pc".into(),
                attributes: json!({ "str": 70 }).as_object().unwrap().clone(),
                resources: json!({ "hp": 30, "mana": 20 }).as_object().unwrap().clone(),
                inventory: inventory.as_object().cloned().unwrap_or_default(),
                location_id: None,
                present: true,
                statuses: vec![],
            },
        );
        WorldState {
            encounters: Default::default(),
            cooldowns: Default::default(),
            turn: Default::default(),
            seq: 0,
            scene_id: "sc-1".into(),
            scene_title: "场景".into(),
            scene_description: None,
            controlled: vec!["char-a".into()],
            characters: chars,
            flags: Default::default(),
            progress: Default::default(),
            locations: vec![],
            meta: ProjectionMeta {
                save_id: "s".into(),
                save_title: "t".into(),
                storybook_title: "sb".into(),
                revision: 1,
                needs_upgrade: false,
                auto_confirm: true,
            },
            rng_seed: 42,
            rng_position: 0,
        }
    }

    /// 把测试 provider 包进可热替换槽。
    fn ai_slot(ai: Arc<dyn AiProvider>) -> AiSlot {
        Arc::new(RwLock::new(ai))
    }

    fn session_with(sb: Value) -> (Arc<Session>, Arc<CaptureSink>) {
        session_with_state(sb, state_with_pc())
    }

    fn session_with_state(sb: Value, state: WorldState) -> (Arc<Session>, Arc<CaptureSink>) {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Session::new(
            "s".into(),
            state,
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(DummyAi)),
            true,
            sb,
        );
        (Arc::new(session), sink)
    }

    #[test]
    fn use_skill_pipeline_applies_effect_and_logs_rng() {
        let sb = json!({
            "skills": [{
                "id": "sk-fire", "name": "火球",
                "cost": [{ "resource": "mana", "amount": 5 }],
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "2d6", "resource": "hp" }] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, sink) = session_with(sb);
        let skill = session.rules.skill("sk-fire").cloned().unwrap();
        session.resolve_skill(None, &skill, None, None);

        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(&e.event, PlayEvent::CheckResult(_))));
        let ok = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.status == ResolutionStatus::Ok => Some(p),
                _ => None,
            })
            .expect("ok resolution");
        assert!(ok
            .state_changes
            .iter()
            .any(|d| d.field == "resources.mana" && d.value == json!(-5)));
        assert!(ok.state_changes.iter().any(|d| d.field == "resources.hp"));
        let rng = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => Some(p),
                _ => None,
            })
            .expect("rng_consume logged");
        let values: Vec<u64> = serde_json::from_str(&rng.text).unwrap();
        assert_eq!(values.len(), 3);

        let proj = session.projection();
        assert_eq!(proj.characters["char-a"]["resources"]["mana"].as_f64(), Some(15.0));
        assert!(proj.characters["char-a"]["resources"]["hp"].as_f64().unwrap() < 30.0);
    }

    // ---------- 缺陷 1（回归）：资源 Add 后的数据形状 ----------

    fn hp_delta(op: DeltaOp, value: Value) -> StateDelta {
        StateDelta {
            domain: DeltaDomain::Character,
            entity_id: "char-a".into(),
            field: "resources.hp".into(),
            op,
            value,
        }
    }

    /// 缺陷 1：整数 + 整数必须仍是 **JSON 整数**。修复前走 as_f64 相加，
    /// 结果落成 21.0，Value::as_i64() 返回 None，按整数读的下游静默拿不到值。
    #[test]
    fn resource_add_keeps_json_integer_for_integer_operands() {
        let mut map = json!({ "hp": 30 }).as_object().unwrap().clone();
        apply_map_value(&mut map, "hp", &hp_delta(DeltaOp::Add, json!(-9)));
        assert!(map["hp"].is_i64(), "整数 + 整数必须是 JSON 整数，实际 {:?}", map["hp"]);
        assert_eq!(map["hp"].as_i64(), Some(21));
        // 继续扣血（负数）仍保持整数，含跨零。
        apply_map_value(&mut map, "hp", &hp_delta(DeltaOp::Add, json!(-25)));
        assert!(map["hp"].is_i64(), "跨零后仍是整数，实际 {:?}", map["hp"]);
        assert_eq!(map["hp"].as_i64(), Some(-4));
    }

    /// 缺陷 1：只有真的带小数才落浮点；整数值的浮点（历史遗留 21.0）按整数回正。
    #[test]
    fn resource_add_floats_only_on_real_fractions() {
        let mut map = json!({ "hp": 10 }).as_object().unwrap().clone();
        apply_map_value(&mut map, "hp", &hp_delta(DeltaOp::Add, json!(2.5)));
        assert!(map["hp"].is_f64(), "真的带小数才落浮点");
        assert_eq!(map["hp"].as_f64(), Some(12.5));
        apply_map_value(&mut map, "hp", &hp_delta(DeltaOp::Add, json!(1)));
        assert!(map["hp"].is_f64(), "一旦是小数，再加整数仍是小数");
        assert_eq!(map["hp"].as_f64(), Some(13.5));
        // 修复前写下的 21.0：整数值浮点按整数处理，形状自动回正。
        let mut legacy = json!({ "hp": 21.0 }).as_object().unwrap().clone();
        apply_map_value(&mut legacy, "hp", &hp_delta(DeltaOp::Add, json!(-1)));
        assert!(legacy["hp"].is_i64(), "整数值浮点按整数回正，实际 {:?}", legacy["hp"]);
        assert_eq!(legacy["hp"].as_i64(), Some(20));
    }

    /// 缺陷 1：Set 分支不动——原样落值（含 7.0，不擅自改形状）。
    #[test]
    fn resource_set_keeps_value_verbatim() {
        let mut map = json!({ "hp": 30 }).as_object().unwrap().clone();
        apply_map_value(&mut map, "hp", &hp_delta(DeltaOp::Set, json!(7.0)));
        assert!(map["hp"].is_f64(), "Set 原样落值");
        assert_eq!(map["hp"].as_f64(), Some(7.0));
    }

    /// 缺陷 1 端到端：伤害结算经 DeltaOp::Add 落到投影后，资源仍是 JSON 整数。
    #[test]
    fn damage_keeps_projected_resource_as_json_integer() {
        let sb = json!({
            "skills": [{
                "id": "sk-hit", "name": "打击",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "5", "resource": "hp" }] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-hit").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        let hp = &session.projection().characters["char-a"]["resources"]["hp"];
        println!("缺陷1 证据：resources.hp = {hp}（is_i64 = {}）", hp.is_i64());
        assert!(hp.is_i64(), "受伤后投影里的生命资源必须仍是 JSON 整数，实际 {hp}");
        assert_eq!(hp.as_i64(), Some(25), "30 - 5（常量伤害）");
    }

    // ---------- 第一批修复（#01 冷却 / #12 状态叠加 · 边界恢复 · 资源边界） ----------

    /// 冷却（#01）：声明 cooldown.turns 的技能在冷却期内由**引擎**驳回。
    /// 这才是「提示词告诉 AI 有冷却」与「引擎真的检查」的一致状态。
    #[test]
    fn skill_cooldown_blocks_second_use_then_expires_by_round() {
        let sb = json!({
            "skills": [{
                "id": "sk-blast", "name": "爆裂",
                "cooldown": { "turns": 2 },
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "1", "resource": "hp" }] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, sink) = session_with(sb);
        let skill = session.rules.skill("sk-blast").cloned().unwrap();

        // 第一次：结算成功并落下冷却起点（当前回合 = 0）。
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert_eq!(
            session.state.lock().unwrap().cooldowns["char-a"]["sk-blast"], 0,
            "用过的技能必须记下使用回合"
        );

        // 同一回合再用：引擎驳回 cooldown_active（不再靠 AI 自觉）。
        session.resolve_skill(None, &skill, Some("char-a"), None);
        let rejected = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.status == ResolutionStatus::Rejected => {
                    p.rejection_code.clone()
                }
                _ => None,
            });
        assert_eq!(rejected.as_deref(), Some("cooldown_active"), "冷却期内必须驳回");

        // 推进到第 2 回合：2 回合冷却已满，可以再用，并刷新起点。
        session.round.store(2, Ordering::SeqCst);
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert_eq!(
            session.state.lock().unwrap().cooldowns["char-a"]["sk-blast"], 2,
            "冷却结束后可再用，并刷新使用回合"
        );
    }

    /// 冷却起点随命令日志重放：重启后不会「冷却凭空消失」。
    #[test]
    fn cooldown_survives_replay() {
        let sb = json!({
            "skills": [{
                "id": "sk-blast", "name": "爆裂",
                "cooldown": { "turns": 3 },
                "check": { "dice": "1d20" }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, _sink) = session_with(sb.clone());
        let skill = session.rules.skill("sk-blast").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);

        let persisted = persisted_of(&session);
        let (restarted, _sink2) = session_with(sb);
        restarted.replay(&persisted);
        assert_eq!(
            restarted.state.lock().unwrap().cooldowns["char-a"]["sk-blast"], 0,
            "冷却起点必须随命令日志重放"
        );
        // 重放后的会话照样拦得住。
        let skill2 = restarted.rules.skill("sk-blast").cloned().unwrap();
        assert!(restarted.cooldown_block("char-a", &skill2).is_some());
    }

    /// 未声明冷却的技能不写冷却记账（避免给每个技能刷无用状态）。
    #[test]
    fn skill_without_cooldown_records_nothing() {
        let sb = json!({
            "skills": [{ "id": "sk-plain", "name": "普通", "check": { "dice": "1d20" } }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-plain").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert!(session.state.lock().unwrap().cooldowns.is_empty(), "无冷却技能不该记冷却");
        // 可以连续使用。
        assert!(session.cooldown_block("char-a", &skill).is_none());
    }

    /// 状态叠加（#12 ③）：stack = add 把两次施加的时长相加，且不产生两条同名状态。
    #[test]
    fn status_stack_add_sums_duration_end_to_end() {
        let sb = json!({
            "statuses": [{ "id": "burn", "name": "灼烧", "duration": 2, "unit": "turns", "stack": "add" }],
            "skills": [{
                "id": "sk-ignite", "name": "点燃",
                "check": { "dice": "1d20" },
                "effect": { "status": ["burn"] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-ignite").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        session.resolve_skill(None, &skill, Some("char-a"), None);
        let st = session.state.lock().unwrap();
        let burns: Vec<_> = st.characters["char-a"]
            .statuses
            .iter()
            .filter(|s| s.id == "burn")
            .collect();
        assert_eq!(burns.len(), 1, "同名状态不该叠成两条");
        assert_eq!(burns[0].turns_left, Some(4), "stack=add 时长相加（2 + 2）");
    }

    /// 状态叠加：未声明 stack 仍是 replace（旧故事书行为逐字不变）。
    #[test]
    fn status_without_stack_still_replaces() {
        let sb = json!({
            "statuses": [{ "id": "burn", "name": "灼烧", "duration": 2, "unit": "turns" }],
            "skills": [{
                "id": "sk-ignite", "name": "点燃",
                "check": { "dice": "1d20" },
                "effect": { "status": ["burn"] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-ignite").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        session.resolve_skill(None, &skill, Some("char-a"), None);
        let st = session.state.lock().unwrap();
        let burns: Vec<_> = st.characters["char-a"].statuses.iter().filter(|s| s.id == "burn").collect();
        assert_eq!(burns.len(), 1);
        assert_eq!(burns[0].turns_left, Some(2), "缺省 replace：用新的 2 回合覆盖");
    }

    /// 边界恢复（#12 ④）：per_turn 在回合边界真的补量，且不超 default_max。
    #[test]
    fn per_turn_recovery_applies_at_turn_boundary() {
        let sb = json!({
            "world": {
                "check": { "dice": "1d20" },
                "resources": [{
                    "id": "hp", "default_max": 30,
                    "natural_recovery": { "amount": 4, "trigger": "per_turn" }
                }]
            }
        });
        let mut st = state_with_pc();
        st.characters.get_mut("char-a").unwrap().resources.insert("hp".into(), json!(26));
        let (session, _sink) = session_with_state(sb, st);
        session.tick_statuses_turn();
        assert_eq!(
            session.projection().characters["char-a"]["resources"]["hp"],
            json!(30),
            "26 + 4 夹在 default_max 30"
        );
    }

    /// 未声明 per_turn / per_scene 的故事书，边界 tick 不产生任何事件（旧行为零影响）。
    #[test]
    fn boundary_tick_emits_nothing_when_no_tick_recovery_declared() {
        let sb = json!({
            "world": {
                "check": { "dice": "1d20" },
                "resources": [{ "id": "hp", "default_max": 30 }]
            }
        });
        let (session, sink) = session_with(sb);
        let before = sink.0.lock().unwrap().len();
        session.tick_statuses_turn();
        session.tick_statuses_scene();
        assert_eq!(
            sink.0.lock().unwrap().len(),
            before,
            "没有 per_turn / per_scene 声明时不该多发事件"
        );
    }

    /// 资源边界（#12 ③）：治疗不越过 default_max，且**进日志的就是夹取后的 Set**（可重放）。
    #[test]
    fn heal_is_capped_at_default_max_and_log_carries_clamped_set() {
        let sb = json!({
            "skills": [{
                "id": "sk-heal", "name": "治疗",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "heal", "amount": "10", "resource": "hp" }] }
            }],
            "world": {
                "check": { "dice": "1d20" },
                "resources": [{ "id": "hp", "default_max": 30 }]
            }
        });
        let mut st = state_with_pc();
        st.characters.get_mut("char-a").unwrap().resources.insert("hp".into(), json!(25));
        let (session, sink) = session_with_state(sb.clone(), st);
        let skill = session.rules.skill("sk-heal").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert_eq!(
            session.projection().characters["char-a"]["resources"]["hp"],
            json!(30),
            "25 + 10 必须夹到上限 30（而不是 35）"
        );

        // 日志里是夹取后的 Set，而不是 +10 的 Add：否则重放会加回 35。
        let delta = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) => {
                    p.state_changes.iter().find(|d| d.field == "resources.hp").cloned()
                }
                _ => None,
            })
            .expect("资源 delta");
        assert_eq!(delta.op, DeltaOp::Set, "越界时必须改写成 Set");
        assert_eq!(delta.value, json!(30));

        // 重放后世界状态逐字一致（夹取后的 Set 进日志，重放不会重新加一遍）。
        let persisted = persisted_of(&session);
        let (restarted, _s2) = session_with_state(sb, state_with_pc());
        restarted.replay(&persisted);
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "夹取后重放必须一致"
        );
    }

    /// 资源边界：已经高于 default_max 的值**不被拉回来**（图鉴满血 30 而 default_max 是 8）。
    #[test]
    fn value_already_over_max_is_not_dragged_down() {
        let sb = json!({
            "skills": [{
                "id": "sk-hit", "name": "打击",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "5", "resource": "hp" }] }
            }],
            "world": {
                "check": { "dice": "1d20" },
                "resources": [{ "id": "hp", "default_max": 8 }]
            }
        });
        let mut st = state_with_pc();
        st.characters.get_mut("char-a").unwrap().resources.insert("hp".into(), json!(30));
        let (session, _sink) = session_with_state(sb, st);
        let skill = session.rules.skill("sk-hit").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert_eq!(
            session.projection().characters["char-a"]["resources"]["hp"],
            json!(30),
            "30 - 5 不得被夹到 8：夹取只拦「越过边界的那一步」，不改作者声明的初始值"
        );
    }

    /// 资源边界：没声明 default_max / min 的资源完全不受影响（旧故事书零影响）。
    #[test]
    fn resource_without_declared_bounds_is_untouched() {
        let sb = json!({
            "skills": [{
                "id": "sk-heal", "name": "治疗",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "heal", "amount": "100", "resource": "hp" }] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-heal").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert_eq!(
            session.projection().characters["char-a"]["resources"]["hp"],
            json!(130),
            "未声明边界的资源照旧不夹"
        );
    }

    /// 资源边界：显式声明 min 的资源不会被扣穿。
    #[test]
    fn declared_min_floor_stops_damage() {
        let sb = json!({
            "skills": [{
                "id": "sk-hit", "name": "打击",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "50", "resource": "hp" }] }
            }],
            "world": {
                "check": { "dice": "1d20" },
                "resources": [{ "id": "hp", "default_max": 30, "min": 0 }]
            }
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-hit").cloned().unwrap();
        session.resolve_skill(None, &skill, Some("char-a"), None);
        assert_eq!(
            session.projection().characters["char-a"]["resources"]["hp"],
            json!(0),
            "声明了 min: 0 的资源不该被扣成负数"
        );
    }

    // ---------- 缺陷 2（回归）：use_skill 自目标回落 Lua 的 host.target ----------

    /// 缺陷 2：不给 target_id（自目标）时，Lua 的 host.target 必须与结算语义一致——
    /// 指向施法者本人，而不是 nil。修复前规则包读 host.target 拿到 nil，
    /// 会「看着跑了其实没结算」。
    #[test]
    fn use_skill_self_target_exposes_actor_as_lua_target() {
        let sb = json!({
            "skills": [{
                "id": "sk-self", "name": "自目标",
                "check": { "dice": "1d20" }
            }],
            "world": { "check": { "dice": "1d20" } },
            "lua_mounts": [{
                "id": "probe", "mount": "check_post_roll",
                "source": "local t = host.target\nif t and t.id then host.apply_effect(t.id, { kind = 'set_flag', flag = 'lua-saw-' .. t.id }) end"
            }]
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-self").cloned().unwrap();
        session.resolve_skill(None, &skill, None, None);
        let flags = session.state.lock().unwrap().flags.clone();
        println!("缺陷2 证据：规则包在自目标下读到 host.target.id → flags = {flags:?}");
        assert_eq!(
            flags.get("lua-saw-char-a"),
            Some(&json!(true)),
            "自目标时 host.target.id 必须是施法者本人（实际 flags={flags:?}）"
        );
    }

    /// 缺陷 2 的真实后果：豁免成功时规则包补的「减半」必须真的落到目标。
    /// 自目标（不给 target_id）+ 判定成功时，host.target 不再是 nil，apply_effect 不再静默不发。
    #[test]
    fn self_target_save_half_effect_is_not_silently_skipped() {
        let sb = json!({
            "skills": [{
                "id": "sk-trap", "name": "陷阱",
                "check": { "dice": "1d20", "kind": "save" }
            }],
            "world": { "check": { "dice": "1d20", "default_dc": 0 } },
            "lua_mounts": [{
                "id": "half", "mount": "check_post_roll",
                "source": "if host.check_kind == 'save' and host.check_result == true then\n  local t = host.target\n  if t and t.id then host.apply_effect(t.id, { kind = 'damage', amount = '3', resource = 'hp' }) end\nend"
            }]
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-trap").cloned().unwrap();
        session.resolve_skill(None, &skill, None, None);
        let hp = session.projection().characters["char-a"]["resources"]["hp"]
            .as_i64()
            .expect("资源仍是整数");
        println!("缺陷2 证据：自目标豁免成功后 resources.hp = {hp}（修复前静默保持 30）");
        assert_eq!(hp, 27, "豁免成功（DC 0 必成功）→ 规则包补的 3 点伤害必须真的结算；修复前静默为 30");
    }

    // ---------- 缺陷 3（回归）：check_pre_roll 判定签名 + check_post_roll expr ----------

    /// 缺陷 3（GAP-D）：check_pre_roll 读得到判定**签名**（attribute + kind + target），
    /// 结果字段仍是 nil；check_post_roll 读得到骰式 expr。
    /// 「取高/取低只有掷骰前有意义」+「只对某一类检定生效」由此才表达得出来。
    #[tokio::test]
    async fn check_pre_roll_sees_signature_and_post_roll_sees_expr() {
        let sb = || {
            json!({
                "world": { "check": { "dice": "1d20", "kind": "attribute" } },
                "lua_mounts": [
                    {
                        "id": "gate",
                        "mount": "check_pre_roll",
                        "source": "local c = host.check\nif c and c.attribute == 'str' and c.kind == 'attribute' and c.target == 10 and c.total == nil and c.result == nil then host.modify_check('keep_high') end"
                    },
                    {
                        "id": "expr",
                        "mount": "check_post_roll",
                        "source": "if host.check_expr == '1d20' and host.check.expr == '1d20' then host.modify_check('add', 5) end"
                    }
                ]
            })
        };
        // 命中签名（str）→ 取高：掷两次，与独立探针同序列。
        let mut probe = DeterministicRng::new(42);
        let d1 = probe.range_inclusive(1, 20);
        let d2 = probe.range_inclusive(1, 20);
        let (session, sink) = session_with(sb());
        session
            .handle_intent(
                Intent::Check {
                    attribute: "str".into(),
                    difficulty: Some(10),
                    actor_id: None,
                    opponent_id: None,
                },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = check_result_of(&events).expect("check result");
        println!(
            "缺陷3 证据：pre_roll 签名驱动取高 → rolls={:?}；post_roll 读 expr → r#mod={}",
            check.rolls, check.r#mod
        );
        assert_eq!(check.rolls, Some(vec![d1.max(d2)]), "签名命中 → 取高生效");
        assert_eq!(check.r#mod, 4 + 5, "post_roll 读到 expr 后加值");
        assert_eq!(rng_logged(&events), probe.consumed);

        // 签名不命中（dex）→ 取高不生效：只掷一次（证明签名真的可读、可判别）。
        let (session2, sink2) = session_with(sb());
        session2
            .handle_intent(
                Intent::Check {
                    attribute: "dex".into(),
                    difficulty: Some(10),
                    actor_id: None,
                    opponent_id: None,
                },
                None,
            )
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        let check2 = check_result_of(&events2).expect("check result 2");
        assert_eq!(rng_logged(&events2).len(), 1, "签名不命中 → 不取高，只掷一次");
        assert_eq!(check2.rolls.as_ref().map(Vec::len), Some(1));
    }

    /// #06 ① 回归（修复前应失败）：进程重启后从日志重放必须恢复 RNG 位置，
    /// 否则未来的骰值会从序列头重演，违背「严格事件溯源」承诺。
    #[tokio::test]
    async fn restart_replay_restores_rng_position() {
        let sb = json!({
            "skills": [{
                "id": "sk-fire", "name": "火球",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "2d6", "resource": "hp" }] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        // 实时会话：消耗 RNG 并把消耗记录进日志。
        let (live, live_sink) = session_with(sb.clone());
        let skill = live.rules.skill("sk-fire").cloned().unwrap();
        live.resolve_skill(None, &skill, None, None);
        let live_next = live.rng.lock().unwrap().range_inclusive(1, 20);
        let events: Vec<EventEnvelope> = live_sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(
            &e.event,
            PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume")
        )));

        // 重启：同一初始状态，从命令日志重建会话。
        let (restarted, _sink) = session_with(sb);
        let persisted: Vec<PersistedEvent> = events
            .into_iter()
            .map(|envelope| PersistedEvent { request_id: None, envelope })
            .collect();
        restarted.replay(&persisted);
        let restarted_next = restarted.rng.lock().unwrap().range_inclusive(1, 20);
        assert_eq!(
            live_next, restarted_next,
            "重启重放后 RNG 必须停在日志记录的位置，未来骰序与不重启一致"
        );

        // 重启后再结算一次同一技能：骰值、资源与不重启的会话逐位一致。
        live.resolve_skill(None, &skill, None, None);
        restarted.resolve_skill(None, &skill, None, None);
        assert_eq!(
            serde_json::to_value(live.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "重启后的继续结算必须产生同一世界状态"
        );
    }

    /// #06 ② 核心不变量：快照 + 其后命令 必须与全量重放得到同一状态、同一 seq、同一 RNG 位置。
    #[test]
    fn snapshot_plus_later_events_equals_full_replay() {
        let sb = json!({
            "skills": [{
                "id": "sk-fire", "name": "火球",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "2d6", "resource": "hp" }] }
            }],
            "world": { "check": { "dice": "1d20" } }
        });
        let (live, live_sink) = session_with(sb.clone());
        let skill = live.rules.skill("sk-fire").cloned().unwrap();
        // 前两轮之后取一份快照（覆盖「快照基线」分支）。
        live.resolve_skill(None, &skill, None, None);
        live.resolve_skill(None, &skill, None, None);
        let snapshot = live.snapshot_base();
        assert!(snapshot.seq > 0, "快照必须带一个有效 seq");
        // 快照之后再跑两轮，制造「其后命令」。
        live.resolve_skill(None, &skill, None, None);
        live.resolve_skill(None, &skill, None, None);
        let events: Vec<PersistedEvent> = live_sink
            .0
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(|envelope| PersistedEvent { request_id: None, envelope })
            .collect();

        // 全量重放（不带快照）
        let (full, _s1) = session_with(sb.clone());
        full.replay(&events);
        // 快照 + 只投影其后的命令
        let (cached, _s2) = session_with(sb);
        cached.replay_from(&events, Some(snapshot));

        assert_eq!(
            serde_json::to_value(full.projection()).unwrap(),
            serde_json::to_value(cached.projection()).unwrap(),
            "快照 + 其后命令必须与全量重放得到同一世界状态"
        );
        assert_eq!(full.current_seq(), cached.current_seq(), "seq 必须一致");
        assert_eq!(
            full.rng.lock().unwrap().range_inclusive(1, 1000),
            cached.rng.lock().unwrap().range_inclusive(1, 1000),
            "RNG 位置必须一致"
        );
    }

    #[test]
    fn use_skill_rejection_emits_insufficient_resource() {
        let sb = json!({
            "skills": [{
                "id": "sk-expensive", "name": "禁术",
                "cost": [{ "resource": "mana", "amount": 999 }],
                "effect": {}
            }]
        });
        let (session, sink) = session_with(sb);
        let skill = session.rules.skill("sk-expensive").cloned().unwrap();
        session.resolve_skill(None, &skill, None, None);

        let events = sink.0.lock().unwrap().clone();
        let rejected = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.status == ResolutionStatus::Rejected => Some(p),
                _ => None,
            })
            .expect("rejected resolution");
        assert_eq!(rejected.rejection_code.as_deref(), Some("insufficient_resource"));
        assert!(rejected.state_changes.is_empty());
        assert!(!events.iter().any(|e| match &e.event {
            PlayEvent::System(p) => p.code.as_deref() == Some("rng_consume"),
            _ => false,
        }));
    }

    #[test]
    fn use_item_requires_ownership() {
        let sb = json!({
            "items": [{ "id": "it-potion", "name": "药水", "skills": ["sk-drink"] }],
            "skills": [{ "id": "sk-drink", "name": "喝药", "effect": {} }]
        });
        // 未持有 → item_not_owned
        let (session, sink) = session_with(sb.clone());
        session.handle_use_item("it-potion", None, None);
        let events = sink.0.lock().unwrap().clone();
        let rejected = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.status == ResolutionStatus::Rejected => Some(p),
                _ => None,
            })
            .expect("rejected");
        assert_eq!(rejected.rejection_code.as_deref(), Some("item_not_owned"));

        // 持有 → 成功
        let (session, sink) = session_with_state(sb, state_with_inventory(json!({ "it-potion": 1 })));
        session.handle_use_item("it-potion", None, None);
        let events = sink.0.lock().unwrap().clone();
        assert!(events
            .iter()
            .any(|e| matches!(&e.event, PlayEvent::Resolution(p) if p.status == ResolutionStatus::Ok)));
    }

    #[test]
    fn event_triggers_and_lua_event_mount_fire() {
        let sb = json!({
            "skills": [{
                "id": "sk-react", "name": "反应",
                "effect": { "triggers": [{
                    "id": "tr1", "event": "scene_change",
                    "effects": [{ "kind": "set_flag", "flag": "scene_changed" }]
                }] }
            }]
        });
        let (session, sink) = session_with(sb);
        session.dispatch_event("scene_change");
        assert_eq!(session.projection().flags.get("scene_changed"), Some(&json!(true)));

        // Lua Event 挂载点：良好脚本不报错
        session.register_lua_hook("ok", LuaMount::Event, "host.trigger_event('x')");
        session.dispatch_event("scene_change");
        let events = sink.0.lock().unwrap().clone();
        assert!(!events.iter().any(|e| matches!(&e.event,
            PlayEvent::System(p) if p.code.as_deref() == Some("event_lua_error"))));

        // 坏脚本 fail-fast 并报错误事件
        session.register_lua_hook("bad", LuaMount::Event, "error('boom')");
        session.dispatch_event("scene_change");
        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(&e.event,
            PlayEvent::System(p) if p.code.as_deref() == Some("event_lua_error"))));
    }

    /// 场景切换事件命名统一（决策 #11）：派发规范名 scene 时，旧名 scene_change
    /// 声明的触发器与规范名 scene 声明的触发器都应命中——旧故事书不静默失效。
    #[test]
    fn scene_event_canonical_and_legacy_are_aliases() {
        let sb = json!({
            "skills": [{
                "id": "sk-react", "name": "反应",
                "effect": { "triggers": [
                    { "id": "tr-canonical", "event": "scene",
                      "effects": [{ "kind": "set_flag", "flag": "canonical_fired" }] },
                    { "id": "tr-legacy", "event": "scene_change",
                      "effects": [{ "kind": "set_flag", "flag": "legacy_fired" }] }
                ] }
            }]
        });
        let (session, _sink) = session_with(sb);
        session.dispatch_event("scene");
        let proj = session.projection();
        assert_eq!(proj.flags.get("canonical_fired"), Some(&json!(true)));
        assert_eq!(
            proj.flags.get("legacy_fired"),
            Some(&json!(true)),
            "派发规范名 scene 时旧名 scene_change 触发器也应命中"
        );
    }

    #[test]
    fn status_effect_applies_on_each_tick() {
        let sb = json!({
            "statuses": [{
                "id": "burn", "name": "灼烧", "duration": 2, "unit": "turns",
                "effect": [{ "kind": "damage", "amount": "3", "resource": "hp" }]
            }],
            "skills": [{
                "id": "sk-curse", "name": "诅咒",
                "effect": { "status": ["burn"] }
            }]
        });
        let (session, _sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "char-a".into(),
                field: "status".into(),
                op: DeltaOp::Set,
                value: json!({ "id": "burn", "name": "灼烧", "turns_left": 2 }),
            }],
        }));

        session.tick_statuses_turn();
        let hp = session.projection().characters["char-a"]["resources"]["hp"].as_f64().unwrap();
        assert_eq!(hp, 27.0, "第一 tick 扣 3");
        let left = session.projection().characters["char-a"]["statuses"][0]["turns_left"].clone();
        assert_eq!(left, json!(1));

        session.tick_statuses_turn();
        let hp = session.projection().characters["char-a"]["resources"]["hp"].as_f64().unwrap();
        assert_eq!(hp, 24.0, "末次 tick 仍结算后到期移除");
        assert!(session.projection().characters["char-a"]["statuses"].as_array().unwrap().is_empty());
    }

    #[test]
    fn statuses_stack_replace_and_tick_down() {
        let (session, _sink) = session_with(json!({}));
        let apply = |left: i32| StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "char-a".into(),
                field: "status".into(),
                op: DeltaOp::Set,
                value: json!({ "id": "burn", "name": "灼烧", "turns_left": left }),
            }],
        };
        session.emit_simple(PlayEvent::StateUpdate(apply(3)));
        let statuses = session.projection().characters["char-a"]["statuses"].as_array().unwrap().clone();
        assert_eq!(statuses.len(), 1);

        // 同名替换而非追加
        session.emit_simple(PlayEvent::StateUpdate(apply(2)));
        let statuses = session.projection().characters["char-a"]["statuses"].as_array().unwrap().clone();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0]["turns_left"], json!(2));

        session.tick_statuses_turn();
        let statuses = session.projection().characters["char-a"]["statuses"].as_array().unwrap().clone();
        assert_eq!(statuses[0]["turns_left"], json!(1));

        session.tick_statuses_turn();
        assert!(session.projection().characters["char-a"]["statuses"].as_array().unwrap().is_empty());
    }

    /// 一次状态 tick 期间写进 resources.mark_* 的标记，按事件顺序取出（时机顺序的可观测证据）。
    fn mark_order(sink: &CaptureSink) -> Vec<String> {
        let events = sink.0.lock().unwrap().clone();
        let mut out = Vec::new();
        for e in &events {
            if let PlayEvent::StateUpdate(p) = &e.event {
                for d in &p.changes {
                    if let Some(name) = d.field.strip_prefix("resources.mark_") {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out
    }

    /// 状态结算的时钟粒度（L4 时机原语）：turn_end / scene_end 每个边界各派发一次，
    /// status_tick 每个 (角色, 状态) 各派发一次，且携带的正是被结算的那个状态实例。
    #[test]
    fn status_tick_mounts_fire_per_boundary_and_per_status() {
        let sb = json!({
            "statuses": [
                { "id": "hold", "name": "定身", "duration": 3, "unit": "turns" },
                { "id": "veil", "name": "帷幕", "duration": 3, "unit": "scenes" }
            ],
            "lua_mounts": [
                { "id": "b-turn", "mount": "turn_end",
                  "source": "host.modify_resource(host.actor.id, 'mark_' .. host.mount, 1)" },
                { "id": "b-scene", "mount": "scene_end",
                  "source": "host.modify_resource(host.actor.id, 'mark_' .. host.mount, 1)" },
                { "id": "s-tick", "mount": "status_tick",
                  "source": "host.modify_resource(host.actor.id, 'mark_' .. host.mount .. '_' .. host.status_id .. '_' .. host.status_unit .. '_' .. host.status_remaining, 1)" }
            ]
        });
        let (session, sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![
                StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: "char-a".into(),
                    field: "status".into(),
                    op: DeltaOp::Set,
                    value: json!({ "id": "hold", "name": "定身", "turns_left": 3 }),
                },
                StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: "char-a".into(),
                    field: "status".into(),
                    op: DeltaOp::Set,
                    value: json!({ "id": "veil", "name": "帷幕", "scenes_left": 3 }),
                },
            ],
        }));

        session.tick_statuses_turn();
        session.tick_statuses_scene();

        assert_eq!(
            mark_order(&sink),
            vec![
                // 逐状态时机在结算前；回合边界时机在结算后（新加的状态不会被同一次 tick 递减）。
                "status_tick_hold_turns_2".to_string(),
                "turn_end".to_string(),
                // 回合边界只结算 turns 单位的状态；上下文带 id / 单位 / 结算后剩余。
                "status_tick_veil_scenes_2".to_string(),
                "scene_end".to_string(),
            ],
            "逐状态时机与边界时机的派发顺序、上下文"
        );
        // 时机没有打乱引擎自己的结算：两个状态各递减一次。
        let chars = session.projection().characters.clone();
        let statuses = chars["char-a"]["statuses"].as_array().unwrap();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0]["id"], json!("hold"));
        assert_eq!(statuses[0]["turns_left"], json!(2));
        assert_eq!(statuses[1]["id"], json!("veil"));
        assert_eq!(statuses[1]["scenes_left"], json!(2));
    }

    /// 边界时机在状态结算**之后**：规则包在 turn_end 新加的状态不会被同一次 tick 立刻递减。
    #[test]
    fn turn_end_runs_after_status_settlement() {
        let sb = json!({
            "statuses": [
                { "id": "burn", "name": "灼烧", "duration": 2, "unit": "turns" },
                { "id": "ward", "name": "守护", "duration": 1, "unit": "turns" }
            ],
            "lua_mounts": [{ "id": "ward-on", "mount": "turn_end",
                "source": "host.apply_status(host.actor.id, 'ward', 1, 'turns')" }]
        });
        let (session, _sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "char-a".into(),
                field: "status".into(),
                op: DeltaOp::Set,
                value: json!({ "id": "burn", "name": "灼烧", "turns_left": 2 }),
            }],
        }));
        session.tick_statuses_turn();
        let statuses = session.projection().characters["char-a"]["statuses"].clone();
        let by_id = |id: &str| {
            statuses
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == json!(id))
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by_id("burn")["turns_left"], json!(1), "已有状态照常递减");
        assert_eq!(by_id("ward")["turns_left"], json!(1), "边界新加的状态保留完整时长");
    }

    /// L4 范式：规则包在 status_tick 里掷骰决定是否结束状态（引擎不认识「豁免」）。
    ///
    /// 骰子取自引擎的确定性序列，因此与同种子的探针逐位相同；
    /// 掷骰成功 → 状态被移除且不会被引擎的递减写回；失败 → 状态照常递减。
    #[test]
    fn status_tick_script_can_end_status_and_consumes_engine_rng() {
        // 探针：与存档同一段 RNG 序列（seed 42 / position 0），先取出脚本将掷的那一颗骰。
        let mut probe = DeterministicRng::new(42);
        let roll = probe.range_inclusive(1, 20);
        assert_eq!(probe.consumed.len(), 1);

        let storybook = |source: String| {
            json!({
                "statuses": [{ "id": "grasp", "name": "缠绕", "duration": 3, "unit": "turns" }],
                "lua_mounts": [{ "id": "grasp-rule", "mount": "status_tick", "source": source }]
            })
        };
        let source = |threshold: i64| {
            format!(
                "if host.status_id == 'grasp' then \
                   local r = host.engine_rng(1, 20) \
                   if r >= {threshold} then host.remove_status(host.actor.id, host.status_id) end \
                 end"
            )
        };
        let apply_grasp = |session: &Arc<Session>| {
            session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
                changes: vec![StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: "char-a".into(),
                    field: "status".into(),
                    op: DeltaOp::Set,
                    value: json!({ "id": "grasp", "name": "缠绕", "turns_left": 3 }),
                }],
            }));
        };
        let rng_events = |sink: &CaptureSink| -> Vec<u64> {
            let events = sink.0.lock().unwrap().clone();
            events
                .iter()
                .find_map(|e| match &e.event {
                    PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => {
                        serde_json::from_str::<Vec<u64>>(&p.text).ok()
                    }
                    _ => None,
                })
                .unwrap_or_default()
        };

        // 成功分支：阈值取探针值 → 脚本掷出的同一颗骰必然达成 → 状态移除。
        let (session, sink) = session_with(storybook(source(roll)));
        apply_grasp(&session);
        session.tick_statuses_turn();
        let statuses = session.projection().characters["char-a"]["statuses"].clone();
        assert_eq!(statuses, json!([]), "脚本掷骰成功 → 状态被移除");
        assert_eq!(
            session.rng.lock().unwrap().consumed,
            probe.consumed,
            "脚本的骰子必须走引擎确定性序列（与探针同序列）"
        );
        assert_eq!(rng_events(&sink), probe.consumed, "掷骰消耗必须进命令日志");
        // 引擎不得在脚本移除之后再把状态递减写回。
        let ops: Vec<DeltaOp> = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::StateUpdate(p) => Some(p.changes.clone()),
                _ => None,
            })
            .flatten()
            .filter(|d| d.field == "status")
            .map(|d| d.op)
            .collect();
        assert_eq!(ops, vec![DeltaOp::Set, DeltaOp::Remove], "移除后不得再写回 Set");

        // 失败分支：阈值取探针值 + 1 → 同一颗骰不达成 → 状态照常递减。
        let (session2, sink2) = session_with(storybook(source(roll + 1)));
        apply_grasp(&session2);
        session2.tick_statuses_turn();
        let statuses = session2.projection().characters["char-a"]["statuses"].clone();
        assert_eq!(statuses[0]["turns_left"], json!(2), "掷骰未达成 → 引擎照常递减");
        assert_eq!(session2.rng.lock().unwrap().consumed, probe.consumed);
        assert_eq!(rng_events(&sink2), probe.consumed);
    }

    /// 向后兼容：无 lua_mounts、或有挂载点但没有 status_tick 脚本时，
    /// 状态 tick 的事件与骰序逐字相同（脚本不参与 = 旧行为）。
    #[test]
    fn status_tick_without_status_tick_scripts_is_unchanged() {
        let statuses = json!([{
            "id": "burn", "name": "灼烧", "duration": 2, "unit": "turns",
            "effect": [{ "kind": "damage", "amount": "2d6", "resource": "hp" }]
        }]);
        let run = |lua_mounts: Option<Value>| -> (Vec<String>, usize, Value) {
            let mut sb = json!({ "statuses": statuses.clone() });
            if let Some(m) = lua_mounts {
                sb["lua_mounts"] = m;
            }
            let (session, sink) = session_with(sb);
            session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
                changes: vec![StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: "char-a".into(),
                    field: "status".into(),
                    op: DeltaOp::Set,
                    value: json!({ "id": "burn", "name": "灼烧", "turns_left": 2 }),
                }],
            }));
            // 只看 tick 自己产生的事件，排除测试自己落的施加 delta。
            sink.0.lock().unwrap().clear();
            session.tick_statuses_turn();
            let events = sink.0.lock().unwrap().clone();
            let summary: Vec<String> = events
                .iter()
                .map(|e| match &e.event {
                    PlayEvent::System(p) => format!("system:{}:{}", p.code.clone().unwrap_or_default(), p.text),
                    PlayEvent::StateUpdate(p) => format!("state:{:?}", p.changes),
                    other => format!("other:{other:?}"),
                })
                .collect();
            let draws = session.rng.lock().unwrap().consumed.len();
            (summary, draws, session.projection().characters["char-a"]["resources"].clone())
        };

        let (plain, plain_draws, _) = run(None);
        // 有挂载点、但没有 status_tick 脚本：同样零影响。
        let (with_other_mount, other_draws, _) = run(Some(json!([
            { "id": "unrelated", "mount": "check_pre_roll", "source": "host.modify_check('add', 6)" },
            { "id": "turn-marker", "mount": "event", "source": "host.trigger_event('noop')" }
        ])));
        assert_eq!(plain, with_other_mount, "无 status_tick 脚本 → 事件逐字不变");
        assert_eq!(plain_draws, 2, "2d6 消耗两颗骰，Lua 不得多掷");
        assert_eq!(plain_draws, other_draws);
        assert!(
            !plain
                .iter()
                .any(|s| s.starts_with("system:") && !s.starts_with("system:rng_consume")),
            "旧路径不得新增 System 事件（rng_consume 除外）：{plain:?}"
        );
        assert!(plain.iter().any(|s| s.starts_with("state:")), "仍要有状态结算：{plain:?}");
    }

    /// 沙箱照旧：status_tick 脚本超指令预算即被拦，且不阻断引擎自己的结算。
    #[test]
    fn status_tick_script_is_still_sandboxed() {
        let sb = json!({
            "statuses": [{ "id": "hold", "name": "定身", "duration": 2, "unit": "turns" }],
            "lua_mounts": [{ "id": "runaway", "mount": "status_tick", "source": "while true do end" }]
        });
        let (session, sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "char-a".into(),
                field: "status".into(),
                op: DeltaOp::Set,
                value: json!({ "id": "hold", "name": "定身", "turns_left": 2 }),
            }],
        }));
        session.tick_statuses_turn();
        let events = sink.0.lock().unwrap().clone();
        assert!(
            events.iter().any(|e| matches!(&e.event,
                PlayEvent::System(p) if p.code.as_deref() == Some("status_tick_lua_error")
                    && p.text.contains("instruction budget"))),
            "超预算脚本必须被沙箱拦下"
        );
        let statuses = session.projection().characters["char-a"]["statuses"].clone();
        assert_eq!(statuses[0]["turns_left"], json!(1), "脚本失败不影响引擎自己的结算");
    }

    /// 端到端：一个完整回合（run_round）的边界就把 status_tick 交给规则包，
    /// 规则包掷骰成功即结束状态——引擎全程不认识「何时结束」这个规则。
    #[tokio::test]
    async fn round_boundary_dispatches_status_tick_end_to_end() {
        let mut probe = DeterministicRng::new(42);
        let roll = probe.range_inclusive(1, 20);
        let sb = json!({
            "statuses": [{ "id": "grasp", "name": "缠绕", "duration": 5, "unit": "turns" }],
            "lua_mounts": [{
                "id": "grasp-rule", "mount": "status_tick",
                "source": format!(
                    "if host.status_id == 'grasp' then \
                       local r = host.engine_rng(1, 20) \
                       if r >= {roll} then host.remove_status(host.actor.id, host.status_id) end \
                     end"
                )
            }]
        });
        let (session, _sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "char-a".into(),
                field: "status".into(),
                op: DeltaOp::Set,
                value: json!({ "id": "grasp", "name": "缠绕", "turns_left": 5 }),
            }],
        }));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Meta, text: "/帮助".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(
            session.projection().characters["char-a"]["statuses"],
            json!([]),
            "回合边界派发 status_tick → 规则包掷骰成功 → 状态结束"
        );
        assert_eq!(session.rng.lock().unwrap().consumed, probe.consumed, "骰子走引擎序列");
    }

    #[test]
    fn lua_apply_status_request_updates_state() {
        let sb = json!({
            "statuses": [{ "id": "burn", "name": "灼烧", "duration": 9, "unit": "turns" }],
            "skills": [{
                "id": "sk-burn", "name": "引燃", "effect": {},
                "lua": "host.apply_status(host.actor.id, 'burn', 2, 'turns')"
            }]
        });
        let (session, _sink) = session_with(sb);
        let skill = session.rules.skill("sk-burn").cloned().unwrap();
        session.resolve_skill(None, &skill, None, None);
        let statuses = session.projection().characters["char-a"]["statuses"].as_array().unwrap().clone();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0]["id"], json!("burn"));
        assert_eq!(statuses[0]["name"], json!("灼烧"));
        assert_eq!(statuses[0]["turns_left"], json!(2), "脚本 duration 覆盖声明默认值");
    }

    #[test]
    fn turn_end_marks_new_goal_and_trigger_once() {
        let sb = json!({
            "skeleton": [{
                "id": "ch-1",
                "scenes": [{
                    "id": "sc-1",
                    "goals": [{ "id": "g1", "condition": { "op": "flag_set", "flag": "met_isa" } }],
                    "triggers": [{ "id": "b1", "condition": { "op": "flag_set", "flag": "met_isa" } }]
                }]
            }]
        });
        let (session, sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Flag,
                entity_id: "met_isa".into(),
                field: "flag".into(),
                op: DeltaOp::Set,
                value: json!(true),
            }],
        }));

        session.evaluate_turn_end();
        let events = sink.0.lock().unwrap().clone();
        let progress = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::System(p) if p.code.as_deref() == Some("skeleton_progress") => Some(p),
                _ => None,
            })
            .expect("skeleton_progress emitted");
        assert!(progress.text.contains("g1"));
        assert!(progress.text.contains("b1"));

        let proj = session.projection();
        assert_eq!(proj.progress.goals.get("g1"), Some(&json!(true)));
        assert_eq!(proj.progress.triggers.get("b1"), Some(&json!(true)));

        // 二次求值不重复触发
        let before = sink.0.lock().unwrap().len();
        session.evaluate_turn_end();
        assert_eq!(sink.0.lock().unwrap().len(), before);
    }

    #[test]
    fn projection_quests_include_current_scene_skeleton_goals() {
        let sb = json!({
            "skeleton": [{
                "id": "ch-1",
                "scenes": [{
                    "id": "sc-1",
                    "title": "场景",
                    "goals": [
                        { "id": "g1", "text": "打倒地精", "primary": true },
                        { "text": "发现巢穴踪迹" }
                    ]
                }]
            }]
        });
        let (session, _sink) = session_with(sb);
        let quests = session.projection().quests;
        assert_eq!(quests.len(), 2, "两条骨架目标都该进 quests");
        assert_eq!(quests[0].source, "skeleton");
        assert_eq!(quests[0].id, "g1");
        assert_eq!(quests[0].text, "打倒地精");
        assert!(quests[0].primary);
        assert_eq!(quests[1].id, "sc-1#goal[1]", "无 id 的骨架目标要有稳定合成 id");
        assert_eq!(quests[1].text, "发现巢穴踪迹");

        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Goal,
                entity_id: "g1".into(),
                field: "achieved".into(),
                op: DeltaOp::Set,
                value: json!(true),
            }],
        }));
        assert!(session.projection().quests[0].done, "达成后 done 要反映进度");
    }

    #[test]
    fn present_actors_excludes_offstage_characters() {
        let mut st = state_with_pc();
        let npc = |id: &str, present: bool| CharacterInstance {
            instance_id: format!("inst-{id}"),
            template_id: id.into(),
            name: id.into(),
            kind: "npc".into(),
            attributes: Default::default(),
            resources: Default::default(),
            inventory: Default::default(),
            location_id: None,
            present,
            statuses: vec![],
        };
        st.characters.insert("char-here".into(), npc("char-here", true));
        st.characters.insert("char-off".into(), npc("char-off", false));
        let actors = present_actors(&st);
        let ids: Vec<&str> = actors.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains(&"char-a"), "受控角色恒在场");
        assert!(ids.contains(&"char-here"));
        assert!(!ids.contains(&"char-off"), "离场角色不该进提示词");
    }

    #[test]
    fn personas_read_ai_fields_and_never_leak_author_notes() {
        let sb = json!({
            "characters": [
                {
                    "id": "char-isa", "name": "伊莎", "kind": "npc",
                    "background": "碎星酒馆老板娘。",
                    "personality": "热情圆滑。",
                    "appearance": "围裙上沾着麦酒渍。",
                    "example_dialogues": "「哟，稀客。」",
                    "notes": "给使用者的备忘：她其实是卧底。"
                },
                { "id": "char-empty", "name": "路人" }
            ]
        });
        let rules = SessionRules::from_storybook(sb);
        let views = rules.personas(&["char-isa".into(), "char-empty".into()], 6);
        assert_eq!(views.len(), 1, "全空的人物不该占用上下文");
        let isa = &views[0];
        assert_eq!(isa.name, "伊莎");
        assert_eq!(isa.personality, "热情圆滑。");
        assert_eq!(isa.example_dialogues, "「哟，稀客。」");
        // 作者注释是「给人看」的字段：结构上就不该进入 PersonaView。
        let dbg = format!("{isa:?}");
        assert!(!dbg.contains("卧底"), "作者注释绝不能进入注入用的人格档案");
        // limit 生效
        assert!(rules.personas(&["char-isa".into()], 0).is_empty());
    }
    #[test]
    fn match_lore_honors_keys_constants_recursion_and_budget() {
        let sb = json!({
            "lore": [
                { "id": "l-core", "title": "世界规则", "content": "魔法有代价。", "constant": true, "priority": 5 },
                { "id": "l-forest", "title": "暗影森林", "content": "终年迷雾，深处有精灵遗迹，也是黑魔法师聚集地。",
                  "keys": ["暗影森林", "Shadow Forest"], "priority": 10 },
                { "id": "l-ruins", "title": "精灵遗迹", "content": "遗迹由古代精灵建造。",
                  "keys": ["精灵遗迹"], "priority": 1 },
                { "id": "l-off", "title": "已停用", "content": "不该出现。", "keys": ["暗影森林"], "enabled": false },
                { "id": "l-void", "title": "无人提及", "content": "不该出现。", "keys": ["虚空"] }
            ]
        });
        let rules = SessionRules::from_storybook(sb);
        let hit = rules.match_lore("We enter the SHADOW FOREST.", 0);
        let ids: Vec<&str> = hit.iter().map(|l| l.id.as_str()).collect();
        assert!(ids.contains(&"l-core"), "常驻条目每回合注入");
        assert!(ids.contains(&"l-forest"), "英文别名要能命中（大小写不敏感）");
        assert!(!ids.contains(&"l-off"), "停用条目不注入");
        assert!(!ids.contains(&"l-void"), "未命中不注入");
        assert_eq!(ids[0], "l-forest", "priority 最大者排最前");

        // 递归扫描：命中条目的内容里再出现其它词条的 key → 连锁命中
        let sb2 = json!({ "lore": [
            { "id": "l-forest", "title": "暗影森林", "content": "深处有精灵遗迹。", "keys": ["暗影森林"], "recursive": true },
            { "id": "l-ruins", "title": "精灵遗迹", "content": "古代精灵建造。", "keys": ["精灵遗迹"] }
        ] });
        let rules2 = SessionRules::from_storybook(sb2);
        let chained = rules2.match_lore("走进暗影森林", 0);
        assert!(chained.iter().any(|l| l.id == "l-ruins"), "递归扫描要连锁命中");

        // 预算：装不下的条目整条跳过，但最高优先级一条必留
        let trimmed = rules.match_lore("走进暗影森林", 12);
        assert_eq!(trimmed.len(), 1, "预算只够一条");
        assert_eq!(trimmed[0].id, "l-forest");
    }

    #[test]
    fn advance_scene_updates_scene_and_roster() {
        let sb = json!({
            "skeleton": [{
                "id": "ch-1",
                "scenes": [
                    { "id": "sc-1", "title": "甲", "goals": [] },
                    { "id": "sc-2", "title": "乙", "description": "乙的描述",
                      "present_char_ids": ["char-b"] }
                ]
            }]
        });
        let mut st = state_with_pc();
        let npc = |id: &str, present: bool| CharacterInstance {
            instance_id: format!("inst-{id}"),
            template_id: id.into(),
            name: id.into(),
            kind: "npc".into(),
            attributes: Default::default(),
            resources: Default::default(),
            inventory: Default::default(),
            location_id: None,
            present,
            statuses: vec![],
        };
        st.characters.insert("char-b".into(), npc("char-b", false));
        st.characters.insert("char-c".into(), npc("char-c", true));
        let (session, sink) = session_with_state(sb, st);

        session.switch_scene("sc-2");
        let proj = session.projection();
        assert_eq!(proj.scene_id, "sc-2");
        assert_eq!(proj.scene_title, "乙");
        assert!(proj.characters["char-b"]["present"].as_bool().unwrap(), "新场景在场者入场");
        assert!(!proj.characters["char-c"]["present"].as_bool().unwrap(), "旧场景角色离场");
        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(
            &e.event,
            PlayEvent::Scene(p) if p.scene_id == "sc-2" && p.description.as_deref() == Some("乙的描述")
        )));
    }

    /// 在场三层优先级（设计 §4.1）：① 作者点名 > ② 位置匹配 > ③ 两者都未声明则全员在场；
    /// 受控角色恒在场。同时钉住旧代码的不一致——「缺省名单」曾被当成空集而清场。
    #[test]
    fn switch_scene_presence_three_layers() {
        let sb = json!({
            "skeleton": [{
                "id": "ch-1",
                "scenes": [
                    { "id": "sc-none", "title": "无声明" },
                    { "id": "sc-tavern", "title": "酒馆", "location_id": "loc-tavern" },
                    { "id": "sc-mixed", "title": "混合", "location_id": "loc-tavern",
                      "present_char_ids": ["char-elsewhere"] },
                    { "id": "sc-empty", "title": "空场", "present_char_ids": [] }
                ]
            }]
        });
        let mut st = state_with_pc();
        let npc = |id: &str, location_id: Option<&str>, present: bool| CharacterInstance {
            instance_id: format!("inst-{id}"),
            template_id: id.into(),
            name: id.into(),
            kind: "npc".into(),
            attributes: Default::default(),
            resources: Default::default(),
            inventory: Default::default(),
            location_id: location_id.map(str::to_string),
            present,
            statuses: vec![],
        };
        st.characters.insert("char-tavern".into(), npc("char-tavern", Some("loc-tavern"), false));
        st.characters.insert("char-elsewhere".into(), npc("char-elsewhere", Some("loc-mine"), false));
        st.characters.insert("char-nomad".into(), npc("char-nomad", None, false));
        let (session, _sink) = session_with_state(sb, st);

        // ③ 两个字段都未声明 → 全员在场（旧代码把缺省名单当空集，一切场就清空所有人）
        session.switch_scene("sc-none");
        let on = |id: &str| session.projection().characters[id]["present"].as_bool().unwrap();
        assert!(on("char-tavern") && on("char-elsewhere") && on("char-nomad"), "未声明的场景不再清场");
        assert!(on("char-a"), "受控角色恒在场");

        // ② 只声明地点 → 位置匹配决定在场（清空 present_char_ids 后位置接管）
        session.switch_scene("sc-tavern");
        assert!(on("char-tavern"), "常驻该地点 → 在场");
        assert!(!on("char-elsewhere"), "常驻别处 → 不在场");
        assert!(!on("char-nomad"), "没有常驻地 → 不在场");
        assert!(on("char-a"), "受控角色恒在场");

        // ① 作者点名优先于 ②：被点名的人即使常驻别处也在场；位置匹配对其他人照旧生效
        session.switch_scene("sc-mixed");
        assert!(on("char-elsewhere"), "点名覆盖位置");
        assert!(on("char-tavern"), "位置匹配照旧生效");
        assert!(!on("char-nomad"));

        // 显式空名单 ≠ 未声明：作者说这一幕没人
        session.switch_scene("sc-empty");
        assert!(!on("char-tavern") && !on("char-elsewhere") && !on("char-nomad"));
        assert!(on("char-a"), "受控角色恒在场");
    }

    /// 回归（设计 §8 验收 1）：旧故事书（每个场景都写了 present_char_ids、人物没有常驻地）
    /// 连续切场时，在场名单与改动前的判据 `present_ids.contains || controlled` 逐字一致。
    #[test]
    fn legacy_storybook_rosters_unchanged_across_scene_switches() {
        let scenes = json!([
            { "id": "sc-1", "title": "甲", "location_id": "loc-a", "present_char_ids": ["char-b"] },
            { "id": "sc-2", "title": "乙", "location_id": "loc-b", "present_char_ids": ["char-c"] },
            { "id": "sc-3", "title": "丙", "present_char_ids": ["char-b", "char-c"] }
        ]);
        let sb = json!({ "skeleton": [{ "id": "ch-1", "scenes": scenes.clone() }] });
        let mut st = state_with_pc();
        let npc = |id: &str, location_id: Option<&str>, present: bool| CharacterInstance {
            instance_id: format!("inst-{id}"),
            template_id: id.into(),
            name: id.into(),
            kind: "npc".into(),
            attributes: Default::default(),
            resources: Default::default(),
            inventory: Default::default(),
            location_id: location_id.map(str::to_string),
            present,
            statuses: vec![],
        };
        // 旧档开档时按 sc-1 的名单判定：char-b 在场、char-c 不在场（人物没有 location_id）
        st.characters.insert("char-b".into(), npc("char-b", None, true));
        st.characters.insert("char-c".into(), npc("char-c", None, false));
        let (session, _sink) = session_with_state(sb, st);
        for target in ["sc-2", "sc-3", "sc-1", "sc-2"] {
            session.switch_scene(target);
            let proj = session.projection();
            let named: Vec<&str> = scenes
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == json!(target))
                .unwrap()["present_char_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let got: Vec<(String, bool)> = proj
                .characters
                .iter()
                .map(|(k, c)| (k.clone(), c["present"].as_bool().unwrap()))
                .collect();
            let expected: Vec<(String, bool)> = proj
                .characters
                .iter()
                .map(|(k, c)| {
                    let template = c["template_id"].as_str().unwrap();
                    (k.clone(), named.contains(&template) || proj.controlled.contains(k))
                })
                .collect();
            assert_eq!(got, expected, "切到 {target} 后在场名单与改动前判据不一致");
        }
    }

    /// Intent::Move 泛化（设计 §4.2）：意图归属的角色移动（NPC 也能被搬），
    /// 未归属时缺省受控角色——旧的「只动受控角色」行为逐字不变。
    #[tokio::test]
    async fn move_intent_moves_named_actor_and_defaults_to_controlled() {
        let mut st = state_with_pc();
        let npc = |id: &str, location_id: Option<&str>, present: bool| CharacterInstance {
            instance_id: format!("inst-{id}"),
            template_id: id.into(),
            name: id.into(),
            kind: "npc".into(),
            attributes: Default::default(),
            resources: Default::default(),
            inventory: Default::default(),
            location_id: location_id.map(str::to_string),
            present,
            statuses: vec![],
        };
        st.characters.insert("char-isa".into(), npc("char-isa", None, true));
        let (session, _sink) = session_with_state(json!({}), st);

        // 指定角色：NPC / 怪物也能被搬
        session
            .handle_intent(
                Intent::Move { destination_id: "loc-tavern".into(), character_id: None },
                Some(ActorRef { id: "char-isa".into(), name: "伊莎".into() }),
            )
            .await;
        let proj = session.projection();
        assert_eq!(proj.characters["char-isa"]["location_id"], json!("loc-tavern"));
        assert!(proj.characters["char-a"].get("location_id").is_none(), "未指定时不该动别人");

        // 未指定 → 受控角色（旧调用行为不变）
        session
            .handle_intent(
                Intent::Move { destination_id: "loc-mine".into(), character_id: None },
                None,
            )
            .await;
        assert_eq!(session.projection().characters["char-a"]["location_id"], json!("loc-mine"));
    }

    struct CapturingAi(StdMutex<Vec<TurnContext>>);

    #[async_trait::async_trait]
    impl AiProvider for CapturingAi {
        async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            self.0.lock().unwrap().push(ctx.clone());
            Ok(AiOutput {
                intents: vec![Intent::FinishTurn.into()],
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    #[tokio::test]
    async fn run_round_context_has_scene_goals_description_and_present_roster() {
        let sb = json!({
            "skeleton": [{
                "id": "ch-1",
                "scenes": [{
                    "id": "sc-1",
                    "title": "场景",
                    "goals": [{ "id": "g1", "text": "打倒地精", "primary": true }]
                }]
            }],
            "world": {}
        });
        let mut st = state_with_pc();
        st.scene_description = Some("秋雨与霜雾。".into());
        st.characters.insert(
            "char-off".into(),
            CharacterInstance {
                instance_id: "inst-char-off".into(),
                template_id: "char-off".into(),
                name: "终局Boss".into(),
                kind: "npc".into(),
                attributes: Default::default(),
                resources: Default::default(),
                inventory: Default::default(),
                location_id: None,
                present: false,
                statuses: vec![],
            },
        );
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let ai = Arc::new(CapturingAi(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            st,
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            sb,
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "我看看周围".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let captured = ai.0.lock().unwrap();
        let ctx = captured.last().expect("story AI 收到上下文");
        assert!(ctx.quests.iter().any(|q| q.source == "skeleton" && q.text == "打倒地精"));
        assert_eq!(ctx.scene_description.as_deref(), Some("秋雨与霜雾。"));
        assert!(ctx.characters.iter().any(|a| a.id == "char-a"));
        assert!(!ctx.characters.iter().any(|a| a.id == "char-off"), "离场角色不进提示词");
        assert_eq!(ctx.scene_id, "sc-1");
        assert!(
            ctx.scenes.iter().any(|s| s.id == "sc-1" && s.title == "场景"),
            "要给出可 advance_scene 的合法场景清单"
        );
    }

    /// 当前地点进回合提示词（设计 §6.6）：TurnContext.location 取「场景 location_id → 地点名」，
    /// 切场后跟着变；未声明地点的场景不注入（老故事书提示词逐字不变）。
    #[tokio::test]
    async fn turn_context_carries_current_location_name() {
        let sb = json!({
            "skeleton": [{ "id": "ch-1", "scenes": [
                { "id": "sc-1", "title": "碎星酒馆的夜晚", "location_id": "loc-tavern" },
                { "id": "sc-2", "title": "矿坑口", "location_id": "loc-mine" },
                { "id": "sc-3", "title": "无名之地" }
            ] }],
            "world": { "locations": [
                { "id": "loc-tavern", "name": "碎星酒馆" },
                { "id": "loc-mine", "name": "废矿坑" }
            ] }
        });
        let mut st = state_with_pc();
        st.scene_id = "sc-1".into();
        st.scene_title = "碎星酒馆的夜晚".into();
        st.locations = json!([
            { "id": "loc-tavern", "name": "碎星酒馆" },
            { "id": "loc-mine", "name": "废矿坑" }
        ])
        .as_array()
        .cloned()
        .unwrap();
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let ai = Arc::new(CapturingAi(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            st,
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            sb,
        ));
        let input = |text: &str| RoundInput { channel: RoundChannel::Character, text: text.into(), refs: vec![] };

        session.run_round(input("看看四周"), None, vec![]).await.unwrap();
        assert_eq!(
            ai.0.lock().unwrap()[0].location.as_deref(),
            Some("碎星酒馆"),
            "初始场景的地点名要进回合上下文"
        );

        session.switch_scene("sc-2");
        session.run_round(input("继续走"), None, vec![]).await.unwrap();
        assert_eq!(
            ai.0.lock().unwrap()[1].location.as_deref(),
            Some("废矿坑"),
            "切场后地点名跟着变"
        );

        session.switch_scene("sc-3");
        session.run_round(input("四下张望"), None, vec![]).await.unwrap();
        assert_eq!(ai.0.lock().unwrap()[2].location, None, "未声明地点的场景不注入地点");
    }

    #[tokio::test]
    async fn narrative_resolves_when_variants_and_player_overrides() {
        let sb = json!({
            "world": {},
            "narrative": { "sections": [
                { "id": "always", "title": "常驻", "slot": "style", "scope": "both", "text": "常驻文本" },
                { "id": "gated", "title": "命中", "slot": "style", "scope": "both", "text": "命中文本",
                  "when": { "op": "flag_set", "flag": "met" } },
                { "id": "blocked", "title": "未命中", "slot": "style", "scope": "both", "text": "不该出现",
                  "when": { "op": "flag_set", "flag": "never" } },
                { "id": "variantsOn", "title": "可变体", "slot": "style", "scope": "both", "playerEditable": true,
                  "variants": [
                    { "key": "a", "label": "甲", "text": "变体甲" },
                    { "key": "b", "label": "乙", "text": "变体乙" }
                  ], "defaultVariant": "b" },
                { "id": "variantsNo", "title": "不可变体", "slot": "style", "scope": "both", "playerEditable": false,
                  "variants": [
                    { "key": "a", "label": "甲", "text": "锁定甲" },
                    { "key": "b", "label": "乙", "text": "锁定乙" }
                  ], "defaultVariant": "b" },
                { "id": "off", "title": "停用", "slot": "style", "scope": "both", "text": "停用文本", "enabled": false },
                { "id": "playerToggle", "title": "可关", "slot": "style", "scope": "both", "text": "可关文本", "playerEditable": true },
                { "id": "lockedToggle", "title": "锁定", "slot": "style", "scope": "both", "text": "锁定文本", "playerEditable": false }
            ]}
        });
        let mut st = state_with_pc();
        st.flags.insert("met".to_string(), json!(true));
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let ai = Arc::new(CapturingAi(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            st,
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            sb,
        ));
        // 玩家偏好：可编辑段关闭 / 选变体；非可编辑段的偏好应被忽略；when 命中的段也被伪造关闭。
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("variantsOn".to_string(), NarrativeOverride::Variant("a".into()));
        overrides.insert("variantsNo".to_string(), NarrativeOverride::Variant("a".into()));
        overrides.insert("playerToggle".to_string(), NarrativeOverride::Enabled(false));
        overrides.insert("lockedToggle".to_string(), NarrativeOverride::Enabled(false));
        overrides.insert("gated".to_string(), NarrativeOverride::Enabled(false));
        session.set_narrative_overrides(overrides);

        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "继续".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let captured = ai.0.lock().unwrap();
        let ctx = captured.last().expect("story AI 收到上下文");
        let ids: Vec<&str> = ctx.narrative.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"always"));
        assert!(ids.contains(&"gated"), "when 命中应注入");
        assert!(!ids.contains(&"blocked"), "when 未命中不应注入");
        assert!(!ids.contains(&"off"), "enabled=false 不应注入");
        assert!(!ids.contains(&"playerToggle"), "playerEditable 段可被玩家关闭");
        assert!(ids.contains(&"lockedToggle"), "非 playerEditable 段忽略玩家偏好");
        let find = |id: &str| ctx.narrative.iter().find(|n| n.id == id).unwrap();
        assert_eq!(find("variantsOn").text, "变体甲", "存档变体选择优先于 defaultVariant");
        assert_eq!(find("variantsNo").text, "锁定乙", "非 playerEditable 段忽略变体选择");
        assert_eq!(find("always").text, "常驻文本");
        assert_eq!(find("gated").text, "命中文本", "when 命中的段忽略伪造的关闭偏好");
    }



    struct FakeRetriever {
        hits: Vec<MemoryHit>,
    }

    #[async_trait::async_trait]
    impl MemoryRetriever for FakeRetriever {
        async fn retrieve(
            &self,
            _save_id: &str,
            _query: &str,
            _k: usize,
        ) -> Result<Vec<MemoryHit>, EngineError> {
            Ok(self.hits.clone())
        }
    }

    struct FailingRetriever;

    #[async_trait::async_trait]
    impl MemoryRetriever for FailingRetriever {
        async fn retrieve(
            &self,
            _save_id: &str,
            _query: &str,
            _k: usize,
        ) -> Result<Vec<MemoryHit>, EngineError> {
            Err(EngineError::Internal("检索器炸了".into()))
        }
    }

    fn session_with_capturing_ai() -> (Arc<Session>, Arc<CapturingAi>) {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let ai = Arc::new(CapturingAi(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            json!({ "world": {} }),
        ));
        (session, ai)
    }

    /// #05 §3.4：注入的检索器把命中填进 TurnContext.memories。
    #[tokio::test]
    async fn wired_retriever_fills_round_memories() {
        let (session, ai) = session_with_capturing_ai();
        session.set_memory_retriever(Some(Arc::new(FakeRetriever {
            hits: vec![MemoryHit {
                seq: 7,
                round: 2,
                kind: "narrate".into(),
                text: "旧事".into(),
                score: 0.8,
            }],
        })));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "还记得吗".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let captured = ai.0.lock().unwrap();
        let ctx = captured.last().expect("story AI 收到上下文");
        assert_eq!(ctx.memories.len(), 1);
        assert_eq!(ctx.memories[0].seq, 7);
        assert_eq!(ctx.memories[0].text, "旧事");
    }

    /// 检索失败静默降级：回合照常结束，memories 为空。
    #[tokio::test]
    async fn retrieval_failure_degrades_silently() {
        let (session, ai) = session_with_capturing_ai();
        session.set_memory_retriever(Some(Arc::new(FailingRetriever)));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "继续".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let captured = ai.0.lock().unwrap();
        let ctx = captured.last().expect("story AI 收到上下文");
        assert!(ctx.memories.is_empty(), "检索失败应静默降级为空");
    }

    /// 回归：PC 名恰好是「你」时，NPC 台词里的「你」是**称呼**，不该把台词判给玩家。
    struct NpcSpeakingAi;

    #[async_trait::async_trait]
    impl AiProvider for NpcSpeakingAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![Intent::Speak { content: "你带伞了吗？".into(), tone: None, actor_id: None }.into()],
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    fn character_instance(id: &str, name: &str, kind: &str) -> CharacterInstance {
        CharacterInstance {
            instance_id: format!("inst-{id}"),
            template_id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            attributes: Default::default(),
            resources: Default::default(),
            inventory: Default::default(),
            location_id: None,
            present: true,
            statuses: vec![],
        }
    }

    #[tokio::test]
    async fn npc_line_containing_pc_name_is_not_attributed_to_the_player() {
        let mut state = state_with_pc();
        let mut chars = std::collections::BTreeMap::new();
        chars.insert("char-pc".to_string(), character_instance("char-pc", "你", "pc"));
        chars.insert("char-lucy".to_string(), character_instance("char-lucy", "露西", "npc"));
        state.characters = chars;
        state.controlled = vec!["char-pc".into()];

        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state,
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(NpcSpeakingAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "我没带伞".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let events = sink.0.lock().unwrap().clone();
        let (actor, content) = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Dialogue(p) => Some((e.actor.clone(), p.content.clone())),
                _ => None,
            })
            .expect("dialogue emitted");
        assert_eq!(content, "你带伞了吗？");
        assert_eq!(
            actor.as_ref().map(|a| a.id.as_str()),
            Some("char-lucy"),
            "台词里的「你」是称呼，归属应是在场 NPC，而不是名为「你」的玩家"
        );
    }

    /// 旁白写的是角色动作（名字在主语位）→ 归属该角色；只是提及（名字不在主语位）→ 不归属。
    struct NarrateActorAi;

    #[async_trait::async_trait]
    impl AiProvider for NarrateActorAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::from_intents(vec![
                Intent::Narrate { content: "露西靠在他身侧，把伞递了过去。".into(), actor_id: None },
                Intent::Narrate { content: "门口那盏灯把台阶照得发亮，露西的车停在那儿。".into(), actor_id: None },
                Intent::FinishTurn,
            ]))
        }
        
    }

    #[tokio::test]
    async fn narrate_actor_is_inferred_only_from_leading_subject() {
        let mut state = state_with_pc();
        let mut chars = std::collections::BTreeMap::new();
        chars.insert("char-pc".to_string(), character_instance("char-pc", "米拉", "pc"));
        chars.insert("char-lucy".to_string(), character_instance("char-lucy", "露西", "npc"));
        state.characters = chars;
        state.controlled = vec!["char-pc".into()];

        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state,
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(NarrateActorAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "我们走吧".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let events = sink.0.lock().unwrap().clone();
        let actors: Vec<(Option<String>, String)> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Narrate(p) => Some((
                    e.actor.as_ref().map(|a| a.id.clone()),
                    p.content.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(actors.len(), 2, "两条旁白都应落地：{actors:?}");
        assert_eq!(
            actors[0].0.as_deref(),
            Some("char-lucy"),
            "「露西靠在他身侧…」是她的动作，应归属露西：{actors:?}"
        );
        assert_eq!(
            actors[1].0.as_deref(),
            Some("char-pc"),
            "「…露西的车停在那儿」只是环境描写里提及，应回落玩家角色：{actors:?}"
        );
    }

    struct ReasoningAi;

    #[async_trait::async_trait]
    impl AiProvider for ReasoningAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![Intent::Narrate { content: "雨。".into(), actor_id: None }.into()],
                reasoning: Some("先想想天气。".into()),
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    #[tokio::test]
    async fn reasoning_events_are_emitted() {
        let sb = json!({ "world": {} });
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let ai = Arc::new(ReasoningAi);
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(ai as Arc<dyn AiProvider>),
            true,
            sb,
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "你好".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let events = sink.0.lock().unwrap().clone();
        let stages: Vec<&str> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Reasoning(p) => Some(p.stage.as_str()),
                _ => None,
            })
            .collect();
        assert!(stages.contains(&"story_thinking"), "单一 AI 的思考要落事件");
        assert!(events.iter().any(|e| matches!(&e.event, PlayEvent::Reasoning(p) if p.text.contains("天气"))));
        // 供应商 reasoning_content 的来源缺省为 provider（兼容既有事件）。
        assert!(events.iter().all(|e| !matches!(&e.event, PlayEvent::Reasoning(p) if p.source != "provider")));
    }

    /// 只输出一条 think 意图的主线 AI：验证它落「模型思考」事件且不产生叙事。
    struct ThinkingAi;

    #[async_trait::async_trait]
    impl AiProvider for ThinkingAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![Intent::Think { content: "先在心里推演一遍。".into() }.into()],
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    #[tokio::test]
    async fn think_intent_emits_model_reasoning_without_narrative_or_state() {
        let sb = json!({ "world": {} });
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(ThinkingAi) as Arc<dyn AiProvider>),
            true,
            sb,
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "我想想".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let events = sink.0.lock().unwrap().clone();
        // think 映射为 source=model 的 Reasoning，且 stage 标为 model_draft。
        assert!(
            events.iter().any(|e| matches!(
                &e.event,
                PlayEvent::Reasoning(p) if p.source == "model" && p.stage == "model_draft" && p.text == "先在心里推演一遍。"
            )),
            "think 意图必须落一条 source=model 的思考事件"
        );
        // 不进叙事条目：不能产生 narrate / dialogue / emote。
        assert!(
            !events.iter().any(|e| matches!(&e.event, PlayEvent::Narrate(_) | PlayEvent::Dialogue(_) | PlayEvent::Emote(_))),
            "think 不得产生叙事事件"
        );
        // 不改世界状态：不能产生 StateUpdate。
        assert!(
            !events.iter().any(|e| matches!(&e.event, PlayEvent::StateUpdate(_))),
            "think 不得改世界状态"
        );
    }

    #[tokio::test]
    async fn run_round_passes_save_model_to_ai() {
        let ai = Arc::new(CapturingAi(StdMutex::new(vec![])));
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            json!({}),
        ));
        // 本存档指定的单一模型应进 TurnContext，供 provider 解析。
        session.set_model(Some(ModelRef { provider_id: "p1".into(), model: "m1".into(), reasoning_effort: Some("high".into()) }));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "看看四周".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let captured = ai.0.lock().unwrap();
        let ctx = captured.last().expect("AI 收到上下文");
        assert_eq!(
            ctx.model
                .as_ref()
                .map(|m| (m.provider_id.as_str(), m.model.as_str(), m.reasoning_effort.as_deref())),
            Some(("p1", "m1", Some("high")))
        );
    }

    #[tokio::test]
    async fn run_round_unset_save_model_falls_back_to_none() {
        // 未设置 / 旧存档：None，由 provider 回落到全局默认。
        let ai = Arc::new(CapturingAi(StdMutex::new(vec![])));
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            json!({}),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "看看四周".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let captured = ai.0.lock().unwrap();
        let ctx = captured.last().expect("AI 收到上下文");
        assert!(ctx.model.is_none());
    }


    #[test]
    fn id_less_goal_is_tracked_after_upcast_backfill() {
        // 导入故事书常见 goal 无 id；upcast 回填后 evaluate + projection 才能对上同一个键。
        let mut sb = json!({
            "schema_version": 3,
            "skeleton": [{
                "id": "ch-1",
                "scenes": [{
                    "id": "sc-1",
                    "title": "场景",
                    "goals": [{
                        "text": "打倒地精",
                        "primary": true,
                        "condition": { "op": "flag_set", "flag": "met" }
                    }]
                }]
            }]
        });
        crate::upcast::upcast_storybook(&mut sb);
        let (session, _sink) = session_with(sb);
        session.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![StateDelta {
                domain: DeltaDomain::Flag,
                entity_id: "met".into(),
                field: "flag".into(),
                op: DeltaOp::Set,
                value: json!(true),
            }],
        }));
        session.evaluate_turn_end();

        let proj = session.projection();
        assert_eq!(
            proj.progress.goals.get("sc-1#goal[0]"),
            Some(&json!(true)),
            "回填 id 后目标达成要落进进度"
        );
        assert!(proj.quests[0].done, "同 id 的骨架任务要反映 done");
    }

    struct ScriptedSeq(StdMutex<Vec<Vec<Intent>>>);

    #[async_trait::async_trait]
    impl AiProvider for ScriptedSeq {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            let mut q = self.0.lock().unwrap();
            let intents: Vec<IntentEnvelope> = if q.is_empty() {
                vec![]
            } else {
                q.remove(0).into_iter().map(IntentEnvelope::from).collect()
            };
            Ok(AiOutput {
                intents,
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    #[tokio::test]
    async fn rerun_rolls_back_and_reuses_same_round() {
        let ai = Arc::new(ScriptedSeq(StdMutex::new(vec![
            vec![Intent::Quest { text: "任务A".into(), hidden: false, primary: true }],
            vec![Intent::Quest { text: "任务B".into(), hidden: false, primary: true }],
        ])));
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            json!({}),
        ));
        let input = RoundInput { channel: RoundChannel::Gm, text: "看看四周".into(), refs: vec![] };
        session.run_round(input, None, vec![]).await.unwrap();
        assert!(session.projection().quests.iter().any(|q| q.text == "任务A"));

        let plan = session.rewind_plan().expect("有可回滚回合");
        assert_eq!(plan.round, 1);
        assert_eq!(plan.input.text, "看看四周");
        let old_start_seq = plan.from_seq;
        session.apply_rewind(plan.from_seq, plan.round);

        let rolled = session.projection();
        assert!(rolled.quests.is_empty(), "旧回合的任务必须被回滚");
        assert!(rolled.progress.goals.is_empty());

        // 重跑：复用同一回合号，但事件用新 seq（不复用旧 seq，避免与归档 / 广播冲突）
        session.run_round(plan.input.clone(), None, vec![]).await.unwrap();
        let proj = session.projection();
        assert_eq!(proj.quests.len(), 1);
        assert_eq!(proj.quests[0].text, "任务B");

        let log = session.event_log.lock().unwrap();
        let (round, seq) = log
            .iter()
            .rev()
            .find_map(|e| match &e.event {
                PlayEvent::RoundStart(_) => Some((e.round, e.seq)),
                _ => None,
            })
            .expect("重跑的 round_start");
        assert_eq!(round, 1, "重跑必须复用同一回合号");
        assert!(seq > old_start_seq, "重跑事件用新 seq");
    }

    // ---------- #05 §3.2/§3.3 摘要派生数据 ----------

    #[derive(Default)]
    struct FakeSummaryStore {
        rounds: StdMutex<Vec<(u32, String)>>,
        scenes: StdMutex<Vec<(String, u32, String)>>,
    }

    impl FakeSummaryStore {
        fn with_rounds(rounds: &[(u32, &str)]) -> Self {
            Self {
                rounds: StdMutex::new(rounds.iter().map(|(r, t)| (*r, t.to_string())).collect()),
                scenes: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::ports::SummaryStore for FakeSummaryStore {
        async fn put_round_summary(
            &self,
            _save_id: &str,
            round: u32,
            text: &str,
        ) -> Result<(), EngineError> {
            self.rounds.lock().unwrap().push((round, text.to_string()));
            Ok(())
        }
        async fn round_summaries_after(
            &self,
            _save_id: &str,
            after_round: u32,
        ) -> Result<Vec<(u32, String)>, EngineError> {
            Ok(self
                .rounds
                .lock()
                .unwrap()
                .iter()
                .filter(|(r, _)| *r > after_round)
                .cloned()
                .collect())
        }
        async fn put_scene_summary(
            &self,
            _save_id: &str,
            scene_id: &str,
            round: u32,
            text: &str,
        ) -> Result<(), EngineError> {
            self.scenes
                .lock()
                .unwrap()
                .push((scene_id.to_string(), round, text.to_string()));
            Ok(())
        }
    }

    /// 写入永远失败的摘要端口：验证派生写失败也不影响回合。
    struct FailingSummaryStore;

    #[async_trait::async_trait]
    impl crate::ports::SummaryStore for FailingSummaryStore {
        async fn put_round_summary(&self, _s: &str, _r: u32, _t: &str) -> Result<(), EngineError> {
            Err(EngineError::Internal("摘要库挂了".into()))
        }
        async fn round_summaries_after(
            &self,
            _s: &str,
            _r: u32,
        ) -> Result<Vec<(u32, String)>, EngineError> {
            Err(EngineError::Internal("摘要库挂了".into()))
        }
        async fn put_scene_summary(
            &self,
            _s: &str,
            _sc: &str,
            _r: u32,
            _t: &str,
        ) -> Result<(), EngineError> {
            Err(EngineError::Internal("摘要库挂了".into()))
        }
    }

    /// #05 §3.2：summary 意图只写派生表，不产生叙事事件、不改世界状态。
    #[tokio::test]
    async fn summary_intent_writes_derived_store_without_narrative_or_state() {
        let (session, sink) = session_with(json!({}));
        let store = Arc::new(FakeSummaryStore::default());
        session.set_summary_store(Some(store.clone() as Arc<dyn crate::ports::SummaryStore>));
        session.round.store(3, Ordering::SeqCst);

        session
            .handle_intent(
                Intent::Summary { text: "  米拉在酒馆听到了传闻。  ".into() },
                None,
            )
            .await;

        // 写进派生表，且文本已 trim。
        assert_eq!(
            store.rounds.lock().unwrap().clone(),
            vec![(3, "米拉在酒馆听到了传闻。".to_string())]
        );
        // 不产生任何事件（含叙事事件），权威 seq / 场景状态逐字不变。
        assert!(sink.0.lock().unwrap().is_empty(), "summary 意图不得 emit 任何事件");
        assert_eq!(session.seq.load(Ordering::SeqCst), 0, "不得改变权威 seq");
        assert_eq!(session.projection().scene_id, "sc-1");
    }

    /// #05 §3.3：advance_scene 结算后把本场景的回合微摘要压缩落表；
    /// 默认 summarize（None）退化为确定性拼接。
    #[tokio::test]
    async fn advance_scene_compresses_scene_summaries_with_concat_fallback() {
        let (session, _sink) = session_with(json!({}));
        let store =
            Arc::new(FakeSummaryStore::with_rounds(&[(1, "第1回合摘要"), (2, "第2回合摘要")]));
        session.set_summary_store(Some(store.clone() as Arc<dyn crate::ports::SummaryStore>));
        session.scene_start_round.store(0, Ordering::SeqCst);
        session.round.store(4, Ordering::SeqCst);

        session
            .handle_intent(Intent::AdvanceScene { target_scene_id: None, abandon: false }, None)
            .await;

        let scenes = store.scenes.lock().unwrap().clone();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].0, "sc-1", "压缩离开的是切换前的场景");
        assert_eq!(scenes[0].1, 4);
        assert_eq!(scenes[0].2, "第1回合摘要\n第2回合摘要", "None → 拼接兜底");
        assert_eq!(session.scene_start_round.load(Ordering::SeqCst), 4, "窗口左界推进到本回合");
    }

    struct FailingSummaryAi;

    #[async_trait::async_trait]
    impl AiProvider for FailingSummaryAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![
                    Intent::Narrate { content: "你们离开了矿洞。".into(), actor_id: None }.into(),
                    Intent::Summary { text: "本回合离开了矿洞".into() }.into(),
                    Intent::AdvanceScene { target_scene_id: None, abandon: false }.into(),
                ],
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
        async fn summarize(&self, _text: &str) -> Result<Option<String>, EngineError> {
            Err(EngineError::Ai("压缩挂了".into()))
        }
    }

    /// #05 硬不变量：AI 压缩失败 / 摘要写库失败都只 warn，绝不失败回合。
    #[tokio::test]
    async fn summary_failures_never_fail_round() {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(Arc::new(FailingSummaryAi)),
            true,
            json!({}),
        ));
        let store = Arc::new(FakeSummaryStore::with_rounds(&[(1, "旧摘要")]));
        session.set_summary_store(Some(store.clone() as Arc<dyn crate::ports::SummaryStore>));

        let res = session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "离开".into(), refs: vec![] },
                None,
                vec![],
            )
            .await;
        assert!(res.is_ok(), "压缩失败不得让回合失败");
        // summarize 返回 Err → 拼接兜底仍落一条场景摘要（含本回合摘要意图的文本）。
        let scenes = store.scenes.lock().unwrap().clone();
        assert_eq!(scenes.len(), 1);
        assert!(scenes[0].2.contains("旧摘要"), "失败也要退化为拼接: {:?}", scenes[0].2);
        assert!(store.rounds.lock().unwrap().iter().any(|(_, t)| t == "本回合离开了矿洞"));

        // 摘要端口整体故障：写微摘要 / 读窗口都报错，回合照常。
        let sink2 = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session2 = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink2 as Arc<dyn EventSink>,
            ai_slot(Arc::new(FailingSummaryAi)),
            true,
            json!({}),
        ));
        session2.set_summary_store(Some(Arc::new(FailingSummaryStore)));
        assert!(
            session2
                .run_round(
                    RoundInput { channel: RoundChannel::Character, text: "离开".into(), refs: vec![] },
                    None,
                    vec![],
                )
                .await
                .is_ok(),
            "摘要库故障不得让回合失败"
        );
    }

    /// #05 硬不变量：replay 不读摘要（摘要非权威），
    /// 但据权威 advance_scene 事件重建场景压缩窗口左界。
    #[tokio::test]
    async fn replay_ignores_summaries_and_rebuilds_scene_window() {
        let (session, _sink) = session_with(json!({}));
        let store =
            Arc::new(FakeSummaryStore::with_rounds(&[(1, "旧场景摘要"), (2, "新场景摘要")]));
        session.set_summary_store(Some(store.clone() as Arc<dyn crate::ports::SummaryStore>));

        session.replay(&[PersistedEvent {
            request_id: None,
            envelope: EventEnvelope {
                id: "ev-1".into(),
                seq: 1,
                round: 1,
                ts: "t".into(),
                actor: None,
                intent_id: None,
                event: PlayEvent::Resolution(ResolutionPayload {
                    intent_id: None,
                    status: ResolutionStatus::Ok,
                    rejection_code: None,
                    narrative: Some("场景推进。".into()),
                    outcome: Some("advance_scene".into()),
                    triggered_events: None,
                    state_changes: vec![],
                }),
            },
        }]);

        // 重放后按最后的 advance_scene（round 1）设定窗口：只压缩 round > 1 的摘要。
        session.round.store(2, Ordering::SeqCst);
        session
            .handle_intent(Intent::AdvanceScene { target_scene_id: None, abandon: false }, None)
            .await;
        assert_eq!(
            store.scenes.lock().unwrap()[0].2,
            "新场景摘要",
            "重放不读摘要表，但窗口左界来自权威 advance_scene 事件"
        );
    }

    // ============================================================
    // #04 / #12 本切片测试：声明式判定 / intent_id 去重 / 查询 / 物件交互 / actor 约束
    // ============================================================

    /// #04/#12：check 意图走故事书 world.check（骰式 / 修正 / 分档）；
    /// 未声明 world.check 时回落今天的 1d20（无修正）。
    #[tokio::test]
    async fn check_intent_uses_declarative_checker_and_defaults_to_1d20() {
        let sb = json!({
            "world": { "check": {
                "dice": "1d20",
                "attribute_modifier": { "str": 7 },
                "degree_thresholds": [100, -100, -100]
            } }
        });
        let (session, sink) = session_with(sb);
        session
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(10), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.clone()),
                _ => None,
            })
            .expect("check result");
        assert_eq!(check.expr.as_deref(), Some("1d20"));
        assert_eq!(check.r#mod, 7, "故事书 attribute_modifier 必须生效");
        assert_eq!(check.target, 10);
        assert_eq!(check.total, check.rolls.as_ref().unwrap()[0] + 7);
        assert_eq!(check.level, SuccessLevel::Success, "按 degree_thresholds 分档");
        assert_eq!(check.result, check.total >= 10);

        // 缺省故事书：仍按 1d20、无修正（不套中心偏移公式）。
        let (session2, sink2) = session_with(json!({ "world": {} }));
        session2
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(12), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        let check2 = events2
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.clone()),
                _ => None,
            })
            .expect("default check result");
        assert_eq!(check2.expr.as_deref(), Some("1d20"));
        assert_eq!(check2.r#mod, 0, "无 world.check 时不应用属性修正");
        assert_eq!(check2.rolls.as_ref().unwrap().len(), 1);
    }

    // ---------- 判定 C3：对抗判定（双方各得一次值比大小） ----------

    /// 一个在场的怪物实例（对手的「角色实例」形态）+ 一个只在故事书里声明属性的
    /// 人物条目（对手的「静态被动值」形态）。
    fn opponent_state() -> WorldState {
        let mut st = state_with_pc();
        st.characters.insert(
            "char-goblin".to_string(),
            CharacterInstance {
                instance_id: "inst-char-goblin".into(),
                template_id: "char-goblin".into(),
                name: "地精斥候".into(),
                kind: "monster".into(),
                attributes: json!({ "insight": 14, "stealth": 18 })
                    .as_object()
                    .unwrap()
                    .clone(),
                resources: json!({ "hp": 7 }).as_object().unwrap().clone(),
                inventory: Default::default(),
                location_id: None,
                present: true,
                statuses: vec![],
            },
        );
        st
    }

    fn last_check(sink: &Arc<CaptureSink>) -> CheckResultPayload {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.clone()),
                _ => None,
            })
            .expect("应有判定结果事件")
    }

    /// 判定 C3 验收 1 / 2 / 3 / 5：**掷 vs 掷**——对手是角色实例时双方各掷一次；
    /// target = 对手 total（不是静态难度），对手走 opposed_attribute，署名落 payload。
    #[tokio::test]
    async fn opposed_check_rolls_both_sides_against_a_character_opponent() {
        let sb = json!({
            "world": { "check": {
                "dice": "1d20",
                "mode": "opposed",
                "attribute": "dex",
                "opposed_attribute": "insight",
                "attribute_modifier": { "dex": 1, "insight": 7 }
            } }
        });
        let (session, sink) = session_with_state(sb, opponent_state());
        session
            .handle_intent(
                Intent::Check {
                    attribute: "dex".into(),
                    // 对抗不看这个难度（写 5 就是为了证明「不是与静态数值比」）。
                    difficulty: Some(5),
                    actor_id: None,
                    opponent_id: Some("char-goblin".into()),
                },
                None,
            )
            .await;
        let check = last_check(&sink);
        let consumed = session.rng.lock().unwrap().consumed.clone();
        assert_eq!(consumed.len(), 2, "双方各掷一次：RNG 消耗 = 2");
        let own_roll = 1 + (consumed[0] % 20) as i64;
        let opponent_roll = 1 + (consumed[1] % 20) as i64;
        assert_eq!(check.rolls, Some(vec![own_roll]));
        assert_eq!(check.total, own_roll + 1, "主动方用主动属性 dex 的修正");
        assert_eq!(
            check.target,
            opponent_roll + 7,
            "target = 对手 total（对手走 opposed_attribute = insight）"
        );
        assert_eq!(check.margin, check.total - check.target);
        assert_eq!(check.result, check.total >= check.target);
        assert_eq!(
            check.opponent,
            Some(ActorRef { id: "char-goblin".into(), name: "地精斥候".into() })
        );
    }

    /// 判定 C3 验收 1 / 3 / 5：**掷 vs 被动**——对手命中不了角色实例时用静态被动值
    /// （passive_base + 属性修正），对手不掷骰。LMoP「隐匿 vs 被动察觉」正是这一形态。
    #[tokio::test]
    async fn opposed_check_uses_static_passive_value_for_a_non_instance_opponent() {
        let sb = json!({
            "attribute_dimensions": [
                { "key": "perception", "baseline": 10, "modifier_step": 2 }
            ],
            "characters": [
                { "id": "goblin-passive", "name": "地精（被动）", "kind": "monster",
                  "attributes": { "perception": 16 } }
            ],
            "world": { "check": {
                "dice": "1d20",
                "mode": "opposed",
                "attribute": "dex",
                "opposed_attribute": "perception",
                "passive_base": 10
            } }
        });
        let (session, sink) = session_with_state(sb, state_with_pc());
        session
            .handle_intent(
                Intent::Check {
                    attribute: "dex".into(),
                    difficulty: None,
                    actor_id: None,
                    opponent_id: Some("goblin-passive".into()),
                },
                None,
            )
            .await;
        let check = last_check(&sink);
        let consumed = session.rng.lock().unwrap().consumed.clone();
        assert_eq!(consumed.len(), 1, "静态被动值不掷骰：只有主动方消耗 RNG");
        // passive_base 10 + floor((16 - 10) / 2) = 13
        assert_eq!(check.target, 13, "静态被动值 = passive_base + 属性修正");
        assert_eq!(check.total, 1 + (consumed[0] % 20) as i64);
        assert_eq!(check.margin, check.total - 13);
        assert_eq!(
            check.opponent,
            Some(ActorRef { id: "goblin-passive".into(), name: "地精（被动）".into() })
        );
    }

    /// 判定 C3 验收 1（第三种组合）：**被动 vs 掷**——主动方 kind = passive 不掷骰，
    /// 对手是角色实例，掷一次。
    #[tokio::test]
    async fn opposed_passive_side_is_compared_with_a_rolling_opponent() {
        let sb = json!({
            "world": { "check": {
                "dice": "1d20",
                "mode": "opposed",
                "kind": "passive",
                "passive_base": 10,
                "attribute": "insight",
                "opposed_attribute": "stealth",
                "attribute_modifier": { "insight": 4, "stealth": 2 }
            } }
        });
        let (session, sink) = session_with_state(sb, opponent_state());
        session
            .handle_intent(
                Intent::Check {
                    attribute: "insight".into(),
                    difficulty: None,
                    actor_id: None,
                    opponent_id: Some("char-goblin".into()),
                },
                None,
            )
            .await;
        let check = last_check(&sink);
        let consumed = session.rng.lock().unwrap().consumed.clone();
        assert_eq!(consumed.len(), 1, "被动一侧不掷骰，只有对手掷一次");
        assert_eq!(check.rolls, None, "被动判定没有骰面");
        assert_eq!(check.total, 10 + 4, "被动值 = passive_base + 修正");
        assert_eq!(check.target, 1 + (consumed[0] % 20) as i64 + 2);
        assert_eq!(
            check.opponent,
            Some(ActorRef { id: "char-goblin".into(), name: "地精斥候".into() })
        );
    }

    /// 判定 C3 验收 4：mode = opposed 却没有对手 → 显式驳回（不再静默降级成 gte）。
    #[tokio::test]
    async fn opposed_without_opponent_is_rejected() {
        let sb = json!({ "world": { "check": { "dice": "1d20", "mode": "opposed" } } });
        let (session, sink) = session_with_state(sb, state_with_pc());
        session
            .handle_intent(
                Intent::Check {
                    attribute: "str".into(),
                    difficulty: Some(12),
                    actor_id: None,
                    opponent_id: None,
                },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        assert!(
            !events.iter().any(|e| matches!(&e.event, PlayEvent::CheckResult(_))),
            "没有对手就不该有判定结果"
        );
        let rejected = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.status == ResolutionStatus::Rejected => Some(p),
                _ => None,
            })
            .expect("应显式驳回");
        assert_eq!(rejected.rejection_code.as_deref(), Some("rule_violation"));
        assert!(session.rng.lock().unwrap().consumed.is_empty(), "驳回不掷骰");
    }

    /// 判定 C3 验收 5：判定器没声明 opposed_attribute 时，对手用主动方同一属性。
    #[tokio::test]
    async fn opponent_attribute_falls_back_to_the_active_attribute() {
        let sb = json!({ "world": { "check": {
            "dice": "1d20",
            "mode": "opposed",
            "attribute": "dex",
            "attribute_modifier": { "dex": 1, "insight": 7 }
        } } });
        let (session, sink) = session_with_state(sb, opponent_state());
        session
            .handle_intent(
                Intent::Check {
                    attribute: "dex".into(),
                    difficulty: None,
                    actor_id: None,
                    opponent_id: Some("char-goblin".into()),
                },
                None,
            )
            .await;
        let check = last_check(&sink);
        let consumed = session.rng.lock().unwrap().consumed.clone();
        assert_eq!(consumed.len(), 2);
        let opponent_roll = 1 + (consumed[1] % 20) as i64;
        assert_eq!(
            check.target,
            opponent_roll + 1,
            "缺省取主动方属性 dex（修正 1），不是 opposed_attribute 缺省成 insight 的 7"
        );
    }

    /// 判定 C3 验收 6：非 opposed 的判定逐字不变——单次掷骰、opponent 为空、摘要格式不变。
    #[tokio::test]
    async fn non_opposed_check_stays_byte_for_byte_the_same() {
        let sb = json!({ "world": { "check": {
            "dice": "1d20",
            "attribute_modifier": { "str": 7 },
            "degree_thresholds": [100, -100, -100]
        } } });
        let (session, sink) = session_with_state(sb, state_with_pc());
        let out = session
            .handle_intent(
                Intent::Check {
                    attribute: "str".into(),
                    difficulty: Some(10),
                    actor_id: None,
                    opponent_id: None,
                },
                None,
            )
            .await;
        let check = last_check(&sink);
        let consumed = session.rng.lock().unwrap().consumed.clone();
        assert_eq!(consumed.len(), 1, "非对抗只掷一次");
        let roll = 1 + (consumed[0] % 20) as i64;
        assert_eq!(check.opponent, None);
        assert_eq!(check.target, 10);
        assert_eq!(check.total, roll + 7);
        assert_eq!(
            out,
            Some(format!(
                "判定「str」：成功（骰 {roll}，修正 7，总值 {}，难度 10）",
                roll + 7
            ))
        );
    }

    /// 判定 C3 验收 3：对手寻址与 actor / target 同一口径——模板 id / 角色名 / 实例键
    /// / instance_id 都能命中同一个角色实例（命中不了才走静态被动值）。
    #[tokio::test]
    async fn opponent_id_accepts_template_id_name_and_instance_key() {
        let sb = json!({ "world": { "check": {
            "dice": "1d20", "mode": "opposed", "attribute": "dex"
        } } });
        for id in ["char-goblin", "地精斥候", "inst-char-goblin"] {
            let (session, sink) = session_with_state(sb.clone(), opponent_state());
            session
                .handle_intent(
                    Intent::Check {
                        attribute: "dex".into(),
                        difficulty: None,
                        actor_id: None,
                        opponent_id: Some(id.into()),
                    },
                    None,
                )
                .await;
            let check = last_check(&sink);
            assert_eq!(
                check.opponent,
                Some(ActorRef { id: "char-goblin".into(), name: "地精斥候".into() }),
                "对手 id `{id}` 必须命中角色实例"
            );
            // 命中实例 = 对手掷骰，所以 RNG 消耗是 2（主动方一次 + 对手一次）。
            assert_eq!(session.rng.lock().unwrap().consumed.len(), 2, "对手 `{id}` 应掷骰");
        }
    }

    // ---------- 判定 C2：攻击与技能共用一个内核 ----------

    /// 造一个含单只敌人的遭遇；ac 由调用方给（0 = 未声明防御值）。
    fn state_with_encounter(ac: i64) -> WorldState {
        let mut st = state_with_pc();
        st.encounters.insert(
            "enc-1".to_string(),
            json!({
                "id": "enc-1", "name": "洞穴", "active": true,
                "enemies": [{ "id": "e1", "name": "灰狼", "hp": 20, "max": 20, "ac": ac }]
            }),
        );
        st
    }

    /// 再加一个角色：验 actor / target 的任意 id 寻址。
    fn state_with_ally() -> WorldState {
        let mut st = state_with_pc();
        st.characters.insert(
            "char-b".to_string(),
            CharacterInstance {
                instance_id: "inst-char-b".into(),
                template_id: "char-b".into(),
                name: "卢克".into(),
                kind: "npc".into(),
                attributes: json!({ "str": 50 }).as_object().unwrap().clone(),
                resources: json!({ "hp": 12 }).as_object().unwrap().clone(),
                inventory: Default::default(),
                location_id: None,
                present: true,
                statuses: vec![],
            },
        );
        st
    }

    fn strike_narrative(sink: &Arc<CaptureSink>) -> String {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("strike") => {
                    p.narrative.clone()
                }
                _ => None,
            })
            .next_back()
            .unwrap_or_default()
    }

    fn enemy_hp(session: &Arc<Session>, enc: &str, idx: usize) -> i64 {
        let st = session.state.lock().unwrap();
        st.encounters[enc]["enemies"][idx]["hp"].as_i64().unwrap_or(-1)
    }

    /// 验收 1：带 Lua 判定器的攻击技能真正执行 Lua。
    ///
    /// 旧实现绕过 resolve_checker 直接调 resolve_declarative_check：Lua 判定器不掷骰，
    /// total = 0 + mod → 除极端情况外恒未命中。
    #[tokio::test]
    async fn strike_runs_lua_checker_and_deals_damage() {
        let sb = json!({
            "skills": [{
                "id": "sk-sneak", "name": "背刺",
                "check": { "lua": "return { total = 100, margin = 100 }" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "3", "resource": "hp" }] }
            }]
        });
        let (session, sink) = session_with_state(sb, state_with_encounter(15));
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-sneak".into()), ..Default::default() },
                None,
            )
            .await;
        let narrative = strike_narrative(&sink);
        assert!(narrative.contains("造成 3 点伤害"), "Lua 判定器必须被真正执行：{narrative}");
        assert_eq!(enemy_hp(&session, "enc-1", 0), 17);
    }

    /// 验收 2：`check: \"world\"`（Ref）读 world.check 的骰式 / 修正 / 阈值，不再静默换裸 1d20。
    #[tokio::test]
    async fn strike_ref_checker_reads_global_check() {
        let sb = json!({
            "world": { "check": {
                "dice": "1d20",
                "attribute_modifier": { "str": 100 },
                "degree_thresholds": [1000, 1000, 1000]
            } },
            "skills": [{
                "id": "sk-atk", "name": "挥砍", "check": "world",
                "effect": { "immediate": [{ "kind": "damage", "amount": "1", "resource": "hp" }] }
            }]
        });
        let (session, sink) = session_with_state(sb, state_with_encounter(15));
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-atk".into()), ..Default::default() },
                None,
            )
            .await;
        let narrative = strike_narrative(&sink);
        assert!(narrative.contains("修正 100"), "Ref 必须用 world.check 的修正：{narrative}");
        assert!(!narrative.contains("骰 0"), "Ref 必须真的掷 1d20：{narrative}");
        assert_eq!(enemy_hp(&session, "enc-1", 0), 19);
    }

    /// 验收 3：攻击参与 attribute_bonuses（挂接定义 / 已装备物品），与 use_skill 同口径。
    #[tokio::test]
    async fn strike_and_use_skill_share_attribute_bonuses() {
        let sb = json!({
            "attribute_dimensions": [{ "key": "str", "baseline": 10, "modifier_step": 2, "min": 1, "max": 200 }],
            "definitions": [{ "id": "def-mighty", "modifiers": [{ "target": "str", "value": 4 }] }],
            "items": [{ "id": "it-gloves", "name": "力手套", "modifiers": [{ "target": "str", "value": 6 }] }],
            "characters": [{
                "id": "char-a", "kind": "pc",
                "attachments": { "trait": ["def-mighty"] },
                "equipped": ["it-gloves"]
            }],
            "skills": [{
                "id": "sk-atk", "name": "挥砍", "attribute": "str",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "1", "resource": "hp" }] }
            }]
        });
        let (session, sink) = session_with_state(sb, state_with_encounter(15));
        let skill = session.rules.skill("sk-atk").cloned().unwrap();
        // 技能路径：修正 = floor((70 基础 + 4 挂接 + 6 已装备 - 10) / 2) = 35
        session.resolve_skill(None, &skill, None, None);
        let skill_mod = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.r#mod),
                _ => None,
            })
            .next_back();
        assert_eq!(skill_mod, Some(35), "技能路径的修正含挂接定义与已装备物品的加值");
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-atk".into()), ..Default::default() },
                None,
            )
            .await;
        let narrative = strike_narrative(&sink);
        assert!(narrative.contains("修正 35"), "攻击必须与技能同口径：{narrative}");
    }

    /// 验收 4：难度来源统一 —— 目标 AC → world.check.default_dc → 12。
    #[tokio::test]
    async fn strike_difficulty_uses_ac_then_default_dc_then_twelve() {
        // 技能自带固定修正：命中无悬念，难度差异只体现在叙事里。
        let attack = json!({
            "id": "sk-atk", "name": "挥砍",
            "check": { "dice": "1d20", "attribute_modifier": { "str": 100 } },
            "effect": { "immediate": [{ "kind": "damage", "amount": "1", "resource": "hp" }] }
        });
        let with_dc = json!({
            "world": { "check": { "dice": "1d20", "default_dc": 20 } },
            "skills": [attack.clone()]
        });
        let choice = || AttackChoice { skill_id: Some("sk-atk".into()), ..Default::default() };
        // ① 敌人 AC 优先
        let (s1, k1) = session_with_state(with_dc.clone(), state_with_encounter(15));
        s1.strike_enemy("e1".into(), choice(), None).await;
        assert!(strike_narrative(&k1).contains("≥ 15"), "{}", strike_narrative(&k1));
        // ② AC 缺省（0）→ default_dc
        let (s2, k2) = session_with_state(with_dc, state_with_encounter(0));
        s2.strike_enemy("e1".into(), choice(), None).await;
        assert!(strike_narrative(&k2).contains("≥ 20"), "{}", strike_narrative(&k2));
        // ③ 两处都缺省 → 12
        let sb = json!({ "skills": [attack] });
        let (s3, k3) = session_with_state(sb, state_with_encounter(0));
        s3.strike_enemy("e1".into(), choice(), None).await;
        assert!(strike_narrative(&k3).contains("≥ 12"), "{}", strike_narrative(&k3));
    }

    /// 验收 4（技能路径）：resolve_skill 读 default_dc，不再硬编码 12。
    #[test]
    fn resolve_skill_difficulty_follows_default_dc() {
        let sb = json!({
            "world": { "check": { "dice": "1d20", "default_dc": 7 } },
            "skills": [{ "id": "sk-hit", "name": "敲击", "check": { "dice": "1d20" }, "effect": {} }]
        });
        let (session, sink) = session_with_state(sb, state_with_encounter(0));
        let skill = session.rules.skill("sk-hit").cloned().unwrap();
        session.resolve_skill(None, &skill, None, None);
        let target = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.target),
                _ => None,
            })
            .expect("check result");
        assert_eq!(target, 7, "难度取自 world.check.default_dc");

        // 无 world.check.default_dc → 回落 12
        let (session2, sink2) = session_with_state(json!({}), state_with_encounter(0));
        let skill2 = {
            let mut s = skill.clone();
            s.id = "sk-hit".into();
            s
        };
        session2.resolve_skill(None, &skill2, None, None);
        let target2 = sink2
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.target),
                _ => None,
            })
            .expect("check result");
        assert_eq!(target2, 12, "无声明时回落 12");
    }

    /// 验收 5：resolve_skill 用传入的 actor（修 D7），target 经 find_character 解析（修 D8）。
    #[test]
    fn resolve_skill_uses_passed_actor_and_resolves_target_by_any_id() {
        let sb = json!({
            "skills": [{
                "id": "sk-hit", "name": "敲击",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "4", "resource": "hp" }] }
            }]
        });
        let (session, sink) = session_with_state(sb, state_with_ally());
        let skill = session.rules.skill("sk-hit").cloned().unwrap();
        let actor = || ActorRef { id: "char-b".into(), name: "卢克".into() };
        // 模板 id / 角色名 / instance_id 三种寻址都要命中。
        for target in ["卢克", "char-b", "inst-char-b"] {
            session.resolve_skill(None, &skill, Some(target), Some(actor()));
        }
        let events = sink.0.lock().unwrap().clone();
        let checks: Vec<&CheckResultPayload> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(checks.len(), 3);
        assert!(
            checks.iter().all(|c| c.actor.id == "char-b"),
            "结算主体取传入的 actor，而不是恒取受控角色"
        );
        let ok: Vec<&ResolutionPayload> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.status == ResolutionStatus::Ok => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(ok.len(), 3);
        assert!(
            ok.iter().all(|r| r
                .state_changes
                .iter()
                .any(|d| d.entity_id == "char-b" && d.field == "resources.hp")),
            "target 必须解析到实例键：{:?}",
            ok.iter().map(|r| r.state_changes.clone()).collect::<Vec<_>>()
        );
        assert_eq!(
            session.projection().characters["char-b"]["resources"]["hp"].as_f64(),
            Some(0.0),
            "三次 4 点伤害打在卢克身上"
        );
    }

    /// 验收 7：判定骰序逐位不变 —— 徒手攻击 = 先 1d20 后 1d6（命令日志重放依赖该序列）。
    #[tokio::test]
    async fn strike_consumes_hit_die_then_damage_die_in_order() {
        let (session, sink) = session_with_state(json!({ "world": {} }), state_with_encounter(1));
        session
            .strike_enemy("e1".into(), AttackChoice::default(), None)
            .await;
        let consumed: Vec<u64> = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => {
                    serde_json::from_str(&p.text).ok()
                }
                _ => None,
            })
            .expect("rng_consume");
        let mut reference = DeterministicRng::new(42);
        let _ = reference.range_inclusive(1, 20);
        let dmg = reference.range_inclusive(1, 6);
        assert_eq!(consumed, reference.consumed, "骰序必须是 1d20 → 1d6");
        assert_eq!(enemy_hp(&session, "enc-1", 0), 20 - dmg, "伤害取自第二次掷骰");
    }

    /// 主线 AI 输出重复 intent_id：只结算一次；缺省 id 保持旧行为（都结算）。
    struct DuplicateIntentAi;

    #[async_trait::async_trait]
    impl AiProvider for DuplicateIntentAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            let quest = |id: Option<&str>, text: &str| IntentEnvelope {
                intent_id: id.map(str::to_string),
                intent: Intent::Quest { text: text.into(), hidden: false, primary: false },
            };
            Ok(AiOutput {
                intents: vec![
                    quest(Some("q-1"), "任务A"),
                    quest(Some("q-1"), "任务B"),
                    quest(None, "任务C"),
                    quest(None, "任务D"),
                ],
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    #[tokio::test]
    async fn duplicate_intent_id_is_applied_once() {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(Arc::new(DuplicateIntentAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "开始".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let quests = session.projection().quests;
        let texts: Vec<&str> = quests.iter().map(|q| q.text.as_str()).collect();
        assert!(texts.contains(&"任务A"));
        assert!(!texts.contains(&"任务B"), "同 intent_id 的第二次必须被跳过");
        assert!(texts.contains(&"任务C") && texts.contains(&"任务D"), "缺省 id 不参与去重");
        assert_eq!(quests.len(), 3);
    }

    /// #04 Query：query_world 对权威状态作答，并进入 canon 供后续回合回喂模型。
    #[tokio::test]
    async fn query_world_answers_from_state_and_feeds_canon() {
        let (session, sink) = session_with(json!({ "world": {} }));
        session
            .handle_intent(Intent::QueryWorld { query: "当前场景有谁".into() }, None)
            .await;
        let events = sink.0.lock().unwrap().clone();
        let answer = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("query_world") => {
                    p.narrative.clone()
                }
                _ => None,
            })
            .expect("query answer event");
        assert!(answer.contains("米拉"), "答案须来自权威状态：{answer}");
        assert!(answer.contains("场景"), "应包含当前场景：{answer}");
        assert!(
            session.canon_lines(8).iter().any(|l| l.contains("米拉")),
            "查询结果必须进入 canon，才能被后续回合的 AI 读到"
        );
    }

    // ---------- #04 ⑦ 回合内续轮（最多 3 轮工具调用） ----------

    /// 构造一个用指定 AI provider 的会话（续轮测试用）。
    fn session_with_provider(ai: Arc<dyn AiProvider>, sb: Value) -> (Arc<Session>, Arc<CaptureSink>) {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Session::new(
            "s".into(),
            state_with_pc(),
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(ai),
            true,
            sb,
        );
        (Arc::new(session), sink)
    }

    /// 同上，但用指定的初始世界状态（预置冷却 / 资源等）。
    fn session_with_provider_and_state(
        ai: Arc<dyn AiProvider>,
        sb: Value,
        state: WorldState,
    ) -> (Arc<Session>, Arc<CaptureSink>) {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Session::new(
            "s".into(),
            state,
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(ai),
            true,
            sb,
        );
        (Arc::new(session), sink)
    }

    /// 第一轮查询世界，第二轮据查询结果续写并 finish_turn。
    struct QueryThenFinishAi {
        story_calls: StdMutex<usize>,
        contexts: StdMutex<Vec<TurnContext>>,
    }

    #[async_trait::async_trait]
    impl AiProvider for QueryThenFinishAi {
        async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            let call = {
                let mut n = self.story_calls.lock().unwrap();
                *n += 1;
                *n
            };
            self.contexts.lock().unwrap().push(ctx.clone());
            if call == 1 {
                Ok(AiOutput::from_intents(vec![
                    Intent::Narrate { content: "第一轮旁白。".into(), actor_id: None },
                    Intent::QueryWorld { query: "当前场景有谁".into() },
                ]))
            } else {
                Ok(AiOutput::from_intents(vec![
                    Intent::Narrate { content: "第二轮据查询结果续写。".into(), actor_id: None },
                    Intent::FinishTurn,
                ]))
            }
        }
        
    }

    #[tokio::test]
    async fn story_loop_feeds_query_result_and_applies_both_rounds() {
        let ai = Arc::new(QueryThenFinishAi {
            story_calls: StdMutex::new(0),
            contexts: StdMutex::new(vec![]),
        });
        let (session, sink) =
            session_with_provider(ai.clone() as Arc<dyn AiProvider>, json!({ "world": {} }));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "看看周围".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        assert_eq!(*ai.story_calls.lock().unwrap(), 2, "查询后必须追加一轮");
        let contexts = ai.contexts.lock().unwrap();
        assert!(contexts[0].turn_feedback.is_empty(), "首轮不携带工具结果");
        assert!(
            contexts[1].turn_feedback.iter().any(|f| f.contains("米拉")),
            "续轮必须带上本轮查询结果：{:?}",
            contexts[1].turn_feedback
        );
        drop(contexts);
        let narrations: Vec<String> = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Narrate(p) => Some(p.content.clone()),
                _ => None,
            })
            .collect();
        assert!(narrations.iter().any(|t| t == "第一轮旁白。"), "首轮效果要落地");
        assert!(
            narrations.iter().any(|t| t == "第二轮据查询结果续写。"),
            "续轮效果也要落地：{narrations:?}"
        );
    }

    /// 驳回回喂（#01 冷却）：技能在冷却中被引擎驳回时，模型必须在**同一回合**拿到原因。
    /// 否则模型看不到叙事里不存在的驳回，会把「技能没生效」照样写成成功。
    struct CooldownRetryAi {
        story_calls: StdMutex<u32>,
        contexts: StdMutex<Vec<TurnContext>>,
    }

    #[async_trait::async_trait]
    impl AiProvider for CooldownRetryAi {
        async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            let call = {
                let mut n = self.story_calls.lock().unwrap();
                *n += 1;
                *n
            };
            self.contexts.lock().unwrap().push(ctx.clone());
            if call == 1 {
                Ok(AiOutput::from_intents(vec![Intent::UseSkill {
                    skill_id: "sk-blast".into(),
                    target_id: None,
                }]))
            } else {
                Ok(AiOutput::from_intents(vec![
                    Intent::Narrate { content: "冷却没好，改用别的招。".into(), actor_id: None },
                    Intent::FinishTurn,
                ]))
            }
        }
    }

    #[tokio::test]
    async fn cooldown_rejection_is_fed_back_to_the_model_in_round() {
        let ai = Arc::new(CooldownRetryAi {
            story_calls: StdMutex::new(0),
            contexts: StdMutex::new(vec![]),
        });
        let sb = json!({
            "skills": [{ "id": "sk-blast", "name": "爆裂", "cooldown": { "turns": 3 }, "check": { "dice": "1d20" } }],
            "world": { "check": { "dice": "1d20" } }
        });
        let mut st = state_with_pc();
        // 预先埋好冷却起点（上一回合用过），本回合再用必须被拦。
        st.cooldowns.insert(
            "char-a".into(),
            [("sk-blast".to_string(), 1u32)].into_iter().collect(),
        );
        let (session, sink) = session_with_provider_and_state(ai.clone() as Arc<dyn AiProvider>, sb, st);
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "再放一次爆裂".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        assert_eq!(*ai.story_calls.lock().unwrap(), 2, "被驳回后必须追加一轮让模型改主意");
        let contexts = ai.contexts.lock().unwrap();
        assert!(contexts[0].turn_feedback.is_empty(), "首轮不携带任何反馈");
        let feedback = contexts[1].turn_feedback.join(" | ");
        assert!(
            feedback.contains("cooldown_active") && feedback.contains("冷却"),
            "续轮必须把冷却驳回原因回喂给模型：{feedback}"
        );
        drop(contexts);
        // 驳回照样落一条 rejected 事件（可审计），只是不进叙事。
        let rejected = sink.0.lock().unwrap().iter().any(|e| matches!(
            &e.event,
            PlayEvent::Resolution(p) if p.rejection_code.as_deref() == Some("cooldown_active")
        ));
        assert!(rejected, "冷却驳回必须留在命令日志里");
    }

    /// 永远只查询、从不 finish_turn：必须在轮次上限处停下。
    struct QueryForeverAi {
        story_calls: StdMutex<usize>,
    }

    #[async_trait::async_trait]
    impl AiProvider for QueryForeverAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            *self.story_calls.lock().unwrap() += 1;
            Ok(AiOutput::from_intents(vec![Intent::QueryWorld { query: "场景".into() }]))
        }
        
    }

    #[tokio::test]
    async fn story_loop_stops_at_round_cap() {
        let ai = Arc::new(QueryForeverAi { story_calls: StdMutex::new(0) });
        let (session, sink) =
            session_with_provider(ai.clone() as Arc<dyn AiProvider>, json!({ "world": {} }));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "看看".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        assert_eq!(*ai.story_calls.lock().unwrap(), MAX_STORY_AI_ROUNDS, "查询到上限必须停下");
        let queries = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(&e.event, PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("query_world")))
            .count();
        assert_eq!(queries, MAX_STORY_AI_ROUNDS, "每轮查询都已结算");
    }

    /// finish_turn 是循环终止信号：不产生任何事件，与空意图列表逐事件同构。
    struct FinishTurnOnlyAi;
    struct EmptyAi;

    #[async_trait::async_trait]
    impl AiProvider for FinishTurnOnlyAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::from_intents(vec![Intent::FinishTurn]))
        }
        
    }

    #[async_trait::async_trait]
    impl AiProvider for EmptyAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::default())
        }
        
    }

    #[tokio::test]
    async fn finish_turn_emits_no_event_and_terminates_loop() {
        let (s_ft, sink_ft) =
            session_with_provider(Arc::new(FinishTurnOnlyAi), json!({ "world": {} }));
        s_ft.run_round(
            RoundInput { channel: RoundChannel::Character, text: "开始".into(), refs: vec![] },
            None,
            vec![],
        )
        .await
        .unwrap();
        let (s_empty, sink_empty) =
            session_with_provider(Arc::new(EmptyAi), json!({ "world": {} }));
        s_empty
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "开始".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let kinds = |sink: &Arc<CaptureSink>| -> Vec<std::mem::Discriminant<PlayEvent>> {
            sink.0
                .lock()
                .unwrap()
                .iter()
                .map(|e| std::mem::discriminant(&e.event))
                .collect()
        };
        assert_eq!(
            kinds(&sink_ft),
            kinds(&sink_empty),
            "finish_turn 不得产生任何事件，必须与空意图列表逐事件同构"
        );
    }

    /// 立即 finish 的模型只调用一次主线 AI，行为与单轮路径一致。
    struct NarrateThenFinishAi {
        story_calls: StdMutex<usize>,
    }

    #[async_trait::async_trait]
    impl AiProvider for NarrateThenFinishAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            *self.story_calls.lock().unwrap() += 1;
            Ok(AiOutput::from_intents(vec![
                Intent::Narrate { content: "一轮讲完。".into(), actor_id: None },
                Intent::FinishTurn,
            ]))
        }
        
    }

    #[tokio::test]
    async fn immediate_finish_keeps_single_story_call() {
        let ai = Arc::new(NarrateThenFinishAi { story_calls: StdMutex::new(0) });
        let (session, sink) =
            session_with_provider(ai.clone() as Arc<dyn AiProvider>, json!({ "world": {} }));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "继续".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(*ai.story_calls.lock().unwrap(), 1, "没有新信息不应续轮");
        assert!(sink.0.lock().unwrap().iter().any(|e| matches!(
            &e.event, PlayEvent::Narrate(p) if p.content == "一轮讲完。"
        )));
    }

    /// 多轮回合的权威日志重放必须得到同一世界状态。
    #[tokio::test]
    async fn replay_of_multi_round_turn_is_identical() {
        let ai = Arc::new(QueryThenFinishAi {
            story_calls: StdMutex::new(0),
            contexts: StdMutex::new(vec![]),
        });
        let (session, _sink) =
            session_with_provider(ai as Arc<dyn AiProvider>, json!({ "world": {} }));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "看看周围".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let persisted: Vec<PersistedEvent> = session
            .event_log
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(|envelope| PersistedEvent { request_id: None, envelope })
            .collect();
        assert!(persisted.len() > 1, "多轮回合应产生多条权威命令");

        let (restarted, _sink2) =
            session_with_provider(Arc::new(EmptyAi), json!({ "world": {} }));
        restarted.replay(&persisted);
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "多轮日志重放必须得到同一世界状态"
        );
        assert_eq!(session.current_seq(), restarted.current_seq());
    }

    /// #04 interact：命中故事书 objects 定义即结算；未知物件/动作驳回；无 objects 优雅降级。
    #[tokio::test]
    async fn interact_resolves_object_and_degrades_without_objects() {
        let sb = json!({ "objects": [ {
            "id": "door-1", "name": "石门", "description": "厚重的石门",
            "actions": [ { "key": "open", "label": "推开" } ]
        } ] });
        let (session, sink) = session_with(sb);
        session
            .handle_intent(Intent::Interact { object_id: "door-1".into(), action: "open".into() }, None)
            .await;
        let events = sink.0.lock().unwrap().clone();
        let ok = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("interact") => Some(p.clone()),
                _ => None,
            })
            .expect("interact resolution");
        assert_eq!(ok.status, ResolutionStatus::Ok);
        let narrative = ok.narrative.unwrap();
        assert!(narrative.contains("石门") && narrative.contains("推开"), "{narrative}");

        // 物件不支持的动作 → target_invalid。
        session
            .handle_intent(Intent::Interact { object_id: "door-1".into(), action: "read".into() }, None)
            .await;
        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(
            &e.event,
            PlayEvent::Resolution(p) if p.rejection_code.as_deref() == Some("target_invalid")
        )));

        // 找不到物件 → target_invalid。
        session
            .handle_intent(Intent::Interact { object_id: "nope".into(), action: "open".into() }, None)
            .await;
        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().filter(|e| matches!(
            &e.event,
            PlayEvent::Resolution(p) if p.rejection_code.as_deref() == Some("target_invalid")
        )).count() >= 2);

        // 故事书完全没声明 objects → 优雅降级（Ok + 明确说明），不报错。
        let (session2, sink2) = session_with(json!({ "world": {} }));
        session2
            .handle_intent(Intent::Interact { object_id: "door-1".into(), action: "open".into() }, None)
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        let degraded = events2
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p) => Some(p.clone()),
                _ => None,
            })
            .expect("degrade resolution");
        assert_eq!(degraded.status, ResolutionStatus::Ok);
        assert!(degraded.narrative.unwrap().contains("未声明"));
    }

    /// #04 ④：显式 actor_id 指向不在场/不存在者必须驳回，绝不静默改判；
    /// 在场合法的 actor_id 正常归属。
    struct ActorScopeAi;

    #[async_trait::async_trait]
    impl AiProvider for ActorScopeAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::from_intents(vec![
                Intent::Speak { content: "我来也".into(), tone: None, actor_id: Some("char-ghost".into()) },
                Intent::Speak { content: "你好呀".into(), tone: None, actor_id: Some("char-lucy".into()) },
            ]))
        }
        
    }

    #[tokio::test]
    async fn explicit_actor_id_out_of_scope_is_rejected_not_reassigned() {
        let mut state = state_with_pc();
        let mut chars = state.characters.clone();
        chars.insert("char-lucy".into(), character_instance("char-lucy", "露西", "npc"));
        let mut ghost = character_instance("char-ghost", "幽灵", "npc");
        ghost.present = false;
        chars.insert("char-ghost".into(), ghost);
        state.characters = chars;

        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state,
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(ActorScopeAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "打招呼".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let events = sink.0.lock().unwrap().clone();
        assert!(
            events.iter().any(|e| matches!(
                &e.event,
                PlayEvent::Resolution(p) if p.rejection_code.as_deref() == Some("actor_not_found")
            )),
            "不在场的 actor_id 必须驳回并落日志"
        );
        let dialogues: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Dialogue(p) => Some((e.actor.clone(), p.content.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(dialogues.len(), 1, "只有合法的在场 actor 台词落事件");
        assert_eq!(dialogues[0].1, "你好呀");
        assert_eq!(dialogues[0].0.as_ref().map(|a| a.id.as_str()), Some("char-lucy"));
    }

    /// #04 ④：单一 AI 不能替玩家受控角色（PC）说台词。
    struct PcPuppetAi;

    #[async_trait::async_trait]
    impl AiProvider for PcPuppetAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::from_intents(vec![Intent::Speak {
                content: "我替玩家说话".into(),
                tone: None,
                actor_id: Some("char-a".into()),
            }]))
        }
    }

    #[tokio::test]
    async fn single_ai_cannot_speak_as_the_controlled_pc() {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(PcPuppetAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "你好".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(
            &e.event,
            PlayEvent::Resolution(p) if p.rejection_code.as_deref() == Some("actor_not_controlled")
        )));
        assert!(
            !events.iter().any(|e| matches!(&e.event, PlayEvent::Dialogue(_))),
            "不得以受控 PC 身份落到对话事件"
        );
    }

    /// PC 的 emote 是正常旁白（描写玩家动作 / 环境），不得被驳回。
    /// 早先「PC 一律禁止说演」过严：会把这类叙述整条丢掉并弹驳回卡片。
    struct PcEmoteAi;

    #[async_trait::async_trait]
    impl AiProvider for PcEmoteAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::from_intents(vec![Intent::Emote {
                content: "你握紧了武器。".into(),
                emotion: None,
                actor_id: Some("char-a".into()),
            }]))
        }
    }

    #[tokio::test]
    async fn controlled_pc_emote_is_accepted_as_narration() {
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(PcEmoteAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "我看看".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let events = sink.0.lock().unwrap().clone();
        assert!(
            !events.iter().any(|e| matches!(
                &e.event,
                PlayEvent::Resolution(p) if p.rejection_code.is_some()
            )),
            "PC 的 emote 不应被驳回"
        );
        let emote = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Emote(p) => Some((e.actor.clone(), p.content.clone())),
                _ => None,
            })
            .expect("emote emitted");
        assert_eq!(emote.1, "你握紧了武器。");
        assert_eq!(emote.0.as_ref().map(|a| a.id.as_str()), Some("char-a"));
    }

    /// 提示词把在场角色渲染成 `名字(id)`，模型常整串回填 actor_id。
    /// 引擎应据括号内的 id 归属到本人，而不是因为原串匹配不到就驳回。
    struct PaddedActorIdAi;

    #[async_trait::async_trait]
    impl AiProvider for PaddedActorIdAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::from_intents(vec![Intent::Speak {
                content: "你好呀".into(),
                tone: None,
                actor_id: Some("露西(char-lucy)".into()),
            }]))
        }
    }

    #[tokio::test]
    async fn actor_id_in_name_paren_form_is_resolved_not_rejected() {
        let mut state = state_with_pc();
        let mut chars = state.characters.clone();
        chars.insert("char-lucy".into(), character_instance("char-lucy", "露西", "npc"));
        state.characters = chars;

        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state,
            sink.clone() as Arc<dyn EventSink>,
            ai_slot(Arc::new(PaddedActorIdAi)),
            true,
            json!({ "world": {} }),
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "打招呼".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();

        let events = sink.0.lock().unwrap().clone();
        assert!(
            !events.iter().any(|e| matches!(
                &e.event,
                PlayEvent::Resolution(p) if p.rejection_code.is_some()
            )),
            "`名字(id)` 形式的 actor_id 不应被驳回"
        );
        let dialogues: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Dialogue(p) => Some((e.actor.clone(), p.content.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(dialogues.len(), 1, "台词应归属到 Lucy");
        assert_eq!(dialogues[0].1, "你好呀");
        assert_eq!(dialogues[0].0.as_ref().map(|a| a.id.as_str()), Some("char-lucy"));
    }
    /// B/D：type 别名掷骰、attributes 白名单驳回、default_dc 兜底难度。
    #[tokio::test]
    async fn check_uses_type_alias_whitelist_and_default_dc() {
        let sb = || json!({ "world": { "check": {
            "type": "d20",
            "attributes": ["str", "agi"],
            "default_dc": 15
        } } });

        // 白名单外的属性：明确驳回，且不产生 CheckResult。
        let (session, sink) = session_with(sb());
        session
            .handle_intent(
                Intent::Check { attribute: "dexterity".into(), difficulty: None, actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let msg = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::Resolution(p)
                    if p.rejection_code.as_deref() == Some("rule_violation") =>
                {
                    p.narrative.clone()
                }
                _ => None,
            })
            .expect("白名单外的判定属性必须驳回");
        assert!(msg.contains("str") && msg.contains("agi"), "驳回信息要列出可用属性: {msg}");
        assert!(!events.iter().any(|e| matches!(&e.event, PlayEvent::CheckResult(_))));

        // 白名单内 + type 别名：真的掷 1d20，target 用 default_dc。
        let (session2, sink2) = session_with(sb());
        session2
            .handle_intent(
                Intent::Check { attribute: "agi".into(), difficulty: None, actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        let check = events2
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.clone()),
                _ => None,
            })
            .expect("check result");
        assert_eq!(check.expr.as_deref(), Some("1d20"), "type 别名要变成骰式");
        assert!(check.rolls.as_ref().is_some_and(|r| r.len() == 1), "必须真的掷骰");
        assert_eq!(check.target, 15, "difficulty 缺省用 default_dc");
        assert_eq!(check.attribute, "agi");
    }

    /// 存档设置来回写 auto_confirm：同值写入不应落 confirm_toggle（否则改模型也会刷屏）。
    #[tokio::test]
    async fn set_auto_confirm_only_emits_on_change() {
        // session_with 建会话时 auto_confirm = true。
        let (session, sink) = session_with(json!({ "world": {} }));
        session.set_auto_confirm(true);
        assert!(
            !sink.0.lock().unwrap().iter().any(|e| matches!(
                &e.event,
                PlayEvent::System(p) if p.code.as_deref() == Some("confirm_toggle")
            )),
            "同值写入不得落 confirm_toggle"
        );

        session.set_auto_confirm(false);
        let events = sink.0.lock().unwrap().clone();
        let n = events
            .iter()
            .filter(|e| matches!(
                &e.event,
                PlayEvent::System(p) if p.code.as_deref() == Some("confirm_toggle")
            ))
            .count();
        assert_eq!(n, 1, "真正切换才落一条");

        session.set_auto_confirm(false);
        let n2 = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(
                &e.event,
                PlayEvent::System(p) if p.code.as_deref() == Some("confirm_toggle")
            ))
            .count();
        assert_eq!(n2, 1, "再次同值写入仍不落事件");
    }

    /// #12 比较模式：world.check.mode = lte 时，最终值 ≤ 目标值才算成功。
    #[tokio::test]
    async fn check_intent_honors_lte_mode() {
        let sb = || json!({ "world": { "check": { "dice": "1d20", "mode": "lte" } } });
        // 目标 30：1d20 + 修正 恒 ≤ 30 → 必成功。
        let (session, sink) = session_with(sb());
        session
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(30), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.clone()),
                _ => None,
            })
            .expect("check result");
        assert!(check.result, "lte 模式 total<=target 必成功: total={}", check.total);

        // 目标 -100：1d20 + 修正 恒 > -100 → 必失败。
        let (session2, sink2) = session_with(sb());
        session2
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(-100), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        let check2 = events2
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::CheckResult(p) => Some(p.clone()),
                _ => None,
            })
            .expect("check result 2");
        assert!(!check2.result, "lte 模式 total>target 必失败");
    }

    // ---------- 规则集挂载点：入口 · 判定路径收敛 · 原语落地 ----------

    fn rng_logged(events: &[EventEnvelope]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => {
                    serde_json::from_str::<Vec<u64>>(&p.text).ok()
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn check_result_of(events: &[EventEnvelope]) -> Option<CheckResultPayload> {
        events.iter().find_map(|e| match &e.event {
            PlayEvent::CheckResult(p) => Some(p.clone()),
            _ => None,
        })
    }

    /// 验收 1 / 5 / 7：storybook.lua_mounts 装载生效；Intent::Check 触发挂载点；
    /// 取高 = 掷两次（RNG 消耗 = 2）且确定性可重放。
    #[tokio::test]
    async fn storybook_lua_mounts_drive_intent_check() {
        let sb = || {
            json!({
                "world": { "check": { "dice": "1d20" } },
                "lua_mounts": [
                    { "id": "gate", "mount": "check_pre_roll", "source": "host.modify_check('keep_high')" },
                    { "id": "prof", "mount": "check_post_roll", "source": "host.modify_check('add', 6)" }
                ]
            })
        };
        // 会话种子 42（state_with_pc）：独立探针取同序列前两颗 d20。
        let mut probe = DeterministicRng::new(42);
        let d1 = probe.range_inclusive(1, 20);
        let d2 = probe.range_inclusive(1, 20);

        let (session, sink) = session_with(sb());
        session
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(10), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = check_result_of(&events).expect("check result");
        assert_eq!(check.rolls, Some(vec![d1.max(d2)]), "取高 = 两次取大");
        assert_eq!(check.r#mod, 4 + 6, "属性修正 +4，挂载点后置加值 +6");
        assert_eq!(check.total, d1.max(d2) + 4 + 6);
        assert_eq!(check.margin, check.total - 10);
        assert_eq!(rng_logged(&events), probe.consumed, "两次掷骰的消耗都进了日志");

        // 同种子重放：骰面与消耗逐字一致。
        let (session2, sink2) = session_with(sb());
        session2
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(10), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        let check2 = check_result_of(&events2).expect("check result 2");
        assert_eq!(check2.rolls, check.rolls);
        assert_eq!(check2.total, check.total);
        assert_eq!(check2.r#mod, check.r#mod);
        assert_eq!(check2.margin, check.margin);
        assert_eq!(check2.level, check.level);
        assert_eq!(rng_logged(&events2), rng_logged(&events));
    }

    /// 挂载点 when 条件：世界快照里 flag 未置位 → 不执行；置位 → 执行。
    #[tokio::test]
    async fn lua_mount_when_condition_gates_execution() {
        let sb = json!({
            "world": { "check": { "dice": "1d20" } },
            "lua_mounts": [{
                "id": "gated", "mount": "check_pre_roll",
                "source": "host.modify_check('keep_high')",
                "when": { "op": "flag_set", "flag": "focused" }
            }]
        });
        let (session, sink) = session_with(sb.clone());
        session
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(10), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(rng_logged(&events).len(), 1, "条件不成立 → 只有裸判定掷一颗");

        let mut state = state_with_pc();
        state.flags.insert("focused".to_string(), json!(true));
        let mut probe = DeterministicRng::new(42);
        let d1 = probe.range_inclusive(1, 20);
        let d2 = probe.range_inclusive(1, 20);
        let (session2, sink2) = session_with_state(sb, state);
        session2
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(10), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events2 = sink2.0.lock().unwrap().clone();
        assert_eq!(rng_logged(&events2).len(), 2, "条件成立 → 取高掷两次");
        assert_eq!(check_result_of(&events2).expect("check").rolls, Some(vec![d1.max(d2)]));
    }

    /// 验收 5：Intent::Strike 也触发挂载点（攻击不再绕过规则集）。
    #[tokio::test]
    async fn strike_runs_storybook_lua_mounts() {
        let sb = json!({
            "lua_mounts": [{
                "id": "prof", "mount": "check_pre_roll",
                "source": "host.modify_check('add', 50)"
            }],
            "skills": [{
                "id": "sk-atk", "name": "挥砍",
                "check": { "dice": "1d20" },
                "effect": { "immediate": [{ "kind": "damage", "amount": "1", "resource": "hp" }] }
            }]
        });
        let (session, sink) = session_with_state(sb, state_with_encounter(15));
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-atk".into()), ..Default::default() },
                None,
            )
            .await;
        let narrative = strike_narrative(&sink);
        assert!(narrative.contains("修正 54"), "攻击必须跑规则集挂载点：{narrative}");
        assert_eq!(enemy_hp(&session, "enc-1", 0), 19);
    }

    // ---------- 判定 C5：攻击判定也在 UI 可见 ----------

    /// 判定 C5 验收 1：strike 也发 CheckResultPayload——骰面 / 修正 / 总值 / 难度齐备，
    /// 前端 CheckCard 直接复用；事件顺序与技能判定一致（判定卡在叙事之前）。
    #[tokio::test]
    async fn strike_emits_check_result_for_the_ui() {
        let sb = json!({
            "skills": [{
                "id": "sk-atk", "name": "挥砍",
                "check": { "dice": "1d20", "attribute_modifier": { "str": 0 } },
                "effect": { "immediate": [{ "kind": "damage", "amount": "1", "resource": "hp" }] }
            }]
        });
        let (session, sink) = session_with_state(sb, state_with_encounter(15));
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-atk".into()), ..Default::default() },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = check_result_of(&events).expect("strike 必须发 CheckResult（UI 看得到骰面）");
        assert_eq!(check.actor, ActorRef { id: "char-a".into(), name: "米拉".into() });
        assert_eq!(check.expr.as_deref(), Some("1d20"));
        assert_eq!(check.rolls.as_ref().map(Vec::len), Some(1), "骰面必须外露");
        assert_eq!(check.target, 15, "难度 = 目标 AC");
        assert_eq!(check.total, check.rolls.as_ref().unwrap()[0] + check.r#mod);
        assert_eq!(check.margin, check.total - check.target);
        assert_eq!(check.result, check.total >= check.target);
        assert_eq!(check.level, level_for_margin(check.margin, &DEFAULT_DEGREE_THRESHOLDS));
        assert_eq!(check.kind, Some(CheckKind::Attack));
        let seq: Vec<&str> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::CheckResult(_) => Some("check"),
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("strike") => {
                    Some("resolution")
                }
                _ => None,
            })
            .collect();
        assert_eq!(seq, vec!["check", "resolution"], "先判定卡、后叙事");
    }

    /// 判定 C4 验收 2：check_post_roll 钩子读得到完整判定细节
    /// （rolls / total / target / margin / level / result / attribute / kind）。
    ///
    /// 脚本把这七项逐个对账：全对才加 100——任何一项读不到就加不上，测试即失败。
    #[tokio::test]
    async fn post_roll_mount_reads_full_check_details() {
        let sb = json!({
            "world": { "check": { "dice": "1d20", "attribute_modifier": { "str": 0 } } },
            "lua_mounts": [{
                "id": "probe", "mount": "check_post_roll",
                "source": "local c = host.check\nif c ~= nil and c.rolls ~= nil and #c.rolls == 1\n   and c.attribute == 'str' and c.kind == 'attribute'\n   and c.total == c.rolls[1] and c.target == 12\n   and c.margin == c.total - c.target\n   and c.result == (c.margin >= 0)\n   and c.level == (c.margin >= 10 and 'great' or (c.margin >= 0 and 'success' or (c.margin >= -10 and 'barely' or 'fail')))\n   and host.check_total == c.total and host.check_target == c.target\n   and host.check_margin == c.margin and host.check_attribute == 'str'\n   and host.check_kind == 'attribute' and host.check_result == c.result\n   and host.check_level == c.level\nthen host.modify_check('add', 100) end"
            }]
        });
        let (session, sink) = session_with(sb);
        session
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(12), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = check_result_of(&events).expect("check result");
        assert_eq!(check.r#mod, 100, "脚本读到完整判定细节并据此加值");
        assert_eq!(check.target, 12);
        assert_eq!(check.total, check.rolls.as_ref().unwrap()[0] + 100);
    }

    /// 判定 C4 验收 3：ModifyCheck 覆盖判定结果——强制失败把大成功压到最低档，
    /// 强制成功把必失败抬到中档；骰面 / 总值 / 差值都不动。
    #[tokio::test]
    async fn modify_check_force_fail_and_success_change_levels() {
        let sb = |force: &str| {
            json!({
                "world": { "check": { "dice": "1d20", "attribute_modifier": { "str": 100 } } },
                "lua_mounts": [{ "id": "override", "mount": "check_post_roll",
                    "source": format!("host.modify_check('{force}')") }]
            })
        };
        // 强制失败：总值 ≥ 112、差值 ≥ 100（本来必是大成功）→ 改判失败，档位落到 Fail。
        let (session, sink) = session_with(sb("force_fail"));
        session
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(12), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let forced = check_result_of(&sink.0.lock().unwrap().clone()).expect("check result");
        assert!(!forced.result, "强制失败必须改判");
        assert_eq!(forced.level, SuccessLevel::Fail, "档位随之落到最低档");
        assert_eq!(forced.rolls.as_ref().map(Vec::len), Some(1), "骰面不动");
        assert_eq!(forced.total, forced.rolls.as_ref().unwrap()[0] + 100, "总值不动");
        assert!(forced.margin >= 100, "差值不动（覆盖结果不是重写骰子）");

        // 强制成功：负修正构造必失败 → 改判成功，档位至少中档。
        let sb2 = json!({
            "world": { "check": { "dice": "1d20", "attribute_modifier": { "str": -100 } } },
            "lua_mounts": [{ "id": "override", "mount": "check_post_roll",
                "source": "host.modify_check('force_success')" }]
        });
        let (session2, sink2) = session_with(sb2);
        session2
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(12), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let forced2 = check_result_of(&sink2.0.lock().unwrap().clone()).expect("check result 2");
        assert!(forced2.result, "强制成功必须改判");
        assert_eq!(forced2.level, SuccessLevel::Success, "最低档抬到中档");
        assert!(forced2.margin < -10, "差值仍是骰子算出来的负值");

        // 回归：不声明 override 的故事书，同样的修正下结果 / 档位与改动前一致。
        let sb3 = json!({ "world": { "check": { "dice": "1d20", "attribute_modifier": { "str": 100 } } } });
        let (session3, sink3) = session_with(sb3);
        session3
            .handle_intent(
                Intent::Check { attribute: "str".into(), difficulty: Some(12), actor_id: None, opponent_id: None },
                None,
            )
            .await;
        let plain = check_result_of(&sink3.0.lock().unwrap().clone()).expect("check result 3");
        assert!(plain.result);
        assert_eq!(plain.level, SuccessLevel::Great);
    }

    /// 验收 3 / 4：apply_effect / modify_resource 写请求真正落状态（可指定目标、可正可负）。
    #[test]
    fn lua_requests_apply_effects_and_target_resources() {
        let (session, sink) = session_with(json!({}));
        session.apply_lua_requests(
            &[
                LuaRequest::ApplyEffect {
                    target: "char-a".into(),
                    effect: json!({ "kind": "heal", "amount": "5" }),
                },
                // 名字寻址：与既有 ApplyStatus 同口径（任意 id 形态）。
                LuaRequest::ModifyResource {
                    target: "米拉".into(),
                    resource: "mana".into(),
                    amount: -3,
                },
                LuaRequest::ModifyResource {
                    target: "char-a".into(),
                    resource: "mana".into(),
                    amount: 7,
                },
                // 非判定时机抛出的判定修正无处可用，忽略（不崩）。
                LuaRequest::ModifyCheck { mode: crate::lua_host::CheckModifier::Add, amount: 3 },
            ],
            "char-a",
        );
        let proj = session.projection();
        assert_eq!(proj.characters["char-a"]["resources"]["hp"].as_f64(), Some(35.0));
        assert_eq!(proj.characters["char-a"]["resources"]["mana"].as_f64(), Some(24.0));
        assert!(sink
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(&e.event, PlayEvent::StateUpdate(_))));
    }

    /// apply_effect 的数量支持骰式：走既有 resolve_effect 路径，骰子消耗记进日志。
    #[test]
    fn lua_apply_effect_with_dice_uses_engine_rng() {
        let (session, sink) = session_with(json!({}));
        session.apply_lua_requests(
            &[LuaRequest::ApplyEffect {
                target: "char-a".into(),
                effect: json!({ "kind": "damage", "amount": "3d6", "resource": "hp" }),
            }],
            "char-a",
        );
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(rng_logged(&events).len(), 3, "3d6 消耗三颗骰且写进日志");
        let hp = session.projection().characters["char-a"]["resources"]["hp"]
            .as_f64()
            .unwrap();
        assert!((12.0..=27.0).contains(&hp), "hp = 30 - 3d6：{hp}");
    }

    /// 非法效果形状只 warn，不 panic 也不中断其余请求。
    #[test]
    fn lua_apply_effect_invalid_shape_is_ignored_with_warning() {
        let (session, sink) = session_with(json!({}));
        session.apply_lua_requests(
            &[
                LuaRequest::ApplyEffect {
                    target: "char-a".into(),
                    effect: json!({ "kind": "bogus" }),
                },
                LuaRequest::ModifyResource {
                    target: "char-a".into(),
                    resource: "mana".into(),
                    amount: 1,
                },
            ],
            "char-a",
        );
        let events = sink.0.lock().unwrap().clone();
        assert!(events.iter().any(|e| matches!(
            &e.event,
            PlayEvent::System(p) if p.code.as_deref() == Some("lua_effect_invalid")
        )));
        assert_eq!(
            session.projection().characters["char-a"]["resources"]["mana"].as_f64(),
            Some(21.0)
        );
    }

    /// 注册表锁不得跨 apply_lua_requests 持有：挂载点触发的事件会回到 dispatch_lua_event
    /// 再取同一把锁，持锁即死锁（用超时线程把它变成失败而不是挂起）。
    #[test]
    fn skill_event_request_does_not_deadlock_registry_lock() {
        let sb = json!({
            "skills": [{ "id": "sk-shout", "name": "呐喊", "lua": "host.trigger_event('scene')" }]
        });
        let (session, _sink) = session_with(sb);
        session.register_lua_hook(
            "evt",
            LuaMount::Event,
            "host.modify_resource('char-a', 'mana', 1)",
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let skill = session.rules.skill("sk-shout").cloned().unwrap();
            session.resolve_skill(None, &skill, None, None);
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok(),
            "注册表锁被跨 apply_lua_requests 持有 → 事件回派死锁"
        );
    }

    // ============================================================
    // 图鉴 M2：怪物实例 / 克隆 / 对称攻击内核 / 派生 AC / 门控
    // ============================================================

    /// 一份最小图鉴故事书（LMoP 的形状）：
    /// - 派生 AC = 10 + dex_mod，挂接定义 monster-armor 再给 ac +3（地精 dex 14 → 15）；
    /// - 生命资源叫 res-hp（不是 hp），攻击资源也写 res-hp；
    /// - 图鉴条目带 background（若不做 kind 门控，它会漏进人格档案）。
    fn bestiary_storybook() -> Value {
        serde_json::json!({
            "attribute_dimensions": [
                { "key": "str", "baseline": 10, "modifier_step": 2, "min": 1, "max": 30 },
                { "key": "dex", "baseline": 10, "modifier_step": 2, "min": 1, "max": 30 }
            ],
            "derived": [
                { "key": "dex_mod", "formula": "floor((dex - 10) / 2)" },
                { "key": "ac", "formula": "10 + dex_mod" }
            ],
            "definitions": [
                { "id": "armor-goblin", "modifiers": [ { "target": "ac", "value": 3, "op": "add" } ] }
            ],
            "characters": [{
                "id": "mon-goblin", "name": "地精", "kind": "monster",
                "background": "地精是小个子、心肠黑、自私的类人生物。",
                "attributes": { "str": 8, "dex": 14 },
                "resources": { "res-hp": 7 },
                "skills": ["sk-scimitar", "sk-bow"],
                "attachments": { "monster-armor": ["armor-goblin"] }
            }],
            "skills": [
                { "id": "sk-scimitar", "name": "弯刀", "attribute": "str",
                  "check": { "dice": "1d20", "kind": "attack" },
                  "effect": { "immediate": [
                      { "kind": "damage", "amount": "1d6+2", "resource": "res-hp" }
                  ] } },
                { "id": "sk-bow", "name": "短弓", "attribute": "dex",
                  "check": { "dice": "1d20", "kind": "attack" },
                  "effect": { "immediate": [
                      { "kind": "damage", "amount": "1d6+2", "resource": "res-hp" }
                  ] } },
                { "id": "sk-hit", "name": "劈砍", "attribute": "str",
                  "check": { "lua": "return { total = 100, margin = 100 }" },
                  "effect": { "immediate": [
                      { "kind": "damage", "amount": "3", "resource": "hp" }
                  ] } }
            ],
            "skeleton": [{ "scenes": [
                { "id": "sc-1", "title": "洞穴", "location_id": "loc-cave" }
            ] }],
            "world": { "locations": [ { "id": "loc-cave", "name": "洞穴" } ] }
        })
    }


    // ---------- 时序与行动经济（#GAP-I） ----------

    /// 未声明 world.turn → 时序整体关闭：不排先攻、闸门恒放行（旧行为逐字不变）。
    #[tokio::test]
    async fn turn_layer_is_inert_without_world_turn_declaration() {
        let (session, _sink) = session_with(json!({ "world": {} }));
        let _enc = create_encounter(&session, vec![temp_spec("灰狼", 11, 12)]).await;
        assert!(
            session.state.lock().unwrap().turn.order.is_empty(),
            "未声明 world.turn 就不该排先攻"
        );
        let check = Intent::Check { attribute: "str".into(), difficulty: None, actor_id: None, opponent_id: None };
        assert!(session.turn_gate(&check, None).is_ok(), "时序关闭时任何机械意图都放行");
    }

    /// 声明 world.turn + 建遭遇 → 排定先攻顺序与全员预算，且**随命令日志重放**。
    #[tokio::test]
    async fn turn_order_is_rolled_on_encounter_and_survives_replay() {
        let sb = json!({
            "characters": [{
                "id": "mon-goblin", "name": "地精", "kind": "monster",
                "attributes": { "dex": 14 }, "resources": { "hp": 7 }
            }],
            "world": {
                "check": { "dice": "1d20" },
                "turn": {
                    "order": "initiative",
                    "initiative": { "dice": "1d20", "attribute": "dex" },
                    "budgets": [{ "id": "action", "amount": 1 }],
                    "intent_budget": { "check": "action" }
                }
            }
        });
        let (session, _sink) = session_with_state(sb.clone(), state_with_pc());
        let _enc = create_encounter(&session, vec![goblin_spec(2)]).await;
        let (order, round, budgets) = {
            let st = session.state.lock().unwrap();
            (st.turn.order.clone(), st.turn.round, st.turn.budgets.clone())
        };
        assert_eq!(round, 1, "战斗开始即第 1 轮");
        assert_eq!(order.len(), 3, "受控角色 + 2 只地精都该排进顺序：{order:?}");
        assert_eq!(
            order.iter().filter(|k| k.contains("mon-goblin")).count(),
            2,
            "两只地精实例都要在序里：{order:?}"
        );
        assert!(order.iter().all(|k| budgets.get(k).and_then(|m| m.get("action")).copied() == Some(1)),
            "全员预算都该按声明建好：{budgets:?}");

        let persisted = persisted_of(&session);
        let (restarted, _s2) = session_with_state(sb, state_with_pc());
        restarted.replay(&persisted);
        assert_eq!(restarted.state.lock().unwrap().turn.order, order, "先攻顺序必须随日志重放");
        assert_eq!(restarted.state.lock().unwrap().turn.round, round);
    }

    /// 固定先攻（fixed）：怪物先手，受控角色发机械意图被 not_your_turn 驳回。
    #[tokio::test]
    async fn fixed_initiative_rejects_actor_whose_turn_it_is_not() {
        let sb = json!({
            "characters": [{
                "id": "mon-goblin", "name": "地精", "kind": "monster",
                "attributes": { "dex": 14 }, "resources": { "hp": 7 }
            }],
            "world": {
                "check": { "dice": "1d20" },
                "turn": {
                    "order": "fixed",
                    "initiative": { "fixed": { "mon-goblin": 100 } },
                    "budgets": [{ "id": "action", "amount": 1 }],
                    "intent_budget": { "check": "action" }
                }
            }
        });
        let (session, _sink) = session_with_state(sb, state_with_pc());
        let _enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        let order = session.state.lock().unwrap().turn.order.clone();
        assert!(order[0].contains("mon-goblin"), "固定先攻 100 的怪物应当排第一：{order:?}");
        assert_eq!(order.len(), 2, "怪物 + 受控角色：{order:?}");

        let mila = ActorRef { id: "char-a".into(), name: "米拉".into() };
        let check = Intent::Check { attribute: "str".into(), difficulty: None, actor_id: None, opponent_id: None };
        let err = session.turn_gate(&check, Some(&mila)).expect_err("现在不是米拉的回合");
        assert_eq!(err.0, RejectionCode::NotYourTurn, "{err:?}");

        // end_turn 推进到下一个行动者：米拉拿到自己的回合，预算被重置。
        session.end_turn();
        {
            let st = session.state.lock().unwrap();
            assert_eq!(st.turn.order[st.turn.index], "char-a", "应当轮到米拉");
        }
        assert!(session.turn_gate(&check, Some(&mila)).is_ok(), "轮到米拉后应当放行");
        assert_eq!(
            session.state.lock().unwrap().turn.budgets["char-a"]["action"],
            0,
            "宣告即消耗：放行后 action 应当扣到 0"
        );
        let err2 = session.turn_gate(&check, Some(&mila)).expect_err("预算已耗尽");
        assert_eq!(err2.0, RejectionCode::InsufficientBudget, "{err2:?}");
    }

    /// 「失去回合」（#GAP-I）：skip_turn 让目标跳过 N 个时序回合，其机械意图被驳回；
    /// 跳过计数在下一次推进时被消耗，之后恢复行动。
    #[tokio::test]
    async fn skip_turn_makes_the_actor_lose_its_turn() {
        let sb = json!({
            "world": {
                "check": { "dice": "1d20" },
                "turn": {
                    "order": "none",
                    "budgets": [{ "id": "action", "amount": 1 }],
                    "intent_budget": { "check": "action" }
                }
            }
        });
        let (session, _sink) = session_with_state(sb, state_with_pc());
        let _enc = create_encounter(&session, vec![temp_spec("灰狼", 11, 12)]).await;
        let key = session.state.lock().unwrap().turn.order[0].clone();
        // 规则包原语（引擎不认识「突袭」，只认识「跳过几个回合」）。
        session.apply_lua_requests(&[LuaRequest::SkipTurn { target: key.clone(), turns: 1 }], &key);
        assert_eq!(session.state.lock().unwrap().turn.skip.get(&key).copied(), Some(1));

        let me = ActorRef { id: key.clone(), name: "米拉".into() };
        let check = Intent::Check { attribute: "str".into(), difficulty: None, actor_id: None, opponent_id: None };
        let err = session.turn_gate(&check, Some(&me)).expect_err("本轮失去了回合");
        assert_eq!(err.0, RejectionCode::NotYourTurn, "{err:?}");

        session.end_turn();
        assert_eq!(
            session.state.lock().unwrap().turn.skip.get(&key).copied(),
            None,
            "跳过计数应当在推进时被消耗掉"
        );
        assert!(session.turn_gate(&check, Some(&me)).is_ok(), "下一轮恢复行动");
    }

    /// encounter_active（#GAP-I 顺带关掉 GAP-M）：有未结束遭遇才成立，且**不能**由
    /// encounter_cleared 取反得到（没有遭遇时两者都不成立）。
    #[tokio::test]
    async fn encounter_active_condition_tracks_live_encounters() {
        let (session, _sink) = session_with(json!({ "world": {} }));
        let active = CondExpr::EncounterActive {};
        let cleared = CondExpr::EncounterCleared {};
        let eval = |c: &CondExpr, s: &Arc<Session>| {
            s.with_eval_context("test", |ctx| eval_cond(c, ctx).unwrap_or(false))
        };
        assert!(!eval(&active, &session), "没有遭遇时 encounter_active 不成立");
        assert!(!eval(&cleared, &session), "没有遭遇时 encounter_cleared 也不成立");
        let _enc = create_encounter(&session, vec![temp_spec("灰狼", 11, 12)]).await;
        assert!(eval(&active, &session), "有未结束的遭遇时成立");
        assert!(!eval(&cleared, &session), "还有敌人活着 → 尚未清空");
    }

    /// Lua 只读口：`host.turn` 给事实（轮次 / 当前行动者 / 预算），不给规则语义。
    #[tokio::test]
    async fn lua_host_turn_exposes_readonly_facts() {
        let sb = json!({
            "world": {
                "check": { "dice": "1d20" },
                "turn": {
                    "order": "none",
                    "budgets": [{ "id": "action", "amount": 2 }],
                    "intent_budget": { "check": "action" }
                }
            }
        });
        let (session, _sink) = session_with_state(sb, state_with_pc());
        assert!(session.turn_snapshot().is_none(), "非战斗中读 host.turn 应当是 nil");
        let _enc = create_encounter(&session, vec![temp_spec("灰狼", 11, 12)]).await;
        let snap = session.turn_snapshot().expect("战斗中应当有 host.turn");
        assert_eq!(snap.get("round").and_then(Value::as_u64), Some(1));
        assert!(snap.get("current").and_then(Value::as_str).is_some(), "当前行动者要可见");
        assert_eq!(
            snap.pointer("/budgets/action").and_then(Value::as_i64),
            Some(2),
            "预算事实要交给规则包：{snap}"
        );
        assert_eq!(snap.get("combat").and_then(Value::as_bool), Some(true));
    }

    fn goblin_spec(count: u32) -> octopus_types::EnemySpec {
        octopus_types::EnemySpec {
            name: "地精".into(),
            hp: None,
            ac: None,
            template_id: Some("mon-goblin".into()),
            count: Some(count),
            skill_id: None,
        }
    }

    fn temp_spec(name: &str, hp: i64, ac: i64) -> octopus_types::EnemySpec {
        octopus_types::EnemySpec {
            name: name.into(),
            hp: Some(hp),
            ac: Some(ac),
            template_id: None,
            count: None,
            skill_id: None,
        }
    }

    /// 结算一次遭遇创建，返回新遭遇的键。
    async fn create_encounter(
        session: &Arc<Session>,
        enemies: Vec<octopus_types::EnemySpec>,
    ) -> String {
        let before: std::collections::BTreeSet<String> =
            session.state.lock().unwrap().encounters.keys().cloned().collect();
        session
            .handle_intent(Intent::Encounter { name: "遭遇".into(), enemies, note: None }, None)
            .await;
        let st = session.state.lock().unwrap();
        st.encounters
            .keys()
            .find(|k| !before.contains(*k))
            .cloned()
            .expect("遭遇已创建")
    }

    fn instance_of(session: &Arc<Session>, key: &str) -> CharacterInstance {
        session
            .state
            .lock()
            .unwrap()
            .characters
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("实例不存在：{key}"))
    }

    fn res_hp_of(session: &Arc<Session>, key: &str) -> i64 {
        instance_of(session, key).resources["res-hp"].as_i64().unwrap_or(-1)
    }

    fn enemy_views(session: &Arc<Session>, enc: &str) -> Vec<EnemyView> {
        let st = session.state.lock().unwrap();
        serde_json::from_value(st.encounters[enc]["enemies"].clone()).expect("敌人条目")
    }

    fn persisted_of(session: &Arc<Session>) -> Vec<PersistedEvent> {
        session
            .event_log
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(|envelope| PersistedEvent { request_id: None, envelope })
            .collect()
    }

    /// 验收 1：DeltaDomain::Character 的 field="instance" + op=Add 能插入完整实例。
    #[test]
    fn character_instance_delta_inserts_full_instance() {
        let mut st = state_with_pc();
        let inst = CharacterInstance {
            instance_id: "enc-x:mon-goblin#1".into(),
            template_id: "mon-goblin".into(),
            name: "地精".into(),
            kind: "monster".into(),
            attributes: serde_json::json!({ "dex": 14 }).as_object().unwrap().clone(),
            resources: serde_json::json!({ "res-hp": 7 }).as_object().unwrap().clone(),
            inventory: Default::default(),
            location_id: Some("loc-cave".into()),
            present: true,
            statuses: vec![],
        };
        apply_delta(
            &mut st,
            &StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "enc-x:mon-goblin#1".into(),
                field: "instance".into(),
                op: DeltaOp::Add,
                value: serde_json::to_value(&inst).unwrap(),
            },
        );
        let got = st.characters.get("enc-x:mon-goblin#1").expect("实例已插入");
        assert_eq!(got.name, "地精");
        assert_eq!(got.kind, "monster");
        assert_eq!(got.resources["res-hp"].as_i64(), Some(7));
        assert_eq!(got.location_id.as_deref(), Some("loc-cave"));
        // 非 instance / 非 Add 的 delta 不走插入路径（旧日志行为不变）。
        apply_delta(
            &mut st,
            &StateDelta {
                domain: DeltaDomain::Character,
                entity_id: "不存在的键".into(),
                field: "resources.hp".into(),
                op: DeltaOp::Set,
                value: serde_json::json!(5),
            },
        );
        assert!(!st.characters.contains_key("不存在的键"));
    }

    /// 极简 tracing 订阅者：只收集 WARN 及以上事件（含域 / 键 / 字段等结构化字段）。
    ///
    /// 刻意不引入 tracing-subscriber（那是 api 层的依赖）：这里只需要「收到一条事件」，
    /// 用 tracing 自带的 `Subscriber` trait 即可，engine 的依赖零变化。
    struct WarnCapture {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl tracing::Subscriber for WarnCapture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() <= tracing::Level::WARN
        }

        fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
            Some(tracing::level_filters::LevelFilter::WARN)
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            struct Fields(String);
            impl tracing::field::Visit for Fields {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0.push_str(&format!("{value:?}"));
                    } else {
                        self.0.push_str(&format!(" {}={value:?}", field.name()));
                    }
                }
            }
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            if let Ok(mut lines) = self.lines.lock() {
                lines.push(format!("{} {}", event.metadata().level(), fields.0));
            }
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// T26/T27 登记：落不到角色的 Character delta 以前是**静默丢弃**。
    ///
    /// 这里钉住三件事：
    /// ① 行为零变化——投影逐字相同（可见性修复不改变任何状态结果）；
    /// ② 可见性——丢弃时确实有一条带 域 / 实体键 / 字段 的 WARN；
    /// ③ 去重——同一条丢弃（实时之后的重放会原样再跑一遍）不再重复刷日志。
    #[test]
    fn dropped_character_delta_keeps_projection_verbatim_but_warns_once() {
        let mut st = state_with_pc();
        let before = serde_json::to_value(&st).unwrap();
        let delta = StateDelta {
            domain: DeltaDomain::Character,
            entity_id: "t27-missing-entity-warn".into(),
            field: "resources.hp".into(),
            op: DeltaOp::Add,
            value: json!(-7),
        };
        let lines = Arc::new(Mutex::new(Vec::new()));
        let subscriber = WarnCapture { lines: lines.clone() };
        tracing::subscriber::with_default(subscriber, || {
            apply_delta(&mut st, &delta);
            // 重放是纯投影，会把同一批 delta 再跑一遍：去重后不该再刷一条。
            apply_delta(&mut st, &delta);
        });
        assert_eq!(
            serde_json::to_value(&st).unwrap(),
            before,
            "丢弃路径的行为不变：世界状态逐字相同"
        );
        let lines = lines.lock().unwrap().clone();
        assert_eq!(lines.len(), 1, "同一条丢弃只告警一次（重放不刷屏）：{lines:?}");
        let line = &lines[0];
        assert!(line.starts_with("WARN"), "必须是 WARN 级：{line}");
        assert!(line.contains("Character"), "带域：{line}");
        assert!(line.contains("t27-missing-entity-warn"), "带实体键：{line}");
        assert!(line.contains("resources.hp"), "带字段：{line}");
    }

    /// 验收 2：遭遇按 template_id + count 克隆实例（键 enc-{enc}:{tpl}#n），
    /// attributes / resources 来自模板快照，present / location_id 按遭遇地点。
    #[tokio::test]
    async fn encounter_clones_bestiary_instances() {
        let (session, _sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let enc = create_encounter(&session, vec![goblin_spec(2)]).await;
        let views = enemy_views(&session, &enc);
        assert_eq!(views.len(), 2, "count = 2 → 两只实例");
        assert_eq!(views[0].id, "e1");
        assert_eq!(views[1].id, "e2");
        for (i, view) in views.iter().enumerate() {
            let key = view.instance_id.clone().expect("实例键");
            assert_eq!(key, format!("enc-{enc}:mon-goblin#{}", i + 1));
            assert_eq!(view.template_id.as_deref(), Some("mon-goblin"));
            assert_eq!(view.hp, 7, "模板快照满值");
            assert_eq!(view.max, 7);
            assert_eq!(view.ac, 15, "10 + dex_mod(2) + 挂接护甲(3)");
            assert_eq!(view.scene_id.as_deref(), Some("sc-1"));
            assert_eq!(view.location_id.as_deref(), Some("loc-cave"));
            let inst = instance_of(&session, &key);
            assert_eq!(inst.kind, "monster");
            assert_eq!(inst.attributes["dex"].as_i64(), Some(14));
            assert_eq!(inst.resources["res-hp"].as_i64(), Some(7));
            assert!(inst.present, "遭遇里的怪物在场");
            assert_eq!(inst.location_id.as_deref(), Some("loc-cave"));
        }
        // 数据卡摘要：技能名 + 伤害骰。
        assert_eq!(views[0].attacks.len(), 2);
        assert_eq!(views[0].attacks[0].skill_id, "sk-scimitar");
        assert_eq!(views[0].attacks[0].name, "弯刀");
        assert_eq!(views[0].attacks[0].damage, "1d6+2");
    }

    /// 验收 3 + 9：玩家 strike 模板怪 → 难度取派生 AC，伤害扣实例的 res-hp，
    /// 遭遇条目是实例的**投影**；重放后实例与 HP 一致。
    #[tokio::test]
    async fn strike_bestiary_monster_hits_derived_ac_and_instance_hp() {
        let (session, sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let enc = create_encounter(&session, vec![goblin_spec(2)]).await;
        let key = enemy_views(&session, &enc)[0].instance_id.clone().unwrap();

        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-hit".into()), ..Default::default() },
                None,
            )
            .await;
        let narrative = strike_narrative(&sink);
        assert!(narrative.contains("≥ 15"), "难度必须是派生 AC 15：{narrative}");
        assert!(narrative.contains("造成 3 点伤害（7 → 4）"), "{narrative}");
        // 伤害落到**实例**的 res-hp（技能声明的是 hp，图鉴怪的生命资源叫 res-hp）。
        assert_eq!(res_hp_of(&session, &key), 4);
        // 遭遇条目同步为投影。
        assert_eq!(enemy_hp(&session, &enc, 0), 4);
        assert_eq!(enemy_hp(&session, &enc, 1), 7, "第二只不受影响");
        // 连续第二次：实例 HP 是权威，钳制到 0 且与投影保持同步。
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-hit".into()), ..Default::default() },
                None,
            )
            .await;
        assert_eq!(res_hp_of(&session, &key), 1);
        assert_eq!(enemy_hp(&session, &enc, 0), 1);

        // 重放（验收 9 的实例部分）：实时与重放必须得到同一世界状态。
        let persisted = persisted_of(&session);
        let (restarted, _sink2) = session_with_state(bestiary_storybook(), state_with_pc());
        restarted.replay(&persisted);
        assert_eq!(res_hp_of(&restarted, &key), 1, "重放后实例 HP 一致");
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "重放后投影逐字一致"
        );
        assert_eq!(session.current_seq(), restarted.current_seq());
    }

    /// 验收 5：同模板两场遭遇的实例互相隔离（各自独立 HP）。
    #[tokio::test]
    async fn bestiary_instances_are_isolated_between_encounters() {
        let (session, _sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let enc_a = create_encounter(&session, vec![goblin_spec(1)]).await;
        let enc_b = create_encounter(&session, vec![goblin_spec(1)]).await;
        assert_ne!(enc_a, enc_b);
        let key_a = enemy_views(&session, &enc_a)[0].instance_id.clone().unwrap();
        let key_b = enemy_views(&session, &enc_b)[0].instance_id.clone().unwrap();
        assert_ne!(key_a, key_b, "实例键带遭遇 id → 天然隔离");

        // 按实例键寻址，精确打第二场的那只（两场都有 e1）。
        session
            .strike_enemy(
                key_b.clone(),
                AttackChoice { skill_id: Some("sk-hit".into()), ..Default::default() },
                None,
            )
            .await;
        assert_eq!(res_hp_of(&session, &key_b), 4);
        assert_eq!(res_hp_of(&session, &key_a), 7, "同模板的另一场不受影响");
        assert_eq!(enemy_hp(&session, &enc_a, 0), 7);
        assert_eq!(enemy_hp(&session, &enc_b, 0), 4);
    }

    /// 验收 4：Intent::EnemyStrike 走同一内核——命中用怪物自身属性，
    /// 难度 = 目标派生 AC，伤害扣目标 resources.hp。
    #[tokio::test]
    async fn enemy_strike_uses_monster_attributes_and_target_derived_ac() {
        // PC dex 10 → 派生 AC = 10（不是回落 12）；怪物 dex 70 → 修正夹到该维度上限 10。
        let mut sb = bestiary_storybook();
        sb["characters"][0]["attributes"] = serde_json::json!({ "str": 8, "dex": 70 });
        let mut base = state_with_pc();
        base.characters.get_mut("char-a").unwrap().attributes =
            serde_json::json!({ "str": 70, "dex": 10 }).as_object().unwrap().clone();
        let (session, sink) = session_with_state(sb.clone(), base.clone());
        let _enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        session
            .handle_intent(
                Intent::EnemyStrike {
                    enemy_id: "e1".into(),
                    target_id: None,
                    skill_id: Some("sk-bow".into()),
                },
                None,
            )
            .await;
        let narrative = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("enemy_strike") => {
                    p.narrative.clone()
                }
                _ => None,
            })
            .next_back()
            .unwrap_or_default();
        // 修正来自**怪物自己**的 dex 70（夹到该维度修正上限 10）；若误用 PC 的 dex 10 会是 0。
        assert!(narrative.contains("修正 10"), "命中必须用怪物自身属性：{narrative}");
        assert!(narrative.contains("≥ 10"), "难度 = 目标派生 AC 10：{narrative}");
        assert!(!narrative.contains("未命中"), "{narrative}");
        let hp = session.projection().characters["char-a"]["resources"]["hp"]
            .as_f64()
            .unwrap();
        assert!(hp < 30.0, "伤害必须落到目标 resources.hp：{hp}");

        // 同一内核的另一方向也走同一条变更路径：重放后玩家 HP 一致。
        let persisted = persisted_of(&session);
        let (restarted, _sink2) = session_with_state(sb, base);
        restarted.replay(&persisted);
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap()
        );
    }

    /// 验收 3（续）：EnemyStrike 缺省用图鉴条目的第一条攻击技能；
    /// 敌人与目标都能按名字 / 任意 id 形态寻址。
    #[tokio::test]
    async fn enemy_strike_defaults_to_first_bestiary_attack() {
        let (session, sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let _enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        session
            .handle_intent(
                Intent::EnemyStrike {
                    enemy_id: "地精".into(),
                    target_id: Some("char-a".into()),
                    skill_id: None,
                },
                None,
            )
            .await;
        let narrative = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::Resolution(p) if p.outcome.as_deref() == Some("enemy_strike") => {
                    p.narrative.clone()
                }
                _ => None,
            })
            .next_back()
            .unwrap_or_default();
        assert!(narrative.contains("弯刀"), "缺省 = 数据卡第一条攻击技能：{narrative}");
        assert!(narrative.contains("米拉"), "目标按 id 寻址：{narrative}");
    }

    /// 判定 C5 验收 1（另一方向）：EnemyStrike 也发 CheckResultPayload，署名攻方（敌人）；
    /// 玩家被攻击时同样看得到骰面。
    #[tokio::test]
    async fn enemy_strike_emits_check_result_for_the_ui() {
        let mut sb = bestiary_storybook();
        sb["characters"][0]["attributes"] = serde_json::json!({ "str": 8, "dex": 70 });
        let mut base = state_with_pc();
        base.characters.get_mut("char-a").unwrap().attributes =
            serde_json::json!({ "str": 70, "dex": 10 }).as_object().unwrap().clone();
        let (session, sink) = session_with_state(sb, base);
        let _enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        session
            .handle_intent(
                Intent::EnemyStrike {
                    enemy_id: "e1".into(),
                    target_id: None,
                    skill_id: Some("sk-bow".into()),
                },
                None,
            )
            .await;
        let events = sink.0.lock().unwrap().clone();
        let check = check_result_of(&events).expect("enemy_strike 必须发 CheckResult");
        assert_eq!(check.actor, ActorRef { id: "mon-goblin".into(), name: "地精".into() });
        assert_eq!(check.attribute, "dex", "用怪物自己的判定属性");
        assert_eq!(check.r#mod, 10, "dex 70 → 夹到该维度修正上限 10");
        assert_eq!(check.expr.as_deref(), Some("1d20"));
        assert_eq!(check.rolls.as_ref().map(Vec::len), Some(1));
        assert_eq!(check.target, 10, "难度 = 目标派生 AC 10");
        assert_eq!(check.result, check.total >= 10);
        assert_eq!(check.kind, Some(CheckKind::Attack));
    }

    /// 验收 4（续）：故事书没有 derived.ac 时回落 12。
    #[tokio::test]
    async fn derived_ac_falls_back_to_12_without_derived_declaration() {
        let sb = serde_json::json!({
            "characters": [{
                "id": "mon-plain", "name": "石像", "kind": "monster",
                "attributes": { "str": 10 },
                "resources": { "hp": 9 }
            }],
            "skills": []
        });
        let (session, _sink) = session_with_state(sb, state_with_pc());
        let enc = create_encounter(
            &session,
            vec![octopus_types::EnemySpec {
                name: "石像".into(),
                hp: None,
                ac: None,
                template_id: Some("mon-plain".into()),
                count: None,
                skill_id: None,
            }],
        )
        .await;
        let views = enemy_views(&session, &enc);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].ac, 12, "缺失 derived.ac → 12");
        assert_eq!(views[0].hp, 9, "生命资源叫 hp 也认得");
    }

    /// 验收 6（回归）：无 template_id 的临时敌人行为与改动前逐字一致——
    /// 条目契约只有 id/name/hp/max/ac，且不产生任何角色实例。
    #[tokio::test]
    async fn temp_enemy_without_template_behaves_as_before() {
        let (session, sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let enc = create_encounter(&session, vec![temp_spec("灰狼", 11, 12)]).await;
        let delta = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::StateUpdate(p) => Some(p.clone()),
                _ => None,
            })
            .next_back()
            .expect("遭遇 delta");
        assert_eq!(delta.changes[0].domain, DeltaDomain::Encounter);
        assert_eq!(
            delta.changes[0].value["enemies"][0],
            serde_json::json!({ "id": "e1", "name": "灰狼", "hp": 11, "max": 11, "ac": 12 }),
            "临时敌人条目的形状必须与改动前逐字一致（无新增字段）"
        );
        assert_eq!(
            delta.changes[0].value["enemies"][0].as_object().unwrap().len(),
            5,
            "不许多出 instance_id / template_id / scene_id / location_id / attacks"
        );
        assert_eq!(delta.changes.len(), 1, "临时敌人不产生实例 delta");
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .characters
                .values()
                .all(|c| c.kind != "monster"),
            "临时敌人不产生角色实例"
        );
        // 旧路径的结算：条目自身是权威。
        session
            .strike_enemy(
                "e1".into(),
                AttackChoice { skill_id: Some("sk-hit".into()), ..Default::default() },
                None,
            )
            .await;
        assert_eq!(enemy_hp(&session, &enc, 0), 8, "11 - 3");
    }

    /// 验收 7：personas / present_npcs / present_actors 均不含 kind=monster。
    #[tokio::test]
    async fn monster_instances_never_leak_into_persona_channels() {
        let (session, _sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        let key = enemy_views(&session, &enc)[0].instance_id.clone().unwrap();
        // 模板带 background：不做 kind 门控的话它会被人格档案收下。
        assert!(
            session.rules.personas(&["mon-goblin".to_string()], 6).is_empty(),
            "怪物不参与扮演，不该占人格预算"
        );
        assert!(
            session.present_npcs().iter().all(|a| a.id != "mon-goblin"),
            "怪物不会说话，不当未署名台词的回落对象"
        );
        let st = session.state.lock().unwrap();
        assert!(
            present_actors(&st).iter().all(|a| a.id != "mon-goblin"),
            "怪物不进「在场角色」名单"
        );
        assert!(st.characters.contains_key(&key), "但实例确实在实例表里（可寻址）");
    }

    /// 验收 9：旧命令日志（Encounter delta 无 instance_id / attacks 等新字段）照常重放——
    /// 老日志不会命中 field="instance" 的插入分支，临时敌人路径与改动前逐字一致。
    #[tokio::test]
    async fn old_command_log_with_temp_enemy_replays_unchanged() {
        let (session, _sink) = session_with_state(bestiary_storybook(), state_with_pc());
        let envelope = |seq: u64, event: PlayEvent| PersistedEvent {
            request_id: None,
            envelope: EventEnvelope {
                id: format!("ev-{seq}"),
                seq,
                round: 1,
                ts: "2025-01-01T00:00:00Z".into(),
                actor: None,
                intent_id: None,
                event,
            },
        };
        let persisted = vec![
            envelope(
                1,
                PlayEvent::StateUpdate(StateUpdatePayload {
                    changes: vec![StateDelta {
                        domain: DeltaDomain::Encounter,
                        entity_id: "enc-old".into(),
                        field: "encounter".into(),
                        op: DeltaOp::Add,
                        value: json!({
                            "id": "enc-old", "name": "旧遭遇",
                            "enemies": [{ "id": "e1", "name": "灰狼", "hp": 11, "max": 11, "ac": 12 }],
                            "active": true
                        }),
                    }],
                }),
            ),
            envelope(
                2,
                PlayEvent::Resolution(ResolutionPayload {
                    intent_id: None,
                    status: ResolutionStatus::Ok,
                    rejection_code: None,
                    narrative: Some("旧日志的一击".into()),
                    outcome: Some("strike".into()),
                    triggered_events: None,
                    // 旧实现原样落库的两条 delta：角色域落在条目 id 上（当时没有实例，是 no-op）。
                    state_changes: vec![
                        StateDelta {
                            domain: DeltaDomain::Character,
                            entity_id: "e1".into(),
                            field: "resources.hp".into(),
                            op: DeltaOp::Add,
                            value: json!(-3),
                        },
                        StateDelta {
                            domain: DeltaDomain::Encounter,
                            entity_id: "enc-old".into(),
                            field: "encounter".into(),
                            op: DeltaOp::Set,
                            value: json!({ "enemies": [
                                { "id": "e1", "name": "灰狼", "hp": 8, "max": 11, "ac": 12 }
                            ] }),
                        },
                    ],
                }),
            ),
        ];
        session.replay(&persisted);
        let st = session.state.lock().unwrap();
        assert_eq!(st.encounters["enc-old"]["enemies"][0]["hp"].as_i64(), Some(8));
        assert!(
            st.characters.values().all(|c| c.kind != "monster"),
            "旧日志不产生怪物实例（field=instance 分支不被命中）"
        );
        assert_eq!(session.current_seq(), 2);
    }

    /// 验收 8：导入的 LMoP 草稿在遭遇创建后能真正产生怪物实例，
    /// 且 AC 由 derived 公式 + 挂接护甲算出来（地精 = 10 + 2 + 3 = 15）。
    #[tokio::test]
    async fn lmop_draft_bestiary_creates_real_instances() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../story_example/dnd/lmop-storybook.draft.json"
        );
        let Ok(raw) = std::fs::read_to_string(path) else {
            eprintln!("跳过：找不到 LMoP 草稿 {path}");
            return;
        };
        let sb: Value = serde_json::from_str(&raw).expect("LMoP 草稿是合法 JSON");
        let scene_id = sb
            .pointer("/skeleton/0/scenes/0/id")
            .and_then(Value::as_str)
            .expect("草稿有骨架场景")
            .to_string();
        let scene_location = sb
            .pointer("/skeleton/0/scenes/0/location_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut st = state_with_pc();
        st.scene_id = scene_id;
        st.scene_title = "LMoP".into();
        let (session, _sink) = session_with_state(sb, st);
        let enc = create_encounter(
            &session,
            vec![octopus_types::EnemySpec {
                name: "地精".into(),
                hp: None,
                ac: None,
                template_id: Some("mon-goblin".into()),
                count: Some(2),
                skill_id: None,
            }],
        )
        .await;
        let views = enemy_views(&session, &enc);
        assert_eq!(views.len(), 2);
        let key = views[0].instance_id.clone().unwrap();
        let inst = instance_of(&session, &key);
        assert_eq!(inst.kind, "monster");
        assert_eq!(inst.resources["res-hp"].as_i64(), Some(7), "附录 B：HP 7");
        assert_eq!(inst.attributes["dex"].as_i64(), Some(14));
        assert_eq!(views[0].ac, 15, "10 + dex_mod(2) + armor-goblin(3)");
        assert_eq!(views[0].hp, 7);
        assert_eq!(inst.location_id, scene_location, "怪物实例继承遭遇地点");
        assert_eq!(views[0].attacks[0].skill_id, "sk-goblin-scimitar");
        assert_eq!(views[0].attacks[0].damage, "1d6+2");
    }

    // ============================================================
    // 地图 P5：剧情关联（触发点预置遭遇 · 遭遇叙事锚 · encounter_cleared）
    // ============================================================

    /// P5 测试故事书：洞穴场景（地点 loc-cave）+ 两条图鉴怪物 + 触发点 tr-ambush。
    ///
    /// 场景目标 g-clear 用 encounter_cleared —— 顺带验证「遭遇建成 → 打完 → 目标达成」。
    fn p5_storybook(condition: Value, encounter: Option<Value>) -> Value {
        let mut trigger = json!({
            "id": "tr-ambush",
            "title": "洞穴伏击",
            "hint": "阴影里有东西在动。",
            "condition": condition,
        });
        if let Some(enc) = encounter {
            trigger["encounter"] = enc;
        }
        json!({
            "attribute_dimensions": [
                { "key": "str", "baseline": 10, "modifier_step": 2, "min": 1, "max": 30 }
            ],
            "characters": [
                { "id": "mon-goblin", "name": "地精", "kind": "monster",
                  "attributes": { "str": 8 }, "resources": { "res-hp": 7 },
                  "skills": ["sk-cleave"] },
                { "id": "mon-wolf", "name": "灰狼", "kind": "monster",
                  "attributes": { "str": 12 }, "resources": { "res-hp": 11 },
                  "skills": ["sk-cleave"] }
            ],
            "skills": [
                { "id": "sk-cleave", "name": "横扫", "attribute": "str",
                  "check": { "lua": "return { total = 100, margin = 100 }" },
                  "effect": { "immediate": [
                      { "kind": "damage", "amount": "99", "resource": "res-hp" } ] } }
            ],
            "skeleton": [{ "id": "ch-1", "scenes": [{
                "id": "sc-cave", "title": "洞穴", "location_id": "loc-cave",
                "goals": [
                    { "id": "g-clear", "text": "清剿地精", "primary": true,
                      "condition": { "op": "encounter_cleared" } }
                ],
                "triggers": [trigger]
            }] }],
            "world": { "locations": [
                { "id": "loc-cave", "name": "洞穴" },
                { "id": "loc-trail", "name": "山道" }
            ] }
        })
    }

    /// P5 世界状态：当前场景 = sc-cave（骨架里声明了 loc-cave）。
    fn p5_state() -> WorldState {
        let mut st = state_with_pc();
        st.scene_id = "sc-cave".into();
        st.scene_title = "洞穴".into();
        st
    }

    fn ambush_preset() -> Value {
        json!({
            "name": "游荡的地精",
            "note": "触发点预置",
            "enemies": [{ "template_id": "mon-goblin", "count": 2 }]
        })
    }

    fn only_encounter(session: &Arc<Session>) -> (String, EncounterView) {
        let st = session.state.lock().unwrap();
        assert_eq!(st.encounters.len(), 1, "应当正好一场遭遇");
        let (id, raw) = st.encounters.iter().next().unwrap();
        (
            id.clone(),
            serde_json::from_value(raw.clone()).expect("EncounterView"),
        )
    }

    /// 验收 2 / 3（地图 P5 §6.3 / §6.4）：触发点被标记 fired 后自动建遭遇——
    /// 与 Intent::Encounter **同一条创建路径**（图鉴实例克隆 + 地点继承），叙事锚创建时快照。
    #[tokio::test]
    async fn fired_trigger_spawns_preset_encounter() {
        let mut st = p5_state();
        st.flags.insert("ambush".into(), json!(true));
        let condition = json!({ "op": "flag_set", "flag": "ambush" });
        let (session, sink) =
            session_with_state(p5_storybook(condition.clone(), Some(ambush_preset())), st);
        assert!(session.state.lock().unwrap().encounters.is_empty(), "求值前没有遭遇");
        session.evaluate_turn_end();

        let (enc_id, view) = only_encounter(&session);
        assert_eq!(view.name, "游荡的地精");
        assert_eq!(view.note.as_deref(), Some("触发点预置"));
        // §6.4 叙事锚：创建时快照（不是查询时推导）。
        assert_eq!(view.scene_id.as_deref(), Some("sc-cave"));
        assert_eq!(view.location_id.as_deref(), Some("loc-cave"), "缺省继承场景地点");
        assert_eq!(view.goal_id.as_deref(), Some("g-clear"), "关联等这场遭遇清空的目标");
        assert_eq!(view.template_ids, vec!["mon-goblin".to_string()]);

        // T5 的实例克隆路径原样复用（键 / 模板快照 / 地点）。
        assert_eq!(view.enemies.len(), 2);
        for (i, enemy) in view.enemies.iter().enumerate() {
            let key = enemy.instance_id.clone().expect("实例键");
            assert_eq!(key, format!("enc-{enc_id}:mon-goblin#{}", i + 1));
            assert_eq!(enemy.template_id.as_deref(), Some("mon-goblin"));
            assert_eq!(enemy.scene_id.as_deref(), Some("sc-cave"));
            assert_eq!(enemy.location_id.as_deref(), Some("loc-cave"));
            assert_eq!(enemy.attacks.len(), 1);
        }
        {
            let st = session.state.lock().unwrap();
            assert_eq!(st.progress.triggers.get("tr-ambush"), Some(&json!(true)));
            let key = view.enemies[0].instance_id.clone().unwrap();
            let inst = st.characters.get(&key).expect("怪物实例");
            assert_eq!(inst.resources["res-hp"].as_i64(), Some(7));
            assert_eq!(inst.location_id.as_deref(), Some("loc-cave"));
            assert_eq!(inst.kind, "monster");
        }

        // 事件顺序：先落触发点 fired，再落遭遇（重放逐条一致的前提）。
        let events = sink.0.lock().unwrap().clone();
        let order: Vec<&str> = events
            .iter()
            .filter_map(|e| match &e.event {
                PlayEvent::StateUpdate(p) => Some(
                    if p.changes.iter().any(|c| c.domain == DeltaDomain::Encounter) {
                        "encounter"
                    } else {
                        "progress"
                    },
                ),
                _ => None,
            })
            .collect();
        assert_eq!(order, vec!["progress", "encounter"], "先标记 fired，再建遭遇");

        // 重放一致：锚来自日志快照，重放后投影逐字相同。
        let persisted = persisted_of(&session);
        let mut st2 = p5_state();
        st2.flags.insert("ambush".into(), json!(true));
        let (restarted, _sink2) =
            session_with_state(p5_storybook(condition, Some(ambush_preset())), st2);
        restarted.replay(&persisted);
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "重放后投影逐字一致"
        );
        assert_eq!(session.current_seq(), restarted.current_seq());
    }

    /// 地图 P5 §6.3：预置声明的地点覆盖场景地点，怪物实例跟着走；name 缺省回落触发点标题。
    #[tokio::test]
    async fn preset_location_overrides_scene_location() {
        let mut st = p5_state();
        st.flags.insert("ambush".into(), json!(true));
        let (session, _sink) = session_with_state(
            p5_storybook(
                json!({ "op": "flag_set", "flag": "ambush" }),
                Some(json!({
                    "location_id": "loc-trail",
                    "enemies": [{ "template_id": "mon-wolf" }]
                })),
            ),
            st,
        );
        session.evaluate_turn_end();
        let (_, view) = only_encounter(&session);
        assert_eq!(view.name, "洞穴伏击", "预置没写 name → 回落触发点标题");
        assert_eq!(view.location_id.as_deref(), Some("loc-trail"));
        assert_eq!(view.template_ids, vec!["mon-wolf".to_string()]);
        let key = view.enemies[0].instance_id.clone().unwrap();
        let st = session.state.lock().unwrap();
        assert_eq!(st.characters[&key].location_id.as_deref(), Some("loc-trail"));
    }

    /// 验收 6（地图 P5 §6.8）：掷表遭遇**不新增任何封闭字段**。链路是
    /// Lua 挂载点掷 engine_rng → 已有原语 apply_effect(set_flag) → 触发点 condition: flag_set
    /// + encounter 预置 → 回合末求值 fired 时建遭遇；表本身只活在 Lua 里。
    #[tokio::test]
    async fn lua_wandering_table_drives_preset_encounter() {
        // 同种子（会话 seed = 42）下的第一颗 d20，据此断言掷表分支。
        let mut probe = DeterministicRng::new(42);
        let roll = probe.range_inclusive(1, 20);

        let mut sb = p5_storybook(
            json!({ "op": "flag_set", "flag": "wander_high" }),
            Some(json!({
                "name": "游荡怪物（高）",
                "enemies": [{ "template_id": "mon-goblin" }]
            })),
        );
        sb["skeleton"][0]["scenes"][0]["triggers"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "tr-wander-low",
                "title": "游荡怪物（低）",
                "condition": { "op": "flag_set", "flag": "wander_low" },
                "encounter": {
                    "name": "游荡怪物（低）",
                    "enemies": [{ "template_id": "mon-wolf" }]
                }
            }));
        sb["lua_mounts"] = json!([{
            "id": "wandering-table",
            "mount": "event",
            "source": "local roll = host.engine_rng(1, 20)\nif roll >= 11 then host.apply_effect('', { kind = 'set_flag', flag = 'wander_high' }) else host.apply_effect('', { kind = 'set_flag', flag = 'wander_low' }) end"
        }]);

        let (session, sink) = session_with_state(sb, p5_state());
        // 进入区域：场景事件 → 挂载点链 → 掷表 → set_flag（已有原语，不是新类型）。
        session.dispatch_event("scene");
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(
            rng_logged(&events),
            probe.consumed,
            "掷表必须真的消耗引擎 RNG：与探针同一确定性序列"
        );

        let high = roll >= 11;
        let flags = session.state.lock().unwrap().flags.clone();
        let expected = if high { "wander_high" } else { "wander_low" };
        let other = if high { "wander_low" } else { "wander_high" };
        assert_eq!(flags.get(expected), Some(&json!(true)), "掷表结果决定置位哪个标记");
        assert!(flags.get(other).is_none());

        // 回合末求值：条件成立 → 触发点 fired → 按预置建遭遇。
        session.evaluate_turn_end();
        let (_, view) = only_encounter(&session);
        assert_eq!(
            view.template_ids,
            vec![if high { "mon-goblin" } else { "mon-wolf" }.to_string()],
            "掷表结果必须驱动遭遇内容"
        );
        assert_eq!(view.enemies[0].name, if high { "地精" } else { "灰狼" });
        assert_eq!(view.location_id.as_deref(), Some("loc-cave"), "表项没写地点 → 继承场景");
        let st = session.state.lock().unwrap();
        assert_eq!(
            st.progress.triggers.get(if high { "tr-ambush" } else { "tr-wander-low" }),
            Some(&json!(true))
        );
    }

    /// 验收 4 / 7（地图 P5 §6.5）：encounter_cleared 让清剿有自然判据；
    /// 没有 encounter 预置的旧故事书照常只标记 fired，一场遭遇都不建，System 文案逐字不变。
    #[tokio::test]
    async fn encounter_cleared_completes_goal_and_legacy_trigger_stays_quiet() {
        let mut st = p5_state();
        st.flags.insert("ambush".into(), json!(true));
        let condition = json!({ "op": "flag_set", "flag": "ambush" });
        let (session, _sink) =
            session_with_state(p5_storybook(condition.clone(), Some(ambush_preset())), st);
        session.evaluate_turn_end();

        // 敌人还活着 → 清剿目标未达成。
        session.evaluate_turn_end();
        assert_ne!(
            session.projection().progress.goals.get("g-clear"),
            Some(&json!(true)),
            "还有活着的敌人时不得判定清剿完成"
        );
        let quest = session
            .projection()
            .quests
            .into_iter()
            .find(|q| q.id == "g-clear")
            .expect("清剿任务");
        assert!(!quest.done);
        assert_eq!(quest.location_id.as_deref(), Some("loc-cave"), "任务继承场景地点");

        // 打光两只 → encounter_cleared 成立 → 目标达成。
        for enemy in ["e1", "e2"] {
            session
                .handle_intent(
                    Intent::Strike { enemy_id: enemy.into(), skill_id: Some("sk-cleave".into()) },
                    None,
                )
                .await;
        }
        session.evaluate_turn_end();
        assert_eq!(
            session.projection().progress.goals.get("g-clear"),
            Some(&json!(true)),
            "敌人全灭 → encounter_cleared 成立"
        );
        assert!(session
            .projection()
            .quests
            .iter()
            .any(|q| q.id == "g-clear" && q.done));

        // 回归：同一场景里没有 encounter 预置的触发点只标记 fired，不建遭遇、文案不变。
        let mut st2 = p5_state();
        st2.flags.insert("ambush".into(), json!(true));
        let (legacy, legacy_sink) = session_with_state(p5_storybook(condition, None), st2);
        legacy.evaluate_turn_end();
        {
            let st = legacy.state.lock().unwrap();
            assert!(st.encounters.is_empty(), "没有 encounter 预置就不该建遭遇");
            assert_eq!(st.progress.triggers.get("tr-ambush"), Some(&json!(true)));
        }
        let text = legacy_sink
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::System(p) if p.code.as_deref() == Some("skeleton_progress") => {
                    Some(p.text.clone())
                }
                _ => None,
            })
            .expect("skeleton_progress");
        assert_eq!(text, "剧情触发：tr-ambush", "旧故事书的提示语逐字不变");
    }

    /// 验收 5：声明 repeatable 的触发点在条件**由假变真**时可再次触发（边沿语义），
    /// 并再次走预置遭遇链路；条件持续为真不重复；重放后世界状态逐字一致。
    #[tokio::test]
    async fn repeatable_trigger_refires_on_edge_and_replays() {
        let mut sb = p5_storybook(json!({ "op": "flag_set", "flag": "ambush" }), Some(ambush_preset()));
        sb["skeleton"][0]["scenes"][0]["triggers"][0]["repeatable"] = json!(true);
        let mut st = p5_state();
        st.flags.insert("ambush".into(), json!(true));
        let (session, _sink) = session_with_state(sb.clone(), st);

        // ① 首次触发：建遭遇 + 落 {fired, active}（不再是一次性的裸 true）。
        session.evaluate_turn_end();
        assert_eq!(session.state.lock().unwrap().encounters.len(), 1);
        assert_eq!(
            session.state.lock().unwrap().progress.triggers.get("tr-ambush"),
            Some(&json!({ "fired": true, "active": true }))
        );

        // ② 条件持续为真：不重复触发、不重复建遭遇。
        session.evaluate_turn_end();
        assert_eq!(session.state.lock().unwrap().encounters.len(), 1, "持续为真不重复触发");

        // ③ 条件转假：不触发，但把 active=false 落库（边沿依赖的「上一次值」）。
        session.state.lock().unwrap().flags.remove("ambush");
        session.evaluate_turn_end();
        assert_eq!(
            session.state.lock().unwrap().progress.triggers.get("tr-ambush"),
            Some(&json!({ "fired": true, "active": false }))
        );

        // ④ 条件再由假变真：再次触发，并再建一场预置遭遇（旧实现整局只触发一次）。
        session.state.lock().unwrap().flags.insert("ambush".into(), json!(true));
        session.evaluate_turn_end();
        assert_eq!(session.state.lock().unwrap().encounters.len(), 2, "边沿再次触发 → 再出遭遇");

        // ⑤ 重放一致：整段日志重放后投影逐字相同（active 由 delta 承载，不靠重跑条件）。
        let persisted = persisted_of(&session);
        let mut st2 = p5_state();
        st2.flags.insert("ambush".into(), json!(true));
        let (restarted, _sink2) = session_with_state(sb, st2);
        restarted.replay(&persisted);
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "重放后投影逐字一致"
        );
        assert_eq!(session.current_seq(), restarted.current_seq());
    }

    /// 验收 6：没有 repeatable 声明的旧故事书，触发点的 delta 与文案逐字不变
    /// （domain / field / op / value 与历史一致；进度值仍是裸 true）。
    #[tokio::test]
    async fn one_shot_trigger_delta_stays_byte_identical() {
        let mut st = p5_state();
        st.flags.insert("ambush".into(), json!(true));
        let (session, sink) = session_with_state(
            p5_storybook(json!({ "op": "flag_set", "flag": "ambush" }), None),
            st,
        );
        session.evaluate_turn_end();
        let events = sink.0.lock().unwrap().clone();
        let changes = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::StateUpdate(p)
                    if p.changes.iter().any(|c| c.domain == DeltaDomain::Trigger) =>
                {
                    Some(p.changes.clone())
                }
                _ => None,
            })
            .expect("触发点进度 StateUpdate");
        assert_eq!(changes.len(), 1, "旧故事书只落一条进度 delta");
        let d = &changes[0];
        assert_eq!(d.domain, DeltaDomain::Trigger);
        assert_eq!(d.entity_id, "tr-ambush");
        assert_eq!(d.field, "fired");
        assert_eq!(d.op, DeltaOp::Set);
        assert_eq!(d.value, json!(true), "一次性触发点仍落裸 true");
        let text = events
            .iter()
            .find_map(|e| match &e.event {
                PlayEvent::System(p) if p.code.as_deref() == Some("skeleton_progress") => {
                    Some(p.text.clone())
                }
                _ => None,
            })
            .expect("skeleton_progress");
        assert_eq!(text, "剧情触发：tr-ambush", "旧故事书的提示语逐字不变");
    }

    /// 验收 3（地图 P5 §6.4）：导演即兴遭遇（Intent::Encounter）同样快照场景 / 地点 / 目标；
    /// 临时敌人不计入 template_ids。
    #[tokio::test]
    async fn director_encounter_snapshots_scene_and_goal() {
        let (session, _sink) = session_with_state(
            p5_storybook(json!({ "op": "flag_set", "flag": "never" }), None),
            p5_state(),
        );
        let enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        let st = session.state.lock().unwrap();
        let view: EncounterView =
            serde_json::from_value(st.encounters[&enc].clone()).unwrap();
        assert_eq!(view.scene_id.as_deref(), Some("sc-cave"));
        assert_eq!(view.location_id.as_deref(), Some("loc-cave"));
        assert_eq!(view.goal_id.as_deref(), Some("g-clear"));
        assert_eq!(view.template_ids, vec!["mon-goblin".to_string()]);
        drop(st);

        // 临时敌人（AI 现编）：锚照样快照，但没有图鉴模板。
        let temp = create_encounter(&session, vec![temp_spec("灰狼", 11, 12)]).await;
        let st = session.state.lock().unwrap();
        let view: EncounterView =
            serde_json::from_value(st.encounters[&temp].clone()).unwrap();
        assert!(view.template_ids.is_empty());
        assert_eq!(view.goal_id.as_deref(), Some("g-clear"));
        assert_eq!(view.location_id.as_deref(), Some("loc-cave"));
    }

    /// 验收 5（地图 P5 §6.2）：QuestView 的地点继承所属场景（推导，不落库）；
    /// 章节地点 = 其 scenes 的 location_id 并集（推导，不存冗余字段）。
    #[tokio::test]
    async fn quest_location_is_derived_and_chapters_union_their_scenes() {
        let (session, _sink) = session_with_state(
            p5_storybook(json!({ "op": "flag_set", "flag": "never" }), None),
            p5_state(),
        );
        let quests = session.projection().quests;
        let clear = quests.iter().find(|q| q.id == "g-clear").expect("骨架任务");
        assert_eq!(clear.location_id.as_deref(), Some("loc-cave"));

        // 导演新增的任务不属于任何场景 → 没有可继承的地点。
        session
            .handle_intent(
                Intent::Quest { text: "临时任务".into(), hidden: false, primary: false },
                None,
            )
            .await;
        let gm = session
            .projection()
            .quests
            .into_iter()
            .find(|q| q.text == "临时任务")
            .expect("导演任务");
        assert_eq!(gm.location_id, None);

        // 章节地点：并集推导（骨架里 sc-cave 声明 loc-cave）。
        let chapters = session.chapter_locations();
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].chapter_id, "ch-1");
        assert_eq!(chapters[0].location_ids, vec!["loc-cave".to_string()]);

        // 世界状态里绝没有「章节地点」这种冗余字段（只在场景上声明）。
        assert!(
            session.projection().locations.iter().all(|l| l.get("location_ids").is_none()),
            "地点条目不该被回写派生字段"
        );
    }

    // ============================================================
    // 通用原语补口（GAP-C 只读开放内容 / GAP-F 击败与遭遇结束事件 / GAP-H 标记）
    // ============================================================

    /// GAP-F 的事件脚本：把事件事实记进脚本私有存储，并在敌人被击败时发一次奖励资源。
    /// （资源名叫什么由规则包决定；引擎只认识「给某个资源加个数」这个通用动作。）
    const DEFEAT_EVENT_SOURCE: &str = r#"
local d = host.storage.defeated or {}
if host.event_name == 'enemy_defeated' then
  d[#d + 1] = {
    enemy = host.event_data.enemy,
    encounter = host.event_data.encounter,
    killer = host.event_data.killer
  }
  host.storage.defeated = d
  host.modify_resource('', 'score', 50)
elseif host.event_name == 'encounter_cleared' then
  host.storage.cleared_count = (host.storage.cleared_count or 0) + 1
  host.storage.cleared = {
    encounter = host.event_data.encounter,
    reason = host.event_data.reason,
    count = #host.event_data.enemies
  }
end
"#;

    /// GAP-F 测试故事书：一只图鉴怪 + 一击必杀的技能 + Event 挂载点脚本。
    fn defeat_storybook(event_source: &str) -> Value {
        json!({
            "attribute_dimensions": [
                { "key": "str", "baseline": 10, "modifier_step": 2, "min": 1, "max": 30 }
            ],
            "characters": [{
                "id": "mon-goblin", "name": "地精", "kind": "monster",
                "attributes": { "str": 8 }, "resources": { "res-hp": 7 },
                "skills": ["sk-finish"]
            }],
            "skills": [{
                "id": "sk-finish", "name": "终结", "attribute": "str",
                "check": { "lua": "return { total = 100, margin = 100 }" },
                "effect": { "immediate": [
                    { "kind": "damage", "amount": "99", "resource": "res-hp" } ] }
            }],
            "lua_mounts": [{ "id": "defeat-log", "mount": "event", "source": event_source }]
        })
    }

    fn finish_choice() -> AttackChoice {
        AttackChoice { skill_id: Some("sk-finish".into()), ..Default::default() }
    }

    /// GAP-C 端到端：脚本读「当前角色模板的挂接 → definition」里的加值并
    /// `modify_check('add', N)`，判定总值随之变化；挂接清空后加值消失（数据驱动，非烘死）。
    #[tokio::test]
    async fn lua_reads_open_content_bonus_into_check_total() {
        let source = r#"local ids = host.get_attachment('trait') or {}
for _, id in ipairs(ids) do
  local def = host.get_definition(id)
  local bonus = def and def.fields and tonumber(def.fields.bonus)
  if host.check_attribute == 'dex' and bonus then
    host.modify_check('add', bonus)
  end
end"#;
        let with_attachment = json!({
            "attribute_dimensions": [
                { "key": "dex", "baseline": 10, "modifier_step": 2, "min": 1, "max": 30 }
            ],
            "characters": [{
                "id": "char-a", "name": "米拉", "kind": "pc",
                "attributes": { "dex": 10 },
                "attachments": { "trait": ["tr-trained"] }
            }],
            "definitions": [{
                "id": "tr-trained", "kind": "trait", "name": "受训",
                "fields": { "bonus": "4" },
                "modifiers": [{ "target": "dex", "value": 4 }]
            }],
            "lua_mounts": [{
                "id": "read-open-content", "mount": "check_post_roll", "source": source
            }]
        });
        // 对照组：同一脚本、同一骰序，只是模板上没有挂接 → 读不到 definition，加值不生效。
        let mut without = with_attachment.clone();
        without["characters"][0]["attachments"] = json!({});

        let check = |sb: Value| async move {
            let (session, sink) = session_with(sb);
            session
                .handle_intent(
                    Intent::Check {
                        attribute: "dex".into(),
                        difficulty: Some(1),
                        actor_id: None,
                        opponent_id: None,
                    },
                    None,
                )
                .await;
            last_check(&sink)
        };
        let with_check = check(with_attachment).await;
        let plain_check = check(without).await;
        assert_eq!(with_check.rolls, plain_check.rolls, "同种子骰序不变");
        let plain_roll = plain_check.rolls.as_ref().and_then(|r| r.first().copied()).unwrap_or(0);
        assert_eq!(plain_check.total, plain_roll, "对照组没有加值");
        assert_eq!(with_check.total, plain_check.total + 4, "挂接里的加值进判定总值");
        assert_eq!(with_check.margin, plain_check.margin + 4, "差值同步重算");
    }

    /// GAP-H 端到端：Lua 能把标记置为 false（清除）；旧 SetFlag 缺省置真逐字不变。
    #[tokio::test]
    async fn lua_clear_flag_resets_a_flag_and_default_set_still_true() {
        let sb = json!({
            "lua_mounts": [{
                "id": "toggle",
                "mount": "event",
                "source": "host.set_flag('kept'); host.clear_flag('done')"
            }]
        });
        let mut st = state_with_pc();
        st.flags.insert("done".into(), json!(true));
        let (session, _sink) = session_with_state(sb, st);
        session.dispatch_event("scene");
        let flags = session.state.lock().unwrap().flags.clone();
        assert_eq!(flags.get("kept"), Some(&json!(true)), "缺省仍置真（旧行为逐字不变）");
        assert_eq!(flags.get("done"), Some(&json!(false)), "clear_flag 把标记置为 false");
    }

    /// GAP-F 端到端：导演即兴建的遭遇里，敌人 HP 归零 → 事件在击杀瞬间派发一次
    ///（上下文含 enemy / encounter / killer）；敌人全灭 → 遭遇结束事件派发一次；
    /// 重放同一日志得到同一结论。
    #[tokio::test]
    async fn defeat_and_encounter_end_events_fire_once_with_context_and_replay() {
        let sb = defeat_storybook(DEFEAT_EVENT_SOURCE);
        let (session, _sink) = session_with_state(sb.clone(), state_with_pc());
        // 导演即兴建的遭遇（Intent::Encounter）——GAP-F 的核心诉求正是它也能拿到事件。
        let enc = create_encounter(&session, vec![goblin_spec(2)]).await;
        let score = |s: &Arc<Session>| {
            s.projection().characters["char-a"]["resources"]["score"].as_i64().unwrap_or(0)
        };

        session.strike_enemy("e1".into(), finish_choice(), None).await;
        assert_eq!(score(&session), 50, "击杀一只 → 派发一次击败事件");
        // 已经倒下的敌人再补刀：不得重复派发（每个事件只派发一次）。
        session.strike_enemy("e1".into(), finish_choice(), None).await;
        assert_eq!(score(&session), 50, "同一只敌人只派发一次击败事件");
        session.strike_enemy("e2".into(), finish_choice(), None).await;
        assert_eq!(score(&session), 100, "两只各派发一次");
        assert_eq!(enemy_hp(&session, &enc, 1), 0);

        // 事件上下文：两次击败（enemy / encounter / killer）+ 一次遭遇结束。
        let probe = LuaHostContext { script_id: "defeat-log".into(), ..Default::default() };
        assert!(
            session
                .lua
                .run_condition(
                    r#"local d = host.storage.defeated
return d ~= nil and #d == 2
  and d[1].enemy.id == 'e1' and d[1].enemy.name == '地精'
  and d[1].enemy.template_id == 'mon-goblin' and d[1].enemy.instance_id ~= nil
  and d[2].enemy.id == 'e2'
  and d[1].encounter.name == '遭遇' and d[1].encounter.scene_id == 'sc-1'
  and d[1].encounter.id ~= nil
  and d[1].killer.id == 'char-a' and d[1].killer.name == '米拉'"#,
                    &probe,
                )
                .unwrap(),
            "击败事件必须带上 enemy / encounter / killer 事实"
        );
        assert!(
            session
                .lua
                .run_condition(
                    r#"local c = host.storage.cleared
local d = host.storage.defeated
return host.storage.cleared_count == 1 and c ~= nil
  and c.reason == 'all_down' and c.count == 2
  and c.encounter.id == d[1].encounter.id
  and c.encounter.name == '遭遇'"#,
                    &probe,
                )
                .unwrap(),
            "遭遇全灭 → 派发一次遭遇结束事件"
        );

        // 重放一致：事件派发产生的世界状态（奖励资源）随权威日志重建，不重跑 Lua。
        let persisted = persisted_of(&session);
        let (restarted, _sink2) = session_with_state(sb, state_with_pc());
        restarted.replay(&persisted);
        assert_eq!(
            serde_json::to_value(session.projection()).unwrap(),
            serde_json::to_value(restarted.projection()).unwrap(),
            "重放后投影逐字一致（结论一致、不重复派发）"
        );
        assert_eq!(session.current_seq(), restarted.current_seq());
    }

    /// 没有 event 挂载点脚本的故事书：击杀 / 遭遇结束不产生任何新增事件（零行为变化）。
    #[tokio::test]
    async fn without_event_mount_a_defeat_adds_no_events_or_state() {
        let mut sb = defeat_storybook(DEFEAT_EVENT_SOURCE);
        sb.as_object_mut().unwrap().remove("lua_mounts");
        let (session, sink) = session_with_state(sb, state_with_pc());
        let _enc = create_encounter(&session, vec![goblin_spec(1)]).await;
        let before = sink.0.lock().unwrap().len();
        session.strike_enemy("e1".into(), finish_choice(), None).await;
        let events = sink.0.lock().unwrap().clone();
        let kinds: Vec<String> = events[before..]
            .iter()
            .map(|e| match &e.event {
                PlayEvent::CheckResult(_) => "check".to_string(),
                PlayEvent::Resolution(p) => {
                    format!("resolution:{}", p.outcome.clone().unwrap_or_default())
                }
                PlayEvent::System(p) => format!("system:{}", p.code.clone().unwrap_or_default()),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["check", "resolution:strike", "system:encounter_cleared"],
            "没有 event 挂载点时事件序列逐字不变（零新增）"
        );
        assert!(
            session.projection().characters["char-a"]["resources"].get("score").is_none(),
            "没有脚本就没有任何状态变化"
        );
    }

    /// GAP-A：会话把存档里的角色实例 / 标记 / 活跃遭遇注入 Lua 只读快照，
    /// 实例键 / 模板 id / 角色名 / instance_id 四种写法都命中同一实例。
    #[test]
    fn lua_world_facts_are_injected_from_state() {
        let sb = json!({
            "lua_mounts": [{ "id": "probe", "mount": "pre_resolve", "source": "return 1" }]
        });
        let mut state = state_with_pc();
        state.flags.insert("met-isa".to_string(), json!(true));
        state.characters.insert(
            "mon-1".to_string(),
            CharacterInstance {
                instance_id: "inst-mon-1".into(),
                template_id: "mon-wolf".into(),
                name: "灰狼".into(),
                kind: "monster".into(),
                attributes: json!({ "dex": 40 }).as_object().unwrap().clone(),
                resources: json!({ "hp": 7 }).as_object().unwrap().clone(),
                inventory: Default::default(),
                location_id: Some("loc-1".into()),
                present: true,
                statuses: vec![StatusInstance {
                    id: "off-guard".into(),
                    name: "疏于防备".into(),
                    turns_left: Some(2),
                    scenes_left: None,
                }],
            },
        );
        state.encounters.insert(
            "enc-1".to_string(),
            json!({
                "id": "enc-1", "name": "洞穴", "active": true,
                "enemies": [{ "id": "e1", "name": "灰狼", "hp": 3, "max": 11, "ac": 12 }]
            }),
        );
        let (session, _sink) = session_with_state(sb, state);
        // 挂载点执行路径会刷新只读世界事实（GAP-A）。
        let _ = session.mount_world("char-a");
        let probe = LuaHostContext { script_id: "probe".into(), ..Default::default() };
        for id in ["mon-1", "mon-wolf", "灰狼", "inst-mon-1"] {
            let script = format!(
                "local c = host.get_character('{id}')
                 return c ~= nil and c.id == 'mon-1' and c.attributes.dex == 40 
                 and c.resources.hp == 7 and c.statuses[1].id == 'off-guard' 
                 and c.location_id == 'loc-1' and c.kind == 'monster' and c.present == true"
            );
            assert!(session.lua.run_condition(&script, &probe).unwrap(), "id 形态 {id} 应命中");
        }
        assert!(session.lua.run_condition("return host.get_flag('met-isa') == true", &probe).unwrap());
        assert!(session
            .lua
            .run_condition("return host.get_encounter('enc-1').enemies[1].hp == 3", &probe)
            .unwrap());
        assert!(session
            .lua
            .run_condition(
                "return host.get_character('nobody') == nil and host.get_flag('nope') == nil 
                 and #host.list_encounters() == 1 and host.list_flags()['met-isa'] == true",
                &probe
            )
            .unwrap());
    }

    /// 验收 5：没有相关挂载点脚本时不构造、不注入世界事实（零开销、零行为变化）。
    #[test]
    fn lua_world_facts_are_not_injected_without_mount_scripts() {
        let (session, _sink) = session_with(json!({}));
        // 即便走一次挂载点世界快照构造，注册表为空 → 不注入。
        let _ = session.mount_world("char-a");
        let probe = LuaHostContext { script_id: "probe".into(), ..Default::default() };
        assert!(session
            .lua
            .run_condition(
                "return host.get_character('char-a') == nil and host.get_flag('met-isa') == nil 
                 and next(host.list_flags()) == nil and #host.list_encounters() == 0",
                &probe
            )
            .unwrap());
    }

    /// 验收 4：只读世界事实口在挂载点链里不产生任何 LuaRequest、不改世界状态。
    #[test]
    fn readonly_world_fact_mount_produces_no_requests_or_state_change() {
        let sb = json!({
            "lua_mounts": [{
                "id": "ro", "mount": "pre_resolve",
                "source": "local c = host.get_character('char-a'); local f = host.get_flag('k'); local e = host.get_encounter('nope'); local l = host.list_encounters()"
            }]
        });
        let mut state = state_with_pc();
        state.flags.insert("k".to_string(), json!(1));
        let (session, _sink) = session_with_state(sb, state);
        let before = serde_json::to_value(&*session.state.lock().unwrap()).unwrap();
        let ctx = LuaHostContext {
            script_id: "ro".into(),
            actor_id: "char-a".into(),
            ..Default::default()
        };
        let requests = session.run_mount_chain(LuaMount::PreResolve, &ctx, None, None).unwrap();
        assert!(requests.is_empty(), "只读 API 不得产生任何 LuaRequest");
        let after = serde_json::to_value(&*session.state.lock().unwrap()).unwrap();
        assert_eq!(before, after, "只读 API 不得改世界状态");
    }

    /// GAP-L 端到端（会话级）：规则包在 `check_pre_roll` 按**目标的状态**声明取高，
    /// 走完整 resolve_skill 管线后实测多掷一颗骰（骰序 / RNG 消耗可观察）。
    #[test]
    fn target_status_drives_check_mount_end_to_end() {
        let sb = json!({
            "skills": [{ "id": "sk-hit", "name": "攻击", "check": { "dice": "1d20" } }],
            "world": { "check": { "dice": "1d20", "default_dc": 12 } },
            "lua_mounts": [{
                "id": "target-status", "mount": "check_pre_roll",
                "source": "local t = host.target\nfor i = 1, #(t.statuses or {}) do\n  if t.statuses[i].id == 'off-guard' then host.modify_check('keep_high') end\nend"
            }]
        });
        let monster = |marked: bool| CharacterInstance {
            instance_id: "inst-mon-1".into(),
            template_id: "mon-wolf".into(),
            name: "灰狼".into(),
            kind: "monster".into(),
            attributes: json!({ "dex": 40 }).as_object().unwrap().clone(),
            resources: json!({ "hp": 7 }).as_object().unwrap().clone(),
            inventory: Default::default(),
            location_id: None,
            present: true,
            statuses: if marked {
                vec![StatusInstance {
                    id: "off-guard".into(),
                    name: "疏于防备".into(),
                    turns_left: Some(1),
                    scenes_left: None,
                }]
            } else {
                vec![]
            },
        };
        let rolled = |marked: bool| -> usize {
            let mut state = state_with_pc();
            state.characters.insert("mon-1".to_string(), monster(marked));
            let (session, _sink) = session_with_state(sb.clone(), state);
            let skill = session.rules.skill("sk-hit").cloned().unwrap();
            session.resolve_skill(None, &skill, Some("mon-1"), None);
            session.rng.lock().unwrap().consumed.len()
        };
        // 目标带「疏于防备」→ 脚本声明取高 = 同一骰式掷两次。
        assert_eq!(rolled(true), 2, "按目标状态声明取高应多掷一颗");
        // 对照：目标没有该状态 → 单次掷骰，行为与不声明挂载点一致。
        assert_eq!(rolled(false), 1);
    }
}

