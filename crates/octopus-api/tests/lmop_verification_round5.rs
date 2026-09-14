//! 第五轮 · 独立验证（全新验证者）：专审 T28/T29 的四项改动 + 两处验证者用例改动。
//!
//! 本文件是**新增**的独立用例，不引用前几轮测试文件的任何辅助函数，也不修改任何既有文件。
//! 只验证，不改产品代码。所有 harness / 场景自带。
//!
//! 审计目标：
//!   ① T29 翻转了第四轮验证者的用例（r4_ambusher_still_ignores_check_signature）与第三轮
//!      的 harness（run_ambusher 补判定签名）。本文件独立核对：
//!      · 旧实况是否仍然「可执行地」存在（OLD_AMBUSHER 常量 = git HEAD 原文，跑出 1/1/1）；
//!      · harness 改动是否忠实（真判定必有签名）；
//!      · 翻转后的断言是否仍有牙齿（变异）。
//!   ② T28-A host.force_effect() 与 scale_effect 的正交性（自造技能，不复用引擎单测）。
//!   ③ T28-B apply_delta 丢弃可见化：投影逐字不变 / 确有 WARN / 不落事件 / 重放去重。
//!   ④ 伏击判定签名与 fail-closed。
//!   ⑤ 「16 条挂载点里只有 ambusher 缺签名」的独立复核。
//!
//! 注入式 provider：零网络、零真实模型；引擎级断言用固定种子。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use async_trait::async_trait;
use octopus_api::{router, AppState};
use octopus_engine::lua_host::{
    CheckModifier, LuaCheckContext, LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest,
    MountEnv,
};
use octopus_engine::rng::DeterministicRng;
use octopus_engine::{
    execute_skill, AiOutput, AiProvider, AssetStore, CommandContext, EngineError, PersistedEvent,
    Session, SqliteStore, TurnContext,
};
use octopus_types::{
    AttributeModifier, CheckKind, CheckerDef, DeltaDomain, DeltaOp, EffectDef, EventEnvelope,
    ImmediateEffect, Intent, PlayEvent, ResolutionPayload, ResolutionStatus, ResourceCost,
    RoundChannel, RoundInput, SkillCheck, SkillDef, StateDelta,
};
use serde_json::{json, Value};

// ============================================================
// 注入式 provider + 会话 harness（本文件自带）
// ============================================================

#[derive(Default)]
struct CueProvider {
    queue: Mutex<VecDeque<Vec<Intent>>>,
}

impl CueProvider {
    fn new(scripts: Vec<Vec<Intent>>) -> Arc<Self> {
        Arc::new(Self { queue: Mutex::new(scripts.into()) })
    }
    fn push(&self, intents: Vec<Intent>) {
        self.queue.lock().expect("cue queue poisoned").push_back(intents);
    }
}

#[async_trait]
impl AiProvider for CueProvider {
    async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
        let mut q = self.queue.lock().expect("cue queue poisoned");
        let mut intents = q.pop_front().unwrap_or_default();
        intents.push(Intent::FinishTurn);
        Ok(AiOutput::from_intents(intents))
    }
}

struct Harness {
    app: Arc<AppState>,
    provider: Arc<CueProvider>,
    base: String,
    client: reqwest::Client,
}

async fn spawn_shared(provider: Arc<CueProvider>) -> Harness {
    let store = Arc::new(SqliteStore::open_in_memory().await.unwrap());
    let assets_dir = std::env::temp_dir().join(format!("octopus-r5-{}", uuid::Uuid::new_v4()));
    let assets = Arc::new(AssetStore::open(&assets_dir).await.unwrap());
    let state = AppState::new(store, provider.clone(), assets);
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/auth/login"))
        .json(&json!({
            "username": octopus_engine::DEFAULT_ADMIN_USERNAME,
            "password": octopus_engine::DEFAULT_ADMIN_PASSWORD,
        }))
        .send()
        .await
        .expect("login");
    assert!(resp.status().is_success());
    let login: Value = resp.json().await.unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    let client = reqwest::Client::builder().default_headers(headers).build().unwrap();
    Harness { app: state, provider, base, client }
}

async fn publish_and_open(h: &Harness, title: &str, mut draft: Value) -> (String, String) {
    let created: Value = h
        .client
        .post(format!("{}/api/storybooks", h.base))
        .json(&json!({ "title": title }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sb_id = created["id"].as_str().unwrap().to_string();
    draft["meta"]["id"] = json!(sb_id);
    draft["meta"]["title"] = json!(title);
    let saved: Value = h
        .client
        .put(format!("{}/api/storybooks/{sb_id}", h.base))
        .json(&json!({ "draft": draft, "base_version": created["draft_version"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let res = h
        .client
        .post(format!("{}/api/storybooks/{sb_id}/publish", h.base))
        .json(&json!({ "base_version": saved["doc"]["draft_version"] }))
        .send()
        .await
        .unwrap();
    let status = res.status();
    let body = res.text().await.unwrap();
    assert!(status.is_success(), "发布失败（{status}）：{body}");
    let save: Value = h
        .client
        .post(format!("{}/api/saves", h.base))
        .json(&json!({
            "storybook_id": sb_id,
            "title": null,
            "controlled_character_id": null,
            "is_sandbox": null
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (sb_id, save["id"].as_str().unwrap().to_string())
}

async fn session_of(h: &Harness, save_id: &str) -> Arc<Session> {
    let s = h.app.session_for(save_id).await.unwrap();
    s.set_auto_confirm(true);
    s
}

async fn run(session: &Session, text: &str) -> u32 {
    let before = session.current_round();
    session
        .run_round(RoundInput { channel: RoundChannel::Character, text: text.to_string(), refs: vec![] }, None, vec![])
        .await
        .unwrap();
    before + 1
}

// ============================================================
// 引擎级小工具
// ============================================================

fn lmop() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../story_example/lmop-storybook.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn mount_source(sb: &Value, id: &str) -> String {
    sb["lua_mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(id))
        .and_then(|m| m["source"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn read_data(sb: &Value) -> Value {
    json!({ "characters": sb["characters"].clone(), "definitions": sb["definitions"].clone() })
}

fn host_with(sb: &Value, seed: u64) -> LuaHost {
    let host = LuaHost::new(seed).expect("LuaHost 建不起来");
    host.set_read_data(read_data(sb));
    host
}

fn keep_high_count(reqs: &[LuaRequest]) -> usize {
    reqs.iter()
        .filter(|r| matches!(r, LuaRequest::ModifyCheck { mode: CheckModifier::KeepHigh, .. }))
        .count()
}

/// 伏击的判定签名场景（对自己给的脚本正文跑同一批数据）。
fn run_ambusher_sig(sb: &Value, source: &str, kind: Option<CheckKind>) -> usize {
    let host = host_with(sb, 5);
    let actor = json!({
        "instance_id": "inst-amb", "template_id": "mon-doppelganger", "name": "袭击者",
        "kind": "monster", "attributes": {}, "resources": { "res-hp": 22 }, "statuses": []
    });
    let target = json!({
        "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
        "attributes": { "dex": 16 }, "resources": { "res-hp": 24 },
        "statuses": [{ "id": "dnd-surprised", "name": "受突袭" }]
    });
    let ctx = LuaHostContext {
        script_id: "r5:ambush".into(),
        actor_id: "inst-amb".into(),
        actor,
        target_id: Some("inst-pc".into()),
        target: Some(target),
        ..Default::default()
    };
    let sig = kind.map(|k| LuaCheckContext {
        attribute: "str".into(),
        kind: Some(k),
        target: 12,
        ..Default::default()
    });
    let env = MountEnv { check: sig.as_ref(), ..Default::default() };
    host.run_hook_with(source, LuaMount::CheckPreRoll, &ctx, &env).expect("伏击脚本执行失败");
    keep_high_count(&host.drain_requests())
}

/// 从 scripts/lmop-engine-check/src/main.rs 里抽出内嵌的旧伏击脚本原文（T29 声称 = git HEAD）。
fn old_ambusher_constant() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/lmop-engine-check/src/main.rs");
    let src = std::fs::read_to_string(p).expect("读不到 engine-check 源码");
    let marker = ["const OLD_AMBUSHER: &str = r", "#"].concat();
    let start = src.find(&marker).expect("找不到 OLD_AMBUSHER 常量") + marker.len() + 1;
    let end = start + src[start..].find("#;").expect("OLD_AMBUSHER 常量没有收尾") - 1;
    src[start..end].to_string()
}

// ============================================================
// tracing 订阅者：只收 WARN 及以上（验证「丢弃可见化」）
// ============================================================

static WARNS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SUBSCRIBER: OnceLock<()> = OnceLock::new();

struct WarnCapture;

impl tracing::Subscriber for WarnCapture {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::WARN
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::WARN)
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0.push_str(&format!("{value:?}"));
                } else {
                    self.0.push_str(&format!(" {}={value:?}", field.name()));
                }
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        if let Ok(mut lines) = WARNS.lock() {
            lines.push(format!("{} {}", event.metadata().level(), fields.0));
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

fn install_subscriber() {
    SUBSCRIBER.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(WarnCapture);
    });
}

fn warn_lines_for(needle: &str) -> Vec<String> {
    WARNS
        .lock()
        .map(|l: MutexGuard<Vec<String>>| l.iter().filter(|s| s.contains(needle)).cloned().collect())
        .unwrap_or_default()
}

fn history_events(session: &Session) -> Vec<EventEnvelope> {
    session.history(None, 1_000_000).events
}

/// 命令日志里有没有「丢弃」类的新事件（warn 不得落事件）。
fn has_drop_event(events: &[EventEnvelope]) -> bool {
    events.iter().any(|e| match &e.event {
        PlayEvent::System(p) => {
            p.text.contains("丢弃") || p.text.contains("落不到") || p.text.contains("dropped")
        }
        _ => false,
    })
}

fn resolution_changes(session: &Session, round: u32) -> Vec<(String, String, Value)> {
    history_events(session)
        .into_iter()
        .filter(|e| e.round == round)
        .filter_map(|e| match e.event {
            PlayEvent::Resolution(p) => Some(p.state_changes),
            _ => None,
        })
        .flatten()
        .map(|d| (d.entity_id, d.field, d.value))
        .collect()
}

fn checks_of(session: &Session, round: u32) -> Vec<(bool, Option<CheckKind>)> {
    history_events(session)
        .into_iter()
        .filter(|e| e.round == round)
        .filter_map(|e| match e.event {
            PlayEvent::CheckResult(p) => Some((p.result, p.kind)),
            _ => None,
        })
        .collect()
}

fn pc_hp(session: &Session) -> i64 {
    let p = session.projection();
    let v = &p.characters["inst-pc-lmop-talin"]["resources"]["res-hp"];
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(i64::MIN)
}

// ============================================================
// ① T29 对验证者用例的改动 —— 审计
// ============================================================

/// **旧实况必须仍然可执行地存在**（第四轮 T27 定的安全前提）。
///
/// 独立核对：scripts/lmop-engine-check/src/main.rs 里的 OLD_AMBUSHER 常量
/// ① 是 git HEAD 版 story_example/lmop-storybook.json 的原文（字节级，见报告 §R5 的外部核对）；
/// ② 在本 harness 的同一批场景下跑出 1 / 1 / 1（= T27 的三条原始发现）。
/// 同时钉住交付物（新脚本）为 1 / 0 / 0。
#[test]
fn r5_ambusher_old_reality_is_executable_and_new_one_is_gated() {
    let sb = lmop();
    let old = old_ambusher_constant();
    // 常量与 HEAD 原文同为 974 字节（外部 git show 核对 sha256 = 77a72f91...，见报告 §R5）。
    assert_eq!(old.as_bytes().len(), 974, "OLD_AMBUSHER 常量字节数应等于 git HEAD 原文");
    assert!(!old.contains("host.check_kind"), "旧脚本确实没有判定签名闸门");
    assert!(old.contains("host.modify_check('keep_high')"), "旧脚本确实是伏击那条");

    let old_triple = (
        run_ambusher_sig(&sb, &old, Some(CheckKind::Attack)),
        run_ambusher_sig(&sb, &old, Some(CheckKind::Attribute)),
        run_ambusher_sig(&sb, &old, Some(CheckKind::Save)),
    );
    let new_src = mount_source(&sb, "dnd-ambusher-keep-high");
    let new_triple = (
        run_ambusher_sig(&sb, &new_src, Some(CheckKind::Attack)),
        run_ambusher_sig(&sb, &new_src, Some(CheckKind::Attribute)),
        run_ambusher_sig(&sb, &new_src, Some(CheckKind::Save)),
    );
    println!("R5 伏击 旧脚本(HEAD 原文)={old_triple:?} 新脚本(交付物)={new_triple:?}");
    assert_eq!(old_triple, (1, 1, 1), "旧脚本原文必须照样给出 1/1/1（旧实况没有消失）");
    assert_eq!(new_triple, (1, 0, 0), "新脚本只对攻击检定给优势");
}

/// **翻转后的断言仍有牙齿**：逐条变异，每条都让新断言翻红。
#[test]
fn r5_ambusher_flipped_assertions_have_teeth() {
    let sb = lmop();
    let src = mount_source(&sb, "dnd-ambusher-keep-high");

    // 变异 A：拿掉 kind == attack 闸门 → attribute / save 回到 1。
    let no_gate = src.replace("if host.check_kind ~= \"attack\" then return end\n", "");
    assert_ne!(no_gate, src, "变异必须生效");
    let a = (
        run_ambusher_sig(&sb, &no_gate, Some(CheckKind::Attack)),
        run_ambusher_sig(&sb, &no_gate, Some(CheckKind::Attribute)),
        run_ambusher_sig(&sb, &no_gate, Some(CheckKind::Save)),
    );
    assert_eq!(a, (1, 1, 1), "去掉闸门 → attribute/save 也拿到优势（断言会 FAIL）");

    // 变异 B：闸门写成恒不早退 → 同上，证明不是恒真。
    let always = src.replace(
        "if host.check_kind ~= \"attack\" then return end",
        "if false then return end",
    );
    let b = (
        run_ambusher_sig(&sb, &always, Some(CheckKind::Attribute)),
        run_ambusher_sig(&sb, &always, Some(CheckKind::Save)),
    );
    assert_eq!(b, (1, 1), "闸门失效 → 反例翻红");

    // 变异 C：把最终给出优势的那句拿掉 → attack 也变 0（不是「怎么都过」）。
    let broken = src.replace("host.modify_check('keep_high')", "-- 优势没了");
    assert_ne!(broken, src, "变异 C 必须生效");
    assert_eq!(run_ambusher_sig(&sb, &broken, Some(CheckKind::Attack)), 0, "规则整体失效必须被抓住");
}

/// **fail-closed**：没有判定快照 / kind 缺省 → 不给优势；attack → 给（排除恒 0）。
#[test]
fn r5_ambusher_fail_closed_without_signature() {
    let sb = lmop();
    let src = mount_source(&sb, "dnd-ambusher-keep-high");
    assert_eq!(run_ambusher_sig(&sb, &src, None), 0, "没有判定快照 → 不给优势");
    // kind 缺省：另建一个 kind=None 的快照（有快照但读不到判定种类）。
    let host = host_with(&sb, 5);
    let actor = json!({ "instance_id": "inst-amb", "template_id": "mon-doppelganger", "name": "袭击者",
        "kind": "monster", "attributes": {}, "resources": { "res-hp": 22 }, "statuses": [] });
    let target = json!({ "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
        "attributes": { "dex": 16 }, "resources": { "res-hp": 24 },
        "statuses": [{ "id": "dnd-surprised", "name": "受突袭" }] });
    let ctx = LuaHostContext {
        script_id: "r5:ambush-kindless".into(),
        actor_id: "inst-amb".into(),
        actor,
        target_id: Some("inst-pc".into()),
        target: Some(target),
        ..Default::default()
    };
    let kindless = LuaCheckContext { attribute: "str".into(), kind: None, target: 12, ..Default::default() };
    let env = MountEnv { check: Some(&kindless), ..Default::default() };
    host.run_hook_with(&src, LuaMount::CheckPreRoll, &ctx, &env).expect("执行失败");
    assert_eq!(keep_high_count(&host.drain_requests()), 0, "kind 缺省 → fail-closed");
    assert_eq!(run_ambusher_sig(&sb, &src, Some(CheckKind::Attack)), 1, "对照：attack → 给（排除恒 0）");
}

/// 真判定的签名注入点（给 harness 改动的忠实性做背书）：production 里只有两处调用
/// check_pre_roll，两处都必然带签名；None 只对应「根本没有判定」。
///
/// 独立核验方式（读代码 + 结构断言）：
///   · command.rs：signature = skill_checker(...).is_some().then(|| check_signature(...))
///     ——有判定器才有签名；check_signature 恒写 kind: Some(kind)。
///   · session.rs：Some(&signature) 无条件传入，缺省 kind = Attribute。
#[test]
fn r5_engine_injects_signature_for_every_real_check() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/octopus-engine/src");
    let cmd = std::fs::read_to_string(root.join("command.rs")).unwrap();
    let ses = std::fs::read_to_string(root.join("session.rs")).unwrap();

    // 签名构造器恒写 kind。
    assert!(
        cmd.contains("pub(crate) fn check_signature(attribute: &str, kind: CheckKind, target: i64)")
            && cmd.contains("kind: Some(kind),"),
        "check_signature 必须恒写 kind: Some(...)"
    );
    // 技能路径：有判定器才有签名（无判定器 = 没有判定）。
    assert!(
        cmd.contains("skill_checker(skill, global_checker)\n        .is_some()\n        .then(|| check_signature"),
        "技能路径的签名必须以「有判定器」为前提"
    );
    // 判定意图路径：无条件传签名，缺省 Attribute。
    assert!(
        ses.contains("let signature = crate::command::check_signature(&attribute, signature_kind, target);")
            && ses.contains("run_mount_chain(LuaMount::CheckPreRoll, &mount_ctx, Some(&signature), None)"),
        "判定意图路径必须无条件下发签名"
    );
    assert!(
        ses.contains(".unwrap_or(CheckKind::Attribute);"),
        "判定意图缺省 kind = Attribute"
    );

    // 全仓真实注入点计数（只看运行时调用行，不看测试断言）。
    let mut runtime_injects = 0;
    for f in ["command.rs", "session.rs", "resolve.rs", "effects.rs"] {
        let s = std::fs::read_to_string(root.join(f)).unwrap();
        for line in s.lines() {
            let t = line.trim();
            if t == "LuaMount::CheckPreRoll," || t.contains("run_mount(LuaMount::CheckPreRoll") {
                runtime_injects += 1;
            }
        }
    }
    println!("R5 check_pre_roll 调用行（含测试）= {runtime_injects}；production 注入点 = command.rs / session.rs");
    assert!(runtime_injects >= 2, "至少要有 command / session 两处真实注入点");
}

// ============================================================
// ② T28-A：force_effect 与 scale_effect 正交
// ============================================================

/// 自造技能（不引用 LMoP / 不复用引擎单测 fixture）：豁免 + 两段数值效果 + 标记 + 消耗。
fn r5_gate_skill() -> SkillDef {
    SkillDef {
        id: "sk-r5-gate".into(),
        name: "R5 效果门".into(),
        cost: vec![ResourceCost { resource: "mana".into(), amount: 5 }],
        check: Some(SkillCheck::Def(CheckerDef {
            dice: Some("1d20".into()),
            kind: Some(CheckKind::Save),
            ..Default::default()
        })),
        effect: Some(EffectDef {
            immediate: Some(vec![
                ImmediateEffect::Damage { amount: "1d6".into(), resource: Some("hp".into()) },
                ImmediateEffect::ModifyResource { resource: "mana".into(), amount: "2d6".into() },
                ImmediateEffect::SetFlag { flag: "r5-buried".into(), value: None },
            ]),
            modifiers: Some(vec![AttributeModifier { attribute: "str".into(), value: 2 }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn run_gate(script: Option<&str>, save_succeeds: bool) -> (Vec<StateDelta>, Vec<u64>, Vec<AttributeModifier>) {
    let host = LuaHost::new(11).unwrap();
    let mut registry = LuaRegistry::new();
    registry.register(
        "r5-pin-result",
        LuaMount::CheckPreRoll,
        if save_succeeds { "host.modify_check('force_success')" } else { "host.modify_check('force_fail')" },
    );
    if let Some(s) = script {
        registry.register("r5-rule", LuaMount::CheckPostRoll, s);
    }
    let actor = json!({
        "instance_id": "char-r5", "template_id": "tpl-r5", "name": "甲", "kind": "pc",
        "attributes": { "str": 12 }, "resources": { "hp": 30, "mana": 20 }, "statuses": []
    });
    let skill = r5_gate_skill();
    let lua_ctx = LuaHostContext {
        script_id: "r5:gate".into(),
        actor_id: "char-r5".into(),
        actor: actor.clone(),
        skill: serde_json::to_value(&skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    let rng = Mutex::new(DeterministicRng::new(11));
    let out = {
        let mut ctx = CommandContext {
            actor_id: "char-r5",
            actor: &actor,
            target_id: None,
            target: None,
            difficulty: 10,
            attribute: Some("str".into()),
            global_checker: None,
            rng: &rng,
            lua: Some((&host, &lua_ctx)),
            registry: Some(&registry),
            status_defs: None,
            profiles: None,
            attribute_bonuses: None,
            extra_bonus: 0,
            effect_requires_success: false,
            mount_gate: None,
        };
        execute_skill(&skill, &mut ctx).expect("execute_skill 失败")
    };
    let rng_used = rng.lock().unwrap().consumed.clone();
    (out.deltas().to_vec(), rng_used, out.effects.modifiers.clone())
}

/// 三组对照：
/// (a) 只声明 force_effect → 效果照常结算且**不缩放**（与「豁免失败的全量结算」逐字相同）；
/// (b) 只声明 scale_effect(1.0) → 与 (a) / 全量逐字相同（旧语义：1.0 也开门）；
/// (c) 不声明 → 逐字旧路径（豁免成功 = 效果完全不结算，只剩消耗）。
/// 另加：force + scale(0.5) → 开门 + 缩放；scale(0.5) 单独 → 开门 + 缩放。
#[test]
fn r5_force_effect_is_orthogonal_to_scaling() {
    let (none_success, none_success_rng, none_success_mods) = run_gate(None, true);
    let (none_fail, none_fail_rng, none_fail_mods) = run_gate(None, false);

    // (c) 不声明 + 成功：只有消耗（逐字旧路径）。
    assert_eq!(none_success.len(), 1, "不声明 + 豁免成功：效果完全不结算，只剩消耗");
    assert_eq!(none_success[0].field, "resources.mana");
    assert_eq!(none_success[0].value, json!(-5));
    assert_eq!(none_success_rng.len(), 1, "只掷判定骰");
    assert_eq!(none_success_mods.len(), 0, "不结算 → 静态修正也不产出");
    // 不声明 + 失败：全量（两段数值 + 标记 + 消耗）。
    assert_eq!(none_fail.len(), 4, "不声明 + 豁免失败：引擎全量结算");
    assert_eq!(none_fail_rng.len(), 4, "1 判定骰 + 1d6 + 2d6");

    let (force_success, force_rng, force_mods) = run_gate(Some("host.force_effect()"), true);
    // (a) force_effect 只开门、不缩放 → 与全量逐字相同。
    assert_eq!(force_success, none_fail, "(a) force_effect 单独声明 = 全量效果，数值一个都不缩");
    assert_eq!(force_rng, none_fail_rng, "(a) 开门不改骰序");
    assert_eq!(force_mods, none_fail_mods, "(a) 静态修正也不缩");

    let (unit_success, unit_rng, unit_mods) = run_gate(Some("host.scale_effect(1.0)"), true);
    // (b) scale_effect(1.0) = 旧语义（开门 + 因子 1）→ 与全量逐字相同。
    assert_eq!(unit_success, none_fail, "(b) scale_effect(1.0) 旧语义：开门且数值不动");
    assert_eq!(unit_rng, none_fail_rng, "(b) 骰序一致");
    assert_eq!(unit_mods, none_fail_mods, "(b) 静态修正一致");
    // 旧语义关键点：scale_effect(1.0) 与「不声明」**不同**（前者开门）。
    assert_ne!(unit_success.len(), none_success.len(), "1.0 与不声明行为必须不同（旧语义逐字保留）");

    // 正交叠加：force + 0.5 = 开门 + 缩放；与「只 scale(0.5)」数值一致。
    let (both, both_rng, _) = run_gate(Some("host.force_effect(); host.scale_effect(0.5)"), true);
    let (half, half_rng, _) = run_gate(Some("host.scale_effect(0.5)"), true);
    assert_eq!(both.len(), 4);
    assert_eq!(both, half, "force + scale(0.5) 与只 scale(0.5) 数值相同（force 不参与缩放）");
    assert_eq!(both_rng, half_rng);
    let raw_hp = none_fail[0].value.as_i64().unwrap();
    let raw_mana = none_fail[1].value.as_i64().unwrap();
    assert_eq!(half[0].value.as_i64().unwrap(), (raw_hp as f64 * 0.5).trunc() as i64);
    assert_eq!(half[1].value.as_i64().unwrap(), (raw_mana as f64 * 0.5).trunc() as i64);
    assert_eq!(half[2].value, json!(true), "标记不缩");
    assert_eq!(half[3].value, json!(-5), "消耗不缩");
    // 关键否证：force 单独声明**不得**产生任何缩放（否则「正交」不成立）。
    assert_ne!(force_success[0].value, half[0].value, "force 只开门：不应等于缩放后的值");
    println!(
        "R5 效果门: 无声明成功={:?} 全量={:?} force={:?} scale1.0={:?} scale0.5={:?}",
        none_success.iter().map(|d| d.value.clone()).collect::<Vec<_>>(),
        none_fail.iter().map(|d| d.value.clone()).collect::<Vec<_>>(),
        force_success.iter().map(|d| d.value.clone()).collect::<Vec<_>>(),
        unit_success.iter().map(|d| d.value.clone()).collect::<Vec<_>>(),
        half.iter().map(|d| d.value.clone()).collect::<Vec<_>>(),
    );
}

/// 错时机调用 force_effect 必须当场报错（不是静默丢请求）——与 scale_effect 同口径。
#[test]
fn r5_force_effect_only_registered_where_consumed() {
    let host = LuaHost::new(3).unwrap();
    let c = LuaHostContext::default();
    for mount in [LuaMount::CheckPostRoll, LuaMount::PreResolve] {
        host.run_hook("host.force_effect()", mount, &c).unwrap();
        assert_eq!(host.drain_requests(), vec![LuaRequest::ForceEffect], "{mount:?} 应注册效果门");
    }
    for mount in [LuaMount::CheckPreRoll, LuaMount::PostResolve, LuaMount::Event] {
        let err = host.run_hook("host.force_effect()", mount, &c).unwrap_err();
        assert!(err.to_string().contains("force_effect"), "{mount:?} 应报错：{err}");
        assert!(host.drain_requests().is_empty(), "{mount:?} 不该留下请求");
    }
}

// ============================================================
// ③ T28-B：apply_delta 丢弃可见化
// ============================================================

/// 实时路径：一次落不到角色实体的 Character delta → 投影零变化、确有 WARN、命令日志无新事件。
#[tokio::test]
async fn r5_live_dropped_delta_warns_without_new_events() {
    install_subscriber();
    let sb = lmop();
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R5 丢弃可见化（实时）", sb).await;
    let session = session_of(&h, &save).await;
    let ghost = format!("verify-r5-ghost-{}", uuid::Uuid::new_v4());

    let mut observed = false;
    for _ in 0..60 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some(ghost.clone()),
        }]);
        let before = pc_hp(&session);
        let r = run(&session, "踩到瓦砾").await;
        let c = checks_of(&session, r).into_iter().next().expect("豁免应有 CheckResult");
        assert_eq!(c.1, Some(CheckKind::Save));
        if !c.0 {
            continue;
        }
        let changes = resolution_changes(&session, r);
        let ghost_delta = changes
            .iter()
            .find(|(e, f, _)| e == &ghost && f == "resources.res-hp")
            .unwrap_or_else(|| panic!("减半分支必须留下 ghost delta：{changes:?}"));
        assert!(ghost_delta.2.as_i64().unwrap_or(0) < 0, "ghost delta 必须是伤害");
        assert_eq!(pc_hp(&session), before, "落不到实体的伤害不得改变投影");
        assert!(!session.projection().characters.contains_key(&ghost), "不得幻影建实例");
        observed = true;
        break;
    }
    assert!(observed, "60 次内需观察到一次豁免成功");

    // 确有 WARN：恰好一条（同一形态在进程内只告警一次）。
    let warns = warn_lines_for(&ghost);
    assert_eq!(warns.len(), 1, "落不到实体时必须恰好有一条 WARN（实时），实际 {warns:?}");
    assert!(warns[0].starts_with("WARN"), "必须是 WARN 级：{}", warns[0]);
    assert!(warns[0].contains("resources.res-hp"), "WARN 必须带字段：{}", warns[0]);

    // 命令日志里不得出现「丢弃」类的新事件（重放安全）。
    let events = history_events(&session);
    assert!(!has_drop_event(&events), "丢弃只准写日志，不得落命令日志事件");
    println!("R5 丢弃可见化（实时）：ghost={ghost} warns={warns:?} events={}", events.len());
}

/// 重放路径（纯投影重建）：同一形态重放两次只告警一次；投影逐字不变；不新增事件。
#[tokio::test]
async fn r5_replay_dropped_delta_keeps_projection_verbatim_and_dedups() {
    install_subscriber();
    let sb = lmop();
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R5 丢弃可见化（重放）", sb).await;
    let session = session_of(&h, &save).await;

    let ghost_a = format!("verify-r5-replay-a-{}", uuid::Uuid::new_v4());
    let ghost_b = format!("verify-r5-replay-b-{}", uuid::Uuid::new_v4());
    let mk = |ghost: &str, seq: u64| PersistedEvent {
        request_id: None,
        envelope: EventEnvelope {
            id: format!("r5-evt-{seq}"),
            seq,
            round: 0,
            ts: "2026-01-01T00:00:00Z".into(),
            actor: None,
            intent_id: None,
            event: PlayEvent::Resolution(ResolutionPayload {
                intent_id: None,
                status: ResolutionStatus::Ok,
                rejection_code: None,
                narrative: None,
                outcome: None,
                triggered_events: None,
                state_changes: vec![StateDelta {
                    domain: DeltaDomain::Character,
                    entity_id: ghost.into(),
                    field: "resources.res-hp".into(),
                    op: DeltaOp::Add,
                    value: json!(-7),
                }],
            }),
        },
    };
    // 三条输入：A、A（同形态再来一次 = 重放再跑一遍）、B。
    // seq 用当前投影的 seq，让重放不推进 world seq（投影里带 seq，才能逐字比对）。
    let seq_now = session.projection().seq;
    println!("R5 重放：env.seq={seq_now}");
    let persisted = vec![mk(&ghost_a, seq_now), mk(&ghost_a, seq_now), mk(&ghost_b, seq_now)];

    let before_proj = serde_json::to_value(session.projection()).unwrap();
    let before_events = history_events(&session).len();
    session.replay(&persisted);
    let after_proj = serde_json::to_value(session.projection()).unwrap();
    let after_events = history_events(&session);

    assert_eq!(after_proj, before_proj, "丢弃路径不得改变投影：序列化必须逐字相同");
    assert_eq!(after_events.len(), before_events + persisted.len(), "只准落输入事件，不得新增丢弃事件");
    assert!(!has_drop_event(&after_events), "命令日志不得出现「丢弃」类事件");
    assert!(
        after_events.iter().any(|e| matches!(&e.event, PlayEvent::Resolution(p)
            if p.state_changes.iter().any(|d| d.entity_id == ghost_a))),
        "ghost delta 必须原样保留在日志里（可见化不改写事件）"
    );
    assert_eq!(warn_lines_for(&ghost_a).len(), 1, "A 的同一形态重放两次只告警一次（不刷屏）");
    assert_eq!(warn_lines_for(&ghost_b).len(), 1, "B 各自告警一次（证明 WARN 真的会来）");
    println!("R5 丢弃可见化（重放）：projection verbatim=true events +{}", persisted.len());
}

// ============================================================
// ④ 「16 条挂载点里只有 ambusher 缺签名」的独立复核
// ============================================================

#[test]
fn r5_mount_signature_coverage_scan() {
    let sb = lmop();
    let mounts = sb["lua_mounts"].as_array().unwrap();
    assert_eq!(mounts.len(), 16, "交付物共 16 条挂载点");

    let reads_sig = |src: &str| {
        src.contains("host.check_kind")
            || src.contains("host.check_attribute")
            || src.contains("host.check_result")
            || src.contains("host.check_total")
    };
    let mount_of = |id: &str| mounts.iter().find(|m| m["id"].as_str() == Some(id)).unwrap();

    let mut no_sig = Vec::new();
    let mut sig = Vec::new();
    for m in mounts {
        let id = m["id"].as_str().unwrap();
        let point = m["mount"].as_str().unwrap();
        if !point.starts_with("check_") {
            continue;
        }
        if reads_sig(m["source"].as_str().unwrap_or("")) {
            sig.push(id.to_string());
        } else {
            no_sig.push(id.to_string());
        }
    }
    no_sig.sort();
    sig.sort();
    println!("R5 判定类挂载点读签名：{sig:?}");
    println!("R5 判定类挂载点不读签名：{no_sig:?}");
    assert_eq!(sig.len(), 6, "读签名的判定类挂载点");
    assert_eq!(
        no_sig,
        vec![
            "dnd-consume-inspiration",
            "dnd-skill-dc",
            "dnd-status-keep-high",
            "dnd-status-keep-low",
        ],
        "不读签名的 4 条：全是「对任意判定都成立」的口径（状态给优/劣势、技能 DC、用掉激励），没有声明过 kind 范围"
    );

    // 数据卡明说「攻击检定」的两条，现在都真的按 kind==attack 收敛。
    for id in ["dnd-pack-tactics", "dnd-ambusher-keep-high"] {
        let src = mount_of(id)["source"].as_str().unwrap();
        assert!(src.contains("host.check_kind ~= \"attack\""), "{id} 必须有 attack 闸门");
    }
    // 声明了 signature_scope 的定义必须能在脚本里找到对应判定（结构化声明 vs 实现）。
    let scoped: Vec<&str> = sb["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["fields"]["signature_scope"].is_string())
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert!(scoped.contains(&"pack-wolf") && scoped.contains(&"ambush-doppelganger"));
    let sunlight = mount_of("dnd-sunlight-sensitivity")["source"].as_str().unwrap();
    assert!(sunlight.contains("attribute == 'wis'"), "日照敏感按 attribute+wis 分支");
    let prof = mount_of("dnd-proficiency")["source"].as_str().unwrap();
    assert!(prof.contains("host.check_kind ~= 'attribute'"), "熟练只对属性检定");
    let save_half = mount_of("dnd-save-half")["source"].as_str().unwrap();
    assert!(save_half.contains("host.check_kind ~= 'save'"), "豁免减半只对豁免");
    let surprise = mount_of("dnd-surprise")["source"].as_str().unwrap();
    assert!(surprise.contains("host.check_kind == 'attribute'"), "突袭按 kind+attribute");
}
