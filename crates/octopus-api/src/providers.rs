//! 供应商模型探测（#26）：服务端 GET {base_url}/models，避免把 API Key 暴露给浏览器。

use std::time::Duration;

use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    /// 协议类型（ollama / openai / anthropic / openai-compatible），暂只作记录。
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelItem {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProbeResponse {
    pub models: Vec<ModelItem>,
    pub source: String,
}

pub async fn probe_models(Json(req): Json<ProbeRequest>) -> Result<Json<ProbeResponse>, ApiError> {
    let base = req.base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "empty_base_url", "Base URL 不能为空"));
    }
    let url = format!("{base}/models");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("octopus/0.1")
        .build()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "http_client", e.to_string()))?;

    let mut rb = client.get(&url);
    if let Some(key) = req.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        rb = rb.bearer_auth(key);
    }

    let resp = rb
        .send()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "probe_failed", format!("请求 {url} 失败：{e}")))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "probe_read_failed", e.to_string()))?;

    if !status.is_success() {
        let snippet: String = body.chars().take(200).collect();
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "probe_http_error",
            format!("{status} {snippet}"),
        ));
    }

    let json: Value = serde_json::from_str(&body)
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "probe_parse_failed", e.to_string()))?;
    let models = parse_models(&json);
    if models.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "probe_empty",
            "端点返回成功但未解析到模型（兼容 OpenAI /models 形状）",
        ));
    }
    Ok(Json(ProbeResponse { models, source: url }))
}

/// 兼容多种形状：OpenAI `{data:[{id,name?}]}`、`{models:[...]}`、`{result:[...]}`、顶层数组。
fn parse_models(v: &Value) -> Vec<ModelItem> {
    let arr = v
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| v.get("models").and_then(Value::as_array))
        .or_else(|| v.get("result").and_then(Value::as_array))
        .or_else(|| v.as_array());
    let Some(arr) = arr else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        match item {
            Value::String(s) => out.push(ModelItem { id: s.clone(), name: None }),
            Value::Object(o) => {
                let id = o
                    .get("id")
                    .or_else(|| o.get("name"))
                    .or_else(|| o.get("model"))
                    .and_then(Value::as_str);
                if let Some(id) = id {
                    let name = o
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|n| n != id);
                    out.push(ModelItem { id: id.to_string(), name });
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct ProviderTestRequest {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct ProviderTestResponse {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

pub async fn test_provider(Json(req): Json<ProviderTestRequest>) -> Result<Json<ProviderTestResponse>, ApiError> {
    let base = req.base_url.as_deref().unwrap_or("").trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Ok(Json(ProviderTestResponse {
            ok: false,
            message: "未填写 Base URL".into(),
            latency_ms: None,
        }));
    }
    let kind = req.kind.as_deref().unwrap_or("openai-compatible");
    let is_local = base.contains("127.0.0.1") || base.contains("localhost");
    let key = req.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty());
    if kind != "ollama" && !is_local && key.is_none() {
        return Ok(Json(ProviderTestResponse {
            ok: false,
            message: "未填写 API Key".into(),
            latency_ms: None,
        }));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("octopus/0.1")
        .build()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "http_client", e.to_string()))?;

    let start = std::time::Instant::now();

    // 先测 GET {base}/models
    let models_url = format!("{base}/models");
    let mut rb = client.get(&models_url);
    if let Some(k) = key {
        rb = rb.bearer_auth(k);
    }
    match rb.send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let latency_ms = start.elapsed().as_millis() as u64;
                return Ok(Json(ProviderTestResponse {
                    ok: true,
                    message: "连通正常（GET /models 成功）".into(),
                    latency_ms: Some(latency_ms),
                }));
            } else if status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED {
                // 部分兼容端点不支持 GET /models，尝试简短 chat completions
                let chat_url = format!("{base}/chat/completions");
                let mut crb = client.post(&chat_url);
                if let Some(k) = key {
                    crb = crb.bearer_auth(k);
                }
                let model_name = req.models.first()
                    .and_then(|m| m.get("id").and_then(Value::as_str))
                    .unwrap_or("gpt-3.5-turbo");
                let body = serde_json::json!({
                    "model": model_name,
                    "messages": [{"role": "user", "content": "ping"}],
                    "max_tokens": 1
                });
                match crb.json(&body).send().await {
                    Ok(cresp) => {
                        let cstatus = cresp.status();
                        let latency_ms = start.elapsed().as_millis() as u64;
                        if cstatus.is_success() {
                            return Ok(Json(ProviderTestResponse {
                                ok: true,
                                message: "连通正常（Chat Completions 成功）".into(),
                                latency_ms: Some(latency_ms),
                            }));
                        } else if cstatus == StatusCode::UNAUTHORIZED || cstatus == StatusCode::FORBIDDEN {
                            return Ok(Json(ProviderTestResponse {
                                ok: false,
                                message: format!("认证失败（HTTP {cstatus}）：API Key 无效或未授权"),
                                latency_ms: None,
                            }));
                        } else {
                            let text = cresp.text().await.unwrap_or_default();
                            let snippet: String = text.chars().take(120).collect();
                            return Ok(Json(ProviderTestResponse {
                                ok: false,
                                message: format!("HTTP {cstatus}：{snippet}"),
                                latency_ms: None,
                            }));
                        }
                    }
                    Err(e) => {
                        return Ok(Json(ProviderTestResponse {
                            ok: false,
                            message: format!("请求聊天端点失败：{e}"),
                            latency_ms: None,
                        }));
                    }
                }
            } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Ok(Json(ProviderTestResponse {
                    ok: false,
                    message: format!("认证失败（HTTP {status}）：请检查 API Key 是否正确"),
                    latency_ms: None,
                }));
            } else {
                let text = resp.text().await.unwrap_or_default();
                let snippet: String = text.chars().take(120).collect();
                return Ok(Json(ProviderTestResponse {
                    ok: false,
                    message: format!("端点返回错误 HTTP {status}：{snippet}"),
                    latency_ms: None,
                }));
            }
        }
        Err(e) => {
            return Ok(Json(ProviderTestResponse {
                ok: false,
                message: format!("无法连接到端点：{e}"),
                latency_ms: None,
            }));
        }
    }
}

