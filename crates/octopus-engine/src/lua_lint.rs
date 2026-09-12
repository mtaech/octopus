//! lua_lint：编辑期 Lua 静态预检（#23 ② 校验范围）。
//!
//! 只编译不执行：语法错误 + 字节码拒绝 + scoped env 白名单越权扫描。
//! 覆盖位置：skill.lua / skill.check.lua / world.check.lua / goal、trigger 条件里的 lua op。

use mlua::chunk::ChunkMode;
use mlua::{Lua, LuaOptions, StdLib};
use serde_json::Value;

/// 一条 Lua 预检问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaIssue {
    pub target: String,
    pub code: String,
    pub message: String,
}

/// 白名单之外的 Lua API（scoped env 不注入，出现即视为越权）。
const FORBIDDEN: &[&str] = &[
    "io.",
    "os.",
    "package.",
    "debug.",
    "require",
    "dofile",
    "loadfile",
    "math.random",
    "collectgarbage",
    "coroutine.",
];

/// 建一个与沙箱同构的最小 Lua 状态，用于编译体检。
pub fn new_lint_state() -> Result<Lua, String> {
    Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(|e| e.to_string())
}

/// 单脚本预检：语法 + 越权。
pub fn lint_script(lua: &Lua, script: &str) -> Result<(), String> {
    lua.load(script)
        .set_name("lint")
        .set_mode(ChunkMode::Text)
        .into_function()
        .map_err(|e| format!("Lua 语法错误：{e}"))?;
    for token in FORBIDDEN {
        if script.contains(token) {
            return Err(format!("Lua 使用了白名单之外的 API：{token}"));
        }
    }
    Ok(())
}

fn walk_cond(cond: &Value, target: &str, out: &mut Vec<(String, String, String)>) {
    match cond {
        Value::Array(arr) => {
            for item in arr {
                walk_cond(item, target, out);
            }
        }
        Value::Object(map) => {
            if map.get("op").and_then(Value::as_str) == Some("lua") {
                if let Some(script) = map.get("script").and_then(Value::as_str) {
                    out.push((target.to_string(), "condition.lua".to_string(), script.to_string()));
                }
            }
            for key in ["children", "child"] {
                if let Some(child) = map.get(key) {
                    walk_cond(child, target, out);
                }
            }
        }
        _ => {}
    }
}

/// 遍历整本故事书，收集所有 Lua 脚本并预检。
pub fn lint_storybook(sb: &Value) -> Vec<LuaIssue> {
    let mut scripts: Vec<(String, String, String)> = Vec::new();

    if let Some(skills) = sb.get("skills").and_then(Value::as_array) {
        for skill in skills {
            let id = skill.get("id").and_then(Value::as_str).unwrap_or("?");
            let target = format!("skill:{id}");
            if let Some(script) = skill.get("lua").and_then(Value::as_str) {
                if !script.trim().is_empty() {
                    scripts.push((target.clone(), "lua".to_string(), script.to_string()));
                }
            }
            if let Some(script) = skill.pointer("/check/lua").and_then(Value::as_str) {
                if !script.trim().is_empty() {
                    scripts.push((target.clone(), "check.lua".to_string(), script.to_string()));
                }
            }
        }
    }
    if let Some(script) = sb.pointer("/world/check/lua").and_then(Value::as_str) {
        if !script.trim().is_empty() {
            scripts.push(("world.check".to_string(), "check.lua".to_string(), script.to_string()));
        }
    }
    if let Some(chapters) = sb.get("skeleton").and_then(Value::as_array) {
        for chapter in chapters {
            let Some(scenes) = chapter.get("scenes").and_then(Value::as_array) else {
                continue;
            };
            for scene in scenes {
                if let Some(goals) = scene.get("goals").and_then(Value::as_array) {
                    for goal in goals {
                        let id = goal.get("id").and_then(Value::as_str).unwrap_or("?");
                        if let Some(cond) = goal.get("condition") {
                            walk_cond(cond, &format!("goal:{id}"), &mut scripts);
                        }
                    }
                }
                if let Some(triggers) = scene.get("triggers").and_then(Value::as_array) {
                    for trigger in triggers {
                        let id = trigger.get("id").and_then(Value::as_str).unwrap_or("?");
                        if let Some(cond) = trigger.get("condition") {
                            walk_cond(cond, &format!("trigger:{id}"), &mut scripts);
                        }
                    }
                }
            }
        }
    }

    let mut issues = Vec::new();
    let Ok(lua) = new_lint_state() else {
        return issues;
    };
    for (target, field, source) in scripts {
        if let Err(message) = lint_script(&lua, &source) {
            let code = if message.contains("白名单") {
                "lua_forbidden_api"
            } else {
                "lua_syntax_error"
            };
            issues.push(LuaIssue {
                target,
                code: code.to_string(),
                message: format!("{field}：{message}"),
            });
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_script_passes() {
        let lua = new_lint_state().unwrap();
        assert!(lint_script(&lua, "return host.get_attribute('str') >= 60").is_ok());
        assert!(lint_script(&lua, "local r = host.engine_rng(1, 20); return { total = r, margin = 0 }").is_ok());
    }

    #[test]
    fn syntax_error_and_forbidden_api_are_caught() {
        let lua = new_lint_state().unwrap();
        let err = lint_script(&lua, "return (").unwrap_err();
        assert!(err.contains("语法错误"), "{err}");
        let err = lint_script(&lua, "os.execute('rm -rf /')").unwrap_err();
        assert!(err.contains("白名单"), "{err}");
        let err = lint_script(&lua, "return math.random()").unwrap_err();
        assert!(err.contains("白名单"), "{err}");
        assert!(lint_script(&lua, "\u{1b}LuaQ").is_err());
    }

    #[test]
    fn storybook_scan_covers_all_lua_slots() {
        let sb = json!({
            "skills": [
                { "id": "sk-a", "lua": "return (" },
                { "id": "sk-b", "check": { "lua": "io.write('x')" } }
            ],
            "world": { "check": { "lua": "return 1" } },
            "skeleton": [{ "id": "ch-1", "scenes": [{
                "id": "sc-1",
                "goals": [{ "id": "g1", "condition": { "op": "all_of", "children": [
                    { "op": "flag_set", "flag": "x" },
                    { "op": "lua", "script": "return (" }
                ] } }],
                "triggers": [{ "id": "b1", "condition": { "op": "lua", "script": "os.time()" } }]
            }] }]
        });
        let issues = lint_storybook(&sb);
        assert_eq!(issues.len(), 4, "{issues:?}");
        assert!(issues.iter().any(|i| i.target == "skill:sk-a" && i.code == "lua_syntax_error"));
        assert!(issues.iter().any(|i| i.target == "skill:sk-b" && i.code == "lua_forbidden_api"));
        let g1 = issues.iter().find(|i| i.target == "goal:g1").expect("goal lua");
        assert_eq!(g1.code, "lua_syntax_error");
        assert!(issues.iter().any(|i| i.target == "trigger:b1" && i.code == "lua_forbidden_api"));
        // world.check 合法，不应有 issue
        assert!(!issues.iter().any(|i| i.target == "world.check"));
    }
}
