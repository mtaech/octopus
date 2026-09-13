# 协议接口（Protocol Interface）实现规格 — P2

> 这是 `docs/narrative-contract.md` §5 的实现级展开，供 P2 直接照着做。
> 前置：P0（`storybook.narrative.sections` + 槽位装配 + 校验）已实现；P1（`when` / 变体 / 存档偏好）串行进行中。

## 1. 目标

允许**故事书覆盖 AI 的输出协议**（格式说明 + 解析），但收敛到一个固定接口：
三种模式出口都是 `Vec<Intent>`，引擎的校验/结算层完全不感知协议模式。

## 2. 数据（故事书）

```ts
// storybook.narrative.protocol
interface ProtocolConfig {
  mode: 'declarative' | 'lua'
  /** declarative：声明使用哪些意图（须是引擎已知集合的子集）；可选追加说明 */
  intents?: string[]
  instructions?: string
  /** lua：协议插件源码 */
  lua?: string
}
```

- 缺省（无 `protocol`）= 引擎默认协议，行为与现状一致。
- `declarative`：覆盖**解释**（协议说明文本），不覆盖**解析**；解析仍用引擎 `parse_intents`，但按 `intents` 白名单拒绝白名单外的意图。
- `lua`：`preamble` 与 `parse` 都由插件实现。

## 3. 引擎端口

新增 `crates/octopus-engine/src/protocol.rs`：

```rust
pub enum ProtocolMode { Default, Declarative, Lua }

pub struct ProtocolSpec {
    pub mode: ProtocolMode,
    pub intents: Vec<String>,        // declarative 白名单
    pub instructions: Option<String>,
    pub lua: Option<String>,
}

pub trait ProtocolAdapter: Send + Sync {
    /// 注入系统层的协议说明（替代硬编码 preamble 里的协议段）。
    fn preamble(&self, ctx: &TurnContext) -> String;
    /// 模型原始输出 → 意图；这是引擎唯一入口。
    fn parse(&self, raw: &str, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError>;
    /// 解析后归一化（可选，默认恒等）。
    fn normalize(&self, intents: Vec<Intent>, _ctx: &TurnContext) -> Vec<Intent> { intents }
}
```

三个实现：

| 实现 | preamble | parse |
|---|---|---|
| `DefaultProtocol` | 现有 `SYSTEM_PREAMBLE` 的协议段 | `rig_provider::parse_intents`（下沉到 engine，供复用） |
| `DeclarativeProtocol` | 引擎模板（按 `intents` 渲染）+ `instructions` | `parse_intents` + 白名单过滤（含白名单外的意图 → 记该意图为拒绝） |
| `LuaProtocol` | Lua `protocol.preamble(ctx)` | Lua `protocol.parse(raw)` + 可选 `normalize` |

**注意**：`parse_intents` 目前在 `crates/octopus-ai/src/rig_provider.rs`（私有）。P2 应把它移到 `octopus-engine`（或 `octopus-types`）供三处复用，`octopus-ai` 改为调用。

## 4. Lua 插件接口（复用既有沙箱）

复用 `crates/octopus-engine/src/lua_host.rs` 的 `LuaHost` + `SandboxLimits`；新增挂载点：

```rust
pub enum LuaMount { /* 现有 7 个 */ , Protocol }
```
`LuaMount::from_str("protocol")` / `as_str()` 同步。

插件必须定义：

```lua
-- 必选：协议说明（注入系统层）
function protocol.preamble(ctx) -> string end
-- 必选：原始输出 → { intents = {...} }
function protocol.parse(raw) -> { intents = { ... }, error = nil|string } end
-- 可选：解析后归一化
function protocol.normalize(intents, ctx) -> intents end
```

上下文：对齐判定器 Lua 的只读快照。P2 先复用 `LuaHostContext`（`scene_id` / `round` / `actor` / `target` / `relationships` 已具备），
新增只读 `present`（在场角色 id 列表）与 `controlled`；**不**暴露世界写入口（`LuaRequest` 在协议挂载点全部拒绝）。

> **缓存约束（重要）**：`protocol.preamble(ctx)` 的返回值就是请求的**系统层**，位于整个请求的最前面。
> 供应商的上下文缓存是**前缀缓存**——系统层一变，后面（整段会话历史）全部按原价重算。
> 所以 preamble 必须是**不含逐回合变化内容**的稳定文本：不要把 `ctx.round` / `ctx.present` / `ctx.scene_id`
> 拼进去（哪怕看起来很方便）。逐回合信息走用户消息——`turn_prompt` 已经注入了场景、在场角色、人物设定、
> 世界词条、相关往事等。需要「按在场角色改变可用动作」时，用工具白名单，而不是把名单拼进 preamble。

错误处理：`parse` 抛错、超时（`SandboxLimits.max_instructions`）、或返回形状非法 → `EngineError::Ai`，与现有「意图解析失败」同路径（不 panic）。

## 5. 装配与调用点

| 位置 | 改动 |
|---|---|
`TurnContext` | 携带解析后的 `ProtocolSpec`（或无 = 默认） |
`rig_provider::complete` | 系统层 preamble = `adapter.preamble(ctx)`（把现有协议段抽出）；用户消息仍含叙事要求等 |
`rig_provider` 解析处 | `adapter.parse(&text, ctx)` 取代直接 `parse_intents` |
`Session` | 按 `storybook.narrative.protocol` 构造 adapter，放进 `TurnContext`（或 provider 内部缓存） |

## 6. 格式校验（发布门）

`validate.rs` 是**纯函数**、不执行 Lua；因此分两层：

### 6.1 纯静态（`validate.rs` 内）
| # | 规则 | 级别 |
|---|---|---|
| 1 | `mode` 合法；`declarative` 必须有 `intents`；`lua` 必须有 `lua` | Error |
| 2 | `declarative.intents` ⊆ 引擎已知意图集合 | Error |
| 3 | `declarative.intents` 至少含一个叙事意图（`narrate`/`speak`/`emote`） | Error |
| 4 | `lua_lint::lint_script` 语法 + 禁用 API | Error |
| 5 | `lua` 文本包含 `protocol.preamble` 与 `protocol.parse` 定义（粗检；精确靠 6.2） | Warning |
| 6 | `instructions` / `lua` 超出建议长度 | Warning |

### 6.2 动态一致性（新增引擎入口）

```rust
// crates/octopus-engine/src/protocol.rs
/// 用固定样例跑一遍 Lua 协议插件，验证它能稳定产出意图。
pub fn check_protocol_conformance(lua_src: &str) -> Vec<ValidationIssue>;
```

- 内部：`LuaHost::with_limits(seed, SandboxLimits::default())` 载入脚本，跑三组样例：
  1. 合法意图数组 → 必须产出非空 `intents`，且每个元素能反序列化为 `Intent`；
  2. 围栏代码块 + 前后废话 → 必须能提取；
  3. 垃圾输入 → 必须返回明确错误，不得 panic / 超时。
- 由 **api 层 publish** 调用（`publish_storybook`），把结果并入发布校验的 issues；draft 保存不跑（避免每次自动保存都执行 Lua）。
- 预算：给协议插件单独的上限（如 `max_instructions: 500_000`），与技能 Lua 分离。

## 7. 白名单语义（declarative）

- 白名单**只过滤最终意图**，不改解析。被过滤的意图记入 `System` 警告事件（`code: "intent_not_allowed"`），便于创作者排查。
- 必须保留：`finish_turn`（回合收束）；建议校验提示（Warning）若缺失 `narrate`。

## 8. 前端

| 落点 | 内容 |
|---|---|
`NarrativePanel` 或新「协议」tab | mode 选择（默认 / 声明式 / Lua）；声明式：意图多选 + 说明 textarea；Lua：代码编辑器（复用 `CodeEditor.vue` + `LuaHookEditor.vue` 的 lint 面板） |
校验展示 | 复用 `ValidationDock`；6.2 的一致性错误在发布时出现在同一清单 |
类型 | `ProtocolConfig` 加入 `NarrativeContract` 类型 |

## 9. 文件级改动清单

| 文件 | 改动 |
|---|---|
`crates/octopus-types/src/lib.rs` | 无（协议只存故事书 JSON） |
`crates/octopus-engine/src/protocol.rs` | 新增：`ProtocolSpec` / `ProtocolAdapter` / 三实现 / `check_protocol_conformance` |
`crates/octopus-engine/src/lua_host.rs` | `LuaMount::Protocol` + `from_str`/`as_str`；协议挂载点拒绝 `LuaRequest` |
`crates/octopus-engine/src/lib.rs` | 导出 |
`crates/octopus-ai/src/rig_provider.rs` | 用 adapter 的 preamble/parse；`parse_intents` 迁移 |
`crates/octopus-engine/src/session.rs` | 从故事书构造 adapter，放进 `TurnContext` |
`crates/octopus-engine/src/validate.rs` | §6.1 静态规则 |
`crates/octopus-api/src/lib.rs` | publish 时跑 §6.2 |
`frontend/src/types/index.ts` + 面板 | 协议编辑 |

## 10. 测试清单

1. `declarative`：白名单以外的意图被过滤并记警告；白名单内正常。
2. Lua：`parse` 吃三种样例（合法 / 围栏 / 垃圾）各自符合预期；超时样例被拒。
3. `check_protocol_conformance`：坏脚本返回 Error，好脚本空。
4. 回归：默认协议下现有 `turn_prompt` / `parse_intents` 行为不变（既有测试全绿）。
5. `validate.rs`：非法 mode / 空 intents / 非子集 / 缺叙事意图 各自报错。

## 11. 待定（开工前定）

1. `LuaMount::Protocol` 与技能 Lua 共用 `LuaHost` 还是独立实例？倾向：共用 host、独立挂载点（复用预算与 RNG）。
2. `LuaHostContext` 扩展字段（`present`/`controlled`）还是新建 `ProtocolContext`？倾向扩展 `LuaHostContext`，避免两套。
3. `instructions` 是否计入 `turn_token_budget` 并参与裁剪？倾向参与（可裁）。
