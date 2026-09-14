# LMoP 导入说明（story_example）

本目录的《凡戴尔的失落矿坑》内容由 `scripts/import-bestiary.mjs` 转成**可导入的故事书草稿**。
脚本只读文件、只写 JSON 草稿，**不写数据库**；合并进正式故事书由人决定。

## 产物

| 文件 | 说明 |
|---|---|
| `../scripts/import-bestiary.mjs` | 导入脚本（图鉴 / 地点树 / 地图锚点 / 骨架），含 `--check` 自断言 |
| `bestiary-lmop.json` | 官方 5e 图鉴文件（LMoP 部分），**原样**保存 |
| `bestiary-lmop-appendix-b.json` | 官方 5e 图鉴文件（MM/DMG）中附录 B 用到的条目子集，**逐字段原样**抽取 |
| `lmop-storybook.draft.json` | 输出：可导入的故事书草稿（**没有规则、没有 PC**） |
| `../scripts/lmop-rulepack.mjs` | 规则包生成器（D&D 规则走 Lua + 开放内容），含 `--check` 自断言 |
| `lmop-storybook.json` | 输出：**可导入的完整故事书** = 草稿 + 规则包 + 开局 PC |
| `lmop.json` / `凡戴尔的失落矿坑.md` / `lmop2md.js` | 既有素材（未改动） |

## 数据来源

* 5e 图鉴（中文镜像）：`https://5e.kiwee.top/data/bestiary/bestiary-lmop.json`、`.../bestiary-mm.json`、`.../bestiary-dmg.json`
* 冒险数据：`https://5e.kiwee.top/data/adventure/adventure-lmop.json`（本目录 `lmop.json`）
* 附录 B 中文数据卡：`凡戴尔的失落矿坑.md` 第 2500 行起（名称顺序与数值交叉核对的事实来源）

### 重建 vendored 源文件

    mkdir -p /tmp/lmop-src && cd /tmp/lmop-src
    curl -O https://5e.kiwee.top/data/bestiary/bestiary-lmop.json
    curl -O https://5e.kiwee.top/data/bestiary/bestiary-mm.json
    curl -O https://5e.kiwee.top/data/bestiary/bestiary-dmg.json
    node scripts/import-bestiary.mjs --vendor-from /tmp/lmop-src

## 用法

    node scripts/import-bestiary.mjs                 # 生成 lmop-storybook.draft.json
    node scripts/import-bestiary.mjs --check         # 自检并断言附录 B 数值（不写文件）
    node scripts/import-bestiary.mjs --out <file>    # 指定输出

    node scripts/lmop-rulepack.mjs                   # 生成 lmop-storybook.json（规则包合并）
    node scripts/lmop-rulepack.mjs --check           # 规则包自检（覆盖 / PC / 撤回字段 / 掷表链路 / 确定性）
    node scripts/lmop-rulepack.mjs --check --engine <bin>   # 追加真实引擎发布校验
    node scripts/lmop-rulepack.mjs --out <file>

规则包生成器**读草稿、不复制数据**：图鉴 id / 六维 / HP / XP 全部从 `lmop-storybook.draft.json` 取；
草稿不存在时它会先调用 `import-bestiary.mjs` 重建。图鉴条目的数值在本步骤逐字不变（`--check` 断言），
只对灰烬丧尸追加一条豁免技能引用（`sk-lmop-ash-eruption`）。

## 字段映射（硬边界：规则集语义不落封闭字段）

依据 [bestiary-design.md](../docs/bestiary-design.md) §6 与 [rules-via-lua.md](../docs/rules-via-lua.md) §5：

| 5e 数据卡 | 故事书落点 | 说明 |
|---|---|---|
| `str/dex/con/int/wis/cha` | `characters[].attributes` | 配合 `attribute_dimensions`（baseline 10 / modifier_step 2） |
| `ac[0]` | 顶层 `derived.ac` = `10 + dex_mod` 加上挂接 `monster-armor` 的 `modifiers` | **不新增 ac 字段**；护甲差值是挂接定义的数据 |
| `hp.average` | `characters[].resources["res-hp"]`（`world.resources` 声明 hp） | 显示用的骰式留在 `statblock.actionsNote` 与译文里 |
| `action[]` | `skills[]`：`check = {dice:"1d20", kind:"attack"}` + `effect.immediate[{kind:"damage", amount, resource:"res-hp"}]` | 近战攻击声明 `attribute: "str"`、纯远程 `"dex"`；豁免类动作（无攻击项）只产出伤害效果，豁免减半留给 5e 规则包（Lua） |
| `trait[]` / `senses` / `languages` / `immune` / `conditionImmune` / `save` / `skill` / `speed` | `statblock.traits`（展示型，引擎不读） | |
| `type` / `alignment` / `size` | `statblock.creatureType` | 如「中型 不死生物，中立邪恶」 |
| `cr` | `statblock.challenge` + `statblock.xp` | XP 合计走 Lua `ModifyResource`，**不进引擎类型系统** |
| `_copy`（数据卡继承） | `attachments["monster-variant"]` → `definitions[]`（kind=`monster-variant`） | **不新增 `extends` 字段**；变体 = 基卡引用 + 额外特性 |

## 变体（开放内容挂接，非 extends）

* **灰烬丧尸** = 丧尸数据卡 + 额外特性「灰烬喷发」。基础关系来自附录 B 原文
  「它们使用丧尸资料卡，并具有以下额外特性」（第 2541 行）；脚本从译文识别基卡，
  再用「自身特性 − 基卡特性」算出额外特性。
* **毒牙** = 青年绿龙（5e `_copy` + `_mod replaceTxt` 名称替换）；
  **刚铎·寻岩者** / **南卓·寻岩者** = 平民（+ 山地矮人模板）。
* 以上三条在 markdown 里因 `lmop2md.js` 未展开 `_copy` 而显示为 `undefined`，
  本脚本按 5e 原始数据展开，因此比译文更完整。
* 变体条目自身仍是**完整可结算**的数据卡（六维 / HP / AC / 攻击齐备），挂接只表达「继承自哪张卡 + 额外特性」。

## 规则包（L5）：D&D 规则全走 Lua + 开放内容

依据 [rules-via-lua.md](../docs/rules-via-lua.md) §4 的五个原语 / §6 的硬边界：**引擎定义「能做什么」，
Lua 定义「什么时候做」**。本规则包不改引擎、不新增任何封闭字段，规则 = `lua_mounts` 脚本 + 开放内容。

### 交付形态

* `lmop-storybook.json.lua_mounts`：**38 条**具名挂载点脚本（12 条规则 + 2 条掷表 + 24 条 XP 回补）。
* `characters[]`：`pc-lmop-talin`（塔林·银溪，kind=`pc`）——没有受控角色就没法试玩。
  六维 10/16/14/12/13/11 · `res-hp` 24 · `res-insp` 1 · 常驻地 `loc-lmop-040`（凡达林）·
  技能 隐匿 / 察觉 / 短剑 / 消耗激励。
* `statuses[]`：激励 · 灰烬呛咳 · 受突袭 · 倒地 · 中毒。
* `skills[]`：6 条（PC 的四条 + 坠落瓦砾 / 灰烬喷发两条豁免类技能）。
* `kinds[]` / `definitions[]`：11 种 / 55 条开放内容（熟练加值、日照敏感、集群战术、伏击、重复豁免、
  豁免减半、遭遇表、XP 合计、伤害类型、战斗轮次）。
* `skeleton`：三猪小径（`sc-lmop-0b7`）下 24 个掷表触发点，每个带 `condition: flag_set` + `encounter` 预置。

### 逐条规则 → 引擎原语

| 规则（Lua 脚本 id） | 挂载点 | `when` 闸门 | 用到的通用原语 |
|---|---|---|---|
| 优势 · 激励 `dnd-status-keep-high` | `check_pre_roll` | — | `has_status` + `modify_check('keep_high')` |
| 激励用掉即消失 `dnd-consume-inspiration` | `check_post_roll` | — | `remove_status` |
| 劣势 · 灰烬呛咳 `dnd-status-keep-low` | `check_pre_roll` | — | `has_status` + `modify_check('keep_low')` |
| 劣势 · 日照敏感 `dnd-sunlight-sensitivity` | `check_pre_roll` | `flag_set dnd-sunlight` | `modify_check('keep_low')`（只在攻击检定） |
| 优势 · 集群战术 `dnd-pack-tactics` | `check_pre_roll` | `flag_set dnd-flanked` | `modify_check('keep_high')` |
| 优势 · 伏击（第 1 轮） `dnd-ambusher-keep-high` | `check_pre_roll` | `all_of[第1轮, 被突袭]` | `modify_check('keep_high')` |
| 技能声明的豁免 DC `dnd-skill-dc` | `check_pre_roll` | — | `definition.check.default_dc` + `host.difficulty` + `modify_check('dc', Δ)` |
| 熟练加值 `dnd-proficiency` | `check_post_roll` | — | `check_attribute` / `check_kind` + `modify_check('add', N)` |
| 豁免成功伤害减半 `dnd-save-half` | `check_post_roll` | — | `check_result` + `engine_rng` + `apply_effect(damage)` |
| 突袭 `dnd-surprise` | `check_post_roll` | — | `apply_status` + `apply_effect(set_flag)` |
| 战斗第一轮 `dnd-battle-round` | `turn_end` | `flag_set dnd-in-combat` | `storage` + `apply_effect(set_flag)` |
| 重复豁免 `dnd-save-ends` | `status_tick` | — | `status_id` + `get_attribute` + `engine_rng` + `remove_status` |
| 掷表遭遇 `dnd-wander-day` / `-night` | `turn_end` | 地点 / 野外标记 + 昼夜 | `engine_rng(1,20)` → `engine_rng(1,12)` → `set_flag` |
| XP 合计 `dnd-xp-<表>-<d12>` × 24 | `turn_end` | `all_of[表行标记, encounter_cleared]` | `storage` 幂等 + `modify_resource('res-xp')` |

「优势 / 劣势」是**规则包自己的词汇**：引擎只认识 `keep_high` / `keep_low`，
脚本里包一层别名由规则包负责（[规则集走 Lua](../docs/rules-via-lua.md) §4 ②）。

### 掷表遭遇的完整链路（无新增字段）

```
turn_end + when(at_location 三猪小径 ∨ dnd-in-wilderness) + 昼夜
  → Lua: engine_rng(1,20)  ≥17 触发
  → Lua: engine_rng(1,12)  查表
  → Lua: apply_effect(actor, { kind = 'set_flag', flag = 'dnd-wander-day-N' })
  → 触发点 condition: { op: 'flag_set', flag = 'dnd-wander-day-N' } + encounter 预置
  → 回合末 evaluate_turn_end → spawn_trigger_encounters → 建遭遇（图鉴实例克隆）
  → 清空后：dnd-xp-table-N 在 when: encounter_cleared 时 modify_resource('res-xp')
```

表项来自模块第 1225 / 1235 行的白昼 / 黑夜两张 d12 表（蚊蝠 / 食人魔 / 地精 / 大地精 / 兽人 / 狼 / 枭熊 / 食尸鬼）。

### 导演 / AI 需要置位的标记

规则包读不到 flag（Lua 没有读标记的口子），**闸门全靠 `when: flag_set`**。上场前按剧情置位：

| 标记 | 何时置位 |
|---|---|
| `dnd-in-wilderness` | 队伍在三猪小径一带野外行进（`at_location` 也可开闸，但角色 `location_id` 只在 `Move` 时更新，不随场景切换） |
| `dnd-night` | 夜里（决定查白昼表还是黑夜表） |
| `dnd-sunlight` | 阳光直射（日照敏感生物在场时） |
| `dnd-in-combat` | 战斗开始（战斗轮次计数开闸） |
| `dnd-flanked` | 有怪物满足「盟友在目标 5 尺内」 |

### 表达不了的缺口（**原样报告，未新增封闭字段**）

`--check` 结尾会打印完整清单（每条：哪条规则 / 卡在哪个原语 / 需要什么）。摘要：

0. **先记一条「引擎实况 + 规则包补法」**：`use_skill` 路径的难度只取 `world.check.default_dc`，
   **技能的 `check.default_dc` 不参与**（只在 `Intent::Check` / `strike` 兜底里读）。
   所以「DC 10 敏捷豁免」这条规则由 `dnd-skill-dc` 用 `modify_check('dc', want - host.difficulty)` 补上——
   不是缺口，但**照抄技能声明是没用的**，必须在 Lua 里翻一次。

1. **Lua 读不到其它实体**：没有 `get_character` / `get_encounter` / 读 flag；`host.target` 只有 id/name/kind。
   → 集群战术、伏击的「对受突袭者」、XP 的遭遇内容都只能用标记近似。
2. **`check_pre_roll` 没有判定签名**：`check_kind` / `check_attribute` 只在掷骰后才有值，
   而「取高/取低」只有掷骰前有意义 → 日照敏感的「感知（察觉）检定」那一半表达不了（攻击那一半靠 `host.definition` 绕过）。
3. **效果掷骰值不暴露**：豁免减半只能按 `host.definition` 的骰式**重掷**再取半——期望值对，不是同一颗骰。
4. **没有「敌人被击败」事件 / 遭遇快照**：XP 只能用 `when: encounter_cleared` + 掷表行标记回补；
   导演即兴建的遭遇拿不到 XP。
5. **预置遭遇数量是静态整数**：表里的「1d8+2 只蚊蝠」只能固定成骰式下界（写进 trigger `hint` 与 preset `note`）。
6. **没有清标记原语、触发点一次性（`repeatable` 未实现）**：24 行表项整局各触发一次。
7. **没有先攻 / 轮次 / 行动经济**：突袭的「战斗第一轮失去其回合」只有状态 + 标记，靠叙事层落实。
8. **挂接定义不进 Lua**：`docs/rules-via-lua.md` §7 的 `host.attachment_bonus` 引擎里没有；
   开放内容只能给人看，规则要用的数据在**生成期烘进 Lua 表**（`lmop-rulepack.mjs` 的 `RULEPACK` 常量）。
9. **模板级状态无声明入口**：`build_state` 建实例时 `statuses` 恒为空，怪物固有特性做不成自带状态。

### 运行期证据

`--check` 只做静态断言（含真实引擎 `--engine` 钩子）。规则**真的生效**另由一次性 Rust 校验器证明：
`LuaRegistry::from_storybook(&sb)` + `LuaHost::new(seed)` + `run_chain_with/run_chain_status`
（`octopus_engine::lua_host` 全是 pub API），断言 19 项：

    registry.loads_all       38 条 lua_mounts 全部装载（when 也全部解析）
    rule.status_keep_high    激励 → ModifyCheck keep_high；无状态 → 零请求
    rule.status_keep_low     灰烬呛咳 → ModifyCheck keep_low
    rule.sunlight_keep_low   阳光+攻击 → 取低；阳光+属性检定 / 无阳光 → 零请求
    rule.pack_tactics_*      dnd-flanked 成立 → 取高；闸门不成立 → 零请求
    rule.ambusher_*          第 1 轮 + 被突袭 → 取高
    rule.skill_dc            技能声明 DC 10 / 当前 12 → dc -2；已相等或攻击技能 → 不加
    rule.proficiency_add     dex 属性检定 → add 5；攻击 / 未熟练属性 → 不加
    rule.save_half           豁免成功 → 3d6 重掷取半 = 数字串，消耗 3 颗骰；失败 → 不补
    rule.consume_inspiration 掷骰后 RemoveStatus 激励
    rule.surprise            隐匿失败 → ApplyStatus 受突袭 + set_flag
    rule.repeat_save         体质 +10 对 DC 10 连续 5 次全移除；不在表里的状态 → 零请求
    rule.battle_round        第 1/2/3 轮各置一次标记，第 4 轮起不再置位
    rule.wander_table        300 回合触发 48 次，12 个 d12 行全部出现；黑夜闸门走另一张表
    rule.xp_award            蚊蝠 25×3 = 75 → modify_resource；重复触发零请求（幂等）
    rule.xp_award.waits_for_clear  遭遇未清空 → 不发 XP

如需固化进仓库，把上面这套调用搬进 `crates/octopus-engine/tests/` 即可（属 crates/，超出本任务 inScope）。

## 地点 / 地图 / 骨架

* 地点：`world.locations[]` 的 `parent_id` 树，id 形如 `loc-lmop-<5etools section id>`（可回溯）。
  根地点 10 个（凡达林、克拉摩窝点、三猪小径、兔莓与阿加莎的巢穴、古枭井、雷树废墟、飞龙突岩、克拉摩堡、回声洞、地精伏击），
  子地点为镇内场所与「第 N 区」（如 `回声洞 > 第 5 区`），共 88 个。
* 骨架：4 章（四个部分）× 88 场，场景 `location_id` 指向地点；草稿里 `goals` / `triggers` 留空待作者填写，规则包在「三猪小径」（`sc-lmop-0b7`）追加 24 个掷表触发点（每个带 `condition: flag_set` + `encounter` 预置）。
* 地图：`world.maps[]` 7 张（剑湾 / 克拉摩窝点 / 凡达林 / 红标帮窝点 / 雷树废墟 / 克拉摩堡 / 回声洞），
  锚点 `pins` 坐标为**归一化 0..1**。

⚠️ **两点必须人工处理**：

1. **底图未随导入**（`MapDef.image` 留空）。官方底图路径如下，上传到资产库后回填：
   `adventure/LMoP/The Sword Coast.webp`、`Cragmaw Hideout.webp`、`Phandalin.webp`、
   `Redbrand Hideout.webp`、`Ruins of Thundertree.webp`、`Cragmaw Castle.webp`、`Wave Echo Cave.webp`
   （作者 Mike Schley；另有对应的 `... (Player).webp` 玩家版未收入）。
2. **锚点坐标是确定性占位布局**（按地点序排成网格），不是底图上的真实位置，需按底图校准。

## 自检

    node scripts/import-bestiary.mjs --check
    node scripts/lmop-rulepack.mjs --check
    node scripts/lmop-rulepack.mjs --check --engine <引擎校验器>

覆盖：条目数与解析命中、kind / statblock / 资源声明、`derived.ac` 与数据卡逐条相等、
攻击技能的 check 形状与伤害效果、变体挂接且全稿无 `extends`、
**与附录 B 逐项交叉核对**（28 条可解析条目全部一致）、地点树无环、锚点归一化且在所属地点子树内、
骨架场景引用有效、两次构建逐字节一致（确定性）。

`lmop-rulepack.mjs --check` 覆盖：十条能力覆盖矩阵（规则 + 开放内容两侧都要有）、开局 PC（六维 / `res-hp` /
技能 / 常驻地）、**撤回字段 deep-scan**（`advantage` / `disadvantage` / `on_save` / `ConditionalModifier` /
`save_ends` / `crit` / `EncounterTable` / `extends` 在整本故事书里零命中）、`lua_mounts` 静态预检
（挂载点白名单 / 禁用 API / `host.*` 存在性 / 撤回词）、图鉴数据未被复制或改动、掷表链路端到端形状
（Lua → flag → 触发点 → 预置遭遇 → XP）、开放内容引用自洽、缺口清单非空、两次构建逐字节一致。
加 `--engine <bin>` 时再跑一次真实引擎的 `validate_storybook` + `lint_storybook`（0 error / 0 lua issue）。

附录 B 共 **31 节 = 30 张独立数据卡 + 毒牙**（青年绿龙的别名条目）。
