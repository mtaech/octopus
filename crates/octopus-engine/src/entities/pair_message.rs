use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pair_messages")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// 所属会话线程（迁移前为 NULL，回填后必有值）。
    pub thread_id: Option<String>,
    pub storybook_id: String,
    pub seq: i64,
    pub role: String,
    #[sea_orm(column_type = "Text")]
    pub content: String,
    pub model: Option<String>,
    pub is_error: bool,
    #[sea_orm(column_type = "Text")]
    pub tools_json: Option<String>,
    /// 该消息显式引用的实体（JSON 数组）。
    #[sea_orm(column_type = "Text")]
    pub refs_json: Option<String>,
    /// 仅 assistant：思考流（reasoning_content）正文。
    #[sea_orm(column_type = "Text")]
    pub reasoning: Option<String>,
    /// user 消息的附件（JSON 数组：name + 文本正文）。
    #[sea_orm(column_type = "Text")]
    pub attachments_json: Option<String>,
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
