# 会话持久化与聊天完善规范

> 状态：已定稿（2026-09-11）
> 范围：游玩页叙事演出流（FeedChat）+ 编辑器 C 范式 AI 结对聊天（PairC）
> 关联：[`CONTEXT.md`](../CONTEXT.md) 中的「命令日志」「叙事条目」「快照」「新原点」

## 1. 背景

当前后端**没有任何会话持久化**：

- 游玩页的演出事件只存在 `Session.event_log: Mutex<Vec<EventEnvelope>>` 里（`crates/octopus-engine/src/session.rs:54,79,98,122`），进程重启即清零。
- `commands` 表虽然存在，但 `Session` 从不写它——`append_command` 只有测试与存档包导入在用，实库中 `commands` 与 `archived_commands` 均为 0 行。
- 世界状态也不持久化：`build_state(save)` 只从内嵌故事书重建（`crates/octopus-api/src/lib.rs:132`），重启后 flags、角色位置、属性全部回退到初始值。
- 「手动存档 / 新原点」是假实现：`manual_save` 只写一行「快照已写入（保留 5 份）」的维护文案（`crates/octopus-api/src/lib.rs:667-680`），`new_origin` 固定返回 `archived_count: 0`（`crates/octopus-api/src/lib.rs:682-688`）。
- 编辑器结对聊天完全无状态：后端不落库（`crates/octopus-api/src/pair.rs`），前端 Pinia 也不持久化（`frontend/src/pages/editor/stores/pair.ts`），刷新即丢。

因此「找回会话」在真实后端与编辑器两侧都无法成立。

## 2. 顶层原则

1. **权威下沉到库，内存只是缓存，前端只是投影。** 单一事实来源是库里的 append-only 日志。
2. **重放不重算。** 重启后从日志纯投影重建，绝不重新调用 AI；否则同一次输入会得到不同结果。
3. **已展示即已持久。** 广播严格排在落库之后；UI 上出现过的事件不可能因进程崩溃而消失。
4. **重放只渲染、不改状态。** 世界投影由后端权威给出；前端重放历史事件只重建 feed，不做 `applyDeltas`。
5. **稳定 id。** feed 条目的 key 使用日志中的 `EventEnvelope.id`（或 `seq`），不使用前端进程内自增序号。

## 3. A 面：游玩页命令日志落库

### 3.1 Schema

`commands` 表形状保持不变：`(id, save_id, seq, round, kind, payload_json, ts)`，索引 `idx_commands_save_seq` 复用。

约定：

| 列 | 含义 |
|---|---|
| `kind` | 事件类型：`scene` / `narrate` / `dialogue` / `emote` / `pending` / `check_result` / `resolution` / `state_update` / `phase` / `round_start` / `round_end` / `system` |
| `payload_json` | 整条 `EventEnvelope`（含 `id/seq/round/ts/actor/intent_id/event`），保证重放无损 |
| `seq` / `round` / `ts` | 冗余列，供索引与分页 |

新增迁移 `m20260910_000003_add_request_id_to_commands`，为 `commands` 增加 `request_id TEXT NULL`。`round_start` 记录携带请求 id，用于重启后的幂等去重（替代当前只存在于内存的 `Session.request_ids`）。

### 3.2 写入路径

- `Session` 通过 `EventSink` 端口获得持久化能力，`emit` 保持同步签名。
- 注入的 sink 实现改为「**先落库、后广播**」：内部用单消费者写任务串行消费事件，成功写入后才向广播通道发送。
- 落库失败：记录错误并置会话为降级状态；事件仍会广播，以避免 UI 静默丢内容，但降级状态可被健康检查观测。
- 实现上不新增第二条 SQLite 连接、不阻塞运行时线程；顺序不变式（持久化先于广播）由单消费者队列保证。

### 3.3 重放

把「修改世界状态」收敛为唯一入口：

```text
实时：intent → apply_event(env) ; log.append(env) ; sink.emit(env)
重放：for env in log: apply_event(env)
```

- 新增纯函数 `apply_event(&mut WorldState, &EventEnvelope)`。
- **所有**状态变更必须表达为 `StateDelta` 并出现在事件里。已补齐：`Move` 改为发 `StateUpdate`（character.location_id）而非直接改状态；`switch_character` 发 `controlled` delta；`set_auto_confirm` 由 `System(confirm_toggle)` 事件重放恢复；`run_check` 的 flag 写入统一走 `StateUpdate`。
- 重放结束：`seq = max(seq)`、`round = max(round)`、`request_ids` 从 `round_start.request_id` 收集。
- `history()` 读取对象仍是会话内事件日志（重放后已填充），`before_seq` 分页语义不变。

### 3.4 快照

v1 策略：**全量重放，不落快照**。预留 `snapshots` 表与接口位置，但不在本次实现。

`manual_save` 已如实描述为「标记检查点」；`new_origin` 返回 `501 not_implemented`，前端按钮同步标注「暂不可用」。

### 3.5 存量数据

加迁移、不回填、不动存量。旧存档没有历史可回放，打开即为「新档」，世界状态从故事书初始值开始。

### 3.6 引用：游玩页的精准指向

与结对共用同一套 `EntityRef` / `FocusEntity`（定义见 §4.5），但**数据源与链路不同**：

- 数据源是**存档内冻结的故事书**（`SaveDetail.storybook`），不是编辑器草稿。
- 输入框旁「引用」按钮 / `@` 唤起同一个（已抽到 `@/components/EntityPicker.vue`）实体选择器；chips 显示在输入框上方与回合头。
- 提交时：轻量 `refs` 随 `RoundInput.refs` 落进 `round_start` 命令日志（展示 / 重放 / 刷新一致）；完整定义 `focus` 仅本轮透传，**不落库**。
- AI 侧：`TurnContext.focus` → `rig_provider::turn_prompt` 注入「本次玩家明确引用了以下实体」段落。**软聚焦**，不改引擎的意图校验与动作协议。
- 引擎不持有整份故事书 JSON，因此完整定义由前端解析后随请求带上。

## 4. B 面：结对会话线程

**一本故事书可有多条按主题隔离的会话线程**；每条线程独立上下文，用于分开讨论「任务」「物品」等不同主题。线程可新建、切换、重命名、删除。

### 4.1 Schema

新表 `pair_threads`：

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | string PK | `pt-` 前缀 |
| `storybook_id` | string | 归属故事书 |
| `title` | string | 默认「新会话」，由首条用户消息自动派生，可手动改名 |
| `created_at` / `updated_at` | string | ISO 8601；`updated_at` 用于列表倒序 |

索引 `(storybook_id, updated_at)`。

`pair_messages` 增加 `thread_id`，消息挂在**线程**上：

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | bigint PK | 自增 |
| `thread_id` | string | 归属线程 |
| `storybook_id` | string | 冗余，便于随书清理 |
| `seq` | bigint | 线程内序号 |
| `role` | string | `user` / `assistant` |
| `content` | text | 正文 |
| `model` | string NULL | 生成该条所用的模型 |
| `is_error` | bool | 是否为失败提示 |
| `tools_json` | text NULL | 本轮工具轨迹（展示用） |
| `created_at` | string | ISO 8601 |

索引 `(thread_id, seq)`。

迁移 `m20260910_000005` 会把迁移前的「一本故事书一条隐含线程」回填为一条「对话 1」，旧消息挂上去。

### 4.2 API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/storybooks/{id}/pair/threads` | 列出该书会话（含 `message_count`，按更新倒序） |
| POST | `/api/storybooks/{id}/pair/threads` | 新建会话（可选 `title`） |
| PATCH | `/api/pair/threads/{thread_id}` | 重命名 |
| DELETE | `/api/pair/threads/{thread_id}` | 删除（级联删消息） |
| GET | `/api/pair/threads/{thread_id}/messages` | 读取会话历史 |
| POST | `/api/pair/threads/{thread_id}/messages` | 追加消息（自动补标题、刷新排序） |
| DELETE | `/api/pair/threads/{thread_id}/messages` | 清空本会话消息 |

自动标题：仅当标题仍是默认值（「新会话」或「对话 N」）时，用首条用户消息（单行、截断 20 字符）派生；手动命名不会被覆盖。

### 4.3 交互与会话语义

- 入口为 C 范式**左侧会话栏**：列表显示「标题 · 消息数」，悬停出现重命名 / 删除，顶部「+」新建。
- **可切换，不并发**：同一时刻只有一条会话在跑；不再有「新开会话会销毁旧会话」的破坏性行为。
- **不做轮次裁剪**：发送的是全量历史，按普通上下文处理；用量以**真实 token** 呈现——会话栏底部报「上下文估算 X tok」（对话 / 故事书 / 工具三块），并显示上一轮的出字 tok、tok/s、缓存命中率与思考字数。
- 「待审查建议」按会话隔离并**落库**（`pair_threads.pending_suggestions_json`，150ms 防抖写入）：切走切回、刷新、重进都不丢，根治「右侧审批卡凭空消失」。

### 4.4 批准门：结对从不直接改草稿

**硬规则**：AI 产出的一切改动都是**提案**，必须经创作者批准才写入草稿。理由：草稿有 deep watch 自动保存，未批准的中间态一旦直写就已经存盘，创作者根本来不及看。

流程：

1. 一轮开始时克隆一份**暂存草稿**（`editor.beginToolTurn`）。
2. 模型的多步工具循环**只改暂存草稿**，工具结果（含新建实体 id）照常回灌，保证链式引用正确；真草稿全程不动。
3. 轮末交出本轮拟改动（`finishToolTurn`），作为「待审查改动」卡片进入右栏，默认全选。
4. 创作者可**逐张应用**或**勾选后整批应用**；合并时整批只压一次撤销快照（`applySuggestions`）。
5. **放弃**则丢弃暂存，真草稿零污染；聊天里那条 assistant 消息保留并标注「本轮改动已放弃」。

统一：非工具模式的建议 JSON 与工具改动走同一套「待审查改动 + 批准」流程，不再有两套落稿语义。系统提示（`crates/octopus-api/src/pair.rs`）已同步改为「工具调用不会直接写入草稿，而是作为待批准改动」。

### 4.5 引用：精准指向要改的实体

问题：模型虽然能拿到实体 id 目录，但**创作者说不清「是哪个」**——打字说「雨果」时重名或指代不清就只能靠猜。

方案：给对话加一层「目标」。

- **入口**：输入框旁「引用」按钮（主入口，可见可发现）+ 输入 `@`（快捷方式）打开同一个实体选择器。
- **选择器**：从草稿枚举全部可引用实体，按类型分组、可搜索。覆盖 14 类顶层实体 + 嵌套 scene/goal/beat + 声明区四类 + meta/world 单例。
- **数据**：`EntityRef { kind, id?, parent_id?, name }`——单例无 id，声明区 id=key，嵌套带 parent_id。选中的引用以 chips 显示在输入框上方，发送后固化为消息的一部分并**持久化**（`pair_messages.refs_json`）。
- **给模型**：发送时前端把引用解析成 `focus`（**完整实体定义**），后端在系统提示里插入「本次改动的目标实体（最高优先级）」段落。模型不再猜，拿到的是现状全貌；并被告知「目标之外的改动要明确说明」。
- **约束强度**：软约束——引用是强提示，不硬锁；提案卡仍照常审查。

### 4.6 流式事件契约（SSE）

`POST /api/pair/chat/stream` 逐帧下发，前端必须处理**全部**帧类型：

| event | data | 说明 |
|---|---|---|
| `delta` | `{text}` | 正文增量 |
| `suggestion` | 建议对象 | 非工具模式的建议（由 JSON 代码块解析） |
| `tool_call` | `{id,name,arguments}` | 工具调用（流末汇总） |
| `usage` | `{finish_reason,usage,reasoning_chars,counts}` | 真实用量与结束原因 |
| `done` | `{full_text,suggestions,tool_calls,finish_reason,usage,reasoning_chars,counts}` | 收尾汇总 |
| `error` | `{message}` | 上游中断 |

`usage.usage` 字段名与前端 `PairUsage` 一致：`input_tokens` / `output_tokens` / `total_tokens` / `cached_input_tokens` / `cache_creation_input_tokens` / `tool_use_prompt_tokens` / `reasoning_tokens`。

**硬规则：工具调用分片必须累积。** rig 的 `ToolCallDelta` 是分片下发的（`Name` / `Delta` 两态），只处理完整 `ToolCall` 会丢调用；上游 `Final` 帧承载 `finish_reason` 与 `usage`，同样不能落进 `Ok(_) => {}` 空分支。历史教训：这两处一起丢，前端只看到空文本 + 零工具调用，于是谎报「模型没有返回任何内容」。

**空返回可诊断。** 后端在「既无文本又无工具调用」时打 `WARN`（含 `counts` / `usage` / `finish_reason`），前端把同一组信息附在兜底提示后面；`finish_reason=length` 时额外提示把要求拆小（输出预算可能被思考占满，可调 `roles.pair.max_tokens`）。

### 4.7 明确延后

跨故事书总列表、会话搜索、置顶、复制 / 分叉会话、超限自动摘要压缩、字段级 diff 预览（当前复用建议卡）；输入框内**真·内联 chip**（现为「输入框上方 chips + 消息顶部 chips」，因中文输入法下 contenteditable 风险高，不划算）；从 A 范式工作台拖拽实体进对话；引用「整个类别」。

## 5. C 面：聊天框完善

### 5.1 必须项（P0，与持久化配套）

| 编号 | 事项 | 现状 |
|---|---|---|
| P0-1 | 重放只渲染不改状态；历史不打字机 | `play.ts:147-178,212,302-304` |
| P0-2 | 历史分页 + 「加载更早」 | 现在固定 50 条，`hasMore` 被丢弃（`play.ts:289`） |
| P0-3 | SSE 断线重连 + 按 seq 补拉 | `onerror` 只 warn；服务端忽略 `Last-Event-ID` |
| P0-4 | 玩家消息乐观追加 | 现在要等 `round_start` SSE 才出现 |
| P0-5 | 修 InputBar busy 丢字 | `input.value=''` 先清空再 `send`，busy 时 `send` 直接 return |
| P0-6 | 稳定 key | 现在用 `'f' + keySeq`，重放后换号 |

### 5.2 完善项（P1）

| 编号 | 事项 |
|---|---|
| P1-1 | 消息操作：复制 / 重发（已实现）；重新生成 / 删除需引擎回滚与分支，v1 不做 |
| P1-2 | 时间戳 |
| P1-3 | AI「正在输入」指示 |
| P1-4 | 滚动：回看时暂停自动滚底 + 「回到底部」按钮 |
| P1-5 | Markdown 渲染（复用编辑器 `renderMarkdown`） |

### 5.3 暂不做（P2）

编辑已发送回合、引用 / 回复、消息搜索、连续气泡合并、虚拟滚动、移动端专版、草稿持久化、`/` 命令自动补全、附件 / @ / 表情、按会话重命名。

**明确延后**：消息「重新生成」与「删除」需要引擎侧的回滚 / 分支能力（CONTEXT.md 的「分支」尚未实现），不在 v1。

### 5.4 mock

mock 每次刷新整库重建、`eventLog` 必空（`frontend/src/api/mock/backend.ts:130-131,136-137`），不作为验收对象，只保证 API 形状与真实后端一致。

## 6. 验收锚点

> 杀掉后端进程 → 重启 → 打开存档：叙事历史与世界状态（flags / 角色位置 / 属性）与杀进程前一致，且全程不重跑 AI。
> 刷新编辑器：该故事书的**全部会话线程与各自对话**原样还在；新建会话不销毁旧会话。

## 7. 决策记录

| 编号 | 决策 | 结论 |
|---|---|---|
| Q6 | 重放粒度 | 存完整 `EventEnvelope` 事件流 |
| Q7 | 写入时机 | 先落库、再广播 |
| Q8 | 快照 | v1 全量重放，按钮文案诚实化 |
| Q9 | 端口形态 | 保持 `emit` 同步，注入「先落库后广播」的 sink |
| Q10 | 存量数据 | 加迁移、不回填、不动存量 |
| Q11 | C 面范围 | P0 全做 + P1 五条全做 |
| Q12–Q16 | 多开会话 | 编辑器结对多线程 · 可切换不并发 · 每书多条 · 左栏列表 · 上下文用量可见 · 标题取首条消息可改名 · 建议按会话隔离（后改为**落库**，见 Q21） |
| Q17 | 结对批准门 | 每轮批量批准 · 暂存草稿零污染 · 复用建议卡 · 逐张勾选 · 放弃后消息保留并标注 · 两条落稿路径统一 |
| Q18 | 引用（精准指向） | 按钮为主 + @ 为辅 · 结构化 EntityRef · 双向持久化 · 发完整定义 focus · 软约束不硬锁 · 多个具体实体 |
| Q19 | 游玩页引用 | 数据源=冻结故事书 · 复用同一选择器 · refs 落命令日志 · focus 仅本轮透传 · 软聚焦不改引擎协议 |
| Q20 | 取消轮次窗口 | 不再限制 12 条，按普通上下文发全量历史；用量直接报真实 token（对话 / 故事书 / 工具 + 上轮出字 / 速度 / 缓存命中） |
| Q21 | 待审查建议落库 | 存 `pair_threads.pending_suggestions_json`（150ms 防抖）；刷新 / 切会话 / 重进都不丢 |
| Q22 | 流式帧全处理 | 累积 `ToolCallDelta` 分片、捕获 `Final`（用量 / 结束原因）、新增 `usage` 事件；空返回打 WARN 并把诊断附在兜底提示里 |
