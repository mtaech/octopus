# Rust 模块划分与测试策略

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 16, 17

## Question

定义 Rust 端（axum 后端 + 引擎）的 crate / 模块划分与测试策略。各核心票已落定，需要拍板：
① crate 切分：单 crate 多模块 vs workspace 多 crate——引擎（状态权威 / 命令管线 / 结算 / Lua 宿主）、AI provider 抽象（rig）、上下文与记忆管线、迁移与升格器（#14 版本链）、API 层各自落在哪。
② 模块依赖方向：状态权威、命令管线、结算、事件发射（待 #17）、记忆管线、迁移升格器之间的依赖纪律（谁可以依赖谁，哪里是单向缝）。
③ 测试策略分层：命令管线确定性重放测试、版本链 golden fixtures（#14 已锁）、Lua 钩子沙箱测试、SSE 端到端测试（#11）各自落在哪层、用什么形态。

## Answer

决议（grilling 两轮全部落定）：

### ① crate 切分：小 workspace（B）

```
octopus-types   # 纯 serde 共享类型（#04 意图 schema、#17 事件包络/事件类型、state_changes delta、驳回码、RNG 记录）；ts-rs 生成前端类型
octopus-engine  # 状态权威、命令管线、结算、Lua 宿主、RNG、事件发射；存储与升格器为内部模块；只暴露端口
octopus-ai      # rig CompletionModel 适配 + EmbeddingBackend 实现（fastembed 本地/Provider）+ 上下文渲染与摘要；依赖 engine，engine 不反向依赖
octopus-api     # axum 路由/SSE/确认门 POST/装配组合根 + 薄 octopus bin
```

记忆管线逻辑（写入时索引、请求前检索 top5 注入）留在 engine（由 commit 阶段与上下文构建驱动）；模型类调用（embedding、场景压缩摘要）在 ai crate；DuckDB 随存档文件，属 engine 存储层。

### ② 依赖方向与单向缝

crate 间：`types ← engine ← ai`（ai 实现 engine 端口），api 依赖 engine + ai 做装配；engine 不反向依赖 ai/api。engine 内部分层：

- **叶子层**：`state`（扁平分面、id 互引）、`rng`（确定性序列，种子+序号，记录进命令日志）——谁都不许依赖，一切依赖向内指向它们。
- **编排层**：`command`（#04 四阶段管线）为唯一编排者，依赖 state/rng/lua_host/event/memory；`session` 持有回合循环（#03：玩家输入→主线 AI 意图→角色 AI 意图并行→引擎结算→流式输出），经 `AiProvider` 端口调 ai——确定性重放测试注入假 AiProvider（固定意图序列），engine 仍是权威驱动方。
- **被调层**：`resolve`（结算、判定分档）只被 command 调；`lua_host`（mlua 沙箱）只被 resolve/event 调；`event`（#17 事件构造，结算即推）只被 command 调。
- **适配层**：`storage`（SQLite/DuckDB/快照）、`upcast`（#14 纯函数版本链）只被 command/memory 与读路径调，不反向依赖。
- **端口层**：`ports` 定义 `Storage` / `EventSink` / `AiProvider` / `EmbeddingBackend` 四 trait；engine 只依赖端口。ai 实现 AiProvider（内部 rig CompletionModel）、EmbeddingBackend（fastembed 本地 / Provider）；api 实现 EventSink（转 SSE）并注入 Storage 实现。

engine 唯一直接依赖的外部实现库：mlua、rusqlite、duckdb（属引擎职责，非外部系统）。

### ③ 前端类型：ts-rs 生成（a）

types crate 共享类型 derive TS，构建时生成 `frontend/src/generated/types.ts`，communication 层引用；单一来源。判别联合/`#[serde(tag)]` 生成边角（flatten、adjacent tagging）实现时验证；CI 校验生成物不漂移。

### ④ 测试策略分层

- **命令管线确定性重放**（engine 层）：固定 RNG 种子 + 固定意图序列驱动 `session`（注入假 AiProvider），断言命令日志逐条 + 物化状态一致；同序列重放两次结果全等。形态 = engine crate 集成测试，纯内存 state + 临时 SQLite。
- **版本链 golden fixtures**（engine 层，#14 已锁）：`tests/fixtures/` 存各历史版本日志段/故事书 JSON/库文件，断言「一路升格到当前 = 期望结构」；每加一版补一 fixture。
- **Lua 钩子沙箱测试**（engine 层）：白名单拒绝（math.random/io/os 不可达）、请求-校验-执行、fail-fast 链、engine_rng 确定性。
- **记忆检索测试**（engine 层）：DuckDB 内存模式注入固定 embedding，断言 top-5 召回与关键词兜底。
- **SSE 端到端**（api 层）：tower `ServiceExt::oneshot` 进程内起 axum router（不绑端口），注入假 AiProvider，驱动完整回合，断言事件序列/seq 水位线/确认门 POST 往返/重连水合；少量真实 socket 测试（tokio TcpListener）覆盖 EventSource 行为。
- **ai 层**：mock `CompletionModel`，断言意图解析、上下文渲染（#05 预算分段）、摘要触发。

### 解锁

本票决议落地引擎/后端工程结构；未开新票，Not yet specified 无毕业项。

---

## 修订（设计复审 2026-09-09）

- 前端类型由 **ts-rs 生成**，`frontend/src/types/index.ts` 只做 re-export（决策记录第 11 条）。
- 存储层 = **单库 SQLite + 独立 DuckDB 文件**（#27）；`duckdb` 仍属 engine。

详见 [决策记录](../decision-log-design-pass.md)。

