# Octopus 设计蓝图实现差距分析（gap analysis）

> 对象：`docs/blueprint/` 设计书（`map.md` + issues/01–27 + research/* + `decision-log-design-pass.md`）。
> 方法：逐票抽取可核对要求（数据模型字段 / 端点 / 引擎行为 / 不变量 / UI 面 / 迁移），用 grep + read 对照 `crates/`、`migration/`、`frontend/src/`、`docs/`、`CONTEXT.md`。
> 锚点：git `4b5b3c4`（2026-09-13）。凡「名称存在但未被读取 / 路由存在但无处理 / 字段存在但从不写入」均已单独核对。
> 图例：✅ 完整 · 🟡 部分 · ❌ 缺失。
> 结果：**✅ 7 张（02/07/09/11/17/22/25）· 🟡 18 张 · ❌ 2 张（14/19）**。
> 在途项按任务要求标注：M1（fastembed 本地 embedding）已落地（✅）；`docs/memory-pipeline.md` 与 `docs/save-role-models.md` 是「已写规格、未实现」（分别对应 #05/#27 与 #26）。

---

> **通宵执行后的更新（2026-09-13 上午）**：本文件是锚点 `4b5b3c4` 时的快照；以下缺口已关闭，结论请以 `docs/overnight-report.md` 为准（那边每项都有验证命令与结果）：
> - **#14 升级链路 + 新原点**：后端端点已实现（dry-run / execute / 新原点），`needs_upgrade` 改为读取时计算。仍缺：`format_version`、golden fixtures、冻结角色的**规则**未与新故事书合并、受控 PC 离场不重指、dry-run 不深 diff、备份无列举 API 等（详见报告 §#14）。
> - **#05 / #27 记忆管线**：fastembed 本地嵌入（`bge-small-zh-v1.5` 512d）、DuckDB 派生向量库、SQLite FTS5（**含中文逐字分词修复**）、写作侧 best-effort 索引、混合检索（**向量 ∪ FTS top-5 → 【相关往事】只给主线 AI**）、回合微摘要 + 场景压缩、WAL 全部落地。仍缺：导出包契约/导入上限收口。
> - **#06 确定性**：RNG 位置随日志持久化并在重放时复位（差距描述里「从未写日志」的说法**部分有误**，真缺陷是重放不恢复位置 + 多条掷骰路径未记录）；快照表（保留 5 份、派生启动缓存、四重门禁回退）已实现。**Player 实体仍未做**。
> - **#24 回合并发**：409 `round_in_progress` 现在**同步**返回给客户端（原先在 spawn 内被吞）。
> - **#26 模型配置**：新增**存档级**模型分工——建档时可选「全游戏共用 / 主线与角色分别配置」（`saves.roles_json` + 每角色 provider/model/思考强度）；`roles.pair`（AI 结对）仍保持全局。
> - **#04 / #12 部分关闭**：`check` 走故事书声明判定器（缺省仍 1d20）、`intent_id` 幂等去重、`interact` + 真 `query_world`、actor/scope 约束。**仍未做**：3 轮 continue 循环、角色 AI 按角色并行、按 AI 的 scope 过滤、`opponent_id/target_value`、Lua `QueryWorld` 回灌、`interact` 执行物件挂载技能。
> - **#01 声明区**：`events`/`relationship_types`/`target_types` 三类悬空引用校验 + `objects` 引用校验已补；关系边端点统一为 `from`/`to`、场景事件统一为 `scene`（旧名走别名读取，向后兼容）。仍缺：声明区条目缺 key 校验、`set_flag` 的 flags 引用校验、`objects.actions/condition` 运行期消费（属 #04）。
> - **#19 演出模板**：仍未做（三模板 + 切换 + CSS token 换肤）。

---

## 1. 总表

| 票 | 状态 | 一句话结论 | 关键证据（实现 / 缺失） |
|---|---|---|---|
| [#01 故事书数据模型](blueprint/issues/01-storybook-data-model.md) | 🟡 | 顶层结构、kind、objects、attribute_dimensions 均在；声明区只校验 flags，events/relationship_types/target_types 无校验，关系字段命名分裂 | 实现 `crates/octopus-engine/src/seed.rs:17-53`、`validate.rs:234`(kind)、`validate.rs:1295`(objects)、`validate.rs:452`(flags)、`session.rs:171-177`(attribute_dimensions)；缺失 `grep relationship_types validate.rs` 无命中、`conditions.rs:72-77` 用 from_id |
| [#02 引擎/Lua 边界](blueprint/issues/02-engine-lua-boundary.md) | ✅ | mlua 沙箱 + scoped env 白名单 + engine_rng + 8 个挂载点 + fail-fast 链齐全 | `lua_host.rs:6-7,44-88,174-180`、`rng.rs`、`command.rs:229-336` |
| [#03 核心游戏循环](blueprint/issues/03-core-loop.md) | 🟡 | 回合边界、双通道路由、确认门在；角色 AI 未按角色并行、叙事类元指令（intervene/节奏）无通路 | 实现 `session.rs:909-1090`、`types 288-293`；缺失 `session.rs:939-943`(meta 直接 return)、`session.rs:1066`(单次角色调用) |
| [#04 动作协议](blueprint/issues/04-action-protocol.md) | 🟡 | 校验/结算/驳回码/FinishTurn 在；interact、opponent_id/target_value、query_character/query_relationships、intent_id 去重、3 轮 continue、并行角色 AI、actor/scope 约束均缺 | 实现 `types 544-616`、`protocol.rs:124-171`、`session.rs:1723-1943`；缺失 `grep Interact` 无命中、`types 570-576` 无 opponent、`session.rs:1935` query 为 no-op |
| [#05 上下文与记忆](blueprint/issues/05-context-memory.md) | 🟡 | 静态注入（前提/人格/词条/叙述段）在；双层摘要、DuckDB 检索、FTS5 兜底、top5 注入、50/50 预算、角色 AI 隔离全缺 | 实现 `session.rs:962-1008`、`ports.rs:77-115`；缺失 `grep summary/duckdb/fts` 无命中；规格 `docs/memory-pipeline.md` |
| [#06 状态与持久化](blueprint/issues/06-state-persistence.md) | 🟡 | 单库、事件溯源、叙事入日志在；全量快照、RNG 消耗持久化、Player 实体、自动快照时机缺 | 实现 `storage.rs:140-197`、`session.rs:680-703`；缺失 `manual_save` 明说不落快照 `api/lib.rs:1343-1355`、`command.rs:322-333` rng 被丢弃、`grep Player` 无命中 |
| [#07 编辑器 UX](blueprint/issues/07-editor-ux.md) | ✅ | A/C/B 三范式、骨架大纲、引用双轨+校验 dock、维度设置、显式发布均在 | `EditorPage.vue:10-12,193-296`、`editor.ts:203-215,324-361`、`WorkspaceA.vue`、`DeclarationsPanel.vue`、`ObjectsPanel.vue` |
| [#08 游玩界面](blueprint/issues/08-play-ui.md) | 🟡 | 聊天流模板、演员卡、抽屉、确认卡、打字机、阶段徽标在；三套演出模板与切换未做 | 实现 `PlayPage.vue:308`、`FeedChat.vue`、`PendingCard.vue`、`play.ts:188-200`；缺失 `PlayPage.vue:4` 自述「单一演出模板」 |
| [#09 Lua 集成调研](blueprint/issues/09-lua-integration-research.md) | ✅ | mlua 0.12 + lua54 + vendored，与结论一致 | `crates/octopus-engine/Cargo.toml`、`lua_host.rs` |
| [#10 LLM Provider 调研](blueprint/issues/10-llm-provider-research.md) | 🟡 | 走了 rig，但只用 openai 兼容客户端，anthropic kind 未接线，且未用工具调用（tools 为空） | `rig_provider.rs:15,478-497`、`api/ai.rs:15,218`；`config.rs:38` 声称的 anthropic 无实现 |
| [#11 流式传输调研](blueprint/issues/11-streaming-transport-research.md) | ✅ | axum SSE 长连接 + EventSource + 退避重连，符合结论 | `api/lib.rs:1426-1447`、`api/index.ts:914-963` |
| [#12 规则系统](blueprint/issues/12-rule-system.md) | 🟡 | 判定器/骰式/模式/阈值/Lua 归一/skill.check/资源/效果四层在；check 意图硬编码 d20、opposed 无入口、status add/max 不生效、per_turn/per_scene 恢复不触发、rng_consume 不落日志 | 实现 `resolve.rs:245-333`、`command.rs:229-336`、`effects.rs`、`modifiers.rs:89-94`；缺失 `session.rs:2431-2445`、`recovery.rs:35-39`、`session.rs:2640-2645` |
| [#13 骨架语义](blueprint/issues/13-skeleton-semantics.md) | 🟡 | 声明式条件树+回合末求值+提示+不自动切在；advance_scene 缺省下一场景、abandon/进入前置校验、repeatable 重触发缺 | 实现 `conditions.rs:40-136`、`session.rs:1139-1175`；缺失 `session.rs:1916-1934`(仅显式 target)、`conditions.rs:112`(已触发即跳过) |
| [#14 存档迁移](blueprint/issues/14-save-migration.md) | ❌ | 只有故事书读侧升格；升级 dry-run/执行、format_version、golden fixtures、升级前备份、新原点全缺（新原点端点直接 501） | 实现 `upcast.rs:17-56`；缺失 `api/lib.rs:1365-1376`、`api/index.ts:334-344`(走 mock)、无 tests/fixtures |
| [#15 Embedding 选型](blueprint/issues/15-embedding-model.md) | 🟡 | M1 fastembed bge-small-zh-v1.5(512) 已落地；DuckDB FLOAT[512]/余弦/VSS 未落地 | 实现 `crates/octopus-ai/src/embedding.rs`、`octopus-ai/Cargo.toml`；缺失 `grep duckdb` 无命中、无 octopus-index crate |
| [#16 一致性/防幻觉](blueprint/issues/16-consistency-anti-hallucination.md) | 🟡 | 原则「不做机检」遵守、机制事实靠校验管线兜底；但 intervene 通路死亡、角色 AI 认知边界未实现，prompt 一致性条款只有优先级一句 | 实现 `session.rs:1942`(FinishTurn)、`protocol.rs` preamble；缺失 `session.rs:1279-1302`(meta)、`rig_provider.rs:426`(仅优先级) |
| [#17 引擎→UI 事件协议](blueprint/issues/17-engine-ui-event-protocol.md) | ✅ | 包络/12 类事件/state_changes/结算即推/确认门往返/409 expired/history 均在 | `types 343-530`、`api/lib.rs:1426-1447,925-934,1020-1028` |
| [#18 前端模块结构](blueprint/issues/18-frontend-modules.md) | 🟡 | 三路由、Pinia、seq 水位线、delta 投影、重连水合在；无独立 communication 层、无模板切换、无空闲检测 | 实现 `router.ts`、`play.ts:203-207,358-361,472-552`、`api/index.ts:914-963`；缺失 `grep --tpl` 无命中、无 30s 空闲探活 |
| [#19 模板自定义](blueprint/issues/19-template-customization.md) | ❌ | v1 范围（CSS token 换肤 + 内置三模板）整体未做，且无插件 API（按决议本应降级 v1.1） | `grep --tpl- frontend/src` 无命中；`PlayPage.vue:4` 单一模板 |
| [#20 Rust 模块与测试](blueprint/issues/20-rust-modules-testing.md) | 🟡 | 四 crate+thin bin、端口、ts-rs 生成在；golden fixtures、engine 集成测试目录、SSE oneshot 全套、CI 漂移校验缺 | 实现 `Cargo.toml`、`ports.rs`、`crates/octopus-types/bindings/`；缺失无 `crates/octopus-engine/tests`、无 fixtures、`frontend/src/types/index.ts` 手写而非纯 re-export |
| [#21 存档管理 UI](blueprint/issues/21-save-management-ui.md) | 🟡 | 抽屉/升级向导/维护历史/导出导入/重命名删除 UI 在；升级与导入格式对接 mock 或非设计格式，新原点按钮触发后端 501 | 实现 `drawer/`、`WizardView.vue`、`SaveMain.vue`；缺失 `api/index.ts:334-344`、`api/lib.rs:1365-1376`、导出为 zip `assets.rs:212` |
| [#22 编辑器前端结构](blueprint/issues/22-editor-frontend-structure.md) | ✅ | 单路由+视图态、document 复用、骨架 tab、引用 dock、editor/pair store、debounced 自动保存均在 | `EditorPage.vue`、`editor.ts`、`pair.ts`、`document/`、`refs/ValidationDock.vue` |
| [#23 编辑器后端 API](blueprint/issues/23-editor-backend-api.md) | 🟡 | 端点面/两态/乐观并发/发布门/validate 在；声明区只校验 flags，结对端点形态与「服务端不持久化会话」不符 | 实现 `api/lib.rs:399-429`、`storage.rs:329-444`；缺失 `validate.rs` 无 events/relationship_types/target_types、`api/lib.rs:397-428`(线程持久化) |
| [#24 游玩页后端 API](blueprint/issues/24-play-backend-api.md) | 🟡 | 开档/续玩/水合/回合/SSE/确认门/存档/历史/设置/角色/导出导入在；升级 dry-run+execute、branch、409 round_in_progress（客户端拿不到）、needs_upgrade 计算、.sqlite 导出缺 | 实现 `api/lib.rs:386-459`；缺失 `submit_round` 把 busy 判在 spawn 内 `api/lib.rs:936-957`、`create_save` 写死 needs_upgrade=false `api/lib.rs:804` |
| [#25 列表页 UI](blueprint/issues/25-list-page-ui.md) | ✅ | A 分区并置、三类卡、空态分流、继续/新建/导入、开档选主角均在 | `ListPage.vue`、`StorybookCard.vue`、`SaveCard.vue`、`NewGameDialog.vue:175-190` |
| [#26 Provider 配置](blueprint/issues/26-ai-provider-config.md) | 🟡 | 配置文件 0600、roles 四类、设置 UI、连通性测试在；重试/退避/超时、解析失败重试、无 key 引导态、每存档调用/token 上限与统计缺 | 实现 `config.rs:85-91,241-271`、`providers.rs`；缺失 `grep retry/backoff/timeout` 无命中、无调用次数上限 |
| [#27 存储单库](blueprint/issues/27-storage-single-db.md) | 🟡 | 应用级单库 + archived_commands + 导入标记在；独立 DuckDB、FTS5、.sqlite 导出、WAL 未做，导入新 id 仅冲突时分配 | 实现 `storage.rs:1,140-149`、`migration/...000001.rs:80-111`；缺失 `grep duckdb/fts5/WAL` 无命中、导出为 zip `assets.rs:212` |

---

## 2. 逐票实现级差距（仅 🟡/❌）

### #01 故事书数据模型 — 🟡

**已实现**：顶层 `meta/world(premise/locations/resources)/skeleton/characters/skills/items/objects/factions/relationships` 与全局 `attribute_dimensions`（`seed.rs:17-53`）；人物 `kind: pc|npc` 有校验（`validate.rs:234`）；判定器命名已改为 `world.check`（`session.rs:198-202`）；objects 有校验与编辑器（`validate.rs:1295`、`ObjectsPanel.vue`）；属性维度类型 number/enum/text + baseline/min/max/modifier_step（`resolve.rs:148-200`）。

**缺口**：
1. **声明区只校验 flags**。设计原文：「新增声明区 `flags` / `events` / `relationship_types` / `target_types`，供引用校验（Lua 兜底可绕过）」。代码仅在 `validate.rs:452-465` 收集 flags 并产出 `undeclared_flag`（`validate.rs:1716-1721`）；`events`/`relationship_types`/`target_types` 全仓无校验引用（`grep relationship_types crates/octopus-engine/src/validate.rs` 无命中）。编辑器侧已有下拉（`RelationshipsPanel.vue:25-26`、`SkillsItemsPanel.vue:38-39`），但服务端不兜底。
   - **该放哪里**：`validate.rs` 声明区收集逻辑（现有 flags 块旁）+ 条件/触发器/关系/技能 target 的引用检查。
2. **关系边字段命名分裂**。设计是 `{ from, to, type, value }`（#01）；校验兼容 `from_id/from` 与 `to_id/to`（`validate.rs:1411-1418`），但条件求值只认 `from_id/to_id`（`conditions.rs:72-77`），编辑器只写 `from_id/to_id`（`RelationshipsPanel.vue:39-91`）。用 `from/to` 写的故事书会让 `relationship_ge` 静默失效。
   - **该放哪里**：`conditions.rs` 读取兼容两套键（或统一到一套并升格）。
3. **叙事字段 Markdown 只有编辑态**：`document/` 组件做 Markdown 渲染，但存储就是字符串，无发布期格式约束（决策第 12 条）。属可接受，但需在数据模型/校验中明确「字段即 Markdown」。
4. **objects.actions / condition 无运行期消费**（见 #04 interact 缺口）。

**依赖**：声明区校验是 #04/#12/#13 引用校验的共同前置。

### #03 核心游戏循环 — 🟡

**已实现**：以玩家输入为回合边界（`session.rs:909-1090`）；`RoundStart→StoryThinking→Story AI→CharacterThinking→角色 AI→结算→RoundEnd`；回合内多次意图顺序结算；双通道 `RoundChannel::Character|Meta`（+扩展 GM，`types 288-293`）；死亡/硬核未特殊处理（与「叙事事件」一致）；确认门与免确认（`session.rs:2388-2429`）。

**缺口**：
1. **角色 AI 未按角色并行、每个 active char 一次调用**。设计原文（#04 §7）：「Character AIs produce intents (parallel)」；#05 ⑤ 还给了「角色 AI 并行调用上限 = 6」的预算。实现是**一次** `ai.character_intents(&char_ctx)` 覆盖所有在场 NPC（`session.rs:1066`），且备注「角色意图暂统一归属首个 NPC」（`session.rs:1072`）。非控制角色的言行无法可靠归属到本人。
   - **该放哪里**：`session.rs::run_round` 角色阶段改为按 `present_actors` 分派 + 并发 join；`AiProvider` 增加按 actor 的入口或复用 `ctx` 裁剪。
2. **叙事类元指令无通路**。设计原文：「叙事类（intervene、节奏调节）：走 `POST /rounds { channel: "meta" }` 交主线 AI」。实现里 meta 回合在 `handle_meta` 后直接 `RoundEnd`（`session.rs:939-943`），`handle_meta` 只认 免确认/帮助/存档，未知即警告（`session.rs:1279-1302`）；`Intent::Intervene` 虽存在（`types 579`）但没有任何路径产出，属于死代码（`session.rs:1884-1890` 仅当被调用时才发 System）。
   - **该放哪里**：`handle_meta` 分支 + 主线 AI 上下文注入 intervene 文本；或把 meta 文本并入 story prompt。
3. **非控制可玩角色由角色 AI 继续扮演**：当前所有 NPC 都交给同一次角色 AI，无法区分「可玩但未受控」与「场景 NPC」，#03 的语义未落实。

**依赖**：#04 的 actor/scope 约束。

### #04 AI→引擎动作协议 — 🟡

**已实现**：discriminator union + `IntentEnvelope`（`types 536-616`）；Validate→Resolve→Commit 骨架；六种驳回码（+2 扩展，`types 630-644`）；Lua 7 挂载点（`lua_host.rs:44-88`）；`finish_turn` 解析后 no-op、不入叙事（`session.rs:1942`）；`ResolutionResult` 的 outcome/triggered_events（`types 471-484`）；回合级 `request_id` 幂等（`session.rs:918-924`）。

**缺口**：
1. **`interact` 完全缺失**。设计原文：「保留 `interact { object_id, action }`；#01 补完整 objects 实体」。`grep "Interact" crates/` 无命中，`KNOWN_INTENTS` 也没有（`protocol.rs:124-143`）。objects 模型与编辑器都在，但运行期无人调用 → #01 objects 是「只写不读」的数据。
   - **该放哪里**：`Intent` 枚举 + `protocol.rs` catalogue + `session.rs::handle_intent` 分支 + 校验（object 存在、action 合法）。
2. **`check` 意图未接入判定器且缺 opposed 字段**。设计原文：「`check` 增加 `opponent_id` / `target_value`（`mode = opposed` 时必填）」。`types 570-576` 只有 `attribute/difficulty/actor_id`；`session.rs::run_check` 硬编码 `1d20`、目标默认 12、固定置 flag `mine_foreshadow`（`session.rs:2431-2491`），完全不用故事书 `world.check`、`attribute_modifier`、`degree_thresholds`。判定器实现（`resolve.rs`）只被技能路径使用（`command.rs:229-336`）。
   - **该放哪里**：`run_check` 改为走 `resolve_checker` + `rules.global_checker()`；Intent 增字段；移除 demo flag。
3. **Query 类基本为空**。`query_world` 在 `session.rs:1935` 与 Lua 路径 `lua_host.rs:2370-2371` 均为 no-op；`query_character`/`query_relationships` 不存在。设计 §6 的 scope 过滤（Character AI 只能看本场景/自身关系）无实现。
4. **`intent_id` 幂等去重未实现**。设计原文：「Engine maintains per-round intent_id set. Duplicate id returns cached ResolutionResult」。`IntentEnvelope.intent_id` 从未被 engine 使用；只有 `request_id` 在回合层去重。
5. **回合内 3 轮 continue 循环未实现**：`run_round` 主线一次、角色一次，没有「continue 最多 3 轮」。
6. **权威边界未强制**：`resolve_intent_actor` 靠文本猜测（`session.rs:1691-1721`），没有按 AI 角色限制可见范围；Meta 类 AI 不可发起这点靠枚举里没有 switch_character/save 实现（部分成立）。
7. **function calling 未使用**：`rig_provider.rs:494-497` `tools: Vec::new()`，靠文本 JSON + `parse_intents`。设计 #04 标题即「结构化工具调用」，此处为等价实现，需在文档中确认可接受。

**依赖**：#01 声明区、#12 判定器、#13 骨架。

### #05 上下文与记忆管线 — 🟡

**已实现（仅①的一半）**：世界前提（`session.rs:311-318`）、当前场景描述、在场人物人格档案（`session.rs:379-409`）、关键词触发 lore（`session.rs:214-301`）、故事书叙述段（`session.rs:332-372`）、token 预算裁剪（`lore_budget_chars` `session.rs:68-74`）。

**缺口（②③④⑤ 全部）**：
1. **双层摘要**（回合微摘要 + 场景压缩）零实现：`grep summary/摘要 crates/` 无业务命中；无 `round_summaries`/`scene_summaries`。
2. **DuckDB 向量检索与 FTS5 兜底**零实现（见 #15/#27）。
3. **top5 相关事件注入**零实现：`TurnContext`（`ports.rs:77-115`）没有 `memories` 槽位，prompt 模板也没有【相关往事】（`rig_provider.rs:425-446`）。
4. **50% 上下文 / 50% 意图预算**未实现：只有 lore 字符裁剪与一个全局 `turn_token_budget`（`config.rs:122-123`）。
5. **角色 AI 严格所见即所知未实现**：`character_intents` 与主线共用同一 `turn_prompt(ctx)`（`rig_provider.rs:538-553`），角色 AI 拿到世界前提、lore、任务、遭遇、全场景清单、canon（`rig_provider.rs:425-446`）。#05 ④ 要求只给自身卡/关系/场景/在场摘要。
   - **该放哪里**：`rig_provider` 按 `ProtocolRole` 出两套 prompt；`TurnContext` 增加 role 过滤后的视图。
6. **规格已写、未实现**：`docs/memory-pipeline.md`（M2–M4）给出端口 `VectorIndex`、DuckDB 表、FTS5、微摘要 intent、检索注入、409；当前代码均未落地。

**依赖**：#15（M1 已完成）→ #27（DuckDB/FTS5）→ #05。

### #06 状态与持久化 — 🟡

**已实现**：应用级单库 SQLite 承载 storybooks/saves/commands/archived_commands/maintenance（`migration/...000001.rs`、`storage.rs:140-197`）；叙事事件（narrate/dialogue/emote）作为权威条目落进 commands（`storage.rs:734-757`、事件→状态投影 `session.rs:2531-2554`）；重放纯投影、不调 AI（`session.rs:678-703`）；驳回只进演出流不进日志（`session.rs::reject`）；分支保留在 archived_commands（`storage.rs:696-732`）。

**缺口**：
1. **全量快照完全缺失**。设计原文：「全量物化状态 JSON，`{ seq, taken_at, state_json }` … 时机 = 每 ~100 条命令 + 场景切换 + 干净退出 + 手动存档。保留最近 5 份」。`grep snapshot` 全仓无命中；`manual_save` 注释明说「v1 不落快照」（`api/lib.rs:1343-1355`）。启动永远是「从零重放全部命令」，设计的「最新快照 + 其后命令」加速路径没有。
2. **RNG 消耗未持久化、重放不推进 RNG**。设计原文：「每次判定…命令日志记录 `rng_consume` 条目…重放时直接使用命令日志中已记录的随机值」。`DeterministicRng.consumed` 有记录（`rng.rs:15-23`），`CommandOutcome.rng_consumed` 也算了（`command.rs:322-333`），但 `session` 从未把它写进日志；`Session::new` 每次用种子重置 RNG（`session.rs:502-503`），`replay` 不消费也不回放（`session.rs:680-703`）。**后果**：进程重启后未来回合的骰值会从序列头重演，与「严格事件溯源/可回放」承诺不符。
   - **该放这里**：命令日志新增 rng 记录条目（或事件附 `rng_consumed`），`replay` 时回填/跳过序列。
3. **Player 实体缺席**：`grep player_id/controlled_instances` 全仓无命中（`controlled` 是 `WorldState` 里的 `Vec<String>`，不等价于 #06⑥ 的 Player 一等实体）。
4. **自动快照时机**（每 ~100 条 + 场景切换 + 干净退出）无实现；无 quit 端点（符合 #24）。

**依赖**：#12 RNG、#27 存储。

### #08 游玩界面 — 🟡

**已实现**：单栏叙事 + 左右栏；演员卡常驻（`ActorsPanel.vue`、`ActorCard.vue`）；流内确认卡（`PendingCard.vue`）；免确认开关（`PlayPage.vue:252-255`）；元指令 `/` 前缀 + 快捷按钮（`InputBar.vue:317-447`）；打字机 + 点击/空格跳过（`FeedText.vue:31`、`play.ts:180-200`）；管线阶段徽标（`PhaseStage` + FeedChat 输入指示）；判定卡（`CheckCard.vue`）。

**缺口**：
1. **三套演出模板并存 + 随时切换未做**。设计原文：「三套演出模板并存、玩家/创作者可随时切换（默认聊天流）…同一场戏、同一回合模型与事件流，仅呈现层不同」。代码自述「单一演出模板（A 聊天流）」（`PlayPage.vue:4`），只有 `FeedChat` 一个渲染器；`play.ts` 里残留 `advancePulse` 注释指向「C 沉浸式」（`play.ts:47`）但无渲染器与切换器。事件 schema 本身零呈现字段（符合），所以这是纯前端。
2. **判定卡呈现跟随故事书判定器**：事件载荷能带 `expr/rolls/mod/target`，但当前 `check` 意图产生的是硬编码 1d20（见 #04/#12），所以「跟随故事书判定器」在 check 路径上不成立；技能路径成立。
3. **NarrativePrefsDialog** 只覆盖叙述段偏好，不是演出模板。

**依赖**：#17（已完成）→ 前端渲染器；#19 主题 token。

### #10 LLM Provider 调研 — 🟡

**已实现**：rig 0.42；`openai::CompletionsClient` 支持 openai/openai-compatible/ollama（`rig_provider.rs:15,63-102`、`api/ai.rs:15-45`）；模型目录/探测/连通性测试（`providers.rs`）。

**缺口**：`ProviderConfig.kind` 注释声明 `anthropic`（`config.rs:38`），但 `ai.rs` 对所有 kind 都用 openai 客户端（`ai.rs:218`），Anthropic 原生协议未接线；rig 的多 Provider 归一未兑现。工具调用未用（`rig_provider.rs:494-497`）。若 v1 只承诺 OpenAI 兼容，应把 kind 收窄到实际支持集。

### #12 规则系统 — 🟡

**已实现**：声明式判定（骰式解析 `resolve.rs:60-130`、mode gte/lte/opposed `resolve.rs:283-287`、`attribute_modifier` 覆盖 + 中心偏移 `resolve.rs:183-200`、`degree_thresholds` `resolve.rs:203-222`）；Lua 判定归一化契约（`resolve.rs:306-333`）；双作用域 `skill.check` 覆盖/引用全局（`command.rs:172-281`）；资源 numerical/binary + cost 扣减（`command.rs:98-137`）；效果四层 immediate/status/triggers/modifiers（`effects.rs`、`modifiers.rs:3,89-94`）；RNG 归引擎。

**缺口**：
1. **`check` 意图绕开判定器**（详见 #04 缺口 2）：`session.rs:2431-2491` 硬编码 1d20 + 固定 flag。
2. **opposed 无表达入口**：枚举有 `Opposed`（`types 51`），`resolve` 也处理，但没有任何 Intent/配置能从玩家/AI 侧传入对抗方（只靠调用方给 `target`）。
3. **status 叠加策略 add/max 未生效**：`StatusStack` 存在（`types 167-175`），但落 delta 时 stack 恒为 None（`effects.rs:233,271`），`apply_delta` 固定 replace（`session.rs:2640-2645` 注释自认「add / max 待状态定义随 delta 携带」）。
4. **`natural_recovery` 的 per_turn / per_scene 不触发**：`RecoveryTrigger::active_on` 对两者返回 false（`recovery.rs:35-39`），`rest_deltas` 只在短休/长休端点被调（`session.rs:891`、`api/lib.rs:1119-1120`）。设计原文：「按回合/场景/休息」。
5. **rng_consume 不落日志**（见 #06 缺口 2）。

**依赖**：#04（check 意图字段）、#06（RNG 持久化）。

### #13 骨架语义 — 🟡

**已实现**：条件表达式树 all_of/any_of/not + 原子谓词 + Lua 兜底（`conditions.rs:40-85`）；回合末求值、标记达成/触发、发 `StateUpdate` + System 提示、不自动切（`session.rs:1139-1175`）；goal 的 hidden/primary 进投影（`session.rs:658-659`）；scene 切换广播 `scene_change` 给 Lua、按 scenes tick、重算在场（`session.rs:1179-1216,1916-1934`）；无 advance_chapter/trigger_beat/complete_goal/query_skeleton（符合设计）。

**缺口**：
1. **`advance_scene` 缺省 target 不前进**。设计原文：「`target_scene_id` 缺省 → 顺序链下一 scene」。`session.rs:1918-1920` 只有 `Some(target)` 才 `switch_scene`，`None` 时只发一条 Resolution，场景不变。
2. **`abandon` 不做任何校验**。设计原文：「缺省 false → 校验主 goal 已达成，否则驳回」；实现无论 abandon 与否都直接放行（`session.rs:1916-1934`）。
3. **目标 scene 进入前置未校验**（scene 是否有 precondition 字段本身也未定义）。
4. **`repeatable` 不重触发**。设计原文：「`repeatable` 默认 false（一次性），true 则每次满足都重新提示」。`evaluate_skeleton` 对已触发的一律跳过（`conditions.rs:112-114,128-129`），repeatable 字段未被读取。
5. **chapter 级 goal 聚合未实现**：求值遍历所有 chapter/scene，不区分主/副 goal 对切换的约束。
6. **命名**：设计用 `beat`/`beat_fired(beat_id)`，实现已统一为 `trigger`/`trigger_fired`（`types 102-118`、`upcast.rs:5-9`），需在 #01/#13/CONTEXT 术语上确认这是最终命名。

**依赖**：#01 声明区、#04 Progression 意图。

### #14 存档版本迁移 — ❌

**已实现**：故事书读侧升格 v1→v3（`upcast.rs:17-56`，含 beats→triggers、条件键、内联 status 提升、骨架 id 回填）；旧命令事件 EntityRef.kind 升格（`upcast.rs:194-213`）；存档内嵌冻结模板（`saves.storybook_json`）；故事书双版本 schema_version + revision（`storage.rs:388-444` 发布 bump）；库结构版本由 SeaORM migrations 管理（`storage.rs:167-170`）；维护历史表 + 端点（`storage.rs:796-824`、`api/lib.rs:1030-1035`）；库内归档表（`migration/...000001.rs:80-96`）。

**缺口**：
1. **升级流程整体缺失**。设计原文（#14②/#21②/#24②）：dry-run 报告 → 逐项人物裁决（freeze/departure）→ 确认执行 → 自动备份 → 维护历史落账。后端路由只有 `GET /api/saves/{id}/maintenance` 等，**没有** `POST /api/saves/{id}/upgrade/dry-run` 与 `POST /api/saves/{id}/upgrade`（`api/lib.rs:431-455` 无）；前端 `upgradeDryRun/upgradeExecute` **只调 mock，从不发真实请求**（`frontend/src/api/index.ts:334-344`）。`needs_upgrade` 服务端从不计算，创建时写死 false（`api/lib.rs:804`、`storage.rs:490-535` 直接读列），所以真实环境永远不会出现升级提示。
2. **日志/快照 `format_version` 缺失**。设计原文：「日志条目、快照：各自携带 `format_version`，读取侧逐条/逐份识别」。`EventEnvelope`（`types 363-376`）与 commands 表（`migration/...000001.rs:50-65`）均无该字段；升级是「无条件应用全部升格」。
3. **纯函数版本链不完整**：只有故事书与事件 kind 的定向升级，没有 `upcast_<artifact>_vN_vN+1` 的可组合链；快照无升格器（无快照）。
4. **golden fixtures 未建立**：仓库无 `crates/octopus-engine/tests/`、无 fixtures 目录（`find -type d -name fixtures` 仅命中外依赖）。
5. **新原点未实现**：`POST /api/saves/{id}/origin` 直接返回 501（`api/lib.rs:1365-1376`），而设计明确「新原点 = 玩家侧 v1 功能」。前端 `newOrigin` 会收到错误并 toast（`drawer.ts:139-146`）。
6. **升级前自动备份**（单库下 = 导出独立包）未实现（随升级流程缺失）。

**依赖**：#01（不可变 id 纪律 + 声明区）、#27（导出包）、#20（fixtures 测试）。

### #15 Embedding 选型 — 🟡

**已实现（M1）**：fastembed 后端（`octopus-ai/src/embedding.rs`，`bge-small-zh-v1.5`=512、`bge-m3` 明确报错并给出替代）与 rig Provider embedding；`EmbeddingBackend` 端口固定维度（`ports.rs:149-155`）；config 默认 `fastembed` 供应商（`config.rs:189-207`）。

**缺口**：
1. **DuckDB FLOAT[512] + 余弦 + VSS HNSW 零实现**：`grep duckdb` 全仓无命中，无 `octopus-index` crate，`octopus-engine/Cargo.toml` 无 duckdb 依赖；`.embed()` 在业务代码中无调用（仅 provider 测试），即 embedding 算完没地方存、没人检索。
2. 因此 #05 检索链路断在「有向量化能力、无向量存储与检索」。

**依赖**：#27。规格见 `docs/memory-pipeline.md` M2。

### #16 一致性/防幻觉 — 🟡

**已实现**：明确不做机检；机制事实靠 validate 管线（`session.rs::handle_intent` 无结算不生效）；玩家自纠三路径中的「重述」天然存在。

**缺口**：
1. **`intervene` 通路缺失**：设计把 intervene 列为玩家自纠主路径并「记入命令日志（可审计）」；实现里 meta 回合不进 AI、Intervene 无产出（见 #03/#04），所以这条自纠路径是死的。
2. **角色 AI 认知边界未实现**：#04 查询范围限制 + #05 上下文隔离都缺（见 #05 缺口 5），角色 AI 能看到全局。
3. **prompt 一致性条款只有一句优先级排序**（`rig_provider.rs:426`：「已裁定的事实 > 人物设定 / 世界设定 > 场景与任务 > 玩家输入」），没有 #16② 的「不得与注入事实矛盾；对在场见证的事实不得矛盾，此外可自由无知、猜测、记错」这类显式条款。

**依赖**：#03/#04/#05。

### #18 前端应用结构 — 🟡

**已实现**：三路由 `/ · /storybook/:id/edit · /play/:saveId`（`router.ts`）；存档管理进游玩页抽屉（无独立存档区）；Pinia feature store（play/editor/list/drawer/theme）；EventSource 薄封装 + 指数退避重连 + onResync（`api/index.ts:914-963`）；seq 水位线去重（`play.ts:358-361,491-493,547-552`）；`state_changes` delta 本地投影（`play.ts:203-207`）；历史分页回读（`play.ts:517-552`）。

**缺口**：
1. **无独立 `communication/` 层**：stream+api 全在 `frontend/src/api/index.ts`（设计要求两半独立层）；`createStream()` 工厂未泛化（pair 用独立 `fetch` 流，`api/index.ts:627`）。
2. **模板切换/多渲染器不存在**（见 #08/#19）：ring buffer 只管当前回合这点成立，但「切换演出模板从 history 拉取」无对象可切。
3. **无客户端空闲检测（~30s）**：设计 #18③ 要求兜住半开连接；实现只有服务端 `KeepAlive::default()`（`api/lib.rs:1446`）+ 客户端指数退避。
4. 模块命名与设计文档不一致（`stores/play.ts` 单 store 而非 engine-projection 独立 store），功能覆盖但结构有差。

**依赖**：#08/#19。

### #19 演出模板自定义 — ❌

**设计要求（v1 收窄后）**：「v1 只做 CSS token 换肤 + 内置三模板」；token 语义命名 `--tpl-<组>-<槽位>`，manifest 声明即文档即表单。

**现状**：`grep -- --tpl- frontend/src` 无命中；`frontend/src/theme/` 是应用主题（theme.ts/tokens.ts），不是模板 token；只有 1 个内置模板（`PlayPage.vue:4`）。无模板 manifest、无模板目录扫描、无换肤设置面板、无「重新加载模板」。插件 API 按决议正确降级 v1.1（`map.md:55`），这部分不算缺口，但 **v1 范围本身（换肤 + 三模板）完全未做**。

**该放哪里**：前端新模块（如 `frontend/src/pages/play/narrative/{chat,script,immersive}` + `theme/template-tokens`）；后端可在 app 数据目录 `templates/` 扫描（v1 可仅内置）。

**依赖**：#08（渲染器）、#17（已完成）。

### #20 Rust 模块与测试 — 🟡

**已实现**：四 crate + thin bin + 额外 launcher（`Cargo.toml`）；依赖向内（`lib.rs:1-4`）；端口四 trait（`ports.rs:130-155`）；ts-rs 生成 `crates/octopus-types/bindings/` 与 `frontend/src/types/generated/`；测试分布：engine 单测 + api tower 测试（`api/lib.rs:1449+`，含重启重放 `test_session_survives_restart_via_command_log`）。

**缺口**：
1. **golden fixtures 未建立**（同 #14 缺口 4）。
2. **无 engine 层集成测试目录**：设计要求的「命令管线确定性重放（engine crate 集成测试）」目前放在 api 测试里；无 `crates/octopus-engine/tests/`。
3. **SSE oneshot 与真实 socket 测试**部分：api 测试用 `spawn_app` 起进程内 router（`api/lib.rs:1450-1490`），但未看到 axum router 不绑端口的 `ServiceExt::oneshot` 形态与确认门往返/水位线断言；确认门测试需补。
4. **`frontend/src/types/index.ts` 不是纯 re-export**（决策第 11 条要求）：它手写了大量接口（`types/index.ts:1-40+`），只 import 了 `EntityRef`；ts-rs 生成物与手写类型存在双份真相漂移风险。
5. **CI 校验生成物漂移**未见到（无 CI 配置变更证据）。

### #21 存档管理 UI — 🟡

**已实现**：抽屉=存读档菜单（`drawer/SaveMain.vue`）窄卡+展开；升级焦点向导（`drawer/WizardView.vue`，面包屑/步进）；打开存档可关横幅（`PlayPage.vue:51,285`）；dry-run 分组与人物内联裁决、未裁决禁用确认；维护历史表（`drawer.ts:30-85`）；导出/导入按钮（`ListPage.vue:77-142`、`SaveMain.vue`）；重命名/删除入口；「新导入」标记（`SaveMain.vue:211`）。

**缺口**：
1. **升级向导的数据来自 mock**（`api/index.ts:334-344` 无 `isMockMode()` 分支、无 fetch），后端没有对应端点（见 #14/#24）。真实模式打开升级向导会展示 mock 报告并「执行」成功，属于**假实现**，最危险的一类差距。
2. **新原点操作会失败**：`drawer.ts:139-146` 调真实端点 → 后端 501。
3. **导出格式非设计**：设计「单文件 .sqlite」，实现为 `.octopus.zip`（save.json + assets，`assets.rs:212`、`api/lib.rs:1394`）；导入同理。
4. **未裁决完禁用确认**已做，但「执行前自动备份提示」对应的备份不存在。

### #23 编辑器后端 API — 🟡

**已实现**：单资源两态 `released_json/draft_json + revision + draft_version`（`migration/...000001.rs:9-25`）；八端点 `GET/POST /api/storybooks`、`GET/PUT/DELETE /:id`、`/publish`、`/api/validate`、pair（`api/lib.rs:399-429`）；PUT 内容级幂等 no-op（`storage.rs:340-342`）、base 不匹配 409 `draft_conflict`（`storage.rs:344-349`）；publish 事务 + 校验门 422（`storage.rs:388-444`、`api/lib.rs:692-725`）；删除不影响存档（内嵌冻结）；无状态 /api/validate（`api/lib.rs:742-744`）；error 信封 `{code,message,detail?}`（`error.rs:93-101`）；校验错误 `{severity,code,target,message,related_refs}`（`types 998-1022`）；Lua 静态预检（`lua_lint.rs`、`api/lib.rs:700-708`）；editor store debounced 自动保存 + baseVersion 冲突裁决（`editor.ts:203-266,469-477`）；服务端 related_refs。

**缺口**：
1. **声明区引用完整性只覆盖 flags**（同 #01 缺口 1），#23 修订明确要求四类都校验。
2. **结对端点与设计不符**：设计是 `POST /api/storybooks/:id/pair`、**服务端不持久化会话**、body `{messages, storybook}`；实现是 `/api/pair/chat[/stream]` + 一整套 `threads/messages` 持久化 CRUD（`api/lib.rs:397-428`、`pair.rs`、migration 000004-000011）。功能更强，但偏离「无状态、会话历史在 pair store、换 tab/重启丢失可接受」，且引入服务端会话数据。
3. 校验器字段存在但 `related_refs` 的实际填充需逐条确认（类型在，填充覆盖有限）。

**依赖**：#01。

### #24 游玩页后端 API — 🟡

**已实现**：隐式按存档寻址（`session_for` `api/lib.rs:145-227`）；`POST /api/saves`（title + controlled_character_id，未发布 409）`api/lib.rs:750-838`；`GET /api/saves/:id`（含内嵌 storybook 全文，口径已统一）、`/state`、`/stream`、`POST /rounds`、`POST /rounds/:round_id/confirmation`（过期 409 expired，`error.rs:71-73`）；`PATCH/DELETE`；`GET /maintenance`；`PUT /settings`；`GET /history`；`POST /character`；`GET /export`、`POST /import`；`POST /save`（检查点）；`POST /origin`(501)。请求幂等 `request_id`（`session.rs:918-924`）。

**缺口**：
1. **`409 round_in_progress` 永远不会返回给客户端**。设计（#24 修订、`docs/memory-pipeline.md` M4）要求：非 idle 提交返回 409。`submit_round` 把 `run_round` 整个丢进 `tokio::spawn` 并总是返回 202（`api/lib.rs:936-957`），busy 检查在 spawn 内部（`session.rs:925-927`），错误只 `tracing::warn`。前端因此拿不到 409、只会超时/静默。
   - **该放哪里**：`submit_round` spawn 前先查 busy（`Session::is_busy()`，需要新增只读方法）→ 返回 409。
2. **升级端点缺失**：无 `/upgrade/dry-run`、`/upgrade`（见 #14）。
3. **branch 端点缺失**：设计 `POST /api/saves/:id/branch { from_seq }`（dev-gated）；实现只有 `/rerun`（`api/lib.rs:970-1018`）做「重跑本轮 + 归档」，不是通用 branch。
4. **`needs_upgrade` 从不计算**：create/list 都读存量列（`api/lib.rs:804`、`storage.rs:490-535`），列表页/横幅/角标在真实后端永远不出现。
5. **导出非 .sqlite**（见 #21/#27）。
6. **安全边界**：只绑 127.0.0.1 已成立（`start.sh:43`）；但**导入无大小/结构上限**（`unpack_bundle` 只做 zip 解析，`assets.rs:246`；`MAX_ASSET_BYTES` 只限图片 `assets.rs:22`），与修订「导入加大小/结构上限」不符。
7. **query 参数命名**：设计 `GET /api/storybooks?released=1`，实现 `released_only`（`api/lib.rs:623-632`、`api/index.ts:169`）——功能等价但契约名不一致。

### #26 Provider 配置与凭据 — 🟡

**已实现**：本地配置文件 + 0600（`config.rs:261-271`，注意实现是 `config.json` 而非设计 `config.toml`）；`[providers.*]` + `[roles]`（story/character/pair/embedding，`config.rs:84-91`）；`OCTOPUS_CONFIG` 覆盖路径、`OCTOPUS_AI/OCTOPUS_EMBEDDING` 后端开关（`ai.rs:122-132,239`）；设置 UI（`SettingsDialog.vue`、`ModelPicker.vue`）；连通性测试/模型探测（`providers.rs`）；embedding 固定本地 bge（`config.rs:231-234`）；每回合 token 预算热更新（`config.rs:122-123,291`、`api/lib.rs:88-110`）；密钥空值过滤（`ai.rs:33-44,217-222`）；存档级模型单一 ModelRef（`storage.rs:604-634`）。

**缺口**：
1. **失败与降级未实现**：无超时/重试/指数退避；无「结构化输出解析失败回填重试一次」；AI 失败只 `EngineError::Ai → 500/internal`（`error.rs:83-88`），不会发 `system` 事件中止本回合（`grep retry/backoff/timeout crates` 无命中）。
2. **无 key 引导态未实现**：空 key 只把请求发出去由供应商失败（`ai.rs:33-44` 仅警告），没有「只读引导态、不发起调用」。
3. **成本护栏不完整**：只有全局 `turn_token_budget`；无「每存档单回合调用次数上限」、无累计 token 统计展示。
4. **env 覆盖密钥优先级未实现**：设计「环境变量 > 配置文件」，代码只有 `OCTOPUS_CONFIG` 路径与后端开关，没有按 provider 的 key env 覆盖。
5. **存档级模型分工**：设计 #26③ 要求三类各自可配——全局 roles 已满足；但「每个存档可分别配置主线/角色」属 `docs/save-role-models.md` 规格（未实现，单一 ModelRef 同用于两角色，`rig_provider.rs:16-17` 注释印证）。

**依赖**：无硬依赖；`docs/save-role-models.md` 为扩展。

### #27 存储单库 — 🟡

**已实现**：应用级单库 `octopus.db`（`start.sh:44`、`storage.rs:140-149`）；故事书/存档/命令/归档/维护同库；按 `save_id` 分区；`archived_commands` 表（`migration/...000001.rs:80-96`）；导入分配新 id（仅冲突时，`storage.rs:1146-1158`）+「新导入」标记。

**缺口**：
1. **独立 DuckDB 向量库未实现**（同 #15）：无 `octopus-vectors.duckdb`、无 duckdb 依赖、无 `octopus-index`。
2. **FTS5 未实现**：迁移无 `CREATE VIRTUAL TABLE ... USING fts5`，`grep fts5` 全仓无命中；关键词检索完全不存在。
3. **导出不是「抽单存档为独立 .sqlite 包」**：实现是 `.octopus.zip`（`assets.rs:212-246`、`api/lib.rs:1378-1405`），不满足设计第 ④ 条的字面要求（自包含诉求满足）。
4. **WAL 未设置**：`storage.rs:141-145` 只 `max_connections`，无 `journal_mode=WAL` pragma；`grep WAL/pragma` 无命中。设计 ⑥ 明确「SQLite WAL」。
5. **整库备份/崩溃恢复**没有对应实现（备份=直接拷贝属运维，可接受；崩溃恢复依赖 SQLite 默认，非 WAL）。
6. **导入未分配新 save_id（无冲突时保留原 id）**：与「导入分配新 `save_id`（不覆盖同 id）」有解释空间——不覆盖成立，但「分配新 id」仅在冲突时才做。

**依赖**：无；#05 依赖本票。

---

## 3. 决策落账核对（map.md + decision-log-design-pass.md）

| # | 决策 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| 1 | 叙事权威 + history 端点 | ✅ | 叙事事件落 commands `storage.rs:734-757`；`GET /history` `api/lib.rs:925-934`；前端回读 `play.ts:517-552` |
| 2 | 应用级单库 + 独立 DuckDB | 🟡 | 单库 ✅ `storage.rs:140-149`；DuckDB ❌（见 #27） |
| 3 | 新原点=玩家侧 v1 + 库内归档 | ❌ | 归档表 ✅ `migration/...000001.rs:80-96`；新原点 501 `api/lib.rs:1365-1376` |
| 4 | 补缺失端点（title/PATCH/DELETE/maintenance/settings） | ✅ | 路由 `api/lib.rs:431-455` |
| 5 | #26 配置与凭据 | 🟡 | 见 #26 |
| 6 | 非 idle 409 + request_id 幂等 | 🟡 | request_id ✅ `session.rs:918-924`；409 客户端拿不到 `api/lib.rs:936-957` |
| 7 | interact + objects 实体 | 🟡 | objects 模型/编辑 ✅ `validate.rs:1295`、`ObjectsPanel.vue`；interact ❌（无 Intent） |
| 8 | 状态类直调端点 / 叙事类走 meta 文本 | 🟡 | 状态类 ✅（switch/settings/save）；叙事类 ❌ `session.rs:1279-1302` |
| 9 | PC/NPC + 开档选主角 | ✅ | `validate.rs:234`；`NewGameDialog.vue:175-190`；`api/lib.rs:832-834` |
| 10 | 语义补漏（声明区/opponent/finish_turn/resolution/六类） | 🟡 | finish_turn ✅ `session.rs:1942`；resolution outcome/triggered ✅ `types 471-484`；声明区仅 flags、opponent 缺、六类缺 interact/query |
| 11 | ts-rs 单一来源 + 命名对齐 | 🟡 | 生成物 ✅ `crates/octopus-types/bindings/`；前端 `types/index.ts` 手写非 re-export；`world.check` ✅ |
| 12 | 非阻断（插件降级/Markdown/预算/127.0.0.1/导入上限/无障碍） | 🟡 | 插件降级 ✅（未做）；Markdown 编辑 ✅ 无格式约束；预算可配 ✅；127.0.0.1 ✅ `start.sh:43`；导入上限 ❌ |
| 派生 | 升级前备份=导出包 | ❌ | 随升级流程缺失 |
| 派生 | auto_confirm 进 state meta | ✅ | `ProjectionMeta.auto_confirm` `types 748-757` |
| 派生 | 关键词检索统一 FTS5 | ❌ | 无 fts5 |
| 派生 | 叙事条目进日志 | ✅ | `storage.rs:734-757` |
| 勘误 | #04 五类→六类 | 🟡 | 六类框架在，interact/query 未齐 |
| 勘误 | #07 version→revision | ✅ | `storage.rs:388-444` |
| 勘误 | #17 移除 snapshot 行 | ✅ | `PlayEvent` 无 snapshot `types 343-361` |
| 勘误 | #19 templates/ 目录 | ❌ | 无模板体系 |
| 勘误 | #06 vs #05 FTS5 | ❌ | 无 fts5 |
| 勘误 | #24 内嵌故事书口径 | ✅ | `SaveDetail.storybook` `types 803-808` |

---

## 4. 推荐实现顺序（依赖优先）

> 前置判断：#01 声明区校验与 #04 协议补全是一切规则类功能的地基；#27 是 #05/#15 检索的地基；#24 的 409 是廉价高收益修复。

| 序 | 缺口 | 依赖 | 规模 | 说明 |
|---|---|---|---|---|
| 1 | #24 `409 round_in_progress`（submit_round spawn 前判 busy） | 无 | 小 | 单点改动，前端已有 toast 语义；`docs/memory-pipeline.md` M4 已写清 |
| 2 | #01 声明区四类校验 + 关系字段命名统一（含 from/to 兼容） | 无 | 中 | 影响 #12/#13 引用校验与编辑器下拉的权威性 |
| 3 | #04 协议补全：interact、opponent_id/target_value、response 归属、intent_id 去重 | #01/#12 | 大 | 六类意图补齐 + objects 真正可用 + opposed 可表达 |
| 4 | #12 check 意图走判定器 + status add/max + per_turn/per_scene 恢复 | #04 | 中 | 让「故事书声明判定器」对 check 生效；删掉硬编码 1d20 与 demo flag |
| 5 | #04 多轮 continue（≤3）+ 角色 AI 并行/按角色归属 | #04 | 大 | 核心循环语义，影响预算与事件归属 |
| 6 | #06 RNG 消耗持久化 + 重放推进（+ 可选全量快照） | #12 | 中 | 修正「重启后骰序重演」的确定性缺陷；快照是 #14 兜底前提 |
| 7 | #13 advance_scene 缺省下一场景/abandon 校验/repeatable | #01/#04 | 中 | 骨架推进闭环 |
| 8 | #27 单库补全：WAL + FTS5 迁移 + 导入大小上限 + .sqlite 导出评估 | 无 | 大 | FTS5 为 #05 兜底；WAL 为并发；导出格式二选一先定契约 |
| 9 | #27/#15 DuckDB 向量库（新 crate octopus-index） | #27 | 大 | 端口 `VectorIndex` + 维度重建；按 `docs/memory-pipeline.md` M2 |
| 10 | #05 记忆管线（微摘要+场景压缩+top5 注入+角色隔离） | 8/9 | 大 | 规格现成；角色隔离可先行（纯 prompt 裁剪） |
| 11 | #14 升级流程（dry-run/execute/needs_upgrade/备份/format_version/fixtures/新原点） | #01/#27/#20 | 大 | 后端 3 端点 + 前端去 mock；新原点与升级共用导出包 |
| 12 | #24 branch 端点 + 导出包契约落地 | #14 | 中 | branch dev-gated；export/import 统一 |
| 13 | #26 韧性：超时/重试/退避/解析失败重试/无 key 引导/成本护栏统计 | 无 | 中 | 与回合原子性（#04）耦合，别写半截命令 |
| 14 | #19 CSS token 换肤 + 内置三模板（#08 多渲染器） | #17 | 大 | 纯前端；事件 schema 已零呈现字段，可直接开工 |
| 15 | #18 结构收口（communication 分层、空闲探活、createStream 泛化）+ #08/#21 细节 | #19 | 中 | 低风险重构 |
| 16 | #20 golden fixtures + engine 集成测试 + SSE oneshot/确认门往返 + types re-export/CI 漂移 | 各票 | 中 | 每加一个升格版本补一个 fixture |

**Top 5 最大缺口**（按影响 × 工作量）：
1. #14 存档版本升级整体缺失（前端假实现 + 后端零端点 + needs_upgrade 永不计算）。
2. #05+#15+#27 记忆/检索链路：有 embedding 能力，无 DuckDB、无 FTS5、无摘要、无注入。
3. #19+#08 演出模板体系：设计产品核心之一（三模板+换肤），当前单一聊天流。
4. #04 协议补全：interact/opponent/query/去重/多轮/并行/scope，缺一半以上。
5. #06 确定性缺口：RNG 消耗不落日志、无快照，重启后重放语义不严格。

---

## 5. 设计本身含糊、需实现者取舍的点（建议一律按「更复杂更保守」的方向做）

1. **命名分裂：beat vs trigger、from/to vs from_id/to_id、`world.rules.check` vs `world.check`**。
   现状代码已选 trigger/from_id/world.check。**保守做法**：数据模型统一到一套（建议保留 #13 的 trigger 术语并同步 CONTEXT.md），读取侧永远双键兼容 + 升格补写，绝不静默丢引用。

2. **`check` 意图与技能的判定路径统一程度**。
   设计把 check 列为 Mechanical 意图、又说技能可 `skill.check` 覆盖。**保守做法**：抽一个 `resolve_any_check`，check 意图与技能共用同一 `world.check`/profile/thresholds 路径，避免两套判定语义。

3. **opposed 的对抗方取值**。
   设计只给 `opponent_id`/`target_value`，没说对抗方是否也走判定器。**保守做法**：对抗方缺 `target_value` 时也用其属性维度 + 对方判定器算出一个 target，命令日志记录双方结果，避免引擎自造目标。

4. **角色 AI 的 actor 归属与 scope 过滤**。
   设计给「Character AI 只演自己」，但没定义「一个角色一次调用还是一批」。**保守做法**：每个在场 NPC 一次调用（并行上限 6），payload 带 `actor_id`，引擎按模板 id 硬过滤非本角色的 speak/emote。

5. **`finish_turn` 与 continue 的语义边界**。
   设计「finish_turn=循环终止信号、不算意图」，又说最多 3 轮 continue。**保守做法**：把 `finish_turn` 从 `Intent` 枚举中移出为工具循环控制字段，仅在协议解析层消费；日志不记。

6. **双层摘要的存储形态**。
   `docs/memory-pipeline.md` §3.2 自陈「派生表 vs 记入日志」两可。**保守做法**：新增 `summary` 意图 → 写派生表 + 同步进 FTS5/向量库；无论选哪种都保证 `replay()` 不读摘要、不依赖摘要。

7. **50% 上下文 / 50% 意图预算**。
   现状只有全局 token 预算与 lore 字符裁剪。**保守做法**：先给记忆段独立上限（`min(K*单条上限, budget*50%)`），再逐步把 lore/persona/narrative 纳入统一预算器，别一次性重写优先级。

8. **导出格式：.sqlite 还是 zip**。
   设计反复写「独立 .sqlite 包」，实现已是 zip + assets（图片内容寻址）。**保守做法**：把导出契约升级为「单文件容器（zip 内含 save.json + 资产）」，同时在文档与 #27 中正式修订「.sqlite」措辞；若必须字面满足，则 zip 内附一个可独立打开的 sqlite。二者都保留导入兼容。

9. **快照是否需要（事件溯源已可零重放）**。
   设计把快照定为「启动加速缓存、可丢弃」。**保守做法**：实现快照表 + 启动「最新快照 + 其后命令」，保留 5 份；它同时是 #14「新原点兜底」与「格式过旧直接作废」的前提。

10. **Player 实体现在进模型 vs 暂缓**。
    设计明确「现在进模型」。**保守做法**：现在加 `players`/`player_controlled` 最小表与命令 `actor` 语义，房间/网络不建；否则后续多人预留会返工命令 schema。

11. **`needs_upgrade` 的判定与刷新时机**。
    设计「服务端算好」。**保守做法**：在 `list_saves`/`get_save` 读取时用「内嵌 revision vs 故事书当前 revision」实时计算，不信任存量列；升级后由同一计算自然归零。

12. **升级冲突与 id 不可变的执行纪律**。
    设计要求「实体 id 发布即不可变」，但没有强约束。**保守做法**：发布门禁止同一 id 换 kind/重命名语义、禁止修改已发布 id 的字段；人物消失逐项裁决（freeze/departure），未裁决不落库。

13. **`status` 的 stack 需要随 delta 携带定义**。
    现状 delta 只带实例、不带 stack 策略。**保守做法**：在 `StatusInstance` 或 `StateDelta` 内附 `stack`（或在状态定义变更时重建实例），让 `apply_delta` 能执行 add/max。

14. **`.octopus.zip` / token 命名 / query 参数等契约细节**。
    `released` vs `released_only`、`--tpl-*` 命名、manifest 字段。**保守做法**：以 ts-rs 生成类型 + 一张契约表为单一来源，端点 query 参数也纳入契约文档，避免只对类型不对参数。

15. **AI Provider kind 与实际支持集**。
    配置声明 anthropic 但实现只 openai 兼容。**保守做法**：要么接线 rig 的 Anthropic `CompletionModel`，要么把 `kind` 白名单收紧并在设置 UI 明确标注「仅 OpenAI 兼容」，不要留下会静默失败的分支。

---

## 附：只写不读 / 只存在不生效的陷阱清单（核对用）

| 位置 | 现象 | 参见 |
|---|---|---|
| `objects`（`validate.rs:1295`、`ObjectsPanel.vue`） | 可编辑、可校验，但无 `interact` 意图消费 | #04/#01 |
| `CheckMode::Opposed`（`types:51`、`resolve.rs:286`） | 引擎支持，无任何 Intent/配置入口 | #04/#12 |
| `RecoveryTrigger::PerTurn/PerScene`（`recovery.rs:17-38`） | 能解析、`active_on` 恒 false，无处调用 | #12 |
| `StatusStack::Add/Max`（`types:167-175`） | 类型在，落 delta 时恒 `None` | #12 |
| `DeterministicRng.consumed` / `CommandOutcome.rng_consumed`（`rng.rs:15`、`command.rs:322-333`） | 有记录、有返回值，从未写日志/回放 | #06/#12 |
| `Intent::Intervene`（`types:579`、`session.rs:1884-1890`） | 枚举在，无产出路径 | #03/#04/#16 |
| `save.item.needs_upgrade`（`storage.rs:473`） | 列存在，服务端从不重算，创建写死 false | #14/#24 |
| `frontend upgradeDryRun/upgradeExecute`（`api/index.ts:334-344`） | 无 mock 分支判断，真实模式也走 mock | #14/#21 |
| `frontend/src/types/index.ts` | 声称单一来源 re-export，实为手写镜像 | #20（决策 11） |
| `POST /api/saves/:id/origin` | 路由存在，恒 501 | #14/#21/#24 |

---

*本报告为只读分析产物；除本文件外未改动任何仓库文件。*
