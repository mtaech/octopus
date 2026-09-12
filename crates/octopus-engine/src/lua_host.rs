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
use serde_json::{json, Value};

use crate::error::EngineError;
use crate::rng::DeterministicRng;

/// 每多少个 VM 指令触发一次预算钩子（越小越精确、开销越大）。
const HOOK_STEP: u32 = 1000;
/// 脚本私有存储区在 Lua 注册表中的键。
const STORAGE_REGISTRY_KEY: &str = "__octopus_script_storage";

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

/// Lua 挂载点（#04 的 7 个时机 + 协议插件；Validate 阶段无 Lua）。
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
    /// 协议插件（preamble / parse / normalize）：只读，不产生任何 LuaRequest。
    Protocol,
}

impl LuaMount {
    /// 解析挂载点字符串（未知值回落到 PreResolve）。
    pub fn from_str(s: &str) -> Self {
        match s {
            "check_pre_roll" => LuaMount::CheckPreRoll,
            "check_post_roll" => LuaMount::CheckPostRoll,
            "check" => LuaMount::Check,
            "pre_resolve" => LuaMount::PreResolve,
            "post_resolve" => LuaMount::PostResolve,
            "event" => LuaMount::Event,
            "condition" => LuaMount::Condition,
            "protocol" => LuaMount::Protocol,
            _ => LuaMount::PreResolve,
        }
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
            LuaMount::Protocol => "protocol",
        }
    }
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
        })
    }

    pub fn limits(&self) -> SandboxLimits {
        self.limits
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
        Ok(match self.eval_value(script, ctx, LuaMount::Condition)? {
            LuaValue::Nil => false,
            LuaValue::Boolean(b) => b,
            _ => true,
        })
    }

    /// 自定义判定脚本：只接受归一化的 { total, margin }（#12 契约）。
    pub fn run_check(&self, script: &str, ctx: &LuaHostContext) -> Result<LuaCheckOutcome, EngineError> {
        let value = self.eval_value(script, ctx, LuaMount::Check)?;
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
        self.eval_value(script, ctx, mount).map(|_| ())
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
        let env = self.make_env(ctx, LuaMount::Protocol)?;
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

    fn eval_value(&self, script: &str, ctx: &LuaHostContext, mount: LuaMount) -> Result<LuaValue, EngineError> {
        let env = self.make_env(ctx, mount)?;
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
    fn make_env(&self, ctx: &LuaHostContext, mount: LuaMount) -> Result<Table, EngineError> {
        let lua = &self.lua;
        let env = lua.create_table().map_err(lua_err)?;
        // 写全局变量只落在 env；读全局走 __index → globals（此时 globals 已是最小白名单）。
        let mt = lua.create_table().map_err(lua_err)?;
        mt.set("__index", lua.globals()).map_err(lua_err)?;
        env.set_metatable(Some(mt)).map_err(lua_err)?;

        let host = lua.create_table().map_err(lua_err)?;
        host.set("script_id", ctx.script_id.clone()).map_err(lua_err)?;
        host.set("mount", mount.as_str()).map_err(lua_err)?;
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
        for key in ["instance_id", "template_id", "name", "kind", "attributes", "resources", "statuses"] {
            if let Some(v) = ctx.actor.get(key) {
                actor_ref.set(key, json_to_lua(lua, v)?).map_err(lua_err)?;
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
            for key in ["instance_id", "template_id", "name", "kind"] {
                if let Some(v) = target.get(key) {
                    target_ref.set(key, json_to_lua(lua, v)?).map_err(lua_err)?;
                }
            }
            host.set("target", target_ref).map_err(lua_err)?;
        }

        if let Some(skill) = &ctx.skill {
            host.set("definition", json_to_lua(lua, skill)?).map_err(lua_err)?;
        }

        let relationships = ctx.relationships.clone();
        host.set(
            "relationship",
            lua.create_function(move |_, (from, to, kind): (String, String, String)| {
                let value = relationships.iter().find_map(|r| {
                    let same = r.get("from_id").and_then(Value::as_str) == Some(from.as_str())
                        && r.get("to_id").and_then(Value::as_str) == Some(to.as_str())
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
        }

        // 调试输出：不产生副作用（避免脚本污染宿主 stdout）。
        host.set("log", lua.create_function(|_, _msg: String| Ok(())).map_err(lua_err)?)
            .map_err(lua_err)?;

        env.set("host", host).map_err(lua_err)?;
        Ok(env)
    }
}

/// 注册到某个挂载点的脚本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedScript {
    pub id: String,
    pub mount: LuaMount,
    pub source: String,
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
        self.scripts.push(MountedScript { id: id.into(), mount, source: source.into() });
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
        let mut executed = 0;
        for script in self.for_mount(mount) {
            let mut script_ctx = ctx.clone();
            script_ctx.script_id = script.id.clone();
            host.run_hook(&script.source, mount, &script_ctx)?;
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
    fn bytecode_is_rejected() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx(json!({}));
        assert!(host.run_hook("\u{1b}LuaQ", LuaMount::PostResolve, &c).is_err());
    }
}
