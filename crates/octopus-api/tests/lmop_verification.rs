//! LMoP 故事书「引擎级端到端」验证（验收 2–14）。
//!
//! 注入式 AI provider：每个回合从脚本队列取一组意图，零网络、零真实模型。
//! 除「存档 id → RNG 种子（fnv1a(save_id)）」外全程确定；骰值逐存档不同，因此断言写成
//! **不变量**（循环到命中 / 结构断言 / RNG 消耗计数 / 数据卡交叉核对），而非固定骰面。
//!
//! 受影响的设计文档：docs/check-mechanism-design.md、docs/rules-via-lua.md、
//! docs/bestiary-design.md、docs/map-and-presence-design.md。

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
// 注入式 AI provider + 装配
// ============================================================

#[derive(Default)]
pub struct CueProvider {
    queue: Mutex<VecDeque<Vec<Intent>>>,
}

impl CueProvider {
    fn new(scripts: Vec<Vec<Intent>>) -> Arc<Self> {
        Arc::new(Self { queue: Mutex::new(scripts.into()) })
    }
    /// 测试中途追加一回合的脚本（动态场景用）。
    fn push(&self, intents: Vec<Intent>) {
        self.queue.lock().expect("cue queue poisoned").push_back(intents);
    }
}

#[async_trait]
impl AiProvider for CueProvider {
    async fn story_intents(&self, _ctx: &TurnContext) -> Result<AiOutput, EngineError> {
        let mut q = self.queue.lock().expect("cue queue poisoned");
        let mut intents = q.pop_front().unwrap_or_default();
        // finish_turn 终止回合内续轮：每次 run_round 恰好消费一条脚本。
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
    let assets_dir = std::env::temp_dir().join(format!("octopus-lmop-{}", uuid::Uuid::new_v4()));
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
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = reqwest::Client::builder().default_headers(headers).build().unwrap();
    Harness { app: state, provider, base, client }
}

async fn spawn_with(scripts: Vec<Vec<Intent>>) -> Harness {
    spawn_shared(CueProvider::new(scripts)).await
}

/// 投稿一本已发布的故事书并开档，返回 (storybook_id, save_id)。
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
    let save_id = save["id"].as_str().unwrap().to_string();
    (sb_id, save_id)
}

async fn session_of(h: &Harness, save_id: &str) -> Arc<Session> {
    let s = h.app.session_for(save_id).await.unwrap();
    s.set_auto_confirm(true);
    s
}

async fn run(session: &Session, text: &str) -> u32 {
    let before = session.current_round();
    session
        .run_round(
            RoundInput { channel: RoundChannel::Character, text: text.to_string(), refs: vec![] },
            None,
            vec![],
        )
        .await
        .unwrap();
    before + 1
}

fn rounds_events(session: &Session, round: u32) -> Vec<EventEnvelope> {
    session
        .history(None, 1_000_000)
        .events
        .into_iter()
        .filter(|e| e.round == round)
        .collect()
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

/// 某回合消耗的 RNG 原始取值个数（= 掷了几颗骰）。
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

/// 数值读取：资源 / 属性是整数口径，但历史日志可能留着修复前的 JSON 浮点（如 21.0），
/// 这里统一按数值处理（整数优先，浮点兜底）。
fn num(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(i64::MIN)
}

fn pc_hp(proj: &WorldProjection) -> i64 {
    num(&proj.characters["inst-pc-lmop-talin"]["resources"]["res-hp"])
}

// ============================================================
// 验收 1 / 2：基于 lmop-storybook.json 创建并发布成功；开档投影不含 monster 初始实例
// ============================================================
#[tokio::test]
async fn a1_a2_publish_and_open_has_no_monster_instances() {
    let sb = lmop();
    // 交付物形状
    assert_eq!(sb["characters"].as_array().unwrap().iter().filter(|c| c["kind"] == "monster").count(), 31);
    assert_eq!(sb["characters"].as_array().unwrap().iter().filter(|c| c["kind"] == "pc").count(), 1);
    // 【口径变更（GAP-F / GAP-H / GAP-N 闭合）】原期望 38 = 12 条规则 + 2 条掷表 + **24 条按掷表行回补的 XP**。
    // 旧形态的 24 条 XP 规则挂在 turn_end + when: all_of[行标记, encounter_cleared]，是「没有敌人被击败事件」
    // （GAP-F）逼出来的近似链路。闭合后重排为 16 条 = 12 规则 + 2 掷表 + 1 条掷表复位（event / encounter_cleared
    // → clear_flag，配合 repeatable 的边沿语义，GAP-N + GAP-H）+ 1 条 XP（event / enemy_defeated，逐只发放）。
    // 这里不只数个数：关键挂载点必须**在场且挂在正确的时机**——数量对但挂错地方同样应当失败。
    let mounts = sb["lua_mounts"].as_array().unwrap();
    assert_eq!(
        mounts.len(),
        16,
        "规则包挂载点数量（GAP-F 闭合后 24 条按行回补的 XP 合并为 1 条 event 规则）"
    );
    let mount_of = |id: &str| {
        mounts
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("缺少关键挂载点 {id}"))
    };
    let xp_mount = mount_of("dnd-xp-award");
    assert_eq!(xp_mount["mount"], "event", "XP 必须挂在 event 上（enemy_defeated 逐只发）");
    assert!(
        xp_mount["source"].as_str().unwrap().contains("enemy_defeated"),
        "XP 规则必须按 enemy_defeated 开闸，实际源码：{}",
        xp_mount["source"]
    );
    let reset_mount = mount_of("dnd-wander-reset");
    assert_eq!(
        reset_mount["mount"], "event",
        "掷表标记复位必须挂在 event 上（encounter_cleared 时 clear_flag）"
    );
    assert!(
        reset_mount["source"].as_str().unwrap().contains("clear_flag"),
        "掷表复位规则必须清标记（GAP-H），实际源码：{}",
        reset_mount["source"]
    );
    assert!(
        !mounts.iter().any(|m| {
            let id = m["id"].as_str().unwrap_or("");
            id.starts_with("dnd-xp-day") || id.starts_with("dnd-xp-night")
        }),
        "旧的「按掷表行回补 XP」规则应已全部移除"
    );
    assert_eq!(sb["world"]["locations"].as_array().unwrap().len(), 88);
    assert_eq!(sb["world"]["maps"].as_array().unwrap().len(), 7);
    assert_eq!(sb["skeleton"].as_array().unwrap().len(), 4);

    let h = spawn_with(vec![vec![Intent::Narrate { content: "开场".into(), actor_id: None }]]).await;
    let (sb_id, save_id) = publish_and_open(&h, "A1/A2", sb).await;
    assert!(!sb_id.is_empty() && !save_id.is_empty());
    let session = session_of(&h, &save_id).await;
    let proj = session.projection();
    let monsters = proj.characters.values().filter(|c| c["kind"] == "monster").count();
    assert_eq!(monsters, 0, "图鉴是模板库，开档投影不得含 monster 初始实例");
    assert_eq!(proj.characters.len(), 1, "LMoP 只有 1 个受控 PC");
    assert_eq!(proj.controlled, vec!["inst-pc-lmop-talin".to_string()]);
    assert_eq!(proj.locations.len(), 88);
    println!("A1/A2 PASS: sb={sb_id} save={save_id} chars={:?}", proj.characters.keys().collect::<Vec<_>>());
}

// ============================================================
// 验收 3：遭遇克隆图鉴条目，AC / HP / 六维与数据卡一致（灰烬丧尸）
// ============================================================
#[tokio::test]
async fn a3_encounter_clones_bestiary_and_matches_datacard() {
    let sb = lmop();
    let card = sb["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "mon-ash-zombie")
        .unwrap()
        .clone();
    assert_eq!(card["attributes"], json!({"str":13,"dex":6,"con":16,"int":3,"wis":6,"cha":5}));
    assert_eq!(card["resources"]["res-hp"], 22);

    let h = spawn_with(vec![vec![Intent::Encounter {
        name: "灰烬丧尸遭遇".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some("mon-ash-zombie".into()),
            count: Some(2),
            skill_id: None,
        }],
        note: None,
    }]])
    .await;
    let (_sb, save) = publish_and_open(&h, "A3 灰烬丧尸", sb).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;

    let proj = session.projection();
    assert_eq!(proj.encounters.len(), 1, "应恰好建出一场遭遇");
    let enc = enc_of(&proj, 0);
    assert_eq!(enc["enemies"].as_array().unwrap().len(), 2, "count=2 应展开两只");
    assert_eq!(enc["template_ids"], json!(["mon-ash-zombie"]));
    for (i, e) in enc["enemies"].as_array().unwrap().iter().enumerate() {
        assert_eq!(e["template_id"], "mon-ash-zombie");
        assert_eq!(e["hp"], 22, "第 {i} 只 HP 应等于数据卡 22");
        assert_eq!(e["max"], 22);
        assert_eq!(e["ac"], 8, "第 {i} 只 AC 应等于 10 + dex_mod(-2) = 8");
        let key = e["instance_id"].as_str().unwrap();
        let inst = &proj.characters[key];
        assert_eq!(inst["kind"], "monster", "图鉴怪以 kind=monster 的实例存在");
        assert_eq!(num(&inst["resources"]["res-hp"]), 22);
        assert_eq!(inst["attributes"], card["attributes"]);
    }
    println!("A3 PASS: enc={enc}");
}

// ============================================================
// 验收 4：玩家攻击怪物——属性取技能声明（dex）、伤害扣实例 HP、发 CheckResult
// ============================================================
#[tokio::test]
async fn a4_player_strike_uses_declared_attribute_and_emits_check_result() {
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
    let (_sb, save) = publish_and_open(&h, "A4 玩家攻击", lmop()).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;

    let mut hit_round = None;
    let mut seen: Option<CheckResultPayload> = None;
    for _ in 0..40 {
        let r = run(&session, "我砍").await;
        if let Some(c) = checks(&session, r).into_iter().next() {
            assert_eq!(c.kind, Some(CheckKind::Attack), "攻击判定 kind=attack");
            assert_eq!(c.attribute, "dex", "判定属性必须取技能声明（短剑 dex），不再是硬编码 str");
            assert_eq!(c.expr.as_deref(), Some("1d20"));
            assert!(c.rolls.as_ref().is_some_and(|v| !v.is_empty()), "CheckResult 必须带骰面");
            seen = Some(c);
        }
        let proj = session.projection();
        if enc_of(&proj, 0)["enemies"][0]["hp"].as_i64().unwrap() < 22 {
            hit_round = Some(r);
            break;
        }
    }
    let hit_round = hit_round.expect("40 回合内至少命中一次");
    assert!(seen.is_some(), "必须产生 CheckResult 事件");

    let proj = session.projection();
    let enc = enc_of(&proj, 0);
    let key = enc["enemies"][0]["instance_id"].as_str().unwrap();
    let inst_hp = num(&proj.characters[key]["resources"]["res-hp"]);
    assert!(inst_hp < 22, "伤害必须扣在实例的 res-hp 上（{inst_hp}）");
    assert_eq!(inst_hp, enc["enemies"][0]["hp"].as_i64().unwrap(), "条目与实例 HP 一致");
    println!("A4 PASS: hit_round={hit_round} instance_hp={inst_hp} check={seen:?}");
}

// ============================================================
// 验收 5：怪物攻击玩家（enemy_strike）——难度 = 玩家派生 AC，伤害扣 PC HP
// ============================================================
#[tokio::test]
async fn a5_enemy_strike_difficulty_is_player_derived_ac() {
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
        scripts.push(vec![Intent::EnemyStrike {
            enemy_id: "e1".into(),
            target_id: None,
            skill_id: None,
        }]);
    }
    let h = spawn_with(scripts).await;
    let (_sb, save) = publish_and_open(&h, "A5 怪物攻击", lmop()).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;

    let before = pc_hp(&session.projection());
    assert_eq!(before, 24);

    let mut target_seen = None;
    let mut hit = false;
    for _ in 0..40 {
        let r = run(&session, "怪物反击").await;
        for c in checks(&session, r) {
            assert_eq!(c.kind, Some(CheckKind::Attack));
            assert_eq!(c.attribute, "str", "灰烬丧尸猛击声明 str");
            target_seen = Some(c.target);
        }
        if pc_hp(&session.projection()) < before {
            hit = true;
            break;
        }
    }
    assert_eq!(target_seen, Some(14), "难度必须是玩家派生 AC（10 + dex_mod 3 + 皮甲 1 = 14）");
    assert!(hit, "40 回合内怪物至少命中一次，PC HP 应下降");
    println!(
        "A5 PASS: target={target_seen:?} pc_resources_raw={}",
        session.projection().characters["inst-pc-lmop-talin"]["resources"]
    );
}

// ============================================================
// 验收 6：位置链路——AtLocation 不再恒假；按地点统计的在场人数对 NPC 为真实数字
// ============================================================
fn fixture_storybook() -> Value {
    json!({
        "schema_version": 3,
        "meta": { "id": "", "title": "" },
        "attribute_dimensions": [
            { "key": "str", "label": "力量", "type": "number", "min": 1, "max": 30, "baseline": 10, "modifier_step": 2 },
            { "key": "dex", "label": "敏捷", "type": "number", "min": 1, "max": 30, "baseline": 10, "modifier_step": 2 }
        ],
        "derived": [
            { "key": "dex_mod", "label": "敏捷调整值", "formula": "floor((dex - 10) / 2)", "group": "属性", "signed": true },
            { "key": "ac", "label": "护甲等级", "formula": "10 + dex_mod", "group": "战斗" }
        ],
        "world": {
            "premise": "位置链路验证用的小故事。",
            "check": { "dice": "1d20", "mode": "gte", "attribute": "str" },
            "locations": [ { "id": "loc-a", "name": "甲地" }, { "id": "loc-b", "name": "乙地" } ],
            "maps": [ { "id": "map-1", "name": "示意图", "pins": [ { "location_id": "loc-a", "x": 0.25, "y": 0.5 }, { "location_id": "loc-b", "x": 0.75, "y": 0.5 } ] } ]
        },
        "characters": [
            { "id": "char-pc", "name": "主角", "kind": "pc", "attributes": { "str": 10, "dex": 12 }, "resources": { "hp": 10 }, "location_id": "loc-a" },
            { "id": "npc-b1", "name": "乙地甲", "kind": "npc", "attributes": { "str": 10, "dex": 10 }, "resources": { "hp": 4 }, "location_id": "loc-b" },
            { "id": "npc-b2", "name": "乙地乙", "kind": "npc", "attributes": { "str": 10, "dex": 10 }, "resources": { "hp": 4 }, "location_id": "loc-b" }
        ],
        "skills": [],
        "skeleton": [ {
            "id": "ch1", "title": "第一章",
            "scenes": [ {
                "id": "sc-1", "title": "乙地", "location_id": "loc-b",
                "triggers": [ { "id": "tr-arrive", "title": "抵达乙地", "condition": { "op": "at_location", "location_id": "loc-b" } } ]
            } ]
        } ]
    })
}

#[tokio::test]
async fn a6_location_chain_and_presence_counts() {
    // 6a：eval_cond(AtLocation) 直接求值。
    let flags: BTreeMap<String, Value> = BTreeMap::new();
    let goals = serde_json::Map::new();
    let triggers = serde_json::Map::new();
    let attrs = json!({ "str": 10 });
    let rels: Vec<Value> = vec![];
    let encounters: Vec<Value> = vec![];
    let ctx = EvalContext {
        flags: &flags,
        goals: &goals,
        triggers: &triggers,
        actor_location: Some("loc-b"),
        actor_attributes: attrs.as_object(),
        relationships: &rels,
        scene_id: Some("sc-1"),
        encounters: &encounters,
        lua: None,
    };
    assert!(eval_cond(&CondExpr::AtLocation { location_id: "loc-b".into() }, &ctx).unwrap());
    assert!(!eval_cond(&CondExpr::AtLocation { location_id: "loc-a".into() }, &ctx).unwrap());
    // 验收 9 的一半：无遭遇时 encounter_cleared 为假
    assert!(!eval_cond(&CondExpr::EncounterCleared {}, &ctx).unwrap());

    // 6b：真实路径——NPC 常驻地灌进实例、按地点数人得真实数字、Move + AtLocation 触发点生效。
    let h = spawn_with(vec![
        vec![Intent::Narrate { content: "开场".into(), actor_id: None }],
        vec![Intent::Move { destination_id: "loc-b".into(), character_id: None }],
        vec![Intent::Narrate { content: "推进".into(), actor_id: None }],
    ])
    .await;
    let (_sb, save) = publish_and_open(&h, "A6 位置链路", fixture_storybook()).await;
    let session = session_of(&h, &save).await;

    let proj = session.projection();
    assert_eq!(proj.characters["inst-npc-b1"]["location_id"], "loc-b");
    assert_eq!(proj.characters["inst-npc-b2"]["location_id"], "loc-b");
    assert_eq!(proj.characters["inst-npc-b1"]["present"], true, "NPC 与场景同地点 → 在场");
    let at_b = proj
        .characters
        .values()
        .filter(|c| c["present"] == true && c["location_id"] == "loc-b")
        .count();
    assert_eq!(at_b, 2, "按地点 loc-b 统计的在场人数应为 2（真实数字，不再恒为 0）");
    assert_eq!(proj.locations.len(), 2);

    run(&session, "开场").await;
    assert_eq!(
        session.projection().progress.triggers.get("tr-arrive"),
        None,
        "PC 在 loc-a 时 at_location(loc-b) 触发点不得触发"
    );
    run(&session, "去乙地").await;
    run(&session, "看看").await;
    let proj = session.projection();
    assert_eq!(proj.characters["inst-char-pc"]["location_id"], "loc-b", "Move 必须更新 location_id");
    assert_eq!(
        proj.progress.triggers.get("tr-arrive"),
        Some(&json!(true)),
        "到达 loc-b 后 at_location 触发点必须 fired"
    );
    println!("A6 PASS: at_b={at_b} triggers={:?}", proj.progress.triggers);
}

// ============================================================
// 验收 7：同一模板的两场遭遇 HP 互相隔离
// ============================================================
#[tokio::test]
async fn a7_two_encounters_same_template_have_isolated_hp() {
    let mk = |name: &str| Intent::Encounter {
        name: name.into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some("mon-ash-zombie".into()),
            count: Some(1),
            skill_id: None,
        }],
        note: None,
    };
    let mut scripts = vec![vec![mk("甲"), mk("乙")]];
    for _ in 0..40 {
        scripts.push(vec![Intent::Strike {
            enemy_id: "e1".into(),
            skill_id: Some("sk-lmop-shortsword".into()),
        }]);
    }
    let h = spawn_with(scripts).await;
    let (_sb, save) = publish_and_open(&h, "A7 HP 隔离", lmop()).await;
    let session = session_of(&h, &save).await;
    run(&session, "两场遭遇").await;

    let proj = session.projection();
    assert_eq!(proj.encounters.len(), 2);
    let e0 = enc_of(&proj, 0);
    let e1 = enc_of(&proj, 1);
    let k0 = e0["enemies"][0]["instance_id"].as_str().unwrap().to_string();
    let k1 = e1["enemies"][0]["instance_id"].as_str().unwrap().to_string();
    assert_ne!(k0, k1, "同模板实例键必须不同");

    let mut damaged = false;
    for _ in 0..40 {
        // 直接按实例键指名：避免 enemy_id "e1" 命中第一场之外
        h.provider.push(vec![Intent::Strike {
            enemy_id: k0.clone(),
            skill_id: Some("sk-lmop-shortsword".into()),
        }]);
        run(&session, "打第一场").await;
        if num(&session.projection().characters[&k0]["resources"]["res-hp"]) < 22 {
            damaged = true;
            break;
        }
    }
    assert!(damaged, "第一场应被打中");
    let proj = session.projection();
    assert!(num(&proj.characters[&k0]["resources"]["res-hp"]) < 22);
    assert_eq!(
        num(&proj.characters[&k1]["resources"]["res-hp"]),
        22,
        "第二场同模板实例 HP 必须原封不动"
    );
    println!("A7 PASS: k0_hp={} k1_hp=22", proj.characters[&k0]["resources"]["res-hp"]);
}

// ============================================================
// 验收 10：对抗判定——掷 vs 掷 与 掷 vs 被动
// ============================================================
#[tokio::test]
async fn a10_opposed_check_both_combinations() {
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
        vec![Intent::Check {
            attribute: "dex".into(),
            difficulty: None,
            actor_id: None,
            opponent_id: Some("灰烬丧尸甲".into()),
        }],
        vec![Intent::Check {
            attribute: "dex".into(),
            difficulty: None,
            actor_id: None,
            opponent_id: Some("mon-goblin".into()),
        }],
    ];
    let h = spawn_with(scripts).await;
    let (_sb, save) = publish_and_open(&h, "A10 对抗判定", lmop()).await;
    let s = session_of(&h, &save).await;
    run(&s, "开战").await;
    let r_roll = run(&s, "掷 vs 掷").await;
    let r_passive = run(&s, "掷 vs 被动").await;

    let c_roll = checks(&s, r_roll).into_iter().next().expect("掷 vs 掷 应有 CheckResult");
    let c_passive = checks(&s, r_passive).into_iter().next().expect("掷 vs 被动 应有 CheckResult");
    assert!(c_roll.opponent.is_some(), "掷 vs 掷：opponent 必须非空");
    assert_eq!(c_roll.opponent.as_ref().unwrap().id, "mon-ash-zombie");
    assert_eq!(c_roll.opponent.as_ref().unwrap().name, "灰烬丧尸甲");
    assert!(c_passive.opponent.is_some(), "掷 vs 被动：opponent 必须非空");
    assert_eq!(c_passive.opponent.as_ref().unwrap().id, "mon-goblin");
    assert_eq!(c_passive.opponent.as_ref().unwrap().name, "地精");
    assert_eq!(dice_count(&s, r_roll), 2, "掷 vs 掷消耗 2 颗骰（主动方 + 对手）");
    assert_eq!(dice_count(&s, r_passive), 1, "掷 vs 被动只掷主动方 1 颗骰");
    println!("A10 PASS: roll={c_roll:?} passive={c_passive:?}");
}

// ============================================================
// 验收 11 / 13：优势经 Lua（keep_high 消耗 2 颗骰）；熟练加值经 Lua（+5）
// ============================================================
#[tokio::test]
async fn a11_a13_advantage_and_proficiency_via_lua() {
    let scripts = vec![
        vec![Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None }],
        vec![
            Intent::Status {
                character_id: "inst-pc-lmop-talin".into(),
                status_id: "dnd-inspired".into(),
                remove: false,
            },
            Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None },
        ],
    ];
    let h = spawn_with(scripts).await;
    let (_sb, save) = publish_and_open(&h, "A11 优势", lmop()).await;
    let session = session_of(&h, &save).await;

    let r1 = run(&session, "无优势").await;
    let r2 = run(&session, "有优势").await;

    assert_eq!(dice_count(&session, r1), 1, "无优势：单颗骰");
    assert_eq!(dice_count(&session, r2), 2, "优势：keep_high 必须消耗 2 颗骰（规则包 Lua 生效）");
    let c2 = checks(&session, r2).into_iter().next().unwrap();
    assert_eq!(c2.attribute, "dex");
    let statuses = session.projection().characters["inst-pc-lmop-talin"]["statuses"].clone();
    assert!(
        !statuses.as_array().unwrap().iter().any(|s| s["id"] == "dnd-inspired"),
        "判定用掉后「激励」必须消失，实际 statuses={statuses}"
    );
    // 验收 13：熟练加值 +5（PC 的 dex 属性检定）→ r#mod = dex_mod 3 + 5 = 8
    assert_eq!(c2.r#mod, 8, "检定总值必须包含规则包声明的熟练加值 +5（3 + 5）");
    assert_eq!(c2.total, c2.rolls.as_ref().unwrap()[0] + 8, "total = 骰面 + 熟练加值");
    println!("A11/A13 PASS: r1_dice=1 r2_dice=2 c2={c2:?}");
}

// ============================================================
// 验收 12：豁免成功伤害减半——由**引擎**结算一次、按 scale_effect(0.5) 缩放
// ============================================================
//
// 【机制（T24 复核后订正）】规则包 `dnd-save-half` 在 check_post_roll 只声明
// `host.scale_effect(0.5)`：引擎照常结算一次效果、掷一次 3d6，数值型 delta 向零取整
// 乘 0.5。**Lua 不再重掷同一骰式、也不再 apply_effect 补一份**。
// 旧注释写的「Lua 重掷 3d6 取半」是 T23 **之前**的机制，已过期。
//
// 【为什么这样断言才有机制辨识力】只看「骰数 == 4」与「delta ∈ 1..9」，
// 「引擎缩放自己那份」与「Lua 重掷再补一份」**无法区分**——T24 的 M3 变异
// （换回旧重掷脚本）就整条逃脱。这里在 PostResolve 挂一支探针，回传引擎**实际**
// 算出的 `host.resolved_effects`：
//   · 新机制（引擎缩放）：factor=0.5、rng_consumed=3（引擎自己那次 3d6）、
//     #deltas=1、引擎 hp delta == -本次掉血；
//   · 旧机制（Lua 重掷）：成功分支引擎根本不结算 → factor=nil、rng_consumed=0、
//     #deltas=0 → 判据 FAIL（见文件末尾的变异验证用例）。

/// a12 / a12b 的机制探针：在 PostResolve 读**引擎自己算出的**效果快照
/// （`host.resolved_effects` 的 factor / rng_consumed / deltas），写成标记回传。
/// 每轮全覆盖写，不残留上一轮的值。
fn lmop_with_save_probe() -> Value {
    let mut sb = lmop();
    sb["lua_mounts"].as_array_mut().unwrap().push(json!({
        "id": "a12-probe-resolved-effects",
        "mount": "post_resolve",
        "source": "local e = host.resolved_effects\n\
                   if e == nil then\n\
                     host.set_flag('a12.missing', 'true')\n\
                     host.set_flag('a12.factor', 'nil')\n\
                     host.set_flag('a12.rng', '0')\n\
                     host.set_flag('a12.deltas', '0')\n\
                     host.set_flag('a12.hp', 'none')\n\
                     return\n\
                   end\n\
                   host.set_flag('a12.missing', 'false')\n\
                   host.set_flag('a12.factor', tostring(e.factor))\n\
                   host.set_flag('a12.rng', tostring(e.rng_consumed))\n\
                   host.set_flag('a12.deltas', tostring(#e.deltas))\n\
                   local hp = nil\n\
                   for _, d in ipairs(e.deltas) do\n\
                     if d.field == 'resources.res-hp' then hp = d.value end\n\
                   end\n\
                   if hp ~= nil then host.set_flag('a12.hp', tostring(hp)) else host.set_flag('a12.hp', 'none') end"
    }));
    sb
}

/// 从投影标记里读回 a12 探针的事实。
#[derive(Debug)]
struct SaveProbe {
    factor: Option<String>,
    rng: Option<String>,
    deltas: Option<String>,
    hp: Option<i64>,
    missing: bool,
}

fn save_probe(proj: &WorldProjection) -> SaveProbe {
    let s = |k: &str| proj.flags.get(k).and_then(|v| v.as_str()).map(str::to_string);
    SaveProbe {
        factor: s("a12.factor"),
        rng: s("a12.rng"),
        deltas: s("a12.deltas"),
        hp: s("a12.hp").and_then(|v| v.parse::<i64>().ok()),
        missing: proj.flags.get("a12.missing").and_then(|v| v.as_str()) == Some("true"),
    }
}

impl SaveProbe {
    /// 新机制的辨识判据：豁免成功时，引擎**自己**掷了一次 3d6 并按 0.5 缩放，
    /// 且这次结算就是 HP 变化的唯一来源（引擎 hp delta == -本次掉血，Lua 没有另补一份）。
    fn is_engine_scaled_half(&self, session_delta: i64) -> bool {
        !self.missing
            && self.factor.as_deref() == Some("0.5")
            && self.rng.as_deref() == Some("3")
            && self.deltas.as_deref() == Some("1")
            && self.hp == Some(-session_delta)
    }
}

#[tokio::test]
async fn a12_save_half_via_lua() {
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "A12 豁免减半", lmop_with_save_probe()).await;
    let session = session_of(&h, &save).await;

    let mut success = None;
    let mut failure = None;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some("pc-lmop-talin".into()),
        }]);
        let before = pc_hp(&session.projection());
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().expect("豁免应有 CheckResult");
        assert_eq!(c.kind, Some(CheckKind::Save), "坠落瓦砾是豁免");
        assert_eq!(c.target, 10, "技能声明的 DC 10 经 dnd-skill-dc 对齐到 10");
        let delta = before - pc_hp(&session.projection());
        let dice = dice_count(&session, r);
        let probe = save_probe(&session.projection());
        if c.result {
            assert_eq!(dice, 4, "豁免成功：1 颗 d20 + 引擎 3d6 = 4 颗（引擎只掷一次效果骰）");
            assert!((1..=9).contains(&delta), "成功伤害应为 3d6 的一半（1..9），实际 {delta}");
            // 机制辨识：减半必须来自**引擎结算的那一份**，不是 Lua 重掷后补的一份。
            assert!(
                probe.is_engine_scaled_half(delta),
                "豁免成功必须由引擎结算并按 scale_effect(0.5) 缩放：\
                 期望 factor=0.5 / rng_consumed=3 / #deltas=1 / 引擎 hp delta == -{delta}，实际 {probe:?}"
            );
            success = Some((delta, dice));
        } else {
            assert_eq!(dice, 4, "豁免失败：1 颗 d20 + 引擎 3d6 = 4 颗");
            assert!((3..=18).contains(&delta), "失败伤害应为完整 3d6（3..18），实际 {delta}");
            failure = Some((delta, dice));
        }
        if success.is_some() && failure.is_some() {
            break;
        }
    }
    println!("A12: success={success:?} failure={failure:?}");
    assert!(success.is_some(), "40 次内需观察到一次豁免成功");
    assert!(failure.is_some(), "40 次内需观察到一次豁免失败");
}

/// A12 附加：不给 target_id（自己对自己用）时，引擎把目标回落成施法者本人，
/// 与结算语义一致 → dnd-save-half 让**引擎结算的那一份**减半。
///
/// 历史可追溯：本用例原名 a12b_save_half_self_target_is_a_silent_hole，
/// 当时钉住的是「自目标时 host.target 为 nil → apply_effect 静默不发、delta = 0」这一缺口。
/// 引擎修复（command::execute_skill 把自目标同步进 Lua 上下文）后，改为断言**正确行为**；
/// T24 之后进一步要求判据能分辨机制（与 a12 同一支 PostResolve 探针）。
#[tokio::test]
async fn a12b_save_half_self_target_applies_half_damage() {
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "A12b 自目标", lmop_with_save_probe()).await;
    let session = session_of(&h, &save).await;

    let mut saw_success = false;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: None,
        }]);
        let before = pc_hp(&session.projection());
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().unwrap();
        assert_eq!(c.kind, Some(CheckKind::Save), "坠落瓦砾是豁免");
        let delta = before - pc_hp(&session.projection());
        let dice = dice_count(&session, r);
        let probe = save_probe(&session.projection());
        if c.result {
            saw_success = true;
            println!("A12b: 自目标豁免成功 delta={delta} dice={dice}（应为 3d6 的一半 1..9）");
            assert_eq!(dice, 4, "豁免成功：1 颗 d20 + 引擎 3d6 = 4 颗");
            assert!((1..=9).contains(&delta), "自目标豁免成功必须施加减半伤害，实际 {delta}");
            assert!(
                probe.is_engine_scaled_half(delta),
                "自目标豁免成功同样必须来自引擎结算（factor=0.5 / rng_consumed=3 / #deltas=1），实际 {probe:?}"
            );
            break;
        }
    }
    assert!(saw_success, "40 次内需观察到一次豁免成功");
}

/// T23 **之前**的旧脚本（git HEAD 原文照抄）：Lua 自己按骰式重掷一次再取半、
/// 用 apply_effect 补一份伤害。a12 的变异验证把它换回规则包。
const OLD_REROLL_RULE: &str = r#"-- 旧版（T23 之前）：Lua 按 host.definition 的骰式重掷一次再取半
if host.check_kind == 'save' and host.check_result == true then
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

/// 把规则包里的 dnd-save-half 换回旧「Lua 重掷」脚本（探针保留）。
fn lmop_with_old_reroll_rule() -> Value {
    let mut sb = lmop_with_save_probe();
    let mounts = sb["lua_mounts"].as_array_mut().unwrap();
    let m = mounts
        .iter_mut()
        .find(|m| m["id"] == "dnd-save-half")
        .expect("规则包必须有 dnd-save-half 挂载点");
    m["source"] = json!(OLD_REROLL_RULE);
    sb
}

/// a12 / a12b 断言的**变异验证**：把规则包换回旧的「Lua 重掷 + apply_effect」。
/// 结果必须是：
///   · 旧断言（dice==4 / delta∈1..9）照样 PASS —— 它们对机制无感（T24 的 M3 结论）；
///   · 新判据（PostResolve 探针）FAIL —— 成功分支引擎根本没结算
///     （factor=nil / rng_consumed=0 / #deltas=0）。
/// 这就是「新断言有牙齿」的证据。
#[tokio::test]
async fn a12_mutation_old_reroll_rule_fails_the_mechanism_probe() {
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "A12 变异：旧重掷脚本", lmop_with_old_reroll_rule()).await;
    let session = session_of(&h, &save).await;

    let mut observed = false;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some("pc-lmop-talin".into()),
        }]);
        let before = pc_hp(&session.projection());
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().unwrap();
        let delta = before - pc_hp(&session.projection());
        let dice = dice_count(&session, r);
        if !c.result {
            continue;
        }
        observed = true;
        // 旧断言无感：机制被换成「Lua 重掷」，它们照样成立。
        assert_eq!(dice, 4, "变异后骰数仍是 4（1 颗 d20 + Lua 重掷 3d6）");
        assert!((1..=9).contains(&delta), "变异后半值仍落在 1..9，实际 {delta}");
        // 新判据有牙齿：引擎在成功分支没有结算任何效果。
        let probe = save_probe(&session.projection());
        assert!(
            !probe.is_engine_scaled_half(delta),
            "换回旧重掷脚本后，机制判据必须 FAIL（否则断言没有牙齿）：{probe:?}"
        );
        assert_eq!(probe.factor.as_deref(), Some("nil"), "旧机制没有声明因子：{probe:?}");
        assert_eq!(probe.rng.as_deref(), Some("0"), "旧机制引擎不掷效果骰：{probe:?}");
        assert_eq!(probe.deltas.as_deref(), Some("0"), "旧机制引擎产物为空：{probe:?}");
        assert_eq!(probe.hp, None, "旧机制引擎没有 hp delta：{probe:?}");
        println!(
            "A12 变异验证 PASS: 旧重掷脚本 dice={dice} delta={delta}（旧断言仍 PASS），但探针 factor={:?} rng={:?} deltas={:?} → 新判据 FAIL",
            probe.factor, probe.rng, probe.deltas
        );
        break;
    }
    assert!(observed, "40 次内需观察到一次豁免成功（变异脚本的成功分支）");
}

// ============================================================
// 验收 8：触发点预置遭遇——触发点 fired 后引擎自动建遭遇
// EncounterView 带 scene_id / location_id / template_ids
// ============================================================
#[tokio::test]
async fn a8_trigger_preset_encounter_spawns() {
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "A8 触发点遭遇", lmop()).await;
    let session = session_of(&h, &save).await;

    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-0b7".into(), character_id: None }]);
    run(&session, "去三猪小径").await;
    assert_eq!(session.projection().characters["inst-pc-lmop-talin"]["location_id"], "loc-lmop-0b7");

    let mut found: Option<Value> = None;
    for _ in 0..400 {
        h.provider.push(vec![Intent::Narrate { content: "野外行进".into(), actor_id: None }]);
        run(&session, "行进").await;
        let proj = session.projection();
        if !proj.encounters.is_empty() {
            found = Some(enc_of(&proj, 0));
            break;
        }
    }
    let enc = found.expect("400 回合内掷表必须建出至少一场遭遇（Lua 掷表 → flag → 触发点 → 预置遭遇）");
    let proj = session.projection();
    assert_eq!(enc["scene_id"], proj.scene_id, "EncounterView 必须带 scene_id");
    assert_eq!(enc["location_id"], "loc-lmop-0b7", "预置遭遇地点来自触发点 encounter.location_id");
    assert!(!enc["template_ids"].as_array().unwrap().is_empty(), "EncounterView 必须带 template_ids");
    // 触发点确实 fired，且 fired 的正是掷表置位的那个 flag
    let fired: Vec<String> = proj.progress.triggers.keys().cloned().collect();
    assert!(
        fired.iter().any(|k| k.starts_with("tr-lmop-wander-")),
        "掷表触发点必须 fired，实际 {fired:?}"
    );
    println!("A8 PASS: enc={enc} fired={fired:?}");
}

// ============================================================
// 验收 9 / 14：encounter_cleared 判据 + XP 逐只发放（数据卡口径）
// ============================================================
// 说明：旧口径的辅助函数 flag_of / expected_xp_for_flag（「按掷表行标记推算整场 XP」）
// 随 GAP-F 闭合一并移除——XP 不再由标记回补，而是由 enemy_defeated 逐只发放。

/// 数据卡 XP（按模板 id）。
fn xp_of_template(sb: &Value, template_id: &str) -> i64 {
    sb["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == template_id)
        .and_then(|c| c["statblock"]["xp"].as_i64())
        .unwrap_or(0)
}

fn xp_of_pc(session: &Session) -> i64 {
    num(&session.projection().characters["inst-pc-lmop-talin"]["resources"]["res-xp"])
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

// ============================================================
// 验收 9 / 14：encounter_cleared 判据 + XP 逐只发放（数据卡口径）
// ============================================================
//
// 【口径变更（GAP-F 闭合）——旧期望为什么过期】
// 原缺口 GAP-F「没有『敌人被击败』事件」把 XP 逼成一条**回补**链路：
//   when: all_of[掷表行标记, encounter_cleared] → 清空遭遇后一次性 modify_resource。
// 于是旧断言是「清空遭遇**之后**的那一回合，XP 增量 == 该掷表行对应的 xp×数量」。
// 这条口径有两个被独立验证报告点名的洞：① 只有掷表链路能回补，**导演即兴建的遭遇拿不到 XP**；
// ② 遭遇里混入非预置敌人（AI 加的、后续回合刷的）时，行标记口径与实际击杀不符。
//
// GAP-F 闭合后：XP 在 **enemy_defeated**（strike 击杀）时按 data.enemy.template_id **逐只**发放，
// 清空遭遇不再补发（增量 = 0）。本用例因此改为断言新口径，并**额外钉死**旧口径的反面：
//   · 每杀一只 → XP 增量恰好等于该模板 statblock.xp（逐只、按数据卡）；
//   · 整场清剿的总增量 == 遭遇内每只敌人的数据卡 XP 之和（合计口径不变）；
//   · 清空之后再跑一回合 → 增量为 0（不再补发，也不会重复发）；
//   · 行标记在 encounter_cleared 时被 clear_flag 复位（GAP-N 的边沿复位，旧口径依赖它保持为真）。
//
// 【这条断言在什么情况下会失败】（仍然有牙齿）
//   1. XP 少发一只、或金额与数据卡不符；
//   2. 补刀（对已倒下的敌人再攻击）重复发 XP；
//   3. 清空后又补发一笔（旧回补链路残留）；
//   4. 敌人倒下了却没发 XP（enemy_defeated 链路断掉）；
//   5. 行标记没有在遭遇结束时复位（GAP-N 会退回「整局只出一次」）。
#[tokio::test]
async fn a9_a14_encounter_cleared_and_xp_award() {
    let sb = lmop();
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "A9/A14 XP", sb.clone()).await;
    let session = session_of(&h, &save).await;

    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-0b7".into(), character_id: None }]);
    run(&session, "去三猪小径").await;

    // 1) 掷表建出第一场遭遇
    let mut found = false;
    for _ in 0..400 {
        h.provider.push(vec![Intent::Narrate { content: "行进".into(), actor_id: None }]);
        run(&session, "行进").await;
        if !session.projection().encounters.is_empty() {
            found = true;
            break;
        }
    }
    assert!(found, "掷表必须建出遭遇");

    // 2) 关掉掷表闸门：走回凡达林（本回合 turn_end 仍可能再置位一行）
    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-040".into(), character_id: None }]);
    run(&session, "离开三猪小径").await;

    // 3) 逐只击杀：每次击杀的 XP 增量必须恰好 == 该敌人模板的数据卡 XP
    let xp_before_clear = xp_of_pc(&session);
    let mut xp_running = xp_before_clear;
    let mut expected_total = 0i64;
    for e in &session.projection().encounters {
        let ev = serde_json::to_value(e).unwrap();
        for en in ev["enemies"].as_array().unwrap() {
            expected_total += xp_of_template(&sb, en["template_id"].as_str().unwrap_or(""));
        }
    }
    assert!(expected_total > 0, "掷表遭遇的模板必须在数据卡里有 XP（否则本用例没有意义）");

    let mut guard = 0;
    while let Some((instance_id, template_id)) = live_enemies(&session).into_iter().next() {
        guard += 1;
        assert!(guard < 200, "清剿超过 200 回合仍未结束（疑似敌人打不死）");
        let expected_kill = xp_of_template(&sb, &template_id);
        let mut awarded = false;
        for _ in 0..60 {
            h.provider.push(vec![Intent::Strike {
                enemy_id: instance_id.clone(),
                skill_id: Some("sk-lmop-shortsword".into()),
            }]);
            run(&session, "攻击").await;
            let after = xp_of_pc(&session);
            if after != xp_running {
                assert_eq!(
                    after - xp_running, expected_kill,
                    "击败 {} 必须恰好发该模板的数据卡 XP（{}）",
                    template_id, expected_kill
                );
                xp_running = after;
                awarded = true;
                break;
            }
            if !live_enemies(&session).iter().any(|(id, _)| id == &instance_id) {
                panic!("{} 已倒下却没有发 XP（enemy_defeated 链路断了）", template_id);
            }
        }
        assert!(awarded, "{} 连续 60 次攻击都没被打倒（或从未发 XP）", template_id);
    }
    assert!(live_enemies(&session).is_empty(), "清剿结束后不应还有活着的敌人");

    // 4) 合计口径不变 + 清空后**不再补发**（旧回补链路已废弃）
    let xp_after_clear = xp_of_pc(&session);
    assert_eq!(
        xp_after_clear - xp_before_clear,
        expected_total,
        "整场清剿的 XP 增量必须等于遭遇内每只敌人的数据卡 XP 之和（逐只发放）"
    );
    h.provider.push(vec![Intent::Narrate { content: "结算经验".into(), actor_id: None }]);
    run(&session, "结算经验").await;
    assert_eq!(
        xp_of_pc(&session) - xp_after_clear,
        0,
        "清空遭遇后不得再补发 XP（新口径：XP 在击倒那一刻到账）"
    );

    // 5) encounter_cleared 判据（清空后为真）——与旧用例一致
    {
        let proj = session.projection();
        let flags: BTreeMap<String, Value> = BTreeMap::new();
        let goals = serde_json::Map::new();
        let triggers = serde_json::Map::new();
        let attrs = json!({});
        let rels: Vec<Value> = vec![];
        let encounters: Vec<Value> =
            proj.encounters.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
        let scene = proj.scene_id.clone();
        let ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: None,
            actor_attributes: attrs.as_object(),
            relationships: &rels,
            scene_id: Some(scene.as_str()),
            encounters: &encounters,
            lua: None,
        };
        assert!(
            eval_cond(&CondExpr::EncounterCleared {}, &ctx).unwrap(),
            "清空后 encounter_cleared 必须为真"
        );
    }

    // 6) 掷表行标记的复位（GAP-N / GAP-H）：遭遇结束后 clear_flag，标记必须落回假，
    //    触发点的 active 也必须是 false——否则同一表项整局只出一次（旧行为）。
    let still_true: Vec<String> = session
        .projection()
        .flags
        .iter()
        .filter(|(k, v)| k.starts_with("dnd-wander-") && v.as_bool() == Some(true))
        .map(|(k, _)| k.clone())
        .collect();
    assert!(
        still_true.is_empty(),
        "遭遇清空后所有掷表行标记都应为 false（实际仍为真：{still_true:?}）"
    );
    let still_active: Vec<String> = session
        .projection()
        .progress
        .triggers
        .iter()
        .filter(|(k, v)| {
            k.starts_with("tr-lmop-wander-") && v.get("active").and_then(Value::as_bool) == Some(true)
        })
        .map(|(k, _)| k.clone())
        .collect();
    assert!(
        still_active.is_empty(),
        "行标记复位后触发点的 active 必须为 false（实际：{still_active:?}）"
    );

    println!(
        "A9/A14 PASS: xp {} -> {}（期望合计 {}，逐只按数据卡）；行标记已复位",
        xp_before_clear, xp_after_clear, expected_total
    );
}

