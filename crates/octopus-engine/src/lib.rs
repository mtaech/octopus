//! octopus-engine：状态权威 / 命令管线 / 结算 / 事件发射 / 存储（#06/#17/#20/#27）。
//!
//! 依赖纪律（#20 ②）：`state`、`rng` 为叶子；`command`/`session` 为编排；
//! `storage` 为适配层；外部 IO 与 LLM 一律走 `ports`。

pub mod entities;
pub mod error;
pub mod ports;
pub mod rng;
pub mod seed;
pub mod session;
pub mod state;
pub mod storage;
pub mod validate;

pub use error::EngineError;
pub use ports::{AiProvider, EmbeddingBackend, EventSink, TurnContext};
pub use rng::DeterministicRng;
pub use session::Session;
pub use state::WorldState;
pub use storage::{SqliteStore, StorybookRow};
pub use validate::{validate_storybook, validate_storybook_result};
