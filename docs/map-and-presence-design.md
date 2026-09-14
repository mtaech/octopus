# 实景与剧情关联设计（地图 · 地点 · 在场 · 章节 · 任务 · 怪物）

> 触发：① 地图、人物、怪物之间没有关联——**哪个 NPC / 怪物出现在哪张地图上，完全没有实景**；
> ② 情节章节与地图、任务、怪物之间同样没有关联。
>
> 核心诊断：位置与剧情的**机制早已存在，但从未被驱动**，关联全靠散文与手抄名单维系。
> 缺的不是机制，是三样东西——① 模板层的初始位置 ② 驱动它的场景切换 ③ 地图与关联视图。
>
> 主张：**场景（Scene）是唯一枢纽**——叙事与空间都从它穿过，其余关联全是推导或引用。

---

## 0. 结论摘要

| 层 | 决定 |
|---|---|
| **地点 (Location)** | 保持现有的树形结构（`world.locations[]` + `parent_id`），不动 |
| **地图 (Map)** | **新实体** `world.maps[]`：一张底图 + 若干地点的**锚点**（归一化坐标）。地图是地点的**投影**，不是地点本身 |
| **在场 (Presence)** | 从「场景硬编码名单」改为**位置驱动 + 显式覆盖**三层优先级；`present_char_ids` 降级为覆盖手段 |
| **驱动链** | `CharacterDef.location_id`（常驻地）→ `build_state` 灌进实例 → `switch_scene` 按位置算在场 → `Move` 泛化到任意角色 |
| **与图鉴衔接** | 图鉴条目带出没地 · 遭遇绑定发生地点 · 怪物实例继承遭遇地点 → 怪物自动出现在地图上 |
| **剧情层关联** | **场景是枢纽**：章节的地点由其场景推导 · 任务的地点继承场景 · 触发点可**预置遭遇** · 遭遇快照场景/地点/目标 |
| **提示词** | 当前**地点名**首次进入提示词（`TurnContext` 现在根本没有这个字段） |
| **兼容** | 纯加性：有 `present_char_ids` 的旧故事书行为**逐字不变**；地图是纯视图，引擎不读，不需要 `upcast` |

---

## 1. 现状：机制齐备，链路是死的

### 1.1 已有的位置机制

| 机制 | 位置 | 状态 |
|---|---|---|
| `CharacterInstance.location_id` | `octopus-types/src/lib.rs:825` | 字段存在，可写 |
| `Intent::Move { destination_id }` | `session.rs:2873` | 只写**受控角色**的位置 |
| `CondExpr::AtLocation { location_id }` | `conditions.rs:63` | 会求值 `actor_location` |
| `ObjectDef.location_id` | `types:238` | 有字段、有校验 |
| `SceneDef.location_id` | `types:122` | 有字段、有校验 |
| `LocationDef.parent_id` | `types:47` | 树形层级（区域 > 城镇 > 建筑） |
| 游玩页地点栏 `charsAt()` | `WorldRail.vue:39` | 按 `c.location_id` 数人 |

### 1.2 五个断点

1. **`CharacterDef`（人物模板）没有 `location_id`**——作者无法声明 NPC 常驻哪。
   种子数据里伊莎的背景写着「碎星酒馆老板娘」，`loc-tavern` 就在那儿，场景
   `sc-tavern-night` 的 `location_id` 也正是它——**这层关系只活在散文里**，
   作者还得在 `present_char_ids` 里手抄一遍名字（`seed.rs:36-45`）。

2. **`build_state` 把位置写死 `None`**（`octopus-api/src/lib.rs:493`）→ 开档后**所有 NPC 都没有位置**。

3. **唯一写位置的 `Move` 只作用于受控角色**（`session.rs:2874-2891`）→ NPC / 怪物的位置**永远是 `None`**。

4. **在场与位置完全无关**：`switch_scene` 的判据是
   `present_ids.contains(template_id) || controlled`（`session.rs:1600`）。

5. **两套机制各有各的兜底，且不一致**：
   - 开档 `is_initially_present`：未声明 `present_char_ids` → **全员在场**（`octopus-api/src/lib.rs:373`）
   - 切场 `switch_scene`：`present_ids` 缺省是**空集** → **全员不在场（除 PC）**

   于是「没写在场名单的场景」一进去就把所有人清空了。这是一个**潜伏的行为不一致**，
   平时被作者总是填 `present_char_ids` 掩盖着。

### 1.3 后果

- `AtLocation` 条件**恒假**（`actor_location` 永远是 `None`）→ 引擎里一个已实现的条件类型是死的；
  「抵达某地点」这类触发点写不出来。
- `WorldRail.charsAt()` 对**所有 NPC 恒为 0** → 地点栏的人数标注没有意义。
- UI 靠**近似补丁**掩盖：`DetailPanel.vue:56` 的注释直言
  「当前场景地点：在场人物都在这里（**实例的 location_id 可能没写**）」。
- 玩家看不到「谁在哪」，作者写不出「谁在哪」。

### 1.4 词汇缺口

CONTEXT.md 里**没有「地点」，也没有「场景」词条**——和遭遇 / 怪物是同一种缺口：
功能带着未登记的概念落地了。（现有 `LocationDef` / `SceneDef` 是仅有的实现侧名词。）

---

## 2. 术语（CONTEXT.md 待新增）

**地点 (Location)**:
世界里一处可寻址的位置，树形组织（区域 > 城镇 > 建筑 > 房间）；场景 / 物件 / 角色实例都挂在它上面。
_实现_: `world.locations[]`（`id` / `name` / `description` / `parent_id` / `image`）。
_Avoid_: 地图（地图是地点的可视层）、场景（场景是剧情单元，一个地点可承载多场戏）

**场景 (Scene)**:
骨架里的一段戏——有标题、描述、目标、触发点，**发生在一个地点**。
_实现_: `skeleton[].scenes[]`；`location_id` 指向地点；`present_char_ids` 是在场名单的**显式覆盖**。
_Avoid_: 地点（空间 ≠ 剧情）

**地图 (Map)**:
地点之上的**可视层**：一张底图 + 若干地点在它上面的**锚点**。地图不是地点，是地点的投影——
同一个地点可以在世界图、城镇图、地牢图上各有一个锚点，位置各不相同。
_实现_: `world.maps[]`（`id` / `name` / `parent_id?` / `image` / `pins[]`）；引擎不读，纯展示。
_Avoid_: 地点（地图是视图，地点是实体）

**在场 (Presence)**:
运行时「某个角色此刻在这一场戏里」。由**位置匹配 + 作者点名**共同决定，不是单一字段。
_实现_: `CharacterInstance.present`；`switch_scene` 按三层优先级重算；提示词「在场角色」按它过滤。
_Avoid_: 出现（过泛）、在场人物（「人物」已专指模板）

---

## 3. 地图（新实体）

```ts
// frontend/src/types/index.ts
export interface StorybookWorld {
  premise: string
  opening?: string
  locations: LocationDef[]
  resources: ResourceDef[]
  check?: CheckerDef | null
  maps?: MapDef[]          // ← 新增
}

export interface MapDef {
  id: string
  name: string
  /** 地图也可层级：世界图 > 区域图 > 地牢图 */
  parent_id?: string
  /** 底图（复用资产不可变存储） */
  image?: AssetRef
  /** 地点锚点 */
  pins?: MapPin[]
}

export interface MapPin {
  location_id: string
  /** **归一化**坐标 0..1（相对底图），换分辨率 / 换底图不破版 */
  x: number
  y: number
  /** 覆盖地点名：同一地点在不同地图上叫法可以不同 */
  label?: string
}
```

### 三个关键取舍

1. **归一化坐标而非像素**：底图分辨率变化、换图都不破坏锚点——与 `AssetRef` 存像素宽高
   是同一个理由（前端预留版面、避免抖动）。
2. **锚点属于地图，不属于地点**：碎星酒馆在世界图上是一个点，在城镇图上是一个门面。
   所以坐标存在 `MapDef.pins[]` 里，而不是 `LocationDef.x/y`。
3. **地图不塞进地点树**：若把地图做成 `LocationDef` 的一个节点类型，「一个地点出现在两张图上」
   就表达不了。地图与地点**平级**，靠 `pins` 关联。

**没有地图的故事书照常运行**：地图是纯视图，引擎不求值、不校验锚点（只在编辑器侧做存在性提示）。

---

## 4. 在场：位置驱动 + 显式覆盖

### 4.1 三层优先级

```
算一个角色此刻在不在场：
  ① 作者点名：场景 present_char_ids 含该模板 id        → 在场（覆盖优先级最高）
  ② 位置匹配：实例 location_id == 场景 location_id    → 在场（新，实景的来源）
  ③ 兜底：场景既无 location_id 又未声明 present_char_ids → 全员在场（沿用开档的既有回落）
  （受控角色恒在场，不变）
```

**为什么必须保留第①层**：骨架剧本的写法依赖「这一幕这几个人出场」。若 NPC 模板没写常驻地，
位置匹配必然落空——只做②会让所有老故事书瞬间空场。所以①是**覆盖**，不是被替代。

**为什么①优先于②**：作者显式点名是「剧作意图」，位置是「世界默认」。作者说这人在这里，
那就是这里（比如「刺客其实一直潜伏在酒馆」）。

### 4.2 配套驱动

| 改动 | 位置 | 说明 |
|---|---|---|
| `CharacterDef` 加 `location_id?: string` | `types:137` | 作者声明常驻地 |
| `build_state` 灌位置 | `octopus-api/src/lib.rs:493` | 替换写死的 `None` |
| `switch_scene` 按三层重算 | `session.rs:1594-1615` | 顺带修掉 §1.2 的「缺省名单即清场」不一致 |
| `Move` 泛化 | `session.rs:2873` | 允许移动任意角色（NPC / 怪物也能被搬），不再只动受控角色 |
| `is_initially_present` | `octopus-api/src/lib.rs:364` | 与 `switch_scene` 统一到同一套三层判据（消除两处不一致） |

### 4.3 地点树不做向上匹配（v1）

场景在 `loc-tavern`（`loc-town` 的子节点）时，位于 `loc-town` 的角色算不算在场？
**v1 只做精确匹配**——简单、可预期、可解释。分层匹配（同建筑可见 / 不可见）需要额外语义
（视线、隔墙、音量），留给后续；登记的触发点在 §8。

---

## 5. 与图鉴的衔接

地图层**不存**「哪个怪物在哪」——那是派生信息。关系挂在数据本身上：

| 层 | 数据 | 效果 |
|---|---|---|
| 图鉴条目（模板） | `location_id`（巢穴）· `habitat?: string[]`（出没地，可多个） | 作者声明「地精出没于回声洞」 |
| 遭遇（运行时） | `Intent::Encounter { …, location_id? }` | 遭遇绑定**发生地点** |
| 怪物实例 | 创建时 `location_id = 遭遇地点` | 怪物出现在地图那个锚点上 |
| 地图视图 | 按 `location_id` 聚合实例 | 「回声洞：3 只地精」 |

于是「哪个怪物出现在哪张地图」由**实例位置**自动成立，地图不需要第二份关系表。
与 [怪物图鉴设计](./bestiary-design.md) 的 M2 合流：怪物克隆实例时多写一个 `location_id` 字段。

---

## 6. 剧情层：章节 · 场景 · 任务 · 遭遇的关联

### 6.1 现状：只有一根线，而且是断的

| 关联 | 现状 |
|---|---|
| 章节 → 场景 | ✅ 包含（`skeleton[].scenes[]`） |
| 场景 → 地点 | ✅ `SceneDef.location_id`（但链路是死的，见 §1、§4） |
| 场景 → 人物 | ⚠️ `present_char_ids` 硬编码名单 |
| 场景 → 目标 / 触发点 | ✅ 包含 |
| 目标 / 触发点 → 世界状态 | ✅ `CondExpr`（`at_location` / `flag_set` / `trigger_fired` / …） |
| **章节 → 空间** | ❌ 无从表达 |
| **目标 / 任务 → 地点 · 怪物** | ❌ 无结构化关联（只有条件，而 `at_location` 恒假） |
| **触发点 → 遭遇 · 怪物** | ❌ 只有一句 `hint` 自由文本 |
| **遭遇 → 场景 · 章节 · 任务** | ❌ 没有任何叙事归宿 |
| **地点 → 提示词** | ❌ AI 根本不知道当前在哪 |

最后一行的证据：`TurnContext` 只有 `scene_title` / `scene_description`
（`ports.rs:95-124`，**没有 location 字段**），渲染时只拼标题 + 描述（`rig_provider.rs:440-453`）。
**地点名从未进过提示词。**

### 6.2 主张：场景是唯一枢纽

不引入通用锚点表——那会滑向「用数据声明语义」，违反 `docs/open-kinds-proposal.md:31` 的判据
（引擎必须求值 / 改写的东西才升格封闭核心，其余靠引用与推导）。
所有关联都穿过 `scene`，其余全是**推导或引用**：

```
章节 chapter ──包含──→ 场景 scene ──location_id──→ 地点 ──pin──→ 地图
                         │
                         ├─ present_char_ids ──→ 人物模板（降级为「覆盖」）
                         ├─ goals[]    ──condition──→ 世界状态
                         │      └─ 派生 QuestView（地点 / 关联继承场景，不存字段）
                         ├─ triggers[] ──condition──→ 世界状态
                         │      └─ ★encounter? ──→ 图鉴条目（怪物模板）
                         └─ ★遭遇创建时快照 scene_id / location_id / goal_id
```

- **章节的地点 = 其 scenes 的地点集合并集**（**推导**，不存字段 → 无冗余、不会不同步）
- **任务的地点 = 所属场景的地点**（**推导**）
- **遭遇的叙事 / 空间锚 = 创建时的快照**（**存**，不推导——运行时事件必须可重放，
  查询「此刻的场景」在重放时会得到错误答案）

### 6.3 新增：触发点可以预置遭遇

现在遭遇只能靠导演 AI 即兴 `encounter` 意图创建——「进门遇袭」全看模型想不想得起来。让作者能声明：

```ts
interface TriggerDef {
  // …既有字段…
  /** 触发时预置的遭遇（作者声明，不再靠 AI 即兴 spawn） */
  encounter?: EncounterPreset
}

interface EncounterPreset {
  name?: string
  note?: string
  /** 缺省继承场景的 location_id */
  location_id?: string
  /** 引用图鉴条目 */
  enemies: { template_id: string; count?: number }[]
}
```

引擎：回合末 `evaluate_skeleton` 把触发点标记为 fired 之后，若带 `encounter`，
复用 `Intent::Encounter` 的**同一条路径**自动建遭遇（含 §5 的实例克隆与地点继承）。

### 6.4 新增：遭遇有叙事归宿

```rust
pub struct EncounterView {
    // …既有字段…
    scene_id: Option<String>,      // 创建时的场景
    location_id: Option<String>,   // 创建时的地点（缺省继承场景）
    goal_id: Option<String>,       // 关联目标（可选）
    template_ids: Vec<String>,     // 涉及的图鉴条目
}
```

用途：地图按 `location_id` 归位 · 任务面板显示「清剿中（3 只存活）」· 提示词能说明「这一战是为了什么」。

### 6.5 新增：目标能等遭遇清空（可选）

`CondExpr` 加 `{ op: 'encounter_cleared' }`：当前场景的遭遇全部结束、或敌人全灭。
让「清剿」有**自然的完成判据**，不必绕道 `flag_set` 让导演记得设标记。是否要做见 §10。

### 6.6 新增：地点进提示词

`TurnContext` 加 `location: Option<String>`，场景块渲染为「场景：碎星酒馆的夜晚（地点：碎星酒馆）」。
**注意位置**：这是 turn 骨架（**用户消息**）的一部分，不是系统 preamble——
符合 AGENTS.md 的上下文缓存不变量（前缀必须逐回合稳定）。

### 6.7 地图从「地点视图」升级为「剧情视图」

这一步是三层关联的最终兑现——地图面板 / `WorldRail` 按地点聚合：

| 聚合项 | 来源 |
|---|---|
| 该地点的角色实例（PC / NPC / 怪物） | `characters[].location_id` |
| 该地点的场景（当前场景高亮） | `scene.location_id` |
| 该地点的遭遇（存活敌人数） | `encounter.location_id` |
| 该地点的任务 | QuestView 继承自场景 |
| 当前章节的范围（其场景地点集合高亮） | `chapter → scenes → location_id` 推导 |

于是「这一章发生在哪、这里有什么任务、谁在这儿、有哪些怪」在地图上一屏可见。

### 6.8 真实剧本核对：漏了「遭遇表」

> ⚠️ 见 [LMoP 核对报告](./lmop-design-verification.md)。LMoP 的「游荡怪物」是**掷表**驱动的：
> 「掷一枚d20……若骰出17-20点，则会发生一次遭遇。**再掷一枚d12骰子并查阅野外遭遇表格**来确定队伍会遇到什么」（第 1221 行）。
>
> §6.3 的 `EncounterPreset` 只能表达**固定遭遇**，表达不了「掷表决定」。
>
> 🔁 **修正**：掷表**不新增 `EncounterTable` 封闭字段**——那是内容 + 规则集语义，按
> [规则集走 Lua](./rules-via-lua.md) 应由 Lua 表达：
>
> ```lua
> -- lua_mounts 里的一条：每次进入区域时掷游荡怪物表
> local trigger = host.engine_rng(1, 20)
> if trigger >= 17 then
>   local roll = host.engine_rng(1, 12)
>   host.trigger_event('wandering_monster_' .. roll)   -- 事件 → 预置遭遇（§6.3 的绑定）
> end
> ```
>
> 引擎只提供两样已有的东西：确定性掷骰 `engine_rng` 与 `trigger_event`；**表本身是内容**，
> 走开放内容或触发器声明，不是引擎类型。
>
> 另外**突袭**在真实剧本里出现 25 处（「将突袭，并在战斗第一轮失去其回合」），
> 不需要完整先攻系统，但需要「战斗第一轮」这个标记——从后置清单提到 P5。

---

## 7. 编辑器与游玩页

**编辑器**
- 世界 tab 加「地图」子区：底图上传 + 拖拽锚点 + 点选地点绑定 + 同名冲突提示
- 人物 / 怪物卡加「常驻地 / 出没地」选择器（引用地点）
- 骨架场景编辑：`location_id` 旁显示「按位置会来哪些人」，让作者看见自动在场的后果再决定是否覆盖
- 按 AGENTS.md 硬规则：可编辑控件静止态必须看得出可编辑

**游玩页**
- 新增「地图」面板：底图 + 锚点 + 当前地点高亮 + 单位头像簇（PC / NPC / 怪物以边框或底色区分）
- `WorldRail` 地点栏改为按位置聚合，替换现在**恒 0** 的 `charsAt()`
- 删掉 `DetailPanel` 的近似补丁（`DetailPanel.vue:56`「当前场景地点：在场人物都在这里」）

---

## 8. 兼容与验收

### 兼容

| 面 | 兼容性 |
|---|---|
| `CharacterDef.location_id` | 可选 → 旧故事书不受影响 |
| 在场三层优先级 | 有 `present_char_ids` 的旧故事书**第①层直接命中**，行为逐字不变 |
| `switch_scene` 修不一致 | 只影响「未声明 `present_char_ids`」的场景——那正是现在**坏的**行为（清场） |
| `Move` 泛化 | 旧调用（受控角色）行为不变 |
| `world.maps[]` | 纯视图，引擎不读，不需要 `upcast`、不需要 `schema_version` 变更 |

### 验收

1. **回归**：旧故事书开档 + 连续切场，在场名单与改动前**逐字一致**
2. NPC 加 `location_id`、场景加 `location_id`、清空 `present_char_ids` → 位置匹配生效
3. 作者点名的角色即使位置不符也在场（第①层覆盖第②层）
4. `AtLocation` 条件从**恒假**变为可真：写一个「抵达 `loc-tavern` 才触发」的触发点，验证触发
5. 未声明 `present_char_ids` 的场景切进去不再清场（§1.2 不一致已修）
6. `WorldRail` 的 `charsAt()` 对 NPC 返回真实数字
7. 地图锚点：底图换分辨率后坐标不漂（归一化的意义）

---

## 9. 里程碑

| 期 | 内容 | 依赖 |
|---|---|---|
| **P1 数据与校验** | `CharacterDef.location_id` · `MapDef`/`MapPin` · 校验（锚点地点存在 / 坐标范围 / 地图 id 唯一 / `parent_id` 无环） | — |
| **P2 驱动** | `build_state` 灌位置 · `switch_scene` 三层在场 · `is_initially_present` 统一 · `Move` 泛化 | P1 |
| **P3 视图** | 编辑器地图编辑 · 游玩页地图面板 · `WorldRail` 位置聚合 · 删 DetailPanel 补丁 | P1 |
| **P4 与图鉴合流** | 怪物出没地字段 · `Encounter.location_id` · 克隆实例继承地点 | 图鉴 M2 + P2 |
| **P5 剧情关联** | `TriggerDef.encounter` 预置遭遇 · 触发点自动建遭遇 · `EncounterView` 叙事锚（scene/location/goal/templates）· **地点进提示词** · `encounter_cleared` 条件 · **掷表遭遇走 Lua**（见 [规则集走 Lua](./rules-via-lua.md)）· 战斗第一轮标记（突袭） | P2 + 图鉴 M2 |
| **P6 地图即剧情视图** | 地图面板聚合：场景 / 遭遇 / 任务 / 角色 / 当前章节范围 | P3 + P5 |

**P2 是这一期真正的价值**：`AtLocation` 从死条件复活，实景从「作者手抄名单」变成「世界状态」。

**后置（登记在案）**：
- 地点树向上匹配（同建筑可见性）
- 移动的路径 / 距离 / 耗时（当前 `Move` 是瞬移）
- 地点上的「未知 / 已探索」迷雾状态
- 地图上的动态标记（任务点、事件，而非仅地点与单位）

---

## 10. 待确认

1. **地图是新实体 `world.maps[]`**（我的建议，支持一地点多图），还是复用地点树加一个节点类型？
2. **在场三层优先级**是否接受？关键点是第①层（作者点名）**优先于**位置匹配——这保证旧故事书逐字兼容，但也意味着位置不是绝对权威。
3. **地图层级**：v1 单层，还是直接支持 `parent_id` 多层（世界图 → 城镇图 → 地牢图）？
4. **与图鉴的排序**：先做 P1–P2（把位置链路打通，图鉴 M2 顺带就位），还是先落图鉴 M1–M2 再回头做地图？
5. **触发点预置遭遇**（§6.3）是否要做？它把「刷怪」从 AI 即兴变成作者声明——`TriggerDef` 会因此从「纯提示」变成「会产生状态变更」的节点，这是语义升级，需要你确认。
6. **`encounter_cleared` 条件**（§6.5）是否要做？不做的话「清剿任务」只能靠导演设 flag。
7. **地点进提示词**（§6.6）我建议**无论如何都做**——零成本，且当前 AI 连自己在哪都不知道。

---

## 附：两份设计的关系

| | 怪物图鉴 | 地图与在场 |
|---|---|---|
| 解决的问题 | 怪物没有模板、数值靠 AI 现编 | 谁在哪没有实景、位置链路是死的 |
| 共同依赖 | `CharacterDef` 扩容 | 同一处 |
| 交汇点 | 怪物出没地 + 遭遇发生地点 | 同 |
| 剧情层 | 遭遇引用图鉴条目（`EncounterPreset.enemies[].template_id`） | 触发点预置遭遇是这条引用的**作者入口** |
| 建议顺序 | **P1–P2 先做**：位置链路打通后，图鉴 M2 的怪物实例天然带上地点，P4 几乎零成本 | |
