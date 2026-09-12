//! 旧格式升格（CONTEXT.md「升格」）：读取历史数据时转换为当前结构，历史数据本身永不改写。
//!
//! 版本链（当前）：v1 → v2 → v3。
//!
//! - v1 → v2：骨架里的「节拍 beat」更名为「剧情触发点 trigger」
//!   - 故事书 JSON：`skeleton[].beats` / `skeleton[].scenes[].beats` → `triggers`
//!   - 条件表达式：`op: "beat_fired"` → `"trigger_fired"`；`beat_id` / `beat` → `trigger_id`
//!   - 命令日志 `EntityRef.kind: "beat"` → `"trigger"`
//!   - `DeltaDomain` 的 `beat` 由 serde alias 兼容（见 octopus-types）
//! - v2 → v3：持续状态提升为故事书顶层声明
//!   - `skill.effect.status`（内联 StatusDef 对象）→ 顶层 `statuses`，技能侧改写为 id 引用数组

use octopus_types::{EventEnvelope, PlayEvent};
use serde_json::Value;

/// 当前故事书 schema 版本。
pub const STORYBOOK_SCHEMA_VERSION: u64 = 3;

/// 就地把故事书 JSON 升格到当前结构（幂等）。
pub fn upcast_storybook(sb: &mut Value) {
    if !sb.is_object() {
        return;
    }

    // 1. 骨架：章节 / 场景的 beats → triggers
    if let Some(chapters) = sb.get_mut("skeleton").and_then(Value::as_array_mut) {
        for ch in chapters.iter_mut() {
            rename_key(ch, "beats", "triggers");
            if let Some(scenes) = ch.get_mut("scenes").and_then(Value::as_array_mut) {
                for sc in scenes.iter_mut() {
                    rename_key(sc, "beats", "triggers");
                }
            }
        }
    }

    // 1.5 目标 / 触发点缺 id 时回填稳定 id（导入的故事书常见）。缺 id 时
    //     evaluate_skeleton 会直接跳过，目标完成态永远无法记录。
    ensure_skeleton_ids(sb);

    // 2. 条件表达式：只改带 op 的条件节点，绝不泛化重命名 beat 键
    //    （属性维度 / 人物 attribute 的键可以合法地叫 beat，不能误伤）
    upcast_conditions(sb);

    // 3. 状态：技能内联 status 定义提升到顶层 statuses，技能侧改为 id 引用
    migrate_statuses(sb);

    // 4. schema_version → 当前版本
    if let Some(obj) = sb.as_object_mut() {
        if let Some(v) = obj.get("schema_version").and_then(Value::as_u64) {
            if v < STORYBOOK_SCHEMA_VERSION {
                obj.insert("schema_version".to_string(), Value::from(STORYBOOK_SCHEMA_VERSION));
            }
        }
    }
}

/// 给骨架里缺失 id 的 goal / trigger 回填稳定 id：
/// `{scene_id}#goal[{index}]` / `{scene_id}#trigger[{index}]`。
///
/// 导入或旧格式故事书常见没有 id；缺 id 时 `evaluate_skeleton` 会跳过该条，
/// 进度永远无法记录，前端也会拿到 `undefined` 键。幂等：已有非空 id 的条目原样保留。
pub fn ensure_skeleton_ids(sb: &mut Value) {
    let Some(chapters) = sb.get_mut("skeleton").and_then(Value::as_array_mut) else {
        return;
    };
    for ch in chapters.iter_mut() {
        let Some(scenes) = ch.get_mut("scenes").and_then(Value::as_array_mut) else {
            continue;
        };
        for sc in scenes.iter_mut() {
            let scene_id = sc
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let Some(scene_id) = scene_id else {
                continue; // 无场景 id 无从生成稳定前缀，留给校验报 missing_id
            };
            for (key, suffix) in [("goals", "goal"), ("triggers", "trigger")] {
                let Some(list) = sc.get_mut(key).and_then(Value::as_array_mut) else {
                    continue;
                };
                for (idx, item) in list.iter_mut().enumerate() {
                    let has_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    if has_id {
                        continue;
                    }
                    if let Some(obj) = item.as_object_mut() {
                        obj.insert(
                            "id".to_string(),
                            Value::String(format!("{scene_id}#{suffix}[{idx}]")),
                        );
                    }
                }
            }
        }
    }
}

/// v2 → v3：把技能效果内联的持续状态定义（StatusDef 对象）提升为故事书顶层 `statuses`，
/// 并把 `skill.effect.status` 改写为状态 id 引用数组。已是引用（字符串）的原样保留；
/// 顶层已存在的同 id 定义不覆盖（首个定义生效）。
fn migrate_statuses(sb: &mut Value) {
    // 无 schema_version 的局部草稿（如 API 幂等测试里的 patch）不做补键，保持原文幂等。
    let version = sb.get("schema_version").and_then(Value::as_u64).unwrap_or(0);
    if version == 0 {
        return;
    }
    let mut defs: Vec<Value> = sb
        .get("statuses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen: std::collections::HashSet<String> = defs
        .iter()
        .filter_map(|d| d.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();

    if let Some(skills) = sb.get_mut("skills").and_then(Value::as_array_mut) {
        for skill in skills.iter_mut() {
            let Some(status) = skill.pointer_mut("/effect/status") else {
                continue;
            };
            let Some(items) = status.as_array().cloned() else {
                continue;
            };
            let mut refs: Vec<Value> = Vec::new();
            for item in items {
                if let Some(id) = item.as_str() {
                    // 已是 id 引用，保持。
                    refs.push(Value::from(id));
                } else if let Some(id) = item.get("id").and_then(Value::as_str) {
                    // 内联 StatusDef：搬到顶层（同 id 首个定义生效）。
                    if seen.insert(id.to_string()) {
                        defs.push(item.clone());
                    }
                    refs.push(Value::from(id));
                }
            }
            *status = Value::Array(refs);
        }
    }

    // 无论是否迁移出定义，都保证顶层有 statuses（v1/v2 文档补齐空数组）。
    if let Some(obj) = sb.as_object_mut() {
        obj.insert("statuses".to_string(), Value::Array(defs));
    }
}

/// 条件节点里的 `beat_fired` / `beat_id` / `beat` → `trigger_*`。
fn upcast_conditions(v: &mut Value) {
    match v {
        Value::Object(map) => {
            if map.get("op").and_then(Value::as_str) == Some("beat_fired") {
                map.insert("op".to_string(), Value::from("trigger_fired"));
            }
            if map.contains_key("op") {
                if let Some(old) = map.remove("beat_id") {
                    map.entry("trigger_id").or_insert(old);
                }
                if let Some(old) = map.remove("beat") {
                    map.entry("trigger").or_insert(old);
                }
            }
            for child in map.values_mut() {
                upcast_conditions(child);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                upcast_conditions(item);
            }
        }
        _ => {}
    }
}

/// 把 from 键的值挪到 to 键（已有 to 时不覆盖，并丢弃遗留的 from）。
fn rename_key(node: &mut Value, from: &str, to: &str) {
    let Some(map) = node.as_object_mut() else {
        return;
    };
    if let Some(v) = map.remove(from) {
        map.entry(to.to_string()).or_insert(v);
    }
}

/// 旧格式实体引用 `kind: "beat"` → `"trigger"`。
pub fn upcast_entity_ref_kind(kind: &mut String) {
    if kind == "beat" {
        *kind = "trigger".to_string();
    }
}

/// 把一条命令日志事件里的旧引用升格。
pub fn upcast_event(env: &mut EventEnvelope) {
    if let PlayEvent::RoundStart(p) = &mut env.event {
        for r in p.input.refs.iter_mut() {
            upcast_entity_ref_kind(&mut r.kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upcasts_beats_key_and_conditions() {
        let mut sb = json!({
            "schema_version": 1,
            "skeleton": [{ "id": "ch-1", "scenes": [{
                "id": "sc-1",
                "beats": [{ "id": "b1", "title": "梦的怪象", "condition": { "op": "flag_set", "flag": "x" } }],
                "goals": [{ "id": "g1", "text": "t", "condition": { "op": "beat_fired", "beat_id": "b1" } }]
            }] }]
        });
        upcast_storybook(&mut sb);
        let scene = &sb["skeleton"][0]["scenes"][0];
        assert_eq!(sb["schema_version"], json!(3));
        assert!(scene.get("beats").is_none(), "beats 键应被移除");
        assert_eq!(scene["triggers"][0]["id"], json!("b1"));
        assert_eq!(scene["goals"][0]["condition"]["op"], json!("trigger_fired"));
        assert_eq!(scene["goals"][0]["condition"]["trigger_id"], json!("b1"));
    }

    #[test]
    fn upcast_is_idempotent_and_keeps_lookalike_keys() {
        let mut sb = json!({
            "schema_version": 3,
            "attribute_dimensions": [{ "key": "beat", "label": "节奏", "type": "number" }],
            "characters": [{ "id": "c1", "attributes": { "beat": 50 } }],
            "statuses": [],
            "skeleton": []
        });
        let before = sb.clone();
        upcast_storybook(&mut sb);
        assert_eq!(sb, before, "已升格的故事书不应被改写");
    }

    #[test]
    fn upcasts_inline_skill_status_to_top_level() {
        let mut sb = json!({
            "schema_version": 2,
            "skills": [
                { "id": "sk-a", "effect": { "status": [
                    { "id": "burn", "name": "灼烧", "duration": 3, "unit": "turns" },
                    { "id": "burn", "name": "重复定义", "duration": 9, "unit": "turns" }
                ] } },
                { "id": "sk-b", "effect": { "status": ["burn"] } }
            ]
        });
        upcast_storybook(&mut sb);
        assert_eq!(sb["schema_version"], json!(3));
        assert_eq!(sb["statuses"].as_array().unwrap().len(), 1, "同 id 只保留首个定义");
        assert_eq!(sb["statuses"][0]["name"], json!("灼烧"));
        assert_eq!(sb["skills"][0]["effect"]["status"], json!(["burn", "burn"]));
        assert_eq!(sb["skills"][1]["effect"]["status"], json!(["burn"]));
    }

    #[test]
    fn backfills_missing_goal_and_trigger_ids() {
        let mut sb = json!({
            "schema_version": 3,
            "skeleton": [{ "id": "ch-1", "scenes": [{
                "id": "sc-1",
                "title": "场景",
                "goals": [{ "text": "a" }, { "id": "g2", "text": "b" }],
                "triggers": [{ "title": "t" }]
            }] }]
        });
        upcast_storybook(&mut sb);
        let scene = &sb["skeleton"][0]["scenes"][0];
        assert_eq!(scene["goals"][0]["id"], json!("sc-1#goal[0]"));
        assert_eq!(scene["goals"][1]["id"], json!("g2"), "已有 id 不动");
        assert_eq!(scene["triggers"][0]["id"], json!("sc-1#trigger[0]"));

        let once = sb.clone();
        upcast_storybook(&mut sb);
        assert_eq!(sb, once, "id 回填必须幂等");
    }

    #[test]
    fn upcasts_legacy_entity_ref_kind() {
        let mut kind = "beat".to_string();
        upcast_entity_ref_kind(&mut kind);
        assert_eq!(kind, "trigger");
        let mut other = "character".to_string();
        upcast_entity_ref_kind(&mut other);
        assert_eq!(other, "character");
    }
}
