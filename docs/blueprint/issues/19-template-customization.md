# 演出模板的自定义与扩展机制

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 08, 17

> **修订（设计复审 2026-09-09）**：v1 范围收窄为 **CSS token 换肤 + 内置三模板**；插件 API（h() 渲染树 / 生命周期 / DOM 逃生舱 / manifest 渲染入口）降级 **v1.1**。详见文末修订块与 [决策记录](../decision-log-design-pass.md)。

## Question

游玩界面已是多模板（#08），需要拍板：玩家 / 创作者如何自定义、扩展演出模板。会话内已定基调（待本票决议落账）：

- 定制深度 = 双轨：**CSS token 主题层**（人人可换肤）+ **插件 API**（进阶可做新交互范式）；**不做**「模板 DSL」中间态（投入产出比最差：代价接近真插件、能力又不如）。
- CSS 层 = token 设置面板 + 可粘贴自定义 CSS 全覆盖，两者叠加；CSS 变量命名即文档。
- 插件加载 = v1 从存档 / 项目目录加载本地脚本，信任本地用户、**不沙箱**。
- 沙箱与生态 = v1 不做；「插件生态化 / 社区分享（需沙箱）」为前置条件，已进地图 Out of scope。

## 待拍板

- 插件 API 的具体接口形态：收到事件流 → 返回渲染树 vs 直接操作容器 DOM；生命周期钩子（模板挂载 / 事件进入 / 回合结束）。
- CSS 变量分类法（token 命名体系）与内置三模板如何标注「可动变量」。
- 「模板 = 主题 + 渲染器」的统一抽象：换肤与换渲染器同一概念下的文件布局 / 清单 / 切换语义。
- 插件加载 / 热更新语义：改文件即生效 / 需刷新 / 版本与冲突处理。
## Answer

**决议（两轮 grilling 全部落定）：**

### ① 统一抽象：模板 = manifest + 渲染器，主题 = 具名 token 值快照

- 演出模板 = 一个目录：manifest（id / 名称 / 版本 / 渲染入口 / 布局声明 / 声明的 token 面 + 默认 token 值）+ 渲染脚本（自定义插件）或应用内 Vue 组件注册（内置三模板）。**抽象统一在 manifest 层**——内置模板同样有 manifest（token 面 + 布局声明 + 默认值），渲染器实现可不统一。
- 主题 = 具名保存的一组 token 值，**绑定具体模板**（三范式布局差异大，不做跨模板通用槽位）。换肤 = 改 / 换 token 值（纯 CSS 变量覆盖，不重绘、不重置状态）；换渲染器 = 切换模板（应用其默认主题，重绘当前回合 ring buffer、重建插件实例，不重连 SSE）。
- 主题选择 / 自定义 token 值 / 自定义 CSS 存 localStorage（随 #18 模板偏好）；token 设置面板与自定义 CSS 叠加生效。

### ② 插件 API：h() 渲染树为主 + DOM 逃生舱

- 渲染模型 = 声明式 `h()`（类 preact hyperscript），插件每事件返回渲染树、框架 diff 落 DOM；另提供 `ref` 拿容器 DOM 作逃生舱（第三方 UI / 重动画）。
- 数据面（只读注入）：① 演出流事件订阅（按 type 过滤、带 seq）；② store 投影（世界状态 delta 投影 / 受控角色 / round / phase）；③ 当前 token 解析值（CSS 变量展开对象）；④ 渲染目标容器 + ref。v1 不注入故事书数据 / 存档元信息。
- 能力边界 = 呈现只读：插件可向输入框**预填**文本（不绕过统一输入通道）；确认门 = 框架内置组件、插件可覆盖 `pending` 呈现。
- 生命周期 = 最小集：`onMount(ctx)` / `onUnmount()` / `onEvent(evt)` / `onRoundStart()` / `onRoundEnd()`（+ 可选 `onThemeChange`）。
- 布局领地 = manifest 声明 `layout: "narrative-slot" | "full-page"`；整页插件接管整个游玩面，仍受同一流 / store / 输入约束。
- 运行时错误隔离：单事件渲染出错 → 该事件回退内置聊天流呈现 + console 报错，回合不中断、插件不卸载。

### ③ CSS token 命名体系与「可动变量」

- 语义命名 `--tpl-<组>-<槽位>`，组 = color / font / spacing / layout / motion。
- manifest 声明 token 面 = 该模板全部可动变量（名称 / 分组 / 默认值 / 中文 label / 可选枚举色板）——**声明即文档、声明即表单**（设置面板按声明渲染）。
- 内置三模板共用骨架槽位（bg / surface / text / accent / font-* / narrative-width / drawer-width / motion-*），值不同，可各自扩展私有槽位（保留前缀）。

### ④ 加载 / 热更新 / 冲突 / 存档关系

- 模板根目录 = 应用数据目录 `templates/`（全局、对所有存档生效），游玩页进入时扫描注册；改文件后手动「重新加载模板」按钮（开发期可自动监听），v1 不自动热替换。
- id 冲突 = 后加载者忽略并报错；插件不得与内置同 id（内置 id 保留前缀）；manifest `version` 仅展示、不做依赖解析。
- 模板与存档**完全解耦**（存档不记录模板依赖，呈现层非权威）；模板缺失 / 加载异常 / 单事件出错 → 回退内置聊天流，不阻塞游玩。「导出模板包（zip）」v1 不做、留口（已挂地图 Not yet specified）。

---

## 修订（设计复审 2026-09-09）

- **v1 范围收窄为 CSS token 换肤 + 内置三模板**；插件 API（h() 渲染树、生命周期、DOM 逃生舱、manifest 渲染入口）**降级 v1.1**，已移入 `map.md` 的 Not yet specified。
- manifest 在 v1 仅承载 token 面 + 布局声明，供换肤设置面板渲染。

详见 [决策记录](../decision-log-design-pass.md)。

