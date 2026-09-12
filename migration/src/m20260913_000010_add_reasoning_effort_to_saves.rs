use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::ReasoningEffort).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::ReasoningEffort)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    ReasoningEffort,
}
