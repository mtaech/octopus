# 设计复审决策记录（2026-09-09）

> 对象：Octopus v1 技术设计蓝图（`map.md` + `issues/01–25` + 4 份调研 + `CONTEXT.md`）。
> 方式：全量通读后与作者逐条确认，12 项全部拍板。
> 落账：本文件 + 相关票内「修订（设计复审 2026-09-09）」块 + 新开 [#26](./issues/26-ai-provider-config.md)/[#27](./issues/27-storage-single-db.md)；`map.md`、`CONTEXT.md` 同步。

## 决策总表

| # | 议题 | 决议 |
|---|---|---|
| 1 | 叙事权威性与历史可见性 | 叙事文本进命令日志（权威、可重放）；新增历史读取端点，刷新/重连/换模板不丢故事 |
| 2 | 存储形态 | 应用级单库 SQLite（所有存档按 save_id 分区）+ 独立 DuckDB 向量库（派生可重建）；导出=抽单存档为独立 .sqlite 包 |
| 3 | 新原点 | 玩家侧 v1 功能（删掉 #24 的 dev 门控）；旧日志归档进库内归档表（按 save_id、只读） |
| 4 | 缺失端点 | 全部补进 #24：存档标题/重命名、删除、维护历史读取、免确认设置 |
| 5 | Provider 配置与凭据 | 新开 #26；密钥存本地配置文件（app 数据目录、0600）；主线/角色/Embedding 三类模型各自可配 |
| 6 | 回合并发 | 非 idle 提交返回 409 round_in_progress；客户端带 request_id 服务端幂等去重 |
| 7 | interact / 物件 | 保留 interact，并在 #01 补完整 objects 实体（可挂技能与条件） |
| 8 | 元指令通路 | 状态类直调结构化端点，叙事类（intervene/节奏）走 meta 文本；intervene 记入命令日志 |
| 9 | 受控角色初始化 | 人物模板加 kind: pc\|npc；开档弹窗加「选主角」，默认第一个 PC，游玩中可切换 |
| 10 | 语义补漏 | #01 补声明区（flags/events/relationship_types/target_types）；check 加 opponent_id/target_value；finish_turn=循环终止信号；#17 resolution 补 outcome/triggered_events |
| 11 | 命名与契约 | 以 #01 + ts-rs 生成类型为单一来源逐项对齐；前端 types 只 re-export；文档勘误全部修 |
| 12 | 非阻断清单 | 插件 API 降级 v1.1（v1 只做 CSS token 换肤）；叙事字段用 Markdown；预算默认 8K/4K 可配；只绑 127.0.0.1 + 导入上限；无障碍/移动端 out of scope |

---

## 逐条明细

### 1. 叙事权威性与历史可见性（原 #04 vs #06 vs #17/#18 冲突）

- **问题**：#04 说 speak/narrate/emote 进命令日志；#06 说叙事产物非权威；#17/#18 规定重连/进页只 GET /state、不回放历史事件、ring buffer 回合结束即弃。后果：刷新或断线重连后整段故事对话消失；切换演出模板也会丢历史。
- **决议**：叙事文本**权威**——进命令日志（可重放、可审计）；新增分页历史读取端点 `GET /api/saves/:id/history?from_seq&limit`（#24）。`GET /state` 仍只做状态水合，历史走 history 端点。
- **影响**：修订 #04（Narrative 进日志的定位）、#06（把「叙事产物非权威」收窄为「摘要/向量索引等派生数据非权威」）、#17（补 history 端点说明）、#18（进页/重连/换模板从 history 拉取，ring buffer 仍只管当前回合）、#24（新端点）、CONTEXT.md（命令日志/叙事）。

### 2. 存储形态改向（原「一存档一 .sqlite」）

- **决议**：
  - **应用级单库 SQLite**：故事书（released/draft）、存档 meta、命令日志、快照、维护历史、归档表，全部按 `save_id` 分区（故事书按 storybook_id）。
  - **独立 DuckDB 文件**（app 数据目录）：只放向量索引，派生、可重建，不参与权威。
  - 关键词检索 = SQLite **FTS5**（终结 #06 FTS5 vs #05 DuckDB BM25 之争）。
  - **导出** = 从共享库抽出该存档为独立 `.sqlite` 包（内嵌冻结模板 + 该存档日志/快照/维护历史/归档），导入分配新 `save_id`。
  - 备份 = 整库拷贝（应用数据目录）。
- **影响**：新开 #27；修订 #05（DuckDB 独立文件）、#06④（存储选型）、#14②（备份/归档）、#20（存储层）、#21④（导出/导入）、#24②（导出/导入）、CONTEXT.md（存档定义）。

### 3. 新原点门控与归档

- **问题**：#21 说新原点进抽屉（玩家可见）；#24 说 origin 与 branch 一起 dev-gated、不进 v1 UI。
- **决议**：新原点 = **玩家侧 v1 功能**（去掉 #24 的 dev 门控）；只有回滚/分支是 dev 工具。单库下旧日志归档到**库内归档表**（按 save_id、只读标记），不再「挪出为独立文件」。
- **影响**：修订 #14④、#21③、#24②。

### 4. 缺失端点（直接修订 #24）

- `POST /api/saves` 增加 `title`（默认故事书名）；新增 `PATCH /api/saves/:id`（重命名）。
- `DELETE /api/saves/:id`（库内删除；不影响已导出包）。
- `GET /api/saves/:id/maintenance`（维护历史读取，#21 抽屉常驻）。
- `PUT /api/saves/:id/settings`（`{ auto_confirm }`；免确认 = 存档级设置，全局默认值在配置文件）。

### 5. AI Provider 配置与凭据（新开 #26）

- 用户侧配置：provider / base_url / model / 参数；主线 AI、角色 AI、Embedding **三类各自可配**。
- 密钥存**本地配置文件**（app 数据目录、权限 0600），不落 keyring。
- 含错误与降级（超时/重试/限流/结构化输出失败）、成本护栏、无 key 时的引导态。

### 6. 回合并发与幂等

- 非 idle（thinking/resolving/waiting_confirm）提交 → `409 { code: "round_in_progress" }`；UI 在该阶段禁用输入或提示等待。
- 提交带 `request_id`，服务端幂等去重（与 #04 的 intent_id 幂等是两层）。

### 7. interact 与物件模型

- **保留** `interact { object_id, action }`；#01 补完整 `objects` 实体（id/name/description/actions[]，可挂技能与条件）。
- 编辑器新增物件编辑；引用图/校验纳入。

### 8. 元指令执行通路

- **状态类**（切换角色、存档、免确认、新原点）：UI 按钮与 `/前缀` 都映射到**结构化端点**，不经 LLM。#24 需补 `POST /api/saves/:id/character`（或等价切换端点）。
- **叙事类**（intervene、节奏调节）：走 `POST /rounds { channel: "meta" }` 交主线 AI。
- `intervene` = 注入玩家高优先级指令给主线 AI，本轮生效并**记入命令日志**（可审计）。

### 9. 受控角色初始化

- 人物模板新增 `kind: "pc" | "npc"`（#01）。
- 开档弹窗增加「选主角」步（#25）；`POST /api/saves` 增加 `controlled_character_id`（#24），默认第一个 PC。
- 游玩中可随时切换（结构化端点，见 8）。

### 10. 结构化语义补漏

- #01 新增声明区：`flags` / `events` / `relationship_types` / `target_types`；引用处校验存在性（Lua 兜底可绕过）。编辑器给下拉 + 校验。
- #04 `check` 增加 `opponent_id` / `target_value`（`mode = opposed` 时必填）。
- `finish_turn` = 工具调用循环终止信号，**不算意图**、不校验、不进日志。
- #17 `resolution` 补 `outcome` / `triggered_events`，与 #04 `ResolutionResult` 对齐。
- #04 五类 → 六类（补 #13 的 Progression/`advance_scene`）。

### 11. 命名与契约单一来源

- 以 **#01 + ts-rs 生成类型**为单一来源，逐项对齐：`world.rules.check → world.check`；`scene_change → scene`；`attribute_dimensions` 进 JSON 树；PC/NPC；opposed 字段等。
- 前端尽快接 ts-rs，`frontend/src/types/index.ts` 只做 re-export。
- 文档勘误全部修（见下）。

### 12. 非阻断清单

- **插件 API**（#19）降级 **v1.1**：v1 只做 CSS token 换肤 + 内置三模板；`h()` 渲染树/生命周期/DOM 逃生舱移入 `map.md` 的 Not yet specified。
- **叙事字段格式 = Markdown**（#01/#22）；结构化数据仍走字段。
- **token 预算**：8K/4K 为默认值，按 provider/模型可配置（#05）。
- **安全边界**：只绑 `127.0.0.1`、无鉴权、导入加大小/结构上限，写进 #24/#21。
- **无障碍 / 移动端**：明确 out of scope，写进 `map.md`。

---

## 派生决定（随上述决议顺带确定）

- 升级前自动备份 = **导出该存档为独立包**（单库下不再「拷贝存档文件」）。
- 免确认 = **存档级设置**（`GET /state` 的 `meta.auto_confirm`），全局默认值在配置文件。
- 关键词检索统一为 **SQLite FTS5**。
- 叙事文本权威化后，命令日志条目增加叙事产物字段（或单独 narrative 表），读取侧由 history 端点分页返回。

## 勘误清单（本次一并修）

| 位置 | 修正 |
|---|---|
| #04、map.md #04 行 | 「五类意图」→ 六类（补 Progression） |
| #07 | 「发布 = meta.version +1」→ 版次 revision（#14/#23） |
| #17 | 事件表移除 `snapshot` 行（它是 REST `GET /state`，非流事件） |
| #19 | Question 的「从存档/项目目录加载」与 Answer 的全局 `templates/` 对齐 |
| #06 vs #05 | FTS5 vs BM25 → 统一 SQLite FTS5 |
| #24 | `GET /api/saves/:id` 是否返回内嵌故事书全文，统一口径 |

## 待续（保留在 map.md）

- 插件 API（v1.1）
- 回滚/分支玩家侧时间轴 UI
- 演出模板导出/导入（zip）
- 遗忘兜底（置顶事实清单）
- 无障碍 / 移动端（out of scope）
