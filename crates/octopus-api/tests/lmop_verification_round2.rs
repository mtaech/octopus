//! 第二轮 · LMoP 故事书「引擎级端到端」独立验证（T20 验证者新增）。
//!
//! 与第一轮 `lmop_verification.rs` **相互独立**：本文件自建 harness，不引用前者的任何
//! 非公开辅助函数，也不修改前者的用例；每个用例对应任务清单里的一个验收号（r2_*）。
//!
//! 注入式 AI provider：每回合从脚本队列取一组意图，零网络、零真实模型。
//! 断言写成**不变量**（循环到命中 / 结构断言 / RNG 消耗计数 / 数据卡交叉核对），
//! 因为存档 id 随机 → rng_seed = fnv1a(save_id) → 骰值逐存档不同。
//!
//! 本文件只做验证，不改 `crates/*/src/` 与 `frontend/src/` 的任何产品代码。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use octopus_api::{router, AppState};
use octopus_engine::{
    eval_cond, AiOutput, AiProvider, AssetStore, EngineError, EvalContext, Session, SqliteStore,
    TurnContext,
};
use octopus_types::{
    CheckKind, CheckResultPayload, CondExpr, EnemySpec, EventEnvelope, Intent, PlayEvent,
    RoundChannel, RoundInput, WorldProjection,
};
use serde_json::{json, Value};

// ============================================================
// 注入式 AI provider + 装配（本文件自带，与前一轮文件无耦合）
// ============================================================

#[derive(Default)]
pub struct CueProvider {
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
    let assets_dir = std::env::temp_dir().join(format!("octopus-r2-{}", uuid::Uuid::new_v4()));
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

async fn spawn_with(scripts: Vec<Vec<Intent>>) -> Harness {
    spawn_shared(CueProvider::new(scripts)).await
}

/// 投稿一本故事书并开档（真实 HTTP 路径的引擎级复用）。
async fn publish_and_open(h: &Harness, title: &str, mut draft: Value) -> (String, String) {
    let created: Value = h
        .client
        .post(format!("{}/api/storybooks", h.base))
        .json(&json!({ "title": title }))
        .send().await.unwrap().json().await.unwrap();
    let sb_id = created["id"].as_str().unwrap().to_string();
    draft["meta"]["id"] = json!(sb_id);
    draft["meta"]["title"] = json!(title);
    let saved: Value = h
        .client
        .put(format!("{}/api/storybooks/{sb_id}", h.base))
        .json(&json!({ "draft": draft, "base_version": created["draft_version"] }))
        .send().await.unwrap().json().await.unwrap();
    let res = h
        .client
        .post(format!("{}/api/storybooks/{sb_id}/publish", h.base))
        .json(&json!({ "base_version": saved["doc"]["draft_version"] }))
        .send().await.unwrap();
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
        .send().await.unwrap().json().await.unwrap();
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

fn checks(session: &Session, round: u32) -> Vec<CheckResultPayload> {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::CheckResult(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn dice_count(session: &Session, round: u32) -> usize {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => {
                serde_json::from_str::<Vec<u64>>(&p.text).ok().map(|v| v.len())
            }
            _ => None,
        })
        .sum()
}

fn lmop() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../story_example/lmop-storybook.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn enc_of(proj: &WorldProjection, idx: usize) -> Value {
    serde_json::to_value(&proj.encounters[idx]).unwrap()
}

fn num(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(i64::MIN)
}

fn pc_hp(proj: &WorldProjection) -> i64 {
    num(&proj.characters["inst-pc-lmop-talin"]["resources"]["res-hp"])
}

fn pc_xp(proj: &WorldProjection) -> i64 {
    num(&proj.characters["inst-pc-lmop-talin"]["resources"]["res-xp"])
}

/// 当前所有遭遇里**还活着**的敌人：(instance_id, template_id)。
fn live_enemies(session: &Session) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in &session.projection().encounters {
        let ev = serde_json::to_value(e).unwrap();
        for en in ev["enemies"].as_array().unwrap() {
            if en["hp"].as_i64().unwrap_or(0) > 0 {
                out.push((
                    en["instance_id"].as_str().unwrap().to_string(),
                    en["template_id"].as_str().unwrap_or("").to_string(),
                ));
            }
        }
    }
    out
}

fn xp_of_template(sb: &Value, template_id: &str) -> i64 {
    sb["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == template_id)
        .and_then(|c| c["statblock"]["xp"].as_i64())
        .unwrap_or(i64::MIN)
}

/// 数据卡（按模板 id）的 AC：10 + dex_mod + 挂接护甲修正之和（从故事书自己算，不信引擎给的数）。
fn ac_of_template(sb: &Value, template_id: &str) -> i64 {
    let c = sb["characters"].as_array().unwrap().iter().find(|c| c["id"] == template_id).unwrap();
    let dex = c["attributes"]["dex"].as_i64().unwrap();
    let mut ac = 10 + (dex - 10).div_euclid(2);
    if let Some(kinds) = c["attachments"].as_object() {
        for ids in kinds.values() {
            for id in ids.as_array().unwrap() {
                if let Some(def) = sb["definitions"].as_array().unwrap().iter().find(|d| d["id"] == *id) {
                    for m in def["modifiers"].as_array().cloned().unwrap_or_default() {
                        if m["target"] == "ac" {
                            ac += m["value"].as_i64().unwrap_or(0);
                        }
                    }
                }
            }
        }
    }
    ac
}

// ============================================================
// 验收 1 / 2 / 7：建书 + 发布 + 开档；投影不含 monster；投影无 maps
// ============================================================
#[tokio::test]
async fn r2_1_2_7_publish_open_projection_shape() {
    let sb = lmop();
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 投影形状", sb.clone()).await;
    let session = session_of(&h, &save).await;
    let proj = session.projection();

    let kinds: BTreeMap<String, usize> = proj.characters.values().fold(BTreeMap::new(), |mut acc, c| {
        *acc.entry(c["kind"].as_str().unwrap_or("?").to_string()).or_insert(0) += 1;
        acc
    });
    assert_eq!(kinds.get("monster"), None, "投影不得含 monster 初始实例：{kinds:?}");
    assert_eq!(kinds.get("pc"), Some(&1), "投影应恰好含 1 个 PC：{kinds:?}");
    assert_eq!(proj.controlled, vec!["inst-pc-lmop-talin".to_string()]);

    // 验收 7：投影**是否**含 maps 字段（如实断言，不替设计圆场）
    let proj_json = serde_json::to_value(&proj).unwrap();
    let keys: Vec<String> = proj_json.as_object().unwrap().keys().cloned().collect();
    println!("R2 投影键 = {keys:?}");
    assert!(proj_json.get("maps").is_none(), "本轮实测投影**仍无** maps 字段（设计上地图只在故事书层）：{keys:?}");
    assert_eq!(proj.locations.len(), sb["world"]["locations"].as_array().unwrap().len());
    assert_eq!(proj.characters.len(), 1, "图鉴 31 条 monster 一条都不进实例表");
    println!(
        "R2-1/2/7 PASS: kinds={kinds:?} keys={keys:?} locations={} encounters={}",
        proj.locations.len(),
        proj.encounters.len()
    );
}

// ============================================================
// 验收 3：遭遇克隆灰烬丧尸 —— AC 8 / HP 22 / 六维 13,6,16,3,6,5
// ============================================================
#[tokio::test]
async fn r2_3_encounter_clone_matches_datacard() {
    let sb = lmop();
    let scripts = vec![vec![Intent::Encounter {
        name: "训练靶".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some("mon-ash-zombie".into()),
            count: Some(2),
            skill_id: None,
        }],
        note: None,
    }]];
    let h = spawn_with(scripts).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 遭遇克隆", sb.clone()).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;

    let proj = session.projection();
    assert_eq!(proj.encounters.len(), 1, "应建出 1 场遭遇");
    let enc = enc_of(&proj, 0);
    let arr = enc["enemies"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "count=2 应展开两只");
    let tpl = sb["characters"].as_array().unwrap().iter().find(|c| c["id"] == "mon-ash-zombie").unwrap();
    assert_eq!(tpl["resources"]["res-hp"].as_i64().unwrap(), 22);
    assert_eq!(ac_of_template(&sb, "mon-ash-zombie"), 8, "数据卡 AC 自算应为 8");
    for en in arr {
        assert_eq!(en["hp"].as_i64().unwrap(), 22, "HP 必须来自数据卡");
        assert_eq!(en["max"].as_i64().unwrap(), 22);
        assert_eq!(en["ac"].as_i64().unwrap(), 8, "AC 必须来自数据卡");
        let key = en["instance_id"].as_str().unwrap();
        let inst = &proj.characters[key];
        assert_eq!(inst["kind"], "monster");
        assert_eq!(inst["template_id"], "mon-ash-zombie");
        assert_eq!(
            inst["attributes"],
            json!({"str":13,"dex":6,"con":16,"int":3,"wis":6,"cha":5}),
            "六维必须与附录 B 逐字一致"
        );
    }
    assert_ne!(arr[0]["instance_id"], arr[1]["instance_id"]);
    println!("R2-3 PASS: enc={enc}");
}

// ============================================================
// 验收 4 + 8：玩家攻击（属性来自技能声明）+ 资源保持 JSON 整数
// ============================================================
#[tokio::test]
async fn r2_4_and_8_player_strike_attribute_damage_and_integer_resources() {
    let sb = lmop();
    let declared = sb["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "sk-lmop-shortsword")
        .unwrap()["attribute"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(declared, "dex", "短剑数据卡声明 dex");

    let mut scripts = vec![vec![Intent::Encounter {
        name: "训练靶".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some("mon-ash-zombie".into()),
            count: Some(1),
            skill_id: None,
        }],
        note: None,
    }]];
    for _ in 0..40 {
        scripts.push(vec![Intent::Strike {
            enemy_id: "e1".into(),
            skill_id: Some("sk-lmop-shortsword".into()),
        }]);
    }
    let h = spawn_with(scripts).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 玩家攻击", sb).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;

    let mut hit_round = None;
    let mut seen: Option<CheckResultPayload> = None;
    for _ in 0..40 {
        let r = run(&session, "我砍").await;
        if let Some(c) = checks(&session, r).into_iter().next() {
            assert_eq!(c.kind, Some(CheckKind::Attack));
            assert_eq!(c.attribute, declared, "判定属性必须==技能声明（{declared}）");
            assert_ne!(c.attribute, "str", "短剑是 dex 武器：硬编码力量会在这里被抓到");
            assert!(c.expr.is_some(), "CheckResult 必须带掷骰表达式");
            assert!(c.rolls.as_ref().is_some_and(|v| !v.is_empty()));
            seen = Some(c);
        }
        if enc_of(&session.projection(), 0)["enemies"][0]["hp"].as_i64().unwrap() < 22 {
            hit_round = Some(r);
            break;
        }
    }
    let hit_round = hit_round.expect("40 回合内至少命中一次");
    assert!(seen.is_some());

    // 验收 8：伤害扣实例 hp，且资源在投影里仍是 **JSON 整数**（不是 21.0）
    let proj = session.projection();
    let enc = enc_of(&proj, 0);
    let key = enc["enemies"][0]["instance_id"].as_str().unwrap();
    let res = proj.characters[key]["resources"].clone();
    let raw = serde_json::to_string(&res).unwrap();
    assert!(res["res-hp"].as_i64().is_some(), "受伤后 res-hp 必须是 JSON 整数，实际 {raw}");
    let inst_hp = res["res-hp"].as_i64().unwrap();
    assert!(inst_hp < 22, "伤害必须落在实例 res-hp 上（{inst_hp}）");
    assert_eq!(inst_hp, enc["enemies"][0]["hp"].as_i64().unwrap(), "条目 hp 与实例同步");
    assert!(!raw.contains('.'), "资源 JSON 不得出现小数点：{raw}");
    let pc_raw = serde_json::to_string(&proj.characters["inst-pc-lmop-talin"]["resources"]).unwrap();
    assert!(!pc_raw.contains('.'), "PC 资源 JSON 不得出现小数点：{pc_raw}");
    println!("R2-4/8 PASS: hit_round={hit_round} instance_res={raw} pc_res={pc_raw} check={seen:?}");
}

// ============================================================
// 验收 5：enemy_strike 难度 == 玩家派生 AC（从故事书自算）
// ============================================================
#[tokio::test]
async fn r2_5_enemy_strike_targets_player_derived_ac() {
    let sb = lmop();
    let expect_ac = ac_of_template(&sb, "pc-lmop-talin");
    assert_eq!(expect_ac, 14, "皮甲 + dex16 → 10+3+1 = 14");

    let mut scripts = vec![vec![Intent::Encounter {
        name: "伏击".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some("mon-ash-zombie".into()),
            count: Some(1),
            skill_id: None,
        }],
        note: None,
    }]];
    for _ in 0..40 {
        scripts.push(vec![Intent::EnemyStrike { enemy_id: "e1".into(), target_id: None, skill_id: None }]);
    }
    let h = spawn_with(scripts).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 怪物攻击", sb).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;
    let before = pc_hp(&session.projection());
    assert_eq!(before, 24);

    let mut target_seen = None;
    let mut attr_seen = None;
    let mut hit = false;
    for _ in 0..40 {
        let r = run(&session, "怪物反击").await;
        for c in checks(&session, r) {
            assert_eq!(c.kind, Some(CheckKind::Attack));
            target_seen = Some(c.target);
            attr_seen = Some(c.attribute.clone());
        }
        if pc_hp(&session.projection()) < before {
            hit = true;
            break;
        }
    }
    assert_eq!(target_seen, Some(expect_ac), "难度必须是玩家派生 AC（自算 {expect_ac}）");
    assert_eq!(attr_seen.as_deref(), Some("str"), "灰烬丧尸猛击声明 str");
    assert!(hit, "40 回合内至少命中一次");
    // 验收 8 的另一半：PC **被打中之后**，投影里的资源仍必须是 JSON 整数（不是 21.0）
    let pc_res = session.projection().characters["inst-pc-lmop-talin"]["resources"].clone();
    let pc_raw = serde_json::to_string(&pc_res).unwrap();
    assert!(pc_res["res-hp"].as_i64().is_some(), "PC 受伤后 res-hp 必须是 JSON 整数：{pc_raw}");
    assert!(!pc_raw.contains('.'), "PC 受伤后资源 JSON 不得有小数点：{pc_raw}");
    println!(
        "R2-5 PASS: target={target_seen:?} attr={attr_seen:?} pc_hp {before} -> {} pc_resources_raw={pc_raw}",
        pc_hp(&session.projection())
    );
}

// ============================================================
// 验收 6（原未通过①）：自目标（不给 target_id）豁免减半不再静默失效
// ============================================================
//
// 对照设计：
//   · 自目标（target_id = None）→ 引擎把目标回落成施法者并把 host.target 同步为施法者
//     → dnd-save-half 施加 floor(3d6/2) ∈ 1..9；
//   · 显式 target_id = Some("pc-lmop-talin") → 同一条路径，同分布；
//   · 负对照：target_id = Some("不存在的实体") → 目标解析不到 → host.target = nil
//     → 减半分支静默不发 → delta = 0。这正是修复前的**失效形态**。
#[tokio::test]
async fn r2_6_self_target_save_half_is_not_silently_dropped() {
    let sb = lmop();
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 自目标减半", sb).await;
    let session = session_of(&h, &save).await;

    // --- A. 自目标（target_id = None）---
    let mut self_success: Option<(i64, usize)> = None;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill { skill_id: "sk-lmop-rubble-collapse".into(), target_id: None }]);
        let before = pc_hp(&session.projection());
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().expect("豁免应有 CheckResult");
        assert_eq!(c.kind, Some(CheckKind::Save));
        let delta = before - pc_hp(&session.projection());
        if c.result {
            assert_eq!(dice_count(&session, r), 4, "成功：1 颗 d20 + Lua 重掷 3d6 = 4 颗");
            assert!((1..=9).contains(&delta), "自目标豁免成功必须施加减半伤害，实际 {delta}");
            self_success = Some((delta, dice_count(&session, r)));
            break;
        }
    }
    let self_success = self_success.expect("40 次内需观察到一次自目标豁免成功");

    // --- B. 显式目标（同一个角色）：同一条路径 ---
    let mut explicit_success: Option<(i64, usize)> = None;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some("pc-lmop-talin".into()),
        }]);
        let before = pc_hp(&session.projection());
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().unwrap();
        let delta = before - pc_hp(&session.projection());
        if c.result {
            assert_eq!(dice_count(&session, r), 4);
            assert!((1..=9).contains(&delta), "显式目标豁免成功也应减半，实际 {delta}");
            explicit_success = Some((delta, dice_count(&session, r)));
            break;
        }
    }
    let explicit_success = explicit_success.expect("40 次内需观察到一次显式目标豁免成功");

    // --- C. 负对照：目标解析不到 → host.target = nil → 减半静默失效（delta = 0）---
    let mut nil_target_zero = false;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some("r2-不存在的目标".into()),
        }]);
        let before = pc_hp(&session.projection());
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().unwrap();
        let delta = before - pc_hp(&session.projection());
        if c.result {
            assert_eq!(delta, 0, "负对照：host.target 为 nil 时减半分支静默失效（delta 应为 0）");
            nil_target_zero = true;
            break;
        }
    }
    assert!(nil_target_zero, "负对照未观察到豁免成功——无法证明「host.target 为 nil 时减半会静默失效」");
    println!("R2-6 PASS: 自目标成功={self_success:?} 显式目标成功={explicit_success:?} 负对照=delta 0");
}

// ============================================================
// 验收 9 + 12：check_pre_roll 判定签名（可读 + 可区分类别）；优势 keep_high RNG=2
// ============================================================
#[tokio::test]
async fn r2_9_and_12_pre_roll_signature_and_keep_high() {
    let mut sb = lmop();
    // 注入一条**探针挂载点**（只加不改）：把 check_pre_roll 看到的签名写成标记，
    // 直接证明「掷骰前就有 attribute / kind」。
    sb["lua_mounts"].as_array_mut().unwrap().push(json!({
        "id": "r2-probe-signature",
        "mount": "check_pre_roll",
        "source": "local k = host.check_kind\nlocal a = host.check_attribute\nif k == nil then host.set_flag('r2-kind-nil') else host.set_flag('r2-kind-' .. tostring(k)) end\nif a == nil then host.set_flag('r2-attr-nil') else host.set_flag('r2-attr-' .. tostring(a)) end"
    }));

    let h = spawn_with(vec![
        vec![Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None }],
        vec![Intent::Check { attribute: "str".into(), difficulty: None, actor_id: None, opponent_id: None }],
        vec![
            Intent::Status { character_id: "inst-pc-lmop-talin".into(), status_id: "dnd-inspired".into(), remove: false },
            Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None },
        ],
    ])
    .await;
    let (_sb_id, save) = publish_and_open(&h, "R2 判定签名", sb).await;
    let session = session_of(&h, &save).await;

    // 9a：dex 属性检定 → 探针读到 kind=attribute / attribute=dex
    let r1 = run(&session, "敏捷检定").await;
    {
        let proj = session.projection();
        let f = &proj.flags;
        assert_eq!(f.get("r2-kind-nil"), None, "check_pre_roll 不得读到 kind=nil");
        assert_eq!(f.get("r2-attr-nil"), None, "check_pre_roll 不得读到 attribute=nil");
        assert_eq!(f.get("r2-kind-attribute").and_then(Value::as_bool), Some(true), "kind 应为 attribute");
        assert_eq!(f.get("r2-attr-dex").and_then(Value::as_bool), Some(true), "attribute 应为 dex");
    }
    assert_eq!(dice_count(&session, r1), 1, "无优势：单颗骰");
    let c1 = checks(&session, r1).into_iter().next().unwrap();
    assert_eq!(c1.kind, Some(CheckKind::Attribute));
    assert_eq!(c1.attribute, "dex");
    assert_eq!(c1.r#mod, 8, "dex 属性检定应拿到熟练 +5（dex_mod 3 + 5）");

    // 9b：str 属性检定 → 探针的 attribute 变 str；规则包不给加值（按签名区分）
    let r2 = run(&session, "力量检定").await;
    {
        let proj = session.projection();
        assert_eq!(proj.flags.get("r2-attr-str").and_then(Value::as_bool), Some(true));
    }
    let c2 = checks(&session, r2).into_iter().next().unwrap();
    assert_eq!(c2.attribute, "str");
    assert_eq!(c2.r#mod, 0, "str 检定没有熟练定义 → 加值 0（按签名区分，不是无差别 +5）");

    // 12：激励（dnd-inspired）→ keep_high → RNG 消耗 2
    let r3 = run(&session, "有优势的敏捷检定").await;
    assert_eq!(dice_count(&session, r3), 2, "keep_high 必须消耗 2 颗骰");
    let c3 = checks(&session, r3).into_iter().next().unwrap();
    assert_eq!(c3.attribute, "dex");
    assert_eq!(c3.r#mod, 8, "优势不改变熟练加值");
    let statuses = session.projection().characters["inst-pc-lmop-talin"]["statuses"].clone();
    assert!(
        !statuses.as_array().unwrap().iter().any(|s| s["id"] == "dnd-inspired"),
        "激励用掉后必须消失：{statuses}"
    );
    println!("R2-9/12 PASS: c1_mod={} c2_mod={} c3_dice={}", c1.r#mod, c2.r#mod, dice_count(&session, r3));
}

// ============================================================
// 验收 13：熟练加值**数据驱动** —— 只改故事书 definition 的加值，检定总值随之变
// ============================================================
#[tokio::test]
async fn r2_13_proficiency_bonus_is_data_driven() {
    fn scripts() -> Vec<Vec<Intent>> {
        vec![vec![Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None }]]
    }

    // 基线：storybook 原样 → dex 检定 r#mod = dex_mod 3 + 熟练 5 = 8
    let h1 = spawn_with(scripts()).await;
    let (_a, save1) = publish_and_open(&h1, "R2 熟练基线", lmop()).await;
    let s1 = session_of(&h1, &save1).await;
    let r1 = run(&s1, "敏捷检定").await;
    let c1 = checks(&s1, r1).into_iter().next().unwrap();
    assert_eq!(c1.r#mod, 8, "基线：3 + 5 = 8");

    // 变异：**只改**开放内容里那条熟练定义的 bonus（5 → 9），其余逐字不动
    let mut sb = lmop();
    let mut touched = 0;
    for d in sb["definitions"].as_array_mut().unwrap() {
        if d["id"] == "prof-pc-lmop-talin-dex" {
            assert_eq!(d["fields"]["bonus"], "5", "前置：基线 bonus 应为 5");
            d["fields"]["bonus"] = json!("9");
            touched += 1;
        }
    }
    assert_eq!(touched, 1, "应恰好命中 1 条熟练定义");
    let h2 = spawn_with(scripts()).await;
    let (_b, save2) = publish_and_open(&h2, "R2 熟练变异", sb).await;
    let s2 = session_of(&h2, &save2).await;
    let r2 = run(&s2, "敏捷检定").await;
    let c2 = checks(&s2, r2).into_iter().next().unwrap();

    assert_eq!(c2.attribute, "dex");
    assert_eq!(
        c2.r#mod, 12,
        "只把开放内容里的熟练加值 5 改成 9，检定总值必须从 8 变到 12（引擎侧没有任何 +5 常量）"
    );
    assert_ne!(c1.r#mod, c2.r#mod, "数据改了、结果必须变");
    println!("R2-13 PASS: 基线 r#mod={} → 改 bonus 5→9 后 r#mod={}", c1.r#mod, c2.r#mod);
}

// ============================================================
// 验收 11：对抗判定 —— 掷 vs 掷 / 掷 vs 被动，opponent 非空
// ============================================================
#[tokio::test]
async fn r2_11_opposed_check_both_modes() {
    let scripts = vec![
        vec![Intent::Encounter {
            name: "对手".into(),
            enemies: vec![EnemySpec {
                name: "灰烬丧尸甲".into(),
                hp: None,
                ac: None,
                template_id: Some("mon-ash-zombie".into()),
                count: Some(1),
                skill_id: None,
            }],
            note: None,
        }],
        vec![Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: Some("灰烬丧尸甲".into()) }],
        vec![Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: Some("mon-goblin".into()) }],
    ];
    let h = spawn_with(scripts).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 对抗判定", lmop()).await;
    let s = session_of(&h, &save).await;
    run(&s, "开战").await;
    let r_roll = run(&s, "掷 vs 掷").await;
    let r_passive = run(&s, "掷 vs 被动").await;

    let c_roll = checks(&s, r_roll).into_iter().next().expect("掷 vs 掷 应有 CheckResult");
    let c_passive = checks(&s, r_passive).into_iter().next().expect("掷 vs 被动 应有 CheckResult");
    assert!(c_roll.opponent.is_some(), "掷 vs 掷：opponent 必须非空");
    assert_eq!(c_roll.opponent.as_ref().unwrap().id, "mon-ash-zombie");
    assert!(c_passive.opponent.is_some(), "掷 vs 被动：opponent 必须非空");
    assert_eq!(c_passive.opponent.as_ref().unwrap().id, "mon-goblin");
    assert_eq!(dice_count(&s, r_roll), 2, "掷 vs 掷消耗 2 颗骰（主动 + 对手）");
    assert_eq!(dice_count(&s, r_passive), 1, "掷 vs 被动只掷主动方 1 颗骰");
    println!("R2-11 PASS: roll={c_roll:?} passive={c_passive:?}");
}

// ============================================================
// 验收 10：触发点预置遭遇 + encounter_cleared 条件
// ============================================================
#[tokio::test]
async fn r2_10_trigger_preset_encounter_and_encounter_cleared() {
    let sb = lmop();
    // 10a：无遭遇时 encounter_cleared 为假
    {
        let flags: BTreeMap<String, Value> = BTreeMap::new();
        let goals = serde_json::Map::new();
        let triggers = serde_json::Map::new();
        let attrs = json!({ "str": 10 });
        let rels: Vec<Value> = vec![];
        let encs: Vec<Value> = vec![];
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: attrs.as_object(),
            relationships: &rels,
            scene_id: None,
            encounters: &encs,
            lua: None,
        };
        assert!(!eval_cond(&CondExpr::EncounterCleared {}, &ctx).unwrap(), "无遭遇时应为假");
    }

    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 触发点遭遇", sb.clone()).await;
    let session = session_of(&h, &save).await;
    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-0b7".into(), character_id: None }]);
    run(&session, "去三猪小径").await;

    // 10b：触发点 fired → 引擎自动建遭遇，EncounterView 带锚点
    let mut enc: Option<Value> = None;
    for _ in 0..400 {
        h.provider.push(vec![Intent::Narrate { content: "野外行进".into(), actor_id: None }]);
        run(&session, "行进").await;
        let proj = session.projection();
        if !proj.encounters.is_empty() {
            enc = Some(enc_of(&proj, 0));
            break;
        }
    }
    let enc = enc.expect("400 回合内掷表必须建出遭遇");
    let proj = session.projection();
    assert_eq!(enc["scene_id"], proj.scene_id, "必须带 scene_id");
    assert_eq!(enc["location_id"], "loc-lmop-0b7", "地点来自触发点 encounter.location_id");
    assert!(!enc["template_ids"].as_array().unwrap().is_empty(), "必须带 template_ids");
    // 与故事书里该触发点的**预置**逐条对齐（不信引擎的摘要）
    let preset = sb["skeleton"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|ch| ch["scenes"].as_array().cloned().unwrap_or_default())
        .flat_map(|sc| sc["triggers"].as_array().cloned().unwrap_or_default())
        .find(|t| t["encounter"]["name"] == enc["name"])
        .expect("遭遇名必须能在故事书触发点里找到");
    let mut want: Vec<String> = preset["encounter"]["enemies"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["template_id"].as_str().map(str::to_string))
        .collect();
    let mut got: Vec<String> = enc["template_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    want.sort();
    got.sort();
    assert_eq!(got, want, "EncounterView.template_ids 必须等于触发点预置的模板集合");
    assert!(
        proj.progress.triggers.keys().any(|k| k.starts_with("tr-lmop-wander-")),
        "掷表触发点必须 fired：{:?}",
        proj.progress.triggers.keys().collect::<Vec<_>>()
    );

    // 10c：清空遭遇 → encounter_cleared 为真。
    // 先把 PC 移出 loc-lmop-0b7：否则掷表（turn_end）会在清剿期间不断建新遭遇，
    // 永远清不空——这本身正是 R2-14 要证的「可反复」。这里只测 encounter_cleared。
    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-040".into(), character_id: None }]);
    run(&session, "离开三猪小径").await;
    let mut guard = 0;
    while live_enemies(&session).iter().count() > 0 {
        guard += 1;
        assert!(guard < 300, "清剿超过 300 回合");
        let ids: Vec<String> = live_enemies(&session).into_iter().map(|(i, _)| i).collect();
        h.provider.push(
            ids.into_iter()
                .map(|i| Intent::Strike { enemy_id: i, skill_id: Some("sk-lmop-shortsword".into()) })
                .collect(),
        );
        run(&session, "清剿").await;
    }
    let proj = session.projection();
    assert!(proj.encounters.iter().all(|e| !e.active || e.enemies.iter().all(|x| x.hp <= 0)));
    let flags: BTreeMap<String, Value> = proj.flags.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let goals = proj.progress.goals.clone();
    let triggers = proj.progress.triggers.clone();
    let encs: Vec<Value> = proj.encounters.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
    let rels: Vec<Value> = vec![];
    let ctx = EvalContext {
        flags: &flags,
        goals: &goals,
        triggers: &triggers,
        actor_location: Some("loc-lmop-0b7"),
        actor_attributes: proj.characters["inst-pc-lmop-talin"]["attributes"].as_object(),
        relationships: &rels,
        scene_id: Some(proj.scene_id.as_str()),
        encounters: &encs,
        lua: None,
    };
    assert!(eval_cond(&CondExpr::EncounterCleared {}, &ctx).unwrap(), "清空后应为真");
    println!("R2-10 PASS: enc={enc} cleared=true");
}

// ============================================================
// 验收 14：掷表遭遇可反复 —— 同一表项触发 ≥ 2 次（GAP-N + GAP-H 关键证据）
// ============================================================
#[tokio::test]
async fn r2_14_wander_table_item_refires() {
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 掷表可反复", lmop()).await;
    let session = session_of(&h, &save).await;
    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-0b7".into(), character_id: None }]);
    run(&session, "去三猪小径").await;

    // 每个掷表触发点的 **active 上升沿** 计数：清零后再置位才算「又出一次」。
    let mut edges: BTreeMap<String, u32> = BTreeMap::new();
    let mut prev_active: BTreeMap<String, bool> = BTreeMap::new();
    let mut rounds = 0;
    while rounds < 600 {
        // 先把还活着的敌人一次打完（清场 → encounter_cleared → dnd-wander-reset 清行标记）
        let live = live_enemies(&session);
        let intents: Vec<Intent> = if live.is_empty() {
            vec![Intent::Narrate { content: "野外行进".into(), actor_id: None }]
        } else {
            live.into_iter()
                .map(|(id, _)| Intent::Strike { enemy_id: id, skill_id: Some("sk-lmop-shortsword".into()) })
                .collect()
        };
        h.provider.push(intents);
        run(&session, "行进").await;
        rounds += 1;

        let proj = session.projection();
        for (k, v) in proj.progress.triggers.iter() {
            if !k.starts_with("tr-lmop-wander-") {
                continue;
            }
            let active = v.get("active").and_then(Value::as_bool).or_else(|| v.as_bool()).unwrap_or(false);
            if active && !prev_active.get(k).copied().unwrap_or(false) {
                *edges.entry(k.clone()).or_insert(0) += 1;
            }
            prev_active.insert(k.clone(), active);
        }
        if edges.values().any(|c| *c >= 2) {
            break;
        }
    }
    let refired: Vec<(String, u32)> = edges.iter().filter(|(_, c)| **c >= 2).map(|(k, c)| (k.clone(), *c)).collect();
    println!("R2-14 上升沿统计（{rounds} 回合）：{edges:?}");
    assert!(
        !refired.is_empty(),
        "600 回合内同一掷表表项必须能触发 ≥ 2 次（GAP-N repeatable + GAP-H clear_flag 复位）；实际 {edges:?}"
    );
    let still_true: Vec<String> = session
        .projection()
        .flags
        .iter()
        .filter(|(k, v)| k.starts_with("dnd-wander-") && v.as_bool() == Some(true))
        .map(|(k, _)| k.clone())
        .collect();
    println!("R2-14 PASS: 反复触发={refired:?}；当前仍置位的行标记={still_true:?}（{rounds} 回合）");
}

// ============================================================
// 验收 15：XP 由 enemy_defeated 发 —— **不在任何掷表行里**的模板也能拿 XP（GAP-F 关键证据）
// ============================================================
#[tokio::test]
async fn r2_15_improvised_encounter_awards_xp_by_template() {
    let sb = lmop();
    let tpl = "mon-skeleton";
    let in_tables: Vec<String> = sb["skeleton"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|ch| ch["scenes"].as_array().cloned().unwrap_or_default())
        .flat_map(|sc| sc["triggers"].as_array().cloned().unwrap_or_default())
        .flat_map(|t| t["encounter"]["enemies"].as_array().cloned().unwrap_or_default())
        .filter_map(|e| e["template_id"].as_str().map(str::to_string))
        .collect();
    assert!(!in_tables.contains(&tpl.to_string()), "{tpl} 不得出现在任何掷表行里（否则本用例没有意义）");
    let want_xp = xp_of_template(&sb, tpl);
    assert!(want_xp > 0);

    let mut scripts = vec![vec![Intent::Encounter {
        name: "即兴遭遇（导演现编，非掷表）".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some(tpl.into()),
            count: Some(1),
            skill_id: None,
        }],
        note: Some("r2 即兴遭遇".into()),
    }]];
    for _ in 0..60 {
        scripts.push(vec![Intent::Strike { enemy_id: "e1".into(), skill_id: Some("sk-lmop-shortsword".into()) }]);
    }
    let h = spawn_with(scripts).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 即兴遭遇 XP", sb).await;
    let session = session_of(&h, &save).await;

    let xp_before = pc_xp(&session.projection());
    run(&session, "开战").await;
    assert!(!session.projection().encounters.is_empty(), "即兴遭遇必须建成");

    let mut awarded = false;
    for _ in 0..60 {
        run(&session, "我砍").await;
        let after = pc_xp(&session.projection());
        if after != xp_before {
            assert_eq!(after - xp_before, want_xp, "即兴遭遇也必须按数据卡发 XP（{tpl}）");
            awarded = true;
            break;
        }
    }
    assert!(awarded, "即兴遭遇（非掷表）的敌人被击败后必须发 XP（GAP-F 闭合）");

    let proj = session.projection();
    let wander_flags: Vec<String> = proj
        .flags
        .iter()
        .filter(|(k, v)| k.starts_with("dnd-wander-") && v.as_bool() == Some(true))
        .map(|(k, _)| k.clone())
        .collect();
    assert!(wander_flags.is_empty(), "即兴遭遇不得借道掷表：{wander_flags:?}");
    assert!(
        !proj.progress.triggers.keys().any(|k| k.starts_with("tr-lmop-wander-")),
        "即兴遭遇不得靠掷表触发点"
    );
    println!("R2-15 PASS: {tpl} 即兴遭遇 XP {xp_before} -> {}（期望 +{want_xp}）", pc_xp(&proj));
}

// ============================================================
// 验收 16：38 → 16 的合并没有丢行为 —— 31 条模板的 XP 定义与 statblock.xp 逐条相等
// ============================================================
#[tokio::test]
async fn r2_16_every_template_xp_definition_equals_statblock() {
    let sb = lmop();
    let monsters: Vec<Value> = sb["characters"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "monster")
        .cloned()
        .collect();
    assert_eq!(monsters.len(), 31, "图鉴应 31 条 monster 模板");

    let defs: Vec<Value> = sb["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["kind"] == "dnd-xp-award")
        .cloned()
        .collect();
    let per_tpl: Vec<Value> =
        defs.iter().filter(|d| d["fields"]["template_id"].is_string()).cloned().collect();

    let mut missing = Vec::new();
    let mut wrong = Vec::new();
    for m in &monsters {
        let id = m["id"].as_str().unwrap();
        let hits: Vec<&Value> = per_tpl.iter().filter(|d| d["fields"]["template_id"] == id).collect();
        if hits.len() != 1 {
            missing.push(format!("{id}: {} 条定义", hits.len()));
            continue;
        }
        let f = &hits[0]["fields"];
        let want = m["statblock"]["xp"].as_i64().unwrap();
        if f["xp"].as_str().unwrap().parse::<i64>().ok() != Some(want) {
            wrong.push(format!("{id}: 定义 {} vs 图鉴 {want}", f["xp"]));
        }
        if f["creature_name"].as_str().unwrap() != m["name"].as_str().unwrap() {
            wrong.push(format!("{id}: 名称 {} vs {}", f["creature_name"], m["name"]));
        }
    }
    let extra: Vec<String> = per_tpl
        .iter()
        .map(|d| d["fields"]["template_id"].as_str().unwrap().to_string())
        .filter(|id| !monsters.iter().any(|m| m["id"] == *id))
        .collect();

    assert!(missing.is_empty(), "缺 XP 定义：{missing:?}");
    assert!(wrong.is_empty(), "XP 与图鉴不符：{wrong:?}");
    assert!(extra.is_empty(), "多余的 XP 定义：{extra:?}");
    assert_eq!(per_tpl.len(), 31, "逐模板 XP 定义应恰好 31 条（+1 条汇总表 = 32）");
    assert_eq!(defs.len(), 32, "dnd-xp-award 定义应为 31 + 1 张汇总表");

    let summary = defs.iter().find(|d| d["id"] == "xp-lmop-table").expect("应有汇总表定义");
    let table = summary["fields"]["table"].as_str().unwrap();
    let mut parsed = 0;
    let mut bad: Vec<String> = Vec::new();
    for seg in table.split('；') {
        let Some((name, val)) = seg.split_once('=') else {
            bad.push(format!("无法解析：{seg}"));
            continue;
        };
        let val: i64 = match val.trim().parse() {
            Ok(v) => v,
            Err(_) => {
                bad.push(format!("数值非法：{seg}"));
                continue;
            }
        };
        match monsters.iter().find(|m| m["name"] == name) {
            Some(m) if m["statblock"]["xp"].as_i64() == Some(val) => parsed += 1,
            Some(m) => bad.push(format!("{name}: 汇总 {val} vs 图鉴 {}", m["statblock"]["xp"])),
            None => bad.push(format!("汇总表出现未知模板名：{name}")),
        }
    }
    assert!(bad.is_empty(), "汇总表与图鉴不一致：{bad:?}");
    assert_eq!(parsed, 31, "汇总表应覆盖全部 31 条模板");
    println!("R2-16 PASS: 31 条模板 XP 定义 + 汇总表全部与图鉴 statblock.xp 相等；defs={}", defs.len());
}

// ============================================================
// 变异审计：T19 改后的 a9_a14 XP 断言是否**真有牙齿**（不是恒真）
// ============================================================
//
// a9_a14 的三条新断言：① 每杀一只 → XP 增量恰好 == 该模板数据卡 XP；
// ② 整场总增量 == 遭遇内每只敌人的数据卡 XP 之和；③ 清空后**不再补发**（增量 0）。
// 这里用三种实现形态做**变异对照**（同一套断言口径，只改实现/内容）：
//   A 基线（当前实现）：逐只发，清空后 0  → 断言应通过；
//   B 删除 dnd-xp-award 挂载点（=「敌人倒下了却不发 XP」）→ 断言① 必然 FAIL；
//   C 加回旧的「按 encounter_cleared 回补 XP」turn_end 规则 → 断言③ 必然 FAIL。
async fn xp_mutation_probe(mutate: impl Fn(&mut Value)) -> (i64, i64, i64) {
    let mut sb = lmop();
    mutate(&mut sb);
    let tpl = "mon-skeleton";
    let want = xp_of_template(&sb, tpl);
    let mut scripts = vec![vec![Intent::Encounter {
        name: "变异对照遭遇".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some(tpl.into()),
            count: Some(1),
            skill_id: None,
        }],
        note: None,
    }]];
    for _ in 0..60 {
        scripts.push(vec![Intent::Strike { enemy_id: "e1".into(), skill_id: Some("sk-lmop-shortsword".into()) }]);
    }
    let h = spawn_with(scripts).await;
    let (_sb_id, save) = publish_and_open(&h, "R2 变异 XP 对照", sb).await;
    let session = session_of(&h, &save).await;
    let xp0 = pc_xp(&session.projection());
    run(&session, "开战").await;

    let mut kill_delta = 0;
    for _ in 0..60 {
        run(&session, "我砍").await;
        let d = pc_xp(&session.projection()) - xp0;
        if d != 0 {
            kill_delta = d;
            break;
        }
    }
    let before_post = pc_xp(&session.projection());
    h.provider.push(vec![Intent::Narrate { content: "结算经验".into(), actor_id: None }]);
    run(&session, "结算经验").await;
    let post = pc_xp(&session.projection()) - before_post;
    (want, kill_delta, post)
}

#[tokio::test]
async fn r2_mut1_a9_xp_assertions_have_teeth() {
    // A 基线
    let (want, kill_a, post_a) = xp_mutation_probe(|_| {}).await;
    assert_eq!(kill_a, want, "基线：逐只发，kill_delta 应 == 数据卡 XP");
    assert_eq!(post_a, 0, "基线：清空后不再补发");

    // B 删除 XP 挂载点 → 断言①（每杀一只必须发 XP）必然 FAIL
    let (_, kill_b, _) = xp_mutation_probe(|sb| {
        sb["lua_mounts"] = json!(sb["lua_mounts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["id"] != "dnd-xp-award")
            .cloned()
            .collect::<Vec<Value>>());
    })
    .await;
    assert_eq!(
        kill_b, 0,
        "变异 B（删 dnd-xp-award）：敌人被击败也不发 XP → a9_a14 的「逐只发 XP / awarded」断言必然 FAIL"
    );

    // C 加回旧的回补规则 → 断言③（清空后增量 0）必然 FAIL
    let (_, _, post_c) = xp_mutation_probe(|sb| {
        sb["lua_mounts"].as_array_mut().unwrap().push(json!({
            "id": "r2-mut-backfill",
            "mount": "turn_end",
            "when": { "op": "encounter_cleared" },
            "source": "host.modify_resource(host.actor.id, 'res-xp', 50)"
        }));
    })
    .await;
    assert_ne!(
        post_c, 0,
        "变异 C（加回 encounter_cleared 回补）：清空后会再补一笔 → a9_a14 的「清空后增量为 0」断言必然 FAIL"
    );

    println!("R2-MUT1 PASS: 基线 kill={kill_a}/post={post_a}；变异B kill={kill_b}；变异C post={post_c}");
}


