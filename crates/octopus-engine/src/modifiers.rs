//! 修正来源（#2 / #5）：挂接定义与已装备物品对属性维度的增量。
//!
//! 与前端 frontend/src/lib/derived.ts 的 collectModifiers 同口径：
//! 叠加规则 add 累加 → max 取高 → set 覆盖。

use std::collections::HashMap;

use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AttrModifier {
    pub add: i64,
    pub max: Option<i64>,
    pub set: Option<i64>,
}

impl AttrModifier {
    pub fn apply(&self, base: f64) -> f64 {
        let mut v = base + self.add as f64;
        if let Some(mx) = self.max {
            v = v.max(mx as f64);
        }
        if let Some(s) = self.set {
            v = s as f64;
        }
        v
    }
}

fn push_mods(out: &mut Vec<(String, i64, String)>, mods: Option<&Value>) {
    let Some(arr) = mods.and_then(|v| v.as_array()) else { return };
    for m in arr {
        let Some(target) = m.get("target").and_then(|v| v.as_str()) else { continue };
        let value = m.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
        let op = m.get("op").and_then(|v| v.as_str()).unwrap_or("add").to_string();
        out.push((target.to_string(), value, op));
    }
}

/// 从故事书算「角色模板 id → 属性修正」：挂接定义 + 已装备物品的 modifiers。
pub fn attribute_modifiers(storybook: &Value) -> HashMap<String, HashMap<String, AttrModifier>> {
    // definition id → modifiers
    let mut def_mods: HashMap<String, Vec<(String, i64, String)>> = HashMap::new();
    if let Some(defs) = storybook.get("definitions").and_then(|v| v.as_array()) {
        for d in defs {
            if let Some(id) = d.get("id").and_then(|v| v.as_str()) {
                let mut out = Vec::new();
                push_mods(&mut out, d.get("modifiers"));
                def_mods.insert(id.to_string(), out);
            }
        }
    }
    // item id → modifiers
    let mut item_mods: HashMap<String, Vec<(String, i64, String)>> = HashMap::new();
    if let Some(items) = storybook.get("items").and_then(|v| v.as_array()) {
        for it in items {
            if let Some(id) = it.get("id").and_then(|v| v.as_str()) {
                let mut out = Vec::new();
                push_mods(&mut out, it.get("modifiers"));
                item_mods.insert(id.to_string(), out);
            }
        }
    }

    let mut result: HashMap<String, HashMap<String, AttrModifier>> = HashMap::new();
    let Some(chars) = storybook.get("characters").and_then(|v| v.as_array()) else { return result };
    for c in chars {
        let Some(tid) = c.get("id").and_then(|v| v.as_str()) else { continue };
        let mut mods: Vec<(String, i64, String)> = Vec::new();
        if let Some(atts) = c.get("attachments").and_then(|v| v.as_object()) {
            for ids in atts.values() {
                if let Some(list) = ids.as_array() {
                    for id in list {
                        if let Some(k) = id.as_str() {
                            if let Some(m) = def_mods.get(k) { mods.extend(m.iter().cloned()); }
                        }
                    }
                }
            }
        }
        if let Some(equipped) = c.get("equipped").and_then(|v| v.as_array()) {
            for e in equipped {
                if let Some(k) = e.as_str() {
                    if let Some(m) = item_mods.get(k) { mods.extend(m.iter().cloned()); }
                }
            }
        }
        let mut by_attr: HashMap<String, AttrModifier> = HashMap::new();
        for (target, value, op) in mods {
            let entry = by_attr.entry(target).or_default();
            match op.as_str() {
                "set" => entry.set = Some(value),
                "max" => entry.max = Some(entry.max.map_or(value, |m| m.max(value))),
                _ => entry.add += value,
            }
        }
        result.insert(tid.to_string(), by_attr);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_attachment_and_equipment_modifiers() {
        let sb = json!({
            "definitions": [ { "id": "race-human", "kind": "race", "name": "人类", "modifiers": [ { "target": "str", "value": 1 }, { "target": "dex", "value": 1 } ] } ],
            "items": [ { "id": "it-shield", "name": "盾牌", "slot": "shield", "modifiers": [ { "target": "ac", "value": 2 } ] } ],
            "characters": [ { "id": "char-1", "attachments": { "race": ["race-human"] }, "equipped": ["it-shield"] } ]
        });
        let m = attribute_modifiers(&sb);
        let c = m.get("char-1").expect("character modifiers");
        assert_eq!(c.get("str").copied().unwrap_or_default().add, 1);
        assert_eq!(c.get("dex").copied().unwrap_or_default().add, 1);
        assert_eq!(c.get("ac").copied().unwrap_or_default().add, 2);
    }

    #[test]
    fn stacking_is_add_then_max_then_set() {
        let m = AttrModifier { add: 2, max: Some(5), set: Some(9) };
        assert_eq!(m.apply(10.0), 9.0, "set 覆盖一切");
        let m2 = AttrModifier { add: 2, max: Some(15), set: None };
        assert_eq!(m2.apply(10.0), 15.0, "max 在 add 之后取高");
        let m3 = AttrModifier { add: -3, max: None, set: None };
        assert_eq!(m3.apply(10.0), 7.0);
    }
}
