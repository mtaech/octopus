use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PairThreads::Table)
                    .add_column(ColumnDef::new(PairThreads::PendingSuggestionsJson).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PairThreads::Table)
                    .drop_column(PairThreads::PendingSuggestionsJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum PairThreads {
    Table,
    PendingSuggestionsJson,
}
