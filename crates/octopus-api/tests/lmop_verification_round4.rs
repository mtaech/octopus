//! 第四轮 · 独立验证（全新验证者）：专审 T26 对「第三轮验证资产」的改动。
//!
//! 本轮只验证、不改产品代码；本文件是**新增**的独立用例，不引用前几轮测试文件的任何辅助函数。
//! 所有 harness 自带，所有场景自带；被测脚本从交付物 story_example/lmop-storybook.json
//! 里**读出原文**（不抄），变异只施加在内存副本上。
//!
//! 审计目标：
//!   ① T26 把第三轮审计用例 r3_audit4 的五条断言**翻转**了（FP-A/B/C/E 期望 1→0、FN-A 期望 0→1）。
//!      本轮用 T24 当初的**原始 5 类场景** + git HEAD 的**旧脚本原文**独立复现「行为真的变了」，
//!      再逐条把每一处修复改回去，确认新断言会 FAIL（不是恒真 / 不是把值改小改没）。
//!   ② a12 / a12b 新增的 PostResolve 机制探针：自己造反例（旧重掷脚本 / 缩放+Lua 双补），
//!      确认「松断言照过、机制判据挂掉」。
//!   ③ r2_6 的新判据（读 Resolution.state_changes 里的 ghost delta）：确认它真能区分
//!      「分支被跳过」与「算了被丢弃」。
//!
//! 注入式 provider：零网络、零真实模型；引擎级断言用固定种子，逐位确定。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use octopus_api::{router, AppState};
use octopus_engine::lua_host::{
    CheckModifier, LuaCheckContext, LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest,
    MountEnv,
};
use octopus_engine::{
    execute_skill, AiOutput, AiProvider, AssetStore, CommandContext, CommandOutcome, EngineError,
    Session, SqliteStore, TurnContext,
};
use octopus_types::{
    CheckKind, EventEnvelope, Intent, PlayEvent, RoundChannel, RoundInput, SkillDef,
};
use serde_json::{json, Value};

// ============================================================
// 会话 harness（本文件自带；只为 r2_6 那条「读 Resolution.state_changes」的判据）
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
    let assets_dir = std::env::temp_dir().join(format!("octopus-r4-{}", uuid::Uuid::new_v4()));
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

fn rounds_events(session: &Session, round: u32) -> Vec<EventEnvelope> {
    session.history(None, 1_000_000).events.into_iter().filter(|e| e.round == round).collect()
}

/// 该回合所有 Resolution 事件声明的 state_changes（引擎结算产物，落不到实体也留证）。
fn resolution_changes(session: &Session, round: u32) -> Vec<(String, String, Value)> {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::Resolution(p) => Some(p.state_changes),
            _ => None,
        })
        .flatten()
        .map(|d| (d.entity_id, d.field, d.value))
        .collect()
}

fn dice_count(session: &Session, round: u32) -> usize {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => {
                serde_json::from_str::<Vec<u64>>(&p.text).ok()
            }
            _ => None,
        })
        .flatten()
        .count()
}

fn num(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(i64::MIN)
}

fn pc_hp(session: &Session) -> i64 {
    let proj = session.projection();
    num(&proj.characters["inst-pc-lmop-talin"]["resources"]["res-hp"])
}

fn checks_kind(session: &Session, round: u32) -> Vec<(bool, Option<CheckKind>)> {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::CheckResult(p) => Some((p.result, p.kind)),
            _ => None,
        })
        .collect()
}

// ============================================================
// 引擎级小工具（确定性；从交付物读脚本）
// ============================================================

fn lmop() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../story_example/lmop-storybook.json");
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
    json!({
        "characters": sb["characters"].clone(),
        "definitions": sb["definitions"].clone(),
    })
}

fn host_with(sb: &Value, seed: u64) -> LuaHost {
    let host = LuaHost::new(seed).expect("LuaHost 建不起来");
    host.set_read_data(read_data(sb));
    host
}

fn skill_of(sb: &Value, id: &str) -> SkillDef {
    let raw = sb["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .expect("技能必须存在")
        .clone();
    serde_json::from_value(raw).expect("技能解析失败")
}

fn instance_json(sb: &Value, template_id: &str, key: &str) -> Value {
    let tpl = sb["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str() == Some(template_id))
        .expect("模板必须存在");
    let mut o = tpl.as_object().cloned().unwrap();
    o.insert("instance_id".into(), json!(key));
    o.insert("template_id".into(), json!(template_id));
    o.entry("statuses".to_string()).or_insert_with(|| json!([]));
    o.entry("attributes".to_string()).or_insert_with(|| json!({}));
    o.entry("resources".to_string()).or_insert_with(|| json!({}));
    o.insert("present".into(), json!(true));
    Value::Object(o)
}

fn keep_high_count(reqs: &[LuaRequest]) -> usize {
    reqs.iter()
        .filter(|r| matches!(r, LuaRequest::ModifyCheck { mode: CheckModifier::KeepHigh, .. }))
        .count()
}

// ============================================================
// 集群战术：与第三轮 r3_audit4 等价的场景构造（本轮自带，独立于 T26 的用例）
// ============================================================

fn wolf_char(key: &str, hp: i64, place: Option<&str>, statuses: Value) -> Value {
    json!({
        "instance_id": key, "template_id": "mon-wolf", "name": "狼", "kind": "monster",
        "attributes": {}, "resources": { "res-hp": hp }, "statuses": statuses,
        "location_id": place, "present": true
    })
}

fn facts_with(chars: Vec<Value>, encounters: Value) -> Value {
    let mut map = serde_json::Map::new();
    for c in chars {
        map.insert(c["instance_id"].as_str().unwrap().to_string(), c);
    }
    json!({ "characters": map, "flags": {}, "encounters": encounters })
}

fn pack_member(key: &str, hp: i64) -> Value {
    json!({ "id": key, "instance_id": key, "template_id": "mon-wolf", "hp": hp })
}

fn pack_advantage(
    source: &str,
    sb: &Value,
    facts: Value,
    actor_place: Option<&str>,
    target_place: Option<&str>,
    check_kind: Option<CheckKind>,
) -> usize {
    let host = host_with(sb, 5);
    host.set_world_facts(facts);
    let actor = json!({
        "instance_id": "inst-wolf-1", "template_id": "mon-wolf", "name": "狼", "kind": "monster",
        "attributes": {}, "resources": { "res-hp": 11 }, "statuses": [],
        "location_id": actor_place, "present": true
    });
    let target = json!({
        "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
        "attributes": { "dex": 16 }, "resources": { "res-hp": 24 }, "statuses": [],
        "location_id": target_place, "present": true
    });
    let ctx = LuaHostContext {
        script_id: "r4:pack".into(),
        actor_id: "inst-wolf-1".into(),
        actor,
        target_id: Some("inst-pc".into()),
        target: Some(target),
        ..Default::default()
    };
    let signature = check_kind.map(|kind| LuaCheckContext {
        attribute: "str".into(),
        kind: Some(kind),
        target: 12,
        ..Default::default()
    });
    let gate = |_: &octopus_types::CondExpr| true;
    let env = MountEnv { gate: Some(&gate), check: signature.as_ref(), ..Default::default() };
    host.run_hook_with(source, LuaMount::CheckPreRoll, &ctx, &env).expect("集群战术脚本执行失败");
    keep_high_count(&host.drain_requests())
}

/// 五个原始场景（T24 用的那五类）+ 两个对照 → 优势计数。
struct PackScenarios {
    fp_a: usize,
    fp_b: usize,
    fp_c: usize,
    fp_e_attr: usize,
    fn_a: usize,
    p0: usize,
    fp_e_attack: usize,
}

fn run_pack_scenarios(source: &str, sb: &Value) -> PackScenarios {
    let one_enc = |members: Value| {
        json!({ "enc-1": { "id": "enc-1", "name": "野外遭遇", "active": true, "enemies": members } })
    };
    let two = |place2: Option<&str>, st2: Value| {
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf_char("inst-wolf-2", 11, place2, st2),
            ],
            one_enc(json!([pack_member("inst-wolf-1", 11), pack_member("inst-wolf-2", 11)])),
        )
    };
    // FP-A：同伴在另一个地点（loc-c），狼与目标都在 loc-a
    let fp_a = pack_advantage(
        source, sb, two(Some("loc-c"), json!([])), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack),
    );
    // FP-B：同伴 HP>0 但带失能状态 stunned
    let fp_b = pack_advantage(
        source, sb, two(Some("loc-a"), json!([{ "id": "stunned", "name": "昏迷" }])),
        Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack),
    );
    // FP-C：狼与目标都没有 location_id
    let fp_c = pack_advantage(
        source,
        sb,
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, None, json!([])),
                wolf_char("inst-wolf-2", 11, None, json!([])),
            ],
            one_enc(json!([pack_member("inst-wolf-1", 11), pack_member("inst-wolf-2", 11)])),
        ),
        None,
        None,
        Some(CheckKind::Attack),
    );
    // FP-E：非攻击判定（属性检定）
    let fp_e_attr = pack_advantage(
        source, sb, two(Some("loc-a"), json!([])), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attribute),
    );
    // FN-A：同伴在另一场遭遇，但同在 loc-a
    let fn_a = pack_advantage(
        source,
        sb,
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf_char("inst-wolf-2", 11, Some("loc-a"), json!([])),
            ],
            json!({
                "enc-1": { "id": "enc-1", "name": "遭遇一", "active": true, "enemies": [pack_member("inst-wolf-1", 11)] },
                "enc-2": { "id": "enc-2", "name": "遭遇二", "active": true, "enemies": [pack_member("inst-wolf-2", 11)] }
            }),
        ),
        Some("loc-a"),
        Some("loc-a"),
        Some(CheckKind::Attack),
    );
    // 正例 / 攻击检定仍给
    let p0 = pack_advantage(
        source, sb, two(Some("loc-a"), json!([])), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack),
    );
    let fp_e_attack = p0;
    PackScenarios { fp_a, fp_b, fp_c, fp_e_attr, fn_a, p0, fp_e_attack }
}

/// T23/T24 **之前**的旧集群战术脚本（逐字取自 git show HEAD:story_example/lmop-storybook.json；
/// 本文件另有 shell 层核对：与 HEAD 的逐字一致性见报告 §R4-1）。
const OLD_PACK_TACTICS: &str = r#"-- 规则包：集群战术（狼，附录 B 原文）。
-- 数据来源：开放内容（角色模板的 dnd-pack-tactics 挂接），脚本无生物名单。
-- T21：读运行时事实——同一场遭遇的同伴（list_encounters + get_character）。
-- 仍受 GAP-B（没有位置 / 距离）：同伴按「同一场遭遇」近似，目标按「同一 location_id」近似。
local ids = host.get_attachment('dnd-pack-tactics')
if type(ids) ~= "table" or #ids == 0 then return end
local me = host.actor
local target = host.target
if type(me) ~= "table" or type(target) ~= "table" then return end
local my_key = me.id or me.instance_id
local function alive(inst)
  local hp = inst and inst.resources and inst.resources['res-hp']
  return type(hp) == "number" and hp > 0
end
local ally_found = false
for _, enc in ipairs(host.list_encounters()) do
  local enemies = enc.enemies
  if type(enemies) == "table" then
    local mine = false
    for _, e in ipairs(enemies) do
      if (e.instance_id or e.id) == my_key then mine = true end
    end
    if mine then
      for _, e in ipairs(enemies) do
        local key = e.instance_id or e.id
        if key and key ~= my_key then
          if alive(host.get_character(key)) then ally_found = true end
        end
      end
    end
  end
end
if not ally_found then return end
-- 「在目标 5 尺内」的可用近似：双方都知道地点时必须同地点（GAP-B 仍然存在）。
local my_place = me.location_id
local target_place = target.location_id
if my_place and target_place and my_place ~= target_place then return end
host.modify_check('keep_high')"#;

// ============================================================
// §R4-1：T26 对 r3_audit4 的翻转 —— 行为真的变了吗？
// ============================================================

#[test]
fn r4_audit4_old_script_reproduces_t24_findings() {
    let sb = lmop();
    let old = run_pack_scenarios(OLD_PACK_TACTICS, &sb);
    println!(
        "R4-1 旧脚本（git HEAD 原文）: P0={} FP-A={} FP-B={} FP-C={} FP-E(attr)={} FN-A={}",
        old.p0, old.fp_a, old.fp_b, old.fp_c, old.fp_e_attr, old.fn_a
    );
    assert_eq!(old.p0, 1, "旧脚本正例仍然给优势（场景本身有效）");
    assert_eq!(old.fp_a, 1, "旧脚本 FP-A：同伴在别的地点仍给优势（T24 原始结论）");
    assert_eq!(old.fp_b, 1, "旧脚本 FP-B：同伴带失能状态仍给优势（T24 原始结论）");
    assert_eq!(old.fp_c, 1, "旧脚本 FP-C：双方无 location_id 仍给优势（T24 原始结论）");
    assert_eq!(old.fp_e_attr, 1, "旧脚本 FP-E：属性检定也给优势（T24 原始结论）");
    assert_eq!(old.fn_a, 0, "旧脚本 FN-A：同伴在另一场遭遇 → 漏判（T24 原始结论）");
}

#[test]
fn r4_audit4_new_script_flips_all_five() {
    let sb = lmop();
    let new = mount_source(&sb, "dnd-pack-tactics");
    assert!(new.contains("incapacitated_statuses"), "交付物必须是新脚本");
    let r = run_pack_scenarios(&new, &sb);
    println!(
        "R4-1 新脚本（交付物）: P0={} FP-A={} FP-B={} FP-C={} FP-E(attr)={} FN-A={}",
        r.p0, r.fp_a, r.fp_b, r.fp_c, r.fp_e_attr, r.fn_a
    );
    assert_eq!(r.p0, 1, "P0 正例必须仍给优势（两端之一：没有退化成恒 0）");
    assert_eq!(r.fp_e_attack, 1, "攻击检定仍给优势");
    assert_eq!(r.fp_a, 0, "FP-A 已修正（同伴位置真检查）");
    assert_eq!(r.fp_b, 0, "FP-B 已修正（失能名单来自开放内容）");
    assert_eq!(r.fp_c, 0, "FP-C 已修正（缺地点 fail-closed）");
    assert_eq!(r.fp_e_attr, 0, "FP-E 已修正（只对 kind==attack）");
    assert_eq!(r.fn_a, 1, "FN-A 已修正（同伴跨遭遇算数）");
}

#[test]
fn r4_audit4_each_fix_reverted_fails_the_new_assertions() {
    let sb = lmop();
    let new = mount_source(&sb, "dnd-pack-tactics");
    let mut report: Vec<String> = Vec::new();

    // M1：把「同伴位置真检查」改回去（旧行为：从不看同伴地点）
    let m1 = new.replace(
        "if able(inst) and inst.location_id == target_place then",
        "if able(inst) then",
    );
    assert_ne!(m1, new, "M1 替换必须发生");
    let r1 = run_pack_scenarios(&m1, &sb);
    report.push(format!("M1(去掉同伴位置检查) FP-A={}（新断言期望 0）", r1.fp_a));
    assert_eq!(r1.fp_a, 1, "M1：去掉同伴位置检查后 FP-A 必须回到 1 → 新断言 FAIL");
    assert_eq!(r1.fp_b, 0, "M1 只影响位置这一条，别的修复不动");

    // M2：把「失能名单」改回去（旧行为：HP>0 即「未失能」）
    let m2 = new.replace(
        "  local statuses = inst.statuses\n  if type(statuses) == \"table\" then\n    for _, st in ipairs(statuses) do\n      local id = st.id or st.name\n      if id and incapacitated[id] then return false end\n    end\n  end\n",
        "",
    );
    assert_ne!(m2, new, "M2 替换必须发生");
    assert!(!m2.contains("incapacitated[id]"), "M2 后不应再有失能判定");
    let r2 = run_pack_scenarios(&m2, &sb);
    report.push(format!("M2(去掉失能名单判定) FP-B={}（新断言期望 0）", r2.fp_b));
    assert_eq!(r2.fp_b, 1, "M2：HP>0 即算未失能后 FP-B 必须回到 1 → 新断言 FAIL");

    // M3：把 fail-closed 去掉（旧行为：没地点就跳过地点闸门）
    let m3 = new.replace(
        "local target_place = target.location_id\nif type(target_place) ~= \"string\" or target_place == \"\" then return end\n",
        "local target_place = target.location_id\n",
    );
    assert_ne!(m3, new, "M3 替换必须发生");
    let r3 = run_pack_scenarios(&m3, &sb);
    report.push(format!("M3(去掉缺地点 fail-closed) FP-C={}（新断言期望 0）", r3.fp_c));
    assert_eq!(r3.fp_c, 1, "M3：去掉 fail-closed 后 FP-C 必须回到 1 → 新断言 FAIL");

    // M4：去掉判定签名闸门（旧行为：任何判定都给优势）
    let m4 = new.replace("if host.check_kind ~= \"attack\" then return end\n", "");
    assert_ne!(m4, new, "M4 替换必须发生");
    let r4 = run_pack_scenarios(&m4, &sb);
    report.push(format!("M4(去掉 kind==attack 闸门) FP-E(attr)={}（新断言期望 0）", r4.fp_e_attr));
    assert_eq!(r4.fp_e_attr, 1, "M4：去掉签名闸门后 FP-E 必须回到 1 → 新断言 FAIL");

    // M5：把「同伴取自所有遭遇」改回去（旧行为：只看我所在那一场）
    let m5 = new.replace(
        "      local key = e.instance_id or e.id\n      if key and key ~= my_key then\n",
        "      local key = e.instance_id or e.id\n      local mine = false\n      for _, e2 in ipairs(enemies) do\n        if (e2.instance_id or e2.id) == my_key then mine = true end\n      end\n      if mine and key and key ~= my_key then\n",
    );
    assert_ne!(m5, new, "M5 替换必须发生");
    let r5 = run_pack_scenarios(&m5, &sb);
    report.push(format!("M5(只看我所在遭遇) FN-A={}（新断言期望 1）", r5.fn_a));
    assert_eq!(r5.fn_a, 0, "M5：只看我所在遭遇后 FN-A 必须回到 0 → 新断言 FAIL");

    // 逐条变异后的其余场景不应被带偏（说明每条修复各自独立）
    assert_eq!(r1.fp_e_attr, 0, "M1 不移除签名闸门");
    assert_eq!(r2.fp_a, 0, "M2 不移除位置检查");
    assert_eq!(r3.fp_b, 0, "M3 不移除失能判定");
    assert_eq!(r4.fp_a, 0, "M4 不移除位置检查");
    println!("R4-1 变异验证: {}", report.join(" | "));
}

#[test]
fn r4_pack_incapacitated_list_is_open_content_data_driven() {
    let sb = lmop();
    let source = mount_source(&sb, "dnd-pack-tactics");
    let one_enc = json!({ "enc-1": { "id": "enc-1", "name": "野外遭遇", "active": true,
        "enemies": [pack_member("inst-wolf-1", 11), pack_member("inst-wolf-2", 11)] } });
    let scenario = |st: Value| {
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf_char("inst-wolf-2", 11, Some("loc-a"), st),
            ],
            one_enc.clone(),
        )
    };
    let stunned = json!([{ "id": "stunned", "name": "昏迷" }]);
    let prone = json!([{ "id": "prone", "name": "倒地" }]);

    // 基线与声明一致：stunned 在名单里 → 不给；prone 不在名单 → 给
    let base_stunned = pack_advantage(&source, &sb, scenario(stunned.clone()), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    let base_prone = pack_advantage(&source, &sb, scenario(prone.clone()), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    assert_eq!(base_stunned, 0, "交付物：stunned 在 incapacitated_statuses 里 → 不算未失能");
    assert_eq!(base_prone, 1, "交付物：prone 不在名单里 → 仍算未失能");

    // 只改开放内容的名单（stunned → prone），脚本逐字不动
    let mut sb2 = sb.clone();
    let defs = sb2["definitions"].as_array_mut().unwrap();
    let def = defs.iter_mut().find(|d| d["kind"] == "dnd-pack-tactics").expect("必须有集群战术定义");
    def["fields"]["incapacitated_statuses"] = json!(["prone"]);
    let mut_stunned = pack_advantage(&source, &sb2, scenario(stunned), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    let mut_prone = pack_advantage(&source, &sb2, scenario(prone), Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    println!(
        "R4-6 失能名单数据驱动: 基线 stunned={base_stunned}/prone={base_prone} → 只改名单 stunned={mut_stunned}/prone={mut_prone}"
    );
    assert_eq!(mut_stunned, 1, "名单改成 [prone] 后 stunned 不再算失能 → 给优势（证明名单来自开放内容）");
    assert_eq!(mut_prone, 0, "名单改成 [prone] 后 prone 算失能 → 不给优势");
}

/// 声明逐条对账（§R4-6）：active_only / 同伴取自所有遭遇 / 目标缺地点 fail-closed。
#[test]
fn r4_pack_declaration_matches_implementation() {
    let sb = lmop();
    let source = mount_source(&sb, "dnd-pack-tactics");
    // 声明说「所有**活跃**遭遇」：非活跃遭遇里的同伴不得算
    let inactive = facts_with(
        vec![
            wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
            wolf_char("inst-wolf-2", 11, Some("loc-a"), json!([])),
        ],
        json!({ "enc-1": { "id": "enc-1", "name": "已结束", "active": false,
                           "enemies": [pack_member("inst-wolf-2", 11)] } }),
    );
    let got = pack_advantage(&source, &sb, inactive, Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    assert_eq!(got, 0, "非活跃遭遇里的同伴不得算（声明：所有**活跃**遭遇）");

    // 声明说「不限与我同场」：我在遭遇外、同伴在活跃遭遇里且与目标同地点 → 必须算
    let ally_elsewhere = facts_with(
        vec![
            wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
            wolf_char("inst-wolf-2", 11, Some("loc-a"), json!([])),
        ],
        json!({ "enc-1": { "id": "enc-1", "name": "别人的遭遇", "active": true,
                           "enemies": [pack_member("inst-wolf-2", 11)] } }),
    );
    let got2 = pack_advantage(&source, &sb, ally_elsewhere, Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    assert_eq!(got2, 1, "我不在任何遭遇、但活跃遭遇里有同地点同伴 → 必须算（声明：不限与我同场）");

    // 声明说目标没有 location_id 时 fail-closed
    let no_target_place = facts_with(
        vec![
            wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
            wolf_char("inst-wolf-2", 11, Some("loc-a"), json!([])),
        ],
        json!({ "enc-1": { "id": "enc-1", "name": "遭遇", "active": true,
                           "enemies": [pack_member("inst-wolf-1", 11), pack_member("inst-wolf-2", 11)] } }),
    );
    let got3 = pack_advantage(&source, &sb, no_target_place, Some("loc-a"), None, Some(CheckKind::Attack));
    assert_eq!(got3, 0, "目标没有 location_id → fail-closed（声明如此）");

    // 脚本里没有状态名常量（失能名单只来自开放内容）
    for name in ["stunned", "unconscious", "paralyzed", "petrified", "incapacitated"] {
        assert!(
            !source.contains(&format!("'{name}'")) && !source.contains(&format!("\"{name}\"")),
            "脚本不得烘死状态名 {name}"
        );
    }
    println!("R4-6 声明对账 PASS: 非活跃遭遇=0 / 不限同场=1 / 目标缺地点=0 / 无状态名常量");
}

/// 原 §R4-6 附带独立发现①（**伏击不看判定签名**）的**修正后**用例（第 5 轮翻转）。
///
/// 历史（不改写）：本用例原名 `r4_ambusher_still_ignores_check_signature`，是第四轮验证者
/// T27 按「如实钉住现状」写的——数据卡原文是「对任何成功受其**突袭**的生物所发动的
/// **攻击检定**具有优势」，而当时的脚本只读 host.target.statuses、**没有** host.check_kind
/// 闸门，于是属性检定 / 豁免同样拿到优势（与 T24 在集群战术上记的 FP-E 同类）。
/// 它当时断言的正是「缺陷存在」：源码里没有 host.check_kind、attribute / save 都 = 1。
///
/// 第 5 轮（T26 修正轮之后）把伏击按数据卡收敛到 `kind == attack` 后，那个口径不再成立：
/// 本用例改为**断言修正后的正确行为**（attack → 给；attribute / save → 不给），
/// 不再用任何源码子串断言钉实现细节（那正是 T27 发现③ 批评的「注释即可满足」形态）。
///
/// **旧实况没有被抹掉**：旧脚本原文以 `OLD_AMBUSHER` 内嵌在
/// `scripts/lmop-engine-check/src/main.rs`，由引擎级断言
/// `ambusher.old_impl_ignores_check_signature` 在同一批场景复现 1 / 1 / 1
/// （= T27 的三条原始发现，可执行地存活，不依赖任何会被翻转的断言）。
///
/// 失效条件（任一成立即 FAIL，说明判据仍有牙齿）：
///   1. `check_kind == attack` 闸门被去掉或写错 → attribute / save 回到 1；
///   2. 闸门被写成放行所有判定种类（恒给优势）→ attribute / save 回到 1；
///   3. 规则整体失效 → attack 变成 0（不是放宽成「怎么都算过」）；
///   4. 目标状态判定被改坏 → 带 dnd-surprised 也不再给优势 → attack 变 0。
#[test]
fn r4_ambusher_check_signature_attack_only_after_r5_fix() {
    let sb = lmop();
    // 交付物里的脚本正文（本用例不再对它做子串断言，只把它当被测对象跑行为）。
    let source = mount_source(&sb, "dnd-ambusher-keep-high");
    let run = |kind: CheckKind| -> usize {
        let host = host_with(&sb, 5);
        let actor = json!({
            "instance_id": "inst-amb", "template_id": "mon-doppelganger", "name": "袭击者", "kind": "monster",
            "attributes": {}, "resources": { "res-hp": 22 }, "statuses": []
        });
        let target = json!({
            "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
            "attributes": { "dex": 16 }, "resources": { "res-hp": 24 },
            "statuses": [{ "id": "dnd-surprised", "name": "受突袭" }],
            "location_id": "loc-a", "present": true
        });
        let ctx = LuaHostContext {
            script_id: "r4:ambush".into(),
            actor_id: "inst-amb".into(),
            actor,
            target_id: Some("inst-pc".into()),
            target: Some(target),
            ..Default::default()
        };
        let sig = LuaCheckContext { attribute: "str".into(), kind: Some(kind), target: 12, ..Default::default() };
        let env = MountEnv { check: Some(&sig), ..Default::default() };
        host.run_hook_with(&source, LuaMount::CheckPreRoll, &ctx, &env).expect("伏击脚本执行失败");
        keep_high_count(&host.drain_requests())
    };
    let attack = run(CheckKind::Attack);
    let attribute = run(CheckKind::Attribute);
    let save = run(CheckKind::Save);
    println!("R4-6 伏击签名: attack={attack} attribute={attribute} save={save}（数据卡只说攻击检定）");
    assert_eq!(attack, 1, "攻击检定给优势（数据卡如此；两端之一：没有退化成恒 0）");
    // 第 5 轮翻转：原「现状」断言（attribute / save 也 = 1）改为断言修正后的正确行为。
    // 旧的「缺陷存在」这一实况改由引擎级 ambusher.old_impl_ignores_check_signature 复现。
    assert_eq!(attribute, 0, "属性检定不给优势（数据卡只说攻击检定；修正后行为）");
    assert_eq!(save, 0, "豁免检定不给优势（修正后行为）");
}

// ============================================================
// §R4-2：a12 / a12b 的机制判据（PostResolve 探针）—— 独立反例
// ============================================================

struct SaveRun {
    out: CommandOutcome,
    dice: Vec<u64>,
}

const PROBE: &str = r#"-- 第四轮自带探针：读引擎**自己算出的**效果快照
local e = host.resolved_effects
if e == nil then
  host.set_flag('r4.missing', 'true')
  host.set_flag('r4.factor', 'nil')
  host.set_flag('r4.rng', '0')
  host.set_flag('r4.deltas', '0')
  host.set_flag('r4.hp', 'none')
  return
end
host.set_flag('r4.missing', 'false')
host.set_flag('r4.factor', tostring(e.factor))
host.set_flag('r4.rng', tostring(e.rng_consumed))
host.set_flag('r4.deltas', tostring(#e.deltas))
local hp = 'none'
for _, d in ipairs(e.deltas) do
  if d.field == 'resources.res-hp' then hp = tostring(d.value) end
end
host.set_flag('r4.hp', hp)
"#;

/// 旧重掷脚本（T23 之前），用于「换回旧机制 → 机制判据 FAIL」的独立复现。
const OLD_REROLL_RULE: &str = r#"if host.check_kind == 'save' and host.check_result == true then
  local definition = host.definition
  local effect = definition and definition.effect
  local immediate = effect and effect.immediate
  local first = immediate and immediate[1]
  if first and first.kind == 'damage' and first.amount then
    local expression = tostring(first.amount)
    local count, sides, tail = string.match(expression, '^(%d+)d(%d+)(.*)$')
    local total = 0
    if count then
      for _ = 1, tonumber(count) do
        total = total + host.engine_rng(1, tonumber(sides))
      end
      total = total + (tonumber(tail) or 0)
    else
      total = tonumber(expression) or 0
    end
    local half = math.floor(total / 2)
    if half > 0 then
      local target = host.target and host.target.id
      if target then
        host.apply_effect(target, { kind = 'damage', amount = tostring(half), resource = first.resource or 'res-hp' })
      end
    end
  end
end"#;

fn run_save_half(sb: &Value, seed: u64, rule: &str, pin_success: bool, target_id: Option<&str>) -> SaveRun {
    let host = host_with(sb, seed);
    let mut registry = LuaRegistry::new();
    registry.register(
        "r4-pin-result",
        LuaMount::CheckPreRoll,
        if pin_success { "host.modify_check('force_success')" } else { "host.modify_check('force_fail')" },
    );
    if !rule.is_empty() {
        registry.register("rule:save-half", LuaMount::CheckPostRoll, rule);
    }
    registry.register("r4-probe", LuaMount::PostResolve, PROBE);
    let skill = skill_of(sb, "sk-lmop-rubble-collapse");
    let actor = instance_json(sb, "pc-lmop-talin", "inst-pc-lmop-talin");
    let target = target_id.map(|t| instance_json(sb, "pc-lmop-talin", t));
    let lua_ctx = LuaHostContext {
        script_id: "r4:save-half".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: actor.clone(),
        target_id: target_id.map(|t| t.to_string()),
        target: target.clone(),
        skill: serde_json::to_value(&skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    let rng = host.rng_handle();
    let out = {
        let mut ctx = CommandContext {
            actor_id: "inst-pc-lmop-talin",
            actor: &actor,
            target_id,
            target: target.as_ref(),
            difficulty: 10,
            attribute: None,
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
    let dice = rng.lock().unwrap().consumed.clone();
    SaveRun { out, dice }
}

fn probe_flags(out: &CommandOutcome) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for r in &out.requests {
        if let LuaRequest::ApplyEffect { effect, .. } = r {
            if effect.get("kind").and_then(Value::as_str) == Some("set_flag") {
                let flag = effect.get("flag").and_then(Value::as_str).unwrap_or("").to_string();
                let val = match effect.get("value") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => "nil".into(),
                };
                if flag.starts_with("r4.") {
                    m.insert(flag, val);
                }
            }
        }
    }
    m
}

fn hp_delta(out: &CommandOutcome) -> Option<i64> {
    out.deltas().iter().find(|d| d.field == "resources.res-hp").and_then(|d| d.value.as_i64())
}

fn lua_damage_amount(out: &CommandOutcome) -> i64 {
    out.requests
        .iter()
        .filter_map(|r| match r {
            LuaRequest::ApplyEffect { effect, .. }
                if effect.get("kind").and_then(Value::as_str) == Some("damage") =>
            {
                effect.get("amount").and_then(Value::as_str).and_then(|s| s.parse::<i64>().ok())
            }
            _ => None,
        })
        .sum()
}

/// T26 的机制判据（本轮按字面独立重写）：引擎自己掷了一次 3d6、按 0.5 缩放，
/// 且引擎 hp delta == -本次掉血（Lua 没另补一份）。
fn engine_scaled_half(flags: &BTreeMap<String, String>, session_delta: i64) -> bool {
    flags.get("r4.missing").map(String::as_str) == Some("false")
        && flags.get("r4.factor").map(String::as_str) == Some("0.5")
        && flags.get("r4.rng").map(String::as_str) == Some("3")
        && flags.get("r4.deltas").map(String::as_str) == Some("1")
        && flags.get("r4.hp").and_then(|v| v.parse::<i64>().ok()) == Some(-session_delta)
}

#[test]
fn r4_a12_mechanism_probe_and_mutations() {
    let sb = lmop();
    let real = mount_source(&sb, "dnd-save-half");
    assert!(real.contains("scale_effect(0.5)"), "交付脚本必须声明 scale_effect(0.5)");
    assert!(!real.contains("engine_rng"), "交付脚本不得自己掷效果骰");

    // 基线：新机制
    let base = run_save_half(&sb, 11, &real, true, Some("inst-pc-lmop-talin"));
    let base_flags = probe_flags(&base.out);
    let base_engine_hp = hp_delta(&base.out).expect("引擎应结算 hp delta");
    let base_delta = base_engine_hp.abs();
    println!(
        "R4-2 基线（引擎缩放）: flags={base_flags:?} dice={} delta={base_delta}",
        base.dice.len()
    );
    assert_eq!(base.dice.len(), 4, "1 颗 d20 + 引擎一次 3d6");
    assert!((1..=9).contains(&base_delta), "半值 1..9");
    assert_eq!(base_flags.get("r4.factor").map(String::as_str), Some("0.5"));
    assert_eq!(base_flags.get("r4.rng").map(String::as_str), Some("3"));
    assert_eq!(base_flags.get("r4.deltas").map(String::as_str), Some("1"));
    assert_eq!(lua_damage_amount(&base.out), 0, "引擎已结算，Lua 不得再补一份");
    assert!(engine_scaled_half(&base_flags, base_delta), "基线必须满足机制判据");

    // 变异 A：换回旧「Lua 重掷」脚本（T26 声称的反例）
    let mut_a = run_save_half(&sb, 11, OLD_REROLL_RULE, true, Some("inst-pc-lmop-talin"));
    let flags_a = probe_flags(&mut_a.out);
    let lua_a = lua_damage_amount(&mut_a.out);
    println!(
        "R4-2 变异A（旧重掷）: dice={} flags={flags_a:?} 引擎hp={:?} Lua伤害={lua_a}",
        mut_a.dice.len(),
        hp_delta(&mut_a.out)
    );
    // 松断言（a12 原先那两条）照样成立 —— 它们对机制无感
    assert_eq!(mut_a.dice.len(), 4, "旧重掷同样消耗 4 颗骰（1×d20 + 重掷 3d6）");
    assert!((1..=9).contains(&lua_a), "旧重掷的半值同样落在 1..9");
    // 机制判据有牙齿：成功分支引擎根本没结算
    assert_eq!(flags_a.get("r4.missing").map(String::as_str), Some("false"), "PostResolve 仍跑，但快照为空");
    assert_eq!(flags_a.get("r4.factor").map(String::as_str), Some("nil"));
    assert_eq!(flags_a.get("r4.rng").map(String::as_str), Some("0"));
    assert_eq!(flags_a.get("r4.deltas").map(String::as_str), Some("0"));
    assert_eq!(flags_a.get("r4.hp").map(String::as_str), Some("none"));
    assert!(!engine_scaled_half(&flags_a, lua_a), "变异 A 必须让机制判据 FAIL");

    // 变异 B（本轮自造）：声明缩放 **并且** Lua 再补一份 → 引擎 hp delta != 会话掉血
    let hybrid = format!("{OLD_REROLL_RULE}\nhost.scale_effect(0.5)\n");
    let mut_b = run_save_half(&sb, 11, &hybrid, true, Some("inst-pc-lmop-talin"));
    let flags_b = probe_flags(&mut_b.out);
    let engine_b = hp_delta(&mut_b.out).expect("混合机制引擎仍结算").abs();
    let lua_b = lua_damage_amount(&mut_b.out);
    println!(
        "R4-2 变异B（缩放+Lua双补）: dice={} flags={flags_b:?} 引擎hp={engine_b} Lua补={lua_b} 会话可见={}",
        mut_b.dice.len(),
        engine_b + lua_b
    );
    assert_eq!(flags_b.get("r4.factor").map(String::as_str), Some("0.5"));
    assert_eq!(flags_b.get("r4.deltas").map(String::as_str), Some("1"));
    assert!(lua_b > 0, "混合机制下 Lua 仍补一份");
    // 会话可见掉血 = 引擎那份 + Lua 补的那份；引擎快照只对得上其中一份 → 判据必须 FAIL
    assert!(
        !engine_scaled_half(&flags_b, engine_b + lua_b),
        "变异 B：hp 对账必须让机制判据 FAIL（引擎快照 != 会话掉血）"
    );
}

// ============================================================
// §R4-3：r2_6 新判据 —— 「分支被跳过」vs「算了被丢弃」的独立确认
// ============================================================

#[test]
fn r4_r26_engine_level_skipped_vs_dropped() {
    let sb = lmop();
    let real = mount_source(&sb, "dnd-save-half");
    let ghost = "r4-不存在的目标";

    // 真实脚本 + 解析不到的目标：成功分支**照样结算**，delta 落在不存在的实体键上
    let computed = run_save_half(&sb, 7, &real, true, Some(ghost));
    let ghost_hp = computed.out.deltas().iter().find(|d| d.field == "resources.res-hp");
    println!(
        "R4-3 真实脚本: 引擎 hp delta={:?} dice={}",
        ghost_hp.map(|d| (d.entity_id.clone(), d.value.clone())),
        computed.dice.len()
    );
    let d = ghost_hp.expect("真实脚本必须算出一条伤害 delta（这就是「算了」）");
    assert_eq!(d.entity_id, ghost, "那条 delta 落在不存在的目标上");
    assert!(num(&d.value) < 0, "是伤害（负数）");
    assert_eq!(computed.dice.len(), 4, "引擎仍掷了效果骰");

    // 变异（删掉缩放声明）：成功分支完全不结算 → 没有 ghost delta（这就是「被跳过」）
    let skipped = run_save_half(&sb, 7, "", true, Some(ghost));
    println!("R4-3 变异(无声明): dice={} deltas={}", skipped.dice.len(), skipped.out.deltas().len());
    assert_eq!(skipped.dice.len(), 1, "没有缩放声明 → 只掷判定骰");
    assert!(
        !skipped.out.deltas().iter().any(|d| d.field == "resources.res-hp"),
        "「被跳过」时不得留下 ghost delta"
    );
}

#[tokio::test]
async fn r4_r26_resolution_state_changes_carry_the_ghost_delta() {
    let sb = lmop();
    let ghost = "r4-不存在的目标";

    // A. 交付物（真实脚本）：投影 delta==0，但 Resolution.state_changes 里有落在 ghost 上的伤害
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R4 r2_6 交付物", sb.clone()).await;
    let session = session_of(&h, &save).await;
    let mut saw = false;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some(ghost.into()),
        }]);
        let before = pc_hp(&session);
        let r = run(&session, "踩到瓦砾").await;
        let c = checks_kind(&session, r).into_iter().next().unwrap();
        if !c.0 {
            continue;
        }
        let delta = before - pc_hp(&session);
        let dice = dice_count(&session, r);
        let changes = resolution_changes(&session, r);
        let ghost_hp: Vec<_> = changes
            .iter()
            .filter(|(e, f, _)| e == ghost && f == "resources.res-hp")
            .collect();
        println!(
            "R4-3 会话级: 投影delta={delta} dice={dice} ghost记录={ghost_hp:?}（state_changes 共 {} 条）",
            changes.len()
        );
        assert_eq!(delta, 0, "落不到实体 → 投影 delta 仍为 0");
        assert_eq!(dice, 4, "但效果骰照掷（分支没被跳过）");
        assert_eq!(ghost_hp.len(), 1, "Resolution.state_changes 必须带那条 ghost delta");
        assert!(num(&ghost_hp[0].2) < 0, "那条记录是伤害");
        assert!(
            !session.projection().characters.contains_key(ghost),
            "被丢弃的实体不得留在投影里（否则不是「静默丢弃」）"
        );
        saw = true;
        break;
    }
    assert!(saw, "40 次内需观察到一次豁免成功");

    // B. 变异（删掉缩放声明）：branch 被跳过 → state_changes 里没有 ghost，骰数 1
    let mut sb2 = sb.clone();
    {
        let mounts = sb2["lua_mounts"].as_array_mut().unwrap();
        let m = mounts.iter_mut().find(|m| m["id"] == "dnd-save-half").unwrap();
        m["source"] = json!("-- r4 变异：删掉缩放声明");
    }
    let h2 = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id2, save2) = publish_and_open(&h2, "R4 r2_6 变异", sb2).await;
    let session2 = session_of(&h2, &save2).await;
    let mut saw2 = false;
    for _ in 0..40 {
        h2.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some(ghost.into()),
        }]);
        let before = pc_hp(&session2);
        let r = run(&session2, "踩到瓦砾").await;
        let c = checks_kind(&session2, r).into_iter().next().unwrap();
        if !c.0 {
            continue;
        }
        let delta = before - pc_hp(&session2);
        let dice = dice_count(&session2, r);
        let changes = resolution_changes(&session2, r);
        let has_ghost = changes.iter().any(|(e, f, _)| e == ghost && f == "resources.res-hp");
        println!("R4-3 变异会话级: 投影delta={delta} dice={dice} ghost存在={has_ghost}");
        assert_eq!(delta, 0);
        assert_eq!(dice, 1, "变异后成功分支不结算，只掷判定骰");
        assert!(!has_ghost, "变异后不得有 ghost delta（否则判据恒真）");
        saw2 = true;
        break;
    }
    assert!(saw2, "40 次内需观察到一次豁免成功（变异）");
}
