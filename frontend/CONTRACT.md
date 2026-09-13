# 前端页面实现契约（subagent 必读）

> 你是 Octopus（通用 AI RPG）v1 前端原型的页面实现 agent。蓝图（设计票）与共享接缝已完成，你只实现**一页**。
> 所有契约类型与数据 API 已就绪且 typecheck 通过。**不要修改共享接缝**：src/types/index.ts、src/api/index.ts、src/api/mock/backend.ts、src/api/seed.ts、src/router.ts、src/theme/tokens.css、src/style.css、src/App.vue、src/main.ts。你的工作目录是**页面专属目录**。若确需共享物变动（缺字段/缺函数），先在最终报告里列出，由主 agent 统一改。

## 运行方式

```bash
export PATH=$HOME/.local/share/mise/installs/node/24.19.0/bin:$PATH
cd /home/huang/Personal/Dev/Code/octopus/frontend
npm run dev        # http://127.0.0.1:5173
npm run typecheck  # vue-tsc -b
npm run build
```

## 技术栈与既有资产

- Vue 3.5 + TypeScript + Vite 8 + Pinia + vue-router（已装好）。
- @ 别名 = src/。设计令牌 CSS 变量（--tpl-color-* 等）见 src/theme/tokens.css；基础控件类 .oct-btn .oct-input .oct-card .oct-badge .oct-muted .oct-faint 等见 src/style.css。**只用这些**，不要引入 UI 库。
- 页面三个 stub 已挂路由：src/pages/list/ListPage.vue、src/pages/editor/EditorPage.vue、src/pages/play/PlayPage.vue。**你负责其中一页**，风格参照同仓库 docs/blueprint/prototypes/*.html（深色、克制、信息密度高）。
- 全局 toast：import { toast } from '@/api' → toast('ok'|'info'|'warn'|'error', '文本')。
- 你的页面可自由新增 src/pages/<page>/stores/*.ts（Pinia store 按 feature 一文件）。

## 完成标准

- npm run typecheck 零错误、npm run build 通过（只跑不修接缝）。
- 页面在 dev server 可交互，主流程走通（可加少量演示友好逻辑）。
- 代码结构清晰、中文 UI 文案、随蓝图术语。
