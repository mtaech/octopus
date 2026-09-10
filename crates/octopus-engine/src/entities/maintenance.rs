use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "maintenance")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub save_id: String,
    pub at: String,
    pub op: String,
    #[sea_orm(column_type = "Text")]
    pub summary: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
