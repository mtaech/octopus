# 怪物图鉴设计（Bestiary）

> 触发：故事书缺「怪物图鉴」。怪物不是新机制，是**人物模板的受限变种**。
>
> 拍板（5/5）：① 走 A 方案——`kind:'monster'` 扩 `characters[]`；② 遭遇里的敌人是**真实例**；
> ③ 玩家打怪 / 怪打玩家**都由引擎结算**；④ 编辑器复用人物面板（面板**改名**）；
> ⑤ 导入 LMoP 附录 B 作为第一份真实图鉴数据集。
>
> 相关：[地图 · 地点 · 在场（实景）设计](./map-and-presence-design.md)——怪物实例的**位置**由那份额定（出没地 + 遭遇发生地点）。
>
> ⚠️ **依赖**：[判定与攻击结算设计](./check-mechanism-design.md) 的 C1 + C2 **必须先做**——否则 §4.3 的对称内核会把
> 「硬编码力量」「绕过 Lua 判定器」原样复制到怪物打玩家的路径上。

---

## 0. 结论摘要

| 层 | 决定 |
|---|---|
| **图鉴条目** | `storybook.characters[]` 里 `kind:'monster'` 的条目 = 模板；额外挂一个**展示型** `statblock` 块 |
| **怪物数值** | 全部落在既有封闭原语上，**不新增机制**：六维→`attributes` · AC→`derived` · HP→`resources.hp` · 攻击→`skills`(check + damage) · 状态→`statuses` |
| **展示型词汇** | 生物类型 / 阵营 / 感官 / 语言 / 伤害免疫 / CR / 特性文本 → `statblock` 自由文本或开放内容 `kinds`，**引擎不读** |
| **运行时** | 图鉴条目**不在开档时实例化**；遭遇创建时按模板**克隆**出遭遇专属实例，进 `st.characters` |
| **引擎新增** | 只有两件事：① `Character` 域支持插入完整实例的 delta；② `strike_enemy` 抽成对称的 `resolve_attack` |
| **兼容** | 纯加性：旧故事书、旧命令日志、AI 现编的临时敌人行为全部不变，无需 `upcast` |

**一句话**：图鉴是模板，遭遇是实例，攻击是同一个内核的两个方向。

---

## 1. 现状与缺口

### 1.1 怪物今天不是故事书实体

| 事实 | 位置 |
|---|---|
| `kind` 被校验死锁在 `'pc' \| 'npc'` | `crates/octopus-engine/src/validate.rs:287` |
| 遭遇是纯运行时产物，AI 现编 | `Intent::Encounter` → `session.rs:2790` |
| 敌方单位只有 `{name, hp?, ac?}`，缺省 hp=10 / ac=12 | `octopus-types/src/lib.rs:765`、`session.rs:2797` |
| `strike` 只用到敌人两个字段：AC 当难度、hp 当血条 | `session.rs:1888` |
| 敌人不在 `st.characters` → `Status` / `Adjust` / `Check` 够不着它 | `session.rs:3740`（Character 域只改已存在的实例键） |
| CONTEXT.md 连「遭遇 / 敌人 / 怪物」三个词都没登记 | — |
| 怪物打玩家**完全没有引擎结算**：只有玩家打怪走 `strike`，怪打玩家靠 AI 叙事 | `session.rs:2786` 是唯一攻击入口 |

后果：同一种地精这次 AC 12、下次 AC 15；数据卡不能复用、不能预先设计、玩家看不到图鉴；「给那只地精上中毒」表达不出来。

### 1.2 数据源其实现成

`story_example/凡戴尔的失落矿坑.md` 附录 B（第 2500 行起）有 30 条完整数据卡，例如灰烬丧尸
（第 2535–2562 行）：`AC 8` / `HP 22(3d8+9)` / 六维 `13,6,16,3,6,5` / 伤害免疫毒素 / CR 1/4 (50 XP)。
而 `story_example/lmop2md.js` 本来就是从 5e bestiary JSON 生成的——数据是结构化的，
只是进了故事书之后全变成不可结算的正文。

---

## 2. 术语（CONTEXT.md 待新增词条）

> 现有 CONTEXT.md **没有**遭遇 / 敌人 / 怪物的任何词条。以下为拟新增，插在「人物」词条附近。

**怪物 (Monster)**:
故事书里定义的敌对生物**模板**，是人物模板的受限变种（`kind: 'monster'`）：只承载引擎要**结算**的数据
（属性 / 派生 AC / 资源 HP / 攻击技能 / 状态），不承载人格档案，不参与对话扮演。
_实现_: `storybook.characters[]` 中 `kind:'monster'` 的条目；展示型数据卡字段在 `statblock`（引擎不读）。
_Avoid_: 敌人（遭遇里的那一只才是敌方单位）、NPC（怪物不上场对话）

**图鉴 (Bestiary)**:
故事书里全部 `kind:'monster'` 人物模板的集合及其编辑视图。
_Avoid_: 怪物表、Monster Manual（外部格式名）

**遭遇 (Encounter)**:
运行时由导演创建的结构化战斗场景——一组**敌方单位**，每个单位绑定一个怪物**实例**。
_实现_: `Intent::Encounter` → `WorldState.encounters`（`EncounterView`）；`DeltaDomain::Encounter`。
_Avoid_: 战斗、战场（遭遇是数据结构，不是战斗流程）

**敌方单位 (Enemy)**:
遭遇内被寻址的一只敌人（`e1` / `e2`…），背后是一个**怪物实例**（`CharacterInstance`，`kind:'monster'`）；
无图鉴引用的临时敌人没有实例，仅存于遭遇条目内。
_Avoid_: 怪物（怪物是模板）

**角色库 (Character Library)**（编辑器面板名，替代原「人物」）:
编辑器中人物模板与怪物模板的统一入口，按 `kind`（玩家角色 / NPC / 怪物）筛选与编辑。
_Avoid_: 人物（只涵盖人形角色，怪物放进去名不副实）、角色（单字，易与运行时「角色实例」混淆）

---

## 3. 数据模型

### 3.1 故事书侧

```ts
// frontend/src/types/index.ts
export interface CharacterDef {
  id: string
  name: string
  kind?: 'pc' | 'npc' | 'monster'   // ← 扩一个值
  // …既有字段不变…

  /** 怪物数据卡（kind='monster'）：**展示型**字段，引擎永不读取，也不注入提示词。
   *  与 notes 同性质——是给人看的，不是给引擎算的。 */
  statblock?: MonsterStatblock
}

export interface MonsterStatblock {
  /** 生物类型与阵营，如「中型 不死生物，中立邪恶」 */
  creatureType?: string
  /** 数据卡正文：特性 / 感官 / 语言 / 伤害免疫 / 状态免疫等自由文本 */
  traits?: string
  /** 挑战等级与经验，如 "1/4" / 50 */
  challenge?: string
  xp?: number
  /** 攻击动作的文字描述（真正结算走 skills；这里只给玩家看） */
  actionsNote?: string
}
```

### 3.2 封闭核心 vs 开放词汇的分界线

这是本次设计**唯一需要判断力**的地方，判据照抄 `docs/open-kinds-proposal.md`：
「一个概念只有在**引擎必须求值或改写它**时才升格为封闭核心」。

| 数据卡字段 | 引擎要求值？ | 归属 |
|---|---|---|
| 六维（力量/敏捷/…） | ✅ 判定用 | 封闭：`attributes` + `attribute_dimensions` |
| 属性调整值（+1 / −2） | ✅ 判定修正 | 封闭：`derived` 公式（`floor((str-10)/2)`） |
| 护甲等级 AC | ✅ 攻击难度 | 封闭：`derived.ac`（`derived` 词条原文就点名了 AC） |
| 生命值 HP | ✅ 扣血 | 封闭：`resources.hp`（`world.resources` 声明） |
| 攻击动作 | ✅ 命中 + 伤害 | 封闭：`skills[]`（`check` + `effect.immediate[damage]`） |
| 状态免疫 | ✅ 拦截施加 | 封闭：状态 id 列表（**M2 后置**，见 §9） |
| 生物类型 / 阵营 | ❌ 只展示 | 开放：`statblock.creatureType` |
| 感官 / 语言 / 速度 | ❌ 只展示 | 开放：`statblock.traits`（或开放内容 `kinds`） |
| 伤害免疫 / 抗性 | ⚠️ **无「伤害类型」概念**（拒绝理由曾是循环论证，见下） | 开放：`statblock.traits` + **登记为待评估新概念** |
| 挑战等级 | ❌ 只展示 | 开放：`statblock.challenge` |
| **XP** | ✅ **要合计**（击败的怪物 XP 累加） | **Lua**：`LuaRequest::ModifyResource`（见 [规则集走 Lua](./rules-via-lua.md)） |

**结论**：新增的封闭字段只有 `kind` 的一个枚举值——连 AC 和 HP 都不用新字段，
它们分别落在早已为此准备好的 `derived` 和 `resources` 上。

> ⚠️ **真实剧本核对修正**（见 [LMoP 核对报告](./lmop-design-verification.md)），
> 并已按 [规则集走 Lua](./rules-via-lua.md) 改走**原语 + Lua**，不新增封闭字段：
> - **XP 判错了**：原先归「引擎不读」是错的——LMoP 85 处「经验值」，且要求
>   「合计角色们克服的每个怪物的经验值」（第 1987 行）。
>   → 用 `LuaRequest::ModifyResource` 在「敌人被击败」事件里累加；**XP 不进引擎类型系统**。
> - **伤害免疫的理由是循环论证**：「没有伤害类型概念所以不做」——没做所以没概念。
>   → 走 `LuaRequest::ApplyEffect` + 作者自己的开放内容词汇（如 `dnd-damage-type`），引擎不认识「毒素」。
> - **数据卡继承不新增 `extends`**：LMoP 的「使用丧尸资料卡，并具有以下额外特性」（第 1433 行）
>   用**开放内容挂接**（`attachments`，已实现）表达——变体怪 = 基础条目 + 挂接带 Lua 的特性定义。

### 3.3 运行时侧（加性扩展）

```rust
// crates/octopus-types/src/lib.rs
pub struct EnemySpec {
    pub name: String,
    pub hp: Option<i64>,
    pub ac: Option<i64>,
    // ↓ 新增（全部可选，旧数据不受影响）
    /// 图鉴引用：命中 storybook.characters[] 中 kind='monster' 的条目 id
    pub template_id: Option<String>,
    /// 展开数量（默认 1）：3 只地精 = 1 条 template_id + count=3
    pub count: Option<u32>,
    /// 覆盖攻击技能（缺省用图鉴条目的第一个攻击技能）
    pub skill_id: Option<String>,
}

pub struct EnemyView {
    pub id: String,          // 遭遇内寻址：e1 / e2…
    pub name: String,
    pub hp: i64,
    pub max: i64,
    pub ac: i64,
    // ↓ 新增
    /// 背后的怪物实例键；缺省 = 临时敌人（AI 现编 / 旧日志）
    pub instance_id: Option<String>,
    pub template_id: Option<String>,
}
```

**不变量**：`instance_id` 存在时，`hp` / `ac` 是该实例的**投影**；不存在时，条目自身是权威（现状）。

---

## 4. 引擎设计

### 4.1 实例化时机与生命周期

**决定：图鉴条目不在开档时实例化。**

`crates/octopus-api/src/lib.rs:435` 的 `build_state` 会把 `characters[]` **全部**建成
`inst-{id}` 实例。若怪物也这样建，同一种地精就成了全局单例——A 遭遇打掉半血，B 遭遇还是残血。
所以 `build_state` 跳过 `kind == "monster"`（顺带 `is_initially_present` 对怪物不再有意义）。

**决定：遭遇创建时克隆实例。**

`Intent::Encounter` 处理逻辑（`session.rs:2790`）扩展为：

```
for spec in enemies:
    if spec.template_id 命中图鉴条目:
        for n in 1..=spec.count.unwrap_or(1):
            key = "enc-{enc_id}:{template_id}#{n}"
            emit StateUpdate(Character, entity_id=key, field="instance", op=Add,
                             value=CharacterInstance {
                                 instance_id: key, template_id, name,
                                 kind: "monster",
                                 attributes: 模板.attributes.clone(),
                                 resources:  模板.resources.clone(),   // hp 满值
                                 present: true, statuses: [],
                             })
            遭遇条目.push(EnemyView { id: e{n}, instance_id: key, template_id, … })
    else:
        保持现状：只建 EncounterView 条目（临时敌人，无实例）
```

实例键带遭遇 id → **天然隔离**：同模板的多场遭遇互不影响。

**需要的引擎改动**：`apply_delta` 的 `DeltaDomain::Character` 分支（`session.rs:3740`）
目前只改**已存在**的实例（`let Some(c) = get_mut else { return }`），没有任何插入路径。
新增分支：

```rust
} else if d.field == "instance" && d.op == DeltaOp::Add {
    if let Ok(inst) = serde_json::from_value::<CharacterInstance>(d.value.clone()) {
        state.characters.insert(key, inst);
    }
}
```

这一条让「克隆怪物」走**同一条唯一变更路径**（`apply_event` → `apply_delta`），
实时与重放必然一致，RNG / 快照 / 归档全部免费兼容。

### 4.2 敌方单位的寻址

- **玩家 → 敌人**：`strike`（已有）先按 `e1` 或名字找遭遇条目；有 `instance_id` 则从实例取 hp / AC。
- **敌人 → 玩家**：**新意图** `Intent::EnemyStrike { enemy_id, target_id?, skill_id? }`。
  需要它是因为现有 `strike` 的语义写死了「受控角色打遭遇里的敌人」；
  让 AI 用同一个意图反向攻击会让「谁是攻方」含糊，且无法表达「3 只地精各打一次」。

### 4.3 对称攻击内核（本次核心重构）

现在 `strike_enemy`（`session.rs:1888`）里混了三件事：找敌人、掷骰、扣血，且攻守写死。
抽成一个纯内核：

```rust
/// 一次攻击的对称结算：攻守双方都是角色实例（玩家 / NPC / 怪物一视同仁）。
/// 玩家打怪与怪打玩家是同一个函数的两个方向。
fn resolve_attack(
    attacker: &CharacterInstance,
    defender: &CharacterInstance,
    skill: Option<&SkillDef>,     // 缺省：1d20 命中 + 1d6 伤害
    difficulty_override: Option<i64>,  // 缺省：defender 的派生 AC
) -> AttackOutcome { /* 掷骰 → 伤害 → 目标 resources.hp 的 delta */ }
```

- `strike`（玩家打怪）= `resolve_attack(受控实例, 敌人实例, 武器/技能)`
- `enemy_strike`（怪打玩家）= `resolve_attack(敌人实例, 目标实例, 怪物攻击技能)`
- 既有行为全部保留：技能 checker 优先、武器命中加值、伤害骰兜底 1d6、最低 1 点。
- 临时敌人（无实例）走一个「临时单位」适配器：AC 取条目、hp 取条目——**旧行为零变化**。

> ⚠️ 这个内核**不是新写的**：它应该是 `command::execute_skill`（已在 `resolve_skill` 复用）的一次调用，
> 而不是第四个平行实现。前置工作见[判定与攻击结算设计](./check-mechanism-design.md) C1 / C2。

### 4.4 AC 求值与派生值

引擎目前**不会**求派生值：`derived.rs` 只有公式求值器，唯一消费者是前端
`frontend/src/lib/derived.ts::computeDerived`。

**决定：按需计算，不把派生值写进 `CharacterInstance`。**

```rust
/// 按故事书 derived[] 顺序求值：变量 = 实例属性 + 之前已算的派生值（+ 挂接/装备修正）。
fn derived_values(&self, inst: &CharacterInstance) -> BTreeMap<String, f64>
```

- `resolve_attack` 取 `derived["ac"]`，缺失回落到 `12`（与现状一致）。
- 不落实例状态 → 属性被状态/装备改动后 AC 自动跟随，无陈旧值、无新不变量。
- 与前端 `computeDerived` **同语法同口径**（两处求值器已声明互为镜像）。

### 4.5 状态与可寻址性

怪物实例一旦进 `st.characters`，以下**全部免费可用**，无需改代码：

`Status`（施加/移除状态）· `Adjust`（改 HP）· `Check`（以怪物为主语判定）·
`QueryCharacter`（读怪物数据卡）· `Move`（怪物换地点）· 技能的 `target_id` 指向怪物。

这是选「真实例」而非「一次性快照」的全部收益。

### 4.6 提示词与扮演边界（关键：别让怪物污染对话）

怪物实例在 `st.characters` 里，默认会漏进三个「人形角色专用」的地方，必须按 `kind` 门控：

| 位置 | 现状 | 改法 |
|---|---|---|
| `SessionRules::personas()` (`session.rs:416`) | 按在场 id 抽人格档案 | 跳过 `kind == "monster"`（怪物没有对话示例，不该占 6 人预算） |
| `present_npcs()` (`session.rs:2155`) | 未署名 `speak`/`emote` 的回落对象 | 跳过 monster（怪物不会说话） |
| `present_actors()` (`session.rs:3647`) | 提示词「在场角色」名单 | 跳过 monster（怪物出现在【当前遭遇】块里） |

**【当前遭遇】块升级**：从「名字 + hp」升为**数据卡摘要**——
名字 / HP / AC / 可用攻击（技能名 + 伤害骰）。AI 不必再瞎编怪物能干什么。
提示词覆盖 key 复用现有的 `story.block.encounters`（`prompt.rs:249`），无需新注册项。

图鉴**不整本注入**（省上下文）：只注入当前遭遇涉及的那几种。

---

## 5. 编辑器

### 5.1 面板改名

一级分组 `character` 与叶 tab `characters` 目前都叫 **「人物」**
（`WorkspaceA.vue:41-42`）。改名为 **「角色库」**：

- 中性：PC / NPC / 怪物同住一个面板，没有「怪物也是人」的别扭。
- 避开 CONTEXT.md 明确回避的单字「角色」（`_Avoid_: 角色（该词易与运行时实例混淆）`）——
  「角色库」是**模板集合**，不是运行时实例。
- 备选：`名册` / `模板库` / `图鉴`（只涵盖怪物，不建议）。

### 5.2 `CharacterPanel.vue`（2120 行）

- 顶部加 `kind` 切换：**玩家角色 / NPC / 怪物**；列表按 kind 分组或筛选。
- `kind = 'monster'` 时隐藏「扮演向」区块：对话示例 / 每日准备 / 关系边 / 受控切换。
- `kind = 'monster'` 时显示**数据卡区块**：`statblock` 展示字段 + 攻击技能绑定
  （从 `skills[]` 里挑 `check.kind = 'attack'` 的）+ 立绘 + 派生值（AC 实时预览，
  复用 `computeDerived`）。
- 按 AGENTS.md 硬规则：所有可编辑字段静止态就要看得出可编辑（虚线底边 / 铅笔图标 / 悬停反馈）。

### 5.3 游玩页

- 遭遇面板：`EnemyView` 有 `instance_id` 时显示数据卡（AC / HP 条 / 立绘）。
- 怪物实例走既有 `CharacterSheet.vue`（`sheet` 分区声明可直接复用）。
- 左栏「在场角色」过滤 `kind === 'monster'`（怪物只在遭遇卡片里出现）。

---

## 6. LMoP 附录 B 导入（第一份真实数据集）

新增 `scripts/import-bestiary.mjs`：读 5e bestiary JSON → 产出 `characters[]` 的怪物的条目。

| 5e 字段 | 产出 |
|---|---|
| `str/dex/con/int/wis/cha` | `attributes`（配合 `attribute_dimensions`：baseline 10 / step 2） |
| `ac[0]` | 条目级常量 + `sb.derived` 里声明 `ac` |
| `hp.average` / `hp.formula` | `resources.hp`（`world.resources` 声明 hp 资源） |
| `action[]` | `skills[]`：`check = {dice:"1d20", kind:"attack"}` + `effect.immediate[damage]` |
| `trait[]` / `senses` / `languages` / `immune` / `cr` | `statblock` 展示字段 |
| `type` / `alignment` | `statblock.creatureType` |

- 中文名对齐 LMoP 附录 B markdown（可选 `--from-md` 读译文名）。
- **产出 JSON，不自动改故事书**——合并由人决定（避免脚本静默改写作者内容）。
- 验收样本：灰烬丧尸 → `AC 8` / `HP 22` / 六维 `13,6,16,3,6,5`。

---

## 7. 兼容与升格

**纯加性，不需要 `upcast`、不需要 `schema_version` 变更。**

| 面 | 兼容性 |
|---|---|
| 旧故事书 | 无 `kind:'monster'` 条目 → 完全不受影响 |
| `kind` 校验 | 从两值扩到三值；旧值照常通过 |
| `EnemySpec` / `EnemyView` | 新字段全可选 + `skip_serializing_if` → 旧日志 / 旧客户端反序列化不变 |
| 旧命令日志 | `Encounter` delta 原样重放；`field:"instance"` 是新分支，旧 delta 不进 |
| 无图鉴的遭遇 | 走「临时敌人」路径 = 现状行为 |
| `build_state` | 多一个 `kind != "monster"` 过滤；旧存档里本来就没有怪物条目 |

---

## 8. 验收

**引擎（`cargo test -p octopus-engine` + `-p octopus-api`）**

1. `kind:'monster'` 通过校验；非法 kind（如 `'beast'`）仍报 `invalid_character_kind`
2. 开档后 `st.characters` **不含**图鉴模板实例
3. 遭遇引用模板 → 生成 `count` 个实例；delta 落库；重放后实例与 HP 一致
4. 玩家 `strike` 模板怪：AC 取派生值、伤害扣实例 `resources.hp`
5. 怪物 `enemy_strike` 玩家：命中用怪物属性、难度 = 玩家派生 AC、伤害扣玩家 HP
6. 同模板两场遭遇**互不影响**（实例隔离）
7. `personas` / `present_actors` / `present_npcs` 均不含怪物
8. 旧格式遭遇（无 `instance_id`）行为与改动前逐字一致（回归）
9. AC 派生：`derived.ac` 缺失时回落 12

**前端**：`pnpm -C frontend build`（含 vue-tsc）+ 编辑器改名后**不悬停不聚焦截图自查**
（AGENTS.md 硬规则：可编辑控件静止态必须看得出可编辑）。

**LMoP 导入**：灰烬丧尸六维 / AC / HP 与附录 B 数值逐项对齐。

---

## 9. 里程碑

| 期 | 内容 | 依赖 |
|---|---|---|
| **M1 数据模型** | `kind:'monster'` + `statblock` + `EnemySpec`/`EnemyView` 扩字段 + `validate.rs` kind 校验 | — |
| **M2 引擎** | `Character` 域 `field:"instance"` 插入 delta · 遭遇克隆实例 · `resolve_attack` 对称内核 · `Intent::EnemyStrike` · `derived_values` · 三处 `kind` 门控 · 遭遇块升级 | M1 |
| **M3 编辑器** | 面板改名「角色库」+ kind 切换 + 怪物数据卡表单 + 游玩页遭遇数据卡 | M1 |
| **M4 导入** | `scripts/import-bestiary.mjs` + LMoP 附录 B 合并进 story_example | M2 |

**M2 后置（本期不做，登记在案）**：
- 状态免疫（需状态 id 列表 + 施加时拦截）
- 伤害免疫 / 抗性（需先有封闭的「伤害类型」概念——那是一个新原语，不该顺手加）
- 先攻与回合顺序（当前遭遇无常驻轮次，攻击由 AI 逐条宣告）
- 图鉴条目的开放内容化（若将来「种类」要数据驱动，`statblock` 可整体迁进 `kinds`）

---

## 10. 待确认

1. **面板改名**：推荐「角色库」（备选：名册 / 模板库）。CONTEXT.md 词条一并落笔。
2. **M2 后置项**是否接受（状态免疫 / 伤害免疫 / 先攻顺序本期不做）。
3. 实施顺序：按 M1 → M4 串行，还是 M3（编辑器）与 M2（引擎）并行。
