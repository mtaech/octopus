use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite 一条 ALTER TABLE 只能加一列，故分两次。
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::ModelProviderId).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::Model).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::ModelProviderId)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::Model)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    ModelProviderId,
    Model,
}
