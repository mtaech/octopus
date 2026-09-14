//! 管理后台（只有 `is_admin` 账户可达）：概览 + 用户管理。
//!
//! 装配方式见 `router`：`/api/admin/*` 是一棵**独立子树**，整棵子树先过 `require_auth`
//! （受保护集合的公共层）再过 `guard_admin`。所以「漏写守卫」不会发生——新增管理端点
//! 只要挂进这棵子树就自动要求管理员。
//!
//! 三条兜底规则（本地实例没有「找回管理员」的通道，必须由规则兜住）：
//! 1. 不能删除自己；
//! 2. 不能删除 / 降级最后一个管理员；
//! 3. 删账户默认**拒绝**（409 + 内容统计），必须显式 `?purge=true` 才连带删除其内容。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;

use octopus_engine::{
    resolve_display_name, validate_password, validate_username,
};
use octopus_types::{
    Account, AdminCreateUserRequest, AdminOverview, AdminResetPasswordRequest,
    AdminUpdateUserRequest, AdminUserRow,
};

use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::ApiError;

/// 本 crate 版本号（工作区统一版本）。
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn user_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "user_not_found", "账户不存在")
}

fn conflict(code: &str, message: &str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, code, message)
}

/// 目标账户；不存在 → 404。
async fn target_account(app: &AppState, id: &str) -> Result<Account, ApiError> {
    app.store()
        .account_by_id(id)
        .await?
        .ok_or_else(user_not_found)
}

/// 取单个账户的管理行（创建 / 更新后回填响应用，保证前端拿到一致形状）。
async fn row_of(app: &AppState, self_id: &str, id: &str) -> Result<AdminUserRow, ApiError> {
    app.store()
        .admin_user_rows(self_id)
        .await?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(user_not_found)
}

/// 概览：账户 / 存档 / 故事书 / 在线登录态 + 库文件规模。
pub async fn overview(State(app): State<Arc<AppState>>) -> Result<Json<AdminOverview>, ApiError> {
    let c = app.store().admin_counts().await?;
    Ok(Json(AdminOverview {
        users: c.users,
        admins: c.admins,
        saves: c.saves,
        storybooks: c.storybooks,
        active_sessions: c.active_sessions,
        db_path: app.store().path().to_string(),
        db_bytes: app.store().db_bytes(),
        version: VERSION.to_string(),
    }))
}

/// 账户列表（含每人内容统计与「是不是你自己」）。
pub async fn list_users(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<AdminUserRow>>, ApiError> {
    Ok(Json(app.store().admin_user_rows(user.id()).await?))
}

/// 新建账户：管理员可以直接指定口令与是否管理员（本地实例没有邀请邮件这条链路）。
pub async fn create_user(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<AdminCreateUserRequest>,
) -> Result<(StatusCode, Json<AdminUserRow>), ApiError> {
    let username = validate_username(&req.username)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid_username", m))?;
    validate_password(&req.password)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid_password", m))?;
    let display_name = resolve_display_name(req.display_name.as_deref(), &username);
    let created = app
        .store()
        .create_account(&username, &display_name, &req.password)
        .await?;
    if req.is_admin {
        app.store().set_admin(&created.id, true).await?;
    }
    tracing::info!(admin = %user.0.username, created = %created.username, is_admin = req.is_admin, "管理后台：新建账户");
    let row = row_of(&app, user.id(), &created.id).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// 改显示名 / 提权降权。降级最后一个管理员会被拒（否则这台实例就没人能进后台了）。
pub async fn update_user(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(req): Json<AdminUpdateUserRequest>,
) -> Result<Json<AdminUserRow>, ApiError> {
    let target = target_account(&app, &id).await?;

    // 省略字段 = 不改；显式给了（哪怕是空串）= 按它设置，空串回落为用户名。
    if let Some(raw) = req.display_name.as_deref() {
        let name = resolve_display_name(Some(raw), &target.username);
        app.store().set_display_name(&id, &name).await?;
    }

    if let Some(want_admin) = req.is_admin {
        if want_admin != target.is_admin {
            if !want_admin {
                let admins = app.store().count_admins().await?;
                if admins <= 1 {
                    return Err(conflict(
                        "last_admin",
                        "这是最后一个管理员，不能取消其管理员身份",
                    ));
                }
            }
            app.store().set_admin(&id, want_admin).await?;
            tracing::info!(
                admin = %user.0.username,
                target = %target.username,
                is_admin = want_admin,
                "管理后台：变更管理员身份"
            );
        }
    }

    Ok(Json(row_of(&app, user.id(), &id).await?))
}

/// 重置口令：管理员直接设新口令，并**吊销该账户的全部登录态**（强制重新登录）。
pub async fn reset_password(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(req): Json<AdminResetPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let target = target_account(&app, &id).await?;
    validate_password(&req.new_password)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid_password", m))?;
    app.store().set_password(&id, &req.new_password).await?;
    let revoked = app.store().delete_user_sessions(&id, None).await?;
    tracing::warn!(
        admin = %user.0.username,
        target = %target.username,
        revoked,
        "管理后台：已重置口令并吊销该账户全部登录态"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// 退出该账户的所有设备（不改口令）。
pub async fn revoke_sessions(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let target = target_account(&app, &id).await?;
    let revoked = app.store().delete_user_sessions(&id, None).await?;
    tracing::info!(admin = %user.0.username, target = %target.username, revoked, "管理后台：已退出该账户的全部设备");
    Ok(Json(json!({ "revoked": revoked })))
}

#[derive(Debug, Deserialize, Default)]
pub struct DeleteUserQuery {
    /// 连同该账户的存档与故事书一起删除。缺省是拒绝——删数据必须是显式动作。
    #[serde(default)]
    purge: bool,
}

/// 删除账户。规则见模块头注释；带 `?purge=true` 时先清内容再删账户。
pub async fn delete_user(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Query(q): Query<DeleteUserQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let target = target_account(&app, &id).await?;
    if target.id == user.id() {
        return Err(conflict("cannot_delete_self", "不能删除当前登录的账户"));
    }
    if target.is_admin && app.store().count_admins().await? <= 1 {
        return Err(conflict("last_admin", "这是最后一个管理员，不能删除"));
    }

    let save_ids = app.store().save_ids_for(&id).await?;
    let book_ids = app.store().storybook_ids_for(&id).await?;
    if (!save_ids.is_empty() || !book_ids.is_empty()) && !q.purge {
        // 内容不为空时默认拒绝：让「删账户」与「删数据」分成两个明确的决定。
        return Err(conflict(
            "user_has_data",
            &format!(
                "该账户有 {} 个存档、{} 本故事书；确认一并删除请带 purge=true",
                save_ids.len(),
                book_ids.len()
            ),
        )
        .with_detail(json!({
            "saves": save_ids.len(),
            "storybooks": book_ids.len(),
        })));
    }

    // 先删内容：走与用户侧同一条删除路径（含内存会话与模型会话的清理）。
    for save_id in &save_ids {
        app.store().delete_save(save_id).await?;
        app.drop_session(save_id).await;
    }
    for book_id in &book_ids {
        app.store().delete_storybook(book_id).await?;
    }
    app.store().delete_account(&id).await?;
    tracing::warn!(
        admin = %user.0.username,
        target = %target.username,
        saves = save_ids.len(),
        storybooks = book_ids.len(),
        "管理后台：已删除账户"
    );
    Ok(Json(json!({
        "saves_deleted": save_ids.len(),
        "storybooks_deleted": book_ids.len(),
    })))
}
