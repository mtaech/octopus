//! AI 结对创作服务（#23 ④，C 范式）：真实大模型推理与结构化建议提取。

use std::collections::HashMap;
use std::convert::Infallible;

use rig::client::CompletionClient;
use rig::completion::message::{
    AssistantContent, Message as RigMessage, ProviderCallId, Reasoning, ReasoningContent, Text,
    ToolCall, ToolCallId, ToolChoice, ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use rig::completion::{CompletionModel, CompletionRequest, FinishReason, ToolDefinition};
use rig::providers::openai;
use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};

use axum::Json;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use octopus_types::FocusEntity;

use crate::ai::sampling_params;
use crate::config::{ProviderConfig, load_config_from_disk};
use crate::error::ApiError;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// assistant 发起工具调用时的 tool_calls 数组（原样透传给供应商）
    #[serde(default)]
    pub tool_calls: Option<Value>,
    /// role = tool 时对应的 tool_call id
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// 用户消息的附件（name + 文本正文）；发给模型时折叠进正文。
    #[serde(default)]
    pub attachments: Option<Vec<ChatAttachment>>,
}

/// 结对消息的文本附件（前端读文件后随消息一起送）。
#[derive(Debug, Clone, Deserialize)]
pub struct ChatAttachment {
    pub name: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PairStorybookContext {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub premise: Option<String>,
    #[serde(default)]
    pub opening: Option<String>,
    #[serde(default)]
    pub locations: Option<Vec<Value>>,
    #[serde(default)]
    pub characters: Option<Vec<Value>>,
    #[serde(default)]
    pub skeleton: Option<Vec<Value>>,
    #[serde(default)]
    pub skills: Option<Vec<Value>>,
    #[serde(default)]
    pub items: Option<Vec<Value>>,
    #[serde(default)]
    pub factions: Option<Vec<Value>>,
    #[serde(default)]
    pub relationships: Option<Vec<Value>>,
    #[serde(default)]
    pub resources: Option<Vec<Value>>,
    #[serde(default)]
    pub objects: Option<Vec<Value>>,
    #[serde(default)]
    pub dimensions: Option<Vec<Value>>,
    #[serde(default)]
    pub statuses: Option<Vec<Value>>,
    #[serde(default)]
    pub lore: Option<Vec<Value>>,
    #[serde(default)]
    pub flags: Option<Vec<Value>>,
    #[serde(default)]
    pub events: Option<Vec<Value>>,
    #[serde(default)]
    pub relationship_types: Option<Vec<Value>>,
    #[serde(default)]
    pub target_types: Option<Vec<Value>>,
    #[serde(default)]
    pub checker: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairChatRequest {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub storybook: Option<PairStorybookContext>,
    #[serde(default)]
    pub custom_provider: Option<ProviderConfig>,
    /// OpenAI 风格 function calling 工具定义；存在时进入工具模式
    #[serde(default)]
    pub tools: Option<Value>,
    /// 本轮显式引用的目标实体（完整定义）：创作者精确指定「要改这个」。
    #[serde(default)]
    pub focus: Option<Vec<FocusEntity>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairSuggestionTarget {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 嵌套实体新建时的父 id：scene = 章节 id；goal / trigger = 场景 id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairSuggestion {
    pub id: String,
    pub action: String, // create | update | delete
    pub target: PairSuggestionTarget,
    pub patch: Value,
    pub label: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairChatResponse {
    pub text: String,
    pub deltas: Vec<String>,
    pub suggestions: Vec<PairSuggestion>,
    #[serde(default)]
    pub tool_calls: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 解析请求中的 Provider 与 Model 参数
fn resolve_provider_and_model(
    req: &PairChatRequest,
) -> Result<(ProviderConfig, String, f64, u32, serde_json::Value), ApiError> {
    let cfg = load_config_from_disk();
    let mut resolved_provider = if let Some(cp) = &req.custom_provider {
        cp.clone()
    } else if let Some(pid) = &req.provider_id {
        cfg.providers
            .iter()
            .find(|p| &p.id == pid)
            .cloned()
            .or_else(|| cfg.providers.first().cloned())
            .unwrap_or_else(|| ProviderConfig {
                id: pid.clone(),
                label: pid.clone(),
                kind: "openai-compatible".into(),
                base_url: Some("https://api.deepseek.com".into()),
                api_key: None,
                models: vec![],
            })
    } else if let Some(role) = &cfg.roles.pair {
        cfg.providers
            .iter()
            .find(|p| p.id == role.provider_id)
            .cloned()
            .or_else(|| cfg.providers.first().cloned())
            .unwrap_or_else(|| cfg.providers[0].clone())
    } else {
        cfg.providers
            .first()
            .cloned()
            .unwrap_or_else(|| ProviderConfig {
                id: "deepseek".into(),
                label: "DeepSeek".into(),
                kind: "openai-compatible".into(),
                base_url: Some("https://api.deepseek.com".into()),
                api_key: None,
                models: vec![],
            })
    };

    // 如果客户端在 custom_provider 没传 api_key，但本地 config 有存储过，则补全
    if resolved_provider
        .api_key
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        if let Some(matched) = cfg.providers.iter().find(|p| p.id == resolved_provider.id) {
            if let Some(k) = &matched.api_key {
                if !k.trim().is_empty() {
                    resolved_provider.api_key = Some(k.clone());
                }
            }
        }
    }

    let base_url = resolved_provider
        .base_url
        .as_deref()
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_base_url",
            format!(
                "供应商「{}」未配置 Base URL，请在设置中填写。",
                resolved_provider.label
            ),
        ));
    }

    let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
    let key = resolved_provider
        .api_key
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if resolved_provider.kind != "ollama" && !is_local && key.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "missing_api_key",
            format!(
                "供应商「{}」未配置 API Key。请点击右上角「设置」填入 API Key 后保存再试。",
                resolved_provider.label
            ),
        ));
    }

    let model = if let Some(m) = &req.model {
        if !m.trim().is_empty() {
            m.trim().to_string()
        } else {
            "deepseek-chat".to_string()
        }
    } else if let Some(role) = &cfg.roles.pair {
        role.model.clone()
    } else if let Some(first_m) = resolved_provider.models.first() {
        first_m.id.clone()
    } else {
        "deepseek-chat".to_string()
    };

    let temp = cfg
        .roles
        .pair
        .as_ref()
        .and_then(|r| r.temperature)
        .unwrap_or(0.8);
    let max_tokens = cfg
        .roles
        .pair
        .as_ref()
        .and_then(|r| r.max_tokens)
        .unwrap_or(4096);
    let sampling = cfg
        .roles
        .pair
        .as_ref()
        .map(sampling_params)
        .unwrap_or_else(|| serde_json::json!({}));

    Ok((resolved_provider, model, temp, max_tokens, sampling))
}

/// 构建上下文增强的 System Prompt
fn build_system_prompt(
    sb: Option<&PairStorybookContext>,
    tool_mode: bool,
    focus: Option<&[FocusEntity]>,
) -> String {
    let mut s = String::from(
        "你是 Octopus 故事书编辑器的「AI 结对创作搭档」(Octo 结对)。\n\
        你的任务是与创作者边聊边成型故事书内容，协助构思世界观设定、丰满人物、推演剧情走向、设计技能与物品。\n\
        \n\
        【当前故事书草稿上下文】\n",
    );

    if let Some(ctx) = sb {
        if let Some(t) = &ctx.title {
            s.push_str(&format!("- 故事书标题: {t}\n"));
        }
        if let Some(d) = &ctx.description {
            if !d.trim().is_empty() {
                s.push_str(&format!("- 故事简介: {d}\n"));
            }
        }
        if let Some(o) = &ctx.opening {
            if !o.trim().is_empty() {
                s.push_str(&format!("- 故事开头 / 开场旁白: {o}\n"));
            }
        }
        if let Some(p) = &ctx.premise {
            if !p.trim().is_empty() {
                s.push_str(&format!("- 世界观背景 / 前提: {p}\n"));
            }
        }
        if let Some(chars) = &ctx.characters {
            let names: Vec<String> = chars
                .iter()
                .filter_map(|c| {
                    let name = c.get("name").and_then(Value::as_str)?;
                    let kind = c.get("kind").and_then(Value::as_str).unwrap_or("npc");
                    Some(format!("{name}({kind})"))
                })
                .collect();
            if !names.is_empty() {
                s.push_str(&format!("- 已有人物: {}\n", names.join(", ")));
            }

            // 人物已绑定技能：让模型知道谁掌握什么，避免把技能安到不相关的人身上
            let skill_name = |id: &str| -> String {
                ctx.skills
                    .as_ref()
                    .and_then(|list| {
                        list.iter()
                            .find(|sk| sk.get("id").and_then(Value::as_str) == Some(id))
                    })
                    .and_then(|sk| sk.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .unwrap_or_else(|| id.to_string())
            };
            let bindings: Vec<String> = chars
                .iter()
                .filter_map(|c| {
                    let cname = c.get("name").and_then(Value::as_str)?;
                    let ids: Vec<String> = c
                        .get("skills")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(Value::as_str)
                                .map(skill_name)
                                .collect()
                        })
                        .unwrap_or_default();
                    if ids.is_empty() {
                        None
                    } else {
                        Some(format!("{cname} → {}", ids.join("、")))
                    }
                })
                .collect();
            if !bindings.is_empty() {
                s.push_str(&format!("- 人物掌握技能: {}\n", bindings.join("；")));
            }
        }
        if let Some(locs) = &ctx.locations {
            let names: Vec<&str> = locs
                .iter()
                .filter_map(|l| l.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                s.push_str(&format!("- 已有地点: {}\n", names.join(", ")));
            }
        }
        if let Some(sk) = &ctx.skills {
            let names: Vec<&str> = sk
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                s.push_str(&format!("- 已有技能: {}\n", names.join(", ")));
            }
        }
        if let Some(it) = &ctx.items {
            let names: Vec<&str> = it
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                s.push_str(&format!("- 已有物品: {}\n", names.join(", ")));
            }
        }
        if let Some(fac) = &ctx.factions {
            let names: Vec<&str> = fac
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                s.push_str(&format!("- 已有势力/阵营: {}\n", names.join(", ")));
            }
        }
        if let Some(skel) = &ctx.skeleton {
            let mut chs = Vec::new();
            for ch in skel {
                if let Some(title) = ch.get("title").and_then(Value::as_str) {
                    chs.push(title);
                }
            }
            if !chs.is_empty() {
                s.push_str(&format!("- 剧情大纲章节: {}\n", chs.join(" -> ")));
            }
        }
    } else {
        s.push_str("（暂无已有草稿信息）\n");
    }

    s.push_str(
        "\n【输出规范与规则】\n\
        1. 保持专业编剧与游戏设计搭档口吻，见解深刻、富有启发性，条理清晰。\n\
        2. 当你的建议包含具体可写入故事书的实体（如新增/修改角色、地点、技能、物品、阵营、剧情目标等）时，请在回答正文后附带一个格式严谨的建议 JSON 代码块，供创作者在右侧审查区一键采纳落稿。\n\
        3. 建议代码块必须使用 ```json:suggestions ... ``` 标记，内容为一个 JSON 数组：\n\
        ```json:suggestions\n\
        [\n\
          {\n\
            \"action\": \"create\", // create 或 update\n\
            \"target\": { \"kind\": \"character\" }, // kind 支持: character, location, skill, item, faction, relationship\n\
            \"label\": \"新增人物 · 守夜人雨果\",\n\
            \"summary\": \"1-2句说明该项改动的作用与背景\",\n\
            \"patch\": {\n\
              \"name\": \"雨果\",\n\
              \"kind\": \"npc\",\n\
              \"background\": \"...\",\n\
              \"personality\": \"...\",\n\
              \"example_dialogues\": \"3-5 轮示范该角色口吻的对话（最能塑造风格）\",\n\
              \"attributes\": { \"str\": 50, \"wit\": 60 }\n\
            }\n\
          }\n\
        ]\n\
        ```\n\
        4. 实体的 patch 规范：\n\
           - character: { \"name\": \"...\", \"kind\": \"npc\"|\"pc\", \"background\": \"...\", \"personality\": \"...\", \"appearance\": \"...\", \"example_dialogues\": \"3-5 轮示范口吻的对话，是最强的风格控制\", \"notes\": \"给创作者的备注（不会发给 AI）\", \"attributes\": { \"str\": 50, \"agi\": 50, \"wit\": 50, \"cha\": 50 }, \"skills\": [技能id], \"inventory\": [{ \"id\": 物品id, \"quantity\": 1 }] }\n\
           - location: { \"name\": \"...\", \"description\": \"...\" }\n\
           - skill: { \"name\": \"...\", \"description\": \"...\", \"category\": \"...\" }\n\
           - item: { \"name\": \"...\", \"description\": \"...\", \"type\": \"...\" }\n\
           - faction: { \"name\": \"...\", \"description\": \"...\" }\n\
           - lore: { \"title\": \"...\", \"content\": \"3-5 句核心事实\", \"keys\": [\"触发词\"], \"priority\": 0, \"constant\": false, \"recursive\": false }\n\
        5. 若本次对话仅为理念探讨或确认，没有需要落入故事书的具体实体，则不要输出 ```json:suggestions 代码块。\n"
    );

    // ---- 补全实体 id 目录：让模型能精准引用既有实体 ----
    if let Some(ctx) = sb {
        s.push_str("\n【实体 id 目录（引用时务必使用这里的 id / key）】\n");
        let dump = |s: &mut String, label: &str, items: &Option<Vec<Value>>| {
            if let Some(list) = items {
                let mut parts: Vec<String> = Vec::new();
                for it in list {
                    let id = it
                        .get("id")
                        .and_then(Value::as_str)
                        .or_else(|| it.get("key").and_then(Value::as_str))
                        .unwrap_or("");
                    if id.is_empty() {
                        continue;
                    }
                    let name = it
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| it.get("title").and_then(Value::as_str))
                        .or_else(|| it.get("label").and_then(Value::as_str))
                        .unwrap_or("");
                    if name.is_empty() {
                        parts.push(id.to_string());
                    } else {
                        parts.push(format!("{name}({id})"));
                    }
                }
                if !parts.is_empty() {
                    s.push_str(&format!("- {label}: {}\n", parts.join(", ")));
                }
            }
        };
        dump(&mut s, "人物", &ctx.characters);
        dump(&mut s, "地点", &ctx.locations);
        dump(&mut s, "资源", &ctx.resources);
        dump(&mut s, "属性维度", &ctx.dimensions);
        dump(&mut s, "技能", &ctx.skills);
        dump(&mut s, "物品", &ctx.items);
        dump(&mut s, "物件", &ctx.objects);
        dump(&mut s, "势力", &ctx.factions);
        dump(&mut s, "关系", &ctx.relationships);
        dump(&mut s, "状态", &ctx.statuses);
        dump(&mut s, "词条", &ctx.lore);
        dump(&mut s, "标记", &ctx.flags);
        dump(&mut s, "事件", &ctx.events);
        dump(&mut s, "关系类型", &ctx.relationship_types);
        dump(&mut s, "目标类型", &ctx.target_types);
        if let Some(skel) = &ctx.skeleton {
            for ch in skel {
                let cid = ch.get("id").and_then(Value::as_str).unwrap_or("");
                let ct = ch.get("title").and_then(Value::as_str).unwrap_or("");
                s.push_str(&format!("- 章节 {ct}({cid})\n"));
                if let Some(scenes) = ch.get("scenes").and_then(Value::as_array) {
                    for sc in scenes {
                        let sid = sc.get("id").and_then(Value::as_str).unwrap_or("");
                        let st = sc.get("title").and_then(Value::as_str).unwrap_or("");
                        s.push_str(&format!("  - 场景 {st}({sid})\n"));
                        if let Some(goals) = sc.get("goals").and_then(Value::as_array) {
                            for g in goals {
                                let gid = g.get("id").and_then(Value::as_str).unwrap_or("");
                                let gt = g.get("text").and_then(Value::as_str).unwrap_or("");
                                if !gid.is_empty() {
                                    s.push_str(&format!("    - 目标({gid}) {gt}\n"));
                                }
                            }
                        }
                        if let Some(triggers) = sc.get("triggers").and_then(Value::as_array) {
                            for t in triggers {
                                let tid = t.get("id").and_then(Value::as_str).unwrap_or("");
                                let tt = t.get("title").and_then(Value::as_str).unwrap_or("");
                                if !tid.is_empty() {
                                    s.push_str(&format!("    - 触发点({tid}) {tt}\n"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- 本轮目标实体：创作者在对话里显式引用，优先级最高 ----
    if let Some(focus) = focus {
        if !focus.is_empty() {
            s.push_str("\n【本次改动的目标实体（最高优先级）】\n");
            s.push_str("创作者明确引用了以下实体。请把它们作为本次改动的主要目标，并基于其完整现状作答：\n");
            for f in focus {
                let id = f.id.as_deref().unwrap_or("-");
                s.push_str(&format!(
                    "\n- {}「{}」({})\n```json\n{}\n```\n",
                    f.kind,
                    f.name,
                    id,
                    serde_json::to_string_pretty(&f.entity).unwrap_or_else(|_| "{}".to_string())
                ));
            }
            s.push_str(
                "\n若为达成目标必须改动其他实体（例如为被引用的人物新建配套技能），可以提出，但请在说明里明确指出这是目标之外的改动。\n",
            );
        }
    }

    // ---- 完整 kind 与寻址规范（优先级最高；与上文示例冲突时以本节为准） ----
    s.push_str(
        r#"
【完整 kind 与寻址规范（优先级最高；与上文示例冲突时以本节为准）】
建议数组每项形状：{ "action": "create|update|delete", "target": { "kind": "...", "id": "...", "parent_id": "..." }, "label": "...", "summary": "...", "patch": { ... } }
- target.id：update / delete 的目标 id（dimension 与声明类用 key）；create 时省略。
- target.parent_id：嵌套实体 create 时必填——scene 填所属章节 id；goal / trigger 填所属场景 id。

顶层实体（create / update / delete 均支持）：
- character: { "name", "kind": "pc"|"npc", "background", "personality", "appearance", "example_dialogues", "notes", "attributes": { 维度key: 值 }, "resources": { 资源id: 数值 }, "skills": [技能id], "inventory": [{ "id": 物品id, "quantity": 数值 }] }（example_dialogues：3-5 轮示范该角色口吻的对话，是最有效的风格控制；notes：只给创作者看，永远不发给 AI）
- location: { "name", "description", "parent_id": 父地点id }
- resource: { "name", "type": "numerical"|"binary", "default_max": 数值 }
- dimension: { "key", "label", "type": "number"|"enum"|"text", "min", "max", "baseline", "modifier_step", "options": [..] }（判定修正默认 floor((值-基线)/步长)；步长缺省 5，D&D 六维用 2）
- status: { "id", "name", "description", "duration": 数值, "unit": "turns"|"scenes", "stack": "replace"|"add"|"max", "effect": [即时效果对象] }（技能与 Lua 按 id 引用状态）
- lore: { "id", "title", "content", "keys": [触发词变体], "priority": 数值, "constant": true|false, "recursive": true|false, "enabled": true|false }（世界词条：命中触发词才注入；每个条目 3-5 句；constant 为 true 则每回合都注入，触发词可留空）
- skill: { "name", "description", "category", "target": 目标类型key, "cost": [{ "resource": 资源id, "amount": 数值 }], "cooldown": { "turns": 数值 }, "effect": 效果对象（status 为状态 id 数组）, "lua" }
- item: { "name", "description", "type", "quantity", "skills": [技能id] }
- object: { "name", "description", "location_id": 地点id, "actions": [{ "key", "label" }], "skills": [技能id] }
- faction: { "name", "description", "goals": [字符串], "default_attitude": -100..100 }
- relationship: { "from_kind": "character"|"faction", "from", "to_kind", "to", "type": 关系类型key, "value": -100..100 }
- chapter: { "title", "description", "scenes": [ 场景对象 ] }

嵌套实体：
- scene（parent_id = 章节 id）: { "title", "description", "location_id", "present_char_ids": [人物id], "goals": [..], "triggers": [..] }
- goal（parent_id = 场景 id）: { "text", "primary": true|false, "hidden": true|false, "condition": 条件对象 }
- trigger（parent_id = 场景 id）: { "title", "description", "hint", "repeatable": true|false, "condition": 条件对象 }

单例（仅 update，无 id）：
- meta: { "title", "description", "author", "language" }
- world: { "opening", "premise", "check": 判定器对象 }

声明区（按 key 寻址；create 的 patch 必带 key；update / delete 用 target.id = key）：
- flag / event / relationship_type / target_type: { "key", "label" }

规则：
1. from / to / location_id / skills / parent_id / 维度key / 资源id / 关系类型 / 状态id 等所有引用，必须来自上面的 id 目录；目录里没有就先 create 建好，再在后续建议里引用。
2. update 为浅合并：数组 / 对象字段必须给出完整新值（例如改 attributes 要带全所有维度）。
3. 图片（封面 / 立绘 / 插图 / 图标）属于资产，由玩家上传；禁止在 patch 里产出图片 / asset 字段。
4. goal / trigger 的 parent_id 不确定时不要猜；可先提 scene 建议或直接询问创作者。
5. **机制 vs 内容（重要）**：机制核心——属性维度 / 派生值 / 资源（含法术位）/ 可结算状态 / 可装备物品——各有专门声明（dimension / resource / status / item；派生值在编辑器「派生值」面板），**绝不要另建开放种类或开放内容把它们重复定义**（反例：建一个 `ability` 种类再存一遍六维；建一个 `combat-stat` 再存一遍 AC / 法术DC）。开放种类 / 开放内容（upsert_kind / upsert_definition）**只装规则书内容**：种族 / 职业 / 背景 / 特性 / 语言 / 熟练项 / 传闻 等。判断口径：引擎要「求值 / 改写」的是机制核心，只给 AI 与玩家看的是开放内容。
6. 派生值与资源当前没有结对工具：需要配置时请在正文说明「请在编辑器『派生值 / 维度设置 / 世界设定』中设置」，不要用开放内容绕过。
"#,
    );

    if tool_mode {
        s.push_str(
            r#"
【工具模式（最高优先级）】
你已接入 write tools，请通过调用工具**提出**改动，不要再输出旧版 suggestions JSON 代码块。
- upsert_entity：新建或更新实体（kind 见上；新建 scene 需 parent_id = 章节 id，新建 goal / trigger 需 parent_id = 场景 id；更新 / 删除必须给 id）。
- delete_entity：删除实体。
- set_meta / set_world：更新元信息 / 世界设定。
- set_declarations：维护声明区四类。
- upsert_kind：新建或更新「开放种类」（内容模板）——声明种类 key、显示名、分组，以及字段 schema（fields）。故事特有的概念（如「传闻」「预言」）先在这里定义，不存在则新建。
- delete_kind：删除种类，并级联删除该种类下的全部内容。
- upsert_definition：新建或更新一条「开放内容」——某种类下的具体条目，kind 指定所属种类 key（须已存在），fields 按该种类 schema 填。
- delete_definition：删除一条开放内容。
- 每次工具调用后你会收到结果（含新建实体的 id），可继续调用以建立引用。
- **重要：工具调用不会直接写入草稿，而是作为「待批准改动」交给创作者审查。** 请只改与本次请求相关的内容；不要批量重写无关实体。
- 需要新的属性维度时，先用 upsert_entity（kind=dimension）建维度，再给人物 attributes 赋值。
- 完成后，用一段简洁中文说明你**建议**改了什么；措辞用「建议 / 拟」，不要声称已经写入草稿。
"#,
        );
    }

    s
}

/// 从大模型完整输出中提取结构化建议并净化展示正文
pub fn extract_suggestions(raw: &str) -> (String, Vec<PairSuggestion>) {
    // 匹配 ```json:suggestions ... ``` 或 ```suggestions ... ``` 或 ```json\n[\n {"action"...
    let patterns = ["```json:suggestions", "```suggestions", "```json"];
    for prefix in patterns {
        if let Some(start_idx) = raw.find(prefix) {
            let after_prefix = &raw[start_idx + prefix.len()..];
            if let Some(end_rel) = after_prefix.find("```") {
                let json_str = after_prefix[..end_rel].trim();
                if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                    if let Some(arr) = val.as_array() {
                        let mut sugs = Vec::new();
                        for item in arr {
                            let action = item
                                .get("action")
                                .and_then(Value::as_str)
                                .unwrap_or("create")
                                .to_string();
                            let kind = item
                                .pointer("/target/kind")
                                .and_then(Value::as_str)
                                .unwrap_or("character")
                                .to_string();
                            let target_id = item
                                .pointer("/target/id")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            let target_parent_id = item
                                .pointer("/target/parent_id")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            let label = item
                                .get("label")
                                .and_then(Value::as_str)
                                .unwrap_or("未命名建议")
                                .to_string();
                            let summary = item
                                .get("summary")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let patch = item
                                .get("patch")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({}));
                            let id = format!("sug-{}", Uuid::new_v4().simple());
                            sugs.push(PairSuggestion {
                                id,
                                action,
                                target: PairSuggestionTarget {
                                    kind,
                                    id: target_id,
                                    parent_id: target_parent_id,
                                },
                                patch,
                                label,
                                summary,
                            });
                        }
                        if !sugs.is_empty() {
                            let clean_text = format!(
                                "{}{}",
                                raw[..start_idx].trim_end(),
                                after_prefix[end_rel + 3..].trim_start()
                            );
                            return (clean_text.trim().to_string(), sugs);
                        }
                    }
                }
            }
        }
    }
    (raw.trim().to_string(), Vec::new())
}

/// 从 ProviderConfig 构造 rig 的 OpenAI 兼容 Chat Completions 客户端。
fn build_pair_client(provider: &ProviderConfig) -> Result<openai::CompletionsClient, ApiError> {
    let base_url = provider
        .base_url
        .as_deref()
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_base_url",
            format!(
                "供应商「{}」未配置 Base URL，请在设置中填写。",
                provider.label
            ),
        ));
    }
    let key = provider.api_key.as_deref().unwrap_or("").trim().to_string();
    openai::CompletionsClient::builder()
        .api_key(if key.is_empty() {
            "not-needed".to_string()
        } else {
            key
        })
        .base_url(base_url)
        .build()
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rig_client",
                format!("构建 rig 客户端失败：{e}"),
            )
        })
}

/// 构造 rig ToolCall：保留供应商原始调用 id，供回放
fn tool_call_from_wire(wire_id: &str, name: String, arguments: Value) -> ToolCall {
    let provider = if wire_id.is_empty() {
        None
    } else {
        Some(ProviderCallId {
            call_id: wire_id.to_string(),
            item_id: None,
        })
    };
    ToolCall {
        id: ToolCallId::new_or_mint(wire_id),
        provider,
        function: ToolFunction { name, arguments },
        signature: None,
        additional_params: None,
    }
}

/// 构造 rig ToolResult：与 assistant 的 tool_call 用同一个供应商 id 回灌
fn tool_result_from_wire(wire_id: &str, name: &str, content: &str) -> ToolResult {
    let provider = if wire_id.is_empty() {
        None
    } else {
        Some(ProviderCallId {
            call_id: wire_id.to_string(),
            item_id: None,
        })
    };
    ToolResult {
        call: ToolCallId::new_or_mint(wire_id),
        provider,
        name: name.to_string(),
        content: vec![ToolResultContent::text(content)],
    }
}

/// 前端 OpenAI 风格工具定义 → rig ToolDefinition
fn convert_tool(v: &Value) -> Option<ToolDefinition> {
    let f = v.get("function")?;
    let name = f.get("name").and_then(Value::as_str)?.to_string();
    let description = f
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let parameters = f
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    Some(ToolDefinition {
        name,
        description,
        parameters,
    })
}

/// 前端历史消息 → rig Message（含 assistant.tool_calls 与 role=tool 结果回灌）
fn convert_message(m: &ChatMessage, names: &mut HashMap<String, String>) -> Option<RigMessage> {
    match m.role.as_str() {
        "system" => Some(RigMessage::System {
            content: m.content.clone(),
        }),
        "user" => {
            // 附件折叠进正文：模型看到的是一条带【附件】块的用户消息。
            let mut content = m.content.clone();
            if let Some(atts) = &m.attachments {
                for a in atts {
                    let text = a.text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if !content.trim().is_empty() {
                        content.push_str("\n\n");
                    }
                    content.push_str(&format!("【附件：{}】\n```\n{}\n```", a.name, text));
                }
            }
            Some(RigMessage::User {
                content: vec![UserContent::Text(Text::new(content))],
            })
        }
        "assistant" => {
            let mut content: Vec<AssistantContent> = Vec::new();
            if !m.content.trim().is_empty() {
                content.push(AssistantContent::Text(Text::new(m.content.clone())));
            }
            if let Some(tcs) = m.tool_calls.as_ref().and_then(Value::as_array) {
                for tc in tcs {
                    let name = tc
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let wire_id = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let args_str = tc
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    let arguments: Value =
                        serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));
                    names.insert(wire_id.clone(), name.clone());
                    content.push(AssistantContent::ToolCall(tool_call_from_wire(
                        &wire_id, name, arguments,
                    )));
                }
            }
            if content.is_empty() {
                None
            } else {
                Some(RigMessage::Assistant { id: None, content })
            }
        }
        "tool" => {
            let wire_id = m.tool_call_id.clone().unwrap_or_default();
            let name = names
                .get(&wire_id)
                .cloned()
                .unwrap_or_else(|| "tool".to_string());
            Some(RigMessage::User {
                content: vec![UserContent::ToolResult(tool_result_from_wire(
                    &wire_id, &name, &m.content,
                ))],
            })
        }
        _ => None,
    }
}

/// 组装 rig CompletionRequest（system preamble + 全量历史 + tools）
fn build_completion_request(
    req: &PairChatRequest,
    temp: f64,
    max_tokens: u32,
    sampling: serde_json::Value,
) -> CompletionRequest {
    let mut names: HashMap<String, String> = HashMap::new();
    let chat_history: Vec<RigMessage> = req
        .messages
        .iter()
        .filter_map(|m| convert_message(m, &mut names))
        .collect();
    let tools: Vec<ToolDefinition> = req
        .tools
        .as_ref()
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(convert_tool).collect())
        .unwrap_or_default();
    let has_tools = !tools.is_empty();
    CompletionRequest {
        model: None,
        preamble: Some(build_system_prompt(
            req.storybook.as_ref(),
            has_tools,
            req.focus.as_deref(),
        )),
        chat_history,
        documents: Vec::new(),
        tools,
        temperature: Some(temp),
        max_tokens: Some(max_tokens as u64),
        tool_choice: if has_tools {
            Some(ToolChoice::Auto)
        } else {
            None
        },
        additional_params: if sampling.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            Some(sampling)
        } else {
            None
        },
        output_schema: None,
        record_telemetry_content: false,
    }
}

fn completion_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "provider_error",
        format!("模型调用失败：{e}"),
    )
}

/// 非流式响应 → (展示文本, 工具调用 JSON)
fn split_choice(choice: &[AssistantContent]) -> (String, Vec<Value>) {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for item in choice {
        match item {
            AssistantContent::Text(t) => text.push_str(&t.text),
            AssistantContent::ToolCall(tc) => {
                let args = serde_json::to_string(&tc.function.arguments)
                    .unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(serde_json::json!({
                    "id": tc.wire_call_id(),
                    "name": tc.function.name,
                    "arguments": args,
                }));
            }
            _ => {}
        }
    }
    (text, tool_calls)
}

/// 非流式对话接口（rig CompletionModel）
pub async fn pair_chat(
    Json(req): Json<PairChatRequest>,
) -> Result<Json<PairChatResponse>, ApiError> {
    let (provider, model, temp, max_tokens, sampling) = resolve_provider_and_model(&req)?;
    let client = build_pair_client(&provider)?;
    let rig_model = client.completion_model(model);
    let request = build_completion_request(&req, temp, max_tokens, sampling);

    let response = rig_model
        .completion(request)
        .await
        .map_err(completion_error)?;
    let (full_content, tool_calls) = split_choice(&response.choice);
    let (clean_text, suggestions) = if tool_calls.is_empty() {
        extract_suggestions(&full_content)
    } else {
        (full_content.trim().to_string(), Vec::new())
    };

    Ok(Json(PairChatResponse {
        text: clean_text.clone(),
        deltas: vec![clean_text],
        suggestions,
        tool_calls,
        finish_reason: None,
    }))
}

/// 完整思考块 → 纯文本（Text / Summary；加密 / 脱敏块跳过）。
fn reasoning_text_of(r: &Reasoning) -> String {
    r.content
        .iter()
        .filter_map(|c| match c {
            ReasoningContent::Text { text, .. } => Some(text.clone()),
            ReasoningContent::Summary(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// 流式 SSE 结对对话接口（rig 流式 CompletionModel；多步工具循环仍在前端）
pub async fn pair_chat_stream(
    Json(req): Json<PairChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (provider, model, temp, max_tokens, sampling) = resolve_provider_and_model(&req)?;
    let model_id = model.clone();
    let client = build_pair_client(&provider)?;
    let rig_model = client.completion_model(model);
    let request = build_completion_request(&req, temp, max_tokens, sampling);
    let tool_mode = !request.tools.is_empty();
    // 日志要用的元信息，必须在 request 被 move 之前取出来
    let tool_count = request.tools.len();
    let message_count = request.chat_history.len();

    let upstream = rig_model.stream(request).await.map_err(completion_error)?;

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(100);

    tokio::spawn(async move {
        let mut upstream = Box::pin(upstream);
        let mut full_text = String::new();
        let mut reasoning_text = String::new();
        let mut reasoning_chars = 0usize;
        // internal_call_id → (name, args_json, wire_id)：ToolCallDelta 是分片下发的，必须自行累积，
        // 否则工具调用会被整段丢失（表现为「模型没有返回任何内容」）。
        let mut order: Vec<String> = Vec::new();
        let mut calls: HashMap<String, (String, String, String)> = HashMap::new();
        let mut finish_reason: Option<String> = None;
        let mut usage = serde_json::json!({});
        let (mut n_text, mut n_tool, mut n_delta, mut n_reason, mut n_unknown) =
            (0usize, 0usize, 0usize, 0usize, 0usize);

        tracing::info!(
            model = %model_id,
            tool_mode,
            tools = tool_count,
            messages = message_count,
            max_tokens,
            "结对：向上游发起流式请求"
        );

        while let Some(item) = upstream.next().await {
            match item {
                Ok(StreamedAssistantContent::Text(t)) => {
                    n_text += 1;
                    if !t.text.is_empty() {
                        full_text.push_str(&t.text);
                        let ev = Event::default()
                            .event("delta")
                            .data(serde_json::json!({ "text": t.text }).to_string());
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(StreamedAssistantContent::ToolCall {
                    tool_call,
                    internal_call_id,
                }) => {
                    n_tool += 1;
                    let args = serde_json::to_string(&tool_call.function.arguments)
                        .unwrap_or_else(|_| "{}".to_string());
                    let wire = tool_call.wire_call_id().to_string();
                    let id = if wire.is_empty() {
                        internal_call_id.clone()
                    } else {
                        wire
                    };
                    let key = if internal_call_id.is_empty() {
                        id.clone()
                    } else {
                        internal_call_id
                    };
                    if !calls.contains_key(&key) {
                        order.push(key.clone());
                    }
                    calls.insert(key, (tool_call.function.name, args, id));
                }
                Ok(StreamedAssistantContent::ToolCallDelta {
                    internal_call_id,
                    content,
                }) => {
                    n_delta += 1;
                    if !calls.contains_key(&internal_call_id) {
                        order.push(internal_call_id.clone());
                        calls.insert(
                            internal_call_id.clone(),
                            (String::new(), String::new(), internal_call_id.clone()),
                        );
                    }
                    if let Some(entry) = calls.get_mut(&internal_call_id) {
                        match content {
                            ToolCallDeltaContent::Name(n) => entry.0 = n,
                            ToolCallDeltaContent::Delta(d) => entry.1.push_str(&d),
                        }
                    }
                }
                Ok(StreamedAssistantContent::Reasoning { reasoning, .. }) => {
                    n_reason += 1;
                    // 完整思考块（部分供应商一次性给出）：按 rig 约定替换此前的增量
                    let text = reasoning_text_of(&reasoning);
                    reasoning_chars += text.len();
                    if !text.is_empty() {
                        reasoning_text = text.clone();
                        let ev = Event::default()
                            .event("reasoning")
                            .data(serde_json::json!({ "text": text, "replace": true }).to_string());
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(StreamedAssistantContent::ReasoningDelta { reasoning, .. }) => {
                    n_reason += 1;
                    reasoning_chars += reasoning.len();
                    if !reasoning.is_empty() {
                        reasoning_text.push_str(&reasoning);
                        let ev = Event::default()
                            .event("reasoning")
                            .data(serde_json::json!({ "text": reasoning }).to_string());
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(StreamedAssistantContent::Final(f)) => {
                    finish_reason = f.finish_reason.as_ref().map(|r| match r {
                        FinishReason::Stop => "stop".to_string(),
                        FinishReason::Length => "length".to_string(),
                        FinishReason::ToolCalls => "tool_calls".to_string(),
                        FinishReason::ContentFilter => "content_filter".to_string(),
                        FinishReason::Other(s) => s.clone(),
                    });
                    usage = serde_json::json!({
                        "input_tokens": f.usage.input_tokens,
                        "output_tokens": f.usage.output_tokens,
                        "total_tokens": f.usage.total_tokens,
                        "cached_input_tokens": f.usage.cached_input_tokens,
                        "cache_creation_input_tokens": f.usage.cache_creation_input_tokens,
                        "tool_use_prompt_tokens": f.usage.tool_use_prompt_tokens,
                        "reasoning_tokens": f.usage.reasoning_tokens,
                    });
                }
                Ok(_) => {
                    n_unknown += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "结对：上游流中断");
                    let ev = Event::default().event("error").data(
                        serde_json::json!({ "message": format!("模型流中断：{e}") }).to_string(),
                    );
                    let _ = tx.send(Ok(ev)).await;
                    return;
                }
            }
        }

        let tool_calls: Vec<Value> = order
            .iter()
            .filter_map(|k| calls.get(k))
            .map(|(name, args, id)| {
                let parsed: Value =
                    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}));
                serde_json::json!({ "id": id, "name": name, "arguments": parsed.to_string() })
            })
            .collect();

        let counts = serde_json::json!({
            "text": n_text, "tool_call": n_tool, "tool_call_delta": n_delta,
            "reasoning": n_reason, "unknown": n_unknown,
        });
        if full_text.trim().is_empty() && tool_calls.is_empty() {
            tracing::warn!(
                ?finish_reason,
                reasoning_chars,
                counts = %counts,
                usage = %usage,
                "结对：上游未产出任何文本或工具调用"
            );
        } else {
            tracing::info!(
                text_chars = full_text.chars().count(),
                tool_calls = tool_calls.len(),
                reasoning_chars,
                ?finish_reason,
                counts = %counts,
                usage = %usage,
                "结对：上游流结束"
            );
        }

        let (clean_text, suggestions) = if tool_mode || !tool_calls.is_empty() {
            (full_text.trim().to_string(), Vec::new())
        } else {
            extract_suggestions(&full_text)
        };

        for sug in &suggestions {
            let ev = Event::default()
                .event("suggestion")
                .data(serde_json::to_string(sug).unwrap_or_default());
            let _ = tx.send(Ok(ev)).await;
        }
        for tc in &tool_calls {
            let ev = Event::default()
                .event("tool_call")
                .data(serde_json::to_string(tc).unwrap_or_default());
            let _ = tx.send(Ok(ev)).await;
        }
        // 用量 / 结束原因：前端据此显示真实 token 与「空响应」的原因
        let meta = serde_json::json!({
            "finish_reason": finish_reason,
            "usage": usage,
            "reasoning_chars": reasoning_chars,
            "counts": counts,
        });
        let _ = tx
            .send(Ok(Event::default().event("usage").data(meta.to_string())))
            .await;
        let done = Event::default().event("done").data(
            serde_json::json!({
                "full_text": clean_text,
                "suggestions": suggestions,
                "tool_calls": tool_calls,
                "finish_reason": finish_reason,
                "usage": usage,
                "reasoning_chars": reasoning_chars,
                "reasoning": reasoning_text,
                "counts": counts,
            })
            .to_string(),
        );
        let _ = tx.send(Ok(done)).await;
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_message_folds_attachments_into_content() {
        let m = ChatMessage {
            role: "user".into(),
            content: "看看这个设定".into(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Some(vec![
                ChatAttachment {
                    name: "lore.md".into(),
                    text: "暗影森林终年迷雾。".into(),
                },
                ChatAttachment {
                    name: "empty.txt".into(),
                    text: "   ".into(),
                },
            ]),
        };
        let mut names = HashMap::new();
        let msg = convert_message(&m, &mut names).expect("user message");
        let text = match msg {
            RigMessage::User { content } => content
                .into_iter()
                .find_map(|c| match c {
                    UserContent::Text(t) => Some(t.text),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => panic!("expected user message"),
        };
        assert!(text.contains("看看这个设定"));
        assert!(text.contains("【附件：lore.md】"), "附件要折叠进正文");
        assert!(text.contains("暗影森林终年迷雾。"));
        assert!(!text.contains("empty.txt"), "空附件不折叠");
    }
}
