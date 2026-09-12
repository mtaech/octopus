# 术语消歧：本项目术语 × D&D 术语

> 本项目是**通用** AI RPG，不预设题材与规则体系；D&D 只是当前用来压测系统的一张具体规则集。
> 两边常有名同实异的词。本文给出对照与本项目的权威叫法；领域词汇表见 [CONTEXT.md](../CONTEXT.md)。

## 对照表

| D&D 说法 | 本项目术语 | 落地形态 | 关键区别 |
|---|---|---|---|
| 属性（力量 / 敏捷…） | **属性维度** (Attribute Dimension) | `attribute_dimensions[]` + 人物 `attributes` | 「属性」是规则词，通用侧统一叫属性维度 |
| 属性调整值 | **派生值** (Derived Value) | `derived[]` 公式 | 调整值是**算出来的**，不是存储字段 |
| 豁免 | 判定种类 = **豁免** (Check Kind: Save) | `CheckerDef.kind = "save"` | **由目标掷骰**，失败才结算效果 |
| 攻击检定 | 判定种类 = **攻击** (Attack) | `CheckerDef.kind = "attack"` | 比较对象是目标防御，不是固定难度 |
| 被动察觉 | 判定种类 = **被动** (Passive) | `kind = "passive"` + `passive_base` | **不掷骰**：基数 + 修正 |
| 技能（奥秘 / 调查…） | **熟练项** (Proficiency) | 开放种类（如 `dnd-skill`）+ 挂接 | **与本项目「技能」同名不同物** |
| 法术 / 戏法 | **技能** (Skill) | `SkillDef` + `category` | 本项目「法术」只是技能的一个分类 |
| 法术位（按环） | **资源** + **资源层阶** | `world.resources[]` + `tier` | 分环 = `tier`；恢复靠**恢复时机** |
| 长休 / 短休 | **休息** (Rest) | `POST /api/saves/{id}/rest` | 玩家显式操作，产出状态增量并进命令日志 |
| 准备法术 | **每日准备** (Prepared) | `CharacterDef.prepared[]` | 每日重挑，区别于永久的「掌握」 |
| 护甲 / 盾牌 / 武器 | **装备位** (Equip Slot) | `ItemDef.slot` + `ItemDef.modifiers` | 装备后修正并入派生计算 |
| 种族 / 职业 / 背景 / 语言 | **开放内容** (Definition) | `kinds` + `definitions` + `attachments` | 内容由数据声明，代码不认识「种族」 |
| 人物卡版面 | **卡面分区** (Sheet Section) | `Storybook.sheet[]` | 版面由故事书声明 |

## 开放内容 ≠ 机制核心

开放种类/内容（`kinds` / `definitions`）只装**规则书内容**；机制核心有各自的专用声明，**不要用开放内容重复定义**：

| 机制核心 | 专用声明 | 典型误用 |
|---|---|---|
| 属性维度 | `attribute_dimensions` + 人物 `attributes` | 建一个 `ability` 种类再存一遍力量/敏捷… |
| 派生值 / 修正 | `derived` + `modifiers` | 建一个 `combat-stat` 种类再存一遍 AC / 法术DC |
| 资源（含法术位） | `world.resources` + `tier` | 建一个 `slot` 种类 |
| 可结算状态 | `statuses` | 建一个 `condition` 种类 |
| 可装备物品 | `items` + `slot` + `modifiers` | 建一个 `gear` 种类 |

判断口径：**引擎要「求值/改写」的，就是机制核心**；只是「给 AI 与玩家看的设定」，才是开放内容。

## 四个最容易踩的坑

1. **技能 ≠ 技能**。本项目「技能」= 可被使用的能力（`SkillDef`）；D&D「技能」= 熟练。
   写文档 / 提示词时，D&D 的技能一律叫**熟练项**。
2. **属性 ≠ 属性维度**。通用侧统一叫属性维度；只有讨论具体规则体系时才用「属性」。
3. **豁免不是属性检定**。属性检定是本人掷骰对抗难度；豁免是**目标**掷骰对抗施加方 DC，且**失败才结算效果**。
4. **掌握 ≠ 准备**。掌握是永久的技能列表；准备是每日重挑的子集。

## 压测样例：莱纳斯·晨星（D&D 5e 法师 1 级）

这张卡的每个部位如何落到本项目：

- 六维 → `attribute_dimensions`（str/dex/con/int/wis/cha）+ 人物 `attributes`
- 调整值 / 豁免 / AC / 法术DC / 法术攻击 / 被动察觉 → `derived` 公式（如 `floor((int - 10) / 2)`、`8 + prof + int_mod`、`armor_base + dex_mod`）
- 种族「人类」的属性 +1 → `race-human.modifiers`（修正来源），人物挂接后自动并入
- 火焰箭 → `SkillDef` + `check.kind = "attack"`（攻击检定）
- 睡眠 → `SkillDef` + `check.kind = "save"`（目标 CON 豁免，失败才结算）
- 被动察觉 → `kind = "passive"` + `passive_base = 10`
- 1 环法术位 → `res-slot-1`（`tier = 1`，`natural_recovery.trigger = "per_long_rest"`）
- 长休 → `POST /api/saves/{id}/rest {kind:"long"}`（实测 HP +5、1 环位 +2）
- 每日准备 4 个 → `CharacterDef.prepared`
- 皮甲 / 盾牌 → `ItemDef.slot` + `modifiers`（实测 AC 12 → 13 → 15）
- 种族 / 职业 / 背景 / 特性 / 熟练项 / D&D 技能 / 战斗数据 / 语言 → 开放种类 + 定义 + 人物挂接
- 卡面版面 → `Storybook.sheet`（战斗数据 / 属性与身份 / 豁免与技能 / 特性 / 语言 / 个性与背景）
