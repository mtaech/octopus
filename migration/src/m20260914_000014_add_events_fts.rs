use sea_orm_migration::prelude::*;

/// events_fts：命令日志叙事文本的 FTS5 派生索引（#05/#27 M2）。
///
/// 只服务关键词兜底检索，**不参与权威**：命令日志（commands）仍是唯一事实来源，
/// 删掉这张虚表可从日志重建。用独立列存 save_id / seq 便于按存档过滤与回查。
///
/// 分词用 unicode61：它按空白 / 标点切词，**连续中文会被当成一个整词**（实测
/// SQLite 3.53：原始文档「月光下的古堡」查「古堡」不命中）。
///
/// M3 的修法没有换 tokenizer，而是在写入 / 查询两侧都过 `octopus_engine::text_index`：
/// 写入时把 CJK 逐字用空格分开，查询时把连续 CJK 串拼成 FTS5 短语。这样 unicode61
/// 逐字成词、子串（如「古堡」）即可命中；见 `crates/octopus-engine/src/text_index.rs`。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(\
                 save_id UNINDEXED, seq UNINDEXED, kind UNINDEXED, text, tokenize='unicode61')",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS events_fts")
            .await?;
        Ok(())
    }
}
