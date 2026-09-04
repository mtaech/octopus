# 前端应用结构与模块划分

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 08, 17

## Question

定义游玩界面前端（Vue3）的应用结构与模块划分。游玩界面已定（#08），事件协议待定（#17），需要拍板：
① 模块边界：流式叙事渲染器、输入 / 元指令处理、演员卡与状态展示、确认门组件、地图 / 日志抽屉如何划分子模块。
② 状态管理：回合状态、受控角色、事件流缓冲的组织方式（组件本地状态 vs 共享 store）。
③ 通信层：SSE 客户端封装（重连、事件缓冲、心跳），与「引擎→UI 事件协议」的对接。
④ 页面路由：游玩页 / 故事书编辑器 / 故事书列表的进入流程与页面划分。
## Answer

**决议（三轮 grilling 全部落定）：**

### ① 模块边界 = 混合分层（C 方案）

- **层为骨架、UI 按区域切片**：`communication/`（stream + api 两半）与 `store/`（Pinia，按 feature 一 store 一文件）为独立层，UI 组件单向依赖；UI 侧按区域切成 feature 模块——`narrative/`（三演出模板 + 打字机 + 确认卡挂载点）、`controls/`（输入框 + 元指令 + 快捷按钮）、`status/`（演员卡）、`drawers/`（地图 / 日志抽屉）。
- **模板 = narrative 下三个渲染器实现**：共用同一 store 与同一演出流；切换只换活动渲染器，不重置会话状态、不重连流。
- **确认门 = 独立组件**（挂入叙事流而非叙事渲染器内部）：免确认模式下不挂载；涉及 HTTP 往返，职责独立于纯呈现。

### ② 状态管理 = 全量共享 store + 当前回合有界缓冲

- **Pinia 全量 store**：回合 / 流状态（当前 round、phase、seq 水位线）+ 世界状态投影（按 domain/entity_id 物化的 `state_changes` delta 投影，engine-projection store，组件只消费派生数据）+ 受控角色 + 确认门 pending。三模板 + 演员卡 + 抽屉共享同一份投影；模板切换 / 抽屉开关不触发重新拉取。
- **演出流缓冲 = 当前回合有界 ring buffer**（`round_start` 清空、回合结束即弃）：模板切换时重绘进行中的回合；不做全量事件历史。
- **模板选择 = 纯前端本地 + localStorage 持久化偏好**（#08：切换不产生任何事件）；受控角色进共享 store（输入框语义归属 + 演员卡高亮来源）。

### ③ 通信层 = 薄封装 EventSource + 重连水合 + keep-alive 心跳

- **`communication/stream`**：单例长连接（游玩页生命周期，模板切换不重连）；EventSource 原生自动重连，封装只补 seq 去重 + 重连后触发 store 重水合钩子。
- **重连语义**：重连即 GET /state 重水合、丢弃 `seq ≤ watermark` 迟到事件；不用 Last-Event-ID（事件 id 为 uuid 非单调 seq，#17 不回放）；断连期间不另做积压缓冲。
- **心跳**：服务端 ~15s SSE 注释行（`:keepalive`，不入 schema、不产生事件）+ 客户端空闲检测（~30s 无任何数据判定僵死、主动关闭触发重连）——兜住 EventSource 管不到的半开连接。
- **`communication/api`**（与 stream 并列同模块）：GET /state 与确认 POST；确认门时序由 store 编排——`pending` → UI 挂起卡 → confirm / cancel → api POST → 监听流上 `resolution` 收尾；`409 expired` 由 api 捕获、转 store 一次性提示，不打断回合。

### ④ 页面路由 = 三路由 + 存档入游玩页

- `/` 故事书列表（含「继续游玩」最近存档直达）、`/storybook/:id/edit` 编辑器（#07）、`/play/:saveId` 游玩页。
- **不设独立存档区**：存档列表 / 详情 / 迁移 / 新原点全部为游玩页内抽屉（存读档菜单）；#21 的呈现与交互随之落在游玩页抽屉内细化。
- 编辑器内部模块结构不在本票范围（本票 scope = 游玩页前端 + 全应用路由），已按同一约定另立「编辑器前端模块结构」票。

### 解锁

本票决议解锁「存档管理界面设计」（#21，Blocked by 18）；新增「编辑器前端模块结构」票（沿用本票约定）。

