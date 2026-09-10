use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "saves")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub title: String,
    pub storybook_id: String,
    pub storybook_title: String,
    pub embedded_revision: i64,
    pub latest_revision: i64,
    pub needs_upgrade: bool,
    pub imported: bool,
    pub is_sandbox: bool,
    #[sea_orm(column_type = "Text")]
    pub storybook_json: String,
    pub auto_confirm: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_played_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
