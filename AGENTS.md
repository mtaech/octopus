# AGENTS.md — Octopus

通用 AI RPG 软件：Rust 引擎 + Vue 3 编辑器/游玩页。玩家用故事书自定义世界与规则，AI 演绎剧情。

- 领域词汇表（术语的唯一权威）：[CONTEXT.md](./CONTEXT.md)
- 具体规则集术语对照（D&D 等）：[docs/terminology-disambiguation.md](./docs/terminology-disambiguation.md)
- 会话持久化模型：[docs/session-persistence.md](./docs/session-persistence.md)

常用命令：`pnpm -C frontend build`（前端，含 vue-tsc）、`cargo test -p octopus-engine`（引擎）、`./start.sh all`（起服务）。

## UI 硬规则：可编辑的东西必须有可见提示

**任何可编辑控件都不得伪装成静态文本。** 这是被反复要求的规则，不是风格偏好。

禁止：

- 用 `border-0` / `border-transparent` / `bg-transparent` 把输入框做成一行死字（角色名、属性数字、章节/文档/分区标题都踩过）
- 只有聚焦（`focus:*`）才显形、静止态和悬停态都没线索
- 拿 `placeholder`（仅空态显示）或 `title` 工具提示充当唯一提示

要求：**静止态**就能看出可编辑，至少满足其一：

- 虚线底边：`border-b border-dashed border-border/70`
- 铅笔图标：`<IconPencil class="size-3.5 text-muted-foreground/60" />`（重要字段可配「标题」类小标签）
- 悬停反馈：`hover:border-solid hover:border-primary/50 hover:bg-muted/40`
- 聚焦反馈：`focus-visible:border-solid focus-visible:border-primary focus-visible:ring-2 focus-visible:ring-primary/20`
- 重要的展示型字段（大标题 / 大数字）加 `title="点击…编辑"`

例外：输入区**本身有明显容器**时可以内部无边框——例如聊天输入栏（带边框的容器 + 发送按钮 + placeholder）、对话框里的普通表单。判断标准是「用户看一眼就知道这是输入的地方」。

自查方法：改完 UI，在**不悬停、不聚焦**的状态下截图，逐个问「这看起来能点进去改吗？」——看不出就是不合格。
