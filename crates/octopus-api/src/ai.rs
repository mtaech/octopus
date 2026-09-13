//! AI provider 装配（#20 组合根）：把 config.json 的 providers / roles 映射成 rig provider。
//!
//! 默认在配置了可用 API Key / 本地端点时启用 rig；否则回退 ScriptedProvider。
//! 环境变量 OCTOPUS_AI=scripted 可强制使用确定性实现（离线 / 测试）。

use std::sync::Arc;

use octopus_ai::{
    CompactionParams, RigParams, RigProvider, RigProviderParams, RigRoleParams, ScriptedProvider,
    StoryPrompts,
};
use octopus_engine::AiProvider;

use crate::config::{AppConfig, RoleConfig};

/// 把 config.json 的 providers 映射成「可用供应商」清单（缺 Base URL / Key 的跳过）。
fn provider_entries(cfg: &AppConfig) -> Vec<RigProviderParams> {
    let mut out = Vec::new();
    for p in &cfg.providers {
        let base_url = p
            .base_url
            .as_deref()
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string();
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
            api_key: if key.is_empty() {
                "not-needed".to_string()
            } else {
                key
            },
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
    if let Some(effort) = role
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
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

/// 每个模型声明的上下文窗口（`providers[].models[].ctx`）→ `"{provider}/{model}"`。
///
/// 自动压缩的阈值 / 保留预算都以它为基数；目录快照（前端 TS）里的 ctx 只有落盘到
/// config.json 才看得见，所以后端只认这里。没声明的模型不做压力压缩（只留溢出兜底）。
fn model_context_windows(cfg: &AppConfig) -> std::collections::HashMap<String, u64> {
    let mut out = std::collections::HashMap::new();
    for p in &cfg.providers {
        for m in &p.models {
            if let Some(ctx) = m.ctx.filter(|c| *c > 0) {
                out.insert(format!("{}/{}", p.id, m.id), ctx as u64);
            }
        }
    }
    out
}

/// 配置里的压缩比例 → rig 参数（保守钳制：配置写歪了也不至于每回合都压）。
fn compaction_params(c: &crate::config::CompactionConfig) -> CompactionParams {
    let threshold = c.threshold_ratio.clamp(0.1, 1.0);
    CompactionParams {
        enabled: c.enabled,
        threshold_ratio: threshold,
        // 保留比例必须低于阈值，否则压完仍然超阈、白压一次。
        retain_ratio: c
            .retain_ratio
            .clamp(0.0, 0.9)
            .min((threshold - 0.05).max(0.0)),
        max_tokens: c.max_tokens.clamp(512, 200_000),
    }
}

fn build_rig(cfg: &AppConfig) -> Result<RigProvider, String> {
    let providers = provider_entries(cfg);
    if providers.is_empty() {
        return Err("没有可用供应商（缺 Base URL / API Key）".to_string());
    }
    let story = role_defaults(cfg, &cfg.roles.story)?;
    // 摘要压缩用 pair 角色（最便宜）；旧配置没配 pair 时回落到 story，保证仍能启动。
    let pair = cfg
        .roles
        .pair
        .as_ref()
        .map(|role| role_defaults(cfg, role))
        .transpose()?
        .unwrap_or_else(|| story.clone());
    let model_ctx = model_context_windows(cfg);
    let compaction = compaction_params(&cfg.ai.compaction);
    tracing::info!(
        providers = providers.len(),
        story_model = %story.model,
        pair_model = %pair.model,
        windows = model_ctx.len(),
        compaction_enabled = compaction.enabled,
        threshold_ratio = compaction.threshold_ratio,
        retain_ratio = compaction.retain_ratio,
        "启用 rig AiProvider（OpenAI 兼容；支持按存档覆盖模型）"
    );
    // 提示词：内置默认 + 配置里的非空覆盖（空 = 用默认）。
    let mut prompts = StoryPrompts::default();
    prompts.apply_overrides(&cfg.prompts);
    RigProvider::new(RigParams {
        providers,
        story,
        pair,
        prompts,
        model_ctx,
        compaction,
    })
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

/// 后端模式：环境变量优先于 config.json（部署侧可覆盖）；空串视为未设。
fn resolve_mode(env_key: &str, configured: &str) -> String {
    std::env::var(env_key)
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| configured.trim().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::sampling_params;

    #[test]
    fn sampling_params_only_includes_configured_fields() {
        use crate::config::RoleConfig;
        let mut role = RoleConfig {
            provider_id: "deepseek".into(),
            model: "deepseek-chat".into(),
            ..Default::default()
        };
        assert_eq!(
            sampling_params(&role),
            serde_json::json!({}),
            "未设置时为空对象，不改变供应商默认"
        );
        role.top_p = Some(0.9);
        role.frequency_penalty = Some(0.1);
        role.stop = Some(vec!["</story>".into(), "  ".into()]);
        let v = sampling_params(&role);
        assert_eq!(v["top_p"], 0.9);
        assert_eq!(v["frequency_penalty"], 0.1);
        assert_eq!(
            v["stop"],
            serde_json::json!(["</story>"]),
            "空白停止序列要被过滤"
        );
        assert!(v.get("presence_penalty").is_none());
    }
}
