# 时序与行动经济设计（Turn & Action Economy）

> 触发：LMoP 多轮独立验证反复登记的 **GAP-I —— 引擎没有先攻 / 轮次 / 行动经济，「没有『回合』可以失去」**，
> 且被标为**超出规则包范围**：现有 Lua 原语全是「判定修正类」，一条时序 / 预算原语都没有，
> 所以任何规则包都写不出「失去一个回合」。
>
> 定位：这是 **14 条 GAP 里唯一一条规则包无法绕过**的（其余都能用标记 + 叙事层近似）。
> 同时它顺手关掉 **GAP-M**（无 `encounter_active` 条件）——「在战斗中」正是时序层的副产物。
>
> ⚠️ **硬约束**（改数据模型前先读）：① 所有新状态必须经 `apply_delta` 走**唯一变更路径**，
> 否则违反事件溯源与重放确定性；② 时序状态**只能进用户消息**，绝不进系统 preamble
>（AGENTS.md 上下文缓存不变量：先攻序 / 当前行动者每回合都变，是前缀缓存头号杀手）。

---

## 0. 结论摘要

| 层 | 决定 |
|---|---|
| **世界声明** | `storybook.world.turn`（对标既有 `world.check`）——**缺省不声明 = 现状逐字不变** |
| **顺序** | `order: none \| initiative \| fixed`；`none` 等于今天（AI 自由宣告），D&D 声明 `initiative` |
| **预算** | `budgets: [{id, amount}]`——`action / bonus / reaction / movement` 只是**作者起的名字**，引擎不认识语义 |
| **运行时** | `WorldState.turn: TurnState`（轮次 / 顺序 / 指针 / 每角色剩余预算 / 跳过次数），经 `DeltaDomain::Turn` 落 delta |
| **闸门** | 机械意图校验「是不是你的回合」+「预算够不够」，驳回码 `not_your_turn` / `insufficient_budget` |
| **推进** | 新增 `Intent::EndTurn`（区别于 AI 循环信号 `FinishTurn`）+ 引擎在预算耗尽时自动推进 |
| **「失去回合」** | 引擎提供 `host.skip_turn(actor, n)` / `host.set_budget(...)` 写原语，规则包在 `turn_start` 挂载点调用 |
| **Lua 只读** | `host.turn`（轮次 / 顺序 / 当前行动者 / 各角色剩余预算） |
| **新增原语** | 只有 `DeltaDomain::Turn` + `SkillDef.budget` + 两个驳回码 + 两个 Lua 写请求；**不新增任何规则集语义** |
| **兼容** | 纯加性：旧故事书、旧命令日志、未声明 `world.turn` 的存档行为全部不变，**无需 `upcast`** |

**一句话**：引擎提供「谁先手、这回合还能做几次」的**通用容器**，容器里装什么预算、叫什么名字，由故事书声明。

---

## 1. 现状与缺口

| 事实 | 位置 |
|---|---|
| `round` 只是「玩家输入一次」的全局计数，不是「谁的第几回合」 | `session.rs` `run_round` 开头 `self.round.fetch_add(1, …)` |
| 遭遇**只建结构与 HP**，先攻与回合限制是显式后置项 | `session.rs`（`// 3a：只建结构与 HP；先攻与回合限制见后续`）、`octopus-types/src/lib.rs:967`（`EnemyView` 同款注释） |
| `initiative` 全仓唯一命中是一段测试数据字符串 | `validate.rs:2948`（`"initiativeBonus": 2`） |
| 敌人行动靠 AI 自己决定去调 `enemy_strike`，不经引擎调度 | `session.rs` `strike_enemy` / `enemy_strike` 均只由意图触发 |
| `CondExpr` 只有 `encounter_cleared`，**没有 `encounter_active`**（取反也得不到「有遭遇」） | `octopus-types/src/lib.rs:135` |
| 一个回合内所有意图**顺序全结算**，没有每回合次数上限 | `session.rs` `run_round` 的意图 for 循环 |
| 确认门曾宣称「会消耗一次行动机会」而引擎并不追踪 | 已修：现为「判定会立即结算，成败写入世界状态」 |

**已具备、可直接复用**（不自造第二套）：

- **回合边界 tick 已存在**：`tick_statuses_turn` / `tick_statuses_scene`，含 `turn_end` / `scene_end` Lua 挂载点。
- **恢复时机已落地**：`per_turn` / `per_scene` 在对应边界生效（`recovery.rs` 的 `TickKind` + `plan_tick`）。
- **冷却记账已落地**：`WorldState.cooldowns` + `cooldown.<skill_id>` delta + `CooldownActive` 驳回，且随命令日志重放。
- **确定性 RNG 与消耗落账已具备**：先攻掷骰走 engine RNG 即自动落 `rng_consume`，重放不分叉。
- **资源边界夹取已落地**：`emit_raw` 在 delta **进日志前**改写越界值，重放逐字一致。

---

## 2. 术语（需登记进 CONTEXT.md）

**回合边界 (Turn Boundary)**:
玩家一次输入产生的结算单位，也是状态 tick 与恢复时机的触发点。**与「时序回合」不是一回事**：前者是「一次输入」，后者是战斗中「某个角色的第几次行动」。
_Avoid_: 轮次（易与战斗轮混淆）

**时序 (Turn Order)**:
战斗 / 冲突场景中决定「谁先行动、每个角色能行动几次」的通用容器——顺序 + 行动预算 + 当前指针；由故事书声明，引擎只负责轮转与校验。
_Avoid_: 先攻系统（那只是 `order: initiative` 的一种取值）、回合制（暗示必须有回合）

**行动预算 (Action Budget)**:
时序中分配给某个角色一个回合内的可消耗计数（如 action / bonus / reaction / movement）；**名字与数量由故事书声明**，引擎只做「够不够、扣多少」。
_Avoid_: 行动点（暗示单一数值）、AP

**失去回合 (Lost Turn)**:
角色在若干时序回合内被禁止行动（突袭 / 昏迷 / 定身等）——由规则包在时序挂载点把该角色的预算清零或跳过；引擎提供原语，不判断何时该失去。
_Avoid_: 眩晕（那是状态，不是时序后果）

---

## 3. 世界声明：`world.turn`

```jsonc
// storybook.world.turn —— 整块缺省 = 现状（无先攻、无预算、AI 自由宣告）
{
  "order": "initiative",              // none | initiative | fixed
  "initiative": {
    "dice": "1d20",                   // 缺省 1d20；走既有骰式解析
    "attribute": "dex",               // 加到骰上的属性维度（走既有 modifier 口径）
    "fixed": { "mon-goblin": 18 }     // 可选：按模板 id 指定固定先攻（胜过掷骰）
  },
  "budgets": [                        // 空 / 缺省 = 不限制次数（只有顺序）
    { "id": "action",   "amount": 1 },
    { "id": "bonus",    "amount": 1 },
    { "id": "reaction", "amount": 1 },
    { "id": "movement", "amount": 30 }
  ],
  "reset": "per_turn",                // per_turn（缺省）| per_round
  // 哪些机械意图消耗哪个预算；缺省 = 不消耗（只受顺序约束）
  "intent_budget": { "strike": "action", "use_skill": "action", "check": "action", "move": "movement" }
}
```

**为什么放在 `world` 下**：与 `world.check`（判定器）、`world.resources` 同一层——都是「故事书对通用机制的参数化声明」，
引擎读它、但不预设它一定存在。`order: "none"` 与「整块不声明」都等于今天。

**技能侧**：`SkillDef.budget: [{ budget, amount }]`，与既有 `SkillDef.cost: [{resource, amount}]`
**对称**（一个是扣资源、一个是扣行动预算）。未声明时按 `world.turn.intent_budget` 的缺省口径扣。

---

## 4. 运行时状态：`WorldState.turn`

```rust
/// 时序运行时状态（#GAP-I）。全部经 DeltaDomain::Turn 的 delta 变更，随命令日志重放。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// 战斗轮（与全局 round 不同：全局 round 是「玩家输入次数」）。
    pub round: u32,
    /// 先攻排序后的角色实例键（降序）；order = none 时为空。
    pub order: Vec<String>,
    /// 当前行动者在 order 中的下标。
    pub index: usize,
    /// 实例键 → 预算 id → 剩余（reset 时按声明重置）。
    pub budgets: BTreeMap<String, BTreeMap<String, i64>>,
    /// 实例键 → 还要跳过几个时序回合（「失去回合」的载体）。
    pub skip: BTreeMap<String, u32>,
    /// 是否处于时序中（遭遇存在且声明了 world.turn）。
    pub active: bool,
}
```

`WorldState` **不是** ts-rs 导出类型（只在检查点 / 快照里序列化），所以加字段只影响
`#[serde(default)]` 的向后兼容，**不动前端绑定**。`DeltaDomain` 是导出类型，新增 `Turn` 变体
需按既有流程重生成绑定（`octopus-types` 的 ts-rs export），前端 `applyDeltas` 的 if/else 链不认的域自动忽略。

**进入 / 退出的判据**（不新增声明）：`world.turn` 已声明 **且** 当前场景存在未结束的遭遇
（`state.encounters` 非空且敌人未全灭）→ `active = true`。遭遇清空 → 退出时序，指针与预算保留但不参与校验。

---

## 5. 引擎闸门与推进

**闸门**（`handle_intent` 内，机械意图之前）：
1. `active` 为假 → 不校验（现状行为逐字不变）。
2. 意图的行动者 ≠ `order[index]` → 驳回 **`not_your_turn`**（新码）。
3. 该角色处于 `skip` → 驳回 **`not_your_turn`**（文案说明「本轮失去回合」）。
4. 预算不足 → 驳回 **`insufficient_budget`**（新码，与既有 `insufficient_resource` 同构、紧邻实现）。

**推进**：`Intent::EndTurn`（新意图，**区别于 AI 循环信号 `FinishTurn`**——后者是工具循环控制，
见 `session.rs` 的「循环终止信号：不算意图」注释）或当前行动者预算耗尽 → `advance_turn`：
扣掉 `skip`、下标 +1、越界则 `round += 1` 并按 `reset` 重置全部预算。

**驳回回喂**：`not_your_turn` / `insufficient_budget` 与既有驳回一样进 `last_rejection` 记账，
由 `run_round` 回喂给模型——否则模型看不到叙事里不存在的驳回，会把失败写成成功。

---

## 6. 「失去回合」怎么表达（GAP-I 的正解）

规则包在时序挂载点声明即可，**引擎不判断何时该失去**：

```lua
-- lua_mounts: { mount = "turn_start" }
-- 突袭：被突袭者在战斗第一轮失去其回合
if host.has_status(host.target_id, "dnd-surprised") and host.turn.round == 1 then
  host.skip_turn(host.target_id, 1)          -- 引擎原语：跳过 1 个时序回合
end
-- 等价写法：把该角色本轮预算清零
-- host.set_budget(host.target_id, "action", 0)
```

现状（近似）：只落 `dnd-surprised` 状态 + `dnd-battle-round-1` 标记，**由叙事层落实**——
这正是「没有『回合』可以失去」的含义。本设计让「失去回合」成为一个**引擎事实**。

---

## 7. 与既有机制的关系

| 机制 | 关系 |
|---|---|
| 状态 tick | 时序回合推进到新一轮时调用既有 `tick_statuses_turn`（**不新增第二套 tick**） |
| 恢复时机 | `per_turn` 已有语义 = 回合边界；时序里的「一轮」是否触发它，由 `world.turn` 存在与否决定（默认沿用现状） |
| 冷却 | 冷却按**全局 round** 计（已实现）。若作者要「按战斗轮」，用 `world.turn` 的轮次号即可，不改冷却口径 |
| 遭遇 | 遭遇存在 = 时序激活；遭遇清空 = 退出。`CondExpr` 顺带补 `EncounterActive {}`（**关掉 GAP-M**） |
| 上下文缓存 | 时序状态（先攻序 / 当前行动者 / 剩余预算）**只进用户消息最后一条**，绝不进 preamble |
| 重放 | 先攻掷骰走 engine RNG → 自动落 `rng_consume`；顺序向量与指针经 `DeltaDomain::Turn` 落 delta → 重启后逐字复原 |

---

## 8. 校验（validate.rs，发布门）

`world.turn` 缺失 → 不校验（旧故事书零影响）。存在时报错级：

- `order` 不在 `none|initiative|fixed`；`order: initiative` 但 `initiative.attribute` 不命中已声明的属性维度。
- `budgets[].id` 为空 / 重复；`amount` 非正数。
- `intent_budget` 的值引用了未声明的预算 id；`SkillDef.budget[].budget` 同上（悬空引用）。
- `initiative.dice` 不可解析为骰式。

---

## 9. 兼容与升格

- **无 `upcast`**：`WorldState.turn` 带 `#[serde(default)]`，旧快照 / 旧检查点 / 旧日志直接可用。
- **行为逐字不变**：未声明 `world.turn` 或 `order: none` 时，闸门全部短路，事件流与今天逐字一致
  （验收方式：现有 361 条引擎测试 + 6 条 GAP 复现用例必须全绿）。
- **Lua 旧脚本**：`host.turn` 在非时序下为 nil，老脚本读它得到 nil（与既有 `host.event_name` 同口径），不会报错。

---

## 10. 验收计划

| # | 用例 | 判据 |
|---|---|---|
| 1 | 未声明 `world.turn` | 行为逐字不变（现有全套测试 + 快照 / 重放一致） |
| 2 | `order: initiative` 建遭遇 | 顺序确定、`rng_consume` 落账；**重放后顺序与指针逐字一致** |
| 3 | 非当前行动者发机械意图 | `not_your_turn` 驳回，且**回喂给模型** |
| 4 | 预算耗尽后再行动 | `insufficient_budget`；`EndTurn` 后按 `reset` 重置 |
| 5 | `fixed` 先攻 | 按模板 id 的固定值排序，不掷骰、不消耗 RNG |
| 6 | `skip_turn`（突袭） | 被跳过的角色本轮机械意图全被驳回；下一轮恢复 |
| 7 | `encounter_active` 条件 | 有遭遇时成立、清空后不成立（GAP-M 关闭） |
| 8 | 上下文缓存 | 时序状态只出现在用户消息，preamble 逐字节稳定（`scripts/cache-audit.sh`） |

---

## 11. 明确不做（非目标）

- **网格 / 距离 / 射程 / 掩体 / 借机攻击** → 属**空间量化**（GAP-B），本设计只做时序，不碰位置。
- **具体规则集语义**：不内置「一轮 = 6 秒」「action 是标准动作」「先攻并列如何打破」——全部由故事书 / Lua 表达。
- **反应动作的触发时机**：本设计只提供 `reaction` 这个**预算名字**；「什么时候能花」由规则包自己判断（`host.turn` 只读口足够）。
- **战斗演出模板**：纯前端（GAP #19），与时序无关。

---

## 12. 待你拍板的决策

1. **进入时序的判据**：用「遭遇存在」自动激活（推荐，顺手关掉 GAP-M），还是要求显式 `world.turn.enter` 条件？
2. **`EndTurn` 的形态**：新增独立意图（推荐，语义干净），还是把既有 `FinishTurn` 一分为二（它现在是 AI 循环信号，混用即 bug）？
3. **预算缺省**：`budgets` 声明了、但某技能 / 意图没声明消耗 → 扣第一个预算（简单），还是**不扣**（更保守、作者必须显式，推荐）？
4. **`DeltaDomain::Turn`**：新增域（推荐，语义清晰，需重生成 ts-rs 绑定），还是复用 `Character` 域 + `turn.*` 字段前缀（零绑定改动，但语义浑浊）？
5. **本批范围**：只落 `order: none|initiative|fixed` + 预算 + 闸门 + 失去回合（推荐，能关 GAP-I/M），
   还是连「`per_turn` 恢复改挂到时序轮」一起收（会改变既有 `per_turn` 语义，建议单独一批）？
