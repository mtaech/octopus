# 骨架语义（goal / beat 触发与场景推进）

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 04

## Question

定义骨架中 goal 与 beat 的语义，以及主线 AI 如何在回合内引用骨架推进场景。需要拍板：
① goal 是什么：场景/章节的完成判据，如何驱动主线 AI 推进。
② beat 是什么：关键事件——触发条件、一次性 vs 可重复、触发后给主线 AI 的提示。
③ 章节/场景的进入与结束条件（何时切换 scene）。
④ 主线 AI 通过哪些「场景推进类」意图引用骨架（依赖「AI→引擎动作协议」先定下意图集）。

## Answer

**驱动模型（根）**：引擎权威 + AI 叙事。goal/beat 均为声明式条件，引擎在回合末求值（#04 挂载点 7 / #02 条件评估时机）；引擎只产出「已达成/已触发」的结构化事实并注入提示，**不自动推进**；主线 AI 在叙事上收束后通过「场景推进意图」提出切换，引擎校验后执行。

### ① goal（目标）语义

- goal = 场景/章节的完成判据，一条声明式布尔条件（见 ④ 条件 schema）。
- 数量：scene 一主多副（主 goal 完成即收束；副 goal = 支线完成度、不强制收束）；chapter 级 goal = 其下各 scene 主 goal 的聚合。
- 完成后：引擎标记「已达成」（入命令日志、成为可引用状态事实）+ 注入主线 AI 提示「本场景目标已达成，可推进」；**不自动切场景**。
- `hidden` 字段：判据对玩家隐藏、只对 AI 可见。

### ② beat（节拍）语义

- beat = 关键事件节点：声明式触发条件 + 提示文本/内容引用。
- 触发：引擎回合末求值；满足则标记「已触发」；`repeatable` 默认 false（一次性），true 则每次满足都重新提示。
- 消费：beat **不改世界状态**（机制交给技能/效果/Lua）；触发后引擎注入主线 AI 提示，由主线 AI 即兴演绎（不强制演出）；给玩家的呈现由主线 AI 转述，引擎不直接广播系统提示。

### ③ 场景/章节切换

- 进入：scene 默认顺序链（chapters → scenes 顺序，首场景入口）；主线 AI 可显式指定目标 scene，引擎校验目标 scene 进入前置条件（允许跨 chapter）。
- 切走判据：当前 scene 主 goal 已达成（且主线 AI 提出切换意图）→ 引擎校验后执行；允许「goal 未完成就离开」（失败/放弃分支），引擎放行并把该 scene 记为「未完成」。
- 切走时引擎动作：广播 `scene_change` 事件（→ Lua 事件钩子、status 按 scenes 单位到期、触发场景上下文压缩）、按新 scene 重算角色实例「在场集合」、清空回合内暂态。
- chapter 切换 = scene 链耗尽自动，或主线 AI 显式跨 chapter（同校验）。

### ④ 条件 schema 与场景推进意图

- **条件表达式** = 结构化 JSON 表达式树 + Lua 兜底（与 #12「声明式核心 + Lua 钩子」同构）。
  - 复合：`all_of` / `any_of` / `not`。
  - 原子谓词（首版最小集）：`beat_fired(beat_id)`、`flag_set(flag)`、`at_location(location_id)`、`attribute_ge(attribute, value)`、`relationship_ge(from, to, type, value)`。
  - 兜底：`lua(script_id)` —— 走「条件评估」挂载点，返回布尔。
  - 编辑器：结构化部分可视化/校验；`lua` 兜底标记「自定义条件」、不可视化。
- **场景推进意图** = 新增第 6 类 **Progression（骨架推进）**，只含一个 `advance_scene { target_scene_id?, abandon? }`：
  - `target_scene_id` 缺省 → 顺序链下一 scene；显式 → 校验目标 scene 进入前置（可跨 chapter）。
  - `abandon: true` → 主 goal 未完成也离开（失败/放弃分支），引擎放行并标记未完成；缺省 false → 校验主 goal 已达成，否则驳回。
  - **不设** `advance_chapter`（chapter 切换由顺序链耗尽或 advance_scene 跨 chapter 达成）。
  - **不设** `trigger_beat` / `complete_goal`（beat 触发、goal 完成只能引擎求值产生，主线 AI 不「宣告」——堵死谎报进度，呼应 #16）。
  - 骨架查询不新增 `query_skeleton`（主线 AI 用 `query_world` 全 scope 过滤）。

### 衍生事实

- goal/beat 的「已达成/已触发」提示只注入主线 AI，角色 AI 不接收（#05 所见即所知 + #04 scope 受限）。
- 术语：goal = 目标、beat = 节拍，已入 CONTEXT.md。

---

## 修订（设计复审 2026-09-09）

- 引擎内部事件 `scene_change` 与 #17 演出流事件 `scene` **统一命名**。
- 条件谓词引用的 flag / event 走 #01 声明区校验。
- `advance_scene` 已补入 #04 第六类意图（Progression）。

详见 [决策记录](../decision-log-design-pass.md)。
