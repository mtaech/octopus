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
| `../scripts/lmop-engine-check/` | **真实引擎校验器**（独立 crate，只 path 依赖 `crates/`）：发布门 + 运行时规则断言 |
| `../scripts/lmop-engine-check.sh` | 上面那支校验器的构建 + 运行包装（产物落 `.scratch/`） |
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

* `lmop-storybook.json.lua_mounts`：**16 条**具名挂载点脚本（12 条规则 + 2 条掷表 + 1 条掷表复位 + 1 条 XP）。
* `characters[]`：`pc-lmop-talin`（塔林·银溪，kind=`pc`）——没有受控角色就没法试玩。
  六维 10/16/14/12/13/11 · `res-hp` 24 · `res-insp` 1 · 常驻地 `loc-lmop-040`（凡达林）·
  技能 隐匿 / 察觉 / 短剑 / 消耗激励。角色模板上的 `attachments` 是**开放内容 → Lua 的唯一入口**。
* `statuses[]`：激励 · 灰烬呛咳 · 受突袭 · 倒地 · 中毒。
* `skills[]`：6 条（PC 的四条 + 坠落瓦砾 / 灰烬喷发两条豁免类技能）。
* `kinds[]` / `definitions[]`：规则包追加 **11 种 / 64 条**开放内容；整本故事书
  **13 种 / 88 条**（草稿自带 2 种 / 23 条 + 皮甲 1 条）。
  （原文写的「11 种 / 55 条」是 T15 的错数，本轮按实测改正：T15 时就是 13 / 56。）
* `flags[]`：36 个（草稿 4 + 规则包常驻 8 + 掷表行 24）。本轮删掉不再使用的 `dnd-flanked`
  （集群战术改为读运行时事实，见 GAP-A）。
* `skeleton`：三猪小径（`sc-lmop-0b7`）下 **24 个掷表触发点**，每个带
  `repeatable: true` + `condition: flag_set` + `encounter` 预置。

### 逐条规则 → 引擎原语

| 规则（Lua 脚本 id） | 挂载点 | `when` 闸门 | 用到的通用原语 |
|---|---|---|---|
| 优势 · 激励 `dnd-status-keep-high` | `check_pre_roll` | — | `has_status` + `modify_check('keep_high')` |
| 激励用掉即消失 `dnd-consume-inspiration` | `check_post_roll` | — | `remove_status` |
| 劣势 · 灰烬呛咳 `dnd-status-keep-low` | `check_pre_roll` | — | `has_status` + `modify_check('keep_low')` |
| 劣势 · 日照敏感 `dnd-sunlight-sensitivity` | `check_pre_roll` | `flag_set dnd-sunlight` | `get_attachment` + **判定签名**（`check_kind` / `check_attribute`）+ `modify_check('keep_low')` |
| 优势 · 集群战术 `dnd-pack-tactics` | `check_pre_roll` | —（脚本内自判） | `get_attachment` + **判定签名 `check_kind`** + `list_encounters` + `get_character`（同伴须与**目标**同 `location_id`、失能名单读开放内容 `incapacitated_statuses`）+ `get_definition` + `modify_check('keep_high')` |
| 优势 · 伏击（第 1 轮） `dnd-ambusher-keep-high` | `check_pre_roll` | `flag_set 第1轮` | `get_attachment` + `get_definition` + **`host.target.statuses`** + `modify_check('keep_high')` |
| 技能声明的豁免 DC `dnd-skill-dc` | `check_pre_roll` | — | `definition.check.default_dc` + `host.difficulty` + `modify_check('dc', Δ)` |
| 熟练加值 `dnd-proficiency` | `check_pre_roll` | — | `get_attachment('dnd-proficiency')` + `get_definition(id).fields` + `check_attribute` + `modify_check('add', N)` |
| 豁免成功伤害减半 `dnd-save-half` | `check_post_roll` | — | `check_result` + **`scale_effect(0.5)`**（引擎照常结算一次、只掷一次效果骰）+ 失败分支 `apply_status`（状态 id 来自开放内容 `fail_status`） |
| 突袭 `dnd-surprise` | `check_post_roll` | — | `apply_status` + `apply_effect(set_flag)` |
| 战斗第一轮 `dnd-battle-round` | `turn_end` | `flag_set dnd-in-combat` | `storage` + `clear_flag` × 3 + `apply_effect(set_flag)` |
| 重复豁免 `dnd-save-ends` | `status_tick` | — | `list_definitions('dnd-save-ends')` + `status_id` + `get_attribute` + `engine_rng` + `remove_status` |
| 掷表遭遇 `dnd-wander-day` / `-night` | `turn_end` | 地点 / 野外标记 + 昼夜 | `engine_rng(1,20)` → `engine_rng(1,12)` → `set_flag` |
| 掷表标记复位 `dnd-wander-reset` | `event` | —（脚本内按 `event_name` 分支） | `event_name` / `event_data` + `clear_flag` |
| XP 合计 `dnd-xp-award` | `event` | —（脚本内按 `event_name` 分支） | `event_data.enemy.template_id` + **`get_encounter`（缺 template_id 时按 instance_id 回查）** + `list_definitions('dnd-xp-award')` + `modify_resource('res-xp')` |

### 本轮新用上的引擎原语（T17 / T18）

| 原语 | 用在哪 | 闭合的缺口 |
|---|---|---|
| `host.get_attachments()` / `get_attachment(kind)` / `get_definition(id)` / `list_definitions(kind)` | 熟练加值、日照敏感、集群战术、伏击、重复豁免、XP 数值 | GAP-C |
| `check_pre_roll` 的判定签名（`check_attribute` / `check_kind` / `check_target`） | 日照敏感（攻击骰 ∨ 感知检定）、熟练加值 | GAP-D |
| `host.set_flag(flag[, value])` / `host.clear_flag(flag)` | 战斗轮次、掷表行标记复位 | GAP-H |
| `TriggerDef.repeatable`（边沿语义：条件由假变真） | 24 个掷表触发点 | GAP-N |
| `event` 挂载点 + `host.event_name` / `host.event_data` | XP（`enemy_defeated`）、掷表复位（`encounter_cleared`） | GAP-F |

### 本轮新用上的引擎原语（T21 / T22）

| 原语 | 用在哪 | 闭合 / 提高了什么 |
|---|---|---|
| `host.get_character(id)`（实例键 / 模板 id / 角色名 / instance_id 四种写法） | 集群战术读同伴实例（HP / `location_id` / `statuses`） | GAP-A（跨实体读取这一**能力**已闭合） |
| `host.list_encounters()` / `host.get_encounter(id)` | 集群战术枚举**所有**活跃遭遇里的同伴（不限我所在那一场）；XP 按 `instance_id` 回查 `template_id` | GAP-A / GAP-F 佐证 |
| `host.get_flag(name)` / `host.list_flags()` | 本轮规则包**未改口径**（`when` 闸门照旧），能力已可用 | — |
| `host.target` 与 `host.actor` 同级完整（含 `statuses`） | 伏击直接读目标状态 | GAP-L |
| `host.scale_effect(factor)`（只在 `check_post_roll` / `pre_resolve`） | 豁免成功伤害减半 | GAP-E |
| `host.resolved_effects`（PostResolve 只读） | 引擎校验核对「缩放的就是引擎那份」 | GAP-E 证据 |

「优势 / 劣势」仍是**规则包自己的词汇**：引擎只认识 `keep_high` / `keep_low`，
脚本里包一层别名由规则包负责（[规则集走 Lua](../docs/rules-via-lua.md) §4 ②）。

### 掷表遭遇的完整链路（无新增字段）

```
turn_end + when(at_location 三猪小径 ∨ dnd-in-wilderness) + 昼夜
  → Lua: engine_rng(1,20)  ≥17 触发
  → Lua: engine_rng(1,12)  查表
  → Lua: host.set_flag('dnd-wander-day-N')
  → 触发点 condition: { op: 'flag_set', flag: 'dnd-wander-day-N' } + repeatable: true + encounter 预置
  → 回合末 evaluate_turn_end（evaluate_skeleton_full）
       · 条件由假变真（边沿）才触发 → spawn_trigger_encounters → 建遭遇（图鉴实例克隆）
  → 遭遇结束：event 'encounter_cleared' → dnd-wander-reset 按遭遇名反查 → host.clear_flag(行标记)
       · 标记落回假 → 该行下一次被掷中就是一次新边沿 → **同一表项可反复出**
  → 击败敌人：event 'enemy_defeated' → dnd-xp-award 按 data.enemy.template_id → modify_resource('res-xp')
       · 与遭遇从哪来无关：掷表建的、导演即兴建的都发
```

表项来自模块第 1225 / 1235 行的白昼 / 黑夜两张 d12 表（蚊蝠 / 食人魔 / 地精 / 大地精 / 兽人 / 狼 / 枭熊 / 食尸鬼）。

复位靠**遭遇名**精确反查：预置遭遇名带 d12 行号且在表项间唯一（如
`野外遭遇（白昼） d12=3：食人魔`），所以多场掷表遭遇同时进行时只复位真正结束的那一场。
遭遇未结束期间同一行再次被掷中不会产生边沿（不重复堆同名遭遇，刻意如此）。

### XP 的口径与边界

* 时机：`enemy_defeated`（strike 击杀路径派发），`data.enemy.template_id` → 开放内容
  `xp-<模板 id>` 定义的 `fields.xp`；数值逐条来自 T8 图鉴 `statblock.xp`（`--check` 与引擎校验都断言相等）。
* 覆盖：逐只结算，**不再依赖掷表行标记**，所以导演 / AI 即兴建的遭遇一样拿得到 XP。
* 边界（原样登记）：非 strike 击杀（Lua 直接把怪物资源打到 0）**不派发** `enemy_defeated`，
  那种路径拿不到 XP——奖励只挂在引擎认得的「击败」事实上。

### 导演 / AI 需要置位的标记

规则脚本用 `when: flag_set` 闸门读标记（T21 之后 Lua 也能用 `host.get_flag` 直接读，本包未改口径）。
上场前按剧情置位：

| 标记 | 何时置位 |
|---|---|
| `dnd-in-wilderness` | 队伍在三猪小径一带野外行进（`at_location` 也可开闸，但角色 `location_id` 只在 `Move` 时更新，不随场景切换） |
| `dnd-night` | 夜里（决定查白昼表还是黑夜表） |
| `dnd-sunlight` | 阳光直射（日照敏感生物在场时） |
| `dnd-in-combat` | 战斗开始（战斗轮次计数开闸） |

### 表达不了的缺口（**逐条复核：已闭合 / 近似提高 / 仍存在**）

`--check` 结尾会打印完整清单（每条：哪条规则 / 卡在哪个原语 / 需要什么 / 本轮复核结论）。
**14 条缺口：闭合 7 条（C / D / E / F / H / L / N）· 近似提高 1 条（A）· 仍存在 6 条（B / G / I / J / K / M）。**

0. **先记一条「引擎实况 + 规则包补法」**：`use_skill` 路径的难度只取 `world.check.default_dc`，
   **技能的 `check.default_dc` 不参与**（只在 `Intent::Check` / `strike` 兜底里读）。
   所以「DC 10 敏捷豁免」这条规则由 `dnd-skill-dc` 用 `modify_check('dc', want - host.difficulty)` 补上——
   不是缺口，但**照抄技能声明是没用的**，必须在 Lua 里翻一次。

#### 已闭合（7 条）

| GAP | 主题 | 闭合原语 | 规则包落地 | 谁在断言 |
|---|---|---|---|---|
| **C** | 挂接 / 开放内容不进 Lua | `host.get_attachments()` / `get_attachment(kind)` / `get_definition(id)` / `list_definitions(kind)` | 熟练加值 / 日照敏感 / 集群战术 / 伏击 / 重复豁免 / XP 数值全部改成**运行时读开放内容**；角色模板的 `attachments` 是唯一入口 | `--check: open_content.data_driven / attachments.resolve`；引擎 `proficiency.data_driven / bestiary.xp_matches` |
| **D** | `check_pre_roll` 没有判定签名 | 掷骰前下发 `check_attribute` / `check_kind` / `check_target` | 日照敏感按签名区分「攻击骰」与「依赖视力的感知（wis）检定」；熟练加值回到掷骰前用签名选属性；伏击也收敛到 `kind == attack`（T26 修正轮，闭合第四轮 T27 复查登记的同口径残留） | `--check: sunlight.by_signature / ambusher.target_status`；引擎 `sunlight.by_signature / ambusher.check_signature` |
| **F** | 没有「敌人被击败」事件 | `event` 挂载点 + `host.event_name` / `host.event_data`（`enemy_defeated` / `encounter_cleared`） | `dnd-xp-award` 按 `data.enemy.template_id` **逐只**发 XP；与遭遇来源无关 | `--check: encounter_table.chain`；引擎 `xp.improvised_encounter` |
| **H** | 没有清标记原语 | `host.set_flag(flag[, value])` / `host.clear_flag(flag)` | `dnd-battle-round` 每轮清 1..3 再置当前轮；`dnd-wander-reset` 在 `encounter_cleared` 时清行标记 | `--check: encounter_table.chain` |
| **N** | 触发点一次性（`repeatable` 未实现） | `TriggerDef.repeatable`（**边沿**语义：条件由假变真） | 24 个掷表触发点全部标 `repeatable: true`，配合 H 的复位形成「结束 → 清标记 → 再触发」循环 | `--check: encounter_table.chain`；引擎 `encounter_table.refires` |
| **E** | 效果掷骰值不暴露 | `host.scale_effect(factor)`（`check_post_roll` / `pre_resolve`）+ PostResolve 只读 `host.resolved_effects` | `dnd-save-half` 改为声明 `scale_effect(0.5)`：引擎照常结算一次、只掷一次效果骰，数值按 0.5 向零取整；Lua 不再重掷。技能的失败附加状态移入开放内容 `fail_status`，由脚本失败分支补 | `--check: save_half.scale_effect`；引擎 `save_half.engine_scales_own_roll` / `save_half.resolved_effects_is_engine_snapshot` |
| **L** | `host.target` 读不到目标状态 | `host.target` 升级为与 `host.actor` 同级完整（含 `statuses`） | `dnd-ambusher-keep-high` 直接读 `host.target.statuses` 判定「是否受我突袭」；`when` 闸门只留战斗第一轮 | `--check: ambusher.target_status`；引擎 `ambusher.reads_target_statuses` |

**C + F 的边界（如实登记）**：
* C 闭合的是「规则要用的**数据**进 Lua」。`get_attachment` / `get_definition` 只读**故事书静态数据**；
  「读运行时实体」由 T21 的 `get_character` / `*_encounter` 补上，但那条路只到 GAP-A（近似提高），见下。
* F 闭合的是「引擎派发击败事实」。**非 strike 击杀**（Lua 直接把怪物资源打到 0）不派发
  `enemy_defeated`，那条路径不发 XP。T21 之后 `get_encounter` 只多了一条「事件缺 `template_id` 时回查」的兜底。

#### 近似提高（1 条）

| GAP | 主题 | 现在的近似口径 | 仍然缺什么 |
|---|---|---|---|
| A | Lua 读不到其它实体 | **能力已闭合**：`get_character` / `list_encounters` / `get_encounter` / `get_flag` / `list_flags` 都已可用，集群战术已改用它们读同伴。「我方同伴」= **所有**活跃遭遇里的其它敌人实例（不限我所在那一场），「在目标附近」= 与**目标**同一 `location_id`（目标缺地点时 fail-closed 不给），「未失能」= HP>0 且不带开放内容 `incapacitated_statuses` 列出的状态；只对 `kind == attack` 生效 | **规则结果仍不精确**：精确到 5 尺做不到（GAP-B）——所以按「近似提高」登记，不按「已闭合」报。T26 修正轮订正了旧的**声明与实现矛盾**（旧脚本从不检查同伴位置，却声明 `range_proxy` = 同一 `location_id`） |

为什么 A 不按「已闭合」报：本条缺口当初登记的规则是**集群战术**，不是一个孤立的能力。
跨实体读取本身确实落地了，但那条规则仍受 GAP-B 限制；**宁可少报闭合，也不夸大**。

#### 仍然存在（6 条，卡在什么原语上）

| GAP | 主题 | 卡在哪个原语 / 缺什么 |
|---|---|---|
| B | 没有位置 / 距离 / 区域 | 引擎只有 `location_id` 这种地点归属，没有 5 尺 / 交战关系；集群战术只能用「与目标同 `location_id` + 未失能」近似（精确距离做不到） |
| G | 预置遭遇数量是静态整数 | `EncounterPresetEnemy.count` 没有骰式落点；「1d8+2 只蚊蝠」只能固定成骰式下界 |
| I | 没有先攻 / 轮次 / 行动经济 | 「在战斗第一轮失去其回合」只有状态 + 标记，靠叙事层落实 |
| J | 没有「掷骰 vs 对手被动」入口 | `CondExpr::AttributeGe` 只看 actor 自己的属性；`get_character` 能读属性，但判定上下文里拿不到「对手是谁」，也没有对抗入口 |
| K | 模板级状态无声明入口 | character 模板没有初始状态声明，建实例时 `statuses` 恒为空；挂接读得到，但「挂接 → 状态」的桥没有 |
| M | 没有 `encounter_active` 条件 | `CondExpr` 仍只有 `encounter_cleared`；战斗轮计数继续靠 `dnd-in-combat` 标记开闸（Lua 能读遭遇，但 `when` 闸门走 `CondExpr`） |

### 运行期证据（真实引擎）

`--check` 本身做静态断言（形状 / 引用 / 数据驱动 / 重复性 / 确定性）。
规则**真的生效**由独立校验器 `scripts/lmop-engine-check`（只 path 依赖 `crates/`，不改引擎）证明：
发布门 `validate_storybook_result` + `lint_storybook`，运行期用 `LuaHost` + `conditions::evaluate_skeleton_full`
跑规则包里**实际的 Lua 源码**：

    ./scripts/lmop-engine-check.sh
    # 或：CARGO_TARGET_DIR=$PWD/.scratch/lmop-engine-check-target \
    #       cargo build --manifest-path scripts/lmop-engine-check/Cargo.toml
    #     node scripts/lmop-rulepack.mjs --check --engine <上面的 debug/lmop-engine-check>

断言（2026 实测全绿）：

    publish_gate                  validate_storybook 错误 0 / lua_lint 问题 0
    proficiency.data_driven       definition bonus=5 → Lua add 5；把 bonus 改成 9（只改故事书数据）→ add 9
    sunlight.by_signature         str+attack → keep_low；wis+attribute → keep_low；str+attribute / con+save → 零请求；狼 → 零请求
    encounter_table.refires       400 回合模拟：12 个触发点被边沿触发，单点最多 12 次；
                                  反证组（去掉 repeatable / 复位）单点最多 1 次
    xp.improvised_encounter       即兴遭遇（encounter=enc-improvised）击败灰烬丧尸 → res-xp +50；
                                  scene / encounter_cleared 事件零请求；再击败一只 → 再 +50
    bestiary.xp_matches           31 条模板的 dnd-xp-award 定义与图鉴 statblock.xp 逐条相等
    save_half.engine_scales_own_roll            同种子下因子 1.0 → delta -11（ctx.rng 4 颗），
                                  规则包 scale_effect(0.5) → delta -5 = trunc(-11×0.5)，ctx.rng 逐位相同
    save_half.resolved_effects_is_engine_snapshot  PostResolve 读到 factor=0.5 / rng_consumed=3 /
                                  delta field=resources.res-hp value=-4，与提交值逐字一致
    pack_tactics.reads_world_facts 同伴与目标同地点 → keep_high；同伴 HP=0 / FP-A 同伴在别的地点 /
                                  FP-B 同伴带失能状态 / FP-C 双方无 location_id / FP-E 属性检定 /
                                  无挂接 → 零请求；FN-A 同伴在另一场遭遇（同地点）→ keep_high
    pack_tactics.old_impl_reproduces_misjudgments  变异：换回旧脚本，同一批反例复现出 5 类错误结论
                                  （FP-A/B/C/E 误给 = 1、FN-A 漏判 = 0）；新脚本对应 0/0/0/0/1
                                  → 反例不是恒真，旧实现真的会被抓住
    ambusher.reads_target_statuses target.statuses 含受突袭 → keep_high；无状态 / 别的状态 /
                                  无伏击挂接 → 零请求
    xp.encounter_snapshot_fallback 事件缺 template_id、只有 instance_id → get_encounter 回查 → res-xp +50；
                                  快照里查不到 → 不发
    ambusher.check_signature      T27 ①（第 5 轮修复）：attack → keep_high；attribute / save → 零请求
    ambusher.old_impl_ignores_check_signature  旧脚本原文内嵌（git HEAD）：同一批场景 1 / 1 / 1
                                  = T27 三条原始发现的可执行复现（旧实况没有被抹掉）
    ambusher.fail_closed_without_signature  签名缺失（无判定快照 / kind 缺省）→ keep_high = 0；
                                  同一 harness 换 attack 签名 → 1（排除「恒 0」）
    ambusher.target_status_data_driven  只改开放内容 target_status（dnd-surprised → dnd-prone），
                                  同一份 Lua 的结论随之翻转
    save_half.check_signature_gate  同一份脚本：save+成功 → scale_effect[0.5]；save+失败 → 不缩放且补 fail_status；
                                  attack / attribute → 零请求
    save_half.rule_comes_from_open_content  只改定义 skill_id → 该技能上的缩放整体消失
    save_ends.reads_open_content  只改定义 dc（1 / 99）→ 本回合是否移除状态随之翻转
    xp.event_gate                 同一 payload 换事件名（scene / encounter_cleared）→ 零 XP
    xp.value_from_open_content    只改定义 xp（50 → 57）→ 发放值随之变

### 既有引擎 E2E 用例的口径更新（经授权）

`crates/octopus-api/tests/lmop_verification.rs` 里两条用例钉的是**旧口径**，GAP-F / GAP-H / GAP-N
闭合后按新语义改写（授权范围内的测试文件；T26 修正轮另有 round2 的 `r2_6` 与 round3 的
`r3_audit4` 判据订正，见本节末尾）：

| 用例 | 旧期望 | 新期望（更严，不是放宽） |
|---|---|---|
| `a1_a2`（挂载点形状） | `lua_mounts.len() == 38`（12 规则 + 2 掷表 + 24 条按行回补 XP） | `== 16`，且**断言关键挂载点在场并挂在正确时机**：`dnd-xp-award` 必须 mount=`event` 且源码含 `enemy_defeated`；`dnd-wander-reset` 必须 mount=`event` 且源码含 `clear_flag`；旧的 `dnd-xp-day*/night*` 规则必须已移除 |
| `a9_a14`（XP） | 清空遭遇后一回合 XP 增量 == 掷表行 xp×数量 | **逐只**击杀时 XP 增量 == 该模板 `statblock.xp`；整场清剿增量 == 遭遇内每只敌人数据卡 XP 之和；清空后再跑一回合增量 **恰好为 0**（不再补发）；行标记与触发点 `active` 必须已复位为 false |

新用例的失败条件（仍有牙齿）：少发 / 多发 / 补刀重复发 / 金额与数据卡不符 / 清空后补发 /
行标记未复位 —— 任意一条都会 FAIL。旧缺口语义以注释形式保留在用例里（可追溯）。

全套回归：`cargo test -p octopus-api --test lmop_verification` → **13 passed**（含 `a11_a13` 的
`r#mod = 8`，即 dex 3 + 数据驱动的熟练 5，证明 GAP-C 在真实 session 里生效）。

**第三轮修正（T26 修正轮）**：`dnd-save-half` 换 `scale_effect` 时留下的**过期注释**与**机制盲断言**一并订正，
不再停在「注释过期但断言照旧」：

* `lmop_verification.rs` 的 `a12` / `a12b`：注释改成新机制（成功豁免由**引擎**结算一次、
  按 `scale_effect(0.5)` 缩放，Lua 不再重掷）；新增 PostResolve **探针**回传 `host.resolved_effects`，
  断言 `factor=0.5 / rng_consumed=3 / #deltas=1 / 引擎 hp delta == -本次掉血`。
  旧断言（骰数 4、delta∈1..9）对机制无感，新判据能把「引擎缩放自己那份」与「Lua 重掷」分开：
  变异用例 `a12_mutation_old_reroll_rule_fails_the_mechanism_probe` 把规则换回旧重掷脚本后，
  旧断言照旧 PASS、新判据 FAIL（`factor=nil / rng=0 / deltas=0`）。
* `lmop_verification_round2.rs` 的 `r2_6`：负对照的描述订正为「分支**发了**、引擎也**算了**，
  只是 delta 落在不存在的实体键上被 `apply_delta` 静默丢弃（`characters.len()==1` → delta 仍为 0）」；
  断言改为直接检查该回合 `Resolution.state_changes` 里确有一条落在不存在目标上的 hp 伤害——
  这样才区分得出「分支被跳过」与「算了被丢弃」。变异用例
  `r2_6_mutation_without_scale_declaration_has_no_ghost_delta` 删掉缩放声明后，
  同一探针找不到 ghost delta（骰数 1），证明判据不是恒真。
* 只做**判据加强**、不放宽期望：投影层面的 `delta==0`（负对照）/ `delta∈1..9`（自目标、显式目标）
  与 `dice_count==4` 逐条保留。实测：`lmop_verification` 13 passed /
  `lmop_verification_round2` 14 passed / `lmop_verification_round3` 14 passed。
* 第三轮验证文件 `lmop_verification_round3.rs` 的 `r3_audit4`（集群战术近似边界）随之改为断言
  **修正后**的边界：FP-A（同伴在别的地点）/ FP-B（同伴带失能状态）/ FP-C（双方无 `location_id`）/
  FP-E（属性检定）从「实测给优势」翻转为**必须不给**，FN-A（同伴在另一场遭遇但同地点）翻转为**必须给**。
  旧的「近似误差，实测」期望是修正前的实况，只在 Git 历史里可追溯。
* **第五轮（第四轮 T27 发现① 的修复）**：`dnd-ambusher-keep-high` 与集群战术**同口径**收敛到
  `kind == attack`（数据卡只说「攻击检定」）。两条**钉住旧实况**的用例随之订正，且旧实况没有消失：
  · 第四轮 `lmop_verification_round4.rs` 的 `r4_ambusher_still_ignores_check_signature` 改名为
    `r4_ambusher_check_signature_attack_only_after_r5_fix`：删掉「源码里没有 `host.check_kind`」那条
    子串断言（T27 发现③ 批评的形态），`attribute` / `save` 由 1 翻转为 0，保留 `attack = 1`（两端都钉）。
  · 旧脚本**原文**（git HEAD 版）以 `OLD_AMBUSHER` 内嵌进 `scripts/lmop-engine-check/src/main.rs`，
    由引擎级断言 `ambusher.old_impl_ignores_check_signature` 在同一批场景复现 1 / 1 / 1
    —— T27 的三条原始发现可执行地存活，不依赖任何会被翻转的断言。
  · 第三轮 `lmop_verification_round3.rs` 的 `r3_16` 的 harness 原本没给 check 快照——那模拟的是
    「根本没有判定」这个不可能出现在真判定里的状态（真实引擎每一次真判定的 `check_pre_roll` 都下发签名：
    `command.rs` L160-167 / L466-494、`session.rs` L5149-5160；既有测试
    `pre_roll_sees_check_signature_in_skill_path`）。第 5 轮经授权补上攻击签名，**四条期望值与数据驱动段一字未改**；
    「签名缺失 → 不给优势」改为由引擎级断言 `ambusher.fail_closed_without_signature` 显式钉住。
* **`--check` 子串型断言加固（T27 发现③）**：每条子串型一致性断言都带行为级兜底标注
  （`--check` 打印，`--engine` 模式核对被引用的引擎断言真的在场：`consistency.behavioural_backing`）；
  为此引擎级断言从 11 条增到 19 条（伏击签名 / 伏击旧实现复现 / 伏击状态数据驱动 / 豁免减半签名闸门 /
  豁免减半技能来自开放内容 / 重复豁免读开放内容 / XP 事件闸门 / XP 数值来自开放内容）。

## 地点 / 地图 / 骨架

* 地点：`world.locations[]` 的 `parent_id` 树，id 形如 `loc-lmop-<5etools section id>`（可回溯）。
  根地点 10 个（凡达林、克拉摩窝点、三猪小径、兔莓与阿加莎的巢穴、古枭井、雷树废墟、飞龙突岩、克拉摩堡、回声洞、地精伏击），
  子地点为镇内场所与「第 N 区」（如 `回声洞 > 第 5 区`），共 88 个。
* 骨架：4 章（四个部分）× 88 场，场景 `location_id` 指向地点；草稿里 `goals` / `triggers` 留空待作者填写，规则包在「三猪小径」（`sc-lmop-0b7`）追加 24 个掷表触发点（每个带 `repeatable: true` + `condition: flag_set` + `encounter` 预置）。
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
    ./scripts/lmop-engine-check.sh                      # 真实引擎：发布门 + 运行时规则断言
    node scripts/lmop-rulepack.mjs --check --engine .scratch/lmop-engine-check-target/debug/lmop-engine-check

覆盖：条目数与解析命中、kind / statblock / 资源声明、`derived.ac` 与数据卡逐条相等、
攻击技能的 check 形状与伤害效果、变体挂接且全稿无 `extends`、
**与附录 B 逐项交叉核对**（28 条可解析条目全部一致）、地点树无环、锚点归一化且在所属地点子树内、
骨架场景引用有效、两次构建逐字节一致（确定性）。

`lmop-rulepack.mjs --check`（28 项断言）覆盖：十条能力覆盖矩阵（规则 + 开放内容两侧都要有）、开局 PC（六维 /
`res-hp` / 技能 / 常驻地）、**撤回字段 deep-scan**（`advantage` / `disadvantage` / `on_save` /
`ConditionalModifier` / `save_ends` / `crit` / `EncounterTable` / `extends` 在整本故事书里零命中）、
`lua_mounts` 静态预检（挂载点白名单 / 禁用 API / `host.*` 存在性 / 撤回词）、图鉴数据未被复制或改动
（规则挂接只允许**追加**规则包自己的 kind）、掷表链路端到端形状（Lua → set_flag → repeatable 触发点 →
预置遭遇 → `clear_flag` 复位）、开放内容引用自洽、**数据驱动**（7 条规则的 Lua 里没有标识 / 数值常量）、
**日照敏感按签名**、**规则挂接全部指向同 kind 的定义**、缺口清单逐条带 status、两次构建逐字节一致。

加 `--engine <bin>` 时，同一支真实引擎校验器再跑：`validate_storybook` 错误 0 +
`lint_storybook` 问题 0（警告 24 条为既有图鉴告警）+ **20 条**运行时规则断言（加值数据驱动 / 判定签名 /
同表项反复触发 / 即兴遭遇发 XP / XP 与图鉴逐条相等 / **缩放就是引擎那份骰** / **`resolved_effects` 核对** /
**集群战术读运行时事实** / **集群战术旧实现变异复现** / **伏击读目标状态** / **XP 遭遇快照回查** /
**伏击只对攻击检定** / **伏击状态数据驱动** / **豁免减半签名闸门** / **豁免减半技能来自开放内容** /
**重复豁免读开放内容** / **XP 事件闸门** / **XP 数值来自开放内容** / **伏击旧实现变异复现** / **伏击签名缺失 fail-closed**）。
另外 `--check` 对每条**子串型一致性断言**都标了行为级兜底（引擎级断言名），
`--engine` 模式逐条核对它们真的在场（`consistency.behavioural_backing`）——防止「注释即可满足」的弱断言。

附录 B 共 **31 节 = 30 张独立数据卡 + 毒牙**（青年绿龙的别名条目）。
