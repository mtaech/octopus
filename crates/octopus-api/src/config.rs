//! 应用配置管理（#26）：持久化存储 providers / roles / 预算到本地 config.json（0600 权限）。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 用户自定义的模型元数据（小中转站等目录未收录的模型）：覆盖目录快照。
    /// 上下文窗口（tokens）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<u32>,
    /// 最大输出（tokens）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_out: Option<u32>,
    /// 是否支持思考。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// 思考等级 → 实际下发值（null = 不发送该参数）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tl: Option<std::collections::BTreeMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub label: String,
    pub kind: String, // openai | anthropic | ollama | openai-compatible
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoleConfig {
    pub provider_id: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 核取样：只保留累计概率前 P 的候选词（OpenAI 兼容 `top_p`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// 存在惩罚：抑制已出现过的词（`presence_penalty`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    /// 频率惩罚：按出现次数抑制高频词（`frequency_penalty`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    /// 重复惩罚：部分供应商支持（`repetition_penalty`），不支持时会报错，默认不发。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f64>,
    /// 停止序列：命中即停。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// 命名预设档（创意写作 / 日常 RP / 严肃对话 / NPC 模式）：只作记录与 UI 回显。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// 思考强度（reasoning_effort）：low / medium / high 等；缺省用供应商默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRoleConfig {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolesConfig {
    pub story: RoleConfig,
    pub character: RoleConfig,
    #[serde(default)]
    pub pair: Option<RoleConfig>,
    pub embedding: EmbeddingRoleConfig,
}

/// AI 后端开关（#26 扩展）：story/character 与 embedding 各自可选。
/// 取值由 ai.rs 解析；环境变量 OCTOPUS_AI / OCTOPUS_EMBEDDING 优先于这里。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiBackendConfig {
    /// story / character 后端：auto | rig | scripted
    #[serde(default = "default_backend_mode")]
    pub provider: String,
    /// embedding 后端：auto | rig | stub
    #[serde(default = "default_backend_mode")]
    pub embedding: String,
}

fn default_backend_mode() -> String {
    "auto".to_string()
}

impl Default for AiBackendConfig {
    fn default() -> Self {
        Self { provider: default_backend_mode(), embedding: default_backend_mode() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub providers: Vec<ProviderConfig>,
    pub roles: RolesConfig,
    #[serde(default)]
    pub turn_token_budget: Option<u32>,
    /// AI 后端开关：provider = auto|rig|scripted；embedding = auto|rig|stub
    #[serde(default)]
    pub ai: AiBackendConfig,
}

pub fn default_config() -> AppConfig {
    AppConfig {
        providers: vec![
            ProviderConfig {
                id: "deepseek".into(),
                label: "DeepSeek".into(),
                kind: "openai-compatible".into(),
                base_url: Some("https://api.deepseek.com".into()),
                api_key: Some("".into()),
                models: vec![
                    ModelEntry { id: "deepseek-chat".into(), name: Some("DeepSeek V3 (Chat)".into()), ..Default::default() },
                    ModelEntry { id: "deepseek-reasoner".into(), name: Some("DeepSeek R1 (Reasoner)".into()), ..Default::default() },
                ],
            },
            ProviderConfig {
                id: "openai".into(),
                label: "OpenAI".into(),
                kind: "openai".into(),
                base_url: Some("https://api.openai.com/v1".into()),
                api_key: Some("".into()),
                models: vec![
                    ModelEntry { id: "gpt-4o".into(), name: Some("GPT-4o".into()), ..Default::default() },
                    ModelEntry { id: "gpt-4o-mini".into(), name: Some("GPT-4o mini".into()), ..Default::default() },
                ],
            },
            ProviderConfig {
                id: "ollama".into(),
                label: "本地 Ollama".into(),
                kind: "ollama".into(),
                base_url: Some("http://127.0.0.1:11434".into()),
                api_key: Some("".into()),
                models: vec![
                    ModelEntry { id: "qwen2.5:7b".into(), name: Some("Qwen2.5 7B".into()), ..Default::default() },
                    ModelEntry { id: "llama3.1:8b".into(), name: Some("Llama 3.1 8B".into()), ..Default::default() },
                ],
            },
            ProviderConfig {
                id: "fastembed".into(),
                label: "本地 Embedding".into(),
                kind: "openai-compatible".into(),
                base_url: None,
                api_key: Some("".into()),
                models: vec![
                    ModelEntry { id: "bge-small-zh-v1.5".into(), name: Some("BGE Small ZH v1.5".into()), ..Default::default() },
                    ModelEntry { id: "bge-m3".into(), name: Some("BGE M3".into()), ..Default::default() },
                ],
            },
        ],
        roles: RolesConfig {
            story: RoleConfig {
                provider_id: "deepseek".into(),
                model: "deepseek-chat".into(),
                temperature: Some(0.8),
                max_tokens: Some(4096),
                ..Default::default()
            },
            character: RoleConfig {
                provider_id: "deepseek".into(),
                model: "deepseek-chat".into(),
                temperature: Some(0.7),
                max_tokens: Some(2048),
                ..Default::default()
            },
            pair: Some(RoleConfig {
                provider_id: "deepseek".into(),
                model: "deepseek-chat".into(),
                temperature: Some(0.8),
                max_tokens: Some(4096),
                ..Default::default()
            }),
            embedding: EmbeddingRoleConfig {
                provider_id: "fastembed".into(),
                model: "bge-small-zh-v1.5".into(),
            },
        },
        turn_token_budget: Some(0),
        ai: AiBackendConfig::default(),
    }
}

pub fn get_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("OCTOPUS_CONFIG") {
        PathBuf::from(p)
    } else {
        PathBuf::from("config.json")
    }
}

pub fn load_config_from_disk() -> AppConfig {
    let path = get_config_path();
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
                return cfg;
            }
        }
    }
    default_config()
}

pub fn save_config_to_disk(cfg: &AppConfig) -> Result<(), std::io::Error> {
    let path = get_config_path();
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub async fn get_config() -> Json<AppConfig> {
    Json(load_config_from_disk())
}

pub async fn put_config(
    State(app): State<Arc<AppState>>,
    Json(req): Json<AppConfig>,
) -> Result<Json<AppConfig>, ApiError> {
    save_config_to_disk(&req).map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "config_save_failed",
            format!("保存配置文件失败：{e}"),
        )
    })?;
    // 配置即改即生效：重建 provider 换进共享槽，已打开的存档会话下一回合就走新模型。
    app.set_ai(crate::ai::build_ai_provider(&req));
    // 成本护栏同样即改即生效：热更新到所有已打开会话。
    app.set_token_budget(req.turn_token_budget.unwrap_or(0));
    Ok(Json(req))
}
