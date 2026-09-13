use sea_orm_migration::prelude::*;

/// 摘要派生表（#05 §3.2/§3.3）：回合微摘要 + 场景压缩。
///
/// 它们是**派生数据**，不进命令日志、不参与重放：命令日志仍是唯一事实来源，
/// 删掉这两张表只会让检索少一段记忆，世界状态与 `replay()` 行为不变。
///
/// - `round_summaries`：一回合一条（同存档同回合覆盖），由 `summary` 意图写。
/// - `scene_summaries`：一场景一条（同存档同场景覆盖），`advance_scene` 结算时压缩。
///
/// `id` 自增列只用于给场景摘要编一个**稳定且不与命令 seq 冲突**的派生索引 seq
/// （见 `octopus_engine::storage` 的 seq 编码）；回合摘要的 seq 由 round 直接推导。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS round_summaries (\
                 save_id TEXT NOT NULL, round INTEGER NOT NULL, text TEXT NOT NULL, \
                 PRIMARY KEY (save_id, round))",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS scene_summaries (\
                 id INTEGER PRIMARY KEY AUTOINCREMENT, save_id TEXT NOT NULL, \
                 scene_id TEXT NOT NULL, round INTEGER NOT NULL, text TEXT NOT NULL, \
                 UNIQUE (save_id, scene_id))",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS scene_summaries")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS round_summaries")
            .await?;
        Ok(())
    }
}
