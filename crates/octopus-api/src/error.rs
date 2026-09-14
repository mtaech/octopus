use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use octopus_engine::EngineError;
use octopus_types::ApiErrorBody;

pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub detail: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn not_implemented(what: &str) -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "not_implemented",
            format!("{what} 待实现"),
        )
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::SaveNotFound(_) => {
                Self::new(StatusCode::NOT_FOUND, "save_not_found", e.to_string())
            }
            EngineError::StorybookNotFound(_) => {
                Self::new(StatusCode::NOT_FOUND, "storybook_not_found", e.to_string())
            }
            EngineError::StorybookUnpublished(_) => {
                Self::new(StatusCode::CONFLICT, "storybook_unpublished", e.to_string())
            }
            EngineError::RoundInProgress => {
                Self::new(StatusCode::CONFLICT, "round_in_progress", e.to_string())
            }
            EngineError::Cancelled => Self::new(StatusCode::CONFLICT, "cancelled", e.to_string()),
            EngineError::EmptyInput => {
                Self::new(StatusCode::BAD_REQUEST, "empty_input", e.to_string())
            }
            EngineError::InvalidAsset(_) => {
                Self::new(StatusCode::BAD_REQUEST, "invalid_asset", e.to_string())
            }
            EngineError::DraftConflict {
                current_draft_version,
                updated_at,
            } => Self::new(
                StatusCode::CONFLICT,
                "draft_conflict",
                format!("草稿版本冲突，当前版本为 {current_draft_version}"),
            )
            .with_detail(serde_json::json!({
                "current_draft_version": current_draft_version,
                "updated_at": updated_at,
            })),
            EngineError::Conflict(ref c) if c == "username_taken" => Self::new(
                StatusCode::CONFLICT,
                "username_taken",
                "该用户名已被占用",
            ),
            EngineError::Conflict(ref c) if c == "expired" => {
                Self::new(StatusCode::CONFLICT, "expired", "该确认已超时过期")
            }
            EngineError::Conflict(ref c) if c == "not_controllable" => Self::new(
                StatusCode::BAD_REQUEST,
                "not_controllable",
                "该角色不可被玩家控制",
            ),
            EngineError::Conflict(ref c) if c == "character_not_found" => {
                Self::new(StatusCode::NOT_FOUND, "character_not_found", "角色不存在")
            }
            EngineError::Conflict(_) => Self::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            EngineError::Ai(_)
            | EngineError::Storage(_)
            | EngineError::Internal(_)
            | EngineError::Lua(_) => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            code: self.code,
            message: self.message,
            detail: self.detail,
        };
        (self.status, Json(body)).into_response()
    }
}
