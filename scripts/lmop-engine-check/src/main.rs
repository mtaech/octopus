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
//! 断言清单（20 条；T27 发现①/③ 新增 8 条 + 「旧实况可执行复现」1 条）：
//!   proficiency.data_driven      改故事书 definition 的 bonus → 真实 Lua 给出的加值随之变
//!   sunlight.by_signature        只对 attack 与 (attribute ∧ wis) 取低，其它判定零请求
//!   encounter_table.refires      同一张表的同一行，在同一局里能触发两次以上（repeatable + clear_flag）；
//!                                内嵌反证组：不标 repeatable / 不复位时同一行整局只触发一次
//!   xp.improvised_encounter      导演即兴建的遭遇（非掷表）杀怪也发 XP；非击败事件不发
//!   bestiary.xp_matches          开放内容里的 XP 与图鉴 statblock.xp 逐条相等
//!   save_half.engine_scales_own_roll             同种子下因子 1.0 与 0.5 的实际 delta 关系
//!   save_half.resolved_effects_is_engine_snapshot PostResolve 读到的就是引擎算出的那一份
//!   pack_tactics.reads_world_facts               集群战术 8 类场景（FP-A/B/C/E、FN-A）
//!   pack_tactics.old_impl_reproduces_misjudgments 变异：旧脚本复现 5 类误判 → 反例有牙齿
//!   ambusher.reads_target_statuses               伏击按 host.target.statuses 判定
//!   xp.encounter_snapshot_fallback               事件缺 template_id 时按 instance_id 回查
//!   ambusher.check_signature                     T27 ①：attack → 给；attribute / save → 不给
//!   ambusher.old_impl_ignores_check_signature   旧脚本原文内嵌：同一批场景 1/1/1（旧实况仍可执行）
//!   ambusher.fail_closed_without_signature     签名缺失（无快照 / kind 缺省）→ 0；attack → 1
//!   ambusher.target_status_data_driven           期望状态 id 只来自开放内容 fields.target_status
//!   save_half.check_signature_gate               只对 kind == save 开缩放门（attack / attribute 零请求）
//!   save_half.rule_comes_from_open_content       改定义 skill_id → 规则整体不生效
//!   save_ends.reads_open_content                 改定义 dc → 结局翻转（DC 来自开放内容）
//!   xp.event_gate                                同一 payload 换事件名 → 零 XP
//!   xp.value_from_open_content                   改定义 xp → 发放值随之变

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::process::ExitCode;
use std::sync::Mutex;

use octopus_engine::conditions::evaluate_skeleton_full;
use octopus_engine::lua_host::{
    CheckModifier, LuaCheckContext, LuaEventContext, LuaHostContext, LuaMount, LuaRegistry,
    LuaStatusContext, MountEnv,
};
use octopus_engine::lua_lint::lint_storybook;
use octopus_engine::rng::DeterministicRng;
use octopus_engine::{
    execute_skill, validate_storybook_result, CommandContext, CommandOutcome, EvalContext, LuaHost, LuaRequest,
};
use octopus_types::{CheckKind, IssueSeverity, SkillDef};
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
    // T21 / T22 新增原语（GAP-A / GAP-E / GAP-L）
    save_half_engine_scales_own_roll(sb, a);
    save_half_resolved_effects_is_engine_snapshot(sb, a);
    pack_tactics_reads_world_facts(sb, a);
    pack_tactics_old_impl_reproduces_the_five_misjudgments(sb, a);
    ambusher_reads_target_statuses(sb, a);
    xp_encounter_snapshot_fallback(sb, a);
    // T27 发现①/③ 新增：伏击按判定签名 + 子串一致性断言的行为级兜底
    ambusher_check_signature(sb, a);
    ambusher_old_impl_ignores_check_signature(sb, a);
    ambusher_fail_closed_without_signature(sb, a);
    ambusher_target_status_data_driven(sb, a);
    save_half_check_signature_gate(sb, a);
    save_half_rule_comes_from_open_content(sb, a);
    save_ends_reads_open_content(sb, a);
    xp_event_gate(sb, a);
    xp_value_from_open_content(sb, a);
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


// ============================================================
// T21 / T22 原语的真实引擎验收（GAP-A / GAP-E / GAP-L）
// ============================================================

/// 把挂载点写请求里的 set_flag 收成 map（探针用它回传只读事实）。
fn flag_writes(requests: &[LuaRequest]) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for req in requests {
        if let LuaRequest::ApplyEffect { effect, .. } = req {
            if effect.get("kind").and_then(Value::as_str) == Some("set_flag") {
                if let Some(flag) = effect.get("flag").and_then(Value::as_str) {
                    out.insert(
                        flag.to_string(),
                        effect.get("value").cloned().unwrap_or(Value::Bool(true)),
                    );
                }
            }
        }
    }
    out
}

/// res-xp 的增量（没有就是 None）。
fn xp_gain(requests: &[LuaRequest]) -> Option<i64> {
    requests.iter().find_map(|r| match r {
        LuaRequest::ModifyResource { resource, amount, .. } if resource == "res-xp" => Some(*amount),
        _ => None,
    })
}

fn save_case_skill(sb: &Value) -> Option<SkillDef> {
    sb.get("skills")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter()
                .find(|s| s.get("id").and_then(Value::as_str) == Some("sk-lmop-rubble-collapse"))
        })
        .cloned()
        .and_then(|v| serde_json::from_value::<SkillDef>(v).ok())
}

fn sample_actor() -> Value {
    json!({
        "instance_id": "inst-save",
        "template_id": "pc-lmop-talin",
        "name": "塔林·银溪",
        "kind": "pc",
        "attributes": { "str": 10, "dex": 16, "con": 14, "int": 12, "wis": 13, "cha": 11 },
        "resources": { "res-hp": 24 },
        "statuses": [],
        "location_id": "loc-lmop-0b7"
    })
}

/// 跑一次真实的 `execute_skill`：规则包的脚本挂在 check_post_roll，PostResolve 挂一支探针
/// 把 `host.resolved_effects` 写成标记回传；豁免结果用 check_pre_roll 的通用原语钉死
/// （骰照掷、结果确定 → 与种子无关）。返回结算产物 + ctx.rng 的真实消耗序列。
fn run_save_case(sb: &Value, skill: &SkillDef, seed: u64, half_rule: &str) -> (CommandOutcome, Vec<u64>) {
    let host = host_with(sb, seed);
    let mut registry = LuaRegistry::new();
    registry.register("pin-save-success", LuaMount::CheckPreRoll, "host.modify_check('force_success')");
    registry.register("rule:save-half", LuaMount::CheckPostRoll, half_rule);
    registry.register(
        "audit:resolved-effects",
        LuaMount::PostResolve,
        r#"local e = host.resolved_effects
if e == nil then host.set_flag('audit-missing', true) return end
host.set_flag('audit-factor', tostring(e.factor))
host.set_flag('audit-rng', tostring(e.rng_consumed))
host.set_flag('audit-deltas', tostring(#e.deltas))
local first = e.deltas[1]
if first then
  host.set_flag('audit-field', tostring(first.field))
  host.set_flag('audit-value', tostring(first.value))
end"#,
    );
    let actor = sample_actor();
    let lua_ctx = LuaHostContext {
        script_id: "lmop-check:save-half".into(),
        actor_id: "inst-save".into(),
        actor: actor.clone(),
        target_id: Some("inst-save".into()),
        target: Some(actor.clone()),
        skill: serde_json::to_value(skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    let rng = Mutex::new(DeterministicRng::new(seed));
    let out = {
        let mut ctx = CommandContext {
            actor_id: "inst-save",
            actor: &actor,
            target_id: Some("inst-save"),
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
        execute_skill(skill, &mut ctx).expect("execute_skill 失败")
    };
    let consumed = rng.lock().unwrap().consumed.clone();
    (out, consumed)
}

/// GAP-E：规则包只声明一个因子，引擎只掷一次效果骰；同一颗骰在 0.5 下恰好是 1.0 的一半。
fn save_half_engine_scales_own_roll(sb: &Value, a: &mut Assertions) {
    let name = "save_half.engine_scales_own_roll";
    let rule = mount_source(sb, "dnd-save-half");
    let Some(skill) = save_case_skill(sb) else {
        a.check(name, false, "找不到 / 解析不了技能 sk-lmop-rubble-collapse");
        return;
    };
    // 同一种子、同一条技能，只换缩放因子：1.0 = 引擎全量结算（基线），规则包真实脚本 = 0.5。
    let (full, full_rng) = run_save_case(sb, &skill, 7, "host.scale_effect(1)");
    let (half, half_rng) = run_save_case(sb, &skill, 7, &rule);
    let full_v = full.deltas().first().and_then(|d| d.value.as_i64());
    let half_v = half.deltas().first().and_then(|d| d.value.as_i64());
    let expect_half = full_v.map(|v| (v as f64 * 0.5).trunc() as i64);
    let full_audit = flag_writes(&full.requests);
    let half_audit = flag_writes(&half.requests);
    let ok = full_rng == half_rng
        && full_rng.len() == 4
        && full.deltas().len() == 1
        && half.deltas().len() == 1
        && full_v.is_some()
        && half_v == expect_half
        && half_audit.get("audit-factor").and_then(Value::as_str) == Some("0.5")
        && half_audit.get("audit-rng").and_then(Value::as_str) == Some("3")
        && full_audit.get("audit-missing").is_none();
    a.check(
        name,
        ok,
        format!(
            "同种子：因子 1.0 → delta {full_v:?}（ctx.rng {full_rng:?}）；规则包脚本 scale_effect(0.5) → delta {half_v:?}（ctx.rng {half_rng:?}）；             half == trunc(full×0.5)={}；引擎快照 factor={:?} rng_consumed={:?}（3 = 一次 3d6，Lua 未重掷）",
            half_v == expect_half,
            half_audit.get("audit-factor"),
            half_audit.get("audit-rng"),
        ),
    );
}

/// GAP-E 核对：PostResolve 读到的 host.resolved_effects 就是引擎实际算出的那一份
/// （factor / 效果骰数 / delta 字段与值都与提交值逐字一致）。
fn save_half_resolved_effects_is_engine_snapshot(sb: &Value, a: &mut Assertions) {
    let name = "save_half.resolved_effects_is_engine_snapshot";
    let rule = mount_source(sb, "dnd-save-half");
    let Some(skill) = save_case_skill(sb) else {
        a.check(name, false, "找不到 / 解析不了技能 sk-lmop-rubble-collapse");
        return;
    };
    let (half, half_rng) = run_save_case(sb, &skill, 11, &rule);
    let audit = flag_writes(&half.requests);
    let committed = half.deltas().first().map(|d| d.value.to_string());
    let audited = audit.get("audit-value").and_then(Value::as_str).map(str::to_string);
    let ok = half_rng.len() == 4
        && audit.get("audit-factor").and_then(Value::as_str) == Some("0.5")
        && audit.get("audit-rng").and_then(Value::as_str) == Some("3")
        && audit.get("audit-deltas").and_then(Value::as_str) == Some("1")
        && audit.get("audit-field").and_then(Value::as_str) == Some("resources.res-hp")
        && audited.is_some()
        && audited == committed;
    a.check(
        name,
        ok,
        format!(
            "PostResolve 读 host.resolved_effects：factor={:?} rng_consumed={:?} #deltas={:?} field={:?} value={:?}；             与提交的 delta {committed:?} 逐字一致={}",
            audit.get("audit-factor"),
            audit.get("audit-rng"),
            audit.get("audit-deltas"),
            audit.get("audit-field"),
            audit.get("audit-value"),
            audited == committed,
        ),
    );
}

/// GAP-A（T26 修正轮后）：集群战术读运行时事实，实现与开放内容声明一致。
/// 正例 + 5 类历史误判的**引擎级反例**（FP-A 同伴异地 / FP-B 同伴失能 / FP-C 无地点 /
/// FP-E 非攻击判定 / FN-A 同伴在别的遭遇）+ 同伴倒下 / 无挂接。
fn pack_tactics_reads_world_facts(sb: &Value, a: &mut Assertions) {
    let name = "pack_tactics.reads_world_facts";
    let c = run_pack_cases(sb, &mount_source(sb, "dnd-pack-tactics"));
    let ok = c.positive == 1
        && c.ally_down == 0
        && c.fp_a == 0
        && c.fp_b == 0
        && c.fp_c == 0
        && c.fp_e == 0
        && c.fn_a == 1
        && c.no_trait == 0;
    a.check(
        name,
        ok,
        format!(
            "同伴与目标同地点 → keep_high={}（期望 1）；同伴 HP=0 → {}；\
             FP-A 同伴在别的地点 → {}；FP-B 同伴 stunned → {}；FP-C 双方无 location_id → {}；\
             FP-E 属性检定 → {}；FN-A 同伴在另一场遭遇（同地点）→ {}（期望 1）；无集群战术挂接 → {}（反例均期望 0）。\
             注：这是「与目标同 location_id + 开放内容失能名单」近似，精确 5 尺仍做不到（GAP-B）",
            c.positive, c.ally_down, c.fp_a, c.fp_b, c.fp_c, c.fp_e, c.fn_a, c.no_trait
        ),
    );
}

/// **变异验证**：把集群战术换回 T26 修正轮之前的旧脚本，同一批反例必须复现出 5 类**错误结论**
/// （FP-A/B/C/E → 误给优势 = 1，FN-A → 漏判 = 0）。这证明 `pack_tactics.reads_world_facts`
/// 的反例不是恒真——旧实现真的会被它们抓住。
fn pack_tactics_old_impl_reproduces_the_five_misjudgments(sb: &Value, a: &mut Assertions) {
    let name = "pack_tactics.old_impl_reproduces_misjudgments";
    let c = run_pack_cases(sb, OLD_PACK_TACTICS);
    let fixed = run_pack_cases(sb, &mount_source(sb, "dnd-pack-tactics"));
    let ok = c.positive == 1
        && c.fp_a == 1
        && c.fp_b == 1
        && c.fp_c == 1
        && c.fp_e == 1
        && c.fn_a == 0
        && fixed.fp_a == 0
        && fixed.fp_b == 0
        && fixed.fp_c == 0
        && fixed.fp_e == 0
        && fixed.fn_a == 1;
    a.check(
        name,
        ok,
        format!(
            "同一批数据：旧脚本 FP-A 同伴异地={} / FP-B 同伴失能={} / FP-C 无地点={} / FP-E 属性检定={}（均误给 = 1）、\
             FN-A 跨遭遇={}（漏判 = 0）；新脚本对应 {} / {} / {} / {} / {}（已修正）。旧结论被反例复现 → 反例有牙齿",
            c.fp_a, c.fp_b, c.fp_c, c.fp_e, c.fn_a,
            fixed.fp_a, fixed.fp_b, fixed.fp_c, fixed.fp_e, fixed.fn_a
        ),
    );
}

/// T26 修正轮**之前**的集群战术脚本（git HEAD 原文照抄，用于变异审计）：只查同伴 HP、
/// 只比「狼 vs 目标」的地点，从不看同伴位置 / 判定签名，且只在我所在的那场遭遇里找同伴。
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

/// 跑同一批反例（正例 / 同伴倒下 / FP-A / FP-B / FP-C / FP-E / FN-A / 无挂接），
/// 脚本正文由调用方给（真实规则包 or 旧脚本变异）。
struct PackCases {
    positive: usize,
    ally_down: usize,
    fp_a: usize,
    fp_b: usize,
    fp_c: usize,
    fp_e: usize,
    fn_a: usize,
    no_trait: usize,
}

fn run_pack_cases(sb: &Value, source: &str) -> PackCases {
    let wolf = |key: &str, hp: i64, place: Option<&str>, statuses: Value| {
        json!({
            "instance_id": key, "template_id": "mon-wolf", "name": "狼", "kind": "monster",
            "attributes": {}, "resources": { "res-hp": hp }, "statuses": statuses,
            "location_id": place
        })
    };
    let pc_at = |place: Option<&str>| {
        json!({
            "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林", "kind": "pc",
            "attributes": {}, "resources": { "res-hp": 24 }, "statuses": [], "location_id": place
        })
    };
    let member = |key: &str, hp: i64| {
        json!({ "id": key, "instance_id": key, "template_id": "mon-wolf", "hp": hp })
    };
    let facts = |chars: Vec<Value>, encounters: Value| {
        let mut map = Map::new();
        for c in chars {
            map.insert(c["instance_id"].as_str().unwrap().to_string(), c);
        }
        json!({ "characters": map, "flags": {}, "encounters": encounters })
    };
    let one_encounter = |members: Value| {
        json!({ "enc-1": { "id": "enc-1", "name": "野外遭遇", "active": true, "enemies": members } })
    };
    // 我（inst-wolf-1，loc-a）+ 同伴（inst-wolf-2，地点 / 状态由调用方指定），同处一场遭遇。
    let two_at = |place: Option<&str>, statuses: Value| {
        facts(
            vec![
                wolf("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf("inst-wolf-2", 11, place, statuses),
            ],
            one_encounter(json!([member("inst-wolf-1", 11), member("inst-wolf-2", 11)])),
        )
    };

    let run_pack = |facts_value: Value, actor: Value, target_place: Option<&str>, kind: CheckKind| -> usize {
        let host = host_with(sb, 5);
        host.set_world_facts(facts_value);
        let ctx = LuaHostContext {
            script_id: "lmop-check:pack".into(),
            actor_id: "inst-wolf-1".into(),
            actor,
            target_id: Some("inst-pc".into()),
            target: Some(pc_at(target_place)),
            ..Default::default()
        };
        let sig = signature("str", kind, 12);
        let env = MountEnv { check: Some(&sig), ..Default::default() };
        modifies(&run(&host, source, LuaMount::CheckPreRoll, &ctx, &env))
            .iter()
            .filter(|(m, _)| *m == CheckModifier::KeepHigh)
            .count()
    };
    let wolf_actor = || wolf("inst-wolf-1", 11, Some("loc-a"), json!([]));

    // P0 正例：同伴与**目标**同地点、未失能 → 优势。
    let positive = run_pack(two_at(Some("loc-a"), json!([])), wolf_actor(), Some("loc-a"), CheckKind::Attack);
    // 同伴倒下（HP=0）→ 不算。
    let ally_down = run_pack(
        facts(
            vec![
                wolf("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf("inst-wolf-2", 0, Some("loc-a"), json!([])),
            ],
            one_encounter(json!([member("inst-wolf-1", 11), member("inst-wolf-2", 0)])),
        ),
        wolf_actor(),
        Some("loc-a"),
        CheckKind::Attack,
    );
    // FP-A：同伴在**别的地点**（旧实现从不检查同伴位置 → 误给优势）。
    let fp_a = run_pack(two_at(Some("loc-c"), json!([])), wolf_actor(), Some("loc-a"), CheckKind::Attack);
    // FP-B：同伴 HP>0 但带开放内容声明的失能状态（stunned）。
    let fp_b = run_pack(
        two_at(Some("loc-a"), json!([{ "id": "stunned", "name": "昏迷" }])),
        wolf_actor(),
        Some("loc-a"),
        CheckKind::Attack,
    );
    // FP-C：双方都没有 location_id（旧实现整道地点闸门被跳过 → 误给优势）。
    let fp_c = run_pack(
        facts(
            vec![
                wolf("inst-wolf-1", 11, None, json!([])),
                wolf("inst-wolf-2", 11, None, json!([])),
            ],
            one_encounter(json!([member("inst-wolf-1", 11), member("inst-wolf-2", 11)])),
        ),
        wolf("inst-wolf-1", 11, None, json!([])),
        None,
        CheckKind::Attack,
    );
    // FP-E：非攻击判定（属性检定）不给优势。
    let fp_e = run_pack(two_at(Some("loc-a"), json!([])), wolf_actor(), Some("loc-a"), CheckKind::Attribute);
    // FN-A：同伴在**另一场遭遇**，但同在目标的地点 → 必须算（旧实现漏判）。
    let fn_a = run_pack(
        facts(
            vec![
                wolf("inst-wolf-1", 11, Some("loc-a"), json!([])),
                wolf("inst-wolf-2", 11, Some("loc-a"), json!([])),
            ],
            json!({
                "enc-1": { "id": "enc-1", "name": "遭遇一", "active": true, "enemies": [member("inst-wolf-1", 11)] },
                "enc-2": { "id": "enc-2", "name": "遭遇二", "active": true, "enemies": [member("inst-wolf-2", 11)] }
            }),
        ),
        wolf_actor(),
        Some("loc-a"),
        CheckKind::Attack,
    );
    // 无集群战术挂接：同一切数据，actor 换成没有挂接的 PC。
    let no_trait = run_pack(
        two_at(Some("loc-a"), json!([])),
        json!({ "instance_id": "inst-wolf-1", "template_id": "pc-lmop-talin", "name": "旁观者", "kind": "pc",
                "attributes": {}, "resources": { "res-hp": 24 }, "statuses": [], "location_id": "loc-a" }),
        Some("loc-a"),
        CheckKind::Attack,
    );

    PackCases { positive, ally_down, fp_a, fp_b, fp_c, fp_e, fn_a, no_trait }
}

/// GAP-L：伏击直接读 host.target.statuses（不再靠 dnd-surprised 全场标记）。
fn ambusher_reads_target_statuses(sb: &Value, a: &mut Assertions) {
    let name = "ambusher.reads_target_statuses";
    let source = mount_source(sb, "dnd-ambusher-keep-high");
    // T26 修正轮起脚本带 kind == attack 闸门：这里显式给攻击判定签名（判定签名缺省会被早退）。
    let sig = signature("str", CheckKind::Attack, 12);
    let env = MountEnv { check: Some(&sig), ..Default::default() };
    let run_amb = |actor_template: &str, statuses: Value| -> usize {
        let host = host_with(sb, 5);
        let actor = json!({ "instance_id": "inst-amb", "template_id": actor_template, "name": "袭击者",
            "kind": "monster", "attributes": {}, "resources": { "res-hp": 22 }, "statuses": [] });
        let target = json!({ "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林",
            "kind": "pc", "attributes": {}, "resources": { "res-hp": 24 }, "statuses": statuses });
        let ctx = LuaHostContext {
            script_id: "lmop-check:ambush".into(),
            actor_id: "inst-amb".into(),
            actor,
            target_id: Some("inst-pc".into()),
            target: Some(target),
            ..Default::default()
        };
        modifies(&run(&host, &source, LuaMount::CheckPreRoll, &ctx, &env))
            .iter()
            .filter(|(m, _)| *m == CheckModifier::KeepHigh)
            .count()
    };
    let with_status = run_amb("mon-doppelganger", json!([{ "id": "dnd-surprised", "name": "受突袭" }]));
    let without = run_amb("mon-doppelganger", json!([]));
    let other_status = run_amb("mon-doppelganger", json!([{ "id": "dnd-prone", "name": "倒地" }]));
    let no_trait = run_amb("mon-wolf", json!([{ "id": "dnd-surprised", "name": "受突袭" }]));
    let ok = with_status == 1 && without == 0 && other_status == 0 && no_trait == 0;
    a.check(
        name,
        ok,
        format!(
            "target.statuses 含受突袭 → keep_high={with_status}（期望 1）；无状态 → {without}；别的状态 → {other_status}；             袭击者无伏击挂接 → {no_trait}（反例均期望 0）。期望状态 id 来自开放内容 fields.target_status"
        ),
    );
}

// ============================================================
// T27 发现①/③ 的行为级兜底（T26 修正轮）
// ============================================================

/// 判定签名（掷骰后）：结果字段可用（check_result / check_total）。
fn resolved_signature(attribute: &str, kind: CheckKind, result: bool) -> LuaCheckContext {
    LuaCheckContext {
        attribute: attribute.to_string(),
        kind: Some(kind),
        resolved: true,
        result,
        target: 10,
        ..Default::default()
    }
}

/// 缩放请求里的因子列表（只有 check_post_roll / pre_resolve 会写这条请求）。
fn scale_factors(requests: &[LuaRequest]) -> Vec<f64> {
    requests
        .iter()
        .filter_map(|r| match r {
            LuaRequest::ScaleEffect { factor } => Some(*factor),
            _ => None,
        })
        .collect()
}

/// 直接跑 dnd-save-half 脚本（不经 execute_skill）：判定签名与结果由调用方钉死，
/// 于是「脚本对哪一类判定开门」这件事可以被单独观察。
fn run_save_half_rule(
    sb: &Value,
    source: &str,
    skill: &SkillDef,
    kind: CheckKind,
    result: bool,
) -> Vec<LuaRequest> {
    let host = host_with(sb, 3);
    let actor = sample_actor();
    let ctx = LuaHostContext {
        script_id: "lmop-check:save-half-gate".into(),
        actor_id: "inst-save".into(),
        actor: actor.clone(),
        target_id: Some("inst-save".into()),
        target: Some(actor),
        skill: serde_json::to_value(skill).ok(),
        difficulty: Some(10),
        ..Default::default()
    };
    let sig = resolved_signature("dex", kind, result);
    let env = MountEnv { check: Some(&sig), ..Default::default() };
    run(&host, source, LuaMount::CheckPostRoll, &ctx, &env)
}

/// 伏击的三个判定签名场景（对**给定脚本正文**跑同一批数据）：
/// 返回 (attack, attribute, save) 各自的 keep_high 数。
fn run_ambusher_signature_cases(sb: &Value, source: &str) -> (usize, usize, usize) {
    let run_amb = |kind: CheckKind| -> usize {
        let host = host_with(sb, 5);
        let actor = json!({ "instance_id": "inst-amb", "template_id": "mon-doppelganger", "name": "袭击者",
            "kind": "monster", "attributes": {}, "resources": { "res-hp": 22 }, "statuses": [] });
        let target = json!({ "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林",
            "kind": "pc", "attributes": { "dex": 16 }, "resources": { "res-hp": 24 },
            "statuses": [{ "id": "dnd-surprised", "name": "受突袭" }] });
        let ctx = LuaHostContext {
            script_id: "lmop-check:ambush-signature".into(),
            actor_id: "inst-amb".into(),
            actor,
            target_id: Some("inst-pc".into()),
            target: Some(target),
            ..Default::default()
        };
        let sig = signature("str", kind, 12);
        let env = MountEnv { check: Some(&sig), ..Default::default() };
        modifies(&run(&host, source, LuaMount::CheckPreRoll, &ctx, &env))
            .iter()
            .filter(|(m, _)| *m == CheckModifier::KeepHigh)
            .count()
    };
    (run_amb(CheckKind::Attack), run_amb(CheckKind::Attribute), run_amb(CheckKind::Save))
}

/// T27 发现①（第 5 轮修复）：伏击与集群战术**同口径**——只对攻击检定给优势。
/// 交付脚本：attack → 给；attribute / save → 不给（数据卡只说「攻击检定」）。
fn ambusher_check_signature(sb: &Value, a: &mut Assertions) {
    let name = "ambusher.check_signature";
    let source = mount_source(sb, "dnd-ambusher-keep-high");
    let (attack, attribute, save) = run_ambusher_signature_cases(sb, &source);
    let ok = attack == 1 && attribute == 0 && save == 0;
    a.check(
        name,
        ok,
        format!(
            "数据卡只说「攻击检定」：attack → keep_high={attack}（期望 1）；attribute → {attribute}；save → {save}（反例均期望 0）。\
             三条一起钉住两端：把 check_kind == attack 闸门去掉，attribute / save 立刻回到 1（见 ambusher.old_impl_ignores_check_signature）"
        ),
    );
}

/// **旧实况必须仍然「可执行地」存在**（T27 定的安全前提）：本断言内嵌 git HEAD 的旧伏击脚本
/// **原文**（OLD_AMBUSHER），用同一批场景复现 T27 的三条原始发现——attack / attribute / save
/// **全部**给优势。这样「伏击不看判定签名会误给优势」这件事不依赖任何会被翻转的断言而存活。
fn ambusher_old_impl_ignores_check_signature(sb: &Value, a: &mut Assertions) {
    let name = "ambusher.old_impl_ignores_check_signature";
    let (attack, attribute, save) = run_ambusher_signature_cases(sb, OLD_AMBUSHER);
    let ok = attack == 1 && attribute == 1 && save == 1;
    a.check(
        name,
        ok,
        format!(
            "旧脚本（git HEAD 原文，无判定签名闸门）：attack → keep_high={attack} / attribute → {attribute} / save → {save}（均期望 1）\
             = T27 原始发现① 的可执行复现；新脚本同一批场景为 1 / 0 / 0（ambusher.check_signature）"
        ),
    );
}

/// 显式钉住的边界：**判定签名缺失就 fail-closed**（不给优势）。
///
/// 为什么这不是「旧 harness 的坑」而是正确行为：真实引擎里每一次**真判定**的 check_pre_roll
/// 都会拿到签名——`command.rs` 的 `check_signature()` 永远写 `kind: Some(kind)`（L160-167），
/// 技能路径的 kind 在掷骰前解析、缺省 `CheckKind::Attribute`（L466-494），session 路径同样缺省
/// Attribute 且无条件传 `Some(&signature)`（session.rs L5149-5160）；引擎既有测试
/// `pre_roll_sees_check_signature_in_skill_path`（command.rs L1532-1559）断言签名在掷骰前可见，
/// `pre_roll_has_no_signature_without_a_check`（L1562+）断言「没有判定就没有签名」。
/// 所以「拿不到签名」只对应「根本没有这次判定」——那时规则必须早退。
/// 本断言把这条从「让 round3 旧 harness 意外变红的坑」变成被明确钉住的行为。
fn ambusher_fail_closed_without_signature(sb: &Value, a: &mut Assertions) {
    let name = "ambusher.fail_closed_without_signature";
    let source = mount_source(sb, "dnd-ambusher-keep-high");
    let run_with_env = |env: &MountEnv<'_>| -> usize {
        let host = host_with(sb, 5);
        let actor = json!({ "instance_id": "inst-amb", "template_id": "mon-doppelganger", "name": "袭击者",
            "kind": "monster", "attributes": {}, "resources": { "res-hp": 22 }, "statuses": [] });
        let target = json!({ "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林",
            "kind": "pc", "attributes": { "dex": 16 }, "resources": { "res-hp": 24 },
            "statuses": [{ "id": "dnd-surprised", "name": "受突袭" }] });
        let ctx = LuaHostContext {
            script_id: "lmop-check:ambush-fail-closed".into(),
            actor_id: "inst-amb".into(),
            actor,
            target_id: Some("inst-pc".into()),
            target: Some(target),
            ..Default::default()
        };
        modifies(&run(&host, &source, LuaMount::CheckPreRoll, &ctx, env))
            .iter()
            .filter(|(m, _)| *m == CheckModifier::KeepHigh)
            .count()
    };
    // ① 没有判定快照（= 根本没有判定）→ 不给。
    let no_snapshot = run_with_env(&MountEnv::default());
    // ② 有快照但 kind 缺省（脚本读到 nil）→ 同样不给。
    let kindless = LuaCheckContext { attribute: "str".into(), kind: None, target: 12, ..Default::default() };
    let no_kind = run_with_env(&MountEnv { check: Some(&kindless), ..Default::default() });
    // 对照：换攻击签名 → 给（同一 harness，排除「恒 0」）。
    let sig = signature("str", CheckKind::Attack, 12);
    let attack = run_with_env(&MountEnv { check: Some(&sig), ..Default::default() });
    let ok = no_snapshot == 0 && no_kind == 0 && attack == 1;
    a.check(
        name,
        ok,
        format!(
            "目标带 dnd-surprised：没有判定快照（host.check == nil）→ keep_high={no_snapshot}（期望 0）；\
             有快照但 kind 缺省（host.check_kind == nil）→ {no_kind}（期望 0）—— 签名缺失一律 fail-closed；\
             同一 harness 换 attack 签名 → {attack}（期望 1，排除「恒 0」）。\
             证据：command.rs L160-167 check_signature 恒写 kind: Some(...)，注入点 command.rs L466-494 / session.rs L5149-5160，\
             既有测试 pre_roll_sees_check_signature_in_skill_path / pre_roll_has_no_signature_without_a_check"
        ),
    );
}

/// T27 发现① 之前（即本仓库 HEAD 版 story_example/lmop-storybook.json 里 dnd-ambusher-keep-high）
/// 的伏击脚本**原文照抄**，只用于「旧实况可执行复现」，不参与任何交付路径。
const OLD_AMBUSHER: &str = r#"-- 规则包：伏击（变形怪，附录 B 原文）：战斗开始的第一轮里，
-- 对任何成功受其突袭的生物所发动的攻击检定具有优势。
-- T21：host.target 与 host.actor 同级完整（含 statuses）——直接按目标状态判定（GAP-L 闭合）。
-- 数据来源：开放内容（角色模板的 dnd-ambusher 挂接 → fields.target_status），脚本无状态名常量。
local ids = host.get_attachment('dnd-ambusher')
if type(ids) ~= "table" or #ids == 0 then return end
local target = host.target
local statuses = target and target.statuses
if type(statuses) ~= "table" then return end
for _, def_id in ipairs(ids) do
  local def = host.get_definition(def_id)
  local wanted = def and def.fields and def.fields.target_status
  if wanted and wanted ~= "" then
    for _, st in ipairs(statuses) do
      if st.id == wanted or st.name == wanted then
        host.modify_check('keep_high')
        return
      end
    end
  end
end"#;

/// T27 发现③的行为级兜底：伏击的期望状态 id 只在开放内容里——
/// 只改定义 fields.target_status，同一份 Lua 的结论随之翻转。
fn ambusher_target_status_data_driven(sb: &Value, a: &mut Assertions) {
    let name = "ambusher.target_status_data_driven";
    let source = mount_source(sb, "dnd-ambusher-keep-high");
    let run_with = |storybook: &Value, status_id: &str| -> usize {
        let host = host_with(storybook, 5);
        let actor = json!({ "instance_id": "inst-amb", "template_id": "mon-doppelganger", "name": "袭击者",
            "kind": "monster", "attributes": {}, "resources": { "res-hp": 22 }, "statuses": [] });
        let target = json!({ "instance_id": "inst-pc", "template_id": "pc-lmop-talin", "name": "塔林",
            "kind": "pc", "attributes": { "dex": 16 }, "resources": { "res-hp": 24 },
            "statuses": [{ "id": status_id, "name": status_id }] });
        let ctx = LuaHostContext {
            script_id: "lmop-check:ambush-data".into(),
            actor_id: "inst-amb".into(),
            actor,
            target_id: Some("inst-pc".into()),
            target: Some(target),
            ..Default::default()
        };
        let sig = signature("str", CheckKind::Attack, 12);
        let env = MountEnv { check: Some(&sig), ..Default::default() };
        modifies(&run(&host, &source, LuaMount::CheckPreRoll, &ctx, &env))
            .iter()
            .filter(|(m, _)| *m == CheckModifier::KeepHigh)
            .count()
    };
    let base_declared = run_with(sb, "dnd-surprised");
    let base_other = run_with(sb, "dnd-prone");
    let mut mutated = sb.clone();
    if let Some(defs) = mutated.get_mut("definitions").and_then(Value::as_array_mut) {
        for def in defs.iter_mut() {
            if def.get("kind").and_then(Value::as_str) == Some("dnd-ambusher") {
                def["fields"]["target_status"] = Value::String("dnd-prone".into());
            }
        }
    }
    let mutated_declared = run_with(&mutated, "dnd-prone");
    let mutated_other = run_with(&mutated, "dnd-surprised");
    let ok =
        base_declared == 1 && base_other == 0 && mutated_declared == 1 && mutated_other == 0;
    a.check(
        name,
        ok,
        format!(
            "基线（定义 target_status=dnd-surprised）：带 dnd-surprised → {base_declared}（期望 1）、带 dnd-prone → {base_other}（期望 0）；\
             只把定义改成 dnd-prone（脚本逐字不动）：带 dnd-prone → {mutated_declared}（期望 1）、带 dnd-surprised → {mutated_other}（期望 0）\
             → 状态 id 真的来自开放内容，不是烘在 Lua 里的常量"
        ),
    );
}

/// T27 发现③的行为级兜底：豁免减半只对**豁免判定**开门（check_kind == save）。
/// 同一份脚本换 Attack / Attribute 判定 → 零请求；失败分支仍按开放内容补 fail_status。
fn save_half_check_signature_gate(sb: &Value, a: &mut Assertions) {
    let name = "save_half.check_signature_gate";
    let source = mount_source(sb, "dnd-save-half");
    let Some(skill) = save_case_skill(sb) else {
        a.check(name, false, "找不到 / 解析不了技能 sk-lmop-rubble-collapse");
        return;
    };
    let save_success = run_save_half_rule(sb, &source, &skill, CheckKind::Save, true);
    let save_fail = run_save_half_rule(sb, &source, &skill, CheckKind::Save, false);
    let attack_success = run_save_half_rule(sb, &source, &skill, CheckKind::Attack, true);
    let attribute_success = run_save_half_rule(sb, &source, &skill, CheckKind::Attribute, true);
    let scale_success = scale_factors(&save_success);
    let scale_fail = scale_factors(&save_fail);
    let fail_status = save_fail
        .iter()
        .any(|r| matches!(r, LuaRequest::ApplyStatus { status, .. } if status == "dnd-prone"));
    let ok = scale_success == vec![0.5]
        && scale_fail.is_empty()
        && attack_success.is_empty()
        && attribute_success.is_empty()
        && fail_status;
    a.check(
        name,
        ok,
        format!(
            "同一份 dnd-save-half 脚本按判定签名分派：save+成功 → scale_effect{scale_success:?}（期望 [0.5]）；\
             save+失败 → scale_effect{scale_fail:?}（不缩放）且补 fail_status={fail_status}；\
             attack / attribute → 请求数 {} / {}（期望 0：脚本按 check_kind ~= 'save' 早退）",
            attack_success.len(),
            attribute_success.len()
        ),
    );
}

/// T27 发现③的行为级兜底：规则只对开放内容 dnd-save-half 声明的 skill_id 生效——
/// 只改定义的 skill_id，同一技能上的缩放整体消失。
fn save_half_rule_comes_from_open_content(sb: &Value, a: &mut Assertions) {
    let name = "save_half.rule_comes_from_open_content";
    let source = mount_source(sb, "dnd-save-half");
    let Some(skill) = save_case_skill(sb) else {
        a.check(name, false, "找不到 / 解析不了技能 sk-lmop-rubble-collapse");
        return;
    };
    let base = scale_factors(&run_save_half_rule(sb, &source, &skill, CheckKind::Save, true));
    let mut mutated = sb.clone();
    if let Some(defs) = mutated.get_mut("definitions").and_then(Value::as_array_mut) {
        for def in defs.iter_mut() {
            if def.get("kind").and_then(Value::as_str) == Some("dnd-save-half") {
                def["fields"]["skill_id"] = Value::String("sk-nonexistent".into());
            }
        }
    }
    let orphan =
        scale_factors(&run_save_half_rule(&mutated, &source, &skill, CheckKind::Save, true));
    let ok = base == vec![0.5] && orphan.is_empty();
    a.check(
        name,
        ok,
        format!(
            "定义 savehalf-rubble.fields.skill_id = sk-lmop-rubble-collapse → 该技能上 scale_effect{base:?}（期望 [0.5]）；\
             只把 skill_id 改成 sk-nonexistent（脚本逐字不动）→ scale_effect{orphan:?}（期望空）\
             → 规则命中的技能真的由开放内容决定"
        ),
    );
}

/// T27 发现③的行为级兜底：重复豁免的 DC 只活在开放内容 dnd-save-ends 定义里——
/// 只改 dc，同一份脚本的结局随之翻转（dc=1 必成功移除 / dc=99 必失败保留）。
fn save_ends_reads_open_content(sb: &Value, a: &mut Assertions) {
    let name = "save_ends.reads_open_content";
    let source = mount_source(sb, "dnd-save-ends");
    let run_with_dc = |storybook: &Value, dc: i64| -> bool {
        let mut mutated = storybook.clone();
        if let Some(defs) = mutated.get_mut("definitions").and_then(Value::as_array_mut) {
            for def in defs.iter_mut() {
                if def.get("kind").and_then(Value::as_str) == Some("dnd-save-ends")
                    && def.pointer("/fields/status_id").and_then(Value::as_str)
                        == Some("dnd-eruption-penalty")
                {
                    def["fields"]["dc"] = Value::String(dc.to_string());
                }
            }
        }
        let host = host_with(&mutated, 9);
        let ctx = LuaHostContext {
            script_id: "lmop-check:save-ends".into(),
            actor_id: "inst-save".into(),
            actor: sample_actor(),
            ..Default::default()
        };
        let status = LuaStatusContext {
            id: "dnd-eruption-penalty".into(),
            name: "灰烬呛咳".into(),
            turns_left: Some(1),
            scenes_left: None,
            unit: "turns",
            remaining: 0,
        };
        host.run_hook_status(&source, LuaMount::StatusTick, &ctx, &MountEnv::default(), &status)
            .expect("重复豁免脚本执行失败");
        host.drain_requests().iter().any(|r| {
            matches!(r, LuaRequest::RemoveStatus { status, .. } if status == "dnd-eruption-penalty")
        })
    };
    let easy = run_with_dc(sb, 1);
    let hard = run_with_dc(sb, 99);
    let ok = easy && !hard;
    a.check(
        name,
        ok,
        format!(
            "只改开放内容 dnd-save-ends 定义里的 dc（脚本逐字不动）：dc=1 → 本回合移除状态={easy}（期望 true）；\
             dc=99 → 移除={hard}（期望 false：20 面骰 + con 调整值不可能够）→ DC 真的来自开放内容"
        ),
    );
}

/// T27 发现③的行为级兜底：XP 只在 enemy_defeated 事件发放——
/// 同一份 payload 换事件名（scene / encounter_cleared）必须零请求。
fn xp_event_gate(sb: &Value, a: &mut Assertions) {
    let name = "xp.event_gate";
    let source = mount_source(sb, "dnd-xp-award");
    let expected = character(sb, "mon-ash-zombie")
        .pointer("/statblock/xp")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let pay = |event: &str| -> Option<i64> {
        let host = host_with(sb, 13);
        let ctx = LuaHostContext {
            script_id: "lmop-check:xp-gate".into(),
            actor_id: "inst-pc-lmop-talin".into(),
            actor: character(sb, "pc-lmop-talin"),
            ..Default::default()
        };
        let data = json!({
            "enemy": { "id": "e1", "name": "灰烬丧尸", "template_id": "mon-ash-zombie" },
            "encounter": { "id": "enc-x", "name": "洞穴" }
        });
        let env = MountEnv {
            event: Some(LuaEventContext { name: event, data: Some(&data) }),
            ..Default::default()
        };
        xp_gain(&run(&host, &source, LuaMount::Event, &ctx, &env))
    };
    let hit = pay("enemy_defeated");
    let scene = pay("scene");
    let cleared = pay("encounter_cleared");
    let ok = expected > 0 && hit == Some(expected) && scene.is_none() && cleared.is_none();
    a.check(
        name,
        ok,
        format!(
            "同一份 data.enemy（mon-ash-zombie）：enemy_defeated → res-xp {hit:?}（期望 {expected}）；\
             scene → {scene:?}；encounter_cleared → {cleared:?}（期望都不发）→ event_name 闸门真的在起作用"
        ),
    );
}

/// T27 发现③的行为级兜底：XP 数值只在开放内容 dnd-xp-award 定义里——
/// 只改定义的 fields.xp，同一事件的发放随之变。
fn xp_value_from_open_content(sb: &Value, a: &mut Assertions) {
    let name = "xp.value_from_open_content";
    let source = mount_source(sb, "dnd-xp-award");
    let declared = definitions(sb, "dnd-xp-award")
        .iter()
        .find(|d| d.get("id").and_then(Value::as_str) == Some("xp-mon-ash-zombie"))
        .and_then(|d| d.pointer("/fields/xp"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok());
    let Some(declared) = declared else {
        a.check(name, false, "故事书里找不到 xp-mon-ash-zombie 的 fields.xp");
        return;
    };
    let pay = |storybook: &Value| -> Option<i64> {
        let host = host_with(storybook, 13);
        let ctx = LuaHostContext {
            script_id: "lmop-check:xp-value".into(),
            actor_id: "inst-pc-lmop-talin".into(),
            actor: character(storybook, "pc-lmop-talin"),
            ..Default::default()
        };
        let data = json!({ "enemy": { "id": "e1", "name": "灰烬丧尸", "template_id": "mon-ash-zombie" } });
        let env = MountEnv {
            event: Some(LuaEventContext { name: "enemy_defeated", data: Some(&data) }),
            ..Default::default()
        };
        xp_gain(&run(&host, &source, LuaMount::Event, &ctx, &env))
    };
    let base = pay(sb);
    let bumped = declared + 7;
    let mut mutated = sb.clone();
    if let Some(defs) = mutated.get_mut("definitions").and_then(Value::as_array_mut) {
        for def in defs.iter_mut() {
            if def.get("id").and_then(Value::as_str) == Some("xp-mon-ash-zombie") {
                def["fields"]["xp"] = Value::String(bumped.to_string());
            }
        }
    }
    let after = pay(&mutated);
    let ok = base == Some(declared) && after == Some(bumped) && bumped != declared;
    a.check(
        name,
        ok,
        format!(
            "定义 xp-mon-ash-zombie.fields.xp={declared} → enemy_defeated 发 res-xp {base:?}；\
             只把定义改成 {bumped}（脚本逐字不动）→ 发 {after:?} → XP 数值不是烘在 Lua 里的常量"
        ),
    );
}

/// GAP-F 佐证：事件缺 template_id 时，用 host.get_encounter 按 instance_id 从遭遇快照回查。
fn xp_encounter_snapshot_fallback(sb: &Value, a: &mut Assertions) {
    let name = "xp.encounter_snapshot_fallback";
    let source = mount_source(sb, "dnd-xp-award");
    let zombie = character(sb, "mon-ash-zombie");
    let expected = zombie.pointer("/statblock/xp").and_then(Value::as_i64).unwrap_or(0);
    let host = host_with(sb, 13);
    host.set_world_facts(json!({
        "characters": {}, "flags": {},
        "encounters": { "enc-2": { "id": "enc-2", "name": "洞穴", "active": true, "enemies": [
            { "id": "e1", "instance_id": "inst-z", "template_id": "mon-ash-zombie", "hp": 0 }
        ] } }
    }));
    let pc = character(sb, "pc-lmop-talin");
    let ctx = LuaHostContext {
        script_id: "lmop-check:xp-fallback".into(),
        actor_id: "inst-pc-lmop-talin".into(),
        actor: pc,
        ..Default::default()
    };
    // 事件里**没有** template_id，只有 instance_id。
    let data = json!({
        "enemy": { "id": "e1", "name": "灰烬丧尸", "instance_id": "inst-z" },
        "encounter": { "id": "enc-2", "name": "洞穴", "scene_id": "sc-x", "location_id": "loc-x" }
    });
    let env = MountEnv {
        event: Some(LuaEventContext { name: "enemy_defeated", data: Some(&data) }),
        ..Default::default()
    };
    let gain = xp_gain(&run(&host, &source, LuaMount::Event, &ctx, &env));
    // 负对照：遭遇快照里没有这只 instance_id → 不发。
    host.set_world_facts(json!({ "characters": {}, "flags": {}, "encounters": {} }));
    let miss = xp_gain(&run(&host, &source, LuaMount::Event, &ctx, &env));
    let ok = gain == Some(expected) && expected > 0 && miss.is_none();
    a.check(
        name,
        ok,
        format!(
            "enemy_defeated 缺 template_id、只有 instance_id → get_encounter('enc-2') 回查 mon-ash-zombie → res-xp +{gain:?}             （图鉴 statblock.xp={expected}）；快照里查不到 → {miss:?}（期望不发）。常规路径仍走 data.enemy.template_id"
        ),
    );
}
