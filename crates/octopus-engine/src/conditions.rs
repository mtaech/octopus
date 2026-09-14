//! conditions：回合末目标 / 触发点条件求值（#13 骨架语义）。
//!
//! 声明式条件树 + lua 兜底；引擎回合末求值，达成即标记并提示主线 AI，不自动切场景。
//! 只读世界快照，不产生副作用（delta 由调用方提交）。

use std::collections::{BTreeMap, BTreeSet};

use octopus_types::{
    relationship_endpoint, CondExpr, DeltaDomain, DeltaOp, EncounterPreset, StateDelta,
};
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
    /// 当前场景 id（地图 P5）：encounter_cleared 的场景归属判据。
    pub scene_id: Option<&'a str>,
    /// 当前世界的遭遇快照（value = EncounterView 形态 JSON）。
    pub encounters: &'a [Value],
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
        // 关系边端点：兼容规范 from/to 与旧 from_id/to_id（决策 #11）。
        CondExpr::RelationshipGe { from, to, r#type, value } => ctx.relationships.iter().any(|r| {
            relationship_endpoint(r, "from", "from_id") == Some(from.as_str())
                && relationship_endpoint(r, "to", "to_id") == Some(to.as_str())
                && r.get("type").and_then(Value::as_str) == Some(r#type.as_str())
                && r.get("value").and_then(as_number).unwrap_or(0.0) >= *value
        }),
        CondExpr::EncounterCleared {} => encounters_cleared(ctx),
        CondExpr::Lua { script } => {
            let (host, lua_ctx) = ctx
                .lua
                .ok_or_else(|| EngineError::Lua("lua 条件缺少 LuaHost".to_string()))?;
            host.run_condition(script, lua_ctx)?
        }
    })
}

/// encounter_cleared（地图 P5 §6.5）：当前场景的遭遇**全部结束**，或**敌人全灭**。
///
/// 判据（确定性、只看快照，不查库）：
/// - 只看**属于当前场景**的遭遇（scene_id 相同）；旧日志没有场景快照（None）时按当前场景计，
///   否则老存档里那场遭遇永远等不到清空；
/// - 场景里**一场遭遇都没有**时不成立——「清剿」不该在开战前自己完成；
/// - 每条遭遇都要么已结束（active = false），要么敌人全灭（每个敌人 hp <= 0）。
fn encounters_cleared(ctx: &EvalContext<'_>) -> bool {
    let mut seen = false;
    for enc in ctx.encounters {
        let scene = enc
            .get("scene_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let belongs = match (scene, ctx.scene_id) {
            (Some(s), Some(cur)) => s == cur,
            (Some(_), None) => false,
            (None, _) => true,
        };
        if !belongs {
            continue;
        }
        seen = true;
        // 已结束的遭遇直接算清空（导演可以显式收尾，不必把每只怪都打到 0）。
        if !enc.get("active").and_then(Value::as_bool).unwrap_or(true) {
            continue;
        }
        let all_down = enc
            .get("enemies")
            .and_then(Value::as_array)
            .is_some_and(|list| {
                list.iter()
                    .all(|e| e.get("hp").and_then(Value::as_i64).unwrap_or(0) <= 0)
            });
        if !all_down {
            return false;
        }
    }
    seen
}

fn parse_cond(value: Option<&Value>) -> Option<CondExpr> {
    match value {
        None | Some(Value::Null) => None,
        Some(v) => serde_json::from_value::<CondExpr>(v.clone()).ok(),
    }
}

/// 骨架一次求值的结果（#13 + repeatable 边沿语义）。
///
/// - goals：本次**新**达成的目标 id（已达成的不重复）
/// - triggers：本次**新**触发的触发点 id（一次性 = 首次满足且未触发过；
///   可重复 = 条件由假变真，即边沿）
/// - trigger_progress：需要落库的触发点进度（id → 进度值）。**完整**列出本次所有
///   发生变化的触发点：一次性触发落 true（与历史逐字一致），可重复触发落
///   { "fired": true, "active": <本次条件真假> }——active 就是边沿检测所依赖的
///   「上一次条件值」。调用方据此 emit StateUpdate，重放才逐条一致。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkeletonEval {
    pub goals: Vec<String>,
    pub triggers: Vec<String>,
    pub trigger_progress: Vec<(String, Value)>,
}

/// 可重复触发点的进度解析：返回（是否触发过，上一次条件是否成立）。
///
/// 旧值 / 非法值一律按「未触发、条件为假」处理——旧存档里没有 repeatable 触发点，
/// 因此这条分支对它们零影响。历史遗留的 true（若作者把既有触发点改成可重复）
/// 视为「触发过且条件曾为真」：不会当场重触发，要等条件先转假再转真。
fn repeatable_progress(entry: Option<&Value>) -> (bool, bool) {
    match entry {
        None | Some(Value::Null) => (false, false),
        Some(Value::Object(obj)) => (
            obj.get("fired").map(truthy).unwrap_or(false),
            obj.get("active").map(truthy).unwrap_or(false),
        ),
        Some(v) => (truthy(v), truthy(v)),
    }
}

/// 遍历骨架，返回本次进展（见 [`SkeletonEval`]）。
///
/// repeatable 语义（CONTEXT.md「剧情触发点」）：**默认一次性**——满足即触发一次并
/// 永久标记；声明 repeatable: true 的触发点在条件**由假变真（边沿）**时可再次触发，
/// 条件持续为真期间不重复触发。一次性路径的判定与落库值与本改动前**逐字一致**。
pub fn evaluate_skeleton_full(
    skeleton: &Value,
    ctx: &EvalContext<'_>,
) -> Result<SkeletonEval, EngineError> {
    let mut out = SkeletonEval::default();
    let Some(chapters) = skeleton.as_array() else {
        return Ok(out);
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
                            out.goals.push(id.to_string());
                        }
                    }
                }
            }
            if let Some(list) = scene.get("triggers").and_then(Value::as_array) {
                for trigger in list {
                    let id = trigger.get("id").and_then(Value::as_str).unwrap_or_default();
                    if id.is_empty() {
                        continue;
                    }
                    let entry = ctx.triggers.get(id);
                    let repeatable = trigger
                        .get("repeatable")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !repeatable {
                        // 一次性：与历史逐字一致——已触发即跳过（**连条件都不求值**，
                        // 免得 Lua 条件脚本在已触发的触发点上被反复执行），首次满足落 true。
                        if entry.map(truthy).unwrap_or(false) {
                            continue;
                        }
                        let Some(cond) = parse_cond(trigger.get("condition")) else {
                            continue;
                        };
                        if eval_cond(&cond, ctx)? {
                            out.triggers.push(id.to_string());
                            out.trigger_progress.push((id.to_string(), Value::Bool(true)));
                        }
                        continue;
                    }
                    let Some(cond) = parse_cond(trigger.get("condition")) else {
                        continue;
                    };
                    let active = eval_cond(&cond, ctx)?;
                    let (fired_before, active_before) = repeatable_progress(entry);
                    if active && !active_before {
                        out.triggers.push(id.to_string());
                    }
                    // 进度只在真的变化时落库：状态恒定不产生 delta（事件流不膨胀）。
                    let fired = fired_before || active;
                    if fired && (fired != fired_before || active != active_before) {
                        out.trigger_progress.push((
                            id.to_string(),
                            serde_json::json!({ "fired": true, "active": active }),
                        ));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 遍历骨架，返回本次**新**达成目标 id 与**新**触发触发点 id（已达成 / 已触发的不重复）。
///
/// 兼容入口：丢掉 [`SkeletonEval::trigger_progress`]（只读调用方不需要落库）。
/// 需要把进度写回世界状态（回合末求值）时用 [`evaluate_skeleton_full`]。
pub fn evaluate_skeleton(
    skeleton: &Value,
    ctx: &EvalContext<'_>,
) -> Result<(Vec<String>, Vec<String>), EngineError> {
    let out = evaluate_skeleton_full(skeleton, ctx)?;
    Ok((out.goals, out.triggers))
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

/// 骨架目标的稳定 id（地图 P5）：显式 id 优先，缺失时合成「场景 id + 序号」。
///
/// 与 skeleton_quests（投影）共用同一口径——两处各写一份就会让
/// 「遭遇关联的目标」与「任务面板里的任务」对不上同一个键。
pub fn goal_id_of(scene_id: &str, idx: usize, goal: &Value) -> String {
    goal.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{scene_id}#goal[{idx}]"))
}

/// 目标是不是「可展示的任务」：有文本才算（与 skeleton_quests 同口径）。
fn goal_has_text(goal: &Value) -> bool {
    goal.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|t| !t.is_empty())
}

/// 条件下的任意深度里是否出现 encounter_cleared（含 all_of / any_of / not 子树）。
fn cond_mentions_encounter_cleared(cond: Option<&Value>) -> bool {
    match cond {
        None | Some(Value::Null) => false,
        Some(Value::Array(list)) => list.iter().any(|c| cond_mentions_encounter_cleared(Some(c))),
        Some(v) => {
            if v.get("op").and_then(Value::as_str) == Some("encounter_cleared") {
                return true;
            }
            let children = v
                .get("children")
                .or_else(|| v.get("conditions"))
                .and_then(Value::as_array);
            if let Some(children) = children {
                if children.iter().any(|c| cond_mentions_encounter_cleared(Some(c))) {
                    return true;
                }
            }
            cond_mentions_encounter_cleared(v.get("child").or_else(|| v.get("condition")))
        }
    }
}

/// 遭遇的关联目标（地图 P5 §6.4）：按触发时所在场景**推导**，由调用方快照进 EncounterView。
///
/// 优先级（确定性，与内容书写顺序无关）：① 条件里等这场遭遇清空（encounter_cleared）的目标
/// → ② 场景主线目标（primary）→ ③ 场景首个有文本的目标。都没有 → None。
pub fn scene_encounter_goal_id(skeleton: &Value, scene_id: &str) -> Option<String> {
    let chapters = skeleton.as_array()?;
    for chapter in chapters {
        let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
            continue;
        };
        for scene in scenes {
            if scene.get("id").and_then(Value::as_str) != Some(scene_id) {
                continue;
            }
            let goals = scene.get("goals").and_then(Value::as_array)?;
            let pick = goals
                .iter()
                .position(|g| cond_mentions_encounter_cleared(g.get("condition")))
                .or_else(|| {
                    goals.iter().position(|g| {
                        goal_has_text(g)
                            && g.get("primary").and_then(Value::as_bool).unwrap_or(false)
                    })
                })
                .or_else(|| goals.iter().position(goal_has_text))
                .or_else(|| (!goals.is_empty()).then_some(0))?;
            return Some(goal_id_of(scene_id, pick, &goals[pick]));
        }
    }
    None
}

/// 触发点预置的遭遇（地图 P5 §6.3）：触发点被标记 fired 之后据此自动建遭遇。
pub struct TriggerEncounterPlan {
    pub preset: EncounterPreset,
    /// 触发点标题：预置没写 name 时作遭遇名。
    pub title: Option<String>,
}

/// 在骨架的场景里按 id 找触发点并解析它的 encounter 预置。
///
/// 找不到触发点 / 没写 encounter / 形状非法 → None（旧故事书零行为变化；形状非法由发布
/// 校验拦成 Error，运行期只跳过，不让坏数据把整轮结算打断）。
///
/// 注意：**掷表遭遇不走这里**——表由 Lua 掷骰 + set_flag 表达，本函数只认作者写死的预置。
pub fn trigger_encounter_preset(skeleton: &Value, trigger_id: &str) -> Option<TriggerEncounterPlan> {
    if trigger_id.trim().is_empty() {
        return None;
    }
    let chapters = skeleton.as_array()?;
    for chapter in chapters {
        let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
            continue;
        };
        for scene in scenes {
            let Some(triggers) = scene.get("triggers").and_then(Value::as_array) else {
                continue;
            };
            for trigger in triggers {
                if trigger.get("id").and_then(Value::as_str) != Some(trigger_id) {
                    continue;
                }
                let preset: EncounterPreset =
                    serde_json::from_value(trigger.get("encounter")?.clone()).ok()?;
                let title = trigger
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                return Some(TriggerEncounterPlan { preset, title });
            }
        }
    }
    None
}

/// 一个章节的空间范围（地图 P5 §6.2 / §6.7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterLocations {
    pub chapter_id: String,
    /// 其 scenes 的 location_id **并集**（按场景顺序去重，稳定）。
    pub location_ids: Vec<String>,
}

/// 章节地点 = 其 scenes 的 location_id 并集（地图 P5 §6.2）：**推导，不存冗余字段**。
///
/// 存一份就一定会与场景定义不同步；地图按它高亮「本章范围」。
pub fn chapter_locations(skeleton: &Value) -> Vec<ChapterLocations> {
    let mut out = Vec::new();
    let Some(chapters) = skeleton.as_array() else {
        return out;
    };
    for chapter in chapters {
        let chapter_id = chapter
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut seen = BTreeSet::new();
        let mut location_ids = Vec::new();
        if let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) {
            for scene in scenes {
                let Some(loc) = scene
                    .get("location_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                if seen.insert(loc.to_string()) {
                    location_ids.push(loc.to_string());
                }
            }
        }
        out.push(ChapterLocations { chapter_id, location_ids });
    }
    out
}

/// 触发点已触发 delta（一次性触发：进度值固定 true，与历史逐字一致）。
pub fn trigger_delta(id: &str) -> StateDelta {
    trigger_progress_delta(id, Value::Bool(true))
}

/// 触发点进度 delta（判定 C5 之外的「触发点可重复」落地）：
/// 一次性触发 value = true；可重复触发 value = { "fired": true, "active": <本次条件> }。
///
/// 域 / 字段 / op 与历史一致（trigger + fired + Set）——重放走同一条 apply_delta，
/// 旧日志里的 bool 值照常恢复为「已触发」。
pub fn trigger_progress_delta(id: &str, value: Value) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Trigger,
        entity_id: id.to_string(),
        field: "fired".into(),
        op: DeltaOp::Set,
        value,
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
            scene_id: Some("sc-1"),
            encounters: &[],
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
            scene_id: None,
            encounters: &[],
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
            scene_id: Some("sc-1"),
            encounters: &[],
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
            scene_id: Some("sc-1"),
            encounters: &[],
            lua: None,
        };
        let (again, _) = evaluate_skeleton(&skeleton, &ctx).unwrap();
        assert!(again.is_empty());
    }

    /// 关系边端点读取：规范 from/to 与旧 from_id/to_id 都能让 relationship_ge 求值（决策 #11）。
    #[test]
    fn relationship_ge_reads_canonical_and_legacy_endpoints() {
        let flags = BTreeMap::new();
        let goals = serde_json::Map::new();
        let triggers = serde_json::Map::new();
        let cond = CondExpr::RelationshipGe {
            from: "char-a".into(),
            to: "char-b".into(),
            r#type: "好感".into(),
            value: 50.0,
        };

        let canonical = vec![json!({ "from": "char-a", "to": "char-b", "type": "好感", "value": 60 })];
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: None,
            relationships: &canonical,
            scene_id: None,
            encounters: &[],
            lua: None,
        };
        assert!(eval_cond(&cond, &ctx).unwrap(), "规范 from/to 应可求值");

        let legacy = vec![json!({ "from_id": "char-a", "to_id": "char-b", "type": "好感", "value": 60 })];
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: None,
            relationships: &legacy,
            scene_id: None,
            encounters: &[],
            lua: None,
        };
        assert!(eval_cond(&cond, &ctx).unwrap(), "旧 from_id/to_id 应继续可求值");
    }

    /// 地图 P5 §6.5：encounter_cleared 的判据——只看当前场景、打完才算、开战前不算。
    #[test]
    fn encounter_cleared_requires_clearing_a_scene_encounter() {
        let flags = BTreeMap::new();
        let goals = serde_json::Map::new();
        let triggers = serde_json::Map::new();
        let cond = CondExpr::EncounterCleared {};

        let eval = |scene: Option<&str>, list: &[Value]| {
            let ctx = EvalContext {
                flags: &flags,
                goals: &goals,
                triggers: &triggers,
                actor_location: None,
                actor_attributes: None,
                relationships: &[],
                scene_id: scene,
                encounters: list,
                lua: None,
            };
            eval_cond(&cond, &ctx).unwrap()
        };

        // 场景里一场遭遇都没有 → 不成立（清剿目标不该在开战前自己完成）。
        assert!(!eval(Some("sc-1"), &[]));

        let alive = json!({
            "id": "enc-1", "name": "地精", "active": true, "scene_id": "sc-1",
            "enemies": [{ "id": "e1", "name": "地精", "hp": 7, "max": 7, "ac": 15 }]
        });
        assert!(!eval(Some("sc-1"), std::slice::from_ref(&alive)), "还有活着的敌人 → 未清空");

        let down = json!({
            "id": "enc-1", "name": "地精", "active": true, "scene_id": "sc-1",
            "enemies": [{ "id": "e1", "name": "地精", "hp": 0, "max": 7, "ac": 15 }]
        });
        assert!(eval(Some("sc-1"), std::slice::from_ref(&down)), "敌人全灭 → 清空");
        assert!(
            !eval(Some("sc-2"), std::slice::from_ref(&down)),
            "别的场景的遭遇不算当前场景清剿完成"
        );

        // 显式结束（active = false）也算清空：导演可以收尾而不必把每只怪打到 0。
        let ended = json!({
            "id": "enc-1", "name": "地精", "active": false, "scene_id": "sc-1",
            "enemies": [{ "id": "e1", "name": "地精", "hp": 7, "max": 7, "ac": 15 }]
        });
        assert!(eval(Some("sc-1"), std::slice::from_ref(&ended)), "已结束的遭遇算清空");

        // 旧日志没有 scene_id 快照 → 按当前场景计，否则老存档永远等不到清空。
        let legacy = json!({
            "id": "enc-1", "name": "地精", "active": true,
            "enemies": [{ "id": "e1", "name": "地精", "hp": 0, "max": 7, "ac": 15 }]
        });
        assert!(eval(Some("sc-1"), std::slice::from_ref(&legacy)), "缺 scene_id 的旧遭遇按当前场景计");

        // 同场景两场：一场打完、一场还有活人 → 未清空。
        assert!(
            !eval(Some("sc-1"), &[down, alive]),
            "还有一场没打完 → 未清空"
        );
    }

    /// 地图 P5 §6.3：预置遭遇只在作者写了 encounter 时才有计划；旧触发点解析出 None。
    #[test]
    fn trigger_encounter_preset_only_for_declared_presets() {
        let skeleton = json!([{
            "id": "ch-1",
            "scenes": [{
                "id": "sc-1",
                "triggers": [
                    { "id": "tr-plain", "title": "无事发生", "condition": { "op": "flag_set", "flag": "f" } },
                    { "id": "tr-ambush", "title": "伏击", "condition": { "op": "flag_set", "flag": "f" },
                      "encounter": { "name": "游荡的地精", "location_id": "loc-cave",
                        "enemies": [{ "template_id": "mon-goblin", "count": 2 }] } }
                ]
            }]
        }]);
        assert!(trigger_encounter_preset(&skeleton, "tr-plain").is_none(), "没写 encounter → 无计划");
        assert!(trigger_encounter_preset(&skeleton, "tr-ghost").is_none(), "找不到触发点 → 无计划");
        let plan = trigger_encounter_preset(&skeleton, "tr-ambush").expect("有预置");
        assert_eq!(plan.title.as_deref(), Some("伏击"));
        assert_eq!(plan.preset.name.as_deref(), Some("游荡的地精"));
        assert_eq!(plan.preset.location_id.as_deref(), Some("loc-cave"));
        assert_eq!(plan.preset.enemies[0].template_id, "mon-goblin");
        assert_eq!(plan.preset.enemies[0].count, Some(2));
    }

    /// 地图 P5 §6.4：遭遇的关联目标推导——优先「等这场遭遇清空」的目标，其次主线目标。
    #[test]
    fn encounter_goal_id_prefers_encounter_cleared_goal() {
        let skeleton = json!([{
            "id": "ch-1",
            "scenes": [{
                "id": "sc-1",
                "goals": [
                    { "id": "g-scout", "text": "找到巢穴", "primary": true,
                      "condition": { "op": "flag_set", "flag": "found" } },
                    { "id": "g-clear", "text": "清剿地精",
                      "condition": { "op": "all_of", "children": [
                        { "op": "encounter_cleared" }, { "op": "flag_set", "flag": "found" } ] } }
                ]
            }, {
                "id": "sc-2",
                "goals": [{ "text": "无 id 的目标", "primary": true }]
            }, { "id": "sc-3", "goals": [] }]
        }]);
        assert_eq!(scene_encounter_goal_id(&skeleton, "sc-1").as_deref(), Some("g-clear"));
        assert_eq!(
            scene_encounter_goal_id(&skeleton, "sc-2").as_deref(),
            Some("sc-2#goal[0]"),
            "无 id 的目标用与 QuestView 同口径的合成 id"
        );
        assert_eq!(scene_encounter_goal_id(&skeleton, "sc-3"), None);
        assert_eq!(scene_encounter_goal_id(&skeleton, "sc-ghost"), None);
    }

    /// 地图 P5 §6.2：章节地点 = 其 scenes 的 location_id 并集（推导，不存字段）。
    #[test]
    fn chapter_locations_are_derived_from_scenes() {
        let skeleton = json!([
            { "id": "ch-1", "scenes": [
                { "id": "sc-1", "location_id": "loc-cave" },
                { "id": "sc-2", "location_id": "loc-town" },
                { "id": "sc-3", "location_id": "loc-cave" },
                { "id": "sc-4" }
            ] },
            { "id": "ch-2", "scenes": [{ "id": "sc-5" }] }
        ]);
        let chapters = chapter_locations(&skeleton);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_id, "ch-1");
        assert_eq!(chapters[0].location_ids, vec!["loc-cave", "loc-town"]);
        assert!(chapters[1].location_ids.is_empty());
    }

    /// repeatable 边沿语义（CONTEXT.md「剧情触发点」）：条件由假变真才再触发，
    /// 条件持续为真不重复；进度里带 active（上一次条件值），落库后重放得到同一结论。
    #[test]
    fn repeatable_trigger_fires_on_false_to_true_edge() {
        let skeleton = json!([{
            "id": "ch-1",
            "scenes": [{
                "id": "sc-1",
                "triggers": [
                    { "id": "tr-repeat", "repeatable": true,
                      "condition": { "op": "flag_set", "flag": "wander" } },
                    { "id": "tr-once",
                      "condition": { "op": "flag_set", "flag": "wander" } }
                ]
            }]
        }]);
        let eval = |flags: &BTreeMap<String, Value>, triggers: &serde_json::Map<String, Value>| {
            let ctx = EvalContext {
                flags,
                goals: &serde_json::Map::new(),
                triggers,
                actor_location: None,
                actor_attributes: None,
                relationships: &[],
                scene_id: Some("sc-1"),
                encounters: &[],
                lua: None,
            };
            evaluate_skeleton_full(&skeleton, &ctx).unwrap()
        };

        let mut flags = BTreeMap::new();
        flags.insert("wander".to_string(), json!(true));
        let mut progress = serde_json::Map::new();

        // ① 首次满足：一次性与可重复都触发；一次性落 true（与历史逐字一致），
        //    可重复落 {fired, active}。
        let out = eval(&flags, &progress);
        assert_eq!(out.triggers, vec!["tr-repeat".to_string(), "tr-once".to_string()]);
        assert_eq!(
            out.trigger_progress,
            vec![
                ("tr-repeat".to_string(), json!({ "fired": true, "active": true })),
                ("tr-once".to_string(), json!(true)),
            ]
        );
        for (id, value) in &out.trigger_progress {
            progress.insert(id.clone(), value.clone());
        }

        // ② 条件持续为真：两者都不再触发，也不产生 delta。
        let out = eval(&flags, &progress);
        assert!(out.triggers.is_empty());
        assert!(out.trigger_progress.is_empty());

        // ③ 条件转假：不触发，但可重复触发点把 active=false 落库（边沿的「上一次值」）。
        flags.remove("wander");
        let out = eval(&flags, &progress);
        assert!(out.triggers.is_empty());
        assert_eq!(
            out.trigger_progress,
            vec![("tr-repeat".to_string(), json!({ "fired": true, "active": false }))]
        );
        for (id, value) in &out.trigger_progress {
            progress.insert(id.clone(), value.clone());
        }

        // ④ 条件再次由假变真：可重复触发点再触发；一次性触发点纹丝不动。
        flags.insert("wander".to_string(), json!(true));
        let out = eval(&flags, &progress);
        assert_eq!(out.triggers, vec!["tr-repeat".to_string()]);
        assert_eq!(
            out.trigger_progress,
            vec![("tr-repeat".to_string(), json!({ "fired": true, "active": true }))]
        );

        // ⑤ 历史遗留的 bool true（作者把既有触发点改成可重复）→ 视为触发过且条件曾为真：
        //    不应当场重触发，要等条件先转假再转真。
        let mut legacy = serde_json::Map::new();
        legacy.insert("tr-repeat".to_string(), json!(true));
        legacy.insert("tr-once".to_string(), json!(true));
        let keep = eval(&flags, &legacy);
        assert!(keep.triggers.is_empty(), "旧 true 值不应当场重触发");
        assert!(keep.trigger_progress.is_empty(), "没有变化就不落 delta");
    }

    /// 一次性触发点一旦触发就**不再求值条件**（Lua 条件脚本不得被反复执行）——
    /// 与改动前的求值顺序逐字一致。
    #[test]
    fn fired_one_shot_trigger_does_not_reevaluate_condition() {
        let skeleton = json!([{ "id": "ch-1", "scenes": [{ "id": "sc-1", "triggers": [
            { "id": "b1", "condition": { "op": "lua", "script": "error('不该再跑')" } }
        ] }] }]);
        let host = LuaHost::new(1).unwrap();
        let lua_ctx = LuaHostContext { script_id: "cond".into(), ..Default::default() };
        let flags = BTreeMap::new();
        let goals = serde_json::Map::new();
        let mut triggers = serde_json::Map::new();
        triggers.insert("b1".to_string(), json!(true));
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: None,
            relationships: &[],
            scene_id: Some("sc-1"),
            encounters: &[],
            lua: Some((&host, &lua_ctx)),
        };
        let out = evaluate_skeleton_full(&skeleton, &ctx).expect("已触发的一次性触发点不得再求值条件");
        assert!(out.triggers.is_empty());
        assert!(out.trigger_progress.is_empty());
    }

    /// 兼容入口 evaluate_skeleton 仍返回 2 元组，且一次性触发点的进度值逐字不变。
    #[test]
    fn one_shot_trigger_progress_is_boolean_true() {
        let skeleton = json!([{ "id": "ch-1", "scenes": [{ "id": "sc-1", "triggers": [
            { "id": "b1", "condition": { "op": "flag_set", "flag": "f" } }
        ] }] }]);
        let mut flags = BTreeMap::new();
        flags.insert("f".to_string(), json!(true));
        let goals = serde_json::Map::new();
        let triggers = serde_json::Map::new();
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: None,
            relationships: &[],
            scene_id: Some("sc-1"),
            encounters: &[],
            lua: None,
        };
        let (new_goals, new_triggers) = evaluate_skeleton(&skeleton, &ctx).unwrap();
        assert!(new_goals.is_empty());
        assert_eq!(new_triggers, vec!["b1".to_string()]);
        let full = evaluate_skeleton_full(&skeleton, &ctx).unwrap();
        assert_eq!(full.trigger_progress, vec![("b1".to_string(), json!(true))]);
        // 一次性触发 delta 与历史逐字一致。
        assert_eq!(
            serde_json::to_value(trigger_progress_delta("b1", json!(true))).unwrap(),
            serde_json::to_value(trigger_delta("b1")).unwrap()
        );
    }
}
