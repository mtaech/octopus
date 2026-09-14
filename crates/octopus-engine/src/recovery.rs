//! 恢复与休息（#4）：把资源声明的 natural_recovery 真正落地。
//!
//! 触发时机：per_turn / per_scene / per_short_rest / per_long_rest（per_rest 视为 per_long_rest 别名）。
//! 长休：声明了「休」类恢复的资源补满到 default_max；短休：仅 per_short_rest 按 amount 补，且不超上限。

use octopus_types::{DeltaDomain, DeltaOp, StateDelta};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestKind {
    Short,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryTrigger {
    PerTurn,
    PerScene,
    PerShortRest,
    PerLongRest,
}

impl RecoveryTrigger {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "per_turn" => Some(Self::PerTurn),
            "per_scene" => Some(Self::PerScene),
            "per_short_rest" => Some(Self::PerShortRest),
            "per_long_rest" | "per_rest" => Some(Self::PerLongRest),
            _ => None,
        }
    }

    /// 该触发是否在本次休息中生效（长休资源不在短休恢复）。
    pub fn active_on(&self, kind: RestKind) -> bool {
        match self {
            Self::PerLongRest => kind == RestKind::Long,
            Self::PerShortRest => true,
            _ => false,
        }
    }
}

/// 回合 / 场景边界（#12 ④）：\`per_turn\` / \`per_scene\` 的触发时机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickKind {
    Turn,
    Scene,
}

impl RecoveryTrigger {
    /// 该触发是否在本次**边界 tick** 中生效（per_turn 只在回合边界、per_scene 只在场景边界）。
    pub fn active_on_tick(&self, tick: TickKind) -> bool {
        match self {
            Self::PerTurn => tick == TickKind::Turn,
            Self::PerScene => tick == TickKind::Scene,
            _ => false,
        }
    }
}

/// 计算一次边界 tick 的自然恢复量：返回 (resource_id, delta)，只含真正有变化的项。
///
/// 上限与休息共用 \`default_max\`；\`per_turn\` / \`per_scene\` 各只在自己的边界生效，
/// 所以没声明这两类触发的故事书**不会产生任何增量、也不会多发事件**（与旧行为逐字一致）。
pub fn plan_tick(world_resources: &Value, actor_resources: &Value, tick: TickKind) -> Vec<(String, i64)> {
    let Some(defs) = world_resources.as_array() else { return Vec::new() };
    let mut out = Vec::new();
    for def in defs {
        let Some(id) = def.get("id").and_then(|v| v.as_str()) else { continue };
        let Some(recovery) = def.get("natural_recovery") else { continue };
        let amount = recovery.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
        let trigger = recovery
            .get("trigger")
            .and_then(|v| v.as_str())
            .and_then(RecoveryTrigger::parse);
        let Some(trigger) = trigger else { continue };
        if !trigger.active_on_tick(tick) { continue; }
        // amount <= 0 的声明不产生恢复（0 是「默认不恢复」的常见写法）。
        if amount <= 0 { continue; }
        let cur = actor_resources.get(id).and_then(|v| v.as_i64()).unwrap_or(0);
        let max = def.get("default_max").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
        let target = (cur + amount).min(max);
        if target > cur {
            out.push((id.to_string(), target - cur));
        }
    }
    out
}

/// 边界 tick → 状态增量（走 Character 域的 resources.<id>，与休息同一条路径）。
pub fn tick_deltas(
    actor_id: &str,
    world_resources: &Value,
    actor_resources: &Value,
    tick: TickKind,
) -> Vec<StateDelta> {
    plan_tick(world_resources, actor_resources, tick)
        .into_iter()
        .map(|(resource, delta)| StateDelta {
            domain: DeltaDomain::Character,
            entity_id: actor_id.to_string(),
            field: format!("resources.{resource}"),
            op: DeltaOp::Add,
            value: Value::from(delta),
        })
        .collect()
}

/// 计算一次休息的恢复量：返回 (resource_id, delta)，只含真正有变化的项。
pub fn plan_rest(world_resources: &Value, actor_resources: &Value, kind: RestKind) -> Vec<(String, i64)> {
    let Some(defs) = world_resources.as_array() else { return Vec::new() };
    let mut out = Vec::new();
    for def in defs {
        let Some(id) = def.get("id").and_then(|v| v.as_str()) else { continue };
        let Some(recovery) = def.get("natural_recovery") else { continue };
        let amount = recovery.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
        let trigger = recovery
            .get("trigger")
            .and_then(|v| v.as_str())
            .and_then(RecoveryTrigger::parse);
        let Some(trigger) = trigger else { continue };
        if !trigger.active_on(kind) { continue; }
        let cur = actor_resources.get(id).and_then(|v| v.as_i64()).unwrap_or(0);
        let max = def.get("default_max").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
        let target = match kind {
            // 短休只到这里（active_on 已过滤），按 amount 补且不超上限。
            RestKind::Short => (cur + amount).min(max),
            // 长休：补满到上限。
            RestKind::Long => max,
        };
        if target > cur {
            out.push((id.to_string(), target - cur));
        }
    }
    out
}

/// 休息 → 状态增量（走 Character 域的 resources.<id>，与运行时资源面板一致）。
pub fn rest_deltas(
    actor_id: &str,
    world_resources: &Value,
    actor_resources: &Value,
    kind: RestKind,
) -> Vec<StateDelta> {
    plan_rest(world_resources, actor_resources, kind)
        .into_iter()
        .map(|(resource, delta)| StateDelta {
            domain: DeltaDomain::Character,
            entity_id: actor_id.to_string(),
            field: format!("resources.{resource}"),
            op: DeltaOp::Add,
            value: Value::from(delta),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn world() -> Value {
        json!([
            { "id": "res-hp", "name": "生命值", "type": "numerical", "default_max": 8, "natural_recovery": { "amount": 0, "trigger": "per_long_rest" } },
            { "id": "res-slot-1", "name": "1环法术位", "type": "numerical", "default_max": 2, "natural_recovery": { "amount": 2, "trigger": "per_long_rest" } },
            { "id": "res-slot-2", "name": "2环法术位", "type": "numerical", "default_max": 1, "natural_recovery": { "amount": 1, "trigger": "per_short_rest" } },
            { "id": "res-stamina", "name": "体力", "type": "numerical", "default_max": 100, "natural_recovery": { "amount": 20, "trigger": "per_short_rest" } }
        ])
    }

    #[test]
    fn long_rest_refills_all_rest_resources() {
        let actor = json!({ "res-hp": 3, "res-slot-1": 0, "res-slot-2": 0, "res-stamina": 40 });
        let out = plan_rest(&world(), &actor, RestKind::Long);
        assert!(out.contains(&("res-hp".to_string(), 5)));
        assert!(out.contains(&("res-slot-1".to_string(), 2)));
        assert!(out.contains(&("res-slot-2".to_string(), 1)));
        assert!(out.contains(&("res-stamina".to_string(), 60)));
    }

    #[test]
    fn short_rest_only_touches_short_rest_resources_and_caps() {
        let actor = json!({ "res-hp": 3, "res-slot-1": 0, "res-slot-2": 0, "res-stamina": 95 });
        let out = plan_rest(&world(), &actor, RestKind::Short);
        assert!(!out.iter().any(|(id, _)| id == "res-hp"));
        assert!(!out.iter().any(|(id, _)| id == "res-slot-1"));
        assert!(out.contains(&("res-slot-2".to_string(), 1)));
        assert!(out.contains(&("res-stamina".to_string(), 5)), "不得超过上限");
    }

    fn world_with_ticks() -> Value {
        json!([
            { "id": "res-hp", "default_max": 10, "natural_recovery": { "amount": 3, "trigger": "per_turn" } },
            { "id": "res-focus", "default_max": 5, "natural_recovery": { "amount": 2, "trigger": "per_scene" } },
            { "id": "res-slot", "default_max": 2, "natural_recovery": { "amount": 2, "trigger": "per_long_rest" } },
            { "id": "res-zero", "default_max": 9, "natural_recovery": { "amount": 0, "trigger": "per_turn" } }
        ])
    }

    #[test]
    fn per_turn_tick_only_heals_turn_resources_and_caps() {
        let actor = json!({ "res-hp": 8, "res-focus": 0, "res-slot": 0, "res-zero": 0 });
        let out = plan_tick(&world_with_ticks(), &actor, TickKind::Turn);
        // 8 + 3 = 11 → 夹到 default_max 10，所以只补 2。
        assert!(out.contains(&("res-hp".to_string(), 2)), "per_turn 补量且不超上限：{out:?}");
        assert!(!out.iter().any(|(id, _)| id == "res-focus"), "per_scene 不在回合边界生效");
        assert!(!out.iter().any(|(id, _)| id == "res-slot"), "休息类不在边界生效");
        assert!(!out.iter().any(|(id, _)| id == "res-zero"), "amount <= 0 不产生恢复");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn per_scene_tick_only_heals_scene_resources() {
        let actor = json!({ "res-hp": 0, "res-focus": 0 });
        let out = plan_tick(&world_with_ticks(), &actor, TickKind::Scene);
        assert_eq!(out, vec![("res-focus".to_string(), 2)]);
    }

    #[test]
    fn tick_at_full_resource_is_a_no_op() {
        let actor = json!({ "res-hp": 10, "res-focus": 5 });
        assert!(plan_tick(&world_with_ticks(), &actor, TickKind::Turn).is_empty());
        assert!(plan_tick(&world_with_ticks(), &actor, TickKind::Scene).is_empty());
    }

    #[test]
    fn tick_deltas_use_character_resources_path() {
        let actor = json!({ "res-hp": 0 });
        let d = tick_deltas("inst-a", &world_with_ticks(), &actor, TickKind::Turn);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].domain, DeltaDomain::Character);
        assert_eq!(d[0].entity_id, "inst-a");
        assert_eq!(d[0].field, "resources.res-hp");
        assert_eq!(d[0].op, DeltaOp::Add);
        assert_eq!(d[0].value, Value::from(3));
    }

    #[test]
    fn rest_deltas_use_character_resources_path() {
        let actor = json!({ "res-slot-1": 0 });
        let d = rest_deltas("char-linas", &world(), &actor, RestKind::Long);
        let slot = d.iter().find(|x| x.field == "resources.res-slot-1").expect("slot delta");
        assert_eq!(slot.op, DeltaOp::Add);
        assert_eq!(slot.value, Value::from(2));
        assert_eq!(slot.entity_id, "char-linas");
    }
}
