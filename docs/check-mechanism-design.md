# 判定与攻击结算设计

> 触发：攻击判定与判定机制**很严重**。
>
> 核心诊断：引擎里有**三条平行的判定路径**，各自实现同一件事，规则互不一致。
> 最要命的是——**故事书作者无法声明「这个技能用哪个属性判定」**，引擎在三个地方硬编码了力量。
> 连带后果：Lua 判定器的攻击技能在遭遇战里**永远打不中**，`opposed` 模式静默降级成 `gte`。

---

## 0. 结论摘要

| 项 | 现状 | 决定 |
|---|---|---|
| **判定属性** | 三处硬编码 `"str"`，作者无法声明 | **新增可声明**：技能级 → 判定器级 → 全局 → 回落 `str` |
| **判定路径** | 三条平行实现（`run_check` / `execute_skill` / `strike_enemy`） | **收敛成一个内核**，三个入口共用 |
| **对抗判定** | `opposed` 与 `gte` 同分支，**从未实现** | 真正实现：双方各掷一次，`opponent` 落 payload |
| **自然 1 / 20** | 完全没有 | ✅ **已落地（C4）**：引擎**不加字段**；Lua 读骰面后 `ModifyCheck('force_success' / 'force_fail')` 覆盖结果（§6.1） |
| **成功度** | 算了不用（攻击里直接丢弃） | 只**外露**给 Lua / 条件，不硬编码任何倍率 |
| **难度来源** | 三处各一套（12 / `default_dc` / 敌人 AC） | 统一为「目标 AC → `world.check.default_dc` → 12」 |
| **攻击可观测** | ✅ **已落地（C5）**：`strike` / `enemy_strike` 与技能判定一样发 `CheckResult` | 复用前端 CheckCard（骰面 / 修正 / 总值 / 难度同卡） |

> ⚠️ **真实剧本核对补充**（见 [LMoP 核对报告](./lmop-design-verification.md)）——以下五项是主干机制，不是锦上添花：
>
> 🔁 **落点已按 [规则集走 Lua](./rules-via-lua.md) 修正**：规则集语义（何时有优势 / 豁免怎么算 / 熟练几点）
> **一律走 Lua**，不进引擎类型系统。原「加封闭字段」的方案已撤回。
>
| 机制 | 真实出现 | 现状 | 落点（修正后） |
|---|---|---|---|
| **优势 / 劣势** | 25 处 | 类型里**零命中** | **Lua**：`LuaRequest::ModifyCheck` + `storybook.lua_mounts`（引擎只认识「掷两次取高/低」） |
| **豁免成功伤害减半** | LMoP 通用规则 | `effect_applies = !result`（成功＝完全不结算） | **Lua**：`LuaRequest::ApplyEffect` + `check_post_roll` 钩子（复用既有 `ImmediateEffect`） |
| **被动值作为对抗方** | 「被动感知」36 处 | 只能算自己的被动值 | **引擎结构**（签名）：`opponent = 实例 \| 被动值`；被动基数用已声明的 `passive_base` |
| **save-ends 重复豁免** | 4 处 | 无 | **Lua**：`status_tick` 作为 Lua 时机 + 判定 |
| **条件性检定修正** | 怪物特性十余条 | 只有静态属性增量 | **Lua**：`ModifyCheck` + 既有 `Condition` 挂载点 |

> 另：**熟练加值给不出来**（「地精的隐匿技能调整值 +6」）——CONTEXT.md 说熟练走开放内容挂接，
> 但挂接只能给属性维度加值，**给不了「隐匿检定 +6」**。既定的词汇决策与实现能力之间有缺口。

---
## 1. 现状：三条平行路径

| | `Intent::Check`（`session.rs:3413`） | `use_skill`/`use_item`（`session.rs:3007`） | `Intent::Strike`（`session.rs:1888`） |
|---|---|---|---|
| 判定器 | `world.check` 全局 | 技能 `check` → 全局 | 技能 `check` → **裸 `1d20`** |
| 判定器解析 | `command::run_check` | `execute_skill` | **直接调 `resolve::resolve_declarative_check`** |
| Lua 判定器 | ✅ 支持 | ✅ 支持 | ❌ **不支持**（绕过 `resolve_checker`） |
| 属性 | 意图指定 + **白名单校验** | **硬编码 `"str"`** | **硬编码 `"str"`** |
| 属性修正来源 | `profiles` | `profiles` + **`attribute_bonuses`**（挂接/装备） | `profiles`（**无挂接/装备**） |
| 难度 | 意图 → `default_dc` → 12 | **硬编码 12** | 敌人 AC → 12 |
| 成功判据 | `resolved.result`（尊重 mode） | `resolved.result` | **`check.total >= difficulty`**（丢弃 mode/result） |
| 成功度 | 分档并落 payload | 分档并落 payload | **丢弃**（大成功 = 险胜） |
| `CheckResult` 事件 | ✅ | ✅ | ❌ **不发**（UI 看不到骰子） |
| 确认门 | ✅ | ❌ | ❌ |

**结论**：同一个故事书里，「我推他」（`check`）和「我砍他」（`strike`）用的**不是同一套规则**。

---

## 2. 缺陷清单（按严重度）

### 🔴 阻断级

**D1 — 判定属性不可声明，三处硬编码力量**

- `CheckerDef` **没有 `attribute` 字段**（`octopus-types/src/lib.rs:71-105`）
- `SkillDef` **没有 `attribute` 字段**（`lib.rs:264-283`）
- 于是引擎自己填：
  - `resolve_skill`：`attribute: Some("str".to_string())`（`session.rs:3058`），
    注释自认「**技能未声明判定属性，v1 以力量为默认维度**」
  - `strike_enemy`：`let attribute = "str".to_string()`（`session.rs:1950`）
  - `run_check`：由意图给（**唯一正确的一条**）

→ 弓、匕首、法术、撬锁**全部掷力量**。作者**无法表达**「这个技能用敏捷」。
→ `CheckerDef.attributes` 是「`check` 意图的合法属性**白名单**」，被误当成「这个技能用哪个属性」——两件事被混为一谈。

**D2 — 遭遇战里，Lua 判定器的攻击技能永远打不中**

`strike_enemy` 绕过了 `command::run_check`，**直接调 `resolve::resolve_declarative_check`**
（`session.rs:1958-1961`）——**不经过 `resolve_checker`，因此不认 `checker.lua`**。
于是带 Lua 判定器的攻击技能走进 `resolve_declarative_check`：`dice` 为空 → 不掷骰 →
`total = 0 + mod`，再与 AC 比 → **除极端情况外恒为未命中**。

**D3 — `SkillCheck::Ref`（引用全局判定器）在攻击里被丢弃**

`strike_enemy` 的判定器选取（`session.rs:1946-1949`）：
```rust
match skill.check.as_ref() {
    Some(SkillCheck::Def(c)) => c.clone(),
    _ => CheckerDef { dice: Some("1d20".into()), ..Default::default() },  // ← Ref 落到这里
}
```
技能声明 `check: "world"` 时**静默换成裸 1d20**，`world.check` 的骰式 / 修正 / 阈值全部失效。

### 🟠 严重级

**D4 — `CheckMode::Opposed` 从未实现，但编辑器让作者能选**

`resolve.rs:311`：`CheckMode::Gte | CheckMode::Opposed => total >= target`，
注释写着「opposed 由调用方给出对抗方数值后同样比较 >=」——**没有任何调用方**。
`CheckResultPayload.opponent` 字段存在（前端类型也有、CheckCard 会渲染），
但两处 emit 都写死 `opponent: None`（`session.rs:3110` / `3569`）。
而编辑器 `CheckerEditor.vue:52` 把「对抗 (opposed)」列为可选项 → **作者能选一个静默降级成 ≥ 的模式**。

LMoP 里就有对抗判定：「让他们各自进行一次魅力（欺瞒）检定，与怪物们的感知（洞悉）检定对抗」
（`story_example/凡戴尔的失落矿坑.md:1712`）——引擎表达不了。

**D5 — 攻击不参与属性修正来源**

`strike` 用 `effective_attribute`（`session.rs:1951`），而 `execute_skill` 走
`template_id → attribute_bonuses`（挂接定义 + 已装备物品，`command.rs:271-278`）。
→ **装备的属性加值在攻击里不生效**，在技能里生效。`strike` 只有另一套
`choice.bonus`（物品 modifiers 里 `target=attack` 的 add）。

**D6 — 难度来源三处分裂**

`resolve_skill` 硬编码 `12`（`session.rs:3035`）**不读 `world.check.default_dc`**；
`run_check` 读；`strike` 用敌人 AC。同一个故事书里三套难度语义。

**D7 — `resolve_skill` 忽略传入的 actor**

`session.rs:3014-3023`：actor 恒取 `st.controlled.first()`，**忽略参数 `actor`**。
但同函数结尾又用参数 `actor` 发 Resolution（`3127`）→
**结算算在受控角色头上，叙事却可能署名别人**。

**D8 — `resolve_skill` 的目标只按实例键查**

`session.rs:3032`：`st.characters.get(t)` 直接取键，不走 `find_character`（`2131`）。
→ 传模板 id / 角色名**查不到目标**（而 `strike` 走的是遭遇条目，完全另一套寻址）。

### 🟡 一般级

**D9 — 没有自然 1 / 自然 20**：`resolve.rs` 全文无 crit 处理，`level_for_margin` 只看 margin。
d20 + 高修正下「自然 1 也命中」，D&D 类规则集的核心期望落空。

**D10 — 成功度算了不用**：攻击里 `level`（大成功/成功/险胜/失败）被直接丢弃（`session.rs:1989`），
没有「大成功 → 额外效果」的表达能力。

**D11 — 攻击判定在 UI 不可见**：`strike` 只发 `Resolution`（一行文本），
不发 `CheckResult` → 玩家看不到骰面 / 修正 / 总值。

**D12 — `modifier_formula` 声明了但没人读**：`CheckerDef.modifier_formula`（`lib.rs:81-82`，
注释自认「v1 仅支持默认公式」）只在 `validate.rs:1005` 的**允许键白名单**里出现过，
没有任何求值器读它。作者能填，引擎忽略。

**D13 — 武器选择靠文本启发式**：`attack_choice_in_text`（`session.rs:1775`）用 12 个动词 +
最长名字子串匹配来猜玩家用了什么武器。规则判定掺进字符串猜测。

---

## 3. 设计：一个内核，三个入口

`command::execute_skill` **已经是那个内核**：Validate（资源 / 物品）→ 判定
（`resolve_checker`，含 Lua 挂载点）→ 效果（`resolve_effect`）→ deltas + RNG 记账。
它已经在 `resolve_skill` 里被复用。问题只是有两条旁路自己拼判定。

**收敛后**：

| 入口 | actor | target | difficulty | 技能 |
|---|---|---|---|---|
| `check` 意图 | 意图指定（白名单校验） | — | 意图 → `default_dc` → 12 | 合成「纯判定」技能（无 cost / 无 effect） |
| `use_skill` / `use_item` | **意图 / 参数指定**（修 D7） | 参数指定，走 `find_character`（修 D8） | `default_dc` → 12（修 D6） | 技能本身 |
| `strike`（玩家 → 敌人） | 攻方实例 | 守方实例 | 守方派生 AC → `default_dc` → 12 | 攻击技能（无则合成徒手） |
| `enemy_strike`（敌人 → 玩家） | 敌人实例 | 目标实例 | 目标派生 AC | 同上 |

于是 `strike_enemy` 从「平行实现」变成「`execute_skill` 的一个调用」——
这也正是[怪物图鉴设计](./bestiary-design.md) §4.3 那个对称内核 `resolve_attack` 的**正确落点**：
不是新写一个内核，而是**让攻击用上已有的那个**。

---

## 4. 关键新增：判定属性可声明（修 D1）

```ts
export interface SkillDef {
  // …既有字段…
  /** 判定属性维度 key（如 'dex' / 'str'）；缺省回落判定器 / 全局 / 'str' */
  attribute?: string
}

export interface CheckerDef {
  // …既有字段…
  /** 判定属性维度 key；技能未声明时用它 */
  attribute?: string
  /** 对抗判定时对手所用的属性维度 key；缺省同 attribute */
  opposed_attribute?: string
}
```

**解析优先级**（从具体到一般）：

```
① SkillDef.attribute                      技能声明（「这把匕首用敏捷」）
② SkillDef.check（内联 Def）.attribute     判定器声明
③ world.check.attribute                   全局缺省判定属性（新字段）
④ "str"                                   现状回落（向后兼容）
```

**校验**：声明的 `attribute` 必须在 `attribute_dimensions` 里，否则 Error
（与 `check` 意图白名单同一套错误码风格）。→ 拼错立刻可见，而不是静默 0 分。

---

## 5. 对抗判定（修 D4）

`opposed` 的真实语义是**双方各掷一次比大小**，而不是「主动方 vs 一个静态数值」。

- `Intent::Check` 加 `opponent_id?: string`：有对手 = 对抗，引擎掷两次。
- 对手用 `opposed_attribute`（缺省与主动方同属性）；对手的属性值走 `effective_attribute` + 修正来源。
- `CheckResultPayload.opponent`（**字段已存在**）填 `{ id, name }`；
  前端 CheckCard 已能渲染，只差数据。
- 对抗的 `target` = 对手 `total`，`margin` = 己方 `total` − 对手 `total`，分档照旧。
- 若 `mode: opposed` 但**意图没给对手** → 明确驳回（`RuleViolation`），不再静默降级。

---

## 6. 骰面特殊与成功度

### 6.1 自然 1 / 自然 20（修 D9）—— **走 Lua，引擎只给通用覆盖动作**

⚠️ **本节已按 [规则集走 Lua](./rules-via-lua.md) §6 修正**：`CheckerDef.crit` 这种字段是把
规则集语义塞进引擎类型系统，**已撤回**。引擎不认识「自然 1 / 自然 20 / 大成功」——
它只提供两个**通用结果覆盖动作**（`LuaRequest::ModifyCheck`）：

```lua
-- check_post_roll：规则包自己读骰面，自己决定何时覆盖
if host.check.rolls and host.check.rolls[1] == 1 then
  host.modify_check('force_fail')      -- 引擎词汇：强制失败（最低档）
elseif host.check.rolls and host.check.rolls[1] == 20 then
  host.modify_check('force_success')   -- 引擎词汇：强制成功（至少中档）
end
```

- `force_fail` → `result = false`、成功度落到最低档（`Fail`）；
- `force_success` → `result = true`、成功度**至少**中档（`Success`；已是更高档则保持）——
  自然 20 掷出的高差值本来就落在最高档，`force_success` 保持它，即「强制大成功」；
- **骰面 / 总值 / 差值 / 难度一律不动**：覆盖的是「这一次算不算成功」，不是重写骰子。

「什么时候算自然 1/20」全在规则包 Lua；缺省（没有脚本调用）行为逐字不变。


### 6.2 成功度只外露，不硬编码（修 D10）

`level`（大成功 / 成功 / 险胜 / 失败）已经在 `ResolvedCheck` 里算出来了，
只是攻击路径丢掉了。修 `D2/D3` 之后它自然回流。**引擎不硬编码任何倍率**，而是外露给：
- **Lua 上下文**：`LuaHostContext` 加 `check_level` / `check_margin`，钩子自行分档
- **条件表达式**：`CondExpr` 加 `{ op: 'success_level', level: 'great' }`，
  让 `EffectDef.triggers` 能按成功度触发额外效果

这样「大成功伤害翻倍」是**故事书的声明**，不是引擎的硬编码。

### 6.3 `modifier_formula`（修 D12）—— **已实现**

**决定：实现**（不是移除）。作者已经能填这个字段，静默忽略是最差的选择；
`derived.rs` 的表达式求值器与派生值同语法，复用成本低。

落地口径（`resolve::modifier_for`）：

- 优先级不变：`attribute_modifier` 固定映射 → `modifier_formula` → 缺省中心偏移公式；
- 可用变量：`v`（**夹取后**的属性值，min / max 已生效）、`value`（原始值）、`baseline` / `step`（该维度声明值）；
- 结果仍**夹到该维度的修正范围**（由 min / max / baseline / step 推出）；
- 求值失败（语法错误 / 未知变量）运行期静默回落到缺省公式——但**发布门会把非法公式拦成
  Error**（`modifier_formula_invalid` / `modifier_formula_unknown_var`），不靠运行期兜底。

于是 D&D 的 `floor((v - 10) / 2)` 直接算得出来（配 `attribute_dimensions` 的
baseline 10 / modifier_step 2 与缺省公式同值，LMoP 故事书的结果逐字不变）。

### 6.4 触发点 `repeatable`（骨架语义）—— **已落地**

`skeleton[].scenes[].triggers[].repeatable`（CONTEXT.md「剧情触发点」：默认一次性、可设可重复）
此前只在种子数据里出现、引擎从不读，于是 LMoP 掷表规则包的 24 行表项整局各触发一次。

落地语义（`conditions::evaluate_skeleton_full`）：

- **默认一次性**：满足即触发一次并永久标记——判定与落库值（`progress.triggers[id] = true`）
  与改动前**逐字一致**，delta 也是同一条（`trigger` + `fired` + `Set`）；已触发后**连条件都不再求值**
  （Lua 条件脚本不会被反复执行，与改动前的求值顺序一致）；
- **`repeatable: true`**：条件**由假变真（边沿）**时再次触发；条件持续为真期间不重复；
  进度落 `{ "fired": true, "active": <本次条件真假> }`——`active` 就是边沿依赖的「上一次值」，
  随 delta 落进命令日志，重放（纯投影）得到同一结论，不靠重跑条件；
- 进度只在真的变化时产生 delta（状态恒定不膨胀事件流）；
- 发布门校验 `repeatable` 必须是布尔（`invalid_trigger_repeatable` Warning），
  避免 `"yes"` 被按 false 静默读成一次性。

⚠️ **LMoP 掷表仍只出一次**：边沿要求标记能回到假，而规则包目前**没有清标记原语**
（`set_flag` 只置真）——这是 [LMoP 导入说明](../story_example/lmop-import.md) GAP 6 的另一半，
**原样报告，未新增封闭字段**：`repeatable` 已实现，但要真正反复出遭遇，还需规则包能重新拉低条件。

---

## 7. 可观测（修 D11）—— ✅ 已落地（C5）

`strike` / `enemy_strike` 与技能一样发 `CheckResultPayload`：
`actor`（攻方署名）· `attribute` · `expr` · `rolls` · `mod` · `total` · `target` · `margin` ·
`result` · `level` · `kind: attack`。前端 `CheckCard` 直接复用，攻击从此**能看到骰子**，
与判定卡一致。

事件顺序与技能判定一致：**先 `CheckResult`、后 `Resolution`**（Resolution 的叙事文案逐字不变，
只是多了一张可复用的判定卡）。

---

## 8. 兼容与验收

**兼容**
| 面 | 兼容性 |
|---|---|
| `SkillDef.attribute` / `CheckerDef.attribute` | 全可选，缺省走 `str` 与现行为 |
| `modifier_formula` / `repeatable` | 已落地；不声明时行为逐字不变（见 §6.3 / §6.4） |
| `ModifyCheck` 结果覆盖 | 只在 Lua 主动调用时生效；不调用 = 现行为 |
| 收敛判定路径 | 修的是**已坏的**行为（D2/D3/D5/D7/D8）；正常路径的骰序需逐字比对回归 |
| `opposed` 明确驳回 | 之前静默降级成 `gte`；给作者的是**显式错误**而非错误结果 |
| RNG 消耗 | 收敛后攻击与技能走同一条掷骰路径 → **rng_consume 记账必须逐字核对**（命令日志重放的骰序连续性） |

**验收**
1. **回归**：现有 `session.rs` 判定测试全绿；`check` 意图的提示词与骰序逐字不变
2. 技能声明 `attribute: 'dex'` → 用敏捷掷骰（构造 dex≠str 的角色，断言修正变了）
3. 带 Lua 判定器的攻击技能 → 走进 `resolve_checker`，Lua 被执行（现为永久未命中）
4. `check: "world"` 的技能 → 用上 `world.check` 的骰式与阈值（现为裸 1d20）
5. 装备的属性加值在攻击里生效（与 `use_skill` 结果一致）
6. `opposed` + 对手 → 掷两次、`opponent` 落 payload、前端渲染；
   `opposed` 无对手 → 明确驳回
7. ~~`crit.success_on: 20`~~ **已改为 Lua 路由**：`check_post_roll` 读 `host.check.rolls` 后
   `modify_check('force_success')` / `('force_fail')` → 结果与档位随之变；不调用时行为不变
8. 角色名 / 模板 id 作为 `target_id` 能命中（现为查不到）
9. ✅ `strike` / `enemy_strike` 发 `CheckResult`，前端能看到骰面（本轮 C5 落地）
10. `./scripts/cache-audit.sh`：提示词前缀不变（判定改动不得污染系统层）

---

## 9. 里程碑

| 期 | 内容 | 依赖 |
|---|---|---|
| **C1 判定属性可声明** | `SkillDef.attribute` · `CheckerDef.attribute`/`opposed_attribute` · `world.check.attribute` · 消除三处硬编码 · 校验 | — |
| **C2 一个内核** | `strike_enemy` 改为走 `execute_skill` · 修 D2（Lua 判定器）· D3（`Ref` 全局判定器）· D5（修正来源）· D6（难度统一）· D7（actor）· D8（target 寻址） | C1 |
| **C3 对抗判定** | `opposed` 真实现 · `Intent::Check.opponent_id` · `opponent` 落 payload · 前端渲染 · 无对手时显式驳回 | C2 |
| **C4 骰面与成功度** | ✅ **已落地**：判定细节进 Lua 上下文（`host.check*`）· `ModifyCheck` 通用**结果覆盖**（`force_success` / `force_fail`，替代撤回的 `crit`）· `modifier_formula` **已实现**（§6.3） | C2 |
| **C5 可观测** | ✅ **已落地**：`strike`/`enemy_strike` 发 `CheckResult` · 攻击判定卡与技能判定卡一致 | C2 |

**C1 + C2 必须先于[怪物图鉴](./bestiary-design.md) M2**——否则那个「对称攻击内核」会把
`str` 硬编码与绕过 Lua 的结构原样复制到怪物打玩家的路径上，把缺陷翻倍。

---

## 10. 待确认

1. **判定属性优先级**（技能 → 判定器 → 全局 → `str`）是否接受？
2. **`opposed` 无对手时显式驳回**——之前静默降级成 `gte`。改成报错会让**现有故事书**里
   声明了 opposed 的技能开始报错。接受吗？（我建议接受：静默给错误结果更糟）
3. **自然 1/20 由故事书声明**（引擎不预设）——还是要在引擎里给一个全局开关键？
4. **成功度只外露不硬编码**（不内置「大成功翻倍」）是否接受？
5. ~~`modifier_formula`：实现，还是从编辑器移除？~~ **已决：实现**（§6.3），并加发布门校验。
