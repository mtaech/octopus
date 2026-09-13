use sea_orm_migration::prelude::*;

/// snapshots 表（#06 ②）：世界状态的全量物化检查点，只作启动加速缓存。
///
/// 设计定位：快照**不是**权威事实，任何一份都能从命令日志重放重建；日志永不删，
/// 快照丢了 / 过期了就直接回退全量重放。因此这里只存「派生用的物化状态 + 元信息」。
///
/// - seq：该快照对应的权威命令序号；启动时只重放 seq 之后的命令。
/// - format_version：快照格式版本；引擎升级后旧版本直接作废（回退全量重放）。
/// - storybook_revision：写入时的内嵌故事书版次；版次升级会让旧快照与新故事书不一致。
/// - state_json：OriginCheckpoint 形态（WorldState + 场景压缩左界 + RNG 位置）。
///
/// UNIQUE(save_id, seq) 让「同一序号重复手动存档」覆盖而非堆积；保留策略（最新 N 份）
/// 由写入方在同一事务内执行。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS snapshots (\
                 id INTEGER PRIMARY KEY AUTOINCREMENT, save_id TEXT NOT NULL, \
                 seq INTEGER NOT NULL, taken_at TEXT NOT NULL, \
                 format_version INTEGER NOT NULL, storybook_revision INTEGER NOT NULL, \
                 state_json TEXT NOT NULL, UNIQUE (save_id, seq))",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS idx_snapshots_save_seq ON snapshots(save_id, seq DESC)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS snapshots")
            .await?;
        Ok(())
    }
}
