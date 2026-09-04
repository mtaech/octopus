# 引擎→UI 流式事件协议

Type: grilling
Status: resolved
Blocked by: 03, 04, 08, 11

## Question

定义引擎结算结果如何以事件流推送给前端 UI（SSE 通道上的事件 schema）。游玩界面已定（#08），需要拍板：
① 事件类型集合（场景 / 旁白 / 对话 / 神态 / 判定结果 / 待确认 / 系统 / 任务与状态更新……）与各自 payload 形状。
⑤ 多模板约束（#08 修订）：事件流须与演出模板无关——同一事件流驱动聊天流/剧本式/沉浸式三种呈现，模板选择为纯前端关注点，不得混入事件 schema。
② 流式语义：增量 token 逐字推送 vs 完整事件块；打字机效果由前端渲染（推荐）还是后端分块。
③ 确认门往返：UI 展示待确认事件后，玩家确认 / 取消 / 超时如何回传引擎；回合边界与「免确认」模式如何表达。
④ 状态同步与订阅：演员卡 / 任务日志 / 地图如何增量更新——是否需专门的状态推送，还是仅靠事件流重放物化。

## Answer

决议（两轮 grilling 全部落定）：

### ① 流组织与包络

- **连接形态**：长连接 SSE——一次游玩会话一条（EventSource 自动重连）；回合边界用 `round_start`/`round_end` 事件标记，不在连接层面切分。
- **包络**（所有事件共享）：`{ id: uuid, seq: 单调递增, round: 回合号, type, ts, actor?: {id,name}, intent_id?: uuid }`。`actor` 为叙事事件的语义归属（渲染器据此署名；narrate 的 actor = 保留叙述者 id）；`intent_id` 把机制事件关联回动作协议的意图（UI 折叠/追溯）。
- **命名**：**演出流**（presentation stream）——引擎→UI 的派生投影流，区别于权威的命令日志（术语表已同步）。

### ② 事件类型表与 payload

| 类 | 事件 | payload |
|---|---|---|
| 叙事 | `scene` | `{ scene_id, title, description?, present: [char_id] }` |
| | `narrate` | `{ content, scene_ref? }` |
| | `dialogue` | `{ content, audience? }` |
| | `emote` | `{ content, emotion?, gesture? }`（content=动作描述，标签供模板加样式） |
| 机制 | `pending` | `{ action_id, intent_id, actor, description, impact?, timeout_ms }` |
| | `check_result` | `{ intent_id, actor, attribute, expr?, rolls?, mod, total, target, margin, result, level }`——`expr`/`rolls` 为判定器骰子表达式与点数（无骰判定缺省），`margin` = total − target；`level ∈ {great, success, barely, fail}`（按 #12 差值阈值分档，自定义 Lua 判定同样归一化、level 恒由引擎分档；#08 原型的 ok/part/crit/fail 为样式映射，不入 schema） |
| | `resolution` | `{ intent_id, status: ok\|rejected, rejection_code?, narrative?, state_changes }`——被驳回意图的反馈同样走 resolution（演出流为派生视图，玩家需要看到驳回原因；#06「驳回只进 debug 流」指命令日志） |
| 状态 | `state_update` | `{ changes: [delta...] }`，只承载非结算来源变更（目标达成提示/节拍触发/场景切换） |
| | snapshot | 全量水合，独立 `GET /state`（REST 拉取，非流事件）；带 `seq` 水位线，前端丢弃 `seq ≤ watermark` 的流内事件 |
| 控制 | `phase` | `{ stage: story_thinking\|character_thinking\|resolving\|waiting_confirm, detail? }` |
| | `round_start` | `{ round, input: { channel: character\|meta, text } }`——回显玩家输入，流自包含（重连/回放可重建） |
| | `round_end` | `{ round }` |
| | `system` | `{ level: info\|warn\|error, code?, text }`（error 为 subtype，不单列） |

共享 `state_changes` delta：`{ domain: character\|goal\|beat\|location\|relationship\|resource\|flag, entity_id, field, op: set\|add\|remove, value }`——`resolution` 与 `state_update` 共用，前端据此更新演员卡/目标日志/地图的本地投影。

### ⑤ 多模板约束

事件 schema **零呈现字段**；叙事事件必带 `actor` 归属 + 语义 `type`（台词/旁白/神态）；模板选择（聊天流/剧本式/沉浸式）= 纯前端本地状态，切换不产生任何事件。

### ② 流式语义

- 完整事件块 + 前端就地打字机（可跳过）；**不做 token 级 delta**（留作扩展点，不进 v1 schema）。
- 推送时机：回合管线中**结算即推**（渐进呈现，角色 AI 并行思考时先到的事件先演）；AI 思考的等待由 `phase` 事件覆盖。

### ③ 确认门往返

- 回传通道 = **HTTP POST**（不引入 WebSocket）：`POST /rounds/{round_id}/confirmation`，body `{ action_id, decision: "confirm"|"cancel" }`；引擎恢复管线后 `resolution` 事件继续在 SSE 上流出。
- **超时默认取消并提示**（`timeout_ms` 由 `pending` 携带；超时确认策略可配置）。
- **免确认 = 引擎侧开关**：开启时引擎不发 `pending`、直接结算；事件流保持唯一事实源。
- 回合边界：确认期间**回合保持 open**，`round_end` 延后至确认后发出。
- 过期确认请求返回 `409 { code: "expired" }`。

### ④ 状态同步与订阅

- 主渠道 = 机制事件自带 `state_changes`，前端据此增量更新本地投影；`state_update` 只做非结算来源的补充。
- 重连/进入游玩页 = `GET /state` 一次性水合（对齐 #06 快照=启动缓存），**不回放历史事件**。
- 前端 store 的物化组织留给「前端应用结构与模块划分」。

### 解锁

本票决议解锁「前端应用结构与模块划分」「演出模板的自定义与扩展机制」「Rust 模块划分与测试策略」三张被阻塞的票。

