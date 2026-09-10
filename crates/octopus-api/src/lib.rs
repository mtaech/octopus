//! octopus-api：axum 路由 / SSE 演出流 / 确认门 / 装配组合根（#17/#24/#20）。
//!
//! 组合根：注入 SqliteStore（#27 单库）与 AiProvider（ai crate）。

pub mod error;
pub mod providers;

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use octopus_engine::{
    AiProvider, EngineError, EventSink, Session, SqliteStore, StorybookRow, WorldState,
};
#[allow(unused_imports)]
use octopus_types::ApiErrorBody;
use octopus_types::{
    CharacterInstance, ConfirmRequest, CreateSaveRequest, CreateStorybookRequest,
    EventEnvelope, HistoryPage, IssueSeverity, MaintenanceRow, PlaytestRequest, ProjectionMeta, PublishRequest,
    RoundInput, SaveDetail, SaveDraftRequest, SaveListItem, SavePackage, SaveSettings, SkeletonProgress,
    StorybookDocument, SubmitRoundRequest, ValidateResult, ValidationIssue,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::error::ApiError;

// ============================================================
// 装配状态
// ============================================================

struct BroadcastSink {
    tx: broadcast::Sender<EventEnvelope>,
}

impl EventSink for BroadcastSink {
    fn emit(&self, event: EventEnvelope) {
        let _ = self.tx.send(event);
    }
}

pub struct AppState {
    store: Arc<SqliteStore>,
    ai: Arc<dyn AiProvider>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    senders: Mutex<HashMap<String, broadcast::Sender<EventEnvelope>>>,
}

impl AppState {
    pub fn new(store: Arc<SqliteStore>, ai: Arc<dyn AiProvider>) -> Arc<Self> {
        Arc::new(Self {
            store,
            ai,
            sessions: Mutex::new(HashMap::new()),
            senders: Mutex::new(HashMap::new()),
        })
    }

    pub fn store(&self) -> &Arc<SqliteStore> {
        &self.store
    }

    /// 惰性建立会话（内存状态 + 事件日志 + 广播出口）。
    pub async fn session_for(&self, save_id: &str) -> Result<Arc<Session>, EngineError> {
        if let Some(s) = self.sessions.lock().expect("sessions poisoned").get(save_id) {
            return Ok(s.clone());
        }
        let save = self
            .store
            .get_save(save_id)
            .await?
            .ok_or_else(|| EngineError::SaveNotFound(save_id.to_string()))?;
        let auto_confirm = self.store.get_auto_confirm(save_id).await?.unwrap_or(false);
        let (tx, _rx) = broadcast::channel(1024);
        let sink = Arc::new(BroadcastSink { tx: tx.clone() });
        let state = build_state(&save);
        let session = Arc::new(Session::new(
            save_id.to_string(),
            state,
            sink,
            self.ai.clone(),
            auto_confirm,
        ));
        self.senders.lock().expect("senders poisoned").insert(save_id.to_string(), tx);
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .insert(save_id.to_string(), session.clone());
        Ok(session)
    }

    fn drop_session(&self, save_id: &str) {
        self.sessions.lock().expect("sessions poisoned").remove(save_id);
        self.senders.lock().expect("senders poisoned").remove(save_id);
    }
}

/// 由内嵌冻结故事书构建运行时世界（#06 ① / #14）。
fn build_state(save: &SaveDetail) -> WorldState {
    let sb = &save.storybook;
    let mut characters: BTreeMap<String, CharacterInstance> = BTreeMap::new();
    if let Some(arr) = sb.get("characters").and_then(|v| v.as_array()) {
        for c in arr {
            let id = c.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if id.is_empty() {
                continue;
            }
            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            let kind = c.get("kind").and_then(|v| v.as_str()).unwrap_or("npc").to_string();
            let attributes = c.get("attributes").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            let resources = c.get("resources").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            characters.insert(
                id.clone(),
                CharacterInstance {
                    instance_id: format!("inst-{id}"),
                    template_id: id,
                    name,
                    kind,
                    attributes,
                    resources,
                    location_id: None,
                    present: true,
                    statuses: vec![],
                },
            );
        }
    }
    let scene = sb.pointer("/skeleton/0/scenes/0");
    let scene_id = scene.and_then(|s| s.get("id")).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let scene_title = scene
        .and_then(|s| s.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let scene_description = scene
        .and_then(|s| s.get("description"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let locations = sb
        .pointer("/world/locations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let controlled: Vec<String> = characters
        .values()
        .find(|c| c.kind == "pc")
        .map(|c| vec![c.template_id.clone()])
        .unwrap_or_default();
    let meta = ProjectionMeta {
        save_id: save.item.id.clone(),
        save_title: save.item.title.clone(),
        storybook_title: save.item.storybook_title.clone(),
        revision: save.item.embedded_revision,
        needs_upgrade: save.item.needs_upgrade,
        auto_confirm: false,
    };
    WorldState {
        seq: 0,
        scene_id,
        scene_title,
        scene_description,
        controlled,
        characters,
        flags: BTreeMap::new(),
        progress: SkeletonProgress::default(),
        locations,
        meta,
        rng_seed: fnv1a(&save.item.id),
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

// ============================================================
// 路由
// ============================================================

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/providers/probe", post(providers::probe_models))
        .route(
            "/api/storybooks",
            get(list_storybooks).post(create_storybook_draft),
        )
        .route(
            "/api/storybooks/{id}",
            get(get_storybook).put(save_storybook_draft).delete(delete_storybook),
        )
        .route("/api/storybooks/{id}/publish", post(publish_storybook))
        .route("/api/storybooks/{id}/sandbox", post(playtest_storybook))
        .route("/api/validate", post(validate_endpoint))
        .route("/api/saves", get(list_saves).post(create_save))
        .route(
            "/api/saves/{id}",
            get(get_save).patch(rename_save).delete(delete_save),
        )
        .route("/api/saves/{id}/state", get(get_state))
        .route("/api/saves/{id}/history", get(get_history))
        .route("/api/saves/{id}/stream", get(stream))
        .route("/api/saves/{id}/rounds", post(submit_round))
        .route(
            "/api/saves/{id}/rounds/{round_id}/confirmation",
            post(confirm_round),
        )
        .route("/api/saves/{id}/maintenance", get(list_maintenance))
        .route(
            "/api/saves/{id}/settings",
            get(get_settings).put(put_settings),
        )
        .route("/api/saves/{id}/character", post(switch_character))
        .route("/api/saves/{id}/export", get(export_save))
        .route("/api/saves/import", post(import_save))
        .route("/api/saves/{id}/save", post(manual_save))
        .route("/api/saves/{id}/origin", post(new_origin))
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "octopus-api" }))
}

fn row_to_doc(row: StorybookRow) -> StorybookDocument {
    StorybookDocument {
        id: row.id,
        revision: row.revision,
        draft_version: row.draft_version,
        updated_at: row.updated_at,
        released_at: row.released_at,
        published: row.published,
        draft: row.draft,
        released: row.released,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorybookMutationResponse {
    pub doc: StorybookDocument,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Deserialize, Default)]
struct ListStorybooksQuery {
    released_only: Option<bool>,
}

async fn list_storybooks(
    State(app): State<Arc<AppState>>,
    Query(q): Query<ListStorybooksQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let released_only = q.released_only.unwrap_or(true);
    Ok(Json(app.store().list_storybooks(released_only).await?))
}

async fn get_storybook(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<StorybookDocument>, ApiError> {
    let row = app
        .store()
        .get_storybook(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在"))?;
    Ok(Json(row_to_doc(row)))
}

async fn create_storybook_draft(
    State(app): State<Arc<AppState>>,
    Json(req): Json<CreateStorybookRequest>,
) -> Result<(StatusCode, Json<StorybookDocument>), ApiError> {
    let initial = json!({
        "schema_version": 1,
        "meta": {
            "title": req.title.as_deref().unwrap_or("未命名故事书"),
        },
        "world": { "premise": "", "locations": [], "resources": [] },
        "attribute_dimensions": [],
        "skeleton": [],
        "characters": [],
        "skills": [],
        "items": [],
        "objects": [],
        "factions": [],
        "relationships": [],
        "flags": [],
        "events": [],
        "relationship_types": [],
        "target_types": [],
    });
    let row = app.store().create_storybook_draft(req.title.as_deref(), &initial).await?;
    Ok((StatusCode::CREATED, Json(row_to_doc(row))))
}

async fn save_storybook_draft(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SaveDraftRequest>,
) -> Result<Json<StorybookMutationResponse>, ApiError> {
    let row = app.store().save_draft(&id, &req.draft, req.base_version).await?;
    let issues = octopus_engine::validate_storybook(&req.draft);
    Ok(Json(StorybookMutationResponse {
        doc: row_to_doc(row),
        issues,
    }))
}

async fn publish_storybook(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<PublishRequest>,
) -> Result<Json<StorybookMutationResponse>, ApiError> {
    let row = app
        .store()
        .get_storybook(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在"))?;
    let issues = octopus_engine::validate_storybook(&row.draft);
    if issues.iter().any(|i| matches!(i.severity, IssueSeverity::Error)) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "故事书存在阻断性错误，无法发布",
        )
        .with_detail(serde_json::json!({ "issues": issues })));
    }
    let row = app.store().publish_storybook(&id, req.base_version).await?;
    Ok(Json(StorybookMutationResponse {
        doc: row_to_doc(row),
        issues,
    }))
}

async fn delete_storybook(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if app.store().delete_storybook(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在"))
    }
}

async fn validate_endpoint(Json(body): Json<Value>) -> Json<ValidateResult> {
    Json(octopus_engine::validate_storybook_result(&body))
}

async fn list_saves(State(app): State<Arc<AppState>>) -> Result<Json<Vec<SaveListItem>>, ApiError> {
    Ok(Json(app.store().list_saves().await?))
}

async fn create_save(
    State(app): State<Arc<AppState>>,
    Json(req): Json<CreateSaveRequest>,
) -> Result<(StatusCode, Json<SaveDetail>), ApiError> {
    let sb = app
        .store()
        .get_storybook(&req.storybook_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在"))?;
    let is_sandbox = req.is_sandbox.unwrap_or(false);
    let (storybook_content, revision) = if is_sandbox {
        let issues = octopus_engine::validate_storybook(&sb.draft);
        if issues.iter().any(|i| matches!(i.severity, IssueSeverity::Error)) {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "草稿存在阻断性错误，无法开始沙箱试玩",
            )
            .with_detail(serde_json::json!({ "issues": issues })));
        }
        (sb.draft.clone(), sb.revision)
    } else {
        let released = sb
            .released
            .clone()
            .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "storybook_unpublished", "故事书尚未发布"))?;
        (released, sb.revision)
    };
    let now = octopus_engine::storage::now_iso();
    let id = if is_sandbox {
        format!("sv-sbx-{}", uuid::Uuid::new_v4().simple())
    } else {
        format!("sv-{}", uuid::Uuid::new_v4().simple())
    };
    let default_title = if is_sandbox {
        format!("【沙箱试玩】{}", sb.title)
    } else {
        sb.title.clone()
    };
    let item = SaveListItem {
        id: id.clone(),
        title: req.title.unwrap_or(default_title),
        storybook_id: sb.id.clone(),
        storybook_title: sb.title.clone(),
        embedded_revision: revision,
        latest_revision: sb.revision,
        needs_upgrade: false,
        imported: Some(false),
        is_sandbox: Some(is_sandbox),
        created_at: now.clone(),
        updated_at: now.clone(),
        last_played_at: now,
    };
    let mut detail = SaveDetail { item, storybook: storybook_content };
    if let Some(cid) = &req.controlled_character_id {
        if let Some(arr) = detail.storybook.get_mut("characters").and_then(|v| v.as_array_mut()) {
            for c in arr {
                if c.get("id").and_then(|v| v.as_str()) == Some(cid.as_str()) {
                    c["kind"] = json!("pc");
                }
            }
        }
    }
    app.store().insert_save(&detail, false).await?;
    if let Some(cid) = &req.controlled_character_id {
        let session = app.session_for(&id).await?;
        let _ = session.switch_character(cid);
    }
    detail.item.imported = Some(false);
    detail.item.is_sandbox = Some(is_sandbox);
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn playtest_storybook(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<PlaytestRequest>,
) -> Result<(StatusCode, Json<SaveDetail>), ApiError> {
    create_save(
        State(app),
        Json(CreateSaveRequest {
            storybook_id: id,
            title: req.title,
            controlled_character_id: req.controlled_character_id,
            is_sandbox: Some(true),
        }),
    )
    .await
}

async fn get_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SaveDetail>, ApiError> {
    let save = app
        .store()
        .get_save(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    Ok(Json(save))
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

async fn rename_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<Json<SaveListItem>, ApiError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "empty_title", "标题不能为空"));
    }
    app.store()
        .rename_save(&id, title)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))
}

async fn delete_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if app.store().delete_save(&id).await? {
        app.drop_session(&id);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))
    }
}

async fn get_state(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<octopus_types::WorldProjection>, ApiError> {
    let session = app.session_for(&id).await?;
    app.store().touch_save(&id).await?;
    Ok(Json(session.projection()))
}

#[derive(Deserialize)]
struct HistoryQuery {
    before_seq: Option<u64>,
    limit: Option<usize>,
}

async fn get_history(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<HistoryPage>, ApiError> {
    let session = app.session_for(&id).await?;
    Ok(Json(session.history(q.before_seq, q.limit.unwrap_or(50).min(200))))
}

async fn submit_round(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SubmitRoundRequest>,
) -> Result<StatusCode, ApiError> {
    let session = app.session_for(&id).await?;
    let input = RoundInput { channel: req.channel, text: req.text };
    let request_id = req.request_id;
    let sid = id.clone();
    // 202 立即返回，事件经 SSE 流出（#24 ①）
    tokio::spawn(async move {
        if let Err(e) = session.run_round(input, request_id).await {
            tracing::warn!(save_id = %sid, error = %e, "回合执行失败");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

async fn confirm_round(
    State(app): State<Arc<AppState>>,
    Path((id, _round_id)): Path<(String, u32)>,
    Json(req): Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    let session = app.session_for(&id).await?;
    session.confirm(&req.action_id, req.decision)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_maintenance(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<MaintenanceRow>>, ApiError> {
    Ok(Json(app.store().list_maintenance(&id).await?))
}

async fn get_settings(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SaveSettings>, ApiError> {
    let v = app
        .store()
        .get_auto_confirm(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    Ok(Json(SaveSettings { auto_confirm: v }))
}

async fn put_settings(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SaveSettings>,
) -> Result<Json<SaveSettings>, ApiError> {
    app.store().set_auto_confirm(&id, body.auto_confirm).await?;
    if let Ok(session) = app.session_for(&id).await {
        session.set_auto_confirm(body.auto_confirm);
    }
    Ok(Json(body))
}

#[derive(Deserialize)]
struct SwitchCharacterBody {
    character_id: String,
}

async fn switch_character(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SwitchCharacterBody>,
) -> Result<StatusCode, ApiError> {
    let session = app.session_for(&id).await?;
    session.switch_character(&body.character_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn manual_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SaveListItem>, ApiError> {
    app.store().touch_save(&id).await?;
    app.store().append_maintenance(&id, "手动存档", "快照已写入（保留 5 份）").await?;
    app.store()
        .list_saves()
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))
}

async fn new_origin(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    app.store().append_maintenance(&id, "压缩为新原点", "以当前状态为新起点；旧日志归档只读").await?;
    Ok(Json(json!({ "ok": true, "archived_count": 0 })))
}

async fn export_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg = app.store().export_save_package(&id).await?;
    let filename = format!("{id}.octopus.json");
    let headers = [
        (axum::http::header::CONTENT_TYPE, "application/json".to_string()),
        (
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        ),
    ];
    Ok((headers, Json(pkg)))
}

async fn import_save(
    State(app): State<Arc<AppState>>,
    Json(pkg): Json<SavePackage>,
) -> Result<(StatusCode, Json<SaveListItem>), ApiError> {
    let item = app.store().import_save_package(&pkg).await?;
    Ok((StatusCode::CREATED, Json(item)))
}

// ============================================================
// SSE 演出流
// ============================================================

async fn stream(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _ = app.session_for(&id).await?;
    let tx = app
        .senders
        .lock()
        .expect("senders poisoned")
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    let rx = tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(env) => {
            let data = serde_json::to_string(&env).unwrap_or_default();
            Some(Ok(Event::default().event("play").id(env.id).data(data)))
        }
        Err(_) => None,
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_ai::ScriptedProvider;
    use serde_json::json;

    async fn spawn_app() -> (String, Arc<AppState>) {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        let ai = Arc::new(ScriptedProvider);
        let state = AppState::new(store, ai);
        let app = router(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), state)
    }

    #[tokio::test]
    async fn test_api_create_get_delete_storybook() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 1. 创建故事书草稿
        let create_req = CreateStorybookRequest {
            title: Some("魔法森林".to_string()),
        };
        let res = client
            .post(format!("{base_url}/api/storybooks"))
            .json(&create_req)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let doc: StorybookDocument = res.json().await.unwrap();
        assert!(doc.id.starts_with("sb-"));
        assert_eq!(doc.revision, 0);
        assert_eq!(doc.draft_version, 1);
        assert!(!doc.published);
        assert_eq!(doc.draft["meta"]["title"], "魔法森林");

        // 2. 获取故事书
        let res = client
            .get(format!("{base_url}/api/storybooks/{}", doc.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let fetched: StorybookDocument = res.json().await.unwrap();
        assert_eq!(fetched.id, doc.id);
        assert_eq!(fetched.draft_version, 1);

        // 3. 删除故事书
        let res = client
            .delete(format!("{base_url}/api/storybooks/{}", doc.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // 再次删除 -> 404
        let res = client
            .delete(format!("{base_url}/api/storybooks/{}", doc.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // 获取已删除故事书 -> 404
        let res = client
            .get(format!("{base_url}/api/storybooks/{}", doc.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_api_save_draft_optimistic_locking_and_idempotency() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 创建草稿
        let res = client
            .post(format!("{base_url}/api/storybooks"))
            .json(&CreateStorybookRequest {
                title: Some("迷雾古堡".to_string()),
            })
            .send()
            .await
            .unwrap();
        let doc: StorybookDocument = res.json().await.unwrap();
        assert_eq!(doc.draft_version, 1);

        // 保存新草稿 (base_version = 1)
        let updated_draft = json!({
            "meta": { "title": "迷雾古堡·修缮版" },
            "world": { "premise": "古老的城堡沉睡在浓雾中" }
        });
        let save_req = SaveDraftRequest {
            draft: updated_draft.clone(),
            base_version: 1,
        };
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&save_req)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let mutation: StorybookMutationResponse = res.json().await.unwrap();
        assert_eq!(mutation.doc.draft_version, 2);
        assert_eq!(mutation.doc.draft, updated_draft);

        // 内容幂等性：传入完全相同的 draft，版本不 bump
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest {
                draft: updated_draft.clone(),
                base_version: 2,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let idemp: StorybookMutationResponse = res.json().await.unwrap();
        assert_eq!(idemp.doc.draft_version, 2);

        // 乐观锁冲突：使用旧版本 base_version = 1 提交新内容
        let conflict_draft = json!({
            "meta": { "title": "冲突分支修改" }
        });
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest {
                draft: conflict_draft,
                base_version: 1,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let err: ApiErrorBody = res.json().await.unwrap();
        assert_eq!(err.code, "draft_conflict");
        assert_eq!(err.detail.unwrap()["current_draft_version"], 2);
    }

    #[tokio::test]
    async fn test_api_publish_storybook() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 创建草稿 (draft_version = 1)
        let res = client
            .post(format!("{base_url}/api/storybooks"))
            .json(&CreateStorybookRequest {
                title: Some("发布测试".to_string()),
            })
            .send()
            .await
            .unwrap();
        let doc: StorybookDocument = res.json().await.unwrap();

        // 错误 base_version 发布 -> 409
        let res = client
            .post(format!("{base_url}/api/storybooks/{}/publish", doc.id))
            .json(&PublishRequest { base_version: 999 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        // 保存一个包含阻断性错误的草稿（例如 meta.title 为空）
        let invalid_draft = json!({
            "schema_version": 1,
            "meta": { "id": doc.id, "title": "" }
        });
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest {
                draft: invalid_draft,
                base_version: 1,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 尝试发布存在阻断性错误的故事书 -> 422 Unprocessable Entity
        let res = client
            .post(format!("{base_url}/api/storybooks/{}/publish", doc.id))
            .json(&PublishRequest { base_version: 2 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let err: ApiErrorBody = res.json().await.unwrap();
        assert_eq!(err.code, "validation_failed");
        assert!(err.detail.unwrap()["issues"].as_array().unwrap().len() > 0);

        // 修复草稿中的错误
        let valid_draft = json!({
            "schema_version": 1,
            "meta": { "id": doc.id, "title": "已修复标题" }
        });
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest {
                draft: valid_draft,
                base_version: 2,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 正确发布 (当前 draft_version = 3)
        let res = client
            .post(format!("{base_url}/api/storybooks/{}/publish", doc.id))
            .json(&PublishRequest { base_version: 3 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let pub_resp: StorybookMutationResponse = res.json().await.unwrap();
        assert_eq!(pub_resp.doc.revision, 1);
        assert_eq!(pub_resp.doc.draft_version, 4);
        assert!(pub_resp.doc.published);
        assert!(pub_resp.doc.released_at.is_some());
        assert_eq!(pub_resp.doc.released, Some(pub_resp.doc.draft.clone()));

        // 再次发布同一 base_version -> 409
        let res = client
            .post(format!("{base_url}/api/storybooks/{}/publish", doc.id))
            .json(&PublishRequest { base_version: 3 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        // 发布不存在的故事书 -> 404
        let res = client
            .post(format!("{base_url}/api/storybooks/sb-nonexistent/publish"))
            .json(&PublishRequest { base_version: 1 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_api_validate_endpoint() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 有效故事书
        let res = client
            .post(format!("{base_url}/api/validate"))
            .json(&json!({
                "schema_version": 1,
                "meta": { "id": "sb-test", "title": "测试验证" }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let result: ValidateResult = res.json().await.unwrap();
        assert!(result.valid);
        assert!(result.issues.is_empty());

        // 无效故事书（缺少 meta.id）
        let res2 = client
            .post(format!("{base_url}/api/validate"))
            .json(&json!({
                "schema_version": 1,
                "meta": { "title": "缺少ID" }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(res2.status(), StatusCode::OK);
        let result2: ValidateResult = res2.json().await.unwrap();
        assert!(!result2.valid);
        assert!(result2.issues.iter().any(|i| i.code == "missing_id"));
    }

    #[tokio::test]
    async fn test_api_export_and_import_save() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 1. 开档
        let create_res = client
            .post(format!("{base_url}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("待导出存档".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(create_res.status(), StatusCode::CREATED);
        let save_detail: SaveDetail = create_res.json().await.unwrap();
        let save_id = save_detail.item.id;

        // 2. 导出
        let export_res = client
            .get(format!("{base_url}/api/saves/{save_id}/export"))
            .send()
            .await
            .unwrap();
        assert_eq!(export_res.status(), StatusCode::OK);
        let content_disp = export_res
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_disp.contains(&format!("{save_id}.octopus.json")));
        let pkg: SavePackage = export_res.json().await.unwrap();
        assert_eq!(pkg.format, "octopus-save-package");
        assert_eq!(pkg.save.item.title, "待导出存档");

        // 3. 导入（产生新记录）
        let import_res = client
            .post(format!("{base_url}/api/saves/import"))
            .json(&pkg)
            .send()
            .await
            .unwrap();
        assert_eq!(import_res.status(), StatusCode::CREATED);
        let imported_item: SaveListItem = import_res.json().await.unwrap();
        assert_eq!(imported_item.imported, Some(true));
        assert!(imported_item.title.contains("(导入)"));
    }

    #[tokio::test]
    async fn test_api_playtest_sandbox() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 1. 新建未发布的草稿
        let create_res = client
            .post(format!("{base_url}/api/storybooks"))
            .json(&CreateStorybookRequest {
                title: Some("测试沙箱书".to_string()),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(create_res.status(), StatusCode::CREATED);
        let doc: StorybookDocument = create_res.json().await.unwrap();
        assert!(!doc.published);

        // 2. 尝试常规开档 -> 409 未发布
        let fail_save = client
            .post(format!("{base_url}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: doc.id.clone(),
                title: None,
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(fail_save.status(), StatusCode::CONFLICT);

        // 3. 沙箱开档（未发布但草稿结构有效）-> 201 成功
        let sbx_res = client
            .post(format!("{base_url}/api/storybooks/{}/sandbox", doc.id))
            .json(&PlaytestRequest {
                title: None,
                controlled_character_id: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(sbx_res.status(), StatusCode::CREATED);
        let sbx_detail: SaveDetail = sbx_res.json().await.unwrap();
        assert_eq!(sbx_detail.item.is_sandbox, Some(true));
        assert!(sbx_detail.item.title.starts_with("【沙箱试玩】"));

        // 4. 将草稿改坏（破坏 meta.title）后保存
        let mut broken_draft = doc.draft.clone();
        broken_draft["meta"]["title"] = json!("");
        let save_draft_res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest {
                draft: broken_draft,
                base_version: doc.draft_version,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(save_draft_res.status(), StatusCode::OK);

        // 5. 损坏草稿进行沙箱开档 -> 422 阻断
        let broken_sbx = client
            .post(format!("{base_url}/api/storybooks/{}/sandbox", doc.id))
            .json(&PlaytestRequest {
                title: None,
                controlled_character_id: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(broken_sbx.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

