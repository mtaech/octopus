//! Embedding 实现（#15）：本地 fastembed + rig provider（OpenAI 兼容 /embeddings）+ 确定性 Stub 回退。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use octopus_engine::{EmbeddingBackend, EngineError};

/// 确定性伪向量：同一文本同一结果，便于测试与离线。
pub struct StubEmbedding {
    dim: usize,
}

impl Default for StubEmbedding {
    fn default() -> Self {
        Self { dim: 512 }
    }
}

impl StubEmbedding {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait]
impl EmbeddingBackend for StubEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError> {
        let mut v = vec![0f32; self.dim];
        for (i, b) in text.bytes().enumerate() {
            v[i % self.dim] += (b as f32) / 255.0;
        }
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        for x in &mut v {
            *x /= norm;
        }
        Ok(v)
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn backend_name(&self) -> &'static str {
        "stub"
    }

    fn fingerprint(&self) -> String {
        format!("stub:{}", self.dim)
    }
}

/// 本地 embedding 后端（fastembed + ONNX Runtime）：完全离线，无需 base_url / api_key。
///
/// 权重在**首次** `embed` 时才下载（bge-small-zh-v1.5 约 100MB），所以构建后端本身是廉价的；
/// `TextEmbedding::try_new` 是同步阻塞调用，必须放进 `spawn_blocking`，否则会卡住 async 运行时。
pub struct FastEmbedBackend {
    /// 配置里的模型 id，用于日志与错误信息。
    model_id: String,
    /// 解析后的 fastembed 模型。
    model: EmbeddingModel,
    /// 与模型一一对应的真实维度（不加载权重也要能回答 `dimension()`）。
    dim: usize,
    /// 权重缓存目录覆盖（OCTOPUS_EMBEDDING_CACHE）；None 表示用 fastembed 默认目录。
    cache_dir: Option<PathBuf>,
    /// 惰性、至多成功初始化一次；用 Arc 是为了把句柄 move 进 spawn_blocking。
    text: tokio::sync::OnceCell<Arc<TextEmbedding>>,
}

impl FastEmbedBackend {
    /// 按模型 id 构建后端（不加载权重；真正加载推迟到首次 `embed`）。
    pub fn new(model_id: &str) -> Result<Self, EngineError> {
        let (model, dim) = resolve_model(model_id)?;
        Ok(Self {
            model_id: model_id.trim().to_string(),
            model,
            dim,
            cache_dir: cache_dir_override(),
            text: tokio::sync::OnceCell::new(),
        })
    }

    /// 本机使用的模型 id。
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// 确保权重已加载（至多一次），返回可移入 `spawn_blocking` 的共享句柄。
    async fn loaded(&self) -> Result<Arc<TextEmbedding>, EngineError> {
        self.text
            .get_or_try_init(|| async {
                let model = self.model.clone();
                let model_id = self.model_id.clone();
                let cache_dir = self.cache_dir.clone();
                // 下载 / 加载权重是同步阻塞 IO，必须离开 async worker 线程。
                tokio::task::spawn_blocking(move || {
                    tracing::info!(model = %model_id, "加载本地 embedding 模型（首次使用会下载权重）");
                    let mut opts = InitOptions::new(model);
                    if let Some(dir) = cache_dir {
                        opts = opts.with_cache_dir(dir);
                    }
                    let text = TextEmbedding::try_new(opts).map_err(|e| {
                        EngineError::Ai(format!(
                            "加载本地 embedding 模型「{model_id}」失败（首次使用需下载权重，请检查网络与缓存目录）：{e}"
                        ))
                    })?;
                    tracing::info!(model = %model_id, "本地 embedding 模型加载完成");
                    Ok::<_, EngineError>(Arc::new(text))
                })
                .await
                .map_err(|e| EngineError::Ai(format!("加载本地 embedding 模型的后台任务失败：{e}")))?
            })
            .await
            .cloned()
    }
}

#[async_trait]
impl EmbeddingBackend for FastEmbedBackend {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError> {
        let text_embedding = self.loaded().await?;
        let input = text.to_string();
        // 推理同样是 CPU 密集的同步调用，同样放进 spawn_blocking。
        tokio::task::spawn_blocking(move || {
            let mut out = text_embedding
                .embed(vec![input], None)
                .map_err(|e| EngineError::Ai(format!("本地 embedding 计算失败：{e}")))?;
            out.pop()
                .ok_or_else(|| EngineError::Ai("本地 embedding 未返回任何向量".to_string()))
        })
        .await
        .map_err(|e| EngineError::Ai(format!("本地 embedding 后台任务失败：{e}")))?
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn backend_name(&self) -> &'static str {
        "fastembed"
    }

    fn fingerprint(&self) -> String {
        format!("fastembed:{}:{}", self.model_id, self.dim)
    }
}

/// 模型 id → fastembed 模型 + 真实维度。
///
/// 维度在这里显式声明，这样 `dimension()` 无需加载权重即可回答；取值来自 fastembed
/// 的模型清单（BGESmallZHV15 = 512）。
///
/// `bge-m3` 返回错误而不是 `BGEM3`：Cargo.lock 是 feature 无关的，rig 0.42 的可选
/// `rig-fastembed` 总会把 fastembed 4.x（`ort =2.0.0-rc.9`）解析进锁文件，而带
/// `BGEM3` 的 fastembed 5.x 需要 `ort` rc.10+；两者都是 exact pin，无法共存。
/// 要启用 bge-m3，需把 rig 升级到使用 fastembed 5.x 的版本，或改用 `rig-core`
/// 绕开 `rig` 伞 crate 对 rig-fastembed 的可选依赖。
fn resolve_model(model_id: &str) -> Result<(EmbeddingModel, usize), EngineError> {
    match model_id.trim().to_ascii_lowercase().as_str() {
        "bge-small-zh-v1.5" => Ok((EmbeddingModel::BGESmallZHV15, 512)),
        "bge-m3" => Err(EngineError::Ai(
            "本地 embedding 模型「bge-m3」暂不可用：当前 fastembed 4.9.1 没有 BGEM3 变体，             且其 ort 版本与 rig 0.42 的 rig-fastembed 锁死。请改用 bge-small-zh-v1.5。"
                .to_string(),
        )),
        other => Err(EngineError::Ai(format!(
            "不支持的本地 embedding 模型「{other}」；当前支持：bge-small-zh-v1.5"
        ))),
    }
}

/// OCTOPUS_EMBEDDING_CACHE 覆盖 fastembed 默认缓存目录；未设或空串则用 fastembed 默认。
fn cache_dir_override() -> Option<PathBuf> {
    std::env::var("OCTOPUS_EMBEDDING_CACHE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// rig EmbeddingModel 适配：任意实现 rig `EmbeddingModel` 的 provider 都能接入。
pub struct RigEmbedding<M> {
    model: M,
    dim: usize,
    /// 配置里的模型 id：进指纹，换模型（哪怕维度相同）也能触发向量库重建。
    model_id: String,
}

impl<M> RigEmbedding<M>
where
    M: rig::embeddings::EmbeddingModel,
{
    /// 维度由 rig 模型的 `ndims()` 决定。
    pub fn new(model: M) -> Self {
        let dim = model.ndims();
        Self { model, dim, model_id: "unknown".into() }
    }

    /// 带模型 id 的构造：id 进指纹，用于判断「向量库里的向量是不是这个模型算的」。
    pub fn with_model_id(model: M, model_id: String) -> Self {
        let dim = model.ndims();
        Self { model, dim, model_id }
    }
}

#[async_trait]
impl<M> EmbeddingBackend for RigEmbedding<M>
where
    M: rig::embeddings::EmbeddingModel + Send + Sync,
{
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError> {
        let e = self
            .model
            .embed_text(text)
            .await
            .map_err(|err| EngineError::Ai(format!("embedding 调用失败：{err}")))?;
        Ok(e.vec.into_iter().map(|x| x as f32).collect())
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn backend_name(&self) -> &'static str {
        "rig"
    }

    fn fingerprint(&self) -> String {
        format!("rig:{}:{}", self.model_id, self.dim)
    }
}

#[cfg(test)]
mod tests {
    use super::{FastEmbedBackend, resolve_model};
    use fastembed::EmbeddingModel;
    use octopus_engine::EmbeddingBackend;

    #[test]
    fn maps_supported_model_ids_to_fastembed_models() {
        assert_eq!(
            resolve_model("bge-small-zh-v1.5").unwrap(),
            (EmbeddingModel::BGESmallZHV15, 512)
        );
        // id 允许前后空白与大小写差异（配置由用户/前端写入）。
        assert_eq!(
            resolve_model(" BGE-Small-ZH-v1.5 ").unwrap().0,
            EmbeddingModel::BGESmallZHV15
        );
    }

    #[test]
    fn rejects_bge_m3_until_fastembed_is_upgraded() {
        // 回归护栏：若将来 fastembed 升到 5.x 并拿回 BGEM3，这个测试会失败，提示更新映射。
        let err = resolve_model("bge-m3").unwrap_err().to_string();
        assert!(err.contains("bge-m3"), "{err}");
        assert!(err.contains("bge-small-zh-v1.5"), "{err}");
    }

    #[test]
    fn rejects_unknown_model_id_and_names_supported_ones() {
        let err = resolve_model("text-embedding-3-small")
            .unwrap_err()
            .to_string();
        assert!(err.contains("text-embedding-3-small"), "{err}");
        assert!(err.contains("bge-small-zh-v1.5"), "{err}");
    }

    #[test]
    fn builds_backend_without_downloading_weights() {
        // 构造只做 id 映射，不触碰网络；维度立即可用。
        let backend = FastEmbedBackend::new("bge-small-zh-v1.5").unwrap();
        assert_eq!(backend.dimension(), 512);
        assert_eq!(backend.model_id(), "bge-small-zh-v1.5");
        assert!(FastEmbedBackend::new("no-such-model").is_err());
    }

    #[tokio::test]
    #[ignore = "首次运行会下载 ~100MB 权重；手动运行：cargo test -p octopus-ai -- --ignored fastembed_real_embed"]
    async fn fastembed_real_embed() {
        let backend = FastEmbedBackend::new("bge-small-zh-v1.5").expect("模型 id 应受支持");
        let v = backend
            .embed("你好，世界")
            .await
            .expect("本地 embedding 应成功");
        assert_eq!(v.len(), 512, "bge-small-zh-v1.5 应为 512 维");
        assert!(v.iter().all(|x| x.is_finite()), "向量应全为有限值");
        assert!(v.iter().any(|x| x.abs() > 1e-6), "向量不应全为零");
    }
}
