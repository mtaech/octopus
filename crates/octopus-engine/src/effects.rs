//! effects：技能 / 物品效果结算（#12 ③）。
//!
//! 声明式四层（immediate / status / triggers / modifiers）+ Lua 兜底；
//! 数量字段支持骰子表达式，求值消耗引擎 RNG 并计数（#12 ④，供命令日志 rng_consume）。
//! 只被 resolve / command 调（#20 ②）。

use std::collections::HashMap;

use octopus_types::{
    AttributeModifier, DeltaDomain, DeltaOp, EffectDef, ImmediateEffect, StateDelta, StatusDef,
    StatusInstance, StatusStack, StatusUnit,
};

use crate::error::EngineError;
use crate::resolve::roll_dice;
use crate::rng::DeterministicRng;

/// damage / heal 未指定 resource 时作用的缺省资源。
pub const DEFAULT_VITAL_RESOURCE: &str = "hp";

/// 一次效果结算的产物。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectResolution {
    pub deltas: Vec<StateDelta>,
    pub statuses: Vec<StatusInstance>,
    /// 静态属性修正（供判定消费；同名取 max 由调用方合并）。
    pub modifiers: Vec<AttributeModifier>,
    /// 本次结算消耗的 RNG 值个数（写进命令日志 rng_consume）。
    pub rng_consumed: usize,
}

/// 求值效果数量：整数常量或骰子表达式（1d6 / 2d6+3 / -5）。
pub fn eval_amount(amount: &str, rng: &mut DeterministicRng) -> Result<i64, EngineError> {
    let trimmed = amount.trim();
    if trimmed.is_empty() {
        return Err(EngineError::Internal("效果数量为空".into()));
    }
    if trimmed.to_ascii_lowercase().contains('d') {
        Ok(roll_dice(trimmed, rng)?.total)
    } else {
        trimmed
            .parse::<i64>()
            .map_err(|_| EngineError::Internal(format!("无法解析效果数量: {amount}")))
    }
}

fn resource_delta(entity_id: &str, resource: &str, value: i64) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Character,
        entity_id: entity_id.to_string(),
        field: format!("resources.{resource}"),
        op: DeltaOp::Add,
        value: serde_json::json!(value),
    }
}

/// 状态 → 状态变更（domain Character / field status），供命令提交阶段广播。
pub fn status_delta(actor_id: &str, status: &StatusInstance) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Character,
        entity_id: actor_id.to_string(),
        field: "status".into(),
        op: DeltaOp::Add,
        value: serde_json::to_value(status).unwrap_or(serde_json::Value::Null),
    }
}

/// 结算即时效果：伤害 / 治疗 / 改资源 / 设标记。
pub fn resolve_immediate(
    effects: &[ImmediateEffect],
    target_id: &str,
    rng: &mut DeterministicRng,
) -> Result<Vec<StateDelta>, EngineError> {
    let mut out = Vec::new();
    for effect in effects {
        match effect {
            ImmediateEffect::Damage { amount, resource } => {
                let value = eval_amount(amount, rng)?;
                let res = resource.as_deref().unwrap_or(DEFAULT_VITAL_RESOURCE);
                out.push(resource_delta(target_id, res, -value.abs()));
            }
            ImmediateEffect::Heal { amount, resource } => {
                let value = eval_amount(amount, rng)?;
                let res = resource.as_deref().unwrap_or(DEFAULT_VITAL_RESOURCE);
                out.push(resource_delta(target_id, res, value.abs()));
            }
            ImmediateEffect::ModifyResource { resource, amount } => {
                let value = eval_amount(amount, rng)?;
                out.push(resource_delta(target_id, resource, value));
            }
            ImmediateEffect::SetFlag { flag } => {
                out.push(StateDelta {
                    domain: DeltaDomain::Flag,
                    entity_id: flag.clone(),
                    field: "flag".into(),
                    op: DeltaOp::Set,
                    value: serde_json::Value::Bool(true),
                });
            }
        }
    }
    Ok(out)
}

/// 持续状态声明 → 运行时状态实例。
pub fn build_status(status: &StatusDef) -> StatusInstance {
    StatusInstance {
        id: status.id.clone(),
        name: status.name.clone(),
        turns_left: if status.unit == StatusUnit::Turns {
            Some(status.duration as i32)
        } else {
            None
        },
        scenes_left: if status.unit == StatusUnit::Scenes {
            Some(status.duration as i32)
        } else {
            None
        },
    }
}

/// 按顶层状态定义构造实例；未声明时回落为 1 回合的同名状态（校验应已拦下悬空引用）。
pub fn build_status_ref(id: &str, defs: &HashMap<String, StatusDef>) -> StatusInstance {
    match defs.get(id) {
        Some(def) => build_status(def),
        None => StatusInstance {
            id: id.to_string(),
            name: id.to_string(),
            turns_left: Some(1),
            scenes_left: None,
        },
    }
}

/// Lua apply_status 用：脚本自带 duration / unit，名称优先取顶层声明。
pub fn build_status_instance(
    id: &str,
    duration: i64,
    unit: &str,
    defs: &HashMap<String, StatusDef>,
) -> StatusInstance {
    let name = defs.get(id).map(|d| d.name.clone()).unwrap_or_else(|| id.to_string());
    let left = duration.max(0) as i32;
    let scenes = unit == "scenes";
    StatusInstance {
        id: id.to_string(),
        name,
        turns_left: if scenes { None } else { Some(left) },
        scenes_left: if scenes { Some(left) } else { None },
    }
}

/// 按声明的叠加策略把「新施加的状态」并入「已有同名状态」（#12 ③）。
///
/// - \`Replace\`（缺省 / 未声明）：新实例原样覆盖；
/// - \`Add\`：时长相加（turns / scenes 各按自己的单位相加）；
/// - \`Max\`：取时长更长的那个。
///
/// 只合并**同 id** 的状态；没有已有实例时原样返回新实例。id / name 一律以新实例为准。
pub fn merge_status(
    existing: Option<&StatusInstance>,
    incoming: StatusInstance,
    stack: Option<StatusStack>,
) -> StatusInstance {
    let Some(old) = existing else { return incoming };
    match stack.unwrap_or(StatusStack::Replace) {
        StatusStack::Replace => incoming,
        StatusStack::Add => StatusInstance {
            turns_left: add_duration(old.turns_left, incoming.turns_left),
            scenes_left: add_duration(old.scenes_left, incoming.scenes_left),
            ..incoming
        },
        StatusStack::Max => StatusInstance {
            turns_left: max_duration(old.turns_left, incoming.turns_left),
            scenes_left: max_duration(old.scenes_left, incoming.scenes_left),
            ..incoming
        },
    }
}

/// 时长相加：两侧都有值才算和；只有一侧有值时保留它（跨单位声明是作者错误，不猜）。
fn add_duration(a: Option<i32>, b: Option<i32>) -> Option<i32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn max_duration(a: Option<i32>, b: Option<i32>) -> Option<i32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// 结算一个效果声明（不含延后的条件触发；触发链在回合末求值）。
/// status_defs 为故事书顶层状态声明表，effect.status 是状态 id 引用。
///
/// \`current_statuses\` 是施法者当前的同名状态实例表，供 \`stack\` 策略合并
///（add / max）；传空表等价于「没有已有状态」，行为与旧版 replace 逐字一致。
pub fn resolve_effect(
    effect: &EffectDef,
    actor_id: &str,
    target_id: &str,
    rng: &mut DeterministicRng,
    status_defs: &HashMap<String, StatusDef>,
    current_statuses: &[StatusInstance],
) -> Result<EffectResolution, EngineError> {
    let before = rng.consumed.len();
    let mut out = EffectResolution::default();

    if let Some(immediate) = &effect.immediate {
        out.deltas.extend(resolve_immediate(immediate, target_id, rng)?);
    }
    if let Some(list) = &effect.status {
        for id in list {
            let incoming = build_status_ref(id, status_defs);
            let merged = merge_status(
                current_statuses.iter().find(|s| s.id == incoming.id),
                incoming,
                status_defs.get(id).and_then(|d| d.stack),
            );
            out.deltas.push(status_delta(actor_id, &merged));
            out.statuses.push(merged);
        }
    }
    if let Some(mods) = &effect.modifiers {
        out.modifiers.extend(mods.iter().cloned());
    }

    out.rng_consumed = rng.consumed.len() - before;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eval_amount_handles_constants_and_dice() {
        let mut rng = DeterministicRng::new(1);
        assert_eq!(eval_amount("15", &mut rng).unwrap(), 15);
        assert_eq!(eval_amount("-5", &mut rng).unwrap(), -5);
        let rolled = eval_amount("2d6+3", &mut rng).unwrap();
        assert!((5..=15).contains(&rolled));
        assert_eq!(rng.consumed.len(), 2);
        assert!(eval_amount("abc", &mut rng).is_err());
        assert!(eval_amount("", &mut rng).is_err());
    }

    #[test]
    fn immediate_effects_produce_deltas() {
        let mut rng = DeterministicRng::new(2);
        let effects = vec![
            ImmediateEffect::Damage { amount: "10".into(), resource: Some("hp".into()) },
            ImmediateEffect::Heal { amount: "4".into(), resource: None },
            ImmediateEffect::ModifyResource { resource: "mana".into(), amount: "-3".into() },
            ImmediateEffect::SetFlag { flag: "poisoned".into() },
        ];
        let deltas = resolve_immediate(&effects, "char-target", &mut rng).unwrap();
        assert_eq!(deltas.len(), 4);
        assert_eq!(deltas[0].field, "resources.hp");
        assert_eq!(deltas[0].value, json!(-10));
        assert_eq!(deltas[1].field, "resources.hp");
        assert_eq!(deltas[1].value, json!(4));
        assert_eq!(deltas[2].field, "resources.mana");
        assert_eq!(deltas[2].value, json!(-3));
        assert_eq!(deltas[3].domain, DeltaDomain::Flag);
        assert_eq!(deltas[3].entity_id, "poisoned");
        assert_eq!(deltas[3].value, json!(true));
    }

    fn status_defs_with(id: &str, name: &str, duration: i64, unit: StatusUnit) -> HashMap<String, StatusDef> {
        let mut m = HashMap::new();
        m.insert(
            id.to_string(),
            StatusDef {
                id: id.into(),
                name: name.into(),
                description: None,
                duration,
                unit,
                stack: None,
                effect: None,
            },
        );
        m
    }

    #[test]
    fn effect_resolution_builds_status_and_counts_rng() {
        let effect = EffectDef {
            immediate: Some(vec![ImmediateEffect::ModifyResource {
                resource: "hp".into(),
                amount: "1d6".into(),
            }]),
            status: Some(vec!["burn".into()]),
            modifiers: Some(vec![AttributeModifier { attribute: "str".into(), value: 2 }]),
            ..Default::default()
        };
        let defs = status_defs_with("burn", "灼烧", 3, StatusUnit::Turns);
        let mut rng = DeterministicRng::new(5);
        let out = resolve_effect(&effect, "char-a", "char-b", &mut rng, &defs, &[]).unwrap();
        assert_eq!(out.rng_consumed, 1);
        assert_eq!(out.statuses.len(), 1);
        assert_eq!(out.statuses[0].turns_left, Some(3));
        assert_eq!(out.statuses[0].scenes_left, None);
        assert_eq!(out.deltas.len(), 2); // 资源 + 状态
        assert_eq!(out.deltas[1].field, "status");
        assert_eq!(out.modifiers, vec![AttributeModifier { attribute: "str".into(), value: 2 }]);
    }

    fn inst(id: &str, turns: Option<i32>, scenes: Option<i32>) -> StatusInstance {
        StatusInstance { id: id.into(), name: id.into(), turns_left: turns, scenes_left: scenes }
    }

    #[test]
    fn merge_status_add_sums_durations() {
        let old = inst("burn", Some(2), None);
        let new = inst("burn", Some(3), None);
        let merged = merge_status(Some(&old), new, Some(StatusStack::Add));
        assert_eq!(merged.turns_left, Some(5), "add 把剩余时长相加");
    }

    #[test]
    fn merge_status_max_keeps_longer_duration() {
        let old = inst("burn", Some(5), None);
        let new = inst("burn", Some(2), None);
        let merged = merge_status(Some(&old), new, Some(StatusStack::Max));
        assert_eq!(merged.turns_left, Some(5), "max 保留更长的那个");
        let merged2 = merge_status(Some(&inst("burn", Some(1), None)), inst("burn", Some(4), None), Some(StatusStack::Max));
        assert_eq!(merged2.turns_left, Some(4));
    }

    #[test]
    fn merge_status_replace_is_the_default_and_overwrites() {
        let old = inst("burn", Some(5), None);
        // 未声明 stack（None）与显式 Replace 等价：新实例原样覆盖。
        for stack in [None, Some(StatusStack::Replace)] {
            let merged = merge_status(Some(&old), inst("burn", Some(1), None), stack);
            assert_eq!(merged.turns_left, Some(1), "replace 用新时长");
        }
    }

    #[test]
    fn merge_status_without_existing_returns_incoming() {
        let merged = merge_status(None, inst("burn", Some(3), None), Some(StatusStack::Add));
        assert_eq!(merged.turns_left, Some(3));
    }

    #[test]
    fn merge_status_handles_scene_unit_separately_from_turns() {
        // 单位不同的声明是作者错误：只合并各自单位上「两侧都有值」的项，不跨单位相加。
        let old = inst("bless", None, Some(2));
        let new = inst("bless", Some(1), None);
        let merged = merge_status(Some(&old), new, Some(StatusStack::Add));
        assert_eq!(merged.turns_left, Some(1), "turns 只有一侧有值 → 保留它");
        assert_eq!(merged.scenes_left, Some(2), "scenes 只有一侧有值 → 保留它");
    }

    #[test]
    fn scenes_unit_sets_scenes_left() {
        let status = StatusDef {
            id: "bless".into(),
            name: "祝福".into(),
            description: None,
            duration: 2,
            unit: StatusUnit::Scenes,
            stack: None,
            effect: None,
        };
        let instance = build_status(&status);
        assert_eq!(instance.turns_left, None);
        assert_eq!(instance.scenes_left, Some(2));
    }
}
