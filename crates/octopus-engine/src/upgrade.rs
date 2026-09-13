//! 存档版次迁移（#14 ②）：内嵌冻结故事书与目标已发布版次的差异计算 + 裁决校验。
//!
//! 纯函数、无 IO、无副作用——dry-run 与执行共用同一份计算，避免「报告看到的」
//! 与「真正执行的」漂移。编辑器纪律（#14）：实体 id 一经发布不可变，因此冲突只剩
//! 「id 消失」与「同名 id 定义变化」两类。

use std::collections::{BTreeMap, BTreeSet};

use octopus_types::{
    DispositionItem, UpgradeChange, UpgradeReport, UpgradeReportGroup, UpgradeGoneCharacter,
};
use serde_json::Value;

/// 自动处理的实体区：区名 / 消失动作 / 定义变化动作。
const SECTIONS: &[(&str, &str, &str)] = &[
    ("skills", "移除角色对该技能的引用", "技能定义已更新（运行时数值保留）"),
    ("items", "从角色背包移除该物品", "物品定义已更新（运行时数值保留）"),
    ("factions", "移除对该势力的引用", "势力定义已更新（运行时数值保留）"),
    ("locations", "移除对该地点的引用", "地点定义已更新（运行时数值保留）"),
    ("statuses", "移除该持续状态定义", "状态定义已更新（运行时数值保留）"),
];

/// 裁决校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispositionError {
    /// 以下人物消失尚未裁决（必须全部处置完才允许升级）。
    Missing(Vec<String>),
    /// 给一个并未消失（或不存在）的人物下达了处置。
    Unknown(String),
    /// 同一人物重复裁决。
    Duplicate(String),
}

impl DispositionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Missing(_) => "missing_dispositions",
            Self::Unknown(_) => "invalid_disposition",
            Self::Duplicate(_) => "invalid_disposition",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Missing(ids) => {
                format!("还有 {} 名人物未裁决处置方式：{}", ids.len(), ids.join("、"))
            }
            Self::Unknown(id) => format!("人物 {id} 并未在新版故事书中消失，无法裁决"),
            Self::Duplicate(id) => format!("人物 {id} 重复裁决"),
        }
    }
}

/// 取某个顶层数组区里 id -> 定义的映射（无 id 的条目跳过）。
fn defs_by_id<'a>(sb: &'a Value, key: &str) -> BTreeMap<String, &'a Value> {
    let mut out = BTreeMap::new();
    if let Some(arr) = sb.get(key).and_then(Value::as_array) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    out.insert(id.to_string(), item);
                }
            }
        }
    }
    out
}

fn name_of(def: &Value) -> String {
    def.get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("（未命名）")
        .to_string()
}

/// 计算 dry-run 报告：旧内嵌副本 -> 新已发布版次。纯函数。
pub fn compute_upgrade_report(old: &Value, new: &Value, from: u32, to: u32) -> UpgradeReport {
    let mut changes: Vec<UpgradeChange> = Vec::new();

    for (key, gone_action, changed_action) in SECTIONS {
        let old_defs = defs_by_id(old, key);
        let new_defs = defs_by_id(new, key);
        for (id, def) in &old_defs {
            match new_defs.get(id) {
                None => changes.push(UpgradeChange {
                    kind: (*key).to_string(),
                    id: id.clone(),
                    label: name_of(def),
                    action: (*gone_action).to_string(),
                }),
                Some(new_def) if *new_def != *def => changes.push(UpgradeChange {
                    kind: (*key).to_string(),
                    id: id.clone(),
                    label: name_of(new_def),
                    action: (*changed_action).to_string(),
                }),
                Some(_) => {}
            }
        }
    }

    // 人物：id 消失需要裁决；仅定义变化是参考项（实例运行时数值保留）。
    let old_chars = defs_by_id(old, "characters");
    let new_chars = defs_by_id(new, "characters");
    let mut gone_characters: Vec<UpgradeGoneCharacter> = Vec::new();
    for (id, def) in &old_chars {
        match new_chars.get(id) {
            None => gone_characters.push(UpgradeGoneCharacter {
                character_id: id.clone(),
                name: name_of(def),
                reason: Some("新版故事书中已移除该人物（模板 id 消失）".to_string()),
            }),
            Some(new_def) if *new_def != *def => changes.push(UpgradeChange {
                kind: "characters".to_string(),
                id: id.clone(),
                label: name_of(new_def),
                action: "人物定义已更新（运行时实例与数值保留）".to_string(),
            }),
            Some(_) => {}
        }
    }

    UpgradeReport {
        from_revision: from,
        to_revision: to,
        groups: UpgradeReportGroup { changes, gone_characters },
    }
}

/// 严格校验裁决集合：必须与报告里的 gone_characters 一一对应。
///
/// 任何缺失 / 多余 / 重复都拒绝，绝不做「默认处置」——半裁决升级会让旧实例
/// 在不知情的情况下被丢弃。
pub fn validate_dispositions(
    report: &UpgradeReport,
    items: &[DispositionItem],
) -> Result<(), DispositionError> {
    let expected: BTreeSet<&str> = report
        .groups
        .gone_characters
        .iter()
        .map(|g| g.character_id.as_str())
        .collect();

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for item in items {
        if !expected.contains(item.character_id.as_str()) {
            return Err(DispositionError::Unknown(item.character_id.clone()));
        }
        if !seen.insert(item.character_id.clone()) {
            return Err(DispositionError::Duplicate(item.character_id.clone()));
        }
    }
    let missing: Vec<String> = report
        .groups
        .gone_characters
        .iter()
        .filter(|g| !seen.contains(&g.character_id))
        .map(|g| g.character_id.clone())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(DispositionError::Missing(missing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_types::UpgradeDisposition;
    use serde_json::json;

    fn old_sb() -> Value {
        json!({
            "skills": [{ "id": "sk-a", "name": "痛饮" }, { "id": "sk-b", "name": "劈砍" }],
            "items": [{ "id": "it-a", "name": "火把" }],
            "locations": [{ "id": "loc-a", "name": "酒馆" }],
            "characters": [
                { "id": "char-a", "name": "米拉", "kind": "pc" },
                { "id": "char-kael", "name": "凯尔", "kind": "npc" }
            ]
        })
    }

    #[test]
    fn reports_gone_character_and_changed_skill() {
        let new = json!({
            "skills": [{ "id": "sk-a", "name": "痛饮", "cost": [{ "resource": "gold", "amount": 1 }] }, { "id": "sk-b", "name": "劈砍" }],
            "items": [{ "id": "it-a", "name": "火把" }],
            "locations": [{ "id": "loc-a", "name": "酒馆" }],
            "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }]
        });
        let rep = compute_upgrade_report(&old_sb(), &new, 1, 3);
        assert_eq!(rep.from_revision, 1);
        assert_eq!(rep.to_revision, 3);
        assert_eq!(rep.groups.gone_characters.len(), 1);
        assert_eq!(rep.groups.gone_characters[0].character_id, "char-kael");
        assert_eq!(rep.groups.gone_characters[0].name, "凯尔");
        let sk_change = rep
            .groups
            .changes
            .iter()
            .find(|c| c.kind == "skills" && c.id == "sk-a")
            .expect("技能定义变化应出现在报告里");
        assert!(sk_change.action.contains("定义已更新"));
    }

    #[test]
    fn reports_gone_item_and_location() {
        let new = json!({
            "skills": [{ "id": "sk-a", "name": "痛饮" }, { "id": "sk-b", "name": "劈砍" }],
            "items": [],
            "locations": [],
            "characters": [
                { "id": "char-a", "name": "米拉", "kind": "pc" },
                { "id": "char-kael", "name": "凯尔", "kind": "npc" }
            ]
        });
        let rep = compute_upgrade_report(&old_sb(), &new, 1, 2);
        assert!(rep.groups.gone_characters.is_empty());
        assert!(rep.groups.changes.iter().any(|c| c.kind == "items" && c.id == "it-a"));
        assert!(rep.groups.changes.iter().any(|c| c.kind == "locations" && c.id == "loc-a"));
    }

    #[test]
    fn validates_dispositions_strictly() {
        let report = compute_upgrade_report(
            &old_sb(),
            &json!({ "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }] }),
            1,
            2,
        );
        // 缺失 -> Missing
        let err = validate_dispositions(&report, &[]).unwrap_err();
        assert_eq!(err, DispositionError::Missing(vec!["char-kael".to_string()]));
        // 多余 / 未知 -> Unknown
        let err = validate_dispositions(
            &report,
            &[DispositionItem {
                character_id: "char-nope".to_string(),
                disposition: UpgradeDisposition::Freeze,
            }],
        )
        .unwrap_err();
        assert_eq!(err, DispositionError::Unknown("char-nope".to_string()));
        // 重复 -> Duplicate
        let err = validate_dispositions(
            &report,
            &[
                DispositionItem { character_id: "char-kael".to_string(), disposition: UpgradeDisposition::Freeze },
                DispositionItem { character_id: "char-kael".to_string(), disposition: UpgradeDisposition::Departure },
            ],
        )
        .unwrap_err();
        assert_eq!(err, DispositionError::Duplicate("char-kael".to_string()));
        // 完整 -> Ok
        validate_dispositions(
            &report,
            &[DispositionItem {
                character_id: "char-kael".to_string(),
                disposition: UpgradeDisposition::Departure,
            }],
        )
        .unwrap();
    }

    #[test]
    fn no_dispositions_needed_when_nothing_gone() {
        let report = compute_upgrade_report(&old_sb(), &old_sb(), 1, 2);
        assert!(report.groups.gone_characters.is_empty());
        assert!(report.groups.changes.is_empty());
        validate_dispositions(&report, &[]).unwrap();
    }
}
