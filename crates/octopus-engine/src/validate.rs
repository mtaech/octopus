//! 故事书静态校验纯函数（#01 / #23 ②）。

use std::collections::{HashMap, HashSet};

use octopus_types::{CondExpr, IssueSeverity, ValidateResult, ValidationIssue};
use serde_json::Value;

use crate::protocol::{is_known_intent, ProtocolMode, KNOWN_INTENTS, NARRATIVE_INTENTS};

/// 协议 instructions 的建议长度上限（超出记 Warning）：它会随系统层注入并计入 token 预算。
const PROTOCOL_INSTRUCTIONS_WARN_CHARS: usize = 4000;
/// 协议 Lua 源码的建议长度上限（超出记 Warning）。
const PROTOCOL_LUA_WARN_CHARS: usize = 20000;

/// 校验故事书静态结构、字段与引用完整性
pub fn validate_storybook(sb: &Value) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if !sb.is_object() {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            code: "invalid_storybook".to_string(),
            target: None,
            message: "Storybook root must be a JSON object".to_string(),
            related_refs: None,
        });
        return issues;
    }

    // 1. meta: check `id` and `title` are non-empty strings. If missing or empty, emit IssueSeverity::Error with code "missing_title" or "missing_id".
    let meta = sb.get("meta");
    let meta_id = meta
        .and_then(|m| m.get("id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if meta_id.is_empty() {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            code: "missing_id".to_string(),
            target: Some("meta.id".to_string()),
            message: "Storybook meta.id must be a non-empty string".to_string(),
            related_refs: None,
        });
    }

    let meta_title = meta
        .and_then(|m| m.get("title"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if meta_title.is_empty() {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            code: "missing_title".to_string(),
            target: Some("meta.title".to_string()),
            message: "Storybook meta.title must be a non-empty string".to_string(),
            related_refs: None,
        });
    }

    // 2. schema_version: must be >= 1. If not, emit Error with code "invalid_schema_version".
    let schema_version = sb
        .get("schema_version")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if schema_version < 1 {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            code: "invalid_schema_version".to_string(),
            target: Some("schema_version".to_string()),
            message: "schema_version must be >= 1".to_string(),
            related_refs: None,
        });
    }

    // 3. attribute_dimensions: each must have `key` (string), `label`, `type` (one of "number", "enum", "text"). Check for duplicate keys (emit Error "duplicate_attribute_key").
    let mut seen_dim_keys = HashSet::new();
    if let Some(dims) = sb.get("attribute_dimensions").and_then(|v| v.as_array()) {
        for (i, dim) in dims.iter().enumerate() {
            let key = dim
                .get("key")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let target = if key.is_empty() {
                format!("attribute_dimension[{i}]")
            } else {
                format!("attribute_dimension:{key}")
            };

            if key.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_attribute_key".to_string(),
                    target: Some(target.clone()),
                    message: "Attribute dimension key must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !seen_dim_keys.insert(key.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_attribute_key".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate attribute dimension key: '{key}'"),
                    related_refs: Some(vec![key.to_string()]),
                });
            }

            let label = dim
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if label.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_attribute_label".to_string(),
                    target: Some(target.clone()),
                    message: "Attribute dimension label must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }

            let type_str = dim.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if !matches!(type_str, "number" | "enum" | "text") {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "invalid_attribute_type".to_string(),
                    target: Some(target),
                    message: format!(
                        "Attribute dimension type '{type_str}' is invalid; must be 'number', 'enum', or 'text'"
                    ),
                    related_refs: None,
                });
            }
        }
    }

    // 4. locations: each location has `id`, `name`. Check duplicate IDs (emit Error "duplicate_location_id").
    // If `parent_id` is set, it must point to an existing location in `locations` and not be a self-reference (emit Error "dangling_location_parent_ref").
    let location_slice: Vec<&Value> =
        if let Some(arr) = sb.pointer("/world/locations").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else if let Some(arr) = sb.get("locations").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else {
            Vec::new()
        };

    let mut location_ids = HashSet::new();
    for (i, loc) in location_slice.iter().enumerate() {
        let id = loc
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        let name = loc
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");

        let target = if id.is_empty() {
            format!("location[{i}]")
        } else {
            format!("location:{id}")
        };

        if id.is_empty() {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                code: "missing_id".to_string(),
                target: Some(target.clone()),
                message: "Location id must be a non-empty string".to_string(),
                related_refs: None,
            });
        } else if !location_ids.insert(id.to_string()) {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                code: "duplicate_location_id".to_string(),
                target: Some(target.clone()),
                message: format!("Duplicate location id: '{id}'"),
                related_refs: Some(vec![id.to_string()]),
            });
        }

        if name.is_empty() {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                code: "missing_name".to_string(),
                target: Some(target),
                message: "Location name must be a non-empty string".to_string(),
                related_refs: None,
            });
        }
    }

    // Check parent_id for locations
    for (i, loc) in location_slice.iter().enumerate() {
        let id = loc
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        let target = if id.is_empty() {
            format!("location[{i}]")
        } else {
            format!("location:{id}")
        };

        if let Some(parent_id_val) = loc.get("parent_id") {
            if !parent_id_val.is_null() {
                if let Some(pid) = parent_id_val.as_str() {
                    let pid_trim = pid.trim();
                    if !pid_trim.is_empty() {
                        if pid_trim == id || !location_ids.contains(pid_trim) {
                            issues.push(ValidationIssue {
                                severity: IssueSeverity::Error,
                                code: "dangling_location_parent_ref".to_string(),
                                target: Some(target),
                                message: format!(
                                    "Location parent_id '{pid_trim}' is invalid (self-reference or non-existent)"
                                ),
                                related_refs: Some(vec![pid_trim.to_string()]),
                            });
                        }
                    }
                }
            }
        }
    }

    // 6. characters: each character has `id`, `name`, `kind` ("pc" | "npc"). Check duplicate IDs (emit Error "duplicate_character_id").
    let mut character_ids = HashSet::new();
    if let Some(chars) = sb.get("characters").and_then(|v| v.as_array()) {
        for (i, c) in chars.iter().enumerate() {
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let kind = c.get("kind").and_then(|v| v.as_str()).unwrap_or("");

            let target = if id.is_empty() {
                format!("character[{i}]")
            } else {
                format!("character:{id}")
            };

            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Character id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !character_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_character_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate character id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }

            if name.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_name".to_string(),
                    target: Some(target.clone()),
                    message: "Character name must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }

            if !matches!(kind, "pc" | "npc") {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "invalid_character_kind".to_string(),
                    target: Some(target.clone()),
                    message: format!("Character kind '{kind}' is invalid; must be 'pc' or 'npc'"),
                    related_refs: None,
                });
            }

            // 人物写作提示（借鉴 SillyTavern 角色卡指南）：描写宜精不宜长，对话示例最关键。
            let bg_len = c.get("background").and_then(Value::as_str).unwrap_or("").chars().count();
            let pers_len = c.get("personality").and_then(Value::as_str).unwrap_or("").chars().count();
            let narrative_len = bg_len + pers_len;
            if narrative_len > 1000 {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "character_description_too_long".to_string(),
                    target: Some(target.clone()),
                    message: format!(
                        "人物「{name}」背景 + 性格共 {narrative_len} 字，偏长（建议 300–500 字）：过长的描写会挤压对话上下文"
                    ),
                    related_refs: None,
                });
            }
            let has_examples = c
                .get("example_dialogues")
                .and_then(Value::as_str)
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_examples && narrative_len > 0 {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "character_missing_example_dialogues".to_string(),
                    target: Some(target.clone()),
                    message: format!(
                        "人物「{name}」还没写「对话示例」——3-5 轮示范口吻的对话是改变 AI 语气最有效的手段"
                    ),
                    related_refs: None,
                });
            }
        }
    }

    // 6b. statuses: 顶层持续状态声明（id/name 必填、id 不重复、duration ≥ 0）。
    let mut status_ids = HashSet::new();
    if let Some(statuses) = sb.get("statuses").and_then(Value::as_array) {
        for (i, st) in statuses.iter().enumerate() {
            let id = st.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let name = st.get("name").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let target = if id.is_empty() { format!("status[{i}]") } else { format!("status:{id}") };
            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Status id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !status_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_status_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate status id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }
            if name.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_name".to_string(),
                    target: Some(target.clone()),
                    message: "Status name must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }
            if st.get("duration").and_then(Value::as_i64).is_some_and(|d| d < 0) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "invalid_status_duration".to_string(),
                    target: Some(target),
                    message: "Status duration must be >= 0".to_string(),
                    related_refs: None,
                });
            }
        }
    }

    // 6c. lore: 世界词条（id 必填且不重复；非常驻需有触发词；内容宜短）。
    let mut lore_ids = HashSet::new();
    if let Some(lore) = sb.get("lore").and_then(Value::as_array) {
        for (i, l) in lore.iter().enumerate() {
            let id = l.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let title = l.get("title").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let content = l.get("content").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let constant = l.get("constant").and_then(Value::as_bool).unwrap_or(false);
            let enabled = l.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let label = if title.is_empty() { id } else { title };
            let target = if id.is_empty() { format!("lore[{i}]") } else { format!("lore:{id}") };
            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Lore id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !lore_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_lore_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate lore id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }
            if title.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "missing_lore_title".to_string(),
                    target: Some(target.clone()),
                    message: "词条建议填写标题，便于编辑器检索".to_string(),
                    related_refs: None,
                });
            }
            if content.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_lore_content".to_string(),
                    target: Some(target.clone()),
                    message: format!("词条「{label}」缺少内容"),
                    related_refs: None,
                });
            } else {
                let len = content.chars().count();
                if len > 300 {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Warning,
                        code: "lore_content_too_long".to_string(),
                        target: Some(target.clone()),
                        message: format!(
                            "词条「{label}」有 {len} 字，偏长（建议 3-5 句）：世界词条是小抄，不是设定集"
                        ),
                        related_refs: None,
                    });
                }
            }
            let has_keys = l
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| a.iter().any(|k| k.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)))
                .unwrap_or(false);
            if enabled && !constant && !has_keys {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "lore_without_trigger".to_string(),
                    target: Some(target.clone()),
                    message: format!("词条「{label}」既非常驻也没有触发词，永远不会被注入"),
                    related_refs: None,
                });
            }
        }
    }


    // Flags for conditions
    let mut flag_keys = HashSet::new();
    if let Some(flags) = sb.get("flags").and_then(|v| v.as_array()) {
        for flag in flags {
            if let Some(s) = flag.as_str() {
                flag_keys.insert(s.to_string());
            } else if let Some(k) = flag.get("key").and_then(|v| v.as_str()) {
                flag_keys.insert(k.to_string());
            } else if let Some(id) = flag.get("id").and_then(|v| v.as_str()) {
                flag_keys.insert(id.to_string());
            }
        }
    }

    // Collect all trigger IDs across skeleton
    let mut all_trigger_ids = HashSet::new();
    if let Some(chapters) = sb.get("skeleton").and_then(|v| v.as_array()) {
        for ch in chapters {
            if let Some(scenes) = ch.get("scenes").and_then(|v| v.as_array()) {
                for sc in scenes {
                    if let Some(triggers) = sc.get("triggers").and_then(|v| v.as_array()) {
                        for t in triggers {
                            if let Some(tid) = t.get("id").and_then(|v| v.as_str()).map(str::trim) {
                                if !tid.is_empty() {
                                    all_trigger_ids.insert(tid.to_string());
                                }
                            }
                        }
                    }
                }
            }
            if let Some(triggers) = ch.get("triggers").and_then(|v| v.as_array()) {
                for t in triggers {
                    if let Some(tid) = t.get("id").and_then(|v| v.as_str()).map(str::trim) {
                        if !tid.is_empty() {
                            all_trigger_ids.insert(tid.to_string());
                        }
                    }
                }
            }
        }
    }

    // 6d. narrative: 叙述段（槽位合法 / id 不重复 / 文本非空 / scope 引用有效 / when / 变体组）。
    let mut narrative_ids = HashSet::new();
    if let Some(sections) = sb.pointer("/narrative/sections").and_then(Value::as_array) {
        for (i, s) in sections.iter().enumerate() {
            let id = s.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let title = s.get("title").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let label = if title.is_empty() { id } else { title };
            let target = if id.is_empty() { format!("narrative[{i}]") } else { format!("narrative:{id}") };
            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Narrative section id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !narrative_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_narrative_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate narrative section id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }
            let slot = s.get("slot").and_then(Value::as_str).unwrap_or("");
            if !matches!(slot, "world" | "style" | "behavior" | "closing") {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "invalid_narrative_slot".to_string(),
                    target: Some(target.clone()),
                    message: format!("叙述段「{label}」槽位 '{slot}' 非法；应为 world / style / behavior / closing"),
                    related_refs: None,
                });
            }
            // 文本 / 变体组：variants 非空即视为「有内容」；text 与 variants 互斥（见下方 P1 规则）。
            let variants = s.get("variants").and_then(Value::as_array);
            let has_variants = variants.is_some_and(|v| !v.is_empty());
            let enabled = s.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let text = s.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if enabled && text.is_empty() && !has_variants {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_narrative_text".to_string(),
                    target: Some(target.clone()),
                    message: format!("叙述段「{label}」已启用但没有内容"),
                    related_refs: None,
                });
            }
            match s.get("scope") {
                None => {}
                Some(Value::String(v)) => {
                    if !matches!(v.as_str(), "story" | "character" | "both") {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "invalid_narrative_scope".to_string(),
                            target: Some(target.clone()),
                            message: format!("叙述段「{label}」scope '{v}' 非法；应为 story / character / both 或 {{characterId}}"),
                            related_refs: None,
                        });
                    }
                }
                Some(Value::Object(o)) => {
                    let cid = o.get("characterId").and_then(Value::as_str).map(str::trim).unwrap_or("");
                    if cid.is_empty() || !character_ids.contains(cid) {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "dangling_narrative_scope_ref".to_string(),
                            target: Some(target.clone()),
                            message: format!("叙述段「{label}」指定了不存在的人物 '{cid}'"),
                            related_refs: if cid.is_empty() { None } else { Some(vec![cid.to_string()]) },
                        });
                    }
                }
                _ => {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        code: "invalid_narrative_scope".to_string(),
                        target: Some(target.clone()),
                        message: format!("叙述段「{label}」scope 形状非法"),
                        related_refs: None,
                    });
                }
            }

            // P1：when 条件——形状按 CondExpr 反序列化把关，引用复用既有条件校验器（触发点 /
            // 地点引用悬空 = Error，未声明 flag = Warning）。非法形状直接挡发布。
            if let Some(when) = s.get("when").filter(|v| !v.is_null()) {
                if serde_json::from_value::<CondExpr>(when.clone()).is_err() {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        code: "invalid_narrative_when".to_string(),
                        target: Some(target.clone()),
                        message: format!("叙述段「{label}」when 条件形状非法（应为 CondExpr 条件树）"),
                        related_refs: None,
                    });
                } else {
                    validate_condition(
                        when,
                        &target,
                        &flag_keys,
                        &all_trigger_ids,
                        &location_ids,
                        &mut issues,
                    );
                }
            }

            // P1：变体组规则——text 与 variants 互斥；key 唯一；defaultVariant 必须命中某个 key；
            // 每个变体都要有内容（否则解析出的那个变体会静默不注入）。
            if !text.is_empty() && has_variants {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "narrative_text_variants_conflict".to_string(),
                    target: Some(target.clone()),
                    message: format!("叙述段「{label}」不能同时声明 text 与 variants；二者互斥"),
                    related_refs: None,
                });
            }
            let default_variant = s
                .get("defaultVariant")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if let Some(list) = variants {
                let mut variant_keys = HashSet::new();
                for (vi, v) in list.iter().enumerate() {
                    let key = v.get("key").and_then(Value::as_str).map(str::trim).unwrap_or("");
                    if key.is_empty() {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "missing_variant_key".to_string(),
                            target: Some(target.clone()),
                            message: format!("叙述段「{label}」第 {} 个变体缺少 key", vi + 1),
                            related_refs: None,
                        });
                    } else if !variant_keys.insert(key.to_string()) {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "duplicate_variant_key".to_string(),
                            target: Some(target.clone()),
                            message: format!("叙述段「{label}」变体 key 重复：'{key}'"),
                            related_refs: Some(vec![key.to_string()]),
                        });
                    }
                    let vtext = v.get("text").and_then(Value::as_str).unwrap_or("").trim();
                    if vtext.is_empty() {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "missing_variant_text".to_string(),
                            target: Some(target.clone()),
                            message: format!("叙述段「{label}」变体 '{key}' 没有内容"),
                            related_refs: if key.is_empty() { None } else { Some(vec![key.to_string()]) },
                        });
                    }
                }
                if default_variant.is_empty() {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        code: "missing_default_variant".to_string(),
                        target: Some(target.clone()),
                        message: format!("叙述段「{label}」声明了 variants 但没有 defaultVariant"),
                        related_refs: None,
                    });
                } else if !variant_keys.contains(default_variant) {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        code: "invalid_default_variant".to_string(),
                        target: Some(target.clone()),
                        message: format!("叙述段「{label}」defaultVariant '{default_variant}' 不在 variants 中"),
                        related_refs: Some(vec![default_variant.to_string()]),
                    });
                }
            } else if !default_variant.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "default_variant_without_variants".to_string(),
                    target: Some(target.clone()),
                    message: format!("叙述段「{label}」声明了 defaultVariant 但没有 variants，将被忽略"),
                    related_refs: None,
                });
            }
        }
    }

    // 6d′. narrative.display（P3）：思考草稿的展示策略。
    // 纯展示字段、引擎不读；但非法值会让前端静默回落 folded，故必须在发布门拦成 Error。
    if let Some(display) = sb.pointer("/narrative/display").filter(|v| !v.is_null()) {
        if let Some(draft) = display.get("draft").filter(|v| !v.is_null()) {
            let v = draft.as_str().map(str::trim).unwrap_or("");
            if !matches!(v, "folded" | "hidden") {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "invalid_narrative_display_draft".to_string(),
                    target: Some("narrative.display".to_string()),
                    message: format!("思考草稿展示策略 '{v}' 非法；应为 folded / hidden"),
                    related_refs: None,
                });
            }
        }
    }

    // 6e. narrative.protocol: 输出协议（P2）——静态规则。
    // validate.rs 是纯函数、不执行 Lua；这里只做语法 / 越权预检与形状检查，
    // 动态一致性（样例跑）由 api 发布门调用 check_protocol_conformance。
    if let Some(proto) = sb.pointer("/narrative/protocol").filter(|v| !v.is_null()) {
        let target = Some("narrative.protocol".to_string());
        let mode_raw = proto.get("mode").and_then(Value::as_str).map(str::trim).unwrap_or("");
        let intents = proto.get("intents").and_then(Value::as_array);
        let instructions = proto.get("instructions").and_then(Value::as_str);
        let lua = proto
            .get("lua")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match ProtocolMode::parse(mode_raw) {
            Some(ProtocolMode::Default) => {
                // 显式 default = 引擎默认协议；intents / lua 一律忽略。
            }
            Some(ProtocolMode::Declarative) => match intents.filter(|l| !l.is_empty()) {
                None => issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_protocol_intents".to_string(),
                    target: target.clone(),
                    message: "declarative 协议必须声明至少一个 intents（引擎已知意图的子集）".to_string(),
                    related_refs: None,
                }),
                Some(list) => {
                    let mut has_narrative = false;
                    for item in list {
                        let name = item.as_str().map(str::trim).unwrap_or("");
                        if name.is_empty() {
                            continue;
                        }
                        if !is_known_intent(name) {
                            issues.push(ValidationIssue {
                                severity: IssueSeverity::Error,
                                code: "unknown_protocol_intent".to_string(),
                                target: target.clone(),
                                message: format!(
                                    "协议白名单含引擎未知的意图 '{name}'；可用：{}",
                                    KNOWN_INTENTS.join(" / ")
                                ),
                                related_refs: Some(vec![name.to_string()]),
                            });
                        }
                        if NARRATIVE_INTENTS.contains(&name) {
                            has_narrative = true;
                        }
                    }
                    if !has_narrative {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "missing_narrative_intent".to_string(),
                            target: target.clone(),
                            message: "declarative 协议至少要声明一个叙事意图（narrate / speak / emote），否则回合会空转".to_string(),
                            related_refs: None,
                        });
                    }
                }
            },
            Some(ProtocolMode::Lua) => match lua {
                None => issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_protocol_lua".to_string(),
                    target: target.clone(),
                    message: "lua 协议必须提供 lua 插件源码".to_string(),
                    related_refs: None,
                }),
                Some(src) => {
                    if let Ok(lint_state) = crate::lua_lint::new_lint_state() {
                        if let Err(message) = crate::lua_lint::lint_script(&lint_state, src) {
                            issues.push(ValidationIssue {
                                severity: IssueSeverity::Error,
                                code: "protocol_lua_invalid".to_string(),
                                target: target.clone(),
                                message: format!("协议 Lua 未通过静态预检：{message}"),
                                related_refs: None,
                            });
                        }
                    }
                    // 粗检两个必选入口；精确一致性在发布时由 check_protocol_conformance 跑。
                    let mut missing = Vec::new();
                    if !src.contains("protocol.preamble") {
                        missing.push("protocol.preamble".to_string());
                    }
                    if !src.contains("protocol.parse") {
                        missing.push("protocol.parse".to_string());
                    }
                    if !missing.is_empty() {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Warning,
                            code: "protocol_missing_functions".to_string(),
                            target: target.clone(),
                            message: format!(
                                "协议 Lua 文本里未发现 {} 的定义（发布时会用固定样例做精确一致性检查）",
                                missing.join(" 与 ")
                            ),
                            related_refs: Some(missing),
                        });
                    }
                    if src.chars().count() > PROTOCOL_LUA_WARN_CHARS {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Warning,
                            code: "protocol_lua_too_long".to_string(),
                            target: target.clone(),
                            message: format!(
                                "协议 Lua 源码 {} 字，偏长（建议 {} 字以内）",
                                src.chars().count(),
                                PROTOCOL_LUA_WARN_CHARS
                            ),
                            related_refs: None,
                        });
                    }
                }
            },
            None => issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                code: "invalid_protocol_mode".to_string(),
                target: target.clone(),
                message: format!("协议 mode '{mode_raw}' 非法；应为 default / declarative / lua"),
                related_refs: None,
            }),
        }
        if let Some(ins) = instructions {
            if ins.chars().count() > PROTOCOL_INSTRUCTIONS_WARN_CHARS {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "protocol_instructions_too_long".to_string(),
                    target: target.clone(),
                    message: format!(
                        "协议说明 {} 字，偏长（建议 {} 字以内，且会计入每回合 token 预算）",
                        ins.chars().count(),
                        PROTOCOL_INSTRUCTIONS_WARN_CHARS
                    ),
                    related_refs: None,
                });
            }
        }
    }

    // 7. skills: check duplicate IDs (emit Error "duplicate_skill_id").
    let mut skill_ids = HashSet::new();
    if let Some(skills) = sb.get("skills").and_then(|v| v.as_array()) {
        for (i, skill) in skills.iter().enumerate() {
            let id = skill
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let name = skill
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");

            let target = if id.is_empty() {
                format!("skill[{i}]")
            } else {
                format!("skill:{id}")
            };

            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Skill id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !skill_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_skill_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate skill id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }

            if name.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_name".to_string(),
                    target: Some(target.clone()),
                    message: "Skill name must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }

            // 技能效果按 id 引用顶层状态声明：悬空引用挡发布。
            if let Some(refs) = skill.pointer("/effect/status").and_then(Value::as_array) {
                for r in refs {
                    let sid = r.as_str().map(str::trim).unwrap_or("");
                    if sid.is_empty() || !status_ids.contains(sid) {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "dangling_status_ref".to_string(),
                            target: Some(target.clone()),
                            message: format!("Skill effect references non-existent status '{sid}'"),
                            related_refs: if sid.is_empty() {
                                None
                            } else {
                                Some(vec![sid.to_string()])
                            },
                        });
                    }
                }
            }
        }
    }

    // 7b. characters: if `skills` references skill IDs, each must exist in `skills` (emit Error "dangling_skill_ref").
    if let Some(chars) = sb.get("characters").and_then(|v| v.as_array()) {
        for (i, c) in chars.iter().enumerate() {
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let target = if id.is_empty() {
                format!("character[{i}]")
            } else {
                format!("character:{id}")
            };

            if let Some(char_skills) = c.get("skills").and_then(|v| v.as_array()) {
                for sk in char_skills {
                    if let Some(sk_id) = sk.as_str() {
                        let sk_id_trim = sk_id.trim();
                        if !sk_id_trim.is_empty() && !skill_ids.contains(sk_id_trim) {
                            issues.push(ValidationIssue {
                                severity: IssueSeverity::Error,
                                code: "dangling_skill_ref".to_string(),
                                target: Some(target.clone()),
                                message: format!(
                                    "Character references non-existent skill '{sk_id_trim}'"
                                ),
                                related_refs: Some(vec![sk_id_trim.to_string()]),
                            });
                        }
                    }
                }
            }
        }
    }

    // 6b. derived：key/label/formula；公式可解析，变量须为已声明维度或前面已声明的派生值。
    {
        let mut dimension_keys: HashSet<String> = HashSet::new();
        if let Some(dims) = sb.get("attribute_dimensions").and_then(|v| v.as_array()) {
            for d in dims {
                if let Some(k) = d.get("key").and_then(|v| v.as_str()) {
                    dimension_keys.insert(k.to_string());
                }
            }
        }
        if let Some(derived) = sb.get("derived").and_then(|v| v.as_array()) {
            let mut known: HashSet<String> = dimension_keys;
            let mut seen: HashSet<String> = HashSet::new();
            for (i, d) in derived.iter().enumerate() {
                let key = d.get("key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let formula = d.get("formula").and_then(|v| v.as_str()).unwrap_or("");
                let target = if key.is_empty() { format!("derived[{i}]") } else { format!("derived:{key}") };
                if key.is_empty() {
                    issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "missing_derived_key".to_string(), target: Some(target.clone()), message: "Derived value key must be a non-empty string".to_string(), related_refs: None });
                } else if !seen.insert(key.clone()) {
                    issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "duplicate_derived_key".to_string(), target: Some(target.clone()), message: format!("Duplicate derived key: '{key}'"), related_refs: Some(vec![key.clone()]) });
                }
                match crate::derived::check_formula(formula) {
                    Err(e) => issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "derived_formula_invalid".to_string(), target: Some(target.clone()), message: format!("Derived '{key}' formula invalid: {e}"), related_refs: None }),
                    Ok(vars) => {
                        for v in vars {
                            if !known.contains(&v) {
                                issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "derived_unknown_var".to_string(), target: Some(target.clone()), message: format!("Derived '{key}' references unknown variable '{v}'"), related_refs: Some(vec![v.clone()]) });
                            }
                        }
                    }
                }
                if !key.is_empty() { known.insert(key); }
            }
        }
    }

    // 6c. checkers：判定种类 / 被动基数合法性（world.check 与 skills[].check）。
    {
        let mut checkers: Vec<(String, &Value)> = Vec::new();
        if let Some(c) = sb.get("world").and_then(|w| w.get("check")) {
            checkers.push(("world.check".to_string(), c));
        }
        if let Some(skills) = sb.get("skills").and_then(|v| v.as_array()) {
            for s in skills {
                if let Some(c) = s.get("check") {
                    let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    checkers.push((format!("skill:{id}.check"), c));
                }
            }
        }
        for (label, c) in checkers {
            if let Some(kind) = c.get("kind").and_then(|v| v.as_str()) {
                if !["attribute", "attack", "save", "passive"].contains(&kind) {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        code: "invalid_check_kind".to_string(),
                        target: Some(label.clone()),
                        message: format!("Unknown check kind '{kind}' (expected attribute/attack/save/passive)"),
                        related_refs: None,
                    });
                }
            }
            if let Some(pb) = c.get("passive_base") {
                if pb.as_i64().is_none() && pb.as_u64().is_none() {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        code: "invalid_passive_base".to_string(),
                        target: Some(label.clone()),
                        message: "passive_base must be an integer".to_string(),
                        related_refs: None,
                    });
                }
            }
        }
    }

    // 6d. resources：恢复触发器 / 层阶；characters[].prepared：每日准备法术必须引用已声明技能。
    if let Some(resources) = sb.get("world").and_then(|w| w.get("resources")).and_then(|v| v.as_array()) {
        for (i, r) in resources.iter().enumerate() {
            let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let target = if id.is_empty() { format!("resource[{i}]") } else { format!("resource:{id}") };
            if let Some(tier) = r.get("tier") {
                if tier.as_i64().is_none() && tier.as_u64().is_none() {
                    issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "invalid_resource_tier".to_string(), target: Some(target.clone()), message: "resource tier must be an integer".to_string(), related_refs: None });
                }
            }
            if let Some(trigger) = r.get("natural_recovery").and_then(|n| n.get("trigger")).and_then(|v| v.as_str()) {
                if !["per_turn", "per_scene", "per_short_rest", "per_long_rest", "per_rest"].contains(&trigger) {
                    issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "invalid_recovery_trigger".to_string(), target: Some(target.clone()), message: format!("unknown natural_recovery trigger '{trigger}'"), related_refs: None });
                }
            }
        }
    }
    if let Some(chars) = sb.get("characters").and_then(|v| v.as_array()) {
        for c in chars {
            let cid = c.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let target = if cid.is_empty() { "character".to_string() } else { format!("character:{cid}") };
            if let Some(prepared) = c.get("prepared").and_then(|v| v.as_array()) {
                for p in prepared {
                    if let Some(pid) = p.as_str() {
                        if !skill_ids.contains(pid.trim()) {
                            issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "dangling_prepared_ref".to_string(), target: Some(target.clone()), message: format!("Prepared spell '{pid}' is not a declared skill"), related_refs: Some(vec![pid.to_string()]) });
                        }
                    }
                }
            }
        }
    }

    // 6e. items[].slot/modifiers/price + characters[].equipped（#5 装备位与属性贡献）。
    {
        let mut item_slots: HashMap<String, Option<String>> = HashMap::new();
        if let Some(items) = sb.get("items").and_then(|v| v.as_array()) {
            for (i, it) in items.iter().enumerate() {
                let iid = it.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
                let target = if iid.is_empty() { format!("item[{i}]") } else { format!("item:{iid}") };
                match it.get("slot") {
                    Some(slot) if slot.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false) => {
                        item_slots.insert(iid.to_string(), slot.as_str().map(|s| s.trim().to_string()));
                    }
                    Some(_) => {
                        issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "invalid_item_slot".to_string(), target: Some(target.clone()), message: "item slot must be a non-empty string when present".to_string(), related_refs: None });
                        item_slots.insert(iid.to_string(), None);
                    }
                    None => {
                        item_slots.insert(iid.to_string(), None);
                    }
                }
                if let Some(price) = it.get("price") {
                    if price.as_i64().is_none() && price.as_f64().is_none() {
                        issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "invalid_item_price".to_string(), target: Some(target.clone()), message: "item price must be a number".to_string(), related_refs: None });
                    }
                }
                if let Some(mods) = it.get("modifiers").and_then(|v| v.as_array()) {
                    for m in mods {
                        let t = m.get("target").and_then(|v| v.as_str()).unwrap_or("").trim();
                        if t.is_empty() {
                            issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "invalid_item_modifier".to_string(), target: Some(target.clone()), message: "item modifier needs a non-empty target".to_string(), related_refs: None });
                        }
                        if let Some(op) = m.get("op").and_then(|v| v.as_str()) {
                            if !["add", "max", "set"].contains(&op) {
                                issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "invalid_modifier_op".to_string(), target: Some(target.clone()), message: format!("unknown modifier op '{op}'"), related_refs: None });
                            }
                        }
                    }
                }
            }
        }
        if let Some(chars) = sb.get("characters").and_then(|v| v.as_array()) {
            for c in chars {
                let cid = c.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let ctarget = if cid.is_empty() { "character".to_string() } else { format!("character:{cid}") };
                let inv: HashSet<String> = c
                    .get("inventory")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                let Some(equipped) = c.get("equipped").and_then(|v| v.as_array()) else { continue };
                let mut slots_seen: HashMap<String, String> = HashMap::new();
                for e in equipped {
                    let Some(eid) = e.as_str() else { continue };
                    if !inv.contains(eid) {
                        issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "equipped_not_owned".to_string(), target: Some(ctarget.clone()), message: format!("Equipped item '{eid}' is not in the character inventory"), related_refs: Some(vec![eid.to_string()]) });
                    }
                    if let Some(Some(slot)) = item_slots.get(eid) {
                        if let Some(prev) = slots_seen.insert(slot.clone(), eid.to_string()) {
                            issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "equip_slot_conflict".to_string(), target: Some(ctarget.clone()), message: format!("Two equipped items share slot '{slot}': '{prev}' and '{eid}'"), related_refs: Some(vec![prev, eid.to_string()]) });
                        }
                    }
                }
            }
        }
    }

    // 6f. sheet：卡面分区引用的 kinds / derived / resources 必须存在。
    if let Some(sheet) = sb.get("sheet").and_then(|v| v.as_array()) {
        let kind_keys: HashSet<String> = sb
            .get("kinds")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|k| k.get("key").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let derived_keys: HashSet<String> = sb
            .get("derived")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|k| k.get("key").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let resource_ids: HashSet<String> = sb
            .get("world")
            .and_then(|w| w.get("resources"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
            .unwrap_or_default();
        for (i, sec) in sheet.iter().enumerate() {
            let title = sec.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let target = format!("sheet[{i}]:{title}");
            let mut check = |field: &str, known: &HashSet<String>, issues: &mut Vec<ValidationIssue>| {
                if let Some(list) = sec.get(field).and_then(|v| v.as_array()) {
                    for v in list {
                        if let Some(k) = v.as_str() {
                            if !known.contains(k) {
                                issues.push(ValidationIssue {
                                    severity: IssueSeverity::Error,
                                    code: "invalid_sheet_ref".to_string(),
                                    target: Some(target.clone()),
                                    message: format!("sheet section '{title}' references unknown {field} '{k}'"),
                                    related_refs: Some(vec![k.to_string()]),
                                });
                            }
                        }
                    }
                }
            };
            check("kinds", &kind_keys, &mut issues);
            check("derived", &derived_keys, &mut issues);
            check("resources", &resource_ids, &mut issues);
        }
    }

    // 6g. kinds：种类 key 不得与机制核心同名（那是封闭语义，各有专用声明）。
    if let Some(kinds) = sb.get("kinds").and_then(|v| v.as_array()) {
        const RESERVED: [&str; 11] = ["attribute", "attributes", "attribute_dimension", "dimension", "dimensions", "derived", "derived_value", "resource", "resources", "status", "statuses"];
        for (i, k) in kinds.iter().enumerate() {
            let key = k.get("key").and_then(|v| v.as_str()).unwrap_or("").trim();
            let target = if key.is_empty() { format!("kind[{i}]") } else { format!("kind:{key}") };
            if !key.is_empty() && RESERVED.contains(&key) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "reserved_kind_key".to_string(),
                    target: Some(target),
                    message: format!("Kind '{key}' 与机制核心同名——属性维度 / 派生值 / 资源 / 状态请用各自的声明，不要用开放种类重复定义"),
                    related_refs: None,
                });
            }
        }
    }

    // 7c. characters[].attachments：开放种类挂接（kind → Definition id）必须存在且种类可挂到人物。
    if let Some(chars) = sb.get("characters").and_then(|v| v.as_array()) {
        let mut kind_keys: HashSet<String> = HashSet::new();
        let mut kind_attach: HashSet<String> = HashSet::new();
        if let Some(kinds) = sb.get("kinds").and_then(|v| v.as_array()) {
            for k in kinds {
                let key = k.get("key").and_then(|v| v.as_str()).unwrap_or("").trim();
                if key.is_empty() { continue; }
                kind_keys.insert(key.to_string());
                let applies = k.get("applies_to").and_then(|v| v.as_array())
                    .map(|a| a.iter().any(|x| x.as_str() == Some("character"))).unwrap_or(false);
                if applies { kind_attach.insert(key.to_string()); }
            }
        }
        let mut def_kind: HashMap<String, String> = HashMap::new();
        if let Some(defs) = sb.get("definitions").and_then(|v| v.as_array()) {
            for def in defs {
                let did = def.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
                let dkind = def.get("kind").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !did.is_empty() { def_kind.insert(did.to_string(), dkind.to_string()); }
            }
        }
        for (i, c) in chars.iter().enumerate() {
            let id = c.get("id").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
            let target = if id.is_empty() { format!("character[{i}]") } else { format!("character:{id}") };
            let atts = match c.get("attachments").and_then(|v| v.as_object()) { Some(a) => a, None => continue };
            for (kind_key, ids) in atts {
                let att_target = format!("{target}.attachments.{kind_key}");
                if !kind_keys.contains(kind_key) {
                    issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "attachment_kind_unknown".to_string(), target: Some(att_target.clone()), message: format!("Attached kind {kind_key} is not declared in kinds"), related_refs: None });
                } else if !kind_attach.contains(kind_key) {
                    issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "attachment_kind_not_applicable".to_string(), target: Some(att_target.clone()), message: format!("Kind {kind_key} does not declare applies_to: [character]"), related_refs: None });
                }
                if let Some(list) = ids.as_array() {
                    for v in list {
                        if let Some(def_id) = v.as_str() {
                            match def_kind.get(def_id.trim()) {
                                None => issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "dangling_attachment_ref".to_string(), target: Some(att_target.clone()), message: format!("Attachment references non-existent definition {def_id}"), related_refs: Some(vec![def_id.to_string()]) }),
                                Some(k) if k != kind_key => issues.push(ValidationIssue { severity: IssueSeverity::Error, code: "attachment_kind_mismatch".to_string(), target: Some(att_target.clone()), message: format!("Definition {def_id} has kind {k}, not {kind_key}"), related_refs: Some(vec![def_id.to_string()]) }),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    // 8. items: each item has `id`, `name`. If `skills` has skill IDs, each must exist in `skills` (emit Error "dangling_skill_ref").
    let mut item_ids = HashSet::new();
    if let Some(items) = sb.get("items").and_then(|v| v.as_array()) {
        for (i, item) in items.iter().enumerate() {
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");

            let target = if id.is_empty() {
                format!("item[{i}]")
            } else {
                format!("item:{id}")
            };

            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Item id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !item_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_item_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate item id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }

            if name.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_name".to_string(),
                    target: Some(target.clone()),
                    message: "Item name must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }

            if let Some(item_skills) = item.get("skills").and_then(|v| v.as_array()) {
                for sk in item_skills {
                    if let Some(sk_id) = sk.as_str() {
                        let sk_id_trim = sk_id.trim();
                        if !sk_id_trim.is_empty() && !skill_ids.contains(sk_id_trim) {
                            issues.push(ValidationIssue {
                                severity: IssueSeverity::Error,
                                code: "dangling_skill_ref".to_string(),
                                target: Some(target.clone()),
                                message: format!(
                                    "Item references non-existent skill '{sk_id_trim}'"
                                ),
                                related_refs: Some(vec![sk_id_trim.to_string()]),
                            });
                        }
                    }
                }
            }
        }
    }

    // 9. objects: each object has `id`, `name`. Duplicate IDs emit Error "duplicate_object_id".
    let mut object_ids = HashSet::new();
    if let Some(objects) = sb.get("objects").and_then(|v| v.as_array()) {
        for (i, obj) in objects.iter().enumerate() {
            let id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");

            let target = if id.is_empty() {
                format!("object[{i}]")
            } else {
                format!("object:{id}")
            };

            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Object id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !object_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_object_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate object id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }

            if name.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_name".to_string(),
                    target: Some(target),
                    message: "Object name must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }
        }
    }

    // 10. factions: each has `id`, `name`.
    let mut faction_ids = HashSet::new();
    if let Some(factions) = sb.get("factions").and_then(|v| v.as_array()) {
        for (i, fac) in factions.iter().enumerate() {
            let id = fac
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let name = fac
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");

            let target = if id.is_empty() {
                format!("faction[{i}]")
            } else {
                format!("faction:{id}")
            };

            if id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(target.clone()),
                    message: "Faction id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            } else if !faction_ids.insert(id.to_string()) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "duplicate_faction_id".to_string(),
                    target: Some(target.clone()),
                    message: format!("Duplicate faction id: '{id}'"),
                    related_refs: Some(vec![id.to_string()]),
                });
            }

            if name.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_name".to_string(),
                    target: Some(target),
                    message: "Faction name must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }
        }
    }

    // 11. relationships: `from` and `to` must exist in characters or factions (emit Error "dangling_relationship_target").
    if let Some(rels) = sb.get("relationships").and_then(|v| v.as_array()) {
        for (i, rel) in rels.iter().enumerate() {
            let rel_id = rel.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let target = if rel_id.is_empty() {
                format!("relationship[{i}]")
            } else {
                format!("relationship:{rel_id}")
            };

            let from_kind = rel.get("from_kind").and_then(|v| v.as_str());
            let to_kind = rel.get("to_kind").and_then(|v| v.as_str());

            let from = rel
                .get("from_id")
                .or_else(|| rel.get("from"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let to = rel
                .get("to_id")
                .or_else(|| rel.get("to"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");

            let from_valid = if from.is_empty() {
                false
            } else {
                match from_kind {
                    Some("character") => character_ids.contains(from),
                    Some("faction") => faction_ids.contains(from),
                    _ => character_ids.contains(from) || faction_ids.contains(from),
                }
            };

            if !from_valid {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "dangling_relationship_target".to_string(),
                    target: Some(target.clone()),
                    message: format!(
                        "Relationship 'from' target '{from}' does not exist in characters or factions"
                    ),
                    related_refs: if from.is_empty() {
                        None
                    } else {
                        Some(vec![from.to_string()])
                    },
                });
            }

            let to_valid = if to.is_empty() {
                false
            } else {
                match to_kind {
                    Some("character") => character_ids.contains(to),
                    Some("faction") => faction_ids.contains(to),
                    _ => character_ids.contains(to) || faction_ids.contains(to),
                }
            };

            if !to_valid {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "dangling_relationship_target".to_string(),
                    target: Some(target),
                    message: format!(
                        "Relationship 'to' target '{to}' does not exist in characters or factions"
                    ),
                    related_refs: if to.is_empty() {
                        None
                    } else {
                        Some(vec![to.to_string()])
                    },
                });
            }
        }
    }

    // 5. skeleton (chapters -> scenes -> goals & triggers):
    //    - each chapter and scene must have non-empty `id` and `title`.
    //    - each scene:
    //      - `location_id`: if present, must exist in `locations` (emit Error "dangling_location_ref").
    //      - `present_char_ids`: each id must exist in `characters` (emit Error "dangling_character_ref").
    //      - `goals` and `triggers`: each condition checking `flag_set` should check if the flag is declared in `flags` (emit Warning "undeclared_flag").
    //        If condition references `trigger_fired`, check that the trigger exists (emit Error "dangling_trigger_ref").
    //        If condition references `at_location`, check that the location exists (emit Error "dangling_location_ref").
    if let Some(chapters) = sb.get("skeleton").and_then(|v| v.as_array()) {
        for (ch_idx, ch) in chapters.iter().enumerate() {
            let ch_id = ch
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let ch_title = ch
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");

            let ch_target = if ch_id.is_empty() {
                format!("chapter[{ch_idx}]")
            } else {
                format!("chapter:{ch_id}")
            };

            if ch_id.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_id".to_string(),
                    target: Some(ch_target.clone()),
                    message: "Chapter id must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }
            if ch_title.is_empty() {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "missing_title".to_string(),
                    target: Some(ch_target.clone()),
                    message: "Chapter title must be a non-empty string".to_string(),
                    related_refs: None,
                });
            }

            if let Some(scenes) = ch.get("scenes").and_then(|v| v.as_array()) {
                for (sc_idx, sc) in scenes.iter().enumerate() {
                    let sc_id = sc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .unwrap_or("");
                    let sc_title = sc
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .unwrap_or("");

                    let sc_target = if sc_id.is_empty() {
                        format!("{ch_target}.scene[{sc_idx}]")
                    } else {
                        format!("scene:{sc_id}")
                    };

                    if sc_id.is_empty() {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "missing_id".to_string(),
                            target: Some(sc_target.clone()),
                            message: "Scene id must be a non-empty string".to_string(),
                            related_refs: None,
                        });
                    }
                    if sc_title.is_empty() {
                        issues.push(ValidationIssue {
                            severity: IssueSeverity::Error,
                            code: "missing_title".to_string(),
                            target: Some(sc_target.clone()),
                            message: "Scene title must be a non-empty string".to_string(),
                            related_refs: None,
                        });
                    }

                    // scene location_id
                    if let Some(loc_val) = sc.get("location_id") {
                        if !loc_val.is_null() {
                            if let Some(loc_id) = loc_val.as_str() {
                                let loc_id_trim = loc_id.trim();
                                if !loc_id_trim.is_empty() && !location_ids.contains(loc_id_trim) {
                                    issues.push(ValidationIssue {
                                        severity: IssueSeverity::Error,
                                        code: "dangling_location_ref".to_string(),
                                        target: Some(sc_target.clone()),
                                        message: format!(
                                            "Scene references non-existent location '{loc_id_trim}'"
                                        ),
                                        related_refs: Some(vec![loc_id_trim.to_string()]),
                                    });
                                }
                            }
                        }
                    }

                    // scene present_char_ids
                    if let Some(char_ids) = sc.get("present_char_ids").and_then(|v| v.as_array()) {
                        for cid_val in char_ids {
                            if let Some(cid) = cid_val.as_str() {
                                let cid_trim = cid.trim();
                                if !character_ids.contains(cid_trim) {
                                    issues.push(ValidationIssue {
                                        severity: IssueSeverity::Error,
                                        code: "dangling_character_ref".to_string(),
                                        target: Some(sc_target.clone()),
                                        message: format!(
                                            "Scene references non-existent character '{cid_trim}' in present_char_ids"
                                        ),
                                        related_refs: Some(vec![cid_trim.to_string()]),
                                    });
                                }
                            }
                        }
                    }

                    // scene goals
                    if let Some(goals) = sc.get("goals").and_then(|v| v.as_array()) {
                        for (g_idx, goal) in goals.iter().enumerate() {
                            let g_id = goal.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let g_target = if g_id.is_empty() {
                                format!("{sc_target}.goal[{g_idx}]")
                            } else {
                                format!("goal:{g_id}")
                            };
                            if let Some(cond) = goal.get("condition") {
                                validate_condition(
                                    cond,
                                    &g_target,
                                    &flag_keys,
                                    &all_trigger_ids,
                                    &location_ids,
                                    &mut issues,
                                );
                            }
                        }
                    }

                    // scene triggers
                    if let Some(triggers) = sc.get("triggers").and_then(|v| v.as_array()) {
                        for (t_idx, trigger) in triggers.iter().enumerate() {
                            let t_id = trigger.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let t_target = if t_id.is_empty() {
                                format!("{sc_target}.trigger[{t_idx}]")
                            } else {
                                format!("trigger:{t_id}")
                            };
                            if let Some(cond) = trigger.get("condition") {
                                validate_condition(
                                    cond,
                                    &t_target,
                                    &flag_keys,
                                    &all_trigger_ids,
                                    &location_ids,
                                    &mut issues,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // 5.5 opening: 有场景骨架但没写故事开头 → 警告（开档时回落为世界前提）
    let has_scenes = sb
        .get("skeleton")
        .and_then(|v| v.as_array())
        .is_some_and(|chs| chs.iter().any(|ch| ch.get("scenes").and_then(|v| v.as_array()).is_some_and(|sc| !sc.is_empty())));
    if has_scenes {
        let opening = sb
            .pointer("/world/opening")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if opening.is_empty() {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                code: "missing_opening".to_string(),
                target: Some("world.opening".to_string()),
                message: "故事书没有「故事开头」——开档时将以世界前提（premise）作为开场旁白".to_string(),
                related_refs: None,
            });
        }
    }

    // 6. Lua 静态预检（#23）：语法 + scoped env 白名单，error 挡发布。
    issues.extend(crate::lua_lint::lint_storybook(sb).into_iter().map(|i| ValidationIssue {
        severity: IssueSeverity::Error,
        code: i.code,
        target: Some(i.target),
        message: i.message,
        related_refs: None,
    }));

    issues
}

fn validate_condition(
    cond: &Value,
    target: &str,
    declared_flags: &HashSet<String>,
    all_trigger_ids: &HashSet<String>,
    all_location_ids: &HashSet<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(arr) = cond.as_array() {
        for item in arr {
            validate_condition(
                item,
                target,
                declared_flags,
                all_trigger_ids,
                all_location_ids,
                issues,
            );
        }
        return;
    }
    if !cond.is_object() {
        return;
    }

    let op = cond.get("op").and_then(|v| v.as_str()).unwrap_or("");
    match op {
        "flag_set" => {
            let flag = cond
                .get("flag")
                .or_else(|| cond.get("flag_key"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !declared_flags.contains(flag) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    code: "undeclared_flag".to_string(),
                    target: Some(target.to_string()),
                    message: format!("Condition references undeclared flag '{flag}'"),
                    related_refs: if flag.is_empty() {
                        None
                    } else {
                        Some(vec![flag.to_string()])
                    },
                });
            }
        }
        "trigger_fired" => {
            let trigger_id = cond
                .get("trigger_id")
                .or_else(|| cond.get("trigger"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !all_trigger_ids.contains(trigger_id) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "dangling_trigger_ref".to_string(),
                    target: Some(target.to_string()),
                    message: format!("Condition references non-existent trigger '{trigger_id}'"),
                    related_refs: if trigger_id.is_empty() {
                        None
                    } else {
                        Some(vec![trigger_id.to_string()])
                    },
                });
            }
        }
        "at_location" => {
            let loc_id = cond
                .get("location_id")
                .or_else(|| cond.get("location"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !all_location_ids.contains(loc_id) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "dangling_location_ref".to_string(),
                    target: Some(target.to_string()),
                    message: format!("Condition references non-existent location '{loc_id}'"),
                    related_refs: if loc_id.is_empty() {
                        None
                    } else {
                        Some(vec![loc_id.to_string()])
                    },
                });
            }
        }
        "all_of" | "any_of" => {
            if let Some(children) = cond
                .get("children")
                .or_else(|| cond.get("conditions"))
                .and_then(|v| v.as_array())
            {
                for child in children {
                    validate_condition(
                        child,
                        target,
                        declared_flags,
                        all_trigger_ids,
                        all_location_ids,
                        issues,
                    );
                }
            }
        }
        "not" => {
            if let Some(child) = cond.get("child").or_else(|| cond.get("condition")) {
                validate_condition(
                    child,
                    target,
                    declared_flags,
                    all_trigger_ids,
                    all_location_ids,
                    issues,
                );
            }
        }
        _ => {}
    }
}

pub fn validate_storybook_result(sb: &Value) -> ValidateResult {
    let issues = validate_storybook(sb);
    let valid = !issues
        .iter()
        .any(|i| matches!(i.severity, IssueSeverity::Error));
    ValidateResult { valid, issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_seed_storybook_valid() {
        let seed = &crate::seed::seed_storybooks()[0].json;
        let res = validate_storybook_result(seed);
        assert!(res.valid, "seed storybook should be valid");
        let errors: Vec<_> = res
            .issues
            .iter()
            .filter(|i| matches!(i.severity, IssueSeverity::Error))
            .collect();
        assert_eq!(
            errors.len(),
            0,
            "seed storybook should have 0 errors: {:?}",
            errors
        );
    }


    #[test]
    fn test_character_writing_warnings_and_lore_validation() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        // 人物：背景很长且没有对话示例 → 两条 warning
        sb["characters"][0]["background"] = json!("背".repeat(1200));
        sb["lore"] = json!([
            { "id": "lore-ok", "title": "暗影森林", "content": "终年迷雾。", "keys": ["暗影森林"] },
            { "id": "lore-ok", "title": "重复", "content": "x", "keys": ["重复"] },
            { "id": "lore-nokey", "title": "无触发词", "content": "永远不会注入。" },
            { "id": "lore-empty", "title": "空内容", "content": "" },
            { "id": "lore-long", "title": "太长", "content": "长".repeat(320), "constant": true }
        ]);
        let res = validate_storybook_result(&sb);
        let has = |code: &str| res.issues.iter().any(|i| i.code == code);
        assert!(has("character_description_too_long"), "长描写要有 warning");
        assert!(has("character_missing_example_dialogues"), "缺对话示例要有 warning");
        assert!(has("duplicate_lore_id"), "重复词条 id 要报错");
        assert!(has("lore_without_trigger"), "无触发词且非常驻要提示");
        assert!(has("missing_lore_content"), "空内容要报错");
        assert!(has("lore_content_too_long"), "超长词条要提示");
        assert!(!res.valid, "存在 error 时故事书不合法");
    }

    #[test]
    fn test_narrative_validation() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["narrative"] = json!({ "sections": [
            { "id": "n-ok", "title": "文风", "slot": "style", "scope": "both", "text": "冷硬派。" },
            { "id": "n-ok", "title": "重复", "slot": "style", "scope": "both", "text": "x" },
            { "id": "n-bad-slot", "title": "坏槽位", "slot": "nope", "scope": "both", "text": "x" },
            { "id": "n-empty", "title": "空", "slot": "behavior", "scope": "both", "text": "" },
            { "id": "n-bad-scope", "title": "坏范围", "slot": "style", "scope": "everyone", "text": "x" },
            { "id": "n-dangling", "title": "悬空", "slot": "style", "scope": { "characterId": "char-ghost" }, "text": "x" }
        ] });
        let res = validate_storybook_result(&sb);
        let has = |code: &str| res.issues.iter().any(|i| i.code == code);
        assert!(has("duplicate_narrative_id"));
        assert!(has("invalid_narrative_slot"));
        assert!(has("missing_narrative_text"));
        assert!(has("invalid_narrative_scope"));
        assert!(has("dangling_narrative_scope_ref"));
        assert!(!res.valid, "存在 error 时故事书不合法");
    }

    #[test]
    fn test_narrative_when_and_variant_validation() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["narrative"] = json!({ "sections": [
            // when 形状非法（未知 op）：反序列化 CondExpr 失败 → Error
            { "id": "n-bad-when", "title": "坏条件", "slot": "style", "scope": "both", "text": "x",
              "when": { "op": "totally_unknown" } },
            // when 引用不存在的触发点 → dangling_trigger_ref（Error）
            { "id": "n-dangling-when", "title": "悬空触发点", "slot": "style", "scope": "both", "text": "x",
              "when": { "op": "trigger_fired", "trigger_id": "trigger-ghost" } },
            // when 引用已声明 flag → 不应误报 undeclared_flag
            { "id": "n-ok-when", "title": "合法条件", "slot": "style", "scope": "both", "text": "x",
              "when": { "op": "flag_set", "flag": "met_isa" } },
            // text 与 variants 同时给 → 冲突
            { "id": "n-conflict", "title": "冲突", "slot": "style", "scope": "both", "text": "x",
              "variants": [ { "key": "a", "label": "甲", "text": "甲文" } ], "defaultVariant": "a" },
            // key 重复 + defaultVariant 不存在
            { "id": "n-variants", "title": "坏变体", "slot": "style", "scope": "both",
              "variants": [
                { "key": "dup", "label": "一", "text": "一" },
                { "key": "dup", "label": "二", "text": "二" }
              ], "defaultVariant": "missing" },
            // 变体缺文本
            { "id": "n-empty-variant", "title": "空变体", "slot": "style", "scope": "both",
              "variants": [ { "key": "v", "label": "", "text": "" } ], "defaultVariant": "v" },
            // 声明 variants 但没 defaultVariant
            { "id": "n-no-default", "title": "缺默认", "slot": "style", "scope": "both",
              "variants": [ { "key": "v", "label": "", "text": "v" } ] }
        ] });
        let res = validate_storybook_result(&sb);
        let has = |code: &str| res.issues.iter().any(|i| i.code == code);
        assert!(has("invalid_narrative_when"), "非法 when 形状必须报错");
        assert!(has("dangling_trigger_ref"), "when 悬空触发点必须报错");
        assert!(!has("undeclared_flag"), "已声明 flag 的 when 不应误报");
        assert!(has("narrative_text_variants_conflict"));
        assert!(has("duplicate_variant_key"));
        assert!(has("invalid_default_variant"));
        assert!(has("missing_variant_text"));
        assert!(has("missing_default_variant"));
        assert!(!res.valid, "存在 error 时故事书不合法");
    }

    #[test]
    fn test_narrative_display_draft_validation() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        // 合法值：folded / hidden 都不报
        for v in ["folded", "hidden"] {
            sb["narrative"] = json!({ "sections": [], "display": { "draft": v } });
            let res = validate_storybook_result(&sb);
            assert!(
                !res.issues.iter().any(|i| i.code == "invalid_narrative_display_draft"),
                "{v} 应合法"
            );
        }
        // 非法值 → Error（前端会静默回落，故发布门必须拦）
        sb["narrative"] = json!({ "sections": [], "display": { "draft": "collapsed" } });
        let res = validate_storybook_result(&sb);
        assert!(res.issues.iter().any(
            |i| i.code == "invalid_narrative_display_draft" && matches!(i.severity, IssueSeverity::Error)
        ));
        // 缺省（没有 display）不报
        sb["narrative"] = json!({ "sections": [] });
        let res = validate_storybook_result(&sb);
        assert!(!res.issues.iter().any(|i| i.code == "invalid_narrative_display_draft"));
    }

    #[test]
    fn test_protocol_validation() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        // 非法 mode
        sb["narrative"] = json!({ "protocol": { "mode": "bogus" } });
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "invalid_protocol_mode"));

        // declarative 缺 intents
        sb["narrative"] = json!({ "protocol": { "mode": "declarative" } });
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "missing_protocol_intents"));

        // 未知意图 + 缺叙事意图
        sb["narrative"] = json!({ "protocol": { "mode": "declarative", "intents": ["strike", "teleport"] } });
        let res = validate_storybook_result(&sb);
        let has = |code: &str| res.issues.iter().any(|i| i.code == code);
        assert!(has("unknown_protocol_intent"));
        assert!(has("missing_narrative_intent"));
        assert!(!res.valid);

        // 合法 declarative（含叙事意图 + 已知机制意图）：不应有协议类问题
        sb["narrative"] = json!({ "protocol": { "mode": "declarative", "intents": ["narrate", "strike"] } });
        let res = validate_storybook_result(&sb);
        assert!(
            !res.issues.iter().any(|i| i.code.starts_with("protocol_")
                || matches!(i.code.as_str(), "invalid_protocol_mode" | "missing_protocol_intents" | "unknown_protocol_intent" | "missing_narrative_intent")),
            "合法 declarative 不应报协议问题：{:?}",
            res.issues
        );
    }

    #[test]
    fn test_protocol_lua_validation() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        // lua 模式缺源码 → Error
        sb["narrative"] = json!({ "protocol": { "mode": "lua" } });
        let res = validate_storybook_result(&sb);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "missing_protocol_lua" && matches!(i.severity, IssueSeverity::Error)));

        // 语法错误 → Error
        sb["narrative"] = json!({ "protocol": { "mode": "lua", "lua": "return (" } });
        let res = validate_storybook_result(&sb);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "protocol_lua_invalid" && matches!(i.severity, IssueSeverity::Error)));

        // 缺两个必选入口 → Warning（精确检查留到发布门）
        sb["narrative"] = json!({ "protocol": { "mode": "lua", "lua": "local x = 1" } });
        let res = validate_storybook_result(&sb);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "protocol_missing_functions" && matches!(i.severity, IssueSeverity::Warning)));
        assert!(res.valid, "只有 warning 时仍可发布");
    }

    #[test]
    fn test_protocol_instructions_length_warning() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["narrative"] = json!({ "protocol": {
            "mode": "declarative",
            "intents": ["narrate"],
            "instructions": "字".repeat(4001)
        } });
        let res = validate_storybook_result(&sb);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "protocol_instructions_too_long" && matches!(i.severity, IssueSeverity::Warning)));
        assert!(res.valid);
    }

    #[test]
    fn test_dangling_location_ref_in_scene() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["skeleton"][0]["scenes"][0]["location_id"] = json!("loc-nonexistent");
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(
            |i| i.code == "dangling_location_ref" && matches!(i.severity, IssueSeverity::Error)
        ));
    }

    #[test]
    fn test_dangling_character_ref_in_scene() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["skeleton"][0]["scenes"][0]["present_char_ids"] = json!(["char-ghost"]);
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "dangling_character_ref"
                && matches!(i.severity, IssueSeverity::Error)));
    }

    #[test]
    fn test_missing_title() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["meta"]["title"] = json!("");
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "missing_title" && matches!(i.severity, IssueSeverity::Error)));

        let mut sb2 = crate::seed::seed_storybooks()[0].json.clone();
        sb2["meta"].as_object_mut().unwrap().remove("title");
        let res2 = validate_storybook_result(&sb2);
        assert!(!res2.valid);
        assert!(res2
            .issues
            .iter()
            .any(|i| i.code == "missing_title" && matches!(i.severity, IssueSeverity::Error)));
    }

    #[test]
    fn test_missing_id_and_invalid_schema() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["meta"]["id"] = json!("");
        sb["schema_version"] = json!(0);
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "missing_id"));
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "invalid_schema_version"));
    }

    #[test]
    fn test_duplicate_attribute_key() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["attribute_dimensions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "key": "str",
                "label": "力量2",
                "type": "number"
            }));
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "duplicate_attribute_key"));
    }

    #[test]
    fn test_locations_duplicate_and_dangling_parent() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["world"]["locations"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "loc-tavern",
                "name": "碎星酒馆2"
            }));
        sb["world"]["locations"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "loc-cellar",
                "name": "地窖",
                "parent_id": "loc-nonexistent"
            }));
        sb["world"]["locations"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "loc-self",
                "name": "自环地点",
                "parent_id": "loc-self"
            }));
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "duplicate_location_id"));
        assert_eq!(
            res.issues
                .iter()
                .filter(|i| i.code == "dangling_location_parent_ref")
                .count(),
            2
        );
    }

    #[test]
    fn test_characters_duplicate_and_invalid_kind() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["characters"].as_array_mut().unwrap().push(json!({
            "id": "char-mira",
            "name": "米拉克隆",
            "kind": "pc"
        }));
        sb["characters"].as_array_mut().unwrap().push(json!({
            "id": "char-alien",
            "name": "外星人",
            "kind": "monster"
        }));
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "duplicate_character_id"));
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "invalid_character_kind"));
    }

    #[test]
    fn test_skills_and_items_references() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["skills"] = json!([
            { "id": "sk-fire", "name": "火球术" },
            { "id": "sk-fire", "name": "火球术重复" }
        ]);
        sb["items"] = json!([
            { "id": "it-wand", "name": "魔杖", "skills": ["sk-fire", "sk-ice-unknown"] }
        ]);
        sb["characters"][0]["skills"] = json!(["sk-fire", "sk-missing"]);
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "duplicate_skill_id"));
        assert_eq!(
            res.issues
                .iter()
                .filter(|i| i.code == "dangling_skill_ref")
                .count(),
            2
        );
    }

    #[test]
    fn test_objects_and_factions_and_relationships() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["objects"] = json!([
            { "id": "obj-chest", "name": "宝箱" },
            { "id": "obj-chest", "name": "重复宝箱" }
        ]);
        sb["factions"] = json!([
            { "id": "fac-guild", "name": "公会" }
        ]);
        sb["relationships"] = json!([
            { "id": "rel-1", "from_kind": "character", "from_id": "char-mira", "to_kind": "faction", "to_id": "fac-guild" },
            { "id": "rel-bad", "from": "char-unknown", "to": "char-ghost" }
        ]);
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "duplicate_object_id"));
        assert_eq!(
            res.issues
                .iter()
                .filter(|i| i.code == "dangling_relationship_target")
                .count(),
            2
        );
    }

    #[test]
    fn test_conditions_warnings_and_errors() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["skeleton"][0]["scenes"][0]["goals"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "g-test",
                "text": "测试目标",
                "condition": {
                    "op": "all_of",
                    "children": [
                        { "op": "flag_set", "flag": "unregistered_flag" },
                        { "op": "trigger_fired", "trigger_id": "b-nonexistent" },
                        { "op": "at_location", "location_id": "loc-void" }
                    ]
                }
            }));
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "undeclared_flag" && matches!(i.severity, IssueSeverity::Warning)));
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "dangling_trigger_ref" && matches!(i.severity, IssueSeverity::Error)));
        assert!(res.issues.iter().any(
            |i| i.code == "dangling_location_ref" && matches!(i.severity, IssueSeverity::Error)
        ));
    }

    #[test]
    fn test_undeclared_flag_warning_still_valid() {
        let mut sb = crate::seed::seed_storybooks()[0].json.clone();
        sb["skeleton"][0]["scenes"][0]["goals"][0]["condition"] = json!({
            "op": "flag_set",
            "flag": "some_extra_flag"
        });
        let res = validate_storybook_result(&sb);
        assert!(res.valid, "Warnings should not invalidate the storybook");
        assert!(res
            .issues
            .iter()
            .any(|i| i.code == "undeclared_flag" && matches!(i.severity, IssueSeverity::Warning)));
    }
}
