//! Embedding 占位实现（#15：默认本地 bge-small-zh-v1.5，512 维）。
//! 真实实现走 rig-fastembed；此处先保证端口形状与维度契约成立。

use octopus_engine::{EmbeddingBackend, EngineError};

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

impl EmbeddingBackend for StubEmbedding {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EngineError> {
        // 确定性伪向量：同一文本同一结果，便于测试对齐。
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
