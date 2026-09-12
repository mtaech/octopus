use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PairMessages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PairMessages::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PairMessages::StorybookId).string().not_null())
                    .col(ColumnDef::new(PairMessages::Seq).big_integer().not_null())
                    .col(ColumnDef::new(PairMessages::Role).string().not_null())
                    .col(ColumnDef::new(PairMessages::Content).text().not_null())
                    .col(ColumnDef::new(PairMessages::Model).string().null())
                    .col(
                        ColumnDef::new(PairMessages::IsError)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(PairMessages::ToolsJson).text().null())
                    .col(ColumnDef::new(PairMessages::CreatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_pair_messages_storybook_seq")
                    .table(PairMessages::Table)
                    .col(PairMessages::StorybookId)
                    .col(PairMessages::Seq)
                    .if_not_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PairMessages::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PairMessages {
    Table,
    Id,
    StorybookId,
    Seq,
    Role,
    Content,
    Model,
    IsError,
    ToolsJson,
    CreatedAt,
}
