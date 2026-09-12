//! octopus-api：axum 路由 / SSE 演出流 / 确认门 / 装配组合根（#17/#24/#20）。
//!
//! 组合根：注入 SqliteStore（#27 单库）与 AiProvider（ai crate）。

pub mod ai;
pub mod config;
pub mod error;
pub mod logging;
pub mod pair;
pub mod providers;

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use futures::Stream;
use octopus_engine::{
    AiProvider, AssetStore, EmbeddingBackend, EngineError, EventSink, LuaHost, LuaHostContext,
    LuaMount, ModelRef, NewPairMessage, PairThreadRow, Session, SqliteStore, StorybookRow, WorldState,
    content_type_of, lint_script, new_lint_state, pack_bundle, unpack_bundle,
};
#[allow(unused_imports)]
use octopus_types::ApiErrorBody;
use octopus_types::{
    CharacterInstance, ConfirmRequest, CreateSaveRequest, CreateStorybookRequest,
    EntityRef, EventEnvelope, FocusEntity, HistoryPage, IssueSeverity, MaintenanceRow, PairMessageRecord,
    PairThreadRecord, PlaytestRequest, ProjectionMeta, PublishRequest, RoundInput, SaveDetail,
    SaveDraftRequest, SaveListItem, SaveSettings, SkeletonProgress, StorybookDocument,
    SubmitRoundRequest, ValidateResult, ValidationIssue,
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

/// 演出流写队列消息：事件，或回滚前的一致性屏障。
enum SinkMsg {
    Event(EventEnvelope),
    Barrier(tokio::sync::oneshot::Sender<()>),
}

/// 演出流出口：事件先进入单消费者队列，由写任务**先落库、后广播**。
/// 顺序不变式保证「UI 上出现过的事件一定已经持久化」。
struct PersistingSink {
    tx: tokio::sync::mpsc::UnboundedSender<SinkMsg>,
}

#[async_trait::async_trait]
impl EventSink for PersistingSink {
    fn emit(&self, event: EventEnvelope) {
        if self.tx.send(SinkMsg::Event(event)).is_err() {
            tracing::error!("演出事件落库队列已关闭，事件未持久化");
        }
    }

    async fn flush(&self) {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        if self.tx.send(SinkMsg::Barrier(ack_tx)).is_ok() {
            let _ = ack_rx.await;
        }
    }
}

pub struct AppState {
    store: Arc<SqliteStore>,
    /// 资产库（#28）：图片内容寻址存于文件系统，不占数据库
    assets: Arc<AssetStore>,
    /// AI provider 槽：保存配置后热替换，后续回合立即用新模型（无需重启）。
    ai: Arc<RwLock<Arc<dyn AiProvider>>>,
    embedding: Arc<dyn EmbeddingBackend>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    senders: Mutex<HashMap<String, broadcast::Sender<EventEnvelope>>>,
    /// 每回合 token 预算（0 = 不限）：保存配置后热更新到所有会话。
    token_budget: AtomicU32,
}

impl AppState {
    pub fn new(
        store: Arc<SqliteStore>,
        ai: Arc<dyn AiProvider>,
        embedding: Arc<dyn EmbeddingBackend>,
        assets: Arc<AssetStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            assets,
            ai: Arc::new(RwLock::new(ai)),
            embedding,
            sessions: Mutex::new(HashMap::new()),
            senders: Mutex::new(HashMap::new()),
            token_budget: AtomicU32::new(
                crate::config::load_config_from_disk().turn_token_budget.unwrap_or(0),
            ),
        })
    }

    pub fn store(&self) -> &Arc<SqliteStore> {
        &self.store
    }

    pub fn assets(&self) -> &Arc<AssetStore> {
        &self.assets
    }

    pub fn embedding(&self) -> &Arc<dyn EmbeddingBackend> {
        &self.embedding
    }

    /// 热替换 AI provider：保存配置后调用，后续回合立即用新模型（无需重启进程/重建会话）。
    pub fn set_ai(&self, ai: Arc<dyn AiProvider>) {
        *self.ai.write().expect("ai poisoned") = ai;
    }

    /// 当前每回合 token 预算（0 = 不限）。
    pub fn token_budget(&self) -> u32 {
        self.token_budget.load(Ordering::SeqCst)
    }

    /// 更新预算并热应用到所有已打开会话。
    pub fn set_token_budget(&self, budget: u32) {
        self.token_budget.store(budget, Ordering::SeqCst);
        for s in self.sessions.lock().expect("sessions poisoned").values() {
            s.set_token_budget(budget);
        }
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

        // 先读回命令日志，再建会话并重放；重放完成前不对外暴露会话。
        let persisted = self.store.load_events(save_id).await?;

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<SinkMsg>();
        let (tx, _rx) = broadcast::channel(1024);
        let store = self.store.clone();
        let sid = save_id.to_string();
        let btx = tx.clone();
        // 单消费者串行落库：保证「先落库、后广播」的顺序不变式。
        tokio::spawn(async move {
            while let Some(msg) = event_rx.recv().await {
                let env = match msg {
                    SinkMsg::Event(env) => env,
                    // 屏障：此前所有事件都已处理完（落库 + 广播），放行等待者。
                    SinkMsg::Barrier(ack) => {
                        let _ = ack.send(());
                        continue;
                    }
                };
                if let Err(e) = store.append_event(&sid, &env).await {
                    tracing::error!(
                        save_id = %sid,
                        seq = env.seq,
                        error = %e,
                        "演出事件落库失败；事件仍会广播，但该条未持久化"
                    );
                }
                let _ = btx.send(env);
            }
        });
        let sink = Arc::new(PersistingSink { tx: event_tx });
        let state = build_state(&save);
        let session = Arc::new(Session::new(
            save_id.to_string(),
            state,
            sink,
            self.ai.clone(),
            auto_confirm,
            save.storybook.clone(),
        ));
        session.replay(&persisted);
        // 每回合 token 预算：构造后按当前配置套用。
        session.set_token_budget(self.token_budget());
        // 本存档覆盖的模型（存 saves 表）：会话建立后立刻套用，后续回合即可用。
        if let Ok((pid, mid, effort)) = self.store.get_save_model(save_id).await {
            if let (Some(provider_id), Some(model)) = (pid, mid) {
                session.set_model(Some(ModelRef { provider_id, model, reasoning_effort: effort }));
            }
        }
        // 叙述段玩家偏好（存 saves 表）：同样在会话建立后套用；不影响已重放的历史。
        if let Some(overrides) = self.store.get_save_narrative(save_id).await? {
            session.set_narrative_overrides(overrides);
        }
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
/// 初始在场判定：
/// - PC 恒在场；
/// - 骨架初始场景**声明了** `present_char_ids` 时以声明为准（避免把全书角色当同场同伴演）；
/// - 没有骨架、或初始场景未声明该字段时回落到「在场」——否则无骨架的角色卡故事书会一个人都不在场，
///   角色 AI 无人可演，剧情无法推进。
fn is_initially_present(sb: &Value, id: &str, kind: &str) -> bool {
    if kind == "pc" {
        return true;
    }
    match sb
        .pointer("/skeleton/0/scenes/0/present_char_ids")
        .and_then(Value::as_array)
    {
        Some(list) => list.iter().any(|v| v.as_str() == Some(id)),
        None => true,
    }
}

fn build_state(save: &SaveDetail) -> WorldState {
    let sb = &save.storybook;
    let scene = sb.pointer("/skeleton/0/scenes/0");
    let mut characters: BTreeMap<String, CharacterInstance> = BTreeMap::new();
    if let Some(arr) = sb.get("characters").and_then(|v| v.as_array()) {
        for c in arr {
            let id = c.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if id.is_empty() {
                continue;
            }
            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            let kind = c.get("kind").and_then(|v| v.as_str()).unwrap_or("npc").to_string();
            let present = is_initially_present(sb, &id, &kind);
            let attributes = c.get("attributes").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            let resources = c.get("resources").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            // 物品栏（#01）：characters[].inventory = [{ id, quantity? }] → id → 数量
            let mut inventory = serde_json::Map::new();
            if let Some(entries) = c.get("inventory").and_then(|v| v.as_array()) {
                for entry in entries {
                    if let Some(item_id) = entry.get("id").and_then(|v| v.as_str()) {
                        let qty = entry.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
                        inventory.insert(item_id.to_string(), Value::from(qty));
                    }
                }
            }
            // 存档内以「角色实例 id」为寻址键 —— 开档后角色与故事书模板解耦
            // （属性 / 资源随游戏进程变化）；template_id 只作为出身引用（立绘 / 定义）。
            let instance_id = format!("inst-{id}");
            characters.insert(
                instance_id.clone(),
                CharacterInstance {
                    instance_id,
                    template_id: id,
                    name,
                    kind,
                    attributes,
                    resources,
                    inventory,
                    location_id: None,
                    present,
                    statuses: vec![],
                },
            );
        }
    }
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
        .map(|c| vec![c.instance_id.clone()])
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
        encounters: BTreeMap::new(),
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
        .route("/api/config", get(config::get_config).put(config::put_config))
        .route("/api/assets", post(upload_asset))
        .route("/api/assets/{name}", get(get_asset))
        .route("/api/providers/probe", post(providers::probe_models))
        .route("/api/providers/test", post(providers::test_provider))
        .route("/api/pair/chat", post(pair::pair_chat))
        .route("/api/pair/chat/stream", post(pair::pair_chat_stream))
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
        .route(
            "/api/storybooks/{id}/pair/threads",
            get(list_pair_threads).post(create_pair_thread),
        )
        .route(
            "/api/pair/threads/{thread_id}",
            patch(rename_pair_thread).delete(delete_pair_thread),
        )
        .route(
            "/api/pair/threads/{thread_id}/messages",
            get(list_pair_messages)
                .post(append_pair_messages)
                .delete(clear_pair_messages),
        )
        .route(
            "/api/pair/threads/{thread_id}/pending",
            put(set_pair_thread_pending),
        )
        .route("/api/validate", post(validate_endpoint))
        .route("/api/lua/run", post(run_lua))
        .route("/api/saves", get(list_saves).post(create_save))
        .route(
            "/api/saves/{id}",
            get(get_save).patch(rename_save).delete(delete_save),
        )
        .route("/api/saves/{id}/state", get(get_state))
        .route("/api/saves/{id}/history", get(get_history))
        .route("/api/saves/{id}/stream", get(stream))
        .route("/api/saves/{id}/rounds", post(submit_round))
        .route("/api/saves/{id}/rerun", post(rerun_round))
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
        .route("/api/saves/{id}/rest", post(rest_save))
        .route("/api/saves/{id}/export", get(export_save))
        .route("/api/saves/import", post(import_save))
        .route("/api/saves/{id}/save", post(manual_save))
        .route("/api/saves/{id}/origin", post(new_origin))
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(logging::http_trace_layer())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "octopus-api" }))
}

/// 编辑期 Lua 试跑（#23）：先静态预检，再在沙箱里执行。
/// mode = condition（返回 bool）| check（返回归一化 total/margin）| hook（返回写请求列表）。
async fn run_lua(Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    let script = req.get("script").and_then(Value::as_str).unwrap_or("");
    if script.trim().is_empty() {
        return Ok(Json(json!({ "ok": false, "error": "脚本为空" })));
    }
    if let Err(e) = new_lint_state().and_then(|lua| lint_script(&lua, script)) {
        return Ok(Json(json!({ "ok": false, "error": e })));
    }
    let host = LuaHost::new(0)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "lua_init", e.to_string()))?;
    let mode = req.get("mode").and_then(Value::as_str).unwrap_or("check");
    let lua_ctx = LuaHostContext {
        script_id: req.get("script_id").and_then(Value::as_str).unwrap_or("editor").to_string(),
        actor_id: req
            .get("actor_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| req.pointer("/actor/id").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default(),
        actor: req.get("actor").cloned().unwrap_or(json!({})),
        target_id: req
            .get("target_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| req.pointer("/target/id").and_then(Value::as_str).map(str::to_string)),
        target: req.get("target").cloned(),
        skill: req.get("skill").cloned(),
        scene_id: req.get("scene_id").and_then(Value::as_str).unwrap_or("").to_string(),
        round: req.get("round").and_then(Value::as_u64).unwrap_or(0) as u32,
        difficulty: req.get("difficulty").and_then(Value::as_i64),
        relationships: req
            .get("relationships")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        // 编辑器试跑：协议插件需要的在场 / 受控快照（缺省为空）。
        present: req
            .get("present")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        controlled: req
            .get("controlled")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    };
    let outcome: Result<Value, String> = match mode {
        "condition" => host
            .run_condition(script, &lua_ctx)
            .map(|v| json!({ "value": v }))
            .map_err(|e| e.to_string()),
        "hook" => {
            let mount = LuaMount::from_str(
                req.get("mount").and_then(Value::as_str).unwrap_or("pre_resolve"),
            );
            host.run_hook(script, mount, &lua_ctx)
                .map(|_| json!({}))
                .map_err(|e| e.to_string())
        }
        _ => host
            .run_check(script, &lua_ctx)
            .map(|o| json!({ "total": o.total, "margin": o.margin }))
            .map_err(|e| e.to_string()),
    };
    let requests: Vec<Value> = host
        .drain_requests()
        .iter()
        .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
        .collect();
    Ok(Json(match outcome {
        Ok(result) => json!({ "ok": true, "mode": mode, "result": result, "requests": requests }),
        Err(error) => json!({ "ok": false, "mode": mode, "error": error, "requests": requests }),
    }))
}

// ============================================================
// 资产库（#28）：图片上传与读取
// 上传走裸 body（前端已用 canvas 压到目标尺寸），类型按魔数判定，
// 名字由内容 sha256 决定 —— 同名即同图，重复上传自然去重。
// ============================================================

async fn upload_asset(
    State(app): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let stored = app.assets().put(&body).await?;
    Ok(Json(json!({
        "asset": stored.name,
        "size": stored.size,
        "url": format!("/api/assets/{}", stored.name),
    })))
}

async fn get_asset(
    State(app): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let bytes = app.assets().read(&name).await?;
    let headers = [
        (axum::http::header::CONTENT_TYPE, content_type_of(&name).to_string()),
        // 内容寻址 → 同一名字的字节永不变，可长期强缓存
        (
            axum::http::header::CACHE_CONTROL,
            "public, max-age=31536000, immutable".to_string(),
        ),
    ];
    Ok((headers, bytes))
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
        "schema_version": octopus_engine::STORYBOOK_SCHEMA_VERSION,
        "meta": {
            "title": req.title.as_deref().unwrap_or("未命名故事书"),
        },
        "world": { "premise": "", "opening": "", "locations": [], "resources": [] },
        "attribute_dimensions": [],
        "skeleton": [],
        "characters": [],
        "skills": [],
        "items": [],
        "objects": [],
        "factions": [],
        "relationships": [],
        "statuses": [],
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
    let mut issues = octopus_engine::validate_storybook(&row.draft);
    // P2：Lua 协议的动态一致性检查只在发布门跑（草稿保存不跑，避免每次自动保存都执行 Lua）。
    if let Some(spec) = octopus_engine::ProtocolSpec::from_storybook(&row.draft) {
        if spec.is_lua() {
            if let Some(lua) = spec.lua.as_deref() {
                issues.extend(octopus_engine::check_protocol_conformance(lua));
            }
        }
    }
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
    let session = app.session_for(&id).await?;
    // 新游戏默认输出世界观里的开场白（事件日志第一/二条），不依赖客户端合成。
    session.emit_opening();
    if let Some(cid) = &req.controlled_character_id {
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
    let input = RoundInput {
        channel: req.channel,
        text: req.text,
        refs: req.refs.unwrap_or_default(),
    };
    let request_id = req.request_id;
    let focus = req.focus.unwrap_or_default();
    let sid = id.clone();
    // 202 立即返回，事件经 SSE 流出（#24 ①）
    tokio::spawn(async move {
        if let Err(e) = session.run_round(input, request_id, focus).await {
            tracing::warn!(save_id = %sid, error = %e, "回合执行失败");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize, Default)]
struct RerunRequest {
    #[serde(default)]
    focus: Option<Vec<FocusEntity>>,
    /// 编辑后重跑：用这段新文本替换原回合输入；缺省表示原样重跑。
    #[serde(default)]
    text: Option<String>,
}

/// 重跑最后一个玩家回合：归档旧回合命令，把会话回滚到回合前，再用原输入重跑。
/// 旧输出作为分支留在归档表（只读），活日志只保留回合前历史。
async fn rerun_round(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<RerunRequest>,
) -> Result<StatusCode, ApiError> {
    let session = app.session_for(&id).await?;
    if !session.can_rewind() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "round_in_progress",
            "回合进行中或有待确认动作，暂时不能重跑",
        ));
    }
    let Some(plan) = session.rewind_plan() else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "no_round", "没有可重跑的回合"));
    };
    // 先等旧回合全部落库，再归档：否则写队列里未落库的旧事件可能在归档后又写回。
    session.flush_events().await;
    app.store().archive_commands_from(&id, plan.from_seq).await?;
    session.apply_rewind(plan.from_seq, plan.round);
    let _ = app
        .store()
        .append_maintenance(&id, "重跑本轮", &format!("重跑第 {} 回合，旧输出已归档", plan.round))
        .await;

    let focus = req.focus.unwrap_or_default();
    let sid = id.clone();
    // 编辑后重跑：替换输入文本，渠道与引用沿用原回合。
    let mut input = plan.input;
    if let Some(t) = req.text.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        input.text = t.to_string();
    }
    tokio::spawn(async move {
        if let Err(e) = session.run_round(input, None, focus).await {
            tracing::warn!(save_id = %sid, error = %e, "重跑回合执行失败");
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
    let (model_provider_id, model, reasoning_effort) = app.store().get_save_model(&id).await?;
    let narrative = app.store().get_save_narrative(&id).await?;
    Ok(Json(SaveSettings { auto_confirm: v, model_provider_id, model, reasoning_effort, narrative }))
}

async fn put_settings(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SaveSettings>,
) -> Result<Json<SaveSettings>, ApiError> {
    app.store().set_auto_confirm(&id, body.auto_confirm).await?;
    app.store()
        .set_save_model(
            &id,
            body.model_provider_id.as_deref(),
            body.model.as_deref(),
            body.reasoning_effort.as_deref(),
        )
        .await?;
    // 叙述段玩家偏好：None 视作全用故事书默认（空 map）。只影响之后的回合。
    let narrative = body.narrative.clone().unwrap_or_default();
    app.store().set_save_narrative(&id, &narrative).await?;
    if let Ok(session) = app.session_for(&id).await {
        session.set_auto_confirm(body.auto_confirm);
        let model = match (&body.model_provider_id, &body.model) {
            (Some(provider_id), Some(model)) => Some(ModelRef {
                provider_id: provider_id.clone(),
                model: model.clone(),
                reasoning_effort: body.reasoning_effort.clone(),
            }),
            _ => None,
        };
        session.set_model(model);
        session.set_narrative_overrides(narrative);
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

#[derive(Deserialize)]
struct RestBody {
    /// short | long
    kind: String,
}

/// 休息（#4）：按 world.resources 的 natural_recovery 恢复受控角色资源，返回恢复明细。
async fn rest_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<RestBody>,
) -> Result<Json<Value>, ApiError> {
    let session = app.session_for(&id).await?;
    let kind = match body.kind.trim().to_ascii_lowercase().as_str() {
        "short" => octopus_engine::RestKind::Short,
        "long" => octopus_engine::RestKind::Long,
        other => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_rest_kind",
                format!("未知休息种类 {other}（expected short/long）"),
            ))
        }
    };
    let changes = session.rest(kind)?;
    Ok(Json(serde_json::json!({ "changes": changes })))
}

// ============================================================
// 结对会话线程（#23 ④）：一本故事书可有多条按主题隔离的会话
// ============================================================

#[derive(Deserialize)]
struct PairMessageInput {
    role: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    tools: Option<Value>,
    #[serde(default)]
    refs: Option<Vec<EntityRef>>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    attachments: Option<Value>,
}

#[derive(Deserialize)]
struct AppendPairMessagesRequest {
    messages: Vec<PairMessageInput>,
}

#[derive(Deserialize)]
struct CreatePairThreadRequest {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct RenamePairThreadRequest {
    title: String,
}

async fn ensure_storybook(app: &AppState, id: &str) -> Result<(), ApiError> {
    if app.store().get_storybook(id).await?.is_none() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "storybook_not_found",
            "故事书不存在",
        ));
    }
    Ok(())
}

async fn ensure_thread(app: &AppState, id: &str) -> Result<PairThreadRow, ApiError> {
    app.store().get_pair_thread(id).await?.ok_or_else(|| {
        ApiError::new(StatusCode::NOT_FOUND, "thread_not_found", "会话不存在")
    })
}

fn thread_record(t: PairThreadRow) -> PairThreadRecord {
    PairThreadRecord {
        id: t.id,
        storybook_id: t.storybook_id,
        title: t.title,
        message_count: t.message_count,
        created_at: t.created_at,
        updated_at: t.updated_at,
        pending_suggestions: t.pending_suggestions,
    }
}

#[derive(Deserialize)]
struct SetPendingSuggestionsRequest {
    #[serde(default)]
    suggestions: Option<Value>,
}

/// 覆盖线程的待审查改动：刷新 / 切会话后据此恢复，应用或放弃后同步收敛。
async fn set_pair_thread_pending(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(body): Json<SetPendingSuggestionsRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_thread(&app, &thread_id).await?;
    app.store()
        .set_pair_thread_pending(&thread_id, body.suggestions)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_pair_threads(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<PairThreadRecord>>, ApiError> {
    ensure_storybook(&app, &id).await?;
    let rows = app.store().list_pair_threads(&id).await?;
    Ok(Json(rows.into_iter().map(thread_record).collect()))
}

async fn create_pair_thread(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<CreatePairThreadRequest>,
) -> Result<(StatusCode, Json<PairThreadRecord>), ApiError> {
    ensure_storybook(&app, &id).await?;
    let t = app.store().create_pair_thread(&id, body.title.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(thread_record(t))))
}

async fn rename_pair_thread(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(body): Json<RenamePairThreadRequest>,
) -> Result<Json<PairThreadRecord>, ApiError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "empty_title", "标题不能为空"));
    }
    let t = app
        .store()
        .rename_pair_thread(&thread_id, title)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "thread_not_found", "会话不存在"))?;
    Ok(Json(thread_record(t)))
}

async fn delete_pair_thread(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if app.store().delete_pair_thread(&thread_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "thread_not_found", "会话不存在"))
    }
}

async fn list_pair_messages(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
) -> Result<Json<Vec<PairMessageRecord>>, ApiError> {
    ensure_thread(&app, &thread_id).await?;
    let rows = app.store().list_pair_messages(&thread_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|m| PairMessageRecord {
                seq: m.seq,
                role: m.role,
                content: m.content,
                model: m.model,
                is_error: m.is_error,
                tools: m.tools,
                refs: m.refs.and_then(|v| serde_json::from_value(v).ok()),
                reasoning: m.reasoning,
                attachments: m.attachments,
            })
            .collect(),
    ))
}

async fn append_pair_messages(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(body): Json<AppendPairMessagesRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_thread(&app, &thread_id).await?;
    let msgs: Vec<NewPairMessage> = body
        .messages
        .into_iter()
        .filter_map(|m| {
            let role = m.role.trim().to_string();
            if role != "user" && role != "assistant" {
                return None;
            }
            Some(NewPairMessage {
                role,
                content: m.content,
                model: m.model,
                is_error: m.is_error,
                tools: m.tools,
                refs: m.refs.map(|r| serde_json::to_value(r).unwrap_or(Value::Null)),
                reasoning: m.reasoning,
                attachments: m.attachments,
            })
        })
        .collect();
    app.store().append_pair_messages(&thread_id, &msgs).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_pair_messages(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ensure_thread(&app, &thread_id).await?;
    app.store().clear_pair_messages(&thread_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn manual_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SaveListItem>, ApiError> {
    app.store().touch_save(&id).await?;
    // v1 不落快照：历史与世界状态由命令日志重放恢复，这里只标记检查点时间。
    app.store()
        .append_maintenance(&id, "手动存档", "已标记检查点；历史由命令日志重放恢复（v1 不落快照）")
        .await?;
    app.store()
        .list_saves()
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))
}

/// 新原点（日志压缩 + 归档）v1 未实现：重放依赖完整命令日志，压缩会破坏找回语义。
/// 诚实返回 501，而不是假装成功。
async fn new_origin(
    State(_app): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "「新原点」压缩尚未实现：v1 依赖完整命令日志重放来恢复会话，暂不可用。",
    ))
}

async fn export_save(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg = app.store().export_save_package(&id).await?;
    // 打包要读资产文件，放阻塞线程池，别占住 async worker
    let assets = app.assets().clone();
    let bytes = tokio::task::spawn_blocking(move || pack_bundle(&pkg, &assets))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("打包任务失败: {e}")))??;
    let filename = format!("{id}.octopus.zip");
    let headers = [
        (axum::http::header::CONTENT_TYPE, "application/zip".to_string()),
        (
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        ),
    ];
    Ok((headers, bytes))
}

/// 导入：接受 zip 存档包（当前格式）或旧版单 JSON（历史导出，无资产）
async fn import_save(
    State(app): State<Arc<AppState>>,
    body: Bytes,
) -> Result<(StatusCode, Json<SaveListItem>), ApiError> {
    let (pkg, assets) = unpack_bundle(&body)?;
    if !assets.is_empty() {
        let n = app.assets().ingest(&assets).await?;
        tracing::info!(count = n, "导入存档包：资产已入库");
    }
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
    use octopus_ai::{ScriptedProvider, StubEmbedding};
    use octopus_types::{PlayEvent, StorybookListItem};
    use serde_json::json;

    async fn spawn_app() -> (String, Arc<AppState>) {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        let ai = Arc::new(ScriptedProvider);
        let assets_dir = std::env::temp_dir().join(format!("octopus-test-assets-{}", uuid::Uuid::new_v4()));
        let assets = Arc::new(AssetStore::open(&assets_dir).await.unwrap());
        let state = AppState::new(store, ai, Arc::new(StubEmbedding::default()), assets);
        let app = router(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), state)
    }

    /// 在指定的数据库文件上起一个 AppState（用于模拟进程重启：同一文件、空的 sessions）。
    async fn spawn_app_with_db(db_path: &std::path::Path) -> String {
        let store = Arc::new(SqliteStore::open(db_path.to_str().unwrap()).await.unwrap());
        let ai = Arc::new(ScriptedProvider);
        let assets_dir =
            std::env::temp_dir().join(format!("octopus-test-assets-{}", uuid::Uuid::new_v4()));
        let assets = Arc::new(AssetStore::open(&assets_dir).await.unwrap());
        let state = AppState::new(store, ai, Arc::new(StubEmbedding::default()), assets);
        let app = router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// 无骨架的角色卡故事书：NPC 也要在场，否则角色 AI 无人可演、剧情不推进。
    #[test]
    fn initial_presence_falls_back_to_present_without_skeleton() {
        // 无骨架：全部人物在场
        let sb = json!({ "characters": [{ "id": "char-lucy", "kind": "npc" }] });
        assert!(is_initially_present(&sb, "char-lucy", "npc"));
        // 骨架声明了 present_char_ids：以声明为准
        let sb2 = json!({ "skeleton": [{ "scenes": [{ "id": "sc1", "present_char_ids": ["char-a"] }] }] });
        assert!(is_initially_present(&sb2, "char-a", "npc"));
        assert!(!is_initially_present(&sb2, "char-b", "npc"));
        assert!(is_initially_present(&sb2, "char-pc", "pc"), "PC 恒在场");
        // 显式空名单：作者说这一幕没人
        let sb3 = json!({ "skeleton": [{ "scenes": [{ "id": "sc1", "present_char_ids": [] }] }] });
        assert!(!is_initially_present(&sb3, "char-a", "npc"));
    }

    /// 验收锚点：杀掉后端进程 → 重启 → 打开存档，叙事历史与世界状态原样还在，且不重跑 AI。
    #[tokio::test]
    async fn test_session_survives_restart_via_command_log() {
        let db_path = std::env::temp_dir()
            .join(format!("octopus-restart-{}.db", uuid::Uuid::new_v4()));
        let base1 = spawn_app_with_db(&db_path).await;
        let client = reqwest::Client::new();

        let res = client
            .post(format!("{base1}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("重启验证".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let detail: SaveDetail = res.json().await.unwrap();
        let save_id = detail.item.id;

        // 元指令 /免确认：写入一条 confirm_toggle 事件（世界状态变更经命令日志承载）。
        let res = client
            .post(format!("{base1}/api/saves/{save_id}/rounds"))
            .json(&json!({ "channel": "meta", "text": "/免确认", "request_id": "req-meta-1" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;

        // 普通回合：产生叙事事件。
        let res = client
            .post(format!("{base1}/api/saves/{save_id}/rounds"))
            .json(&json!({
                "channel": "character",
                "text": "你好",
                "request_id": "req-char-1",
                "refs": [{ "kind": "character", "id": "char-mira", "name": "米拉" }],
                "focus": [{ "kind": "character", "id": "char-mira", "name": "米拉", "entity": { "id": "char-mira", "name": "米拉" } }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);

        // 等命令日志落库（写任务异步，轮询到至少两个回合的事件）。
        let mut history: Option<HistoryPage> = None;
        for _ in 0..100 {
            let h: HistoryPage = client
                .get(format!("{base1}/api/saves/{save_id}/history?limit=200"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let enough = h.events.len() >= 5
                && h.events.iter().any(|e| {
                    matches!(&e.event, PlayEvent::System(p) if p.code.as_deref() == Some("confirm_toggle"))
                })
                && h.events.iter().any(|e| {
                    matches!(&e.event, PlayEvent::RoundEnd(_))
                });
            history = Some(h);
            if enough {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let history = history.unwrap();
        assert!(
            history.events.len() >= 5,
            "命令日志应至少包含两个回合的事件，实际 {}",
            history.events.len()
        );

        let state_before: octopus_types::WorldProjection = client
            .get(format!("{base1}/api/saves/{save_id}/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(state_before.meta.auto_confirm, "免确认应在回合内生效");

        // —— 模拟进程重启：同一数据库文件上起新的 AppState（空 sessions）——
        let base2 = spawn_app_with_db(&db_path).await;
        let state_after: octopus_types::WorldProjection = client
            .get(format!("{base2}/api/saves/{save_id}/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(state_after.seq, state_before.seq, "重启后 seq 应延续");
        assert!(
            state_after.meta.auto_confirm,
            "重启后免确认状态应由命令日志重放恢复"
        );

        let history_after: HistoryPage = client
            .get(format!("{base2}/api/saves/{save_id}/history?limit=200"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            history_after.events.len(),
            history.events.len(),
            "重启后历史条目数应一致"
        );
        assert_eq!(
            history_after.events.first().unwrap().id,
            history.events.first().unwrap().id,
            "重启后历史条目 id 应稳定"
        );
        assert!(
            history_after
                .events
                .iter()
                .any(|e| matches!(&e.event, PlayEvent::RoundStart(p) if !p.input.refs.is_empty())),
            "重启后 round_start 应保留玩家引用的实体"
        );

        let _ = std::fs::remove_file(&db_path);
    }

    /// 首页草稿可见性的契约：默认（released_only=true）只含已发布；
    /// 显式 released_only=false 必须把未发布草稿也列出来（带 published=false）。
    #[tokio::test]
    async fn test_api_storybook_list_includes_drafts_when_requested() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        let res = client
            .post(format!("{base_url}/api/storybooks"))
            .json(&CreateStorybookRequest {
                title: Some("草稿可见性".to_string()),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let doc: StorybookDocument = res.json().await.unwrap();
        assert!(!doc.published);

        let published: Vec<StorybookListItem> = client
            .get(format!("{base_url}/api/storybooks?released_only=true"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            published.iter().all(|s| s.published),
            "默认列表应只含已发布故事书"
        );
        assert!(published.iter().all(|s| s.id != doc.id));

        let all: Vec<StorybookListItem> = client
            .get(format!("{base_url}/api/storybooks?released_only=false"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let draft = all
            .iter()
            .find(|s| s.id == doc.id)
            .expect("草稿必须出现在 released_only=false 的列表里");
        assert!(!draft.published, "草稿应带 published=false");
    }

    /// 待审查改动随线程落库：刷新 / 切会话后可恢复；清空后为 None。
    #[tokio::test]
    async fn test_api_pair_thread_pending_suggestions_persist() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();
        let sb = "sb-fallingstar";

        let t: PairThreadRecord = client
            .post(format!("{base_url}/api/storybooks/{sb}/pair/threads"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let res = client
            .put(format!("{base_url}/api/pair/threads/{}/pending", t.id))
            .json(&json!({ "suggestions": [{ "id": "s1", "label": "更新属性维度", "action": "update" }] }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        let threads: Vec<PairThreadRecord> = client
            .get(format!("{base_url}/api/storybooks/{sb}/pair/threads"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let pend = threads
            .iter()
            .find(|x| x.id == t.id)
            .and_then(|x| x.pending_suggestions.clone())
            .expect("待审查改动应随线程持久化");
        assert_eq!(pend[0]["label"], "更新属性维度");

        let res = client
            .put(format!("{base_url}/api/pair/threads/{}/pending", t.id))
            .json(&json!({ "suggestions": null }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let threads: Vec<PairThreadRecord> = client
            .get(format!("{base_url}/api/storybooks/{sb}/pair/threads"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            threads.iter().find(|x| x.id == t.id).unwrap().pending_suggestions.is_none(),
            "清空后应为 None"
        );
    }

    /// 删除故事书：级联清掉结对线程；既有存档因内嵌冻结副本而保持可玩（自包含）。
    #[tokio::test]
    async fn test_api_delete_storybook_drops_threads_keeps_saves() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        // 基于已发布故事书开一档（存档内嵌故事书冻结副本）
        let save: SaveDetail = client
            .post(format!("{base_url}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("删书前开档".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = save.item.id;

        // 给它建一条结对线程
        client
            .post(format!("{base_url}/api/storybooks/sb-fallingstar/pair/threads"))
            .json(&json!({}))
            .send()
            .await
            .unwrap();

        // 删除故事书
        let res = client
            .delete(format!("{base_url}/api/storybooks/sb-fallingstar"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // 故事书与其结对线程都没了
        let res = client
            .get(format!("{base_url}/api/storybooks/sb-fallingstar"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let res = client
            .get(format!("{base_url}/api/storybooks/sb-fallingstar/pair/threads"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // 但既有存档仍可读——自包含，不依赖故事书行
        let res = client
            .get(format!("{base_url}/api/saves/{save_id}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "删除故事书不应影响既有存档");
    }

    #[tokio::test]
    async fn test_api_pair_threads_and_messages() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();
        let sb = "sb-fallingstar";

        // 两条独立会话：一条默认命名、一条手动命名。
        let t1: PairThreadRecord = client
            .post(format!("{base_url}/api/storybooks/{sb}/pair/threads"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let t2: PairThreadRecord = client
            .post(format!("{base_url}/api/storybooks/{sb}/pair/threads"))
            .json(&json!({ "title": "物品线" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(t1.id.starts_with("pt-"));
        assert_eq!(t1.title, "新会话");
        assert_eq!(t2.title, "物品线");

        // 往 t1 追加消息（含非法 role，应被过滤）。
        let res = client
            .post(format!("{base_url}/api/pair/threads/{}/messages", t1.id))
            .json(&json!({ "messages": [
                { "role": "user", "content": "帮我加一个酒馆", "refs": [{ "kind": "location", "id": "loc-tavern", "name": "酒馆" }] },
                { "role": "assistant", "content": "可以，建议这样…", "model": "test-model" },
                { "role": "bogus", "content": "应被忽略" }
            ]}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        let rows: Vec<PairMessageRecord> = client
            .get(format!("{base_url}/api/pair/threads/{}/messages", t1.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "非法 role 应被过滤");
        assert_eq!(rows[0].seq, 1);
        assert_eq!(rows[0].role, "user");
        // 实体引用（精准指向）随消息往返持久化。
        let refs = rows[0].refs.as_ref().expect("refs 应被持久化");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, "location");
        assert_eq!(refs[0].id.as_deref(), Some("loc-tavern"));
        assert_eq!(rows[1].model.as_deref(), Some("test-model"));

        // 线程列表：消息计数 + 首条用户消息自动补标题。
        let threads: Vec<PairThreadRecord> = client
            .get(format!("{base_url}/api/storybooks/{sb}/pair/threads"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(threads.len(), 2);
        let t1_after = threads.iter().find(|t| t.id == t1.id).unwrap();
        assert_eq!(t1_after.message_count, 2);
        assert_eq!(t1_after.title, "帮我加一个酒馆");
        let t2_after = threads.iter().find(|t| t.id == t2.id).unwrap();
        assert_eq!(t2_after.message_count, 0, "另一条线程消息数应为 0");
        assert_eq!(t2_after.title, "物品线", "手动命名的标题不应被自动覆盖");

        // 重命名。
        let renamed: PairThreadRecord = client
            .patch(format!("{base_url}/api/pair/threads/{}", t1.id))
            .json(&json!({ "title": "任务线" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(renamed.title, "任务线");

        // 删除 t2，不影响 t1 的消息。
        let res = client
            .delete(format!("{base_url}/api/pair/threads/{}", t2.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let rows: Vec<PairMessageRecord> = client
            .get(format!("{base_url}/api/pair/threads/{}/messages", t1.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "删除另一条线程不应影响本线程消息");
        let res = client
            .get(format!("{base_url}/api/pair/threads/{}/messages", t2.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
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
    async fn test_api_publish_runs_protocol_conformance() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();
        let res = client
            .post(format!("{base_url}/api/storybooks"))
            .json(&CreateStorybookRequest { title: Some("协议一致性".to_string()) })
            .send()
            .await
            .unwrap();
        let doc: StorybookDocument = res.json().await.unwrap();

        // 静态合法但语义不达标的 Lua 协议（parse 永远返回空 intents）→ 发布门应拒绝。
        let bad = json!({
            "schema_version": 1,
            "meta": { "id": doc.id, "title": "协议一致性" },
            "narrative": { "protocol": { "mode": "lua",
                "lua": "function protocol.preamble(ctx) return '' end\nfunction protocol.parse(raw) return { intents = {} } end" } }
        });
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest { draft: bad, base_version: 1 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = client
            .post(format!("{base_url}/api/storybooks/{}/publish", doc.id))
            .json(&PublishRequest { base_version: 2 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let err: ApiErrorBody = res.json().await.unwrap();
        assert_eq!(err.code, "validation_failed");
        let issues = err.detail.unwrap()["issues"].clone();
        assert!(
            issues.as_array().unwrap().iter().any(|i| i["code"] == "protocol_conformance_failed"),
            "发布必须跑动态一致性：{issues}"
        );

        // 草稿保存不跑动态一致性：同样的坏协议在 PUT 返回里不应出现 conformance 问题。
        // （上面的 PUT 已证明；这里改为修成能通过三组样例的插件并成功发布。）
        let good = json!({
            "schema_version": 1,
            "meta": { "id": doc.id, "title": "协议一致性" },
            "narrative": { "protocol": { "mode": "lua",
                "lua": "function protocol.preamble(ctx) return 'json' end\nfunction protocol.parse(raw)\n  if not raw:find('[', 1, true) then return { error = 'no intents' } end\n  return { intents = { { type = 'narrate', content = 'ok' } } }\nend" } }
        });
        let res = client
            .put(format!("{base_url}/api/storybooks/{}", doc.id))
            .json(&SaveDraftRequest { draft: good, base_version: 2 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = client
            .post(format!("{base_url}/api/storybooks/{}/publish", doc.id))
            .json(&PublishRequest { base_version: 3 })
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "通过一致性的 Lua 协议应可发布");
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
        assert!(content_disp.contains(&format!("{save_id}.octopus.zip")));
        assert_eq!(
            export_res.headers().get("content-type").unwrap(),
            "application/zip"
        );
        let zip_bytes = export_res.bytes().await.unwrap();
        // 存档包 = zip（save.json + assets/），解包后校验清单
        let (pkg, _assets) = unpack_bundle(&zip_bytes).unwrap();
        assert_eq!(pkg.format, "octopus-save-package");
        assert_eq!(pkg.save.item.title, "待导出存档");

        // 3. 导入（重新上传同一个 zip → 产生新记录）
        let import_res = client
            .post(format!("{base_url}/api/saves/import"))
            .header("content-type", "application/zip")
            .body(zip_bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(import_res.status(), StatusCode::CREATED);
        let imported_item: SaveListItem = import_res.json().await.unwrap();
        assert_eq!(imported_item.imported, Some(true));
        assert!(imported_item.title.contains("(导入)"));
    }

    /// 伪 PNG：服务端只看魔数（不解码），足够验证内容寻址与去重
    fn fake_png(tag: &[u8]) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(tag);
        v
    }

    #[tokio::test]
    async fn test_api_asset_upload_dedup_and_serve() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();

        let png = fake_png(b"cover-bytes");
        let up1: Value = client
            .post(format!("{base_url}/api/assets"))
            .header("content-type", "image/png")
            .body(png.clone())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let name = up1["asset"].as_str().unwrap().to_string();
        assert!(name.ends_with(".png"));
        assert_eq!(up1["size"].as_u64().unwrap(), png.len() as u64);

        // 同内容再传一次：内容寻址 → 同名（去重）
        let up2: Value = client
            .post(format!("{base_url}/api/assets"))
            .header("content-type", "image/png")
            .body(png.clone())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(up2["asset"].as_str().unwrap(), name);

        // 不同内容 → 不同资产
        let up3: Value = client
            .post(format!("{base_url}/api/assets"))
            .header("content-type", "image/png")
            .body(fake_png(b"another"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_ne!(up3["asset"].as_str().unwrap(), name);

        // 读回：字节一致 + 正确的 content-type + 强缓存
        let res = client
            .get(format!("{base_url}/api/assets/{name}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("content-type").unwrap(), "image/png");
        assert!(res.headers().get("cache-control").unwrap().to_str().unwrap().contains("immutable"));
        assert_eq!(res.bytes().await.unwrap().as_ref(), png.as_slice());

        // 非图片内容被拒
        let bad = client
            .post(format!("{base_url}/api/assets"))
            .header("content-type", "image/png")
            .body(b"not an image at all".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

        // 目录穿越 / 非法名被拒
        let traversal = client
            .get(format!("{base_url}/api/assets/..%2F..%2Foctopus.db"))
            .send()
            .await
            .unwrap();
        assert!(!traversal.status().is_success());
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

#[cfg(test)]
mod lua_run_tests {
    use super::*;

    async fn call(body: Value) -> Value {
        match run_lua(Json(body)).await {
            Ok(Json(v)) => v,
            Err(e) => json!({ "ok": false, "error": e.message }),
        }
    }

    #[tokio::test]
    async fn lua_run_supports_check_condition_and_hook() {
        let check = call(json!({ "script": "return { total = 18, margin = 6 }", "mode": "check" })).await;
        assert_eq!(check["ok"], json!(true));
        assert_eq!(check["result"]["total"], json!(18));
        assert_eq!(check["result"]["margin"], json!(6));

        let cond = call(json!({
            "script": "return host.round == 0 and host.difficulty == 12",
            "mode": "condition",
            "difficulty": 12
        }))
        .await;
        assert_eq!(cond["ok"], json!(true));
        assert_eq!(cond["result"]["value"], json!(true));

        let hook = call(json!({
            "script": "host.request_cost('mana', 5); host.trigger_event('boom')",
            "mode": "hook"
        }))
        .await;
        assert_eq!(hook["ok"], json!(true));
        let reqs = hook["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0]["Cost"]["resource"], json!("mana"));
        assert_eq!(reqs[1]["TriggerEvent"]["event"], json!("boom"));
    }

    #[tokio::test]
    async fn lua_run_rejects_bad_scripts() {
        let syntax = call(json!({ "script": "return (", "mode": "check" })).await;
        assert_eq!(syntax["ok"], json!(false));
        assert!(syntax["error"].as_str().unwrap().contains("语法错误"));

        let forbidden = call(json!({ "script": "os.execute('x')", "mode": "hook" })).await;
        assert_eq!(forbidden["ok"], json!(false));
        assert!(forbidden["error"].as_str().unwrap().contains("白名单"));

        let empty = call(json!({ "script": "  ", "mode": "check" })).await;
        assert_eq!(empty["ok"], json!(false));
    }
}

