//! octopus-ai：AiProvider 实现（#10/#20）。
//!
//! - `RigProvider`：基于 rig 框架的真实 LLM 实现（OpenAI 兼容 Chat Completions + Agent）。
//! - `ScriptedProvider`：确定性、零依赖，供冒烟与确定性重放测试（#20 ④）。

pub mod prompt;
pub mod rig_provider;
pub mod scripted;

pub use prompt::{StoryBlockPrompts, StoryPrompts, render, render_block};
pub use rig_provider::{
    CompactionParams, RigParams, RigProvider, RigProviderParams, RigRoleParams, is_context_overflow,
};
pub use scripted::ScriptedProvider;
