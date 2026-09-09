//! 存储层（#27 应用级单库 SQLite）：故事书 / 存档 / 命令日志 / 维护历史同库。
//! 向量索引为独立 DuckDB 文件（派生可重建），本里程碑暂未接入。

use std::sync::{Mutex, MutexGuard};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use octopus_types::{MaintenanceRow, SaveDetail, SaveListItem};

use crate::{error::EngineError, seed::seed_storybooks};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

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

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        let store = Self { conn: Mutex::new(Connection::open(path)?) };
        store.init_schema()?;
        store.seed_if_empty()?;
        Ok(store)
    }

    /// 仅内存（测试 / 冒烟用）。
    pub fn open_in_memory() -> Result<Self, EngineError> {
        let store = Self { conn: Mutex::new(Connection::open_in_memory()?) };
        store.init_schema()?;
        store.seed_if_empty()?;
        Ok(store)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("sqlite mutex poisoned")
    }

    fn init_schema(&self) -> Result<(), EngineError> {
        self.conn().execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS storybooks (
                id            TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                draft_json    TEXT NOT NULL,
                released_json TEXT,
                revision      INTEGER NOT NULL DEFAULT 0,
                draft_version INTEGER NOT NULL DEFAULT 0,
                updated_at    TEXT NOT NULL,
                released_at   TEXT
            );
            CREATE TABLE IF NOT EXISTS saves (
                id                TEXT PRIMARY KEY,
                title             TEXT NOT NULL,
                storybook_id      TEXT NOT NULL,
                storybook_title   TEXT NOT NULL,
                embedded_revision INTEGER NOT NULL,
                latest_revision   INTEGER NOT NULL,
                needs_upgrade     INTEGER NOT NULL DEFAULT 0,
                imported          INTEGER NOT NULL DEFAULT 0,
                storybook_json    TEXT NOT NULL,
                auto_confirm      INTEGER NOT NULL DEFAULT 0,
                created_at        TEXT NOT NULL,
                updated_at        TEXT NOT NULL,
                last_played_at    TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS commands (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                save_id      TEXT NOT NULL,
                seq          INTEGER NOT NULL,
                round        INTEGER NOT NULL,
                kind         TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                ts           TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_commands_save_seq ON commands(save_id, seq);
            CREATE TABLE IF NOT EXISTS archived_commands (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                save_id      TEXT NOT NULL,
                origin_seq   INTEGER NOT NULL,
                seq          INTEGER NOT NULL,
                round        INTEGER NOT NULL,
                kind         TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                ts           TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS maintenance (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                save_id TEXT NOT NULL,
                at      TEXT NOT NULL,
                op      TEXT NOT NULL,
                summary TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    fn seed_if_empty(&self) -> Result<(), EngineError> {
        let count: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM storybooks", [], |r| r.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let now = now_iso();
        let conn = self.conn();
        for sb in seed_storybooks() {
            let json = serde_json::to_string(&sb.json)?;
            conn.execute(
                "INSERT INTO storybooks (id,title,draft_json,released_json,revision,draft_version,updated_at,released_at)
                 VALUES (?1,?2,?3,?3,?4,?4,?5,?5)",
                params![sb.id, sb.title, json, sb.revision as i64, now],
            )?;
        }
        Ok(())
    }

    // ---------- 故事书 ----------

    pub fn list_storybooks(&self, released_only: bool) -> Result<Vec<Value>, EngineError> {
        let conn = self.conn();
        let sql = if released_only {
            "SELECT id,title,revision,draft_version,updated_at,released_at FROM storybooks WHERE released_json IS NOT NULL ORDER BY updated_at DESC"
        } else {
            "SELECT id,title,revision,draft_version,updated_at,released_at FROM storybooks ORDER BY updated_at DESC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map([], |r| {
                let released_at: Option<String> = r.get(5)?;
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                    "revision": r.get::<_, i64>(2)?,
                    "draft_version": r.get::<_, i64>(3)?,
                    "updated_at": r.get::<_, String>(4)?,
                    "released_at": released_at,
                    "published": released_at.is_some(),
                    "description": Value::Null,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_storybook(&self, id: &str) -> Result<Option<StorybookRow>, EngineError> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id,title,revision,draft_version,updated_at,released_at,draft_json,released_json
                 FROM storybooks WHERE id = ?1",
                params![id],
                |r| {
                    let draft_json: String = r.get(6)?;
                    let released_json: Option<String> = r.get(7)?;
                    Ok(StorybookRow {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        revision: r.get::<_, i64>(2)? as u32,
                        draft_version: r.get::<_, i64>(3)? as u32,
                        updated_at: r.get(4)?,
                        released_at: r.get(5)?,
                        published: released_json.is_some(),
                        draft: serde_json::from_str(&draft_json).unwrap_or(Value::Null),
                        released: released_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    // ---------- 存档 ----------

    pub fn insert_save(&self, d: &SaveDetail, auto_confirm: bool) -> Result<(), EngineError> {
        let conn = self.conn();
        let storybook_json = serde_json::to_string(&d.storybook)?;
        conn.execute(
            "INSERT INTO saves (id,title,storybook_id,storybook_title,embedded_revision,latest_revision,
                                needs_upgrade,imported,storybook_json,auto_confirm,created_at,updated_at,last_played_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                d.item.id, d.item.title, d.item.storybook_id, d.item.storybook_title,
                d.item.embedded_revision as i64, d.item.latest_revision as i64,
                d.item.needs_upgrade as i64, d.item.imported.unwrap_or(false) as i64,
                storybook_json, auto_confirm as i64,
                d.item.created_at, d.item.updated_at, d.item.last_played_at
            ],
        )?;
        Ok(())
    }

    fn row_to_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<SaveListItem> {
        Ok(SaveListItem {
            id: r.get(0)?,
            title: r.get(1)?,
            storybook_id: r.get(2)?,
            storybook_title: r.get(3)?,
            embedded_revision: r.get::<_, i64>(4)? as u32,
            latest_revision: r.get::<_, i64>(5)? as u32,
            needs_upgrade: r.get::<_, i64>(6)? != 0,
            imported: Some(r.get::<_, i64>(7)? != 0),
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
            last_played_at: r.get(10)?,
        })
    }

    pub fn list_saves(&self) -> Result<Vec<SaveListItem>, EngineError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id,title,storybook_id,storybook_title,embedded_revision,latest_revision,
                    needs_upgrade,imported,created_at,updated_at,last_played_at
             FROM saves ORDER BY last_played_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Self::row_to_item(r))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_save(&self, id: &str) -> Result<Option<SaveDetail>, EngineError> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id,title,storybook_id,storybook_title,embedded_revision,latest_revision,
                        needs_upgrade,imported,created_at,updated_at,last_played_at,storybook_json
                 FROM saves WHERE id = ?1",
                params![id],
                |r| {
                    let item = Self::row_to_item(r)?;
                    let storybook_json: String = r.get(11)?;
                    Ok(SaveDetail {
                        item,
                        storybook: serde_json::from_str(&storybook_json).unwrap_or(Value::Null),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn rename_save(&self, id: &str, title: &str) -> Result<Option<SaveListItem>, EngineError> {
        let now = now_iso();
        {
            let conn = self.conn();
            conn.execute(
                "UPDATE saves SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, title, now],
            )?;
        }
        Ok(self.list_saves()?.into_iter().find(|s| s.id == id))
    }

    pub fn delete_save(&self, id: &str) -> Result<bool, EngineError> {
        let conn = self.conn();
        let n = conn.execute("DELETE FROM saves WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM commands WHERE save_id = ?1", params![id])?;
        conn.execute("DELETE FROM maintenance WHERE save_id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn get_auto_confirm(&self, id: &str) -> Result<Option<bool>, EngineError> {
        let conn = self.conn();
        let v: Option<i64> = conn
            .query_row("SELECT auto_confirm FROM saves WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?;
        Ok(v.map(|x| x != 0))
    }

    pub fn set_auto_confirm(&self, id: &str, v: bool) -> Result<(), EngineError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE saves SET auto_confirm = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, v as i64, now_iso()],
        )?;
        Ok(())
    }

    pub fn touch_save(&self, id: &str) -> Result<(), EngineError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE saves SET updated_at = ?2, last_played_at = ?2 WHERE id = ?1",
            params![id, now_iso()],
        )?;
        Ok(())
    }

    // ---------- 命令日志 / 维护历史 ----------

    pub fn append_command(
        &self,
        save_id: &str,
        seq: i64,
        round: i64,
        kind: &str,
        payload_json: &str,
    ) -> Result<(), EngineError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO commands (save_id,seq,round,kind,payload_json,ts) VALUES (?1,?2,?3,?4,?5,?6)",
            params![save_id, seq, round, kind, payload_json, now_iso()],
        )?;
        Ok(())
    }

    pub fn append_maintenance(&self, save_id: &str, op: &str, summary: &str) -> Result<(), EngineError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO maintenance (save_id,at,op,summary) VALUES (?1,?2,?3,?4)",
            params![save_id, now_iso(), op, summary],
        )?;
        Ok(())
    }

    pub fn list_maintenance(&self, save_id: &str) -> Result<Vec<MaintenanceRow>, EngineError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT at,op,summary FROM maintenance WHERE save_id = ?1 ORDER BY id DESC LIMIT 100",
        )?;
        let rows = stmt
            .query_map(params![save_id], |r| {
                Ok(MaintenanceRow { at: r.get(0)?, op: r.get(1)?, summary: r.get(2)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
