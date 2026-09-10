//! 存储层（#27 应用级单库 SQLite）：故事书 / 存档 / 命令日志 / 维护历史同库。
//! 基于 SeaORM 实现全异步存储与自动建表（auto table creation）。

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use serde_json::Value;

use octopus_types::{
    ArchivedCommandRecord, CommandRecord, MaintenanceRow, SaveDetail, SaveListItem, SavePackage,
};
use migration::{Migrator, MigratorTrait};

use crate::{entities, error::EngineError, seed::seed_storybooks};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

#[derive(Debug, Clone)]
pub struct StorybookRow {
    pub id: String,
    pub title: String,
    pub revision: u32,
    pub draft_version: u32,
    pub updated_at: String,
    pub released_at: Option<String>,
    pub published: bool,
    pub draft: Value,
    pub released: Option<Value>,
}

pub fn normalize_sqlite_url(path: &str) -> String {
    if path == ":memory:" || path == "sqlite::memory:" {
        "sqlite::memory:?cache=shared".to_string()
    } else if path.starts_with("sqlite:") {
        path.to_string()
    } else if path.starts_with('/') {
        format!("sqlite://{path}?mode=rwc")
    } else {
        format!("sqlite:{path}?mode=rwc")
    }
}

#[derive(Clone)]
pub struct SqliteStore {
    db: DatabaseConnection,
}

impl SqliteStore {
    pub async fn open(path: &str) -> Result<Self, EngineError> {
        let url = normalize_sqlite_url(path);
        let mut opt = sea_orm::ConnectOptions::new(url);
        opt.max_connections(5);
        let db = Database::connect(opt).await?;
        Self::run_migrations(&db).await?;
        let store = Self { db };
        store.seed_if_empty().await?;
        Ok(store)
    }

    /// 仅内存（测试 / 冒烟用）。
    pub async fn open_in_memory() -> Result<Self, EngineError> {
        let mut opt = sea_orm::ConnectOptions::new("sqlite::memory:?cache=shared");
        opt.max_connections(1);
        let db = Database::connect(opt).await?;
        Self::run_migrations(&db).await?;
        let store = Self { db };
        store.seed_if_empty().await?;
        Ok(store)
    }

    pub fn conn(&self) -> &DatabaseConnection {
        &self.db
    }

    pub async fn run_migrations(db: &DatabaseConnection) -> Result<(), EngineError> {
        Migrator::up(db, None).await?;
        Ok(())
    }

    pub async fn auto_create_tables(db: &DatabaseConnection) -> Result<(), EngineError> {
        Self::run_migrations(db).await
    }

    async fn seed_if_empty(&self) -> Result<(), EngineError> {
        let count = entities::storybook::Entity::find().count(&self.db).await?;
        if count > 0 {
            return Ok(());
        }
        let now = now_iso();
        for sb in seed_storybooks() {
            let json = serde_json::to_string(&sb.json)?;
            let active = entities::storybook::ActiveModel {
                id: Set(sb.id),
                title: Set(sb.title),
                draft_json: Set(json.clone()),
                released_json: Set(Some(json)),
                revision: Set(sb.revision as i64),
                draft_version: Set(sb.revision as i64),
                updated_at: Set(now.clone()),
                released_at: Set(Some(now.clone())),
            };
            active.insert(&self.db).await?;
        }
        Ok(())
    }

    // ---------- 故事书 ----------

    pub async fn list_storybooks(&self, released_only: bool) -> Result<Vec<Value>, EngineError> {
        let mut query = entities::storybook::Entity::find()
            .order_by_desc(entities::storybook::Column::UpdatedAt);
        if released_only {
            query = query.filter(entities::storybook::Column::ReleasedJson.is_not_null());
        }
        let rows = query.all(&self.db).await?;
        let result = rows
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "revision": r.revision,
                    "draft_version": r.draft_version,
                    "updated_at": r.updated_at,
                    "released_at": r.released_at,
                    "published": r.released_at.is_some(),
                    "description": Value::Null,
                })
            })
            .collect();
        Ok(result)
    }

    pub async fn get_storybook(&self, id: &str) -> Result<Option<StorybookRow>, EngineError> {
        let model = entities::storybook::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?;
        Ok(model.map(|r| StorybookRow {
            id: r.id,
            title: r.title,
            revision: r.revision as u32,
            draft_version: r.draft_version as u32,
            updated_at: r.updated_at,
            released_at: r.released_at,
            published: r.released_json.is_some(),
            draft: serde_json::from_str(&r.draft_json).unwrap_or(Value::Null),
            released: r.released_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        }))
    }

    pub async fn create_storybook_draft(
        &self,
        title: Option<&str>,
        initial: &Value,
    ) -> Result<StorybookRow, EngineError> {
        let id = format!("sb-{}", uuid::Uuid::new_v4().simple());
        let resolved_title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .or_else(|| {
                initial
                    .get("meta")
                    .and_then(|m| m.get("title"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
            })
            .unwrap_or("未命名故事书")
            .to_string();

        let mut draft = initial.clone();
        if let Some(obj) = draft.as_object_mut() {
            if let Some(meta) = obj.get_mut("meta").and_then(|m| m.as_object_mut()) {
                if !meta.contains_key("id") {
                    meta.insert("id".to_string(), Value::String(id.clone()));
                }
                if title.is_some() || !meta.contains_key("title") {
                    meta.insert("title".to_string(), Value::String(resolved_title.clone()));
                }
            }
        }

        let draft_json = serde_json::to_string(&draft)?;
        let now = now_iso();
        let active = entities::storybook::ActiveModel {
            id: Set(id.clone()),
            title: Set(resolved_title.clone()),
            draft_json: Set(draft_json),
            released_json: Set(None),
            revision: Set(0),
            draft_version: Set(1),
            updated_at: Set(now.clone()),
            released_at: Set(None),
        };
        active.insert(&self.db).await?;

        Ok(StorybookRow {
            id,
            title: resolved_title,
            revision: 0,
            draft_version: 1,
            updated_at: now,
            released_at: None,
            published: false,
            draft,
            released: None,
        })
    }

    pub async fn save_draft(
        &self,
        id: &str,
        draft: &Value,
        base_version: u32,
    ) -> Result<StorybookRow, EngineError> {
        let current = self
            .get_storybook(id)
            .await?
            .ok_or_else(|| EngineError::StorybookNotFound(id.to_string()))?;

        if current.draft == *draft {
            return Ok(current);
        }

        if current.draft_version != base_version {
            return Err(EngineError::DraftConflict {
                current_draft_version: current.draft_version,
                updated_at: current.updated_at,
            });
        }

        let new_draft_version = current.draft_version + 1;
        let new_title = draft
            .get("meta")
            .and_then(|m| m.get("title"))
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(&current.title)
            .to_string();
        let now = now_iso();
        let draft_json = serde_json::to_string(draft)?;

        let model = entities::storybook::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?
            .ok_or_else(|| EngineError::StorybookNotFound(id.to_string()))?;

        let mut active: entities::storybook::ActiveModel = model.into();
        active.draft_json = Set(draft_json);
        active.draft_version = Set(new_draft_version as i64);
        active.title = Set(new_title.clone());
        active.updated_at = Set(now.clone());
        active.update(&self.db).await?;

        Ok(StorybookRow {
            id: current.id,
            title: new_title,
            revision: current.revision,
            draft_version: new_draft_version,
            updated_at: now,
            released_at: current.released_at,
            published: current.published,
            draft: draft.clone(),
            released: current.released,
        })
    }

    pub async fn publish_storybook(&self, id: &str, base_version: u32) -> Result<StorybookRow, EngineError> {
        let target_id = id.to_string();
        let result = self
            .db
            .transaction::<_, StorybookRow, EngineError>(|txn| {
                Box::pin(async move {
                    let model = entities::storybook::Entity::find_by_id(target_id.clone())
                        .one(txn)
                        .await?
                        .ok_or_else(|| EngineError::StorybookNotFound(target_id.clone()))?;

                    if (model.draft_version as u32) != base_version {
                        return Err(EngineError::DraftConflict {
                            current_draft_version: model.draft_version as u32,
                            updated_at: model.updated_at,
                        });
                    }

                    let new_revision = model.revision + 1;
                    let new_draft_version = model.draft_version + 1;
                    let now = now_iso();
                    let draft_val: Value = serde_json::from_str(&model.draft_json).unwrap_or(Value::Null);

                    let mut active: entities::storybook::ActiveModel = model.into();
                    let draft_content = active.draft_json.as_ref().clone();
                    active.released_json = Set(Some(draft_content));
                    active.revision = Set(new_revision);
                    active.draft_version = Set(new_draft_version);
                    active.released_at = Set(Some(now.clone()));
                    active.updated_at = Set(now.clone());
                    let updated = active.update(txn).await?;

                    Ok(StorybookRow {
                        id: updated.id,
                        title: updated.title,
                        revision: updated.revision as u32,
                        draft_version: updated.draft_version as u32,
                        updated_at: updated.updated_at,
                        released_at: updated.released_at,
                        published: true,
                        draft: draft_val.clone(),
                        released: Some(draft_val),
                    })
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => EngineError::from(db_err),
                sea_orm::TransactionError::Transaction(engine_err) => engine_err,
            })?;

        Ok(result)
    }

    pub async fn delete_storybook(&self, id: &str) -> Result<bool, EngineError> {
        let res = entities::storybook::Entity::delete_by_id(id.to_string())
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    // ---------- 存档 ----------

    pub async fn insert_save(&self, d: &SaveDetail, auto_confirm: bool) -> Result<(), EngineError> {
        let storybook_json = serde_json::to_string(&d.storybook)?;
        let active = entities::save::ActiveModel {
            id: Set(d.item.id.clone()),
            title: Set(d.item.title.clone()),
            storybook_id: Set(d.item.storybook_id.clone()),
            storybook_title: Set(d.item.storybook_title.clone()),
            embedded_revision: Set(d.item.embedded_revision as i64),
            latest_revision: Set(d.item.latest_revision as i64),
            needs_upgrade: Set(d.item.needs_upgrade),
            imported: Set(d.item.imported.unwrap_or(false)),
            is_sandbox: Set(d.item.is_sandbox.unwrap_or(false)),
            storybook_json: Set(storybook_json),
            auto_confirm: Set(auto_confirm),
            created_at: Set(d.item.created_at.clone()),
            updated_at: Set(d.item.updated_at.clone()),
            last_played_at: Set(d.item.last_played_at.clone()),
        };
        active.insert(&self.db).await?;
        Ok(())
    }

    pub async fn list_saves(&self) -> Result<Vec<SaveListItem>, EngineError> {
        let rows = entities::save::Entity::find()
            .order_by_desc(entities::save::Column::LastPlayedAt)
            .all(&self.db)
            .await?;
        let list = rows
            .into_iter()
            .map(|m| SaveListItem {
                id: m.id,
                title: m.title,
                storybook_id: m.storybook_id,
                storybook_title: m.storybook_title,
                embedded_revision: m.embedded_revision as u32,
                latest_revision: m.latest_revision as u32,
                needs_upgrade: m.needs_upgrade,
                imported: Some(m.imported),
                is_sandbox: Some(m.is_sandbox),
                created_at: m.created_at,
                updated_at: m.updated_at,
                last_played_at: m.last_played_at,
            })
            .collect();
        Ok(list)
    }

    pub async fn get_save(&self, id: &str) -> Result<Option<SaveDetail>, EngineError> {
        let model = entities::save::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?;
        Ok(model.map(|m| {
            let storybook: Value = serde_json::from_str(&m.storybook_json).unwrap_or(Value::Null);
            let item = SaveListItem {
                id: m.id,
                title: m.title,
                storybook_id: m.storybook_id,
                storybook_title: m.storybook_title,
                embedded_revision: m.embedded_revision as u32,
                latest_revision: m.latest_revision as u32,
                needs_upgrade: m.needs_upgrade,
                imported: Some(m.imported),
                is_sandbox: Some(m.is_sandbox),
                created_at: m.created_at,
                updated_at: m.updated_at,
                last_played_at: m.last_played_at,
            };
            SaveDetail { item, storybook }
        }))
    }

    pub async fn rename_save(&self, id: &str, title: &str) -> Result<Option<SaveListItem>, EngineError> {
        let model = entities::save::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?;
        if let Some(m) = model {
            let now = now_iso();
            let mut active: entities::save::ActiveModel = m.into();
            active.title = Set(title.to_string());
            active.updated_at = Set(now);
            let updated = active.update(&self.db).await?;
            Ok(Some(SaveListItem {
                id: updated.id,
                title: updated.title,
                storybook_id: updated.storybook_id,
                storybook_title: updated.storybook_title,
                embedded_revision: updated.embedded_revision as u32,
                latest_revision: updated.latest_revision as u32,
                needs_upgrade: updated.needs_upgrade,
                imported: Some(updated.imported),
                is_sandbox: Some(updated.is_sandbox),
                created_at: updated.created_at,
                updated_at: updated.updated_at,
                last_played_at: updated.last_played_at,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_save(&self, id: &str) -> Result<bool, EngineError> {
        let save_id = id.to_string();
        let res = entities::save::Entity::delete_by_id(save_id.clone())
            .exec(&self.db)
            .await?;
        entities::command::Entity::delete_many()
            .filter(entities::command::Column::SaveId.eq(save_id.clone()))
            .exec(&self.db)
            .await?;
        entities::maintenance::Entity::delete_many()
            .filter(entities::maintenance::Column::SaveId.eq(save_id))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    pub async fn get_auto_confirm(&self, id: &str) -> Result<Option<bool>, EngineError> {
        let model = entities::save::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?;
        Ok(model.map(|m| m.auto_confirm))
    }

    pub async fn set_auto_confirm(&self, id: &str, v: bool) -> Result<(), EngineError> {
        if let Some(m) = entities::save::Entity::find_by_id(id.to_string()).one(&self.db).await? {
            let mut active: entities::save::ActiveModel = m.into();
            active.auto_confirm = Set(v);
            active.updated_at = Set(now_iso());
            active.update(&self.db).await?;
        }
        Ok(())
    }

    pub async fn touch_save(&self, id: &str) -> Result<(), EngineError> {
        if let Some(m) = entities::save::Entity::find_by_id(id.to_string()).one(&self.db).await? {
            let now = now_iso();
            let mut active: entities::save::ActiveModel = m.into();
            active.updated_at = Set(now.clone());
            active.last_played_at = Set(now);
            active.update(&self.db).await?;
        }
        Ok(())
    }

    // ---------- 命令日志 / 维护历史 ----------

    pub async fn append_command(
        &self,
        save_id: &str,
        seq: i64,
        round: i64,
        kind: &str,
        payload_json: &str,
    ) -> Result<(), EngineError> {
        let active = entities::command::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            save_id: Set(save_id.to_string()),
            seq: Set(seq),
            round: Set(round),
            kind: Set(kind.to_string()),
            payload_json: Set(payload_json.to_string()),
            ts: Set(now_iso()),
        };
        active.insert(&self.db).await?;
        Ok(())
    }

    pub async fn append_maintenance(&self, save_id: &str, op: &str, summary: &str) -> Result<(), EngineError> {
        let active = entities::maintenance::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            save_id: Set(save_id.to_string()),
            at: Set(now_iso()),
            op: Set(op.to_string()),
            summary: Set(summary.to_string()),
        };
        active.insert(&self.db).await?;
        Ok(())
    }

    pub async fn list_maintenance(&self, save_id: &str) -> Result<Vec<MaintenanceRow>, EngineError> {
        let rows = entities::maintenance::Entity::find()
            .filter(entities::maintenance::Column::SaveId.eq(save_id.to_string()))
            .order_by_desc(entities::maintenance::Column::Id)
            .limit(100)
            .all(&self.db)
            .await?;
        let list = rows
            .into_iter()
            .map(|m| MaintenanceRow {
                at: m.at,
                op: m.op,
                summary: m.summary,
            })
            .collect();
        Ok(list)
    }

    // ---------- 通用自包含存档包（#27 / 跨数据库导出与导入） ----------

    pub async fn export_save_package(&self, save_id: &str) -> Result<SavePackage, EngineError> {
        let save_detail = self
            .get_save(save_id)
            .await?
            .ok_or_else(|| EngineError::SaveNotFound(save_id.to_string()))?;

        let command_models = entities::command::Entity::find()
            .filter(entities::command::Column::SaveId.eq(save_id.to_string()))
            .order_by_asc(entities::command::Column::Seq)
            .all(&self.db)
            .await?;

        let commands = command_models
            .into_iter()
            .map(|m| CommandRecord {
                seq: m.seq,
                round: m.round,
                kind: m.kind,
                payload: serde_json::from_str(&m.payload_json).unwrap_or(Value::Null),
                ts: m.ts,
            })
            .collect();

        let archived_models = entities::archived_command::Entity::find()
            .filter(entities::archived_command::Column::SaveId.eq(save_id.to_string()))
            .order_by_asc(entities::archived_command::Column::Seq)
            .all(&self.db)
            .await?;

        let archived_commands = archived_models
            .into_iter()
            .map(|m| ArchivedCommandRecord {
                origin_seq: m.origin_seq,
                seq: m.seq,
                round: m.round,
                kind: m.kind,
                payload: serde_json::from_str(&m.payload_json).unwrap_or(Value::Null),
                ts: m.ts,
            })
            .collect();

        let maintenance_models = entities::maintenance::Entity::find()
            .filter(entities::maintenance::Column::SaveId.eq(save_id.to_string()))
            .order_by_asc(entities::maintenance::Column::Id)
            .all(&self.db)
            .await?;

        let maintenance = maintenance_models
            .into_iter()
            .map(|m| MaintenanceRow {
                at: m.at,
                op: m.op,
                summary: m.summary,
            })
            .collect();

        Ok(SavePackage {
            format: "octopus-save-package".to_string(),
            version: 1,
            exported_at: now_iso(),
            save: save_detail,
            commands,
            archived_commands,
            maintenance,
        })
    }

    pub async fn import_save_package(&self, pkg: &SavePackage) -> Result<SaveListItem, EngineError> {
        if pkg.format != "octopus-save-package" {
            return Err(EngineError::Internal("无效的存档包格式，缺少 octopus-save-package 标识".to_string()));
        }

        let exists = self.get_save(&pkg.save.item.id).await?.is_some();
        let (new_id, new_title) = if exists {
            (
                format!("sv-{}", uuid::Uuid::new_v4().simple()),
                format!("{} (导入)", pkg.save.item.title),
            )
        } else {
            (pkg.save.item.id.clone(), pkg.save.item.title.clone())
        };

        let mut detail = pkg.save.clone();
        detail.item.id = new_id.clone();
        detail.item.title = new_title;
        detail.item.imported = Some(true);
        let now = now_iso();
        detail.item.updated_at = now.clone();

        let pkg_commands = pkg.commands.clone();
        let pkg_archived = pkg.archived_commands.clone();
        let pkg_maintenance = pkg.maintenance.clone();
        let detail_to_save = detail.clone();

        self.db
            .transaction::<_, (), EngineError>(|txn| {
                let save_id = new_id.clone();
                let storybook_json =
                    serde_json::to_string(&detail_to_save.storybook).unwrap_or_else(|_| "{}".to_string());
                Box::pin(async move {
                    let save_active = entities::save::ActiveModel {
                        id: Set(detail_to_save.item.id.clone()),
                        title: Set(detail_to_save.item.title.clone()),
                        storybook_id: Set(detail_to_save.item.storybook_id.clone()),
                        storybook_title: Set(detail_to_save.item.storybook_title.clone()),
                        embedded_revision: Set(detail_to_save.item.embedded_revision as i64),
                        latest_revision: Set(detail_to_save.item.latest_revision as i64),
                        needs_upgrade: Set(detail_to_save.item.needs_upgrade),
                        imported: Set(true),
                        is_sandbox: Set(detail_to_save.item.is_sandbox.unwrap_or(false)),
                        storybook_json: Set(storybook_json),
                        auto_confirm: Set(false),
                        created_at: Set(detail_to_save.item.created_at.clone()),
                        updated_at: Set(now.clone()),
                        last_played_at: Set(detail_to_save.item.last_played_at.clone()),
                    };
                    save_active.insert(txn).await?;

                    for cmd in pkg_commands {
                        let payload_json = serde_json::to_string(&cmd.payload)
                            .unwrap_or_else(|_| "{}".to_string());
                        let cmd_active = entities::command::ActiveModel {
                            id: sea_orm::ActiveValue::NotSet,
                            save_id: Set(save_id.clone()),
                            seq: Set(cmd.seq),
                            round: Set(cmd.round),
                            kind: Set(cmd.kind),
                            payload_json: Set(payload_json),
                            ts: Set(cmd.ts),
                        };
                        cmd_active.insert(txn).await?;
                    }

                    for arch in pkg_archived {
                        let payload_json = serde_json::to_string(&arch.payload)
                            .unwrap_or_else(|_| "{}".to_string());
                        let arch_active = entities::archived_command::ActiveModel {
                            id: sea_orm::ActiveValue::NotSet,
                            save_id: Set(save_id.clone()),
                            origin_seq: Set(arch.origin_seq),
                            seq: Set(arch.seq),
                            round: Set(arch.round),
                            kind: Set(arch.kind),
                            payload_json: Set(payload_json),
                            ts: Set(arch.ts),
                        };
                        arch_active.insert(txn).await?;
                    }

                    for m in pkg_maintenance {
                        let m_active = entities::maintenance::ActiveModel {
                            id: sea_orm::ActiveValue::NotSet,
                            save_id: Set(save_id.clone()),
                            at: Set(m.at),
                            op: Set(m.op),
                            summary: Set(m.summary),
                        };
                        m_active.insert(txn).await?;
                    }

                    let import_m = entities::maintenance::ActiveModel {
                        id: sea_orm::ActiveValue::NotSet,
                        save_id: Set(save_id.clone()),
                        at: Set(now.clone()),
                        op: Set("导入存档".to_string()),
                        summary: Set("自自包含存档包导入".to_string()),
                    };
                    import_m.insert(txn).await?;

                    Ok(())
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => EngineError::from(db_err),
                sea_orm::TransactionError::Transaction(engine_err) => engine_err,
            })?;

        Ok(detail.item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_create_and_save_draft() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let initial = json!({
            "meta": { "title": "初始标题" },
            "world": { "premise": "世界设定" }
        });
        let row = store.create_storybook_draft(None, &initial).await.unwrap();
        assert!(row.id.starts_with("sb-"));
        assert_eq!(row.title, "初始标题");
        assert_eq!(row.revision, 0);
        assert_eq!(row.draft_version, 1);
        assert!(!row.published);
        assert!(row.released_at.is_none());
        assert!(row.released.is_none());

        // 保存草稿
        let updated_draft = json!({
            "meta": { "title": "修改后标题" },
            "world": { "premise": "新世界设定" }
        });
        let saved = store.save_draft(&row.id, &updated_draft, 1).await.unwrap();
        assert_eq!(saved.draft_version, 2);
        assert_eq!(saved.title, "修改后标题");
        assert_eq!(saved.draft, updated_draft);
        assert_eq!(saved.revision, 0);

        // 重新获取验证
        let fetched = store.get_storybook(&row.id).await.unwrap().unwrap();
        assert_eq!(fetched.draft_version, 2);
        assert_eq!(fetched.title, "修改后标题");
        assert_eq!(fetched.draft, updated_draft);
    }

    #[tokio::test]
    async fn test_content_idempotency() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let initial = json!({
            "meta": { "title": "幂等测试" },
            "content": 123
        });
        let row = store.create_storybook_draft(None, &initial).await.unwrap();
        assert_eq!(row.draft_version, 1);
        let updated_at = row.updated_at.clone();

        // 传入相同内容保存：no-op，不 bump draft_version，返回当前行
        let saved = store.save_draft(&row.id, &row.draft, 1).await.unwrap();
        assert_eq!(saved.draft_version, 1);
        assert_eq!(saved.updated_at, updated_at);

        // 哪怕 base_version 是旧的/不同的，由于内容完全一致，依旧是 no-op 幂等返回
        let saved2 = store.save_draft(&row.id, &row.draft, 999).await.unwrap();
        assert_eq!(saved2.draft_version, 1);
    }

    #[tokio::test]
    async fn test_optimistic_locking_conflict() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let initial = json!({ "meta": { "title": "锁测试" } });
        let row = store.create_storybook_draft(None, &initial).await.unwrap();

        // 客户端 A 更新成功，version 变为 2
        let draft_a = json!({ "meta": { "title": "A的修改" } });
        let saved_a = store.save_draft(&row.id, &draft_a, 1).await.unwrap();
        assert_eq!(saved_a.draft_version, 2);

        // 客户端 B 仍基于 version 1 提交修改 -> 冲突 409
        let draft_b = json!({ "meta": { "title": "B的修改" } });
        let err = store.save_draft(&row.id, &draft_b, 1).await.unwrap_err();
        match err {
            EngineError::DraftConflict { current_draft_version, updated_at } => {
                assert_eq!(current_draft_version, 2);
                assert_eq!(updated_at, saved_a.updated_at);
            }
            other => panic!("expected DraftConflict, got {:?}", other),
        }

        // 验证草稿内容未被 B 的修改覆盖
        let current = store.get_storybook(&row.id).await.unwrap().unwrap();
        assert_eq!(current.draft_version, 2);
        assert_eq!(current.title, "A的修改");
    }

    #[tokio::test]
    async fn test_atomic_publishing() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let initial = json!({ "meta": { "title": "发布测试" } });
        let row = store.create_storybook_draft(None, &initial).await.unwrap();
        assert_eq!(row.revision, 0);
        assert_eq!(row.draft_version, 1);
        assert!(!row.published);

        // 使用错误的 base_version 发布 -> 冲突
        let err = store.publish_storybook(&row.id, 999).await.unwrap_err();
        assert!(matches!(err, EngineError::DraftConflict { current_draft_version: 1, .. }));

        // 正确发布
        let pub_row = store.publish_storybook(&row.id, 1).await.unwrap();
        assert_eq!(pub_row.revision, 1);
        assert_eq!(pub_row.draft_version, 2);
        assert!(pub_row.published);
        assert!(pub_row.released_at.is_some());
        assert_eq!(pub_row.released.as_ref(), Some(&pub_row.draft));

        // 再次发布同一 base_version 失败（已更新到 2）
        let err2 = store.publish_storybook(&row.id, 1).await.unwrap_err();
        assert!(matches!(err2, EngineError::DraftConflict { current_draft_version: 2, .. }));

        // 发布后从 DB 获取验证
        let fetched = store.get_storybook(&row.id).await.unwrap().unwrap();
        assert_eq!(fetched.revision, 1);
        assert_eq!(fetched.draft_version, 2);
        assert!(fetched.published);
        assert_eq!(fetched.released, Some(fetched.draft));
    }

    #[tokio::test]
    async fn test_delete_storybook() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let row = store.create_storybook_draft(Some("待删除"), &json!({})).await.unwrap();
        assert!(store.get_storybook(&row.id).await.unwrap().is_some());

        assert!(store.delete_storybook(&row.id).await.unwrap());
        assert!(!store.delete_storybook(&row.id).await.unwrap());
        assert!(store.get_storybook(&row.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_saves_and_commands_lifecycle() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let saves = store.list_saves().await.unwrap();
        assert_eq!(saves.len(), 0);

        let detail = SaveDetail {
            item: SaveListItem {
                id: "save-1".to_string(),
                title: "测试存档".to_string(),
                storybook_id: "sb-1".to_string(),
                storybook_title: "故事书1".to_string(),
                embedded_revision: 1,
                latest_revision: 1,
                needs_upgrade: false,
                imported: Some(false),
                is_sandbox: Some(false),
                created_at: now_iso(),
                updated_at: now_iso(),
                last_played_at: now_iso(),
            },
            storybook: json!({ "meta": { "title": "故事书1" } }),
        };

        store.insert_save(&detail, true).await.unwrap();
        let fetched = store.get_save("save-1").await.unwrap().unwrap();
        assert_eq!(fetched.item.title, "测试存档");
        assert_eq!(fetched.item.is_sandbox, Some(false));
        assert_eq!(store.get_auto_confirm("save-1").await.unwrap(), Some(true));

        store.set_auto_confirm("save-1", false).await.unwrap();
        assert_eq!(store.get_auto_confirm("save-1").await.unwrap(), Some(false));

        let renamed = store.rename_save("save-1", "新存档名").await.unwrap().unwrap();
        assert_eq!(renamed.title, "新存档名");
        assert_eq!(renamed.is_sandbox, Some(false));

        store.append_command("save-1", 1, 1, "test", "{}").await.unwrap();
        store.append_maintenance("save-1", "手动存档", "已保存").await.unwrap();
        let m = store.list_maintenance("save-1").await.unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].op, "手动存档");

        assert!(store.delete_save("save-1").await.unwrap());
        assert!(store.get_save("save-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_export_and_import_save_package() {
        let store1 = SqliteStore::open_in_memory().await.unwrap();
        let detail = SaveDetail {
            item: SaveListItem {
                id: "sv-test-pkg".to_string(),
                title: "通用存档包测试".to_string(),
                storybook_id: "sb-1".to_string(),
                storybook_title: "测试故事书".to_string(),
                embedded_revision: 1,
                latest_revision: 1,
                needs_upgrade: false,
                imported: Some(false),
                is_sandbox: Some(true),
                created_at: now_iso(),
                updated_at: now_iso(),
                last_played_at: now_iso(),
            },
            storybook: json!({ "meta": { "title": "测试故事书" } }),
        };
        store1.insert_save(&detail, false).await.unwrap();
        store1
            .append_command("sv-test-pkg", 1, 1, "round_start", "{\"input\":\"hello\"}")
            .await
            .unwrap();
        store1
            .append_maintenance("sv-test-pkg", "手动存档", "快照备份")
            .await
            .unwrap();

        // 导出
        let pkg = store1.export_save_package("sv-test-pkg").await.unwrap();
        assert_eq!(pkg.format, "octopus-save-package");
        assert_eq!(pkg.version, 1);
        assert_eq!(pkg.save.item.title, "通用存档包测试");
        assert_eq!(pkg.commands.len(), 1);
        assert_eq!(pkg.commands[0].kind, "round_start");
        assert_eq!(pkg.maintenance.len(), 1);

        // 导入到全新实例 store2
        let store2 = SqliteStore::open_in_memory().await.unwrap();
        let imported_item = store2.import_save_package(&pkg).await.unwrap();
        assert_eq!(imported_item.id, "sv-test-pkg");
        assert_eq!(imported_item.imported, Some(true));

        let fetched = store2.get_save("sv-test-pkg").await.unwrap().unwrap();
        assert_eq!(fetched.item.title, "通用存档包测试");
        let m2 = store2.list_maintenance("sv-test-pkg").await.unwrap();
        assert_eq!(m2.len(), 2); // 原维护记录 + 导入操作记录

        // 再次导入到 store2 -> 触发碰撞改名
        let imported_again = store2.import_save_package(&pkg).await.unwrap();
        assert_ne!(imported_again.id, "sv-test-pkg");
        assert!(imported_again.title.contains("(导入)"));
    }
}
