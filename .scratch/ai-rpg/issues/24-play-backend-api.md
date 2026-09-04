# 游玩页后端 API 设计

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 03, 04, 06, 17, 20

## Question

定义游玩页后端 API 的形态。机制层面已定（#03 核心游戏循环：玩家输入→主线 AI 意图→角色 AI 意图→引擎结算→流式输出、角色/元指令双通道；#04 动作协议：六类意图 + 四阶段管线 + intent_id 幂等；#06 状态持久化：命令日志 + 快照、SQLite 一存档一文件、回滚/分支；#17 引擎→UI 事件协议：SSE 包络 + 确认门 HTTP POST 往返 + GET /state 水合 + seq 水位线；#20 Rust 模块：octopus-api 装配 EventSink→SSE、注入 Storage/AiProvider，session 回合循环在 engine），需要拍板：

① 会话生命周期与端点划分：游玩会话的 REST 形态——显式 session 资源（创建/销毁）还是隐式（加载存档即会话、按存档寻址）？新建游戏（基于故事书已发布版次开档）与读取存档的端点；回合输入与回合内 SSE 流的组织（单 POST 触发 + 回合流 vs 其他）；确认门 POST 的路径与载荷（#17 已定机制与超时语义，路径/载荷未定）。

② 存档端点的边界：存档读写与 #06/#14/#21 的接口面——save 触发（自动/手动）、升级流程（#14 两段式 dry-run→确认→执行 + #21 抽屉）的后端端点；「新原点」「回滚/分支」的端点形态（#21 确认 v1 仅 dev 工具）。

③ 列表页数据源：故事书列表（#23 已定 GET /api/storybooks）与存档列表的端点与字段；游玩侧可见性（仅已发布故事书可开档）。

④ 并发与生命周期边角：v1 单用户多 tab 游玩同一存档的冲突策略（会话锁？只读拒绝？）；会话退出/超时与命令管线的一致性（回合中断、未完成回合的处置）。

## Answer

**决议（两轮 grilling 全部落定）：**

### ① 会话形态与端点划分：隐式按存档寻址

- **不建显式 session 资源**：引擎 session（内存、按 save_id 键控，#20）即会话，SSE 流即会话实例（#17「一次游玩会话一条 SSE」）；多人预留停在 Player 实体（#06），不上升为会话协议。
- **开档**：`POST /api/saves { storybook_id }` → 内嵌冻结该故事书**当前已发布版次**（#14），返回 save meta；未发布 / 已删除 → 404/409。
- **续玩**：`GET /api/saves/:id` 返回 meta（标题、storybook 引用、内嵌版次、最新版次、`needs_upgrade`、时间戳）。
- **水合**：`GET /api/saves/:id/state`（#17 `GET /state` 落到存档寻址下）。
- **回合输入**：`POST /api/saves/:id/rounds { channel: character|meta, text }` → 202（无流响应体，事件经常驻 SSE 流出，`round_start` 回显 #17）。单 POST 触发 + 常驻流，不做每回合短流（#17 长连接决定）。
- **演出流**：`GET /api/saves/:id/stream` = 长连接 SSE（包络 / seq / round 边界按 #17，EventSource 重连水合按 #18）。
- **确认门**：`POST /api/saves/:id/rounds/:round_id/confirmation { action_id, decision: confirm|cancel }`；超时 → `409 { code: "expired" }`；未知 → 404。错误信封统一 `{ code, message, detail? }`（#23）。

### ② 存档端点边界

- **手动存档**：`POST /api/saves/:id/save` = 强制快照（#06 手动时机，保留 5 份）；自动存档（每 ~100 条 + 场景切换 + 干净退出）为引擎内部行为，不暴露端点。
- **升级**（#14 两段式 + #21 抽屉）：
  - `POST /api/saves/:id/upgrade/dry-run` → 迁移报告（引用变更 / 人物消失分组，#14 引擎按需 diff 内嵌旧模板 vs 目标新模板）。
  - `POST /api/saves/:id/upgrade { dispositions: [{ character_id, disposition: freeze|departure }] }` → 自动备份 → 执行 → 维护历史落账 → 引发的世界变更以特殊命令进日志；未提供全部人物裁决 → 422（#21「未裁决完禁用确认」）。
- **新原点 / 回滚分支**：dev-gated 端点 `POST /api/saves/:id/origin`、`POST /api/saves/:id/branch { from_seq }`（dev 开关门控，不进 v1 抽屉 UI，#21）；便于 #20 api 层测试与 dev 工具驱动。
- **导出 / 导入**（#21 交互落地）：导出 `GET /api/saves/:id/export` → 单文件 .sqlite 下载；导入 `POST /api/saves/import`（multipart）→ 能打开即通过，返回带「新导入」标记的 save meta。

### ③ 列表页数据源

- **故事书列表**：复用 `GET /api/storybooks?released=1`（#23 端点本体不变，可见性规则落服务端）。
- **存档列表**：`GET /api/saves` → `[{ id, storybook_id, storybook_title, embedded_revision, latest_revision, needs_upgrade, created_at, updated_at, last_played_at }]`，按 `last_played_at` 降序；「继续游玩」取首条（#18/#21）；`needs_upgrade` 服务端算好（#21 横幅 + 角标）。

### ④ 并发与生命周期（最小模型，不建会话 API）

- **多 tab**：不设会话锁、不做接管（实际基本不存在）。多 tab = 同一演出流的多个订阅者，回合输入全走服务端权威命令管线串行结算，无状态差异；视为 v1 可接受。
- **在途回合**：引擎权威，客户端断开不中止结算；已提交命令持久化（#06），事件发往断开流即丢弃，重连后 `GET /state` + seq 水位线补齐。
- **挂起确认门**：客户端消失按 `timeout_ms` 默认取消（#17）。
- **不做 quit 端点**：持久化由 #06 快照节奏兜底；「干净退出」快照为引擎内部行为。
- **SSE 抖动存活**：EventSource 自动重连，引擎 session 跨抖动存活，空闲超时后拆除。

### 解锁 / 新增

- 解锁「列表页（故事书 / 存档入口）UI 规格」——数据源（③）已定，另立新票。
- 雾区清理：列表页 UI 规格从 Not yet specified 毕业。

