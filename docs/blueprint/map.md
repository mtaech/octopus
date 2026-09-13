# 地图：Octopus（通用 AI RPG 软件）

## Destination

产出一份《Octopus：通用 AI RPG 软件》的技术设计蓝图（架构、数据模型、AI 编排、引擎/Lua 边界、游戏循环、状态持久化、UI 规格）。v1 单人、架构预留多人扩展路径；故事书由应用内编辑器编写；AI 走可插拔 Provider；独立开发者、文档可直接照着开工。

## Notes

- 领域：面向玩家的通用 AI RPG 软件——玩家用故事书自定义故事，接入 AI 演绎体验；不是跑团模拟器，也不是 CRPG 引擎。
- charting 已锁定的方向：进程形态 = 本地 Web 服务（axum 后端权威 + Vue3 浏览器前端）；游戏循环 = 结构化骨架 + 主线 AI 即兴；AI→引擎 = 结构化工具调用（function calling）；属性维度用户自定义、技能 = 声明式 + Lua 钩子；状态 = 命令日志 + 快照；记忆 = 静态设定注入 + 滚动摘要 + 相关事件检索；多人 = v1 只做架构预留。
- 每会话必加载的 skills：`domain-modeling`、`grilling`（解决 grilling/prototype 票时）。
- 术语表见仓库根 `CONTEXT.md`。

## Decisions so far

<!-- 一票一行：关闭票的标题（带链接）+ 一句话结论 -->

- [Rust 模块划分与测试策略](./issues/20-rust-modules-testing.md) — 小 workspace 四 crate（types/engine/ai/api + 薄 bin）；依赖向内指向 state/rng 叶子、编排只此一家（command+session）、IO 与 LLM 全走端口（Storage/EventSink/AiProvider/EmbeddingBackend）；ts-rs 生成前端类型；测试分层 = 确定性重放+golden fixtures+Lua 沙箱在 engine、SSE oneshot 在 api、mock CompletionModel 在 ai。
- [Embedding 模型选型](./issues/15-embedding-model.md) — bge-small-zh-v1.5（512d）默认，rig-fastembed 本地加载；bge-m3（1024d）可选升级；EmbeddingBackend 枚举统一 Local/Provider，对外固定维度；DuckDB FLOAT[512] + 余弦相似度 + VSS HNSW。详见 [research/15](./research/15-embedding-model.md)。
- [上下文与记忆管线](./issues/05-context-memory.md) — 静态设定分层注入（系统层+场景层，自然语言渲染）；双层摘要（回合微摘要+场景压缩）；DuckDB 向量检索+关键词兜底 top 5 自动注入；角色 AI 严格所见即所知；上下文 50%+意图 50% 预算分配。
- [引擎职责边界（Rust / Lua / AI）](./issues/02-engine-lua-boundary.md) — 三界划分：引擎管状态权威+结算+Lua宿主；Lua管动态效果+事件触发+判定修正（scoped env白名单，请求-校验-执行）；AI管叙事+角色言行+意图决策。判定混合模式（默认参数化判定器+可Lua覆盖），RNG归引擎独家（engine_rng走命令日志确定性序列）。钩子链按注册顺序fail-fast。
- [Rust 的 Lua 集成生态调研](./issues/09-lua-integration-research.md) — 锁 mlua（rlua 已弃用）；主后端 lua54/lua55，LuaJIT 是 5.1 语义需另取舍；内建沙箱 + scoped env 白名单；热加载复用 into_function、只收文本源码。详见 [research/09](./research/09-lua-integration-research.md)。
- [前后端流式通信选型调研](./issues/11-streaming-transport-research.md) — 单向流用 SSE（axum::response::sse + Vue3 EventSource），双向才用 WebSocket；LLM 流即 SSE、端到端转发；注意 HTTP/1.1 每域名 6 连接上限、多流上 HTTP/2。详见 [research/11](./research/11-streaming-transport-research.md)。
- [可插拔 LLM Provider 生态调研](./issues/10-llm-provider-research.md) — 多 Provider 抽象走 rig-core 的 CompletionModel trait（归一工具/结构化输出/流式）；三家 function calling 同构、structured output 分歧大；DeepSeek 双协议兼容。详见 [research/10](./research/10-llm-provider-research.md)。
- [故事书数据模型](./issues/01-storybook-data-model.md) — 故事书 = 模板唯一事实来源（meta/world/skeleton/characters/skills/items/factions/relationships，JSON+版本）；属性维度全局定义（数值/枚举/文本）；技能单一实体、物品引用为物品技能；关系 = 有向边；人物（模板）≠ 角色实例（运行时）。
- [核心游戏循环](./issues/03-core-loop.md) — 回合 = 玩家输入→主线 AI 意图→角色 AI 意图→引擎结算→流式输出；输入分「角色输入/元指令」两通道；多角色随时切换、非控制角色由角色 AI 扮演；死亡 = 叙事事件；机制动作结算前确认（可关）。
- [运行时状态与持久化](./issues/06-state-persistence.md) — 扁平分面状态、叙事权威（进命令日志）、派生数据非权威；严格事件溯源（记命令 + rng 消耗，驳回意图只进 debug 流）；应用级单库 SQLite；全量快照 = 启动缓存（保留 5 份）；回滚 = append-only 分支；多人预留 = Player 实体 + 命令管线纪律。
- [规则系统设计（判定 / 资源 / 效果机制）](./issues/12-rule-system.md) — 判定器 = 声明式配置或 Lua 脚本（归一化输出 total/margin 由引擎分档），全局 + 技能级双作用域；骰子表达式/比较模式/修正映射/成功度阈值皆可配，无骰则 AI 叙事裁决；成功度按差值分档、与骰面无关；资源 = numerical/binary，natural_recovery 按回合/场景/休息；效果分 immediate/status/triggers/modifiers 四层声明式 + Lua 钩子；同名 modifier 取 max、同名 status 按 stack 策略；RNG 全局种子 + 命令日志记录骰值保证重放。
- [AI→引擎动作协议](./issues/04-action-protocol.md) — 六类意图（叙事/世界交互/机制/查询/元指令/骨架推进）；discriminator union schema 每 type 独立 function；四阶段管线（校验→结算→回写→回填）；7 个 Lua 挂载点；主线 AI 对角色 AI 冲突获胜；回合内最多 3 轮 tool call；角色 AI 并行 + scope 受限；intent_id 幂等去重。

- [故事书编辑器设计](./issues/07-editor-ux.md) — 混合范式（用户反馈修订：A 表单工作台 = 有计划详细编辑、C AI 结对 = 无头绪启发式编辑，两者为主范式；B 文档保留为叙事自由书写载体）；骨架大纲式编辑器；引用双轨（下拉 + @提及）共享引用图 + 实时校验；属性维度独立全局设置区；AI 生成采纳即定型不留来源标记；显式「发布」（版本+1，语义归「存档版本迁移」）。原型见 [prototypes/editor-ux.html](./prototypes/editor-ux.html)。
- [游玩界面设计](./issues/08-play-ui.md) — 叙事 = 多模板（聊天流/剧本式/沉浸式三套演出模板并存、随时切换，默认聊天流；同一回合模型仅呈现层不同）；单栏叙事 + 抽屉布局、演员卡常驻紧凑；机制动作 = 流内挂起确认卡（可全局免确认）；元指令 = 同一输入框 / 前缀 + 常驻快捷按钮；流式 = 就地打字机 + 可跳过 + 管线阶段徽标；判定卡呈现跟随故事书判定器（演示为 d20 风格参数）、成功度按差值分档。原型见 [prototypes/play-ui.html](./prototypes/play-ui.html)。
- [骨架语义（goal / beat 触发与场景推进）](./issues/13-skeleton-semantics.md) — 引擎权威+AI 叙事：goal=声明式完成判据（scene 一主多副、chapter 聚合，达成标记+提示、不自动切）；beat=声明式触发+提示（默认一次性、可设 repeatable、不改状态）；场景切换=顺序链+显式 advance_scene 意图（引擎校验主 goal 或 abandon 放弃分支）；新增第 6 类 Progression 意图、条件走结构化表达式树 + Lua 兜底。
- [一致性/防幻觉约束机制](./issues/16-consistency-anti-hallucination.md) — 故事推进优先、幻觉治理不建机检：机制事实由 #04 校验管线结构性兜底（无结算不生效）；一致性 = prompt 工程（系统 prompt 一致性条款一句）；冲突玩家自纠（intervene / 回滚分支 / 重述）；遗忘靠 #05 摘要+检索，置顶事实 v1 不做；角色 AI 认知边界已有机制 + 一句 prompt 条款。
- [引擎→UI 流式事件协议](./issues/17-engine-ui-event-protocol.md) — 演出流 = 长连接 SSE（包络 id/seq/round/type/ts/actor/intent_id）；四类事件：叙事 scene/narrate/dialogue/emote、机制 pending/check_result/resolution（check_result 独立、level 按 #12 差值阈值分档）、状态 state_update + GET /state 水合（seq 水位线）、控制 phase/round/system；完整事件块 + 前端打字机、结算即推、无 token delta；确认门 = HTTP POST 往返、超时默认取消、免确认 = 引擎侧不发 pending、回合保持 open；state_changes 共享 delta 驱动前端投影；schema 零呈现字段（多模板纯前端）。
- [前端应用结构与模块划分](./issues/18-frontend-modules.md) — 混合分层：communication（stream+api）与 Pinia store 独立层、UI 按区域切片、模板切换只换活动渲染器；全量 store（回合/流 + 世界投影 + 受控角色 + 确认门 pending）+ 当前回合有界 ring buffer；薄封装 EventSource 重连即水合（丢 seq≤水位线）、keep-alive 注释 + 空闲检测心跳；三路由（列表 / 编辑器 / 游玩页）+ 存档管理入游玩页抽屉（#21 落点确定）。
- [存档版本迁移](./issues/14-save-migration.md) — 版本锚点分层各记（库结构全局一处、日志/快照各带 format_version）；故事书双版本（schema_version + 版次 revision，#07 发布语义落地）；存档内嵌冻结模板 + 显式两段式升级（id 发布即不可变；人物消失默认阻挡→遗留冻结/叙事离场逐项裁决；升级记维护历史、引发的世界变更作为命令进日志）；一切迁移 = vN→vN+1 纯函数版本链 + golden fixtures；日志读侧升格为主、显式新原点兜底（旧日志归档入库内归档表，回滚上限=原点）。
- [演出模板的自定义与扩展机制](./issues/19-template-customization.md) — 模板 = manifest + 渲染器统一抽象（内置模板同样有 manifest、渲染器实现可不统一）；主题 = 具名 token 值快照绑定具体模板，换肤仅 CSS 变量覆盖不重绘；插件 API = h() 渲染树 + DOM 逃生舱、呈现只读可预填输入、最小生命周期集、layout 可声明整页、单事件出错回退聊天流；token 语义命名 `--tpl-<组>-<槽位>`、manifest 声明即文档即表单；模板目录全局 templates/、手动重载、与存档完全解耦（缺失回退内置），导出包留口。
- [存档管理界面设计](./issues/21-save-management-ui.md) — 抽屉 = 存读档菜单骨架（窄卡钻取）+ 版次升级独立聚焦向导（报告→确认→结果步进、面包屑返回）；升级提示 = 打开存档时游玩页可关横幅 + 抽屉内角标；dry-run 分组折叠 + 人物消失内联裁决、未裁决完禁用确认；执行前自动备份、完成后维护历史落账；新原点入抽屉、回滚/分支 v1 不进 UI（dev 工具）；导出单文件 .sqlite、导入校验即过带「新导入」标记。原型见 [prototypes/save-manager.html](./prototypes/save-manager.html)。
- [编辑器前端模块结构](./issues/22-editor-frontend-structure.md) — 编辑器 = 单路由 + 范式视图态（A 表单工作台 / C AI 结对 / B 文档三平级，共享同一草稿、localStorage 记偏好、新建默认 C）；B = document 富文本组件（字段级内嵌 + 整页文档视图复用）；骨架 = A 内一级 tab、引用图 + 实时校验 = 跨范式共享 dock；editor store 草稿唯一主人 + debounced 草稿自动保存（不 bump revision）、发布才 bump revision 触发 #14；editor/pair store 暂存建议、采纳经 editor action 落稿；编辑器 API 并入 communication/api、stream 泛化 createStream 工厂（游玩页单例、结对每会话轻量实例）。
- [编辑器后端 API 设计](./issues/23-editor-backend-api.md) — 故事书 = 单资源两态（released/draft 同表 + revision + draft_version）；新建仅草稿、发布显式（released:=draft、revision+1、draft_version+1）；八端点：CRUD + 草稿保存（base_version 乐观并发 409 裁决、内容级幂等）+ Publish 校验门 + DELETE + 无状态 /api/validate + 结对 SSE 短流；校验 = engine 纯函数 validate_storybook（schema + 引用完整性 + Lua 静态预检），error 挡发布、引用图客户端投影 related_refs 服务端兜底；结对 = 无状态、全量草稿随请求、建议结构化事件（采纳时校验）、AiProvider chat 适配。
- [游玩页后端 API 设计](./issues/24-play-backend-api.md) — 隐式按存档寻址、不建会话资源（SSE 流即会话）；开档 POST /api/saves、续玩 GET /api/saves/:id、水合 GET /api/saves/:id/state；回合 = 单 POST 触发 + 常驻 SSE（GET /api/saves/:id/stream），确认门 POST /rounds/:round_id/confirmation（超时 409 expired）；存档 = 手动 POST /save、升级 dry-run→执行两步、新原点玩家侧、分支 dev-gated、导出 GET /export、导入 POST /import；列表页数据源 = GET /api/storybooks?released=1 + GET /api/saves（含 needs_upgrade）；多 tab 不做锁、在途回合引擎权威、无 quit 端点。

- [列表页设计（故事书 / 存档入口）](./issues/25-list-page-ui.md) — A 分区并置（动作带：继续游玩大卡 + 新建/导入 → 故事书网格 → 存档列表）；故事书卡 = 标题/版次/发布态 + 新建游戏·进编辑器；存档卡 = 标题/故事书名/版次/时间/升级角标 + 继续游玩·升级（仅 needs_upgrade 时出现）；管理操作全不进列表卡；空态分流（无故事书→去编辑器新建，无存档→新建/导入引导）；列表页只做入口直达、存档管理全在游玩页抽屉；新建游戏 = 选书+起名确认后开档；继续游玩对需升级存档直达（升级提示归游玩页横幅）。原型见 [prototypes/list-page.html](./prototypes/list-page.html)。
- [AI Provider 配置与凭据管理](./issues/26-ai-provider-config.md) — 用户侧配置（provider/base_url/model/参数）+ 密钥存本地配置文件（0600）+ 主线/角色/Embedding 三类模型各自可配 + 失败降级与成本护栏。
- [存储架构改向（应用级单库 + 独立向量库）](./issues/27-storage-single-db.md) — 推翻「一存档一 .sqlite」：应用级单库 SQLite（按 save_id 分区）+ 独立 DuckDB 向量库（派生可重建）；关键词检索 SQLite FTS5；导出=抽单存档为独立包、导入分配新 save_id；新原点旧日志进库内归档表。

## Not yet specified

<!-- 雾区：尚无法精确提问的待决项，随前沿推进逐个毕业成票 -->

- 遗忘兜底（置顶事实清单）：#16 挂起——若实测检索窗口外矛盾频发再立项
- 演出模板导出 / 导入（zip 包，跨机器迁移自定义模板）：#19 留口 v1 不做，待 v1 后立项
- 回滚 / 分支的玩家侧时间轴 UI：#06 原归「游玩界面设计」、#08 未落地；#21 确认 v1 仅 dev 工具、不进抽屉 UI，玩家侧呈现待后续立项
- 演出模板插件 API（h() 渲染树 / 生命周期 / DOM 逃生舱）：#19 降级 v1.1，v1 只做 CSS token 换肤

## Out of scope

<!-- v1 之外，本次地图不覆盖 -->

- 多人联机的网络协议、房间、状态同步（v1 仅架构预留，具体设计另起地图）
- 云端部署 / 多租户
- AI 生图、语音合成
- 真人 GM 旁观 / 共同主持
- 插件生态化 / 社区分发（需沙箱；v1 仅本地加载、信任本地用户，见 #19）
- 叙事文本机检防幻觉（实体校验器 / 软门自证 / 冲突裁决卡）——v1 判定为模型 + prompt 问题，单条幻觉玩家自纠（见 #16）
- 无障碍 / 移动端适配——v1 桌面优先，明确不做（设计复审 2026-09-09）

## 修订（设计复审 2026-09-09）

全量通读后与作者逐条确认 12 项，决议与影响见 [decision-log-design-pass.md](./decision-log-design-pass.md)：
叙事权威化 + 历史端点（#04/#06/#17/#18/#24）、存储改向单库（#27，改 #05/#06/#14/#20/#21/#24）、新原点玩家侧 + 库内归档（#14/#21/#24）、缺失端点补进 #24、新开 #26、回合并发与幂等（#24）、objects 实体（#01/#04）、元指令通路（#04/#24）、PC/NPC 与开档选主角（#01/#03/#24/#25）、声明区与语义补漏（#01/#04/#12/#13/#17）、命名对齐与 ts-rs 单一来源（#20）、插件 API 降级 v1.1（#19）。