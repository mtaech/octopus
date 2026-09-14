use sea_orm::entity::prelude::*;

/// 账户（系统侧「账号」）：用户名一律以小写形态落库，唯一约束因此天然大小写不敏感。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub username: String,
    pub display_name: String,
    /// argon2id PHC 串（含盐与参数），绝不外传。
    #[sea_orm(column_type = "Text")]
    pub password_hash: String,
    pub is_admin: bool,
    /// 仍在使用初始口令：前端据此常驻提醒改密。
    pub must_change_password: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
