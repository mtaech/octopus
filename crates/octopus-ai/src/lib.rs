//! octopus-ai：AiProvider / EmbeddingBackend 实现（#10/#20）。
//!
//! 里程碑 1 先落「脚本化 Provider」把端到端链路跑通；rig `CompletionModel`
//! 适配与 fastembed 本地 embedding 随后接入（见 #10/#15）。

pub mod embedding;
pub mod scripted;

pub use embedding::StubEmbedding;
pub use scripted::ScriptedProvider;
