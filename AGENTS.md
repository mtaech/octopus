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

## 上下文缓存不变量（硬规则：改提示词 / 历史 / 请求组装前先读）

供应商的上下文缓存是**前缀缓存**——按请求序列的公共前缀匹配，前缀一变，后面整段按原价重算。
这条不是优化项，是钱：`octopus.db` 的 `ai_call` 事件实测，每回合从队首砍消息时命中率 **0.4%**，
纯追加时 **~97%**。所以下面几条是硬约束，改动前先确认它们还成立：

1. **系统层（preamble）必须逐回合稳定。** 逐回合会变的东西——故事书草稿快照、实体 id 目录、
   引用目标、回合序号、在场角色——只能进**用户消息**，而且宁可放最后一条。
   Lua 协议的 `protocol.preamble(ctx)` 尤其危险（ctx 里有 round / present），见 [docs/protocol-interface.md](./docs/protocol-interface.md)。
2. **会话历史只追加，不许「每回合丢最旧」。** 要减负只能走自动压缩：`ai.compaction` 到阈值才压一次、
   一次砍到低水位（DSH 同款）；压缩是**唯一**允许的前缀改写，且必须落日志（`context_compacted`）。
3. **客户端重放的字节必须一致。** 前端发来的历史要与上一轮逐字相同（附件、思考块的折叠也要可复现），
   否则从那条起全部失配；后端**只准改模型 surface**（`ai_conversations` / `pair_threads.compaction_json`），
   权威日志与展示历史（`commands` / `pair_messages`）永不改写。
4. **运行期守门已经内置**：`rig_provider::complete` 每轮比对「这一轮的请求是不是上一轮的前缀」，
   意外失配打 WARN（`缓存前缀意外失配`），压缩 / 回合重跑导致的重建打 INFO。
   对账工具：`./scripts/cache-audit.sh <save_id>`（逐轮命中率 + 「整段命中 / 从第几条起失配」）。
