# 开放内容元模型（探针）

> 触发：一个故事要定义的东西太多，不能每加一个概念就加一套数据类型。
> 结论先行：**词汇开放、语义封闭**——故事书可以定义任意「种类」，但不能定义新「机制」。
> 本文记录一个最小探针：验证「新概念 = 数据、零新增代码」是否成立。

## 问题（开发侧）

现状把「故事内容的概念」和「引擎必须结算的机制」混在一个封闭类型系统里：

- **机制类型很小且已通用**：`CondExpr` / `ImmediateEffect` / `EffectDef` / `CheckerDef` /
  `ResourceCost` / `Cooldown` / `DeltaDomain`。
- **爆炸的是容器类型**：`SkillDef` / `ItemDef` / `ObjectDef` / `StatusDef` / `CharacterDef` /
  `FactionDef` / `RelationshipDef` / `LocationDef` / `GoalDef` / `TriggerDef` /
  `SceneDef` / `ChapterDef` ≈ 14 个，外加 4 个声明区。
- 一个 kind 要在约 **23 个文件**里登记：`EntityPicker.vue` 的 `KIND_LABEL`/`GROUP_ORDER`、
  workspace-a 面板、`entity-refs.ts` 的 `kindList`、`pair-context.ts` 的上下文键、
  `lua-context.ts`、editor store 的 `normalizeKind`/`withDefaults`/`cascadeCleanup`、
  `WorkspaceA.vue` 的 tab、后端 `validate.rs` 的引用校验……

这是用封闭类型去追开放概念空间，必然追不上。

## 原则

**词汇开放，语义封闭。**

- 开放：`KindDef`（种类）与 `Definition`（实例）由故事书数据自带，代码不枚举。
- 封闭：引擎能求值 / 改写的只有那几个固定原语；真正的新机制走原语扩展或 Lua 兜底，
  **不允许用数据声明新的求值语义**（否则会滑成一门没人维护的配置语言）。

决策规则：一个概念只有在「引擎必须求值或改写它」时才升格为封闭核心；
当前真正合格的只有属性维度值、资源、flag、地点、关系边、trigger fired / goal 条件。

## 形状

```ts
KindDef  { key, label, group?, singleton?, fields: FieldDef[] }   // 种类 = 数据
FieldDef { key, label, type: FieldType, options?, ref_kind?, ... } // 字段 schema

Definition { id, kind, name, description?, fields: Record<string, unknown>, mechanics? }
DefinitionMechanics { check?, cost?, cooldown?, target?, effect? }  // 既有原语的固定形状
```

- `Storybook` 增开 `kinds?: KindDef[]` 与 `definitions?: Definition[]`，与现有集合**并存**（渐进迁移）。
- 引擎结算不看 `kind`，看 `mechanics` 是否存在（duck typing）；机制仍是强类型原语。

## 探针做了什么

在真实代码上做一条最小、加性的纵切，新增两个**代码里从未存在**的种类
`rumor`（传闻）/ `prophecy`（预言）：

| 文件 | 改动 |
|---|---|
| `frontend/src/types/index.ts` | 新增 `KindDef` / `FieldDef` / `Definition` / `DefinitionMechanics`；`Storybook.kinds/definitions` |
| `frontend/src/pages/editor/components/workspace-a/DynamicForm.vue` | 新增：按 `FieldDef[]` 渲染表单（text/textarea/number/boolean/enum/ref/ref_list/asset） |
| `frontend/src/pages/editor/components/workspace-a/OpenKindsPanel.vue` | 新增：一个面板，分「内容 / 种类」两态 |
| `frontend/src/pages/editor/components/workspace-a/KindEditor.vue` | 新增：种类（KindDef）本身的字段 schema 编辑器（key / 显示名 / 分组 / 单例 + 字段列表） |
| `frontend/src/pages/editor/components/workspace-a/WorkspaceA.vue` | 注册一个「开放内容」tab（**一次性**） |
| `frontend/src/lib/entity-refs.ts` | `kindList` 兜底查 `definitions`；`listEntityRefs` 通用枚举 |
| `frontend/src/lib/pair-context.ts` | 结对上下文带上 `kinds` + `definitions` |
| `frontend/src/api/seed.ts` | 种子故事书加 `kinds` + `definitions`（纯数据） |

结果：两个新种类自动出现在 A 工作台（含 ref 到 character/location 的下拉）、
引用选择器、结对 AI 上下文里——**没有为任一 kind 写新面板 / 新校验 / 新上下文键**。

关键闭环：种类**本身**也能在 UI 里新建——「开放内容」tab 的「种类」页可以新建 KindDef、
增删字段、声明字段类型（含 ref 的目标种类）；新建的故事书（无任何 kind）也能从零建起。
所以「新概念 = 数据」在编辑器侧已完整成立，不必再碰种子数据或代码。

验证：`pnpm -C frontend typecheck` 与 `pnpm -C frontend build` 均通过。

## 尚未做（后续纵切）

- 引擎侧：`resolve` 走通用 `Definition.mechanics` 结算（`UseSkill` 等意图按 duck typing 解析）；
  `validate.rs` 按 `KindDef.fields` 做通用存在性 / 引用校验。
- `EntityPicker` / `REF_KIND_LABEL` / editor store 的 `normalizeKind`/`withDefaults` 从 kinds 派生。
- `lua-context.ts` 的补全集合从 kinds 派生。
- `pair-tools.ts` / `applySuggestionTo` 支持 `definition` 的 upsert。
- `upcast` v3→v4：把 `skills/items/objects/factions/statuses` 迁成内置 kinds 的 `definitions`。
- 迁移时需定「kind schema 版本」策略（现有 `schema_version` 只管格式）。

## 待拍板

1. 是否认这条路线：开放词汇 + 封闭机制。
2. 内置 kind（character/world/scene）保留手工面板作为例外，还是也 schema 化？
3. 通用表单是否够用（探针手感）——还是需要 per-kind 的 layout/自定义组件挂钩。
