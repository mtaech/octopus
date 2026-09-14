//! 账户与登录态（系统侧「账号」）——区别于领域里的「玩家」（CONTEXT.md）。
//!
//! 分工：本模块只有**持久化与密码学原语**（口令哈希、令牌生成与摘要、users / sessions
//! 读写、归属列回填）；HTTP 语义（路由守卫、状态码、限流）在 `octopus-api::auth`。
//!
//! 硬约束：
//! - 口令只以 argon2id PHC 串落库，任何响应 / 日志都不出现哈希；
//! - 登录令牌只以 sha256 落库，原始令牌只在签发那一刻出现在响应里；
//! - 归属判定 fail-closed：查不到行或行为空归属一律「无权限」。

use chrono::{DateTime, Duration, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait,
    QueryFilter, Set, Statement,
};
use sha2::{Digest, Sha256};

use octopus_types::{Account, AdminUserRow};

use crate::entities;
use crate::error::EngineError;
use crate::storage::{now_iso, SqliteStore};

/// 默认管理员：首次运行（users 表为空）时创建。
pub const DEFAULT_ADMIN_USER_ID: &str = "user-octopus";
pub const DEFAULT_ADMIN_USERNAME: &str = "octopus";
pub const DEFAULT_ADMIN_PASSWORD: &str = "octopus";

/// 登录态有效期（天）。
pub const SESSION_TTL_DAYS: i64 = 30;
/// 两次 last_seen 落库的最小间隔（秒）：避免每个请求都写一次库。
pub const SESSION_TOUCH_INTERVAL_SECS: i64 = 300;

pub const USERNAME_MIN_CHARS: usize = 2;
pub const USERNAME_MAX_CHARS: usize = 24;
pub const PASSWORD_MIN_CHARS: usize = 4;

/// 账户 + 口令哈希：只在登录校验 / 改密路径上出现，绝不序列化给前端。
#[derive(Debug, Clone)]
pub struct AccountSecret {
    pub account: Account,
    pub password_hash: String,
}

/// 管理后台概览用的原始计数（一次查询拿全，不做 N+1）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdminCounts {
    pub users: u64,
    pub admins: u64,
    pub saves: u64,
    pub storybooks: u64,
    /// 未过期的登录态条数。
    pub active_sessions: u64,
}

// ============================================================
// 密码学原语
// ============================================================

/// argon2id（随机盐）→ PHC 串。
pub fn hash_password(plain: &str) -> Result<String, EngineError> {
    use argon2::password_hash::{SaltString, rand_core::OsRng};
    use argon2::{Argon2, PasswordHasher};
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| EngineError::Internal(format!("口令哈希失败：{e}")))
}

/// 校验口令。哈希串损坏 / 算法不认识 → false（不 panic、不放行）。
pub fn verify_password(plain: &str, phc: &str) -> bool {
    use argon2::password_hash::PasswordHash;
    use argon2::{Argon2, PasswordVerifier};
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(plain.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// 新登录令牌：32 字节 CSPRNG → 64 位十六进制。
pub fn new_session_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    hex(&buf)
}

/// 令牌的库存形态：sha256 十六进制（令牌本身是高熵随机串，无需再套口令哈希）。
pub fn hash_session_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ============================================================
// 输入校验
// ============================================================

/// 用户名规范化：去首尾空白 + 转小写。大小写不敏感的唯一性完全靠这一步
/// （落库的永远是规范化形态，所以 username 上的 UNIQUE 就是大小写不敏感的唯一约束）。
pub fn normalize_username(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// 校验用户名，成功返回规范化后的形态。
pub fn validate_username(raw: &str) -> Result<String, String> {
    let name = normalize_username(raw);
    let n = name.chars().count();
    if n < USERNAME_MIN_CHARS || n > USERNAME_MAX_CHARS {
        return Err(format!("用户名需 {USERNAME_MIN_CHARS}–{USERNAME_MAX_CHARS} 个字符"));
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '\\')
    {
        return Err("用户名不能包含空白、斜杠或控制字符".to_string());
    }
    Ok(name)
}

pub fn validate_password(pw: &str) -> Result<(), String> {
    if pw.chars().count() < PASSWORD_MIN_CHARS {
        return Err(format!("密码至少 {PASSWORD_MIN_CHARS} 个字符"));
    }
    Ok(())
}

/// 显示名：留空回落用户名；过长截断（按字符，不切坏 UTF-8）。
pub fn resolve_display_name(raw: Option<&str>, username: &str) -> String {
    let name = raw.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(username);
    name.chars().take(32).collect()
}

// ============================================================
// users / sessions
// ============================================================

fn account_of(m: entities::user::Model) -> Account {
    Account {
        id: m.id,
        username: m.username,
        display_name: m.display_name,
        is_admin: m.is_admin,
        must_change_password: m.must_change_password,
        created_at: m.created_at,
        last_login_at: m.last_login_at,
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

impl SqliteStore {
    /// 首次运行（users 为空）时创建默认管理员，并把无主的历史数据回填给它。
    ///
    /// 幂等：每次打开库都跑一遍（回填条件是 `owner_id IS NULL`）。多账户之前建的
    /// 存档 / 故事书因此在升级后自动归到默认管理员名下，不会变成谁都看不见的孤儿。
    pub async fn ensure_default_admin(&self) -> Result<(), EngineError> {
        let count = entities::user::Entity::find().count(self.conn()).await?;
        if count == 0 {
            let now = now_iso();
            let active = entities::user::ActiveModel {
                id: Set(DEFAULT_ADMIN_USER_ID.to_string()),
                username: Set(DEFAULT_ADMIN_USERNAME.to_string()),
                display_name: Set(DEFAULT_ADMIN_USERNAME.to_string()),
                password_hash: Set(hash_password(DEFAULT_ADMIN_PASSWORD)?),
                is_admin: Set(true),
                must_change_password: Set(true),
                created_at: Set(now.clone()),
                updated_at: Set(now),
                last_login_at: Set(None),
            };
            active.insert(self.conn()).await?;
            tracing::warn!(
                username = DEFAULT_ADMIN_USERNAME,
                "已创建默认管理员账户（用户名与口令相同），请登录后立即修改口令"
            );
        }

        let owner = DEFAULT_ADMIN_USER_ID.to_string();
        entities::save::Entity::update_many()
            .col_expr(entities::save::Column::OwnerId, Expr::value(owner.clone()))
            .filter(entities::save::Column::OwnerId.is_null())
            .exec(self.conn())
            .await?;
        entities::storybook::Entity::update_many()
            .col_expr(entities::storybook::Column::OwnerId, Expr::value(owner))
            .filter(entities::storybook::Column::OwnerId.is_null())
            .exec(self.conn())
            .await?;
        Ok(())
    }

    pub async fn count_users(&self) -> Result<u64, EngineError> {
        Ok(entities::user::Entity::find().count(self.conn()).await?)
    }

    pub async fn account_by_id(&self, id: &str) -> Result<Option<Account>, EngineError> {
        Ok(entities::user::Entity::find_by_id(id.to_string())
            .one(self.conn())
            .await?
            .map(account_of))
    }

    /// 带口令哈希的账户行（登录校验专用）。
    pub async fn account_secret_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AccountSecret>, EngineError> {
        Ok(entities::user::Entity::find()
            .filter(entities::user::Column::Username.eq(normalize_username(username)))
            .one(self.conn())
            .await?
            .map(|m| AccountSecret {
                account: account_of(m.clone()),
                password_hash: m.password_hash,
            }))
    }

    /// 建账户（普通账户）。用户名重复 → `Conflict("username_taken")`。
    pub async fn create_account(
        &self,
        username: &str,
        display_name: &str,
        password: &str,
    ) -> Result<Account, EngineError> {
        let username = normalize_username(username);
        if entities::user::Entity::find()
            .filter(entities::user::Column::Username.eq(username.clone()))
            .one(self.conn())
            .await?
            .is_some()
        {
            return Err(EngineError::Conflict("username_taken".to_string()));
        }
        let now = now_iso();
        let active = entities::user::ActiveModel {
            id: Set(format!("user-{}", uuid::Uuid::new_v4().simple())),
            username: Set(username),
            display_name: Set(display_name.to_string()),
            password_hash: Set(hash_password(password)?),
            is_admin: Set(false),
            must_change_password: Set(false),
            created_at: Set(now.clone()),
            updated_at: Set(now),
            last_login_at: Set(None),
        };
        // 并发注册同名时由 username 上的 UNIQUE 兜底。
        match active.insert(self.conn()).await {
            Ok(m) => Ok(account_of(m)),
            Err(e) if e.to_string().to_lowercase().contains("unique") => {
                Err(EngineError::Conflict("username_taken".to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// 改口令：顺手清掉「仍在使用初始口令」标记。
    pub async fn set_password(&self, user_id: &str, new_password: &str) -> Result<(), EngineError> {
        let Some(model) = entities::user::Entity::find_by_id(user_id.to_string())
            .one(self.conn())
            .await?
        else {
            return Err(EngineError::Internal("账户不存在".to_string()));
        };
        let hash = hash_password(new_password)?;
        let mut active: entities::user::ActiveModel = model.into();
        active.password_hash = Set(hash);
        active.must_change_password = Set(false);
        active.updated_at = Set(now_iso());
        active.update(self.conn()).await?;
        Ok(())
    }

    pub async fn touch_login(&self, user_id: &str) -> Result<(), EngineError> {
        if let Some(model) = entities::user::Entity::find_by_id(user_id.to_string())
            .one(self.conn())
            .await?
        {
            let mut active: entities::user::ActiveModel = model.into();
            active.last_login_at = Set(Some(now_iso()));
            active.update(self.conn()).await?;
        }
        Ok(())
    }

    /// 签发登录态，返回过期时间（RFC3339）。
    pub async fn create_session(
        &self,
        user_id: &str,
        token_hash: &str,
        user_agent: Option<&str>,
        ttl_days: i64,
    ) -> Result<String, EngineError> {
        let now = Utc::now();
        let expires_at = (now + Duration::days(ttl_days)).to_rfc3339();
        let active = entities::session::ActiveModel {
            token_hash: Set(token_hash.to_string()),
            user_id: Set(user_id.to_string()),
            created_at: Set(now.to_rfc3339()),
            expires_at: Set(expires_at.clone()),
            last_seen_at: Set(now.to_rfc3339()),
            user_agent: Set(user_agent.map(|s| s.chars().take(200).collect())),
        };
        active.insert(self.conn()).await?;
        Ok(expires_at)
    }

    /// 按令牌摘要取账户：过期即删并返回 None。有效时按需推进 last_seen（节流写库）。
    pub async fn account_by_session(
        &self,
        token_hash: &str,
    ) -> Result<Option<Account>, EngineError> {
        let Some(session) = entities::session::Entity::find_by_id(token_hash.to_string())
            .one(self.conn())
            .await?
        else {
            return Ok(None);
        };
        let now = Utc::now();
        let expired = parse_ts(&session.expires_at).is_none_or(|t| t <= now);
        if expired {
            entities::session::Entity::delete_by_id(session.token_hash)
                .exec(self.conn())
                .await?;
            return Ok(None);
        }
        let stale = parse_ts(&session.last_seen_at)
            .is_none_or(|t| (now - t).num_seconds() >= SESSION_TOUCH_INTERVAL_SECS);
        let user_id = session.user_id.clone();
        if stale {
            let mut active: entities::session::ActiveModel = session.into();
            active.last_seen_at = Set(now.to_rfc3339());
            active.update(self.conn()).await?;
        }
        self.account_by_id(&user_id).await
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool, EngineError> {
        let res = entities::session::Entity::delete_by_id(token_hash.to_string())
            .exec(self.conn())
            .await?;
        Ok(res.rows_affected > 0)
    }

    /// 删某账户的全部登录态；`keep` 指定的令牌摘要保留（改密后只留当前会话）。
    pub async fn delete_user_sessions(
        &self,
        user_id: &str,
        keep: Option<&str>,
    ) -> Result<u64, EngineError> {
        let mut q = entities::session::Entity::delete_many()
            .filter(entities::session::Column::UserId.eq(user_id.to_string()));
        if let Some(keep) = keep {
            q = q.filter(entities::session::Column::TokenHash.ne(keep.to_string()));
        }
        Ok(q.exec(self.conn()).await?.rows_affected)
    }

    /// 清理过期登录态（登录时顺手跑一次）。
    pub async fn purge_expired_sessions(&self) -> Result<u64, EngineError> {
        let now = Utc::now().to_rfc3339();
        Ok(entities::session::Entity::delete_many()
            .filter(entities::session::Column::ExpiresAt.lt(now))
            .exec(self.conn())
            .await?
            .rows_affected)
    }

    // ---------- 归属 ----------

    /// 存档归属；`None` = 行不存在。归属为空的历史行视为默认管理员的（回填已覆盖）。
    pub async fn save_owner(&self, id: &str) -> Result<Option<String>, EngineError> {
        Ok(entities::save::Entity::find_by_id(id.to_string())
            .one(self.conn())
            .await?
            .map(|m| m.owner_id.unwrap_or_else(|| DEFAULT_ADMIN_USER_ID.to_string())))
    }

    /// 故事书作者；`None` = 行不存在。
    pub async fn storybook_owner(&self, id: &str) -> Result<Option<String>, EngineError> {
        Ok(entities::storybook::Entity::find_by_id(id.to_string())
            .one(self.conn())
            .await?
            .map(|m| m.owner_id.unwrap_or_else(|| DEFAULT_ADMIN_USER_ID.to_string())))
    }

    /// 故事书的（作者, 是否已发布）；`None` = 行不存在。
    /// 读权限判定要同时看这两项，所以合并成一次点查（不加载整份草稿）。
    pub async fn storybook_owner_and_released(
        &self,
        id: &str,
    ) -> Result<Option<(String, bool)>, EngineError> {
        Ok(entities::storybook::Entity::find_by_id(id.to_string())
            .one(self.conn())
            .await?
            .map(|m| {
                (
                    m.owner_id.unwrap_or_else(|| DEFAULT_ADMIN_USER_ID.to_string()),
                    m.released_json.is_some(),
                )
            }))
    }

    // ---------- 管理后台 ----------

    /// 概览计数：账户 / 管理员 / 存档 / 故事书 / 有效登录态。
    pub async fn admin_counts(&self) -> Result<AdminCounts, EngineError> {
        let now = now_iso();
        let row = self
            .conn()
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT (SELECT COUNT(*) FROM users) AS users, \
                        (SELECT COUNT(*) FROM users WHERE is_admin = 1) AS admins, \
                        (SELECT COUNT(*) FROM saves) AS saves, \
                        (SELECT COUNT(*) FROM storybooks) AS storybooks, \
                        (SELECT COUNT(*) FROM sessions WHERE expires_at > ?) AS active_sessions",
                [now.into()],
            ))
            .await?;
        let Some(row) = row else {
            return Ok(AdminCounts::default());
        };
        let n = |col: &str| -> Result<u64, EngineError> {
            Ok(row.try_get::<i64>("", col)?.max(0) as u64)
        };
        Ok(AdminCounts {
            users: n("users")?,
            admins: n("admins")?,
            saves: n("saves")?,
            storybooks: n("storybooks")?,
            active_sessions: n("active_sessions")?,
        })
    }

    /// 管理后台账户列表：相关子查询一次算清内容统计（账户数量级很小，但没必要 N+1）。
    /// `self_id` 用来标记「就是你自己」，前端据此禁用删除。
    pub async fn admin_user_rows(
        &self,
        self_id: &str,
    ) -> Result<Vec<AdminUserRow>, EngineError> {
        let now = now_iso();
        let rows = self
            .conn()
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT u.id, u.username, u.display_name, u.is_admin, u.must_change_password, \
                        u.created_at, u.last_login_at, \
                        (SELECT COUNT(*) FROM saves s WHERE s.owner_id = u.id) AS saves, \
                        (SELECT COUNT(*) FROM storybooks b WHERE b.owner_id = u.id) AS storybooks, \
                        (SELECT COUNT(*) FROM sessions x WHERE x.user_id = u.id AND x.expires_at > ?) AS active_sessions \
                 FROM users u ORDER BY u.created_at ASC, u.username ASC",
                [now.into()],
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let count = |col: &str| -> Result<u64, EngineError> {
                Ok(row.try_get::<i64>("", col)?.max(0) as u64)
            };
            let id: String = row.try_get("", "id")?;
            out.push(AdminUserRow {
                is_self: id == self_id,
                id,
                username: row.try_get("", "username")?,
                display_name: row.try_get("", "display_name")?,
                is_admin: row.try_get("", "is_admin")?,
                must_change_password: row.try_get("", "must_change_password")?,
                created_at: row.try_get("", "created_at")?,
                last_login_at: row.try_get("", "last_login_at")?,
                saves: count("saves")?,
                storybooks: count("storybooks")?,
                active_sessions: count("active_sessions")?,
            });
        }
        Ok(out)
    }

    /// 管理员数量：用于「不能没有管理员」这条兜底规则。
    pub async fn count_admins(&self) -> Result<u64, EngineError> {
        Ok(entities::user::Entity::find()
            .filter(entities::user::Column::IsAdmin.eq(true))
            .count(self.conn())
            .await?)
    }

    /// 设置 / 取消管理员。返回是否命中账户。
    pub async fn set_admin(&self, user_id: &str, is_admin: bool) -> Result<bool, EngineError> {
        let res = entities::user::Entity::update_many()
            .col_expr(entities::user::Column::IsAdmin, Expr::value(is_admin))
            .col_expr(entities::user::Column::UpdatedAt, Expr::value(now_iso()))
            .filter(entities::user::Column::Id.eq(user_id.to_string()))
            .exec(self.conn())
            .await?;
        Ok(res.rows_affected > 0)
    }

    pub async fn set_display_name(
        &self,
        user_id: &str,
        display_name: &str,
    ) -> Result<bool, EngineError> {
        let res = entities::user::Entity::update_many()
            .col_expr(
                entities::user::Column::DisplayName,
                Expr::value(display_name.to_string()),
            )
            .col_expr(entities::user::Column::UpdatedAt, Expr::value(now_iso()))
            .filter(entities::user::Column::Id.eq(user_id.to_string()))
            .exec(self.conn())
            .await?;
        Ok(res.rows_affected > 0)
    }

    /// 删账户（连同它的全部登录态）。**内容不在这里删**：调用方先按需清理
    /// 存档 / 故事书（并走各自的删除路径清内存会话与派生数据）。
    pub async fn delete_account(&self, user_id: &str) -> Result<bool, EngineError> {
        let res = entities::user::Entity::delete_by_id(user_id.to_string())
            .exec(self.conn())
            .await?;
        entities::session::Entity::delete_many()
            .filter(entities::session::Column::UserId.eq(user_id.to_string()))
            .exec(self.conn())
            .await?;
        Ok(res.rows_affected > 0)
    }

    /// 某账户的全部存档 id（管理后台清理内容用）。
    pub async fn save_ids_for(&self, owner_id: &str) -> Result<Vec<String>, EngineError> {
        Ok(entities::save::Entity::find()
            .filter(entities::save::Column::OwnerId.eq(owner_id.to_string()))
            .all(self.conn())
            .await?
            .into_iter()
            .map(|m| m.id)
            .collect())
    }

    /// 某账户的全部故事书 id（管理后台清理内容用）。
    pub async fn storybook_ids_for(&self, owner_id: &str) -> Result<Vec<String>, EngineError> {
        Ok(entities::storybook::Entity::find()
            .filter(entities::storybook::Column::OwnerId.eq(owner_id.to_string()))
            .all(self.conn())
            .await?
            .into_iter()
            .map(|m| m.id)
            .collect())
    }

    /// 结对线程所属故事书（用于「改草稿」权限判定）。
    pub async fn pair_thread_storybook(&self, thread_id: &str) -> Result<Option<String>, EngineError> {
        Ok(entities::pair_thread::Entity::find_by_id(thread_id.to_string())
            .one(self.conn())
            .await?
            .map(|m| m.storybook_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::ConnectionTrait;

    #[test]
    fn password_round_trip_and_rejection() {
        let phc = hash_password("s3cret-口令").unwrap();
        assert!(phc.starts_with("$argon2"), "应是 argon2 PHC 串：{phc}");
        assert!(verify_password("s3cret-口令", &phc));
        assert!(!verify_password("s3cret-口吟", &phc));
        assert!(!verify_password("", &phc));
        // 损坏的哈希串不得 panic，也不得放行。
        assert!(!verify_password("s3cret-口令", "not-a-phc"));
        assert!(!verify_password("s3cret-口令", ""));
    }

    #[test]
    fn salt_is_random_per_hash() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b, "每次哈希都要新盐");
        assert!(verify_password("same", &a) && verify_password("same", &b));
    }

    #[test]
    fn session_token_shape_and_hash() {
        let t = new_session_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(t, new_session_token());
        assert_eq!(hash_session_token(&t).len(), 64);
        assert_ne!(hash_session_token(&t), t, "库存的是摘要，不是令牌本身");
        assert_eq!(hash_session_token(&t), hash_session_token(&t), "摘要稳定");
    }

    #[test]
    fn username_normalization_and_validation() {
        assert_eq!(normalize_username("  Octopus "), "octopus");
        assert_eq!(validate_username(" Octopus ").unwrap(), "octopus");
        // 大小写不同 = 同一个用户名（唯一性靠规范化，不靠 DB collation）
        assert_eq!(validate_username("OCTOPUS").unwrap(), validate_username("octopus").unwrap());
        assert!(validate_username("a").is_err());
        assert!(validate_username(&"x".repeat(USERNAME_MAX_CHARS + 1)).is_err());
        assert!(validate_username("有 空格").is_err());
        assert!(validate_username("a/b").is_err());
        assert!(validate_username("凯尔").is_ok(), "中文用户名可用");
    }

    #[test]
    fn display_name_falls_back_and_truncates() {
        assert_eq!(resolve_display_name(None, "octopus"), "octopus");
        assert_eq!(resolve_display_name(Some("   "), "octopus"), "octopus");
        assert_eq!(resolve_display_name(Some(" 星尘 "), "octopus"), "星尘");
        assert_eq!(resolve_display_name(Some(&"长".repeat(40)), "octopus").chars().count(), 32);
    }

    async fn store() -> SqliteStore {
        SqliteStore::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn default_admin_is_seeded_once_and_is_admin() {
        let s = store().await;
        let admin = s
            .account_secret_by_username(DEFAULT_ADMIN_USERNAME)
            .await
            .unwrap()
            .expect("默认管理员已创建");
        assert_eq!(admin.account.id, DEFAULT_ADMIN_USER_ID);
        assert!(admin.account.is_admin);
        assert!(admin.account.must_change_password, "初始口令必须被标记为待改");
        assert!(verify_password(DEFAULT_ADMIN_PASSWORD, &admin.password_hash));
        assert_eq!(s.count_users().await.unwrap(), 1);

        // 幂等：再跑一次不重复建号
        s.ensure_default_admin().await.unwrap();
        assert_eq!(s.count_users().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn create_account_rejects_duplicate_username_case_insensitively() {
        let s = store().await;
        s.create_account("Mira", "米拉", "pw-1234").await.unwrap();
        let err = s.create_account("mira", "另一个人", "pw-1234").await;
        assert!(matches!(err, Err(EngineError::Conflict(ref c)) if c == "username_taken"));
        let dup = s.create_account("  MIRA ", "x", "pw-1234").await;
        assert!(dup.is_err());
    }

    #[tokio::test]
    async fn password_change_clears_must_change_flag() {
        let s = store().await;
        s.set_password(DEFAULT_ADMIN_USER_ID, "new-pw-1234").await.unwrap();
        let admin = s
            .account_secret_by_username(DEFAULT_ADMIN_USERNAME)
            .await
            .unwrap()
            .unwrap();
        assert!(!admin.account.must_change_password);
        assert!(verify_password("new-pw-1234", &admin.password_hash));
        assert!(!verify_password(DEFAULT_ADMIN_PASSWORD, &admin.password_hash));
    }

    #[tokio::test]
    async fn session_round_trip_expiry_and_revocation() {
        let s = store().await;
        let user = s.create_account("mira", "米拉", "pw-1234").await.unwrap();
        let token = new_session_token();
        let th = hash_session_token(&token);
        let expires = s.create_session(&user.id, &th, Some("test-agent"), SESSION_TTL_DAYS).await.unwrap();
        assert!(expires > now_iso(), "过期时间在未来");

        let got = s.account_by_session(&th).await.unwrap().expect("登录态有效");
        assert_eq!(got.id, user.id);

        assert!(s.account_by_session(&hash_session_token("bogus")).await.unwrap().is_none());

        // 过期即失效并被删除
        s.create_session(&user.id, &hash_session_token("expired"), None, -1).await.unwrap();
        assert!(s.account_by_session(&hash_session_token("expired")).await.unwrap().is_none());
        assert!(s.delete_session(&th).await.unwrap());
        assert!(s.account_by_session(&th).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoke_other_sessions_keeps_current() {
        let s = store().await;
        let user = s.create_account("mira", "米拉", "pw-1234").await.unwrap();
        let keep = hash_session_token("keep-me");
        s.create_session(&user.id, &keep, None, SESSION_TTL_DAYS).await.unwrap();
        s.create_session(&user.id, &hash_session_token("other-1"), None, SESSION_TTL_DAYS).await.unwrap();
        s.create_session(&user.id, &hash_session_token("other-2"), None, SESSION_TTL_DAYS).await.unwrap();

        assert_eq!(s.delete_user_sessions(&user.id, Some(&keep)).await.unwrap(), 2);
        assert!(s.account_by_session(&keep).await.unwrap().is_some());
        assert!(s.account_by_session(&hash_session_token("other-1")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn admin_rows_counts_and_account_removal() {
        let s = store().await;
        // 断言用增量而不是绝对值：`open_in_memory` 在 sqlx 下是否跨测试共享库属于实现细节。
        let before = s.admin_counts().await.unwrap();
        let u = s.create_account("mira", "米拉", "pw-1234").await.unwrap();

        let rows = s.admin_user_rows(DEFAULT_ADMIN_USER_ID).await.unwrap();
        let me = rows.iter().find(|r| r.id == DEFAULT_ADMIN_USER_ID).unwrap();
        assert!(me.is_self && me.is_admin && me.username == DEFAULT_ADMIN_USERNAME);
        assert!(me.must_change_password, "初始口令待改标记要在列表里看得见");
        let other = rows
            .iter()
            .find(|r| r.id == u.id)
            .expect("新账户出现在管理列表");
        assert!(!other.is_self && !other.is_admin);
        assert_eq!((other.saves, other.storybooks), (0, 0));

        let after = s.admin_counts().await.unwrap();
        assert_eq!(after.users, before.users + 1);
        assert_eq!(after.admins, before.admins, "新账户默认不是管理员");

        // 提权 / 降级
        assert!(s.set_admin(&u.id, true).await.unwrap());
        assert_eq!(s.count_admins().await.unwrap(), after.admins + 1);
        assert!(s.set_admin(&u.id, false).await.unwrap());
        assert_eq!(s.count_admins().await.unwrap(), after.admins);
        assert!(!s.set_admin("user-nope", true).await.unwrap(), "不存在的账户不改");

        // 显示名
        assert!(s.set_display_name(&u.id, "米拉二世").await.unwrap());
        assert_eq!(
            s.account_by_id(&u.id).await.unwrap().unwrap().display_name,
            "米拉二世"
        );

        // 删账户连登录态一起清掉
        let th = hash_session_token(&new_session_token());
        s.create_session(&u.id, &th, None, SESSION_TTL_DAYS).await.unwrap();
        assert!(s.account_by_session(&th).await.unwrap().is_some());
        assert!(s.delete_account(&u.id).await.unwrap());
        assert!(s.account_by_id(&u.id).await.unwrap().is_none());
        assert!(s.account_by_session(&th).await.unwrap().is_none());
        assert_eq!(s.admin_counts().await.unwrap().users, before.users);
    }

    #[tokio::test]
    async fn legacy_rows_without_owner_are_claimed_by_default_admin() {
        let s = store().await;
        // 直接插一行「多账户之前」的无主数据，模拟升级后的历史库。
        let sb = s.create_storybook_draft(None, &serde_json::json!({}), DEFAULT_ADMIN_USER_ID).await.unwrap();
        s.conn()
            .execute_unprepared(&format!("UPDATE storybooks SET owner_id = NULL WHERE id = '{}'", sb.id))
            .await
            .unwrap();
        s.conn()
            .execute_unprepared("INSERT INTO saves (id, title, storybook_id, storybook_title, embedded_revision, latest_revision, needs_upgrade, imported, is_sandbox, storybook_json, auto_confirm, created_at, updated_at, last_played_at) VALUES ('sv-legacy','旧档','sb-x','书',1,1,0,0,0,'{}',0,'now','now','now')")
            .await
            .unwrap();

        s.ensure_default_admin().await.unwrap();
        assert_eq!(s.storybook_owner(&sb.id).await.unwrap().as_deref(), Some(DEFAULT_ADMIN_USER_ID));
        assert_eq!(s.save_owner("sv-legacy").await.unwrap().as_deref(), Some(DEFAULT_ADMIN_USER_ID));
        // 不存在的行 → None（fail-closed 的调用方据此判 404）
        assert!(s.save_owner("sv-nope").await.unwrap().is_none());
    }
}
