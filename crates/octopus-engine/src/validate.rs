//! 故事书静态校验纯函数（#01 / #23 ②）。

use std::collections::HashSet;

use octopus_types::{IssueSeverity, ValidateResult, ValidationIssue};
use serde_json::Value;

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
                    target: Some(target),
                    message: format!("Character kind '{kind}' is invalid; must be 'pc' or 'npc'"),
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
                    target: Some(target),
                    message: "Skill name must be a non-empty string".to_string(),
                    related_refs: None,
                });
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

    // Collect all beat IDs across skeleton
    let mut all_beat_ids = HashSet::new();
    if let Some(chapters) = sb.get("skeleton").and_then(|v| v.as_array()) {
        for ch in chapters {
            if let Some(scenes) = ch.get("scenes").and_then(|v| v.as_array()) {
                for sc in scenes {
                    if let Some(beats) = sc.get("beats").and_then(|v| v.as_array()) {
                        for b in beats {
                            if let Some(bid) = b.get("id").and_then(|v| v.as_str()).map(str::trim) {
                                if !bid.is_empty() {
                                    all_beat_ids.insert(bid.to_string());
                                }
                            }
                        }
                    }
                }
            }
            if let Some(beats) = ch.get("beats").and_then(|v| v.as_array()) {
                for b in beats {
                    if let Some(bid) = b.get("id").and_then(|v| v.as_str()).map(str::trim) {
                        if !bid.is_empty() {
                            all_beat_ids.insert(bid.to_string());
                        }
                    }
                }
            }
        }
    }

    // 5. skeleton (chapters -> scenes -> goals & beats):
    //    - each chapter and scene must have non-empty `id` and `title`.
    //    - each scene:
    //      - `location_id`: if present, must exist in `locations` (emit Error "dangling_location_ref").
    //      - `present_char_ids`: each id must exist in `characters` (emit Error "dangling_character_ref").
    //      - `goals` and `beats`: each condition checking `flag_set` should check if the flag is declared in `flags` (emit Warning "undeclared_flag").
    //        If condition references `beat_fired`, check that the beat exists (emit Error "dangling_beat_ref").
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
                                    &all_beat_ids,
                                    &location_ids,
                                    &mut issues,
                                );
                            }
                        }
                    }

                    // scene beats
                    if let Some(beats) = sc.get("beats").and_then(|v| v.as_array()) {
                        for (b_idx, beat) in beats.iter().enumerate() {
                            let b_id = beat.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let b_target = if b_id.is_empty() {
                                format!("{sc_target}.beat[{b_idx}]")
                            } else {
                                format!("beat:{b_id}")
                            };
                            if let Some(cond) = beat.get("condition") {
                                validate_condition(
                                    cond,
                                    &b_target,
                                    &flag_keys,
                                    &all_beat_ids,
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

    issues
}

fn validate_condition(
    cond: &Value,
    target: &str,
    declared_flags: &HashSet<String>,
    all_beat_ids: &HashSet<String>,
    all_location_ids: &HashSet<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(arr) = cond.as_array() {
        for item in arr {
            validate_condition(
                item,
                target,
                declared_flags,
                all_beat_ids,
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
        "beat_fired" => {
            let beat_id = cond
                .get("beat_id")
                .or_else(|| cond.get("beat"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !all_beat_ids.contains(beat_id) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    code: "dangling_beat_ref".to_string(),
                    target: Some(target.to_string()),
                    message: format!("Condition references non-existent beat '{beat_id}'"),
                    related_refs: if beat_id.is_empty() {
                        None
                    } else {
                        Some(vec![beat_id.to_string()])
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
                        all_beat_ids,
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
                    all_beat_ids,
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
        let res = validate_storybook_result(&sb);
        assert!(!res.valid);
        assert!(res.issues.iter().any(|i| i.code == "duplicate_skill_id"));
        assert!(res.issues.iter().any(|i| i.code == "dangling_skill_ref"));
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
                        { "op": "beat_fired", "beat_id": "b-nonexistent" },
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
            .any(|i| i.code == "dangling_beat_ref" && matches!(i.severity, IssueSeverity::Error)));
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
