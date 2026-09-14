//! AI 结对创作服务（#23 ④，C 范式）：真实大模型推理与结构化建议提取。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use rig::client::CompletionClient;
use rig::completion::message::{
    AssistantContent, Message as RigMessage, ProviderCallId, Reasoning, ReasoningContent, Text,
    ToolCall, ToolCallId, ToolChoice, ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use rig::completion::{CompletionModel, CompletionRequest, FinishReason, ToolDefinition};
use rig::providers::openai;
use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};

use axum::Json;
use axum::extract::{Extension, State};
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

use octopus_ai::is_context_overflow;
use octopus_engine::PairCompaction;

use crate::AppState;
use crate::ai::sampling_params;
use crate::config::{ProviderConfig, load_config_from_disk};
use crate::error::ApiError;
use crate::prompts::PairPrompts;

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
    /// 仅 assistant：上一轮思考正文（reasoning_content）。
    /// DeepSeek 思考模式在带 tool_calls 的助手消息上**要求回传**它，否则 400
    /// （「The reasoning_content in the thinking mode must be passed back to the API.」），
    /// 多步工具调用会直接断在那里。
    #[serde(default)]
    pub reasoning: Option<String>,
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
    /// 会话线程 id：后端据此持久化「上下文压缩检查点」。缺省 = 不压缩
    ///（例如还没落库的新会话；那时历史也短）。
    #[serde(default)]
    pub thread_id: Option<String>,
    /// 客户端显式下发的输出预算（tokens）。缺省时自适应解析（见 resolve_effective_max_tokens）：
    /// 思考模型的 reasoning_tokens 也算在这个预算里，写死小值会把正文整段挤掉。
    #[serde(default)]
    pub max_tokens: Option<u32>,
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
    /// 本轮真正发给模型的上下文构成（字符）：状态行如实报数（压缩后前端估算会偏大）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 输出预算下限：低于此值只当「历史遗留默认」看，不当用户意图。
/// 思考模型的 reasoning_tokens 与正文共享同一份 max_tokens，4096 常常全部烧在思考上
/// （实测 deepseek-flash：output_tokens=4096、reasoning_tokens=4096、正文 0 字）。
const PAIR_MAX_TOKENS_FLOOR: u32 = 16_384;
/// 单轮输出预算上限：再高没有意义（供应商也不认），只是防止配错值把上下文挤爆。
const PAIR_MAX_TOKENS_CEIL: u32 = 65_536;
/// 客户端显式下发的合法区间。
const PAIR_MAX_TOKENS_MIN_EXPLICIT: u32 = 1_024;

/// 本轮输出预算：显式请求 > 自适应默认。纯函数（配置值由调用方读出），便于单测。
///
/// 自适应取「角色配置 / 模型 max_out / 兜底下限」三者中的最大值——配置里那个历史默认
/// 4096 会被自动抬起来；用户真想要更小的值仍可用显式 max_tokens 下发。
fn effective_max_tokens(explicit: Option<u32>, cfg_value: u32, model_max_out: u32) -> u32 {
    if let Some(v) = explicit {
        return v.clamp(PAIR_MAX_TOKENS_MIN_EXPLICIT, PAIR_MAX_TOKENS_CEIL);
    }
    cfg_value
        .max(model_max_out)
        .max(PAIR_MAX_TOKENS_FLOOR)
        .min(PAIR_MAX_TOKENS_CEIL)
}

/// 从请求 + 已解析的供应商/模型读出本轮输出预算（配置值取自 roles.pair）。
fn resolve_effective_max_tokens(
    req: &PairChatRequest,
    provider: &ProviderConfig,
    model: &str,
) -> u32 {
    let cfg_value = load_config_from_disk()
        .roles
        .pair
        .as_ref()
        .and_then(|r| r.max_tokens)
        .unwrap_or(0);
    let model_max_out = provider
        .models
        .iter()
        .find(|m| m.id == model)
        .and_then(|m| m.max_out)
        .unwrap_or(0);
    effective_max_tokens(req.max_tokens, cfg_value, model_max_out)
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
    let max_tokens = resolve_effective_max_tokens(req, &resolved_provider, &model);
    let sampling = cfg
        .roles
        .pair
        .as_ref()
        .map(sampling_params)
        .unwrap_or_else(|| serde_json::json!({}));

    Ok((resolved_provider, model, temp, max_tokens, sampling))
}

/// 本轮草稿快照（易变内容）的抬头：写清「这是数据，不是创作者的新发言」。
const PAIR_CONTEXT_HEADER: &str =
    "【本轮故事书草稿快照（编辑器自动附加，非创作者发言；下列事实与 id 目录是本轮的依据）】\n";

/// 组装结对的提示词：返回 `(系统层, 本轮上下文块)`。
///
/// **系统层只放逐回合稳定的文本**（角色 + 规则 + 规范 + 工具说明）；故事书草稿快照、
/// 实体 id 目录、本轮引用目标每回合都在变，一律进第二个返回值，由调用方作为请求的
/// **最后一条消息**下发（见 `build_completion_request`）。
///
/// 为什么必须拆开：供应商的上下文缓存是**前缀缓存**——按请求序列的公共前缀匹配。
/// 系统层在请求最前面，它变一个字，后面整段会话历史都按原价重算；而结对每采纳一次
/// 建议、每跑一步工具，草稿就变一次。把草稿留在系统层 = 把整段会话的缓存永久废掉。
fn build_prompts(
    sb: Option<&PairStorybookContext>,
    tool_mode: bool,
    focus: Option<&[FocusEntity]>,
    prompts: &PairPrompts,
) -> (String, String) {
    let mut s = prompts.role.clone();
    let mut volatile = String::new();

    volatile.push_str(PAIR_CONTEXT_HEADER);

    if let Some(ctx) = sb {
        if let Some(t) = &ctx.title {
            volatile.push_str(&format!("- 故事书标题: {t}\n"));
        }
        if let Some(d) = &ctx.description {
            if !d.trim().is_empty() {
                volatile.push_str(&format!("- 故事简介: {d}\n"));
            }
        }
        if let Some(o) = &ctx.opening {
            if !o.trim().is_empty() {
                volatile.push_str(&format!("- 故事开头 / 开场旁白: {o}\n"));
            }
        }
        if let Some(p) = &ctx.premise {
            if !p.trim().is_empty() {
                volatile.push_str(&format!("- 世界观背景 / 前提: {p}\n"));
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
                volatile.push_str(&format!("- 已有人物: {}\n", names.join(", ")));
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
                volatile.push_str(&format!("- 人物掌握技能: {}\n", bindings.join("；")));
            }
        }
        if let Some(locs) = &ctx.locations {
            let names: Vec<&str> = locs
                .iter()
                .filter_map(|l| l.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                volatile.push_str(&format!("- 已有地点: {}\n", names.join(", ")));
            }
        }
        if let Some(sk) = &ctx.skills {
            let names: Vec<&str> = sk
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                volatile.push_str(&format!("- 已有技能: {}\n", names.join(", ")));
            }
        }
        if let Some(it) = &ctx.items {
            let names: Vec<&str> = it
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                volatile.push_str(&format!("- 已有物品: {}\n", names.join(", ")));
            }
        }
        if let Some(fac) = &ctx.factions {
            let names: Vec<&str> = fac
                .iter()
                .filter_map(|x| x.get("name").and_then(Value::as_str))
                .collect();
            if !names.is_empty() {
                volatile.push_str(&format!("- 已有势力/阵营: {}\n", names.join(", ")));
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
                volatile.push_str(&format!("- 剧情大纲章节: {}\n", chs.join(" -> ")));
            }
        }
    } else {
        volatile.push_str("（暂无已有草稿信息）\n");
    }

    s.push_str(&prompts.rules);

    // ---- 补全实体 id 目录：让模型能精准引用既有实体 ----
    if let Some(ctx) = sb {
        volatile.push_str("\n【实体 id 目录（引用时务必使用这里的 id / key）】\n");
        let dump = |out: &mut String, label: &str, items: &Option<Vec<Value>>| {
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
                    out.push_str(&format!("- {label}: {}\n", parts.join(", ")));
                }
            }
        };
        dump(&mut volatile, "人物", &ctx.characters);
        dump(&mut volatile, "地点", &ctx.locations);
        dump(&mut volatile, "资源", &ctx.resources);
        dump(&mut volatile, "属性维度", &ctx.dimensions);
        dump(&mut volatile, "技能", &ctx.skills);
        dump(&mut volatile, "物品", &ctx.items);
        dump(&mut volatile, "物件", &ctx.objects);
        dump(&mut volatile, "势力", &ctx.factions);
        dump(&mut volatile, "关系", &ctx.relationships);
        dump(&mut volatile, "状态", &ctx.statuses);
        dump(&mut volatile, "词条", &ctx.lore);
        dump(&mut volatile, "标记", &ctx.flags);
        dump(&mut volatile, "事件", &ctx.events);
        dump(&mut volatile, "关系类型", &ctx.relationship_types);
        dump(&mut volatile, "目标类型", &ctx.target_types);
        if let Some(skel) = &ctx.skeleton {
            for ch in skel {
                let cid = ch.get("id").and_then(Value::as_str).unwrap_or("");
                let ct = ch.get("title").and_then(Value::as_str).unwrap_or("");
                volatile.push_str(&format!("- 章节 {ct}({cid})\n"));
                if let Some(scenes) = ch.get("scenes").and_then(Value::as_array) {
                    for sc in scenes {
                        let sid = sc.get("id").and_then(Value::as_str).unwrap_or("");
                        let st = sc.get("title").and_then(Value::as_str).unwrap_or("");
                        volatile.push_str(&format!("  - 场景 {st}({sid})\n"));
                        if let Some(goals) = sc.get("goals").and_then(Value::as_array) {
                            for g in goals {
                                let gid = g.get("id").and_then(Value::as_str).unwrap_or("");
                                let gt = g.get("text").and_then(Value::as_str).unwrap_or("");
                                if !gid.is_empty() {
                                    volatile.push_str(&format!("    - 目标({gid}) {gt}\n"));
                                }
                            }
                        }
                        if let Some(triggers) = sc.get("triggers").and_then(Value::as_array) {
                            for t in triggers {
                                let tid = t.get("id").and_then(Value::as_str).unwrap_or("");
                                let tt = t.get("title").and_then(Value::as_str).unwrap_or("");
                                if !tid.is_empty() {
                                    volatile.push_str(&format!("    - 触发点({tid}) {tt}\n"));
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
            volatile.push_str("\n【本次改动的目标实体（最高优先级）】\n");
            volatile.push_str("创作者明确引用了以下实体。请把它们作为本次改动的主要目标，并基于其完整现状作答：\n");
            for f in focus {
                let id = f.id.as_deref().unwrap_or("-");
                volatile.push_str(&format!(
                    "\n- {}「{}」({})\n```json\n{}\n```\n",
                    f.kind,
                    f.name,
                    id,
                    serde_json::to_string_pretty(&f.entity).unwrap_or_else(|_| "{}".to_string())
                ));
            }
            volatile.push_str(
                "\n若为达成目标必须改动其他实体（例如为被引用的人物新建配套技能），可以提出，但请在说明里明确指出这是目标之外的改动。\n",
            );
        }
    }

    s.push_str(&prompts.schema);

    if tool_mode {
        s.push_str(&prompts.tools);
    }

    (s, volatile)
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
            // 思考块必须在最前面：供应商按顺序回放，且 DeepSeek 要求 tool_calls 轮带上它。
            if let Some(reasoning) = m.reasoning.as_deref().map(str::trim) {
                if !reasoning.is_empty() {
                    content.push(AssistantContent::Reasoning(Reasoning::new(reasoning)));
                }
            }
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

// ============================================================
// 结对上下文压缩（服务端裁 surface；对齐 DSH compaction-basic 的语义）
//
// 前端照旧发**全量展示历史**；「这一轮到底发什么给模型」是后端裁的：
//   [稳定 system, (压缩检查点)?, ...尾巴..., 本轮草稿快照]
// 权威展示历史 `pair_messages` 永不改写——压缩不会让创作者丢掉任何一条对话，
// 换设备/刷新后照样完整。检查点落 `pair_threads.compaction_json`（派生数据）。
// ============================================================

/// 结对压缩的「字符 / token」粗估系数：草稿与对话以中文为主（实测 ≈1.8）。
/// 阈值给到窗口的 80%，估算误差 ±30% 只会让它早压或晚压一点，不影响正确性。
const PAIR_CHARS_PER_TOKEN: f64 = 1.8;

/// 摘要替换块的抬头：写清「这是数据、不是创作者的新发言」。
const PAIR_CHECKPOINT_PREAMBLE: &str = "以下是自动生成的上下文存档：它压缩了更早的一段结对对话，用来腾出上下文。把它当作既定的背景事实，接着后面的消息继续，不要在回复里提到这份存档。";

/// 前端 OpenAI 风格工具定义 → rig 工具定义（压缩请求要复用它，前缀才对得上）。
fn pair_tools(req: &PairChatRequest) -> Vec<ToolDefinition> {
    req.tools
        .as_ref()
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(convert_tool).collect())
        .unwrap_or_default()
}

/// 压缩检查点 → 一条 user 消息（措辞固定，逐字回放）。
fn checkpoint_message(summary: &str) -> RigMessage {
    RigMessage::User {
        content: vec![UserContent::Text(Text::new(format!(
            "{PAIR_CHECKPOINT_PREAMBLE}\n\n<已压缩摘要>\n{}\n</已压缩摘要>",
            summary.trim()
        )))],
    }
}

/// 本次要发给模型的消息：`(检查点)? + messages[shadowed..]`。
///
/// 转换时**每条都过一遍** convert_message（维持 tool_call_id → name 的映射），
/// 只把尾巴那部分留下。
fn surface_messages(req: &PairChatRequest, compaction: Option<&PairCompaction>) -> Vec<RigMessage> {
    let mut names: HashMap<String, String> = HashMap::new();
    let skip = compaction
        .map(|c| c.shadowed.min(req.messages.len()))
        .unwrap_or(0);
    let mut out: Vec<RigMessage> = Vec::new();
    if let Some(c) = compaction {
        out.push(checkpoint_message(&c.summary));
    }
    for (i, m) in req.messages.iter().enumerate() {
        let converted = convert_message(m, &mut names);
        if i >= skip {
            if let Some(msg) = converted {
                out.push(msg);
            }
        }
    }
    out
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= *b as u64;
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// 遮蔽前缀的指纹（FNV-1a 64，跨版本稳定）：判断前端这次发来的历史还是不是当初那段。
/// 清空 / 换线程 / 改了历史都会让它对不上，检查点随即作废重压。
fn prefix_fingerprint(messages: &[ChatMessage]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for m in messages {
        fnv1a(&mut hash, m.role.as_bytes());
        fnv1a(&mut hash, &[0]);
        fnv1a(&mut hash, m.content.as_bytes());
        fnv1a(&mut hash, &[0xff]);
    }
    format!("{hash:016x}")
}

/// 压缩切点只能落在**真正的用户消息**上：`role = "tool"` 是工具结果，assistant 带
/// tool_calls 的那条也不能劈开。往前找（只会保留更多）。
fn snap_to_user_turn(messages: &[ChatMessage], idx: usize) -> Option<usize> {
    let mut i = idx.min(messages.len().saturating_sub(1));
    loop {
        if messages[i].role == "user" {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// 选本次要遮蔽到哪：从尾巴往前累加字符，至少保留 `retain_chars`；再吸附到用户消息
/// 起点。`keep_tail` 是「至少留几条消息」（兜底压缩也要留一口气）。
/// 返回保留段的第一条下标；切不动（全都要留）返回 None。
fn select_pair_cut(
    messages: &[ChatMessage],
    retain_chars: usize,
    keep_tail: usize,
) -> Option<usize> {
    if messages.len() < 2 {
        return None;
    }
    let mut acc = 0usize;
    let mut idx = messages.len() - 1;
    loop {
        acc += messages[idx].content.chars().count();
        if acc >= retain_chars || idx == 0 {
            break;
        }
        idx -= 1;
    }
    while messages.len() - idx < keep_tail.max(1) && idx > 0 {
        idx -= 1;
    }
    let cut = snap_to_user_turn(messages, idx)?;
    if cut == 0 { None } else { Some(cut) }
}

/// 该模型声明的上下文窗口（config.json 的 `models[].ctx`）。
fn model_context_window(provider: &ProviderConfig, model: &str) -> Option<u64> {
    provider
        .models
        .iter()
        .find(|m| m.id == model)
        .and_then(|m| m.ctx)
        .map(|c| c as u64)
        .filter(|c| *c > 0)
}

/// 服务端的结对压缩：读检查点 → 判断是否需要压 → 摘要一段并落库。
///
/// 返回本轮该用的检查点（可能是旧的；压不动/失败时原样返回）。
/// 摘要请求逐字重放当前 surface 前缀（system + 旧检查点 + 新遮蔽段）+ 末尾指令，
/// 复用供应商的热缓存——只有指令与输出未命中（DSH summarizer 的同款做法）。
#[allow(clippy::too_many_arguments)]
/// 结对对话请求体里带 thread_id 时必须是自己线程（路径上没有 id，靠这里 fail-closed）。
async fn ensure_thread_owner(
    app: &AppState,
    req: &PairChatRequest,
    user_id: &str,
) -> Result<(), ApiError> {
    match req.thread_id.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(thread_id) => crate::auth::ensure_pair_thread_owner(app, thread_id, user_id).await,
        None => Ok(()),
    }
}

async fn plan_pair_compaction(
    app: &AppState,
    req: &PairChatRequest,
    prompts: &PairPrompts,
    client: &openai::CompletionsClient,
    model: &str,
    window: Option<u64>,
    system: &str,
    tools: &[ToolDefinition],
    force: bool,
) -> Option<PairCompaction> {
    let thread_id = req
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    // 现存的检查点：指纹对不上（清空 / 换线程 / 改过历史）就作废。
    let existing = app
        .store()
        .pair_thread_compaction(thread_id)
        .await
        .ok()
        .flatten()
        .filter(|c| {
            c.shadowed <= req.messages.len()
                && prefix_fingerprint(&req.messages[..c.shadowed]) == c.fingerprint
        });
    let skip = existing.as_ref().map(|c| c.shadowed).unwrap_or(0);
    // 当前 surface 的体量（系统层 + 检查点 + 尾巴）。
    let surface_chars = system.chars().count()
        + existing
            .as_ref()
            .map(|c| c.summary.chars().count())
            .unwrap_or(0)
        + req.messages[skip..]
            .iter()
            .map(|m| m.content.chars().count())
            .sum::<usize>();
    let spec = load_config_from_disk().ai.compaction;
    let retain_chars = if force {
        0
    } else {
        let threshold_window = window?;
        let threshold = (threshold_window as f64 * spec.threshold_ratio.clamp(0.1, 1.0)) as u64;
        if ((surface_chars as f64 / PAIR_CHARS_PER_TOKEN).ceil() as u64) < threshold {
            return existing;
        }
        (threshold_window as f64 * spec.retain_ratio.clamp(0.0, 0.9) * PAIR_CHARS_PER_TOKEN)
            as usize
    };
    let keep_tail = if force { 4 } else { 1 };
    let Some(cut) = select_pair_cut(&req.messages, retain_chars, keep_tail) else {
        return existing;
    };
    if cut <= skip {
        return existing;
    }
    // 摘要请求：逐字重放当前 surface 前缀 + 末尾压缩指令。
    let mut history: Vec<RigMessage> = Vec::new();
    if let Some(c) = &existing {
        history.push(checkpoint_message(&c.summary));
    }
    let mut names: HashMap<String, String> = HashMap::new();
    for m in &req.messages[skip..cut] {
        if let Some(msg) = convert_message(m, &mut names) {
            history.push(msg);
        }
    }
    history.push(RigMessage::User {
        content: vec![UserContent::Text(Text::new(prompts.compaction.clone()))],
    });
    let chars_before = existing
        .as_ref()
        .map(|c| c.summary.chars().count())
        .unwrap_or(0)
        + req.messages[skip..cut]
            .iter()
            .map(|m| m.content.chars().count())
            .sum::<usize>();
    let request = CompletionRequest {
        model: None,
        preamble: Some(system.to_string()),
        chat_history: history,
        documents: Vec::new(),
        tools: tools.to_vec(),
        temperature: Some(0.3),
        max_tokens: Some(spec.max_tokens.clamp(512, 200_000)),
        tool_choice: None,
        additional_params: None,
        output_schema: None,
        record_telemetry_content: false,
    };
    let started = std::time::Instant::now();
    let response = match client
        .completion_model(model.to_string())
        .completion(request)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(thread_id, error = %e, "结对压缩的摘要请求失败（本轮照常继续）");
            return existing;
        }
    };
    let (text, _tool_calls) = split_choice(&response.choice);
    let summary = text.trim().to_string();
    let after = summary.chars().count();
    if after == 0 || after >= chars_before {
        tracing::warn!(
            thread_id,
            before = chars_before,
            after,
            "结对压缩：摘要没比原文更短，放弃"
        );
        return existing;
    }
    let next = PairCompaction {
        shadowed: cut,
        fingerprint: prefix_fingerprint(&req.messages[..cut]),
        summary,
        chars_before: chars_before as u64,
        chars_after: after as u64,
    };
    if let Err(e) = app
        .store()
        .set_pair_thread_compaction(thread_id, Some(&next))
        .await
    {
        tracing::warn!(thread_id, error = %e, "结对压缩检查点落库失败（本轮仍按压缩后的 surface 发）");
    }
    tracing::info!(
        thread_id,
        force,
        shadowed = cut,
        chars_before,
        chars_after = after,
        input = response.usage.input_tokens,
        cached = response.usage.cached_input_tokens,
        latency_ms = started.elapsed().as_millis() as u64,
        "结对上下文压缩完成（只改模型 surface，展示历史不动）"
    );
    Some(next)
}
/// token 粗估（与前端 `lib/tokens.ts` 同口径）：CJK 1，其余 1/4。
/// 只用于状态行报「发给模型的构成」，不参与阈值 / 选区判定。
fn estimate_meta_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for ch in text.chars() {
        let c = ch as u32;
        if (0x4e00..=0x9fff).contains(&c)
            || (0x3000..=0x303f).contains(&c)
            || (0xff00..=0xffef).contains(&c)
        {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

/// 一条 rig 消息的 token 粗估（正文 / 工具结果 / 工具调用参数）。
fn rig_message_tokens(m: &RigMessage) -> usize {
    match m {
        RigMessage::System { content } => estimate_meta_tokens(content),
        RigMessage::User { content } => content
            .iter()
            .map(|c| match c {
                UserContent::Text(t) => estimate_meta_tokens(&t.text),
                UserContent::ToolResult(r) => r
                    .content
                    .iter()
                    .map(|x| match x {
                        ToolResultContent::Text(t) => estimate_meta_tokens(&t.text),
                        _ => 0,
                    })
                    .sum(),
                _ => 0,
            })
            .sum(),
        RigMessage::Assistant { content, .. } => content
            .iter()
            .map(|c| match c {
                AssistantContent::Text(t) => estimate_meta_tokens(&t.text),
                AssistantContent::ToolCall(tc) => {
                    estimate_meta_tokens(&tc.function.arguments.to_string())
                }
                _ => 0,
            })
            .sum(),
    }
}

/// 本轮真正发给模型的上下文构成（字符）：给编辑器状态行一个诚实口径——
/// 前端只知道自己发了多少展示历史，压缩之后那个数会偏大。
fn context_meta(
    req: &PairChatRequest,
    system: &str,
    history: &[RigMessage],
    tail_context: &str,
    tools: &[ToolDefinition],
    compaction: Option<&PairCompaction>,
) -> Value {
    serde_json::json!({
        "system_tokens": estimate_meta_tokens(system),
        "history_tokens": history.iter().map(rig_message_tokens).sum::<usize>(),
        "tail_context_tokens": estimate_meta_tokens(tail_context),
        "tools_tokens": estimate_meta_tokens(&serde_json::to_string(tools).unwrap_or_default()),
        "display_messages": req.messages.len(),
        "sent_messages": history.len() + 1,
        "shadowed_messages": compaction.map(|c| c.shadowed).unwrap_or(0),
        "compacted": compaction.is_some(),
    })
}

/// 组装 rig CompletionRequest（稳定 system preamble + 全量历史 + tools + 本轮上下文块）
///
/// 消息顺序是缓存的关键：`[稳定 system, ...历史..., 本轮请求, 本轮草稿快照]`。
/// 快照永远在**最后**、且每次重新生成（不进历史），所以「system + 历史」这段前缀逐字节
/// 稳定，供应商的前缀缓存能整段复用；草稿怎么变都只影响最后那一条。
fn build_completion_request(
    req: &PairChatRequest,
    temp: f64,
    max_tokens: u32,
    sampling: serde_json::Value,
    prompts: &PairPrompts,
    compaction: Option<&PairCompaction>,
) -> (CompletionRequest, Value) {
    let tools = pair_tools(req);
    let has_tools = !tools.is_empty();
    let (system, context) = build_prompts(
        req.storybook.as_ref(),
        has_tools,
        req.focus.as_deref(),
        prompts,
    );
    let mut chat_history = surface_messages(req, compaction);
    // 状态行口径：在压上本轮快照之前取，历史与快照分开报。
    let meta = context_meta(req, &system, &chat_history, &context, &tools, compaction);
    chat_history.push(RigMessage::User {
        content: vec![UserContent::Text(Text::new(context))],
    });
    let request = CompletionRequest {
        model: None,
        preamble: Some(system),
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
    };
    (request, meta)
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
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Json(req): Json<PairChatRequest>,
) -> Result<Json<PairChatResponse>, ApiError> {
    ensure_thread_owner(&app, &req, user.id()).await?;
    let (provider, model, temp, max_tokens, sampling) = resolve_provider_and_model(&req)?;
    let client = build_pair_client(&provider)?;
    let rig_model = client.completion_model(model.clone());
    // 提示词即改即生效：每次都读盘上的覆盖表（结对请求频率低，代价可忽略）。
    let prompts = PairPrompts::from_overrides(&load_config_from_disk().prompts);
    // 服务端裁 surface：前端照旧发全量展示历史，压不压、压到哪由后端定。
    let window = model_context_window(&provider, &model);
    let tools = pair_tools(&req);
    let (system, _) = build_prompts(
        req.storybook.as_ref(),
        !tools.is_empty(),
        req.focus.as_deref(),
        &prompts,
    );
    let compaction = plan_pair_compaction(
        &app, &req, &prompts, &client, &model, window, &system, &tools, false,
    )
    .await;
    let (mut request, mut context) = build_completion_request(
        &req,
        temp,
        max_tokens,
        sampling.clone(),
        &prompts,
        compaction.as_ref(),
    );

    let response = match rig_model.completion(request).await {
        Ok(r) => r,
        Err(e) => {
            // 供应商确认上下文超限：更激进地压一次再重试（DSH 的 overflow 恢复）。
            if !is_context_overflow(&e.to_string()) {
                return Err(completion_error(e));
            }
            let Some(forced) = plan_pair_compaction(
                &app, &req, &prompts, &client, &model, window, &system, &tools, true,
            )
            .await
            else {
                return Err(completion_error(e));
            };
            (request, context) = build_completion_request(
                &req,
                temp,
                max_tokens,
                sampling,
                &prompts,
                Some(&forced),
            );
            rig_model
                .completion(request)
                .await
                .map_err(completion_error)?
        }
    };
    let (full_content, tool_calls) = split_choice(&response.choice);
    let (clean_text, suggestions) = if tool_calls.is_empty() {
        extract_suggestions(&full_content)
    } else {
        (full_content.trim().to_string(), Vec::new())
    };

    let finish_reason = response.finish_reason().map(|r| match r {
        FinishReason::Stop => "stop".to_string(),
        FinishReason::Length => "length".to_string(),
        FinishReason::ToolCalls => "tool_calls".to_string(),
        FinishReason::ContentFilter => "content_filter".to_string(),
        FinishReason::Other(s) => s,
    });

    Ok(Json(PairChatResponse {
        text: clean_text.clone(),
        deltas: vec![clean_text],
        suggestions,
        tool_calls,
        finish_reason,
        context: Some(context),
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
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Json(req): Json<PairChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    ensure_thread_owner(&app, &req, user.id()).await?;
    let (provider, model, temp, max_tokens, sampling) = resolve_provider_and_model(&req)?;
    let model_id = model.clone();
    let client = build_pair_client(&provider)?;
    let rig_model = client.completion_model(model.clone());
    // 提示词即改即生效：每次都读盘上的覆盖表（结对请求频率低，代价可忽略）。
    let prompts = PairPrompts::from_overrides(&load_config_from_disk().prompts);
    // 服务端裁 surface：前端照旧发全量展示历史，压不压、压到哪由后端定。
    let window = model_context_window(&provider, &model);
    let tools = pair_tools(&req);
    let (system, _) = build_prompts(
        req.storybook.as_ref(),
        !tools.is_empty(),
        req.focus.as_deref(),
        &prompts,
    );
    let compaction = plan_pair_compaction(
        &app, &req, &prompts, &client, &model, window, &system, &tools, false,
    )
    .await;
    let (mut request, mut context) = build_completion_request(
        &req,
        temp,
        max_tokens,
        sampling.clone(),
        &prompts,
        compaction.as_ref(),
    );
    let tool_mode = !request.tools.is_empty();
    // 日志要用的元信息，必须在 request 被 move 之前取出来
    let tool_count = request.tools.len();
    let message_count = request.chat_history.len();

    let upstream = match rig_model.stream(request).await {
        Ok(u) => u,
        Err(e) => {
            // 供应商确认上下文超限：更激进地压一次再重试（DSH 的 overflow 恢复）。
            if !is_context_overflow(&e.to_string()) {
                return Err(completion_error(e));
            }
            let Some(forced) = plan_pair_compaction(
                &app, &req, &prompts, &client, &model, window, &system, &tools, true,
            )
            .await
            else {
                return Err(completion_error(e));
            };
            (request, context) = build_completion_request(
                &req,
                temp,
                max_tokens,
                sampling,
                &prompts,
                Some(&forced),
            );
            rig_model.stream(request).await.map_err(completion_error)?
        }
    };

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(100);

    tokio::spawn(async move {
        let mut upstream = Box::pin(upstream);
        let mut full_text = String::new();
        let mut reasoning_text = String::new();
        // 本轮思考正文（增量累积）：作为 done 帧的 reasoning 回给前端，多步工具循环要原样回传。
        let mut reasoning_for_replay = String::new();
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
                        reasoning_for_replay = text.clone();
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
                        reasoning_for_replay.push_str(&reasoning);
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

        // 输出预算被用尽（finish_reason=length）时，尾部工具调用的 arguments 是半截 JSON。
        // 绝不能把它当成 {} 执行——那会拿空参数去建实体；标记 arguments_valid=false，
        // 由前端拒绝执行并把「参数不完整」回灌给模型，让它重发一次。
        let mut incomplete_tool_calls = 0usize;
        let tool_calls: Vec<Value> = order
            .iter()
            .filter_map(|k| calls.get(k))
            .map(|(name, args, id)| match serde_json::from_str::<Value>(args) {
                Ok(parsed) => serde_json::json!({
                    "id": id, "name": name, "arguments": parsed.to_string(), "arguments_valid": true
                }),
                Err(_) => {
                    incomplete_tool_calls += 1;
                    serde_json::json!({
                        "id": id, "name": name, "arguments": args, "arguments_valid": false
                    })
                }
            })
            .collect();

        if incomplete_tool_calls > 0 {
            tracing::warn!(
                incomplete_tool_calls,
                truncated = finish_reason.as_deref() == Some("length"),
                max_tokens,
                "结对：工具调用参数不完整（输出预算被用尽），已标记为不可执行"
            );
        }

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
            "context": context,
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
                "context": context.clone(),
                "reasoning_chars": reasoning_chars,
                "reasoning": reasoning_text,
                // 回传给下一轮请求用的思考正文（与展示用的 reasoning 相同，单独留名以免被 UI 改造牵连）
                "reasoning_for_replay": reasoning_for_replay,
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
    fn explicit_max_tokens_wins_and_is_clamped() {
        assert_eq!(effective_max_tokens(Some(8192), 4096, 0), 8192);
        assert_eq!(effective_max_tokens(Some(10), 4096, 0), PAIR_MAX_TOKENS_MIN_EXPLICIT);
        assert_eq!(effective_max_tokens(Some(u32::MAX), 4096, 0), PAIR_MAX_TOKENS_CEIL);
    }

    #[test]
    fn legacy_config_value_is_lifted_to_the_floor() {
        // config 里写着历史默认 4096、模型也没标 max_out：预算必须被抬到下限，
        // 否则思考会把整份预算烧光、正文一个字都出不来。
        assert_eq!(effective_max_tokens(None, 4096, 0), PAIR_MAX_TOKENS_FLOOR);
        assert_eq!(effective_max_tokens(None, 4096, 2048), PAIR_MAX_TOKENS_FLOOR);
    }

    #[test]
    fn adaptive_default_takes_the_largest_and_respects_the_ceiling() {
        assert_eq!(effective_max_tokens(None, 32000, 8192), 32000);
        assert_eq!(effective_max_tokens(None, 0, 32000), 32000);
        assert_eq!(effective_max_tokens(None, 0, 0), PAIR_MAX_TOKENS_FLOOR);
        assert_eq!(effective_max_tokens(None, 0, 500_000), PAIR_MAX_TOKENS_CEIL);
    }

    #[test]
    fn assistant_message_keeps_reasoning_for_replay() {
        // DeepSeek 思考模式：带 tool_calls 的助手轮若丢掉 reasoning_content，下一轮直接 400。
        let m = ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(serde_json::json!([{
                "id": "call-1",
                "type": "function",
                "function": { "name": "web_fetch", "arguments": "{}" }
            }])),
            tool_call_id: None,
            attachments: None,
            reasoning: Some("先查规则再落设定".into()),
        };
        let mut names = HashMap::new();
        let msg = convert_message(&m, &mut names).expect("assistant message");
        let content = match msg {
            RigMessage::Assistant { content, .. } => content,
            _ => panic!("expected assistant message"),
        };
        assert!(
            matches!(content.first(), Some(AssistantContent::Reasoning(r))
                if matches!(r.content.first(), Some(ReasoningContent::Text { text, .. }) if text == "先查规则再落设定")),
            "思考块必须排在工具调用之前：{content:?}"
        );
        assert!(matches!(content.last(), Some(AssistantContent::ToolCall(_))));
    }

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
            reasoning: None,
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

    /// 逐回合会变的草稿快照必须走请求的**最后一条消息**，不能进系统层。
    ///
    /// 系统层在请求最前面：它一变，前缀缓存（系统层 + 全部历史）整段失效；而结对
    /// 每采纳一次建议、每跑一步工具草稿就变一次——放系统层等于把缓存永久废掉。
    #[test]
    fn mutable_draft_context_is_the_last_message_not_the_system_prompt() {
        let storybook: PairStorybookContext = serde_json::from_value(serde_json::json!({
            "title": "露西的第一课",
            "characters": [{ "id": "char-lucy", "name": "露西", "kind": "npc" }]
        }))
        .expect("storybook ctx");
        let other: PairStorybookContext = serde_json::from_value(serde_json::json!({
            "title": "另一个标题",
            "characters": [{ "id": "char-hugo", "name": "雨果", "kind": "npc" }]
        }))
        .expect("storybook ctx");
        let prompts = PairPrompts::default();

        // ① 草稿变了，系统层必须逐字节不变（前缀缓存稳定的充要条件）。
        let (sys_a, ctx_a) = build_prompts(Some(&storybook), false, None, &prompts);
        let (sys_b, ctx_b) = build_prompts(Some(&other), false, None, &prompts);
        assert_eq!(
            sys_a, sys_b,
            "草稿变了系统层不能跟着变，否则整段会话缓存失效"
        );
        assert_ne!(ctx_a, ctx_b);
        assert!(sys_a.contains("AI 结对创作搭档"));
        assert!(
            !sys_a.contains("露西的第一课"),
            "草稿快照不能进系统层：{sys_a}"
        );
        assert!(!sys_a.contains("char-lucy"), "实体 id 目录不能进系统层");

        // ② 组装请求时，草稿快照必须是最后一条消息（历史之后）。
        let req = PairChatRequest {
            provider_id: None,
            model: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "把露西的性格写细一点".into(),
                tool_calls: None,
                tool_call_id: None,
                attachments: None,
                reasoning: None,
            }],
            storybook: Some(storybook),
            custom_provider: None,
            tools: None,
            focus: None,
            thread_id: None,
            max_tokens: None,
        };
        let (request, _context) =
            build_completion_request(&req, 0.8, 4096, serde_json::json!({}), &prompts, None);
        let system = request.preamble.expect("system prompt");
        assert!(!system.contains("露西的第一课"), "草稿快照不能进系统层");
        assert_eq!(request.chat_history.len(), 2, "历史 1 条 + 末尾快照 1 条");
        let text = match request.chat_history.last().expect("本轮上下文块") {
            RigMessage::User { content } => content
                .iter()
                .find_map(|c| match c {
                    UserContent::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            other => panic!("上下文块必须是 user 消息：{other:?}"),
        };
        assert!(text.contains("露西的第一课"), "草稿快照要在最后一条消息里");
        assert!(text.contains("char-lucy"), "实体 id 目录要在最后一条消息里");
    }
    /// 状态行口径：token 粗估与前端 `lib/tokens.ts` 同口径（CJK 1、其余 1/4）。
    #[test]
    fn meta_token_estimate_matches_the_frontend_convention() {
        assert_eq!(super::estimate_meta_tokens(""), 0);
        assert_eq!(super::estimate_meta_tokens("四个汉字"), 4);
        assert_eq!(super::estimate_meta_tokens("abcd"), 1);
        assert_eq!(super::estimate_meta_tokens("汉字abcd"), 3);
    }

    /// 结对压缩：切点落在真正的用户消息上（tool 结果 / assistant 不能当切点）。
    #[test]
    fn pair_cut_lands_on_user_turns() {
        let msg = |role: &str| ChatMessage {
            role: role.into(),
            content: "x".repeat(100),
            tool_calls: None,
            tool_call_id: None,
            attachments: None,
            reasoning: None,
        };
        let mut msgs = vec![msg("user"), msg("assistant")];
        msgs.push(msg("user"));
        msgs.push(msg("assistant"));
        msgs.push(msg("user"));
        msgs.push(msg("tool"));
        msgs.push(msg("user"));
        msgs.push(msg("assistant"));
        // 末尾是 tool：往前吸附到最近一条用户消息。
        let tail = vec![msg("user"), msg("assistant"), msg("user"), msg("tool")];
        assert_eq!(super::select_pair_cut(&tail, 0, 1), Some(2));
        // 预算覆盖全部历史：压不动。
        assert!(super::select_pair_cut(&msgs, 100_000, 1).is_none());
        // 常规切点：落在 user 上，且尾巴不为空。
        let cut = super::select_pair_cut(&msgs, 200, 1).expect("有得压");
        assert_eq!(msgs[cut].role, "user");
        assert!(cut > 0 && cut < msgs.len());
        // 兜底压缩至少留 keep_tail 条。
        let cut = super::select_pair_cut(&msgs, 0, 4).expect("有得压");
        assert!(msgs.len() - cut >= 4, "兜底也要留一口气");
    }

    /// 前缀指纹：同前缀稳定、改内容就变（检查点据此作废重压）。
    #[test]
    fn pair_prefix_fingerprint_is_stable_and_prefix_only() {
        let msg = |c: &str| ChatMessage {
            role: "user".into(),
            content: c.into(),
            tool_calls: None,
            tool_call_id: None,
            attachments: None,
            reasoning: None,
        };
        let a = vec![msg("甲"), msg("乙")];
        assert_eq!(super::prefix_fingerprint(&a), super::prefix_fingerprint(&a.clone()));
        let longer = vec![msg("甲"), msg("乙"), msg("丙")];
        assert_eq!(
            super::prefix_fingerprint(&a),
            super::prefix_fingerprint(&longer[..2]),
            "只看遮蔽前缀"
        );
        let changed = vec![msg("甲"), msg("丙")];
        assert_ne!(super::prefix_fingerprint(&a), super::prefix_fingerprint(&changed));
    }

    /// 检查点消息措辞固定（逐字回放，前缀缓存才稳）。
    #[test]
    fn pair_checkpoint_message_is_framed() {
        let m = super::checkpoint_message("  摘要正文  ");
        let text = match m {
            RigMessage::User { content } => content
                .into_iter()
                .find_map(|c| match c {
                    UserContent::Text(t) => Some(t.text),
                    _ => None,
                })
                .unwrap_or_default(),
            other => panic!("检查点必须是 user 消息：{other:?}"),
        };
        assert!(text.contains("<已压缩摘要>") && text.contains("</已压缩摘要>"));
        assert!(text.contains("摘要正文"));
        assert!(!text.contains("  摘要正文  "), "摘要要去掉首尾空白");
    }
}
