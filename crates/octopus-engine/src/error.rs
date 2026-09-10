use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("存档不存在: {0}")]
    SaveNotFound(String),
    #[error("故事书不存在: {0}")]
    StorybookNotFound(String),
    #[error("故事书未发布: {0}")]
    StorybookUnpublished(String),
    #[error("回合仍在进行中")]
    RoundInProgress,
    #[error("输入为空")]
    EmptyInput,
    #[error("冲突: {0}")]
    Conflict(String),
    #[error("草稿版本冲突，当前版本为 {current_draft_version}")]
    DraftConflict {
        current_draft_version: u32,
        updated_at: String,
    },
    #[error("AI 调用失败: {0}")]
    Ai(String),
    #[error("存储错误: {0}")]
    Storage(String),
    #[error("内部错误: {0}")]
    Internal(String),
}


impl From<sea_orm::DbErr> for EngineError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(format!("json: {e}"))
    }
}
