use sea_orm_migration::prelude::*;

/// saves 表新增 legacy_json：存档遗留区（#14 ②）。
///
/// 版次升级时，新版故事书删掉的人物若被玩家裁决为「遗留冻结 / 叙事离场」，
/// 其旧定义存入此列，实例在升级后仍能据旧定义重建。属于存档自包含状态的一部分，
/// 随导出包一并交付，因此是普通列而非单独表。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::LegacyJson).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::LegacyJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    LegacyJson,
}
