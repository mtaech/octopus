//! 存储层（#27 应用级单库 SQLite）：故事书 / 存档 / 命令日志 / 维护历史同库。
//! 基于 SeaORM 实现全异步存储与自动建表（auto table creation）。

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use serde_json::Value;

use octopus_types::{
    ArchivedCommandRecord, CommandRecord, EventEnvelope, MaintenanceRow, NarrativeOverride, PlayEvent,
    SaveDetail, SaveListItem, SavePackage,
};
use migration::{Migrator, MigratorTrait};

use crate::{entities, error::EngineError, seed::seed_storybooks};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

/// 从命令日志读回的一条事件，附带该条记录的幂等请求 id。
#[derive(Debug, Clone)]
pub struct PersistedEvent {
    pub request_id: Option<String>,
    pub envelope: EventEnvelope,
}

/// 待写入的结对会话消息（seq 由存储层分配）。
#[derive(Debug, Clone)]
pub struct NewPairMessage {
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub is_error: bool,
    pub tools: Option<Value>,
    /// 该消息显式引用的实体（JSON 数组）。
    pub refs: Option<Value>,
    /// 仅 assistant：思考流正文。
    pub reasoning: Option<String>,
    /// user 消息的附件（JSON 数组）。
    pub attachments: Option<Value>,
}

/// 结对会话线程。
#[derive(Debug, Clone)]
pub struct PairThreadRow {
    pub id: String,
    pub storybook_id: String,
    pub title: String,
    pub message_count: i64,
    pub created_at: String,
    pub updated_at: String,
    /// 尚未处理的待审查改动（JSON 数组）。
    pub pending_suggestions: Option<Value>,
}

/// 由首条用户消息派生会话标题（单行、截断到 20 字符）。
fn derive_thread_title(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out = String::new();
    for (i, ch) in line.chars().enumerate() {
        if i >= 20 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// 读回的结对会话消息。
#[derive(Debug, Clone)]
pub struct PairMessageRow {
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub is_error: bool,
    pub tools: Option<Value>,
    pub refs: Option<Value>,
    pub reasoning: Option<String>,
    pub attachments: Option<Value>,
}

/// 事件类型标签（写入 `commands.kind`，便于按类型检索与调试）。
pub fn event_kind(event: &PlayEvent) -> &'static str {
    match event {
        PlayEvent::Scene(_) => "scene",
        PlayEvent::Narrate(_) => "narrate",
        PlayEvent::Dialogue(_) => "dialogue",
        PlayEvent::Emote(_) => "emote",
        PlayEvent::Pending(_) => "pending",
        PlayEvent::CheckResult(_) => "check_result",
        PlayEvent::Resolution(_) => "resolution",
        PlayEvent::StateUpdate(_) => "state_update",
        PlayEvent::Phase(_) => "phase",
        PlayEvent::RoundStart(_) => "round_start",
        PlayEvent::RoundEnd(_) => "round_end",
        PlayEvent::System(_) => "system",
        PlayEvent::Reasoning(_) => "reasoning",
    }
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
                // 列表卡要的元信息取自「已发布版次优先、否则草稿」的 meta。
                // 注意：description 以前写死为 Null，列表页因此永远显示「尚未填写简介」。
                let meta = r
                    .released_json
                    .as_deref()
                    .or(Some(r.draft_json.as_str()))
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .and_then(|v| v.get("meta").cloned())
                    .unwrap_or(Value::Null);
                let description = meta.get("description").cloned().unwrap_or(Value::Null);
                let cover = meta.get("cover").cloned().unwrap_or(Value::Null);
                // 内容评级（P3）：缺省 sfw，仅给列表徽标用；不参与任何过滤 / 校验分支。
                let rating = meta.get("rating").cloned().unwrap_or(Value::Null);
                serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "revision": r.revision,
                    "draft_version": r.draft_version,
                    "updated_at": r.updated_at,
                    "released_at": r.released_at,
                    "published": r.released_at.is_some(),
                    "description": description,
                    "cover": cover,
                    "rating": rating,
                })
            })
            .collect();
        Ok(result)
    }

    pub async fn get_storybook(&self, id: &str) -> Result<Option<StorybookRow>, EngineError> {
        let model = entities::storybook::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?;
        Ok(model.map(|r| {
            let mut draft: Value = serde_json::from_str(&r.draft_json).unwrap_or(Value::Null);
            let mut released: Option<Value> = r
                .released_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());
            // 读旧格式时升格（历史数据本身不改写）
            crate::upcast::upcast_storybook(&mut draft);
            if let Some(rel) = released.as_mut() {
                crate::upcast::upcast_storybook(rel);
            }
            StorybookRow {
                id: r.id,
                title: r.title,
                revision: r.revision as u32,
                draft_version: r.draft_version as u32,
                updated_at: r.updated_at,
                released_at: r.released_at,
                published: r.released_json.is_some(),
                draft,
                released,
            }
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
                    let mut draft_val: Value =
                        serde_json::from_str(&model.draft_json).unwrap_or(Value::Null);
                    // 发布即生成新快照：写入前升格，避免新存档内嵌旧字段
                    crate::upcast::upcast_storybook(&mut draft_val);
                    let draft_content =
                        serde_json::to_string(&draft_val).unwrap_or_else(|_| "{}".to_string());

                    let mut active: entities::storybook::ActiveModel = model.into();
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
        // 结对会话随故事书一并清理（消息 → 线程）。
        entities::pair_message::Entity::delete_many()
            .filter(entities::pair_message::Column::StorybookId.eq(id.to_string()))
            .exec(&self.db)
            .await?;
        entities::pair_thread::Entity::delete_many()
            .filter(entities::pair_thread::Column::StorybookId.eq(id.to_string()))
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
            model_provider_id: Set(None),
            model: Set(None),
            reasoning_effort: Set(None),
            narrative_json: Set(None),
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
            let mut storybook: Value = serde_json::from_str(&m.storybook_json).unwrap_or(Value::Null);
            // 存档内嵌冻结故事书：读旧格式时升格
            crate::upcast::upcast_storybook(&mut storybook);
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

    /// 本存档的模型（provider id, model id, reasoning_effort）；未设置返回 (None, None, None)。
    pub async fn get_save_model(
        &self,
        id: &str,
    ) -> Result<(Option<String>, Option<String>, Option<String>), EngineError> {
        let model = entities::save::Entity::find_by_id(id.to_string()).one(&self.db).await?;
        Ok(model
            .map(|m| (m.model_provider_id, m.model, m.reasoning_effort))
            .unwrap_or((None, None, None)))
    }

    /// 设置本存档的模型；传 None 表示回落到全局默认。
    pub async fn set_save_model(
        &self,
        id: &str,
        provider_id: Option<&str>,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> Result<(), EngineError> {
        if let Some(m) = entities::save::Entity::find_by_id(id.to_string()).one(&self.db).await? {
            let mut active: entities::save::ActiveModel = m.into();
            active.model_provider_id = Set(provider_id.map(str::to_string));
            active.model = Set(model.map(str::to_string));
            active.reasoning_effort = Set(reasoning_effort.map(str::to_string));
            active.updated_at = Set(now_iso());
            active.update(&self.db).await?;
        }
        Ok(())
    }

    /// 本存档的叙述段玩家偏好；无记录 / 解析失败返回 None（等价于全用故事书默认）。
    /// 只读存档设置，绝不触碰命令日志——历史条目无法被偏好改写。
    pub async fn get_save_narrative(
        &self,
        id: &str,
    ) -> Result<Option<std::collections::BTreeMap<String, NarrativeOverride>>, EngineError> {
        let model = entities::save::Entity::find_by_id(id.to_string()).one(&self.db).await?;
        Ok(model
            .and_then(|m| m.narrative_json)
            .and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// 保存本存档的叙述段玩家偏好。传空 map 也照存（表示玩家显式清空）。
    pub async fn set_save_narrative(
        &self,
        id: &str,
        overrides: &std::collections::BTreeMap<String, NarrativeOverride>,
    ) -> Result<(), EngineError> {
        if let Some(m) = entities::save::Entity::find_by_id(id.to_string()).one(&self.db).await? {
            let json = serde_json::to_string(overrides).unwrap_or_else(|_| "{}".to_string());
            let mut active: entities::save::ActiveModel = m.into();
            active.narrative_json = Set(Some(json));
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
            request_id: sea_orm::ActiveValue::NotSet,
        };
        active.insert(&self.db).await?;
        Ok(())
    }

    /// 把 `seq >= from_seq` 的命令归档到只读归档表，并从活日志删除。
    /// 「重跑本轮」用：被舍弃的旧回合作为分支保留（origin_seq = from_seq），
    /// 活日志截断到回合起点之前，重放即得到回合前的世界状态。
    pub async fn archive_commands_from(
        &self,
        save_id: &str,
        from_seq: u64,
    ) -> Result<u64, EngineError> {
        let from = from_seq as i64;
        let txn = self.db.begin().await?;
        let rows = entities::command::Entity::find()
            .filter(entities::command::Column::SaveId.eq(save_id.to_string()))
            .filter(entities::command::Column::Seq.gte(from))
            .all(&txn)
            .await?;
        let count = rows.len() as u64;
        for m in &rows {
            let active = entities::archived_command::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                save_id: Set(m.save_id.clone()),
                origin_seq: Set(from),
                seq: Set(m.seq),
                round: Set(m.round),
                kind: Set(m.kind.clone()),
                payload_json: Set(m.payload_json.clone()),
                ts: Set(m.ts.clone()),
            };
            active.insert(&txn).await?;
        }
        entities::command::Entity::delete_many()
            .filter(entities::command::Column::SaveId.eq(save_id.to_string()))
            .filter(entities::command::Column::Seq.gte(from))
            .exec(&txn)
            .await?;
        txn.commit().await?;
        Ok(count)
    }

    /// 追加一条演出事件到命令日志（权威条目，payload 为完整 `EventEnvelope` JSON）。
    pub async fn append_event(
        &self,
        save_id: &str,
        env: &EventEnvelope,
    ) -> Result<(), EngineError> {
        let request_id = match &env.event {
            PlayEvent::RoundStart(p) => p.request_id.clone(),
            _ => None,
        };
        let payload_json = serde_json::to_string(env)?;
        let active = entities::command::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            save_id: Set(save_id.to_string()),
            seq: Set(env.seq as i64),
            round: Set(env.round as i64),
            kind: Set(event_kind(&env.event).to_string()),
            payload_json: Set(payload_json),
            ts: Set(env.ts.clone()),
            request_id: Set(request_id),
        };
        active.insert(&self.db).await?;
        Ok(())
    }

    /// 读回一个存档的完整事件日志（按 seq 升序）。无法解析为 `EventEnvelope` 的
    /// 历史行（旧格式 / 测试桩）会被跳过，保证重放不因脏数据整体失败。
    pub async fn load_events(&self, save_id: &str) -> Result<Vec<PersistedEvent>, EngineError> {
        let rows = entities::command::Entity::find()
            .filter(entities::command::Column::SaveId.eq(save_id.to_string()))
            .order_by_asc(entities::command::Column::Seq)
            .all(&self.db)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for m in rows {
            match serde_json::from_str::<EventEnvelope>(&m.payload_json) {
                Ok(mut envelope) => {
                    // 旧格式日志：EntityRef.kind 仍是 beat 时升格为 trigger
                    crate::upcast::upcast_event(&mut envelope);
                    // 旧存档包导入时 request_id 列可能为空，回落到事件负载里的值。
                    let embedded = match &envelope.event {
                        PlayEvent::RoundStart(p) => p.request_id.clone(),
                        _ => None,
                    };
                    out.push(PersistedEvent {
                        request_id: m.request_id.or(embedded),
                        envelope,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        save_id = %save_id,
                        seq = m.seq,
                        error = %err,
                        "命令日志存在无法解析的条目，已跳过"
                    );
                }
            }
        }
        Ok(out)
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

    // ---------- 结对会话（#23 ④）：一本故事书可有多条按主题隔离的线程 ----------

    async fn pair_message_count(&self, thread_id: &str) -> Result<i64, EngineError> {
        let n = entities::pair_message::Entity::find()
            .filter(entities::pair_message::Column::ThreadId.eq(thread_id.to_string()))
            .count(&self.db)
            .await?;
        Ok(n as i64)
    }

    fn thread_row(m: entities::pair_thread::Model, message_count: i64) -> PairThreadRow {
        PairThreadRow {
            id: m.id,
            storybook_id: m.storybook_id,
            title: m.title,
            message_count,
            created_at: m.created_at,
            updated_at: m.updated_at,
            pending_suggestions: m
                .pending_suggestions_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok()),
        }
    }

    /// 覆盖线程的待审查改动；None / null 表示清空。
    pub async fn set_pair_thread_pending(
        &self,
        thread_id: &str,
        suggestions: Option<Value>,
    ) -> Result<bool, EngineError> {
        let Some(m) = entities::pair_thread::Entity::find_by_id(thread_id.to_string())
            .one(&self.db)
            .await?
        else {
            return Ok(false);
        };
        let json = match suggestions {
            Some(v) if !v.is_null() => Some(serde_json::to_string(&v)?),
            _ => None,
        };
        let mut active: entities::pair_thread::ActiveModel = m.into();
        active.pending_suggestions_json = Set(json);
        active.update(&self.db).await?;
        Ok(true)
    }

    pub async fn list_pair_threads(
        &self,
        storybook_id: &str,
    ) -> Result<Vec<PairThreadRow>, EngineError> {
        let rows = entities::pair_thread::Entity::find()
            .filter(entities::pair_thread::Column::StorybookId.eq(storybook_id.to_string()))
            .order_by_desc(entities::pair_thread::Column::UpdatedAt)
            .all(&self.db)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for m in rows {
            let id = m.id.clone();
            let count = self.pair_message_count(&id).await?;
            out.push(Self::thread_row(m, count));
        }
        Ok(out)
    }

    pub async fn get_pair_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<PairThreadRow>, EngineError> {
        let m = entities::pair_thread::Entity::find_by_id(thread_id.to_string())
            .one(&self.db)
            .await?;
        match m {
            Some(m) => {
                let count = self.pair_message_count(thread_id).await?;
                Ok(Some(Self::thread_row(m, count)))
            }
            None => Ok(None),
        }
    }

    /// 新建一条会话线程。标题缺省为「新会话」，首条用户消息落下后自动补标题。
    pub async fn create_pair_thread(
        &self,
        storybook_id: &str,
        title: Option<&str>,
    ) -> Result<PairThreadRow, EngineError> {
        let now = now_iso();
        let resolved = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("新会话")
            .to_string();
        let active = entities::pair_thread::ActiveModel {
            id: Set(format!("pt-{}", uuid::Uuid::new_v4().simple())),
            storybook_id: Set(storybook_id.to_string()),
            title: Set(resolved.clone()),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
            pending_suggestions_json: Set(None),
        };
        let inserted = active.insert(&self.db).await?;
        Ok(PairThreadRow {
            id: inserted.id,
            storybook_id: storybook_id.to_string(),
            title: resolved,
            message_count: 0,
            created_at: now.clone(),
            updated_at: now,
            pending_suggestions: None,
        })
    }

    pub async fn rename_pair_thread(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<Option<PairThreadRow>, EngineError> {
        let Some(m) = entities::pair_thread::Entity::find_by_id(thread_id.to_string())
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let mut active: entities::pair_thread::ActiveModel = m.into();
        active.title = Set(title.to_string());
        active.updated_at = Set(now_iso());
        let updated = active.update(&self.db).await?;
        let count = self.pair_message_count(thread_id).await?;
        Ok(Some(Self::thread_row(updated, count)))
    }

    pub async fn delete_pair_thread(&self, thread_id: &str) -> Result<bool, EngineError> {
        entities::pair_message::Entity::delete_many()
            .filter(entities::pair_message::Column::ThreadId.eq(thread_id.to_string()))
            .exec(&self.db)
            .await?;
        let res = entities::pair_thread::Entity::delete_by_id(thread_id.to_string())
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    pub async fn list_pair_messages(
        &self,
        thread_id: &str,
    ) -> Result<Vec<PairMessageRow>, EngineError> {
        let rows = entities::pair_message::Entity::find()
            .filter(entities::pair_message::Column::ThreadId.eq(thread_id.to_string()))
            .order_by_asc(entities::pair_message::Column::Seq)
            .all(&self.db)
            .await?;
        Ok(rows
            .into_iter()
            .map(|m| PairMessageRow {
                seq: m.seq,
                role: m.role,
                content: m.content,
                model: m.model,
                is_error: m.is_error,
                tools: m.tools_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
                refs: m.refs_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
                reasoning: m.reasoning,
                attachments: m.attachments_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
            })
            .collect())
    }

    pub async fn append_pair_messages(
        &self,
        thread_id: &str,
        msgs: &[NewPairMessage],
    ) -> Result<(), EngineError> {
        if msgs.is_empty() {
            return Ok(());
        }
        let thread = entities::pair_thread::Entity::find_by_id(thread_id.to_string())
            .one(&self.db)
            .await?
            .ok_or_else(|| EngineError::Internal(format!("结对线程不存在：{thread_id}")))?;

        let existing = self.pair_message_count(thread_id).await?;
        let last = entities::pair_message::Entity::find()
            .filter(entities::pair_message::Column::ThreadId.eq(thread_id.to_string()))
            .order_by_desc(entities::pair_message::Column::Seq)
            .one(&self.db)
            .await?;
        let mut seq = last.map(|m| m.seq).unwrap_or(0);
        for m in msgs {
            seq += 1;
            let tools_json = match &m.tools {
                Some(v) => Some(serde_json::to_string(v)?),
                None => None,
            };
            let refs_json = match &m.refs {
                Some(v) => Some(serde_json::to_string(v)?),
                None => None,
            };
            let attachments_json = match &m.attachments {
                Some(v) => Some(serde_json::to_string(v)?),
                None => None,
            };
            let active = entities::pair_message::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                thread_id: Set(Some(thread_id.to_string())),
                storybook_id: Set(thread.storybook_id.clone()),
                seq: Set(seq),
                role: Set(m.role.clone()),
                content: Set(m.content.clone()),
                model: Set(m.model.clone()),
                is_error: Set(m.is_error),
                tools_json: Set(tools_json),
                refs_json: Set(refs_json),
                reasoning: Set(m.reasoning.clone()),
                attachments_json: Set(attachments_json),
                created_at: Set(now_iso()),
            };
            active.insert(&self.db).await?;
        }

        // 首条用户消息自动补标题（仅当标题仍是默认值时），并刷新 updated_at 用于排序。
        let mut new_title = thread.title.clone();
        let is_default = thread.title.is_empty()
            || thread.title == "新会话"
            || thread.title.starts_with("对话 ");
        if existing == 0 && is_default {
            if let Some(first) = msgs.iter().find(|m| m.role == "user") {
                let derived = derive_thread_title(&first.content);
                if !derived.is_empty() {
                    new_title = derived;
                }
            }
        }
        let mut active: entities::pair_thread::ActiveModel = thread.into();
        active.title = Set(new_title);
        active.updated_at = Set(now_iso());
        active.update(&self.db).await?;
        Ok(())
    }

    pub async fn clear_pair_messages(&self, thread_id: &str) -> Result<u64, EngineError> {
        let res = entities::pair_message::Entity::delete_many()
            .filter(entities::pair_message::Column::ThreadId.eq(thread_id.to_string()))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected)
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
        // 导入旧存档包时升格内嵌故事书
        crate::upcast::upcast_storybook(&mut detail.storybook);
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
                        model_provider_id: Set(None),
                        model: Set(None),
                        reasoning_effort: Set(None),
                        narrative_json: Set(None),
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
                            request_id: Set(None),
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
    async fn test_get_storybook_upcasts_legacy_beats() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        // 旧格式（v1）：骨架用 beats，条件用 beat_fired / beat_id
        let legacy = json!({
            "schema_version": 1,
            "meta": { "id": "sb-legacy", "title": "旧书" },
            "skeleton": [{ "id": "ch-1", "title": "第一章", "scenes": [{
                "id": "sc-1",
                "title": "场景",
                "beats": [{ "id": "b1", "title": "触发点", "hint": "提示" }],
                "goals": [{ "id": "g1", "text": "目标", "condition": { "op": "beat_fired", "beat_id": "b1" } }]
            }] }]
        });
        let row = store.create_storybook_draft(None, &legacy).await.unwrap();

        // 读回来即为当前结构
        let fetched = store.get_storybook(&row.id).await.unwrap().unwrap();
        let scene = &fetched.draft["skeleton"][0]["scenes"][0];
        assert!(scene.get("beats").is_none(), "旧 beats 键应被升格");
        assert_eq!(scene["triggers"][0]["id"], json!("b1"));
        assert_eq!(scene["goals"][0]["condition"]["op"], json!("trigger_fired"));
        assert_eq!(scene["goals"][0]["condition"]["trigger_id"], json!("b1"));
        assert_eq!(fetched.draft["schema_version"], json!(3));
        assert_eq!(fetched.draft["statuses"], json!([]));

        // 落库内容仍是旧格式：历史数据永不改写
        let model = entities::storybook::Entity::find_by_id(row.id.clone())
            .one(&store.db)
            .await
            .unwrap()
            .unwrap();
        assert!(model.draft_json.contains("\"beats\""), "DB 内的历史数据不应被改写");
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
    async fn test_archive_commands_from_moves_rows() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        store.append_command("sv-arch", 1, 0, "scene", "{}").await.unwrap();
        store.append_command("sv-arch", 2, 1, "round_start", "{}").await.unwrap();
        store.append_command("sv-arch", 3, 1, "round_end", "{}").await.unwrap();
        store.append_command("sv-other", 2, 1, "round_start", "{}").await.unwrap();

        let n = store.archive_commands_from("sv-arch", 2).await.unwrap();
        assert_eq!(n, 2, "只归档该存档 seq>=2 的命令");

        let live = entities::command::Entity::find()
            .filter(entities::command::Column::SaveId.eq("sv-arch"))
            .all(&store.db)
            .await
            .unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].seq, 1);
        assert_eq!(
            entities::command::Entity::find()
                .filter(entities::command::Column::SaveId.eq("sv-other"))
                .count(&store.db)
                .await
                .unwrap(),
            1,
            "别的存档不受影响"
        );

        let arch = entities::archived_command::Entity::find()
            .filter(entities::archived_command::Column::SaveId.eq("sv-arch"))
            .all(&store.db)
            .await
            .unwrap();
        assert_eq!(arch.len(), 2);
        assert!(arch.iter().all(|m| m.origin_seq == 2));
        assert_eq!(arch.iter().map(|m| m.seq).max(), Some(3));
    }


    #[tokio::test]
    async fn test_save_model_roundtrip() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let detail = SaveDetail {
            item: SaveListItem {
                id: "sv-model".to_string(),
                title: "模型存档".to_string(),
                storybook_id: "sb-1".to_string(),
                storybook_title: "测试".to_string(),
                embedded_revision: 1,
                latest_revision: 1,
                needs_upgrade: false,
                imported: Some(false),
                is_sandbox: Some(false),
                created_at: now_iso(),
                updated_at: now_iso(),
                last_played_at: now_iso(),
            },
            storybook: json!({ "meta": { "title": "测试" } }),
        };
        store.insert_save(&detail, false).await.unwrap();
        assert_eq!(store.get_save_model("sv-model").await.unwrap(), (None, None, None));

        store
            .set_save_model("sv-model", Some("deepseek"), Some("deepseek-v4-pro"), Some("high"))
            .await
            .unwrap();
        assert_eq!(
            store.get_save_model("sv-model").await.unwrap(),
            (
                Some("deepseek".to_string()),
                Some("deepseek-v4-pro".to_string()),
                Some("high".to_string())
            )
        );

        store.set_save_model("sv-model", None, None, None).await.unwrap();
        assert_eq!(store.get_save_model("sv-model").await.unwrap(), (None, None, None));
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
