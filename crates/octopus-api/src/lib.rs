//! octopus-api：axum 路由 / SSE 演出流 / 确认门 / 装配组合根（#17/#24/#20）。
//!
//! 组合根：注入 SqliteStore（#27 单库）与 AiProvider（ai crate）。

pub mod ai;
pub mod config;
pub mod error;
pub mod fetch;
pub mod logging;
pub mod pair;
pub mod prompts;
pub mod providers;

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use futures::Stream;
use octopus_engine::{
    AiProvider, AssetStore, ConversationStore, ConvRecord, EngineError, EventSink,
    FtsMemoryRetriever, LuaHost, LuaHostContext, LuaMount, ModelRef, NewPairMessage, PairThreadRow,
    SaveUpgradeWrite, Session, SnapshotBase, SnapshotRow, SqliteStore, StorybookRow,
    SummaryStore, WorldState, compute_upgrade_report, content_type_of,
    lint_script, new_lint_state, pack_bundle, unpack_bundle,
    validate_dispositions,
};
#[allow(unused_imports)]
use octopus_types::ApiErrorBody;
use octopus_types::{
    CharacterInstance, ConfirmRequest, CreateSaveRequest, CreateStorybookRequest, DeltaDomain,
    DeltaOp, EntityRef, EventEnvelope, FocusEntity, HistoryPage, IssueSeverity,
    LegacyDefinition, MaintenanceRow, NewOriginResult, PairMessageRecord, PairThreadRecord,
    PlayEvent, PlaytestRequest, ProjectionMeta, PublishRequest, RoundInput, SaveDetail,
    SaveDraftRequest, SaveListItem, SaveSettings, SkeletonProgress, StateDelta,
    StateUpdatePayload, StatusInstance, StorybookDocument, SubmitRoundRequest, SystemLevel,
    SystemPayload, UpgradeDisposition, UpgradeReport, UpgradeRequest, UpgradeResult, ValidateResult,
    ValidationIssue,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

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

/// 摘要落库端口实现（#05 §3.2/§3.3）：写派生表 + FTS5。
///
/// 组合根在建立会话时注入；端口方法返回错误由 Session 只记 warn，绝不影响权威回合。
struct StoreSummaryStore {
    store: Arc<SqliteStore>,
}

#[async_trait::async_trait]
impl SummaryStore for StoreSummaryStore {
    async fn put_round_summary(
        &self,
        save_id: &str,
        round: u32,
        text: &str,
    ) -> Result<(), EngineError> {
        self.store.upsert_round_summary(save_id, round, text).await?;
        Ok(())
    }

    async fn round_summaries_after(
        &self,
        save_id: &str,
        after_round: u32,
    ) -> Result<Vec<(u32, String)>, EngineError> {
        self.store.round_summaries_after(save_id, after_round).await
    }

    async fn put_scene_summary(
        &self,
        save_id: &str,
        scene_id: &str,
        round: u32,
        text: &str,
    ) -> Result<(), EngineError> {
        self.store.upsert_scene_summary(save_id, scene_id, round, text).await?;
        Ok(())
    }
}

pub struct AppState {
    store: Arc<SqliteStore>,
    /// 资产库（#28）：图片内容寻址存于文件系统，不占数据库
    assets: Arc<AssetStore>,
    /// AI provider 槽：保存配置后热替换，后续回合立即用新模型（无需重启）。
    ai: Arc<RwLock<Arc<dyn AiProvider>>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    senders: Mutex<HashMap<String, broadcast::Sender<EventEnvelope>>>,
    /// 每回合 token 预算（0 = 不限）：保存配置后热更新到所有会话。
    token_budget: AtomicU32,
    /// 模型会话持久化（派生数据）：热替换 AI provider 时重新注入。
    conv_store: Arc<dyn ConversationStore>,
}

/// 旧 model_* 三列 → 单一模型；provider / model 任一缺失 → None（回落全局默认）。
fn model_from_legacy(
    provider_id: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
) -> Option<ModelRef> {
    match (provider_id, model) {
        (Some(provider_id), Some(model)) => Some(ModelRef {
            provider_id,
            model,
            reasoning_effort,
        }),
        _ => None,
    }
}

/// ConversationStore 的 SQLite 实现：每存档一行的会话 JSON 快照（派生数据）。
struct SqliteConversationStore {
    store: Arc<SqliteStore>,
}

#[async_trait::async_trait]
impl ConversationStore for SqliteConversationStore {
    async fn load(&self, save_id: &str) -> Result<Vec<ConvRecord>, EngineError> {
        self.store.load_ai_conversation(save_id).await
    }
    async fn save(&self, save_id: &str, records: &[ConvRecord]) -> Result<(), EngineError> {
        self.store.save_ai_conversation(save_id, records).await
    }
    async fn clear(&self, save_id: &str) -> Result<(), EngineError> {
        self.store.clear_ai_conversation(save_id).await
    }
}

impl AppState {
    /// 组装应用状态。
    pub fn new(
        store: Arc<SqliteStore>,
        ai: Arc<dyn AiProvider>,
        assets: Arc<AssetStore>,
    ) -> Arc<Self> {
        // 会话持久化端口：让「每存档一条会话」跨重启保持缓存前缀（派生数据）。
        let conv_store: Arc<dyn ConversationStore> =
            Arc::new(SqliteConversationStore { store: store.clone() });
        ai.set_conversation_store(conv_store.clone());
        Arc::new(Self {
            store,
            assets,
            ai: Arc::new(RwLock::new(ai)),
            sessions: Mutex::new(HashMap::new()),
            senders: Mutex::new(HashMap::new()),
            token_budget: AtomicU32::new(
                crate::config::load_config_from_disk()
                    .turn_token_budget
                    .unwrap_or(0),
            ),
            conv_store,
        })
    }

    pub fn store(&self) -> &Arc<SqliteStore> {
        &self.store
    }

    pub fn assets(&self) -> &Arc<AssetStore> {
        &self.assets
    }

    /// 热替换 AI provider：保存配置后调用，后续回合立即用新模型（无需重启进程/重建会话）。
    pub fn set_ai(&self, ai: Arc<dyn AiProvider>) {
        ai.set_conversation_store(self.conv_store.clone());
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

    /// 解析本存档生效的单一模型：旧 model_* 兼容列；缺失 → None，由 provider 回落全局默认。
    pub async fn resolve_save_model(&self, save_id: &str) -> Result<Option<ModelRef>, EngineError> {
        let (pid, mid, effort) = self.store.get_save_model(save_id).await?;
        Ok(model_from_legacy(pid, mid, effort))
    }

    /// 惰性建立会话（内存状态 + 事件日志 + 广播出口）。
    pub async fn session_for(&self, save_id: &str) -> Result<Arc<Session>, EngineError> {
        if let Some(s) = self
            .sessions
            .lock()
            .expect("sessions poisoned")
            .get(save_id)
        {
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
        // 启动缓存（#06 ②）：有可用快照就只重放其后的命令；任何异常一律回退全量重放。
        let max_seq = persisted.last().map(|p| p.envelope.seq).unwrap_or(0);
        let snapshot_base = load_snapshot_base(
            self.store.as_ref(),
            save_id,
            save.item.embedded_revision,
            max_seq,
        )
        .await;

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
        session.replay_from(&persisted, snapshot_base);
        // 每回合 token 预算：构造后按当前配置套用。
        session.set_token_budget(self.token_budget());
        // 本存档的模型（存 saves 表）：会话建立后立刻套用，后续回合即可用。
        if let Ok(model) = self.resolve_save_model(save_id).await {
            session.set_model(model);
        }
        // 叙述段玩家偏好（存 saves 表）：同样在会话建立后套用；不影响已重放的历史。
        if let Some(overrides) = self.store.get_save_narrative(save_id).await? {
            session.set_narrative_overrides(overrides);
        }
        // 摘要派生落库（#05 §3.2/§3.3）：写派生表 + FTS。
        session.set_summary_store(Some(Arc::new(StoreSummaryStore {
            store: self.store.clone(),
        })));
        // 相关往事检索（#05 §3.4）：FTS5 关键词召回；失败由检索器内部降级为空。
        session.set_memory_retriever(Some(Arc::new(FtsMemoryRetriever::new(self.store.clone()))));
        self.senders
            .lock()
            .expect("senders poisoned")
            .insert(save_id.to_string(), tx);
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .insert(save_id.to_string(), session.clone());
        Ok(session)
    }

    async fn drop_session(&self, save_id: &str) {
        // 会话被重建（新原点 / 升级 / 导入 / 删除）时，连带丢弃该存档的追加式模型会话：
        // 否则重建后的 AI 会把已归档的旧对话继续当上下文发出，缓存前缀也会残留。
        let ai = self.ai.read().ok().map(|g| g.clone());
        if let Some(ai) = ai {
            ai.clear_conversation(save_id).await;
        }
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .remove(save_id);
        self.senders
            .lock()
            .expect("senders poisoned")
            .remove(save_id);
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

/// 自动快照间隔（回合）：一回合通常产生多条命令，10 回合约等于设计里的「每 ~100 条命令」。
/// 手动存档另有强制快照；这是最便宜的正确答案——保留 5 份，磁盘有界。
const SNAPSHOT_EVERY_ROUNDS: u32 = 10;

/// 解析快照 state_json（OriginCheckpoint 形态）为回放基座；坏数据返回 None。
fn decode_snapshot_base(row: &SnapshotRow) -> Option<SnapshotBase> {
    let v: Value = serde_json::from_str(&row.state_json).ok()?;
    let state: WorldState = serde_json::from_value(v.get("state")?.clone()).ok()?;
    let scene_start_round = v.get("scene_start_round").and_then(Value::as_u64).unwrap_or(0) as u32;
    Some(SnapshotBase { seq: row.seq.max(0) as u64, state, scene_start_round })
}

/// 载入最新可用快照作为回放基座（#06 ②）；没有 / 过期 / 坏数据一律返回 None → 全量重放。
async fn load_snapshot_base(
    store: &SqliteStore,
    save_id: &str,
    revision: u32,
    max_seq: u64,
) -> Option<SnapshotBase> {
    let row = match store.latest_snapshot(save_id).await {
        Ok(r) => r?,
        Err(e) => {
            tracing::warn!(save_id = %save_id, error = %e, "读取快照失败，回退全量重放");
            return None;
        }
    };
    if !row.usable_for(revision) {
        tracing::info!(save_id = %save_id, "快照格式 / 版次过期，回退全量重放");
        return None;
    }
    if row.seq.max(0) as u64 > max_seq {
        tracing::warn!(save_id = %save_id, "快照 seq 超出日志，视为过期，回退全量重放");
        return None;
    }
    decode_snapshot_base(&row)
}

/// 回合结束后按间隔落一份全量快照（#06 ②）：派生缓存，失败只 warn，绝不影响回合。
async fn maybe_snapshot(app: &AppState, session: &Session, save_id: &str) {
    let round = session.current_round();
    if round == 0 || round % SNAPSHOT_EVERY_ROUNDS != 0 {
        return;
    }
    write_snapshot(app, session, save_id).await;
}

/// 把当前会话状态物化为一份快照。先 flush 事件再取状态，保证快照 seq 已在库里；
/// 写入 / 裁剪（保留最新 5 份）由存储层在同一事务完成。
async fn write_snapshot(app: &AppState, session: &Session, save_id: &str) {
    session.flush_events().await;
    let seq = session.current_seq() as i64;
    let revision = session.projection().meta.revision;
    let state_json = session.snapshot_value().to_string();
    if let Err(e) = app.store().put_snapshot(save_id, seq, revision, &state_json).await {
        tracing::warn!(save_id = %save_id, error = %e, "快照写入失败（派生缓存，忽略）");
    }
}

fn build_state(save: &SaveDetail) -> WorldState {
    let sb = &save.storybook;
    let scene = sb.pointer("/skeleton/0/scenes/0");
    let mut characters: BTreeMap<String, CharacterInstance> = BTreeMap::new();
    if let Some(arr) = sb.get("characters").and_then(|v| v.as_array()) {
        for c in arr {
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                continue;
            }
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let kind = c
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("npc")
                .to_string();
            let present = is_initially_present(sb, &id, &kind);
            let attributes = c
                .get("attributes")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let resources = c
                .get("resources")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
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
    // 存档遗留区（#14）：新版故事书删掉的人物旧定义仍存于存档；实例继续可用，
    // 因此按旧定义补建缺失的角色实例（升级检查点会再精确覆盖运行时数值）。
    for legacy in &save.legacy {
        if legacy.kind != "character" {
            continue;
        }
        let id = legacy.id.clone();
        if id.is_empty() {
            continue;
        }
        let instance_id = format!("inst-{id}");
        if characters.contains_key(&instance_id) {
            continue;
        }
        let c = &legacy.definition;
        let name = c
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_string();
        let kind = c
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("npc")
            .to_string();
        let attributes = c
            .get("attributes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let resources = c
            .get("resources")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut inventory = serde_json::Map::new();
        if let Some(entries) = c.get("inventory").and_then(Value::as_array) {
            for entry in entries {
                if let Some(item_id) = entry.get("id").and_then(Value::as_str) {
                    let qty = entry.get("quantity").and_then(Value::as_i64).unwrap_or(1);
                    inventory.insert(item_id.to_string(), Value::from(qty));
                }
            }
        }
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
                present: true,
                statuses: vec![],
            },
        );
    }
    let scene_id = scene
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
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
        // 新档 RNG 从 0 起；快照 / 检查点会在重放时覆盖它（#06 ②）。
        rng_position: 0,
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
        .route(
            "/api/config",
            get(config::get_config).put(config::put_config),
        )
        .route("/api/assets", post(upload_asset))
        .route("/api/assets/{name}", get(get_asset))
        .route("/api/providers/probe", post(providers::probe_models))
        .route("/api/providers/test", post(providers::test_provider))
        .route("/api/prompts", get(prompts::list_prompts))
        .route("/api/pair/chat", post(pair::pair_chat))
        .route("/api/fetch-url", post(fetch::fetch_url))
        .route("/api/pair/chat/stream", post(pair::pair_chat_stream))
        .route(
            "/api/storybooks",
            get(list_storybooks).post(create_storybook_draft),
        )
        .route(
            "/api/storybooks/{id}",
            get(get_storybook)
                .put(save_storybook_draft)
                .delete(delete_storybook),
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
        .route("/api/saves/{id}/rounds/cancel", post(cancel_round))
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
        .route("/api/saves/{id}/upgrade/dry-run", post(upgrade_dry_run))
        .route("/api/saves/{id}/upgrade", post(upgrade_execute))
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
        script_id: req
            .get("script_id")
            .and_then(Value::as_str)
            .unwrap_or("editor")
            .to_string(),
        actor_id: req
            .get("actor_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                req.pointer("/actor/id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        actor: req.get("actor").cloned().unwrap_or(json!({})),
        target_id: req
            .get("target_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                req.pointer("/target/id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        target: req.get("target").cloned(),
        skill: req.get("skill").cloned(),
        scene_id: req
            .get("scene_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
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
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
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
                req.get("mount")
                    .and_then(Value::as_str)
                    .unwrap_or("pre_resolve"),
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
        (
            axum::http::header::CONTENT_TYPE,
            content_type_of(&name).to_string(),
        ),
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
    let row = app.store().get_storybook(&id).await?.ok_or_else(|| {
        ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在")
    })?;
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
    let row = app
        .store()
        .create_storybook_draft(req.title.as_deref(), &initial)
        .await?;
    Ok((StatusCode::CREATED, Json(row_to_doc(row))))
}

async fn save_storybook_draft(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SaveDraftRequest>,
) -> Result<Json<StorybookMutationResponse>, ApiError> {
    let row = app
        .store()
        .save_draft(&id, &req.draft, req.base_version)
        .await?;
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
    let row = app.store().get_storybook(&id).await?.ok_or_else(|| {
        ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在")
    })?;
    let mut issues = octopus_engine::validate_storybook(&row.draft);
    // P2：Lua 协议的动态一致性检查只在发布门跑（草稿保存不跑，避免每次自动保存都执行 Lua）。
    if let Some(spec) = octopus_engine::ProtocolSpec::from_storybook(&row.draft) {
        if spec.is_lua() {
            if let Some(lua) = spec.lua.as_deref() {
                issues.extend(octopus_engine::check_protocol_conformance(lua));
            }
        }
    }
    if issues
        .iter()
        .any(|i| matches!(i.severity, IssueSeverity::Error))
    {
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
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "storybook_not_found",
            "故事书不存在",
        ))
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
        .ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在")
        })?;
    let is_sandbox = req.is_sandbox.unwrap_or(false);
    let (storybook_content, revision) = if is_sandbox {
        let issues = octopus_engine::validate_storybook(&sb.draft);
        if issues
            .iter()
            .any(|i| matches!(i.severity, IssueSeverity::Error))
        {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "草稿存在阻断性错误，无法开始沙箱试玩",
            )
            .with_detail(serde_json::json!({ "issues": issues })));
        }
        (sb.draft.clone(), sb.revision)
    } else {
        let released = sb.released.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "storybook_unpublished",
                "故事书尚未发布",
            )
        })?;
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
    let mut detail = SaveDetail {
        item,
        storybook: storybook_content,
        legacy: Vec::new(),
    };
    if let Some(cid) = &req.controlled_character_id {
        if let Some(arr) = detail
            .storybook
            .get_mut("characters")
            .and_then(|v| v.as_array_mut())
        {
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
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_title",
            "标题不能为空",
        ));
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
        app.drop_session(&id).await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "save_not_found",
            "存档不存在",
        ))
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
    Ok(Json(
        session.history(q.before_seq, q.limit.unwrap_or(50).min(200)),
    ))
}

async fn submit_round(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SubmitRoundRequest>,
) -> Result<StatusCode, ApiError> {
    let session = app.session_for(&id).await?;
    // 设计决策 #6：非 idle（thinking / resolving / waiting_confirm）提交必须**同步**返回 409，
    // 而不是 202 之后在 spawn 里静默吞掉——否则前端永远等不到 round_in_progress。
    // request_id 幂等去重仍在 `run_round` 内（重复提交同一 id 会正常返回 202）。
    if !session.is_idle() {
        // 幂等重复（同一 request_id）放行；否则视为并发的新回合 → 409。
        let duplicate = req
            .request_id
            .as_deref()
            .map(|rid| session.has_seen_request(rid))
            .unwrap_or(false);
        if !duplicate {
            return Err(EngineError::RoundInProgress.into());
        }
    }
    let input = RoundInput {
        channel: req.channel,
        text: req.text,
        refs: req.refs.unwrap_or_default(),
    };
    let request_id = req.request_id;
    let focus = req.focus.unwrap_or_default();
    let sid = id.clone();
    let app_for_snap = app.clone();
    // 202 立即返回，事件经 SSE 流出（#24 ①）
    tokio::spawn(async move {
        if let Err(e) = session.run_round(input, request_id, focus).await {
            tracing::warn!(save_id = %sid, error = %e, "回合执行失败");
        }
        maybe_snapshot(&app_for_snap, &session, &sid).await;
    });
    Ok(StatusCode::ACCEPTED)
}

/// 停止本回合的 AI 推理：取消在途调用。
///
/// 回合仍在进行时才有意义（否则 409）；取消后引擎会发 System（code=round_cancelled）
/// 与 RoundEnd，玩家可以立刻重新发送。
async fn cancel_round(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = app.session_for(&id).await?;
    if session.is_idle() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "no_round_in_progress",
            "当前没有正在进行的回合",
        ));
    }
    let ai = app.ai.read().expect("ai poisoned").clone();
    ai.cancel(&id);
    tracing::info!(save_id = %id, "收到停止请求：已取消在途 AI 调用");
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
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "no_round",
            "没有可重跑的回合",
        ));
    };
    // 先等旧回合全部落库，再归档：否则写队列里未落库的旧事件可能在归档后又写回。
    session.flush_events().await;
    app.store()
        .archive_commands_from(&id, plan.from_seq)
        .await?;
    session.apply_rewind(plan.from_seq, plan.round);
    let _ = app
        .store()
        .append_maintenance(
            &id,
            "重跑本轮",
            &format!("重跑第 {} 回合，旧输出已归档", plan.round),
        )
        .await;

    let focus = req.focus.unwrap_or_default();
    let sid = id.clone();
    // 编辑后重跑：替换输入文本，渠道与引用沿用原回合。
    let mut input = plan.input;
    if let Some(t) = req.text.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        input.text = t.to_string();
    }
    let app_for_snap = app.clone();
    tokio::spawn(async move {
        if let Err(e) = session.run_round(input, None, focus).await {
            tracing::warn!(save_id = %sid, error = %e, "重跑回合执行失败");
        }
        maybe_snapshot(&app_for_snap, &session, &sid).await;
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
    Ok(Json(SaveSettings {
        auto_confirm: v,
        model_provider_id,
        model,
        reasoning_effort,
        narrative,
    }))
}

async fn put_settings(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SaveSettings>,
) -> Result<Json<SaveSettings>, ApiError> {
    app.store().set_auto_confirm(&id, body.auto_confirm).await?;
    // 本存档的单一模型（旧 model_* 兼容列）：直接写入。
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
        // 直接用刚写入的 body 计算，避免再查库。
        let model = model_from_legacy(
            body.model_provider_id.clone(),
            body.model.clone(),
            body.reasoning_effort.clone(),
        );
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
            ));
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
    app.store()
        .get_pair_thread(id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "thread_not_found", "会话不存在"))
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
    let t = app
        .store()
        .create_pair_thread(&id, body.title.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(thread_record(t))))
}

async fn rename_pair_thread(
    State(app): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(body): Json<RenamePairThreadRequest>,
) -> Result<Json<PairThreadRecord>, ApiError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_title",
            "标题不能为空",
        ));
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
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "thread_not_found",
            "会话不存在",
        ))
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
                refs: m
                    .refs
                    .map(|r| serde_json::to_value(r).unwrap_or(Value::Null)),
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
    // 手动存档同时写一份全量快照（#06 ②）：启动缓存，权威仍是命令日志。
    let session = app.session_for(&id).await?;
    session.flush_events().await;
    let seq = session.current_seq() as i64;
    let revision = session.projection().meta.revision;
    let state_json = session.snapshot_value().to_string();
    app.store().put_snapshot(&id, seq, revision, &state_json).await?;
    app.store()
        .append_maintenance(
            &id,
            "手动存档",
            "已写入全量快照（保留最新 5 份）；历史与权威状态仍由命令日志重放恢复",
        )
        .await?;
    app.store()
        .list_saves()
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))
}

/// 维护操作（升级 / 新原点）占用会话 busy 位；无论成功、报错还是 panic 都释放。
struct MaintenanceGuard {
    session: Arc<Session>,
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        self.session.end_maintenance();
    }
}

/// 自动备份包落盘目录：与资产库同级的 backups/。
/// 单库（#27）下「升级前自动备份」= 导出该存档为独立包（派生决定 2026-09-09）。
fn backup_dir(assets: &AssetStore) -> std::path::PathBuf {
    let root = assets.root();
    match root.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent.join("backups"),
        None => std::path::PathBuf::from("backups"),
    }
}

/// 把当前存档导出为独立包并落盘，返回文件名。失败即中止升级（存档保持原样）。
async fn write_upgrade_backup(app: &AppState, save_id: &str) -> Result<String, ApiError> {
    let pkg = app.store().export_save_package(save_id).await?;
    let assets = app.assets().clone();
    let bytes = tokio::task::spawn_blocking(move || pack_bundle(&pkg, &assets))
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("备份打包任务失败: {e}"),
            )
        })??;
    let dir = backup_dir(app.assets());
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("创建备份目录失败: {e}"),
        )
    })?;
    // 时间戳精确到纳秒，避免同一存档连续升级覆盖前一份备份。
    let stamp = octopus_engine::storage::now_iso().replace([':', 'T', '+'], "-");
    let name = format!("{save_id}-backup-{stamp}.octopus.zip");
    let path = dir.join(&name);
    tokio::fs::write(&path, &bytes).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("写入备份失败 {}: {e}", path.display()),
        )
    })?;
    Ok(name)
}

fn event_envelope(seq: u64, round: u32, ts: &str, event: PlayEvent) -> EventEnvelope {
    EventEnvelope {
        id: uuid::Uuid::new_v4().to_string(),
        seq,
        round,
        ts: ts.to_string(),
        actor: None,
        intent_id: None,
        event,
    }
}

fn origin_delta(save_id: &str, checkpoint: Value) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Origin,
        entity_id: save_id.to_string(),
        field: "state".to_string(),
        op: DeltaOp::Set,
        value: checkpoint,
    }
}

fn character_delta(character_id: &str, field: &str, value: Value) -> StateDelta {
    StateDelta {
        domain: DeltaDomain::Character,
        entity_id: character_id.to_string(),
        field: field.to_string(),
        op: DeltaOp::Set,
        value,
    }
}

/// 两段式升级第一段（#14 ②）：纯读 dry-run，列出消失人物与自动处理项。
async fn upgrade_dry_run(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UpgradeReport>, ApiError> {
    let save = app
        .store()
        .get_save(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    let sb = app
        .store()
        .get_storybook(&save.item.storybook_id)
        .await?
        .ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在")
        })?;
    let released = sb.released.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "storybook_unpublished",
            "故事书尚未发布，没有可升级的目标版次",
        )
    })?;
    Ok(Json(compute_upgrade_report(
        &save.storybook,
        &released,
        save.item.embedded_revision,
        sb.revision,
    )))
}

/// 两段式升级第二段（#14 ②）：备份 -> 换内嵌故事书 -> 应用裁决 -> 维护历史。
///
/// 全部数据库写入在一个事务里；升级引起的世界状态变更（全量检查点 + 人物离场）
/// 作为引擎命令进日志，重放即得一致状态——升级本身不依赖重跑 diff。
async fn upgrade_execute(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpgradeRequest>,
) -> Result<Json<UpgradeResult>, ApiError> {
    let save = app
        .store()
        .get_save(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    let sb = app
        .store()
        .get_storybook(&save.item.storybook_id)
        .await?
        .ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "storybook_not_found", "故事书不存在")
        })?;
    let released = sb.released.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "storybook_unpublished",
            "故事书尚未发布，没有可升级的目标版次",
        )
    })?;
    let from = save.item.embedded_revision;
    let to = sb.revision;

    // 幂等：已到 / 超过目标版次直接返回，不再备份、不再写历史。
    if from >= to {
        return Ok(Json(UpgradeResult {
            detail: save,
            backup_name: String::new(),
        }));
    }

    let report = compute_upgrade_report(&save.storybook, &released, from, to);
    if let Err(e) = validate_dispositions(&report, &req.dispositions) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, e.code(), e.message()));
    }

    // 会话必须空闲：占住维护位，并发回合（submit_round）会拿到 409。
    let session = app.session_for(&id).await?;
    if !session.try_begin_maintenance() {
        return Err(EngineError::RoundInProgress.into());
    }
    let _guard = MaintenanceGuard {
        session: session.clone(),
    };
    session.flush_events().await;

    // (a) 自动备份必须先于任何写入；失败即中止，存档不动。
    let backup_name = write_upgrade_backup(&app, &id).await?;

    // (b) 组装升级事件：全量检查点 + 人物裁决。
    let checkpoint = session.snapshot_value();
    let round = session.current_round();
    let now = octopus_engine::storage::now_iso();
    let save_id = id.clone();
    let mut seq = session.current_seq() + 1;
    let mut events: Vec<EventEnvelope> = Vec::new();
    events.push(event_envelope(
        seq,
        round,
        &now,
        PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![origin_delta(&save_id, checkpoint)],
        }),
    ));
    seq += 1;

    // 消失人物的旧定义一律进遗留区：无论「遗留冻结」还是「叙事离场」，
    // 实例都要先能按旧定义重建（检查点已含运行时数值，这里保证基线可解释）。
    let mut old_chars: HashMap<String, Value> = HashMap::new();
    if let Some(arr) = save.storybook.get("characters").and_then(Value::as_array) {
        for c in arr {
            if let Some(cid) = c.get("id").and_then(Value::as_str) {
                old_chars.insert(cid.to_string(), c.clone());
            }
        }
    }
    let mut legacy: Vec<LegacyDefinition> = save.legacy.clone();
    for gc in &report.groups.gone_characters {
        if legacy
            .iter()
            .any(|l| l.kind == "character" && l.id == gc.character_id)
        {
            continue;
        }
        if let Some(def) = old_chars.get(&gc.character_id) {
            legacy.push(LegacyDefinition {
                kind: "character".to_string(),
                id: gc.character_id.clone(),
                name: gc.name.clone(),
                definition: def.clone(),
                frozen_at_revision: from,
            });
        }
    }

    let departure_count = req
        .dispositions
        .iter()
        .filter(|d| d.disposition == UpgradeDisposition::Departure)
        .count();
    let freeze_count = req
        .dispositions
        .iter()
        .filter(|d| d.disposition == UpgradeDisposition::Freeze)
        .count();

    for item in &req.dispositions {
        if item.disposition != UpgradeDisposition::Departure {
            continue;
        }
        let name = report
            .groups
            .gone_characters
            .iter()
            .find(|g| g.character_id == item.character_id)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| item.character_id.clone());
        let status = serde_json::to_value(StatusInstance {
            id: "departed".to_string(),
            name: "已离场".to_string(),
            turns_left: None,
            scenes_left: None,
        })
        .unwrap_or(Value::Null);
        events.push(event_envelope(
            seq,
            round,
            &now,
            PlayEvent::StateUpdate(StateUpdatePayload {
                changes: vec![
                    character_delta(&item.character_id, "present", Value::Bool(false)),
                    character_delta(&item.character_id, "status", status),
                ],
            }),
        ));
        seq += 1;
        // 注入给主线 AI 的离场提示：作为权威 System 事件落日志，重放 / 历史可见。
        events.push(event_envelope(
            seq,
            round,
            &now,
            PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: Some("upgrade_departure".to_string()),
                text: format!(
                    "人物「{name}」因故事书版次升级已离场：请在后续叙事中自然交代其离开。"
                ),
            }),
        ));
        seq += 1;
    }

    events.push(event_envelope(
        seq,
        round,
        &now,
        PlayEvent::System(SystemPayload {
            level: SystemLevel::Info,
            code: Some("upgrade".to_string()),
            text: format!(
                "存档已从版次 {from} 升级到 {to}：{departure_count} 名人物离场，{freeze_count} 名遗留冻结。"
            ),
        }),
    ));

    let write = SaveUpgradeWrite {
        storybook: released,
        storybook_title: sb.title.clone(),
        to_revision: to,
        legacy,
        events,
        maintenance_op: format!("升级 rev{from} → rev{to}"),
        maintenance_summary: format!(
            "备份 {backup_name}；{departure_count} 名人物离场，{freeze_count} 名遗留冻结"
        ),
    };
    app.store().apply_save_upgrade(&id, &write).await?;

    // 丢弃旧会话：下次访问会按新内嵌故事书重建并重放日志（含升级检查点）。
    app.drop_session(&id).await;
    let detail = app
        .store()
        .get_save(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "save_not_found", "存档不存在"))?;
    Ok(Json(UpgradeResult {
        detail,
        backup_name,
    }))
}

/// 新原点（#14 ④ 修订 / 决策 3）：以当前状态检查点为新起点，旧日志移入库内归档表（只读）。
///
/// 检查点作为活日志第一条承载全量状态，因此不依赖快照表也能保证重放正确；
/// 检查点、归档、维护历史在一个事务里，杜绝半压缩状态。
async fn new_origin(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<NewOriginResult>, ApiError> {
    let session = app.session_for(&id).await?;
    if !session.try_begin_maintenance() {
        return Err(EngineError::RoundInProgress.into());
    }
    let _guard = MaintenanceGuard {
        session: session.clone(),
    };
    session.flush_events().await;

    let max_seq = session.current_seq();
    if max_seq == 0 {
        // 还没有任何权威事件：无可归档，也无状态可锚定，直接如实返回。
        return Ok(Json(NewOriginResult {
            ok: true,
            archived_count: 0,
            origin_seq: 0,
        }));
    }

    let origin_seq = max_seq + 1;
    let now = octopus_engine::storage::now_iso();
    let checkpoint = event_envelope(
        origin_seq,
        session.current_round(),
        &now,
        PlayEvent::StateUpdate(StateUpdatePayload {
            changes: vec![origin_delta(&id, session.snapshot_value())],
        }),
    );
    let summary = format!("以 seq {origin_seq} 为新起点；旧日志移入库内归档表（只读）");
    let archived_count = app.store().new_origin(&id, &checkpoint, &summary).await?;
    app.drop_session(&id).await;
    Ok(Json(NewOriginResult {
        ok: true,
        archived_count,
        origin_seq,
    }))
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
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("打包任务失败: {e}"),
            )
        })??;
    let filename = format!("{id}.octopus.zip");
    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            "application/zip".to_string(),
        ),
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
    use octopus_ai::ScriptedProvider;
    use octopus_engine::{AiOutput, MemoryRetriever, TurnContext};
    use octopus_types::{
        Intent, MaintenanceRow, NewOriginResult, PlayEvent, SaveDetail,
        StorybookListItem, UpgradeReport, UpgradeResult, WorldProjection,
    };
    use serde_json::{Value, json};

    async fn spawn_app() -> (String, Arc<AppState>) {
        spawn_app_with_ai(Arc::new(ScriptedProvider)).await
    }

    /// 用自定义 AI 起一个测试服务（并发/阻塞场景用）。
    async fn spawn_app_with_ai(ai: Arc<dyn AiProvider>) -> (String, Arc<AppState>) {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        let assets_dir =
            std::env::temp_dir().join(format!("octopus-test-assets-{}", uuid::Uuid::new_v4()));
        let assets = Arc::new(AssetStore::open(&assets_dir).await.unwrap());
        let state = AppState::new(store, ai, assets);
        let app = router(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), state)
    }

    /// 等某存档的回合彻底结算（权威日志出现 RoundEnd）再统计派生数据。
    ///
    /// 回合是异步处理的：test 里若一看到首个事件就统计条数，边跑边数会 flaky。
    /// RoundEnd 在全部叙事 / 摘要意图之后落库，见到它就说明该回合的派生写已发生。
    async fn wait_round_end(store: &SqliteStore, save_id: &str) {
        for _ in 0..200 {
            let events = store.load_events(save_id).await.unwrap();
            if events
                .iter()
                .any(|p| matches!(&p.envelope.event, PlayEvent::RoundEnd(_)))
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("回合未在预期时间内结算（未见 RoundEnd）");
    }

    /// 在指定的数据库文件上起一个 AppState（用于模拟进程重启：同一文件、空的 sessions）。
    async fn spawn_app_with_db(db_path: &std::path::Path) -> String {
        let store = Arc::new(SqliteStore::open(db_path.to_str().unwrap()).await.unwrap());
        let ai = Arc::new(ScriptedProvider);
        let assets_dir =
            std::env::temp_dir().join(format!("octopus-test-assets-{}", uuid::Uuid::new_v4()));
        let assets = Arc::new(AssetStore::open(&assets_dir).await.unwrap());
        let state = AppState::new(store, ai, assets);
        let app = router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// 测试辅助：创建故事书 -> 存草稿 -> 发布首版，返回 (storybook_id, 当前 draft_version)。
    async fn create_published_storybook(
        client: &reqwest::Client,
        base: &str,
        title: &str,
        draft: Value,
    ) -> (String, u32) {
        let created: StorybookDocument = client
            .post(format!("{base}/api/storybooks"))
            .json(&CreateStorybookRequest { title: Some(title.to_string()) })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut draft = draft;
        if let Some(meta) = draft.get_mut("meta").and_then(Value::as_object_mut) {
            meta.insert("id".to_string(), json!(created.id));
            meta.insert("title".to_string(), json!(title));
        }
        let saved: StorybookMutationResponse = client
            .put(format!("{base}/api/storybooks/{}", created.id))
            .json(&SaveDraftRequest {
                draft,
                base_version: created.draft_version,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let published: StorybookMutationResponse = client
            .post(format!("{base}/api/storybooks/{}/publish", created.id))
            .json(&PublishRequest {
                base_version: saved.doc.draft_version,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(published.doc.revision, 1);
        (created.id, published.doc.draft_version)
    }

    /// 测试辅助：在既有故事书上存新草稿并发布下一版，返回新 revision。
    async fn publish_next_revision(
        client: &reqwest::Client,
        base: &str,
        sb_id: &str,
        draft: Value,
        base_version: u32,
    ) -> u32 {
        let saved: StorybookMutationResponse = client
            .put(format!("{base}/api/storybooks/{sb_id}"))
            .json(&SaveDraftRequest {
                draft,
                base_version,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let published: StorybookMutationResponse = client
            .post(format!("{base}/api/storybooks/{sb_id}/publish"))
            .json(&PublishRequest {
                base_version: saved.doc.draft_version,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        published.doc.revision
    }

    /// #14：dry-run 列出消失人物；缺裁决执行被拒且不污染存档；正确执行后换版、备份、记历史。
    #[tokio::test]
    async fn test_api_save_upgrade_two_stage() {
        let (base, state) = spawn_app().await;
        let client = reqwest::Client::new();

        let draft_v1 = json!({
            "schema_version": 3,
            "meta": { "title": "升级测试书" },
            "world": { "premise": "故事开场。" },
            "characters": [
                { "id": "char-a", "name": "米拉", "kind": "pc" },
                { "id": "char-kael", "name": "凯尔", "kind": "npc" }
            ],
            "skills": [{ "id": "sk-a", "name": "痛饮" }],
            "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{
                "id": "sc1", "title": "开场", "present_char_ids": ["char-a", "char-kael"]
            }] }]
        });
        let (sb_id, after_v1) =
            create_published_storybook(&client, &base, "升级测试书", draft_v1).await;

        let detail: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: sb_id.clone(),
                title: Some("升级存档".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = detail.item.id.clone();
        assert_eq!(detail.item.embedded_revision, 1);
        assert!(!detail.item.needs_upgrade);

        // 新版：删掉凯尔、改痛饮定义，发布 rev2。
        let draft_v2 = json!({
            "schema_version": 3,
            "meta": { "id": sb_id, "title": "升级测试书" },
            "world": { "premise": "故事开场。" },
            "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }],
            "skills": [{ "id": "sk-a", "name": "痛饮", "cost": [{ "resource": "gold", "amount": 1 }] }],
            "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{
                "id": "sc1", "title": "开场", "present_char_ids": ["char-a"]
            }] }]
        });
        let rev = publish_next_revision(&client, &base, &sb_id, draft_v2, after_v1).await;
        assert_eq!(rev, 2);

        // needs_upgrade 读时计算为 true。
        let got: SaveDetail = client
            .get(format!("{base}/api/saves/{save_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(got.item.needs_upgrade);
        assert_eq!(got.item.latest_revision, 2);

        // dry-run 报告列出消失人物。
        let rep: UpgradeReport = client
            .post(format!("{base}/api/saves/{save_id}/upgrade/dry-run"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rep.from_revision, 1);
        assert_eq!(rep.to_revision, 2);
        assert!(
            rep.groups
                .gone_characters
                .iter()
                .any(|g| g.character_id == "char-kael"),
            "dry-run 必须列出消失的凯尔"
        );

        // 缺裁决执行 -> 400，且存档未变（升级前自动备份前就拒绝）。
        let res = client
            .post(format!("{base}/api/saves/{save_id}/upgrade"))
            .json(&json!({ "dispositions": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let err: Value = res.json().await.unwrap();
        assert_eq!(err["code"], "missing_dispositions");
        let untouched: SaveDetail = client
            .get(format!("{base}/api/saves/{save_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(untouched.item.embedded_revision, 1, "失败的升级不得改动存档");

        // 正确裁决 -> 执行。
        let res = client
            .post(format!("{base}/api/saves/{save_id}/upgrade"))
            .json(&json!({
                "dispositions": [{ "character_id": "char-kael", "disposition": "departure" }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let result: UpgradeResult = res.json().await.unwrap();
        assert_eq!(result.detail.item.embedded_revision, 2);
        assert!(!result.detail.item.needs_upgrade);
        assert!(
            result.detail.legacy.iter().any(|l| l.id == "char-kael"),
            "消失人物旧定义应进遗留区"
        );
        assert!(!result.backup_name.is_empty(), "应返回备份文件名");
        assert!(
            backup_dir(state.assets()).join(&result.backup_name).exists(),
            "自动备份包必须已落盘"
        );

        // 维护历史记录升级。
        let m: Vec<MaintenanceRow> = client
            .get(format!("{base}/api/saves/{save_id}/maintenance"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(m.iter().any(|r| r.op.contains("升级")));

        // 升级引起的世界变更进了命令日志（检查点 + 离场提示）。
        let events = state.store().load_events(&save_id).await.unwrap();
        assert!(events.iter().any(|p| matches!(&p.envelope.event, PlayEvent::StateUpdate(u) if u.changes.iter().any(|d| d.domain == DeltaDomain::Origin))));
        assert!(events.iter().any(|p| matches!(&p.envelope.event, PlayEvent::System(s) if s.code.as_deref() == Some("upgrade_departure"))));

        // 离场人物在投影中 present=false。
        let st: WorldProjection = client
            .get(format!("{base}/api/saves/{save_id}/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let kael = st.characters.get("inst-char-kael").expect("遗留实例应存在");
        assert_eq!(kael["present"], json!(false));
        assert!(st.characters.contains_key("inst-char-a"));

        // 幂等：重复执行不再备份、不再写历史。
        let history_len = m.len();
        let res = client
            .post(format!("{base}/api/saves/{save_id}/upgrade"))
            .json(&json!({
                "dispositions": [{ "character_id": "char-kael", "disposition": "departure" }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let again: UpgradeResult = res.json().await.unwrap();
        assert!(again.backup_name.is_empty(), "幂等重放不应再产生备份");
        let m2: Vec<MaintenanceRow> = client
            .get(format!("{base}/api/saves/{save_id}/maintenance"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(m2.len(), history_len, "幂等重放不应重复记历史");
    }

    /// #14：新原点把旧日志移入库内归档表，检查点保证重启后重放一致。
    #[tokio::test]
    async fn test_api_new_origin_archives_and_replays_across_restart() {
        let db_path =
            std::env::temp_dir().join(format!("octopus-origin-{}.db", uuid::Uuid::new_v4()));
        let base1 = spawn_app_with_db(&db_path).await;
        let client = reqwest::Client::new();

        let detail: SaveDetail = client
            .post(format!("{base1}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("新原点测试".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = detail.item.id.clone();

        let before: WorldProjection = client
            .get(format!("{base1}/api/saves/{save_id}/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let history_before: HistoryPage = client
            .get(format!("{base1}/api/saves/{save_id}/history?limit=200"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let n_before = history_before.events.len();
        assert!(n_before > 0, "开档应至少产出开场事件");

        let origin: NewOriginResult = client
            .post(format!("{base1}/api/saves/{save_id}/origin"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(origin.ok);
        assert_eq!(origin.archived_count as usize, n_before);
        assert_eq!(origin.origin_seq, before.seq + 1);

        // 活日志只剩检查点。
        let history_after: HistoryPage = client
            .get(format!("{base1}/api/saves/{save_id}/history?limit=200"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(history_after.events.len(), 1);

        let after: WorldProjection = client
            .get(format!("{base1}/api/saves/{save_id}/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(after.seq, origin.origin_seq);
        assert_eq!(after.scene_id, before.scene_id, "新原点不改变世界状态");

        // 重启（同一库文件、空会话）：只用检查点也能恢复。
        let base2 = spawn_app_with_db(&db_path).await;
        let restored: WorldProjection = client
            .get(format!("{base2}/api/saves/{save_id}/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(restored.seq, after.seq, "重启后 seq 应延续");
        assert_eq!(restored.scene_id, after.scene_id);
        assert_eq!(restored.characters.len(), after.characters.len());

        let m: Vec<MaintenanceRow> = client
            .get(format!("{base2}/api/saves/{save_id}/maintenance"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(m.iter().any(|r| r.op.contains("新原点")));

        let _ = std::fs::remove_file(&db_path);
    }

    /// 只产出「叙事 + 回合微摘要」的确定性 provider（验证摘要进索引与检索）。
    struct SummaryProvider;

    #[async_trait::async_trait]
    impl AiProvider for SummaryProvider {
        async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
            Ok(AiOutput {
                intents: vec![
                    Intent::Narrate { content: "矿洞里有一点微光。".into(), actor_id: None }.into(),
                    Intent::Summary { text: "在矿洞里发现微光".into() }.into(),
                ],
                reasoning: None,
                intent_warnings: vec![],
                trace: None,
                compaction: None,
            })
        }
        
    }

    /// #05 §3.2：微摘要写派生表、进 FTS5，并能被检索器召回。
    #[tokio::test]
    async fn round_summary_is_indexed_and_retrievable() {
        let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
        let assets_dir =
            std::env::temp_dir().join(format!("octopus-test-assets-{}", uuid::Uuid::new_v4()));
        let assets = Arc::new(AssetStore::open(&assets_dir).await.unwrap());
        let state = AppState::new(store.clone(), Arc::new(SummaryProvider), assets);
        let app = router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{addr}");
        let client = reqwest::Client::new();

        let detail: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("摘要索引测试".to_string()),
                controlled_character_id: None,
                is_sandbox: Some(true),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = detail.item.id.clone();

        let res = client
            .post(format!("{base}/api/saves/{save_id}/rounds"))
            .json(&json!({ "channel": "character", "text": "四处看看" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);

        // 等回合彻底结算（RoundEnd 在 summary 意图之后落库），再校验派生表。
        wait_round_end(&store, &save_id).await;
        let rounds = store.round_summaries_after(&save_id, 0).await.unwrap();
        assert_eq!(rounds.len(), 1, "summary 意图应写入 round_summaries");
        assert_eq!(rounds[0].1, "在矿洞里发现微光");

        // FTS 命中摘要（与叙事事件共用 events_fts）。
        let fts = store.search_events_fts(&save_id, "微光", 10).await.unwrap();
        assert!(!fts.is_empty(), "摘要应进 FTS5");

        // 检索器无需特判即可召回摘要。
        let retriever = FtsMemoryRetriever::new(store.clone());
        let hits = retriever.retrieve(&save_id, "微光", 5).await.unwrap();
        assert!(
            hits.iter().any(|h| h.kind == "summary" && h.text.contains("微光")),
            "摘要应可被检索: {hits:?}"
        );
    }

    /// #24 决策 6：非 idle 提交必须**同步**拿到 409 round_in_progress，而不是 202 之后被吞掉。
    #[tokio::test]
    async fn concurrent_round_submission_returns_409() {
        struct BlockingAi {
            started: Arc<tokio::sync::Semaphore>,
            gate: Arc<tokio::sync::Semaphore>,
        }
        #[async_trait::async_trait]
        impl AiProvider for BlockingAi {
            async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
                self.started.add_permits(1);
                let _ = self.gate.acquire().await;
                Ok(AiOutput {
                    intents: vec![Intent::Narrate { content: "……".into(), actor_id: None }.into()],
                    reasoning: None,
                    intent_warnings: vec![],
                    trace: None,
                    compaction: None,
                })
            }
            
        }

        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let (base, _state) = spawn_app_with_ai(Arc::new(BlockingAi {
            started: started.clone(),
            gate: gate.clone(),
        }))
        .await;
        let client = reqwest::Client::new();
        let detail: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("并发测试".to_string()),
                controlled_character_id: None,
                is_sandbox: Some(true),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = detail.item.id.clone();

        // 第一回合：AI 进入后挂起，会话进入非 idle
        let first = client
            .post(format!("{base}/api/saves/{save_id}/rounds"))
            .json(&json!({ "channel": "character", "text": "我看看周围" }))
            .send()
            .await
            .unwrap();
        assert_eq!(first.status().as_u16(), 202);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), started.acquire())
            .await
            .expect("第一回合应进入 AI");

        // 第二回合：非 idle → 409 round_in_progress
        let second = client
            .post(format!("{base}/api/saves/{save_id}/rounds"))
            .json(&json!({ "channel": "character", "text": "再来一次" }))
            .send()
            .await
            .unwrap();
        assert_eq!(second.status().as_u16(), 409, "非 idle 提交应返回 409");
        let body: serde_json::Value = second.json().await.unwrap();
        assert_eq!(body["code"], "round_in_progress");

        // 放行第一回合，避免测试结束时悬挂任务
        gate.add_permits(1);
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
        let db_path =
            std::env::temp_dir().join(format!("octopus-restart-{}.db", uuid::Uuid::new_v4()));
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
                    // 必须等**第二个（角色）回合**的 RoundEnd：只等任意 RoundEnd 会在
                    // 元指令回合后就提前 break，捕获到不完整的历史（flaky）。
                    matches!(&e.event, PlayEvent::RoundEnd(p) if p.round >= 2)
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

    /// 结对压缩检查点随线程落库（派生数据）：写回 → 读回一致；清空 → None。
    #[tokio::test]
    async fn pair_thread_compaction_round_trips() {
        let (base_url, state) = spawn_app().await;
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

        let c = octopus_engine::PairCompaction {
            shadowed: 4,
            fingerprint: "abcd1234".into(),
            summary: "## 目标\n- 做一本轻小说".into(),
            chars_before: 4000,
            chars_after: 40,
        };
        assert!(state
            .store()
            .set_pair_thread_compaction(&t.id, Some(&c))
            .await
            .unwrap());
        assert_eq!(
            state.store().pair_thread_compaction(&t.id).await.unwrap().as_ref(),
            Some(&c),
            "检查点应原样读回"
        );

        state
            .store()
            .set_pair_thread_compaction(&t.id, None)
            .await
            .unwrap();
        assert!(state
            .store()
            .pair_thread_compaction(&t.id)
            .await
            .unwrap()
            .is_none());
        // 不存在的线程：读 None、写 false。
        assert!(state
            .store()
            .pair_thread_compaction("pt-nope")
            .await
            .unwrap()
            .is_none());
        assert!(!state
            .store()
            .set_pair_thread_compaction("pt-nope", Some(&c))
            .await
            .unwrap());
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
            threads
                .iter()
                .find(|x| x.id == t.id)
                .unwrap()
                .pending_suggestions
                .is_none(),
            "清空后应为 None"
        );
    }

    /// 停止按钮：取消在途 AI 调用 → 干净收尾（round_cancelled + round_end），会话回到 idle。
    ///
    /// 这条盯的是最容易出的毛病：取消后前端永远卡在「思考中」。
    #[tokio::test]
    async fn cancel_round_stops_the_inflight_ai_call() {
        struct CancellableAi {
            started: Arc<tokio::sync::Semaphore>,
            gate: Arc<tokio::sync::Semaphore>,
        }
        #[async_trait::async_trait]
        impl AiProvider for CancellableAi {
            async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
                self.started.add_permits(1);
                // 等「停止」：取消后返回 Cancelled，模拟在途请求被中断。
                let _ = self.gate.acquire().await;
                Err(EngineError::Cancelled)
            }
            fn cancel(&self, _save_id: &str) {
                self.gate.add_permits(1);
            }
        }

        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let (base, state) = spawn_app_with_ai(Arc::new(CancellableAi {
            started: started.clone(),
            gate: gate.clone(),
        }))
        .await;
        let client = reqwest::Client::new();
        let detail: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("停止测试".to_string()),
                controlled_character_id: None,
                is_sandbox: Some(true),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = detail.item.id.clone();

        let round = client
            .post(format!("{base}/api/saves/{save_id}/rounds"))
            .json(&json!({ "channel": "character", "text": "我看看周围" }))
            .send()
            .await
            .unwrap();
        assert_eq!(round.status().as_u16(), 202);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), started.acquire())
            .await
            .expect("回合应进入 AI");

        // 空闲会话上按停止：409（没有在途回合）。
        let other = client
            .post(format!("{base}/api/saves/{save_id}/rounds/cancel"))
            .send()
            .await
            .unwrap();
        assert_eq!(other.status().as_u16(), 202, "进行中的回合应受理停止请求");

        // 取消后：日志里必须有 round_cancelled + round_end——前端就是靠它们退出「思考中」。
        let mut saw_cancelled = false;
        let mut saw_round_end = false;
        for _ in 0..200 {
            let rows = state.store().load_events(&save_id).await.unwrap_or_default();
            saw_cancelled = rows.iter().any(|r| {
                matches!(&r.envelope.event, PlayEvent::System(p)
                    if p.code.as_deref() == Some("round_cancelled"))
            });
            saw_round_end = rows
                .iter()
                .any(|r| matches!(&r.envelope.event, PlayEvent::RoundEnd(_)));
            if saw_cancelled && saw_round_end {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            saw_cancelled,
            "日志里应有 code=round_cancelled 的 System 事件"
        );
        assert!(saw_round_end, "取消也要发 RoundEnd，前端才会退出「思考中」");

        // 取消之后可以立刻重发（busy 已释放，不再被 409 拦住）。
        let mut again_status = 0u16;
        for _ in 0..100 {
            let again = client
                .post(format!("{base}/api/saves/{save_id}/rounds"))
                .json(&json!({ "channel": "character", "text": "重新来" }))
                .send()
                .await
                .unwrap();
            again_status = again.status().as_u16();
            if again_status == 202 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(again_status, 202, "取消后必须能重新提交回合");
        let _ = state;
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
            .post(format!(
                "{base_url}/api/storybooks/sb-fallingstar/pair/threads"
            ))
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
            .get(format!(
                "{base_url}/api/storybooks/sb-fallingstar/pair/threads"
            ))
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
            .json(&CreateStorybookRequest {
                title: Some("协议一致性".to_string()),
            })
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
            .json(&SaveDraftRequest {
                draft: bad,
                base_version: 1,
            })
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
            issues
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "protocol_conformance_failed"),
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
            .json(&SaveDraftRequest {
                draft: good,
                base_version: 2,
            })
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
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "通过一致性的 Lua 协议应可发布"
        );
    }

    /// 提示词目录（GET /api/prompts）：默认值非空、覆盖表能反映到目录、回合模板变量齐全。
    #[tokio::test]
    async fn test_api_prompts_catalog() {
        let (base_url, _state) = spawn_app().await;
        let client = reqwest::Client::new();
        let res = client
            .get(format!("{base_url}/api/prompts"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let list: serde_json::Value = res.json().await.unwrap();
        let arr = list.as_array().expect("目录应是数组");
        assert!(arr.len() >= 24, "游玩 + 结对两处的提示词都应登记，实际 {}", arr.len());
        for p in arr {
            let key = p["key"].as_str().unwrap_or_default();
            assert!(!key.is_empty());
            let default = p["default"].as_str().unwrap_or_default();
            assert!(!default.trim().is_empty(), "默认文本不能为空：{key}");
            assert!(p["override_text"].is_null(), "未覆盖时应为 null：{key}");
        }
        let pair = arr.iter().find(|p| p["key"] == "pair.role").expect("结对 AI 角色提示词");
        assert!(pair["default"].as_str().unwrap().contains("结对"));
        let turn = arr
            .iter()
            .find(|p| p["key"] == "story.turn.template")
            .expect("游玩 AI 回合模板");
        let vars = turn["variables"].as_array().unwrap();
        for name in ["round", "personas", "text", "gm"] {
            assert!(
                vars.iter().any(|v| v["name"] == name),
                "回合模板应暴露变量 {name}"
            );
        }
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
        assert!(
            res.headers()
                .get("cache-control")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("immutable")
        );
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
        let check =
            call(json!({ "script": "return { total = 18, margin = 6 }", "mode": "check" })).await;
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

    /// #06 ② 启动缓存门禁：没有 / 版次过期 / 坏 JSON / seq 越界一律回退全量重放。
    #[tokio::test]
    async fn snapshot_base_loads_valid_and_rejects_absent_stale_corrupt() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        // 没有快照 → None（启动走全量重放）。
        assert!(load_snapshot_base(&store, "sv", 1, 0).await.is_none());

        // 造一份可解析的真实 WorldState 作为快照内容。
        let state = WorldState {
            seq: 5,
            scene_id: "sc".into(),
            scene_title: "场景".into(),
            scene_description: None,
            controlled: vec!["c".into()],
            characters: Default::default(),
            flags: Default::default(),
            encounters: Default::default(),
            progress: Default::default(),
            locations: vec![],
            meta: ProjectionMeta {
                save_id: "sv".into(),
                save_title: "t".into(),
                storybook_title: "sb".into(),
                revision: 1,
                needs_upgrade: false,
                auto_confirm: false,
            },
            rng_seed: 7,
            rng_position: 0,
        };
        let state_value = serde_json::to_value(&state).unwrap();
        let good = json!({ "state": state_value, "scene_start_round": 3 }).to_string();
        store.put_snapshot("sv", 5, 1, &good).await.unwrap();

        let base = load_snapshot_base(&store, "sv", 1, 10)
            .await
            .expect("同版次的有效快照应被采用");
        assert_eq!(base.seq, 5);
        assert_eq!(base.scene_start_round, 3);

        // 版次过期（存储层升级后旧快照）→ 回退。
        assert!(
            load_snapshot_base(&store, "sv", 2, 10).await.is_none(),
            "版次不一致的快照必须作废"
        );
        // 坏 JSON（同版次、同 seq 覆盖）→ 回退。
        store.put_snapshot("sv", 5, 1, "{not json").await.unwrap();
        assert!(
            load_snapshot_base(&store, "sv", 1, 10).await.is_none(),
            "无法解析的快照必须作废"
        );
        // seq 超出日志 → 回退。
        store.put_snapshot("sv", 50, 1, &good).await.unwrap();
        assert!(
            load_snapshot_base(&store, "sv", 1, 10).await.is_none(),
            "seq 超出命令日志的快照必须作废"
        );
    }
}
