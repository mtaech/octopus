//! Embedding 实现（#15）：rig provider（OpenAI 兼容 /embeddings）+ 确定性 Stub 回退。

use async_trait::async_trait;
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
}

/// rig EmbeddingModel 适配：任意实现 rig \`EmbeddingModel\` 的 provider 都能接入。
pub struct RigEmbedding<M> {
    model: M,
    dim: usize,
}

impl<M> RigEmbedding<M>
where
    M: rig::embeddings::EmbeddingModel,
{
    /// 维度由 rig 模型的 \`ndims()\` 决定。
    pub fn new(model: M) -> Self {
        let dim = model.ndims();
        Self { model, dim }
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
}
