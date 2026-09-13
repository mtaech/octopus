# Rust 集成 Lua 的现状与最佳实践（mlua vs rlua）

> Type: research
> 面向问题：为「引擎职责边界」（issues/02-engine-lua-boundary）提供事实依据。
> 范围：mlua vs rlua 对比、沙箱与权限隔离、热加载、FFI 边界、错误处理。
> 采信标准：仅官方文档 / 源码 / crate 文档 / 语言规范；每条结论标注出处编号，对应文末「来源」。

## TL;DR

- **用 mlua，不要用 rlua。** rlua 已官方弃用，其 0.20 起只是「薄封装、直接 re-export mlua」的过渡层 [2]。新项目直接依赖 mlua。
- mlua 是 rlua 的 fork，官方定位「安全（尽可能）、高层、易用、实用、灵活」的 Lua 绑定，支持 Lua 5.1–5.5（含 LuaJIT）与 Luau [1]。
- 沙箱是内建一等能力：安全模式（`Lua::new`/`new_with`）默认拒绝 `debug`/`ffi` 并禁用 C 模块加载 [6][7]；资源限制有 `set_memory_limit`（字节级内存上限）与 `set_hook`（按指令数截断）[6]；scoped env 通过 `Chunk::set_environment` 设置 `_ENV` 上值实现 [8]。
- 热加载没有专门的「watch」API，由 `Lua::load` → `Chunk::eval/exec` + `set_environment` + `set_name` 组合实现 [8]。
- FFI 边界靠 `IntoLua`/`FromLua`/`IntoLuaMulti`/`FromLuaMulti` + `UserData`（derive）+ `LuaSerdeExt`；错误处理靠单一 `mlua::Error` 枚举，`CallbackError` 携带 Lua 回溯栈 [5][9]。

---

## 1. mlua vs rlua

### 1.1 定位与关系

- mlua 官方定义：「一套对 [Lua](https://www.lua.org) 的 Rust 绑定，目标是提供一个**尽可能安全**、高层、易用、实用且灵活的 API」；README 明确写明 **“Started as an rlua fork”**（始于 rlua 的分支）[1]。
- rlua 官方 README 明确 **“rlua is now deprecated in favour of mlua”**；自 0.20 起它只是 **“a thin transitional wrapper around mlua”**，直接 re-export mlua [2]。

### 1.2 维护活跃度（crates.io 一手数据）

| | mlua | rlua |
|---|---|---|
| 最新版本 | 0.12.1 | 0.20.1 |
| 最近更新（updated_at） | 2026-08-29 | 2024-07-20 |
| 累计下载 | 6 371 639 | 781 652 |
| 近 90 天下载（recent_downloads） | 1 363 244 | 33 221 |
| 首次发布 | 2019-11-04 | 2017-05-22 |
| 仓库 | https://github.com/mlua-rs/mlua | （同组织，已弃用/过渡） |

来源：[3][4]。结论：mlua 仍在活跃迭代（2026 年 8 月仍在更新，近 90 天下载量是 rlua 的约 40 倍）；rlua 最后一次更新停留在 2024 年 7 月，仅作为迁移垫片存在。

### 1.3 功能与能力对比

rlua 0.20 起功能上等价于 mlua（re-export），因此对比实质是「历史 rlua 0.19 及更早」与 mlua。迁移差异由 rlua README 官方列出，核心三点 [2]：

1. `Lua::context()` 不再需要，原先 `Context` 上的方法直接挂在 `Lua` 上（rlua 0.20 提供 `RluaCompat` 扩展 trait 作临时兼容）。
2. `ToLua` trait 重命名为 `IntoLua`，`to_lua` 方法改为 `into_lua`（rlua 0.20 提供 `ToLua` 别名与 `ToLuaCompat`）。
3. mlua 对「是否阻止从 Lua 加载 C 库/编译模块、是否捕获 Rust panic」有不同的默认值与开关，见 `Lua::new_with` 与 unsafe 变体。

mlua 相对历史 rlua 的增量能力（源自 mlua README / lib.rs）[1][5]：async/await（协程 + `Thread`/`AsyncThread`）、serde 互转（`LuaSerdeExt`）、Luau 支持、`send`（`Lua: Send + Sync`）、模块模式（写原生 Lua 模块 `cdylib`）、`chunk!` 等过程宏。

### 1.4 Lua 版本与 LuaJIT 支持

mlua 支持矩阵（README）[1]：

- 通过 feature flag 选择后端，**默认不启用任何 feature**，必须显式开启其一：`lua55`、`lua54`、`lua53`、`lua52`、`lua51`、`luajit`、`luajit52`（带部分 5.2 兼容的 LuaJIT）、`luau`、`luau-jit`、`luau-vector4`。
- README 原句：“supports Lua 5.5, 5.4, 5.3, 5.2, 5.1 (including LuaJIT) and Luau” [1]。
- 链接方式：默认用 `pkg-config` 找系统 Lua；可用 `LUA_LIB`/`LUA_LIB_NAME`/`LUA_LINK` 环境变量指定；也可 `vendored` feature 用 `lua-src`/`luajit-src` 从源码静态编译 [1]。
- MSRV：Rust 1.88+ [1]。

上游语言版本事实（lua.org 官方版本史）[14]：

- **Lua 5.4**：2020-06-29 发布；最后一个版本 5.4.9（2026-08-25），官方声明 **“There will be no further releases of Lua 5.4.”**。
- **Lua 5.5**：2025-12-22 发布，当前 5.5.1（2026-08-03）；新特性含全局变量声明、具名可变参数表、更紧凑数组、增量式主 GC。

LuaJIT 事实（luajit.org 官方扩展文档，LuaJIT 2.1）[15]：

- **完全向上兼容 Lua 5.1**；支持全部标准库函数与完整 Lua/C API；在链接/动态加载层面与 Lua 5.1 **ABI 兼容**（用标准 Lua 头文件编译的 C 模块可被 Lua 或 LuaJIT 加载）。
- 额外扩展模块：`bit.*`（位运算）、`ffi.*`（FFI 库）、`jit.*`、string buffer 等 [15]。
- 因此：选 LuaJIT 意味着**语言层是 5.1 语义 + 部分扩展**，而非 5.4 语法；这是本项目的关键取舍点。

### 1.5 性能

- 关键事实：**rlua ≥ 0.20 只是 re-export mlua 的包装层，运行时执行的是同一份 mlua 代码，二者性能相同**；不存在需要独立测量的 mlua-vs-rlua 差异 [2]。
- mlua 官方 README 把基准测试指向仓库 `script-bench-rs`（“Benchmarks” 链接），该仓库将 mlua（Lua 5.4 与 Luau）与 boa、koto、rhai、roto、rquickjs、wasmi、wasmtime 等嵌入脚本引擎做评测 [1][16]。
- 注意：`script-bench-rs` 是第三方基准仓库（mlua 官方 README 引用的评测入口），且不单独包含 rlua；跨引擎绝对数字应以实际 `uv run bench.py` 复现为准 [16]。此处不引用二手综述里的相对数字，避免失真。

---

## 2. 沙箱与权限隔离

### 2.1 标准库白名单：safe 模式 vs unsafe 模式

mlua 用 `StdLib` 位标志精确控制加载哪些标准库（`src/stdlib.rs`）[7]：

- 标志位：`COROUTINE`、`TABLE`、`IO`、`OS`、`STRING`、`UTF8`、`BIT`（5.2/LuaJIT/Luau）、`MATH`、`PACKAGE`、`DEBUG`、`JIT`（LuaJIT）、`FFI`（LuaJIT）、`NONE`、`ALL`、`ALL_SAFE`，另有 Luau 专属 `BUFFER`/`VECTOR`/`INTEGER`。
- 官方在源码中标注 **`DEBUG`（bit 31）与 `FFI`（bit 30）为 unsafe**；`ALL_SAFE = (1 << 30) - 1`，即「除 FFI 与 DEBUG 外的全部库」[7]。

`Lua` 构造三档（`src/state.rs`）[6]：

- `Lua::new()` = `new_with(StdLib::ALL_SAFE, LuaOptions::default())` —— **安全默认**。
- `new_with(libs, options)`：安全构造，显式拒绝 `DEBUG`（返回 `Error::SafetyError`），LuaJIT 下拒绝 `FFI`；若加载了 `PACKAGE` 则调用 `disable_c_modules()` **禁止从 Lua 加载 C 模块** [6]。
- `unsafe fn unsafe_new()` / `unsafe_new_with(StdLib::ALL, ...)`：无安全保证，允许加载 C 模块 [6]。

结论：对「引擎」而言，应始终走 `new_with` 安全路径，并在 `StdLib` 里进一步裁剪（例如去掉 `IO`/`OS`/`PACKAGE`），而不是使用 `unsafe_new`。

### 2.2 scoped env（按脚本隔离全局环境）

`Chunk::set_environment(env: Table)`（`src/chunk.rs` 官方文档）[8]：

> 「在 Lua ≥ 5.2 中主 chunk 恰好有一个上值（upvalue），该上值用作 chunk 内的 `_ENV` 变量。默认它被设为全局环境。调用此方法把该 `_ENV` 上值改为给定表，chunk 内变量将引用给定环境而非全局环境。**所有全局变量（包括标准库）都在 `_ENV` 中查找**，因此使用自定义环境时通常需要自行填充该表。」

即：每个脚本通过 `lua.load(code).set_environment(sandbox_table)` 获得独立命名空间，脚本间互不可见，且可决定注入哪些「白名单函数」。这是 scoped env 的官方机制 [8]。

### 2.3 资源限制（内存 / CPU）

内存上限 —— `Lua::set_memory_limit(limit: usize) -> Result<usize>`（`src/state.rs`）[6]：

- 单位**字节**；一旦分配会超过上限即产生 `Error::MemoryError`。
- 返回**上一个上限**（0 表示无上限）。
- 「模块模式下不生效（Lua 状态由外部管理）」，否则返回 `Error::MemoryControlNotAvailable`。

CPU/指令上限 —— `Lua::set_hook(triggers: HookTriggers, callback)`（`src/state.rs` + `src/debug.rs`）[6][10]：

- `HookTriggers.every_nth_instruction: Option<u32>`：每执行 N 条 VM 指令调用一次钩子 [10]。
- 官方文档：「钩子函数可以报错，该错误会传播到钩子触发时正在执行的 Lua 代码。**可借此实现一种受限的执行上限：设置 every_nth_instruction 并在达到指令上限时返回 Err**」[6]。
- `VmState`（`src/types.rs`）只有 `Continue` 与 `Yield`（`Yield` 需 Lua 5.3+ 或 Luau）两个变体 [11]；终止执行是通过钩子返回 `Err`，而非 `Terminate` 变体。
- 注意：`every_nth_instruction` 设得过低会带来很高的开销（官方标注 Performance 警告）[10]。

### 2.4 Luau 专属：`sandbox` 与 `set_interrupt`

- `Lua::sandbox(enabled: bool)` 与 `Lua::set_interrupt(callback)` 均为 **Luau-only**（`#[cfg(feature = "luau")]`）[6]。
- 即：若本项目选择标准 Lua（5.4/5.5），沙箱要靠 `StdLib` 白名单 + `set_memory_limit` + `set_hook` 组合，而非 `sandbox()` API [6][7]。

### 2.5 字节码安全警示

`Chunk::set_mode(ChunkMode::Text | Binary)`（`src/chunk.rs`）[8]：

> 「Lua **不检查二进制 chunk 内部代码的一致性**。运行恶意构造的字节码可能使解释器崩溃。」

结论：沙箱若接受用户脚本，应只接受文本源码（`ChunkMode::Text`），不要加载不受信的预编译字节码 [8]。

---

## 3. 热加载

mlua 没有内置「文件监听/自动重载」，热加载由加载 API 组合实现（`src/chunk.rs`）[8]：

- **加载**：`Lua::load(chunk)` 接受任何 `AsChunk`——`&str`/`String`（源码）、`&[u8]`/`Vec<u8>`（源码或字节码）、`&Path`/`PathBuf`（自动读文件，chunk 名自动带 `@` 前缀）。返回 `Chunk` [8]。
- **执行**：`Chunk` 是惰性的（`#[must_use]`），必须调 `exec()`（执行语句块）、`eval()`（求表达式值）、`call(args)` 或 `into_function()`（只编译不执行，得到可复用的 `Function`）[8]。
- **命名**：`Chunk::set_name(name)`，前缀 `@` 表文件路径、`=` 表自定义名，用于更友好的错误回溯 [8]。
- **环境**：`Chunk::set_environment(table)` 切换 `_ENV`，见 2.2 [8]。

热加载模式（由上述官方 API 推导，非官方「watch」特性）：保存/编辑脚本后，重新 `lua.load(src).set_name(...).set_environment(same_env).exec()` 覆盖旧行为；需要「预热」时可 `into_function()` 缓存编译产物；配合 `Chunk::set_mode(ChunkMode::Text)` 保证只接受文本源码（见 2.5）。运行时状态若存在 `RegistryKey`/用户数据，可在重载时按需保留或重建（见第 4 节 FFI 边界）。

---

## 4. FFI 边界

### 4.1 类型转换 trait

- `IntoLua` / `FromLua`：Rust 类型 ↔ 单个 Lua 值的双向转换，为大量标准库类型实现 [5]。
- `IntoLuaMulti` / `FromLuaMulti`：Rust 类型 ↔ **任意数量** Lua 值的转换（多返回值/变参）[5]。
- 注意命名：历史 `ToLua` 已更名为 `IntoLua`（见 1.3）[2]。

### 4.2 自定义 UserData

- `UserData` trait 让自定义 Rust 类型暴露给 Lua；方法用 `UserDataMethods`，字段用 `UserDataFields` API 添加 [5]。
- 过程宏（`macros` feature）：`#[derive(UserData)]`、`#[derive(FromLua)]`、`#[userdata_impl]`（在 `impl` 块内注册方法/字段）[5]。
- `AnyUserData` 用于运行时对 UserData 的借用/类型检查（错误见第 5 节）[9]。

### 4.3 serde 互转

- `serde` feature 下，`LuaSerdeExt`（实现于 `Lua`）提供 Rust 类型 ↔ Lua 值的 serde 序列化/反序列化；任意 `serde::Serialize`/`Deserialize` 类型均可转换；`Value` 等类型也实现 `serde::Serialize` [1][5]。

### 4.4 async / 多线程 / 模块模式

- **async**：`Lua::create_async_function` 创建返回 `Future` 的非阻塞函数；用 `Function::call_async` 系列或轮询 `AsyncThread`，可接任意 runtime（Tokio 等）；基于 Lua 协程 + `Thread`，全 Lua 版本（含 Luau）可用 [1][5]。
- **send**：默认 `Lua` 为 `!Send`；开 `send` feature 后 `Lua: Send + Sync`（对 `Function` 要求 `Send`、对 `UserData` 要求 `Send + Sync`）；内部用**可重入互斥锁**同步对 VM 的访问 [5]。
- **module**：写可被 Lua 加载的原生 `cdylib` 模块，`lua_module` 宏（`mlua_derive`）；注意 `module` 与 `send` 不兼容（编译期报错），且 module 模式下 `set_memory_limit` 不生效 [5][6]。

### 4.5 生命周期与作用域

- 所有 `Lua` 值带 `'lua` 生命周期，绑定到所属 `Lua` 状态，Rust 借检查器在编译期防止跨状态/悬垂引用 [5]。
- `Lua::scope` 提供临时值的受控作用域；`RegistryKey` 把值存进 Lua 注册表以跨越作用域存活，但跨不同 `Lua` 状态使用会得到 `Error::MismatchedRegistryKey` [5][6][9]。

---

## 5. 错误处理

统一错误类型 `mlua::Error`（`src/error.rs`），所有可失败操作返回 `Result<T, Error>`。变体（节选关键项）[9]：

- `SyntaxError { message, incomplete_input }` —— 解析期；`incomplete_input` 表示「很可能再追加输入就能修好」，专为 REPL 设计 [9]。
- `RuntimeError(String)` —— 运行时（`LUA_ERRRUN`），如对 `nil` 做索引/调用。
- `MemoryError(String)` —— 内存耗尽（`LUA_ERRMEM`），与 `set_memory_limit` 联动（见 2.3）。
- `GarbageCollectorError`（5.2/5.3）、`SafetyError`（安全模式违规，如尝试加载 `debug`）、`MemoryControlNotAvailable`、`StackError`、`BindError`。
- `BadArgument { to, pos, name, cause }` —— 调用时参数错误，定位是第几个参数。
- `FromLuaConversionError { from, to, message }` —— Lua 值无法转成目标 Rust 类型。
- `CallbackError { traceback, cause }` —— Rust 回调返回 `Err` 并以 Lua 错误抛出，**携带 Lua 调用栈回溯**；`Display` 会打印根因 + 回溯 [9]。
- `ExternalError(Arc<DynStdError>)` —— 包装任意外部错误；`WithContext` 用于附加上下文。
- 若干 UserData 借用错误（`UserDataBorrowError`/`UserDataBorrowMutError`/`UserDataDestructed`/`UserDataTypeMismatch`）与 `MismatchedRegistryKey` [9]。

辅助方法 [9]：

- `Error::external(err)` 包装外部错误对象；`Error::downcast_ref::<T>()` 穿透包装层取具体错误类型；`Error::chain()` 迭代嵌套错误链。
- `Error` 实现标准 `std::error::Error`（含 `source()`），可接入 `anyhow`/`thiserror` 生态；`anyhow` feature 提供 `anyhow::Error` → Lua 的转换 [1][9]。

---

## 6. 对本项目（引擎职责边界）的落地建议

以下为基于上述一手事实的结论，供 issues/02 决策：

1. **锁定 mlua**：rlua 已弃用且只是 re-export 包装层，无独立维护或性能价值 [2][3][4]。
2. **后端版本**：优先 `lua54`（成熟、语法稳定）或 `lua55`（最新）作为主后端；若未来需要 LuaJIT 的 FFI/极致性能，注意其语言面是 **5.1 语义**，与 5.4 语法不互通用 [1][14][15]。建议用 feature flag 保持后端可切换，业务 Lua 代码控制在兼容子集内。
3. **沙箱**：一律 `Lua::new_with(裁剪后的 StdLib, LuaOptions::default())` 安全构造（去掉 `IO`/`OS`/`PACKAGE`/`DEBUG`/`FFI`），配合 `set_memory_limit`（内存预算）+ `set_hook(every_nth_instruction)`（指令预算，钩子里返回 `Err` 终止），并仅加载文本源码（拒绝字节码）[6][7][8][10]。
4. **scoped env**：每个脚本 `load(...).set_environment(env_table)`，在 `env_table` 里只注入引擎暴露的白名单 API，实现「技能/钩子」的权限隔离 [8]。
5. **热加载**：复用 `Lua::load` + `set_name` + `set_environment` + `exec`/`into_function`；用 `into_function` 缓存编译产物，重载时替换函数引用 [8]。
6. **错误处理**：在 Rust 侧 `match` 关键变体（`SyntaxError`/`RuntimeError`/`CallbackError` 的 `traceback`），把 Lua 回溯栈完整回传引擎/AI 侧便于诊断 [9]。

---

## 来源（全部为官方文档 / 源码 / crate 元数据 / 规范）

1. mlua README（官方）— https://github.com/mlua-rs/mlua
2. rlua README（官方弃用说明）— https://github.com/mlua-rs/rlua
3. mlua crates.io 元数据 — https://crates.io/crates/mlua
4. rlua crates.io 元数据 — https://crates.io/crates/rlua
5. mlua 源码 `src/lib.rs`（v0.12.1）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/lib.rs
6. mlua 源码 `src/state.rs`（v0.12.1）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/state.rs
7. mlua 源码 `src/stdlib.rs`（v0.12.1）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/stdlib.rs
8. mlua 源码 `src/chunk.rs`（v0.12.1）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/chunk.rs
9. mlua 源码 `src/error.rs`（v0.12.1）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/error.rs
10. mlua 源码 `src/debug.rs`（v0.12.1，`HookTriggers`）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/debug.rs
11. mlua 源码 `src/types.rs`（v0.12.1，`VmState`）— https://github.com/mlua-rs/mlua/blob/v0.12.1/src/types.rs
12. mlua crate 文档（docs.rs）— https://docs.rs/mlua
13. mlua FAQ（官方）— https://github.com/mlua-rs/mlua/blob/master/FAQ.md
14. Lua 官方版本史 — https://www.lua.org/versions.html
15. LuaJIT 官方扩展文档（LuaJIT 2.1）— https://luajit.org/extensions.html
16. script-bench-rs（mlua README 指向的基准测试）— https://github.com/khvzak/script-bench-rs
