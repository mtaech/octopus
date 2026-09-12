use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1) 结对会话线程表：一本故事书下可有多条按主题隔离的会话。
        manager
            .create_table(
                Table::create()
                    .table(PairThreads::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(PairThreads::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(PairThreads::StorybookId).string().not_null())
                    .col(ColumnDef::new(PairThreads::Title).string().not_null())
                    .col(ColumnDef::new(PairThreads::CreatedAt).string().not_null())
                    .col(ColumnDef::new(PairThreads::UpdatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_pair_threads_storybook_updated")
                    .table(PairThreads::Table)
                    .col(PairThreads::StorybookId)
                    .col(PairThreads::UpdatedAt)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // 2) 消息挂到线程上。
        manager
            .alter_table(
                Table::alter()
                    .table(PairMessages::Table)
                    .add_column(ColumnDef::new(PairMessages::ThreadId).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_pair_messages_thread_seq")
                    .table(PairMessages::Table)
                    .col(PairMessages::ThreadId)
                    .col(PairMessages::Seq)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // 3) 回填：迁移前「一本故事书一条隐含线程」，为它建一条「对话 1」并挂回旧消息。
        let db = manager.get_connection();
        db.execute_unprepared(
            "INSERT INTO pair_threads (id, storybook_id, title, created_at, updated_at) \
             SELECT 'pt-' || lower(hex(randomblob(16))), m.storybook_id, '对话 1', \
                    strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now') \
             FROM (SELECT DISTINCT storybook_id FROM pair_messages WHERE thread_id IS NULL) AS m \
             WHERE NOT EXISTS (SELECT 1 FROM pair_threads t WHERE t.storybook_id = m.storybook_id)",
        )
        .await?;
        db.execute_unprepared(
            "UPDATE pair_messages \
             SET thread_id = (SELECT id FROM pair_threads t WHERE t.storybook_id = pair_messages.storybook_id LIMIT 1) \
             WHERE thread_id IS NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_pair_messages_thread_seq")
                    .table(PairMessages::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PairMessages::Table)
                    .drop_column(PairMessages::ThreadId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(PairThreads::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PairThreads {
    Table,
    Id,
    StorybookId,
    Title,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PairMessages {
    Table,
    ThreadId,
    Seq,
}
