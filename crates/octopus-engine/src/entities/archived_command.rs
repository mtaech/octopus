use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "archived_commands")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub save_id: String,
    pub origin_seq: i64,
    pub seq: i64,
    pub round: i64,
    pub kind: String,
    #[sea_orm(column_type = "Text")]
    pub payload_json: String,
    pub ts: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
