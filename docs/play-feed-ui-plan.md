# 游玩页演出流改造细案

- 依据:Impeccable critique 快照 `.impeccable/critique/2026-09-13T12-32-29Z__ontend-src-pages-play-components-feed-feedchat-vue.md`(28/40,Good)
- 用户选定方向:**先修安全与正确性** → 保留回合卡、大幅降噪 → (后续)小字体系与窄屏
- 本文档是阶段 1 / 阶段 2 的实施契约;阶段 3 只列方向,待排期

---

## 阶段 1(先做):安全与正确性

> **状态:已实施并验收**(FeedChat.vue,80 增 14 删)。`pnpm -C frontend build` 通过(含 vue-tsc);`detect.mjs` 对该文件 0 发现;浏览器实测见文末「验收记录」。

### 1.1 [P0] 「重跑本轮」两步确认

**现状**:`FeedChat.vue` L173-182 的「重跑本轮」直接 `@click="rerun"` → `store.rerunLastRound()`。后端会归档旧回合并把会话回滚到回合前(`stores/play.ts` L772-804),**不可撤销**。这是演出流唯一的不可逆破坏面,且是核心循环里的高频操作。

**改法**(全在 `frontend/src/pages/play/components/feed/FeedChat.vue`):

1. 新增与 `editingKey` 平行的确认态:
   ```ts
   const confirmingKey = ref<string | null>(null)
   let confirmTimer: ReturnType<typeof setTimeout> | null = null
   function clearConfirmTimer() { if (confirmTimer) { clearTimeout(confirmTimer); confirmTimer = null } }
   function armRerun(b: Block) {
     if (b.kind !== 'round' || store.busy) return
     cancelEdit()                       // 确认态与编辑态互斥
     confirmingKey.value = b.key
     clearConfirmTimer()
     confirmTimer = setTimeout(() => { confirmingKey.value = null; confirmTimer = null }, 3000)
   }
   function cancelRerun() { clearConfirmTimer(); confirmingKey.value = null }
   async function confirmRerun() { clearConfirmTimer(); confirmingKey.value = null; await rerun() }
   onUnmounted(clearConfirmTimer)       // 组件卸载清理定时器
   ```
2. 模板:footer 的 `v-else` 分支(L153-183)前面插入确认分支:
   ```vue
   <div v-if="confirmingKey === b.key" class="flex flex-wrap items-center justify-end gap-1.5">
     <p class="mr-auto text-[11px] text-muted-foreground">
       将丢弃本轮 {{ b.items.length }} 段演绎,且无法恢复。
     </p>
     <button … @click="cancelRerun">取消</button>
     <button … class="border-warning/60 bg-warning/15 text-warning hover:bg-warning/25"
             title="丢弃本轮演绎并用同样的输入重新生成" @click="confirmRerun">确认重跑</button>
   </div>
   ```
3. 原「重跑本轮」按钮的 `@click="rerun"` 改为 `@click="armRerun(b)"`;文案与 title 保持,二者关系变成「危险入口 → 确认」。
4. 新确认行出现时把焦点移到「确认重跑」(`nextTick` + `ref`,可选但与 `Esc` 取消配套:`@keydown.esc` 已在编辑态用过)。

**要点**
- 文案报的是**真实代价**:`b.items.length` 就是本轮将被丢弃的演绎段数,不是泛泛的「确定吗?」。
- 单击不再是执行键,是**确认态的开启键**;3 秒超时复位保证不打断连续重跑的手感。
- `store.busy` 时两个按钮都 `disabled`(沿用现有 `:disabled="store.busy"` 模式)。
- 不改 `stores/play.ts`(确认是纯 UI 层职责)。

**验收**
- 单击「重跑本轮」**不**发起任何请求,出现确认行;
- 点「取消」或 3 秒无操作 → 恢复原按钮,无请求;
- 点「确认重跑」→ 走 `store.rerunLastRound()`,成功 toast「已重跑本轮」;
- 只有最后一轮(`b.key === lastRoundKey`)出现该按钮,`pending` 与 `meta` 回合仍无 footer。

### 1.2 [P1] 「重发」的通道漂移

**现状**:`FeedChat.vue` L46-49 `resend(text)` 调 `store.send(text)`。`stores/play.ts` L620-628 的通道判定是「`/` 前缀 → meta;显式 `'gm'` → 导演;否则 character」,所以**导演回合点「重发」会静默变成角色发言**。

**改法**:
```ts
function resend(e: RoundEntry) {
  if (store.busy) return
  void store.send(e.text, e.channel === 'gm' ? 'gm' : undefined)
}
```
模板 `@click="resend(b.entry)"`;导演回合按钮 title 改为「用同样的导演指令再开一个新回合」。

**对 A 报告建议的修正(重要)**:A 建议「角色回合重发时保留原 `actorName`」。核实后**不采纳**——`api/index.ts` L887 `submitRound(saveId, channel, text, requestId, …)` **不接受 actor 覆盖参数**,服务端一律按存档当前受控角色执行;若乐观条目仍显示原角色名,就会与真实结算结果不符,属于**显示谎言**。正确做法是诚实提示:当 `b.entry.actorName` 与 `store.controlled?.name` 不同(玩家中途切过角色)时,按钮 title 提示「将以当前受控角色「X」重发」。

**验收**
- 导演回合重发后,新回合徽标是「导演」(旧行为是「玩家 · 角色名」);
- 角色回合重发仍走角色通道;
- 切换受控角色后,历史角色回合的重发 tooltip 说明将用哪个角色。

### 1.3 随阶段 1 一起进的两行修

- **空标题场景分隔**:`FeedChat.vue` L188-199,scene 条目无 `title` 时退化成「两条孤线夹空白」(线上实例截图可见)。给分隔块加 `v-if="b.entry.title"`,无标题时只保留 `my-3` 间距。
- **footer 按钮焦点环**:L131-183 各按钮补 `focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30`,与编辑态 textarea 的 `ring-2 ring-primary/20` 统一。

---

## 阶段 2(已改向,原「保留回合卡」方案作废):合并叙事流

> **状态:已实施并验收(2026-09-13)**。用户看过参考图后改向——气泡版整体退场,不再「保留回合卡降噪」。
> 三项决策:①说话人只在切换时署名 ②玩家输入改为右侧「你」引用行 ③直接替换,不留可切换的聊天流。
> 与蓝图一致:docs/blueprint/issues/08-play-ui.md 的 ① 早已定「B 剧本式(纸面浅色)」,原型 docs/blueprint/prototypes/play-ui.html 有 B 变体;代码此前只剩 A 聊天流。

实施与验收明细见文末「阶段 2 实施记录」。以下 2.1–2.4 为作废的原方案,仅留档。

### 2.1 拆掉「卡片套气泡」的第二层容器(最大一刀)

- 现状:回合卡 `rounded-2xl border border-border/70 bg-card/45 shadow-2xs`(`FeedChat.vue` L82-85)内部再放气泡 `rounded-2xl border border-border bg-card`(`FeedText.vue` L67-69)。检测器 `nested-cards` 命中 51-57 处。
- 改法:回合卡外壳降级为「段落容器」——去掉 `border` 与 `bg-card/45`,改为 `border-b border-border/40` 分隔 + 留白;气泡保留自己的边框(它才是需要边界的元素)。悬停回馈用 `hover:bg-muted/20` 补,避免失去区块感。
- 卡头随之减重:`bg-muted/25` → 透明;徽标保留(承载通道语义);时间戳挪进 hover 操作行。
- 待定项:是否给玩家回合加一条左侧 2px 通道色细线以保住「这是谁的一轮」的一瞥可得。**建议出 A/B 两版截图再定**,不在本阶段预先锁死。

### 2.2 footer 的条件呈现

- 历史回合卡:footer 默认低权重/隐藏,`group-hover` + `group-focus-within` 浮现,并加 `[@media(hover:none)]:opacity-100` 保证触屏可达;或收纳为单个「⋯」+ `components/ui/dropdown-menu`(原语已存在)。
- 最后一轮:footer 常驻(核心操作区,且是 P0 确认态的落点)。
- 目标:阅读面上绝大多数卡不再有常驻按钮行。

### 2.3 条目级操作行可达

`FeedText.vue` L98-106:现为 `opacity-0 group-hover:opacity-100`,触屏永久不可见、键盘 Tab 聚焦到隐形按钮。加上 `group-focus-within:opacity-100` 与 `[@media(hover:none)]:opacity-100`;时间戳(弱信息)改常显,只保留「复制」的渐进披露。

### 2.4 导演裁定卡不再伪装系统通知

`FeedText.vue` L41-47 的 `border-l-2 border-info/60 bg-info/5` 蓝卡 = 检测器 `side-tab`(CLI 1 处 + 运行时 5 处,两份评估互证),且把纯叙事散文装进系统通知样式。改法:正文回落旁白排版(衬线/行距),段首挂一个无底色无边框的 `text-[10.5px] text-info` 「导演裁定」小标。

---

## 阶段 3(待排期)

- **小字体系**:检测器 `undersized-ui-text` 170+ 处(10-10.5px)、`flat-type-hierarchy`(10-16px 共 12 档、对比 1.6:1)。收敛为 4-5 档(`11 / 12 / 13 / 14.5 / 16`),元信息下限 11px。
- **窄屏**:`PlayPage.vue` L300/346 两个 aside 固定 240px + 300px,414px 视口下演出流被挤成单字竖排。WorldRail 加 `hidden md:flex`、DetailPanel 加 `hidden xl:flex`,顶栏补抽屉入口(复用 SaveDrawer 模式)。

---

## 验证

- 构建 + 类型检查:`pnpm -C frontend build`(AGENTS.md 规定的常用命令,含 vue-tsc)。
- 浏览器手测(存档 `sv-23aecc66bbe6435daa6a79264000ed4c`,http://localhost:5173/play/sv-23aecc66bbe6435daa6a79264000ed4c ):
  - P0:单击重跑 → 出现确认行;取消/超时复位;确认后 toast「已重跑本轮」。
  - P1:导演回合重发 → 新回合徽标「导演」。
- 回归面:编辑并重跑、元指令回合(无 footer)、pending 回合(无 footer)、`lastRoundKey` 门控不变。
- 阶段 2 完成后重跑 `node /home/huang/.agents/skills/impeccable/scripts/detect.mjs`,确认 `nested-cards` / `side-tab` 下降。

## 验收记录(阶段 1,2026-09-13)

存档 sv-23aecc66bbe6435daa6a79264000ed4c,http://localhost:5173/play/sv-23aecc66bbe6435daa6a79264000ed4c 。

| 用例 | 结果 |
|---|---|
| 单击「重跑本轮」不发请求,出现确认行 | 通过:确认行出现,「重跑本轮」消失 |
| 确认行报出真实代价 | 通过:「将丢弃本轮 2 段演绎,且无法恢复。」 |
| 确认行自动接管焦点 | 通过:document.activeElement = 「确认重跑」 |
| 3 秒无操作自动复位 | 通过:恢复「重跑本轮」,无请求 |
| 点「取消」/Esc | 通过:复位且焦点归还触发按钮(data-rerun-last) |
| 编辑行回归 | 通过:带入原文、取消可关、「保存并重跑」仍在 |
| 导演回合「重发」提示 | 通过:2 个导演回合显示「用同样的导演指令再开一个新回合…」 |
| 构建 + 类型检查 | 通过:pnpm -C frontend build exit 0 |
| 检测器 | 通过:该文件 0 发现 |

未做:真正按下「确认重跑」的端到端重跑(会改写存档);实际发起一次导演回合重发(同上)。两者都是单行、已类型检查的调用路径。

## 阶段 2 实施记录(合并叙事流,2026-09-13)

### 改动

| 文件 | 改动 |
|---|---|
| feed/ProseFlow.vue | 新增。台词走气泡(同一说话人连续台词合并、头像 + 名字在气泡上方)、旁白/神态/导演裁定无气泡成段;后继样式以「样式修订(第二次)」为准 |
| feed/SceneDivider.vue | 新增。场景分隔抽出;无标题时退化为纯间距 |
| feed/FeedChat.vue | 重写。Segment = prose / scene / block,Block = round / loose;回合卡 = 右侧「你」引用行(通道色保留:你 / 导演 / 元指令)+ 分段 + 回合操作;开场与幕间正文走 loose 散段(同一套合并渲染,无卡);阅读行宽 1024px → max-w-3xl;历史回合 footer 降权 |
| StreamItem.vue | 收窄为纯机器事件宿主,去掉 content 分支与 showSpeaker |
| FeedText.vue | 删除(气泡 + 头像 + 条目级 hover 复制行随之下线) |
| InputBar.vue | 输入框 min-h 12rem(192px) → 5rem(80px)、max-h 440px → 300px、py-3 → py-2.5。实测:空态 80px、3 行 93px、20 行封顶 300px 后内部滚动 |

### 决策与取舍

- **说话人署名**:只在切换时署名。参考图是单角色场景可以不署名,但同场多角色 + 导演通道会读混。旁白会重置署名基准(旁白之后再说同一人的话,重新署名更清楚);神态不重置(它是同一拍的动作,不该打断)。
- **头像挂在署名上**(2026-09-13 修订):用户要求保留角色头像。做法是让它和「说话人切换」的署名一起出现(36px 圆角立绘 + 名字,缺立绘回落姓名首字色块),同一人连说不重复——既把角色找回来,又不会退回「一行一个头像」的碎感。
  初次实施曾整轮不挂头像(理由是「一轮 = 玩家的行动 + 世界的回应,头像无锚点」),该判断被用户否决,保留此记录以免再次走回头路。
- **机器事件不并进散文**:判定卡 / 确认门 / 系统行 / 目标进度仍是独立块——它们是引擎事件,不是叙事文本。
- **复制改为回合级**:条目级 hover 复制行删除后,改为回合 footer 的常显「复制」按钮(复制整轮)。

### 样式修订(2026-09-13,第二次)

用户定稿为**气泡 + 无气泡混排**,取代上一版「全部连读散文」:

- **台词 → 气泡**:头像在左、名字在气泡上方、气泡内可含多行台词;同一说话人的连续台词合并成一个气泡。
  - 注意:合并**已在 store 层完成**(play.ts L320-337:同说话人连续台词并进同一个 content 条目,以 \n 分隔),组件只需按气泡渲染;`renderMarkdownInline` 会把 \n 转成 `<br>`,故多行不会塌成一行。真实数据里第 11 轮有一个双行台词气泡,已截图验证。
- **旁白 / 神态 / 导演裁定 → 无气泡**:直接成段;神态斜体 + 情绪小标;导演裁定段首小标。
- **玩家输入 → 右对齐气泡**,标签(你 / 导演 / 元指令)移到气泡上方,通道色变为气泡边框 + 浅底。
- 气泡内动作行(store 折叠的 emote)改为**独占一行**的斜体,不再跟在台词同一行末尾。

### 样式修订(2026-09-13,第三次):角色动作并入气泡

用户指出「夹在两句台词之间的角色动作被拆到气泡外」,要求并入。

**根因(已核实到代码)**:这类旁白在事件里 **actor = null**——31 条 narrate 只有 5 条带 actor。
引擎 `session.rs` 的 `resolve_intent_actor` 对 `Narrate` 传空串,即 **narrate 不参与「按名字认人」的归属推断**,只有 `emote` 参与;
而 `protocol.rs` 的 SYSTEM_PREAMBLE 只说「emote 要带 actor_id」,没说「角色自身的动作应该用 emote 而不是 narrate」,所以模型把角色动作写成了无归属旁白。

**现方案(前端呈现层启发式)**:一段旁白同时满足①夹在同一角色的前后两句台词之间、②主语指向该角色 —— 才收进该角色气泡作为动作行。
主语判定:`以角色名起句` 直接认;`以「她 / 它」起句` 仅在本批内容只有一个说话人时才认;`以「他」起句` 保守跳过(既可能是玩家也可能是男性角色,认错比不认更糟);纯场景描写不以人称作主语,自然落选。

**边界(已知)**:①男角色以「他」起句的动作不会合并;②多说话人场景不用代词判定,只认名字起句;③紧跟在台词之后、后面没有同角色台词的旁白(如第 19 轮「她把姜茶轻轻放在料理台上…」之后的收尾描写)不合并——因为缺少「前后都是同一人」这个结构信号。

**根本解(已实施 2026-09-13,引擎侧)**:

1. **prompt**(`protocol.rs` 的 SYSTEM_PREAMBLE 第 3 条 / TOOL_PREAMBLE 第 2 条):补一条正向规则——「**角色的动作、神态与反应一律用 emote(带 actor_id)**;narrate 只写环境、天气、时间流逝、场景切换与玩家角色自己的动作」。意图目录(`INTENT_CATALOG`)与工具清单(`INTENT_TOOLS`)的说明同步改清楚。
   —— 注意:初稿用了「不要写成 narrate」,被 `rig_provider::tests::preamble_is_positive_first` 拦下(项目要求系统提示词正向表述、避免「不要…」),已改为正向说法。
2. **引擎**(`session.rs`):`resolve_intent_actor` 让 `Narrate` 也参与文本归属推断,但**只认句首主语位**(窗口 `NARRATE_ACTOR_HEAD_CHARS = 6` 字)——「露西靠在他身侧…」认成露西的动作,「…露西的车停在那儿」只是环境描写里提及,仍回落玩家角色。实现上把原 `actor_in_text` 拆出 `longest_npc_in`,新增 `actor_leading_in_text`;PC 名照旧不认(那是称呼)。
3. **前端**(`ProseFlow.vue`):规则 3 改为**优先采信引擎给的事件归属**,主语启发式降级为「引擎没给归属」时的兜底。

**遗留**:①归属在事件产生时确定,**已有历史事件的 actor 仍是 null**,老数据继续走前端兜底;②prompt 改动需**重启引擎进程**才生效(改动前启动的 octopus-bin 用的是旧提示词)。

**测试**:`cargo test --workspace` 全绿(325 个用例),其中新增 `session::tests::narrate_actor_is_inferred_only_from_leading_subject` 覆盖「主语位认人 / 仅提及不认」两侧;`pnpm -C frontend build` exit 0。

**验证**:存档 sv-23aecc66bbe6435daa6a79264000ed4c 实测——用户举的第 22 轮那段已并入 Lucy 气泡;
收紧规则前误并入的「车子拐进枫树街…」(纯场景)与「他绕过车头…」(玩家动作)已排除;气泡内动作行 5 处,均为角色本人动作;独立神态 2 处(体贴 / 惊慌)保持无气泡。

### 验收(补充阶段 1 记录)

| 用例 | 结果 |
|---|---|
| 构建 + 类型检查 | 通过:pnpm -C frontend build exit 0 |
| 检测器(4 个文件) | 通过:0 发现(原 side-tab 蓝卡问题随卡片删除消失) |
| 头像 / 气泡下线 | 通过:整页 0 个逐条头像,0 个气泡 |
| 合并连读 | 通过:旁白成段、台词内嵌「」、说话人切换才署名 |
| 开场散段(第一个回合之前) | 通过:loose 块渲染为一栏纸面正文,无卡 |
| 阅读行宽 | 通过:实测卡宽 708px(约 45 字/行) |
| 历史操作行降权 | 通过:class 绑定正确(headless 因 (hover:none) 命中触屏兜底而常显,符合设计) |
| P0 重跑确认回归 | 通过:单击开确认行、焦点落在「确认重跑」、取消后焦点归还「重跑本轮」 |
| 情绪小标 / 导演裁定小标 | 通过:体贴、惊慌、导演裁定 各自渲染 |

未做:判定卡 / 确认门在本存档无数据,未做视觉验收(代码路径未改,仅换了容器)。

## 重跑不再「刷新页面」(2026-09-13)

**现象**:确认重跑后整页像被刷新了一次。

**根因**(非浏览器 reload,全项目只有 setMockMode 里有 location.reload):
`rerunLastRound` → `await init(saveId)` 整页重新水合;而 `init` 开头就 `teardown()` + `entries = []` + `ready = false`,
PlayPage 的 `<main v-if="store.ready">` 于是整块换成「水合世界中…」占位;重建完成后 `entries.length` 变化又触发自动滚底。
之所以必须重新水合:引擎重跑会归档旧回合、回滚会话,**事件用新 seq**(`session.rs` 的 `rerun_rolls_back_and_reuses_same_round`),增量 patch 无法覆盖。

**方案 A(已实施):数据面照旧完整水合,呈现面不再重建**
1. `init(id, { keepVisible })`:该模式下不清屏、不动 `ready`;历史先回放进 `buf`,全部就绪后 `entries.value = buf` 原子替换。
2. `rerunLastRound` 传 `{ keepVisible: true }`,并置 `rerunning` 状态。
3. PlayPage:`rerunning` 期间不自动滚底,替换后恢复原阅读位置(按新的可滚范围夹取)。
4. FeedChat:重跑期间在最后一轮操作行显示「重跑中,生成完成后原地替换本轮…」——不再有占位屏,必须留一个「确实在跑」的信号。

**验证时发现并修掉的三个问题**:
- **缓冲回放查重读错数组**(自己引入):`round_start` 的对账/去重仍在读 `entries.value`(旧数组),于是回放出来的回合条目被判为「已存在」而丢弃——实测重建后 0 个回合卡、内容全落进散段。修:新增 `currentEntries()`,回放期一切查重走缓冲。
- **SSE 订阅泄漏**(keepVisible 引入):该模式跳过 `teardown()`,而 init 结尾直接 `unsub = subscribe(...)` 覆盖引用——旧 EventSource 永不关闭,**实时事件会双投**。修:重订前显式 `unsub()`。用 `EventSource.prototype.close` 打点证明:修前重跑全程 0 次调用,修后 1 次。
- **焦点接管顺带把阅读位置拽到底**(既有):确认行的 `focus()` 触发浏览器滚动。修:`focus({ preventScroll: true })`,两个焦点接管点都改了。

**验证方式**(不动存档、不消耗 AI):在页面里覆盖 `window.fetch`,把 `POST .../rerun` 桩成延迟 2.5s 的 204,其余请求照常走真后端;走真实交互 arm → 确认。

**结果**:回合 25→25、气泡 33→33、`scrollTop` 全程 8495 不变、全程无「水合世界中」、重跑中提示可见、旧订阅已关闭。

**已知取舍**:`init` 只拉最后一页历史;若此前点过「加载更早」,重跑后更早的页会丢(既有行为),此时滚动位置按新的可滚范围夹取。

## 涉及文件

| 文件 | 阶段 | 内容 |
|---|---|---|
| `frontend/src/pages/play/components/feed/FeedChat.vue` | 1 + 2 | P0 确认态、P1 通道、空标题守卫、焦点环、卡片减重、footer 条件呈现 |
| `frontend/src/pages/play/components/FeedText.vue` | 2 | 操作行可达、导演裁定卡回落旁白 |
| `frontend/src/pages/play/PlayPage.vue` | 3 | 窄屏折叠 |
| `frontend/src/pages/play/stores/play.ts` | — | **不改**(send 已有 explicit 通道参数) |
