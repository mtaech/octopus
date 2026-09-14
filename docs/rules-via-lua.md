# 规则集走 Lua（不写进类型系统）

> 触发：设计里那些 `advantage` / `on_save` / `ConditionalModifier` / `save_ends` / `crit` /
> `EncounterTable` / XP —— **不能写死，要走 Lua 引擎**。
>
> 核对结论：**你是对的，我上一轮的方向错了**。12 项核对发现里 9 项被我写成了新的封闭字段，
> 那是把 D&D 的规则集语义塞进引擎类型系统——换一套规则集（COC / 无限流 / 自定义）就要改引擎。
>
> 但核对下去还发现更硬的障碍：**现有的 Lua 面是「半通」的**——
> 判定修正挂载点跑了却改不了判定，注册表根本没有故事书入口。
> 所以这条路线缺的不是「更多 Lua 钩子」，是**五个最小原语 / 入口**。

---

## 1. 原则：仓库自己早就定了，是我没照做

`docs/open-kinds-proposal.md` 写得很清楚：

> **词汇开放，语义封闭。**
> 封闭：引擎能求值 / 改写的只有那几个固定原语；真正的新机制走原语扩展或 **Lua 兜底**，
> **不允许用数据声明新的求值语义**（否则会滑成一门没人维护的配置语言）。

判据原文：「一个概念只有在**引擎必须求值或改写它**时才升格为封闭核心」。

我上一轮的做法违反了两条：

1. 把「优势」「豁免减半」「熟练」这类**规则集语义**当成了封闭核心——
   它们不是引擎要「求值」的东西，是引擎要「被调用」的东西。
2. 加进去的字段（`on_save` / `ConditionalModifier` / `save_ends` / `crit`）**本身就是一门配置语言**的雏形，
   正是那段话警告的滑坡。

### 正确的三分法

| 层 | 是什么 | 归属 | 例子 |
|---|---|---|---|
| **签名** | 引擎要求值必须知道的**入口参数** | 封闭字段 | 判定用哪个属性 · 难度 · 攻守方 · 骰式 |
| **原语** | 引擎能做的**通用动作** | 封闭原语（要新增时按原语走） | 掷两次取高 · 施加即时效果 · 加减资源 |
| **规则** | 参数在不同规则集下取什么值、何时取 | **Lua** | 何时有优势 · 豁免成功怎么算 · 熟练给几点 · XP 怎么发 |

一句话：**引擎定义「能做什么」，Lua 定义「什么时候做」。**

---

## 2. 现有 Lua 面：比想象的强

| 能力 | 位置 | 状态 |
|---|---|---|
| 挂载点 | `LuaMount`（`lua_host.rs:45`） | `CheckPreRoll` / `CheckPostRoll` / `Check` / `PreResolve` / `PostResolve` / `Event` / `Condition` / `Protocol` |
| 确定性掷骰 | `host.engine_rng(min, max)` | ✅ 走引擎 RNG 序列，可重放 |
| 只读 API | `get_attribute` / `get_resource` / `has_status` / `relationship` | ✅ |
| 写请求（outbox → 引擎校验） | `request_cost` / `apply_status` / `remove_status` / `trigger_event` / `query_world` | ✅ |
| 自定义判定脚本 | `CheckerDef.lua` → `{ total, margin }` | ✅ 引擎只分档，不干预算法 |
| 条件脚本 | `CondExpr::Lua { script }` | ✅ |
| 技能钩子 | `SkillDef.lua`（挂 PreResolve） | ✅ |
| 脚本私有持久区 | `host.storage` | ✅ |
| 沙箱 + 静态校验 | `SandboxLimits` / `lua_lint.rs` | ✅ 指令预算 / 内存上限 / 禁用 API |

**所以「优势/劣势」这类纯掷骰规则，今天就能写**：

```lua
-- 作者在 CheckerDef.lua 里
local a = host.engine_rng(1, 20)
local b = host.engine_rng(1, 20)
local d = math.max(a, b)        -- 优势
local total = d + host.get_attribute('dex')
return { total = total, margin = total - host.difficulty }
```

问题不在「能不能写」，在于**只能长在单个技能上，无法组合**。

---

## 3. 五个断裂：为什么现在走不通

### 断裂 ①：挂载点只在「技能 / 物品」路径触发

`run_mount(...)` 全部调用点都在 `execute_skill` 内（`command.rs:257 / 296 / 297 / 318`）。
于是：

| 入口 | 跑挂载点？ |
|---|---|
| `use_skill` / `use_item` | ✅ |
| `Intent::Check`（`command::run_check`） | ❌ **不跑** |
| `Intent::Strike`（`strike_enemy`） | ❌ **不跑** |

→ 写进 Lua 的规则**对普通判定和攻击一律不生效**。
（这与[判定设计](./check-mechanism-design.md)的 D2/D3 是同一个病根：攻击绕过了内核。）

### 断裂 ②：判定修正挂载点**改不了判定**

`CheckPreRoll` 的注释是「判定修正（随机前）」，但：
- `run_mount` 返回 `Result<(), _>`——**返回值被丢弃**
- `CheckPostRoll` 在判定算完之后才跑（`command.rs:296` vs 判定在 `262-294`）
- `LuaRequest` 里**没有任何「改判定」的变体**（只有 cost / status / event / query）

→ 这两个挂载点**名存实亡**：跑了，说什么都没用。
这是「优势/劣势走 Lua」最直接的拦路石。

### 断裂 ③：状态 tick 不是 Lua 时机

`tick_statuses_turn` / `tick_statuses_scene`（`session.rs:3135-3144`）是纯引擎内部流程；
`dispatch_event` 只在场景切换（`2919`）与显式 `trigger_event` 请求（`3407`）时调用。

→ 「每回合末重复豁免（save-ends）」「专注打断」没有落点。

### 断裂 ④：注册表**没有故事书入口**

`lua_registry`（`session.rs:523`）注册的挂载点脚本，唯一入口是 `register_lua_hook`——
**一个只在测试里用的 API**（`session.rs:4130`）。
故事书里能挂 Lua 的地方只有：`SkillDef.lua` · `CheckerDef.lua` · `CondExpr::Lua` · `ProtocolConfig.lua`。

→ 作者**无法声明「在任何判定前，若角色有某状态则给优势」这种跨实体、可组合的规则**。
而这正是优势/劣势、条件修正、熟练加值的本质。
**这是最根本的一条**：没有它，前三个原语也只能长在单个技能上。

### 断裂 ⑤：outbox 不能造成伤害 / 不能加资源

`LuaRequest` 只有 `Cost`（扣当前 actor 的资源）、`ApplyStatus`、`RemoveStatus`、`TriggerEvent`、`QueryWorld`。
**没有**「施加即时效果」「加减任意目标资源」。

→ 「豁免成功伤害减半」「XP 合计」「吸血」「按成功度分档的额外效果」都写不出来。

---

## 4. 五个最小原语 / 入口

每一个都是「引擎必须开的口子」，**与规则集无关**——引擎不认识「优势」，只认识「掷两次取高」。

### ① 故事书级挂载点注册表（断裂 ④）

`ts`
export interface Storybook {
  // …
  /** 规则集插件：故事书声明的挂载点脚本 */
  lua_mounts?: LuaMountDef[]
}

export interface LuaMountDef {
  id: string
  /** check_pre_roll / check_post_roll / pre_resolve / post_resolve / event / condition / status_tick */
  mount: string
  /** 源文本（走 lua_lint 静态校验） */
  source: string
  /** 可选：仅在条件满足时执行（复用 CondExpr） */
  when?: CondExpr | null
  enabled?: boolean
}
`

→ 规则集 = 一组具名挂载点脚本，随故事书发布、随存档冻结、可被校验与禁用。

### ② `LuaRequest::ModifyCheck`（断裂 ②）

```lua
host.modify_check('keep_high')        -- 掷两次取高
host.modify_check('keep_low')         -- 掷两次取低
host.modify_check('add', 6)           -- 固定加值（熟练 +6）
host.modify_check('dc', -2)           -- 难度修正
```

> **落地实况（T3 已实现）**：引擎词汇就是上面这四个 —— `keep_high` / `keep_low` / `add` / `dc`。
> **引擎不认识「优势」这个词**。规则包要写 `host.modify_check('keep_high')`，
> 或者在自己的 Lua 里包一个 `local function advantage() host.modify_check('keep_high') end` 别名。
> 这是「引擎定义能做什么、Lua 定义什么时候做」的直接体现，不是命名疏漏。

- `CheckPreRoll` 收集 → 引擎**掷骰前**应用（改骰数 / 取高取低 / 改难度）
- `CheckPostRoll` 收集 → **掷骰后**应用（改 total / margin）
- **引擎只实现「掷两次取高/低」这个通用动作**；「什么时候算优势」全在 Lua

这一条同时解决：优势/劣势 · 条件性检定修正 · 熟练加值 · 日照敏感 · 集群战术。

### ③ `LuaRequest::ApplyEffect`（断裂 ⑤）

```lua
host.apply_effect(target_id, { kind = 'damage', amount = '3d6', resource = 'hp' })
host.apply_effect(target_id, { kind = 'heal', amount = '1d8' })
host.apply_effect(target_id, { kind = 'modify_resource', resource = 'hp', amount = '-5' })
host.apply_effect(target_id, { kind = 'set_flag', flag = 'surprised' })
```

- **直接复用既有的 `ImmediateEffect` 封闭原语**——不新增任何语义
- 「豁免成功伤害减半」= Lua 在 `check_post_roll` 里按判定结果发一个 `ApplyEffect`；
  于是 `EffectDef.on_save` 这个封闭字段**根本不需要**

### ④ `LuaRequest::ModifyResource`（断裂 ⑤）

```lua
host.modify_resource(target_id, 'xp', 50)
```

- 可正可负、可指定目标（`request_cost` 只能扣当前 actor）
- 「XP 合计」= 敌人被击败事件里 `modify_resource(actor, 'xp', monster.xp)`；
  于是 XP **不必升为封闭核心**（上一轮我改错了方向，这里收回）

### ⑤ 状态 tick / 回合末成为 Lua 时机（断裂 ③）

> ✅ **已落地（L4）**。挂载点名扩到 10 个：`check_pre_roll` / `check_post_roll` / `pre_resolve` / `post_resolve` /
> `event` / `condition` / `status_tick` / `turn_end` / `scene_end` / `protocol`。落地语义：
>
> - **每 (角色, 状态) 一次 `status_tick`**，在**该状态结算之前**派发；脚本若 `remove_status` 成功，引擎跳过本 tick 的持续效果与递减（否则后置的 `Set` delta 会把移除写回，save-ends 会失效）
> - **`turn_end` / `scene_end` 在该边界全部状态结算之后各派发一次** —— 规则包在边界新加的状态不会被同一次 tick 立刻递减
> - 上下文：`host.status{id,name,turns_left,scenes_left,unit,remaining}` + 平铺别名（`host.status_id` / `host.status_remaining` …），以及 `host.event`（= `host.mount` = 挂载点名）
> - 脚本报错 → 该链 fail-fast 并发 `*_lua_error` 系统事件，**不阻断**引擎自己的结算
> - 引擎侧只有「时机 + 事实」，没有任何结算语义（是否结束状态、按什么概率，全在 Lua）

- `tick_statuses_turn` / `_scene` 前后派发 `LuaMount::Event`（`status_tick` / `turn_end` / `scene_end`），
  上下文带上当前状态实例
- save-ends = Lua 在 `status_tick` 里掷豁免 → `remove_status`
- 专注 = 同一时机 + 伤害事件钩子

---

## 5. 路由表：12 项核对发现各归何处

| 核对发现 | 上一轮（写死） | 改为 |
|---|---|---|
| 优势 / 劣势 | `CheckerDef.advantage` | **Lua**：`ModifyCheck` + ① 注册表 |
| 条件性检定修正 | `ConditionalModifier` | **Lua**：同上（`Condition` 挂载点已有） |
| 熟练 +6 | 新「检定加值」作用面 | **Lua**：`ModifyCheck('add', 6)` + 开放内容挂接（已有） |
| 豁免成功减半 | `EffectDef.on_save` | **Lua**：`ApplyEffect` + `check_post_roll` |
| save-ends | `StatusDef.save_ends` | **Lua**：⑤ tick 时机 + 判定 |
| 专注 | 新字段 | **Lua**：⑤ + ③ |
| XP 合计 | 升为封闭核心 ❌ | **Lua**：④ `ModifyResource` |
| 数据卡继承 | `CharacterDef.extends` | **开放内容挂接**（`attachments`，已实现）+ 带 Lua 的特性定义 |
| 遭遇表 | `EncounterTable` 字段 | **Lua**：`engine_rng` 掷表 + `trigger_event`（已有）+ 事件→遭遇绑定 |
| 伤害类型 | 新概念 | **Lua**：③ + 作者自己的开放内容词汇（`dnd-damage-type`） |
| **突袭 / 战斗第一轮** | 新字段 | **Lua + flag**（本就没有「战斗」概念，靠 flag + AI 叙事） |
| **判定属性** | `SkillDef.attribute` | ⚠️ **仍必须封闭**——见 §6 |
| **对抗判定** | `opposed` 实现 | ⚠️ **仍需引擎结构**——见 §6 |

---

## 6. 什么仍然必须封闭（判据：签名 vs 规则）

### 必须封闭

**A. 判定属性（签名）**
引擎必须知道「用哪个维度的值去算修正」，且 AI 提示词要列「可用判定属性」白名单。
这是**求值入口**，Lua 无法替代（Lua 拿不到「这个体系有哪些属性」）。
→ `CheckerDef.attribute` / `SkillDef.attribute` 保留（作为**参数**，不是「何时用」）。

**B. 对抗判定的结构（签名）**
引擎必须支持「两方比较」这个形状：`opponent = 实例 | 被动值`。
但**对手是谁、用哪个属性、被动基数多少**——由声明 / Lua 给（`passive_base` 已在 `CheckerDef` 里）。
→ 只加「支持 opponent 参数」的结构，不加规则集词汇。

**C. 四个新原语本身**（②③④⑤）——它们是「引擎能做的通用动作」，属于封闭原语，
但**不含任何规则集语义**：引擎不认识「优势」，只认识「掷两次取高」。

### 一句话边界

> 引擎的实现里**不应该出现 `advantage` / `save` / `half` / `proficiency` / `xp` 这类词**。
> 出现即说明规则集语义漏进了引擎。

---

## 7. 一个 D&D 规则包长什么样

> ⚠️ **本节已按 L5 的实测结果改写**：下面三段原本是「设计意图」，其中两段暴露了引擎缺口。
> 现按**真实可运行的写法**给出，缺口单独列出（详见 [LMoP 核对报告的证伪清单](./lmop-design-verification.md) 与
> `story_example/lmop-import.md` 的 14 条 GAP）。

```lua
-- ① 优势：inspired 状态 → 属性检定掷两次取高（真实可运行）
--   注意 check_kind / check_attribute 在 check_pre_roll 里【拿不到】(GAP-D)，
--   所以判据只能用状态/标记，不能用判定签名。
{ id = 'dnd-inspired', mount = 'check_pre_roll', source = [=[
    if host.has_status('dnd-inspiration') then
      host.modify_check('keep_high')   -- 引擎词汇是通用动作；advantage 只是规则包自己的叫法
    end
  ]=] }

-- ② 熟练加值：⚠️ 引擎没有读挂接/开放内容的 API（GAP-C），
--    `host.attachment_bonus` 【不存在】。现实做法是在生成期把规则数据烘进脚本表。
{ id = 'dnd-proficiency', mount = 'check_pre_roll', source = [=[
    local prof = { stealth = 5, perception = 3 }   -- 生成期烘入，非运行期读挂接
    if host.check_kind == 'attribute' and prof[host.check_attribute] then
      host.modify_check('add', prof[host.check_attribute])
    end
  ]=] }

-- ③ 豁免成功伤害减半：⚠️ 引擎不暴露本次已掷出的伤害（GAP-E），
--    只能按同一骰式重掷再取半（期望值对，但不是同一颗骰）。
{ id = 'dnd-save-half', mount = 'check_post_roll', source = [=[
    if host.check_kind == 'save' and host.check_result then
      host.apply_effect(host.target_id, { kind = 'damage', amount = 'half(3d6)' })
    end
  ]=] }
```

**这三段暴露的缺口就是 L5 的价值**：优势能表达，熟练与豁免减半只能在「引擎不提供读口」的情况下**近似**。
正确的结论不是「加个 `proficiency` 字段」，而是「补一个只读的挂接/定义读取原语」与「把本次效果暴露给 post-roll 钩子」。


→ 换一套规则集（COC 的理智检定、无限流的基因锁）= **换一组挂载点脚本 + 开放内容**，引擎一行不动。

---

## 8. 兼容与验收

**兼容**
- 五个原语全是**加性**：旧故事书没有 `lua_mounts` → 行为逐字不变
- `ModifyCheck` 只在 Lua 主动调用时生效；不调用 = 现行为
- 判定路径收敛（把 `check` / `strike` 也接上挂载点）**会改变现有行为**——但这正是修复 D2/D3，需要逐字回归

**验收**
1. **回归**：无 `lua_mounts` 的故事书，判定骰序与提示词逐字不变
2. `check_pre_roll` 挂载点调用 `modify_check('keep_high')` → 骰两次取高（断言 RNG 消耗 = 2）
3. `check_post_roll` 的 `modify_check('add', 6)` → total 加 6、margin 重算、分档重算
4. `apply_effect` 从 Lua 造成伤害 → 与声明式 `ImmediateEffect` 走**同一条** `resolve_effect` 路径
5. `status_tick` 事件 → save-ends 掷豁免成功 → 状态被移除
6. **`Intent::Check` 与 `Strike` 也触发挂载点**（断裂 ① 修复）
7. `lua_lint` 拒绝 `lua_mounts` 里的禁用 API；发布门拦截语法错误
8. 引擎源码 grep `advantage|proficiency|on_save` **零命中**（边界自查）

---

## 9. 里程碑

| 期 | 内容 | 依赖 |
|---|---|---|
| **L1 入口** | `storybook.lua_mounts` · 加载进 `lua_registry` · `lua_lint` 接入发布门 · 编辑器一处源码编辑 | 判定 C1 |
| **L2 判定原语** | `LuaRequest::ModifyCheck` · `CheckPreRoll`/`PostRoll` 真正生效 · 判定路径收敛（= C2）· 骰序回归 | L1 + C2 |
| **L3 效果原语** | `LuaRequest::ApplyEffect` · `LuaRequest::ModifyResource` · 走既有 `resolve_effect` / `apply_delta` | L2 |
| **L4 时机原语** | `status_tick` / `turn_end` / `scene_end` 派发 · save-ends 与专注的 Lua 范式 | L3 |

> **进度**：**L1 / L2 / L3 / L4 均已落地**（`lua_mounts` 注册表 + `ModifyCheck`/`ApplyEffect`/`ModifyResource` + 判定路径与状态 tick 接挂载点）。
> 引擎 276 tests 全绿、禁词零命中。**L5（LMoP 示例规则包）进行中**——它是这套设计的证伪点：
> 若某条 D&D 规则用现有原语表达不了，结论应是「原语不够」而不是「加个封闭字段」。
| **L5 示例规则包** | 用 LMoP 附录 B 的数据卡验证：优势/劣势 · 豁免减半 · 熟练 · XP · 变体怪（挂接 + Lua） | L4 |

**L1 + L2 必须先于一切「把规则写进 Lua」的工作**——否则写了也不生效（断裂 ①②）。

---

## 10. 待确认

1. **五个原语**是否认这个方向？（它们都不含规则集语义，是通用动作）
2. **`storybook.lua_mounts`** 作为规则集的交付形态是否接受？
   它意味着故事书从「纯数据」变成「数据 + 插件」——发布门、存档冻结、校验都要带上源码。
3. **「引擎里不应出现 advantage / save / proficiency / xp 这类词」**是否作为硬规则写进 AGENTS.md？
   这是防止滑坡最有效的一条。
4. 上一轮我加的那些封闭字段（`on_save` / `ConditionalModifier` / `save_ends` / `crit` / `EncounterTable` / XP 升格）
   **全部撤回**，只剩 `SkillDef.attribute` 与「对抗判定结构」两项——确认？