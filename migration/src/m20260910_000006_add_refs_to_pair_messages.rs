use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PairMessages::Table)
                    .add_column(ColumnDef::new(PairMessages::RefsJson).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PairMessages::Table)
                    .drop_column(PairMessages::RefsJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum PairMessages {
    Table,
    RefsJson,
}
