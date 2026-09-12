//! AI provider 装配（#20 组合根）：把 config.json 的 providers / roles 映射成 rig provider。
//!
//! 默认在配置了可用 API Key / 本地端点时启用 rig；否则回退 ScriptedProvider。
//! embedding 若指向对话模型（模型名不像 embedding 模型），自动回退 StubEmbedding 并告警。
//! 环境变量 OCTOPUS_AI=scripted / OCTOPUS_EMBEDDING=stub 可强制使用确定性实现（离线 / 测试）。

use std::sync::Arc;

use octopus_ai::{
    RigEmbedding, RigParams, RigProvider, RigProviderParams, RigRoleParams, ScriptedProvider, StubEmbedding,
};
use octopus_engine::{AiProvider, EmbeddingBackend};
use rig::client::EmbeddingsClient;
use rig::providers::openai;

use crate::config::{AppConfig, RoleConfig};

/// 把 config.json 的 providers 映射成「可用供应商」清单（缺 Base URL / Key 的跳过）。
fn provider_entries(cfg: &AppConfig) -> Vec<RigProviderParams> {
    let mut out = Vec::new();
    for p in &cfg.providers {
        let base_url = p.base_url.as_deref().unwrap_or("").trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            continue;
        }
        let key = p.api_key.as_deref().unwrap_or("").trim().to_string();
        let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
        if key.is_empty() && !is_local && p.kind != "ollama" {
            continue;
        }
        out.push(RigProviderParams {
            id: p.id.clone(),
            base_url,
            api_key: if key.is_empty() { "not-needed".to_string() } else { key },
        });
    }
    out
}

/// 采样参数 → 供应商追加参数对象；未设的字段不发，保留供应商默认。
pub(crate) fn sampling_params(role: &RoleConfig) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(v) = role.top_p {
        m.insert("top_p".into(), serde_json::json!(v));
    }
    if let Some(v) = role.presence_penalty {
        m.insert("presence_penalty".into(), serde_json::json!(v));
    }
    if let Some(v) = role.frequency_penalty {
        m.insert("frequency_penalty".into(), serde_json::json!(v));
    }
    if let Some(v) = role.repetition_penalty {
        m.insert("repetition_penalty".into(), serde_json::json!(v));
    }
    if let Some(stop) = &role.stop {
        let cleaned: Vec<&String> = stop.iter().filter(|s| !s.trim().is_empty()).collect();
        if !cleaned.is_empty() {
            m.insert("stop".into(), serde_json::json!(cleaned));
        }
    }
    if let Some(effort) = role.reasoning_effort.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        m.insert("reasoning_effort".into(), serde_json::json!(effort));
    }
    serde_json::Value::Object(m)
}

/// 角色默认：供应商 id + 模型 + 采样参数。
fn role_defaults(cfg: &AppConfig, role: &RoleConfig) -> Result<RigRoleParams, String> {
    let provider_id = cfg
        .providers
        .iter()
        .find(|p| p.id == role.provider_id)
        .map(|p| p.id.clone())
        .or_else(|| cfg.providers.first().map(|p| p.id.clone()))
        .ok_or_else(|| "未配置任何供应商".to_string())?;
    Ok(RigRoleParams {
        provider_id,
        model: role.model.clone(),
        temperature: role.temperature.unwrap_or(0.8).clamp(0.0, 2.0),
        max_tokens: role.max_tokens.unwrap_or(4096) as u64,
        sampling: sampling_params(role),
    })
}

fn build_rig(cfg: &AppConfig) -> Result<RigProvider, String> {
    let providers = provider_entries(cfg);
    if providers.is_empty() {
        return Err("没有可用供应商（缺 Base URL / API Key）".to_string());
    }
    let story = role_defaults(cfg, &cfg.roles.story)?;
    let character = role_defaults(cfg, &cfg.roles.character)?;
    tracing::info!(
        providers = providers.len(),
        story_model = %story.model,
        character_model = %character.model,
        "启用 rig AiProvider（OpenAI 兼容；支持按存档覆盖模型）"
    );
    RigProvider::new(RigParams { providers, story, character })
}

/// 后端模式：环境变量优先于 config.json（部署侧可覆盖）；空串视为未设。
fn resolve_mode(env_key: &str, configured: &str) -> String {
    std::env::var(env_key)
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| configured.trim().to_ascii_lowercase())
}

/// 组合根调用：按配置装配 AiProvider（rig 优先，失败回退脚本）。
pub fn build_ai_provider(cfg: &AppConfig) -> Arc<dyn AiProvider> {
    let mode = resolve_mode("OCTOPUS_AI", &cfg.ai.provider);
    if mode == "scripted" {
        tracing::info!("AI 后端 = scripted：使用确定性 ScriptedProvider");
        return Arc::new(ScriptedProvider);
    }
    match build_rig(cfg) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            tracing::warn!("rig AiProvider 不可用（{e}），回退 ScriptedProvider");
            Arc::new(ScriptedProvider)
        }
    }
}

/// 模型名里出现这些片段，才认为它可能是 embedding 模型。
/// 用「白名单」而非「黑名单」：embedding 模型命名很有辨识度，对话模型则千奇百怪。
const EMBEDDING_MODEL_HINTS: &[&str] = &[
    "embed", "bge", "gte", "jina", "nomic", "mxbai", "minilm", "e5", "voyage",
    "text-similarity", "paraphrase",
];

/// 该模型名是否像 embedding 模型（不区分大小写）。
pub fn looks_like_embedding_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    EMBEDDING_MODEL_HINTS.iter().any(|h| m.contains(h))
}

fn build_embedding(cfg: &AppConfig) -> Result<Arc<dyn EmbeddingBackend>, String> {
    let role = &cfg.roles.embedding;
    let p = cfg
        .providers
        .iter()
        .find(|p| p.id == role.provider_id)
        .ok_or_else(|| format!("未找到 embedding 供应商「{}」", role.provider_id))?;
    let base_url = p.base_url.as_deref().unwrap_or("").trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(format!("供应商「{}」未配置 Base URL", p.label));
    }
    if !looks_like_embedding_model(&role.model) {
        return Err(format!(
            "embedding role 指向的模型「{}」（供应商「{}」）不是 embedding 模型",
            role.model, p.label
        ));
    }
    let key = p.api_key.as_deref().unwrap_or("").trim().to_string();
    let client = openai::CompletionsClient::builder()
        .api_key(if key.is_empty() { "not-needed".to_string() } else { key })
        .base_url(base_url.clone())
        .build()
        .map_err(|e| format!("构建 rig embedding 客户端失败：{e}"))?;
    tracing::info!(
        provider = %p.label,
        base = %base_url,
        model = %role.model,
        "EmbeddingBackend 使用 rig（OpenAI 兼容 /embeddings；该端点需支持 embeddings）"
    );
    let model = client.embedding_model(role.model.clone());
    Ok(Arc::new(RigEmbedding::new(model)))
}

/// 组合根调用：按配置装配 EmbeddingBackend（rig 优先，失败回退 Stub）。
pub fn build_embedding_backend(cfg: &AppConfig) -> Arc<dyn EmbeddingBackend> {
    let mode = resolve_mode("OCTOPUS_EMBEDDING", &cfg.ai.embedding);
    if mode == "stub" {
        tracing::info!("Embedding 后端 = stub：使用 StubEmbedding");
        return Arc::new(StubEmbedding::default());
    }
    match build_embedding(cfg) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("rig EmbeddingBackend 不可用（{e}），回退 StubEmbedding");
            Arc::new(StubEmbedding::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_embedding_model;

    #[test]
    fn recognizes_embedding_models() {
        for m in [
            "text-embedding-3-small",
            "text-embedding-3-large",
            "bge-small-zh-v1.5",
            "bge-m3",
            "nomic-embed-text",
            "embed-english-v3.0",
            "all-MiniLM-L6-v2",
            "voyage-3",
            "gte-large",
            "jina-embeddings-v3",
        ] {
            assert!(looks_like_embedding_model(m), "{m} 应识别为 embedding 模型");
        }
    }

    #[test]
    fn rejects_chat_models() {
        for m in [
            "deepseek-chat",
            "deepseek-reasoner",
            "deepseek-v4-pro",
            "gpt-4o",
            "claude-sonnet-4",
            "llama3.1:8b",
            "qwen2.5:7b",
        ] {
            assert!(!looks_like_embedding_model(m), "{m} 不应识别为 embedding 模型");
        }
    }

    #[test]
    fn sampling_params_only_includes_configured_fields() {
        use crate::config::RoleConfig;
        let mut role = RoleConfig {
            provider_id: "deepseek".into(),
            model: "deepseek-chat".into(),
            ..Default::default()
        };
        assert_eq!(
            super::sampling_params(&role),
            serde_json::json!({}),
            "未设置时为空对象，不改变供应商默认"
        );
        role.top_p = Some(0.9);
        role.frequency_penalty = Some(0.1);
        role.stop = Some(vec!["</story>".into(), "  ".into()]);
        let v = super::sampling_params(&role);
        assert_eq!(v["top_p"], 0.9);
        assert_eq!(v["frequency_penalty"], 0.1);
        assert_eq!(v["stop"], serde_json::json!(["</story>"]), "空白停止序列要被过滤");
        assert!(v.get("presence_penalty").is_none());
    }
}
