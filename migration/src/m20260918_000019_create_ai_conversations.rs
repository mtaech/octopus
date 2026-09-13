use sea_orm_migration::prelude::*;

/// ai_conversations 表：每个存档一行的追加式模型会话快照。
///
/// 整段会话（user 提示词 + assistant 原文）序列化成 JSON 存一行，用于跨重启保住
/// 供应商 prompt / KV 缓存前缀。它是**派生数据**：丢了只影响缓存命中率与跨重启连续感，
/// 权威回合与 replay 都不依赖它。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS ai_conversations (                 save_id TEXT PRIMARY KEY, messages_json TEXT NOT NULL, updated_at TEXT NOT NULL)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS ai_conversations")
            .await?;
        Ok(())
    }
}
