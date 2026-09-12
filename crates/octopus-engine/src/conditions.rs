//! conditions：回合末目标 / 触发点条件求值（#13 骨架语义）。
//!
//! 声明式条件树 + lua 兜底；引擎回合末求值，达成即标记并提示主线 AI，不自动切场景。
//! 只读世界快照，不产生副作用（delta 由调用方提交）。

use std::collections::BTreeMap;

use octopus_types::{CondExpr, DeltaDomain, DeltaOp, StateDelta};
use serde_json::Value;

use crate::error::EngineError;
use crate::lua_host::{LuaHost, LuaHostContext};

/// 条件求值的只读世界快照。
pub struct EvalContext<'a> {
    pub flags: &'a BTreeMap<String, Value>,
    pub goals: &'a serde_json::Map<String, Value>,
    pub triggers: &'a serde_json::Map<String, Value>,
    pub actor_location: Option<&'a str>,
    pub actor_attributes: Option<&'a serde_json::Map<String, Value>>,
    pub relationships: &'a [Value],
    pub lua: Option<(&'a LuaHost, &'a LuaHostContext)>,
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn as_number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_i64().map(|i| i as f64))
}

/// 求值一个条件表达式（#13 的结构化树 + lua 兜底）。
pub fn eval_cond(cond: &CondExpr, ctx: &EvalContext<'_>) -> Result<bool, EngineError> {
    Ok(match cond {
        CondExpr::AllOf { children } => {
            for child in children {
                if !eval_cond(child, ctx)? {
                    return Ok(false);
                }
            }
            true
        }
        CondExpr::AnyOf { children } => {
            for child in children {
                if eval_cond(child, ctx)? {
                    return Ok(true);
                }
            }
            false
        }
        CondExpr::Not { child } => !eval_cond(child, ctx)?,
        CondExpr::FlagSet { flag } => ctx.flags.get(flag).map(truthy).unwrap_or(false),
        CondExpr::TriggerFired { trigger_id } => {
            ctx.triggers.get(trigger_id).map(truthy).unwrap_or(false)
        }
        CondExpr::AtLocation { location_id } => {
            ctx.actor_location == Some(location_id.as_str())
        }
        CondExpr::AttributeGe { attribute, value } => ctx
            .actor_attributes
            .and_then(|attrs| attrs.get(attribute))
            .and_then(as_number)
            .map(|v| v >= *value)
            .unwrap_or(false),
        CondExpr::RelationshipGe { from, to, r#type, value } => ctx.relationships.iter().any(|r| {
            r.get("from_id").and_then(Value::as_str) == Some(from.as_str())
                && r.get("to_id").and_then(Value::as_str) == Some(to.as_str())
                && r.get("type").and_then(Value::as_str) == Some(r#type.as_str())
                && r.get("value").and_then(as_number).unwrap_or(0.0) >= *value
        }),
        CondExpr::Lua { script } => {
            let (host, lua_ctx) = ctx
                .lua
                .ok_or_else(|| EngineError::Lua("lua 条件缺少 LuaHost".to_string()))?;
            host.run_condition(script, lua_ctx)?
        }
    })
}

fn parse_cond(value: Option<&Value>) -> Option<CondExpr> {
    match value {
        None | Some(Value::Null) => None,
        Some(v) => serde_json::from_value::<CondExpr>(v.clone()).ok(),
    }
}

/// 遍历骨架，返回本次**新**达成目标 id 与**新**触发触发点 id（已达成 / 已触发的不重复）。
pub fn evaluate_skeleton(
    skeleton: &Value,
    ctx: &EvalContext<'_>,
) -> Result<(Vec<String>, Vec<String>), EngineError> {
    let mut goals = Vec::new();
    let mut triggers = Vec::new();
    let Some(chapters) = skeleton.as_array() else {
        return Ok((goals, triggers));
    };
    for chapter in chapters {
        let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
            continue;
        };
        for scene in scenes {
            if let Some(list) = scene.get("goals").and_then(Value::as_array) {
                for goal in list {
                    let id = goal.get("id").and_then(Value::as_str).unwrap_or_default();
                    if id.is_empty() || ctx.goals.get(id).map(truthy).unwrap_or(false) {
                        continue;
                    }
                    if let Some(cond) = parse_cond(goal.get("condition")) {
                        if eval_cond(&cond, ctx)? {
                            goals.push(id.to_string());
                        }
                    }
                }
            }
            if let Some(list) = scene.get("triggers").and_then(Value::as_array) {
                for trigger in list {
                    let id = trigger.get("id").and_then(Value::as_str).unwrap_or_default();
                    if id.is_empty() || ctx.triggers.get(id).map(truthy).unwrap_or(false) {
                        continue;
                    }
                    if let Some(cond) = parse_cond(trigger.get("condition")) {
                        if eval_cond(&cond, ctx)? {
                            triggers.push(id.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok((goals, triggers))
}

/// 目标达成 delta（骨架进度域）。
pub fn goal_delta(id: &str) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Goal,
        entity_id: id.to_string(),
        field: "achieved".into(),
        op: DeltaOp::Set,
        value: Value::Bool(true),
    }
}

/// 触发点已触发 delta。
pub fn trigger_delta(id: &str) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Trigger,
        entity_id: id.to_string(),
        field: "fired".into(),
        op: DeltaOp::Set,
        value: Value::Bool(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua_host::LuaHost;
    use serde_json::json;

    fn empty() -> (BTreeMap<String, Value>, serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
        (BTreeMap::new(), serde_json::Map::new(), serde_json::Map::new())
    }

    #[test]
    fn declarative_ops_evaluate() {
        let mut flags = BTreeMap::new();
        flags.insert("met_isa".to_string(), json!(true));
        let mut triggers = serde_json::Map::new();
        triggers.insert("b1".to_string(), json!(true));
        let mut attrs = serde_json::Map::new();
        attrs.insert("cha".to_string(), json!(80));
        let relationships = vec![json!({
            "from_id": "char-a", "to_id": "char-b", "type": "好感", "value": 60
        })];
        let ctx = EvalContext {
            flags: &flags,
            goals: &serde_json::Map::new(),
            triggers: &triggers,
            actor_location: Some("loc-tavern"),
            actor_attributes: Some(&attrs),
            relationships: &relationships,
            lua: None,
        };
        assert!(eval_cond(&CondExpr::FlagSet { flag: "met_isa".into() }, &ctx).unwrap());
        assert!(!eval_cond(&CondExpr::FlagSet { flag: "nope".into() }, &ctx).unwrap());
        assert!(eval_cond(&CondExpr::TriggerFired { trigger_id: "b1".into() }, &ctx).unwrap());
        assert!(eval_cond(&CondExpr::AtLocation { location_id: "loc-tavern".into() }, &ctx).unwrap());
        assert!(!eval_cond(&CondExpr::AtLocation { location_id: "loc-mine".into() }, &ctx).unwrap());
        assert!(eval_cond(&CondExpr::AttributeGe { attribute: "cha".into(), value: 70.0 }, &ctx).unwrap());
        assert!(!eval_cond(&CondExpr::AttributeGe { attribute: "cha".into(), value: 90.0 }, &ctx).unwrap());
        assert!(eval_cond(
            &CondExpr::RelationshipGe { from: "char-a".into(), to: "char-b".into(), r#type: "好感".into(), value: 50.0 },
            &ctx
        )
        .unwrap());
        let tree = CondExpr::AllOf {
            children: vec![
                CondExpr::FlagSet { flag: "met_isa".into() },
                CondExpr::Not { child: Box::new(CondExpr::FlagSet { flag: "nope".into() }) },
            ],
        };
        assert!(eval_cond(&tree, &ctx).unwrap());
        assert!(eval_cond(
            &CondExpr::AnyOf { children: vec![CondExpr::FlagSet { flag: "nope".into() }, CondExpr::FlagSet { flag: "met_isa".into() }] },
            &ctx
        )
        .unwrap());
    }

    #[test]
    fn lua_op_delegates_to_sandbox() {
        let host = LuaHost::new(1).unwrap();
        let lua_ctx = LuaHostContext {
            script_id: "cond".into(),
            actor: json!({ "attributes": { "str": 70 } }),
            ..Default::default()
        };
        let (flags, goals, triggers) = empty();
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: None,
            relationships: &[],
            lua: Some((&host, &lua_ctx)),
        };
        assert!(eval_cond(
            &CondExpr::Lua { script: "return host.get_attribute('str') >= 60".into() },
            &ctx
        )
        .unwrap());
        assert!(!eval_cond(
            &CondExpr::Lua { script: "return host.get_attribute('str') >= 90".into() },
            &ctx
        )
        .unwrap());
    }

    #[test]
    fn skeleton_evaluation_marks_new_progress_only() {
        let skeleton = json!([{
            "id": "ch-1",
            "scenes": [{
                "id": "sc-1",
                "goals": [
                    { "id": "g1", "condition": { "op": "flag_set", "flag": "met_isa" } },
                    { "id": "g2", "condition": { "op": "flag_set", "flag": "nope" } }
                ],
                "triggers": [
                    { "id": "b1", "condition": { "op": "at_location", "location_id": "loc-tavern" } }
                ]
            }]
        }]);
        let mut flags = BTreeMap::new();
        flags.insert("met_isa".to_string(), json!(true));
        let mut goals = serde_json::Map::new();
        goals.insert("g2".to_string(), json!(true)); // 已达成，不再重复
        let triggers = serde_json::Map::new();
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: Some("loc-tavern"),
            actor_attributes: None,
            relationships: &[],
            lua: None,
        };
        let (new_goals, new_triggers) = evaluate_skeleton(&skeleton, &ctx).unwrap();
        assert_eq!(new_goals, vec!["g1".to_string()]);
        assert_eq!(new_triggers, vec!["b1".to_string()]);
        drop(ctx);

        // 已标记的 g1 不再出现
        goals.insert("g1".to_string(), json!(true));
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: Some("loc-tavern"),
            actor_attributes: None,
            relationships: &[],
            lua: None,
        };
        let (again, _) = evaluate_skeleton(&skeleton, &ctx).unwrap();
        assert!(again.is_empty());
    }
}
