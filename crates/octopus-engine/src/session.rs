//! 会话（#03 核心循环 / #17 事件发射 / #24 回合并发与确认门）。
//!
//! 一个存档一个 Session：内存权威状态 + 确定性 RNG + 演出流出口 + AI 端口。
//! 回合串行（#24 ④）：非 idle 提交返回 `RoundInProgress`。

use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
};

use octopus_types::{
    ActorRef, CheckKind, CheckResultPayload, CheckerDef, CondExpr, ConfirmDecision, DeltaDomain, DeltaOp,
    DialoguePayload,
    EmotePayload, EventEnvelope, FocusEntity, HistoryPage, Intent, NarrativeOverride, NarratePayload, PendingPayload, PhasePayload,
    PhaseStage, PlayEvent, RejectionCode, ResolutionPayload, ResolutionStatus, RoundChannel,
    EncounterView, EnemyView, QuestView, RoundEndPayload, RoundInput, RoundStartPayload, ScenePayload, Seq,
    SkillDef, StateDelta, StateUpdatePayload, StatusUnit,
    EffectTrigger, StatusDef, StatusInstance, SuccessLevel, SystemLevel, SystemPayload,
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
    ports::{AiOutput, AiSlot, EventSink, LoreView, ModelRef, NarrativeView, PersonaView, SceneBrief, TurnContext},
    protocol::ProtocolSpec,
    resolve::ModifierProfile,
    rng::DeterministicRng,
    state::WorldState,
    storage::{now_iso, PersistedEvent},
};

struct Pending {
    action_id: String,
    tx: oneshot::Sender<ConfirmDecision>,
}

/// 抽出主线 AI 本回合的叙事文本（旁白/台词/神态），交给角色 AI 接着演。
fn story_intent_text(intents: &[Intent]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for intent in intents {
        let content = match intent {
            Intent::Narrate { content, .. }
            | Intent::Speak { content, .. }
            | Intent::Emote { content, .. } => content,
            _ => continue,
        };
        let t = content.trim();
        if !t.is_empty() {
            parts.push(t.to_string());
        }
    }
    parts.join("\n")
}

/// 单回合最多注入多少个人物的人格档案（防止上下文被设定撑爆）。
const PERSONA_LIMIT: usize = 6;

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

pub struct Session {
    pub save_id: String,
    state: Mutex<WorldState>,
    /// 开档时的世界状态（重放日志之前）：回滚时以它为基线重放保留的事件。
    base_state: WorldState,
    rng: Arc<Mutex<DeterministicRng>>,
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
    /// 本存档使用的模型（None = 全局角色默认）。
    model: Mutex<Option<ModelRef>>,
    /// 每回合 token 预算（0 = 不限）：裁剪 lore 等可选注入。
    token_budget: AtomicU32,
    /// 存档级叙述段玩家偏好（section id → 开关 | 变体 key）；只作用于之后的回合。
    narrative_overrides: Mutex<std::collections::BTreeMap<String, NarrativeOverride>>,
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
        let rng = Arc::new(Mutex::new(DeterministicRng::new(seed)));
        // Lua 宿主与引擎共享同一 RNG 序列（#12 ④：engine_rng 走确定性序列）。
        let lua = LuaHost::with_rng(rng.clone(), SandboxLimits::default())
            .expect("初始化 Lua 沙箱宿主失败");
        let base_state = state.clone();
        Self {
            save_id,
            state: Mutex::new(state),
            base_state,
            rng,
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

    // ---------- 事件 ----------

    fn emit(&self, event: PlayEvent, actor: Option<ActorRef>, intent_id: Option<String>) -> EventEnvelope {
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
        let mut max_seq: Seq = 0;
        let mut max_round: u32 = 0;
        {
            let mut st = self.state.lock().expect("state poisoned");
            let mut log = self.event_log.lock().expect("event log poisoned");
            let mut reqs = self.request_ids.lock().expect("request ids poisoned");
            for p in persisted {
                let env = &p.envelope;
                apply_event(&mut st, &env.event);
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
        }
        self.seq.store(max_seq, Ordering::SeqCst);
        self.round.store(max_round, Ordering::SeqCst);
    }

    /// 等待此前 emit 的事件全部落库（回滚前保证读写一致）。
    pub async fn flush_events(&self) {
        self.sink.flush().await;
    }

    /// 是否空闲到可以回滚：没有进行中的回合，也没有待确认动作。
    pub fn can_rewind(&self) -> bool {
        !self.busy.load(Ordering::SeqCst)
            && self.pending.lock().map(|p| p.is_none()).unwrap_or(false)
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

    /// 回滚到 from_seq 之前：世界状态以开档基线重放保留的事件重建，事件日志就地截断。
    /// seq 计数器**不复位**（重跑事件用新 seq，避免与旧广播 / 归档冲突）；
    /// round 回退到 round-1，让重跑复用同一回合号。
    pub fn apply_rewind(&self, from_seq: Seq, round: u32) {
        {
            let mut st = self.state.lock().expect("state poisoned");
            *st = self.base_state.clone();
            let mut log = self.event_log.lock().expect("event log poisoned");
            log.retain(|e| e.seq < from_seq);
            for env in log.iter() {
                apply_event(&mut st, &env.event);
            }
            st.seq = self.seq.load(Ordering::SeqCst);
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

    /// 本存档使用的模型（None = 全局角色默认）。
    pub fn model(&self) -> Option<ModelRef> {
        self.model.lock().ok().and_then(|m| m.clone())
    }

    /// 设置本存档使用的模型；None 表示回落到全局角色默认。
    pub fn set_model(&self, model: Option<ModelRef>) {
        if let Ok(mut slot) = self.model.lock() {
            *slot = model;
        }
    }

    pub fn set_auto_confirm(&self, v: bool) {
        self.auto_confirm.store(v, Ordering::SeqCst);
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
            token_budget,
            story_narration: None,
            focus,
            canon: self.canon_lines(8),
            quests: live.quests,
            encounters: live.encounters,
            scenes: self.scene_briefs(),
            model: self.model(),
            protocol: self.rules.protocol_spec(),
        };

        self.phase(PhaseStage::StoryThinking, None);
        // 每回合取一次当前 provider：配置热替换后下一个回合立即生效。
        let ai = self.ai.read().expect("ai poisoned").clone();
        let story_out = ai.story_intents(&ctx).await?;
        if let Some(reasoning) = story_out.reasoning.clone() {
            self.emit_reasoning("story_thinking", reasoning);
        }
        self.emit_intent_warnings(&story_out.intent_warnings);
        let story = story_out.intents;
        // 主线 AI 本回合的叙事：稍后并入角色 AI 的上下文，避免两个 AI 重描同一段场景。
        let story_narration = story_intent_text(&story);
        // 导演通道：人代替 GM 推进剧情，事件归「故事本身」；不跑角色 AI。
        let is_gm = ctx.channel == RoundChannel::Gm;
        let story_fallback = if is_gm { Some(story_actor()) } else { self.controlled_actor() };
        // 输入里点名的武器（伤害/命中加值）：AI 漏给 skill_id 时也要用上
        let actor_key = self.controlled_actor().map(|a| a.id.clone()).unwrap_or_default();
        let text_choice = self.attack_choice_in_text(&input.text, &actor_key);
        let mut struck = false;
        for intent in story {
            // 归属优先级：意图自带的 actor_id → 台词里提到的角色 → 阶段默认。
            // 普通回合默认归玩家角色（是你在行动）；导演回合默认归故事本身。
            let actor = self.resolve_intent_actor(&intent).or_else(|| story_fallback.clone());
            if let Intent::Strike { enemy_id, skill_id } = intent {
                struck = true;
                let mut choice = text_choice.clone();
                if skill_id.is_some() {
                    choice.skill_id = skill_id;
                }
                self.strike_enemy(enemy_id, choice, actor).await;
                continue;
            }
            self.handle_intent(intent, actor).await;
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

        self.phase(PhaseStage::CharacterThinking, None);
        // 角色 AI 看到主线 AI 本回合已叙述的内容：接着演，而不是重复描写。
        let mut char_ctx = ctx.clone();
        if !story_narration.is_empty() {
            char_ctx.story_narration = Some(story_narration);
        }
        let character_out = ai.character_intents(&char_ctx).await?;
        if let Some(reasoning) = character_out.reasoning.clone() {
            self.emit_reasoning("character_thinking", reasoning);
        }
        self.emit_intent_warnings(&character_out.intent_warnings);
        let character = character_out.intents;
        // 里程碑：角色意图暂统一归属首个 NPC（真正的 actor 归属随 #04 意图 actor_id 补全）。
        let npc = {
            let st = self.state.lock().expect("state poisoned");
            st.characters
                .values()
                .find(|c| c.kind != "pc")
                .map(|c| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
        };
        for intent in character {
            let actor = self.resolve_intent_actor(&intent).or_else(|| npc.clone());
            self.handle_intent(intent, actor).await;
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

    /// 按任意一种 id（实例键 / 模板 id / 角色名）找角色。
    fn actor_by_id(&self, id: &str) -> Option<ActorRef> {
        let st = self.state.lock().ok()?;
        let found = st
            .characters
            .get(id)
            .or_else(|| st.characters.values().find(|c| c.template_id == id || c.name == id));
        found.map(|c| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
    }

    /// 从台词/神态文本里认人：取**最长**的名字匹配，避免「诺德罗」被更短的别名抢先。
    fn actor_in_text(&self, text: &str) -> Option<ActorRef> {
        if text.is_empty() { return None }
        let st = self.state.lock().ok()?;
        let mut best: Option<(usize, ActorRef)> = None;
        for c in st.characters.values() {
            if c.name.is_empty() || !text.contains(&c.name) { continue }
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

    async fn handle_intent(&self, intent: Intent, actor: Option<ActorRef>) {
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
                self.run_check(attribute, difficulty.unwrap_or(12), actor).await;
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
                        return;
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
                // 事件广播：declarative triggers + Lua Event 挂载点
                self.dispatch_event("scene_change");
            }
            Intent::QueryWorld { .. } => {}
            Intent::UseSkill { skill_id, target_id } => {
                self.handle_use_skill(&skill_id, target_id.as_deref(), actor);
            }
            Intent::UseItem { item_id, target_id } => {
                self.handle_use_item(&item_id, target_id.as_deref(), actor);
            }
            Intent::FinishTurn => {}
        }
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
        // RNG 消耗进命令日志（#12 ④：重放时直接复用记录值）。
        if !outcome.rng_consumed.is_empty() {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: Some("rng_consume".into()),
                text: serde_json::to_string(&outcome.rng_consumed).unwrap_or_default(),
            }));
        }
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
            .filter(|(_, trigger)| trigger.event == event)
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

    async fn run_check(&self, attribute: String, target: i64, actor: Option<ActorRef>) {
        // 判定归属：意图指定 → 受控角色（是玩家在掷骰）。
        // 绝不回落到种子/测试角色；真找不到人时用中性占位。
        let actor = actor
            .or_else(|| self.controlled_actor())
            .unwrap_or(ActorRef { id: String::new(), name: "未知角色".into() });
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
                return;
            }
            self.phase(PhaseStage::Resolving, None);
        }

        let roll = {
            let mut rng = self.rng.lock().expect("rng poisoned");
            rng.range_inclusive(1, 20)
        };
        let total = roll;
        let margin = total - target;
        let level = if margin >= 10 {
            SuccessLevel::Great
        } else if margin >= 0 {
            SuccessLevel::Success
        } else if margin >= -10 {
            SuccessLevel::Barely
        } else {
            SuccessLevel::Fail
        };
        self.emit(
            PlayEvent::CheckResult(CheckResultPayload {
                intent_id: None,
                actor: actor.clone(),
                attribute: attribute.clone(),
                expr: Some("1d20".into()),
                rolls: Some(vec![roll]),
                r#mod: 0,
                total,
                target,
                margin,
                result: total >= target,
                level,
                opponent: None,
                kind: Some(CheckKind::Attribute),
            }),
            Some(actor.clone()),
            None,
        );

        let ok = total >= target;
        let mut changes = Vec::new();
        if ok {
            let delta = StateDelta {
                domain: octopus_types::DeltaDomain::Flag,
                entity_id: "mine_foreshadow".into(),
                field: "flag".into(),
                op: octopus_types::DeltaOp::Set,
                value: Value::Bool(true),
            };
            changes.push(delta);
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes: changes.clone() }));
        }
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(if ok { "你注意到了些线索。".into() } else { "你什么也没看清。".into() }),
                outcome: None,
                triggered_events: None,
                state_changes: changes,
            }),
            Some(actor),
            None,
        );
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
    use crate::ports::AiProvider;
    use octopus_types::{CharacterInstance, ProjectionMeta};
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
        async fn character_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
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
            Ok(AiOutput { intents: vec![Intent::FinishTurn], reasoning: None, intent_warnings: vec![] })
        }
        async fn character_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput { intents: vec![Intent::FinishTurn], reasoning: None, intent_warnings: vec![] })
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



    struct NarrationCapturingAi(StdMutex<Vec<(String, TurnContext)>>);

    #[async_trait::async_trait]
    impl AiProvider for NarrationCapturingAi {
        async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            self.0.lock().unwrap().push(("story".into(), ctx.clone()));
            Ok(AiOutput { intents: vec![Intent::Narrate { content: "雨云压得极低。".into(), actor_id: None }], reasoning: None, intent_warnings: vec![] })
        }
        async fn character_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            self.0.lock().unwrap().push(("character".into(), ctx.clone()));
            Ok(AiOutput { intents: vec![Intent::FinishTurn], reasoning: None, intent_warnings: vec![] })
        }
    }

    #[tokio::test]
    async fn character_ai_sees_story_narration_from_same_round() {
        let sb = json!({ "world": {} });
        let sink = Arc::new(CaptureSink(StdMutex::new(vec![])));
        let ai = Arc::new(NarrationCapturingAi(StdMutex::new(vec![])));
        let session = Arc::new(Session::new(
            "s".into(),
            state_with_pc(),
            sink as Arc<dyn EventSink>,
            ai_slot(ai.clone() as Arc<dyn AiProvider>),
            true,
            sb,
        ));
        session
            .run_round(
                RoundInput { channel: RoundChannel::Character, text: "我没带伞".into(), refs: vec![] },
                None,
                vec![],
            )
            .await
            .unwrap();
        let captured = ai.0.lock().unwrap();
        let story = &captured.iter().find(|(k, _)| k == "story").expect("story AI ran").1;
        let character = &captured.iter().find(|(k, _)| k == "character").expect("character AI ran").1;
        assert!(story.story_narration.is_none(), "主线 AI 自己不需要已叙述字段");
        assert_eq!(
            character.story_narration.as_deref(),
            Some("雨云压得极低。"),
            "角色 AI 要拿到主线 AI 本回合的叙事，才能接着演而不是重复"
        );
    }

    struct ReasoningAi;

    #[async_trait::async_trait]
    impl AiProvider for ReasoningAi {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![Intent::Narrate { content: "雨。".into(), actor_id: None }],
                reasoning: Some("先想想天气。".into()),
                intent_warnings: vec![],
            })
        }
        async fn character_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![Intent::FinishTurn],
                reasoning: Some("露西该怎么回应。".into()),
                intent_warnings: vec![],
            })
        }
    }

    #[tokio::test]
    async fn reasoning_events_are_emitted_for_both_stages() {
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
        assert!(stages.contains(&"story_thinking"), "主线 AI 的思考要落事件");
        assert!(stages.contains(&"character_thinking"), "角色 AI 的思考要落事件");
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
                intents: vec![Intent::Think { content: "先在心里推演一遍。".into() }],
                reasoning: None,
                intent_warnings: vec![],
            })
        }
        async fn character_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput { intents: vec![Intent::FinishTurn], reasoning: None, intent_warnings: vec![] })
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
    async fn run_round_passes_per_save_model_to_ai() {
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
        // 本存档覆盖模型：应原样进 TurnContext，供 provider 解析。
        session.set_model(Some(ModelRef { provider_id: "p1".into(), model: "m1".into(), reasoning_effort: None }));
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
            ctx.model.as_ref().map(|m| (m.provider_id.as_str(), m.model.as_str())),
            Some(("p1", "m1"))
        );
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
            let intents = if q.is_empty() { vec![] } else { q.remove(0) };
            Ok(AiOutput { intents, reasoning: None, intent_warnings: vec![] })
        }
        async fn character_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput::default())
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
}
