use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. storybooks
        manager
            .create_table(
                Table::create()
                    .table(Storybooks::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Storybooks::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Storybooks::Title).string().not_null())
                    .col(ColumnDef::new(Storybooks::DraftJson).text().not_null())
                    .col(ColumnDef::new(Storybooks::ReleasedJson).text().null())
                    .col(ColumnDef::new(Storybooks::Revision).big_integer().not_null().default(0))
                    .col(ColumnDef::new(Storybooks::DraftVersion).big_integer().not_null().default(0))
                    .col(ColumnDef::new(Storybooks::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(Storybooks::ReleasedAt).string().null())
                    .to_owned(),
            )
            .await?;

        // 2. saves
        manager
            .create_table(
                Table::create()
                    .table(Saves::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Saves::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Saves::Title).string().not_null())
                    .col(ColumnDef::new(Saves::StorybookId).string().not_null())
                    .col(ColumnDef::new(Saves::StorybookTitle).string().not_null())
                    .col(ColumnDef::new(Saves::EmbeddedRevision).big_integer().not_null())
                    .col(ColumnDef::new(Saves::LatestRevision).big_integer().not_null())
                    .col(ColumnDef::new(Saves::NeedsUpgrade).boolean().not_null().default(false))
                    .col(ColumnDef::new(Saves::Imported).boolean().not_null().default(false))
                    .col(ColumnDef::new(Saves::StorybookJson).text().not_null())
                    .col(ColumnDef::new(Saves::AutoConfirm).boolean().not_null().default(false))
                    .col(ColumnDef::new(Saves::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Saves::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(Saves::LastPlayedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        // 3. commands
        manager
            .create_table(
                Table::create()
                    .table(Commands::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Commands::Id).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(Commands::SaveId).string().not_null())
                    .col(ColumnDef::new(Commands::Seq).big_integer().not_null())
                    .col(ColumnDef::new(Commands::Round).big_integer().not_null())
                    .col(ColumnDef::new(Commands::Kind).string().not_null())
                    .col(ColumnDef::new(Commands::PayloadJson).text().not_null())
                    .col(ColumnDef::new(Commands::Ts).string().not_null())
                    .to_owned(),
            )
            .await?;

        // 3.1 idx_commands_save_seq
        manager
            .create_index(
                Index::create()
                    .name("idx_commands_save_seq")
                    .table(Commands::Table)
                    .col(Commands::SaveId)
                    .col(Commands::Seq)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // 4. archived_commands
        manager
            .create_table(
                Table::create()
                    .table(ArchivedCommands::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ArchivedCommands::Id).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(ArchivedCommands::SaveId).string().not_null())
                    .col(ColumnDef::new(ArchivedCommands::OriginSeq).big_integer().not_null())
                    .col(ColumnDef::new(ArchivedCommands::Seq).big_integer().not_null())
                    .col(ColumnDef::new(ArchivedCommands::Round).big_integer().not_null())
                    .col(ColumnDef::new(ArchivedCommands::Kind).string().not_null())
                    .col(ColumnDef::new(ArchivedCommands::PayloadJson).text().not_null())
                    .col(ColumnDef::new(ArchivedCommands::Ts).string().not_null())
                    .to_owned(),
            )
            .await?;

        // 5. maintenance
        manager
            .create_table(
                Table::create()
                    .table(Maintenance::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Maintenance::Id).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(Maintenance::SaveId).string().not_null())
                    .col(ColumnDef::new(Maintenance::At).string().not_null())
                    .col(ColumnDef::new(Maintenance::Op).string().not_null())
                    .col(ColumnDef::new(Maintenance::Summary).text().not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Maintenance::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ArchivedCommands::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Commands::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Saves::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Storybooks::Table).if_exists().to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Storybooks {
    Table,
    Id,
    Title,
    DraftJson,
    ReleasedJson,
    Revision,
    DraftVersion,
    UpdatedAt,
    ReleasedAt,
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    Id,
    Title,
    StorybookId,
    StorybookTitle,
    EmbeddedRevision,
    LatestRevision,
    NeedsUpgrade,
    Imported,
    StorybookJson,
    AutoConfirm,
    CreatedAt,
    UpdatedAt,
    LastPlayedAt,
}

#[derive(DeriveIden)]
enum Commands {
    Table,
    Id,
    SaveId,
    Seq,
    Round,
    Kind,
    PayloadJson,
    Ts,
}

#[derive(DeriveIden)]
enum ArchivedCommands {
    Table,
    Id,
    SaveId,
    OriginSeq,
    Seq,
    Round,
    Kind,
    PayloadJson,
    Ts,
}

#[derive(DeriveIden)]
enum Maintenance {
    Table,
    Id,
    SaveId,
    At,
    Op,
    Summary,
}
