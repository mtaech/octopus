//! 会话（#03 核心循环 / #17 事件发射 / #24 回合并发与确认门）。
//!
//! 一个存档一个 Session：内存权威状态 + 确定性 RNG + 演出流出口 + AI 端口。
//! 回合串行（#24 ④）：非 idle 提交返回 `RoundInProgress`。

use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
};

use octopus_types::{
    ActorRef, CheckResultPayload, ConfirmDecision, DialoguePayload, EmotePayload, EventEnvelope,
    HistoryPage, Intent, NarratePayload, PendingPayload, PhasePayload, PhaseStage, PlayEvent,
    RejectionCode, ResolutionPayload, ResolutionStatus, RoundChannel, RoundEndPayload, RoundInput,
    RoundStartPayload, Seq, StateDelta, StateUpdatePayload, SuccessLevel, SystemLevel, SystemPayload,
    WorldProjection,
};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

use crate::{
    error::EngineError,
    ports::{AiProvider, EventSink, TurnContext},
    rng::DeterministicRng,
    state::WorldState,
    storage::now_iso,
};

struct Pending {
    action_id: String,
    tx: oneshot::Sender<ConfirmDecision>,
}

/// 回合结束后自动清 busy（无 finally 语义，用 Drop 保证）。
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct Session {
    pub save_id: String,
    state: Mutex<WorldState>,
    rng: Mutex<DeterministicRng>,
    sink: Arc<dyn EventSink>,
    ai: Arc<dyn AiProvider>,
    seq: AtomicU64,
    round: AtomicU32,
    busy: AtomicBool,
    auto_confirm: AtomicBool,
    pending: Mutex<Option<Pending>>,
    event_log: Mutex<Vec<EventEnvelope>>,
    request_ids: Mutex<Vec<String>>,
    confirmation_timeout_ms: u64,
}

impl Session {
    pub fn new(
        save_id: String,
        state: WorldState,
        sink: Arc<dyn EventSink>,
        ai: Arc<dyn AiProvider>,
        auto_confirm: bool,
    ) -> Self {
        let seed = state.rng_seed;
        Self {
            save_id,
            state: Mutex::new(state),
            rng: Mutex::new(DeterministicRng::new(seed)),
            sink,
            ai,
            seq: AtomicU64::new(0),
            round: AtomicU32::new(0),
            busy: AtomicBool::new(false),
            auto_confirm: AtomicBool::new(auto_confirm),
            pending: Mutex::new(None),
            event_log: Mutex::new(Vec::new()),
            request_ids: Mutex::new(Vec::new()),
            confirmation_timeout_ms: 20_000,
        }
    }

    // ---------- 事件 ----------

    fn emit(&self, event: PlayEvent, actor: Option<ActorRef>, intent_id: Option<String>) -> EventEnvelope {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let env = EventEnvelope {
            id: uuid::Uuid::new_v4().to_string(),
            seq,
            round: self.round.load(Ordering::SeqCst),
            ts: now_iso(),
            actor,
            intent_id,
            event,
        };
        if let Ok(mut log) = self.event_log.lock() {
            log.push(env.clone());
        }
        self.sink.emit(env.clone());
        env
    }

    fn emit_simple(&self, event: PlayEvent) -> EventEnvelope {
        self.emit(event, None, None)
    }

    fn phase(&self, stage: PhaseStage, detail: Option<String>) {
        self.emit_simple(PlayEvent::Phase(PhasePayload { stage, detail }));
    }

    // ---------- 查询 ----------

    pub fn projection(&self) -> WorldProjection {
        let mut st = self.state.lock().expect("state poisoned");
        st.seq = self.seq.load(Ordering::SeqCst);
        st.projection()
    }

    pub fn history(&self, before_seq: Option<Seq>, limit: usize) -> HistoryPage {
        let log = self.event_log.lock().expect("event log poisoned");
        let all: Vec<EventEnvelope> = log
            .iter()
            .filter(|e| before_seq.is_none_or(|b| e.seq < b))
            .cloned()
            .collect();
        let start = all.len().saturating_sub(limit);
        HistoryPage { events: all[start..].to_vec(), has_more: start > 0 }
    }

    pub fn auto_confirm(&self) -> bool {
        self.auto_confirm.load(Ordering::SeqCst)
    }

    // ---------- 玩家动作 ----------

    pub fn confirm(&self, action_id: &str, decision: ConfirmDecision) -> Result<(), EngineError> {
        let mut slot = self.pending.lock().expect("pending poisoned");
        match slot.take() {
            Some(pending) if pending.action_id == action_id => {
                let _ = pending.tx.send(decision);
                Ok(())
            }
            other => {
                *slot = other;
                Err(EngineError::Conflict("expired".into()))
            }
        }
    }

    pub fn set_auto_confirm(&self, v: bool) {
        self.auto_confirm.store(v, Ordering::SeqCst);
        if let Ok(mut st) = self.state.lock() {
            st.meta.auto_confirm = v;
        }
        self.emit_simple(PlayEvent::System(SystemPayload {
            level: SystemLevel::Info,
            code: Some("confirm_toggle".into()),
            text: format!("免确认模式已{}", if v { "开启" } else { "关闭" }),
        }));
    }

    pub fn switch_character(&self, template_id: &str) -> Result<(), EngineError> {
        let found = {
            let st = self.state.lock().expect("state poisoned");
            st.characters.get(template_id).map(|c| (c.name.clone(), c.kind.clone()))
        };
        match found {
            Some((name, kind)) if kind == "pc" => {
                self.state.lock().expect("state poisoned").controlled = vec![template_id.to_string()];
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("switch_character".into()),
                    text: format!("已切换受控角色：{name}"),
                }));
                Ok(())
            }
            Some(_) => Err(EngineError::Conflict("not_controllable".into())),
            None => Err(EngineError::Conflict("character_not_found".into())),
        }
    }

    // ---------- 回合管线（#03） ----------

    pub async fn run_round(&self, input: RoundInput, request_id: Option<String>) -> Result<(), EngineError> {
        if input.text.trim().is_empty() {
            return Err(EngineError::EmptyInput);
        }
        if let Some(rid) = &request_id {
            let mut ids = self.request_ids.lock().expect("request ids poisoned");
            if ids.iter().any(|x| x == rid) {
                return Ok(());
            }
            ids.push(rid.clone());
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(EngineError::RoundInProgress);
        }
        let _guard = BusyGuard(&self.busy);

        let round = self.round.fetch_add(1, Ordering::SeqCst) + 1;
        self.emit_simple(PlayEvent::RoundStart(RoundStartPayload { input: input.clone() }));

        if input.channel == RoundChannel::Meta {
            self.handle_meta(&input.text);
            self.emit_simple(PlayEvent::RoundEnd(RoundEndPayload { round }));
            return Ok(());
        }

        let (scene_title, controlled, chars) = {
            let st = self.state.lock().expect("state poisoned");
            (
                st.scene_title.clone(),
                st.controlled.first().cloned().unwrap_or_default(),
                st.characters
                    .values()
                    .map(|c| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
                    .collect::<Vec<_>>(),
            )
        };
        let ctx = TurnContext {
            save_id: self.save_id.clone(),
            round,
            scene_title,
            controlled,
            player_text: input.text.clone(),
            channel: input.channel,
            characters: chars,
        };

        self.phase(PhaseStage::StoryThinking, None);
        let story = self.ai.story_intents(&ctx).await?;
        for intent in story {
            self.handle_intent(intent, None).await;
        }

        self.phase(PhaseStage::CharacterThinking, None);
        let character = self.ai.character_intents(&ctx).await?;
        // 里程碑：角色意图暂统一归属首个 NPC（真正的 actor 归属随 #04 意图 actor_id 补全）。
        let npc = {
            let st = self.state.lock().expect("state poisoned");
            st.characters
                .values()
                .find(|c| c.kind != "pc")
                .map(|c| ActorRef { id: c.template_id.clone(), name: c.name.clone() })
        };
        for intent in character {
            self.handle_intent(intent, npc.clone()).await;
        }

        self.emit_simple(PlayEvent::RoundEnd(RoundEndPayload { round }));
        Ok(())
    }

    fn handle_meta(&self, text: &str) {
        if text.contains("免确认") {
            let v = !self.auto_confirm.load(Ordering::SeqCst);
            self.set_auto_confirm(v);
        } else if text.contains("帮助") || text.contains("help") {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: None,
                text: "输入你想做的事；元指令：/存档 /免确认 /帮助".into(),
            }));
        } else if text.contains("存档") {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Info,
                code: Some("saved".into()),
                text: "已手动存档".into(),
            }));
        } else {
            self.emit_simple(PlayEvent::System(SystemPayload {
                level: SystemLevel::Warn,
                code: None,
                text: format!("未知元指令：{text}"),
            }));
        }
    }

    async fn handle_intent(&self, intent: Intent, actor: Option<ActorRef>) {
        match intent {
            Intent::Narrate { content } => {
                self.emit(PlayEvent::Narrate(NarratePayload { content, scene_ref: None }), None, None);
            }
            Intent::Speak { content, .. } => {
                self.emit(PlayEvent::Dialogue(DialoguePayload { content, audience: None }), actor, None);
            }
            Intent::Emote { content, emotion } => {
                self.emit(PlayEvent::Emote(EmotePayload { content, emotion, gesture: None }), actor, None);
            }
            Intent::Check { attribute, difficulty } => {
                self.run_check(attribute, difficulty.unwrap_or(12), actor).await;
            }
            Intent::Intervene { content } => {
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("intervene".into()),
                    text: format!("玩家干预：{content}"),
                }));
            }
            Intent::Move { destination_id } => {
                if let Ok(mut st) = self.state.lock() {
                    if let Some(c) = st.controlled.first().cloned() {
                        if let Some(inst) = st.characters.get_mut(&c) {
                            inst.location_id = Some(destination_id.clone());
                        }
                    }
                }
                self.emit_simple(PlayEvent::System(SystemPayload {
                    level: SystemLevel::Info,
                    code: Some("moved".into()),
                    text: format!("移动到 {destination_id}"),
                }));
            }
            Intent::AdvanceScene { target_scene_id, abandon } => {
                self.emit_simple(PlayEvent::Resolution(ResolutionPayload {
                    intent_id: None,
                    status: ResolutionStatus::Ok,
                    rejection_code: None,
                    narrative: Some(if abandon { "你们离开了这里。".into() } else { "场景推进。".into() }),
                    outcome: Some("advance_scene".into()),
                    triggered_events: target_scene_id.map(|t| vec![t]),
                    state_changes: vec![],
                }));
            }
            Intent::QueryWorld { .. } => {}
            Intent::UseSkill { .. } | Intent::UseItem { .. } => {
                self.emit_simple(PlayEvent::Resolution(ResolutionPayload {
                    intent_id: None,
                    status: ResolutionStatus::Ok,
                    rejection_code: None,
                    narrative: Some("动作已结算。".into()),
                    outcome: None,
                    triggered_events: None,
                    state_changes: vec![],
                }));
            }
            Intent::FinishTurn => {}
        }
    }

    async fn run_check(&self, attribute: String, target: i64, actor: Option<ActorRef>) {
        let actor = actor.unwrap_or(ActorRef { id: "char-mira".into(), name: "米拉".into() });
        if !self.auto_confirm.load(Ordering::SeqCst) {
            self.phase(PhaseStage::WaitingConfirm, None);
            let action_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            *self.pending.lock().expect("pending poisoned") = Some(Pending { action_id: action_id.clone(), tx });
            self.emit(
                PlayEvent::Pending(PendingPayload {
                    action_id: action_id.clone(),
                    intent_id: None,
                    actor: actor.clone(),
                    description: format!("用{attribute}进行一次判定"),
                    impact: Some("会消耗一次行动机会".into()),
                    timeout_ms: self.confirmation_timeout_ms,
                }),
                Some(actor.clone()),
                None,
            );
            let decision = match timeout(Duration::from_millis(self.confirmation_timeout_ms), rx).await {
                Ok(Ok(d)) => d,
                _ => {
                    self.pending.lock().expect("pending poisoned").take();
                    ConfirmDecision::Cancel
                }
            };
            if decision == ConfirmDecision::Cancel {
                self.emit(
                    PlayEvent::Resolution(ResolutionPayload {
                        intent_id: None,
                        status: ResolutionStatus::Rejected,
                        rejection_code: Some(RejectionCode::Cancelled.as_str().into()),
                        narrative: Some("你收回了动作。".into()),
                        outcome: None,
                        triggered_events: None,
                        state_changes: vec![],
                    }),
                    Some(actor),
                    None,
                );
                return;
            }
            self.phase(PhaseStage::Resolving, None);
        }

        let roll = {
            let mut rng = self.rng.lock().expect("rng poisoned");
            rng.range_inclusive(1, 20)
        };
        let total = roll;
        let margin = total - target;
        let level = if margin >= 10 {
            SuccessLevel::Great
        } else if margin >= 0 {
            SuccessLevel::Success
        } else if margin >= -10 {
            SuccessLevel::Barely
        } else {
            SuccessLevel::Fail
        };
        self.emit(
            PlayEvent::CheckResult(CheckResultPayload {
                intent_id: None,
                actor: actor.clone(),
                attribute: attribute.clone(),
                expr: Some("1d20".into()),
                rolls: Some(vec![roll]),
                r#mod: 0,
                total,
                target,
                margin,
                result: total >= target,
                level,
                opponent: None,
            }),
            Some(actor.clone()),
            None,
        );

        let ok = total >= target;
        let mut changes = Vec::new();
        if ok {
            let delta = StateDelta {
                domain: octopus_types::DeltaDomain::Flag,
                entity_id: "mine_foreshadow".into(),
                field: "flag".into(),
                op: octopus_types::DeltaOp::Set,
                value: Value::Bool(true),
            };
            changes.push(delta);
            if let Ok(mut st) = self.state.lock() {
                st.flags.insert("mine_foreshadow".into(), Value::Bool(true));
            }
            self.emit_simple(PlayEvent::StateUpdate(StateUpdatePayload { changes: changes.clone() }));
        }
        self.emit(
            PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: Some(if ok { "你注意到了些线索。".into() } else { "你什么也没看清。".into() }),
                outcome: None,
                triggered_events: None,
                state_changes: changes,
            }),
            Some(actor),
            None,
        );
    }
}
