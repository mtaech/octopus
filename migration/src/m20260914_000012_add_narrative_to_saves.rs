use sea_orm_migration::prelude::*;

/// saves 表新增 narrative_json：存档级叙述段玩家偏好（section id → 开关 | 变体 key）。
/// 只影响之后的回合，不参与命令日志重放，因此是存档设置的普通列而非事件。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::NarrativeJson).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::NarrativeJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    NarrativeJson,
}
