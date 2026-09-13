//! 应用配置管理（#26）：持久化存储 providers / roles / 预算到本地 config.json（0600 权限）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;

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
pub struct RolesConfig {
    /// 单一 AI（旁白 / 世界响应 / 扮演所有 NPC）的默认模型与采样。
    pub story: RoleConfig,
    #[serde(default)]
    pub pair: Option<RoleConfig>,
}

/// 自动上下文压缩（派生 surface 重写；权威命令日志不动）。
///
/// 语义对齐 DSH `compaction-basic`：token 压力越过 `ctx × threshold_ratio` 时，把最旧的
/// 一段原文换成一份摘要，逐字保留最近 `ctx × retain_ratio`；供应商报上下文超限时再做一次
/// 更激进的兜底压缩并重试。`ctx` 取 `providers[].models[].ctx`（目录快照里的上下文窗口），
/// 没声明 ctx 的模型只走溢出兜底。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// false = 关掉压力压缩，只保留「供应商报超限」的兜底。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 越过 `ctx × threshold_ratio` 触发（默认 0.8）。
    #[serde(default = "default_threshold_ratio")]
    pub threshold_ratio: f64,
    /// 逐字保留的尾巴 = `ctx × retain_ratio`（默认 0.16）。
    #[serde(default = "default_retain_ratio")]
    pub retain_ratio: f64,
    /// 摘要请求的输出上限（token）。
    #[serde(default = "default_compaction_max_tokens")]
    pub max_tokens: u64,
}

fn default_true() -> bool {
    true
}

fn default_threshold_ratio() -> f64 {
    0.8
}

fn default_retain_ratio() -> f64 {
    0.16
}

fn default_compaction_max_tokens() -> u64 {
    8192
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            threshold_ratio: default_threshold_ratio(),
            retain_ratio: default_retain_ratio(),
            max_tokens: default_compaction_max_tokens(),
        }
    }
}

/// AI 后端开关（#26 扩展）。
/// 取值由 ai.rs 解析；环境变量 OCTOPUS_AI 优先于这里。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiBackendConfig {
    /// story / character 后端：auto | rig | scripted
    #[serde(default = "default_backend_mode")]
    pub provider: String,
    /// 自动上下文压缩。
    #[serde(default)]
    pub compaction: CompactionConfig,
}

fn default_backend_mode() -> String {
    "auto".to_string()
}

impl Default for AiBackendConfig {
    fn default() -> Self {
        Self {
            provider: default_backend_mode(),
            compaction: CompactionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub providers: Vec<ProviderConfig>,
    pub roles: RolesConfig,
    #[serde(default)]
    pub turn_token_budget: Option<u32>,
    /// AI 后端开关：provider = auto|rig|scripted
    #[serde(default)]
    pub ai: AiBackendConfig,
    /// 提示词覆盖表：key → 自定义文本（空 / 缺失 = 用内置默认）。
    ///
    /// key 见 `crate::prompts::keys`，目录见 GET /api/prompts。
    #[serde(default)]
    pub prompts: BTreeMap<String, String>,
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
                    ModelEntry {
                        id: "deepseek-chat".into(),
                        name: Some("DeepSeek V3 (Chat)".into()),
                        ..Default::default()
                    },
                    ModelEntry {
                        id: "deepseek-reasoner".into(),
                        name: Some("DeepSeek R1 (Reasoner)".into()),
                        ..Default::default()
                    },
                ],
            },
            ProviderConfig {
                id: "openai".into(),
                label: "OpenAI".into(),
                kind: "openai".into(),
                base_url: Some("https://api.openai.com/v1".into()),
                api_key: Some("".into()),
                models: vec![
                    ModelEntry {
                        id: "gpt-4o".into(),
                        name: Some("GPT-4o".into()),
                        ..Default::default()
                    },
                    ModelEntry {
                        id: "gpt-4o-mini".into(),
                        name: Some("GPT-4o mini".into()),
                        ..Default::default()
                    },
                ],
            },
            ProviderConfig {
                id: "ollama".into(),
                label: "本地 Ollama".into(),
                kind: "ollama".into(),
                base_url: Some("http://127.0.0.1:11434".into()),
                api_key: Some("".into()),
                models: vec![
                    ModelEntry {
                        id: "qwen2.5:7b".into(),
                        name: Some("Qwen2.5 7B".into()),
                        ..Default::default()
                    },
                    ModelEntry {
                        id: "llama3.1:8b".into(),
                        name: Some("Llama 3.1 8B".into()),
                        ..Default::default()
                    },
                ],
            },
        ],
        roles: RolesConfig {
            story: RoleConfig {
                provider_id: "deepseek".into(),
                model: "deepseek-chat".into(),
                temperature: Some(0.8),
                // 思考模型的 reasoning_tokens 与正文共享这份预算：4096 会把正文挤成 0 字。
                max_tokens: Some(65_536),
                ..Default::default()
            },
            pair: Some(RoleConfig {
                provider_id: "deepseek".into(),
                model: "deepseek-chat".into(),
                temperature: Some(0.8),
                max_tokens: Some(65_536),
                ..Default::default()
            }),
        },
        turn_token_budget: Some(0),
        ai: AiBackendConfig::default(),
        prompts: BTreeMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 模型条目的自定义元数据（小中转站）落盘 schema：与 AppConfig 其余字段一致是 snake_case。
    /// 字段名一改，前端 PUT 上来的 maxOut 会被 serde 当未知字段静默忽略——用户填了却存不下，
    /// 而 GET 回读也拿不到。这个测试把键名钉死。
    /// 提示词覆盖表落盘 / 回读：键名与 AppConfig 其余字段一致是 snake_case 的 prompts。
    #[test]
    fn app_config_round_trips_prompts() {
        let raw = r#"{
            "providers": [],
            "roles": { "story": { "provider_id": "p", "model": "m" } },
            "prompts": { "pair.role": "自定义结对角色", "story.turn.template": "T" }
        }"#;
        let cfg: AppConfig = serde_json::from_str(raw).expect("配置应能反序列化");
        assert_eq!(cfg.prompts.get("pair.role").map(String::as_str), Some("自定义结对角色"));
        let back = serde_json::to_value(&cfg).expect("配置应能序列化");
        assert_eq!(back["prompts"]["story.turn.template"], "T");
        // 旧配置没有 prompts 字段也不能失败
        let legacy: AppConfig = serde_json::from_str(
            r#"{ "providers": [], "roles": { "story": { "provider_id": "p", "model": "m" } } }"#,
        )
        .expect("缺少 prompts 的旧配置应能反序列化");
        assert!(legacy.prompts.is_empty());
        // 旧配置没有 ai.compaction：用 DSH 同款默认（0.8 / 0.16），且默认开启。
        assert!(legacy.ai.compaction.enabled);
        assert_eq!(legacy.ai.compaction.threshold_ratio, 0.8);
        assert_eq!(legacy.ai.compaction.retain_ratio, 0.16);
        assert_eq!(legacy.ai.compaction.max_tokens, 8192);
    }

    #[test]
    fn model_entry_round_trips_custom_meta() {
        let raw = r#"{
            "id": "deepseek-flash",
            "ctx": 131072,
            "max_out": 8192,
            "reasoning": true,
            "tl": { "high": "high" }
        }"#;
        let entry: ModelEntry = serde_json::from_str(raw).expect("模型条目应能反序列化");
        assert_eq!(entry.ctx, Some(131072));
        assert_eq!(entry.max_out, Some(8192));
        assert_eq!(entry.reasoning, Some(true));

        let back = serde_json::to_value(&entry).expect("模型条目应能序列化");
        assert_eq!(back["ctx"], serde_json::json!(131072));
        assert_eq!(back["max_out"], serde_json::json!(8192));
        assert!(
            back.get("maxOut").is_none(),
            "落盘键必须是 max_out，不能退回 camelCase 的 maxOut"
        );
    }
}
