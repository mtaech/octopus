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
    /// 本存档使用的模型（provider id + model id）；NULL 表示用全局角色默认。
    #[sea_orm(nullable)]
    pub model_provider_id: Option<String>,
    #[sea_orm(nullable)]
    pub model: Option<String>,
    /// 本存档的思考强度（reasoning_effort）；NULL 表示用角色默认 / 供应商默认。
    #[sea_orm(nullable)]
    pub reasoning_effort: Option<String>,
    /// 存档级叙述段玩家偏好（section id → 开关 | 变体 key）的 JSON；NULL 表示全用故事书默认。
    #[sea_orm(nullable, column_type = "Text")]
    pub narrative_json: Option<String>,
    /// 存档遗留区（#14）：版次升级时被删除的旧定义（JSON 数组）；NULL 表示无遗留。
    #[sea_orm(nullable, column_type = "Text")]
    pub legacy_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_played_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
