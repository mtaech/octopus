# P3 实现规格：think 意图、ST 预设导入、meta.rating

> 承接 `docs/narrative-contract.md` §7/§8/§9 与 §13 的 P3。
> 前置：P0（叙述段 + 槽位装配）已完成；P1（when/变体/存档偏好）、P2（协议接口）串行在前。

## A. `think` 意图（模型写在正文里的思考）

### 问题
provider 原生 `reasoning_content` 已由 `PlayEvent::Reasoning` 承载并折叠展示；但有些模型把思考**写在正文里**。
ST 用 `<draft_notes>` + 正则隐藏；Octopus 不用正则——把它变成**结构化意图**。

### 设计
1. `octopus-types`：`Intent` 新增 `Think { content: String }`（serde tag `think`）。
2. 引擎 `handle_intent`：`Think` **不改世界状态、不进叙事条目**，只落一条事件。
3. 展示：扩展 `ReasoningPayload` 加 `source: "provider" | "model"`（缺省 `provider`，兼容既有数据），
   `think` 映射为 `source: "model"`。前端沿用同一个「思考」折叠组件，无需新 UI 槽位。
   （备选：新增独立 `PlayEvent::Think`。**推荐复用**，少一个事件类型与一套 UI，且语义相同：都是「思考」。）
4. 故事书显示策略：`storybook.narrative.display?.draft: 'folded' | 'hidden'`（缺省 `folded`）。
5. 协议段文案：默认协议里补一句「需要打草稿时用 `think` 意图，引擎会折叠展示」。

### 校验
- `display.draft` 取值合法（Error）。
- 协议白名单（P2 declarative）若允许 `think`，一致即可；无额外规则。

### 测试
- `think` 意图 → 产出 `PlayEvent::Reasoning { source: "model" }`，且**不**产生 narrate/dialogue。
- `display.draft: hidden` 时前端不渲染（前端逻辑，`pnpm build` 覆盖类型）。

## B. SillyTavern 预设导入（编辑器提案）

### 原则
沿用 C 范式「AI 改动只是待审查提案」：导入**不直接改草稿**，先在编辑器里以提案列出，创作者逐段采纳。

### 转换器（新文件 `frontend/src/lib/st-preset.ts`，纯函数，仿 `st-import.ts`）
输入：ST 预设 JSON（`prompts` + `prompt_order` + 采样 + `extensions.regex_scripts`）。
输出：`StPresetProposal { sections: ProposedSection[]; dropped: DroppedItem[]; rating: 'sfw' | 'nsfw' }`，
其中 `ProposedSection = { id, title, slot, scope, text, sourceIdentifier }`。

映射规则（**语义映射，不保留 ST 的块名与顺序机制**）：

| ST | Octopus |
|---|---|
文风类块（文风 / 写作要求 / 笔触…） | `slot: 'style'` |
行为约束类块（防抢话 / 防全知 / 防媚 user / 禁八股…） | `slot: 'behavior'` |
人称 / 推进速度 / 篇幅（互斥多选） | **变体组**（P1 的能力）：按同组识别，产出 `variants` + `defaultVariant` |
`worldInfoBefore/After`、`charDescription`、`personaDescription`、`scenario`、`chatHistory` | **丢弃**（由槽位注入） |
`extensions.regex_scripts` | **丢弃**，在 `dropped` 里列出原因「Octopus 不做正则层；隐藏思考走 think 意图」 |
采样参数 | 不写入叙事契约；提示用户用现有「采样预设」 |
`jailbreak` / `nsfw` / `瑟瑟` 块 | 归入 `behavior`/`style`，并在 `rating` 上标 `nsfw` |

分类启发式（仅作**默认建议**，提案 UI 允许逐段改）：
- 含「文风/笔触/修辞/描写/句式/字数」→ style；
- 含「禁止/不要/防/必须/规则/一致性」→ behavior；
- 命中 NSFW 关键词表 → 标 `nsfw`（关键词表放 `st-preset.ts` 常量）；
- 其他 → style，并在 UI 标「需要你确认」。

### 提案 UI
- 入口：编辑器 A「叙事」tab 或 C 范式工具栏的「导入 ST 预设」。
- 对话框：逐段显示 `源名称 / 建议槽位 / 预览`，可改槽位、可勾选；顶部提示 ignored 项（占位符 / 正则）与推断出的 rating。
- 采纳：写入 `storybook.narrative.sections`（一次 undo 快照）。

### 测试
- 前端无测试基建，转换器用 `tsx` 手跑一次（仿之前 `st-import` 的验证方式）拿 `silly/TGbreak😺V3.1.2.json` 跑，
  断言：能认出文风/行为若干段、能识别一组「N 选 1」、正则与占位符进 `dropped`、NSFW 块把 rating 标成 `nsfw`。

## C. `meta.rating`

| 项 | 设计 |
|---|---|
数据 | `storybook.meta.rating?: 'sfw' | 'nsfw'`（可选，缺省 sfw） |
用途 | **仅**列表徽标与筛选；不做内容过滤、不做年龄验证 |
编辑器 | 故事书元信息里一个下拉（默认 / SFW / NSFW） |
列表页 | `StorybookCard` 角标；筛选可选（先做标，不做筛） |
导入 | ST 导入推断的 rating 作为提案默认值，创作者可改 |
代码分支 | **无**。引擎/提示词/校验都不读它 |

## D. 文件级改动清单

| 文件 | 改动 |
|---|---|
`crates/octopus-types/src/lib.rs` | `Intent::Think`；`ReasoningPayload.source` |
`crates/octopus-engine/src/session.rs` | `handle_intent` 处理 `Think` |
`crates/octopus-engine/src/validate.rs` | `display.draft` 校验 |
`crates/octopus-ai/src/rig_provider.rs` | 协议段补 `think` 说明 |
`frontend/src/types/index.ts` | `display` 策略、`meta.rating` |
`frontend/src/lib/st-preset.ts` | 转换器（新） |
`frontend/src/pages/editor/.../StPresetImportDialog.vue` | 提案 UI（新） |
`frontend/src/pages/editor/components/workspace-a/NarrativePanel.vue` | 导入入口 + `display.draft` |
`frontend/src/pages/list/components/StorybookCard.vue` | rating 角标 |

## E. 待定

1. `think` 用「复用 Reasoning + source」还是「独立 Think 事件」？（本文推荐复用）
2. ST 导入放 A 范式还是 C 范式入口？（推荐两侧都给，逻辑同）
3. rating 只做角标，还是同时做列表筛选？
