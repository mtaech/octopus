# Rust 的 Lua 集成生态调研

Type: research
Status: resolved

## Question

调研 Rust 集成 Lua 的现状与最佳实践：mlua vs rlua（功能/维护/性能/Lua 版本与 LuaJIT 支持）、沙箱与权限隔离（scoped env、资源限制）、热加载、FFI 边界、错误处理。为「引擎职责边界」提供事实依据。

## Answer

见 [research/09-lua-integration-research.md](../research/09-lua-integration-research.md)。

结论：锁 mlua（rlua 官方弃用、0.20 起只是 mlua 薄包装）；主后端 lua54/lua55，LuaJIT 语言面为 5.1 语义、与 5.4 不互通，需在「引擎职责边界」另行取舍；沙箱内建（安全模式 + set_memory_limit + set_hook + scoped env 白名单）；热加载用 load→eval/exec + set_environment + set_name，仅接受文本源码（字节码不受信）；错误统一为 mlua::Error，CallbackError 携带 Lua 回溯栈。
