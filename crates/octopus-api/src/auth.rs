//! 多账户鉴权：注册 / 登录 / 登出 / 当前用户 / 改口令，以及全部路由守卫。
//!
//! 守卫一律 **fail-closed**（漏配守卫 = 401，而不是放行）：
//! - `require_auth`：一切非公开路由。校验令牌 → 注入 `CurrentUser` / `CurrentToken`；
//! - `guard_admin`：管理员专属（全局配置里有 API Key）；
//! - `guard_save_owner`：存档归属。别人（或不存在的）存档一律 404——不用 403 区分
//!   「存在但不是你的」，否则状态码本身就成了 id 探测器；
//! - `guard_storybook_read`：作者本人 **或** 已发布（已发布的书是跨账户的公共可开档库）；
//! - `guard_storybook_edit` / `guard_pair_thread_edit`：只有作者能改草稿。
//!
//! 令牌传递：`Authorization: Bearer <token>`；GET 另外接受 `?token=`——浏览器
//! `EventSource` 无法自定义请求头，演出流（SSE）只能走查询串。令牌是 64 位十六进制，
//! 不需要百分号解码；日志层只记请求行，查询串不进日志。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use axum::Json;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use octopus_engine::{
    AccountSecret, SESSION_TTL_DAYS, hash_password, hash_session_token, new_session_token,
    normalize_username, resolve_display_name, validate_password, validate_username,
    verify_password,
};
use octopus_types::{
    Account, ChangePasswordRequest, LoginRequest, LoginResponse, RegisterRequest,
};
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;

/// 同一用户名在窗口内的连续失败上限。
const MAX_FAILURES: u32 = 10;
/// 失败计数窗口。
const FAILURE_WINDOW: StdDuration = StdDuration::from_secs(15 * 60);

/// 守卫成功后注入请求扩展的当前用户。
#[derive(Debug, Clone)]
pub struct CurrentUser(pub Account);

/// 当前请求所用的令牌摘要（登出 / 改密时用来精确吊销「这一条」会话）。
#[derive(Debug, Clone)]
pub struct CurrentToken(pub String);

impl CurrentUser {
    pub fn id(&self) -> &str {
        &self.0.id
    }
    pub fn is_admin(&self) -> bool {
        self.0.is_admin
    }
}

/// 当前用户（必须先过 `require_auth`）。取不到说明路由守卫漏配——直接 500 暴露问题，
/// 绝不「当成匿名放行」。
fn current(req: &Request) -> Result<CurrentUser, ApiError> {
    req.extensions().get::<CurrentUser>().cloned().ok_or_else(|| {
        tracing::error!(path = %req.uri().path(), "受保护路由缺少鉴权守卫（内部错误）");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "guard_missing",
            "服务端路由守卫缺失",
        )
    })
}

fn unauthorized(msg: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", msg)
}

fn forbidden(msg: &str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "forbidden", msg)
}

// ============================================================
// 登录失败限流（内存态：单进程本地服务够用）
// ============================================================

/// 按用户名记失败次数：达到上限后窗口内一律 429，成功登录立即清零。
#[derive(Default)]
pub struct LoginThrottle(Mutex<HashMap<String, (u32, Instant)>>);

impl LoginThrottle {
    /// 允许尝试 → Ok；已被封 → Err(还需等待的秒数)。
    pub fn check(&self, key: &str) -> Result<(), u64> {
        let map = self.0.lock().expect("throttle poisoned");
        match map.get(key) {
            Some((count, first)) if *count >= MAX_FAILURES => {
                let elapsed = first.elapsed();
                if elapsed < FAILURE_WINDOW {
                    return Err((FAILURE_WINDOW - elapsed).as_secs() + 1);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn record_failure(&self, key: &str) {
        let mut map = self.0.lock().expect("throttle poisoned");
        map.retain(|_, (_, first)| first.elapsed() < FAILURE_WINDOW);
        let entry = map.entry(key.to_string()).or_insert((0, Instant::now()));
        entry.0 += 1;
    }

    pub fn clear(&self, key: &str) {
        self.0.lock().expect("throttle poisoned").remove(key);
    }
}

/// 未知用户也走一次同代价的校验，避免用响应时间区分「用户是否存在」。
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| hash_password("octopus-timing-equalizer").unwrap_or_default())
}

// ============================================================
// 令牌解析与守卫
// ============================================================

fn bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// GET 的 `?token=`：只服务于无法自定义请求头的 `EventSource`（演出流）。
fn query_token(req: &Request) -> Option<String> {
    if req.method() != Method::GET {
        return None;
    }
    let q = req.uri().query()?;
    for pair in q.split('&') {
        if let Some(v) = pair.strip_prefix("token=") {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn token_of(req: &Request) -> Option<String> {
    bearer(req.headers()).or_else(|| query_token(req))
}

/// 路径参数：`/api/saves/sv-1/state` + prefix `/api/saves/` → `sv-1`。
fn path_param(path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    let seg = rest.split('/').next().unwrap_or("");
    (!seg.is_empty()).then(|| seg.to_string())
}

/// 守卫 1：登录。失败 401；成功把用户与令牌摘要注入请求扩展。
pub async fn require_auth(
    State(app): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = token_of(&req)
        .ok_or_else(|| unauthorized("需要登录：缺少访问令牌"))?;
    let token_hash = hash_session_token(&token);
    let account = app
        .store()
        .account_by_session(&token_hash)
        .await?
        .ok_or_else(|| unauthorized("登录已过期，请重新登录"))?;
    req.extensions_mut().insert(CurrentUser(account));
    req.extensions_mut().insert(CurrentToken(token_hash));
    Ok(next.run(req).await)
}

/// 守卫 2：管理员（全局配置 / API Key 只有管理员能读写）。
pub async fn guard_admin(req: Request, next: Next) -> Result<Response, ApiError> {
    if !current(&req)?.is_admin() {
        return Err(forbidden("仅管理员可访问"));
    }
    Ok(next.run(req).await)
}

/// 守卫 3：存档归属。不存在与越权同样回 404。
pub async fn guard_save_owner(
    State(app): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let user = current(&req)?;
    let id = path_param(req.uri().path(), "/api/saves/")
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在");
    match app.store().save_owner(&id).await? {
        Some(owner) if owner == user.id() => Ok(next.run(req).await),
        _ => Err(not_found()),
    }
}

/// 守卫 4：故事书可读 = 作者本人或已发布。
pub async fn guard_storybook_read(
    State(app): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let user = current(&req)?;
    let id = path_param(req.uri().path(), "/api/storybooks/")
        .ok_or_else(|| storybook_not_found())?;
    match app.store().storybook_owner_and_released(&id).await? {
        Some((owner, released)) if owner == user.id() || released => Ok(next.run(req).await),
        _ => Err(storybook_not_found()),
    }
}

/// 守卫 5：故事书可改（草稿、发布、删除）= 只有作者。
pub async fn guard_storybook_edit(
    State(app): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let user = current(&req)?;
    let id = path_param(req.uri().path(), "/api/storybooks/")
        .ok_or_else(|| storybook_not_found())?;
    match app.store().storybook_owner(&id).await? {
        Some(owner) if owner == user.id() => Ok(next.run(req).await),
        _ => Err(storybook_not_found()),
    }
}

/// 守卫 6：结对线程可改 = 其所属故事书的作者。
pub async fn guard_pair_thread_edit(
    State(app): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let user = current(&req)?;
    let id = path_param(req.uri().path(), "/api/pair/threads/")
        .ok_or_else(|| pair_thread_not_found())?;
    let sb = app.store().pair_thread_storybook(&id).await?;
    match sb {
        Some(sb_id) => match app.store().storybook_owner(&sb_id).await? {
            Some(owner) if owner == user.id() => Ok(next.run(req).await),
            _ => Err(pair_thread_not_found()),
        },
        None => Err(pair_thread_not_found()),
    }
}

/// 请求体里带 `thread_id` 的端点（结对对话，路径上没有 id）用的归属校验：
/// 不是自己的线程 → 404，与路径守卫同一套语义。
pub async fn ensure_pair_thread_owner(
    app: &AppState,
    thread_id: &str,
    user_id: &str,
) -> Result<(), ApiError> {
    let sb = app
        .store()
        .pair_thread_storybook(thread_id)
        .await?
        .ok_or_else(pair_thread_not_found)?;
    match app.store().storybook_owner(&sb).await? {
        Some(owner) if owner == user_id => Ok(()),
        _ => Err(pair_thread_not_found()),
    }
}

fn storybook_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在")
}

fn pair_thread_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "pair_thread_not_found", "结对会话不存在")
}

// ============================================================
// 端点
// ============================================================

async fn issue_session(
    app: &AppState,
    account: &Account,
    headers: &HeaderMap,
) -> Result<LoginResponse, ApiError> {
    let token = new_session_token();
    let token_hash = hash_session_token(&token);
    let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let expires_at = app
        .store()
        .create_session(&account.id, &token_hash, agent, SESSION_TTL_DAYS)
        .await?;
    // 登录时顺手清掉过期登录态，避免表随时间无限增长。
    let _ = app.store().purge_expired_sessions().await;
    Ok(LoginResponse {
        token,
        expires_at,
        account: account.clone(),
    })
}

/// 注册：成功即签发登录态（注册完不用再登录一次）。
pub async fn register(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let username = validate_username(&req.username)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid_username", m))?;
    validate_password(&req.password)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid_password", m))?;
    let display_name = resolve_display_name(req.display_name.as_deref(), &username);
    let account = app
        .store()
        .create_account(&username, &display_name, &req.password)
        .await?;
    let session = issue_session(&app, &account, &headers).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

/// 登录：用户名或口令错误一律同一句话（不泄露账号是否存在）。
pub async fn login(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let key = normalize_username(&req.username);
    if let Err(wait) = app.throttle().check(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            format!("登录失败次数过多，请 {wait} 秒后重试"),
        )
        .with_detail(json!({ "retry_after_secs": wait })));
    }

    let found: Option<AccountSecret> = app.store().account_secret_by_username(&key).await?;
    let ok = match &found {
        Some(secret) => verify_password(&req.password, &secret.password_hash),
        None => {
            let _ = verify_password(&req.password, dummy_hash());
            false
        }
    };
    let Some(secret) = found.filter(|_| ok) else {
        app.throttle().record_failure(&key);
        tracing::warn!(username = %key, "登录失败");
        return Err(unauthorized("用户名或密码不正确"));
    };

    app.throttle().clear(&key);
    app.store().touch_login(&secret.account.id).await?;
    let account = app
        .store()
        .account_by_id(&secret.account.id)
        .await?
        .unwrap_or(secret.account);
    Ok(Json(issue_session(&app, &account, &headers).await?))
}

/// 登出：只吊销当前会话（其它设备的登录态不受影响）。
pub async fn logout(
    State(app): State<Arc<AppState>>,
    Extension(token): Extension<CurrentToken>,
) -> Result<StatusCode, ApiError> {
    app.store().delete_session(&token.0).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 当前用户：前端启动时用它校验本地令牌是否还有效。
pub async fn me(Extension(user): Extension<CurrentUser>) -> Json<Account> {
    Json(user.0)
}

/// 改口令：校验旧口令 → 写入新哈希 → 吊销其它会话（只留当前这条）。
pub async fn change_password(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Extension(token): Extension<CurrentToken>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    validate_password(&req.new_password)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid_password", m))?;
    let secret = app
        .store()
        .account_secret_by_username(&user.0.username)
        .await?
        .ok_or_else(|| unauthorized("登录已失效，请重新登录"))?;
    if !verify_password(&req.current_password, &secret.password_hash) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "wrong_password",
            "当前密码不正确",
        ));
    }
    if req.new_password == req.current_password {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "password_unchanged",
            "新密码与当前密码相同",
        ));
    }
    app.store().set_password(&user.0.id, &req.new_password).await?;
    let revoked = app
        .store()
        .delete_user_sessions(&user.0.id, Some(&token.0))
        .await?;
    tracing::info!(user = %user.0.username, revoked, "口令已修改，其它设备的登录态已吊销");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;

    #[test]
    fn bearer_header_parsing() {
        let mut h = HeaderMap::new();
        assert!(bearer(&h).is_none());
        h.insert(header::AUTHORIZATION, "Bearer abc123".parse().unwrap());
        assert_eq!(bearer(&h).as_deref(), Some("abc123"));
        h.insert(header::AUTHORIZATION, "bearer  spaced ".parse().unwrap());
        assert_eq!(bearer(&h).as_deref(), Some("spaced"));
        h.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert!(bearer(&h).is_none());
        h.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert!(bearer(&h).is_none());
    }

    #[test]
    fn query_token_only_for_get() {
        let get = HttpRequest::builder()
            .method(Method::GET)
            .uri("/api/saves/sv-1/stream?token=deadbeef")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(token_of(&get).as_deref(), Some("deadbeef"));

        // 非 GET 不接受查询串令牌（防 CSRF 式的可分享链接）
        let post = HttpRequest::builder()
            .method(Method::POST)
            .uri("/api/saves/sv-1/rounds?token=deadbeef")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(token_of(&post).is_none());
    }

    #[test]
    fn path_param_extracts_first_segment() {
        assert_eq!(
            path_param("/api/saves/sv-1/state", "/api/saves/").as_deref(),
            Some("sv-1")
        );
        assert_eq!(
            path_param("/api/saves/sv-1", "/api/saves/").as_deref(),
            Some("sv-1")
        );
        assert!(path_param("/api/saves/", "/api/saves/").is_none());
        assert!(path_param("/api/storybooks/x", "/api/saves/").is_none());
    }

    #[test]
    fn throttle_locks_after_max_failures_and_clears_on_success() {
        let t = LoginThrottle::default();
        for _ in 0..MAX_FAILURES {
            assert!(t.check("mira").is_ok());
            t.record_failure("mira");
        }
        let wait = t.check("mira").expect_err("达到上限后应被拒");
        assert!(wait > 0 && wait <= FAILURE_WINDOW.as_secs() + 1);
        // 其它用户名不受影响
        assert!(t.check("other").is_ok());
        // 成功登录清零
        t.clear("mira");
        assert!(t.check("mira").is_ok());
    }

    #[test]
    fn dummy_hash_is_a_real_phc_string() {
        assert!(dummy_hash().starts_with("$argon2"));
        assert!(!verify_password("随便", dummy_hash()));
    }
}
