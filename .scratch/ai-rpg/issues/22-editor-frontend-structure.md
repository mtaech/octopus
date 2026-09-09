# 编辑器前端模块结构

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 07, 18

## Question

定义故事书编辑器（#07）页面的前端模块结构与 A / C 双主范式共存形态。编辑器 UX 已定（#07：A 表单工作台 = 有计划详细编辑、C AI 结对 = 无头绪启发式编辑，两者为主范式；B 文档为叙事自由书写载体）、前端通用约定已定（#18：混合分层 + Pinia store + 三路由），需要拍板：
① A 与 C 两主范式在一页内如何共存——视图切换 / 分栏并存 / 上下文感知引导；B 文档载体在布局中的位置。
② 骨架大纲编辑器（#07 ②）与文档 / 表单 / AI 结对区的布局关系；引用图 + 实时校验面板（#07 ③⑥）在页面结构中的落点。
③ 编辑器专属通信与状态：故事书 CRUD / 引用校验 / 发布接口（#07 待办交圈）的前端封装归属；AI 结对会话若走 LLM 流式，如何复用 #18 的 communication 层约定。
## Answer

**决议（一轮 grilling，Q1–Q8 全部按推荐落定）：**

### ① A/C 双主范式共存 = 视图切换 + 轻量进入引导

- 编辑器页内范式 = **视图态**（非路由，切换不换 URL、不重载组件），一次激活一个，A 表单工作台 / C AI 结对 / B 文档三平级。
- 三者读写**同一共享草稿**（editor store，见 ③），切换零丢失、不重置编辑态。
- 进入引导：一次性「你怎么开始」轻提示（有明确目标→A、没头绪→C），可关不强制；新建故事书默认进 C（冷启动起稿）；打开已有书保持上次范式；范式偏好 localStorage 持久化（与 #18 模板偏好同机制）。

### ② B = document 组件复用 + 独立「文档」视图

- `document` 富文本组件两处用：A 表单内的叙事字段（premise / 人物背景 / 物品描述等）字段级内嵌编辑；B「文档」视图把整本故事书渲染成长文供纯书写。
- 模块上 B = 一个被复用的组件 + 一个视图容器，无独立状态。

### ③ 骨架 = A 工作台内一级 tab；引用图 + 校验 = 跨范式共享 dock

- 骨架（左场景树 + 右 goal/beat 卡片，形态沿用 #07）为 A 的一级分区（世界 / 骨架 / 人物 / …）；B/C 无骨架编辑入口（C 可建议结构，落点是 A）。
- 引用图 + 实时校验 = 右侧可折叠共享 dock（A/B/C 常在），数据源 = editor store 引用索引；形态对齐 #18 游玩页抽屉交互家族。

### ④ 状态与通信归属

- **editor store**（Pinia）：整本故事书草稿唯一主人（#01 JSON 形状）+ dirty 追踪 + 引用索引（供 dock）；debounced 自动保存草稿（不 bump revision）；「发布」才 bump revision 并触发 #14 迁移流。
- **editor/pair store**：结对会话 + 暂存建议（「待审查」不入草稿）；采纳 = 调 editor store 建/改 action 校验落稿、不留来源标记（#07⑤）；草稿是唯一持久事实，会话为临时态。
- **communication/**：编辑器 API（CRUD / 引用校验 / 发布 / 草稿保存）并入 `communication/api`；`communication/stream` 泛化为 `createStream()` 工厂——游玩页 = 单例实例（保留原重连水合语义）、结对 = 每会话一实例（轻量，无 seq/水合）；结对流式 v1 可选（大段建议文本、打字机非必需，接口留流式位）。
- 编辑器**不**接入游玩页演出流（/state、presentation SSE 只属游玩页生命周期）。

### 模块树

```
editor 页（路由 /storybook/:id/edit，单路由 + 内部视图态）
├── shell/           顶栏（范式切换器 + 发布 + dirty 态）· 进入引导 · 共享引用校验 dock
├── paradigms/
│   ├── workspace-a/  A 表单工作台：实体 tab（世界/骨架/人物/技能/物品/势力/关系/维度设置）
│   │                 骨架 tab = skeleton 组件（场景树 + goal/beat 卡片）· 叙事字段 = document 组件
│   ├── pair-c/       C AI 结对：会话流 + 建议卡 + 采纳/重写
│   └── document-b/   B 文档视图：整本故事书长文（document 组件）
├── components/
│   ├── document/     富文本编辑/渲染（字段级 + 整页复用）
│   ├── skeleton/     场景树 + goal/beat 卡片
│   └── refs/         引用图 + 校验面板（跨范式 dock）
├── stores/
│   ├── editor.ts     草稿唯一主人 + dirty + 引用索引 + 保存/发布 action
│   └── pair.ts       结对会话 + 暂存建议
└── communication/    api（CRUD/校验/发布/草稿）+ createStream 工厂
```

### 解锁 / 新增

- 前端契约已定 → 解锁「编辑器后端 API 设计」（#07 待办交圈：CRUD / 草稿保存 / 引用校验 / 发布端点），另立新票。

---

## 修订（设计复审 2026-09-09）

- `document` 组件存储格式 = **Markdown**。
- 新增 `objects` 编辑与声明区（flags/events/relationship_types/target_types）编辑与校验。
- 校验含注册表引用完整性。

详见 [决策记录](../decision-log-design-pass.md)。
