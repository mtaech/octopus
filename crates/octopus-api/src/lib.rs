//! octopus-api：axum 路由 / SSE 演出流 / 确认门 / 装配组合根（#17/#24/#20）。
//!
//! 组合根：注入 SqliteStore（#27 单库）与 AiProvider（ai crate）。

pub mod admin;
pub mod ai;
pub mod auth;
pub mod config;
pub mod error;
pub mod fetch;
pub mod filename;
pub mod logging;
pub mod pair;
pub mod prompts;
pub mod providers;

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::middleware;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use futures::Stream;
use octopus_engine::{
    AiProvider, AssetStore, ConversationStore, ConvRecord, EngineError, EventSink,
    FtsMemoryRetriever, LuaHost, LuaHostContext, LuaMount, ModelRef, NewPairMessage, PairThreadRow,
    SaveUpgradeWrite, Session, SnapshotBase, SnapshotRow, SqliteStore, StorybookRow,
    SummaryStore, WorldState, compute_upgrade_report, content_type_of,
    lint_script, new_lint_state, pack_book_bundle, pack_bundle, unpack_book_bundle, unpack_bundle,
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

use crate::auth::CurrentUser;
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
    /// 登录失败限流（内存态：单进程本地服务）。
    throttle: auth::LoginThrottle,
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
            throttle: auth::LoginThrottle::default(),
        })
    }

    pub fn throttle(&self) -> &auth::LoginThrottle {
        &self.throttle
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

/// 开档初始在场判定。
///
/// 与切场（octopus-engine `Session::switch_scene`）**共用同一套三层判据**
/// `octopus_engine::session::scene_presence`（docs/map-and-presence-design.md §4.1），
/// 消除「开档全员在场 / 切场缺省名单即清空」的历史不一致：
/// ① 初始场景点名 `present_char_ids` 命中 → 在场（避免把全书角色当同场同伴演）；
/// ② 角色模板 `location_id` == 场景 `location_id` → 在场（常驻地驱动，P2 新增）；
/// ③ 场景既未声明名单、也未声明地点 → 全员在场——否则无骨架 / 无名单的角色卡故事书
///   会一个人都不在场，角色 AI 无人可演，剧情无法推进。
///
/// `location_id` 是人物模板的常驻地（`characters[].location_id`）。
fn is_initially_present(sb: &Value, id: &str, kind: &str, location_id: Option<&str>) -> bool {
    let scene = sb.pointer("/skeleton/0/scenes/0");
    let named: Option<std::collections::HashSet<String>> = scene
        .and_then(|s| s.get("present_char_ids"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let scene_location_id = scene
        .and_then(|s| s.get("location_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    octopus_engine::session::scene_presence(
        named.as_ref(),
        scene_location_id,
        id,
        location_id,
        kind == "pc",
    )
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
            // 图鉴条目（kind="monster"）是**模板库**，不是世界里的实体：怪物只在遭遇里
            // 克隆成实例（图鉴 M2）。开档时若把模板写进初始实例表，「一只地精模板」就变成
            // 「世界里真站着一只地精」——地点栏 / 在场 / 提示词全线被污染。
            if kind == "monster" {
                continue;
            }
            // 常驻地（地图 P2）：人物模板的 location_id 灌进实例，位置链路才有源头。
            let location_id = c
                .get("location_id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let present = is_initially_present(sb, &id, &kind, location_id.as_deref());
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
                    location_id,
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
        // 与新版故事书同一条规矩：图鉴条目（怪物模板）不进初始实例表。
        if kind == "monster" {
            continue;
        }
        let location_id = c
            .get("location_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
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
                location_id,
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
        cooldowns: BTreeMap::new(),
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

/// 包导入请求体上限。包内嵌整本故事书 / 存档引用的全部图片（单图上限 8MB），
/// axum 默认的 2MB 会让「自己导出、自己导入」直接 413——只放宽这两个自包含包的入口。
const PACKAGE_BODY_LIMIT: usize = 64 * 1024 * 1024;

/// 路由装配。安全结构（多账户）：
///
/// - **公开**：健康检查 + 注册 / 登录；
/// - **受保护**：其余全部，合并后统一套 `require_auth`——新增端点只要落进受保护
///   集合就自动要求登录（fail-closed，不会因为漏写守卫而裸奔）；
/// - **资源归属**另加一层 `route_layer`：存档按 `{id}` 判归属，故事书分「可读」
///   （作者或已发布）与「可改」（只有作者），结对线程跟着其故事书。
///
/// 顺序：`require_auth` 最外层，先认人，再判资源权限。
pub fn router(state: Arc<AppState>) -> Router {
    let public = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login));

    // 账户自身：登录即可（me / 登出 / 改口令）。
    let account = Router::new()
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password));

    // 管理员专属：全局配置里有 API Key，普通账户既不能读也不能改。
    // 管理后台是同一条规则下的一棵**独立子树**：挂进来就自动只有管理员可达。
    let admin = Router::new()
        .route(
            "/api/config",
            get(config::get_config).put(config::put_config),
        )
        .route("/api/admin/overview", get(admin::overview))
        .route("/api/admin/users", get(admin::list_users).post(admin::create_user))
        .route(
            "/api/admin/users/{id}",
            patch(admin::update_user).delete(admin::delete_user),
        )
        .route("/api/admin/users/{id}/password", post(admin::reset_password))
        .route(
            "/api/admin/users/{id}/sessions",
            axum::routing::delete(admin::revoke_sessions),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::guard_admin,
        ));

    // 存档集合端点：`/api/saves/import` 是静态段，axum 优先于 `/api/saves/{id}`。
    let saves = Router::new()
        .route("/api/saves", get(list_saves).post(create_save))
        .route(
            "/api/saves/import",
            post(import_save).layer(DefaultBodyLimit::max(PACKAGE_BODY_LIMIT)),
        );
    let saves_item = Router::new()
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
        .route("/api/saves/{id}/save", post(manual_save))
        .route("/api/saves/{id}/upgrade/dry-run", post(upgrade_dry_run))
        .route("/api/saves/{id}/upgrade", post(upgrade_execute))
        .route("/api/saves/{id}/origin", post(new_origin))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::guard_save_owner,
        ));

    // 故事书集合端点 + 整本导入（导入方自己成为作者）。
    let storybooks = Router::new()
        .route(
            "/api/storybooks",
            get(list_storybooks).post(create_storybook_draft),
        )
        .route(
            "/api/storybooks/import",
            post(import_storybook).layer(DefaultBodyLimit::max(PACKAGE_BODY_LIMIT)),
        );
    // 读：作者本人，或已发布（已发布 = 跨账户的公共可开档库：可读 / 可导出 / 可试玩）。
    let storybooks_read = Router::new()
        .route("/api/storybooks/{id}", get(get_storybook))
        .route("/api/storybooks/{id}/export", get(export_storybook))
        .route("/api/storybooks/{id}/sandbox", post(playtest_storybook))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::guard_storybook_read,
        ));
    // 改：草稿 / 发布 / 删除 / 结对（改草稿）——只有作者。
    let storybooks_edit = Router::new()
        .route(
            "/api/storybooks/{id}",
            put(save_storybook_draft).delete(delete_storybook),
        )
        .route("/api/storybooks/{id}/publish", post(publish_storybook))
        .route(
            "/api/storybooks/{id}/pair/threads",
            get(list_pair_threads).post(create_pair_thread),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::guard_storybook_edit,
        ));
    let pair_threads = Router::new()
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
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::guard_pair_thread_edit,
        ));

    // 其余「登录即可」：编辑器工具 / 资产 / 供应商探针 / 结对对话。
    let misc = Router::new()
        .route("/api/assets", post(upload_asset))
        .route("/api/assets/{name}", get(get_asset))
        .route("/api/providers/probe", post(providers::probe_models))
        .route("/api/providers/test", post(providers::test_provider))
        .route("/api/prompts", get(prompts::list_prompts))
        .route("/api/pair/chat", post(pair::pair_chat))
        .route("/api/fetch-url", post(fetch::fetch_url))
        .route("/api/pair/chat/stream", post(pair::pair_chat_stream))
        .route("/api/validate", post(validate_endpoint))
        .route("/api/lua/run", post(run_lua));

    let protected = Router::new()
        .merge(account)
        .merge(admin)
        .merge(saves)
        .merge(saves_item)
        .merge(storybooks)
        .merge(storybooks_read)
        .merge(storybooks_edit)
        .merge(pair_threads)
        .merge(misc)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    public
        .merge(protected)
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

/// 故事书列表：默认（released_only=true）是**跨账户的已发布库**（谁能开档）；
/// 显式 released_only=false 只看自己的书（书架 = 我的创作，含草稿）。
async fn list_storybooks(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<ListStorybooksQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let released_only = q.released_only.unwrap_or(true);
    Ok(Json(
        app.store()
            .list_storybooks_for(user.id(), released_only)
            .await?,
    ))
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
    Extension(user): Extension<CurrentUser>,
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
        .create_storybook_draft(req.title.as_deref(), &initial, user.id())
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

/// 导出故事书为自包含 zip 包：`storybook.json`（草稿 + 已发布快照）+ `assets/`（全部图片）。
async fn export_storybook(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg = app.store().export_storybook_package(&id).await?;
    // 下载文件名用故事书标题（不是 id）：中文标题走 RFC 5987 的 filename*
    let stem = filename::safe_file_stem(&pkg.storybook.title, &id);
    // 打包要读资产文件，放阻塞线程池，别占住 async worker
    let assets = app.assets().clone();
    let bytes = tokio::task::spawn_blocking(move || pack_book_bundle(&pkg, &assets))
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("打包任务失败: {e}"),
            )
        })??;
    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            "application/zip".to_string(),
        ),
        (
            axum::http::header::CONTENT_DISPOSITION,
            filename::content_disposition(&stem, &id, "octopus-book.zip"),
        ),
    ];
    Ok((headers, bytes))
}

/// 导入故事书包：资产先入库（内容寻址去重），再重建故事书记录；返回新文档与校验问题。
async fn import_storybook(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    body: Bytes,
) -> Result<(StatusCode, Json<StorybookMutationResponse>), ApiError> {
    let (pkg, assets) = unpack_book_bundle(&body)?;
    if !assets.is_empty() {
        let n = app.assets().ingest(&assets).await?;
        tracing::info!(count = n, "导入故事书包：资产已入库");
    }
    let row = app.store().import_storybook_package(&pkg, user.id()).await?;
    let issues = octopus_engine::validate_storybook(&row.draft);
    Ok((
        StatusCode::CREATED,
        Json(StorybookMutationResponse {
            doc: row_to_doc(row),
            issues,
        }),
    ))
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

/// 存档列表：只见自己的（多账户隔离）。
async fn list_saves(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<SaveListItem>>, ApiError> {
    Ok(Json(app.store().list_saves_for(user.id()).await?))
}

async fn create_save(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
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
    app.store().insert_save(&detail, false, user.id()).await?;
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
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(req): Json<PlaytestRequest>,
) -> Result<(StatusCode, Json<SaveDetail>), ApiError> {
    create_save(
        State(app),
        Extension(user),
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
        .save_item(&id)
        .await?
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
    // 下载文件名用存档标题（与故事书包同一套规整 + RFC 5987 编码）
    let stem = filename::safe_file_stem(&pkg.save.item.title, &id);
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
    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            "application/zip".to_string(),
        ),
        (
            axum::http::header::CONTENT_DISPOSITION,
            filename::content_disposition(&stem, &id, "octopus.zip"),
        ),
    ];
    Ok((headers, bytes))
}

/// 导入：接受 zip 存档包（当前格式）或旧版单 JSON（历史导出，无资产）
async fn import_save(
    State(app): State<Arc<AppState>>,
    Extension(user): Extension<CurrentUser>,
    body: Bytes,
) -> Result<(StatusCode, Json<SaveListItem>), ApiError> {
    let (pkg, assets) = unpack_bundle(&body)?;
    if !assets.is_empty() {
        let n = app.assets().ingest(&assets).await?;
        tracing::info!(count = n, "导入存档包：资产已入库");
    }
    let item = app.store().import_save_package(&pkg, user.id()).await?;
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
        AdminOverview, AdminUserRow, Intent, MaintenanceRow, NewOriginResult, PlayEvent, SaveDetail,
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

    /// 默认管理员的登录态客户端：受保护路由一律要 `Authorization: Bearer`，
    /// 测试统一经这里换取令牌（也顺带覆盖了登录链路本身）。
    ///
    /// 注意：令牌存在同一个库里，所以「重启进程」的测试拿 base1 换的令牌在 base2 一样有效。
    async fn authed_client(base: &str) -> reqwest::Client {
        let resp = reqwest::Client::new()
            .post(format!("{base}/api/auth/login"))
            .json(&json!({
                "username": octopus_engine::DEFAULT_ADMIN_USERNAME,
                "password": octopus_engine::DEFAULT_ADMIN_PASSWORD,
            }))
            .send()
            .await
            .expect("登录请求应发出");
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        assert!(status.is_success(), "默认管理员登录应成功（{status}）：{body}");
        let login: octopus_types::LoginResponse =
            serde_json::from_str(&body).expect("解析登录响应");
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", login.token).parse().unwrap(),
        );
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap()
    }

    /// 独立库（临时文件）起一个服务：管理后台的「最后一个管理员」这类断言需要确定前提，
    /// 不能被同进程其它测试创建的账户污染。
    async fn isolated_app() -> String {
        let db_path =
            std::env::temp_dir().join(format!("octopus-admin-{}.db", uuid::Uuid::new_v4()));
        spawn_app_with_db(&db_path).await
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
        // meta 是书内权威身份：草稿没写就补一个（发布校验要求 meta.id / meta.title）。
        if draft.is_object() && !draft.get("meta").is_some_and(Value::is_object) {
            draft["meta"] = json!({});
        }
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
        // 先取文本再解析：发布被拒时错误体是 ApiErrorBody，直接 .json() 只会说「缺 doc」。
        let publish_res = client
            .post(format!("{base}/api/storybooks/{}/publish", created.id))
            .json(&PublishRequest {
                base_version: saved.doc.draft_version,
            })
            .send()
            .await
            .unwrap();
        let publish_status = publish_res.status();
        let publish_body = publish_res.text().await.unwrap();
        let published: StorybookMutationResponse = serde_json::from_str(&publish_body)
            .unwrap_or_else(|e| panic!("发布失败（{publish_status}）：{publish_body}\n{e}"));
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
        let client = authed_client(&base).await;

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
        let client = authed_client(&base1).await;

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
        let client = authed_client(&base).await;

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
        let client = authed_client(&base).await;
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
        assert!(is_initially_present(&sb, "char-lucy", "npc", None));
        // 骨架声明了 present_char_ids：以声明为准
        let sb2 = json!({ "skeleton": [{ "scenes": [{ "id": "sc1", "present_char_ids": ["char-a"] }] }] });
        assert!(is_initially_present(&sb2, "char-a", "npc", None));
        assert!(!is_initially_present(&sb2, "char-b", "npc", None));
        assert!(is_initially_present(&sb2, "char-pc", "pc", None), "PC 恒在场");
        // 显式空名单：作者说这一幕没人
        let sb3 = json!({ "skeleton": [{ "scenes": [{ "id": "sc1", "present_char_ids": [] }] }] });
        assert!(!is_initially_present(&sb3, "char-a", "npc", None));
        // ② 位置匹配：清空名单只留 location_id，常驻该地点的人在初始场景在场
        let sb4 = json!({ "skeleton": [{ "scenes": [{ "id": "sc1", "location_id": "loc-tavern" }] }] });
        assert!(is_initially_present(&sb4, "char-isa", "npc", Some("loc-tavern")));
        assert!(!is_initially_present(&sb4, "char-kael", "npc", Some("loc-mine")));
        assert!(!is_initially_present(&sb4, "char-nobody", "npc", None), "场景有地点但人没有常驻地 → 不在场");
        // ① 作者点名优先于 ②：点名的人即使常驻别处也在场
        let sb5 = json!({ "skeleton": [{ "scenes": [{
            "id": "sc1", "location_id": "loc-tavern", "present_char_ids": ["char-kael"]
        }] }] });
        assert!(is_initially_present(&sb5, "char-kael", "npc", Some("loc-mine")), "点名覆盖位置");
        assert!(is_initially_present(&sb5, "char-isa", "npc", Some("loc-tavern")), "位置匹配仍生效");
        // 既有行为不变：名单与地点都未声明 → 全员在场（开档回落）
        let sb6 = json!({ "skeleton": [{ "scenes": [{ "id": "sc1" }] }] });
        assert!(is_initially_present(&sb6, "char-a", "npc", Some("loc-mine")));
    }

    /// 位置链路 P2（设计 §4.2）：人物模板的 location_id 灌进实例；图鉴条目（kind=monster）
    /// 不进初始实例表；初始场景未声明 present_char_ids 时由位置决定在场（与切场同一套判据）。
    #[tokio::test]
    async fn test_save_state_carries_template_location_and_skips_bestiary() {
        let (base, _state) = spawn_app().await;
        let client = authed_client(&base).await;
        let draft = json!({
            "schema_version": 3,
            "meta": { "title": "位置链路" },
            "world": {
                "premise": "边境小镇。",
                "locations": [
                    { "id": "loc-tavern", "name": "碎星酒馆" },
                    { "id": "loc-mine", "name": "废矿坑" }
                ]
            },
            "characters": [
                { "id": "char-mira", "name": "米拉", "kind": "pc" },
                { "id": "char-isa", "name": "伊莎", "kind": "npc", "location_id": "loc-tavern" },
                { "id": "char-oden", "name": "奥登", "kind": "npc", "location_id": "loc-mine" },
                { "id": "mon-goblin", "name": "地精", "kind": "monster", "location_id": "loc-tavern" }
            ],
            "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [
                { "id": "sc-tavern", "title": "碎星酒馆的夜晚", "location_id": "loc-tavern" },
                { "id": "sc-mine", "title": "矿坑口", "location_id": "loc-mine",
                  "present_char_ids": ["char-oden"] }
            ] }]
        });
        let (sb_id, _rev) = create_published_storybook(&client, &base, "位置链路", draft).await;
        let detail: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: sb_id,
                title: Some("位置链路".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let state: WorldProjection = client
            .get(format!("{base}/api/saves/{}/state", detail.item.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        // ① 图鉴条目（怪物模板）不进初始实例表——怪物只在遭遇里克隆成实例
        assert!(
            state.characters.get("inst-mon-goblin").is_none(),
            "怪物模板不该变成世界实例：{:?}",
            state.characters.keys().collect::<Vec<_>>()
        );
        // ② 人物模板的常驻地灌进实例（此前这里写死 None）
        assert_eq!(state.characters["inst-char-isa"]["location_id"], json!("loc-tavern"));
        assert_eq!(state.characters["inst-char-oden"]["location_id"], json!("loc-mine"));
        // ③ 初始场景 sc-tavern 未声明 present_char_ids → 位置匹配定在场
        assert!(state.characters["inst-char-isa"]["present"].as_bool().unwrap(), "位置匹配让伊莎在场");
        assert!(!state.characters["inst-char-oden"]["present"].as_bool().unwrap(), "常驻别处者不在场");
        assert!(state.characters["inst-char-mira"]["present"].as_bool().unwrap(), "PC 恒在场");

        // 回归（设计 §8 验收 1）：旧种子故事书开档，初始在场名单逐字不变
        let seed: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("回归档".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let seed_state: WorldProjection = client
            .get(format!("{base}/api/saves/{}/state", seed.item.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut present: Vec<String> = seed_state
            .characters
            .iter()
            .filter(|(_, c)| c["present"].as_bool() == Some(true))
            .map(|(k, _)| k.clone())
            .collect();
        present.sort();
        assert_eq!(
            present,
            vec!["inst-char-isa", "inst-char-kael", "inst-char-mira", "inst-char-oden"],
            "旧故事书开档在场名单必须逐字不变"
        );
    }

    /// 验收锚点：杀掉后端进程 → 重启 → 打开存档，叙事历史与世界状态原样还在，且不重跑 AI。
    #[tokio::test]
    async fn test_session_survives_restart_via_command_log() {
        let db_path =
            std::env::temp_dir().join(format!("octopus-restart-{}.db", uuid::Uuid::new_v4()));
        let base1 = spawn_app_with_db(&db_path).await;
        let client = authed_client(&base1).await;

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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;
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
        let client = authed_client(&base_url).await;
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
        let client = authed_client(&base).await;
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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;
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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;
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
        let client = authed_client(&base_url).await;
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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;

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
        // 标题是中文 → filename= 放 ASCII 兜底（id），真实标题走 filename*
        assert!(content_disp.contains(&format!("filename=\"{save_id}.octopus.zip\"")));
        assert!(content_disp.contains("filename*=UTF-8''"));
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

    /// 故事书包：整本导出（草稿 + 已发布快照 + 封面资产）→ 导入（新 id +「(导入)」后缀，发布态保留）。
    #[tokio::test]
    async fn test_api_export_and_import_storybook() {
        let (base, _state) = spawn_app().await;
        let client = authed_client(&base).await;

        // 封面资产先进库：导出时必须随包交付
        let cover = fake_png(b"book-cover");
        let up: Value = client
            .post(format!("{base}/api/assets"))
            .header("content-type", "image/png")
            .body(cover.clone())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let cover_name = up["asset"].as_str().unwrap().to_string();

        let draft = json!({
            "schema_version": 3,
            "meta": {
                "title": "待导出故事书",
                "cover": { "asset": cover_name, "w": 8, "h": 8 }
            },
            "world": { "premise": "故事开场。" },
            "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }],
            "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{
                "id": "sc1", "title": "开场", "present_char_ids": ["char-a"]
            }] }]
        });
        let (sb_id, _after) = create_published_storybook(&client, &base, "待导出故事书", draft).await;

        // 1. 导出：zip 包（storybook.json + assets/）
        let export_res = client
            .get(format!("{base}/api/storybooks/{sb_id}/export"))
            .send()
            .await
            .unwrap();
        assert_eq!(export_res.status(), StatusCode::OK);
        assert_eq!(
            export_res.headers().get("content-type").unwrap(),
            "application/zip"
        );
        let content_disp = export_res
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap();
        // 文件名用标题而不是 id：中文标题进 filename*，ASCII 兜底才是 id
        assert!(content_disp.contains(&format!("filename=\"{sb_id}.octopus-book.zip\"")));
        assert!(content_disp.contains("filename*=UTF-8''"));
        assert!(content_disp.contains(".octopus-book.zip"));
        let zip_bytes = export_res.bytes().await.unwrap();

        let (pkg, assets) = unpack_book_bundle(&zip_bytes).unwrap();
        assert_eq!(pkg.format, "octopus-storybook-package");
        assert_eq!(pkg.storybook.title, "待导出故事书");
        assert!(pkg.storybook.published);
        assert_eq!(assets.len(), 1, "封面资产要随包交付");
        assert_eq!(assets[0].0, cover_name);
        assert_eq!(assets[0].1, cover);

        // 2. 导入：同库 id 冲突 → 新 id + 后缀；发布态与书内身份都跟着改
        let import_res = client
            .post(format!("{base}/api/storybooks/import"))
            .header("content-type", "application/zip")
            .body(zip_bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(import_res.status(), StatusCode::CREATED);
        let body: StorybookMutationResponse = import_res.json().await.unwrap();
        assert_ne!(body.doc.id, sb_id);
        let imported_title = body.doc.draft["meta"]["title"].as_str().unwrap();
        assert!(imported_title.ends_with("(导入)"));
        assert!(body.doc.published);
        assert!(body.doc.released.is_some());
        assert_eq!(body.doc.draft["meta"]["id"], json!(body.doc.id));
        assert!(
            !body
                .issues
                .iter()
                .any(|i| matches!(i.severity, IssueSeverity::Error)),
            "导入的书不该带阻断性校验问题"
        );

        // 3. 两本都在「已发布」列表里（导入那本可直接开档）
        let list: Vec<StorybookListItem> = client
            .get(format!("{base}/api/storybooks?released_only=true"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(list.iter().any(|s| s.id == sb_id));
        assert!(list.iter().any(|s| s.id == body.doc.id));
    }

    /// 拒收外来载荷：垃圾字节与**存档包**都不该被当成故事书（400，不是 500）。
    #[tokio::test]
    async fn test_api_import_storybook_rejects_foreign_payload() {
        let (base, _state) = spawn_app().await;
        let client = authed_client(&base).await;

        let garbage = client
            .post(format!("{base}/api/storybooks/import"))
            .header("content-type", "application/zip")
            .body(b"definitely not a zip".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(garbage.status(), StatusCode::BAD_REQUEST);

        // 存档包（清单名 save.json）与故事书包（storybook.json）互不冒充
        let detail: SaveDetail = client
            .post(format!("{base}/api/saves"))
            .json(&CreateSaveRequest {
                storybook_id: "sb-fallingstar".to_string(),
                title: Some("借来导出存档包".to_string()),
                controlled_character_id: None,
                is_sandbox: None,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_zip = client
            .get(format!("{base}/api/saves/{}/export", detail.item.id))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let crossed = client
            .post(format!("{base}/api/storybooks/import"))
            .header("content-type", "application/zip")
            .body(save_zip)
            .send()
            .await
            .unwrap();
        assert_eq!(crossed.status(), StatusCode::BAD_REQUEST);
    }

    /// 包会比 axum 默认的 2MB 大：3MB 图的书包要能自己导出、自己导入（不放宽就 413）。
    #[tokio::test]
    async fn test_api_import_storybook_accepts_large_package() {
        let (base, state) = spawn_app().await;
        let client = authed_client(&base).await;

        // 直接写资产库（跳过上传端点的限制），只为造出「包内有大图」这一事实
        let big = state
            .assets()
            .put(&fake_png(&vec![7u8; 3 * 1024 * 1024]))
            .await
            .unwrap();
        let draft = json!({
            "schema_version": 3,
            "meta": { "title": "大图故事书", "cover": { "asset": big.name, "w": 8, "h": 8 } },
            "world": { "premise": "开场。" },
            "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }],
            "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{
                "id": "sc1", "title": "开场", "present_char_ids": ["char-a"]
            }] }]
        });
        let (sb_id, _after) =
            create_published_storybook(&client, &base, "大图故事书", draft).await;

        let zip_bytes = client
            .get(format!("{base}/api/storybooks/{sb_id}/export"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert!(
            zip_bytes.len() > 2 * 1024 * 1024,
            "包必须真的超过默认上限，否则这条测试测不到点子上"
        );

        let res = client
            .post(format!("{base}/api/storybooks/import"))
            .header("content-type", "application/zip")
            .body(zip_bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body: StorybookMutationResponse = res.json().await.unwrap();
        assert!(body.doc.published);
        assert_eq!(body.doc.draft["meta"]["cover"]["asset"], json!(big.name));
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
        let client = authed_client(&base_url).await;

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
        let client = authed_client(&base_url).await;

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

    // ============================================================
    // 多账户：鉴权、归属、权限
    //
    // 这些测试刻意**不**断言全局列表为空、也不动默认管理员的口令：`open_in_memory`
    // 在 sqlx 下是否跨测试共享库属于实现细节，断言必须只依赖本用例自己造的 id。
    // ============================================================

    /// 带令牌的客户端（不校验令牌有效性：无效令牌就该拿到 401）。
    fn bearer_client(token: &str) -> reqwest::Client {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap()
    }

    /// 登录并取回令牌（失败返回 None，便于断言「旧口令不再可用」）。
    async fn login_token(base: &str, username: &str, password: &str) -> Option<String> {
        let res = reqwest::Client::new()
            .post(format!("{base}/api/auth/login"))
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await
            .unwrap();
        if !res.status().is_success() {
            return None;
        }
        let body: octopus_types::LoginResponse = res.json().await.unwrap();
        Some(body.token)
    }

    /// 注册一个新账户并返回其登录态客户端。用户名带随机后缀：测试之间互不干扰。
    async fn register_client(base: &str, prefix: &str) -> reqwest::Client {
        let username = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let res = reqwest::Client::new()
            .post(format!("{base}/api/auth/register"))
            .json(&json!({
                "username": username,
                "password": "pw-123456",
                "display_name": "测试账户",
            }))
            .send()
            .await
            .unwrap();
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        assert!(status.is_success(), "注册应成功（{status}）：{body}");
        let login: octopus_types::LoginResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(login.account.username, username.to_lowercase());
        bearer_client(&login.token)
    }

    /// 任意方法 + 一个 JSON 体的状态码。
    async fn status_of(
        client: &reqwest::Client,
        method: &str,
        url: &str,
        body: Value,
    ) -> StatusCode {
        let rb = match method {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "PATCH" => client.patch(url),
            "DELETE" => client.delete(url),
            other => panic!("未支持的方法 {other}"),
        };
        rb.json(&body).send().await.unwrap().status()
    }

    /// 未登录 / 伪造令牌：受保护路由一律 401（fail-closed）；公开端点不受影响。
    #[tokio::test]
    async fn protected_routes_fail_closed_without_token() {
        let (base, _state) = spawn_app().await;
        let anon = reqwest::Client::new();
        for (method, path) in [
            ("GET", "/api/saves"),
            ("GET", "/api/storybooks?released_only=false"),
            ("POST", "/api/storybooks"),
            ("GET", "/api/config"),
            ("GET", "/api/prompts"),
            ("GET", "/api/assets/whatever.png"),
            ("POST", "/api/validate"),
            ("POST", "/api/auth/me"),
        ] {
            assert_eq!(
                status_of(&anon, method, &format!("{base}{path}"), json!({})).await,
                StatusCode::UNAUTHORIZED,
                "{method} {path} 必须要求登录"
            );
        }

        // 公开端点照旧：健康检查与登录入口。
        assert!(anon
            .get(format!("{base}/api/health"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        assert_eq!(
            status_of(&anon, "POST", &format!("{base}/api/auth/login"), json!({"username":"nobody","password":"x"})).await,
            StatusCode::UNAUTHORIZED,
            "登录端点公开，但凭据不对仍是 401"
        );

        // 伪造 / 过期令牌同样 401。
        let forged = bearer_client("deadbeef");
        assert_eq!(
            forged.get(format!("{base}/api/saves")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    /// 注册 → me → 登录 → 登出：账户主链路 + 登出只吊销当前会话。
    #[tokio::test]
    async fn account_lifecycle_register_login_me_logout() {
        let (base, _state) = spawn_app().await;
        let anon = reqwest::Client::new();

        // 注册即登录（用户名大小写不敏感：存的是规范化小写）。
        let res = anon
            .post(format!("{base}/api/auth/register"))
            .json(&json!({ "username": "Mira", "password": "pw-123456", "display_name": "米拉" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let reg: octopus_types::LoginResponse = res.json().await.unwrap();
        assert_eq!(reg.account.username, "mira");
        assert_eq!(reg.account.display_name, "米拉");
        assert!(!reg.account.is_admin, "注册出来的都是普通账户");
        assert!(!reg.account.must_change_password);
        assert!(reg.expires_at > octopus_engine::storage::now_iso());

        // 重名（大小写不敏感）→ 409；用户名不合规 → 400；口令太短 → 400。
        assert_eq!(
            status_of(&anon, "POST", &format!("{base}/api/auth/register"), json!({"username":"MIRA","password":"pw-123456"})).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(&anon, "POST", &format!("{base}/api/auth/register"), json!({"username":"a","password":"pw-123456"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(&anon, "POST", &format!("{base}/api/auth/register"), json!({"username":"someone","password":"x"})).await,
            StatusCode::BAD_REQUEST
        );

        // me
        let c = bearer_client(&reg.token);
        let me: octopus_types::Account = c
            .get(format!("{base}/api/auth/me"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(me.username, "mira");
        assert_eq!(me.id, reg.account.id);

        // 口令错 → 401（不透露账号是否存在）；口令对 → 新会话。
        assert!(login_token(&base, "mira", "nope").await.is_none());
        assert!(login_token(&base, "no-such-user", "pw-123456").await.is_none());
        let second = login_token(&base, "MIRA", "pw-123456")
            .await
            .expect("大小写不敏感地登录成功");
        let c2 = bearer_client(&second);

        // 登出只吊销当前会话：另一条（注册时那条）仍然有效。
        assert_eq!(
            c2.post(format!("{base}/api/auth/logout")).send().await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            c2.get(format!("{base}/api/auth/me")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            c.get(format!("{base}/api/auth/me")).send().await.unwrap().status(),
            StatusCode::OK
        );
    }

    /// 改口令：旧口令必须对、新口令生效、其它会话被吊销、当前会话保留。
    #[tokio::test]
    async fn change_password_verifies_old_and_revokes_other_sessions() {
        let (base, _state) = spawn_app().await;
        let anon = reqwest::Client::new();
        let username = format!("pw{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let first = anon
            .post(format!("{base}/api/auth/register"))
            .json(&json!({ "username": username, "password": "pw-123456" }))
            .send()
            .await
            .unwrap()
            .json::<octopus_types::LoginResponse>()
            .await
            .unwrap();
        let current = bearer_client(&first.token);
        let other = bearer_client(&login_token(&base, &username, "pw-123456").await.unwrap());

        // 旧口令不对 → 401
        assert_eq!(
            status_of(&current, "POST", &format!("{base}/api/auth/password"), json!({"current_password":"wrong","new_password":"new-pw-99"})).await,
            StatusCode::UNAUTHORIZED
        );
        // 新旧相同 → 400
        assert_eq!(
            status_of(&current, "POST", &format!("{base}/api/auth/password"), json!({"current_password":"pw-123456","new_password":"pw-123456"})).await,
            StatusCode::BAD_REQUEST
        );

        // 正常改密
        assert_eq!(
            status_of(&current, "POST", &format!("{base}/api/auth/password"), json!({"current_password":"pw-123456","new_password":"new-pw-99"})).await,
            StatusCode::NO_CONTENT
        );
        // 旧口令失效，新口令可用
        assert!(login_token(&base, &username, "pw-123456").await.is_none());
        assert!(login_token(&base, &username, "new-pw-99").await.is_some());
        // 其它会话被吊销，当前会话保留
        assert_eq!(
            other.get(format!("{base}/api/auth/me")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            current.get(format!("{base}/api/auth/me")).send().await.unwrap().status(),
            StatusCode::OK
        );
    }

    /// 默认管理员：初始口令被标记为待改（前端据此常驻提醒）。
    #[tokio::test]
    async fn default_admin_is_flagged_to_change_password() {
        let (base, _state) = spawn_app().await;
        let admin = authed_client(&base).await;
        let me: octopus_types::Account = admin
            .get(format!("{base}/api/auth/me"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(me.username, octopus_engine::DEFAULT_ADMIN_USERNAME);
        assert!(me.is_admin);
        assert!(
            me.must_change_password,
            "默认口令是公开知识，必须一直提醒到改掉为止"
        );
    }

    /// 存档归属：别人的存档不出现、也访问不到（一律 404，不给 id 探测留缝隙）。
    #[tokio::test]
    async fn saves_are_isolated_between_accounts() {
        let (base, _state) = spawn_app().await;
        let admin = authed_client(&base).await;
        let (sb_id, _v) = create_published_storybook(
            &admin,
            &base,
            "归属测试书",
            json!({
                "schema_version": 3,
                "world": { "premise": "开场。" },
                "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }],
                "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{
                    "id": "sc1", "title": "开场", "present_char_ids": ["char-a"]
                }] }]
            }),
        )
        .await;
        let save: SaveDetail = admin
            .post(format!("{base}/api/saves"))
            .json(&json!({ "storybook_id": sb_id, "title": "管理员的档" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save_id = save.item.id.clone();

        let other = register_client(&base, "iso").await;

        // 列表隔离
        let listed: Vec<SaveListItem> = other
            .get(format!("{base}/api/saves"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            !listed.iter().any(|s| s.id == save_id),
            "别人的存档不该出现在列表里"
        );

        // 逐条端点：全部 404（守卫在处理器之前，所以不会碰到引擎）
        for (method, path) in [
            ("GET", format!("/api/saves/{save_id}")),
            ("PATCH", format!("/api/saves/{save_id}")),
            ("DELETE", format!("/api/saves/{save_id}")),
            ("GET", format!("/api/saves/{save_id}/state")),
            ("GET", format!("/api/saves/{save_id}/history")),
            ("GET", format!("/api/saves/{save_id}/stream")),
            ("GET", format!("/api/saves/{save_id}/export")),
            ("GET", format!("/api/saves/{save_id}/maintenance")),
            ("GET", format!("/api/saves/{save_id}/settings")),
            ("PUT", format!("/api/saves/{save_id}/settings")),
            ("POST", format!("/api/saves/{save_id}/rounds")),
            ("POST", format!("/api/saves/{save_id}/rounds/cancel")),
            ("POST", format!("/api/saves/{save_id}/rounds/1/confirmation")),
            ("POST", format!("/api/saves/{save_id}/rerun")),
            ("POST", format!("/api/saves/{save_id}/character")),
            ("POST", format!("/api/saves/{save_id}/rest")),
            ("POST", format!("/api/saves/{save_id}/save")),
            ("POST", format!("/api/saves/{save_id}/upgrade/dry-run")),
            ("POST", format!("/api/saves/{save_id}/upgrade")),
            ("POST", format!("/api/saves/{save_id}/origin")),
        ] {
            assert_eq!(
                status_of(&other, method, &format!("{base}{path}"), json!({ "title": "偷改" })).await,
                StatusCode::NOT_FOUND,
                "{method} {path} 越权必须 404"
            );
        }

        // 作者自己不受影响
        let mine: Vec<SaveListItem> = admin
            .get(format!("{base}/api/saves"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(mine.iter().any(|s| s.id == save_id));
        assert_eq!(
            other
                .get(format!("{base}/api/saves/{save_id}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    /// 故事书：未发布是私有的；已发布跨账户「可读可开档」，但只有作者能改。
    #[tokio::test]
    async fn storybooks_are_private_until_published_and_only_author_can_edit() {
        let (base, _state) = spawn_app().await;
        let admin = authed_client(&base).await;
        let draft: StorybookDocument = admin
            .post(format!("{base}/api/storybooks"))
            .json(&json!({ "title": "私密草稿" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let other = register_client(&base, "sb").await;

        // 未发布：不存在（连 id 都探不出来）
        assert_eq!(
            other
                .get(format!("{base}/api/storybooks/{}", draft.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        // 别人的草稿不进我的书架，也不进可开档库
        for released_only in ["false", "true"] {
            let list: Vec<Value> = other
                .get(format!("{base}/api/storybooks?released_only={released_only}"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(
                !list.iter().any(|b| b["id"] == draft.id),
                "别人的未发布草稿不该出现在 released_only={released_only} 列表里"
            );
        }

        // 发布后：可读 / 可导出 / 可开档（跨账户玩别人的书是这个功能的正当路径）
        let (sb_id, _v) = create_published_storybook(
            &admin,
            &base,
            "共享故事书",
            json!({
                "schema_version": 3,
                "world": { "premise": "开场。" },
                "characters": [{ "id": "char-a", "name": "米拉", "kind": "pc" }],
                "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{
                    "id": "sc1", "title": "开场", "present_char_ids": ["char-a"]
                }] }]
            }),
        )
        .await;
        assert_eq!(
            other
                .get(format!("{base}/api/storybooks/{sb_id}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let list: Vec<Value> = other
            .get(format!("{base}/api/storybooks?released_only=true"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(list.iter().any(|b| b["id"] == sb_id), "已发布的书在可开档库里");

        let save: SaveDetail = other
            .post(format!("{base}/api/saves"))
            .json(&json!({ "storybook_id": sb_id, "title": "玩别人的书" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(save.item.storybook_id, sb_id);
        // 我开的档是我自己的：作者看不到
        let author_saves: Vec<SaveListItem> = admin
            .get(format!("{base}/api/saves"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!author_saves.iter().any(|s| s.id == save.item.id));

        // 但改 / 删 / 发布 / 结对（改草稿）只有作者能：全部 404
        for (method, path) in [
            ("PUT", format!("/api/storybooks/{sb_id}")),
            ("DELETE", format!("/api/storybooks/{sb_id}")),
            ("POST", format!("/api/storybooks/{sb_id}/publish")),
            ("GET", format!("/api/storybooks/{sb_id}/pair/threads")),
            ("POST", format!("/api/storybooks/{sb_id}/pair/threads")),
        ] {
            assert_eq!(
                status_of(&other, method, &format!("{base}{path}"), json!({ "base_version": 1 })).await,
                StatusCode::NOT_FOUND,
                "{method} {path} 非作者必须 404"
            );
        }
        // 作者本人可以
        assert_eq!(
            status_of(&admin, "POST", &format!("{base}/api/storybooks/{sb_id}/pair/threads"), json!({"title":"结对"})).await,
            StatusCode::CREATED
        );
    }

    /// 全局配置（含 API Key）只有管理员能读写。
    #[tokio::test]
    async fn global_config_is_admin_only() {
        let (base, _state) = spawn_app().await;
        let admin = authed_client(&base).await;
        let other = register_client(&base, "cfg").await;

        assert_eq!(
            other.get(format!("{base}/api/config")).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(&other, "PUT", &format!("{base}/api/config"), json!({})).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            admin.get(format!("{base}/api/config")).send().await.unwrap().status(),
            StatusCode::OK
        );
    }

    /// SSE 拿不到自定义请求头：GET 必须接受 `?token=`（演出流专用逃生口）。
    #[tokio::test]
    async fn get_accepts_token_query_for_sse() {
        let (base, _state) = spawn_app().await;
        let admin = authed_client(&base).await;
        let (sb_id, _v) = create_published_storybook(
            &admin,
            &base,
            "SSE 令牌书",
            json!({
                "schema_version": 3,
                "world": { "premise": "开场。" },
                "skeleton": [{ "id": "ch1", "title": "第一章", "scenes": [{ "id": "sc1", "title": "开场" }] }]
            }),
        )
        .await;
        let save: SaveDetail = admin
            .post(format!("{base}/api/saves"))
            .json(&json!({ "storybook_id": sb_id, "title": "SSE 档" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let token = login_token(&base, octopus_engine::DEFAULT_ADMIN_USERNAME, octopus_engine::DEFAULT_ADMIN_PASSWORD)
            .await
            .unwrap();
        let anon = reqwest::Client::new();
        assert_eq!(
            anon.get(format!("{base}/api/saves/{}/state?token={token}", save.item.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        // 没令牌 / 令牌错 → 401
        assert_eq!(
            anon.get(format!("{base}/api/saves/{}/state", save.item.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            anon.get(format!("{base}/api/saves/{}/state?token=deadbeef", save.item.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    // ============================================================
    // 管理后台（/api/admin/*）
    // ============================================================

    /// 门禁：未登录 401；普通账户 403；管理员才通。
    #[tokio::test]
    async fn admin_routes_are_admin_only() {
        let (base, _state) = spawn_app().await;
        let admin = authed_client(&base).await;
        let other = register_client(&base, "na").await;

        let admin_routes: [(&str, &str); 7] = [
            ("GET", "/api/admin/overview"),
            ("GET", "/api/admin/users"),
            ("POST", "/api/admin/users"),
            ("PATCH", "/api/admin/users/u-1"),
            ("DELETE", "/api/admin/users/u-1"),
            ("POST", "/api/admin/users/u-1/password"),
            ("DELETE", "/api/admin/users/u-1/sessions"),
        ];
        for (method, path) in admin_routes {
            assert_eq!(
                status_of(&other, method, &format!("{base}{path}"), json!({})).await,
                StatusCode::FORBIDDEN,
                "普通账户访问 {method} {path} 必须 403"
            );
            let anon = reqwest::Client::new();
            let anon_status = match method {
                "GET" => anon.get(format!("{base}{path}")),
                "POST" => anon.post(format!("{base}{path}")),
                "PATCH" => anon.patch(format!("{base}{path}")),
                _ => anon.delete(format!("{base}{path}")),
            }
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status();
            assert_eq!(
                anon_status,
                StatusCode::UNAUTHORIZED,
                "未登录访问 {method} {path} 必须 401（守卫顺序：先认人，再判管理员）"
            );
        }

        // 管理员可用
        let over: AdminOverview = admin
            .get(format!("{base}/api/admin/overview"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(over.users >= 1 && over.admins >= 1);
        assert_eq!(over.version, env!("CARGO_PKG_VERSION"));
        assert!(!over.db_path.is_empty());
    }

    /// 用户管理主链路：新建 → 改名 / 提权 → 重置口令（踢下线）→ 退出设备 → 删除。
    #[tokio::test]
    async fn admin_user_management_flow() {
        let base = isolated_app().await;
        let admin = authed_client(&base).await;
        let username = format!("staff{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);

        // 列表：能看到自己，且被标记 is_self
        let rows: Vec<AdminUserRow> = admin
            .get(format!("{base}/api/admin/users"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let me = rows
            .iter()
            .find(|r| r.username == octopus_engine::DEFAULT_ADMIN_USERNAME)
            .expect("管理员自己在列表里");
        assert!(me.is_self && me.is_admin && me.must_change_password);

        // 新建普通账户
        let created: AdminUserRow = admin
            .post(format!("{base}/api/admin/users"))
            .json(&json!({ "username": username, "password": "pw-123456", "display_name": "员工甲" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(created.username, username);
        assert_eq!(created.display_name, "员工甲");
        assert!(!created.is_admin && !created.is_self);
        assert_eq!(
            (created.saves, created.storybooks, created.active_sessions),
            (0, 0, 0)
        );

        // 重名 409 / 弱口令 400 / 非法用户名 400
        assert_eq!(
            status_of(&admin, "POST", &format!("{base}/api/admin/users"), json!({"username": username, "password": "pw-123456"})).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(&admin, "POST", &format!("{base}/api/admin/users"), json!({"username": "someone-else", "password": "x"})).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(&admin, "POST", &format!("{base}/api/admin/users"), json!({"username": "a", "password": "pw-123456"})).await,
            StatusCode::BAD_REQUEST
        );

        // 改名 + 提权
        let updated: AdminUserRow = admin
            .patch(format!("{base}/api/admin/users/{}", created.id))
            .json(&json!({ "display_name": "员工乙", "is_admin": true }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(updated.display_name, "员工乙");
        assert!(updated.is_admin);

        // 省略 display_name = 不改；显式空串 = 回落为用户名
        let untouched: AdminUserRow = admin
            .patch(format!("{base}/api/admin/users/{}", created.id))
            .json(&json!({ "is_admin": true }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(untouched.display_name, "员工乙", "省略字段不应清空显示名");
        let reset: AdminUserRow = admin
            .patch(format!("{base}/api/admin/users/{}", created.id))
            .json(&json!({ "display_name": "" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(reset.display_name, username, "空串回落为用户名");
        // 复位显示名，后面的断言沿用「员工乙」的语义
        admin
            .patch(format!("{base}/api/admin/users/{}", created.id))
            .json(&json!({ "display_name": "员工乙" }))
            .send()
            .await
            .unwrap();

        // 提权后该账户能自己进后台
        let staff = bearer_client(&login_token(&base, &username, "pw-123456").await.unwrap());
        assert_eq!(
            staff
                .get(format!("{base}/api/admin/users"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        // 重置口令：旧口令失效 + 该账户全部登录态被吊销
        assert_eq!(
            status_of(&admin, "POST", &format!("{base}/api/admin/users/{}/password", created.id), json!({"new_password":"pw-654321"})).await,
            StatusCode::NO_CONTENT
        );
        assert!(login_token(&base, &username, "pw-123456").await.is_none());
        assert_eq!(
            staff.get(format!("{base}/api/auth/me")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "重置口令必须把旧会话一并踢掉"
        );
        let staff2 = bearer_client(&login_token(&base, &username, "pw-654321").await.unwrap());

        // 退出其全部设备
        let revoked: Value = admin
            .delete(format!("{base}/api/admin/users/{}/sessions", created.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(revoked["revoked"].as_u64().unwrap() >= 1);
        assert_eq!(
            staff2.get(format!("{base}/api/auth/me")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        // 无内容账户直接删（不需要 purge）；删完登录不了，再删 404
        assert_eq!(
            admin
                .delete(format!("{base}/api/admin/users/{}", created.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(login_token(&base, &username, "pw-654321").await.is_none());
        assert_eq!(
            admin
                .delete(format!("{base}/api/admin/users/{}", created.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    /// 删账户默认拒绝（409 + 内容统计）；purge=true 才连带删除其存档与故事书。
    #[tokio::test]
    async fn admin_delete_user_with_data_requires_explicit_purge() {
        let base = isolated_app().await;
        let admin = authed_client(&base).await;
        let username = format!("own{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let created: AdminUserRow = admin
            .post(format!("{base}/api/admin/users"))
            .json(&json!({ "username": username, "password": "pw-123456" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let staff = bearer_client(&login_token(&base, &username, "pw-123456").await.unwrap());

        // 他有自己的书，也从公共书架开了一档
        let book: StorybookDocument = staff
            .post(format!("{base}/api/storybooks"))
            .json(&json!({ "title": "员工的书" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let save: SaveDetail = staff
            .post(format!("{base}/api/saves"))
            .json(&json!({ "storybook_id": "sb-fallingstar", "title": "员工的档" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        // 统计要出现在列表里
        let rows: Vec<AdminUserRow> = admin
            .get(format!("{base}/api/admin/users"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let row = rows.iter().find(|r| r.id == created.id).unwrap();
        assert_eq!((row.saves, row.storybooks), (1, 1));

        // 不 purge → 409 + 明细，账户还在
        let res = admin
            .delete(format!("{base}/api/admin/users/{}", created.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["code"], "user_has_data");
        assert_eq!(body["detail"]["saves"], 1);
        assert_eq!(body["detail"]["storybooks"], 1);
        assert!(login_token(&base, &username, "pw-123456").await.is_some());

        // purge=true → 连内容一起删
        let res = admin
            .delete(format!("{base}/api/admin/users/{}?purge=true", created.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["saves_deleted"], 1);
        assert_eq!(body["storybooks_deleted"], 1);
        assert_eq!(
            admin
                .get(format!("{base}/api/storybooks/{}", book.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            admin
                .get(format!("{base}/api/saves/{}", save.item.id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert!(login_token(&base, &username, "pw-123456").await.is_none());
    }

    /// 防锁死：不能删自己、不能降级最后一个管理员；有第二个管理员后即可降级。
    #[tokio::test]
    async fn admin_lockout_guards() {
        let base = isolated_app().await;
        let admin = authed_client(&base).await;
        let rows: Vec<AdminUserRow> = admin
            .get(format!("{base}/api/admin/users"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let me = rows.iter().find(|r| r.is_self).expect("自己").clone();
        assert_eq!(rows.iter().filter(|r| r.is_admin).count(), 1, "独立库只有一个管理员");

        let res = admin
            .delete(format!("{base}/api/admin/users/{}", me.id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        assert_eq!(res.json::<Value>().await.unwrap()["code"], "cannot_delete_self");

        let res = admin
            .patch(format!("{base}/api/admin/users/{}", me.id))
            .json(&json!({ "is_admin": false }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        assert_eq!(res.json::<Value>().await.unwrap()["code"], "last_admin");

        // 有第二个管理员后，降级不再被拦（规则不是一刀切）
        let second: AdminUserRow = admin
            .post(format!("{base}/api/admin/users"))
            .json(&json!({ "username": "admin2", "password": "pw-123456", "is_admin": true }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let demoted: AdminUserRow = admin
            .patch(format!("{base}/api/admin/users/{}", second.id))
            .json(&json!({ "is_admin": false }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!demoted.is_admin);
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
            cooldowns: Default::default(),
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
