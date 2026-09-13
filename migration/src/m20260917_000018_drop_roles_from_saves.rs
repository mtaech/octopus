use sea_orm_migration::prelude::*;

/// 删除 saves.roles_json：单一 AI 后不再有「按角色模型分工」。
///
/// 模型选择退回存档级单模型（旧的 model_provider_id / model / reasoning_effort 三列）。
/// 这是存档设置而非事件：只影响之后的回合，不参与命令日志重放。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::RolesJson)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::RolesJson).text().null())
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    RolesJson,
}
