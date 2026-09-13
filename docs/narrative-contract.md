# 叙事契约与提示词装配（Octopus 原生设计）

> 状态：**P0 已实现**（`storybook.narrative.sections` + 槽位装配 + 校验），P1–P3 未实现。四项未决已定：**协议可覆盖（含 Lua 自定义，须过格式校验）**、
> **scope 可细到单个角色**、**变体选择存存档级**、**内置默认段极简**。

## 1. 立场：从 Octopus 的既有约束反推

| Octopus 既有事实 | 对设计的要求 |
|---|---|
**故事书是单一事实来源**，随版次发布、随存档冻结 | 「怎么讲」属于**故事书**，不是应用级全局预设库 |
**AI 只输出意图，引擎校验→结算** | 无论协议怎么自定义，出口必须是 `Vec<Intent>` |
**命令日志权威、append-only、可重放** | 装配是**纯函数**；展示变换不改写日志 |
**Lua 沙箱已存在**（`lua_host` + `SandboxLimits` + `lua_lint`），判定器已支持 Lua 归一化输出 | 协议自定义**直接复用**同一沙箱与同一「归一化输出」范式 |
**条件系统 (CondExpr)** 已用于 goal / trigger / lore | 叙述段可带条件 |
**编辑范式 A/B/C**，AI 改动只是待审查提案 | ST 导入走编辑器提案 |
**升格 + schema_version** | 新字段走版本链 |

## 2. 概念

| 术语 | 定义 | 归属 |
|---|---|---|
**叙事契约 (Narrative Contract)** | 故事书声明的「怎么讲」：叙述段 + 协议 | 故事书（随版次发布/冻结） |
**叙述段 (Narrative Section)** | 具名文本，声明槽位、作用对象、可否被玩家调整 | `storybook.narrative.sections[]` |
**槽位 (Slot)** | 装配管线里由**引擎固定**的插入位置 | 引擎 |
**变体组 (Variant Group)** | 互斥选项（人称/节奏/篇幅）；**存档选择 > 故事书默认** | 故事书 + 存档 |
**协议 (Protocol)** | AI 输出的格式约定；默认引擎提供，**故事书可覆盖** | `storybook.narrative.protocol` |
**协议插件 (Protocol Plugin)** | 用 Lua 实现的协议适配器（生成协议说明 + 解析原始输出） | 故事书 + Lua 沙箱 |
**玩家偏好 (Narrative Override)** | 玩家对 `playerEditable` 段的开关/变体选择 | 存档设置 |

## 3. 不照搬 ST 的四件事

| ST | 为什么不要 | Octopus 替代 |
|---|---|---|
用户自定义整个 `prompt_order` | 会与分层优先级、协议打架 | **槽位固定顺序**，段只能填槽位 |
正则后处理改文本 | 我们是结构化输出；「该隐藏的」不该是叙述意图 | **不渲染的意图**（`think`） |
全局预设库 + 角色卡覆盖 | 与「故事书单一事实来源、随存档冻结」冲突 | **叙事契约进故事书** |
`identifier` 槽位名（`jailbreak`/`nsfw`…） | 名实不符、语义模糊 | 按用途命名；内容中性 |

## 4. 数据模型

### 4.1 故事书 `storybook.narrative`

```ts
interface NarrativeContract {
  sections: NarrativeSection[]
  protocol?: ProtocolConfig        // 缺省 = 引擎默认协议
}

interface NarrativeSection {
  id: string
  title: string
  slot: 'world' | 'style' | 'behavior' | 'closing'
  /** 作用对象：主线 / 角色 / 两者 / 指定角色实例模板 */
  scope: 'story' | 'character' | 'both' | { characterId: string }
  text?: string
  variants?: { key: string; label: string; text: string }[]
  defaultVariant?: string
  when?: CondExpr                 // 复用现有条件系统
  playerEditable?: boolean
  enabled?: boolean
}
```

### 4.2 元信息：内容评级

```ts
storybook.meta.rating?: 'sfw' | 'nsfw'   // 创作者自报；仅用于列表标识与筛选
```

### 4.3 存档：玩家偏好（变体选择存这里）

```ts
narrative?: Record<string, boolean | string>   // sectionId -> 开关 | 变体 key
```

## 5. 协议接口 (Protocol Interface)

这是本轮决策的核心：**协议允许故事书覆盖，但必须落到一个固定接口上；接口之外的自定义一律在校验期挡掉。**

### 5.1 故事书声明

```ts
interface ProtocolConfig {
  mode: 'declarative' | 'lua'
  /** declarative：声明使用哪些意图（须是引擎已知集合的子集）+ 追加说明 */
  intents?: string[]
  instructions?: string
  /** lua：协议插件源码 */
  lua?: string
}
```

### 5.2 引擎侧：`ProtocolAdapter` 端口（与 `AiProvider` 同构）

| 模式 | 生成协议说明 | 解析原始输出 | 说明 |
|---|---|---|---|
**default**（缺省） | 引擎内置文本 | `parse_intents` | 现状不变 |
**declarative** | 引擎按 `intents` 渲染模板 + `instructions` | `parse_intents` + **意图白名单** | 覆盖解释、不覆盖解析 |
**lua** | 插件 `preamble(ctx)` | 插件 `parse(raw)` | 完全自定义格式，但出口必须归一化 |

**引擎保证**：三种模式出口都是 `Vec<Intent>`；结算与状态写入完全不感知协议模式。

### 5.3 Lua 协议插件接口（固定，与判定器 Lua 同范式）

```lua
-- 必选：注入系统层的协议说明
function protocol.preamble(ctx) -> string end
-- 必选：把模型原始输出解析为意图数组（引擎唯一入口）
function protocol.parse(raw) -> { intents = { ... }, error = nil|string } end
-- 可选：解析后归一化（补 actor_id、纠正字段）
function protocol.normalize(intents, ctx) -> intents end
```

- `ctx` 为只读快照（场景 / 在场角色 / 受控角色…），与 Lua 判定器同源。
- 走既有 `SandboxLimits`（指令预算 / 内存 / 禁用 API），协议插件单独设预算上限。
- `parse` 抛错或返回非法形状 → 按现有「意图解析失败」路径处理（`EngineError::Ai`），不 panic。

### 5.4 格式校验（发布门，`validate.rs`）

| # | 规则 | 级别 |
|---|---|---|
| 1 | `lua_lint` 静态检查（语法 + 禁用 API） | Error |
| 2 | 必须定义 `protocol.preamble` 与 `protocol.parse` | Error |
| 3 | **一致性样例跑**（conformance）：给 `parse` 喂固定样例 —— 合法数组 / 围栏+废话 / 垃圾输入，要求分别「产出可反序列化的意图」「能提取」「返回明确错误而非超时」 | Error |
| 4 | declarative：`intents` ⊆ 引擎已知意图集合 | Error |
| 5 | declarative：至少声明一个叙事意图（`narrate`/`speak`/`emote` 之一），否则回合会空转 | Error |
| 6 | `instructions` 长度与令牌预算上限 | Warning |
| 7 | Lua 未用到的引擎意图（如完全禁用了 `strike`）导致战斗无法结算 | Warning |

任何 Error 阻止**发布**；草稿可存。

## 6. 装配管线（引擎拥有槽位顺序）

```
① protocol   默认文本 / declarative 渲染 / Lua preamble —— 可覆盖，须过 §5.4
② world      世界前提 + 叙述段(world) + 命中词条(lore)
③ persona    在场人物档案
④ scene      当前场景 + 任务 / 遭遇 / 已裁定事实
⑤ directives 叙述段(style) + 叙述段(behavior，按 scope 过滤)
⑥ focus      玩家显式引用的实体
⑦ input      玩家输入
⑧ closing    叙述段(closing)
```

- **纯函数**：输入 = 故事书冻结副本 + 当回合世界状态 + 玩家输入 + 存档偏好。
- **重放安全**：AI 输出不参与状态重放；契约变化只影响之后的回合。
- **预算**：段计入 `turn_token_budget`；裁剪顺序 `closing → style → behavior → world`；`protocol` 永不裁。
- **scope 解析**：`{ characterId }` 只在扮演该角色时注入；校验拦截悬空 characterId（复用既有引用校验）。

## 7. 「思考」用协议解决，不用正则

provider 原生 `reasoning_content` 已是 `PlayEvent::Reasoning`（前端折叠）。模型**写在回复里**的思考同样不该靠正则：
新增意图 `think { content }` —— 引擎记一条**非叙事**事件（与 `reasoning` 共用展示槽位），不进 `narrate/dialogue/emote`。
故事书可声明 `display.draft: 'folded' | 'hidden'`，默认 `folded`。

## 8. SFW / NSFW：同一机制，不同数据

| 关注点 | 设计 |
|---|---|
代码分支 | **没有** `if nsfw` |
载体 | NSFW 就是另一些叙述段 / 协议说明，写在故事书里，随版次发布、随存档冻结 |
默认 | 内置种子故事书 `rating: sfw`；应用不内置任何 NSFW 文本 |
评级 | `meta.rating` 只驱动徽标 / 筛选，**不做过滤、不做年龄验证** |
导入 | ST 的 `jailbreak`/`nsfw` 块按内容归入 `behavior`/`style`，转换器标注 rating |

## 9. SillyTavern 导入 = 编辑器提案

沿用 C 范式原则「AI 的改动只是待审查提案」：转换器产出 `NarrativeSection[]` 草案 → 编辑器逐段采纳 → 才写入契约。

| ST | Octopus |
|---|---|
文风 / 行为约束块 | `slot: style` / `slot: behavior` |
人称 / 节奏 / 篇幅（N 选 1） | 变体组 |
`worldInfoBefore/After`、`charDescription` 等占位 | 忽略（槽位注入） |
采样参数 | 并入已有 `RoleConfig` |
正则脚本 | **不导入**（隐藏思考走 `think` 意图） |
`jailbreak` / `nsfw` 槽位 | 归入 behavior/style，标 rating，源名只留备注 |

## 10. 校验与升格

| 环节 | 规则 |
|---|---|
校验 | 段 `id` 唯一；`slot` 合法；`text` 与 `variants` 互斥；变体 key 唯一且 `defaultVariant` 存在；`when` 走条件校验；`scope.characterId` 引用有效；**协议规则见 §5.4** |
发布 | 契约随版次发布；未发布改动不影响存档 |
升格 | `schema_version` +1：旧故事书 `narrative` 缺省为空、`protocol` 缺省为 default |
存档冻结 | 内嵌冻结故事书自带契约；开档后变化走既有「新版次待升级」 |

## 11. 编辑器落点

| 范式 | 落点 |
|---|---|
A 表单工作台 | 「叙事」tab：按槽位分组的段列表 + 段编辑 + **协议编辑**（declarative 表单 / Lua 代码编辑器，带 lint 与样例跑结果） |
C AI 结对 | AI 可提议叙述段与协议（走既有待审查提案） |
B 文档 | 叙述段按文档块渲染 |
游玩页 | 只读展示；`playerEditable` 段给开关 / 变体选择，写存档设置 |

## 12. 改动点

| 现有 | 改造 |
|---|---|
`SYSTEM_PREAMBLE` | 内置**默认协议** + 极简默认叙述段 |
`turn_prompt()` | 改为按 §6 槽位渲染 |
`TurnContext` | 新增解析后的 `narrative`（含变体选择）与 `protocol` 模式 |
`lua_host` | 新增协议插件宿主（复用沙箱与限额） |
新增 `ProtocolAdapter` 端口 | default / declarative / lua 三实现 |
`validate.rs` | §5.4 协议规则 + §10 契约规则 |
`upcast.rs` | schema_version +1 |
`PlayEvent` | 新增 `Think` |

## 13. 分期

| 阶段 | 内容 |
|---|---|
P0 | `storybook.narrative`（段 + scope）+ 槽位装配 + 极简默认段 + 校验/升格 |
P1 | `when` 条件段 + 变体组 + 存档玩家偏好 |
P2 | **协议接口**：declarative + Lua 插件 + §5.4 校验 |
P3 | `think` 意图与展示策略；ST 预设导入提案；`meta.rating` |

## 14. 仍待定

1. `think` 意图是新增独立事件，还是复用 `PlayEvent::Reasoning` 加一个来源字段？（倾向前者，语义更清）
2. 协议 Lua 插件的 `ctx` 快照要暴露多少？（倾向对齐判定器 Lua 的上下文）
3. declarative 模式下 `instructions` 是否也参与令牌预算裁剪？（倾向参与，可裁）

## 15. 应用级提示词覆盖（与叙事契约的分工）

本契约管的是**故事书声明的「怎么讲」**。引擎各处还有一批**应用固定文本**（系统提示词、每回合提示词骨架、分节引导语、记忆压缩与纠错消息、结对 AI 的系统提示词），它们不属于任何故事书，因此单独走一层**提示词覆盖**：

| 维度 | 叙事契约（本文件） | 提示词覆盖 |
|---|---|---|
| 归属 | 故事书（随版次发布、随存档冻结） | 应用配置 `config.json` |
| 粒度 | 叙述段按槽位注入 | 具名条目（key → 覆盖文本），留空 = 内置默认 |
| 变量 | 无（段就是纯文本） | 双花括号变量，单遍替换 |
| 与协议的关系 | **优先**：故事书显式声明 declarative / lua 时，系统提示词以故事书为准 | 仅在默认协议下替换系统提示词 |

实现落点：注册表 `octopus-api/src/prompts.rs`，默认文本常量集中在 `octopus-ai/src/prompt.rs`；目录由 `GET /api/prompts` 返回，覆盖存 `AppConfig.prompts`，保存走 `PUT /api/config`（即改即生效），前端设置面板「提示词」分区编辑。
