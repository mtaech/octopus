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
    CheckKind, CheckerDef, CheckMode, DeltaDomain, DeltaOp, RejectionCode, SkillCheck, SkillDef,
    StateDelta, StatusDef,
};
use serde_json::Value;

use crate::effects::{resolve_effect, EffectResolution};
use crate::error::EngineError;
use crate::modifiers::AttrModifier;
use crate::lua_host::{LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest};
use crate::resolve::{
    apply_check_bonus, degree_thresholds, resolve_declarative_check, resolve_lua_check,
    ModifierProfile, ResolvedCheck, DEFAULT_BASELINE,
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

/// 跑一次判定：Lua 判定器不持锁（其内部 engine_rng 会自己加锁）；声明式才短暂加锁。
///
/// 公开给 session 的 check 意图复用（#04/#12：check 与技能走同一判定器路径），
/// 避免两套判定语义。
pub(crate) fn run_check(
    checker: &CheckerDef,
    attribute: &str,
    value: f64,
    difficulty: i64,
    profile: ModifierProfile,
    rng: &Mutex<DeterministicRng>,
    lua: Option<(&LuaHost, &LuaHostContext)>,
) -> Result<ResolvedCheck, EngineError> {
    if let Some(script) = checker.lua.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
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
    resolve_declarative_check(checker, attribute, value, difficulty, profile, &mut guard)
}

/// 执行某挂载点：注册表脚本按序跑；技能自身 lua 钩子挂在 PreResolve。
fn run_mount(skill: &SkillDef, mount: LuaMount, ctx: &CommandContext<'_>) -> Result<(), EngineError> {
    let Some((host, lua_ctx)) = ctx.lua else {
        return Ok(());
    };
    if let Some(registry) = ctx.registry {
        registry.run_chain(host, mount, lua_ctx)?;
    }
    if mount == LuaMount::PreResolve {
        if let Some(script) = skill.lua.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let mut skill_ctx = lua_ctx.clone();
            skill_ctx.script_id = format!("{}:lua", skill.id);
            skill_ctx.skill = Some(serde_json::to_value(skill).unwrap_or(Value::Null));
            host.run_hook(script, LuaMount::PreResolve, &skill_ctx)?;
        }
    }
    Ok(())
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
    let target_entity = ctx.target;
    let actor_id = ctx.actor_id.to_string();
    let target_id = ctx.target_id.unwrap_or(ctx.actor_id).to_string();
    let difficulty = ctx.difficulty;
    let attribute = ctx.attribute.clone().unwrap_or_default();
    let global_checker = ctx.global_checker;
    let lua = ctx.lua;

    // [2] Resolve：判定前/后 Lua 修正 + 核心判定。
    run_mount(skill, LuaMount::CheckPreRoll, ctx)?;

    let checker_kind = skill_checker(skill, global_checker)
        .and_then(|c| c.kind)
        .unwrap_or(CheckKind::Attribute);
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
            let mut resolved = run_check(checker, &attribute, value, difficulty, profile, ctx.rng, lua)?;
            apply_check_bonus(
                &mut resolved,
                skill_modifier_bonus(skill, &attribute),
                checker.mode.unwrap_or(CheckMode::Gte),
                degree_thresholds(checker),
            );
            Some(resolved)
        }
        None => None,
    };

    run_mount(skill, LuaMount::CheckPostRoll, ctx)?;
    run_mount(skill, LuaMount::PreResolve, ctx)?;

    // [2b] 核心效果 + 消耗扣减（效果求值可能掷骰，临时持锁）。
    let empty_status_defs = HashMap::new();
    let status_defs = ctx.status_defs.unwrap_or(&empty_status_defs);
    // 豁免：失败（result = false）才结算效果；其余判定保持「结果交 AI 叙事」的既有语义。
    let effect_applies = match (&check, checker_kind) {
        (Some(resolved), CheckKind::Save) => !resolved.result,
        _ => true,
    };
    let mut effects = match &skill.effect {
        Some(effect) if effect_applies => {
            let mut guard = ctx.rng.lock().map_err(|_| poisoned())?;
            resolve_effect(effect, &actor_id, &target_id, &mut guard, status_defs)?
        }
        _ => EffectResolution::default(),
    };
    for cost in &skill.cost {
        effects.deltas.push(resource_delta(&actor_id, &cost.resource, -cost.amount));
    }

    run_mount(skill, LuaMount::PostResolve, ctx)?;

    // [3] Commit 数据：RNG 消耗 + Lua 请求。
    let requests = ctx
        .lua
        .map(|(host, _)| host.drain_requests())
        .unwrap_or_default();
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
    };
    execute_skill(skill, &mut ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_types::{CheckerDef, EffectDef, ImmediateEffect, ResourceCost};
    use serde_json::json;

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
        };
        let out = execute_item_skill("it-wand", &skill, &mut ctx).unwrap();
        assert_eq!(out.outcome.as_deref(), Some("it-wand::sk-fire"));
    }
}
