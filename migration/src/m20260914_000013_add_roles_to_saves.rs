use sea_orm_migration::prelude::*;

/// saves 表新增 roles_json：存档级模型分工（shared / split + 两份 RoleModel）。
///
/// 这是存档设置而非事件：只影响之后的回合，不参与命令日志重放。
/// 迁移时把旧的 model_* 三列回填成 mode=shared 的 roles_json，让既有存档行为不变；
/// 旧列保留一版，读取顺序 = roles_json → 旧 model_*（同时套到两个角色）→ 全局默认。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::RolesJson).text().null())
                    .to_owned(),
            )
            .await?;

        // 回填：有旧模型覆盖的存档写成 shared（主线与角色同模型），保证升级后行为与今天一致。
        // 用 Rust 侧构造 JSON 再带参写回，避免依赖各数据库的 JSON 函数 / 字符串转义差异。
        let conn = manager.get_connection();
        let backend = conn.get_database_backend();
        let rows = conn
            .query_all(sea_orm::Statement::from_string(
                backend,
                "SELECT id, model_provider_id, model, reasoning_effort FROM saves \
                 WHERE model_provider_id IS NOT NULL AND model IS NOT NULL"
                    .to_string(),
            ))
            .await?;
        for row in rows {
            let id: String = row.try_get("", "id")?;
            let provider_id: String = row.try_get("", "model_provider_id")?;
            let model: String = row.try_get("", "model")?;
            let effort: Option<String> = row.try_get("", "reasoning_effort")?;
            let role = serde_json::json!({
                "provider_id": provider_id,
                "model": model,
                "reasoning_effort": effort,
            });
            let roles = serde_json::json!({
                "mode": "shared",
                "story": role,
                "character": role,
            });
            conn.execute(sea_orm::Statement::from_sql_and_values(
                backend,
                "UPDATE saves SET roles_json = ? WHERE id = ?",
                [roles.to_string().into(), id.into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::RolesJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    RolesJson,
}
