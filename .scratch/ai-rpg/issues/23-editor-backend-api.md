# 编辑器后端 API 设计

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 01, 07, 14, 22

## Question

定义故事书编辑器后端 API 的形态（#07「待办交圈」落地）。前端契约已定（#22：editor store 草稿唯一主人 + debounced 草稿自动保存、显式发布才 bump revision 并触发 #14 迁移流；引用图 + 实时校验 dock 依赖引用校验接口；A/C 双范式 + B 文档组件复用同一草稿），需要拍板：

① 端点划分：故事书 CRUD、草稿自动保存、引用校验、发布各自的 REST 形态与载荷；草稿（不 bump revision）与发布（revision+1）如何分离——同一资源两态（draft/released 同表分列）还是分离资源；「新建故事书」与「新建即发布」的语义。

② 校验语义：引用校验的校验范围与实时性（保存时服务端校验 vs 独立校验端点）；与 #01 数据模型、#04 引擎校验的关系（引擎侧校验是运行期，编辑器校验是编辑期，两者的重叠与分工）。

③ 并发与一致性：同一故事书多开 / 多会话编辑的冲突处理（v1 单用户但可能多 tab）；草稿自动保存的幂等与版本竞争（基于哪一版 revision 起的编辑，保存时如何检测他人/他 tab 的变更）。

④ 结对会话端点：AI 结对对话 / 流式的后端形态——复用 #10 的 AiProvider 抽象与 #11 的 SSE 单向流（与游玩页演出流的区别：结对 = 每会话请求-响应 / 短流，非长连接事件流）；结对建议采纳前后的后端职责（会话历史持久化与否）。

## Answer

**决议（三轮 grilling，Q1–Q13 全部按推荐落定）：**

### ① 资源形态与端点划分

**单资源两态（同表分列）**：故事书 = 一个资源，存储上一行两列 JSON——`released_json`（当前已发布版次，存档嵌入的冻结模板，#14）与 `draft_json`（编辑中工作副本）+ `revision`（版次）+ `draft_version`（草稿并发版本，见 ③）。发布 = 单行事务：`released := draft; revision+1; draft_version+1`——草稿恒为「下一版次的工作副本」，发布后自然等于刚发布内容，并发编辑器经 draft_version 感知。

**新建语义**：新建 = 仅草稿（revision 0、released 空）；「发布」始终显式、是唯一 bump revision 的入口；不提供「新建即发布」。存档只能基于已发布的版次，未发布的故事书在游玩侧（列表/开档）不可用。

**端点面**（`octopus-api` crate 装配，#20）：

| 端点 | 语义 |
|------|------|
| `GET /api/storybooks` | 列表：`{ id, title, revision, draft_version, updated_at, released_at? }` |
| `POST /api/storybooks` | 新建草稿（revision 0、released 空） |
| `GET /api/storybooks/:id` | 完整读取：`draft_json` + released 元信息 + `revision` + `draft_version` |
| `PUT /api/storybooks/:id` | 整稿保存草稿：body = `{ draft_json, base_version }`；允许不完整草稿落盘，响应附校验问题列表；base 不匹配 → 409 |
| `POST /api/storybooks/:id/publish` | 发布门：校验通过 → 按 ① 原子推进；校验失败 → 422 带问题列表；base 不匹配 → 409 |
| `DELETE /api/storybooks/:id` | 删除（不检查 base，v1 单用户）；不影响已引用它的存档（内嵌冻结模板） |
| `POST /api/validate` | 无状态校验：body = 故事书 JSON → `{ issues }`，不挂在具体书 id 下（未落盘草稿也能校验） |
| `POST /api/storybooks/:id/pair` | 结对短流（SSE，见 ④） |

**错误信封**：统一 JSON `{ code, message, detail? }`；409 `detail = { current_draft_version, updated_at }`；422 `detail = { issues }`。

### ② 校验语义

- **实时性**：独立无状态校验端点（编辑器 debounced 调用、只查不改，供引用图/校验 dock 实时展示）+ 保存/发布时服务端校验兜底（PUT 附问题列表不挡落盘，publish 强制通过才 bump revision）。
- **校验范围**：① schema 一致性（#01 `schema_version` + #14 编辑器/引擎共用迁移链）② 引用完整性（id 存在、无悬挂引用、被删维度仍被引用）。Lua 钩子静态预检（语法 + scoped env 白名单，#02）也放编辑期。
- **校验器归属**：`octopus-engine` 纯函数 `validate_storybook(&Storybook) -> Vec<Issue>`，api 层包装成端点。与 #04 引擎运行期校验（六类驳回码）分工 = 编辑期管静态结构/引用完整性，运行期管意图/状态动态合法性，不重叠、共享同一 JSON schema。
- **结果结构**：`{ issues: [{ severity: error|warning, code, target (实体 kind+id / JSON 路径), message, related_refs (被引用清单) }] }`；error 阻止发布，warning 仅提示。
- **引用图数据源**：dock 引用图维持客户端投影（#22 已锁，editor store 引用索引）；服务端 `related_refs` 为权威被引用清单，保存/发布时兜底，不一致以服务端为准并提示。

### ③ 并发与一致性

- **乐观并发**：`draft_version` 单调 +1（只增不减，与版次 revision 不混用）；PUT 与 publish 均携带 `base_version`，不匹配 → 409。
- **冲突裁决**：前端弹「加载服务端版 / 保留我的强制覆盖」；强制覆盖 = 客户端取服务端当前 `draft_version` 作 base 重发 PUT，**服务端无 force 标志**（保持无特权路径）。
- **自动保存幂等（内容级）**：请求 `draft_json` 与当前草稿完全相同 → 200 no-op（不 bump draft_version）；不同且 base 过期 → 409。天然覆盖超时重试，不推高版本、不产生假冲突。
- **发布并发**：发布 = 「发布我所见的版本」，并发发布 409 提示先看新版次。

### ④ 结对会话端点

- **形态**：无状态请求-响应短流——`POST /api/storybooks/:id/pair`，body = `{ messages, storybook }`（storybook = 编辑中最新草稿全量）；服务端**不持久化会话**（历史在 pair store，换 tab/重启丢失可接受）；消息历史窗口裁剪为客户端职责。
- **流式**：SSE 短流事件 `text_delta` / `suggestion` / `done`；v1 前端可退化单块返回，接口留流式位（#22）。
- **AI 通路**：复用 AiProvider 走 **chat 适配**（不经引擎回合管线、无确认门/结算/工具循环）；**不接** #05 记忆管线（结对是创作对话，上下文 = 当前草稿 + 消息历史，服务端统一渲染）。
- **建议载荷与采纳闭环**：`{ id, action: create|update|delete, target: { kind, id? }, patch }`（patch 与 #01 实体形状同构，直接映射 #22 editor store 建/改 action）；生成时**不校验**（待审查语义），采纳 → editor store action 落稿 → 校验端点，失败则拒绝采纳并提示原因。

### 解锁 / 新增

- 术语落账 CONTEXT.md：新增**发布 (Publish)**（把当前草稿定型为新的已发布版次、唯一 bump 版次的入口；存档只能基于已发布版次；发布后草稿成为下一版次基座）。
- 解锁「游玩页后端 API 设计」（#03/#06/#17/#20 已定机制、端点面未定），另立新票。
- 雾区新增：列表页（故事书 / 存档入口，路由已定 #18）UI 规格，数据源随新票落定后立项。

