//! 会话（#03 核心循环 / #17 事件发射 / #24 回合并发与确认门）。
//!
//! 一个存档一个 Session：内存权威状态 + 确定性 RNG + 演出流出口 + AI 端口。
//! 回合串行（#24 ④）：非 idle 提交返回 `RoundInProgress`。

use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

use octopus_types::{
    ActorRef, CheckKind, CheckResultPayload, CheckerDef, CondExpr, ConfirmDecision, DeltaDomain, DeltaOp,
    DialoguePayload,
    EmotePayload, EventEnvelope, FocusEntity, HistoryPage, Intent, NarrativeOverride, NarratePayload, PendingPayload, PhasePayload,
    normalize_event_name,
    relationship_endpoint,
    PhaseStage, PlayEvent, RejectionCode, ResolutionPayload, ResolutionStatus, RoundChannel,
    EncounterView, EnemyView, QuestView, RoundEndPayload, RoundInput, RoundStartPayload, ScenePayload, Seq,
    SkillDef, StateDelta, StateUpdatePayload, StatusUnit,
    EffectTrigger, StatusDef, StatusInstance, SystemLevel, SystemPayload,
    ReasoningPayload,
    WorldProjection,
};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

use crate::{
    command::{execute_item_skill, execute_skill, CommandContext},
    conditions::{eval_cond, evaluate_skeleton, goal_delta, trigger_delta, EvalContext},
    effects::{build_status_instance, resolve_immediate, status_delta},
    error::EngineError,
    lua_host::{LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest, SandboxLimits},
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

/// 单回合最多注入多少条「相关往事」（#05 §3.4 的 K=5）。
const MEMORY_TOP_K: usize = 5;

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
        Self { storybook, skills, status_defs, profiles, attribute_bonuses }
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

    pub fn skill(&self, id: &str) -> Option<&SkillDef> {
        self.skills.get(id)
    }

    pub fn global_checker(&self) -> Option<CheckerDef> {
        self.storybook
            .pointer("/world/check")
            .and_then(|v| serde_json::from_value::<CheckerDef>(v.clone()).ok())
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
            lua_registry: Mutex::new(LuaRegistry::new()),
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
    /// 派生数据：检索器内部已做「向量失败 → FTS → 空」的降级；这里再兜一层，
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
    fn emit_raw(&self, event: PlayEvent, actor: Option<ActorRef>, intent_id: Option<String>) -> EventEnvelope {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let env = EventEnvelope {
            id: uuid::Uuid::new_v4().to_string(),
            seq,
            round: self.round.load(Ordering::SeqCst),
            ts: now_iso(),
            actor,
            intent_id,
            event,
        };
        // 唯一的状态变更入口：实时与重放走同一条路径，保证二者结果一致。
        if let Ok(mut st) = self.state.lock() {
            apply_event(&mut st, &env.event);
        }
        if let Ok(mut log) = self.event_log.lock() {
            log.push(env.clone());
        }
        self.sink.emit(env.clone());
        env
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
                for (idx, goal) in goals.iter().enumerate() {
                    let text = goal.get("text").and_then(Value::as_str).unwrap_or("").trim();
                    if text.is_empty() {
                        continue;
                    }
                    let id = goal
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{scene_id}#goal[{idx}]"));
                    let done = progress.get(&id).and_then(Value::as_bool).unwrap_or(false);
                    out.push(QuestView {
                        id,
                        text: text.to_string(),
                        done,
                        source: "skeleton".into(),
                        hidden: goal.get("hidden").and_then(Value::as_bool).unwrap_or(false),
                        primary: goal.get("primary").and_then(Value::as_bool).unwrap_or(false),
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
        let ctx = TurnContext {
            save_id: self.save_id.clone(),
            round,
            scene_id,
            scene_title,
            scene_description,
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
            let out = ai.story_intents(&call_ctx).await?;
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
                if let Intent::Strike { enemy_id, skill_id } = intent {
                    struck = true;
                    let mut choice = text_choice.clone();
                    if skill_id.is_some() {
                        choice.skill_id = skill_id;
                    }
                    self.strike_enemy(enemy_id, choice, actor).await;
                    continue;
                }
                // 只有 query_world / check / interact 会返回「新引擎信息」供下一轮回喂。
                if let Some(info) = self.handle_intent(intent, actor).await {
                    next_feedback.push(info);
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
        let (flags, goals, triggers, actor_attrs, actor_loc) = {
            let st = self.state.lock().expect("state poisoned");
            let actor_id = st.controlled.first().cloned().unwrap_or_default();
            let actor = st.characters.get(&actor_id);
            (
                st.flags.clone(),
                st.progress.goals.clone(),
                st.progress.triggers.clone(),
                actor.map(|c| c.attributes.clone()),
                actor.and_then(|c| c.location_id.clone()),
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
            lua: Some((&self.lua, &lua_ctx)),
        };

        f(&ctx)
    }

    /// 回合末对全部 goal / trigger 求值，把新进展落成权威 StateUpdate 事件。
    fn evaluate_turn_end(&self) {
        let skeleton = self.rules.skeleton().cloned().unwrap_or(Value::Null);
        match self.with_eval_context("turn_end", |ctx| evaluate_skeleton(&skeleton, ctx)) {
            Ok((new_goals, new_triggers)) => {
                if new_goals.is_empty() && new_triggers.is_empty() {
                    return;
                }
                let mut changes = Vec::new();
                for id in &new_goals {
                    changes.push(goal_delta(id));
                }
                for id in &new_triggers {
                    changes.push(trigger_delta(id));
                }
                self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
                let mut parts = Vec::new();
                if !new_goals.is_empty() {
                    parts.push(format!("目标达成：{}", new_goals.join("、")));
                }
                if !new_triggers.is_empty() {
                    parts.push(format!("剧情触发：{}", new_triggers.join("、")));
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

    /// 切换到骨架里的目标场景：把在场名单重置为该场景声明（present_char_ids + 受控角色），
    /// 并以 Scene 事件记录场景元信息。此前只发了一条 Resolution，场景实际没有变化。
    fn switch_scene(&self, scene_id: &str) {
        let Some((title, description, present_ids)) = self.scene_def(scene_id) else {
            return;
        };
        let (present, deltas) = {
            let st = self.state.lock().expect("state poisoned");
            let mut present = Vec::new();
            let mut deltas = Vec::new();
            for (key, c) in &st.characters {
                let on =
                    present_ids.contains(&c.template_id) || st.controlled.contains(key) || st.controlled.contains(&c.instance_id);
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
            title,
            description,
            present,
        }));
    }

    /// 从骨架取场景定义：(标题, 描述, 在场模板 id 集合)。
    fn scene_def(
        &self,
        scene_id: &str,
    ) -> Option<(String, Option<String>, std::collections::HashSet<String>)> {
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
                let present_ids = scene
                    .get("present_char_ids")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                return Some((title, description, present_ids));
            }
        }
        None
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

    /// 有效属性值（基础 + 挂接/装备修正），与 command 管线同口径。
    fn effective_attribute(&self, key: &str, attribute: &str) -> f64 {
        let (raw, template) = {
            let st = self.state.lock().expect("state poisoned");
            let c = st
                .characters
                .get(key)
                .or_else(|| st.characters.values().find(|c| c.template_id == key));
            match c {
                Some(c) => (
                    c.attributes.get(attribute).and_then(Value::as_f64).unwrap_or(0.0),
                    c.template_id.clone(),
                ),
                None => (0.0, String::new()),
            }
        };
        self.rules
            .attribute_bonuses()
            .get(&template)
            .and_then(|m| m.get(attribute))
            .map(|b| b.apply(raw))
            .unwrap_or(raw)
    }

    /// 结算一次对遭遇内敌人的攻击：**命中与伤害全由引擎算**，AI 只叙事。
    async fn strike_enemy(&self, enemy_id: String, choice: AttackChoice, actor: Option<ActorRef>) {
        let actor = actor
            .or_else(|| self.controlled_actor())
            .unwrap_or(ActorRef { id: String::new(), name: "未知角色".into() });

        // 1) 找遭遇与敌人（enemy_id 认 id，也认名字，方便「我砍头狼」）
        let found = {
            let st = self.state.lock().expect("state poisoned");
            let mut hit: Option<(String, Value, usize)> = None;
            for (enc_id, enc) in st.encounters.iter() {
                if !enc.get("active").and_then(Value::as_bool).unwrap_or(true) {
                    continue;
                }
                let Some(list) = enc.get("enemies").and_then(Value::as_array) else { continue };
                for (i, e) in list.iter().enumerate() {
                    let eid = e.get("id").and_then(Value::as_str).unwrap_or("");
                    let name = e.get("name").and_then(Value::as_str).unwrap_or("");
                    if eid == enemy_id || (!enemy_id.is_empty() && name == enemy_id) {
                        hit = Some((enc_id.clone(), enc.clone(), i));
                        break;
                    }
                }
                if hit.is_some() {
                    break;
                }
            }
            hit
        };
        let Some((enc_id, enc, idx)) = found else {
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
        let hp_before = enc
            .pointer(&format!("/enemies/{idx}/hp"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        // 难度 = 该敌人的防御值（没有就用 12）
        let difficulty = enc
            .pointer(&format!("/enemies/{idx}/ac"))
            .and_then(Value::as_i64)
            .filter(|a| *a > 0)
            .unwrap_or(12);

        // 2) 命中判定：技能自带 checker 优先，否则默认 1d20 vs 12
        let skill = choice.skill_id.as_deref().and_then(|id| self.rules.skill(id).cloned());
        let skill_name = skill
            .as_ref()
            .map(|s| s.name.clone())
            .or_else(|| choice.label.clone())
            .unwrap_or_else(|| "徒手攻击".into());
        let checker = match skill.as_ref().and_then(|s| s.check.as_ref()) {
            Some(octopus_types::SkillCheck::Def(c)) => c.clone(),
            _ => CheckerDef { dice: Some("1d20".into()), ..Default::default() },
        };
        let attribute = "str".to_string();
        let value = self.effective_attribute(&actor.id, &attribute);
        let profile = self
            .rules
            .profiles()
            .get(&attribute)
            .copied()
            .unwrap_or_default();
        let resolved = {
            let mut rng = self.rng.lock().expect("rng poisoned");
            crate::resolve::resolve_declarative_check(&checker, &attribute, value, difficulty, profile, &mut rng)
        };
        let Ok(mut check) = resolved else {
            self.reject("攻击判定失败".into(), RejectionCode::RuleViolation);
            return;
        };
        // 武器自带的命中加值（物品 modifiers 里 target=attack 的 add 之和）
        if choice.bonus != 0 {
            crate::resolve::apply_check_bonus(
                &mut check,
                choice.bonus,
                checker.mode.unwrap_or(octopus_types::CheckMode::Gte),
                crate::resolve::degree_thresholds(&checker),
            );
        }

        // 3) 伤害：技能声明的伤害骰优先，否则 1d6；最低 1 点
        let damage_expr = skill
            .as_ref()
            .and_then(|s| s.effect.as_ref())
            .and_then(|e| e.immediate.as_ref())
            .and_then(|list| {
                list.iter().find_map(|f| match f {
                    octopus_types::ImmediateEffect::Damage { amount, .. } => Some(amount.clone()),
                    _ => None,
                })
            })
            .or_else(|| choice.damage.clone())
            .unwrap_or_else(|| "1d6".to_string());
        let harm = if check.total >= difficulty {
            let dmg = {
                let mut rng = self.rng.lock().expect("rng poisoned");
                crate::resolve::roll_dice(&damage_expr, &mut rng).map(|r| r.total).unwrap_or(1)
            };
            dmg.max(1)
        } else {
            0
        };
        let hp_after = (hp_before - harm).max(0);

        // 4) 写回敌人 HP（遭遇局部更新）
        let mut list = enc
            .get("enemies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(entry) = list.get_mut(idx).and_then(Value::as_object_mut) {
            entry.insert("hp".into(), Value::from(hp_after));
        }
        let all_down = list
            .iter()
            .all(|e| e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0);

        let mut changes = vec![StateDelta {
            domain: DeltaDomain::Encounter,
            entity_id: enc_id,
            field: "encounter".into(),
            op: DeltaOp::Set,
            value: serde_json::json!({ "enemies": list }),
        }];
        let die = check.rolls.first().copied().unwrap_or(0);
        let verdict = format!(
            "命中判定 {total}（骰 {die} + 修正 {m}）",
            total = check.total,
            die = die,
            m = check.r#mod,
        );
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
        if all_down {
            changes.push(StateDelta {
                domain: DeltaDomain::Encounter,
                entity_id: String::new(),
                field: "encounter".into(),
                op: DeltaOp::Set,
                value: Value::Null,
            });
        }
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(narrative),
                outcome: Some("strike".into()),
                triggered_events: None,
                state_changes: changes
                    .into_iter()
                    .filter(|c| !c.entity_id.is_empty())
                    .collect(),
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
            .filter(|c| c.kind != "pc" && c.present)
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
        if text.is_empty() { return None }
        let st = self.state.lock().ok()?;
        let mut best: Option<(usize, ActorRef)> = None;
        for c in st.characters.values() {
            if c.name.is_empty() || !text.contains(&c.name) { continue }
            // 玩家角色（PC）不由 AI 代说：意图文本里出现的「你 / PC 名」是**称呼**，不是说话人。
            // 否则 PC 名恰好是「你」时，「你带伞了吗？」这类 NPC 台词会被判给玩家。
            if c.kind == "pc" { continue }
            let len = c.name.chars().count();
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((len, ActorRef { id: c.template_id.clone(), name: c.name.clone() }));
            }
        }
        best.map(|(_, a)| a)
    }

    /// 意图归属：① 意图自带的 actor_id；② 台词里提到的角色名；③ None（调用方回落）。
    fn resolve_intent_actor(&self, intent: &Intent) -> Option<ActorRef> {
        let (id, content) = match intent {
            Intent::Speak { actor_id, content, .. } => (actor_id.clone(), content.as_str()),
            Intent::Emote { actor_id, content, .. } => (actor_id.clone(), content.as_str()),
            Intent::Check { actor_id, .. } => (actor_id.clone(), ""),
            Intent::Narrate { actor_id, .. } => (actor_id.clone(), ""),
            _ => (None, ""),
        };
        if let Some(a) = id.and_then(|i| self.actor_by_id(&i)) {
            return Some(a);
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
            Intent::Check { attribute, difficulty, .. } => {
                return self.run_check(attribute, difficulty, actor).await;
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
            Intent::Encounter { name, enemies, note } => {
                // 3a：只建结构与 HP；先攻与回合限制见后续。
                let id = format!("enc-{}", uuid::Uuid::new_v4().simple());
                let list: Vec<EnemyView> = enemies
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let hp = e.hp.filter(|h| *h > 0).unwrap_or(10);
                        let ac = e.ac.filter(|a| *a > 0).unwrap_or(12);
                        EnemyView { id: format!("e{}", i + 1), name: e.name, hp, max: hp, ac }
                    })
                    .collect();
                let view = EncounterView { id: id.clone(), name, enemies: list, note, active: true };
                self.emit(
                    PlayEvent::StateUpdate(StateUpdatePayload {
                        changes: vec![StateDelta {
                            domain: DeltaDomain::Encounter,
                            entity_id: id,
                            field: "encounter".into(),
                            op: DeltaOp::Add,
                            value: serde_json::to_value(view).unwrap_or(Value::Null),
                        }],
                    }),
                    Some(story_actor()),
                    None,
                );
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
            Intent::Move { destination_id } => {
                let controlled = self
                    .state
                    .lock()
                    .expect("state poisoned")
                    .controlled
                    .first()
                    .cloned();
                if let Some(c) = controlled {
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

    /// 走 command 管线结算一次技能/物品使用，并把结果落成权威事件。
    fn resolve_skill(
        &self,
        item_id: Option<&str>,
        skill: &SkillDef,
        target_id: Option<&str>,
        actor: Option<ActorRef>,
    ) {
        let (actor_id, actor_json, scene_id) = {
            let st = self.state.lock().expect("state poisoned");
            let id = st.controlled.first().cloned().unwrap_or_default();
            let json = st
                .characters
                .get(&id)
                .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            (id, json, st.scene_id.clone())
        };
        if actor_id.is_empty() {
            self.reject("没有受控角色".into(), RejectionCode::ActorNotFound);
            return;
        }
        let target_json = target_id.and_then(|t| {
            self.state
                .lock()
                .ok()
                .and_then(|st| st.characters.get(t).and_then(|c| serde_json::to_value(c).ok()))
        });
        let global_checker = self.rules.global_checker();
        let difficulty = 12;
        // 技能未声明判定属性，v1 以力量为默认维度。
        let lua_ctx = LuaHostContext {
            script_id: format!("skill:{}", skill.id),
            actor_id: actor_id.clone(),
            actor: actor_json.clone(),
            target_id: target_id.map(str::to_string),
            target: target_json.clone(),
            skill: serde_json::to_value(skill).ok(),
            scene_id,
            round: self.round.load(Ordering::SeqCst),
            difficulty: Some(difficulty),
            relationships: vec![],
            present: vec![],
            controlled: String::new(),
        };
        let registry = self.lua_registry.lock().expect("lua registry poisoned");
        let mut ctx = CommandContext {
            actor_id: &actor_id,
            actor: &actor_json,
            target_id,
            target: target_json.as_ref(),
            difficulty,
            attribute: Some("str".to_string()),
            global_checker: global_checker.as_ref(),
            rng: &self.rng,
            lua: Some((&self.lua, &lua_ctx)),
            registry: Some(&*registry),
            status_defs: Some(self.rules.status_defs()),
            profiles: Some(self.rules.profiles()),
            attribute_bonuses: Some(self.rules.attribute_bonuses()),
        };
        let result = match item_id {
            Some(id) => execute_item_skill(id, skill, &mut ctx),
            None => execute_skill(skill, &mut ctx),
        };
        let outcome = match result {
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
        let actor_ref = actor.clone().unwrap_or_else(|| ActorRef {
            id: actor_id.clone(),
            name: actor_json
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&actor_id)
                .to_string(),
        });
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
                Some(actor_ref),
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
            actor,
            None,
        );
        // Lua 钩子的写请求（消耗 / 施加与移除状态 / 触发事件）真正落到世界状态。
        self.apply_lua_requests(&outcome.requests, &actor_id);
    }

    /// 回合开始 tick：turns 单位的持续状态减 1，归零移除（#12 ④）。
    fn tick_statuses_turn(&self) {
        self.tick_statuses(true);
    }

    /// 场景切换 tick：scenes 单位的持续状态减 1。
    fn tick_statuses_scene(&self) {
        self.tick_statuses(false);
    }

    fn tick_statuses(&self, turn: bool) {
        let status_defs = self.rules.status_defs();
        let mut changes: Vec<StateDelta> = Vec::new();
        {
            let st = self.state.lock().expect("state poisoned");
            for (char_id, c) in st.characters.iter() {
                for status in &c.statuses {
                    let left = if turn { status.turns_left } else { status.scenes_left };
                    let Some(left) = left else {
                        continue;
                    };
                    // 先结算状态自身的持续效果（#12 ③）：每经过一个 duration 单位结算一次。
                    if let Some(def) = status_defs.get(&status.id) {
                        if let Some(effects) = def.effect.as_ref().filter(|e| !e.is_empty()) {
                            let resolved = {
                                let mut rng = self.rng.lock().expect("rng poisoned");
                                resolve_immediate(effects, char_id, &mut rng)
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
                            entity_id: char_id.clone(),
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
                            entity_id: char_id.clone(),
                            field: "status".into(),
                            op: DeltaOp::Set,
                            value: serde_json::to_value(next).unwrap_or(Value::Null),
                        });
                    }
                }
            }
        }
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
        // 状态 tick 也可能掷骰（每 tick 效果）：即使没有状态变更也要补记消耗（#06 ②）。
        self.flush_rng_consumption();
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

        let (flags, goals, prog_triggers, actor_id, actor_attrs, actor_loc) = {
            let st = self.state.lock().expect("state poisoned");
            let actor_id = st.controlled.first().cloned().unwrap_or_default();
            let actor = st.characters.get(&actor_id);
            (
                st.flags.clone(),
                st.progress.goals.clone(),
                st.progress.triggers.clone(),
                actor_id,
                actor.map(|c| c.attributes.clone()),
                actor.and_then(|c| c.location_id.clone()),
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
        let has_scripts = self
            .lua_registry
            .lock()
            .map(|registry| registry.for_mount(LuaMount::Event).next().is_some())
            .unwrap_or(false);
        if !has_scripts {
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
            ..Default::default()
        };
        let outcome = {
            let registry = self.lua_registry.lock().expect("lua registry poisoned");
            registry.run_chain(&self.lua, LuaMount::Event, &lua_ctx)
        };
        match outcome {
            // Lua 事件脚本的写请求同样落状态（此前被直接丢弃）。
            Ok(_) => {
                let requests = self.lua.drain_requests();
                self.apply_lua_requests(&requests, &controlled);
            }
            Err(e) => {
                // fail-fast：报错即不落该链的任何请求（与命令侧一致）。
                let _ = self.lua.drain_requests();
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Error,
                    code: Some("event_lua_error".into()),
                    text: e.to_string(),
                }));
            }
        }
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
            }
        }
        if !changes.is_empty() {
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes }));
        }
        for event in events {
            self.dispatch_event(&event);
        }
    }

    /// 结算一次判定。成功结算时返回结果摘要，供回合内续轮回喂模型（#04 ⑦）；
    /// 取消 / 出错没有可续写的信息，返回 None。
    async fn run_check(&self, attribute: String, difficulty: Option<i64>, actor: Option<ActorRef>) -> Option<String> {
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
                    description: format!("用{attribute}进行一次判定"),
                    impact: Some("会消耗一次行动机会".into()),
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
        let target = difficulty
            .or_else(|| declared.as_ref().and_then(|c| c.default_dc))
            .unwrap_or(12);
        let resolved = match &declared {
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
                    scene_id: self.state.lock().map(|st| st.scene_id.clone()).unwrap_or_default(),
                    round: self.round.load(Ordering::SeqCst),
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
                // 缺省路径与历史逐字一致：1d20，无修正，默认成功度阈值。
                let roll = {
                    let mut rng = self.rng.lock().expect("rng poisoned");
                    rng.range_inclusive(1, 20)
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
                opponent: None,
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
        Some(format!(
            "判定「{}」：{level}（骰 {dice}，修正 {}，总值 {}，难度 {}）",
            resolved.attribute, resolved.r#mod, resolved.total, resolved.target
        ))
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
            let Some(c) = state.characters.get_mut(&key) else {
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
            let cur = map.get(key).and_then(Value::as_f64).unwrap_or(0.0);
            let add = d.value.as_f64().unwrap_or(0.0);
            map.insert(key.to_string(), Value::from(cur + add));
        }
        DeltaOp::Set => {
            map.insert(key.to_string(), d.value.clone());
        }
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

    struct CapturingAi(StdMutex<Vec<TurnContext>>);

    #[async_trait::async_trait]
    impl AiProvider for CapturingAi {
        async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            self.0.lock().unwrap().push(ctx.clone());
            Ok(AiOutput { intents: vec![Intent::FinishTurn.into()], reasoning: None, intent_warnings: vec![] })
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

    struct ReasoningAi;

    #[async_trait::async_trait]
    impl AiProvider for ReasoningAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![Intent::Narrate { content: "雨。".into(), actor_id: None }.into()],
                reasoning: Some("先想想天气。".into()),
                intent_warnings: vec![],
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
            Ok(AiOutput { intents, reasoning: None, intent_warnings: vec![] })
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
                Intent::Check { attribute: "str".into(), difficulty: Some(10), actor_id: None },
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
                Intent::Check { attribute: "str".into(), difficulty: Some(12), actor_id: None },
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
                Intent::Check { attribute: "dexterity".into(), difficulty: None, actor_id: None },
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
                Intent::Check { attribute: "agi".into(), difficulty: None, actor_id: None },
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
                Intent::Check { attribute: "str".into(), difficulty: Some(30), actor_id: None },
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
                Intent::Check { attribute: "str".into(), difficulty: Some(-100), actor_id: None },
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

}


