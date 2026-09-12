use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pair_threads")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub storybook_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    /// 待审查改动（JSON 数组，前端形状）：刷新 / 切会话后可恢复，避免建议丢失。
    #[sea_orm(column_type = "Text")]
    pub pending_suggestions_json: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
