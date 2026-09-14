//! scripts/lmop-engine-check —— LMoP 故事书的**真实引擎**校验器（T15 缺口闭合验收）。
//!
//! 做什么（两件事，一次跑完）：
//!   ① 发布门：validate_storybook（错误 / 警告）+ lint_storybook（Lua 预检问题）
//!   ② 运行时规则断言：用真实 LuaHost + 真实骨架求值器跑规则包里**实际的 Lua 脚本**，
//!      断言 T17/T18 新原语真的把缺口闭合了（不是靠读 JSON 结构猜）。
//!
//! 用法：
//!   lmop-engine-check <storybook.json>
//! 输出（stdout 一个 JSON 对象）：
//!   { error_count, warning_count, errors, lua_issues, rule_assertions, rule_failures }
//! 规则断言失败时 **退出码 1**（发布门失败也是 1）。
//!
//! 构建（target 放 .scratch，不入库）：
//!   CARGO_TARGET_DIR=$PWD/.scratch/lmop-engine-check-target \
//!     cargo build --manifest-path scripts/lmop-engine-check/Cargo.toml
//!
//! 断言清单：
//!   proficiency.data_driven      改故事书 definition 的 bonus → 真实 Lua 给出的加值随之变
//!   sunlight.by_signature        只对 attack 与 (attribute ∧ wis) 取低，其它判定零请求
//!   encounter_table.refires      同一张表的同一行，在同一局里能触发两次以上（repeatable + clear_flag）
//!   encounter_table.legacy_once  反证：不标 repeatable、不复位时，同一行整局只触发一次
//!   xp.improvised_encounter      导演即兴建的遭遇（非掷表）杀怪也发 XP；非击败事件不发
//!   bestiary.xp_matches          开放内容里的 XP 与图鉴 statblock.xp 逐条相等

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::process::ExitCode;

use octopus_engine::conditions::evaluate_skeleton_full;
use octopus_engine::lua_host::{CheckModifier, LuaCheckContext, LuaEventContext, LuaHostContext, LuaMount, MountEnv};
use octopus_engine::lua_lint::lint_storybook;
use octopus_engine::{validate_storybook_result, EvalContext, LuaHost, LuaRequest};
use octopus_types::{CheckKind, IssueSeverity};
use serde_json::{json, Map, Value};

struct Assertions {
    items: Vec<Value>,
    failures: usize,
}

impl Assertions {
    fn new() -> Self {
        Self { items: Vec::new(), failures: 0 }
    }
    fn check(&mut self, name: &str, pass: bool, detail: impl Into<String>) {
        let detail = detail.into();
        if !pass {
            self.failures += 1;
        }
        self.items.push(json!({ "name": name, "pass": pass, "detail": detail }));
    }
}

fn main() -> ExitCode {
    let path = std::env::args().nth(1).unwrap_or_default();
    if path.is_empty() {
        eprintln!("用法：lmop-engine-check <storybook.json>");
        return ExitCode::from(2);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("读不到故事书 {path}：{e}");
            return ExitCode::from(2);
        }
    };
    let sb: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("故事书 JSON 解析失败：{e}");
            return ExitCode::from(2);
        }
    };

    // ---------- ① 发布门 ----------
    let result = validate_storybook_result(&sb);
    let errors: Vec<&_> = result
        .issues
        .iter()
        .filter(|i| matches!(i.severity, IssueSeverity::Error))
        .collect();
    let warnings: Vec<&_> = result
        .issues
        .iter()
        .filter(|i| matches!(i.severity, IssueSeverity::Warning))
        .collect();
    let lua_issues = lint_storybook(&sb);

    // ---------- ② 运行时规则断言 ----------
    let mut a = Assertions::new();
    run_rule_assertions(&sb, &mut a);

    let out = json!({
        "error_count": errors.len(),
        "warning_count": warnings.len(),
        "errors": errors.iter().map(|i| json!({ "code": i.code, "target": i.target, "message": i.message })).collect::<Vec<_>>(),
        "lua_issues": lua_issues.iter().map(|i| json!({ "target": i.target, "code": i.code, "message": i.message })).collect::<Vec<_>>(),
        "rule_assertions": a.items,
        "rule_failures": a.failures,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());

    if errors.is_empty() && lua_issues.is_empty() && a.failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

// ============================================================
// 工具
// ============================================================

fn mount_source(sb: &Value, id: &str) -> String {
    sb.get("lua_mounts")
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(id)))
        .and_then(|m| m.get("source"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn character(sb: &Value, id: &str) -> Value {
    sb.get("characters")
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(id)))
        .cloned()
        .unwrap_or(Value::Null)
}

fn definitions(sb: &Value, kind: &str) -> Vec<Value> {
    sb.get("definitions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|d| d.get("kind").and_then(Value::as_str) == Some(kind))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn read_data(sb: &Value) -> Value {
    json!({
        "characters": sb.get("characters").cloned().unwrap_or(Value::Array(vec![])),
        "definitions": sb.get("definitions").cloned().unwrap_or(Value::Array(vec![])),
    })
}

fn host_with(sb: &Value, seed: u64) -> LuaHost {
    let host = LuaHost::new(seed).expect("LuaHost 建不起来");
    host.set_read_data(read_data(sb));
    host
}

/// 判定签名（掷骰前）：签名在 check_pre_roll 就有值，这是 GAP-D 闭合的判据。
fn signature(attribute: &str, kind: CheckKind, target: i64) -> LuaCheckContext {
    LuaCheckContext {
        attribute: attribute.to_string(),
        kind: Some(kind),
        target,
        ..Default::default()
    }
}

/// 跑一条脚本正文（不经过 when 闸门；when 由调用方在断言里显式当作成立）。
fn run(host: &LuaHost, source: &str, mount: LuaMount, ctx: &LuaHostContext, env: &MountEnv<'_>) -> Vec<LuaRequest> {
    host.run_hook_with(source, mount, ctx, env)
        .unwrap_or_else(|e| panic!("Lua 执行失败：{e}"));
    host.drain_requests()
}

fn modifies(requests: &[LuaRequest]) -> Vec<(CheckModifier, i64)> {
    requests
        .iter()
        .filter_map(|r| match r {
            LuaRequest::ModifyCheck { mode, amount } => Some((*mode, *amount)),
            _ => None,
        })
        .collect()
}

// ============================================================
// 规则断言
// ============================================================

fn run_rule_assertions(sb: &Value, a: &mut Assertions) {
    proficiency_data_driven(sb, a);
    sunlight_by_signature(sb, a);
    encounter_table_refires(sb, a);
    xp_improvised_encounter(sb, a);
    bestiary_xp_matches(sb, a);
}

/// 验收 1：熟练加值数据驱动。
fn proficiency_data_driven(sb: &Value, a: &mut Assertions) {
    let source = mount_source(sb, "dnd-proficiency");
    let pc = character(sb, "pc-lmop-talin");
    let def_id = "prof-pc-lmop-talin-dex";
    let declared = definitions(sb, "dnd-proficiency")
        .iter()
        .find(|d| d.get("id").and_then(Value::as_str) == Some(def_id))
        .and_then(|d| d.pointer("/fields/bonus"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok());

    let Some(declared) = declared else {
        a.check("proficiency.data_driven", false, format!("故事书里找不到 {def_id} 的 fields.bonus"));
        return;
    };

    let ctx = LuaHostContext {
        script_id: "lmop-check:proficiency".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: pc.clone(),
        ..Default::default()
    };

    // 命中：dex 属性检定（签名 attribute=dex, kind=attribute）
    let host = host_with(sb, 7);
    let env = MountEnv {
        check: Some(&signature("dex", CheckKind::Attribute, 12)),
        ..Default::default()
    };
    let hit = modifies(&run(&host, &source, LuaMount::CheckPreRoll, &ctx, &env));

    // 未命中：同一角色、同样 kind，但属性是 str（没有对应 definition）
    let host_miss = host_with(sb, 7);
    let env_miss = MountEnv {
        check: Some(&signature("str", CheckKind::Attribute, 12)),
        ..Default::default()
    };
    let miss = modifies(&run(&host_miss, &source, LuaMount::CheckPreRoll, &ctx, &env_miss));

    // 关键一步：**改故事书数据**（不是改脚本）——把 definition 的 bonus 换成新值，再跑同一条 Lua。
    let mut mutated = sb.clone();
    let bumped = declared + 4;
    if let Some(list) = mutated.get_mut("definitions").and_then(Value::as_array_mut) {
        for def in list.iter_mut() {
            if def.get("id").and_then(Value::as_str) == Some(def_id) {
                def["fields"]["bonus"] = Value::String(bumped.to_string());
            }
        }
    }
    let host_bumped = host_with(&mutated, 7);
    let bumped_req = modifies(&run(&host_bumped, &source, LuaMount::CheckPreRoll, &ctx, &env));

    let hit_value = hit.first().map(|(_, n)| *n);
    let miss_len = miss.len();
    let bumped_value = bumped_req.first().map(|(_, n)| *n);
    let pass = hit_value == Some(declared)
        && miss_len == 0
        && bumped_value == Some(bumped)
        && bumped != declared;
    a.check(
        "proficiency.data_driven",
        pass,
        format!(
            "definition {def_id}.fields.bonus={declared} → Lua 给 add {}；同角色 str 检定请求数={miss_len}；\
             把 bonus 改成 {bumped}（只改故事书数据）→ Lua 给 add {}（证明加值不是烘死的）",
            hit_value.map(|v| v.to_string()).unwrap_or_else(|| "无".into()),
            bumped_value.map(|v| v.to_string()).unwrap_or_else(|| "无".into()),
        ),
    );
}

/// 验收 2：日照敏感按判定签名（只对攻击骰与依赖视力的感知检定）。
fn sunlight_by_signature(sb: &Value, a: &mut Assertions) {
    let source = mount_source(sb, "dnd-sunlight-sensitivity");
    let wraith = character(sb, "mon-mormesk-the-wraith");
    let wolf = character(sb, "mon-wolf");

    let ctx_for = |actor: &Value| LuaHostContext {
        script_id: "lmop-check:sunlight".into(),
        actor_id: "inst-x".into(),
        actor: actor.clone(),
        ..Default::default()
    };

    let cases: [(&str, CheckKind, bool); 4] = [
        ("str", CheckKind::Attack, true),      // 攻击骰：劣势
        ("wis", CheckKind::Attribute, true),   // 依赖视力的感知（察觉）检定：劣势
        ("str", CheckKind::Attribute, false),  // 力量检定：不受影响
        ("con", CheckKind::Save, false),       // 体质豁免：不受影响
    ];
    let mut details = Vec::new();
    let mut pass = true;
    for (attribute, kind, expect_low) in cases {
        let host = host_with(sb, 11);
        let sig = signature(attribute, kind, 12);
        let env = MountEnv { check: Some(&sig), ..Default::default() };
        let requests = modifies(&run(&host, &source, LuaMount::CheckPreRoll, &ctx_for(&wraith), &env));
        let lows = requests.iter().filter(|(m, _)| *m == CheckModifier::KeepLow).count();
        let got = lows > 0;
        if got != expect_low || requests.len() != lows {
            pass = false;
        }
        details.push(format!(
            "{}({:?}) → keep_low={}（期望 {}）",
            attribute,
            kind,
            lows,
            if expect_low { 1 } else { 0 }
        ));
    }
    // 非日照敏感生物（狼）在阳光下攻击：零请求（名单也从开放内容读）
    let host = host_with(sb, 11);
    let sig = signature("str", CheckKind::Attack, 12);
    let env = MountEnv { check: Some(&sig), ..Default::default() };
    let other = modifies(&run(&host, &source, LuaMount::CheckPreRoll, &ctx_for(&wolf), &env));
    if !other.is_empty() {
        pass = false;
    }
    details.push(format!("狼（无日照敏感挂接）→ 请求数={}", other.len()));
    a.check("sunlight.by_signature", pass, details.join("；"));
}

/// 验收 3：掷表遭遇可反复（repeatable 边沿 + clear_flag 复位）。
///
/// 用**真实的 Lua 脚本**（dnd-wander-day / dnd-wander-reset）与**真实的骨架求值器**
/// （conditions::evaluate_skeleton_full）跑一局 400 回合的迷你模拟：
///   回合开始：跑掷表 Lua（真 RNG）→ set_flag
///   回合中  ：上一场遭遇结束 → 跑复位 Lua（encounter_cleared）→ clear_flag
///   回合末  ：骨架求值 → 记录边沿触发的触发点
/// 断言：某个触发点 id 在同一局里触发 ≥ 2 次。
fn encounter_table_refires(sb: &Value, a: &mut Assertions) {
    let wander = mount_source(sb, "dnd-wander-day");
    let reset = mount_source(sb, "dnd-wander-reset");
    let skeleton = sb.get("skeleton").cloned().unwrap_or(Value::Null);
    let pc = character(sb, "pc-lmop-talin");

    let mut skeleton_legacy = skeleton.clone();
    if let Some(chapters) = skeleton_legacy.as_array_mut() {
        for chapter in chapters.iter_mut() {
            if let Some(scenes) = chapter.get_mut("scenes").and_then(Value::as_array_mut) {
                for scene in scenes.iter_mut() {
                    if let Some(triggers) = scene.get_mut("triggers").and_then(Value::as_array_mut) {
                        for trigger in triggers.iter_mut() {
                            // 反证对照组：把 repeatable 去掉（= 旧世界）
                            if let Some(obj) = trigger.as_object_mut() {
                                obj.remove("repeatable");
                            }
                        }
                    }
                }
            }
        }
    }

    let fired = simulate(sb, &skeleton, &wander, &reset, &pc, 400, true);
    let legacy = simulate(sb, &skeleton_legacy, &wander, &reset, &pc, 400, false);

    let repeat = fired.iter().filter(|(_, n)| **n >= 2).count();
    let max = fired.values().copied().max().unwrap_or(0);
    let legacy_max = legacy.values().copied().max().unwrap_or(0);
    let mut top: Vec<(&String, &usize)> = fired.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let sample: Vec<String> = top.iter().take(3).map(|(k, n)| format!("{k}×{n}")).collect();

    let pass = max >= 2 && repeat >= 1 && legacy_max <= 1;
    a.check(
        "encounter_table.refires",
        pass,
        format!(
            "400 回合模拟：{} 个触发点被边沿触发，最多 {max} 次（{}）；\
             反证组（去掉 repeatable / 复位）最多 {legacy_max} 次",
            fired.len(),
            sample.join("、"),
        ),
    );
}

fn simulate(
    sb: &Value,
    skeleton: &Value,
    wander: &str,
    reset: &str,
    pc: &Value,
    rounds: usize,
    repeatable: bool,
) -> BTreeMap<String, usize> {
    let host = host_with(sb, 20250614);
    let ctx = LuaHostContext {
        script_id: "lmop-check:wander".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: pc.clone(),
        scene_id: "sc-lmop-0b7".into(),
        ..Default::default()
    };
    let actx = LuaHostContext {
        script_id: "lmop-check:wander-reset".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: pc.clone(),
        ..Default::default()
    };

    let mut flags: BTreeMap<String, Value> = BTreeMap::new();
    let mut triggers: Map<String, Value> = Map::new();
    let goals: Map<String, Value> = Map::new();
    let relationships: Vec<Value> = Vec::new();
    let encounters: Vec<Value> = Vec::new();
    let attributes = pc.get("attributes").cloned();
    let attributes_obj = attributes.as_ref().and_then(Value::as_object);

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    // 正在进行的掷表遭遇：(触发点 id, 遭遇名)。下一回合「中途」结束它并复位标记。
    let mut active: VecDeque<(String, String)> = VecDeque::new();

    for _ in 0..rounds {
        // 回合开始：掷表 Lua（真 engine_rng）→ set_flag
        for req in run(&host, wander, LuaMount::TurnEnd, &ctx, &MountEnv::default()) {
            if let LuaRequest::ApplyEffect { effect, .. } = req {
                if effect.get("kind").and_then(Value::as_str) == Some("set_flag") {
                    if let Some(flag) = effect.get("flag").and_then(Value::as_str) {
                        let value = effect.get("value").cloned().unwrap_or(Value::Bool(true));
                        flags.insert(flag.to_string(), value);
                    }
                }
            }
        }
        // 回合中：上一场遭遇结束 → 复位 Lua → clear_flag（旧世界不做这一步）
        if repeatable {
            if let Some((_tid, enc_name)) = active.pop_front() {
                let data = json!({ "encounter": { "id": "enc-sim", "name": enc_name, "scene_id": "sc-lmop-0b7" }, "reason": "all_down" });
                let env = MountEnv {
                    event: Some(LuaEventContext { name: "encounter_cleared", data: Some(&data) }),
                    ..Default::default()
                };
                for req in run(&host, reset, LuaMount::Event, &actx, &env) {
                    if let LuaRequest::ApplyEffect { effect, .. } = req {
                        if effect.get("kind").and_then(Value::as_str) == Some("set_flag") {
                            if let Some(flag) = effect.get("flag").and_then(Value::as_str) {
                                flags.insert(flag.to_string(), Value::Bool(false));
                            }
                        }
                    }
                }
            }
        }
        // 回合末：骨架求值（真实求值器）
        let eval_ctx = EvalContext {
            flags: &flags,
            goals: &goals,
            triggers: &triggers,
            actor_location: Some("loc-lmop-0b7"),
            actor_attributes: attributes_obj,
            relationships: &relationships,
            scene_id: Some("sc-lmop-0b7"),
            encounters: &encounters,
            lua: None,
        };
        let out = evaluate_skeleton_full(skeleton, &eval_ctx).expect("骨架求值失败");
        for (id, value) in out.trigger_progress {
            triggers.insert(id, value);
        }
        for id in out.triggers {
            *counts.entry(id.clone()).or_insert(0) += 1;
            if let Some(name) = encounter_name_of(skeleton, &id) {
                active.push_back((id, name));
            }
        }
    }
    counts
}

/// 触发点对应的预置遭遇名（复位 Lua 靠它反查标记）。
fn encounter_name_of(skeleton: &Value, trigger_id: &str) -> Option<String> {
    skeleton.as_array()?.iter().find_map(|chapter| {
        chapter.get("scenes")?.as_array()?.iter().find_map(|scene| {
            scene.get("triggers")?.as_array()?.iter().find_map(|trigger| {
                if trigger.get("id").and_then(Value::as_str) == Some(trigger_id) {
                    trigger
                        .pointer("/encounter/name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            })
        })
    })
}

/// 验收 4：XP 由 enemy_defeated 发放（导演即兴建的遭遇同样覆盖）。
fn xp_improvised_encounter(sb: &Value, a: &mut Assertions) {
    let source = mount_source(sb, "dnd-xp-award");
    let pc = character(sb, "pc-lmop-talin");
    let ctx = LuaHostContext {
        script_id: "lmop-check:xp".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: pc.clone(),
        ..Default::default()
    };
    let host = host_with(sb, 13);

    // 选一只**不在任何掷表行里**的怪物：掷表链路覆盖不到它，只有 enemy_defeated 能发。
    let wander_names: Vec<String> = definitions(sb, "dnd-encounter-table")
        .iter()
        .filter_map(|d| d.pointer("/fields/rows").and_then(Value::as_str).map(str::to_string))
        .collect();
    let improvised = sb
        .get("characters")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter().find(|c| {
                let name = c.get("name").and_then(Value::as_str).unwrap_or("");
                let xp = c.pointer("/statblock/xp").and_then(Value::as_i64).unwrap_or(0);
                xp > 0 && !wander_names.iter().any(|row| row.contains(name))
            })
        })
        .cloned();
    let Some(improvised) = improvised else {
        a.check("xp.improvised_encounter", false, "找不到「不在掷表里且有 XP」的怪物");
        return;
    };
    let template_id = improvised.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let creature_name = improvised.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let statblock_xp = improvised.pointer("/statblock/xp").and_then(Value::as_i64).unwrap_or(0);

    // ① 即兴遭遇（encounter.id 不是掷表建的，也没有任何 dnd-wander-* 标记）杀怪
    let data = json!({
        "enemy": { "id": "enc-improvised:0", "name": "即兴怪", "template_id": template_id, "instance_id": "inst-1" },
        "encounter": { "id": "enc-improvised", "name": "导演即兴遭遇", "scene_id": "sc-lmop-040", "location_id": "loc-lmop-040" },
        "killer": { "id": "inst-pc-lmop-talin", "name": "塔林·银溪" }
    });
    let env = MountEnv {
        event: Some(LuaEventContext { name: "enemy_defeated", data: Some(&data) }),
        ..Default::default()
    };
    let gains: Vec<i64> = run(&host, &source, LuaMount::Event, &ctx, &env)
        .iter()
        .filter_map(|r| match r {
            LuaRequest::ModifyResource { resource, amount, .. } if resource == "res-xp" => Some(*amount),
            _ => None,
        })
        .collect();

    // ② 其它事件（scene / encounter_cleared）不应发 XP
    let scene_env = MountEnv {
        event: Some(LuaEventContext { name: "scene", data: None }),
        ..Default::default()
    };
    let scene_req = run(&host, &source, LuaMount::Event, &ctx, &scene_env);
    let clear_env = MountEnv {
        event: Some(LuaEventContext {
            name: "encounter_cleared",
            data: Some(&json!({ "encounter": { "id": "enc-improvised" }, "enemies": [], "reason": "all_down" })),
        }),
        ..Default::default()
    };
    let clear_req = run(&host, &source, LuaMount::Event, &ctx, &clear_env);

    // ③ 逐只结算：再击败一只同类 → 再加一份
    let second = run(&host, &source, LuaMount::Event, &ctx, &env);
    let second_gain = second.iter().find_map(|r| match r {
        LuaRequest::ModifyResource { resource, amount, .. } if resource == "res-xp" => Some(*amount),
        _ => None,
    });

    let total: i64 = gains.iter().sum();
    let pass = gains.len() == 1
        && total == statblock_xp
        && statblock_xp > 0
        && scene_req.is_empty()
        && clear_req.is_empty()
        && second_gain == Some(statblock_xp);
    a.check(
        "xp.improvised_encounter",
        pass,
        format!(
            "即兴遭遇（encounter=enc-improvised）击败 {creature_name}({template_id}) → res-xp +{}（图鉴 statblock.xp={statblock_xp}）；\
             scene 事件请求数={}；encounter_cleared 请求数={}；再击败一只 → +{}",
            total,
            scene_req.len(),
            clear_req.len(),
            second_gain.map(|v| v.to_string()).unwrap_or_else(|| "无".into()),
        ),
    );
}

/// 开放内容里的 XP 必须与图鉴 statblock.xp 逐条相等（不复制、不漂移）。
fn bestiary_xp_matches(sb: &Value, a: &mut Assertions) {
    let defs = definitions(sb, "dnd-xp-award");
    let mut mismatches = Vec::new();
    let mut checked = 0usize;
    for character in sb.get("characters").and_then(Value::as_array).cloned().unwrap_or_default() {
        if character.get("kind").and_then(Value::as_str) != Some("monster") {
            continue;
        }
        let id = character.get("id").and_then(Value::as_str).unwrap_or("");
        let xp = character.pointer("/statblock/xp").and_then(Value::as_i64).unwrap_or(0);
        let def = defs.iter().find(|d| d.get("id").and_then(Value::as_str) == Some(&format!("xp-{id}")));
        match def.and_then(|d| d.pointer("/fields/xp")).and_then(Value::as_str) {
            Some(v) if v.parse::<i64>().ok() == Some(xp) => checked += 1,
            other => mismatches.push(format!("{id}: 定义={other:?} 图鉴={xp}")),
        }
    }
    a.check(
        "bestiary.xp_matches",
        mismatches.is_empty() && checked > 0,
        if mismatches.is_empty() {
            format!("{checked} 条模板的 dnd-xp-award 定义与图鉴 statblock.xp 逐条相等")
        } else {
            mismatches.join(" ; ")
        },
    );
}
