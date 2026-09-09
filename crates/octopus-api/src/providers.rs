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
