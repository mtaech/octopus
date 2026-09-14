//! lua_host：mlua 沙箱宿主（#02 引擎职责边界 / #20 模块划分）。
//!
//! 职责边界（Lua 侧）：
//! - **只读 API**：当前角色（属性 / 资源 / 状态）、场景 / 回合、当前技能物品定义、相关关系边。
//! - **可写 API**：一律走「请求-校验-执行」——脚本只把请求写进 outbox，引擎校验后才改状态。
//! - **RNG 归引擎**：不加载 math.random，脚本只能调 host.engine_rng（走确定性序列）。
//! - **沙箱**：只加载白名单标准库（无 io / os / package / debug），scoped env 隔离，
//!   内存上限 + 指令预算，只接受文本源码（拒绝字节码）。
//!
//! 本模块只被 resolve / event 调（#20 ②）；不参与 Validate 阶段。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::chunk::ChunkMode;
use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Table, Value as LuaValue, VmState};
use octopus_types::relationship_endpoint;
use octopus_types::{CheckKind, CondExpr, ImmediateEffect, LuaMountDef, SuccessLevel};
use serde_json::{json, Value};

use crate::error::EngineError;
use crate::rng::DeterministicRng;

/// 每多少个 VM 指令触发一次预算钩子（越小越精确、开销越大）。
const HOOK_STEP: u32 = 1000;
/// 脚本私有存储区在 Lua 注册表中的键。
const STORAGE_REGISTRY_KEY: &str = "__octopus_script_storage";
/// 角色只读快照下发给脚本的键（GAP-L）：`host.actor` 与 `host.target` 同级完整。
///
/// 引擎不知道这些键的语义，只是把角色实例（`CharacterInstance`）的既有字段原样转过去；
/// 缺字段（如没有 location_id）就不下发，脚本读到 nil。
const CHARACTER_SNAPSHOT_KEYS: &[&str] = &[
    "instance_id",
    "template_id",
    "name",
    "kind",
    "attributes",
    "resources",
    "inventory",
    "statuses",
    "location_id",
    "present",
];

/// 沙箱资源预算。
#[derive(Debug, Clone, Copy)]
pub struct SandboxLimits {
    /// Lua 状态内存上限（字节）。
    pub memory_bytes: usize,
    /// 单次脚本执行的 VM 指令上限。
    pub max_instructions: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self { memory_bytes: 8 * 1024 * 1024, max_instructions: 2_000_000 }
    }
}

/// Lua 挂载点（#04 的判定 / 事件时机 + 「规则集走 Lua」的时机原语，另加协议插件；
/// Validate 阶段无 Lua）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuaMount {
    /// 判定修正（随机前）。
    CheckPreRoll,
    /// 判定修正（随机后）。
    CheckPostRoll,
    /// 自定义判定脚本（归一化输出 total / margin）。
    Check,
    /// 结算前钩子。
    PreResolve,
    /// 结算后钩子。
    PostResolve,
    /// 事件触发。
    Event,
    /// 条件评估（goal / trigger）。
    Condition,
    /// 状态结算时机（每个「角色 × 状态」一次）：引擎只派发时机与状态实例快照。
    StatusTick,
    /// 回合边界时机（该边界的状态结算完成后一次）。
    TurnEnd,
    /// 场景边界时机（该边界的状态结算完成后一次）。
    SceneEnd,
    /// 协议插件（preamble / parse / normalize）：只读，不产生任何 LuaRequest。
    Protocol,
}

impl LuaMount {
    /// 故事书 `lua_mounts` 允许声明的挂载点。
    ///
    /// `protocol` 不在列：协议插件走独立的 `narrative.protocol` 声明，不进规则集挂载点表。
    pub const DECLARABLE: [&'static str; 10] = [
        "check_pre_roll",
        "check_post_roll",
        "check",
        "pre_resolve",
        "post_resolve",
        "event",
        "condition",
        "status_tick",
        "turn_end",
        "scene_end",
    ];

    /// 严格解析挂载点字符串；未知值返回 None。
    ///
    /// 发布门用它拦截拼写错误——静默回落 PreResolve 会让「写错挂载点的规则」在
    /// 一个不相干的时机悄悄生效，比报错难查得多。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "check_pre_roll" => Some(LuaMount::CheckPreRoll),
            "check_post_roll" => Some(LuaMount::CheckPostRoll),
            "check" => Some(LuaMount::Check),
            "pre_resolve" => Some(LuaMount::PreResolve),
            "post_resolve" => Some(LuaMount::PostResolve),
            "event" => Some(LuaMount::Event),
            "condition" => Some(LuaMount::Condition),
            "status_tick" => Some(LuaMount::StatusTick),
            "turn_end" => Some(LuaMount::TurnEnd),
            "scene_end" => Some(LuaMount::SceneEnd),
            "protocol" => Some(LuaMount::Protocol),
            _ => None,
        }
    }

    /// 该挂载点是否可由故事书 `lua_mounts` 声明。
    ///
    /// `protocol` 是只读协议插件挂载点，走独立的 `narrative.protocol` 声明，不进规则集挂载点表。
    pub fn is_declarable(self) -> bool {
        LuaMount::DECLARABLE.contains(&self.as_str())
    }

    /// 解析挂载点字符串（未知值回落到 PreResolve；编辑器试跑等宽松入口用）。
    pub fn from_str(s: &str) -> Self {
        Self::parse(s).unwrap_or(LuaMount::PreResolve)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LuaMount::CheckPreRoll => "check_pre_roll",
            LuaMount::CheckPostRoll => "check_post_roll",
            LuaMount::Check => "check",
            LuaMount::PreResolve => "pre_resolve",
            LuaMount::PostResolve => "post_resolve",
            LuaMount::Event => "event",
            LuaMount::Condition => "condition",
            LuaMount::StatusTick => "status_tick",
            LuaMount::TurnEnd => "turn_end",
            LuaMount::SceneEnd => "scene_end",
            LuaMount::Protocol => "protocol",
        }
    }
}

/// 一次状态结算中被派发的状态实例快照（`status_tick` 挂载点的只读上下文）。
///
/// 引擎只提供**时机与事实**：谁、哪一个状态、还剩多久、本次按哪个单位结算。
/// 要不要在此结束这个状态、按什么概率结束——全是规则包 Lua 的事，引擎不解释。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LuaStatusContext {
    /// 状态实例 id（对应 `StatusInstance.id`）。
    pub id: String,
    /// 状态显示名。
    pub name: String,
    /// 剩余回合数（按回合结算的状态才有值）。
    pub turns_left: Option<i32>,
    /// 剩余场景数（按场景结算的状态才有值）。
    pub scenes_left: Option<i32>,
    /// 本次 tick 的单位：turns（回合边界）/ scenes（场景边界）。
    pub unit: &'static str,
    /// 本次结算后即将写入的剩余量（0 表示本次结算后到期移除）。
    pub remaining: i32,
}

/// 判定修正动作（通用原语）：引擎只认识「掷两次取高/低」「给判定加 N」「改难度」
/// 「覆盖判定结果」这四个动作。
///
/// 「优势 / 劣势 / 自然 1 / 自然 20」都是规则集词汇，属于 Lua 作者；脚本里写
/// `if host.has_status('x') then host.modify_check('keep_high') end`，或按判定细节决定
/// `host.modify_check('force_fail')`——引擎不认识那个状态 / 骰面代表什么，
/// 只认识「掷两次取高」「强制失败」这些通用动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckModifier {
    /// 掷两次取高。
    KeepHigh,
    /// 掷两次取低。
    KeepLow,
    /// 给判定加 N（固定加值，可负）。
    Add,
    /// 改难度（正数更难；判定前生效）。
    Difficulty,
    /// 覆盖判定结果为**成功**（成功度至少中档；骰面 / 总值不动）。
    ForceSuccess,
    /// 覆盖判定结果为**失败**（成功度落到最低档；骰面 / 总值不动）。
    ForceFail,
}

impl CheckModifier {
    /// 解析脚本给的修正名；未知值返回 None（下发即报错，不静默忽略）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "keep_high" | "high" => Some(CheckModifier::KeepHigh),
            "keep_low" | "low" => Some(CheckModifier::KeepLow),
            "add" | "bonus" => Some(CheckModifier::Add),
            "dc" | "difficulty" => Some(CheckModifier::Difficulty),
            "force_success" | "force_pass" | "succeed" => Some(CheckModifier::ForceSuccess),
            "force_fail" | "force_failure" | "fail" => Some(CheckModifier::ForceFail),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CheckModifier::KeepHigh => "keep_high",
            CheckModifier::KeepLow => "keep_low",
            CheckModifier::Add => "add",
            CheckModifier::Difficulty => "dc",
            CheckModifier::ForceSuccess => "force_success",
            CheckModifier::ForceFail => "force_fail",
        }
    }
}

/// 判定的只读快照（判定挂载点可读：改总值、按结果发效果都由脚本自行决定）。
///
/// 同一个结构承载两个阶段：
/// - **掷骰前**（`resolved = false`）：判定**签名**——属性 / 种类 / 难度在掷骰前已经确定，
///   钩子用它决定「对哪一类判定做什么」（取高 / 取低只有掷骰前有意义）。
/// - **掷骰后**（`resolved = true`）：在签名之上再给结果事实——总值 / 差值 / 结果 / 档位 /
///   骰面 / 骰式 `expr`。
///
/// 结果字段（total / margin / result / level / rolls）只在 `resolved = true` 时下发；
/// 掷骰前读它们仍是 nil，钩子不会把「还没掷的 0」误当成判定结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LuaCheckContext {
    pub attribute: String,
    pub kind: Option<CheckKind>,
    /// 判定是否已出结果：false = 掷骰前签名，true = 掷骰后快照（结果字段可用）。
    pub resolved: bool,
    /// 骰式字符串（声明式判定才有；Lua 判定 / 被动判定为 None）。
    pub expr: Option<String>,
    pub total: i64,
    pub target: i64,
    pub margin: i64,
    pub result: bool,
    pub level: Option<SuccessLevel>,
    pub rolls: Vec<i64>,
}

/// 挂载点执行附加值：`when` 条件闸门 + 判定结果快照。
///
/// 刻意**不**并进 `LuaHostContext`：后者是跨 crate 的字面量构造契约（API 层逐字段构造），
/// 加字段会破坏外部调用方；这些是「本次执行的额外输入」，用独立参数传递更稳。
#[derive(Default)]
pub struct MountEnv<'a> {
    /// `when` 条件求值器（由调用方按世界快照构造）；None = 声明了 `when` 的脚本一律跳过。
    pub gate: Option<&'a dyn Fn(&CondExpr) -> bool>,
    /// 已掷骰判定的快照（判定后挂载点可读）。
    pub check: Option<&'a LuaCheckContext>,
    /// 事件挂载点的通用上下文（事件名 + 只读事实快照）。
    ///
    /// 只在 Lua Event 挂载点下发；其他挂载点为 None（脚本读 `host.event_name` /
    /// `host.event_data` 得到 nil——旧脚本看不到新字段，也不会因此报错）。
    pub event: Option<LuaEventContext<'a>>,
    /// 本次结算**实际算出的效果**只读快照（host.resolved_effects）。
    ///
    /// 只在效果结算之后、PostResolve 挂载点下发（command::execute_skill）；其他挂载点
    /// 为 None，脚本读到 nil——旧脚本看不到新字段，也不会因此报错。引擎只导出事实
    ///（deltas / statuses / modifiers / rng_consumed / factor），不解释用法。
    pub resolved_effects: Option<&'a Value>,
}

/// 一次事件派发的**通用**上下文：引擎只把「发生了什么（名字）」与「相关事实（任意 JSON）」
/// 原样交给脚本，**不解释** data 的内容。
#[derive(Debug, Clone, Copy, Default)]
pub struct LuaEventContext<'a> {
    /// 事件名；由派发方给出（引擎不认识它的含义）。
    pub name: &'a str,
    /// 该事件的只读事实快照（缺省 = 没有附加事实）。
    pub data: Option<&'a Value>,
}

/// Lua 向引擎发起的写请求（请求-校验-执行：引擎负责校验与落状态）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum LuaRequest {
    /// 声明资源消耗。
    Cost { resource: String, amount: i64 },
    /// 施加状态。
    ApplyStatus { target: String, status: String, duration: i64, unit: String },
    /// 移除状态。
    RemoveStatus { target: String, status: String },
    /// 触发事件（交给其他监听者）。
    TriggerEvent { event: String, payload: Value },
    /// 查询世界（返回过滤后的实体；由引擎解释）。
    QueryWorld { query: String },
    /// 判定修正：掷两次取高/低 · 加值 · 难度 · 覆盖结果（只在判定挂载点被消费）。
    ModifyCheck { mode: CheckModifier, amount: i64 },
    /// 施加即时效果：复用既有 `ImmediateEffect` 封闭原语，不新增任何效果语义。
    ApplyEffect { target: String, effect: Value },
    /// 加减任意目标的资源（可正可负；target 为空 = 当前 actor）。
    ModifyResource { target: String, resource: String, amount: i64 },
    /// 效果缩放（通用原语）：声明「本次结算的**数值型** delta（资源增减）按此因子缩放」。
    ///
    /// 引擎不认识「豁免 / 减半 / 抗性 / 易伤」——只有一个因子；要不要缩放、缩放多少
    /// 由规则包 Lua 决定。只在判定之后、效果结算之前被消费（check_post_roll /
    /// pre_resolve），这两处正是效果结算的上游时机。
    ///
    /// **顺带开门**（既有语义，逐字保留）：这条声明本身还意味着「即使判定成功 /
    /// 未命中，也照常结算效果」——证据是因子 1.0 与「不声明」行为并不相同。
    /// 只想开门、不想缩放时用 [`LuaRequest::ForceEffect`]。
    ScaleEffect { factor: f64 },
    /// 打开效果门（通用原语）：声明「即使判定成功 / 未命中，也照常结算效果一次」。
    ///
    /// 与 [`LuaRequest::ScaleEffect`] **正交**：它不改变任何数值（缩放是 ScaleEffect
    /// 的事），只覆盖「判定结果决定是否结算效果」那一道门。两者可同时声明
    ///（开门 + 缩放）。同样只在判定之后、效果结算之前被消费（check_post_roll /
    /// pre_resolve）。
    ForceEffect,
}

/// 归一化判定输出（#12）：脚本只能给最终值 total 与差值 margin，档位由引擎分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaCheckOutcome {
    pub total: i64,
    pub margin: i64,
}

/// 一次脚本调用的上下文（只读数据；由 resolve / event 组装）。
#[derive(Debug, Clone, Default)]
pub struct LuaHostContext {
    /// 脚本标识：per-script 存储区按它隔离，且用于错误回溯。
    pub script_id: String,
    /// 当前角色实例在存档里的寻址 id（state.characters 的键）。
    pub actor_id: String,
    /// 当前角色实例（CharacterInstance 的 JSON 形态）。
    pub actor: Value,
    /// 当前目标在存档里的寻址 id（可选）。
    pub target_id: Option<String>,
    /// 当前目标（可选）。
    pub target: Option<Value>,
    /// 当前技能 / 物品声明式定义（可选）。
    pub skill: Option<Value>,
    pub scene_id: String,
    pub round: u32,
    /// 本次判定的难度（供 Lua 判定脚本读取）。
    pub difficulty: Option<i64>,
    /// 与当前角色相关的有向关系边。
    pub relationships: Vec<Value>,
    /// 在场角色 id 列表（协议插件的只读快照，对齐提示词的「在场角色」名单）。
    pub present: Vec<String>,
    /// 受控角色（提示词里的「名字(id)」形态）；协议插件只读。
    pub controlled: String,
}

/// mlua 沙箱宿主。一个存档 / 会话持有一个实例。
pub struct LuaHost {
    lua: Lua,
    rng: Arc<Mutex<DeterministicRng>>,
    outbox: Arc<Mutex<Vec<LuaRequest>>>,
    instr: Arc<AtomicU64>,
    limits: SandboxLimits,
    /// 只读开放内容快照（GAP-C）：故事书的 characters / definitions 原文。
    ///
    /// 引擎**不理解**它的语义（不认识 kind，也不解释 fields / modifiers），只负责按
    /// 「当前角色模板 → 挂接 → definition」把原始数据交给脚本。缺省 Null = 无快照
    /// （编辑器试跑等独立入口），脚本读到 nil / 空表。
    read_data: Arc<Mutex<Value>>,
    /// 只读世界事实快照（GAP-A）：角色实例 / 世界标记 / 活跃遭遇。
    ///
    /// 与 `read_data`（冻结的开放内容）分开：这些事实随回合变化，由会话组合根在**跑脚本前**
    /// 用 `set_world_facts` 刷新。引擎**不理解**其语义，只按 id / 名字做查表搬运；
    /// 缺省 Null = 没有快照（单测 / 编辑器试跑），脚本读到 nil / 空表，行为与不注入时一致。
    world_data: Arc<Mutex<Value>>,
}

impl LuaHost {
    /// 以存档种子新建宿主。
    pub fn new(seed: u64) -> Result<Self, EngineError> {
        Self::with_limits(seed, SandboxLimits::default())
    }

    pub fn with_limits(seed: u64, limits: SandboxLimits) -> Result<Self, EngineError> {
        Self::with_rng(Arc::new(Mutex::new(DeterministicRng::new(seed))), limits)
    }

    /// 复用引擎已有的确定性 RNG（保证骰值与命令日志一致）。
    pub fn with_rng(rng: Arc<Mutex<DeterministicRng>>, limits: SandboxLimits) -> Result<Self, EngineError> {
        // 白名单：table / string / utf8 / math。IO / OS / PACKAGE / DEBUG / FFI 一律不加载；
        // 基础库（print / pairs / type / pcall 等）由 mlua 无条件装载，无需显式声明。
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
        let lua = Lua::new_with(libs, LuaOptions::default()).map_err(lua_err)?;

        // RNG 归引擎独家：抹掉 Lua 自带随机源。
        if let Ok(math) = lua.globals().get::<Table>("math") {
            let _ = math.set("random", LuaValue::Nil);
            let _ = math.set("randomseed", LuaValue::Nil);
        }

        lua.set_memory_limit(limits.memory_bytes).map_err(lua_err)?;

        // 指令预算：钩子按固定步长累加，超预算即抛错终止本次执行。
        let instr = Arc::new(AtomicU64::new(0));
        let hook_instr = instr.clone();
        let budget = limits.max_instructions;
        lua.set_hook(HookTriggers::new().every_nth_instruction(HOOK_STEP), move |_lua, _dbg| {
            let used = hook_instr.fetch_add(HOOK_STEP as u64, Ordering::Relaxed) + HOOK_STEP as u64;
            if used > budget {
                Err(mlua::Error::RuntimeError(format!("instruction budget exceeded ({budget})")))
            } else {
                Ok(VmState::Continue)
            }
        })
        .map_err(lua_err)?;

        let storage = lua.create_table().map_err(lua_err)?;
        lua.set_named_registry_value(STORAGE_REGISTRY_KEY, storage).map_err(lua_err)?;

        Ok(Self {
            lua,
            rng,
            outbox: Arc::new(Mutex::new(Vec::new())),
            instr,
            limits,
            read_data: Arc::new(Mutex::new(Value::Null)),
            world_data: Arc::new(Mutex::new(Value::Null)),
        })
    }

    pub fn limits(&self) -> SandboxLimits {
        self.limits
    }

    /// 注入只读开放内容快照（GAP-C）：故事书的 `characters` / `definitions` 原文。
    ///
    /// 由会话组合根在**冻结故事书**上调用一次；引擎不理解数据语义，只做懒查表。
    /// 没调用过的宿主（编辑器试跑 / 单测）读到 nil，行为与不注入时一致。
    pub fn set_read_data(&self, data: Value) {
        if let Ok(mut slot) = self.read_data.lock() {
            *slot = data;
        }
    }

    /// 注入只读世界事实快照（GAP-A）：角色实例 / 世界标记 / 活跃遭遇。
    ///
    /// 与 `set_read_data`（冻结的开放内容）分开：这些事实随回合变化，由会话组合根在**跑脚本前**
    /// 刷新。引擎不理解数据语义，只按 id / 名字把存档里的就绪事实原样交给脚本查表；
    /// 没调用过的宿主（编辑器试跑 / 单测 / 没有相关挂载点脚本的存档）读到 nil / 空表。
    pub fn set_world_facts(&self, data: Value) {
        if let Ok(mut slot) = self.world_data.lock() {
            *slot = data;
        }
    }

    /// 引擎侧持有的同一 RNG 句柄（读取 consumed 或继续消费）。
    pub fn rng_handle(&self) -> Arc<Mutex<DeterministicRng>> {
        self.rng.clone()
    }

    /// 取走并清空脚本累积的写请求（引擎负责逐个校验执行）。
    pub fn drain_requests(&self) -> Vec<LuaRequest> {
        let mut q = self.outbox.lock().expect("lua outbox poisoned");
        std::mem::take(&mut *q)
    }

    /// 条件评估：脚本返回真值即成立（#12 / #13 的 lua 兜底）。
    pub fn run_condition(&self, script: &str, ctx: &LuaHostContext) -> Result<bool, EngineError> {
        Ok(match self.eval_value(script, ctx, LuaMount::Condition, &MountEnv::default(), None)? {
            LuaValue::Nil => false,
            LuaValue::Boolean(b) => b,
            _ => true,
        })
    }

    /// 自定义判定脚本：只接受归一化的 { total, margin }（#12 契约）。
    pub fn run_check(&self, script: &str, ctx: &LuaHostContext) -> Result<LuaCheckOutcome, EngineError> {
        let value = self.eval_value(script, ctx, LuaMount::Check, &MountEnv::default(), None)?;
        let LuaValue::Table(t) = value else {
            return Err(EngineError::Lua("check script must return a table { total, margin }".into()));
        };
        Ok(LuaCheckOutcome {
            total: read_int(&t, "total")?,
            margin: read_int(&t, "margin")?,
        })
    }

    /// 结算 / 事件钩子：只产生副作用（写请求），返回值忽略。
    pub fn run_hook(&self, script: &str, mount: LuaMount, ctx: &LuaHostContext) -> Result<(), EngineError> {
        self.run_hook_with(script, mount, ctx, &MountEnv::default())
    }

    /// 同上，但带挂载点附加值（`when` 闸门 + 判定结果快照）。
    pub fn run_hook_with(
        &self,
        script: &str,
        mount: LuaMount,
        ctx: &LuaHostContext,
        env: &MountEnv<'_>,
    ) -> Result<(), EngineError> {
        self.eval_value(script, ctx, mount, env, None).map(|_| ())
    }

    /// 状态结算挂载点（`status_tick`）：额外把「正在结算的状态实例」暴露给脚本
    ///（`host.status` / `host.status_id` / `host.status_turns_left` …）。
    ///
    /// 引擎只给事实快照；脚本据此掷骰、加减资源、移除状态——全由规则包决定。
    pub fn run_hook_status(
        &self,
        script: &str,
        mount: LuaMount,
        ctx: &LuaHostContext,
        env: &MountEnv<'_>,
        status: &LuaStatusContext,
    ) -> Result<(), EngineError> {
        self.eval_value(script, ctx, mount, env, Some(status)).map(|_| ())
    }

    // ---------- 协议插件（LuaMount::Protocol） ----------
    //
    // 与判定器 Lua 同源：复用同一份 scoped env / 只读 API / SandboxLimits；
    // 区别是协议挂载点不注册任何世界写入 API（见 make_env），插件只能返回意图。

    /// 调用 `protocol.preamble(ctx)` → 系统层协议说明。
    pub fn run_protocol_preamble(&self, source: &str, ctx: &LuaHostContext) -> Result<String, EngineError> {
        let ctx_table = self.protocol_ctx_table(ctx)?;
        match self.eval_protocol(source, "preamble", LuaValue::Table(ctx_table), ctx)? {
            Some(LuaValue::String(s)) => Ok(s.to_string_lossy()),
            Some(other) => Err(EngineError::Lua(format!(
                "protocol.preamble 必须返回字符串，实际得到 {}",
                other.type_name()
            ))),
            None => Err(EngineError::Lua("协议插件未定义 protocol.preamble".into())),
        }
    }

    /// 调用 `protocol.parse(raw)`，把返回表转成 JSON（引擎再反序列化为 Vec<Intent>）。
    pub fn run_protocol_parse(&self, source: &str, raw: &str, ctx: &LuaHostContext) -> Result<Value, EngineError> {
        let raw = self.lua.create_string(raw).map_err(lua_err)?;
        match self.eval_protocol(source, "parse", LuaValue::String(raw), ctx)? {
            Some(v) => lua_to_json(&v),
            None => Err(EngineError::Lua("协议插件未定义 protocol.parse".into())),
        }
    }

    /// 调用可选的 `protocol.normalize(intents, ctx)`；未定义时返回 None。
    pub fn run_protocol_normalize(
        &self,
        source: &str,
        intents: &Value,
        ctx: &LuaHostContext,
    ) -> Result<Option<Value>, EngineError> {
        let ctx_table = self.protocol_ctx_table(ctx)?;
        let list = intents.as_array().cloned().unwrap_or_default();
        let table = self.lua.create_table().map_err(lua_err)?;
        for (i, item) in list.iter().enumerate() {
            table
                .set(i + 1, json_to_lua(&self.lua, item).map_err(lua_err)?)
                .map_err(lua_err)?;
        }
        match self.eval_protocol(
            source,
            "normalize",
            (LuaValue::Table(table), LuaValue::Table(ctx_table)),
            ctx,
        )? {
            Some(v) => Ok(Some(lua_to_json(&v)?)),
            None => Ok(None),
        }
    }

    /// 载入插件源码定义 `protocol.*`，再调用指定函数。函数未定义时返回 Ok(None)。
    fn eval_protocol<A: mlua::IntoLuaMulti>(
        &self,
        source: &str,
        func: &str,
        args: A,
        ctx: &LuaHostContext,
    ) -> Result<Option<LuaValue>, EngineError> {
        let env = self.make_env(ctx, LuaMount::Protocol, &MountEnv::default(), None)?;
        // 预置 protocol 命名空间：插件里 `protocol = protocol or {}` 或直接
        // `function protocol.parse(...)` 都能落到同一张表。
        let protocol = self.lua.create_table().map_err(lua_err)?;
        env.set("protocol", protocol.clone()).map_err(lua_err)?;

        self.instr.store(0, Ordering::Relaxed);
        let chunk = self
            .lua
            .load(source)
            .set_name("protocol-plugin")
            .set_environment(env.clone())
            .set_mode(ChunkMode::Text)
            .into_function()
            .map_err(lua_err)?;
        chunk.call::<()>(()).map_err(lua_err)?;

        let protocol: Table = env.get("protocol").map_err(lua_err)?;
        let target: LuaValue = protocol.get(func).map_err(lua_err)?;
        let LuaValue::Function(f) = target else {
            return Ok(None);
        };
        let result = f.call::<LuaValue>(args).map_err(lua_err)?;
        Ok(Some(result))
    }

    /// 协议插件的只读 ctx：场景 / 回合 / 在场角色 / 受控角色。
    fn protocol_ctx_table(&self, ctx: &LuaHostContext) -> Result<Table, EngineError> {
        let t = self.lua.create_table().map_err(lua_err)?;
        t.set("scene_id", ctx.scene_id.clone()).map_err(lua_err)?;
        t.set("round", ctx.round).map_err(lua_err)?;
        let present = self.lua.create_table().map_err(lua_err)?;
        for (i, id) in ctx.present.iter().enumerate() {
            present.set(i + 1, id.clone()).map_err(lua_err)?;
        }
        t.set("present", present).map_err(lua_err)?;
        t.set("controlled", ctx.controlled.clone()).map_err(lua_err)?;
        if !ctx.actor_id.is_empty() {
            t.set("actor_id", ctx.actor_id.clone()).map_err(lua_err)?;
        }
        Ok(t)
    }

    // ---------- 内部 ----------

    fn eval_value(
        &self,
        script: &str,
        ctx: &LuaHostContext,
        mount: LuaMount,
        mount_env: &MountEnv<'_>,
        status: Option<&LuaStatusContext>,
    ) -> Result<LuaValue, EngineError> {
        let env = self.make_env(ctx, mount, mount_env, status)?;
        self.instr.store(0, Ordering::Relaxed);
        self.lua
            .load(script)
            .set_name(ctx.script_id.clone())
            .set_environment(env)
            .set_mode(ChunkMode::Text)
            .eval::<LuaValue>()
            .map_err(lua_err)
    }

    /// 每脚本独立的沙箱存储区（跨回合保留，脚本之间互不可见）。
    fn script_storage(&self, script_id: &str) -> Result<Table, EngineError> {
        let root: Table = self.lua.named_registry_value(STORAGE_REGISTRY_KEY).map_err(lua_err)?;
        if let Ok(existing) = root.get::<Table>(script_id) {
            return Ok(existing);
        }
        let table = self.lua.create_table().map_err(lua_err)?;
        root.set(script_id, table.clone()).map_err(lua_err)?;
        Ok(table)
    }

    /// 构造本次执行的 scoped env：host 表 + 指向 globals 的只读白名单 __index。
    fn make_env(
        &self,
        ctx: &LuaHostContext,
        mount: LuaMount,
        mount_env: &MountEnv<'_>,
        status: Option<&LuaStatusContext>,
    ) -> Result<Table, EngineError> {
        let lua = &self.lua;
        let env = lua.create_table().map_err(lua_err)?;
        // 写全局变量只落在 env；读全局走 __index → globals（此时 globals 已是最小白名单）。
        let mt = lua.create_table().map_err(lua_err)?;
        mt.set("__index", lua.globals()).map_err(lua_err)?;
        env.set_metatable(Some(mt)).map_err(lua_err)?;

        let host = lua.create_table().map_err(lua_err)?;
        host.set("script_id", ctx.script_id.clone()).map_err(lua_err)?;
        host.set("mount", mount.as_str()).map_err(lua_err)?;
        // 事件名 = 挂载点名（一个挂载点就是一个时机）：脚本可用 host.event 写通用分支。
        host.set("event", mount.as_str()).map_err(lua_err)?;
        host.set("scene_id", ctx.scene_id.clone()).map_err(lua_err)?;
        host.set("round", ctx.round).map_err(lua_err)?;
        host.set("difficulty", ctx.difficulty).map_err(lua_err)?;
        host.set("controlled", ctx.controlled.clone()).map_err(lua_err)?;
        // 在场角色 id 列表（协议插件用它做白名单 / 归一化；判定器也可见，无副作用）。
        let present = lua.create_table().map_err(lua_err)?;
        for (i, id) in ctx.present.iter().enumerate() {
            present.set(i + 1, id.clone()).map_err(lua_err)?;
        }
        host.set("present", present).map_err(lua_err)?;
        host.set("storage", self.script_storage(&ctx.script_id)?).map_err(lua_err)?;

        // 事件上下文（Event 挂载点）：事件名 + 只读事实快照。
        // 引擎只负责把「发生了什么」与相关事实原样交给脚本，不解释 data 的内容。
        if let Some(event) = mount_env.event {
            host.set("event_name", event.name).map_err(lua_err)?;
            if let Some(data) = event.data {
                host.set("event_data", json_to_lua(lua, data)?).map_err(lua_err)?;
            }
        }

        // host.actor：当前角色的只读快照；id 用存档寻址键，供 apply_status(target, ...) 等使用。
        let actor_id = if ctx.actor_id.is_empty() {
            ctx.actor
                .get("id")
                .or_else(|| ctx.actor.get("instance_id"))
                .or_else(|| ctx.actor.get("template_id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            ctx.actor_id.clone()
        };
        let actor_ref = lua.create_table().map_err(lua_err)?;
        actor_ref.set("id", actor_id).map_err(lua_err)?;
        for key in CHARACTER_SNAPSHOT_KEYS {
            if let Some(v) = ctx.actor.get(*key) {
                actor_ref.set(*key, json_to_lua(lua, v)?).map_err(lua_err)?;
            }
        }
        host.set("actor", actor_ref).map_err(lua_err)?;

        // ---------- 只读 API ----------
        let actor = ctx.actor.clone();
        host.set(
            "get_attribute",
            lua.create_function(move |lua, name: String| {
                match actor.get("attributes").and_then(|a| a.get(name.as_str())) {
                    Some(v) => json_to_lua(lua, v),
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let actor = ctx.actor.clone();
        host.set(
            "get_resource",
            lua.create_function(move |lua, name: String| {
                match actor.get("resources").and_then(|r| r.get(name.as_str())) {
                    Some(v) => json_to_lua(lua, v),
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let actor = ctx.actor.clone();
        host.set(
            "has_status",
            lua.create_function(move |_, name: String| {
                let found = actor
                    .get("statuses")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter().any(|s| {
                            s.get("id").and_then(Value::as_str) == Some(name.as_str())
                                || s.get("name").and_then(Value::as_str) == Some(name.as_str())
                        })
                    })
                    .unwrap_or(false);
                Ok(found)
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        // ---------- 只读世界事实（GAP-A）：任意实体 / 标记 / 遭遇 ----------
        //
        // 引擎**不理解**这些数据的语义：它只把存档里的事实按 id / 名字做查表搬运，
        // 不判断「谁和谁是一伙」「哪个遭遇算赢」——那是规则包 Lua 的事。
        // 快照没注入 / 查不到时返回 nil / 空表，不抛错（旧存档、编辑器试跑行为一致）。
        let world_data = self.world_data.clone();
        host.set(
            "get_character",
            lua.create_function(move |lua, id: String| {
                let data = world_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("world data poisoned".into()))?;
                match find_character_snapshot(&data, &id) {
                    Some((key, inst)) => {
                        // 快照带存档寻址键（id），供 apply_status(target, ...) 等写请求使用——
                        // 与 host.actor.id 同一口径，脚本不必自己拼实例键。
                        let mut obj = inst.as_object().cloned().unwrap_or_default();
                        obj.insert("id".to_string(), Value::String(key.to_string()));
                        json_to_lua(lua, &Value::Object(obj))
                    }
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let world_data = self.world_data.clone();
        host.set(
            "get_flag",
            lua.create_function(move |lua, name: String| {
                let data = world_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("world data poisoned".into()))?;
                match data.get("flags").and_then(|flags| flags.get(name.trim())) {
                    Some(v) => json_to_lua(lua, v),
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let world_data = self.world_data.clone();
        host.set(
            "list_flags",
            lua.create_function(move |lua, ()| {
                let data = world_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("world data poisoned".into()))?;
                let flags = data
                    .get("flags")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                json_to_lua(lua, &flags)
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let world_data = self.world_data.clone();
        host.set(
            "get_encounter",
            lua.create_function(move |lua, id: String| {
                let data = world_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("world data poisoned".into()))?;
                match find_encounter(&data, id.trim()) {
                    Some(enc) => json_to_lua(lua, enc),
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let world_data = self.world_data.clone();
        host.set(
            "list_encounters",
            lua.create_function(move |lua, ()| {
                let data = world_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("world data poisoned".into()))?;
                json_to_lua(lua, &Value::Array(active_encounters(&data)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        // ---------- 只读开放内容（GAP-C）：挂接 / definition ----------
        //
        // 引擎只做「故事书数据 → 脚本」的搬运：不认识 kind，也不解释 fields / modifiers /
        // mechanics；脚本据此自己决定怎么用。没有注入快照时全部返回 nil / 空表。
        let read_data = self.read_data.clone();
        let actor_for_atts = ctx.actor.clone();
        host.set(
            "get_attachments",
            lua.create_function(move |lua, ()| {
                let data = read_data.lock().map(|d| d.clone()).unwrap_or(Value::Null);
                json_to_lua(lua, &Value::Object(attachment_map(&data, &actor_for_atts)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let read_data = self.read_data.clone();
        let actor_for_atts = ctx.actor.clone();
        host.set(
            "get_attachment",
            lua.create_function(move |lua, kind: String| {
                let data = read_data.lock().map(|d| d.clone()).unwrap_or(Value::Null);
                match attachment_map(&data, &actor_for_atts).get(kind.trim()) {
                    Some(v) => json_to_lua(lua, v),
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let read_data = self.read_data.clone();
        host.set(
            "get_definition",
            // 直接在大快照的锁内查表（只读，不做拷贝）：definition 可能成千条，
            // 每次调用都克隆整段数据不划算。
            lua.create_function(move |lua, id: String| {
                let data = read_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("read data poisoned".into()))?;
                match find_definition(&data, id.trim()) {
                    Some(def) => json_to_lua(lua, def),
                    None => Ok(LuaValue::Nil),
                }
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        let read_data = self.read_data.clone();
        host.set(
            "list_definitions",
            lua.create_function(move |lua, kind: Option<String>| {
                let data = read_data
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("read data poisoned".into()))?;
                let list = definitions_of(&data, kind.as_deref());
                json_to_lua(lua, &Value::Array(list))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        // 状态结算快照（status_tick 挂载点可读）：谁、哪个状态、还剩多久、按哪个单位结算。
        // 引擎只给事实；「是否在此结束这个状态」由规则包 Lua 自己掷骰决定。
        if let Some(status) = status {
            let t = lua.create_table().map_err(lua_err)?;
            t.set("id", status.id.clone()).map_err(lua_err)?;
            t.set("name", status.name.clone()).map_err(lua_err)?;
            t.set("turns_left", status.turns_left).map_err(lua_err)?;
            t.set("scenes_left", status.scenes_left).map_err(lua_err)?;
            t.set("unit", status.unit).map_err(lua_err)?;
            t.set("remaining", status.remaining).map_err(lua_err)?;
            // 平铺别名：脚本可直接读 host.status_id / host.status_turns_left 等。
            host.set("status_id", status.id.clone()).map_err(lua_err)?;
            host.set("status_name", status.name.clone()).map_err(lua_err)?;
            host.set("status_turns_left", status.turns_left).map_err(lua_err)?;
            host.set("status_scenes_left", status.scenes_left).map_err(lua_err)?;
            host.set("status_unit", status.unit).map_err(lua_err)?;
            host.set("status_remaining", status.remaining).map_err(lua_err)?;
            host.set("status", t).map_err(lua_err)?;
        }

        if let Some(target) = &ctx.target {
            let target = target.clone();
            let target_ref = lua.create_table().map_err(lua_err)?;
            let target_id = ctx
                .target_id
                .clone()
                .or_else(|| target.get("id").and_then(Value::as_str).map(str::to_string))
                .or_else(|| target.get("instance_id").and_then(Value::as_str).map(str::to_string))
                .or_else(|| target.get("template_id").and_then(Value::as_str).map(str::to_string));
            if let Some(id) = target_id {
                target_ref.set("id", id).map_err(lua_err)?;
            }
            for key in CHARACTER_SNAPSHOT_KEYS {
                if let Some(v) = target.get(*key) {
                    target_ref.set(*key, json_to_lua(lua, v)?).map_err(lua_err)?;
                }
            }
            host.set("target", target_ref).map_err(lua_err)?;
        }

        if let Some(skill) = &ctx.skill {
            host.set("definition", json_to_lua(lua, skill)?).map_err(lua_err)?;
        }

        // 判定快照（判定挂载点可读）：掷骰前下发**判定签名**（属性 / 种类 / 难度 / 骰式），
        // 判定出结果后才追加结果事实。引擎只给事实；「对哪一类判定做什么」由规则包 Lua 决定
        //（例如「豁免成功只结算一半」——「一半」是 Lua 的算术，引擎只提供 apply_effect）。
        if let Some(check) = mount_env.check {
            let t = lua.create_table().map_err(lua_err)?;
            t.set("attribute", check.attribute.clone()).map_err(lua_err)?;
            t.set("target", check.target).map_err(lua_err)?;
            t.set("resolved", check.resolved).map_err(lua_err)?;
            if let Some(expr) = &check.expr {
                t.set("expr", expr.clone()).map_err(lua_err)?;
            }
            if let Some(kind) = check.kind {
                let v = serde_json::to_value(kind).unwrap_or(Value::Null);
                t.set("kind", json_to_lua(lua, &v)?).map_err(lua_err)?;
            }
            // 平铺别名（签名部分）：脚本可直接读 host.check_attribute / host.check_kind 等。
            host.set("check_attribute", check.attribute.clone()).map_err(lua_err)?;
            host.set("check_target", check.target).map_err(lua_err)?;
            if let Some(expr) = &check.expr {
                host.set("check_expr", expr.clone()).map_err(lua_err)?;
            }
            if let Some(kind) = check.kind {
                let v = serde_json::to_value(kind).unwrap_or(Value::Null);
                host.set("check_kind", json_to_lua(lua, &v)?).map_err(lua_err)?;
            }
            // 结果事实：只有判定已出结果才下发（掷骰前读 total / margin / result 仍是 nil）。
            if check.resolved {
                t.set("total", check.total).map_err(lua_err)?;
                t.set("margin", check.margin).map_err(lua_err)?;
                t.set("result", check.result).map_err(lua_err)?;
                if let Some(level) = check.level {
                    let v = serde_json::to_value(level).unwrap_or(Value::Null);
                    t.set("level", json_to_lua(lua, &v)?).map_err(lua_err)?;
                }
                let rolls = lua.create_table().map_err(lua_err)?;
                for (i, roll) in check.rolls.iter().enumerate() {
                    rolls.set(i + 1, *roll).map_err(lua_err)?;
                }
                t.set("rolls", rolls).map_err(lua_err)?;
                // 平铺别名（结果部分）：脚本可直接读 host.check_total / host.check_result 等。
                host.set("check_total", check.total).map_err(lua_err)?;
                host.set("check_margin", check.margin).map_err(lua_err)?;
                host.set("check_result", check.result).map_err(lua_err)?;
                if let Some(level) = check.level {
                    let v = serde_json::to_value(level).unwrap_or(Value::Null);
                    host.set("check_level", json_to_lua(lua, &v)?).map_err(lua_err)?;
                }
            }
            host.set("check", t).map_err(lua_err)?;
        }

        // 效果快照（PostResolve 可读）：引擎**实际**算出的效果与数值（缩放之后），
        // 让规则包能核对「引擎到底算了什么」，而不是自己另掷一份近似。
        // 只在效果结算之后下发；其他挂载点是 nil（旧脚本看不到，也不会因此报错）。
        if let Some(effects) = mount_env.resolved_effects {
            host.set("resolved_effects", json_to_lua(lua, effects)?).map_err(lua_err)?;
        }

        let relationships = ctx.relationships.clone();
        host.set(
            "relationship",
            lua.create_function(move |_, (from, to, kind): (String, String, String)| {
                let value = relationships.iter().find_map(|r| {
                    // 兼容规范 from/to 与旧 from_id/to_id（决策 #11）。
                    let same = relationship_endpoint(r, "from", "from_id") == Some(from.as_str())
                        && relationship_endpoint(r, "to", "to_id") == Some(to.as_str())
                        && r.get("type").and_then(Value::as_str) == Some(kind.as_str());
                    if same { r.get("value").and_then(Value::as_i64) } else { None }
                });
                Ok(value)
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        // ---------- 可写 API：只入 outbox，引擎校验后执行 ----------
        let rng = self.rng.clone();
        host.set(
            "engine_rng",
            lua.create_function(move |_, (min, max): (i64, i64)| {
                let mut r = rng
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("rng poisoned".into()))?;
                Ok(r.range_inclusive(min, max))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

        macro_rules! push {
            ($fname:literal, $body:expr) => {{
                let outbox = self.outbox.clone();
                host.set(
                    $fname,
                    lua.create_function(move |_, args| {
                        let req = $body(args);
                        if let Ok(mut q) = outbox.lock() {
                            q.push(req);
                        }
                        Ok(())
                    })
                    .map_err(lua_err)?,
                )
                .map_err(lua_err)?;
            }};
        }

        // 可失败的写入 API：参数非法即当场报错（作者立刻看到，而不是静默丢请求）。
        macro_rules! push_try {
            ($fname:literal, $body:expr) => {{
                let outbox = self.outbox.clone();
                host.set(
                    $fname,
                    lua.create_function(move |_, args| {
                        let req = $body(args).map_err(mlua::Error::RuntimeError)?;
                        if let Ok(mut q) = outbox.lock() {
                            q.push(req);
                        }
                        Ok(())
                    })
                    .map_err(lua_err)?,
                )
                .map_err(lua_err)?;
            }};
        }

        // 协议插件是只读的：不注册任何世界写入 API（LuaRequest 在协议挂载点全部拒绝），
        // 插件只能返回意图，由引擎校验后结算。
        if mount != LuaMount::Protocol {
        push!("request_cost", |(resource, amount): (String, i64)| LuaRequest::Cost {
            resource,
            amount,
        });
        push!("apply_status", |(target, status, duration, unit): (
            String,
            String,
            Option<i64>,
            Option<String>
        )| LuaRequest::ApplyStatus {
            target,
            status,
            duration: duration.unwrap_or(1),
            unit: unit.unwrap_or_else(|| "turns".into()),
        });
        push!("remove_status", |(target, status): (String, String)| LuaRequest::RemoveStatus {
            target,
            status,
        });
        push!("trigger_event", |event: String| LuaRequest::TriggerEvent {
            event,
            payload: json!({}),
        });
        push!("query_world", |query: String| LuaRequest::QueryWorld { query });

        // ---- 通用原语（不含任何规则集语义：引擎不认识「优势 / 熟练 / 豁免」，只认识这些动作）----

        // 判定修正：掷两次取高/低 · 加值 · 改难度 · 覆盖结果。名字必须命中通用动作表，否则当场报错。
        push_try!("modify_check", |(mode, amount): (String, Option<i64>)| {
            let parsed = CheckModifier::parse(&mode).ok_or_else(|| {
                format!(
                    "未知的判定修正「{mode}」（可用：keep_high / keep_low / add / dc / force_success / force_fail）"
                )
            })?;
            Ok(LuaRequest::ModifyCheck { mode: parsed, amount: amount.unwrap_or(0) })
        });

        // 效果缩放（通用原语）：声明「本次结算的数值型 delta 按此因子缩放」。
        // 与判定修正不同，它**只在会被消费的两个时机**注册（check_post_roll / pre_resolve）：
        // 写错时机即当场报错（调用一个不存在的函数），不会静默丢请求。
        //
        // 注意这条声明**顺带开门**（既有语义，逐字保留）：声明缩放同时意味着
        // 「即使判定成功 / 未命中，也照常结算效果」。只想开门、不想缩放时用
        // force_effect()——两者是正交的两件事。
        if matches!(mount, LuaMount::CheckPostRoll | LuaMount::PreResolve) {
            push_try!("scale_effect", |factor: f64| {
                if !factor.is_finite() {
                    return Err(format!("缩放因子必须是有限数：{factor}"));
                }
                if factor < 0.0 {
                    return Err(format!("缩放因子不能为负：{factor}（0 = 完全抵消，1 = 不变）"));
                }
                Ok(LuaRequest::ScaleEffect { factor })
            });
            // 效果门（通用原语）：显式声明「即使判定成功 / 未命中，也照常结算效果一次」。
            // 与 scale_effect **正交**：它不缩放任何数值；两者可同时声明（开门 + 缩放）。
            // 同样只在会被消费的两个时机注册：写错时机即当场报错，不会静默丢请求。
            push!("force_effect", |(): ()| LuaRequest::ForceEffect);
        }

        // 施加即时效果：形状按 ImmediateEffect 校验，结算走同一条 resolve_effect 路径。
        push_try!("apply_effect", |(target, effect): (String, LuaValue)| {
            let value = lua_to_json(&effect).map_err(|e| e.to_string())?;
            serde_json::from_value::<ImmediateEffect>(value.clone())
                .map_err(|e| format!("即时效果形状非法（kind / amount / resource）：{e}"))?;
            Ok(LuaRequest::ApplyEffect { target, effect: value })
        });

        // 加减资源：可正可负、可指定目标（不受「当前 actor 消耗」限制）。
        push!("modify_resource", |(target, resource, amount): (String, String, i64)| {
            LuaRequest::ModifyResource { target, resource, amount }
        });

        // 标记原语（通用）：把 flag 置为某值——缺省 true（与旧 SetFlag 同义），
        // clear_flag 是 set_flag(flag, false) 的具名写法。引擎只当它是**世界级键**，
        // 与任何角色实体无关，也不解释值的含义（置位 / 清除由脚本按内容约定）。
        push_try!("set_flag", |(flag, value): (String, Option<LuaValue>)| {
            if flag.trim().is_empty() {
                return Err("set_flag 需要非空 flag".to_string());
            }
            let value = match value {
                Some(v) => lua_to_json(&v).map_err(|e| e.to_string())?,
                None => Value::Bool(true),
            };
            Ok(LuaRequest::ApplyEffect {
                target: String::new(),
                effect: json!({ "kind": "set_flag", "flag": flag, "value": value }),
            })
        });
        push!("clear_flag", |flag: String| LuaRequest::ApplyEffect {
            target: String::new(),
            effect: json!({ "kind": "set_flag", "flag": flag, "value": false }),
        });
        }

        // 调试输出：不产生副作用（避免脚本污染宿主 stdout）。
        host.set("log", lua.create_function(|_, _msg: String| Ok(())).map_err(lua_err)?)
            .map_err(lua_err)?;

        env.set("host", host).map_err(lua_err)?;
        Ok(env)
    }
}

/// 注册到某个挂载点的脚本。
#[derive(Debug, Clone, PartialEq)]
pub struct MountedScript {
    pub id: String,
    pub mount: LuaMount,
    pub source: String,
    /// 可选执行条件（`when`）：不成立即跳过；缺省恒执行。
    pub when: Option<CondExpr>,
}

/// 挂载点 → 脚本注册表（#02 ③）：同一挂载点按注册顺序执行，任一脚本报错即中断（fail-fast）。
#[derive(Debug, Clone, Default)]
pub struct LuaRegistry {
    scripts: Vec<MountedScript>,
}

impl LuaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: impl Into<String>, mount: LuaMount, source: impl Into<String>) {
        self.scripts.push(MountedScript { id: id.into(), mount, source: source.into(), when: None });
    }

    /// 注册一条故事书挂载点声明；id / source / mount 非法即拒绝并返回 false。
    ///
    /// `enabled: false` 的条目直接不装载（发布门仍会静态校验它的源码）。
    pub fn register_def(&mut self, def: &LuaMountDef) -> bool {
        if def.enabled == Some(false) {
            return false;
        }
        let Some(mount) = LuaMount::parse(&def.mount).filter(|m| m.is_declarable()) else {
            return false;
        };
        let id = def.id.trim();
        let source = def.source.trim();
        if id.is_empty() || source.is_empty() {
            return false;
        }
        self.scripts.push(MountedScript {
            id: id.to_string(),
            mount,
            source: def.source.clone(),
            when: def.when.clone(),
        });
        true
    }

    /// 从故事书 JSON 的 `lua_mounts` 声明装载规则集挂载点脚本。
    ///
    /// 发布门（validate + lua_lint）负责拦非法声明；运行期对坏条目静默跳过，
    /// 保证旧故事书 / 手工 JSON 不会因一条坏声明整本开不了局。
    /// 故事书随存档冻结，因此规则集脚本也随存档冻结（存档内嵌故事书已含则自然生效）。
    pub fn from_storybook(storybook: &Value) -> Self {
        let mut registry = Self::new();
        if let Some(list) = storybook.get("lua_mounts").and_then(Value::as_array) {
            for item in list {
                if let Ok(def) = serde_json::from_value::<LuaMountDef>(item.clone()) {
                    registry.register_def(&def);
                }
            }
        }
        registry
    }

    pub fn for_mount<'a>(&'a self, mount: LuaMount) -> impl Iterator<Item = &'a MountedScript> {
        self.scripts.iter().filter(move |s| s.mount == mount)
    }

    pub fn len(&self) -> usize {
        self.scripts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// 顺序执行某挂载点的脚本链；返回执行条数。中间报错则后续不跑。
    pub fn run_chain(
        &self,
        host: &LuaHost,
        mount: LuaMount,
        ctx: &LuaHostContext,
    ) -> Result<usize, EngineError> {
        self.run_chain_with(host, mount, ctx, &MountEnv::default())
    }

    /// 同上，但带挂载点附加值（`when` 闸门 + 判定结果快照）。
    ///
    /// `when` 不成立即跳过该脚本（不计入执行条数）；求值出错按「不成立」处理——
    /// 与回合末骨架求值同一口径，坏条件不该中断整轮结算。
    pub fn run_chain_with(
        &self,
        host: &LuaHost,
        mount: LuaMount,
        ctx: &LuaHostContext,
        env: &MountEnv<'_>,
    ) -> Result<usize, EngineError> {
        self.run_chain_status(host, mount, ctx, env, None)
    }

    /// 同上，但额外携带「正在结算的状态实例」（`status_tick` 用）。
    pub fn run_chain_status(
        &self,
        host: &LuaHost,
        mount: LuaMount,
        ctx: &LuaHostContext,
        env: &MountEnv<'_>,
        status: Option<&LuaStatusContext>,
    ) -> Result<usize, EngineError> {
        let mut executed = 0;
        for script in self.for_mount(mount) {
            if let Some(cond) = &script.when {
                match env.gate {
                    Some(gate) if gate(cond) => {}
                    _ => continue,
                }
            }
            let mut script_ctx = ctx.clone();
            script_ctx.script_id = script.id.clone();
            match status {
                Some(s) => host.run_hook_status(&script.source, mount, &script_ctx, env, s)?,
                None => host.run_hook_with(&script.source, mount, &script_ctx, env)?,
            }
            executed += 1;
        }
        Ok(executed)
    }
}

fn lua_err(e: mlua::Error) -> EngineError {
    EngineError::Lua(e.to_string())
}

fn read_int(t: &Table, key: &str) -> Result<i64, EngineError> {
    let value: LuaValue = t.get(key).map_err(lua_err)?;
    match value {
        LuaValue::Integer(i) => Ok(i),
        LuaValue::Number(n) => Ok(n as i64),
        other => Err(EngineError::Lua(format!(
            "check field '{key}' must be a number, got {}",
            other.type_name()
        ))),
    }
}

/// 当前角色模板的挂接表（kind key → definition id 列表）：故事书 `characters[]` 的原文。
///
/// 引擎**不理解** kind 的含义，只是把作者挂在模板上的数据原样转给脚本；
/// 缺快照 / 缺模板 / 没挂接都返回空表（脚本读到空，不是错误）。
fn attachment_map(data: &Value, actor: &Value) -> serde_json::Map<String, Value> {
    let template = actor
        .get("template_id")
        .or_else(|| actor.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    template
        .and_then(|tid| {
            data.get("characters")
                .and_then(Value::as_array)
                .and_then(|arr| arr.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(tid)))
        })
        .and_then(|c| c.get("attachments"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// 按 id 找一条 definition（原文返回；找不到 / id 为空返回 None）。
fn find_definition<'a>(data: &'a Value, id: &str) -> Option<&'a Value> {
    if id.is_empty() {
        return None;
    }
    data.get("definitions")
        .and_then(Value::as_array)?
        .iter()
        .find(|d| d.get("id").and_then(Value::as_str) == Some(id))
}

/// 按 kind 列出 definition（kind 为空 = 全部）；顺序即故事书声明顺序，确定性可重放。
fn definitions_of(data: &Value, kind: Option<&str>) -> Vec<Value> {
    let kind = kind.map(str::trim).filter(|s| !s.is_empty());
    data.get("definitions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|d| match kind {
                    Some(k) => d.get("kind").and_then(Value::as_str) == Some(k),
                    None => true,
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// 只读角色快照（GAP-A）里的角色查找：与引擎 `Session::find_character` 同口径——
/// 实例键（存档寻址键）/ 模板 id / 角色名 / instance_id 四种写法都能命中。
///
/// 另兼容把提示词里的「名字(id)」整串当 id 回填的形式（与 `actor_id_candidates` 同一处理）。
/// 查不到 / id 为空返回 None（脚本读到 nil），不抛错。
fn find_character_snapshot<'a>(data: &'a Value, id: &str) -> Option<(&'a str, &'a Value)> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let chars = data.get("characters").and_then(Value::as_object)?;
    for cand in id_candidates(id) {
        for (key, c) in chars {
            let hit = key.as_str() == cand
                || c.get("template_id").and_then(Value::as_str) == Some(cand)
                || c.get("name").and_then(Value::as_str) == Some(cand)
                || c.get("instance_id").and_then(Value::as_str) == Some(cand);
            if hit {
                return Some((key.as_str(), c));
            }
        }
    }
    None
}

/// id 原串 + 其中「名字(id)」括号内的 id（与 `Session::actor_id_candidates` 同口径）。
fn id_candidates(id: &str) -> Vec<&str> {
    let mut out = vec![id];
    for (open, close) in [('(', ')'), ('（', '）')] {
        let Some(start) = id.rfind(open) else { continue };
        let after = start + open.len_utf8();
        let Some(rel) = id[after..].find(close) else { continue };
        let inner = id[after..after + rel].trim();
        if !inner.is_empty() && inner != id {
            out.push(inner);
        }
    }
    out
}

/// 活跃遭遇（`active` 缺省为真，与引擎其它处的口径一致）。
///
/// 顺序 = 存档 BTreeMap 顺序：确定性、可重放。没有快照 / 没有遭遇 → 空表。
fn active_encounters(data: &Value) -> Vec<Value> {
    data.get("encounters")
        .and_then(Value::as_object)
        .map(|m| {
            m.values()
                .filter(|e| e.get("active").and_then(Value::as_bool).unwrap_or(true))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// 按 id 找一场**活跃**遭遇：先按存档寻址键，再按遭遇自身的 `id` 字段；查不到返回 None。
fn find_encounter<'a>(data: &'a Value, id: &str) -> Option<&'a Value> {
    if id.is_empty() {
        return None;
    }
    let is_active = |e: &Value| e.get("active").and_then(Value::as_bool).unwrap_or(true);
    let encs = data.get("encounters").and_then(Value::as_object)?;
    encs.get(id)
        .filter(|e| is_active(e))
        .or_else(|| encs.values().find(|e| e.get("id").and_then(Value::as_str) == Some(id) && is_active(e)))
}

/// serde_json::Value → Lua 值（对象为 string key 表，数组为 1 基表）。
fn json_to_lua(lua: &Lua, value: &Value) -> mlua::Result<LuaValue> {
    Ok(match value {
        Value::Null => LuaValue::Nil,
        Value::Bool(b) => LuaValue::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                LuaValue::Integer(i)
            } else {
                LuaValue::Number(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => LuaValue::String(lua.create_string(s)?),
        Value::Array(arr) => {
            let table = lua.create_table()?;
            for (i, item) in arr.iter().enumerate() {
                table.set(i + 1, json_to_lua(lua, item)?)?;
            }
            LuaValue::Table(table)
        }
        Value::Object(map) => {
            let table = lua.create_table()?;
            for (k, v) in map {
                table.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            LuaValue::Table(table)
        }
    })
}

/// Lua 值 → serde_json::Value（协议插件返回值归一化）。
///
/// 空表按数组处理：协议返回的 `intents = {}` 应等价于 `[]`，不能因空对象歧义而误判形状非法。
fn lua_to_json(value: &LuaValue) -> Result<Value, EngineError> {
    Ok(match value {
        LuaValue::Nil => Value::Null,
        LuaValue::Boolean(b) => Value::Bool(*b),
        LuaValue::Integer(i) => json!(*i),
        LuaValue::Number(n) => json!(*n),
        LuaValue::String(s) => Value::String(s.to_string_lossy()),
        LuaValue::Table(t) => {
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    let item: LuaValue = t.get(i).map_err(lua_err)?;
                    arr.push(lua_to_json(&item)?);
                }
                Value::Array(arr)
            } else {
                let mut entries = t.pairs::<LuaValue, LuaValue>();
                match entries.next() {
                    None => Value::Array(Vec::new()),
                    Some(first) => {
                        let mut map = serde_json::Map::new();
                        let insert = |k: LuaValue, v: Value, map: &mut serde_json::Map<String, Value>| match k {
                            LuaValue::String(s) => {
                                map.insert(s.to_string_lossy(), v);
                            }
                            LuaValue::Integer(i) => {
                                map.insert(i.to_string(), v);
                            }
                            LuaValue::Number(n) => {
                                map.insert(n.to_string(), v);
                            }
                            _ => {}
                        };
                        let (k, v) = first.map_err(lua_err)?;
                        insert(k, lua_to_json(&v)?, &mut map);
                        for pair in entries {
                            let (k, v) = pair.map_err(lua_err)?;
                            insert(k, lua_to_json(&v)?, &mut map);
                        }
                        Value::Object(map)
                    }
                }
            }
        }
        other => {
            return Err(EngineError::Lua(format!(
                "协议返回值含不支持的类型：{}",
                other.type_name()
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(actor: Value) -> LuaHostContext {
        LuaHostContext {
            script_id: "test-script".into(),
            actor,
            scene_id: "sc-1".into(),
            round: 3,
            ..Default::default()
        }
    }

    #[test]
    fn unsafe_libs_and_lua_random_are_unreachable() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        assert!(host
            .run_condition("return io == nil and os == nil and package == nil and debug == nil", &c)
            .unwrap());
        assert!(host
            .run_condition("return math.random == nil and math.randomseed == nil", &c)
            .unwrap());
        // 白名单库与基础库仍可用
        assert!(host
            .run_condition(
                "return string.len('abc') == 3 and math.floor(1.9) == 1 and type(pairs) == 'function'",
                &c,
            )
            .unwrap());
    }

    #[test]
    fn read_api_exposes_actor_state() {
        let host = LuaHost::new(1).unwrap();
        let mut c = ctx(json!({
            "id": "char-x",
            "attributes": { "str": 55, "trait": "多疑" },
            "resources": { "hp": 30 },
            "statuses": [ { "id": "burn" } ]
        }));
        c.relationships = vec![json!({
            "from_id": "char-x", "to_id": "char-y", "type": "好感", "value": 42
        })];
        assert!(host.run_condition("return host.get_attribute('str') == 55", &c).unwrap());
        assert!(host.run_condition("return host.get_attribute('trait') == '多疑'", &c).unwrap());
        assert!(host.run_condition("return host.get_resource('hp') == 30", &c).unwrap());
        assert!(host
            .run_condition("return host.has_status('burn') and not host.has_status('poison')", &c)
            .unwrap());
        assert!(host.run_condition("return host.scene_id == 'sc-1' and host.round == 3", &c).unwrap());
        assert!(host.run_condition("return host.relationship('char-x', 'char-y', '好感') == 42", &c).unwrap());
    }

    #[test]
    fn engine_rng_is_deterministic_and_recorded() {
        let a = LuaHost::new(42).unwrap();
        let b = LuaHost::new(42).unwrap();
        let c = ctx(json!({}));
        let script = "return { total = host.engine_rng(1, 1000), margin = 0 }";
        let va = a.run_check(script, &c).unwrap().total;
        let vb = b.run_check(script, &c).unwrap().total;
        assert_eq!(va, vb, "同种子同序列");
        assert_eq!(a.rng_handle().lock().unwrap().consumed.len(), 1);
    }

    #[test]
    fn check_script_normalizes_total_and_margin() {
        let host = LuaHost::new(7).unwrap();
        let c = ctx(json!({ "attributes": { "str": 70 } }));
        let out = host
            .run_check(
                "local base = host.get_attribute('str'); local r = host.engine_rng(1, 20); return { total = base + r, margin = (base + r) - 12 }",
                &c,
            )
            .unwrap();
        assert_eq!(out.margin, out.total - 12);
        assert!((71..=90).contains(&out.total));
    }

    #[test]
    fn check_script_must_return_table() {
        let host = LuaHost::new(7).unwrap();
        let c = ctx(json!({}));
        assert!(host.run_check("return 3", &c).is_err());
    }

    #[test]
    fn write_requests_go_to_outbox() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        host.run_hook(
            "host.request_cost('res-mana', 10); host.apply_status('char-x', 'burn', 3, 'turns'); host.trigger_event('character_death')",
            LuaMount::PostResolve,
            &c,
        )
        .unwrap();
        let reqs = host.drain_requests();
        assert_eq!(reqs.len(), 3);
        match &reqs[0] {
            LuaRequest::Cost { resource, amount } => {
                assert_eq!(resource, "res-mana");
                assert_eq!(*amount, 10);
            }
            other => panic!("expected cost, got {other:?}"),
        }
        assert!(matches!(reqs[1], LuaRequest::ApplyStatus { .. }));
        assert!(matches!(reqs[2], LuaRequest::TriggerEvent { .. }));
        assert!(host.drain_requests().is_empty());
    }

    #[test]
    fn per_script_storage_persists_across_calls() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        let script = "host.storage.n = (host.storage.n or 0) + 1; return { total = host.storage.n, margin = 0 }";
        let first = host.run_check(script, &c).unwrap().total;
        let second = host.run_check(script, &c).unwrap().total;
        assert_eq!((first, second), (1, 2));

        let mut other = ctx(json!({}));
        other.script_id = "other-script".into();
        assert_eq!(host.run_check(script, &other).unwrap().total, 1);
    }

    #[test]
    fn instruction_budget_stops_infinite_loop() {
        let host = LuaHost::with_limits(
            1,
            SandboxLimits { memory_bytes: 8 * 1024 * 1024, max_instructions: 50_000 },
        )
        .unwrap();
        let c = ctx(json!({}));
        let err = host.run_hook("while true do end", LuaMount::PostResolve, &c).unwrap_err();
        assert!(format!("{err}").contains("instruction budget"), "got: {err}");
    }

    #[test]
    fn memory_limit_is_enforced() {
        let host = LuaHost::with_limits(
            1,
            SandboxLimits { memory_bytes: 1024 * 1024, max_instructions: 500_000_000 },
        )
        .unwrap();
        let c = ctx(json!({}));
        assert!(host
            .run_hook("local t = {}; for i = 1, 1e7 do t[i] = i end", LuaMount::PostResolve, &c)
            .is_err());
    }

    #[test]
    fn registry_runs_mount_chain_in_order_and_fails_fast() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        let mut registry = LuaRegistry::new();
        registry.register("pre-a", LuaMount::PreResolve, "host.trigger_event('a')");
        registry.register("pre-b", LuaMount::PreResolve, "host.trigger_event('b')");
        registry.register("post", LuaMount::PostResolve, "host.trigger_event('c')");
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.run_chain(&host, LuaMount::CheckPreRoll, &c).unwrap(), 0);
        assert_eq!(registry.run_chain(&host, LuaMount::PreResolve, &c).unwrap(), 2);
        let events: Vec<String> = host
            .drain_requests()
            .into_iter()
            .filter_map(|r| match r {
                LuaRequest::TriggerEvent { event, .. } => Some(event),
                _ => None,
            })
            .collect();
        assert_eq!(events, vec!["a".to_string(), "b".to_string()]);

        // fail-fast：第一个脚本报错，后续脚本不执行
        let mut failing = LuaRegistry::new();
        failing.register("bad", LuaMount::PreResolve, "host.trigger_event('x'); error('boom')");
        failing.register("never", LuaMount::PreResolve, "host.trigger_event('y')");
        assert!(failing.run_chain(&host, LuaMount::PreResolve, &c).is_err());
        let events: Vec<String> = host
            .drain_requests()
            .into_iter()
            .filter_map(|r| match r {
                LuaRequest::TriggerEvent { event, .. } => Some(event),
                _ => None,
            })
            .collect();
        assert_eq!(events, vec!["x".to_string()]);
    }

    #[test]
    fn protocol_mount_roundtrips() {
        assert_eq!(LuaMount::from_str("protocol"), LuaMount::Protocol);
        assert_eq!(LuaMount::Protocol.as_str(), "protocol");
    }

    #[test]
    fn protocol_mount_is_read_only_and_exposes_context() {
        let host = LuaHost::new(1).unwrap();
        let mut c = ctx(json!({}));
        c.present = vec!["char-a".to_string(), "char-b".to_string()];
        c.controlled = "米拉(char-mira)".to_string();
        let src = "function protocol.preamble(ctx) return tostring(#ctx.present) .. '|' .. ctx.controlled .. '|' .. tostring(host.present[2]) end\nfunction protocol.parse(raw) host.request_cost('mp', 1); return { intents = {} } end";
        let p = host.run_protocol_preamble(src, &c).unwrap();
        assert_eq!(p, "2|米拉(char-mira)|char-b");
        assert!(host.run_protocol_parse(src, "[]", &c).is_err(), "协议挂载点不得写入世界");
        assert!(host.drain_requests().is_empty(), "协议挂载点不得产生 LuaRequest");
    }

    #[test]
    fn protocol_parse_converts_lua_tables_to_json() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        let src = "function protocol.parse(raw) return { intents = { { type = 'narrate', content = raw } } } end";
        let out = host.run_protocol_parse(src, "你好", &c).unwrap();
        assert_eq!(out["intents"][0]["type"], "narrate");
        assert_eq!(out["intents"][0]["content"], "你好");
        // 空 intents 表按数组处理（不能因空对象歧义而误判形状）。
        let src2 = "function protocol.parse(raw) return { intents = {} } end";
        let out2 = host.run_protocol_parse(src2, "[]", &c).unwrap();
        assert_eq!(out2["intents"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn storybook_mounts_load_into_registry() {
        let sb = json!({
            "lua_mounts": [
                { "id": "a", "mount": "check_pre_roll", "source": "host.modify_check('keep_high')" },
                { "id": "b", "mount": "check_post_roll", "source": "host.modify_check('add', 6)", "enabled": true },
                { "id": "off", "mount": "event", "source": "host.trigger_event('x')", "enabled": false },
                { "id": "tick", "mount": "status_tick", "source": "host.remove_status(host.actor.id, host.status_id)" },
                { "id": "tend", "mount": "turn_end", "source": "return 1" },
                { "id": "send", "mount": "scene_end", "source": "return 1" },
                { "id": "bad-mount", "mount": "check_after_roll", "source": "return 1" },
                { "id": "protocol-mount", "mount": "protocol", "source": "return 1" },
                { "id": "", "mount": "event", "source": "return 1" },
                { "id": "no-source", "mount": "event", "source": "   " }
            ]
        });
        let registry = LuaRegistry::from_storybook(&sb);
        assert_eq!(registry.len(), 5, "非法 / 禁用条目不装载");
        assert_eq!(registry.for_mount(LuaMount::CheckPreRoll).count(), 1);
        assert_eq!(registry.for_mount(LuaMount::CheckPostRoll).count(), 1);
        assert_eq!(registry.for_mount(LuaMount::StatusTick).count(), 1);
        assert_eq!(registry.for_mount(LuaMount::TurnEnd).count(), 1);
        assert_eq!(registry.for_mount(LuaMount::SceneEnd).count(), 1);
        // 没有 lua_mounts 的故事书 → 空注册表（旧故事书行为逐字不变）。
        assert!(LuaRegistry::from_storybook(&json!({ "skills": [] })).is_empty());
        // 挂载点名解析：未知值不再静默回落 PreResolve。
        assert_eq!(LuaMount::parse("check_pre_roll"), Some(LuaMount::CheckPreRoll));
        assert_eq!(LuaMount::parse("check_after_roll"), None);
        assert_eq!(LuaMount::from_str("check_after_roll"), LuaMount::PreResolve);
    }

    /// 时机原语（L4）：三个新挂载点名可声明、可解析、可回写；未知名仍被拦。
    #[test]
    fn timing_mount_names_round_trip_and_unknown_still_rejected() {
        for (name, mount) in [
            ("status_tick", LuaMount::StatusTick),
            ("turn_end", LuaMount::TurnEnd),
            ("scene_end", LuaMount::SceneEnd),
        ] {
            assert_eq!(LuaMount::parse(name), Some(mount));
            assert_eq!(LuaMount::from_str(name), mount);
            assert_eq!(mount.as_str(), name);
            assert!(mount.is_declarable(), "{name} 必须可由故事书声明");
            assert!(LuaMount::DECLARABLE.contains(&name));
            // 严格解析对未知名仍返回 None（发布门据它报错）。
            assert_eq!(LuaMount::parse("status_ticks"), None);
            assert_eq!(LuaMount::parse("turn_ends"), None);
            assert_eq!(LuaMount::parse("scene_ends"), None);
        }
    }

    /// status_tick 上下文：脚本能读到被结算的角色与状态实例；其他挂载点看不到它。
    #[test]
    fn status_tick_context_is_visible_to_scripts() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({
            "id": "char-a",
            "name": "米拉",
            "resources": { "hp": 30 },
            "statuses": [ { "id": "grasp", "name": "缠绕", "turns_left": 3 } ]
        }));
        let status = LuaStatusContext {
            id: "grasp".into(),
            name: "缠绕".into(),
            turns_left: Some(3),
            scenes_left: None,
            unit: "turns",
            remaining: 2,
        };
        host.run_hook_status(
            "assert(host.mount == 'status_tick'); assert(host.event == 'status_tick');              assert(host.actor.id == 'char-a'); assert(host.status.id == 'grasp');              assert(host.status.name == '缠绕'); assert(host.status.turns_left == 3);              assert(host.status.scenes_left == nil); assert(host.status.unit == 'turns');              assert(host.status.remaining == 2); assert(host.status_id == 'grasp');              assert(host.status_turns_left == 3); assert(host.status_scenes_left == nil);              assert(host.actor.statuses[1].id == 'grasp');              host.remove_status(host.actor.id, host.status_id)",
            LuaMount::StatusTick,
            &c,
            &MountEnv::default(),
            &status,
        )
        .unwrap();
        assert_eq!(
            host.drain_requests(),
            vec![LuaRequest::RemoveStatus { target: "char-a".into(), status: "grasp".into() }]
        );
        // 其他挂载点没有状态快照（向后兼容：旧脚本读不到新字段，也不会因此报错）。
        assert!(host
            .run_condition("return host.status == nil and host.status_id == nil", &c)
            .unwrap());
    }

    #[test]
    fn mount_when_gate_decides_whether_script_runs() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        let mut registry = LuaRegistry::new();
        registry.register("always", LuaMount::Event, "host.trigger_event('always')");
        registry.register_def(&LuaMountDef {
            id: "gated".into(),
            mount: "event".into(),
            source: "host.trigger_event('gated')".into(),
            when: Some(CondExpr::FlagSet { flag: "on".into() }),
            enabled: None,
        });
        let events = |host: &LuaHost| -> Vec<String> {
            host.drain_requests()
                .into_iter()
                .filter_map(|r| match r {
                    LuaRequest::TriggerEvent { event, .. } => Some(event),
                    _ => None,
                })
                .collect()
        };

        // 没有闸门 → 声明了 when 的脚本一律跳过（不瞎跑）。
        assert_eq!(registry.run_chain(&host, LuaMount::Event, &c).unwrap(), 1);
        assert_eq!(events(&host), vec!["always".to_string()]);

        let gate_false = |_: &CondExpr| false;
        let env = MountEnv { gate: Some(&gate_false), check: None, event: None, resolved_effects: None };
        assert_eq!(registry.run_chain_with(&host, LuaMount::Event, &c, &env).unwrap(), 1);
        assert_eq!(events(&host), vec!["always".to_string()]);

        let gate_true = |_: &CondExpr| true;
        let env = MountEnv { gate: Some(&gate_true), check: None, event: None, resolved_effects: None };
        assert_eq!(registry.run_chain_with(&host, LuaMount::Event, &c, &env).unwrap(), 2);
        assert_eq!(events(&host), vec!["always".to_string(), "gated".to_string()]);
    }

    #[test]
    fn generic_write_primitives_push_requests() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        host.run_hook(
            "host.modify_check('keep_high'); host.modify_check('add', 6); host.modify_check('dc', -2);              host.apply_effect('char-b', { kind = 'damage', amount = '3d6', resource = 'hp' });              host.modify_resource('char-c', 'mana', -3)",
            LuaMount::CheckPreRoll,
            &c,
        )
        .unwrap();
        let reqs = host.drain_requests();
        assert_eq!(reqs.len(), 5);
        assert_eq!(reqs[0], LuaRequest::ModifyCheck { mode: CheckModifier::KeepHigh, amount: 0 });
        assert_eq!(reqs[1], LuaRequest::ModifyCheck { mode: CheckModifier::Add, amount: 6 });
        assert_eq!(reqs[2], LuaRequest::ModifyCheck { mode: CheckModifier::Difficulty, amount: -2 });
        match &reqs[3] {
            LuaRequest::ApplyEffect { target, effect } => {
                assert_eq!(target, "char-b");
                assert_eq!(effect["kind"], "damage");
                assert_eq!(effect["amount"], "3d6");
            }
            other => panic!("expected apply_effect, got {other:?}"),
        }
        assert_eq!(
            reqs[4],
            LuaRequest::ModifyResource { target: "char-c".into(), resource: "mana".into(), amount: -3 }
        );
        // 引擎不认识的名字 / 非法效果形状 → 当场报错。
        assert!(host.run_hook("host.modify_check('luck')", LuaMount::CheckPreRoll, &c).is_err());
        assert!(host
            .run_hook("host.apply_effect('x', { kind = 'bogus' })", LuaMount::PreResolve, &c)
            .is_err());
    }

    /// 判定 C4：结果覆盖也是通用动作——引擎只认识「强制成功 / 强制失败」，
    /// 不认识「自然 20 / 大成功」这类规则集词汇。
    #[test]
    fn modify_check_force_modes_are_generic_actions() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        host.run_hook(
            "host.modify_check('force_fail'); host.modify_check('force_success')",
            LuaMount::CheckPostRoll,
            &c,
        )
        .unwrap();
        assert_eq!(
            host.drain_requests(),
            vec![
                LuaRequest::ModifyCheck { mode: CheckModifier::ForceFail, amount: 0 },
                LuaRequest::ModifyCheck { mode: CheckModifier::ForceSuccess, amount: 0 },
            ]
        );
        assert_eq!(CheckModifier::parse("force_success"), Some(CheckModifier::ForceSuccess));
        assert_eq!(CheckModifier::parse("force_fail"), Some(CheckModifier::ForceFail));
        // 不在通用动作表里的名字一律拒绝（规则集词汇到不了引擎；'luck' 已在上一个用例覆盖）。
        assert_eq!(CheckModifier::parse("override"), None);
    }

    #[test]
    fn check_snapshot_is_visible_to_post_roll_scripts() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        let check = LuaCheckContext {
            attribute: "str".into(),
            kind: Some(CheckKind::Save),
            resolved: true,
            expr: Some("1d20".into()),
            total: 12,
            target: 15,
            margin: -3,
            result: false,
            level: Some(SuccessLevel::Barely),
            rolls: vec![9],
        };
        // 判定前：没有快照。
        assert!(host
            .run_condition("return host.check == nil and host.check_result == nil", &c)
            .unwrap());
        // 判定后：脚本能读到判定结果（分档语义由 Lua 自己决定）。
        let env = MountEnv { gate: None, check: Some(&check), event: None, resolved_effects: None };
        host.run_hook_with(
            "assert(host.check.total == 12); assert(host.check.margin == -3);              assert(host.check.result == false); assert(host.check_level == 'barely');              assert(host.check_kind == 'save'); assert(host.check.rolls[1] == 9);              host.modify_resource('char-a', 'mana', host.check.total)",
            LuaMount::CheckPostRoll,
            &c,
            &env,
        )
        .unwrap();
        assert_eq!(
            host.drain_requests(),
            vec![LuaRequest::ModifyResource {
                target: "char-a".into(),
                resource: "mana".into(),
                amount: 12
            }]
        );
    }

    #[test]
    fn bytecode_is_rejected() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        assert!(host.run_hook("\u{1b}LuaQ", LuaMount::PostResolve, &c).is_err());
    }

    /// GAP-C：只读开放内容口——当前角色模板的挂接 + 按 id / kind 读 definition。
    /// 引擎不认识 kind，也不解释 fields / modifiers，只把原文交给脚本。
    #[test]
    fn open_content_read_api_exposes_attachments_and_definitions() {
        let host = LuaHost::new(1).unwrap();
        host.set_read_data(json!({
            "characters": [
                { "id": "pc-1", "attachments": { "trait": ["tr-strong"] } }
            ],
            "definitions": [
                { "id": "tr-strong", "kind": "trait", "name": "强壮",
                  "fields": { "bonus": 3 },
                  "modifiers": [ { "target": "str", "value": 1 } ] }
            ]
        }));
        let c = ctx(json!({ "id": "pc-1", "template_id": "pc-1" }));
        assert!(host
            .run_condition("local a = host.get_attachments(); return a.trait[1] == 'tr-strong'", &c)
            .unwrap());
        assert!(host
            .run_condition(
                "local l = host.get_attachment('trait'); return #l == 1 and l[1] == 'tr-strong'",
                &c
            )
            .unwrap());
        assert!(host.run_condition("return host.get_attachment('nope') == nil", &c).unwrap());
        assert!(host
            .run_condition(
                "local d = host.get_definition('tr-strong'); return d.kind == 'trait' and d.name == '强壮' and d.fields.bonus == 3 and d.modifiers[1].target == 'str' and d.modifiers[1].value == 1",
                &c
            )
            .unwrap());
        assert!(host.run_condition("return host.get_definition('missing') == nil", &c).unwrap());
        assert!(host
            .run_condition(
                "local l = host.list_definitions('trait'); return #l == 1 and l[1].id == 'tr-strong'",
                &c
            )
            .unwrap());
        assert!(host.run_condition("return #host.list_definitions() == 1", &c).unwrap());
        assert!(host.run_condition("return #host.list_definitions('other') == 0", &c).unwrap());
        // 另一个模板 / 没注入快照的宿主：读到 nil / 空表，不报错（编辑器试跑等入口）。
        let other = ctx(json!({ "id": "pc-2", "template_id": "pc-2" }));
        assert!(host.run_condition("return host.get_attachment('trait') == nil", &other).unwrap());
        let bare = LuaHost::new(1).unwrap();
        assert!(bare
            .run_condition(
                "return host.get_definition('tr-strong') == nil and host.get_attachment('trait') == nil and #host.get_attachments() == 0",
                &c
            )
            .unwrap());
    }

    /// GAP-F（读取侧）：事件名 + 通用事实快照只下发给 Event 挂载点；其他挂载点读到 nil。
    #[test]
    fn event_context_is_exposed_only_to_event_mounts() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        assert!(host
            .run_condition("return host.event_name == nil and host.event_data == nil", &c)
            .unwrap());
        let data = json!({
            "enemy": { "id": "e1", "name": "灰狼", "template_id": "mon-wolf" },
            "encounter": { "id": "enc-1", "name": "遭遇" }
        });
        let env = MountEnv {
            event: Some(LuaEventContext { name: "enemy_defeated", data: Some(&data) }),
            ..Default::default()
        };
        host.run_hook_with(
            "assert(host.event_name == 'enemy_defeated');              assert(host.event_data.enemy.template_id == 'mon-wolf');              assert(host.event_data.enemy.name == '灰狼');              assert(host.event_data.encounter.id == 'enc-1')",
            LuaMount::Event,
            &c,
            &env,
        )
        .unwrap();
        // 没有附加事实的事件：只有名字，data 仍是 nil。
        let env = MountEnv {
            event: Some(LuaEventContext { name: "scene", data: None }),
            ..Default::default()
        };
        host.run_hook_with(
            "assert(host.event_name == 'scene'); assert(host.event_data == nil)",
            LuaMount::Event,
            &c,
            &env,
        )
        .unwrap();
    }

    /// GAP-H：标记原语是通用的「把 flag 置为某值」——缺省置真，给值即置为该值。
    #[test]
    fn set_and_clear_flag_are_generic_flag_writes() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        host.run_hook(
            "host.set_flag('a'); host.set_flag('b', false); host.set_flag('c', 3); host.clear_flag('d')",
            LuaMount::Event,
            &c,
        )
        .unwrap();
        let reqs = host.drain_requests();
        assert_eq!(reqs.len(), 4);
        let effect = |r: &LuaRequest| match r {
            LuaRequest::ApplyEffect { effect, .. } => effect.clone(),
            other => panic!("expected apply_effect, got {other:?}"),
        };
        assert_eq!(effect(&reqs[0]), json!({ "kind": "set_flag", "flag": "a", "value": true }));
        assert_eq!(effect(&reqs[1]), json!({ "kind": "set_flag", "flag": "b", "value": false }));
        assert_eq!(effect(&reqs[2]), json!({ "kind": "set_flag", "flag": "c", "value": 3 }));
        assert_eq!(effect(&reqs[3]), json!({ "kind": "set_flag", "flag": "d", "value": false }));
        // 空 flag 当场报错（不静默丢请求）。
        assert!(host.run_hook("host.set_flag('  ')", LuaMount::Event, &c).is_err());
    }

    /// GAP-A：只读世界事实口——按任意 id 形态读角色实例（与引擎 find_character 同口径）。
    ///
    /// 引擎不认识 kind / 状态 / 遭遇的语义，只做查表搬运；查不到返回 nil，不抛错。
    #[test]
    fn world_fact_read_api_resolves_any_character_id_form() {
        let host = LuaHost::new(1).unwrap();
        host.set_world_facts(json!({
            "characters": {
                "char-a": {
                    "instance_id": "inst-char-a", "template_id": "pc-mira", "name": "米拉", "kind": "pc",
                    "attributes": { "str": 70 }, "resources": { "hp": 30 },
                    "statuses": [ { "id": "bless", "name": "祝福" } ],
                    "location_id": "loc-1", "present": true
                }
            },
            "flags": { "met-isa": true },
            "encounters": {}
        }));
        let c = ctx(json!({}));
        // 四种 id 写法（实例键 / 模板 id / 角色名 / instance_id）都能命中同一实例。
        for id in ["char-a", "pc-mira", "米拉", "inst-char-a"] {
            let script = format!(
                "local c = host.get_character('{id}')\n\
                 return c ~= nil and c.id == 'char-a' and c.attributes.str == 70 \n\
                 and c.resources.hp == 30 and c.statuses[1].id == 'bless' \n\
                 and c.location_id == 'loc-1' and c.kind == 'pc' and c.present == true"
            );
            assert!(host.run_condition(&script, &c).unwrap(), "id 形态 {id} 应命中同一实例");
        }
        // 提示词里常见的「名字(id)」整串回填也能命中。
        assert!(host
            .run_condition("return host.get_character('米拉(char-a)').template_id == 'pc-mira'", &c)
            .unwrap());
        // 查不到 / id 为空 → nil（不抛错）。
        assert!(host.run_condition("return host.get_character('nobody') == nil", &c).unwrap());
        assert!(host.run_condition("return host.get_character('') == nil", &c).unwrap());
    }

    /// GAP-A：标记与活跃遭遇的只读口；查不到返回 nil / 空表，不抛错。
    #[test]
    fn world_fact_read_api_exposes_flags_and_active_encounters() {
        let host = LuaHost::new(1).unwrap();
        host.set_world_facts(json!({
            "characters": {},
            "flags": { "met-isa": true, "gate-open": false, "count": 3 },
            "encounters": {
                "enc-1": { "id": "enc-1", "name": "洞穴", "active": true,
                    "enemies": [ { "id": "e1", "name": "灰狼", "hp": 3, "max": 11, "ac": 12 } ] },
                "enc-old": { "id": "enc-old", "name": "旧账", "active": false, "enemies": [] }
            }
        }));
        let c = ctx(json!({}));
        assert!(host.run_condition("return host.get_flag('met-isa') == true", &c).unwrap());
        // false 是合法值，不是「查不到」：必须原样返回，不能变成 nil。
        assert!(host.run_condition("return host.get_flag('gate-open') == false", &c).unwrap());
        assert!(host.run_condition("return host.get_flag('count') == 3", &c).unwrap());
        assert!(host.run_condition("return host.get_flag('missing') == nil", &c).unwrap());
        assert!(host
            .run_condition("local f = host.list_flags(); return f['met-isa'] == true and f.count == 3", &c)
            .unwrap());
        // 只列活跃遭遇（含敌人与 hp）；已结束的遭遇按 id 也查不到。
        assert!(host
            .run_condition(
                "local l = host.list_encounters(); return #l == 1 and l[1].id == 'enc-1' and l[1].enemies[1].hp == 3",
                &c
            )
            .unwrap());
        assert!(host.run_condition("return host.get_encounter('enc-1').name == '洞穴'", &c).unwrap());
        assert!(host.run_condition("return host.get_encounter('enc-old') == nil", &c).unwrap());
        assert!(host.run_condition("return host.get_encounter('missing') == nil", &c).unwrap());

        // 没有注入快照的宿主（编辑器试跑 / 无相关脚本）：nil / 空表，不报错。
        let bare = LuaHost::new(1).unwrap();
        assert!(bare
            .run_condition(
                "return host.get_character('x') == nil and host.get_flag('x') == nil \n\
                 and next(host.list_flags()) == nil and host.get_encounter('x') == nil \n\
                 and #host.list_encounters() == 0",
                &c
            )
            .unwrap());
    }

    /// 只读性：这些 API 只查表，不产生任何 LuaRequest、不改世界状态。
    #[test]
    fn world_fact_read_api_produces_no_requests() {
        let host = LuaHost::new(1).unwrap();
        host.set_world_facts(json!({
            "characters": { "char-a": { "name": "米拉", "statuses": [] } },
            "flags": { "k": 1 },
            "encounters": { "enc-1": { "id": "enc-1", "active": true, "enemies": [] } }
        }));
        let c = ctx(json!({}));
        host.run_hook(
            "local a = host.get_character('char-a'); local b = host.get_character('missing'); \n\
             local f = host.get_flag('k'); local l = host.list_flags(); \n\
             local e = host.get_encounter('enc-1'); local es = host.list_encounters()",
            LuaMount::PreResolve,
            &c,
        )
        .unwrap();
        assert!(host.drain_requests().is_empty(), "只读 API 不得产生任何 LuaRequest");
    }

    /// GAP-L：host.target 的只读快照与 host.actor 同级完整（状态 / 属性 / 资源 / 位置 / 种类 / 在场）。
    #[test]
    fn actor_and_target_snapshots_carry_the_same_facts() {
        let host = LuaHost::new(1).unwrap();
        let snapshot = |name: &str| {
            json!({
                "instance_id": format!("inst-{name}"),
                "template_id": format!("tmpl-{name}"),
                "name": name,
                "kind": "pc",
                "attributes": { "dex": 40 },
                "resources": { "hp": 7 },
                "statuses": [ { "id": "off-guard", "name": "疏于防备" } ],
                "location_id": "loc-1",
                "present": true
            })
        };
        let mut c = ctx(snapshot("米拉"));
        c.target = Some(snapshot("目标"));
        assert!(host
            .run_condition(
                "local function ok(c) return c ~= nil and c.attributes.dex == 40 and c.resources.hp == 7 \n\
                 and c.statuses[1].id == 'off-guard' and c.location_id == 'loc-1' \n\
                 and c.kind == 'pc' and c.present == true end \n\
                 return ok(host.actor) and ok(host.target)\n\
                 and host.actor.id == 'inst-米拉' and host.target.id == 'inst-目标'",
                &c
            )
            .unwrap());
    }

    // ---------- GAP-E：效果缩放原语 + PostResolve 只读快照 ----------

    /// scale_effect 只在**会消费它**的两个时机注册（check_post_roll / pre_resolve）：
    /// 写错时机即当场报错（不是静默丢请求）。因子必须是有限非负数。
    #[test]
    fn scale_effect_is_registered_only_where_it_is_consumed() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        for mount in [LuaMount::CheckPostRoll, LuaMount::PreResolve] {
            host.run_hook("host.scale_effect(0.5)", mount, &c).unwrap();
            assert_eq!(
                host.drain_requests(),
                vec![LuaRequest::ScaleEffect { factor: 0.5 }],
                "{mount:?} 应收到缩放声明"
            );
        }
        // 其他时机没有这个 API：调用即报错，作者立刻看到时机写错了。
        for mount in [LuaMount::CheckPreRoll, LuaMount::PostResolve, LuaMount::Event] {
            let err = host.run_hook("host.scale_effect(0.5)", mount, &c).unwrap_err();
            assert!(err.to_string().contains("scale_effect"), "{mount:?} 应报错，实际：{err}");
            assert!(host.drain_requests().is_empty(), "{mount:?} 不该产生请求");
        }
        // 因子校验：负 / NaN / 无穷 一律当场报错（引擎不接受说不清的因子）。
        for bad in ["host.scale_effect(-1)", "host.scale_effect(0/0)", "host.scale_effect(1/0)"] {
            let err = host.run_hook(bad, LuaMount::CheckPostRoll, &c).unwrap_err();
            assert!(err.to_string().contains("缩放因子"), "{bad} → {err}");
        }
        assert!(host.drain_requests().is_empty(), "非法因子不得留下请求");
    }

    /// 效果门与效果缩放是**两个正交的动作**，各有各的 API：
    /// - force_effect() 只开门（「判定成功也照常结算效果」），不缩放任何数值；
    /// - scale_effect(f) 开门 **且** 按 f 缩放（既有语义，逐字保留——这是「作者脚枪」
    ///   的来源，现在有了只开门的显式写法）。
    ///
    /// 两者都只在会被消费的两个时机（check_post_roll / pre_resolve）注册：
    /// 写错时机即当场报错，不是静默丢请求。同时声明互不吞并、顺序原样。
    #[test]
    fn force_effect_is_registered_only_where_it_is_consumed() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        for mount in [LuaMount::CheckPostRoll, LuaMount::PreResolve] {
            host.run_hook("host.force_effect()", mount, &c).unwrap();
            assert_eq!(
                host.drain_requests(),
                vec![LuaRequest::ForceEffect],
                "{mount:?} 应收到效果门声明"
            );
            host.run_hook("host.force_effect(); host.scale_effect(0.5)", mount, &c).unwrap();
            assert_eq!(
                host.drain_requests(),
                vec![LuaRequest::ForceEffect, LuaRequest::ScaleEffect { factor: 0.5 }],
                "{mount:?} 两个声明互不吞并，顺序原样"
            );
        }
        // 其他时机没有这个 API：调用即报错，作者立刻看到时机写错了。
        for mount in [LuaMount::CheckPreRoll, LuaMount::PostResolve, LuaMount::Event] {
            let err = host.run_hook("host.force_effect()", mount, &c).unwrap_err();
            assert!(err.to_string().contains("force_effect"), "{mount:?} 应报错，实际：{err}");
            assert!(host.drain_requests().is_empty(), "{mount:?} 不该产生请求");
        }
    }

    /// resolved_effects 是 PostResolve 专属的只读事实：其他时机读它是 nil（旧脚本零影响）。
    #[test]
    fn resolved_effects_is_exposed_only_to_post_resolve() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        assert!(host.run_condition("return host.resolved_effects == nil", &c).unwrap());
        for mount in [LuaMount::CheckPostRoll, LuaMount::PreResolve, LuaMount::Event] {
            host.run_hook("assert(host.resolved_effects == nil)", mount, &c).unwrap();
        }
        let snapshot = json!({
            "deltas": [
                { "domain": "character", "entity_id": "char-b", "field": "resources.hp",
                  "op": "add", "value": -3 }
            ],
            "statuses": [],
            "modifiers": [],
            "rng_consumed": 1,
            "factor": 0.5
        });
        let env = MountEnv { resolved_effects: Some(&snapshot), ..Default::default() };
        host.run_hook_with(
            "assert(host.resolved_effects.deltas[1].value == -3); 
             assert(host.resolved_effects.deltas[1].field == 'resources.hp'); 
             assert(host.resolved_effects.factor == 0.5); 
             assert(host.resolved_effects.rng_consumed == 1)",
            LuaMount::PostResolve,
            &c,
            &env,
        )
        .unwrap();
        assert!(host.drain_requests().is_empty(), "只读快照不得产生请求");
        // 没有声明因子时 factor 下发 nil（脚本据此区分「没缩放」与「因子 0」）。
        let no_factor = json!({ "deltas": [], "statuses": [], "modifiers": [], "rng_consumed": 0, "factor": null });
        let env = MountEnv { resolved_effects: Some(&no_factor), ..Default::default() };
        host.run_hook_with(
            "assert(host.resolved_effects.factor == nil)",
            LuaMount::PostResolve,
            &c,
            &env,
        )
        .unwrap();
    }
}

