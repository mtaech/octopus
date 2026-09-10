use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "storybooks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub title: String,
    #[sea_orm(column_type = "Text")]
    pub draft_json: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub released_json: Option<String>,
    pub revision: i64,
    pub draft_version: i64,
    pub updated_at: String,
    pub released_at: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
