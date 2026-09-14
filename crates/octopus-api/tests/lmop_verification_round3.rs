//! 第三轮 · LMoP 故事书「引擎级端到端」独立验证（第三轮验证者新增）。
//!
//! 与被测交付物相互独立：本文件自建 harness，不引用前两轮测试文件的任何辅助函数，
//! 也不修改前两轮的用例（审计它们走「读 + 跑 + 变异推理」）。
//!
//! 关注点（T21 / T22 引擎原语 + T23 规则包改动之后）：
//!   · 四件专门审计：save-half 是否真的缩放「引擎那份效果骰」/ 连带修正是否保住行为 /
//!     两处过期注释的断言是否还有牙齿 / 集群战术近似的边界；
//!   · 清单 15–18（GAP-A / GAP-L / GAP-E / 多段效果缩放）；
//!   · 清单 1–7 / 9 / 13 的第一手复核。
//!
//! 注入式 AI provider：零网络、零真实模型。会话级断言写成不变量（循环到命中 / 结构断言 /
//! RNG 消耗与骰面序列 / 数据卡交叉核对），因为 save_id 随机 → rng_seed = fnv1a(save_id)。
//! 引擎级（execute_skill / LuaHost）断言用固定种子，逐位确定。
//!
//! 本文件只做验证，不改 crates/*/src/ 与 frontend/src/ 的任何产品代码。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use octopus_api::{router, AppState};
use octopus_engine::lua_host::{
    CheckModifier, LuaCheckContext, LuaHost, LuaHostContext, LuaMount, LuaRegistry, LuaRequest, MountEnv,
};
use octopus_engine::rng::DeterministicRng;
use octopus_engine::{
    eval_cond, execute_skill, AiOutput, AiProvider, AssetStore, CommandContext, CommandOutcome,
    EngineError, EvalContext, Session, SqliteStore, TurnContext,
};
use octopus_types::{
    CheckKind, CheckResultPayload, CondExpr, EnemySpec, EventEnvelope, Intent, PlayEvent,
    RoundChannel, RoundInput, SkillDef, WorldProjection,
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
    let assets_dir = std::env::temp_dir().join(format!("octopus-r3-{}", uuid::Uuid::new_v4()));
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

fn checks(session: &Session, round: u32) -> Vec<CheckResultPayload> {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::CheckResult(p) => Some(p),
            _ => None,
        })
        .collect()
}

/// 本回合引擎实际消耗的骰面序列（rng_consume 事件原文，逐位可比较）。
fn dice_values(session: &Session, round: u32) -> Vec<u64> {
    rounds_events(session, round)
        .into_iter()
        .filter_map(|e| match e.event {
            PlayEvent::System(p) if p.code.as_deref() == Some("rng_consume") => {
                serde_json::from_str::<Vec<u64>>(&p.text).ok()
            }
            _ => None,
        })
        .flatten()
        .collect()
}

fn num(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(i64::MIN)
}

fn pc_hp(session: &Session) -> i64 {
    let proj = session.projection();
    num(&proj.characters["inst-pc-lmop-talin"]["resources"]["res-hp"])
}

fn enc_of(proj: &WorldProjection, idx: usize) -> Value {
    serde_json::to_value(&proj.encounters[idx]).unwrap()
}

/// 当前所有遭遇里还活着的敌人：(instance_id, template_id)。
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

fn has_status(session: &Session, key: &str, status_id: &str) -> bool {
    let proj = session.projection();
    proj.characters
        .get(key)
        .and_then(|c| c.get("statuses"))
        .and_then(|s| s.as_array())
        .map(|arr| arr.iter().any(|s| s["id"].as_str() == Some(status_id)))
        .unwrap_or(false)
}

// ============================================================
// 引擎级小工具（确定性）
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

/// 故事书模板 → 运行时实例 JSON（补 instance_id / template_id / statuses / present）。
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

struct SaveRun {
    out: CommandOutcome,
    dice: Vec<u64>,
}

/// 用**真实技能 + 真实 / 变异的规则脚本**跑一次 execute_skill（固定种子）。
///
/// pin_success 把豁免结果钉死（骰照掷、结果确定），使断言与种子无关；
/// target_id = None 走「自目标」语义（引擎把 host.target 同步成施法者本人）。
fn run_save_half(
    sb: &Value,
    seed: u64,
    rule: &str,
    pin_success: bool,
    target_id: Option<&str>,
) -> SaveRun {
    let host = host_with(sb, seed);
    let mut registry = LuaRegistry::new();
    registry.register(
        "r3-pin-result",
        LuaMount::CheckPreRoll,
        if pin_success {
            "host.modify_check('force_success')"
        } else {
            "host.modify_check('force_fail')"
        },
    );
    if !rule.is_empty() {
        registry.register("rule:save-half", LuaMount::CheckPostRoll, rule);
    }
    let skill = skill_of(sb, "sk-lmop-rubble-collapse");
    let actor = instance_json(sb, "pc-lmop-talin", "inst-pc-lmop-talin");
    let target = target_id.map(|t| instance_json(sb, "pc-lmop-talin", t));
    let lua_ctx = LuaHostContext {
        script_id: "r3:save-half".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: actor.clone(),
        target_id: target_id.map(|t| t.to_string()),
        target: target.clone(),
        skill: serde_json::to_value(&skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    // RNG 必须与 LuaHost 共享（真实会话就是这样接线的）：否则 host.engine_rng 掷的骰
    // 不在 ctx.rng 的记账里，旧「重掷」机制会被误测成「没掷」。
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

fn hp_delta(out: &CommandOutcome) -> Option<i64> {
    out.deltas()
        .iter()
        .find(|d| d.field == "resources.res-hp")
        .and_then(|d| d.value.as_i64())
}

/// 规则包真实脚本（从交付物里读，不抄）。
fn real_save_half_rule(sb: &Value) -> String {
    mount_source(sb, "dnd-save-half")
}

/// T23 **之前**的旧脚本（git HEAD 原文照抄，用于机制对照 / 变异审计）。
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

// ============================================================
// 清单 1 / 2：真实路径建书 + 发布 + 开档；投影不含 monster 初始实例
// ============================================================
#[tokio::test]
async fn r3_1_2_publish_open_projection_has_no_monster_instance() {
    let sb = lmop();
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (sb_id, save_id) = publish_and_open(&h, "R3 投影形状", sb.clone()).await;
    let session = session_of(&h, &save_id).await;
    let proj = session.projection();

    let kinds: BTreeMap<String, usize> =
        proj.characters.values().fold(BTreeMap::new(), |mut acc, c| {
            *acc.entry(c["kind"].as_str().unwrap_or("?").to_string()).or_insert(0) += 1;
            acc
        });
    assert_eq!(kinds.get("monster"), None, "投影不得含 monster 初始实例：{kinds:?}");
    assert_eq!(kinds.get("pc"), Some(&1), "投影应恰好含 1 个 PC：{kinds:?}");
    assert_eq!(proj.controlled, vec!["inst-pc-lmop-talin".to_string()]);
    let pc_templates: Vec<&str> = sb["characters"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "pc")
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(pc_templates, vec!["pc-lmop-talin"]);
    assert_eq!(proj.characters["inst-pc-lmop-talin"]["template_id"], "pc-lmop-talin");
    assert_eq!(proj.locations.len(), sb["world"]["locations"].as_array().unwrap().len());
    let proj_json = serde_json::to_value(&proj).unwrap();
    println!(
        "R3-1/2 PASS: sb={sb_id} save={save_id} kinds={kinds:?} locations={} has_maps={}",
        proj.locations.len(),
        proj_json.get("maps").is_some()
    );
}

// ============================================================
// 清单 3 / 4 / 7：遭遇克隆 + 玩家攻击 + 资源仍是 JSON 整数
// ============================================================
#[tokio::test]
async fn r3_3_4_7_encounter_clone_strike_and_integer_resources() {
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
    let (_sb, save) = publish_and_open(&h, "R3 遭遇克隆 + 攻击", sb.clone()).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;

    let proj = session.projection();
    let enc = enc_of(&proj, 0);
    assert_eq!(enc["enemies"].as_array().unwrap().len(), 1);
    assert_eq!(enc["enemies"][0]["hp"], 22, "灰烬丧尸 HP 22");
    assert_eq!(enc["enemies"][0]["max"], 22);
    assert_eq!(enc["enemies"][0]["ac"], 8, "灰烬丧尸 AC 8");
    let key = enc["enemies"][0]["instance_id"].as_str().unwrap().to_string();
    assert_eq!(
        proj.characters[&key]["attributes"],
        json!({"str":13,"dex":6,"con":16,"int":3,"wis":6,"cha":5})
    );

    let mut hit = false;
    let mut seen: Option<CheckResultPayload> = None;
    for _ in 0..40 {
        let r = run(&session, "我砍").await;
        if let Some(c) = checks(&session, r).into_iter().next() {
            assert_eq!(c.kind, Some(CheckKind::Attack));
            assert_eq!(c.attribute, declared, "判定属性必须==技能声明 {declared}");
            assert_ne!(c.attribute, "str", "短剑是 dex 武器");
            assert!(c.expr.is_some() && c.rolls.as_ref().is_some_and(|v| !v.is_empty()));
            seen = Some(c);
        }
        let proj = session.projection();
        let cur = num(&proj.characters[&key]["resources"]["res-hp"]);
        if cur < 22 {
            hit = true;
            let raw = serde_json::to_string(&proj.characters[&key]["resources"]).unwrap();
            assert!(proj.characters[&key]["resources"]["res-hp"].as_i64().is_some(), "实例 res-hp 必须是整数：{raw}");
            assert!(!raw.contains('.'), "资源 JSON 不得出现小数点：{raw}");
            assert_eq!(cur, num(&enc_of(&proj, 0)["enemies"][0]["hp"]), "条目 hp 与实例同步");
            break;
        }
    }
    assert!(hit, "40 回合内至少命中一次");
    assert!(seen.is_some());
    println!("R3-3/4/7 PASS: declared={declared} check={:?}", seen.unwrap().attribute);
}

// ============================================================
// 清单 5：enemy_strike 难度 = 玩家派生 AC
// ============================================================
#[tokio::test]
async fn r3_5_enemy_strike_targets_player_derived_ac() {
    let sb = lmop();
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
    let (_sb, save) = publish_and_open(&h, "R3 怪物攻击", sb.clone()).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;
    let before = pc_hp(&session);
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
        if pc_hp(&session) < before {
            hit = true;
            break;
        }
    }
    let pc_tpl = sb["characters"].as_array().unwrap().iter().find(|c| c["id"] == "pc-lmop-talin").unwrap();
    let expect = 10 + (pc_tpl["attributes"]["dex"].as_i64().unwrap() - 10).div_euclid(2) + 1;
    assert_eq!(target_seen, Some(expect), "难度必须是玩家派生 AC（自算 {expect}）");
    assert_eq!(attr_seen.as_deref(), Some("str"), "灰烬丧尸猛击声明 str");
    assert!(hit, "40 回合内至少命中一次");
    println!("R3-5 PASS: target={target_seen:?} attr={attr_seen:?} hp {before}→{}", pc_hp(&session));
}

// ============================================================
// 审计② / 清单 6：save-half 连带修正后两条分支的行为
//   · 成功 = 只减半伤害（不倒地）
//   · 失败 = 满伤 + 倒地（dnd-prone 由开放内容 fields.fail_status 驱动）
// ============================================================
#[tokio::test]
async fn r3_audit2_save_half_branches_preserve_behavior() {
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "R3 减半两分支", lmop()).await;
    let session = session_of(&h, &save).await;

    let mut success = None;
    let mut failure = None;
    for _ in 0..80 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: None,
        }]);
        let before = pc_hp(&session);
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().expect("豁免应有 CheckResult");
        assert_eq!(c.kind, Some(CheckKind::Save));
        assert_eq!(c.target, 10, "技能声明的 DC 10");
        let delta = before - pc_hp(&session);
        let prone = has_status(&session, "inst-pc-lmop-talin", "dnd-prone");
        let dice = dice_values(&session, r).len();
        if c.result {
            assert!((1..=9).contains(&delta), "成功：3d6 的一半（1..9），实际 {delta}");
            assert!(!prone, "成功分支不得倒地（T23 把 effect.status 移走后由 fail_status 只在失败补）");
            success = Some((delta, dice, prone));
        } else {
            assert!((3..=18).contains(&delta), "失败：完整 3d6（3..18），实际 {delta}");
            assert!(prone, "失败分支必须倒地（dnd-prone）");
            failure = Some((delta, dice, prone));
        }
        if success.is_some() && failure.is_some() {
            break;
        }
    }
    assert!(success.is_some(), "80 次内需观察到一次成功");
    assert!(failure.is_some(), "80 次内需观察到一次失败");
    println!("R3-AUDIT2 PASS: 成功(delta,dice,prone)={success:?} 失败={failure:?}");
}

// ============================================================
// 清单 8 / 10 / 11：判定签名 / 对抗判定 / keep_high 消耗 2 颗骰
// ============================================================
#[tokio::test]
async fn r3_8_10_11_signature_opposed_and_keep_high() {
    let mut sb = lmop();
    sb["lua_mounts"].as_array_mut().unwrap().push(json!({
        "id": "r3-probe-signature",
        "mount": "check_pre_roll",
        "source": "local k = host.check_kind\nlocal a = host.check_attribute\nif k == nil then host.set_flag('r3.kind-nil') else host.set_flag('r3.kind-' .. tostring(k)) end\nif a == nil then host.set_flag('r3.attr-nil') else host.set_flag('r3.attr-' .. tostring(a)) end"
    }));
    let h = spawn_with(vec![]).await;
    let (_sb, save) = publish_and_open(&h, "R3 签名 + 对抗", sb).await;
    let session = session_of(&h, &save).await;
    // 对手必须是**世界上的实例**，否则对抗只能退回静态被动值（掷 vs 被动）
    h.provider.push(vec![Intent::Encounter {
        name: "对抗靶".into(),
        enemies: vec![EnemySpec {
            name: String::new(),
            hp: None,
            ac: None,
            template_id: Some("mon-ash-zombie".into()),
            count: Some(1),
            skill_id: None,
        }],
        note: None,
    }]);
    run(&session, "开战").await;

    // 清单 8：掷骰前就有签名
    h.provider.push(vec![Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None }]);
    let r1 = run(&session, "敏捷检定").await;
    let c1 = checks(&session, r1).into_iter().next().unwrap();
    let (mod1, dice1) = (c1.r#mod, dice_values(&session, r1).len());
    {
        let proj = session.projection();
        assert_eq!(proj.flags.get("r3.kind-attribute"), Some(&json!(true)), "check_pre_roll 必须看到 kind=attribute");
        assert_eq!(proj.flags.get("r3.attr-dex"), Some(&json!(true)), "check_pre_roll 必须看到 attribute=dex");
        assert_eq!(proj.flags.get("r3.kind-nil"), None, "签名不得为 nil");
        assert_eq!(proj.flags.get("r3.attr-nil"), None, "属性不得为 nil");
    }

    // 清单 11：给激励 → keep_high → 多消耗一颗骰（2 颗）
    h.provider.push(vec![
        Intent::Status {
            character_id: "inst-pc-lmop-talin".into(),
            status_id: "dnd-inspired".into(),
            remove: false,
        },
        Intent::Check { attribute: "dex".into(), difficulty: None, actor_id: None, opponent_id: None },
    ]);
    let r2 = run(&session, "激励后检定").await;
    let dice2 = dice_values(&session, r2).len();
    assert_eq!(dice1, 1, "无激励时单颗 d20（实际 {dice1}）");
    assert_eq!(dice2, 2, "dnd-inspired → keep_high → 掷两次取高（实际 {dice2}）");

    // 清单 10：对抗判定（对手在世界上 → 掷 vs 掷），opponent 非空
    h.provider.push(vec![Intent::Check {
        attribute: "dex".into(),
        difficulty: None,
        actor_id: None,
        opponent_id: Some("mon-ash-zombie".into()),
    }]);
    let r3 = run(&session, "对抗丧尸").await;
    let c3 = checks(&session, r3).into_iter().next().unwrap();
    let opp = c3.opponent.clone().expect("对抗判定必须有 opponent 署名");
    let dice3 = dice_values(&session, r3).len();
    assert_eq!(dice3, 2, "掷 vs 掷：主动方 + 对手各一颗（实际 {dice3}）");
    println!(
        "R3-8/10/11 PASS: mod1={mod1} dice1={dice1} dice2={dice2} opposed_opponent={:?} dice3={dice3}",
        opp.name
    );
}

// ============================================================
// 清单 9 / 13：触发点预置遭遇 + encounter_cleared + 掷表可反复
// ============================================================
#[tokio::test]
async fn r3_9_13_trigger_preset_cleared_and_refire() {
    let sb = lmop();
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
        assert!(!eval_cond(&CondExpr::EncounterCleared {}, &ctx).unwrap(), "无遭遇应为假");
    }

    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb, save) = publish_and_open(&h, "R3 触发点 + 反复", sb.clone()).await;
    let session = session_of(&h, &save).await;
    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-0b7".into(), character_id: None }]);
    run(&session, "去三猪小径").await;
    assert_eq!(session.projection().characters["inst-pc-lmop-talin"]["location_id"], "loc-lmop-0b7");

    let mut edges: BTreeMap<String, u32> = BTreeMap::new();
    let mut prev_active: BTreeMap<String, bool> = BTreeMap::new();
    let mut first_enc: Option<Value> = None;
    let mut rounds = 0;
    while rounds < 600 {
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
        if first_enc.is_none() && !proj.encounters.is_empty() {
            first_enc = Some(enc_of(&proj, 0));
        }
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
    assert!(!refired.is_empty(), "600 回合内同一掷表表项必须触发 ≥ 2 次；实际 {edges:?}");

    let enc = first_enc.expect("应至少建出一场遭遇");
    assert_eq!(enc["location_id"], "loc-lmop-0b7", "地点来自触发点");
    assert!(!enc["template_ids"].as_array().unwrap().is_empty(), "必须带 template_ids");
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
    let mut got: Vec<String> = enc["template_ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    want.sort();
    got.sort();
    assert_eq!(got, want, "template_ids 必须等于触发点预置集合");

    h.provider.push(vec![Intent::Move { destination_id: "loc-lmop-040".into(), character_id: None }]);
    run(&session, "离开").await;
    let mut guard = 0;
    while !live_enemies(&session).is_empty() {
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
    println!("R3-9/13 PASS: 反复触发={refired:?}（{rounds} 回合）；enc={enc}");
}

// ============================================================
// 清单 15（GAP-A）：get_character 四形态 / get_flag / get_encounter /
//                  host.target 与 host.actor 同级完整
// ============================================================
#[tokio::test]
async fn r3_15_gap_a_world_fact_primitives() {
    let mut sb = lmop();
    sb["lua_mounts"].as_array_mut().unwrap().push(json!({
        "id": "r3-probe-facts",
        "mount": "pre_resolve",
        "source": "local function hit(id) local c = host.get_character(id) return c ~= nil and c.id == 'inst-pc-lmop-talin' end\nhost.set_flag('r3.gc.key', hit('inst-pc-lmop-talin'))\nhost.set_flag('r3.gc.tpl', hit('pc-lmop-talin'))\nhost.set_flag('r3.gc.name', hit('塔林·银溪'))\nhost.set_flag('r3.gc.iid', hit('inst-pc-lmop-talin'))\nhost.set_flag('r3.gc.miss', host.get_character('nobody') == nil)\nhost.set_flag('r3.flags.table', type(host.list_flags()) == 'table')\nhost.set_flag('r3.flags.miss', host.get_flag('r3.not.set') == nil)\nlocal encounters = host.list_encounters()\nhost.set_flag('r3.enc.count', #encounters)\nif #encounters > 0 then\n  local e = host.get_encounter(encounters[1].id)\n  host.set_flag('r3.enc.byid', e ~= nil and e.id == encounters[1].id and #e.enemies > 0)\nelse\n  host.set_flag('r3.enc.byid', false)\nend\nlocal t = host.target\nhost.set_flag('r3.target.statuses', t ~= nil and type(t.statuses) == 'table')\nhost.set_flag('r3.target.attrs', t ~= nil and type(t.attributes) == 'table' and t.attributes.dex == 16)\nhost.set_flag('r3.target.res', t ~= nil and type(t.resources) == 'table' and t.resources['res-hp'] ~= nil)\nhost.set_flag('r3.target.place', t ~= nil and t.location_id ~= nil)\nhost.set_flag('r3.target.kind', t ~= nil and t.kind == 'pc')"
    }));
    let h = spawn_with(vec![
        vec![Intent::Encounter {
            name: "事实探针遭遇".into(),
            enemies: vec![EnemySpec {
                name: String::new(),
                hp: None,
                ac: None,
                template_id: Some("mon-ash-zombie".into()),
                count: Some(1),
                skill_id: None,
            }],
            note: None,
        }],
        vec![Intent::UseSkill { skill_id: "sk-lmop-stealth".into(), target_id: None }],
    ])
    .await;
    let (_sb, save) = publish_and_open(&h, "R3 GAP-A 事实探针", sb).await;
    let session = session_of(&h, &save).await;
    run(&session, "开战").await;
    run(&session, "潜行").await;

    let proj = session.projection();
    let f = |k: &str| proj.flags.get(k).cloned().unwrap_or(Value::Null);
    assert_eq!(f("r3.gc.key"), json!(true), "实例键命中");
    assert_eq!(f("r3.gc.tpl"), json!(true), "模板 id 命中");
    assert_eq!(f("r3.gc.name"), json!(true), "角色名命中");
    assert_eq!(f("r3.gc.iid"), json!(true), "instance_id 命中");
    assert_eq!(f("r3.gc.miss"), json!(true), "查不到返回 nil");
    assert_eq!(f("r3.flags.table"), json!(true), "list_flags 是表");
    assert_eq!(f("r3.flags.miss"), json!(true), "get_flag 查不到 nil");
    assert!(num(&f("r3.enc.count")) >= 1, "list_encounters 应看到刚建的遭遇");
    assert_eq!(f("r3.enc.byid"), json!(true), "get_encounter 按 id 命中且带 enemies");
    assert_eq!(f("r3.target.statuses"), json!(true), "host.target 含 statuses");
    assert_eq!(f("r3.target.attrs"), json!(true), "host.target 含 attributes（dex=16）");
    assert_eq!(f("r3.target.res"), json!(true), "host.target 含 resources");
    assert_eq!(f("r3.target.place"), json!(true), "host.target 含 location_id");
    assert_eq!(f("r3.target.kind"), json!(true), "host.target 含 kind");
    println!("R3-15 PASS: enc.count={} 四形态/标记/遭遇/目标快照全部命中", num(&f("r3.enc.count")));
}

// ============================================================
// 清单 16（GAP-L）：伏击按 host.target.statuses 给优势（正例 + 反例 + 数据驱动）
// ============================================================
fn run_ambusher(sb: &Value, actor_template: &str, statuses: Value) -> usize {
    let source = mount_source(sb, "dnd-ambusher-keep-high");
    let host = host_with(sb, 5);
    let actor = json!({
        "instance_id": "inst-amb", "template_id": actor_template, "name": "袭击者", "kind": "monster",
        "attributes": {}, "resources": { "res-hp": 22 }, "statuses": []
    });
    let target = json!({
        "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
        "attributes": { "dex": 16 }, "resources": { "res-hp": 24 }, "statuses": statuses,
        "location_id": "loc-a", "present": true
    });
    let ctx = LuaHostContext {
        script_id: "r3:ambush".into(),
        actor_id: "inst-amb".into(),
        actor,
        target_id: Some("inst-pc".into()),
        target: Some(target),
        ..Default::default()
    };
    let gate = |_: &CondExpr| true;
    let env = MountEnv { gate: Some(&gate), ..Default::default() };
    host.run_hook_with(&source, LuaMount::CheckPreRoll, &ctx, &env).expect("伏击脚本执行失败");
    keep_high_count(&host.drain_requests())
}

#[test]
fn r3_16_gap_l_ambusher_reads_target_statuses() {
    let sb = lmop();
    let with_status = run_ambusher(&sb, "mon-doppelganger", json!([{ "id": "dnd-surprised", "name": "受突袭" }]));
    let without = run_ambusher(&sb, "mon-doppelganger", json!([]));
    let other = run_ambusher(&sb, "mon-doppelganger", json!([{ "id": "dnd-prone", "name": "倒地" }]));
    let no_trait = run_ambusher(&sb, "mon-wolf", json!([{ "id": "dnd-surprised", "name": "受突袭" }]));
    assert_eq!(with_status, 1, "目标带 dnd-surprised → keep_high");
    assert_eq!(without, 0, "目标无状态 → 不给优势");
    assert_eq!(other, 0, "别的状态不算");
    assert_eq!(no_trait, 0, "袭击者没有伏击挂接 → 不给优势");

    // 数据驱动：把开放内容里的 fields.target_status 换成 dnd-prone，期望值随之翻转
    let mut sb2 = sb.clone();
    let defs = sb2["definitions"].as_array_mut().unwrap();
    let d = defs.iter_mut().find(|d| d["id"] == "ambush-doppelganger").unwrap();
    d["fields"]["target_status"] = json!("dnd-prone");
    let flipped_surprised = run_ambusher(&sb2, "mon-doppelganger", json!([{ "id": "dnd-surprised", "name": "受突袭" }]));
    let flipped_prone = run_ambusher(&sb2, "mon-doppelganger", json!([{ "id": "dnd-prone", "name": "倒地" }]));
    assert_eq!(flipped_prone, 1, "改 target_status=dnd-prone 后，倒地目标应给优势");
    assert_eq!(flipped_surprised, 0, "改后 dnd-surprised 不再给优势");
    println!(
        "R3-16 PASS: 正例={with_status} 无状态={without} 别的状态={other} 无挂接={no_trait}；改数据后 prone={flipped_prone} surprised={flipped_surprised}"
    );
}

// ============================================================
// 审计① / 清单 17（GAP-E）：save-half 缩放的是**引擎那份**效果骰，不是重掷
// ============================================================
#[test]
fn r3_audit1_17_save_half_scales_the_engine_roll() {
    let sb = lmop();
    let real = real_save_half_rule(&sb);
    assert!(real.contains("scale_effect(0.5)"), "规则包脚本必须声明 scale_effect(0.5)");
    assert!(!real.contains("engine_rng"), "规则包脚本不得自己掷效果骰");

    // 唯一差异 = 因子：把真实脚本的 0.5 换成 1.0，其余逐字不动
    let one = real.replace("scale_effect(0.5)", "scale_effect(1.0)");
    assert_ne!(one, real, "替换必须发生");

    let mut checked = 0;
    for seed in 1..=32u64 {
        let full = run_save_half(&sb, seed, &one, true, Some("inst-pc-lmop-talin"));
        let half = run_save_half(&sb, seed, &real, true, Some("inst-pc-lmop-talin"));

        // (1) 骰序逐位相同 —— 因子不改变掷骰，「同一颗效果骰」由此成立
        assert_eq!(full.dice, half.dice, "种子 {seed}：因子不得改变骰序");
        assert_eq!(full.dice.len(), 4, "种子 {seed}：1 颗 d20 + 一次 3d6 = 4 颗（这是引擎掷的）");

        // (2) 半值恰好是同一份的向零取整
        let f = hp_delta(&full.out).expect("因子 1.0 应有伤害 delta");
        let h = hp_delta(&half.out).expect("因子 0.5 应有伤害 delta");
        assert_eq!(h, (f as f64 * 0.5).trunc() as i64, "种子 {seed}：half={h} 必须是 full={f} 的一半");

        // (3) 出处：减半伤害是**引擎结算产物**（不是 Lua 请求替代品）
        assert_eq!(
            half.out.deltas().iter().filter(|d| d.field == "resources.res-hp").count(),
            1,
            "引擎产物里恰好一条 hp delta"
        );
        let lua_damage = half
            .out
            .requests
            .iter()
            .filter(|r| matches!(r, LuaRequest::ApplyEffect { effect, .. } if effect.get("kind").and_then(Value::as_str) == Some("damage")))
            .count();
        assert_eq!(lua_damage, 0, "引擎已结算，Lua 不得再补一份伤害");
        checked += 1;
    }

    // (4) 机制对照：T23 之前的脚本（Lua 重掷 + apply_effect）在同一技能上
    //     引擎产物为空（成功 = 不结算），减半只能作为 Lua 请求出现。
    let old = run_save_half(&sb, 7, OLD_REROLL_RULE, true, Some("inst-pc-lmop-talin"));
    let old_engine_hp = old.out.deltas().iter().filter(|d| d.field == "resources.res-hp").count();
    let old_lua_damage = old
        .out
        .requests
        .iter()
        .filter(|r| matches!(r, LuaRequest::ApplyEffect { effect, .. } if effect.get("kind").and_then(Value::as_str) == Some("damage")))
        .count();
    assert_eq!(old_engine_hp, 0, "旧机制：成功时引擎不结算效果（产物里没有伤害）");
    assert_eq!(old_lua_damage, 1, "旧机制：减半伤害只能由 Lua 自己补一份");
    println!(
        "R3-AUDIT1/17 PASS: {checked} 个种子下骰序逐位相同且 half==trunc(full*0.5)；新机制 引擎 hp delta=1 / Lua 伤害请求=0；旧机制 引擎=0 / Lua=1"
    );
}

// ============================================================
// 清单 18：多段效果 + modifiers 的缩放（重掷做不到）—— 独立构造
// ============================================================
fn own_fanout_skill() -> SkillDef {
    serde_json::from_value(json!({
        "id": "sk-r3-fanout",
        "name": "三段落石（第三轮自建）",
        "check": { "dice": "1d20", "kind": "save" },
        "cost": [{ "resource": "res-mana", "amount": 5 }],
        "effect": {
            "immediate": [
                { "kind": "damage", "amount": "2d6", "resource": "res-hp" },
                { "kind": "modify_resource", "resource": "res-mana", "amount": "1d4" },
                { "kind": "heal", "amount": "1d8", "resource": "res-hp" },
                { "kind": "set_flag", "flag": "r3.fanout.buried" }
            ],
            "modifiers": [{ "attribute": "dex", "value": -2 }]
        }
    }))
    .unwrap()
}

fn run_fanout(seed: u64, rule: &str, pin_success: bool) -> SaveRun {
    let host = LuaHost::new(seed).unwrap();
    let mut registry = LuaRegistry::new();
    registry.register(
        "r3-pin",
        LuaMount::CheckPreRoll,
        if pin_success { "host.modify_check('force_success')" } else { "host.modify_check('force_fail')" },
    );
    if !rule.is_empty() {
        registry.register("rule:fanout", LuaMount::CheckPostRoll, rule);
    }
    let skill = own_fanout_skill();
    let actor = json!({
        "instance_id": "inst-r3", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
        "attributes": { "str": 10, "dex": 16 }, "resources": { "res-hp": 30, "res-mana": 20 },
        "statuses": [], "location_id": "loc-a", "present": true
    });
    let lua_ctx = LuaHostContext {
        script_id: "r3:fanout".into(),
        actor_id: "inst-r3".into(),
        actor: actor.clone(),
        target_id: Some("inst-r3".into()),
        target: Some(actor.clone()),
        skill: serde_json::to_value(&skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    let rng = host.rng_handle();
    let out = {
        let mut ctx = CommandContext {
            actor_id: "inst-r3",
            actor: &actor,
            target_id: Some("inst-r3"),
            target: Some(&actor),
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
    SaveRun { out, dice: rng.lock().unwrap().consumed.clone() }
}

#[test]
fn r3_18_scale_covers_every_numeric_delta() {
    let rule = "if host.check_kind == 'save' and host.check_result then host.scale_effect(0.5) end";
    let one = rule.replace("0.5", "1.0");
    let mut covered = 0;
    for seed in 1..=24u64 {
        let full = run_fanout(seed, &one, true);
        let half = run_fanout(seed, rule, true);
        assert_eq!(full.dice, half.dice, "种子 {seed}：骰序不受因子影响");
        // 3 段数值骰：2d6(2) + 1d4(1) + 1d8(1) = 4 颗 + 1 颗 d20 = 5
        assert_eq!(full.dice.len(), 5, "种子 {seed}：判定 1 + 效果 4 颗");

        // 只比引擎的**效果**产物（不含消耗：消耗在缩放之后并入，字段名与第 2 段重名）
        let numeric_paths = ["resources.res-hp", "resources.res-mana"];
        // effects.deltas 在结尾会并入消耗 → 只取效果自身的前 4 条
        for (i, d) in full.out.effects.deltas.iter().take(4).enumerate() {
            let h = &half.out.effects.deltas[i];
            assert_eq!(d.field, h.field, "种子 {seed}：第 {i} 条 delta 字段必须对齐");
            if numeric_paths.contains(&d.field.as_str()) {
                let vf = d.value.as_i64().unwrap();
                let vh = h.value.as_i64().unwrap();
                assert_eq!(vh, (vf as f64 * 0.5).trunc() as i64, "种子 {seed}：第 {i} 条数值 delta 必须缩放");
            } else {
                assert_eq!(d.value, h.value, "种子 {seed}：非数值 delta 不得改（第 {i} 条：{}）", d.field);
            }
        }
        // 消耗不随因子缩（在缩放之后并入）：res-mana 上恰好有一条 -5
        let cost = half
            .out
            .deltas()
            .iter()
            .filter(|d| d.field == "resources.res-mana" && d.value == json!(-5))
            .count();
        assert_eq!(cost, 1, "种子 {seed}：消耗 -5 必须逐字保留");
        // 静态修正原样保留
        assert_eq!(half.out.effects.modifiers, full.out.effects.modifiers);
        assert_eq!(half.out.effects.modifiers.len(), 1);
        covered += 1;
    }

    // 决定性对照：把 T23 之前的「Lua 重掷」脚本挂到同一多段技能上 ——
    // 它只看 immediate[1] 且只认 NdM 形态 → 第 2 / 3 段数值全丢，modifiers 也丢。
    let old = run_fanout(7, OLD_REROLL_RULE, true);
    assert_eq!(
        old.out.deltas().iter().filter(|d| d.field == "resources.res-hp").count(),
        0,
        "旧脚本：引擎不结算，产物里没有伤害"
    );
    let old_applied: Vec<i64> = old
        .out
        .requests
        .iter()
        .filter_map(|r| match r {
            LuaRequest::ApplyEffect { effect, .. } if effect.get("kind").and_then(Value::as_str) == Some("damage") => {
                effect.get("amount").and_then(Value::as_str).and_then(|s| s.parse::<i64>().ok())
            }
            _ => None,
        })
        .collect();
    assert_eq!(old_applied.len(), 1, "旧脚本只补出第一段伤害");
    let old_requests = old.out.requests.len();
    println!(
        "R3-18 PASS: {covered} 个种子下 3 段数值 delta 全部按因子缩放（消耗/标记/静态修正不动）；旧重掷脚本只有 {old_requests} 条请求、只补第一段 {old_applied:?}（第 2/3 段与 modifiers 全丢）"
    );
}

// ============================================================
// 审计③a：a12 / a12b 的断言是否还有牙齿（变异）
// ============================================================
#[test]
fn r3_audit3a_a12_assertions_teeth() {
    let sb = lmop();
    let real = real_save_half_rule(&sb);
    let one = real.replace("scale_effect(0.5)", "scale_effect(1.0)");
    let quarter = real.replace("scale_effect(0.5)", "scale_effect(0.25)");

    // 基线：真实规则包，自目标（a12b 口径）
    let base = run_save_half(&sb, 7, &real, true, None);
    let base_dice = base.dice.len();
    let base_delta = -hp_delta(&base.out).unwrap();
    assert_eq!(base_dice, 4, "a12 断言 dice==4 在基线成立");
    assert!((1..=9).contains(&base_delta), "a12 断言 delta∈1..9 在基线成立（{base_delta}）");

    // M1：删掉缩放声明（=「减半这条规则没了」）→ 成功时引擎不结算 → dice=1、delta=0
    let m1 = run_save_half(&sb, 7, "", true, None);
    let m1_dice = m1.dice.len();
    let m1_delta = hp_delta(&m1.out).map(|v| -v).unwrap_or(0);
    let m1_fails_dice = m1_dice != 4;
    let m1_fails_delta = !(1..=9).contains(&m1_delta);
    assert!(m1_fails_dice && m1_fails_delta, "M1 必须让 a12 的两条断言都 FAIL");

    // M2：因子 0.5→1.0（= 把减半改坏成满伤）→ delta∈3..18：delta>9 时断言 FAIL
    let mut m2_got = Vec::new();
    let mut m2_fail = 0usize;
    for seed in 1..=64u64 {
        let r = run_save_half(&sb, seed, &one, true, None);
        let d = -hp_delta(&r.out).unwrap();
        if !(1..=9).contains(&d) {
            m2_fail += 1;
        }
        m2_got.push(d);
    }

    // M3：T23 之前的「Lua 重掷」脚本 —— a12 的两条断言**照样 PASS**（机制盲）
    let m3 = run_save_half(&sb, 7, OLD_REROLL_RULE, true, None);
    let m3_dice = m3.dice.len();
    let m3_engine = m3.out.deltas().iter().filter(|d| d.field == "resources.res-hp").count();
    let m3_lua = m3
        .out
        .requests
        .iter()
        .filter(|r| matches!(r, LuaRequest::ApplyEffect { effect, .. } if effect.get("kind").and_then(Value::as_str) == Some("damage")))
        .count();
    assert_eq!(m3_dice, 4, "M3（重掷）dice 仍是 4 → a12 的 dice 断言抓不到机制替换");
    assert_eq!(m3_engine, 0, "M3：引擎产物没有伤害（成功=不结算）");
    assert_eq!(m3_lua, 1, "M3：伤害来自 Lua 补的一份");

    // M4：因子 0.25 → delta∈1..4，a12 的区间断言抓不到
    let mut m4_fail = 0usize;
    let mut m4_got = Vec::new();
    for seed in 1..=64u64 {
        let r = run_save_half(&sb, seed, &quarter, true, None);
        let d = -hp_delta(&r.out).unwrap();
        if !(1..=9).contains(&d) {
            m4_fail += 1;
        }
        m4_got.push(d);
    }

    println!(
        "R3-AUDIT3a: 基线 dice={base_dice} delta={base_delta}\n  M1(无声明) dice={m1_dice} delta={m1_delta} → 断言 FAIL={}\n  M2(因子1.0) 64 种子失败 {m2_fail}/64（delta 例：{:?}）\n  M3(旧重掷) dice={m3_dice} 引擎delta={m3_engine} Lua补={m3_lua} → a12 仍 PASS\n  M4(因子0.25) 64 种子失败 {m4_fail}/64（delta 例：{:?}）",
        m1_fails_dice && m1_fails_delta,
        &m2_got[..8.min(m2_got.len())],
        &m4_got[..8.min(m4_got.len())]
    );
    assert!(m2_fail > 0, "M2 必须至少有一个种子被抓住");
    assert_eq!(m4_fail, 0, "M4 全部逃脱 —— 断言对「缩放过头」没有牙齿");
}

// ============================================================
// 审计③b：r2_6 负对照的**失效机制**描述是否还成立
// ============================================================
#[tokio::test]
async fn r3_audit3b_r26_negative_control_mechanism() {
    let sb = lmop();
    let real = real_save_half_rule(&sb);

    // 引擎级：显式给一个解析不到的目标（target=None），豁免成功
    let ghost = run_save_half(&sb, 7, &real, true, Some("r3-不存在的目标"));
    let ghost_hp = ghost.out.deltas().iter().find(|d| d.field == "resources.res-hp").cloned();
    let engine_hp = ghost.out.deltas().iter().filter(|d| d.field == "resources.res-hp").count();
    // 旧注释说「host.target 为 nil → 减半分支静默不发」；实测：分支发了、引擎结算了，
    // 只是 delta 的 entity_id 指向不存在的实体（真实会话里被 apply_delta 静默丢弃）。
    assert_eq!(engine_hp, 1, "减半分支**确实发了**：引擎结算出一条 hp delta");
    let ghost_entity = ghost_hp.as_ref().unwrap().entity_id.clone();
    assert_eq!(ghost_entity, "r3-不存在的目标", "伤害落在一个不存在的实体键上");
    assert!(ghost_hp.as_ref().unwrap().value.as_i64().unwrap() < 0, "确实是伤害（负数）");

    // 对照：自目标（target=None）时伤害才会落到 PC 身上 —— 说明断言 delta==0 不是恒真
    let self_run = run_save_half(&sb, 7, &real, true, None);
    assert_eq!(self_run.out.deltas()[0].entity_id, "inst-pc-lmop-talin", "自目标时落到施法者本人");
    assert!(hp_delta(&self_run.out).unwrap() < 0);

    // 会话级：跑真实 r2_6-C 形态（不存在的 target_id），确认 delta==0 且不留幻影实例
    let h = spawn_shared(CueProvider::new(vec![])).await;
    let (_sb_id, save) = publish_and_open(&h, "R3 负对照", sb.clone()).await;
    let session = session_of(&h, &save).await;
    let mut saw_success_zero = false;
    for _ in 0..40 {
        h.provider.push(vec![Intent::UseSkill {
            skill_id: "sk-lmop-rubble-collapse".into(),
            target_id: Some("r3-不存在的目标".into()),
        }]);
        let before = pc_hp(&session);
        let r = run(&session, "踩到瓦砾").await;
        let c = checks(&session, r).into_iter().next().unwrap();
        let dice = dice_values(&session, r).len();
        if c.result {
            assert_eq!(before - pc_hp(&session), 0, "r2_6-C 的 delta==0 仍成立");
            assert_eq!(dice, 4, "但引擎仍掷了效果骰（分支没有静默失效）");
            saw_success_zero = true;
            break;
        }
    }
    assert!(saw_success_zero, "40 次内需观察到一次成功的负对照");
    let proj = session.projection();
    assert_eq!(proj.characters.len(), 1, "不存在的目标不得留下幻影实例");
    println!(
        "R3-AUDIT3b PASS: 负对照 delta==0 仍成立（断言未恒真：自目标时 delta={}），但机制已变——引擎结算了 entity_id={ghost_entity} 的伤害并静默丢弃，旧注释「host.target 为 nil → 分支不发」不再成立",
        hp_delta(&self_run.out).unwrap()
    );
}

// ============================================================
// 审计④：集群战术近似的边界（GAP-A 是「近似提高」还是「闭合」）
// ============================================================
fn run_pack(
    sb: &Value,
    facts: Value,
    actor_key: &str,
    actor_place: Option<&str>,
    target_place: Option<&str>,
    check_kind: Option<CheckKind>,
) -> usize {
    let source = mount_source(sb, "dnd-pack-tactics");
    let host = host_with(sb, 5);
    host.set_world_facts(facts);
    let actor = json!({
        "instance_id": actor_key, "template_id": "mon-wolf", "name": "狼", "kind": "monster",
        "attributes": {}, "resources": { "res-hp": 11 }, "statuses": [],
        "location_id": actor_place, "present": true
    });
    let target = json!({
        "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
        "attributes": { "dex": 16 }, "resources": { "res-hp": 24 }, "statuses": [],
        "location_id": target_place, "present": true
    });
    let ctx = LuaHostContext {
        script_id: "r3:pack".into(),
        actor_id: actor_key.into(),
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
    let gate = |_: &CondExpr| true;
    let env = MountEnv { gate: Some(&gate), check: signature.as_ref(), ..Default::default() };
    host.run_hook_with(&source, LuaMount::CheckPreRoll, &ctx, &env).expect("集群战术脚本执行失败");
    keep_high_count(&host.drain_requests())
}

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

#[test]
fn r3_audit4_pack_tactics_approximation_boundaries() {
    let sb = lmop();
    let enc = |members: Value| {
        json!({ "enc-1": { "id": "enc-1", "name": "野外遭遇", "active": true, "enemies": members } })
    };
    let member = |key: &str, hp: i64| json!({ "id": key, "instance_id": key, "template_id": "mon-wolf", "hp": hp });
    let two = |hp2: i64, place2: &str, st2: Value| {
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf_char("inst-wolf-2", hp2, Some(place2), st2),
            ],
            enc(json!([member("inst-wolf-1", 11), member("inst-wolf-2", hp2)])),
        )
    };

    // P0 正例 / P1 同伴倒下 / P2 狼与目标异地
    let p0 = run_pack(&sb, two(11, "loc-a", json!([])), "inst-wolf-1", Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    let p1 = run_pack(&sb, two(0, "loc-a", json!([])), "inst-wolf-1", Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    let p2 = run_pack(&sb, two(11, "loc-a", json!([])), "inst-wolf-1", Some("loc-a"), Some("loc-b"), Some(CheckKind::Attack));
    // 边界 FP-A：同伴在**另一个地点**（脚本从不检查同伴位置）
    let fp_a = run_pack(&sb, two(11, "loc-c", json!([])), "inst-wolf-1", Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    // 边界 FP-B：同伴 HP>0 但带失能状态（「未失能」被近似成 HP>0）
    let fp_b = run_pack(&sb, two(11, "loc-a", json!([{ "id": "stunned", "name": "昏迷" }])), "inst-wolf-1", Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));
    // 边界 FN-A：同伴在**另一场遭遇**里（同地点）→ 漏判
    let fn_a = run_pack(
        &sb,
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf_char("inst-wolf-2", 11, Some("loc-a"), json!([])),
            ],
            json!({
                "enc-1": { "id": "enc-1", "name": "遭遇一", "active": true, "enemies": [member("inst-wolf-1", 11)] },
                "enc-2": { "id": "enc-2", "name": "遭遇二", "active": true, "enemies": [member("inst-wolf-2", 11)] }
            }),
        ),
        "inst-wolf-1",
        Some("loc-a"),
        Some("loc-a"),
        Some(CheckKind::Attack),
    );
    // 边界 FP-C：狼与目标都没有 location_id → 地点闸门被跳过
    let fp_c = run_pack(
        &sb,
        facts_with(
            vec![
                wolf_char("inst-wolf-1", 11, None, json!([])),
                wolf_char("inst-wolf-2", 11, None, json!([])),
            ],
            enc(json!([member("inst-wolf-1", 11), member("inst-wolf-2", 11)])),
        ),
        "inst-wolf-1",
        None,
        None,
        Some(CheckKind::Attack),
    );
    // 边界 FP-D：遭遇条目在，但实例查不到 → 不算
    let fp_d = run_pack(
        &sb,
        facts_with(
            vec![wolf_char("inst-wolf-1", 11, Some("loc-a"), json!([]))],
            enc(json!([member("inst-wolf-1", 11), member("inst-wolf-ghost", 11)])),
        ),
        "inst-wolf-1",
        Some("loc-a"),
        Some("loc-a"),
        Some(CheckKind::Attack),
    );
    // 边界 FP-E：非攻击判定（属性检定）照样给优势
    let fp_e_attr = run_pack(&sb, two(11, "loc-a", json!([])), "inst-wolf-1", Some("loc-a"), Some("loc-a"), Some(CheckKind::Attribute));
    let fp_e_attack = run_pack(&sb, two(11, "loc-a", json!([])), "inst-wolf-1", Some("loc-a"), Some("loc-a"), Some(CheckKind::Attack));

    assert_eq!(p0, 1, "P0 正例必须给优势");
    assert_eq!(p1, 0, "P1 同伴倒下 → 不给");
    assert_eq!(p2, 0, "P2 目标不在一处 → 不给");
    assert_eq!(fp_a, 1, "FP-A：同伴在别的地点仍给优势（近似误差，实测）");
    assert_eq!(fp_b, 1, "FP-B：同伴带失能状态但 HP>0 仍给优势（近似误差，实测）");
    assert_eq!(fn_a, 0, "FN-A：同伴在另一场遭遇 → 漏判（近似误差，实测）");
    assert_eq!(fp_c, 1, "FP-C：双方都无 location_id → 地点闸门被跳过（近似误差，实测）");
    assert_eq!(fp_d, 0, "FP-D：遭遇条目查不到实例 → 不给");
    assert_eq!(fp_e_attr, 1, "FP-E：非攻击判定也给优势（脚本不看判定签名）");
    assert_eq!(fp_e_attack, 1);
    println!(
        "R3-AUDIT4: P0={p0} P1(同伴倒)={p1} P2(异地)={p2} | FP-A(同伴在别处)={fp_a} FP-B(同伴失能)={fp_b} FP-C(无地点)={fp_c} FP-E(属性检定)={fp_e_attr} | FN-A(同伴在别的遭遇)={fn_a} FP-D(无实例)={fp_d}"
    );
}

// ============================================================
// 审计②补：失败附加状态落在**目标**而不是施法者（连带修正的一处位置变化）
// ============================================================
#[test]
fn r3_audit2b_fail_status_lands_on_target_not_caster() {
    let sb = lmop();
    let real = real_save_half_rule(&sb);
    let skill = skill_of(&sb, "sk-lmop-rubble-collapse");
    let host = host_with(&sb, 3);
    let mut registry = LuaRegistry::new();
    registry.register("r3-pin", LuaMount::CheckPreRoll, "host.modify_check('force_fail')");
    registry.register("rule:save-half", LuaMount::CheckPostRoll, &real);
    let actor = instance_json(&sb, "pc-lmop-talin", "inst-pc-lmop-talin");
    let target = instance_json(&sb, "mon-ash-zombie", "inst-zombie");
    let lua_ctx = LuaHostContext {
        script_id: "r3:save-half-target".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: actor.clone(),
        target_id: Some("inst-zombie".into()),
        target: Some(target.clone()),
        skill: serde_json::to_value(&skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    let rng = host.rng_handle();
    let out = {
        let mut ctx = CommandContext {
            actor_id: "inst-pc-lmop-talin",
            actor: &actor,
            target_id: Some("inst-zombie"),
            target: Some(&target),
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
    let hp = out
        .deltas()
        .iter()
        .find(|d| d.field == "resources.res-hp")
        .expect("失败分支必须结算伤害");
    assert_eq!(hp.entity_id, "inst-zombie", "伤害必须落在目标实例");
    let st = out
        .requests
        .iter()
        .find_map(|r| match r {
            LuaRequest::ApplyStatus { target, status, .. } => Some((target.clone(), status.clone())),
            _ => None,
        })
        .expect("失败分支必须由脚本补 dnd-prone");
    assert_eq!(st.0, "inst-zombie", "附加状态必须落在目标（旧实现由引擎写 actor_id）");
    assert_eq!(st.1, "dnd-prone");
    assert_ne!(st.0, "inst-pc-lmop-talin", "不得落在施法者头上");
    println!(
        "R3-AUDIT2b PASS: 失败分支 伤害 entity={} / 状态 target={} status={}（旧引擎路径是 status_delta(actor_id)）",
        hp.entity_id, st.0, st.1
    );
}



