use sea_orm::entity::prelude::*;

/// 世界状态快照行（#06 ②）：派生启动缓存，非权威。字段语义见迁移文件。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "snapshots")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub save_id: String,
    pub seq: i64,
    pub taken_at: String,
    pub format_version: i32,
    pub storybook_revision: i32,
    #[sea_orm(column_type = "Text")]
    pub state_json: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
