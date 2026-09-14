pub use sea_orm_migration::prelude::*;

mod m20260910_000001_create_initial_tables;
mod m20260910_000002_add_is_sandbox_to_saves;
mod m20260910_000003_add_request_id_to_commands;
mod m20260910_000004_create_pair_messages;
mod m20260910_000005_create_pair_threads;
mod m20260910_000006_add_refs_to_pair_messages;
mod m20260910_000007_add_pending_suggestions_to_pair_threads;
mod m20260912_000008_add_reasoning_to_pair_messages;
mod m20260913_000009_add_model_to_saves;
mod m20260913_000010_add_reasoning_effort_to_saves;
mod m20260913_000011_add_attachments_to_pair_messages;
mod m20260914_000012_add_narrative_to_saves;
mod m20260914_000013_add_roles_to_saves;
mod m20260914_000014_add_events_fts;
mod m20260914_000015_add_summaries;
mod m20260915_000016_add_legacy_to_saves;
mod m20260916_000017_create_snapshots;
mod m20260917_000018_drop_roles_from_saves;
mod m20260918_000019_create_ai_conversations;
mod m20260919_000020_add_compaction_to_pair_threads;
mod m20260920_000021_create_accounts;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260910_000001_create_initial_tables::Migration),
            Box::new(m20260910_000002_add_is_sandbox_to_saves::Migration),
            Box::new(m20260910_000003_add_request_id_to_commands::Migration),
            Box::new(m20260910_000004_create_pair_messages::Migration),
            Box::new(m20260910_000005_create_pair_threads::Migration),
            Box::new(m20260910_000006_add_refs_to_pair_messages::Migration),
            Box::new(m20260910_000007_add_pending_suggestions_to_pair_threads::Migration),
            Box::new(m20260912_000008_add_reasoning_to_pair_messages::Migration),
            Box::new(m20260913_000009_add_model_to_saves::Migration),
            Box::new(m20260913_000010_add_reasoning_effort_to_saves::Migration),
            Box::new(m20260913_000011_add_attachments_to_pair_messages::Migration),
            Box::new(m20260914_000012_add_narrative_to_saves::Migration),
            Box::new(m20260914_000013_add_roles_to_saves::Migration),
            Box::new(m20260914_000014_add_events_fts::Migration),
            Box::new(m20260914_000015_add_summaries::Migration),
            Box::new(m20260915_000016_add_legacy_to_saves::Migration),
            Box::new(m20260916_000017_create_snapshots::Migration),
            Box::new(m20260917_000018_drop_roles_from_saves::Migration),
            Box::new(m20260918_000019_create_ai_conversations::Migration),
            Box::new(m20260919_000020_add_compaction_to_pair_threads::Migration),
            Box::new(m20260920_000021_create_accounts::Migration),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 000021：账户两张表 + saves/storybooks 的归属列（列可空，回填在引擎侧）。
    #[tokio::test]
    async fn migration_creates_accounts_and_owner_columns() {
        let path = std::env::temp_dir().join(format!(
            "octopus-migrate-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // 与引擎一致：绝对路径 + mode=rwc（缺了 rwc，新建文件会报 unable to open）。
        let mut opt = sea_orm::ConnectOptions::new(format!("sqlite://{}?mode=rwc", path.display()));
        opt.max_connections(1);
        let db = sea_orm::Database::connect(opt).await.expect("打开临时库");
        Migrator::up(&db, None).await.expect("跑全部迁移");

        for sql in [
            "SELECT id, username, display_name, password_hash, is_admin, must_change_password, \
             created_at, updated_at, last_login_at FROM users LIMIT 0",
            "SELECT token_hash, user_id, created_at, expires_at, last_seen_at, user_agent \
             FROM sessions LIMIT 0",
            "SELECT owner_id FROM saves LIMIT 0",
            "SELECT owner_id FROM storybooks LIMIT 0",
        ] {
            db.query_all(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                sql.to_string(),
            ))
            .await
            .unwrap_or_else(|e| panic!("000021 后应能查询：{sql}\n{e}"));
        }

        let _ = std::fs::remove_file(&path);
    }

    /// 回填：000013 之前只写了旧 model_* 的存档，升级后应得到 mode=shared 的 roles_json
    /// （主线与角色同模型），保证旧存档升级后行为不变。
    #[tokio::test]
    async fn migration_backfills_legacy_model_into_roles() {
        let mut opt = sea_orm::ConnectOptions::new("sqlite::memory:?cache=shared");
        opt.max_connections(1);
        let db = sea_orm::Database::connect(opt).await.expect("打开内存库");
        // 先只跑到 000012，模拟 000013 之前的旧库。
        Migrator::up(&db, Some(12)).await.expect("跑前 12 个迁移");
        db.execute_unprepared(
            concat!(
                "INSERT INTO saves ",
                "(id, title, storybook_id, storybook_title, embedded_revision, latest_revision, ",
                "needs_upgrade, imported, is_sandbox, storybook_json, auto_confirm, ",
                "model_provider_id, model, reasoning_effort, narrative_json, ",
                "created_at, updated_at, last_played_at) ",
                "VALUES ('sv-old', 't', 'sb-1', 'sb', 1, 1, 0, 0, 0, '{}', 0, ",
                "'deepseek', 'deepseek-v4-pro', 'high', NULL, 'now', 'now', 'now')",
            ),
        )
        .await
        .expect("插入旧存档");

        // 在 000012 的基础上再应用一个（= 000013）：验证新增 + 回填。
        // 更后的 000018 会删掉 roles_json，所以这里不能用 None。
        Migrator::up(&db, Some(1)).await.expect("应用 000013");

        let rows = db
            .query_all(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT roles_json FROM saves WHERE id = 'sv-old'".to_string(),
            ))
            .await
            .expect("读回填结果");
        let json: String = rows[0].try_get("", "roles_json").expect("roles_json 已回填");
        let v: serde_json::Value = serde_json::from_str(&json).expect("合法 JSON");
        assert_eq!(v["mode"], "shared");
        assert_eq!(v["story"]["provider_id"], "deepseek");
        assert_eq!(v["story"]["model"], "deepseek-v4-pro");
        assert_eq!(v["story"]["reasoning_effort"], "high");
        assert_eq!(v["character"], v["story"], "shared 时两份模型一致");
    }
}
