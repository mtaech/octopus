use sea_orm::entity::prelude::*;

/// 登录态：主键是令牌的 sha256（原始令牌只在签发那一刻出现在响应里）。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "sessions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub token_hash: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
    pub last_seen_at: String,
    pub user_agent: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
