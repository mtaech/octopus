//! command：意图结算管线（#04 四阶段 / #20 编排层）。
//!
//! 唯一编排者：Validate -> Resolve -> Commit -> Feedback 收敛在这里。
//! 目前覆盖机制类意图（use_skill / use_item）；叙事意图不进本模块。
//! Resolve 阶段按顺序跑 Lua 挂载点：CheckPreRoll -> 掷骰 -> CheckPostRoll
//! -> PreResolve（含技能自身 lua 钩子）-> 核心效果 -> PostResolve。
//!
//! RNG 以可锁句柄传入（而非 &mut）：声明式掷骰时才短暂加锁，调 Lua 前必须释放，
//! 否则 Lua 的 host.engine_rng 会重入同一把锁而死锁。消耗按 #12 ④ 记录进
//! CommandOutcome，供命令日志 rng_consume 重放。

use std::collections::HashMap;
use std::sync::Mutex;

use octopus_types::{
    CheckKind, CheckerDef, CheckMode, CondExpr, DeltaDomain, DeltaOp, RejectionCode, SkillCheck,
    SkillDef, StateDelta, StatusDef, StatusInstance,
};
use serde_json::Value;

use crate::effects::{resolve_effect, EffectResolution, EffectScale};
use crate::error::EngineError;
use crate::modifiers::AttrModifier;
use crate::lua_host::{
    CheckModifier, LuaCheckContext, LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest,
    MountEnv,
};
use crate::resolve::{
    apply_check_bonus, apply_forced_result, checker_dice, compare, degree_thresholds,
    level_for_margin, resolve_checker, resolve_lua_check, roll_dice, ModifierProfile,
    ResolvedCheck, DEFAULT_BASELINE,
};
use crate::rng::DeterministicRng;

/// 一次机制结算的只读输入 + RNG 句柄 + Lua 运行时。
pub struct CommandContext<'a> {
    pub actor_id: &'a str,
    /// 角色实例 JSON（CharacterInstance 形态）。
    pub actor: &'a Value,
    pub target_id: Option<&'a str>,
    pub target: Option<&'a Value>,
    pub difficulty: i64,
    /// 判定所用属性维度 key。
    pub attribute: Option<String>,
    /// 全局判定器（world.check）：技能 check 引用全局时使用。
    pub global_checker: Option<&'a CheckerDef>,
    /// 可锁 RNG 句柄（与 LuaHost.engine_rng 共享同一序列）。
    pub rng: &'a Mutex<DeterministicRng>,
    /// Lua 宿主 + 本轮上下文（可选；无 Lua 脚本时可为 None）。
    pub lua: Option<(&'a LuaHost, &'a LuaHostContext)>,
    /// 挂载点脚本注册表（可选）。
    pub registry: Option<&'a LuaRegistry>,
    /// 故事书顶层持续状态声明表（effect.status 按 id 引用解析）。
    pub status_defs: Option<&'a HashMap<String, StatusDef>>,
    /// 各属性维度的修正配置（基线 / 范围 / 步长）；缺省走默认。
    pub profiles: Option<&'a HashMap<String, ModifierProfile>>,
    /// 角色模板 → 属性修正（挂接定义 + 已装备物品）。
    pub attribute_bonuses: Option<&'a HashMap<String, HashMap<String, AttrModifier>>>,
    /// 调用方追加的固定判定修正（武器命中加值 / 临时加值）——通用原语，不含任何规则集语义。
    pub extra_bonus: i64,
    /// 效果是否必须判定成功才结算（命中门）——通用原语；缺省沿用「结果交叙事」的既有语义。
    pub effect_requires_success: bool,
    /// 挂载点 `when` 条件闸门（世界快照由调用方——session——组装）。
    /// None = 声明了 `when` 的挂载点脚本一律跳过（不瞎跑）。
    pub mount_gate: Option<&'a dyn Fn(&CondExpr) -> bool>,
}

/// 判定修正累加器（通用原语）：挂载点在判定前 / 后收集，引擎负责应用。
///
/// 引擎只认识「掷两次取高/低」「加值」「改难度」「覆盖结果」这四个动作；
/// 「什么时候加、加多少、何时强制成败」全在 Lua 脚本里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CheckAdjustments {
    /// 取高 / 取低的净次数（互相抵消）：正 = 掷两次取高，负 = 取低，0 = 单次。
    pub keep: i64,
    /// 固定加值合计（正负均可）。
    pub add: i64,
    /// 难度修正合计（正数更难）。
    pub dc: i64,
    /// 结果覆盖（判定 C4）：Some(true) = 强制成功，Some(false) = 强制失败；
    /// 同一个判定里多次覆盖以**最后一条**为准（脚本顺序即优先级）。
    pub force: Option<bool>,
}

impl CheckAdjustments {
    /// 吸收一批 Lua 请求里的判定修正，返回其余请求（顺序不变）。
    pub(crate) fn absorb(&mut self, requests: Vec<LuaRequest>) -> Vec<LuaRequest> {
        let mut rest = Vec::with_capacity(requests.len());
        for req in requests {
            match req {
                LuaRequest::ModifyCheck { mode, amount } => match mode {
                    CheckModifier::KeepHigh => self.keep += 1,
                    CheckModifier::KeepLow => self.keep -= 1,
                    CheckModifier::Add => self.add += amount,
                    CheckModifier::Difficulty => self.dc += amount,
                    CheckModifier::ForceSuccess => self.force = Some(true),
                    CheckModifier::ForceFail => self.force = Some(false),
                },
                other => rest.push(other),
            }
        }
        rest
    }

    /// 掷骰策略：取高 / 取低互相抵消（净 0 = 单次掷骰，行为与不声明挂载点时逐字一致）。
    pub(crate) fn roll_policy(&self) -> RollPolicy {
        if self.keep > 0 {
            RollPolicy::KeepHigh
        } else if self.keep < 0 {
            RollPolicy::KeepLow
        } else {
            RollPolicy::Single
        }
    }
}

/// 掷骰策略（通用动作）：单次 / 掷两次取高 / 掷两次取低。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RollPolicy {
    Single,
    KeepHigh,
    KeepLow,
}

/// 已判定结果 → Lua 只读快照（判定后挂载点读它决定「接下来做什么」）。
pub(crate) fn check_context(resolved: &ResolvedCheck) -> LuaCheckContext {
    LuaCheckContext {
        attribute: resolved.attribute.clone(),
        kind: Some(resolved.kind),
        resolved: true,
        expr: resolved.expr.clone(),
        total: resolved.total,
        target: resolved.target,
        margin: resolved.margin,
        result: resolved.result,
        level: Some(resolved.level),
        rolls: resolved.rolls.clone(),
    }
}

/// 效果快照 → Lua 只读事实（PostResolve 的 `host.resolved_effects`）。
///
/// 引擎只导出**它实际算出的东西**：数值型 delta（资源增减，缩放之后）、状态 / 静态修正、
/// 本次效果自己消耗的骰数，以及本次声明的缩放因子（None = 没声明）。
/// 消耗扣减**不在这里**——它不是效果本身。
fn effects_snapshot(effects: &EffectResolution, factor: Option<f64>) -> Value {
    serde_json::json!({
        "deltas": effects.deltas,
        "statuses": effects.statuses,
        "modifiers": effects.modifiers,
        "rng_consumed": effects.rng_consumed,
        "factor": factor,
    })
}

/// 判定签名 → Lua 只读快照（**掷骰前**挂载点读它决定「对哪一类判定做什么」）。
///
/// 判定签名（属性 / 判定种类 / 难度）在掷骰前已经确定，这里只是把**已知事实**交给钩子，
/// 不是新语义：`resolved = false`，结果字段一律不下发。
pub(crate) fn check_signature(attribute: &str, kind: CheckKind, target: i64) -> LuaCheckContext {
    LuaCheckContext {
        attribute: attribute.to_string(),
        kind: Some(kind),
        target,
        ..Default::default()
    }
}

/// 判定**后**修正：加值（重算 total / margin / 结果 / 档位）与难度修正（重算后三者），
/// 最后按需施加**结果覆盖**（通用原语，见 `resolve::apply_forced_result`）。
///
/// 掷骰已是既成事实，所以后置的「取高/取低」不生效——那只在掷骰前有意义。
pub(crate) fn apply_post_roll_adjustments(
    check: &mut ResolvedCheck,
    add: i64,
    dc: i64,
    mode: CheckMode,
    thresholds: &[i64],
    force: Option<bool>,
) {
    if add == 0 && dc == 0 && force.is_none() {
        return;
    }
    check.target += dc;
    // add 走既有原语（它同时并入 r#mod 并重算档位）；add == 0 时它会提前返回。
    apply_check_bonus(check, add, mode, thresholds);
    if dc != 0 {
        check.margin = check.total - check.target;
        check.result = compare(check.total, check.target, mode);
        check.level = level_for_margin(check.margin, thresholds);
    }
    if let Some(success) = force {
        apply_forced_result(check, success);
    }
}

/// 结算产物（Commit / Feedback 阶段消费）。
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutcome {
    pub rejection: Option<RejectionCode>,
    pub narrative: Option<String>,
    pub outcome: Option<String>,
    pub check: Option<ResolvedCheck>,
    pub effects: EffectResolution,
    /// 本次结算消耗的 RNG 原始输出（命令日志 rng_consume）。
    pub rng_consumed: Vec<u64>,
    /// Lua 脚本向引擎发起的写请求（引擎校验后执行）。
    pub requests: Vec<LuaRequest>,
}

impl CommandOutcome {
    fn rejected(code: RejectionCode) -> Self {
        Self {
            rejection: Some(code),
            narrative: None,
            outcome: None,
            check: None,
            effects: EffectResolution::default(),
            rng_consumed: Vec::new(),
            requests: Vec::new(),
        }
    }

    pub fn is_rejected(&self) -> bool {
        self.rejection.is_some()
    }

    pub fn deltas(&self) -> &[StateDelta] {
        &self.effects.deltas
    }
}

fn poisoned() -> EngineError {
    EngineError::Internal("rng poisoned".into())
}

/// 校验资源消耗：任一 cost 超过当前余额即 insufficient_resource。
pub fn insufficient_cost(skill: &SkillDef, actor: &Value) -> Option<RejectionCode> {
    if skill.cost.is_empty() {
        return None;
    }
    let resources = actor.get("resources");
    skill
        .cost
        .iter()
        .find(|cost| {
            let have = resources
                .and_then(|r| r.get(&cost.resource))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            have < cost.amount
        })
        .map(|_| RejectionCode::InsufficientResource)
}

fn number_of(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|i| i as f64))
        .unwrap_or(0.0)
}

/// 校验物品持有：背包数量 > 0；未持有即 item_not_owned。
pub fn insufficient_item(item_id: &str, actor: &Value) -> Option<RejectionCode> {
    let held = actor
        .get("inventory")
        .and_then(|inv| inv.get(item_id))
        .map(number_of)
        .unwrap_or(0.0);
    if held > 0.0 {
        None
    } else {
        Some(RejectionCode::ItemNotOwned)
    }
}

fn resource_delta(entity_id: &str, resource: &str, amount: i64) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Character,
        entity_id: entity_id.to_string(),
        field: format!("resources.{resource}"),
        op: DeltaOp::Add,
        value: serde_json::json!(amount),
    }
}

fn attribute_value(actor: &Value, attribute: &str) -> f64 {
    actor
        .get("attributes")
        .and_then(|a| a.get(attribute))
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        .unwrap_or(DEFAULT_BASELINE)
}

/// 技能静态属性修正：命中判定属性的 modifier 同名取 max（#12 ③）。
fn skill_modifier_bonus(skill: &SkillDef, attribute: &str) -> i64 {
    skill
        .effect
        .as_ref()
        .and_then(|effect| effect.modifiers.as_ref())
        .map(|mods| {
            mods.iter()
                .filter(|m| m.attribute == attribute)
                .map(|m| m.value)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// 技能判定来源：内联声明优先；引用全局时取 world.check。
fn skill_checker<'a>(skill: &'a SkillDef, global: Option<&'a CheckerDef>) -> Option<&'a CheckerDef> {
    match skill.check.as_ref()? {
        SkillCheck::Def(def) => Some(def),
        SkillCheck::Ref(_) => global,
    }
}

/// 判定属性解析优先级（判定 C2 §4）：技能声明 → 内联判定器 → 调用方指定 → 全局 → 'str'。
///
/// 「技能声明」是作者对「这一招用哪个维度」的表达；全局 world.check.attribute 只是缺省。
fn resolve_attribute(skill: &SkillDef, global: Option<&CheckerDef>, explicit: Option<&str>) -> String {
    skill
        .attribute
        .clone()
        .or_else(|| match skill.check.as_ref() {
            Some(SkillCheck::Def(def)) => def.attribute.clone(),
            _ => None,
        })
        .or_else(|| explicit.map(str::to_string))
        .or_else(|| global.and_then(|c| c.attribute.clone()))
        .unwrap_or_else(|| "str".to_string())
}

/// 跑一次判定：Lua 判定器不持锁（其内部 engine_rng 会自己加锁）；声明式才短暂加锁。
///
/// 公开给 session 的 check 意图复用（#04/#12：check 与技能走同一判定器路径），
/// 避免两套判定语义。
///
/// `policy` = 通用掷骰策略（单次 / 掷两次取高 / 掷两次取低）。单次时与历史逐字一致：
/// 只调一次 resolve_checker、只掷一次骰。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_check(
    checker: &CheckerDef,
    attribute: &str,
    value: f64,
    difficulty: i64,
    profile: ModifierProfile,
    rng: &Mutex<DeterministicRng>,
    lua: Option<(&LuaHost, &LuaHostContext)>,
    policy: RollPolicy,
) -> Result<ResolvedCheck, EngineError> {
    if let Some(script) = checker.lua.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // Lua 判定器自己掷骰（脚本内 host.engine_rng）：策略由脚本自行表达，引擎不重复掷。
        let (host, ctx) = lua.ok_or_else(|| {
            EngineError::Lua("判定器声明了 lua 脚本，但未提供 LuaHost".to_string())
        })?;
        let mode = checker.mode.unwrap_or(CheckMode::Gte);
        let mut out = resolve_lua_check(
            host,
            script,
            ctx,
            mode,
            degree_thresholds(checker),
            checker.kind.unwrap_or(CheckKind::Attribute),
        )?;
        out.attribute = attribute.to_string();
        return Ok(out);
    }
    let mut guard = rng.lock().map_err(|_| poisoned())?;
    // 声明式分支统一走 resolve_checker（lua 传 None = 不走 Lua 分支），与分派入口同名同义。
    let mut best = resolve_checker(checker, attribute, value, difficulty, profile, &mut guard, None)?;
    if policy == RollPolicy::Single || !best.rolled {
        return Ok(best);
    }
    // 「掷两次取高/低」：再掷一次同一骰式，保留更优的一组（RNG 消耗 = 骰式的两倍）。
    let Some(dice) = checker_dice(checker) else {
        return Ok(best);
    };
    let extra = roll_dice(&dice, &mut guard)?;
    let total = extra.total + best.r#mod;
    let better = match policy {
        RollPolicy::KeepHigh => total > best.total,
        RollPolicy::KeepLow => total < best.total,
        RollPolicy::Single => false,
    };
    if better {
        let mode = checker.mode.unwrap_or(CheckMode::Gte);
        best.expr = Some(extra.expr);
        best.rolls = extra.rolls;
        best.total = total;
        best.margin = total - best.target;
        best.result = compare(best.total, best.target, mode);
        best.level = level_for_margin(best.margin, degree_thresholds(checker));
    }
    Ok(best)
}

/// 执行某挂载点：注册表脚本按序跑（含 `when` 闸门）；技能自身 lua 钩子挂在 PreResolve。
///
/// 返回该挂载点新产生的写请求（顺序 = 脚本执行顺序）。Drain 放在每次挂载点之后，
/// 调用方才能把「判定修正 / 效果缩放」挑出来当场应用，而不是等到最后一起丢掉。
///
/// `resolved_effects` 只在 PostResolve 传 Some（本次**实际算出的效果**只读快照）；
/// 其他挂载点传 None，脚本读 `host.resolved_effects` 得到 nil。
fn run_mount(
    skill: &SkillDef,
    mount: LuaMount,
    ctx: &CommandContext<'_>,
    lua_ctx: &LuaHostContext,
    check: Option<&LuaCheckContext>,
    resolved_effects: Option<&Value>,
) -> Result<Vec<LuaRequest>, EngineError> {
    let Some((host, _)) = ctx.lua else {
        return Ok(Vec::new());
    };
    let env = MountEnv { gate: ctx.mount_gate, check, event: None, resolved_effects };
    if let Some(registry) = ctx.registry {
        registry.run_chain_with(host, mount, lua_ctx, &env)?;
    }
    if mount == LuaMount::PreResolve {
        if let Some(script) = skill.lua.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let mut skill_ctx = lua_ctx.clone();
            skill_ctx.script_id = format!("{}:lua", skill.id);
            skill_ctx.skill = Some(serde_json::to_value(skill).unwrap_or(Value::Null));
            host.run_hook_with(script, LuaMount::PreResolve, &skill_ctx, &env)?;
        }
    }
    Ok(host.drain_requests())
}

/// 结算一次技能使用：Validate -> Resolve（Lua 挂载点 + 判定 + 效果）-> Commit 数据。
pub fn execute_skill(
    skill: &SkillDef,
    ctx: &mut CommandContext<'_>,
) -> Result<CommandOutcome, EngineError> {
    // [1] Validate：资源不足直接驳回（不进入命令日志）。
    if let Some(code) = insufficient_cost(skill, ctx.actor) {
        return Ok(CommandOutcome::rejected(code));
    }

    // 清掉可能残留的 Lua 请求，保证 requests 只含本轮。
    if let Some((host, _)) = ctx.lua {
        let _ = host.drain_requests();
    }

    let before = ctx.rng.lock().map_err(|_| poisoned())?.consumed.len();
    let actor = ctx.actor;
    let declared_target = ctx.target_id;
    let actor_id = ctx.actor_id.to_string();
    // 结算语义：不给目标 = 自目标（目标回落成施法者本人）。
    let target_id = declared_target.unwrap_or(ctx.actor_id).to_string();
    // Lua 上下文的 target 必须与结算语义一致：自目标时指向施法者本人，
    // 而不是 None。否则规则包读 host.target 拿到 nil，会静默不结算（例如豁免成功后补效果）。
    let target_entity = match declared_target {
        // 调用方给了目标：沿用（解析不到实例时也保持「目标不是自己」）。
        Some(_) => ctx.target,
        None => Some(actor),
    };
    let global_checker = ctx.global_checker;
    let lua = ctx.lua;
    let extra_bonus = ctx.extra_bonus;
    let effect_requires_success = ctx.effect_requires_success;
    // 判定属性（判定 C2）：技能声明 → 内联判定器 → 调用方指定 → 全局 → 'str'。
    let attribute = resolve_attribute(skill, global_checker, ctx.attribute.as_deref());
    // 判定种类 / 比较方向在掷骰前就确定：check_pre_roll 的签名要用它，所以先解析。
    let checker_kind = skill_checker(skill, global_checker)
        .and_then(|c| c.kind)
        .unwrap_or(CheckKind::Attribute);
    let check_mode = skill_checker(skill, global_checker)
        .and_then(|c| c.mode)
        .unwrap_or(CheckMode::Gte);
    // 挂载点上下文用本地副本：判定后要往它里面放判定结果快照。
    let mut mount_ctx = lua.map(|(_, c)| c.clone()).unwrap_or_default();
    // 同步 target：Lua 看到的 target 语义与结算语义一致（自目标 = 施法者本人）。
    mount_ctx.target_id = Some(target_id.clone());
    mount_ctx.target = target_entity.cloned();
    let mut requests: Vec<LuaRequest> = Vec::new();

    // [2] Resolve：判定前/后 Lua 修正 + 核心判定。
    // 判定前（CheckPreRoll）：收集 → 掷骰前应用（取高/取低、改难度、加值）。
    // 签名（属性 / 种类 / 难度）在掷骰前已经确定，一并交给钩子；没有判定就没有签名。
    let signature = skill_checker(skill, global_checker)
        .is_some()
        .then(|| check_signature(&attribute, checker_kind, ctx.difficulty));
    let mut adjustments = CheckAdjustments::default();
    requests.extend(adjustments.absorb(run_mount(
        skill,
        LuaMount::CheckPreRoll,
        ctx,
        &mount_ctx,
        signature.as_ref(),
        None,
    )?));
    let difficulty = ctx.difficulty + adjustments.dc;
    mount_ctx.difficulty = Some(difficulty);
    let policy = adjustments.roll_policy();
    let check = match skill_checker(skill, global_checker) {
        Some(checker) => {
            // 豁免由「目标」掷骰（本人是施加方）；攻击 / 属性检定由本人掷骰。
            let subject = if checker_kind == CheckKind::Save {
                target_entity.unwrap_or(actor)
            } else {
                actor
            };
            let base_value = attribute_value(subject, &attribute);
            // 修正来源（挂接定义 / 已装备物品）：按目标的模板 id 并入属性值。
            let value = subject
                .get("template_id")
                .and_then(|v| v.as_str())
                .and_then(|tid| ctx.attribute_bonuses.and_then(|m| m.get(tid)))
                .and_then(|m| m.get(&attribute))
                .map(|m| m.apply(base_value))
                .unwrap_or(base_value);
            let profile = ctx
                .profiles
                .and_then(|m| m.get(&attribute))
                .copied()
                .unwrap_or_default();
            let mut resolved =
                run_check(checker, &attribute, value, difficulty, profile, ctx.rng, lua, policy)?;
            apply_check_bonus(
                &mut resolved,
                skill_modifier_bonus(skill, &attribute) + extra_bonus + adjustments.add,
                checker.mode.unwrap_or(CheckMode::Gte),
                degree_thresholds(checker),
            );
            Some(resolved)
        }
        None => None,
    };
    let mut check = check;
    let thresholds = skill_checker(skill, global_checker)
        .map(degree_thresholds)
        .unwrap_or(&crate::resolve::DEFAULT_DEGREE_THRESHOLDS);
    // 判定前挂载点的结果覆盖（判定 C4）：掷骰前就能声明「这次必定成功 / 失败」，
    // 在掷骰之后兑现——骰面照掷、RNG 记账不变，只覆盖结果与档位。
    if let (Some(resolved), Some(success)) = (check.as_mut(), adjustments.force) {
        apply_forced_result(resolved, success);
    }
    // 判定结果快照（判定期之后才存在）：判定后挂载点读它决定接下来做什么。
    let mut check_snapshot = check.as_ref().map(check_context);

    // 判定后（CheckPostRoll）：收集 → 掷骰后应用（改 total / margin / 档位 / 覆盖结果）。
    let mut post = CheckAdjustments::default();
    // 效果缩放（GAP-E）：判定后即可声明——与判定修正同一条收集路径（顺序不变）。
    let mut scale = EffectScale::default();
    requests.extend(scale.absorb(post.absorb(run_mount(
        skill,
        LuaMount::CheckPostRoll,
        ctx,
        &mount_ctx,
        check_snapshot.as_ref(),
        None,
    )?)));
    if let Some(resolved) = check.as_mut() {
        apply_post_roll_adjustments(resolved, post.add, post.dc, check_mode, thresholds, post.force);
        // 快照刷新为**后置修正之后**的值：PreResolve / PostResolve 读到的是最终判定。
        check_snapshot = Some(check_context(resolved));
    }

    // 效果缩放的第二个收集点：结算前钩子（含技能自身 lua）也能声明。
    requests.extend(scale.absorb(run_mount(
        skill,
        LuaMount::PreResolve,
        ctx,
        &mount_ctx,
        check_snapshot.as_ref(),
        None,
    )?));

    // [2b] 核心效果 + 消耗扣减（效果求值可能掷骰，临时持锁）。
    let empty_status_defs = HashMap::new();
    let status_defs = ctx.status_defs.unwrap_or(&empty_status_defs);
    // 豁免：失败（result = false）才结算效果；其余判定保持「结果交 AI 叙事」的既有语义。
    // 调用方要求命中门时（攻击类入口），一律以判定结果决定是否结算效果。
    //
    // GAP-E：规则包**声明了缩放因子**即表示「这次效果照常结算一次、数值按因子缩放」——
    // 声明本身就覆盖上面两道门（「豁免成功 = 完全不结算」不再是唯一出路）。
    // 没有声明时这里逐字不变。
    let effect_applies = scale.is_declared()
        || if effect_requires_success {
            check.as_ref().map(|c| c.result).unwrap_or(true)
        } else {
            match (&check, checker_kind) {
                (Some(resolved), CheckKind::Save) => !resolved.result,
                _ => true,
            }
        };
    let mut effects = match &skill.effect {
        Some(effect) if effect_applies => {
            // 同名状态的叠加策略（#12 ③）：以**施法者当前状态**为基座合并 add / max。
            // ctx.actor 是结算时点的角色实例快照，读不到时按「没有已有状态」处理（= 旧 replace）。
            let current_statuses: Vec<StatusInstance> = ctx
                .actor
                .get("statuses")
                .and_then(|v| serde_json::from_value::<Vec<StatusInstance>>(v.clone()).ok())
                .unwrap_or_default();
            let mut guard = ctx.rng.lock().map_err(|_| poisoned())?;
            resolve_effect(
                effect,
                &actor_id,
                &target_id,
                &mut guard,
                status_defs,
                &current_statuses,
            )?
        }
        _ => EffectResolution::default(),
    };
    // GAP-E：数值型 delta 按声明的因子缩放。**在消耗并入之前**——消耗是本次施法的
    // 代价，不是效果本身，不该跟着一起减半。没有声明因子时一个字节都不动。
    scale.apply(&mut effects);
    // PostResolve 的只读快照：引擎**实际**算出的效果与数值（缩放之后、消耗之前）。
    let resolved_effects = effects_snapshot(&effects, scale.factor);
    for cost in &skill.cost {
        effects.deltas.push(resource_delta(&actor_id, &cost.resource, -cost.amount));
    }

    requests.extend(run_mount(
        skill,
        LuaMount::PostResolve,
        ctx,
        &mount_ctx,
        check_snapshot.as_ref(),
        Some(&resolved_effects),
    )?);
    // Lua 判定器脚本自己也可能写请求：挂载点跑完后兜底收一次，不漏。
    if let Some((host, _)) = ctx.lua {
        requests.extend(host.drain_requests());
    }

    // [3] Commit 数据：RNG 消耗 + Lua 请求。
    let rng_consumed = {
        let guard = ctx.rng.lock().map_err(|_| poisoned())?;
        guard.consumed[before..].to_vec()
    };

    Ok(CommandOutcome {
        rejection: None,
        narrative: Some(format!("使用技能「{}」", skill.name)),
        outcome: Some(skill.id.clone()),
        check,
        effects,
        rng_consumed,
        requests,
    })
}

/// 物品技能：物品引用的技能与其执行的机制完全一致，仅 outcome 标注来源。
pub fn execute_item_skill(
    item_id: &str,
    skill: &SkillDef,
    ctx: &mut CommandContext<'_>,
) -> Result<CommandOutcome, EngineError> {
    // 先校验持有（#01 物品栏），再走常规技能结算。
    if let Some(code) = insufficient_item(item_id, ctx.actor) {
        return Ok(CommandOutcome::rejected(code));
    }
    let mut out = execute_skill(skill, ctx)?;
    if !out.is_rejected() {
        out.outcome = Some(format!("{item_id}::{}", skill.id));
    }
    Ok(out)
}

/// 便捷：无 Lua 的纯声明式结算（测试 / 无脚本场景）。
#[allow(clippy::too_many_arguments)]
pub fn execute_declarative_skill(
    skill: &SkillDef,
    actor_id: &str,
    actor: &Value,
    target_id: Option<&str>,
    target: Option<&Value>,
    attribute: &str,
    difficulty: i64,
    global_checker: Option<&CheckerDef>,
    rng: &Mutex<DeterministicRng>,
) -> Result<CommandOutcome, EngineError> {
    let mut ctx = CommandContext {
        actor_id,
        actor,
        target_id,
        target,
        difficulty,
        attribute: Some(attribute.to_string()),
        global_checker,
        rng,
        lua: None,
        registry: None,
        status_defs: None,
        profiles: None,
        attribute_bonuses: None,
        extra_bonus: 0,
        effect_requires_success: false,
        mount_gate: None,
    };
    execute_skill(skill, &mut ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_types::{
        AttributeModifier, CheckerDef, EffectDef, ImmediateEffect, LuaMountDef, ResourceCost,
        SuccessLevel,
    };
    use serde_json::json;

    /// 造一条规则集挂载点声明（文档里的 `lua_mounts` 条目形状）。
    fn md(id: &str, mount: &str, source: &str) -> LuaMountDef {
        LuaMountDef {
            id: id.into(),
            mount: mount.into(),
            source: source.into(),
            when: None,
            enabled: None,
        }
    }

    /// 以注册表 + 挂载点闸门跑一次技能结算，返回产物与 RNG 全量消耗。
    fn run_with_mounts(
        skill: &SkillDef,
        seed: u64,
        difficulty: i64,
        mounts: &[LuaMountDef],
        mount_gate: Option<&dyn Fn(&CondExpr) -> bool>,
    ) -> (CommandOutcome, Vec<u64>) {
        let host = LuaHost::new(seed).unwrap();
        let mut registry = LuaRegistry::new();
        for def in mounts {
            registry.register_def(def);
        }
        let a = actor(json!({ "hp": 30, "mana": 20 }));
        let lua_ctx = LuaHostContext {
            script_id: "mount-test".into(),
            actor_id: "char-a".into(),
            actor: a.clone(),
            difficulty: Some(difficulty),
            ..Default::default()
        };
        let rng = Mutex::new(DeterministicRng::new(seed));
        let out = {
            let mut ctx = CommandContext {
                actor_id: "char-a",
                actor: &a,
                target_id: None,
                target: None,
                difficulty,
                attribute: Some("str".into()),
                global_checker: None,
                rng: &rng,
                lua: Some((&host, &lua_ctx)),
                registry: Some(&registry),
                status_defs: None,
                profiles: None,
                attribute_bonuses: None,
                extra_bonus: 0,
                effect_requires_success: false,
                mount_gate,
            };
            execute_skill(skill, &mut ctx).unwrap()
        };
        let consumed = rng.lock().unwrap().consumed.clone();
        (out, consumed)
    }

    fn plain_check_skill() -> SkillDef {
        SkillDef {
            id: "sk-t".into(),
            name: "判定".into(),
            check: Some(SkillCheck::Def(CheckerDef {
                dice: Some("1d20".into()),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn skill_fire() -> SkillDef {
        SkillDef {
            id: "sk-fire".into(),
            name: "火球".into(),
            cost: vec![ResourceCost { resource: "mana".into(), amount: 5 }],
            check: Some(SkillCheck::Def(CheckerDef {
                dice: Some("1d20".into()),
                ..Default::default()
            })),
            effect: Some(EffectDef {
                immediate: Some(vec![ImmediateEffect::Damage {
                    amount: "2d6".into(),
                    resource: Some("hp".into()),
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn actor(resources: Value) -> Value {
        json!({ "id": "char-a", "attributes": { "str": 70 }, "resources": resources })
    }

    #[test]
    fn rejects_when_resource_insufficient() {
        let skill = skill_fire();
        let a = actor(json!({ "mana": 3, "hp": 30 }));
        let rng = Mutex::new(DeterministicRng::new(1));
        let out =
            execute_declarative_skill(&skill, "char-a", &a, None, None, "str", 12, None, &rng).unwrap();
        assert!(out.is_rejected());
        assert_eq!(out.rejection, Some(RejectionCode::InsufficientResource));
        assert!(out.rng_consumed.is_empty());
        assert!(out.deltas().is_empty());
        assert_eq!(rng.lock().unwrap().consumed.len(), 0);
    }

    #[test]
    fn success_deducts_cost_and_applies_effect() {
        let skill = skill_fire();
        let a = actor(json!({ "mana": 20, "hp": 30 }));
        let rng = Mutex::new(DeterministicRng::new(2024));
        let out =
            execute_declarative_skill(&skill, "char-a", &a, None, None, "str", 12, None, &rng).unwrap();
        assert!(!out.is_rejected());
        assert_eq!(out.outcome.as_deref(), Some("sk-fire"));
        let check = out.check.as_ref().expect("check present");
        assert!(check.rolled);
        assert_eq!(check.r#mod, 4);

        assert_eq!(out.deltas().len(), 2);
        assert_eq!(out.deltas()[0].field, "resources.hp");
        assert!(out.deltas()[0].value.as_i64().unwrap() <= -2);
        assert_eq!(out.deltas()[1].field, "resources.mana");
        assert_eq!(out.deltas()[1].value, json!(-5));

        assert_eq!(out.rng_consumed.len(), 3);
        assert_eq!(out.rng_consumed, rng.lock().unwrap().consumed);
    }

    #[test]
    fn lua_mounts_run_and_requests_are_collected() {
        let host = LuaHost::new(3).unwrap();
        let mut skill = skill_fire();
        skill.check = None;
        skill.effect = None;
        skill.cost.clear();
        skill.lua = Some("host.trigger_event('skill_lua')".into());
        let a = actor(json!({ "mana": 20 }));
        let lua_ctx =
            LuaHostContext { script_id: "skill-run".into(), actor: a.clone(), ..Default::default() };
        let mut registry = LuaRegistry::new();
        registry.register("pre", LuaMount::CheckPreRoll, "host.trigger_event('pre_roll')");
        registry.register("post", LuaMount::PostResolve, "host.trigger_event('post_resolve')");
        let rng = Mutex::new(DeterministicRng::new(1));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 12,
            attribute: Some("str".into()),
            global_checker: None,
            rng: &rng,
            lua: Some((&host, &lua_ctx)),
            registry: Some(&registry),
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        let out = execute_skill(&skill, &mut ctx).unwrap();
        let events: Vec<String> = out
            .requests
            .iter()
            .filter_map(|r| match r {
                LuaRequest::TriggerEvent { event, .. } => Some(event.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(events, vec!["pre_roll", "skill_lua", "post_resolve"]);
        assert!(out.rng_consumed.is_empty());
    }

    /// GAP-L 端到端：`host.target` 的只读快照与 `host.actor` 同级完整——
    /// 规则包脚本按**目标的状态**给本次攻击判定「掷两次取高」，实测生效（RNG 多消耗一颗）。
    #[test]
    fn target_status_snapshot_drives_check_mount() {
        let host = LuaHost::new(99).unwrap();
        let mut registry = LuaRegistry::new();
        registry.register(
            "target-status",
            LuaMount::CheckPreRoll,
            "local t = host.target\n\
             assert(t ~= nil and t.statuses ~= nil)\n\
             assert(t.attributes.dex == 40 and t.resources.hp == 7 and t.location_id == 'loc-1')\n\
             assert(t.kind == 'monster' and t.present == true)\n\
             for i = 1, #t.statuses do\n\
               if t.statuses[i].id == 'off-guard' then host.modify_check('keep_high') end\n\
             end",
        );
        let a = actor(json!({ "hp": 30 }));
        let with_status = json!({
            "instance_id": "inst-mon-1", "template_id": "mon-wolf", "name": "灰狼", "kind": "monster",
            "attributes": { "dex": 40 }, "resources": { "hp": 7 }, "location_id": "loc-1", "present": true,
            "statuses": [ { "id": "off-guard", "name": "疏于防备" } ]
        });
        let without_status = json!({
            "instance_id": "inst-mon-1", "template_id": "mon-wolf", "name": "灰狼", "kind": "monster",
            "attributes": { "dex": 40 }, "resources": { "hp": 7 }, "location_id": "loc-1", "present": true,
            "statuses": []
        });
        let rolls = |target: &Value| -> usize {
            let rng = Mutex::new(DeterministicRng::new(99));
            let lua_ctx = LuaHostContext {
                script_id: "target-status".into(),
                actor_id: "char-a".into(),
                actor: a.clone(),
                target_id: Some("mon-1".into()),
                target: Some(target.clone()),
                difficulty: Some(12),
                ..Default::default()
            };
            let mut ctx = CommandContext {
                actor_id: "char-a",
                actor: &a,
                target_id: Some("mon-1"),
                target: Some(target),
                difficulty: 12,
                attribute: Some("str".into()),
                global_checker: None,
                rng: &rng,
                lua: Some((&host, &lua_ctx)),
                registry: Some(&registry),
                status_defs: None,
                profiles: None,
                attribute_bonuses: None,
                extra_bonus: 0,
                effect_requires_success: false,
                mount_gate: None,
            };
            execute_skill(&plain_check_skill(), &mut ctx).unwrap().rng_consumed.len()
        };
        // 目标带「疏于防备」→ 脚本声明取高 = 同一骰式掷两次。
        assert_eq!(rolls(&with_status), 2, "按目标状态声明取高应多掷一颗");
        // 对照：目标没有该状态 → 单次掷骰。
        assert_eq!(rolls(&without_status), 1, "没有该状态时行为与不声明挂载点一致");
    }

    #[test]
    fn lua_hook_failure_is_fail_fast() {
        let host = LuaHost::new(1).unwrap();
        let a = actor(json!({}));
        let lua_ctx =
            LuaHostContext { script_id: "s".into(), actor: a.clone(), ..Default::default() };
        let mut registry = LuaRegistry::new();
        registry.register("boom", LuaMount::PreResolve, "error('boom')");
        let rng = Mutex::new(DeterministicRng::new(1));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 10,
            attribute: None,
            global_checker: None,
            rng: &rng,
            lua: Some((&host, &lua_ctx)),
            registry: Some(&registry),
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        assert!(execute_skill(&SkillDef::default(), &mut ctx).is_err());
    }

    #[test]
    fn global_checker_is_used_for_ref() {
        let mut skill = skill_fire();
        skill.cost.clear();
        skill.check = Some(SkillCheck::Ref("world".into()));
        let a = actor(json!({ "hp": 30 }));
        let global = CheckerDef { dice: Some("1d20".into()), ..Default::default() };
        let rng = Mutex::new(DeterministicRng::new(7));
        let out = execute_declarative_skill(
            &skill, "char-a", &a, None, None, "str", 10, Some(&global), &rng,
        )
        .unwrap();
        assert!(out.check.is_some());
        assert!(out.check.unwrap().rolled);
    }

    #[test]
    fn attribute_bonuses_shift_check_modifier() {
        // D&D 维度：基线 10 / 步长 2。基础 13 → +1；挂接 +1 后有效 14 → +2
        let skill = SkillDef {
            id: "sk-dex".into(),
            name: "敏捷检定".into(),
            check: Some(SkillCheck::Def(CheckerDef { dice: Some("1d20".into()), ..Default::default() })),
            ..Default::default()
        };
        let a = json!({ "id": "char-a", "template_id": "char-x", "attributes": { "dex": 13 }, "resources": {} });
        let mut profiles = HashMap::new();
        profiles.insert("dex".to_string(), ModifierProfile { baseline: 10.0, min: Some(1.0), max: Some(30.0), step: 2.0 });
        let mut per = HashMap::new();
        per.insert("dex".to_string(), AttrModifier { add: 1, max: None, set: None });
        let mut bonuses = HashMap::new();
        bonuses.insert("char-x".to_string(), per);
        let rng = Mutex::new(DeterministicRng::new(3));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 10,
            attribute: Some("dex".to_string()),
            global_checker: None,
            rng: &rng,
            lua: None,
            registry: None,
            status_defs: None,
            profiles: Some(&profiles),
            attribute_bonuses: Some(&bonuses),
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        let out = execute_skill(&skill, &mut ctx).unwrap();
        let check = out.check.expect("check present");
        assert_eq!(check.r#mod, 2, "挂接 +1 后 13→14，修正 floor((14-10)/2)=+2");
    }

    #[test]
    fn use_item_without_ownership_is_rejected() {
        let mut skill = skill_fire();
        skill.cost.clear();
        let a = actor(json!({ "hp": 30 })); // 无 inventory
        let rng = Mutex::new(DeterministicRng::new(1));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 12,
            attribute: Some("str".into()),
            global_checker: None,
            rng: &rng,
            lua: None,
            registry: None,
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        let out = execute_item_skill("it-wand", &skill, &mut ctx).unwrap();
        assert_eq!(out.rejection, Some(RejectionCode::ItemNotOwned));
        assert!(out.deltas().is_empty());
    }

    #[test]
    fn static_modifier_is_applied_to_check() {
        let mut skill = skill_fire();
        skill.cost.clear();
        skill.effect = Some(EffectDef {
            modifiers: Some(vec![octopus_types::AttributeModifier {
                attribute: "str".into(),
                value: 3,
            }]),
            ..Default::default()
        });
        let a = actor(json!({ "hp": 30 }));
        let rng = Mutex::new(DeterministicRng::new(11));
        let out =
            execute_declarative_skill(&skill, "char-a", &a, None, None, "str", 12, None, &rng).unwrap();
        let check = out.check.expect("check");
        assert_eq!(check.r#mod, 7, "属性修正 4 + 静态修正 3");
        assert_eq!(check.total, check.rolls.iter().sum::<i64>() + 7);
        assert_eq!(check.margin, check.total - 12);
    }

    #[test]
    fn item_skill_marks_outcome() {
        let mut skill = skill_fire();
        skill.cost.clear();
        let a = json!({
            "id": "char-a", "attributes": { "str": 70 }, "resources": { "hp": 30 },
            "inventory": { "it-wand": 1 }
        });
        let rng = Mutex::new(DeterministicRng::new(1));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 12,
            attribute: Some("str".into()),
            global_checker: None,
            rng: &rng,
            lua: None,
            registry: None,
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        let out = execute_item_skill("it-wand", &skill, &mut ctx).unwrap();
        assert_eq!(out.outcome.as_deref(), Some("it-wand::sk-fire"));
    }

    /// 判定属性优先级（判定 C2 §4）：技能声明 → 内联判定器 → 全局 world.check → 'str'。
    #[test]
    fn attribute_priority_skill_then_checker_then_global_then_default() {
        let a = actor(json!({ "hp": 30 }));
        let global = CheckerDef {
            dice: Some("1d20".into()),
            attribute: Some("con".into()),
            ..Default::default()
        };
        let rng = Mutex::new(DeterministicRng::new(5));
        let build = |skill_attribute: Option<&str>, checker_attribute: Option<&str>| SkillDef {
            id: "sk".into(),
            name: "技能".into(),
            attribute: skill_attribute.map(str::to_string),
            check: Some(SkillCheck::Def(CheckerDef {
                dice: Some("1d20".into()),
                attribute: checker_attribute.map(str::to_string),
                ..Default::default()
            })),
            ..Default::default()
        };
        for (skill_attribute, checker_attribute, want) in [
            (Some("dex"), Some("int"), "dex"),
            (None, Some("int"), "int"),
            (None, None, "con"),
        ] {
            let skill = build(skill_attribute, checker_attribute);
            let mut ctx = CommandContext {
                actor_id: "char-a",
                actor: &a,
                target_id: None,
                target: None,
                difficulty: 10,
                attribute: None,
                global_checker: Some(&global),
                rng: &rng,
                lua: None,
                registry: None,
                status_defs: None,
                profiles: None,
                attribute_bonuses: None,
                extra_bonus: 0,
                effect_requires_success: false,
                mount_gate: None,
            };
            let out = execute_skill(&skill, &mut ctx).unwrap();
            assert_eq!(out.check.expect("check").attribute, want);
        }
        // 三级声明全缺省 → 回落 'str'（向后兼容）。
        let skill = build(None, None);
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 10,
            attribute: None,
            global_checker: None,
            rng: &rng,
            lua: None,
            registry: None,
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        let out = execute_skill(&skill, &mut ctx).unwrap();
        assert_eq!(out.check.expect("check").attribute, "str");
    }

    /// `check: "world"`（Ref）必须读全局判定器的骰式 / 修正 / 阈值，而不是静默换骰（修 D3）。
    #[test]
    fn ref_checker_reads_global_dice_modifier_and_thresholds() {
        let mut skill = skill_fire();
        skill.cost.clear();
        skill.check = Some(SkillCheck::Ref("world".into()));
        let a = actor(json!({ "hp": 30 }));
        let mut fixed = std::collections::BTreeMap::new();
        fixed.insert("str".to_string(), 50i64);
        let global = CheckerDef {
            dice: Some("1d4".into()),
            attribute_modifier: Some(fixed),
            degree_thresholds: Some(vec![1000, 1000, 1000]),
            ..Default::default()
        };
        let rng = Mutex::new(DeterministicRng::new(7));
        let out =
            execute_declarative_skill(&skill, "char-a", &a, None, None, "str", 5, Some(&global), &rng)
                .unwrap();
        let check = out.check.expect("check");
        assert_eq!(check.expr.as_deref(), Some("1d4"), "骰式取自全局判定器（裸 1d20 = 未生效）");
        assert!(check.rolled);
        assert_eq!(check.r#mod, 50, "修正取自全局判定器");
        assert_eq!(
            check.level,
            SuccessLevel::Fail,
            "阈值取自全局判定器（默认阈值下同一差值会落到更高档）"
        );
    }

    /// 调用方追加的固定判定修正并入 total / margin（武器命中加值的通用落点）。
    #[test]
    fn extra_bonus_is_added_to_check_total() {
        let skill = skill_fire();
        let a = actor(json!({ "mana": 20, "hp": 30 }));
        let rng = Mutex::new(DeterministicRng::new(2024));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 12,
            attribute: Some("str".into()),
            global_checker: None,
            rng: &rng,
            lua: None,
            registry: None,
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 5,
            effect_requires_success: false,
            mount_gate: None,
        };
        let out = execute_skill(&skill, &mut ctx).unwrap();
        let check = out.check.expect("check");
        assert_eq!(check.r#mod, 9, "属性修正 4 + 调用方加值 5");
        assert_eq!(check.total, check.rolls.iter().sum::<i64>() + 9);
        assert_eq!(check.margin, check.total - 12);
    }

    /// 命中门（通用原语）：判定不成立时不结算效果，也不掷伤害骰。
    #[test]
    fn effect_requires_success_skips_effect_when_check_fails() {
        let skill = skill_fire();
        let a = actor(json!({ "mana": 20, "hp": 30 }));
        let rng = Mutex::new(DeterministicRng::new(3));
        let mut ctx = CommandContext {
            actor_id: "char-a",
            actor: &a,
            target_id: None,
            target: None,
            difficulty: 10_000,
            attribute: Some("str".into()),
            global_checker: None,
            rng: &rng,
            lua: None,
            registry: None,
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: true,
            mount_gate: None,
        };
        let out = execute_skill(&skill, &mut ctx).unwrap();
        assert!(!out.check.as_ref().expect("check").result, "难度极高必然判定失败");
        assert!(
            !out.deltas().iter().any(|d| d.field == "resources.hp"),
            "判定失败不得结算伤害"
        );
        assert_eq!(out.rng_consumed.len(), 1, "未命中只掷命中骰，不掷伤害骰");
    }

    // ---------- 规则集挂载点：判定原语（通用动作，不含规则集语义） ----------

    /// 取高/取低 = 掷两次取优：RNG 消耗 = 2，且同种子逐字可重放。
    #[test]
    fn pre_roll_keep_high_rolls_twice_and_replays() {
        let skill = plain_check_skill();
        let mounts = [md("gate", "check_pre_roll", "host.modify_check('keep_high')")];
        let (out, consumed) = run_with_mounts(&skill, 2024, 12, &mounts, None);
        assert_eq!(consumed.len(), 2, "取高 = 掷两次");
        assert_eq!(out.rng_consumed.len(), 2);
        let check = out.check.clone().expect("check");
        // 独立探针：同一序列的前两颗 d20，取大者即取高的结果。
        let mut probe = DeterministicRng::new(2024);
        let d1 = probe.range_inclusive(1, 20);
        let d2 = probe.range_inclusive(1, 20);
        assert_eq!(check.rolls, vec![d1.max(d2)]);
        assert_eq!(check.total, d1.max(d2) + check.r#mod);
        assert_eq!(check.margin, check.total - 12);
        assert_eq!(
            check.level,
            level_for_margin(check.margin, &crate::resolve::DEFAULT_DEGREE_THRESHOLDS)
        );

        // 同种子重跑 → 骰面与消耗逐字一致。
        let (again, consumed2) = run_with_mounts(&skill, 2024, 12, &mounts, None);
        assert_eq!(out.check, again.check);
        assert_eq!(consumed, consumed2);
    }

    /// 取低同样掷两次，取较小者。
    #[test]
    fn pre_roll_keep_low_rolls_twice() {
        let skill = plain_check_skill();
        let (out, consumed) = run_with_mounts(
            &skill,
            99,
            12,
            &[md("g", "check_pre_roll", "host.modify_check('keep_low')")],
            None,
        );
        assert_eq!(consumed.len(), 2);
        let mut probe = DeterministicRng::new(99);
        let d1 = probe.range_inclusive(1, 20);
        let d2 = probe.range_inclusive(1, 20);
        assert_eq!(out.check.expect("check").rolls, vec![d1.min(d2)]);
    }

    /// 判定前改难度：check_pre_roll 的 `dc` 在掷骰前生效（target 已改）。
    #[test]
    fn pre_roll_dc_shifts_target_before_rolling() {
        let skill = plain_check_skill();
        let (base, _) = run_with_mounts(&skill, 5, 12, &[], None);
        assert_eq!(base.check.expect("base").target, 12);
        let (out, consumed) = run_with_mounts(
            &skill,
            5,
            12,
            &[md("dc", "check_pre_roll", "host.modify_check('dc', -4)")],
            None,
        );
        assert_eq!(consumed.len(), 1, "改难度不额外掷骰");
        assert_eq!(out.check.expect("check").target, 8);
    }

    /// 判定后加值 / 改难度：重算 total、margin 与成功度分档。
    #[test]
    fn post_roll_add_and_dc_recompute_total_margin_level() {
        let skill = plain_check_skill();
        // 先用探针算出裸判定，把难度卡在「险胜」（margin = -1）这一档上。
        let mut probe = DeterministicRng::new(2024);
        let d = probe.range_inclusive(1, 20);
        let base_mod = 4; // str 70 → floor((70-50)/5)
        let difficulty = d + base_mod + 1; // margin = -1 → 险胜
        let (base, _) = run_with_mounts(&skill, 2024, difficulty, &[], None);
        let bc = base.check.expect("base check");
        assert_eq!(bc.margin, -1);
        assert_eq!(bc.level, SuccessLevel::Barely);

        // +6 → margin 5 → 成功；total / 修正同步改写。
        let (out, consumed) = run_with_mounts(
            &skill,
            2024,
            difficulty,
            &[md("prof", "check_post_roll", "host.modify_check('add', 6)")],
            None,
        );
        assert_eq!(consumed.len(), 1, "判定后修正不额外掷骰");
        let c = out.check.expect("check");
        assert_eq!(c.total, bc.total + 6);
        assert_eq!(c.r#mod, bc.r#mod + 6);
        assert_eq!(c.margin, 5);
        assert_eq!(c.level, SuccessLevel::Success);

        // 难度 -12 → margin 11 → 大成功（分档随之重算）。
        let (out2, _) = run_with_mounts(
            &skill,
            2024,
            difficulty,
            &[md("ease", "check_post_roll", "host.modify_check('dc', -12)")],
            None,
        );
        let c2 = out2.check.expect("check2");
        assert_eq!(c2.total, bc.total, "只改难度不改总值");
        assert_eq!(c2.target, difficulty - 12);
        assert_eq!(c2.margin, 11);
        assert_eq!(c2.level, SuccessLevel::Great);
    }

    /// 判定结果对 check_post_roll / pre_resolve 脚本可见（改总值、按结果发效果由此可写）。
    #[test]
    fn post_roll_scripts_can_read_check_snapshot() {
        let skill = plain_check_skill();
        let (out, _) = run_with_mounts(
            &skill,
            11,
            3,
            &[md(
                "branch",
                "check_post_roll",
                "if host.check_result then host.modify_resource('char-a', 'mana', -host.check_total) else host.modify_resource('char-a', 'mana', 1) end",
            )],
            None,
        );
        let c = out.check.expect("check");
        assert!(c.result, "难度 3 必成功");
        assert_eq!(
            out.requests,
            vec![LuaRequest::ModifyResource {
                target: "char-a".into(),
                resource: "mana".into(),
                amount: -c.total
            }]
        );
    }

    /// 施加即时效果 / 加减资源走 outbox 请求（引擎再校验与落状态）。
    #[test]
    fn effect_and_resource_requests_reach_outcome() {
        let (out, _) = run_with_mounts(
            &SkillDef::default(),
            1,
            12,
            &[md(
                "burst",
                "pre_resolve",
                "host.apply_effect('char-b', { kind = 'damage', amount = '3d6', resource = 'hp' }); host.modify_resource('char-a', 'mana', -3); host.modify_resource('char-a', 'mana', 5)",
            )],
            None,
        );
        assert!(out
            .requests
            .iter()
            .any(|r| matches!(r, LuaRequest::ApplyEffect { target, .. } if target == "char-b")));
        assert_eq!(
            out.requests
                .iter()
                .filter(|r| matches!(r, LuaRequest::ModifyResource { .. }))
                .count(),
            2
        );
        assert!(out.rng_consumed.is_empty(), "即时效果在 session 侧结算，不在命令内核掷骰");
    }

    /// 未知修正名 / 非法效果形状当场报错（fail-fast，不静默丢规则）。
    #[test]
    fn invalid_mount_requests_are_rejected() {
        for script in [
            "host.modify_check('luck')",
            "host.apply_effect('x', { kind = 'bogus' })",
            "host.modify_check('add', 'six')",
        ] {
            let host = LuaHost::new(1).unwrap();
            let mut registry = LuaRegistry::new();
            registry.register("bad", LuaMount::PreResolve, script);
            let a = actor(json!({ "hp": 30 }));
            let lua_ctx =
                LuaHostContext { script_id: "t".into(), actor: a.clone(), ..Default::default() };
            let rng = Mutex::new(DeterministicRng::new(1));
            let mut ctx = CommandContext {
                actor_id: "char-a",
                actor: &a,
                target_id: None,
                target: None,
                difficulty: 12,
                attribute: None,
                global_checker: None,
                rng: &rng,
                lua: Some((&host, &lua_ctx)),
                registry: Some(&registry),
                status_defs: None,
                profiles: None,
                attribute_bonuses: None,
                extra_bonus: 0,
                effect_requires_success: false,
                mount_gate: None,
            };
            assert!(
                execute_skill(&SkillDef::default(), &mut ctx).is_err(),
                "非法挂载点请求必须当场报错：{script}"
            );
        }
    }

    /// when 闸门：条件不成立即跳过（引擎只做「成立才跑」）。
    #[test]
    fn mount_gate_skips_scripts_when_condition_is_false() {
        let skill = plain_check_skill();
        let mut def = md("gated", "check_pre_roll", "host.modify_check('keep_high')");
        def.when = Some(CondExpr::FlagSet { flag: "focused".into() });
        let mounts = [def];

        let gate_false = |_: &CondExpr| false;
        let (out, consumed) =
            run_with_mounts(&skill, 3, 12, &mounts, Some(&gate_false));
        assert_eq!(consumed.len(), 1, "闸门不成立 → 只有裸判定掷一颗");
        assert_eq!(out.check.expect("check").target, 12);

        let gate_true = |_: &CondExpr| true;
        let (out2, consumed2) = run_with_mounts(&skill, 3, 12, &mounts, Some(&gate_true));
        assert_eq!(consumed2.len(), 2, "闸门成立 → 取高掷两次");
        assert!(out2.check.is_some());
    }

    /// 向后兼容：没有 lua_mounts（注册表缺失或为空）时，骰序与消耗逐字不变。
    #[test]
    fn absent_or_empty_mounts_keep_dice_order_identical() {
        let skill = plain_check_skill();
        let (empty, consumed) = run_with_mounts(&skill, 7, 12, &[], None);
        let a = actor(json!({ "hp": 30, "mana": 20 }));
        let rng = Mutex::new(DeterministicRng::new(7));
        let absent =
            execute_declarative_skill(&skill, "char-a", &a, None, None, "str", 12, None, &rng)
                .unwrap();
        let mut probe = DeterministicRng::new(7);
        let d = probe.range_inclusive(1, 20);
        assert_eq!(consumed, probe.consumed, "RNG 消耗逐字不变");
        assert_eq!(consumed.len(), 1);
        assert_eq!(empty.check, absent.check);
        assert_eq!(empty.rng_consumed, absent.rng_consumed);
        assert_eq!(empty.check.expect("check").rolls, vec![d]);
    }

    /// 缺陷 3（GAP-D）：技能路径的 check_pre_roll 也拿得到判定**签名**
    /// （属性 / 种类 / 难度），且结果字段仍是 nil——「只对某一类判定取高/取低」
    /// 因此能在掷骰前表达。
    #[test]
    fn pre_roll_sees_check_signature_in_skill_path() {
        let skill = SkillDef {
            id: "sk-sig".into(),
            name: "签名".into(),
            check: Some(SkillCheck::Def(CheckerDef {
                dice: Some("1d20".into()),
                kind: Some(CheckKind::Save),
                ..Default::default()
            })),
            ..Default::default()
        };
        // 命中签名才取高；同时断言结果字段在掷骰前不可见（total / result / rolled 皆 nil）。
        let script = "local c = host.check\nif c and c.attribute == 'str' and c.kind == 'save' and c.target == 9 and c.total == nil and c.result == nil and c.resolved == false and host.check_kind == 'save' and host.check_attribute == 'str' and host.check_target == 9 and host.check_total == nil then host.modify_check('keep_high') end";
        let (out, consumed) =
            run_with_mounts(&skill, 2024, 9, &[md("gate", "check_pre_roll", script)], None);
        assert_eq!(consumed.len(), 2, "签名可见 → 取高生效（掷两次）");
        let check = out.check.expect("check");
        assert_eq!(check.kind, CheckKind::Save);
        assert_eq!(check.target, 9);

        // 签名不匹配（kind 是 attribute 而非 save）→ 不取高，只掷一次。
        let script2 = "local c = host.check\nif c and c.kind == 'attribute' then host.modify_check('keep_high') end";
        let (out2, consumed2) =
            run_with_mounts(&skill, 2024, 9, &[md("gate", "check_pre_roll", script2)], None);
        assert_eq!(consumed2.len(), 1, "签名不匹配 → 不取高");
        assert_eq!(out2.check.expect("check2").rolls.len(), 1);
    }

    /// 缺陷 3：没有判定就没有签名——不会凭空造判定事实。
    #[test]
    fn pre_roll_has_no_signature_without_a_check() {
        let script = "if host.check ~= nil then host.trigger_event('signature_without_check') end";
        let (out, consumed) =
            run_with_mounts(&SkillDef::default(), 5, 12, &[md("probe", "check_pre_roll", script)], None);
        assert_eq!(consumed.len(), 0, "无判定：不掷骰");
        assert!(out.requests.is_empty(), "无判定时 check_pre_roll 不该看到签名：{:?}", out.requests);
    }

    /// 缺陷 3：check_post_roll 的判定快照在既有字段上补骰式 expr。
    #[test]
    fn post_roll_snapshot_carries_dice_expr() {
        let skill = plain_check_skill();
        let script = "if host.check_expr == '1d20' and host.check.expr == '1d20' and host.check.resolved == true then host.modify_check('add', 3) end";
        let (out, _) =
            run_with_mounts(&skill, 7, 12, &[md("expr", "check_post_roll", script)], None);
        assert_eq!(out.check.expect("check").r#mod, 4 + 3, "post_roll 读到 expr 才加值");
    }
    // ============================================================
    // GAP-E：效果缩放原语（「豁免成功伤害减半」不再是重掷近似）
    // ============================================================

    /// 一条「豁免 + 两段数值效果 + 标记 + 静态修正 + 消耗」的技能——正是旧 Lua 近似
    /// 够不到的形态：规则包只能重掷 immediate[1] 的骰式，第二段数值与 modifiers 全部丢。
    fn save_multi_effect_skill() -> SkillDef {
        SkillDef {
            id: "sk-save-multi".into(),
            name: "坠落瓦砾".into(),
            cost: vec![ResourceCost { resource: "mana".into(), amount: 5 }],
            check: Some(SkillCheck::Def(CheckerDef {
                dice: Some("1d20".into()),
                kind: Some(CheckKind::Save),
                ..Default::default()
            })),
            effect: Some(EffectDef {
                immediate: Some(vec![
                    ImmediateEffect::Damage { amount: "1d6".into(), resource: Some("hp".into()) },
                    ImmediateEffect::ModifyResource { resource: "mana".into(), amount: "2d6".into() },
                    ImmediateEffect::SetFlag { flag: "buried".into(), value: None },
                ]),
                modifiers: Some(vec![AttributeModifier { attribute: "str".into(), value: 2 }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// 跑一次「豁免 + 多段效果」结算。
    ///
    /// mounts 是规则包脚本（按声明顺序注册在各自挂载点）。豁免结果由判定前挂载点的
    /// 通用原语钉死（force_success / force_fail）：骰照掷、结果确定，断言与种子无关。
    fn run_scale_case(
        skill: &SkillDef,
        mounts: &[(LuaMount, &str)],
        save_succeeds: bool,
    ) -> (CommandOutcome, Vec<u64>) {
        let host = LuaHost::new(7).unwrap();
        let mut registry = LuaRegistry::new();
        registry.register(
            "pin-save-result",
            LuaMount::CheckPreRoll,
            if save_succeeds {
                "host.modify_check('force_success')"
            } else {
                "host.modify_check('force_fail')"
            },
        );
        for (mount, source) in mounts {
            registry.register(format!("rule:{}", mount.as_str()), *mount, *source);
        }
        let a = actor(json!({ "hp": 30, "mana": 20 }));
        let lua_ctx = LuaHostContext {
            script_id: "gap-e".into(),
            actor_id: "char-a".into(),
            actor: a.clone(),
            difficulty: Some(10),
            ..Default::default()
        };
        let rng = Mutex::new(DeterministicRng::new(7));
        let out = {
            let mut ctx = CommandContext {
                actor_id: "char-a",
                actor: &a,
                target_id: None,
                target: None,
                difficulty: 10,
                attribute: Some("str".into()),
                global_checker: None,
                rng: &rng,
                lua: Some((&host, &lua_ctx)),
                registry: Some(&registry),
                status_defs: None,
                profiles: None,
                attribute_bonuses: None,
                extra_bonus: 0,
                effect_requires_success: false,
                mount_gate: None,
            };
            execute_skill(skill, &mut ctx).unwrap()
        };
        (out, rng.lock().unwrap().consumed.clone())
    }

    /// GAP-E 核心价值：规则包只声明一个因子，引擎只掷一次效果骰，缩放覆盖全部
    /// 数值型 delta（两段效果 + modifiers 一并覆盖）——不再有「门 Lua 另掷一份」的近似。
    #[test]
    fn declared_scale_shrinks_every_numeric_delta_from_a_single_roll() {
        // 基线 A：没有因子 + 豁免成功 = 旧语义（效果完全不结算，只剩消耗）。
        let (untouched, untouched_rng) = run_scale_case(&save_multi_effect_skill(), &[], true);
        assert_eq!(untouched_rng.len(), 1, "无因子：豁免成功不结算效果 → 只有判定骰");
        assert_eq!(untouched.deltas().len(), 1, "无因子：只有消耗扣减");
        assert_eq!(untouched.deltas()[0].field, "resources.mana");
        assert_eq!(untouched.deltas()[0].value, json!(-5));

        // 基线 B：豁免失败 = 引擎全量结算——这就是「引擎实际算出的那份效果」。
        let (failed, failed_rng) = run_scale_case(&save_multi_effect_skill(), &[], false);
        assert_eq!(failed_rng.len(), 4, "豁免失败：1 颗 d20 + 1d6 + 2d6 = 4 颗");
        assert_eq!(failed.deltas().len(), 4, "两段数值 + 标记 + 消耗");
        let raw_hp = failed.deltas()[0].value.as_i64().unwrap();
        let raw_mana = failed.deltas()[1].value.as_i64().unwrap();
        assert!((-6..=-1).contains(&raw_hp), "1d6 伤害：{raw_hp}");
        assert!((2..=12).contains(&raw_mana), "2d6 资源变动：{raw_mana}");
        assert_eq!(failed.deltas()[2].domain, DeltaDomain::Flag);
        assert_eq!(failed.deltas()[2].value, json!(true));
        assert_eq!(failed.deltas()[3].value, json!(-5), "消耗 -5");

        // 因子 1：与「豁免失败的全量结算」逐字相同 → 缩放的就是引擎算出的那一份。
        let (full, full_rng) = run_scale_case(
            &save_multi_effect_skill(),
            &[(LuaMount::CheckPostRoll, "host.scale_effect(1)")],
            true,
        );
        assert_eq!(full_rng, failed_rng, "因子不改变掷骰：有因子时恰好「判定骰 + 一次效果骰」");
        assert_eq!(full.deltas(), failed.deltas(), "因子 1 = 引擎实际算出的效果，逐字相同");
        assert_eq!(full.effects.modifiers, failed.effects.modifiers);
        let brief = |o: &CommandOutcome| -> Vec<String> {
            o.deltas().iter().map(|d| format!("{}={}", d.field, d.value)).collect()
        };
        println!("GAP-E 无因子/豁免成功：rng={untouched_rng:?} deltas={:?}", brief(&untouched));
        println!(
            "GAP-E 无因子/豁免失败：rng={failed_rng:?} deltas={:?} modifiers={:?}",
            brief(&failed),
            failed.effects.modifiers
        );
        println!("GAP-E 有因子 1.0（成功）：rng={full_rng:?} deltas={:?}", brief(&full));

        // 因子 0 / 0.5 / 1.5 / 2：骰序不变，两段数值都按因子缩放（向零取整）。
        for factor in [0.0_f64, 0.5, 1.5, 2.0] {
            let script = format!("host.scale_effect({factor})");
            let (out, rng_used) = run_scale_case(
                &save_multi_effect_skill(),
                &[(LuaMount::CheckPostRoll, script.as_str())],
                true,
            );
            assert_eq!(rng_used, full_rng, "因子 {factor} 不得改变骰序 / 骰数");
            assert_eq!(
                out.deltas()[0].value.as_i64().unwrap(),
                (raw_hp as f64 * factor).trunc() as i64,
                "因子 {factor}：第一段数值 delta"
            );
            assert_eq!(
                out.deltas()[1].value.as_i64().unwrap(),
                (raw_mana as f64 * factor).trunc() as i64,
                "因子 {factor}：第二段数值 delta（旧 Lua 近似覆盖不到的那一段）"
            );
            assert_eq!(out.deltas()[2].domain, DeltaDomain::Flag);
            assert_eq!(out.deltas()[2].value, json!(true), "标记不受缩放（因子 {factor}）");
            assert_eq!(out.deltas()[3].value, json!(-5), "消耗不受缩放（因子 {factor}）");
            assert_eq!(out.effects.modifiers, full.effects.modifiers, "静态修正不受缩放");
            println!("GAP-E 有因子 {factor}：rng={rng_used:?} deltas={:?}", brief(&out));
        }
    }

    /// 收集时机：check_post_roll 与 pre_resolve（含技能自身 lua 钩子）都能声明；
    /// 同一轮多次声明以最后一条为准（脚本顺序即优先级）。
    #[test]
    fn scale_factor_can_be_declared_at_post_roll_or_pre_resolve() {
        let via_post = run_scale_case(
            &save_multi_effect_skill(),
            &[(LuaMount::CheckPostRoll, "host.scale_effect(0.5)")],
            true,
        );
        let via_pre = run_scale_case(
            &save_multi_effect_skill(),
            &[(LuaMount::PreResolve, "host.scale_effect(0.5)")],
            true,
        );
        assert_eq!(via_post.0.deltas(), via_pre.0.deltas(), "两个时机的同一因子结果一致");
        assert_eq!(via_post.1, via_pre.1, "两个时机的骰序一致");

        // 技能自身 lua 钩子（也挂在 PreResolve）同口径。
        let mut skill = save_multi_effect_skill();
        skill.lua = Some("host.scale_effect(0.5)".into());
        let via_skill_lua = run_scale_case(&skill, &[], true);
        assert_eq!(via_skill_lua.0.deltas(), via_post.0.deltas(), "技能自身 lua 钩子同口径");

        // 多次声明：后一条覆盖前一条。
        let last_wins = run_scale_case(
            &save_multi_effect_skill(),
            &[
                (LuaMount::CheckPostRoll, "host.scale_effect(0.5)"),
                (LuaMount::PreResolve, "host.scale_effect(1)"),
            ],
            true,
        );
        let (full, _) = run_scale_case(&save_multi_effect_skill(), &[], false);
        assert_eq!(last_wins.0.deltas(), full.deltas(), "PreResolve 的后一条声明覆盖前者");
    }

    /// 规则包的原样用法：豁免成功才声明因子；失败分支不声明 → 默认全量。
    /// 全程只掷一次伤害骰。
    #[test]
    fn rule_pack_idiom_scales_only_the_success_branch() {
        let rule = "if host.check_kind == 'save' and host.check_result then host.scale_effect(0.5) end";
        let (success, success_rng) =
            run_scale_case(&save_multi_effect_skill(), &[(LuaMount::CheckPostRoll, rule)], true);
        let (failure, failure_rng) =
            run_scale_case(&save_multi_effect_skill(), &[(LuaMount::CheckPostRoll, rule)], false);

        // 两边都是「判定骰 + 一次效果骰」：同种子下骰子完全一致（骰序不因分支而变）。
        assert_eq!(success_rng, failure_rng, "豁免成败不改变骰序");
        assert_eq!(success_rng.len(), 4);
        assert_eq!(success.deltas().len(), 4);
        assert_eq!(failure.deltas().len(), 4);
        for i in [0usize, 1] {
            let full = failure.deltas()[i].value.as_i64().unwrap();
            assert_eq!(
                success.deltas()[i].value.as_i64().unwrap(),
                (full as f64 * 0.5).trunc() as i64,
                "豁免成功那份 = 同一次掷骰的一半（第 {i} 段）"
            );
        }
        // 非数值效果与消耗两边逐字相同。
        assert_eq!(success.deltas()[2], failure.deltas()[2]);
        assert_eq!(success.deltas()[3], failure.deltas()[3]);
    }

    /// 硬要求：没有因子时逐字不变——挂载点事件顺序、骰序、rng_consumed 全部照旧。
    #[test]
    fn absent_factor_keeps_mount_and_dice_order_verbatim() {
        let mut skill = save_multi_effect_skill();
        skill.lua = Some("host.trigger_event('skill_lua')".into());
        let mounts: [(LuaMount, &str); 4] = [
            (LuaMount::CheckPostRoll, "host.trigger_event('post_roll')"),
            (LuaMount::PreResolve, "host.trigger_event('pre_resolve')"),
            (LuaMount::PostResolve, "host.trigger_event('post_resolve')"),
            (LuaMount::CheckPreRoll, "host.trigger_event('pre_roll')"),
        ];
        let (out, rng) = run_scale_case(&skill, &mounts, false);
        let events: Vec<String> = out
            .requests
            .iter()
            .filter_map(|r| match r {
                LuaRequest::TriggerEvent { event, .. } => Some(event.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            events,
            vec!["pre_roll", "post_roll", "pre_resolve", "skill_lua", "post_resolve"],
            "挂载点事件顺序不得因缩放收集而改变"
        );
        assert_eq!(rng.len(), 4, "骰序：1 颗 d20 + 1d6 + 2d6");
        assert_eq!(out.rng_consumed, rng, "rng_consumed = 本次全量消耗");
        // 同一个种子的「豁免成功 + 无因子」：效果一个都不结算，只掷判定骰。
        let (ok, ok_rng) = run_scale_case(&save_multi_effect_skill(), &[], true);
        assert_eq!(ok_rng.len(), 1);
        assert_eq!(ok_rng, rng[..1].to_vec(), "判定骰是序列里的第一颗");
        assert_eq!(ok.deltas().len(), 1);
    }

    /// PostResolve 只读快照：脚本能核对「引擎到底算了什么」（缩放后的实际数值），
    /// 而且改这张表不影响已经算出的结算产物。
    #[test]
    fn post_resolve_reads_back_the_engine_resolved_effect() {
        let script = "local e = host.resolved_effects; assert(e ~= nil); assert(e.factor == 0.5); assert(e.rng_consumed == 3); assert(#e.deltas == 3); assert(e.deltas[1].field == 'resources.hp'); assert(e.deltas[2].field == 'resources.mana'); host.set_flag('seen-hp', e.deltas[1].value); host.set_flag('seen-mana', e.deltas[2].value); host.set_flag('seen-flag', e.deltas[3].value); host.set_flag('seen-factor', e.factor); e.deltas[1].value = 999";
        let (out, _) = run_scale_case(
            &save_multi_effect_skill(),
            &[
                (LuaMount::CheckPostRoll, "if host.check_result then host.scale_effect(0.5) end"),
                (LuaMount::PostResolve, script),
            ],
            true,
        );
        let flag_value = |flag: &str| -> Option<Value> {
            out.requests.iter().find_map(|r| match r {
                LuaRequest::ApplyEffect { effect, .. }
                    if effect.get("flag").and_then(Value::as_str) == Some(flag) =>
                {
                    effect.get("value").cloned()
                }
                _ => None,
            })
        };
        assert_eq!(flag_value("seen-hp"), Some(out.deltas()[0].value.clone()), "如实回报伤害数值");
        assert_eq!(
            flag_value("seen-mana"),
            Some(out.deltas()[1].value.clone()),
            "如实回报第二段数值 delta"
        );
        assert_eq!(flag_value("seen-flag"), Some(json!(true)), "非数值效果也如实回报");
        assert_eq!(flag_value("seen-factor"), Some(json!(0.5)), "声明过的因子可读");
        // 只读：Lua 侧把快照改成 999，结算产物纹丝不动。
        assert_ne!(out.deltas()[0].value, json!(999), "快照是导出的事实，不是引擎状态本身");
        assert!(out.deltas()[0].value.as_i64().unwrap() < 0);
    }

    /// PostResolve 的只读快照里不含消耗：消耗是本次施法的代价，不是「效果」。
    #[test]
    fn resolved_effects_snapshot_excludes_cost() {
        let script = "local e = host.resolved_effects; assert(#e.deltas == 3); assert(e.rng_consumed == 3); assert(e.factor == nil); host.set_flag('seen-count', #e.deltas)";
        let (out, _) = run_scale_case(
            &save_multi_effect_skill(),
            &[(LuaMount::PostResolve, script)],
            false,
        );
        assert_eq!(out.deltas().len(), 4, "产物里 3 条效果 + 1 条消耗");
        let seen = out.requests.iter().find_map(|r| match r {
            LuaRequest::ApplyEffect { effect, .. }
                if effect.get("flag").and_then(Value::as_str) == Some("seen-count") =>
            {
                effect.get("value").cloned()
            }
            _ => None,
        });
        assert_eq!(seen, Some(json!(3)), "快照只含效果自身，不含消耗");
    }
}
