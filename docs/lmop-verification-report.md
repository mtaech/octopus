# 凡戴尔的失落矿坑 故事书 · 独立验证报告

> 角色：独立验证者（**只验证，不修产品代码**）。inScope：`docs/lmop-verification-report.md` · `crates/octopus-api/tests/` · `scripts/`。
> 被测交付物：`story_example/lmop-storybook.json`（生成器 `scripts/import-bestiary.mjs` + `scripts/lmop-rulepack.mjs`）。
> 用户原话要求：「开发完成后自己新建一个 凡戴尔的失落矿坑 的故事书验证一下」——本报告走的是**真实 HTTP 产品路径**新建故事书（不是只读 JSON）。
> 本报告只做记录与取证；发现的不符项**原样登记，未改动 `crates/*/src/` 与 `frontend/src/` 的任何产品代码**。
>
> **⚠️ 本文件含四轮验证。第 1 轮（§0–§10，T15/T12 之后的交付物）原样保留在下方，作为历史证据。**
> **第 2 轮（T17/T18 引擎原语 + T19 规则包升级之后）见 §R2。** 第 2 轮由**另一名独立验证者**
> （未参与 T17/T18/T19 实现）重新取证，**不采信任务方自述**；并专门对 T19 修改过的两处第一轮用例做了
> **变异审计**（§R2-3）。
> **第 3 轮（T21/T22 引擎原语 + T23 规则包改动之后）见 §R3。** 第 3 轮由**第三名独立验证者**
> （未参与 T21/T22/T23 实现）重新取证，专门审计四件事：save-half 是否真的不重掷 / 连带修正是否保住行为 /
> 两处过期注释的断言还有没有牙齿 / 集群战术近似的边界；并逐条复核 14 条 GAP 的第三轮现状（§R3-3 / §R3-4）。
> **第 4 轮（T26 修掉第三轮遗留的 A/B/C 三类问题之后）见 §R4。** 第 4 轮由**第四名独立验证者**
> （未参与 T21/T22/T23/T24/T26）专审三件事：T26 对第三轮审计用例 `r3_audit4` 的**断言翻转**是合法修复还是弱化
> （用 git HEAD 旧脚本原文 + 逐条变异独立复现）、a12/a12b 与 r2_6 新判据的机制辨识力、以及 5 类误判与
> 「开放内容声明 vs Lua 实现」的逐条对账；并复核 14 条 GAP 的第四轮现状（含 GAP-A 能否升为「闭合」）。
> **第 5 轮（T28/T29 四项改动 + T29 对验证者用例的两处改动之后）见 §R5。** 第 5 轮由**第五名独立验证者**
> （未参与 T28/T29）专审三件事：T29 对第四轮用例 `r4_ambusher_still_ignores_check_signature` 的**翻转**
> 与对第三轮 harness `run_ambusher` 的补签名是否忠实（旧实况是否可执行存活、逐条变异）、
> `force_effect` 与 `scale_effect` 的正交性 / `apply_delta` 丢弃可见化 / 伏击签名与 fail-closed 的独立复现、
> 以及「16 条挂载点里只有 ambusher 缺签名」与 `--check` 行为级兜底的独立复核；并复核 14 条 GAP 的第五轮现状。

---

## 0. 结论摘要

| | 数量 | 说明 |
|---|---|---|
| 验收项 **通过** | **15 / 15** | 见 §2 逐条验收表 |
| **未通过**（子项 / 字面口径） | **2** | ① 验收 12 的**自目标分支**：不给 `target_id` 时「豁免成功减半」静默失效；② 验收 4 的字面要求「**投影**里有 maps」不成立（地图只在故事书层） |
| **未覆盖** | **0** | 15 条全部有可复现证据 |
| 附加上报（非验收项，观察/缺陷候选） | 3 | 资源经 `Add` 后变 JSON 浮点；文档计数 `55 条开放内容 / 11 种` 与实际 `56 / 13` 不符；`rng_seed = fnv1a(save_id)` 使骰值逐次不同 |

**一句话**：这套「图鉴 + 判定内核 + 规则集走 Lua + 实景/在场」的设计**在 LMoP 这条真实链路上成立**——
真实 HTTP 建书/发布/开档/投影全绿；引擎级 12 条端到端用例（图鉴克隆、攻击双内核、位置链路、遭遇预置、对抗判定、Lua 优势/熟练/减半/XP）连续 3 轮全绿。
设计的**证伪点也如实证伪**：Lua 读不到跨实体/挂接/效果骰值，规则包只能靠「生成期烘表 + 叙事层置标记」近似——这些缺口被规则包如实登记为 14 条 GAP，本轮验证**逐条复核后确认仍然存在（仅 GAP-N 的引擎侧被 T12 修掉一半）**。

---

## 1. 环境与复现命令

```bash
# 起服务（AGENTS.md 指定方式；本报告用 backend 目标，前端与验证无关）
cd /home/huang/Personal/Dev/Code/octopus
./start.sh backend            # 真实产品路径：127.0.0.1:8787 + 现有 octopus.db

# A. 真实 HTTP 路径（确定性，不需要 AI）
node scripts/lmop-verify-http.mjs        # EXIT=0，11/11 通过

# B. 引擎级端到端（注入式 AI provider，零联网）
cargo test -p octopus-api --test lmop_verification -- --test-threads=4
#   12 passed; 0 failed（连续 3 轮复跑均 12/12）

# T15 引擎边界自查
./scripts/lmop-boundary-check.sh          # EXIT=0

# 生成器自检（交付物侧证据）
node scripts/import-bestiary.mjs --check  # EXIT=0，28/28
node scripts/lmop-rulepack.mjs --check    # EXIT=0，22 项断言 0 失败 + 14 条 GAP
```

原始输出留档：`/tmp/lmop-http.log` · `/tmp/lmop-engine-tests.log` · `/tmp/lmop-rulepack-check.log` · `/tmp/lmop-boundary.log`。

---

## 2. 逐条验收表

| # | 验收项 | 结论 | 证据（命令 / 断言 / 原始片段） |
|---|---|---|---|
| 1 | 用真实路径基于 `lmop-storybook.json` 创建故事书并发布成功 | **通过** | `scripts/lmop-verify-http.mjs`：`POST /api/storybooks`→201，`PUT`→200，`POST …/publish`→200 且 `revision=1`；本轮 ID `sb-ba4d44baec164da09cdb756c01528f97`。引擎用例 `a1_a2` / 其余 10 条每条的 `publish_and_open` 也走同一条发布门（0 error）。 |
| 2 | 开档后的世界投影不含 `kind="monster"` 的初始实例 | **通过** | HTTP：`character kinds = {"pc":1}`、`keys=inst-pc-lmop-talin`；引擎 `a1_a2`：`monsters=0, characters.len()=1`。 |
| 3 | 触发引用图鉴条目的遭遇 → 数量 / AC / HP / 六维与数据卡一致（灰烬丧尸 AC 8 / HP 22 / 13,6,16,3,6,5） | **通过** | `a3_encounter_clones_bestiary_and_matches_datacard`：`count=2` 展开两只，`hp=22 max=22 ac=8`，实例 `attributes={"str":13,"dex":6,"con":16,"int":3,"wis":6,"cha":5}`。 |
| 4 | 玩家攻击：判定属性与技能声明一致（不再硬编码力量）、伤害扣实例 hp、产生 CheckResult | **通过**（附字面口径差异，见 §3-②） | `a4`：`attribute="dex"`（短剑声明 dex，非 str）、`expr="1d20"`、`rolls=[…]`、`kind=Attack`；命中回合实例 `res-hp 22→14`，且遭遇条目 `hp` 与实例同步。 |
| 5 | 怪物攻击玩家（enemy_strike）：难度 = 玩家派生 AC | **通过** | `a5`：`CheckResult.target=14`（10 + dex_mod 3 + 皮甲挂接 +1），40 回合内命中一次，PC `res-hp 24→22`。 |
| 6 | 位置链路：AtLocation 不再恒假；按地点统计的在场人数对 NPC 返回真实数字 | **通过** | `a6`：`eval_cond(AtLocation)` 匹配真/不匹配假；补充 fixture（LMoP 无 NPC）中两个 NPC `location_id=loc-b`、`present=true`，按 loc-b 数人 = **2**；真枪实弹：PC `Move` 到 loc-b 后 `at_location` 触发点 `fired=true`。另有 `a8`：把 PC 移到 `loc-lmop-0b7` 后 `dnd-wander-day` 的 `when: at_location` 闸门真的开闸（掷表触发点 fired）。 |
| 7 | 同一模板的两场遭遇 HP 互相隔离 | **通过** | `a7`：两场各 1 只灰烬丧尸，实例键不同（带遭遇 id）；打第一场 `22→14`，第二场仍 `22`。 |
| 8 | 触发点预置遭遇：fired 后引擎自动建遭遇，EncounterView 带 scene_id / location_id / template_ids | **通过** | `a8`：真实规则包掷表 → `dnd-wander-day-*` → 触发点 `fired` → 自动建遭遇；`scene_id="sc-lmop-00e"`、`location_id="loc-lmop-0b7"`、`template_ids=["mon-owlbear"]`（本轮命中 day-12）。 |
| 9 | encounter_cleared 条件可用：清空后为真；无遭遇时为假 | **通过** | `a6`（无遭遇→false，直接 `eval_cond`）；`a9_a14` 清空全部遭遇后 `eval_cond(EncounterCleared)=true`。 |
| 10 | 对抗判定：掷 vs 掷 与 掷 vs 被动都跑通，`CheckResultPayload.opponent` 非空 | **通过** | `a10`：掷 vs 掷 `opponent={id:"mon-ash-zombie",name:"灰烬丧尸甲"}`、消耗 2 颗骰；掷 vs 被动 `opponent={id:"mon-goblin",name:"地精"}`、只 1 颗骰。 |
| 11 | 优势经 Lua 生效：keep_high 消耗 2 颗骰 | **通过** | `a11`：无激励 `rng_consume=1`；施加 `dnd-inspired` 后同一技能 `rng_consume=2`（`dnd-status-keep-high` → `modify_check('keep_high')`）。 |
| 12 | 豁免成功伤害减半经 Lua 生效（按规则包口径，说明近似程度） | **通过（显式 target 口径）**，自目标分支**未通过** | `a12`：`sk-lmop-rubble-collapse` + `target_id`；成功回合 `dice=4`（d20 + Lua 重掷 3d6 取半）、伤害落在 `floor(3d6/2)=1..9`（实测 5）；失败回合 `dice=4`（d20 + 引擎 3d6）、伤害 `3..18`（实测 10）。`a12b` 复现缺口：**不给 `target_id` 时成功回合伤害 delta=0**。近似程度见 §5。 |
| 13 | 熟练加值经 Lua 生效：检定总值含规则包声明的加值 | **通过** | `a11_a13`：PC dex 属性检定 `r#mod = 8`（dex_mod 3 + 规则包 `dnd-proficiency` 的 +5），`total = 骰面 + 8`。 |
| 14 | XP 经 Lua 累加：结算后角色 XP 按数据卡数值增加 | **通过** | `a9_a14`：清空遭遇后下一回合 `res-xp 0→75`，期望值由**数据卡**算得：`dnd-wander-day-2` → `mon-stirge` 25 × 3 = **75**，逐字相等。 |
| 15 | 引擎边界自查：engine/src 与 types/src grep 不到规则集词汇 | **通过** | `./scripts/lmop-boundary-check.sh` EXIT=0：`advantage|proficiency|on_save|save_ends|ConditionalModifier|EncounterTable|extends` **零命中**；既有例外 `CheckKind::Save`（types:63）如实列出。 |

---

## 3. 未通过项与原始证据

### ① 验收 12 的自目标分支：不给 `target_id` 时「豁免成功减半」静默失效

- 用例：`crates/octopus-api/tests/lmop_verification.rs::a12b_save_half_self_target_is_a_silent_hole`（PASS，因为它断言的正是这个**缺口**）。
- 原始输出：
  ```
  A12b: 豁免成功但伤害 delta=0（期望「减半」，实际 0 = 缺口）
  test a12b_save_half_self_target_is_a_silent_hole ... ok
  ```
- 机理（读代码复核）：
  - `SkillDef.sk-lmop-rubble-collapse` 的 `check.kind = save`，豁免主体 = 目标；`target_id` 缺省时 `command::execute_skill` 把 `target_id` 回落成 `actor_id`（`command.rs` `let target_id = ctx.target_id.unwrap_or(ctx.actor_id)`），**但 Lua 上下文的 `ctx.target` 仍是 `None`**；
  - 于是 `dnd-save-half` 里 `host.target and host.target.id` 取到 nil，`apply_effect` 根本不发；
  - 同时引擎侧 `effect_applies = !resolved.result`（豁免成功 = 效果完全不结算），所以「完整的 3d6」也没结算 → **成功回合伤害恰好为 0**。
- 影响：技能在「目标=自己」这一常见写法下，减半规则**看着跑了（无报错、无事件）其实没生效**。这是「规则包 × 引擎接口」的静默错配，不是设计文档登记的 14 条 GAP 之一，**新发现**。

### ② 验收 4 的字面要求「GET state/projection → locations **与 maps** 存在」不成立

- 原始输出：投影键 = `seq,scene_id,scene_title,characters,controlled,flags,progress,quests,encounters,locations,meta`——**没有 `maps`**。
- `WorldProjection`（`octopus-types/src/lib.rs:1057`）确实没有 `maps` 字段；`world.maps` 只在故事书里（`GET /api/saves/{id}` → `storybook.world.maps`）。
- 已在该层完成断言：`maps=7 pins=93 越界=0`（`scripts/lmop-verify-http.mjs` 4d）。
- 结论：**locations 通过；maps 不在投影**。若「投影要下发地图」是硬要求，则此项按字面**未通过**；若地图本就由前端读故事书（现状），则是口径问题——本报告不替设计做判断，原样登记。

---

## 4. 未覆盖项

**无。** 15 条验收全部有可复现证据与原始输出。

需要说明的两处「覆盖方式」：

- 验收 6 的「对 NPC 返回真实数字」：`lmop-storybook.json` 里**只有 1 个 PC、0 个 NPC**（31 怪物是模板，开档不入实例），因此该子项用**补充 fixture 故事书**（1 PC + 2 NPC，内联在测试里）覆盖，已明确标注非 LMoP 数据。
- 验收 4 的 maps：在**存档故事书层**覆盖（投影层无该字段，见 §3-②）。

---

## 5. 规则包口径与近似程度（验收 12 附注）

`dnd-save-half`（`story_example/lmop-storybook.json` → `lua_mounts[7]`）的真实口径：

1. 引擎先按 `CheckKind::Save` 结算：**成功 = 声明的效果完全不结算**（伤害与 `dnd-prone` 都不发），失败 = 完整结算；
2. Lua 在 `check_post_roll` 里，若 `host.check_kind=='save' and host.check_result==true`，**按同一骰式重新掷一次**（`3d6`），算出 `floor(total/2)`，再 `apply_effect(damage, half)`；
3. 因此：
   - 期望值等价（`E[3d6]/2`），但**不是同一颗骰**（方差更大、多消耗 3 颗 RNG）——这正是规则包自己登记的 **GAP-E**；
   - 失败分支下「倒地」状态由引擎结算、成功分支不结算，与数据卡「失败受伤害并倒地，成功只受一半伤害」语义一致；
   - **自目标（无 `target_id`）分支静默失效**（§3-①）。
4. 实测（`a12`）：成功 `dice=4, delta=5`；失败 `dice=4, delta=10`。与上述口径逐条吻合。

---

## 6. T15 证伪报告登记的 14 条 GAP —— 原样汇总与现状

规则包 `--check` 逐条打印、并断言「缺口清单非空」（`node scripts/lmop-rulepack.mjs --check` → `[PASS] gaps.registered — 14 条`）。
以下为 **`/tmp/lmop-rulepack-check.log` 原文**（未改写），随后是逐条现状。

```
—— 表达不了的缺口（14 条，原样报告，不新增封闭字段）——
· [GAP-A-no-cross-entity-read] 集群战术（狼）/ 任何「对手或盟友」类规则
    卡在：Lua 只读得到**当前 actor**：host.get_attribute / get_resource / has_status 全部只作用于 actor；host.target 只带 id / name / kind；没有 get_character / get_encounter / get_flag。
    需要：一个「按 id 读任意角色实例（属性 / 资源 / 状态 / 位置）」的原语，或把遭遇快照交给 Lua。现状：规则包只能用 dnd-flanked 标记代替「盟友在目标 5 尺内」这个事实，由叙事层置位。
· [GAP-B-no-positioning] 集群战术的 5 尺判定 / 任何距离、区域、掩体规则
    卡在：引擎没有位置、距离、区域概念（只有 location_id 这种地点归属）。
    需要：位置或「交战关系」的封闭结构（超出规则包范围）；当前只能靠标记。
· [GAP-C-no-open-content-in-lua] 熟练加值 / 日照敏感 / 集群战术 / 伏击 / 重复豁免的**数据来源**
    卡在：docs/rules-via-lua.md §7 用了 host.attachment_bonus 读开放内容挂接——引擎里**没有这个 API**，也没有任何「读挂接定义 / 读 kinds / 读 definitions」的口子。
    需要：host.attachment(...) 一类只读口子，让规则真正由开放内容驱动；现状：生成期把开放内容烘进 Lua 表（本脚本的 RULEPACK 常量），开放内容只能给人看。
· [GAP-D-no-check-signature-pre-roll] 按判定种类 / 判定属性给优势（日照敏感的「依赖视力的感知（察觉）检定」那一半）
    卡在：check_pre_roll 拿不到判定签名：host.check / host.check_kind / host.check_attribute 只在**掷骰后**才有值；而「取高/取低」只有掷骰前有意义（引擎 apply_post_roll_adjustments 注释明说后置取高无效）。
    需要：check_pre_roll 也下发判定签名（attribute / kind / difficulty）。现状：攻击检定那半靠 host.definition.check.kind 绕过去；感知检定那半表达不了。
· [GAP-E-effect-roll-not-exposed] 豁免成功「伤害减半」的数值等价
    卡在：Lua 读不到本次效果已掷出的伤害（没有 pending_effect / last_effect 快照）。
    需要：把待结算效果的交涉值暴露给 check_post_roll。现状：规则包按 host.definition 的骰式**重新掷一次**再取半——期望值对，但与规则书的同一颗骰不等价（方差更大、多消耗 RNG）。
· [GAP-F-no-defeat-event] XP 合计的触发时机（「合计角色们克服的每个怪物的经验值」）
    卡在：引擎只在场景切换派发 event（scene / scene_change）；没有「敌人被击败 / 遭遇结束」事件，Lua 也读不到遭遇内容（encounters 只在 when 闸门的 CondExpr 快照里）。
    需要：击败事件（带 enemy / template_id / xp）或遭遇快照只读口。现状：只用 when: encounter_cleared + 掷表行标记回补 XP；导演即兴创建的遭遇（LMoP 里的大多数）拿不到 XP。
· [GAP-G-encounter-count-static] 野外遭遇表的数量（1d8+2 只蚊蝠）
    卡在：EncounterPresetEnemy.count 是静态整数，没有骰式的落点。
    需要：预置遭遇支持骰式数量（或让 Lua 参与建遭遇）。现状：规则包把每行固定成骰式下界，原始骰式写进 note。
· [GAP-H-no-unset-flag] 战斗轮次标记 / 掷表行标记的清理
    卡在：ImmediateEffect 只有 set_flag，没有 unset_flag；Lua 也没有清标记口子。
    需要：清标记原语（或按轮次命名并按序号收敛）。现状：dnd-battle-round-1..3 只置不清，闸门必须按具体轮次取值。
· [GAP-I-no-turn-economy] 突袭的「在战斗第一轮失去其回合」
    卡在：引擎没有先攻、轮次、行动经济——没有「回合」可以失去。
    需要：战斗轮次结构（超出规则包范围）。现状：只落 dnd-surprised 状态 + dnd-battle-round-1 标记，失去回合由叙事层落实。
· [GAP-J-no-opposed-passive] 突袭对抗「与地精的被动感知（察觉）属性值对抗」
    卡在：挂载点拿不到对手的被动值；CondExpr 也没有「掷骰 vs 对手被动」的入口（CondExpr::AttributeGe 只看 actor 自己的属性值）。
    需要：对抗判定 / 被动值作为对手（crates 侧进行中）。现状：难度由导演按数据卡被动值给（地精被动察觉 9），规则只按判定成败落突袭标记。
· [GAP-K-status-not-seeded] 怪物固有特性用状态表达（把「日照敏感」做成模板自带状态）
    卡在：build_state 建实例时 statuses 恒为空数组（octopus-api/src/lib.rs），character 模板没有初始状态声明。
    需要：模板级初始状态（或初始挂接 → 状态的桥）。现状：模板级特性只能写进 Lua 表（见 GAP-C）。
· [GAP-L-target-status-unreadable] 伏击的「对受其突袭的生物发动攻击」
    卡在：host.target 不含 statuses / attributes / resources（只有 id / name / kind）。
    需要：目标的只读快照（至少 statuses）。现状：用 dnd-surprised 标记近似「本场有人被突袭」。
· [GAP-M-no-encounter-active-cond] 战斗轮次计数的开闸条件
    卡在：CondExpr 只有 encounter_cleared（「有遭遇」没有对应条件；「无遭遇」时 encounter_cleared 也不成立，取反得不到「有遭遇」）。
    需要：encounter_active 条件。现状：靠 dnd-in-combat 标记开闸（导演 / AI 置位）。
· [GAP-N-trigger-one-shot] 掷表遭遇的可重复性（遭遇表本该反复生效）
    卡在：evaluate_skeleton 对每个触发点做进度去重（progress.triggers），且 TriggerDef.repeatable 只在种子数据里出现、**引擎从未实现**；掷表行标记又没有清除原语（GAP-H），所以每行表项整局只能触发一次。
    需要：可重复触发点（或「触发点 fired 后可复位」的表达）。现状：24 行表项 = 整局最多 24 场掷表遭遇，之后三猪小径不再出事。

```

### 逐条现状（本轮实测复核）

| GAP | 主题 | 本轮现状 |
|---|---|---|
| A | Lua 读不到其它实体 | **仍然存在**。`lua_host.rs` / `types` 中无 `get_character`/`get_encounter`/`get_flag`；`host.target` 仍只带 id/name/kind（见 GAP-L 复核）。 |
| B | 无位置/距离/区域概念 | **仍然存在**。引擎只有 `location_id`，无 5 尺/交战关系。 |
| C | 挂接/开放内容不进 Lua | **仍然存在**。无 `host.attachment`；规则包仍靠生成期烘表（`lmop-rulepack.mjs` 的 `RULEPACK` 常量）。 |
| D | `check_pre_roll` 没有判定签名 | **仍然存在（本轮实证）**。`LuaCheckContext` 只在 `check_post_roll` 下发（`lua_host.rs:211`），且字段为 `attribute/kind/total/target/margin/result/level/rolls`——**不含 `expr`**；`make_env` 里 `host.check*` 仅在 `mount_env.check` 有值时设置。规则包的日照敏感仍只能用 `host.definition.check.kind` 绕。 |
| E | 效果骰值不暴露 | **仍然存在**。无 `pending_effect`/`last_effect`；减半只能重掷（§5）。 |
| F | 无「敌人被击败」事件 | **仍然存在**。XP 仍靠 `when: encounter_cleared` + 掷表行标记回补（验收 14 实测的正是这条近似链路）。 |
| G | 预置遭遇数量是静态整数 | **仍然存在**。`EncounterPresetEnemy.count: Option<u32>`，无骰式落点。 |
| H | 没有清标记原语 | **仍然存在（本轮实证）**。`ImmediateEffect` 只有 `Damage/Heal/ModifyResource/SetFlag`（`types:158`），grep `unset_flag|UnsetFlag` 在 engine/types **0 命中**。 |
| I | 无先攻/轮次/行动经济 | **仍然存在**。突袭只落状态 + 标记。 |
| J | 对抗的「掷 vs 被动」入口 | **引擎侧已具备（本轮实证），规则包未改用**。`Intent::Check.opponent_id` 现支持「对手在世界上 → 掷 vs 掷；命中不了实例 → 静态被动值」（`session.rs::opponent_total`），`a10` 两种组合都跑通且 `opponent` 非空。`CondExpr` 仍无被动对手入口；`dnd-surprise` 仍由导演给 DC。→ **原 GAP 描述里的「crates 侧进行中」现已落地，但规则包仍按缺口写**。 |
| K | 模板级状态无声明入口 | **仍然存在**。`build_state` / `create_encounter` 建实例时 `statuses` 恒空数组。 |
| L | `host.target` 读不到目标状态 | **仍然存在**。`make_env` 只注入 `instance_id/template_id/name/kind`。 |
| M | 没有 `encounter_active` 条件 | **仍然存在**。`CondExpr` 变体仍只有 `encounter_cleared`。 |
| N | 触发点一次性（`repeatable` 未实现） | **引擎侧已被 T12 修掉，规则包侧仍存在**：`conditions::evaluate_skeleton_full` 已实现 `repeatable: true` 的**边沿**语义（并有 4 个引擎单测）；但整本 `lmop-storybook.json` grep `repeatable` = **0 命中**（24 条掷表触发点没标），且缺清标记原语（GAP-H）→ 结果不变：**24 行表项整局各触发一次**。 |

**汇总**：14 条 GAP 中，**T12 只修掉了 GAP-N 的引擎一半**（`repeatable` 语义落地）；**GAP-J 的引擎能力（对抗判定结构）也已落地但 GAP 文本未更新**；其余 12 条**全部仍然存在**。

---

## 7. T12 自报未完成项 —— 逐条复核

| # | T12 自报 | 复核结论 | 证据 |
|---|---|---|---|
| ① | LMoP 掷表 24 行仍只触发一次（引擎 `repeatable` 已实现，但规则包没标 `repeatable:true`，且没有清 flag 原语 = GAP-H） | **属实** | `grep -c repeatable story_example/lmop-storybook.json` = 0；`unset_flag` 在引擎/类型 0 命中；`a8` 实测每个触发点只 fired 一次（`fired=["tr-lmop-wander-day-12"]`，同场景后续不再出同表项）。 |
| ② | post-roll 快照不含 `expr`、`check_pre_roll` 拿不到判定签名（GAP-D） | **属实** | `LuaCheckContext`（`lua_host.rs:211-220`）字段无 `expr`；`check_pre_roll` 时 `mount_env.check=None`（`command.rs` 传 `None`），`host.check_kind/check_attribute` 均 nil。规则包因此在 `dnd-sunlight-sensitivity` 里退化为读 `host.definition.check.kind`。 |
| ③ | `CondExpr::success_level` 未实现 | **属实** | `octopus-types/src/lib.rs:118-137` 的 `CondExpr` 无 `success_level`；全仓 grep 只命中一条测试函数名 `success_levels_bucket_by_margin`。 |

---

## 8. 其它观察（非验收项，如实上报）

1. **资源经 `Add` 后变成 JSON 浮点**：`session.rs::apply_map_value` 用 `as_f64` 相加后 `Value::from(cur+add)`，于是被伤害过的 PC 投影是 `{"res-hp":21.0,"res-insp":1,"res-xp":0}`（原始输出见 A5）。后果：`Value::as_i64()` 对已变更资源返回 `None`（本报告的测试助手被迫做数值兜底）。前端大概率无感，但这是一个**真实的数据形状不一致**。
2. **文档计数与产物不符**：任务说明与 `story_example/lmop-import.md` 写「11 种 / **55 条**开放内容定义」；实际 `kinds=13`、`definitions=56`（草稿 2 种 / 23 条，规则包追加 11 种 / 33 条）。`lua_mounts=38`、地点 88、地图 7、骨架 4 章、31 怪物 + 1 PC 均与说明一致。
3. **可复现性口径**：存档 id 随机 → `rng_seed = fnv1a(save_id)`（`octopus-api/src/lib.rs:639`），骰值逐次不同。引擎用例因此全部写成**不变量**（循环到命中、结构断言、RNG 消耗计数、数据卡交叉核对），连续 3 轮复跑 12/12 全绿；注入式 provider 本身逐字节确定。
4. `world.maps` 的 `image` 留空、pins 为确定性占位布局（`lmop-import.md` 已如实标注），本轮未做底图校准——**属已知人工项，不计入本次验收**。

---

## 9. 设计成立性判断

**成立的部分（被实测证实的设计假设）**

- **「图鉴是模板库，不是常驻角色」成立**：真实开档投影只含 1 个 PC，31 条 monster 一条都没进初始实例表（验收 2）；遭遇里才按 `template_id` 克隆实例（验收 3），且**同模板多场遭遇 HP 天然隔离**（实例键带遭遇 id，验收 7）。
- **「一个内核，三个入口」成立**：玩家 strike 与怪物 enemy_strike 共用 `command::execute_skill`（同一份 Lua 挂载点 + 判定 + 效果），两侧都发 `CheckResult`；判定属性取技能声明（短剑 → dex；丧尸猛击 → str），**实测不再是硬编码力量**（验收 4/5）。难度来源也统一了：对手 AC / 目标派生 AC / `default_dc` / 12（验收 5 的 target=14 正是玩家派生 AC）。
- **「引擎定义能做什么，Lua 定义什么时候做」成立**：引擎源码禁词零命中（验收 15），而规则语义全部活着——优势 `keep_high` 让 RNG 消耗 1→2（验收 11）、熟练 +5 进 `r#mod`（验收 13）、豁免减半由 Lua 补（验收 12）、XP 由 Lua 累加且与数据卡逐字相等（验收 14）。**规则集确实可以只靠 `lua_mounts` + 开放内容交付，引擎一行不改。**
- **「签名封闭 / 语义开放」成立**：判定属性、对抗判定结构留在引擎（封闭签名）；优势/熟练/减半/XP/遭遇表全在 Lua。验收 10 证明「掷 vs 掷 / 掷 vs 被动」两种对抗都真跑通、`opponent` 落 payload。
- **「位置驱动在场 + 触发点预置遭遇」成立**：`AtLocation` 不再恒假（Move 后触发点 fired），地点驱动的 NPC 在场与按地点数人在 fixture 上得到真实数字；掷表 → flag → 触发点 → 自动建遭遇整链在**真实规则包**上跑通，`EncounterView` 带齐 scene_id/location_id/template_ids（验收 6/8）。
- **`encounter_cleared` 语义正确**：无遭遇时为假、清空后为真，正是 XP 链路能成立的前提（验收 9/14）。

**被证伪的部分（设计假设与实测不符）**

- **「规则可以完全由开放内容驱动」被证伪**：Lua 读不到挂接定义/开放内容（GAP-C），规则数据只能**生成期烘进 Lua 表**；开放内容对人类读者有效，对引擎无效。
- **「post-roll 钩子够用」被证伪**：`check_pre_roll` 拿不到判定签名（GAP-D），「按判定种类给优势」表达不了；post-roll 快照又没有 `expr`，脚本看不到骰式。
- **「豁免减半可以用既有原语精确表达」被证伪**：读不到已掷出的效果骰（GAP-E），只能重掷近似；且**自目标分支静默失效**（本报告新发现，§3-①）。
- **「掷表遭遇可以反复发生」被证伪**：`repeatable` 引擎已实现，但缺清标记原语（GAP-H）且规则包未标注（GAP-N）→ 24 行整局各一次；三猪小径打完 24 场后「不再出事」。
- **「XP 可以覆盖所有战斗」被证伪**：没有「敌人被击败」事件（GAP-F），导演即兴建的遭遇拿不到 XP；只有掷表链路能回补。
- **「预置遭遇能表达数据卡数量」被证伪**：`count` 是静态整数（GAP-G），「1d8+2 只蚊蝠」只能固定成下界。

**结论**：设计与实现**在它自己划定的边界内是自洽且可运行的**——LMoP 这本书能建、能发布、能开档、能打、能结算 XP。
但 14 条 GAP 里有 12 条**依然存在**，它们不是边缘：跨实体读取、效果骰值、失败事件、标记复位，恰好是「真实剧本里高频出现」的那一类。
所以更准确的判断是：**「规则集走 Lua」这条路是对的（本轮全部实测通过），但原语集还不够（缺只读挂接/判定签名/效果快照/击败事件/清标记），设计文档里那些「缺口」不是文档问题，是引擎能力问题。**

---

## 10. 复现清单（本轮新增文件）

| 文件 | 作用 |
|---|---|
| `docs/lmop-verification-report.md` | 本报告 |
| `crates/octopus-api/tests/lmop_verification.rs` | 12 条引擎级端到端用例（注入式 AI provider，零联网） |
| `scripts/lmop-verify-http.mjs` | 真实 HTTP 路径验证（创建/发布/开档/投影断言，11 项） |
| `scripts/lmop-boundary-check.sh` | T15 引擎边界自查（禁词零命中 + 例外说明） |

**未改动**：`crates/*/src/` 与 `frontend/src/` 的任何产品代码。
本轮新增仅上述 4 个路径（`git status` 里其余改动 / 未跟踪文件均为工作区**既有**的未提交内容，非本报告产生）。

---

## 第二轮·独立复核（T17 / T18 引擎原语 + T19 规则包）

> 角色：**全新的独立验证者**（只验证，不修产品代码）。inScope：`docs/lmop-verification-report.md` ·
> `scripts/`（只新增验证脚本） · `crates/octopus-api/tests/`（只新增测试文件）。
> **未修改** T19 改过的那两个用例（`crates/octopus-api/tests/lmop_verification.rs` 的 mtime 保持 `11:42:19.98` 未变），
> 对其审计走「读 + 跑 + 变异」。
> 被测交付物：`story_example/lmop-storybook.json`（sha256 `b49694ba3601b007b90adf622cb8748184d43e0b0f568f2408b5f0385ea63cb5`，
> 与任务给定值逐字一致）。

### R2-0. 结论摘要

| | 数量 | 说明 |
|---|---|---|
| 清单项 **通过** | **17 / 17** | 见 §R2-2 逐条验收表 |
| **未通过** | **0** | 第一轮的 2 条未通过子项：① 自目标减半已修复（§R2-2 第 6 条）；② 投影仍无 `maps`（如实登记，按设计可解释，非缺陷） |
| **未覆盖** | **0** | 17 条全部有可复现命令 + 原始输出；另有 2 处**覆盖边界**如实登记（§R2-6） |
| T19 两处测试改动审计 | **两条都判「合法，未弱化」** | 含 10 条变异验证（§R2-3）；同时登记 2 个**残余盲区**（不是弱化，是覆盖范围） |
| 14 条 GAP 第二轮现状 | **独立复核后确认：闭合 5 / 仍存在 9** | 闭合 = C / D / F / H / N；仍存在 = A / B / E / G / I / J / K / L / M（§R2-4） |

**一句话**：T17/T18 补的原语**真的把 5 条 GAP 补上了**（不是规则包自述——本轮用「改数据 → 结果变」「同一表项触发两次」
「即兴遭遇也能拿 XP」三类**变异 / 反例**独立复现）；T19 对第一轮用例的两处改动**没有弱化断言**（新断言是旧断言的严格超集）。
**但 9 条 GAP 仍在**，其中 GAP-E（效果骰值不暴露）与 GAP-A / L（跨实体读取 / 目标快照只读）仍是「真实剧本高频」的能力缺口。

### R2-1. 环境与复现命令（第二轮）

```bash
cd /home/huang/Personal/Dev/Code/octopus

# A. 真实 HTTP 路径（真实产品 API；起服务 → 建书/发布/开档/读投影 → 停服务）
./start.sh backend
node scripts/lmop-verify-http-round2.mjs     # EXIT=0，14/14 通过
./start.sh stop                              # 验证结束后停掉（8787 已释放）

# B. 引擎级端到端（注入式 provider，零联网；本轮新增独立测试文件）
cargo test -p octopus-api --test lmop_verification_round2 -- --test-threads=4 --nocapture
#   → 13 passed; 0 failed（连跑 3 轮均 13/13）

# T19 的两处测试改动审计（变异）
node scripts/lmop-verify-t19-test-audit.mjs  # EXIT=0
# T19 自己那份用例（只读不改，跑一遍确认它仍绿）
cargo test -p octopus-api --test lmop_verification   # 12 passed; 0 failed

# 交付物 / 边界
node scripts/lmop-rulepack.mjs --check       # EXIT=0，25 项断言 0 失败 + 14 条 GAP（闭合 5 / 仍存在 9）
node scripts/import-bestiary.mjs --check     # EXIT=0，28/28
./scripts/lmop-boundary-check.sh             # EXIT=0（禁词零命中）
./scripts/lmop-engine-check.sh               # EXIT=0（T18 的真实引擎校验器：0 error / 0 lua issue / 5 条规则断言全过）
```

原始输出留档（本轮）：`/tmp/r2logs/http-round2.log` · `/tmp/r2logs/round2-tests.log` ·
`/tmp/r2logs/t19-tests.log` · `/tmp/r2logs/t19-mutation-audit.log` · `/tmp/r2logs/rulepack-check.log` ·
`/tmp/r2logs/bestiary-check.log` · `/tmp/r2logs/boundary-check.log`。

### R2-2. 逐条验收表（第二轮）

| # | 清单项 | 结论 | 证据（命令 / 断言 / 原始片段） |
|---|---|---|---|
| **1** | 真实路径建书 + 发布 + 开档成功（用重新生成的故事书） | **通过** | `scripts/lmop-verify-http-round2.mjs`（EXIT=0，14/14）。首个断言即钉死交付物：`sha256 = b49694ba3601b007b90adf622cb8748184d43e0b0f568f2408b5f0385ea63cb5` 与任务给定值一致。`POST /api/storybooks → 201`（`sb-cbf3606ad30b464d8ccd5f22127b14b0`）、`PUT → 200`、`POST /publish → 200 revision=1 errors=undefined`、`POST /api/saves → 201 embedded_revision=1`（`sv-ed129f94803b4adfb289783cea3acda9`）。**注**：既有 `octopus.db` 里 `octopus/octopus` 口令已被改过（401），脚本改走真实注册端点开验证账户后继续——两条都是真实产品路径。整条链路跑了 **2 次**，都 14/14。 |
| **2** | 投影不含 `kind="monster"` 初始实例 | **通过** | HTTP：`character kinds = {"pc":1}`、`keys=inst-pc-lmop-talin`、`encounters=[]`。引擎：`r2_1_2_7` → `kinds={"pc": 1}`、`proj.characters.len()==1`（31 条图鉴模板一条都没进实例表）。 |
| **3** | 遭遇克隆：灰烬丧尸 AC 8 / HP 22 / 六维 13,6,16,3,6,5 | **通过** | `r2_3_encounter_clone_matches_datacard`：`count=2` 展开两只；`{"ac":8,"hp":22,"max":22,"template_id":"mon-ash-zombie"}`；两实例 `attributes={"str":13,"dex":6,"con":16,"int":3,"wis":6,"cha":5}`。AC **由故事书自算**（`10 + floor((dex-10)/2) + 挂接 monster-armor 修正 1 = 8`），不采信引擎返回值。 |
| **4** | 玩家攻击：判定属性来自技能声明（非硬编码 str）；伤害扣实例 hp；发 CheckResult | **通过** | `r2_4_and_8`：先读出 `skills[sk-lmop-shortsword].attribute == "dex"`，再断言 `CheckResult.attribute == declared && != "str"`；`kind=Attack`、`expr=Some("1d20")`、`rolls=[13]`。命中回合实例 `{"res-hp":16}`（<22），且 `EncounterView.enemies[0].hp` 与实例同步。**反面对照**：`r2_5` 里灰烬丧尸猛击 `attribute=="str"`——两个方向各自取自己技能声明的属性。 |
| **5** | `enemy_strike`：难度 = 玩家派生 AC | **通过** | `r2_5_enemy_strike_targets_player_derived_ac`：`CheckResult.target == 14`，且 14 是测试**自己从故事书算的**（PC dex 16 → +3；`attachments.monster-armor → armor-lmop-talin-leather.modifiers[target=ac] +1`）。40 回合内命中一次，PC `res-hp 24 → 18`。 |
| **6** | **原未通过①**：自目标（不给 `target_id`）「豁免成功减半」不再静默失效 | **通过（已修复）** | `r2_6_self_target_save_half_is_not_silently_dropped`，三组对照：**A 自目标**（`target_id: None`）成功回合 `dice=4`、伤害 delta **1..9**（实测 6）；**B 显式目标**（`Some("pc-lmop-talin")`）同样 `dice=4`、delta 1..9（实测 6）；**C 负对照**（`target_id: Some("r2-不存在的目标")` → `host.target == nil`）成功回合 **delta = 0**。原始输出：`R2-6 PASS: 自目标成功=(6, 4) 显式目标成功=(6, 4) 负对照=delta 0`。C 组正是修复前的失效形态，用来证明 A 组断言不是恒真。 |
| **7** | **原未通过②**：投影是否仍无 `maps` 字段 | **通过（按设计可解释，如实登记）** | 引擎：`R2 投影键 = ["characters","controlled","encounters","flags","locations","meta","progress","quests","scene_id","scene_title","seq"]`，`has maps == false`。HTTP 第 5b 项同样为 false；而 `GET /api/saves/{id}` 的故事书层 `world.maps = 7 / pins = 93`，坐标全在 0..1、`location_id` 全部命中地点表（越界 0、悬空 0）。**结论**：第二轮仍未下发 `maps`；若「投影要下发地图」是硬要求则仍不成立，若前端读故事书（现状）则属口径——本报告不替设计做判断。 |
| **8** | 资源浮点：战斗受伤后投影里的资源仍是 JSON 整数 | **通过（已修复）** | 形态证据（原始 JSON 串，不是 `as_i64` 兜底）：受伤的敌人实例 `instance_res={"res-hp":16}`；**被打中的 PC** `pc_resources_raw={"res-hp":18,"res-insp":1,"res-xp":0}`。断言 `res["res-hp"].as_i64().is_some()` 且序列化串**不含小数点**。第一轮实测的 `21.0` 形态已不复现。 |
| **9** | 判定签名：`check_pre_roll` 能读到 attribute/kind，且能据此区分检定类别 | **通过** | 双证据。①**探针挂载点**（用例往内存副本里追加一条 `check_pre_roll` 脚本，把签名写成标记）：dex 检定后 `flags` 有 `r2-kind-attribute=true`、`r2-attr-dex=true`，且**没有** `r2-kind-nil` / `r2-attr-nil`——掷骰前就能读到签名。②**交付规则包真的用它分类**：同一次会话里 dex 属性检定 `r#mod=8`（+5 熟练）、str 属性检定 `r#mod=0`（没有 str 的熟练定义）——按签名区分，不是无差别加值。原始输出：`R2-9/12 PASS: c1_mod=8 c2_mod=0 c3_dice=2`。另代码复核：`LuaCheckContext` 新增 `resolved: bool` 与 `expr: Option<String>`（`lua_host.rs:218-233`），第一轮登记的「post-roll 快照不含 expr」也已补上。 |
| **10** | 触发点预置遭遇 + `encounter_cleared` 条件 | **通过** | `r2_10_trigger_preset_encounter_and_encounter_cleared`。①无遭遇时 `eval_cond(EncounterCleared)==false`。②PC `Move` 到 `loc-lmop-0b7` 后 400 回合内掷表建遭遇：`scene_id == proj.scene_id`、`location_id=="loc-lmop-0b7"`、`template_ids==["mon-hobgoblin"]`——并且**与故事书里该触发点 `encounter.enemies` 的模板集合逐条比对相等**（不采信引擎摘要）；`progress.triggers` 有 `tr-lmop-wander-*` fired。③移开三猪小径后逐只清剿，之后 `eval_cond(EncounterCleared)==true`（原始输出 `cleared=true`）。 |
| **11** | 对抗判定：掷 vs 掷 与 掷 vs 被动都跑通，`opponent` 非空 | **通过** | `r2_11_opposed_check_both_modes`：掷 vs 掷 `opponent={id:"mon-ash-zombie",name:"灰烬丧尸甲"}`、`dice_count=2`；掷 vs 被动 `opponent={id:"mon-goblin",name:"地精"}`、`dice_count=1`。 |
| **12** | 优势 `keep_high` 的 RNG 消耗为 2 | **通过** | `r2_9_and_12`：同一 `dex` 属性检定，无激励 `dice_count=1`；施加 `dnd-inspired` 后 `dice_count=2`（`dnd-status-keep-high` → `modify_check('keep_high')`），且用掉后状态消失。 |
| **13** | **熟练加值数据驱动**：只改故事书 definition 的加值 → 检定总值随之变（GAP-C 关键证据，独立复现） | **通过（独立复现）** | `r2_13_proficiency_bonus_is_data_driven`：基线 `r#mod=8`（dex_mod 3 + 熟练 5）；把内存副本里 `definitions[prof-pc-lmop-talin-dex].fields.bonus` 从 `"5"` 改成 `"9"`（**只改这一处**，其余逐字不动）→ 发布后同一检定 `r#mod=12`。原始输出：`R2-13 PASS: 基线 r#mod=8 → 改 bonus 5→9 后 r#mod=12`。**实现里不可能有烘死的 5**。 |
| **14** | **掷表遭遇可反复**：同一表项触发两次以上（GAP-N + GAP-H 关键证据，独立复现） | **通过（独立复现）** | `r2_14_wander_table_item_refires`：统计每个 `tr-lmop-wander-*` 的 **active 上升沿**（条件假→真才计数）。原始输出：`上升沿统计（19 回合）：{"tr-lmop-wander-day-10":1,"tr-lmop-wander-day-12":1,"tr-lmop-wander-day-4":1,"tr-lmop-wander-day-9":2}` → `tr-lmop-wander-day-9` 触发 **2 次**。另两轮复跑分别在 13 / 12 回合内让 `day-8`、`day-9` 各自触发 2 次——**三轮都出现了同一表项的第二次触发**。 |
| **15** | **XP 由 `enemy_defeated` 发放**：用不在任何掷表行里的模板造即兴遭遇也能拿 XP（GAP-F 关键证据） | **通过（独立复现）** | `r2_15_improvised_encounter_awards_xp_by_template`：先**从故事书自己枚举**全部掷表行的 `template_id`（8 个：stirge / ogre / goblin / hobgoblin / orc / wolf / owlbear / ghoul），断言 `mon-skeleton` 不在其中；再用 `Intent::Encounter` 现造一场，打死后 `res-xp 0 → 50`（`= mon-skeleton.statblock.xp`）。反证：全程 `dnd-wander-*` 标记全假、没有任何 `tr-lmop-wander-*` fired → XP 不是借道掷表链路。 |
| **16** | **38 → 16 的合并没有丢行为**：31 条模板的 XP 定义与图鉴 `statblock.xp` 逐条相等（独立核对） | **通过（独立核对）** | `r2_16_every_template_xp_definition_equals_statblock`：31 条 monster **每条恰好 1 条** `dnd-xp-award` 定义，`fields.xp` 与 `fields.creature_name` 与 `statblock.xp` / `name` **逐字相等**；无缺失、无多余（`per_tpl.len()==31`、`defs.len()==32` = 31 逐模板 + 1 汇总表 `xp-lmop-table`）；再把汇总表的 `fields.table`（`名称=数值；…`，31 段）逐段解析回图鉴比对，31/31 相等。原始输出：`R2-16 PASS: 31 条模板 XP 定义 + 汇总表全部与图鉴 statblock.xp 相等；defs=32`。 |
| **17** | 引擎边界：`crates/octopus-engine/src` 与 `crates/octopus-types/src` 禁词零命中 | **通过** | `./scripts/lmop-boundary-check.sh` EXIT=0（`advantage / proficiency / on_save / save_ends / ConditionalModifier / EncounterTable / extends` 零命中；既有例外 `CheckKind::Save` 如实列出）。**另做独立 grep**（更宽词表）：`advantage / proficiency / on_save / save_ends / ConditionalModifier / EncounterTable / extends / disadvantage / saving_throw / dnd-` 在两个 crate 全部 **0 命中**；`keep_high / keep_low / enemy_defeated / encounter_cleared / repeatable / modifier_formula` 有命中但都是**引擎通用原语名**（不是规则集词汇）。`lmop` / `mon-` 的命中（engine 41 / types 2）**全部位于 `#[cfg(test)]` 模块内**（`resolve.rs` 的 `#[cfg(test)]` 从 448 行起、`lua_host.rs` 测试模块、`conditions.rs:723`、`types/lib.rs:1878/1915`）——产品代码零命中。 |

### R2-3. T19 两处测试改动的审计结论

判定标准（照任务要求）：**不是「测试通过了」，而是「把实现改坏，这条测试会不会失败」**。
审计手段：①读改动前后的 diff；②跑 T19 那份用例（只读不改）；③**变异验证**——
`scripts/lmop-verify-t19-test-audit.mjs` 把改后的 `a1_a2` 断言段**逐字复刻**成可执行代码，
对故事书内存副本施加 9 组变异（EXIT=0）；`a9_a14` 的运行时断言则用本轮新增的
`r2_mut1_a9_xp_assertions_have_teeth` 做三种实现形态的对照（EXIT=0）。

#### 改动 1 —— `a1_a2`：`lua_mounts.len() == 38` → `== 16`

改后断言＝**5 条**：①`len() == 16`；②存在 `dnd-xp-award` 且 `mount == "event"` 且源码含 `enemy_defeated`；
③存在 `dnd-wander-reset` 且 `mount == "event"` 且源码含 `clear_flag`；
④不存在 id 以 `dnd-xp-day` / `dnd-xp-night` 开头的挂载点；⑤（配套）`monster=31 / pc=1` 等形状断言未动。

变异验证原始输出（`/tmp/r2logs/t19-mutation-audit.log`，EXIT=0）：

```text
[OK]  M0 未变异基线 → 断言 PASS（期望 PASS）
[OK]  M1 删除 XP 挂载点 dnd-xp-award → 断言 FAIL · 挂载点数量 != 16（实际 15）
[OK]  M2 把 dnd-xp-award 从 event 挪到 turn_end → 断言 FAIL · XP 必须挂在 event 上（实际 turn_end）
[OK]  M3 把 dnd-xp-award 的开闸条件从 enemy_defeated 改成 scene → 断言 FAIL · XP 规则必须按 enemy_defeated 开闸
[OK]  M4 删除掷表复位挂载点 dnd-wander-reset → 断言 FAIL · 挂载点数量 != 16（实际 15）
[OK]  M5 把 dnd-wander-reset 的 host.clear_flag(flag) 整句换成 no-op → 断言 FAIL · 掷表复位规则必须清标记
[OK]  M6 重新塞回一条旧的按掷表行回补 XP（dnd-xp-day-1）→ 断言 FAIL · 挂载点数量 != 16（实际 17）
[OK]  M6b 数量不变、但把 dnd-wander-reset 换成一条旧的按掷表行回补 XP → 断言 FAIL · 缺少关键挂载点 dnd-wander-reset
[OK]  M7 残余盲区：删掉一条未被点名的规则挂载点（dnd-proficiency）+ 补一条合法占位脚本 → 断言 PASS（盲区）
[OK]  M8 残余盲区：复位规则仍在，但把要清的 flag 写死成单行 → 断言 PASS（盲区）
-- 对照：第一轮的旧断言（len()==38）在**未变异的当前交付物**上 → FAIL（实际 16）
```

**结论：合法，未弱化。** 新断言是旧断言的**严格超集**——旧断言唯一能抓的「挂载点数量变动」新断言照样抓
（M1 / M4 / M6 全部因数量变化失败），此外还多了 4 条结构性断言。
「关键挂载点缺失 / 被移除」**确实会被抓住**：M1（删 XP 挂载点）、M4（删复位挂载点）失败；
M6b（数量不变、复位被换成旧回补规则）也失败（缺 `dnd-wander-reset`）。
M2（挪到错误时机）、M3（开闸事件错）、M5（复位原语被删）失败，说明「挂错地方」也会被抓——这正是新断言相对旧断言的增益。

**如实登记的残余盲区（不是本轮弱化，旧断言同样没有这条覆盖）**：

| 变异 | 结果 | 说明 |
|---|---|---|
| M7 删掉一条**未被点名**的挂载点（如 `dnd-proficiency`）再补一条合法占位脚本 | 断言 **PASS**（盲区） | 新断言只点名 2 条关键挂载点 + 一个总数；其余 12 条规则脚本没有逐条点名。旧断言（`len==38`）在同样的「删一补一」下也会 PASS——覆盖范围相当，非退化。 |
| M8 复位规则仍在，但把要清的 flag 写死成 `dnd-wander-day-1` | 断言 **PASS**（盲区） | 断言只检查源码里出现 `clear_flag` 字面量，不检查清的**是哪一个** flag。该盲区由**行为测试**兜住：T19 的 `a9_a14`「行标记必须复位」+ 本轮 `r2_14`「同一表项必须触发 ≥2 次」——写死单行会让其它行永远只触发一次。 |

#### 改动 2 —— `a9_a14`：`「清空遭遇后 XP 增量 == 行口径」` → `「逐只按数据卡发放 + 清空后增量为 0」`

改后断言＝**6 条**：①`expected_total > 0`（防恒真）；②每杀一只 → XP 增量 **恰好等于** 该敌模板
`statblock.xp`；③敌人已倒下但 XP 没变 → `panic!`（"enemy_defeated 链路断了"）；
④整场清剿总增量 == 遭遇内每只敌人 `statblock.xp` 之和；⑤清空后再跑一回合 → 增量 **== 0**；
⑥所有 `dnd-wander-*` 行标记必须为 false，且行触发点的 `active` 必须为 false。

变异验证（`r2_mut1_a9_xp_assertions_have_teeth`，同一套 XP 口径跑三种实现形态）：

```text
R2-MUT1 PASS: 基线 kill=50/post=0；变异B kill=0；变异C post=50
```

| 形态 | 实现 | 观测 | 对 a9_a14 断言的含义 |
|---|---|---|---|
| A 基线 | 当前实现（`enemy_defeated` 逐只发） | `kill_delta = 50`、`post_clear = 0` | 断言②④⑤ 全过 |
| B 「敌人倒下了却不发 XP」 | 删掉 `dnd-xp-award` 挂载点 | `kill_delta = 0` | 断言②／③ 必然 **FAIL**（`awarded` 永远为 false → 断言失败） |
| C 「清空后仍在回补」 | 加回 `turn_end + when: encounter_cleared → modify_resource +50` | `post_clear = 50` | 断言⑤ 必然 **FAIL**（"清空后不得再补发 XP"） |

**结论：合法，未弱化。** 新断言**不是恒真**：B 让它挂在"逐只发 XP"、C 让它挂在"清空后增量为 0"，
两条都对应真实的坏实现。相对旧断言（只有一条 post-clear delta == 行回补期望）：
新断言把口径拆成"**逐只金额 + 合计 + 清空后为 0 + 标记复位**"四条互相独立的断言，并加了
`expected_total > 0` 与"已倒下必须发 XP"两道防恒真闸门——**信息量更多，不是更少**。
另外它也补上了旧断言覆盖不到的一类缺陷：旧口径只看掷表行，**导演即兴遭遇的 XP 根本不在其射程内**；
新断言配合本轮第 15 条（即兴遭遇拿 XP）把那一半也钉住了。

**口径变更的理由本身是否成立？** 成立，且不是"为了让测试变绿"：
"XP 改由 `enemy_defeated` 逐只发"这件事被本轮**独立复现**——第 15 条（不在任何掷表行里的
`mon-skeleton` 即兴遭遇也拿到 50 XP）、第 16 条（31 条模板 XP 定义与图鉴逐条相等）。
若只是把测试改成恒真，第 15 / 16 条不会有这些结果。

#### R2-3 小结

| 改动 | 判定 | 依据 |
|---|---|---|
| `a1_a2`：38 → 16 | **合法（未弱化）** | 新断言是旧断言的严格超集；删除 / 挪位 / 改开闸事件 / 删复位原语 / 塞回旧回补规则 6 类变异全部 FAIL。残余盲区 2 处（M7 / M8），旧断言同样没有覆盖，且 M8 有行为测试兜底。 |
| `a9_a14`：行口径 → 逐只发放 | **合法（未弱化）** | 3 种实现形态对照：坏实现 B 挂在②／③、坏实现 C 挂在⑤，证明断言不是恒真；断言条数与覆盖面都比旧版多。 |

### R2-4. 14 条 GAP 的第二轮现状（独立复核，不是照抄 T19 自述）

复核方法：对每条 GAP 的**原始诉求**去 `crates/*/src` 找对应原语（有 / 无，带行号），再用本轮
的运行时用例反向确认「规则包真的用上了它」。规则包自报「闭合 5 / 仍存在 9」——

**独立结论：数目一致（5 闭合 / 9 仍存在）**，但下面逐条的「卡在哪」是本轮自己的取证，不是抄自述。

| GAP | 主题 | 第二轮现状 | 独立证据 |
|---|---|---|---|
| **A** | Lua 读不到其它实体 | **仍存在** | `get_character` / `get_encounter` / `get_flag` 在 engine+types **0 命中**；`host.target` 仍只注入 `instance_id / template_id / name / kind` 四个键（`lua_host.rs:801`）。T17 新增的只读口子读的是**开放内容**（故事书静态数据），不是运行时实体——「盟友在目标 5 尺内」仍只能靠 `dnd-flanked` 标记。 |
| **B** | 无位置 / 距离 / 区域概念 | **仍存在** | engine 里 `distance` **0 命中**；位置仍只有 `location_id` 归属。 |
| **C** | 挂接 / 开放内容不进 Lua | **已闭合** | 新增 `host.get_attachments() / get_attachment(kind) / get_definition(id) / list_definitions(kind?)`（`lua_host.rs` 的 read_data 快照 + 引擎单测 `open_content_read_api_exposes_attachments_and_definitions`）。**运行时反证**：本轮 `r2_13` 只改定义里的 `bonus 5→9`，检定 `r#mod 8→12`——数值真的从故事书读，不是烘死在 Lua 里。 |
| **D** | `check_pre_roll` 拿不到判定签名 | **已闭合** | `LuaCheckContext` 现含 `attribute / kind / resolved / expr`（`lua_host.rs:218-233`），且 `check_pre_roll` 真的下发签名（`session.rs:5113-5124`）。**运行时反证**：本轮探针挂载点读到 `kind=attribute / attribute=dex` 且**没有** nil 标记；交付规则包据此分类（dex 检定 +5、str 检定 +0）。附带：第一轮登记的「post-roll 快照不含 expr」也已补上（`expr` 字段在场）。 |
| **E** | 效果骰值不暴露（豁免减半的数值等价） | **仍存在** | `pending_effect` / `last_effect` 在 engine+types **0 命中**；`dnd-save-half` 的源码仍是「读 `host.definition.effect` 的骰式**重新掷一次**再取半」。本轮 `r2_6` 实测成功回合 `dice=4`（1 颗 d20 + 3 颗重掷 3d6）——与第一轮 §5 的口径逐条一致，期望值等价但不是同一颗骰。 || **F** | 无「敌人被击败」事件 | **已闭合（有明确边界）** | `session.rs:2701` 在 strike 结算内部派发 `enemy_defeated`（带 `data.enemy.template_id` / `instance_id` / `name` + 遭遇事实）。**运行时反证**：`r2_15` 用**不在任何掷表行**里的 `mon-skeleton` 造即兴遭遇，同样 `+50 XP`。**边界（如实登记）**：该事件只在 `strike_enemy` 路径派发；Lua 直接把敌人资源打到 0（不走 strike）不会触发，也不发 XP——规则包 `--check` 自己也登记了这条边界。本轮**只做代码复核**，未做该路径的运行时反例（见 §R2-6 覆盖边界）。 |
| **G** | 预置遭遇数量是静态整数 | **仍存在** | `EncounterPresetEnemy.count: Option<u32>`（`octopus-types/src/lib.rs:847`）——仍无骰式落点；规则包仍把每行固定成骰式下界，原始骰式写进 note / hint。 |
| **H** | 没有清标记原语 | **已闭合** | `ImmediateEffect::SetFlag` 现带 `value: Option<Value>`（`types:170-180`，缺省 = true 逐字沿用旧行为），Lua 侧有 `host.set_flag(flag[, value]) / host.clear_flag(flag)`（`lua_host.rs`）。`unset_flag / UnsetFlag` 在引擎里仍是 0 命中——因为根本不需要（有值即可清除）。**运行时反证**：`r2_14` 依赖行标记被清掉才可能出现第二次上升沿。 |
| **I** | 无先攻 / 轮次 / 行动经济 | **仍存在** | `initiative` 唯一命中是 `validate.rs:2948` 的测试数据字符串；没有先攻序、没有行动经济。突袭仍只落状态 + `dnd-battle-round-N` 标记。 || **J** | 对抗的「掷 vs 被动」入口 | **仍存在（引擎能力已具备，GAP 文本所指的条件入口仍缺）** | 引擎侧 `Intent::Check.opponent_id` 已能表达两种对抗：本轮 `r2_11` 复现「掷 vs 掷（`dice=2`）」与「掷 vs 被动（`dice=1`）」，`opponent` 都非空。但 `CondExpr` 仍**没有**「掷骰 vs 对手被动值」的入口（`AttributeGe` 只看 actor 自己），规则包仍由导演给 DC。第一轮对这条的判断（引擎已落地 / 规则包未改用）本轮维持。 |
| **K** | 模板级状态无声明入口 | **仍存在** | `octopus-api/src/lib.rs` 建实例时 `statuses: vec![]`（520 / 591 行）——怪物模板自带状态仍落不了地。 |
| **L** | `host.target` 读不到目标状态 | **仍存在** | `lua_host.rs:801` 仍只注入 `instance_id / template_id / name / kind`；不含 `statuses / attributes / resources`。伏击的「对受其突袭的生物发动攻击」仍只能靠 `dnd-surprised` 标记近似。 |
| **M** | 没有 `encounter_active` 条件 | **仍存在** | `CondExpr` 变体（`types:118-137`）仍只有 `EncounterCleared`；`EncounterActive / encounter_active` 在 engine+types **0 命中**。战斗轮计数继续靠 `dnd-in-combat` 标记开闸。 |
| **N** | 触发点一次性（`repeatable` 未实现） | **已闭合** | `conditions.rs::evaluate_skeleton_full` 的边沿语义 + 规则包把 24 个掷表触发点全部标 `repeatable: true` + `dnd-wander-reset` 在 `encounter_cleared` 时 `clear_flag`。**运行时反证**：`r2_14` 连续三轮都在十几回合内观察到**同一表项第二次触发**（day-9 ×2 / day-8 ×2 / day-9 ×2）。 |

**汇总**：闭合 **5**（C / D / F / H / N）／仍存在 **9**（A / B / E / G / I / J / K / L / M）。
与 T19 自述的数目一致，但上面每条的「卡在哪」都带本轮自己的行号 / 反证。
第一轮「14 条里 12 条仍存在」的判断，在第二轮被更新为 **9 条仍存在**；被推翻的那 3 条是 C / D / F（连同 H / N 共 5 条）。

### R2-5. 这套设计成立性的第二轮判断

第一轮的判断是「**路是对的，但原语集不够**」。第二轮在补了 6 类原语之后重新判断：

**第一轮的结论中被本轮证实的**

- **「图鉴是模板库，不是常驻角色」**——再次证实（HTTP + 引擎双路径：投影只有 1 个 PC，31 条 monster 全在模板层）。
- **「一个内核，三个入口」**——再次证实：玩家 strike（`attribute=dex`，取自技能声明）与怪物 enemy_strike（`attribute=str`，取自它自己的技能声明 + 难度 = 玩家派生 AC 14）走同一条结算路径。
- **「引擎定义能做什么，Lua 定义什么时候做」**——**更强地**证实了：本轮不仅看到规则活着（优势 2 颗骰、熟练 +5、豁免减半、掷表、XP），还第一次证明**规则的数值真的来自开放内容**（改一个 definition 的 bonus → 检定总值 8→12）。
- **「签名封闭 / 语义开放」**——再次证实，且比第一轮更强：判定签名（attribute / kind / resolved / expr）现在是**掷骰前**就下发的封闭签名，规则包据此分类（dex +5 / str +0）。
- **`encounter_cleared` 语义**——再次证实（无遭遇假、清空真）。
- **「位置驱动在场 + 触发点预置遭遇」**——再次证实。

**第一轮「被证伪」的结论中，本轮被推翻的（即：设计假设其实是对的实现落后了）**

| 第一轮被证伪的假设 | 第二轮 |
|---|---|
| 「规则可以完全由开放内容驱动」 | **推翻该证伪**：`get_attachments / get_attachment / get_definition / list_definitions` 落地，规则数值运行时读故事书；变异实验（bonus 5→9 → r#mod 8→12）证明没有烘死常量。**但**「当前角色模板」以外的实体仍读不到（GAP-A），所以准确说法是「开放内容作为规则数据源」成立，「开放内容 + 跨实体事实」仍不成立。 |
| 「post-roll 钩子够用」 | **部分推翻**：GAP-D 闭合——`check_pre_roll` 有签名，`check_post_roll` 有 `expr`。 |
| 「掷表遭遇可以反复发生」 | **推翻该证伪**：24 个触发点 repeatable + `clear_flag` 复位，三轮实测都观察到同一表项二次触发。 |
| 「XP 可以覆盖所有战斗」 | **部分推翻**：`enemy_defeated` 逐只发 XP 覆盖**所有经 strike 击杀的遭遇**（含导演即兴）；**但**「非 strike 击杀不发」仍是边界。 |
| 「豁免减半可以用既有原语精确表达」 | **仍被证伪**：GAP-E 未闭合，仍是重掷近似（4 颗骰、不是同一颗）。唯一被修的是自目标分支不再静默失效。 |
| 「预置遭遇能表达数据卡数量」 | **仍被证伪**：GAP-G 未动。 |

**仍然未知 / 未闭合的（第二轮没有新证据推翻第一轮的判断）**

- 跨实体读取（GAP-A）、位置 / 距离（GAP-B）、效果骰值（GAP-E）、数量骰式（GAP-G）、
  轮次经济（GAP-I）、`CondExpr` 的被动对手入口（GAP-J）、模板初始状态（GAP-K）、
  `host.target` 快照（GAP-L）、`encounter_active`（GAP-M）。
- **第一轮的两条「未通过子项」**：① **已修复**（自目标减半，含负对照）；② **仍未下发 maps**（如实登记，按设计可解释）。

**第一轮 §8 三条附加观察的第二轮现状**

1. **资源经 `Add` 变浮点** → **已修复**。`add_numbers` 保持 JSON 整数（`session.rs:5733-5767`）；实测形态 `{"res-hp":18,"res-insp":1,"res-xp":0}`。
2. **文档计数** → **大部分已改正**：`lmop-import.md` 现写「16 条挂载点 / 整本 13 种 88 条」，与本轮实测一致。
   仍有一处**口径瑕疵**（低severity）：同处写「规则包追加 **11 种 / 64 条**」，而实测 `rulepack 追加 = 88 − 23 = 65 条`
   （草稿 23 条里**不含**皮甲；皮甲 1 条也是规则包加的）。不影响任何断言，纯文档口径。
3. **`rng_seed = fnv1a(save_id)`（骰值逐存档不同）** → **仍成立**，并且本轮有直接旁证：
   `r2_14` 的三次复跑分别命中了**不同的**掷表行（day-9 / day-8 / day-9），说明每局的骰序不同；
   因此本轮全部断言同样写成不变量（循环到命中 / 结构断言 / RNG 消耗计数 / 数据卡交叉核对），而非固定骰面。

**结论（第二轮）**：设计**在它自己划定的边界内成立**，而且边界**明显变宽了**——
「规则集走 Lua + 开放内容」这条主路现在有了「数据驱动」的实证（不只是样例）；
但「跨实体读取 / 目标快照 / 效果骰值」这三类**读不到东西**的缺口依旧，
它们不是边缘：真实剧本里「狼群有盟友在侧就占优」「伏击只对受突袭者生效」「豁免成功伤害减半」全都踩在上面。

### R2-6. 未通过 / 未覆盖项清单（不得隐瞒）

**未通过：无（0 项）。**

**未覆盖：无（0 项）**，但有 **2 处覆盖边界**必须声明：

1. **GAP-F 的非 strike 击杀路径**：`enemy_defeated` 只在 `strike_enemy` 内派发（`session.rs:2701`，位于 strike 结算体内）；
   Lua 直接把敌人资源打到 0 不会发 XP。本轮**只做了代码复核**（读出派发点位置），**没有**构造该路径的运行时反例。
2. **边界 grep 的 `#[cfg(test)]` 范围**：`lmop` / `mon-` 在 engine / types 有命中，逐条核对后**全部落在测试模块内**，
   但它们确实在「整文件 grep」的计数里——所以「产品代码零命中」这一结论是我**逐条读上下文**得出的，不是计数器直接给出的。

**其它如实登记（非验收项）**：

- 真实 `octopus.db` 的默认管理员口令已被改过（`octopus/octopus` → 401），本轮验证因此**注册了一个验证账户**
  （`verify2probe` / `verify2-*`）并在该库留下 3 本验证故事书与 3 个存档。这是**对既有数据库的副作用**，如实上报（未删除，也未改动任何产品数据）。
- `world.maps` 仍是占位布局（第一轮已登记，属已知人工项）。

### R2-7. 本轮新增 / 改动文件

| 文件 | 作用 |
|---|---|
| `docs/lmop-verification-report.md` | 本报告（第一轮 §0–§10 **原样保留**，第二轮 §R2 追加） |
| `crates/octopus-api/tests/lmop_verification_round2.rs` | **新增**：13 条独立引擎级端到端用例（含负对照与变异对照） |
| `scripts/lmop-verify-http-round2.mjs` | **新增**：真实 HTTP 路径第二轮验证（14 项断言） |
| `scripts/lmop-verify-t19-test-audit.mjs` | **新增**：T19 两处测试改动的变异审计 |

**未改动**：`crates/*/src/` 与 `frontend/src/` 的任何产品代码；
**未改动** T19 改过的那两个用例（`crates/octopus-api/tests/lmop_verification.rs` 的 mtime 仍是 `11:42:19.98`）。
本轮只**新增**了上面 3 个文件 + 追加本节。第一轮的 §0–§10 未删改一字（除在文首加了一段指向第二轮的导航）。

---

## 第三轮·独立复核（T21 / T22 引擎原语 + T23 规则包）

> 角色：**全新的独立验证者（第三轮）**（未参与 T21 / T22 / T23 的任何实现）。只验证，不修产品代码。
> inScope：`docs/lmop-verification-report.md` · `scripts/`（只新增） · `crates/octopus-api/tests/`（只新增）。
> **未修改**前两轮的用例：`lmop_verification.rs` / `lmop_verification_round2.rs` 逐字未动（只读 + 跑 + 变异推理）。
> 被测交付物：`story_example/lmop-storybook.json`，sha256 `f7b15b723c9f38ab90fdf4487c5dce56931db0fe3d7ae6272d8f738375d29bc2`（T23 重新生成后的版本；与第二轮验的 `b49694ba…` **不是同一版**）。
> 查询口径：**不采信 T23 自述**；四条专门审计全部独立复现，前两轮用例重跑只为确认「没被改坏」。

### R3-0. 结论摘要

| | 数量 | 说明 |
|---|---|---|
| 清单项 **通过** | **19 / 19** | 见 §R3-2 逐条验收表 |
| **未通过** | **0** | —— |
| **未覆盖** | **0** | 19 条全部有可复现命令 + 原始输出；另有 3 处**覆盖边界**如实登记（§R3-6） |
| 四件专门审计 | ①通过 ②通过（含 1 处**改善**）③**注释过期**（断言仍有牙齿，但一条机制描述已失实）④**近似边界 5 类错误结论** ｜ **⚠️ 第四轮（§R4）：③④ 已由 T26 修掉并被独立复现**（③ 的注释/判据已订正、④ 的 5 类 → 0/0/0/0/1） |
| 14 条 GAP 第三轮现状 | **闭合 7 / 近似提高 1 / 仍存在 6** | 独立复核（§R3-4），**数目与 T23 自述一致**，但逐条证据是本轮自己取的 |

**一句话**：第三轮补的 `scale_effect` / `get_character` 等原语**是真的**——我独立复现了「同一颗效果骰被缩放」（32 个种子骰序逐位相同、半值恰为向零取整、引擎产物里 1 条 hp delta 而 Lua 无伤害请求），也独立确认了「多段效果 + modifiers 全部被缩放」这件**重掷做不到**的事。T23 的连带修正**保住了行为**（成功=半伤不倒地、失败=满伤+倒地），并且顺手修掉了一处旧实现的位置错误（附加状态从前落在**施法者**，现在落在**目标**）。
**但有两处不能替它圆场**：a12 / a12b 的注释与 r2_6 负对照的**失效机制描述已经过期**——前者只是措辞，后者是**事实错误**（减半分支其实发了、引擎也结算了，只是 delta 落在不存在的实体上被静默丢弃）。集群战术的「近似提高」标签是**诚实**的：我实测出 5 类**错误结论**（同伴在别处、同伴失能、无地点、非攻击判定、同伴在另一场遭遇），而且开放内容自己声明的 `range_proxy`（「与目标同一 location_id」）**脚本根本没实现**——它只比了「狼 vs 目标」的地点，从没看同伴在哪。

> **⚠️ 第四轮订正（T26 已修）**：本段列出的问题**全部已修**（见 §R4-1 / §R4-2 / §R4-3）：
> a12/a12b 的注释与判据、r2_6 的失效机制描述、集群战术的 5 类错误结论与 `range_proxy` 实现，均已订正；
> 第四轮已用旧脚本原文独立复现上述「修正前」的每一条，确认它们**当时确实为真**。

### R3-1. 环境与复现命令（第三轮）

```bash
cd /home/huang/Personal/Dev/Code/octopus

# A. 真实 HTTP 路径（真实产品 API：起服务 → 建书/发布/开档/读投影 → 停服务）
./start.sh backend
node scripts/lmop-verify-http-round3.mjs      # EXIT=0，13/13 通过
./start.sh stop                               # 验证结束后停掉（8787 已释放，pgrep octopus-bin 空）

# B. 引擎级端到端（注入式 provider，零联网；本轮新增独立测试文件）
export CARGO_TARGET_DIR=$PWD/.scratch/engine-test-target
cargo test -p octopus-api --test lmop_verification_round3 -- --test-threads=4 --nocapture
#   → 14 passed; 0 failed（连跑 3 轮均 14/14）

# 前两轮的用例：只读不改，原地重跑（确认 T21/T22/T23 没改坏它们）
cargo test -p octopus-api --test lmop_verification        # 12 passed; 0 failed
cargo test -p octopus-api --test lmop_verification_round2 # 13 passed; 0 failed

# 交付物侧自检
node scripts/lmop-rulepack.mjs --check        # EXIT=0，28 项断言 0 失败 + 14 条 GAP（闭合 7 / 近似 1 / 仍存在 6）
node scripts/import-bestiary.mjs --check      # EXIT=0，28/28
./scripts/lmop-boundary-check.sh              # EXIT=0（禁词零命中）
./scripts/lmop-engine-check.sh                # EXIT=0（0 error / 0 lua issue / 规则断言 0 失败）
```

原始输出留档（本轮）：`/tmp/r3logs/round3-tests.log` · `/tmp/r3logs/prev-rounds.log` · `/tmp/r3logs/audit3a-mutations.log` ·
`/tmp/r3logs/rulepack-check.log` · `/tmp/r3logs/bestiary-check.log` · `/tmp/r3logs/boundary-check.log` ·
`/tmp/r3logs/engine-check.log`（HTTP 那次的完整输出见 §R3-2 第 1 条的原始片段）。

### R3-2. 逐条验收表（第三轮 19 条）

| # | 清单项 | 结论 | 证据（命令 / 断言 / 原始片段） |
|---|---|---|---|
| **1** | 真实路径建书 + 发布 + 开档（**重新生成后的**故事书） | **通过** | `node scripts/lmop-verify-http-round3.mjs` → EXIT=0，13/13。首行钉死交付物：`sha256 = f7b15b723c9f38ab90fdf4487c5dce56931db0fe3d7ae6272d8f738375d29bc2`。`POST /api/storybooks → 201`（`sb-849c8abbf1a241f4980b5d314e821286`）、`PUT → 200`、`POST /publish → 200 revision=1`、`POST /api/saves → 201 embedded_revision=1`（`sv-7f6d038763344b0cb155a4637681cb7b`）。引擎层每条用例的 `publish_and_open` 走同一条发布门（0 error）。 |
| **2** | 投影不含 `kind="monster"` 初始实例 | **通过** | HTTP：`character kinds = {"pc":1}`、`keys=inst-pc-lmop-talin`、`encounters=[]`。引擎 `r3_1_2`：`kinds={"pc": 1}`、`characters.len()==1`（31 条图鉴模板一条都不进实例表）。 |
| **3** | 遭遇克隆：灰烬丧尸 AC 8 / HP 22 / 六维 13,6,16,3,6,5 | **通过** | `r3_3_4_7`：`count=1` → `{"hp":22,"max":22,"ac":8}`；实例 `attributes={"str":13,"dex":6,"con":16,"int":3,"wis":6,"cha":5}`。 |
| **4** | 玩家攻击：属性来自技能声明、伤害扣实例 hp、发 CheckResult | **通过** | `r3_3_4_7`：先读 `skills[sk-lmop-shortsword].attribute == "dex"`，再断言 `CheckResult.attribute == declared && != "str"`、`kind=Attack`、`expr`/`rolls` 非空；命中回合实例 `res-hp 22→<22` 且与 `EncounterView.enemies[0].hp` 同步。 |
| **5** | `enemy_strike`：难度 = 玩家派生 AC | **通过** | `r3_5`：`CheckResult.target=14`，14 是测试**自己从故事书算的**（10 + dex16 的 +3 + 皮甲挂接 +1）；`attribute="str"`（丧尸猛击自己的声明）；PC `res-hp 24→<24`。 |
| **6** | 自目标豁免减半不再静默失效 | **通过** | `r3_audit2`（自目标，成功 =(delta,dice,prone)= `(3,4,false)`／失败 `(7,4,true)`）；前两轮用例重跑仍绿（`a12b`、`r2_6`）。见审计②。 |
| **7** | 资源经 Add 后仍是 JSON 整数 | **通过** | `r3_3_4_7`：受伤实例 `resources` 的序列化串**不含小数点**且 `res-hp.as_i64()` 为 `Some`（形态证据，不是 `as_i64` 兜底）；`r2_4_and_8` 重跑覆盖 PC 侧。 |
| **8** | `check_pre_roll` 能读 attribute/kind | **通过** | `r3_8_10_11`：探针挂载点写下 `r3.kind-attribute` / `r3.attr-dex`，且**没有** `r3.kind-nil` / `r3.attr-nil`；`r2_9_and_12` 重跑给出同一结论（`c1_mod=8 c2_mod=0`）。 |
| **9** | 触发点预置遭遇 + `encounter_cleared` 条件 | **通过** | `r3_9_13`：无遭遇时 `eval_cond(EncounterCleared)==false`；PC 到 `loc-lmop-0b7` 后掷表建遭遇，`enc.location_id=="loc-lmop-0b7"`、`template_ids` 与故事书该触发点预置集合**逐条相等**（不采信引擎摘要）；移开后清剿 → `eval_cond==true`。 |
| **10** | 对抗判定：掷 vs 掷 与 掷 vs 被动都跑通，`opponent` 非空 | **通过** | `r3_8_10_11`：先建遭遇使对手**在世界上** → 掷 vs 掷 `opponent="灰烬丧尸"`、`dice=2`。掷 vs 被动由 `r2_11` 重跑覆盖（`dice=1`、`opponent` 非空）。 |
| **11** | 优势 `keep_high` 的 RNG 消耗为 2 | **通过** | `r3_8_10_11`：无激励 `dice=1`；施加 `dnd-inspired` 后同一 dex 检定 `dice=2`。 |
| **12** | 熟练加值数据驱动（改故事书 definition → 检定总值随之变） | **通过** | 前两轮用例 `r2_13` 重跑：基线 `r#mod=8`，把 `prof-pc-lmop-talin-dex.fields.bonus` 由 `"5"` 改 `"9"`（只改一处）→ `r#mod=12`。交付物自检 `proficiency.data_driven` 同结论。 |
| **13** | 掷表遭遇可反复 | **通过** | `r3_9_13` 按 active 上升沿计数（连跑 3 轮，归档于 `/tmp/r3logs/round3-tests.log`）：分别命中 `tr-lmop-wander-day-12`=**2**（74 回合）／`day-4`=**2**（38 回合）／`day-1`=**2**（13 回合）；更早的若干次运行还见过 `day-11` / `day-7` 各 2 次——**每轮都出现同一表项的第二次触发**（具体命中哪一行随存档种子变化）。 |
| **14** | XP 由 `enemy_defeated` 发放（含不在任何掷表行里的模板） | **通过** | 前两轮用例 `r2_15` 重跑：`mon-skeleton`（不在 8 个掷表模板里）即兴遭遇打死 `res-xp 0→50`。交付物自检 `xp.improvised_encounter` / `xp.encounter_snapshot_fallback` 同结论。 |
| **15** | **GAP-A 新能力**：`get_character` 四形态命中；`get_flag`/`get_encounter` 可用；`host.target` 含 `statuses` | **通过** | `r3_15`（会话级探针）：四形态全命中、`get_character('nobody')==nil`、`list_flags` 是表、`get_flag` 未命中返回 nil、`get_encounter(list[1].id)` 命中且带 `enemies`、`host.target` 的 `statuses/attributes/resources/location_id/kind` 全部可读。**口径注**：本交付物里实例键 `== instance_id`（同一字符串），四形态实为 **3 个不同取值 + 1 个重复**。 |
| **16** | **GAP-L 生效**：伏击按目标状态给优势（正例 + 反例） | **通过** | `r3_16`（真实脚本 + 真实开放内容）：目标带 `dnd-surprised` → `keep_high=1`；无状态 → 0；别的状态 → 0；袭击者无挂接 → 0。**数据驱动反证**：把 `definitions[ambush-doppelganger].fields.target_status` 改成 `dnd-prone` → `prone=1 / surprised=0`。 |
| **17** | **GAP-E 生效**：save-half 缩放**引擎那份**效果（同一次掷骰的一半），不是重掷 | **通过** | `r3_audit1_17`：把真实脚本里**唯一**的 `scale_effect(0.5)` 换成 `scale_effect(1.0)`，同种子 32 次——**骰序逐位相同**（每次 4 颗 = 1×d20 + 3×d6），`half == trunc(full×0.5)`，引擎产物 1 条 hp delta、Lua 伤害请求 **0**。对照旧重掷脚本：引擎 0 / Lua 1。 |
| **18** | 多段效果 / 带 modifiers 的技能：缩放覆盖全部数值 delta（**独立构造**） | **通过** | `r3_18`（自建技能：`cost` + `2d6` 伤害 + `1d4` 改资源 + `1d8` 治疗 + `set_flag` + `modifiers`）：24 个种子下骰序不变（5 颗 = d20 + 2d6 + 1d4 + 1d8），**3 段数值 delta 全部按因子缩放**，标记 / 消耗 / 静态修正逐字不动。反面对照：同一个技能挂上 T23 之前的 Lua 重掷脚本 → 只补出**第一段**伤害，第 2/3 段与 modifiers 全丢。 |
| **19** | 引擎边界：`crates/octopus-engine/src` 与 `crates/octopus-types/src` 禁词零命中 | **通过** | `./scripts/lmop-boundary-check.sh` EXIT=0（`advantage|proficiency|on_save|save_ends|ConditionalModifier|EncounterTable|extends` 零命中）。**独立宽词表复核**：`disadvantage` / `saving_throw` / `dnd-` 在两个 crate 均 **0 命中**；`scale_effect / get_character / list_encounters / resolved_effects` 有命中但都是**通用原语名**。一处精度更正见 §R3-6。 |

### R3-3. 四件专门审计（独立复现，不采信自述）

审计手段统一为：**从交付物里读出真实脚本原文**（不抄），用**固定种子**跑真实 `command::execute_skill`；
RNG 与 `LuaHost` **共享同一个句柄**（真实会话就是这么接线的——第一版 harness 里我各建了一个，
结果旧「重掷」脚本掷的骰不在记账里，被误测成「没掷」，已修正；这条踩坑本身也说明「骰序可比」不是自动成立的）。

#### 审计①：save-half 是否真的不再重掷 —— **成立（不是另掷一份）**

复现：把真实脚本里唯一的 `scale_effect(0.5)` 逐字换成 `scale_effect(1.0)`，其余不动；同种子、同技能、
同一条 `sk-lmop-rubble-collapse`（豁免结果用 `check_pre_roll` 的 `force_success` 钉死，骰照掷），跑 32 个种子：

```text
R3-AUDIT1/17 PASS: 32 个种子下骰序逐位相同且 half==trunc(full*0.5)；
  新机制 引擎 hp delta=1 / Lua 伤害请求=0；旧机制 引擎=0 / Lua=1
```

| 判据 | 观测 | 含义 |
|---|---|---|
| RNG 序列 | 32/32 种子下 `factor=1.0` 与 `factor=0.5` 的 `consumed` **逐位相同**（各 4 颗） | 因子不改变掷骰 |
| 数值 | `half == (full as f64 * 0.5).trunc()` 全部成立 | 半值 = **同一份**的向零取整 |
| 出处 | 成功回合引擎产物里恰好 **1 条** `resources.res-hp` delta；Lua `ApplyEffect(damage)` 请求 **0 条** | 减半伤害是**引擎结算产物**，不是 Lua 另补 |
| 对照（T23 之前的脚本） | 引擎 hp delta **0** 条、Lua 伤害请求 **1** 条 | 旧机制确实是「门 Lua 另掷一份」 |

结论：**T23 的自述在这一条上经得起独立复现**。`scale_effect` 缩放的就是引擎**那次**结算；
规则包脚本里已无 `engine_rng`（断言 `!real.contains("engine_rng")` 通过）。
「多段效果 / modifiers 全丢」这个重掷无法回避的问题，由清单 18 独立钉死。

#### 审计②：T23 的连带修正是否保住行为 —— **保住了，而且顺手修了一处位置错误**

连带修正的内容：`scale_effect` 会打开「成功也结算」的门，而缩放只作用于**数值型** delta，
所以 T23 把 `dnd-prone` 从技能的 `effect.status` 移到开放内容 `savehalf-rubble.fields.fail_status`，
由脚本在失败分支补。本轮独立复现两条分支：

```text
R3-AUDIT2  PASS: 成功(delta,dice,prone)=Some((5, 4, false)) 失败(delta,dice,prone)=Some((9, 4, true))
R3-AUDIT2b PASS: 失败分支 伤害 entity=inst-zombie / 状态 target=inst-zombie status=dnd-prone
```

（具体 delta 随存档 id 派生的种子变化：上面这段是某一次真实运行的原文，
连跑归档里还出现 `(3,4,false)/(7,4,true)`、`(5,4,false)/(15,4,true)` 等；
**区间断言**（成功 1..9 / 失败 3..18）与 `prone` 断言在全部复跑里都成立。）

| 分支 | 契约 | 实测 |
|---|---|---|
| 豁免**成功** | 只受一半伤害（1..9）、**不倒地** | `delta∈1..9`、`prone=false`（多次复跑一致） |
| 豁免**失败** | 满伤（3..18）+ **倒地** | `delta∈3..18`、`prone=true` |
| 附加状态落点 | —— | 伤害与 `dnd-prone` **都落在目标实例**；引擎旧路径是 `status_delta(actor_id)`（`effects.rs:229`），即**显式非自身目标时状态以前会落在施法者头上**——T23 把它改成落在目标，是**改善**，不是回归 |

时长口径也逐字等价：`dnd-prone` 顶层定义 `duration:1 / unit:turns`，
`build_status_ref`（旧路径）与 `build_status_instance(status,1,"turns")`（新路径）都得到 `turns_left=1`。
LMoP 的实际用法是「自己踩瓦砾打自己」，因此**可观测行为没有变化**。

**如实登记一个作者陷阱**（不是缺陷，代码注释也写明了）：`scale_effect` 的语义是
「**声明** = 这次效果照常结算一次、数值按因子缩放」——`effect_applies = scale.is_declared() || …`。
所以 `scale_effect(1.0)` **不等于**「没有声明」：它照样把「豁免成功 = 不结算」那道门打开。
审计③的 M2 变异（把 0.5 改成 1.0）实测在成功分支打出**满伤 3..18**，正是这个语义的直接后果。
规则作者若想表达「成功就不结算」，不能写 `scale_effect(1)`。

#### 审计③：两处「注释口径过期」 —— 结论分两半

审计标准（照任务要求）：**不是「注释对不对」，而是「把实现改坏，这条断言会不会 FAIL」**。
手段：把 a12 / a12b 的断言段按字面复刻到固定种子的引擎级用例上，施加 4 组变异
（`/tmp/r3logs/audit3a-mutations.log`）：

```text
R3-AUDIT3a: 基线 dice=4 delta=5
  M1(无声明)      dice=1 delta=0                → 断言 FAIL=true
  M2(因子1.0)     64 种子失败 41/64（delta 例：[4, 7, 11, 13, 13, 14, 11, 12]）
  M3(旧重掷脚本)   dice=4 引擎delta=0 Lua补=1   → a12 仍 PASS
  M4(因子0.25)    64 种子失败 0/64（delta 例：[1, 1, 2, 3, 3, 3, 2, 3]）
```

##### ③a `crates/octopus-api/tests/lmop_verification.rs` 的 a12 / a12b

> **⚠️ 第四轮订正（T26 已修，见 §R4-2）**：T26 已把注释改成新机制，并加了 PostResolve 机制探针
> （`factor / rng_consumed / #deltas / 引擎 hp delta == -本次掉血`）+ 一条变异用例。
> 第四轮用**自己的** harness 独立复现：旧重掷脚本下松断言（dice=4 / delta∈1..9）照过、机制判据 FAIL；
> 另外自造「缩放 + Lua 双补」反例证明 hp 对账也有牙齿。下面的「机制盲」判断描述的是**修正前**的断言。

过期文本：文件头 `// 验收 12：豁免成功伤害减半（Lua 重掷同一骰式再取半）`（707 行）、
`assert_eq!(dice, 4, "豁免成功：1 颗 d20 + Lua 重掷 3d6 取半 = 4 颗")`（730 / 774 行）。
现在成功分支是「引擎掷 1 次 d20 + 1 次 3d6，数值减半」。

**判断：注释过期，不是断言失效；但断言是「机制盲」的，且对「缩放过头」没有牙齿。**

- M1（删掉缩放声明）让 `dice==4` 与 `delta∈1..9` **双双 FAIL** ⇒ 断言仍在抓「减半到底有没有生效」，不是恒真。
- M2（0.5→1.0，即「把减半改坏成满伤」）被抓住 **41/64** 个种子（`delta>9` 时才 FAIL）——**概率性**，不是必然。
- M3（把实现换回 T23 之前的 Lua 重掷）**两条断言都 PASS**：重掷路径同样消耗 4 颗骰、同样落在 `1..9`。
  所以「重掷 3d6 取半」这句注释**没有任何断言在管**——它现在是纯文字，注释改了断言也不会变。
- M4（0.25）**0/64 被抓**：`delta∈1..4 ⊂ 1..9`，断言对「缩放过头」没有覆盖。

即：**没有替它圆场的余地**——这句注释确实过期了，且断言无法区分「缩放引擎那份」与「另掷一份」；
但断言本身没有失去牙齿（M1 会让它挂）。覆盖空档由本轮新增的 `r3_audit1_17`（同骰证明）
与 `r3_18`（多段 / 过度缩放反面）补上。

##### ③b `crates/octopus-api/tests/lmop_verification_round2.rs` 的 r2_6 负对照

> **⚠️ 第四轮订正（T26 已修，见 §R4-3）**：T26 已订正机制描述，并把负对照改成直接读该回合
> `Resolution.state_changes`，要求找到「entity_id = 不存在目标 + field = resources.res-hp + value < 0」的记录
> ——**诊断价值已恢复**（能区分「分支被跳过」与「算了被丢弃」）。第四轮独立复现该区分：
> 真实脚本 → ghost delta 在场、骰数 4；删掉缩放声明 → 无 ghost、骰数 1。
> 下面「诊断价值已消失」的判断描述的是**修正前**的 r2_6。**「静默丢弃无日志」这条新观察仍然成立。**

过期文本（466–470 行注释、488/516/528/533 行断言消息）：「C 负对照：`target_id = Some("不存在的实体")`
→ 目标解析不到 → `host.target = nil` → **减半分支静默不发** → delta = 0。这正是修复前的**失效形态**。」

**判断：这是事实错误，不只是口径过期（但断言仍 PASS，且不恒真）。**

```text
R3-AUDIT3b PASS: 负对照 delta==0 仍成立（断言未恒真：自目标时 delta=-5），
  但机制已变——引擎结算了 entity_id=r3-不存在的目标 的伤害并静默丢弃，
  旧注释「host.target 为 nil → 分支不发」不再成立
```

- 引擎级实测：显式给一个解析不到的目标 + 豁免成功 → 规则脚本**照样声明了缩放**
  （成功分支根本不读 `host.target`），引擎**结算出 1 条伤害 delta**，`entity_id = "r3-不存在的目标"`。
- 会话级实测（真实 r2_6-C 形态）：成功回合 `delta == 0`（`apply_delta` 对未知实体直接 `return`，
  `session.rs:5703`），**但引擎仍掷了 4 颗骰**；投影 `characters.len()==1`（不留幻影实例）。
- 因此负对照的 `delta==0` **不是因为分支没发**，而是「算完了落在不存在的实体上被丢掉」。
  它**仍不是恒真**（自目标时 `delta=-5`），也就是说「不可解析目标回落到施法者」这类坏实现仍会被它抓住；
  但它**已经无法区分**「分支被跳过」与「算了但被丢弃」——R2 赋予它的诊断价值（证明修复前的失效形态）已经没有了。
- **新观察（低 severity，如实登记）**：显式不可解析目标 + 成功分支，现在会「掷效果骰 → 算出伤害 →
  静默丢弃」，全程**无日志、无事件、无报错**。失败分支本来就是这个形态（引擎一向如此），
  成功分支是新开的（因为 `scale_effect` 打开了结算门）。

#### 审计④：集群战术的近似边界 —— 在什么条件下会给出**错误结论**

> **⚠️ 第四轮订正（T26 已修，见 §R4-1）**：本节记录的 5 类错误结论与 `range_proxy` 声明/实现矛盾，
> **已在 T26 修正**。以下文字是**修正前**的实况，作为第三轮的历史证据**原样保留**。
> 第四轮已用 git HEAD 的旧脚本原文（与 HEAD 逐字一致）独立复现本节每一条（确实为真），
> 并逐条把修复改回去，确认新断言会 FAIL。修正后的实况：FP-A/B/C/E = 0、FN-A = 1。

规则包现在的判据（`dnd-pack-tactics` 脚本原文，**修正前**的版本）：
`host.get_attachment('dnd-pack-tactics')` 非空 → 在 `host.list_encounters()` 里找**我所在的那场遭遇** →
同遭遇里除我以外、`host.get_character(key)` 存在且 `resources['res-hp'] > 0` 的条目算「未失能的盟友」→
若 **我** 与 **目标** 都有 `location_id` 且不同则退出 → 否则 `modify_check('keep_high')`。

```text
R3-AUDIT4: P0=1 P1(同伴倒)=0 P2(异地)=0 |
  FP-A(同伴在别处)=1 FP-B(同伴失能)=1 FP-C(无地点)=1 FP-E(属性检定)=1 |
  FN-A(同伴在别的遭遇)=0 FP-D(无实例)=0
```

| 场景 | 规则应得 | 实测 | 判定 |
|---|---|---|---|
| P0 同遭遇 + 同伴存活 + 狼与目标同地点 | 优势 | `1` | 对 |
| P1 同伴 HP=0 | 无 | `0` | 对（「未失能」的近似在此成立） |
| P2 狼与目标不同地点 | 无 | `0` | 对 |
| FP-D 遭遇条目在、实例查不到 | 无 | `0` | 对 |
| **FP-A 同伴在** `loc-c`**、狼与目标在** `loc-a` | 无（同伴不在目标 5 尺内） | **`1`** | **错**——脚本**从不检查同伴的 location** |
| **FP-B 同伴 HP>0 但带** `stunned`**（失能）状态** | 无（「未失能」不成立） | **`1`** | **错**——`HP>0` 只是「未失能」的近似 |
| **FP-C 狼与目标都没有** `location_id` | 无（无法证明同处） | **`1`** | **错**——地点闸门被整体跳过 |
| **FP-E 狼做**非攻击**判定（签名 kind=attribute）** | 无（数据卡只说「攻击检定」） | **`1`** | **错**——脚本不看判定签名，也没有 `when` 闸门 |
| **FN-A 同伴与目标同地点，但在**另一场遭遇** | 有 | **`0`** | **错**（漏判） |

> **§R4-1 已订正（T26 已修）**：下面这段结论描述的是**修正前**的实况——「脚本从没实现 range_proxy」
> 已经不再成立（实现已对齐声明）；FP-E 也已收敛到 `kind == attack`。原样保留作历史。

**这决定 GAP-A 该算「近似提高」而不是「已闭合」**：跨实体读取这个**能力**确实闭合了
（清单 15/16 + 上面的 P0），但**规则结论不精确**，而且不止是 GAP-B 的锅：
开放内容自己声明的 `range_proxy` 写的是「与目标同一 location_id」，**脚本从没实现这一步**——
它比的是「狼 vs 目标」的地点。这是一处**声明与实现不一致**，比单纯「没有 5 尺」更具体。
另有 FP-E：脚本注册在 `check_pre_roll` 且 `when=null`，对**任何**判定都跑，数据卡写的是「攻击检定」。

### R3-4. 14 条 GAP 的第三轮现状（独立复核，不是照抄 T23 自述）

复核方法：每条先看**引擎/类型里有没有那个原语**（带行号），再看**规则包真的用上了没有**
（运行时用例 / 交付物自检 / 读源码），最后给出本轮的标签。T23 自报「闭合 7 / 近似提高 1 / 仍存在 6」——

**独立结论：数目一致**；下面每条的“卡在哪”是本轮自己取的证，不是抄自述。

| GAP | 主题 | 第三轮现状 | 独立证据 |
|---|---|---|---|
| **A** | Lua 读不到其它实体（集群战术 / 任何「对手或盟友」类规则） | **近似提高**（能力闭合，规则结论不精确） | 原语在场：`lua_host.rs` 的 `get_character / get_flag / list_flags / get_encounter / list_encounters`（`find_character_snapshot` 四形态；`active_encounters` 只列 active），会话侧由 `session.rs::refresh_lua_world_facts` 在跑脚本前注入。运行时反证：清单 15（会话级四形态/标记/遭遇/目标快照全命中）＋清单 16（伏击按目标状态）。**仍不精确的证据**：审计④ 的 FP-A/FP-B/FP-C/FP-E/FN-A。**⚠️ 第四轮订正（§R4-1）：这 5 类已被 T26 修掉**（旧脚本复现 → 1/1/1/1/0；新脚本 → 0/0/0/0/1，见 §R4-1）。A 仍按「近似提高」登记，但残余不精确**只剩 GAP-B 的距离不可表达**（§R4-4）。 |
| **B** | 无位置 / 距离 / 区域概念 | **仍存在** | `distance`（词边界 grep）在 `crates/octopus-engine/src` **0 命中**；位置仍只有 `location_id` 归属。集群战术只能近似，且（审计④ FP-A）连「同伴同地点」这一步都没实现。**⚠️ 第四轮订正（§R4-1）：「同伴同地点」已实现（真检查同伴 `location_id` + 缺地点 fail-closed）；B 仍然存在，只是 A 的残余不精确现在完全归因于 B。** |
| **C** | 挂接 / 开放内容不进 Lua | **已闭合** | 原语：`get_attachments / get_attachment(kind) / get_definition(id) / list_definitions(kind)`。运行时反证：前两轮 `r2_13` 重跑（只改 definition 的 `bonus 5→9` → `r#mod 8→12`）；交付物自检 `open_content.data_driven`（7 条规则全部运行时读开放内容）。 |
| **D** | `check_pre_roll` 拿不到判定签名 | **已闭合** | `LuaCheckContext` 含 `attribute/kind/resolved/expr`；清单 8 的探针在掷骰前读到 `kind=attribute / attribute=dex` 且无 nil 标记。 |
| **E** | 效果骰值不暴露（豁免减半的数值等价） | **已闭合** | 原语 `host.scale_effect(factor)`（只在 `check_post_roll / pre_resolve` 注册，写错时机当场报错）＋ `PostResolve` 只读 `host.resolved_effects`。独立反证：审计①（32 种子同骰、半值恰为向零取整、引擎产物而非 Lua 补）＋清单 18（多段/modifiers 全覆盖）。 |
| **F** | 无「敌人被击败」事件 | **已闭合（边界不变）** | `enemy_defeated` 逐只发 XP，与遭遇来源无关；前两轮 `r2_15` 重跑（不在掷表行的 `mon-skeleton` 也拿 50 XP）。**边界（如实登记）**：只在 `strike_enemy` 路径派发，Lua 直接把资源打到 0 不发 XP（本轮仍只做代码复核）。 |
| **G** | 预置遭遇数量是静态整数 | **仍存在** | `EncounterPresetEnemy.count: Option<u32>`（`octopus-types/src/lib.rs:878-883`）。运行时旁证：清单 13 的遭遇 `note = "规则包预置遭遇 · 原表 1d8+2 → 固定 3"`（每行固定成骰式**下界**）。 |
| **H** | 没有清标记原语 | **已闭合** | `ImmediateEffect::SetFlag` 带 `value`；Lua 侧 `set_flag(flag[,value]) / clear_flag(flag)`。运行时反证：清单 13 依赖行标记被清掉才可能出现第二次上升沿。 |
| **I** | 无先攻 / 轮次 / 行动经济 | **仍存在** | `initiative` 在 engine 唯一命中是 `validate.rs:2948` 的测试数据字符串。突袭仍只落状态 + `dnd-battle-round-N` 标记。 |
| **J** | 对抗的「掷 vs 被动」入口 | **仍存在（引擎能力已具备，规则包未改用）** | `CondExpr` 变体（`types:118-137`）仍只有 `AttributeGe`（只看 actor 自己），没有「掷骰 vs 对手被动」入口；`Intent::Check.opponent_id` 两种对抗都真跑通（清单 10 + `r2_11` 重跑）。 |
| **K** | 模板级状态无声明入口 | **仍存在** | `octopus-api/src/lib.rs:520 / 591` 建实例时 `statuses: vec![]`；character 模板没有初始状态声明。 |
| **L** | `host.target` 读不到目标状态 | **已闭合** | `CHARACTER_SNAPSHOT_KEYS` 让 `host.target` 与 `host.actor` 同级（`statuses/attributes/resources/inventory/location_id/present`）；清单 16 的正例/反例 + 数据驱动反证（改 `fields.target_status` → 期望值翻转）。 |
| **M** | 没有 `encounter_active` 条件 | **仍存在** | `encounter_active / EncounterActive` 在 `crates/*/src` **0 命中**；`CondExpr` 仍只有 `EncounterCleared`。战斗轮计数继续靠 `dnd-in-combat` 标记开闸。 |
| **N** | 触发点一次性（`repeatable` 未实现） | **已闭合** | 24 个掷表触发点全部 `repeatable:true` + `dnd-wander-reset` 在 `encounter_cleared` 清标记。运行时反证：清单 13 三轮各观察到**同一表项第二次触发**（day-12 / day-4 / day-1，更早运行还见过 day-11 / day-7）。 |

**汇总**：闭合 **7**（C / D / E / F / H / L / N）／近似提高 **1**（A）／仍存在 **6**（B / G / I / J / K / M）。
与 T23 自述数目一致；第二轮是「闭合 5 / 仍存在 9」，本轮新增闭合 **E / L**，并把 **A** 从「仍存在」改判为「近似提高」。

> **第四轮（§R4-4）复核后这个数目不变**（闭合 7 / 近似提高 1 / 仍存在 6，规则包 `--check` 与我的独立复核一致），
> 但 **A 的「近似提高」理由变了**：第三轮的理由含「声明与实现矛盾」，T26 修掉之后，残余不精确**只剩 GAP-B 的距离概念**。

### R3-5. 这套设计成立性的第三轮判断

**第三轮被推翻的证伪（设计假设其实是对的，只是实现落后）**

| 前两轮仍成立的证伪 | 第三轮 |
|---|---|
| 「豁免减半可以用既有原语精确表达」被证伪（GAP-E，只能重掷） | **推翻该证伪**：`scale_effect` 让引擎只掷一次、按因子缩放，且覆盖多段效果与 modifiers。这不是「期望值等价」，是**同一颗骰**（审计①）。 |
| 「Lua 读不到其它实体」（GAP-A 能力面） | **推翻该证伪的能力面**：`get_character / get_flag / get_encounter / list_encounters` + 完整的 `host.target` 快照都到位并被规则包真的用上（清单 15/16）。**但规则结论面仍被证伪**（审计④）。**⚠️ 第四轮订正（§R4-1）：T26 已修掉审计④ 的 5 类；该证伪的「规则结论面」现在只剩 GAP-B 的距离不可表达。** |
| 「`host.target` 读不到目标状态」（GAP-L） | **推翻**：伏击直接按 `host.target.statuses` 判定，期望状态 id 还来自开放内容。 |

**第三轮仍成立的证伪 / 未知**

- **「集群战术能得出正确结论」仍被证伪**：5 类错误结论（审计④），其中 FP-A（不看同伴 location）是**声明与实现不一致**，
  不只是「引擎没有 5 尺」；FP-E（非攻击判定也给优势）是**规则包自己写宽了**。
  > **⚠️ 第四轮订正（§R4-1）**：这 5 类已由 T26 修掉，此条**不再成立**（残余只剩 GAP-B 的距离概念）；
  > 但**同类误判在「伏击」上仍在**——`dnd-ambusher-keep-high` 不看判定签名，属性检定 / 豁免也给优势（§R4-6 实测 attack=1/attribute=1/save=1）。
- **「引擎有位置概念」仍被证伪**（GAP-B，`distance` 0 命中）。
- **「预置遭遇能表达数据卡数量」仍被证伪**（GAP-G，`count: Option<u32>`）。
- **仍然存在的结构性缺口**：轮次经济（I）、`CondExpr` 的被动对手入口（J）、模板初始状态（K）、`encounter_active`（M）。
  这四条与第一轮登记时**一字未变**，且都不是「读不到东西」，而是**引擎刻意没有的概念**——属于边界选择，不是原语欠账。
- **本轮新登记的未知 / 风险**（都不是验收缺陷，但值得记）：
  1. `scale_effect` 把「要不要结算」和「结算多少」绑在一个原语上，**因子 1.0 也会开门**（审计②的陷阱）。
  2. 开放内容声明的 `range_proxy` 与脚本实现不一致（审计④ FP-A）。**⚠️ 第四轮订正（§R4-1）：T26 已把实现对齐声明（真检查同伴位置 + 缺地点 fail-closed），这条风险已消除。**
  3. 伏击 / 集群战术都**不看判定签名**，对非攻击判定也生效（审计④ FP-E；对比 `dnd-sunlight-sensitivity` 是按签名收敛的）。**⚠️ 第四轮订正：集群战术已收敛到 `kind == attack`；「伏击」仍未收敛（§R4-6 实测属性检定 / 豁免照样给优势）——半条仍成立。**
  4. r2_6 负对照的**机制描述失实**，其诊断价值已消失（审计③b）。**⚠️ 第四轮订正（§R4-3）：T26 已订正描述并把负对照升级为「读 Resolution.state_changes 里的 ghost delta」，诊断价值已恢复；「静默丢弃无日志」本身仍在。**
  5. `enemy_defeated` 只在 strike 路径发 XP（边界，与第二轮相同）。

**结论（第三轮）**：第一轮说「路是对的，但原语集不够」，第二轮说「边界明显变宽」，第三轮可以更明确地说：
**「读事实」这一类缺口已经补齐**——跨实体、目标快照、效果数值三样都能读了，而且不是文档层面，是运行时实测。
剩下的 6 条仍存在里，**4 条（I/J/K/M）是引擎没有那种概念**，**2 条（B/G）是引擎没有那种结构**；
真正还没解决的不是「引擎能力」，而是**规则包自己的精确度**（审计④）与**几处固化的近似口径**（负对照、注释、range_proxy）。
> **⚠️ 第四轮订正（§R4-1 / §R4-6）**：「负对照 / 注释」已由 T26 订正；`range_proxy` 的实现已对齐声明；
> 集群战术的精确度问题已收敛到 GAP-B。**仍在的是「伏击不看判定签名」这一条**（§R4-6）。
另需注意：本轮四条专门审计里，**两条落在「登记/注释跟不上实现」**（审计③）——这不是实现错，是**验证资产会过期**，
下一轮若要继续用这些负对照，得先把它们的机制描述改成当前事实。

### R3-6. 未通过 / 未覆盖项清单（不得隐瞒）

**未通过：无（0 项）。未覆盖：无（0 项）。**

必须声明的 **3 处覆盖边界** 与 **1 处精度更正**：

1. **HTTP 侧「既有数据没少」的可见范围有限**：`GET /api/saves` 是**按所有者**过滤的，
   本次用新注册的验证账户跑，跑前该账户看到的存档是 0 个——所以「既有 1 个存档」这条我没法从这个账户证实，
   只能证实：故事书清单（可公共读）跑前 1 本（`sb-ffa1d260083048a68f1e63a2e9a219c3`，用户的）、跑后 2 本，
   **没有任何既有 id 消失**；且脚本只发 `POST/PUT/GET`，**没有发过任何 `DELETE`**。
2. **GAP-F 的非 strike 击杀路径**仍只有代码复核（`enemy_defeated` 派发点在 strike 结算体内），**没有**构造运行时反例（与第二轮相同）。
3. **`scale_effect` 的开门语义**是在引擎级 `execute_skill` 上用真实技能验证的，**没有**再造一个第二种 save 技能的完整会话用例。
4. **精度更正（不是缺陷）**：第二轮写「`lmop / mon-` 的命中全部位于 `#[cfg(test)]` 模块内」。
   本轮逐行核对：`crates/octopus-engine/src/session.rs` 有 **4 处** 落在产品代码里的命中，
   但它们**全部是注释**（`1925 / 4947 / 5412 / 5450` 行的说明文字，含故事书名 “LMoP”）；
   这些行在 HEAD 就存在，不是 T21/T22/T23 引入的。禁词表（`advantage|proficiency|on_save|save_ends|ConditionalModifier|EncounterTable|extends`）
   与宽词表（`disadvantage|saving_throw|dnd-`）仍是 **0 命中**，「产品代码零规则集语义」这一实质结论不变。

**副作用（如实上报，未删除、未修改任何既有条目）**：本轮在真实 `octopus.db` 里**新增**了
1 个验证账户、1 本故事书、1 个存档——id 见 §R3-7，便于事后清理。
既有的用户故事书 `sb-ffa1d260083048a68f1e63a2e9a219c3`（凡戴尔的失落矿坑）**未改动**；
`octopus/octopus` 口令仍不可用（401），所以走了真实注册端点（与第二轮同一条真实路径）。

### R3-7. 本轮新增 / 改动文件 + 新增验证数据 id

| 文件 | 作用 |
|---|---|
| `docs/lmop-verification-report.md` | 本报告（§0–§10 与 §R2 **原样保留**，仅追加 §R3 + 文首导航补一行） |
| `crates/octopus-api/tests/lmop_verification_round3.rs` | **新增**：14 条独立引擎级用例（清单 1–18 + 四件审计，含自建多段效果技能与变异组） |
| `scripts/lmop-verify-http-round3.mjs` | **新增**：真实 HTTP 路径第三轮验证（13 项断言） |

**未改动**：`crates/*/src/` 与 `frontend/src/` 的任何产品代码；
**未改动**前两轮的用例（`lmop_verification.rs` / `lmop_verification_round2.rs` 逐字未动，只读 + 跑 + 变异推理）；
**未改动** `story_example/lmop-storybook.json`（只读；所有变异都施加在内存副本上）。

**新增验证数据（真实 octopus.db，便于清理）**：

```text
账户    verify3-mu0r3x4o   (user-9d319dff1234451c80551f7b5f0e03cb)
故事书  sb-849c8abbf1a241f4980b5d314e821286   凡戴尔的失落矿坑（第三轮验证 2026-09-14T04:36:53）
存档    sv-7f6d038763344b0cb155a4637681cb7b   embedded_revision=1
（既有，未改动：sb-ffa1d260083048a68f1e63a2e9a219c3）
```

---

## 第四轮·独立复核（T26 对第三轮验证资产的改动之后）

> 角色：**全新的独立验证者（第四轮）**（未参与 T21/T22/T23/T24/T26 的任何实现与验证）。
> 只验证，不修产品代码；inScope：`docs/lmop-verification-report.md` · `scripts/`（只新增） · `crates/octopus-api/tests/`（**只新增**）。
> **未修改**任何既有用例：`lmop_verification.rs` / `lmop_verification_round2.rs` / `lmop_verification_round3.rs`
> 逐字未动；对 T26 改过的 `r3_audit4` 走「读 + 跑 + 旧脚本原文复现 + 逐条变异推理」。
> 被测交付物：`story_example/lmop-storybook.json`，sha256 `ffc93b3e8cab551c6964aa792a2abc94625727dcc27e376f787e4a334e9cfdaa`
> （第三轮是 `f7b15b…`，**不是同一版**）。第四轮实测：用 `scripts/lmop-rulepack.mjs` **重新生成**，
> 产物与交付物**逐字节相同**（同 sha256）→ 交付物确实是规则包脚本的产物，脚本是唯一真源。
> 查询口径：**不采信 T26 自述**；下面每一条都是本轮自建 harness / 自造场景跑出来的。

### R4-0. 结论摘要

| | 结论 | 说明 |
|---|---|---|
| **T26 对 `r3_audit4` 的翻转** | **合法（未弱化）** | 用旧脚本原文独立复现 T24 的 1/1/1/1/0；把 5 处修复逐条改回去，对应新断言逐条 FAIL；场景集合未缩水；断言两端都钉 |
| a12 / a12b 新机制判据 | **合法（有牙齿）** | 旧重掷脚本：松断言照过、机制判据 FAIL；本轮自造「缩放 + Lua 双补」反例 → hp 对账也 FAIL |
| r2_6 新判据 | **合法（真能区分）** | 真实脚本 → ghost delta 在场且 dice=4；删缩放声明 → 无 ghost 且 dice=1 |
| 5 类误判 | **确认已修** | 旧脚本 1/1/1/1/0 → 新脚本 0/0/0/0/1（本轮自有场景，不抄 T26） |
| 声明 vs 实现 | **不再矛盾** | `range_proxy / signature_scope / incapacitated_statuses / ally_proxy` 逐条对上；失能名单数据驱动 |
| 14 条 GAP | **闭合 7 / 近似提高 1 / 仍存在 6**（数目不变） | **GAP-A 不能升为「闭合」**，理由见 §R4-4 |
| 全套回归 | **全绿** | r1 13 / r2 14 / r3 14 / **r4 9（新增）** / api-lib 74 / engine 357 / ai 36 / rulepack 28 / engine-check 11 / boundary 0 命中，全部 0 失败 |
| 本轮新发现 | 2 条（1 缺陷候选 + 1 低危）+ 2 条设计层观察 | ①**「伏击」仍不看判定签名**（与已被修的集群战术 FP-E 同类，属性检定 / 豁免照样给优势）；②代码/测试/文档把 T26 的修复记成「**T24 修正**」，归因错位 |

**一句话**：T26 的三处改动**都经得起独立复现**——`r3_audit4` 的断言翻转**不是放宽**（旧脚本原文照样复现出修正前的 5 类误判，逐条变异又都能把新断言打红）；
a12/a12b 与 r2_6 的新判据**确实能分辨机制**，不是恒真。
**但不能替 T26 圆场的有两条**：①它只修了「集群战术」这一条规则的判定范围，**同一类误判在「伏击」上原样保留**（§R4-6 ①）；
②这轮改动的注释/文档把修复者写成「T24」（T24 是上一轮验证者，只报告不实现），是可追溯性上的错位（§R4-6 ②）。

### R4-1. T26 对 `r3_audit4` 的翻转审计（本轮最重要产出）

#### (a) 改了什么

`git diff` 显示改动只落在 `r3_audit4` 一处：5 条断言的值 FP-A/B/C/E `1 → 0`、FN-A `0 → 1`，
header / 注释改写、`two` 闭包从 `&str` 改成 `Option<&str>`。**断言条数 10 → 10，场景一条没删**
（P0 / P1 / P2 / FP-A / FP-B / FP-C / FP-D / FP-E(attr) / FP-E(attack) / FN-A 一一对应）。

#### (b) 判定标准

不是「测试通过」，而是两条：**①用 T24 当初的原始场景独立复现，确认规则包行为真的变了**（而不是期望值被改小改没）；
**②把对应的修复改回去，新断言会不会 FAIL**。

#### (c) 证据 1 —— 行为真的变了（旧脚本原文复现）

旧脚本逐字取自 `git show HEAD:story_example/lmop-storybook.json`；本轮核对：
嵌入 `lmop_verification_round4.rs` 的 `OLD_PACK_TACTICS` 与 HEAD **逐字节相同**（1356/1356 字节）。
用**同一批场景**分别跑旧脚本与新脚本（本轮自建 `pack_advantage` harness，5 个体征场景 + 2 个对照）：

```text
R4-1 旧脚本（git HEAD 原文）: P0=1 FP-A=1 FP-B=1 FP-C=1 FP-E(attr)=1 FN-A=0
R4-1 新脚本（交付物）      : P0=1 FP-A=0 FP-B=0 FP-C=0 FP-E(attr)=0 FN-A=1
```

旧脚本**精确复现** T24 的五条原始结论（FP-A/B/C/E 各 1、FN-A 为 0）；新脚本全部翻转。
所以这次翻转是**行为变化**，不是把期望值改小 / 改没——如果只是改数字，新脚本不会给出 0/0/0/0/1。

#### (d) 证据 2 —— 逐条变异，新断言逐条有牙齿

每次只回退**一处**修复，其余逐字不动：

```text
R4-1 变异验证: M1(去掉同伴位置检查) FP-A=1（新断言期望 0）
             M2(去掉失能名单判定)   FP-B=1（新断言期望 0）
             M3(去掉缺地点 fail-closed) FP-C=1（新断言期望 0）
             M4(去掉 kind==attack 闸门) FP-E(attr)=1（新断言期望 0）
             M5(只看我所在遭遇)      FN-A=0（新断言期望 1）
```

5 处修复各自独立：回退哪一处，只有那一条断言会翻；本轮对每个变异都抽查了其余维度**未被带偏**
（用例里逐条断言了代表性交叉项，例如 M1 后 `fp_b` 仍为 0、M2 后 `fp_a` 仍为 0）。
也就是说新断言**不是恒真**，任何一处退化都会被打红。

#### (e) 证据 3 —— 断言仍然钉得住两端

- **「必须给」端**：`p0 == 1`、`fp_e_attack == 1`、`fn_a == 1` → 「一律不给」的实现会被抓。
- **「必须不给」端**：`p1 / p2 / fp_a / fp_b / fp_c / fp_d / fp_e_attr == 0` → 「一律给」或任一维度退化的实现会被抓。

#### (f) 结论：**合法，未弱化**——附一条过程性条件

这是「实施者改验证者的审计用例」，最敏感的一类改动。它之所以安全，是因为**旧实况没有消失**：

- T26 把「旧脚本复现 5 类误判」搬进了 `scripts/lmop-engine-check/src/main.rs`
  （断言 `pack_tactics.old_impl_reproduces_misjudgments`）与 `story_example/lmop-import.md`；
- 本轮又补了一枚 **cargo test 层**的钉子 `r4_audit4_old_script_reproduces_t24_findings`（断言旧脚本 = 1/1/1/1/0），
  以及 §R3 各处的「修正前实况」标注（见 §R4-9）。

**如果当时只改 `r3_audit4` 而不给旧实况留任何可执行记录，同一处改动就应判为「弱化（抹掉历史证据）」。**
现在的判法是：**合法**，但这是「实施者改验证者资产」必须补上的条件，不是天然成立的。

### R4-2. a12 / a12b 新机制判据的审计

T26 的说法：加了一支 PostResolve 探针（`factor / rng_consumed / #deltas / hp delta 对账`），
并用「换回旧重掷脚本 → FAIL」做变异用例。本轮**用自己的 harness 重写**了探针与场景（不调用 T26 的用例函数）：

```text
R4-2 基线（引擎缩放）    : flags={deltas:1, factor:0.5, hp:-4, missing:false, rng:3} dice=4 delta=4
R4-2 变异A（旧重掷）     : dice=4 flags={deltas:0, factor:nil, hp:none, rng:0} 引擎hp=None Lua伤害=4
R4-2 变异B（缩放+Lua双补）: dice=7 flags={deltas:1, factor:0.5, hp:-2, rng:3} 引擎hp=2 Lua补=4 会话可见=6
```

| 形态 | 松断言（骰数 4 / 半值 ∈ 1..9） | 机制判据 |
|---|---|---|
| 基线：引擎结算 + `scale_effect(0.5)` | PASS（dice=4、delta=4） | **PASS**（factor=0.5 / rng=3 / #deltas=1 / 引擎 hp delta == -掉血） |
| **变异 A**：换回旧「Lua 重掷 3d6 取半 + `apply_effect`」 | **PASS**（dice=4、Lua 补 4） | **FAIL**（factor=nil / rng=0 / #deltas=0 / hp=none） |
| **变异 B**（本轮自造）：既 `scale_effect(0.5)` 又让 Lua 再补一份 | PASS | **FAIL**（因子字段全对，但引擎快照 hp=-2 ≠ 会话可见掉血 6 → **hp 对账**这一半也有牙齿） |

**结论：合法、有牙齿。** 变异 A 是 T26 自己给的反例，本轮独立复现成立；变异 B 是 T26 **没做**的反例，
用来证明「hp 对账」不是装饰——只有「引擎没结算」和「Lua 又补一份」两种坏法都能被抓，判据才算完整。
旧的两条松断言（骰数 4、1..9）逐条保留，没有放宽。

**如实登记的适用边界**：`rng_consumed == 3` 钉的是「3d6」这个骰式、`#deltas == 1` 钉的是这个技能的形状，
是**交付物专用**断言（换骰式 / 换技能要同步改）；这是可接受的，但不能当成通用判据。

### R4-3. r2_6 新判据的审计

T26 的说法：改成读该回合 `Resolution.state_changes`，要求找到 `entity_id = 不存在目标 + field = resources.res-hp + value < 0`。
本轮从两侧独立确认：

| 层次 | 真实脚本（交付物） | 变异（删掉 `dnd-save-half` 的缩放声明） |
|---|---|---|
| 引擎级（`execute_skill` 产物） | 引擎算出 1 条 `entity_id="r4-不存在的目标"` 的伤害 delta（-5），`dice=4` | 无任何 hp delta，`dice=1` |
| 会话级（真实回合） | 投影 delta=0（落不到实体），但 `Resolution.state_changes` 里有 `("r4-不存在的目标","resources.res-hp",-5)`，`dice=4` | 无 ghost 记录，`dice=1` |

代码路径旁证：`session.rs` 的 `resolve_skill` 把 `outcome.effects.deltas.clone()` 原样放进
`Resolution.state_changes`（**不经实体过滤**），而 `apply_delta` 对未知实体直接 `return` ——
「事件里有、投影里没有」正是「算了但被丢弃」的可观测形态，与「分支被跳过」在 dice 数与 ghost 记录上**双双可分**。

**结论：合法，不是靠放宽阈值。** 旧断言 `delta == 0` 一条没少，另加了三条**更强**的断言
（`dice == 4`、ghost delta 必须存在且为负数、不得留下幻影实例）。变异（删缩放声明）会让它 FAIL（dice=1、无 ghost）。

### R4-4. 5 类误判的独立复现 + 14 条 GAP 第四轮现状

#### (a) 5 类误判（本轮自造场景，不采信 T26 的 engine-check 自述）

| 场景 | 规则应得 | **旧脚本（HEAD 原文）实测** | **新脚本（交付物）实测** | 判定 |
|---|---|---|---|---|
| FP-A 同伴在 `loc-c`、狼与目标在 `loc-a` | 无 | `1` | `0` | **已修** |
| FP-B 同伴 HP>0 但带 `stunned`（在开放内容失能名单里） | 无 | `1` | `0` | **已修** |
| FP-C 狼与目标都无 `location_id` | 无 | `1` | `0` | **已修** |
| FP-E 非攻击判定（`kind=attribute`） | 无 | `1` | `0` | **已修** |
| FN-A 同伴在**另一场遭遇**但同在 `loc-a` | 有 | `0` | `1` | **已修** |
| P0 正例（同遭遇 + 同伴存活 + 与目标同地点） | 有 | `1` | `1` | 对（未退化） |
| P1 同伴 HP=0 / P2 狼与目标异地 / FP-D 实例查不到 | 无 | `0 / 0 / 0` | `0 / 0 / 0` | 对 |

#### (b) 14 条 GAP 第四轮现状

复核方法：①规则包 `--check` 的逐条 status；②本轮自己去 `crates/*/src` 找原语 / 找结构；
③运行时反例（本轮 r4 用例 + 前三轮用例重跑）。

| GAP | 第四轮现状 | 独立证据 |
|---|---|---|
| A | **近似提高**（**不能升为「闭合」**，见下） | 5 类误判已修（§R4-4a）；声明与实现逐条对上（§R4-5）；但「目标 5 尺内」仍不可表达 |
| B | **仍存在** | `distance` 在 `crates/octopus-engine/src` 词边界 grep 0 命中；只有 `location_id` 归属 |
| C | **已闭合** | `get_attachments / get_attachment / get_definition / list_definitions`；本轮失能名单数据驱动反证（改名单 → 期望翻转） |
| D | **已闭合** | `check_pre_roll` 下发 `kind / attribute`；集群战术新脚本正是靠它收敛（M4 变异证明有牙齿） |
| E | **已闭合** | `scale_effect(0.5)` + PostResolve `host.resolved_effects`；本轮 §R4-2 独立复现 |
| F | **已闭合（边界不变）** | `enemy_defeated` 逐只发 XP；边界：只在 strike 路径（与第二 / 三轮同，未构造非 strike 运行时反例） |
| G | **仍存在** | `EncounterPresetEnemy.count: Option<u32>`，无骰式落点 |
| H | **已闭合** | `set_flag(flag[, value]) / clear_flag` |
| I | **仍存在** | 引擎无先攻 / 轮次 / 行动经济 |
| J | **仍存在** | `CondExpr` 无「掷 vs 对手被动」入口（引擎侧 `Intent::Check.opponent_id` 已有） |
| K | **仍存在** | 建实例时 `statuses: vec![]`；模板初始状态无声明入口 |
| L | **已闭合** | `host.target` 与 `host.actor` 同级完整；**但伏击不看判定签名**（§R4-6 ①，与 L 无关，是规则包自己写宽了） |
| M | **仍存在** | `CondExpr` 只有 `encounter_cleared` |
| N | **已闭合** | 24 个掷表触发点 `repeatable: true` + `dnd-wander-reset` |

**汇总：闭合 7 / 近似提高 1 / 仍存在 6** —— 与规则包 `--check`、与第三轮数目一致。第四轮没有新增闭合，也没有把任何一条改判回去。

#### (c) GAP-A 能不能从「近似提高」升为「闭合」？——**不能**

理由（三条，都是本轮实测 / 逐条读代码得到的）：

1. **登记的缺口对象是「集群战术这条规则能不能得出正确结论」，不是「能不能读跨实体」。**
   跨实体读取这个**能力**确实闭合（清单 15 是证据）；但规则的结论仍可能错：同伴与目标只要同在**一个 `location_id`**
   就算「5 尺内」，而一个 `location_id` 覆盖整个地点，同伴可以在**任意远处**。本轮 P0 场景就是这条不可区分性的直接体现
   ——引擎里没有任何字段能把「同在 loc-a 的 1 尺」与「同在 loc-a 的 100 尺」分开。这是 **GAP-B**，不是 A 的实现缺陷。
2. **T24 记的那条「声明与实现矛盾」是 A 里唯一的实现缺陷，它已经闭合。** §R4-5 逐条对账：实现与开放内容的
   `range_proxy / signature_scope / incapacitated_statuses / ally_proxy` **完全一致**。所以 A 的「近似提高」现在
   **纯粹由 B 造成**，而不是「实现没做到声明」。
3. **按前三轮一贯的登记口径，「闭合」= 规则结论正确。** 现在规则结论仍不精确（只是近似口径已被开放内容如实声明）。
   把 A 改成「闭合」会**高估**；更准确的分层是：
   **A 的「跨实体读取能力」闭合；A 的「集群战术规则」按声明的近似口径实现一致，精确性受 B 限制 → 维持「近似提高」。**

> 建议（不实施，本轮不修产品代码）：如果要把口径说得更准，可把 A 拆成两条登记
> ——「A1 跨实体读取能力（已闭合）」/「A2 集群战术的距离近似（受 B 限制）」。这会比现在一条 GAP 混装能力和结论更清楚。

### R4-5. 开放内容声明 vs Lua 实现：逐条对账（不再矛盾）

（读 `story_example/lmop-storybook.json` 的 `definitions[kind=dnd-pack-tactics]`.fields 与 `lua_mounts[dnd-pack-tactics]`.source 原文）

| 声明（开放内容 fields） | 实现（脚本行） | 对得上？ |
|---|---|---|
| `range_proxy`：「与目标同一 `location_id`（目标没有 `location_id` 时不给优势，fail-closed）」 | `local target_place = target.location_id` → `if type(target_place) ~= "string" or target_place == "" then return end` → `inst.location_id == target_place` | ✅ |
| `signature_scope`：`kind == attack`（description：「攻击检定有优势」） | `if host.check_kind ~= "attack" then return end` | ✅ |
| `incapacitated_statuses`：`["stunned","unconscious","paralyzed","petrified","incapacitated"]` | 运行时读 `host.get_definition(def_id).fields.incapacitated_statuses` → 建表 → 按 `st.id or st.name` 判失能 | ✅（且**数据驱动**） |
| `ally_proxy`：「所有活跃遭遇里的其它敌人实例（不限与我同场）；HP>0 且不带 `incapacitated_statuses` 才算未失能」 | `for _, enc in ipairs(host.list_encounters())`（引擎只列 active）→ 遍历 `enc.enemies` 除自己 → `able(inst)`（HP>0 且无失能状态） | ✅ |

本轮四条**独立反证**（不是读代码，是跑出来的）：

```text
R4-6 失能名单数据驱动: 基线 stunned=0/prone=1 → 只改名单（["prone"]）stunned=1/prone=0
R4-6 声明对账 PASS: 非活跃遭遇=0 / 不限同场=1 / 目标缺地点=0 / 无状态名常量
```

- 只改 `definitions.fields.incapacitated_statuses`（脚本逐字不动）→ 期望翻转 ⇒ 名单真的来自开放内容，不是 Lua 常量。
- 脚本里**没有任何状态名字符串**（逐个状态名 grep 字面量 = 0 命中）。
- 「所有**活跃**遭遇」：非活跃遭遇里的同伴**不算**（=0）；「不限与我同场」：我不在任何遭遇、但活跃遭遇里有同地点同伴**算**（=1）。
- 目标缺 `location_id` → fail-closed（=0）。

**结论：T24 记的「声明与实现不一致」已经消除，四条声明与实现逐条对上。** 唯一仍属「近似」的是
声明自己承认的那件事——精确 5 尺做不到（GAP-B），以及「其它敌人实例」是 proxy 而不是真正的盟友关系（声明已如实写出）。

### R4-6. 本轮新发现（不替任何人圆场）

#### ① 「伏击」仍然不看判定签名（**缺陷候选**，与 T24 记的 FP-E 同类）

T24 在集群战术上报的 FP-E（非攻击判定也给优势）被 T26 修了，但**同一类误判在 `dnd-ambusher-keep-high` 上原样保留**：
数据卡写的是「对任何成功受其**突袭**的生物所发动的**攻击检定**具有优势」，
而脚本只读 `host.target.statuses`，**没有** `host.check_kind` 闸门。本轮自造场景实测：

```text
R4-6 伏击签名: attack=1 attribute=1 save=1（数据卡只说攻击检定）
```

即：对同一个「受突袭」目标，**属性检定与豁免检定也拿到优势**。这不是 GAP，是**规则包自己写宽了**，
和 T24 记的 FP-E 是同一条判据的同类缺陷。**本轮原样钉住现状（断言观察值 = 1），未修产品代码。**
>
> **⚠️ 第五轮订正**：该缺陷已由 T29 按开放内容数据卡的「攻击检定」口径收敛（`host.check_kind ~= "attack"` 早退）；
> 旧实况（旧脚本 attack/attribute/save 全给 = 1/1/1）以 `OLD_AMBUSHER` 常量**可执行地**存活于
> `scripts/lmop-engine-check/src/main.rs`，由引擎级断言与 §R5-1 独立跑通。本段原文保留为第四轮的历史实况。

#### ② 归因错位（低 severity，但影响可追溯性）

T26 把这轮修复在代码 / 测试 / 文档里统一记成「**T24 修正**」：
`scripts/lmop-rulepack.mjs`（5 处）、`scripts/lmop-engine-check/src/main.rs`（3 处）、
`crates/octopus-api/tests/lmop_verification_round3.rs`（7 处）、`story_example/lmop-import.md`（1 处）。
按本任务的编号，**T24 是第三轮验证者（只报告、不实现），T26 才是实施修复的那个 agent**。
同一批注释里还并存「T24 **之前**的旧脚本」这种把 T24 当时间点的用法，两种含义混在一起。
建议后续统一改成「T26 修正」或「第四轮修复」——**不影响功能，但会让下一轮验证者追错责任链**。

#### ③ 新增的 `--check` 一致性断言是**字符串匹配**（低 severity）

T26 在 `runChecks` 里加的 4 条 GAP-A 断言是 `packSource.includes('inst.location_id == target_place')`
一类的**源码子串**检查。它们**可以被注释 / 死代码满足**（例如脚本里加一行含该子串的注释即可通过），
也不检查该表达式是否真的参与判定。**缓解事实**：同一条主张另有**行为级**断言兜底
（`r3_audit4` 的场景断言、engine-check 的 `pack_tactics.reads_world_facts`、本轮 §R4-1 的变异验证），
所以这不是真空的断言；但它本身**不是**机制判据，不应被当成「声明与实现一致」的机器保证。

#### ④ 设计层观察：`range_proxy` 是自然语言，无法被机器校验

`range_proxy / ally_proxy` 这些「近似口径声明」是散文；机器能查的只有 `signature_scope`（可枚举）、
`incapacitated_statuses`（数组）、`target_status` 这类**结构化字段**。
本轮之所以能做逐条对账，靠的是**人读 + 自造场景**，不是工具。这套设计要长期维持「声明即契约」，
需要把可结构化的口径尽量结构化（本轮不实施）。

### R4-7. 环境与复现命令 / 退出码（第四轮）

```bash
cd /home/huang/Personal/Dev/Code/octopus
export CARGO_TARGET_DIR=$PWD/.scratch/engine-test-target

# 引擎级：本轮新增的独立用例（9 条）
cargo test -p octopus-api --test lmop_verification_round4 -- --test-threads=2 --nocapture
#   → 9 passed; 0 failed

# 全套回归
cargo test -p octopus-api                              # lib 74 + r1 13 + r2 14 + r3 14 + r4 9，全 0 失败
cargo test -p octopus-engine -p octopus-ai             # 357 + 36 passed，0 失败

# 交付物侧自检
node scripts/lmop-rulepack.mjs --check                 # EXIT=0，28 项断言 0 失败 + 14 条 GAP（闭合 7 / 近似 1 / 仍存在 6）
node scripts/lmop-rulepack.mjs --out /tmp/r4-regen-storybook.json   # EXIT=0；sha256 == 交付物（逐字节相同）
node scripts/import-bestiary.mjs --check               # EXIT=0，28/28
./scripts/lmop-engine-check.sh                         # EXIT=0，11 条规则断言 / 0 rule_failures / lua 0 issue
./scripts/lmop-boundary-check.sh                       # EXIT=0，禁词零命中
```

| 命令 | 退出码 | 关键结果 |
|---|---|---|
| `cargo test -p octopus-api --test lmop_verification_round4` | 0 | **9 passed / 0 failed** |
| `cargo test -p octopus-api` | 0 | lib 74 / r1 13 / r2 14 / r3 14 / r4 9，全部 0 失败 |
| `cargo test -p octopus-engine -p octopus-ai` | 0 | 357 + 36 passed / 0 failed |
| `node scripts/lmop-rulepack.mjs --check` | 0 | 28 项断言 0 失败；14 GAP = 7 / 1 / 6 |
| `node scripts/lmop-rulepack.mjs --out /tmp/…` | 0 | sha256 `ffc93b3e…` == 交付物 |
| `node scripts/import-bestiary.mjs --check` | 0 | 28/28 |
| `./scripts/lmop-engine-check.sh` | 0 | 11 规则断言全过 / 0 error / 0 lua issue |
| `./scripts/lmop-boundary-check.sh` | 0 | 禁词零命中 |

**验证基线（本轮实测时的文件指纹，便于复现）**：

```text
story_example/lmop-storybook.json              sha256 ffc93b3e8cab551c6964aa792a2abc94625727dcc27e376f787e4a334e9cfdaa
scripts/lmop-rulepack.mjs                      sha256 7f63e4bab79f92892a5e8d05fb5313aae6f070a8e40d9a1e8db57f32dc0d720d
crates/octopus-api/tests/lmop_verification_round4.rs  sha256 9f0704e2f1338c14faf6811bcdfba2bcf573c176ada1408eac303f71b583734c
scripts/lmop-engine-check/src/main.rs          sha256 17cdcc0ee23feb29d8fed13ea8bb07ca0b7804c96052b015b5e66d672483a6a4
```

**并发改动提示（如实登记）**：本轮审计期间，`scripts/lmop-engine-check/src/main.rs` 在 **12:59** 被**另一次改动**重构过
（把 pack 场景抽成 `run_pack_cases`、旧脚本抽成 `OLD_PACK_TACTICS` 常量；断言名与语义不变）。
我第一次跑 engine-check 是在该重构之前，**已在该重构之后重跑一遍**：仍是 11 条规则断言全过 / 0 error / 0 lua issue。
本报告引用的断言名 `pack_tactics.old_impl_reproduces_misjudgments` 与当前文件一致。

### R4-8. 未通过 / 未覆盖 / 副作用（不得隐瞒）

**未通过：无（0 项）。**

必须声明的**覆盖边界**：

1. **本轮没有跑真实 HTTP 路径（没有起服务、没有碰真实 `octopus.db`）。**
   原因：第四轮的回归清单是 r1 / r2 / r3 / engine / api / ai / rulepack / engine-check / boundary（不含 HTTP），
   且任务明确要求保护既有 1 本故事书 + 1 个存档。
   **发布门的覆盖没有丢**：r3 / r4 的每条用例都走真实 `publish_and_open`（内存库 + 真实 validate / lint），
   交付物在真实发布门上是 0 error 的；上一轮的 HTTP 证据（13/13）仍然是对应版本之前的最后一次 HTTP 级验证，
   **T26 之后没有再跑过 HTTP**——这一条如实登记。
2. **GAP-F 的非 strike 击杀路径**仍然只有代码复核（与第二、三轮同）。
3. **本轮所有 `octopus.db` 操作数为 0**：所有会话用例用 `SqliteStore::open_in_memory()`；
   `--out` 只写 `/tmp`；`import-bestiary --check` 不写文件。
   既有的用户故事书 / 存档 / octopus 账户**未读改、未删除**（`git status` 里没有 `octopus.db`）。

**副作用 / 新增数据**：

- **真实 `octopus.db`：本轮新增 0 条验证数据**（无新账户 / 故事书 / 存档），因此没有需要清理的 id。
- 新增文件只有 `crates/octopus-api/tests/lmop_verification_round4.rs` 与本节报告改动。

### R4-9. 报告里「修正前实况」的标注清单（T26 已指出，本轮落实）

在不删改第三轮原文的前提下，本轮在上述各处加了 **⚠️ 第四轮订正** 标注：

| 位置 | 标注内容 |
|---|---|
| §R3-0 摘要表「四件专门审计」 | ③④ 已由 T26 修掉并被独立复现 |
| §R3-0「一句话」 | 该段列出的全部问题「已修」；旧实况仍被复现为真 |
| §R3-3 审计③a（a12/a12b） | 注释 / 判据已订正；「机制盲」判断描述的是修正前 |
| §R3-3 审计③b（r2_6） | 描述已订正、诊断价值已恢复；「静默丢弃无日志」仍成立 |
| §R3-3 审计④（集群战术） | 5 类误判与 `range_proxy` 矛盾已在 T26 修正 |
| §R3-4 GAP-A / GAP-B 行 | 5 类已修；A 的残余不精确只剩 GAP-B |
| §R3-4 汇总 | 数目不变，但 A 的「近似提高」理由变了 |
| §R3-5 被推翻的证伪 / 仍成立的证伪 / 风险清单 / 结论 | 逐条标注哪一半已消除、哪一半仍成立（伏击签名不收敛） |

### R4-10. 这套设计成立性的第四轮判断

**第四轮被推翻的第三轮证伪 / 风险**

| 第三轮仍成立的判断 | 第四轮 |
|---|---|
| 「集群战术能得出正确结论」仍被证伪（5 类误判） | **推翻**：5 类已修，且旧脚本原文复现证明「当时确实错」；残余不精确**只剩 GAP-B 的距离概念** |
| 开放内容 `range_proxy` 与脚本实现不一致 | **推翻**：实现已对齐声明（真检查同伴位置 + 缺地点 fail-closed）；§R4-5 逐条对账 |
| r2_6 负对照机制描述失实、诊断价值消失 | **推翻**：描述已订正，判据升级为「读 Resolution.state_changes 的 ghost delta」，可区分「跳过」与「丢弃」 |
| a12 / a12b 断言「机制盲」 | **推翻**：PostResolve 探针 + hp 对账；本轮两种坏机制（旧重掷 / 缩放+Lua 双补）都被抓 |
| `scale_effect` 的「因子 1.0 也开门」陷阱 | **仍成立**（T26 未动；这是原语语义，规则作者需要知道） |

**第四轮仍然成立的证伪 / 缺口**

- **「引擎有位置概念」仍被证伪**（GAP-B，`distance` 0 命中）——这也是 GAP-A 仍然只能是「近似提高」的唯一原因。
- **GAP-G / I / J / K / M** 五条与第一轮登记时实质相同（预置数量静态、无轮次经济、无被动对手条件入口、无模板初始状态、无 `encounter_active`）。
- **新登记（本轮）**：「规则包按判定签名收敛」**没有成为通用约定**——日照敏感做了、集群战术这轮补了，
  **伏击没做**（§R4-6 ①）。这不是引擎能力问题，是规则包的一致性欠账；同类问题可能还有别处。
  > **⚠️ 第五轮订正**：伏击已按 `kind == attack` 收敛（§R5-4）；但「4 条不读签名的挂载点没有结构化声明」
  > 仍在（§R5-5），所以这条的**后半句「没有成为通用约定」在第五轮依然成立**，只是样本少了一个。

**第四轮对设计成立性的判断**

前三轮的判断是「路对、原语不够（R1）→ 边界变宽（R2）→ 读事实的缺口补齐（R3）」。
第四轮在 T26 之后可以补一句：**这套设计第一次做到了「声明即契约，且声明可被独立验证」**——
T26 的修复本质就是把实现对齐开放内容里已经写下的 `range_proxy / incapacitated_statuses`，
修完之后我用「改开放内容 → 期望翻转」的反证确认这些字段**真的在驱动行为**，而不是装饰性文档。

但**设计里没有任何机制强制维持这种一致性**：`range_proxy` 是散文、
`--check` 的一致性断言是源码子串匹配（§R4-6 ③），
真正把它钉住的是**行为级测试 + 变异验证**（本轮 r4 用例、r3_audit4、engine-check）。
所以更准确的说法是：**设计成立，且这次是三处「实现落后于声明」被追平；但「实现落后于声明」这类漂移
仍是这套设计的主要风险形态**——它不会报错，只会静默地给出错误结论（T24 的 5 类误判就是实证）。
下一轮若要继续加固，优先级建议：①把可结构化的近似口径继续结构化（§R4-6 ④）；
②把「判定范围 `signature_scope`」变成所有规则挂载点的通用声明并按它收敛（先补伏击）；
③把「声明 vs 实现的语义一致性」从字符串匹配升级为行为断言。

### R4-11. 本轮新增 / 改动文件

| 文件 | 作用 |
|---|---|
| `docs/lmop-verification-report.md` | 本报告（§0–§10 / §R2 / §R3 **原样保留**，§R3 内加「⚠️ 第四轮订正」标注，追加 §R4） |
| `crates/octopus-api/tests/lmop_verification_round4.rs` | **新增**：9 条独立用例（旧/新脚本对照、5 处单点变异、声明对账、失能名单数据驱动、a12 机制探针 + 两种反例、r2_6 引擎级 + 会话级区分、伏击签名残留） |

**未改动**：`crates/*/src/` 与 `frontend/src/` 的任何产品代码；
**未改动**任何既有测试文件（`lmop_verification.rs` / `lmop_verification_round2.rs` / `lmop_verification_round3.rs`）；
**未改动** `story_example/lmop-storybook.json`（只读；所有变异都施加在内存副本上）。













---

## 第五轮·独立复核（T28/T29 四项改动 + T29 对验证者用例的两处改动）

> 角色：**全新的独立验证者（第五轮）**（未参与 T28/T29 的任何实现与验证，也未参与前三、四轮）。
> 只验证，不修产品代码；inScope：`docs/lmop-verification-report.md` · `scripts/`（只新增） ·
> `crates/octopus-api/tests/`（**只新增**）。
> **未改动**任何产品代码与任何既有用例：对本轮被翻转的 `r4_ambusher_still_ignores_check_signature`（现名 `r4_ambusher_check_signature_attack_only_after_r5_fix`）
> 与第三轮 harness `run_ambusher` 走「读 + 跑 + 旧实况复现 + 逐条变异推理」。
> 被测交付物：`story_example/lmop-storybook.json`，sha256 `ce6b557f9d95c816c852a0ee08e6e4cb4b4663d21bc4ab8cd5882a12784a4f01`
> （第四轮是 `ffc93b3e…`——T26 与 T29 之后已不是同一版）。第五轮实测：用 `scripts/lmop-rulepack.mjs` **重新生成**，
> 产物与交付物**逐字节相同**（同 sha256）→ 交付物仍是规则包脚本的唯一产物。
> 查询口径：**不采信 T29 自述**；下面每一条都是本轮自建 harness / 自造场景 / 自建变异跑出来的。

### R5-0. 结论摘要

| | 结论 | 说明 |
|---|---|---|
| **T29 翻转第四轮用例**（`r4_ambusher…`） | **合法（未弱化）** | 旧实况以可执行形式存活：`OLD_AMBUSHER` 与 git HEAD 原文**逐字节相同**（974 字节，sha256 `77a72f91…`），本轮独立跑出 1/1/1；翻转后的断言逐条有牙齿（3 个变异全红）；行为确实变了（有当时的失败日志为证） |
| **T29 改第三轮 harness**（`run_ambusher` 补判定签名） | **合法（忠实）** | 真实引擎里每一次**真判定**的 `check_pre_roll` 都必然带签名（两处注入点都恒写 `kind: Some(...)`）；无签名只对应「根本没有判定」。四条期望值与数据驱动段逐字未改 |
| **T28-A force_effect 正交性** | **成立** | 自造技能三组对照：force 单独 = 全量且不缩放；`scale_effect(1.0)` = 旧语义（开门、数值不动）；不声明 = 逐字旧路径 |
| **T28-B apply_delta 可见化** | **成立** | 投影序列化前后逐字相同；丢弃确有 WARN；命令日志零新增事件；同形态重放只告警一次 |
| 伏击签名 + fail-closed | **成立** | attack→给 / attribute→不给 / save→不给；无快照或 kind 缺省→不给；attack 对照排除恒 0 |
| 「16 条挂载点里只有 ambusher 缺签名」 | **成立（限 T26 之后的口径）** | 16 条中 10 条判定类；读签名的 6 条，不读的 4 条全是「对任意判定都成立」的口径。HEAD 上 pack-tactics 也没有闸门，T26 补上后只剩 ambusher——说法准确 |
| `--check` 行为级兜底 | **在场且能区分** | 5 个变异（源码 / 开放内容各半）全部让声明的引擎级断言变红；`consistency.behavioural_backing` 20 条引用无一悬空。**子串断言本身仍可被注释满足**（T27 发现③未被消除，靠行为级兜底救场） |
| 14 条 GAP | **闭合 7 / 近似提高 1 / 仍存在 6**（与第四轮数目一致） | 无新增闭合、无回退；本轮无 GAP 状态变化 |
| 全套回归 | **全绿** | engine 361 / ai 36 / api-lib 74 / r1 13 / r2 14 / r3 14 / r4 9 / **r5 9（新增）** / rulepack 28 / engine-check 20 条运行时断言 / boundary 0 命中，全部 0 失败 |
| 本轮新发现 | 1 条低危声明漂移 + 2 条设计层观察 | ①`rule-dnd-proficiency` 声明的挂载点是 `check_post_roll`，真实挂载点是 `check_pre_roll`（`--check` 不校验这层）；②`--check` 的源码子串仍可被注释满足；③丢弃 WARN 的去重是**进程级、跨存档**的 |

**一句话**：T29 的两处改动都**经得起独立复核**——它把「伏击不看判定签名」这个第四轮钉住的实况修掉了，
同时**没有抹掉历史证据**（旧脚本原文进了引擎校验器，被断言在同一批场景复现 1/1/1）；
第三轮 harness 补签名也不是「改测试迁就实现」，而是把「不可能出现在真判定里的状态」纠正成真实状态
（真判定必有签名，已读代码 + 独立结构断言双重确认）。
**但不替 T29 圆场的有三条**：①源码子串型一致性断言**仍然**是「注释即可满足」的形态（§R5-6 实证）；
②`rule-dnd-proficiency` 的挂载点声明与实现不一致（§R5-8①，低危但属同类漂移）；
③丢弃 WARN 一旦某形态出现过，**跨存档、跨会话**都不再提示（§R5-8③）。

---

### R5-1. T29 对验证者用例的两处改动 —— 审计（本轮最重要产出）

#### (a) 两处改动是什么

1. `crates/octopus-api/tests/lmop_verification_round4.rs`：第四轮验证者写的
   `r4_ambusher_still_ignores_check_signature`（**如实钉住现状**：断言源码里没有 `host.check_kind`、
   且 attribute / save 也 = 1）被**改名并翻转**为 `r4_ambusher_check_signature_attack_only_after_r5_fix`
   （断言 attack=1、attribute=0、save=0），**去掉了源码子串断言**。
2. `crates/octopus-api/tests/lmop_verification_round3.rs` 的 `r3_16` harness `run_ambusher`：
   原本构造 `MountEnv` 时**不给 check 快照**（`check: None`），现补上
   `LuaCheckContext { attribute:"str", kind: Some(CheckKind::Attack), target:12 }`。
   `r3_16` 的四条期望值与后面的数据驱动段**一字未改**（`git diff` 只落在 `run_ambusher` 与 `r3_audit4` 两处）。

#### (b) 判定标准（沿用第四轮 T27）

不是「测试通过」，而是：①**旧实况是否仍然可执行地存在**（只改被翻转的断言而不留任何可执行旧实况 = 弱化）；
②**harness 改动是否忠实**；③**翻转后的断言是否仍有牙齿**（变异）。

#### (c) 证据 1 —— 旧实况仍然可执行地存在（**是**）

外部核对（本轮实测）：

`
$ git show HEAD:story_example/lmop-storybook.json → lua_mounts[dnd-ambusher-keep-high].source
  974 字节，sha256 77a72f91922d78d0ce94eeb96abdf5bbb379abe426755d73720d77e67cdd6755
$ 从 scripts/lmop-engine-check/src/main.rs 抽 const OLD_AMBUSHER
  974 字节，sha256 77a72f91922d78d0ce94eeb96abdf5bbb379abe426755d73720d77e67cdd6755
  → BYTE EQUAL: true
`

本轮新增的 `lmop_verification_round5.rs` 里 `r5_ambusher_old_reality_is_executable_and_new_one_is_gated`
**独立跑这段常量**（自己的 harness，不复用 T29 的断言函数）：

`
R5 伏击 旧脚本(HEAD 原文)=(1, 1, 1) 新脚本(交付物)=(1, 0, 0)
`

即：T27 的三条原始发现（attack / attribute / save **全部**给优势）**可执行地存活**，
不依赖任何会被翻转的断言。引擎校验器另有 `ambusher.old_impl_ignores_check_signature` 在同一批场景复现 1/1/1
（本轮实跑通过）。**结论：不属于「抹掉历史证据」。**

**行为真的变了**（旁证）：本机留有改动当口的失败日志（`/tmp/baseline-r4.log` 13:08、`/tmp/v5.log` 13:13）：
改动脚本之前，旧用例 `r4_ambusher_still_ignores_check_signature` **通过**（9 passed）；
改动之后同一份旧用例 **FAILED**，打印 `attack=1 attribute=0 save=0`，并在
`assert!(!source.contains("host.check_kind"))` 处 panic。说明翻转是**行为变化**，不是把期望值改小 / 改没。

#### (d) 证据 2 —— 第三轮 harness 改动忠实（**是**）

T29 的论证是「真实引擎在 `check_pre_roll` 永远下发判定签名，旧 harness 模拟的是不可能出现的状态」。
本轮独立核验：

- `command.rs`：`check_signature()` **恒写** `kind: Some(kind)`；
  技能路径 `signature = skill_checker(...).is_some().then(|| check_signature(...))`
  ——有判定器才有签名；没有判定器 = 根本没有这次判定。
- `session.rs`：判定意图路径 `run_mount_chain(LuaMount::CheckPreRoll, &mount_ctx, Some(&signature), None)`
  **无条件**下发；缺省 kind = `CheckKind::Attribute`。
- 全仓 `check_pre_roll` 的运行时调用点只有这两处（本轮新增用例把这两条结构与计数都写成断言）。

所以「无签名」只对应「**没有判定**」——那时 `modify_check('keep_high')` 本来也无判定可改。
旧 harness 用 `check: None` + 「目标带 dnd-surprised」去期望 1，模拟的是一个真实引擎不会出现的组合
（**有判定却拿不到签名**）。补上 attack 签名 = 让 harness 回到真实状态，**忠实**。
`r3_16` 的四条期望值与数据驱动段逐字未改；「没有签名 → 不给优势」这一边界改由引擎级断言
`ambusher.fail_closed_without_signature` 与第五轮新增用例显式钉住，覆盖没有丢。

#### (e) 证据 3 —— 翻转后的断言仍有牙齿（**是**）

新一轮独立变异（每次只改一处，跑同一个 harness）：

`
R5 变异 A（拿掉 kind == attack 闸门）      → (attack, attribute, save) = (1, 1, 1)  ← 新断言期望 0
R5 变异 B（闸门写成恒不早退）             → (attribute, save) = (1, 1)            ← 新断言期望 0
R5 变异 C（删掉最终 host.modify_check）   → attack = 0                            ← 新断言期望 1
`

三条都让翻转后的断言（或两端的对照）翻红：**不是恒真、也不是「怎么都算过」**。
「必须给」端（attack=1）与「必须不给」端（attribute/save=0）都被钉住。

#### (f) 结论：**两处改动都合法，未弱化**——附一条过程性条件

这是「实施者改验证者资产」最敏感的一类改动。它之所以安全，是因为**旧实况没有消失**：
旧脚本原文进了 `scripts/lmop-engine-check/src/main.rs`（`OLD_AMBUSHER`，与 HEAD 逐字节相同）并由引擎级断言复现，
本轮又补了一枚 cargo test 层的独立钉子（`r5_ambusher_old_reality…`）。
**如果当时只翻转 `r4_ambusher…` 而不给旧实况留任何可执行记录，同一改动就应判「弱化」。**
另需说明的是：翻转后的用例**去掉了源码子串断言**，这与 T27 发现③的方向一致（子串断言弱于行为断言），
不是放宽。

---

### R5-2. T28-A：`host.force_effect()` 与 `scale_effect` 的正交性（独立复现）

自造技能 `sk-r5-gate`（**不引用 LMoP、不复用引擎单测 fixture**）：豁免判定 + 1d6 伤害 + 2d6 资源 + 置标记 + 消耗 5；
结果由判定前挂载点钉死（force_success / force_fail），固定种子 11。

`
R5 效果门: 无声明成功=[-5]  全量=[-1, 7, true, -5]  force=[-1, 7, true, -5]
           scale1.0=[-1, 7, true, -5]  scale0.5=[0, 3, true, -5]
`

| 对照 | 期望 | 实测 | 判定 |
|---|---|---|---|
| (c) 不声明 + 豁免成功 | 逐字旧路径：效果**完全不结算**，只剩消耗 | 只有 1 条 `resources.mana=-5`，1 颗骰 | **通过** |
| (a) 只声明 `force_effect()` | 开门、**不缩放**：与「豁免失败的全量结算」逐字相同 | deltas / 骰序 / 静态修正全等于全量（[-1,7,true,-5]，4 颗骰） | **通过** |
| (b) 只声明 `scale_effect(1.0)` | 旧语义：**也开门**，因子 1 数值不动 | 与 (a) / 全量逐字相同；且**不等于**「不声明」 | **通过** |
| force + scale(0.5) | 开门 + 缩放 | 与只 scale(0.5) 完全相同（[0,3,true,-5]） | **通过** |
| 只 scale(0.5) | 开门 + 缩放（非数值不缩） | hp `-1→0`、mana `7→3`（向零取整），标记 / 消耗不动 | **通过** |

**旧语义逐字保留**的关键点是 (b)：`scale_effect(1.0)` 仍与「不声明」行为**不同**（前者开门）。
本轮没有改坏第四轮及以前的规则包与用例：`r3_audit1_17` / `r3_18` / `r4_a12` 等旧用例原地重跑全绿。
另有边界：`force_effect()` 只在 `check_post_roll` / `pre_resolve` 注册，错时机调用**当场报错**
（不是静默丢请求），与 `scale_effect` 同口径（本轮独立实测 5 个挂载点）。

---

### R5-3. T28-B：`apply_delta` 丢弃可见化（独立复现）

| 主张 | 本轮独立复现 | 结论 |
|---|---|---|
| 投影序列化前后**逐字相同** | 重放路径：`serde_json::to_value(session.projection())` 前后相等（喂入同 seq 的合成事件） | **成立** |
| 丢弃时**确有 WARN** | 实时路径（真实回合 + 不存在的 target_id）：恰好一条 WARN，带 `domain=Character` / `entity_id` / `field=resources.res-hp` / `op` | **成立** |
| **不得**新增落进命令日志的事件 | 重放后事件数 = 原事件数 + 输入事件数（无额外 System 事件）；日志里 grep 不到「丢弃 / 落不到」类事件 | **成立** |
| 重放**不刷屏** | 同一批输入里同形态出现两次，同形态只告警 1 次；不同形态各告警 1 次 | **成立** |

原始片段（实时路径）：

`
WARN 状态增量落不到任何角色实例，已丢弃（投影逐字不变）
  domain=Character entity_id=verify-r5-ghost-<uuid> field=resources.res-hp op=Add
`

机理复核：`warn_dropped_character_delta` 只调 `tracing::warn!`，**不 push 任何事件**；
去重表是 `OnceLock<Mutex<HashSet<(domain, entity, field)>>>`，上限 256。
本条改动的**行为面为零**：除了日志，世界状态 / 事件日志 / 投影都不动。

**覆盖边界（如实登记）**：去重是**进程级**的——同一形态在进程生命周期内只提示一次，
且**跨存档共享**（A 存档丢弃过 `(Character, k, f)`，B 存档同样丢弃不再提示）；
到 256 上限后，重复形态仍被识别，但**新形态每次都告警**。这是取舍，不是缺陷，但会削弱「复发」的可见性。

---

### R5-4. 伏击判定签名与 fail-closed（独立复现）

| 场景 | 期望 | 实测 |
|---|---|---|
| `kind == attack` + 目标带 `dnd-surprised` | 给优势 | keep_high = **1** |
| `kind == attribute`（同一目标） | 不给 | **0** |
| `kind == save`（同一目标） | 不给 | **0** |
| 无判定快照（`host.check == nil`） | fail-closed | **0** |
| 有快照但 `kind == None`（`host.check_kind == nil`） | fail-closed | **0** |
| attack 对照（同一 harness） | 给（排除恒 0） | **1** |

数据驱动旁证：只把开放内容 `ambush-doppelganger.fields.target_status` 改成 `dnd-prone`（脚本逐字不动），
期望随之翻转（prone→1、surprised→0）——引擎级 `ambusher.target_status_data_driven` 通过。

---

### R5-5. 「16 条挂载点里只有 ambusher 缺签名」的独立复核

本轮自扫 16 条 `lua_mounts`（7 `check_pre_roll` / 3 `check_post_roll` / 1 `status_tick` / 3 `turn_end` / 2 `event`）。
判定类共 10 条，读判定签名的 6 条：

`
读签名：dnd-ambusher-keep-high / dnd-pack-tactics / dnd-proficiency / dnd-save-half / dnd-sunlight-sensitivity / dnd-surprise
不读签名：dnd-consume-inspiration / dnd-skill-dc / dnd-status-keep-high / dnd-status-keep-low
`

对 4 条不读签名的逐条核对「是否确有理由」：

| 挂载点 | 声明口径 | 是否需要签名 |
|---|---|---|
| `dnd-status-keep-high` | 有 `dnd-inspired` 状态 → 任意检定优势（5e 激励可用于任意 d20） | **不需要**（对任意判定都成立） |
| `dnd-status-keep-low` | 有 `dnd-eruption-penalty` → 任意检定劣势 | **不需要** |
| `dnd-skill-dc` | 把技能声明的 DC 落到本次判定 | **不需要**（跟随技能自己的判定） |
| `dnd-consume-inspiration` | 任意一次判定后消耗激励 | **不需要** |

这 4 条都没有声明过 kind 范围（无 `signature_scope`），脚本也没有把某种判定排除在外——
「不读签名」与声明一致。**HEAD 口径核对**：`git HEAD` 上 `dnd-pack-tactics` 与 `dnd-ambusher-keep-high`
**都没有**闸门；T26 补了 pack-tactics，T29 补了 ambusher。
所以第四轮 T27 说的「16 条里只有 ambusher 缺签名」，**限定在 T26 修正之后**成立且准确：
当时数据卡明说「攻击检定」的两条规则里，只剩 ambusher 没按签名收敛。本轮实测当前交付物两条都已有闸门。
**如实指出边界**：这 4 条「不需要」的依据是 `rule-dnd-*` 定义里的 coverage 文本 + 数据卡语义，
**不是机器可校验的结构化声明**（它们连 `signature_scope` 都没有）——「设计是否要强制所有判定类挂载点声明
`signature_scope`」是下一轮可以讨论的加固点，本轮不实施。

---

### R5-6. `--check` 的行为级兜底：复核（在场 + 真能区分）

#### (a) 声明不悬空

`node scripts/lmop-rulepack.mjs --check --engine .scratch/lmop-engine-check-target/debug/lmop-engine-check`：

`
[PASS] engine.publish_gate — validate_storybook 错误 0 / lua_lint 问题 0（警告 24）
[PASS] consistency.behavioural_backing — 20 条被引用的引擎级断言 全部在场（声明不悬空）
总结：50 项断言，0 项失败（含真实引擎发布门 + 20 条运行时规则断言）
`

**独立核对**：引擎校验器当前共 20 条 `rule_assertions` / 0 `rule_failures`——
即「被引用的 20 条」与「实际存在的 20 条」是同一集合，没有悬空声明，也没有未被引用的行为断言。

#### (b) 抽 5 条变异，逐条确认「真的能区分」（新增脚本 `scripts/lmop-r5-mutation-audit.mjs`）

本轮**不采信** `--check` 自己的 PASS，改成「只改一处 → 用真实引擎重跑 → 看声明的断言是否变红」：

`
[基线] 交付物：6 条目标断言全部在场且通过
[PASS] M1 拿掉伏击的 kind==attack 闸门（源码）        → ambusher.check_signature 已变红
[PASS] M1                                                → ambusher.fail_closed_without_signature 已变红
[PASS] M2 清空集群战术的失能名单（开放内容）          → pack_tactics.reads_world_facts 已变红
[PASS] M3 注释掉 dnd-save-half 的 scale_effect(0.5)（源码） → save_half.engine_scales_own_roll 已变红
[PASS] M4 只改开放内容 target_status=dnd-prone（脚本不动） → ambusher.target_status_data_driven 已变红
[PASS] M5 去掉日照敏感的 attribute+wis 分支（源码）   → sunlight.by_signature 已变红
`

**结论：`--check` 里那 7 条子串型断言声明的「行为级兜底」是在场的、且真的能区分。**
本轮抽的 5 条变异（3 条改源码、2 条只改开放内容）**无一只靠子串混过去**。

#### (c) 但子串断言本身**仍然**可被注释满足（T27 发现③未被消除）

本轮实证：把 `dnd-ambusher-keep-high` 的真实闸门**注释掉**、注释里保留同一子串，然后跑
**不带 `--engine`** 的 `--check`：

`
[PASS] ambusher.target_status — ……只对 kind==attack 生效……
总结：28 项断言，0 项失败            ← 纯子串模式**没能**发现闸门已死
`

同一份变异交给真实引擎：`ambusher.check_signature=false`、`ambusher.fail_closed_without_signature=false`、
`rule_failures=2`。**即：`--check` 的基础模式对「实现已死」仍然盲；救它的是行为级兜底，不是子串断言本身。**
这和第四轮 §R4-6③ 的判断一致，T29-B 的加固**改善了可追溯性（谁兜底、怎么区分写清楚了）并加了悬空检查**，
但**没有**把子串断言升级成机制判据。本报告不把它记成「已闭合」。

---

### R5-7. 14 条 GAP 的第五轮现状（独立复核，不是照抄 T29 自述）

复核方法：①规则包 `--check` 的逐条 status；②本轮自己去 `crates/*/src` 重新 grep 原语 / 结构；
③运行时反例（本轮 r5 用例 + 前三、四轮用例重跑）。

| GAP | 第五轮现状 | 独立证据（本轮实测） |
|---|---|---|
| A | **近似提高**（不升为「闭合」） | 跨实体读取能力闭合；集群战术按声明近似实现一致；「目标 5 尺内」仍不可表达 |
| B | **仍存在** | `\bdistance\b` 在 `crates/octopus-engine/src` **0 命中**；只有 `location_id` 归属 |
| C | **已闭合** | `get_attachment / get_definition / list_definitions` 在脚本里真实驱动行为（本轮 M4 反证） |
| D | **已闭合** | `check_pre_roll` 下发签名；本轮 M1/M5 变异证明两条按签名收敛的规则都有牙齿 |
| E | **已闭合** | `scale_effect(0.5)` + PostResolve 快照；本轮 M3 + force/scale 正交对照 |
| F | **已闭合（边界不变）** | `enemy_defeated` 逐只发；边界仍只在 strike 路径（未构造非 strike 运行时反例） |
| G | **仍存在** | `EncounterPresetEnemy.count: Option<u32>`，无骰式落点 |
| H | **已闭合** | `set_flag / clear_flag` |
| I | **仍存在** | 引擎无先攻 / 轮次 / 行动经济（`initiative\|先攻` 仅 3 处：一个 statblock 字段 + 两句注释） |
| J | **仍存在** | `CondExpr` 变体里无「掷 vs 对手被动」入口 |
| K | **仍存在** | `build_state` 建实例时 `statuses: vec![]`（api/src/lib.rs:520 / 591） |
| L | **已闭合** | `host.target` 同级完整；伏击读目标状态。**第四轮登记的「不看签名」已由 T29 修掉**（§R5-4） |
| M | **仍存在** | `CondExpr` 只有 `EncounterCleared` |
| N | **已闭合** | 24 个掷表触发点 `repeatable: true` + `dnd-wander-reset` |

**汇总：闭合 7 / 近似提高 1 / 仍存在 6** —— 与规则包 `--check`、与第四轮**数目一致**。
第五轮**没有新增闭合**（A 的残余仍由 B 造成），也没有把任何一条改判回去。

---

### R5-8. 本轮新发现（不替任何人圆场）

#### ① `rule-dnd-proficiency` 的挂载点声明与实现不一致（**低危，新发现**）

`story_example/lmop-storybook.json` 里：

| 位置 | 声明 | 真实 |
|---|---|---|
| `definitions[rule-dnd-proficiency].fields.mount` | `check_post_roll` | `lua_mounts[dnd-proficiency].mount = check_pre_roll` |
| `definition.description` | 「lua_mounts:dnd-proficiency（挂载点 check_post_roll）」 | 同上 |
| `definitions[prof-pc-lmop-talin-dex/wis].fields.note` | 「在 check_post_roll 追加（引擎只在掷骰后给判定属性）」 | 实际在 `check_pre_roll` 用签名（GAP-D 闭合后已改） |

生成器源头：`scripts/lmop-rulepack.mjs` 的 `ruleMeta['dnd-proficiency'] = ['check_post_roll', …]`
（第 1344 行），而 `buildRules` 把同一条脚本注册在 `check_pre_roll`（第 1241 行）。
这是 GAP-D 闭合（熟练加值从掷骰后改到掷骰前）时的**文档没跟着改**。**低危**（不影响行为），
但它正是这套设计的主要风险形态——「实现落后于声明」——的一个新样本；而且
`--check` 的 `references.self_consistent / open_content.data_driven` **不校验**
`rule-*` 定义的 `fields.mount` 与真实挂载点是否一致（本轮用独立脚本逐条比对 16 条，只有这 1 条不一致）。
**只报告，不修**（inScope 不含产品代码 / 生成器）。

#### ② `--check` 的源码子串仍可被注释满足（T27 发现③，**未消除**）

实证见 §R5-6(c)。T29-B 的加固方向（标注 behavioural + 悬空检查）**是对的**，但子串断言本身仍是弱形态。
建议（不实施）：把「挂载点 / 判定范围 / 开放内容字段」这类可结构化的一致性，从子串匹配升级为
「跑引擎 → 观察行为」的断言，或至少让 `--check` 的基础模式也调用引擎校验器的行为断言。

#### ③ 丢弃 WARN 的去重是进程级、跨存档（设计取舍，如实登记）

`warn_dropped_character_delta` 的 `static SEEN` 不区分存档 / 会话。后果：同一 `(domain, entity_id, field)`
形态在**任何**存档里第一次出现后，其它存档的同类丢弃都不再提示；到 256 上限后新形态每次都告警。
「重放不刷屏」的目标达成，但「复发可见性」被削弱。本轮不改产品代码，只登记。

---

### R5-9. 环境与复现命令 / 退出码（第五轮）

`
cd /home/huang/Personal/Dev/Code/octopus
export CARGO_TARGET_DIR=$PWD/.scratch/engine-test-target

# 0. 旧实况与 HEAD 的逐字节核对（外部）
git show HEAD:story_example/lmop-storybook.json   # → dnd-ambusher-keep-high.source sha256 77a72f91…（974 B）
#   与 scripts/lmop-engine-check/src/main.rs 的 OLD_AMBUSHER 常量逐字节相同

# 1. 第五轮新增的独立用例（9 条）
cargo test -p octopus-api --test lmop_verification_round5 -- --test-threads=2 --nocapture

# 2. 全套回归
cargo test -p octopus-engine -p octopus-ai
cargo test -p octopus-api

# 3. 交付物 / 规则包 / 引擎校验
node scripts/lmop-rulepack.mjs --check
node scripts/lmop-rulepack.mjs --check --engine .scratch/lmop-engine-check-target/debug/lmop-engine-check
node scripts/lmop-rulepack.mjs --out /tmp/r5-regen-storybook.json   # sha256 == 交付物
node scripts/import-bestiary.mjs --check
./scripts/lmop-engine-check.sh
./scripts/lmop-boundary-check.sh

# 4. 本轮新增：行为级兜底的变异审计（5 个变异）
node scripts/lmop-r5-mutation-audit.mjs
`

| 命令 | 退出码 | 关键结果 |
|---|---|---|
| `cargo test -p octopus-api --test lmop_verification_round5` | 0 | **9 passed / 0 failed** |
| `cargo test -p octopus-engine -p octopus-ai` | 0 | engine **361** / ai **36**，0 失败 |
| `cargo test -p octopus-api` | 0 | lib 74 / r1 13 / r2 14 / r3 14 / r4 9 / **r5 9**，全 0 失败 |
| `node scripts/lmop-rulepack.mjs --check` | 0 | 28 项断言 0 失败；14 GAP = 7 / 1 / 6 |
| `node scripts/lmop-rulepack.mjs --check --engine …` | 0 | 50 项断言 0 失败；`consistency.behavioural_backing` 20/20 不悬空 |
| `node scripts/lmop-rulepack.mjs --out /tmp/…` | 0 | sha256 `ce6b557f…` == 交付物 |
| `node scripts/import-bestiary.mjs --check` | 0 | 28/28 |
| `./scripts/lmop-engine-check.sh` | 0 | error 0 / lua 0 issue / warning 24 / **20 条运行时断言 / 0 failures** |
| `./scripts/lmop-boundary-check.sh` | 0 | 禁词零命中 |
| `node scripts/lmop-r5-mutation-audit.mjs` | 0 | **5/5 变异都被声明的行为级断言抓住** |

**验证基线（本轮实测时的文件指纹，便于复现）**：

`
story_example/lmop-storybook.json                     sha256 ce6b557f9d95c816c852a0ee08e6e4cb4b4663d21bc4ab8cd5882a12784a4f01
scripts/lmop-rulepack.mjs                             sha256 0d5e5b3d54a6e373bbe5674e8fc6563647f7eed8bf78f73b7502497eca312cba
scripts/lmop-engine-check/src/main.rs                 sha256 3acd9bbf5d30eb486dc2ee6b19c9783f44ba78979e321df3c38b33b4a54bedba
crates/octopus-api/tests/lmop_verification_round3.rs  sha256 7b439730b618b4631444cf04cf89243790235b35a853145582085f0614f27f67
crates/octopus-api/tests/lmop_verification_round4.rs  sha256 475fa426c1475673709d294da77a34cccf7c10defaef864d506c93ba1cb83a8a
crates/octopus-api/tests/lmop_verification_round5.rs  sha256 0fd0dd6cb41361944260f0fa097d563fb080143a40ba8142a1f53083e808b926
git HEAD 的 dnd-ambusher-keep-high.source             sha256 77a72f91922d78d0ce94eeb96abdf5bbb379abe426755d73720d77e67cdd6755（974 B，= OLD_AMBUSHER）
`

---

### R5-10. 未通过 / 未覆盖 / 副作用（不得隐瞒）

**未通过：无（0 项）。**

必须声明的**覆盖边界**：

1. **本轮没有跑真实 HTTP 路径**（没有起服务、没有碰真实 `octopus.db`）。第五轮的回归清单
   r1 / r2 / r3 / r4 / r5 / api-lib / engine / ai / rulepack / engine-check / boundary（不含 HTTP）；
   与第四轮同口径。发布门的覆盖没有丢：r3 / r4 / r5 的每条会话用例都走真实 `publish_and_open`
   （内存库 + 真实 validate / lint），交付物在真实发布门上 0 error。
2. **GAP-F 的非 strike 击杀路径**仍只有代码复核（与第二、三、四轮同）。
3. **`octopus.db` 操作数为 0**：所有会话用例用 `SqliteStore::open_in_memory()`；
   `--out` 只写 `/tmp`；变异只写系统临时目录。既有用户故事书 / 存档 / octopus 账户
   **未读改、未删除**（`git status` 里没有 `octopus.db`）。

**副作用 / 新增数据**：

- **真实 `octopus.db`：本轮新增 0 条验证数据**（无新账户 / 故事书 / 存档），无需清理。
- 内存库里的临时实体键（仅存在于测试进程，未落库）：`verify-r5-ghost-<uuid>`、
  `verify-r5-replay-a-<uuid>` / `verify-r5-replay-b-<uuid>`；HTTP harness 的内存 saves 为随机 id。

---

### R5-11. 这套设计成立性的第五轮判断

**第五轮推翻的第四轮遗留**

| 第四轮登记 | 第五轮 |
|---|---|
| 「伏击仍不看判定签名」（缺陷候选） | **推翻**：T29 按 `kind == attack` 收敛；旧实况仍以 `OLD_AMBUSHER` 可执行存活；本轮变异证明闸门有牙齿 |
| 「规则包按判定签名收敛没有成为通用约定」 | **部分推翻**：数据卡明说「攻击检定」的两条（pack-tactics / ambusher）现在都收敛了；剩下 4 条不读签名的挂载点确有「对任意判定成立」的口径 |

**第五轮仍然成立 / 新登记的缺口**

- **GAP-B**（无位置 / 距离）仍是 GAP-A 只能「近似提高」的唯一原因。
- **GAP-G / I / J / K / M** 五条与第一轮登记时实质相同。
- **新登记**：`rule-dnd-proficiency` 的挂载点声明与实现不一致（§R5-8①）；
  `--check` 的子串断言仍是「注释即可满足」的形态（§R5-6c）；
  丢弃 WARN 的去重是进程级、跨存档（§R5-8③）。

**第五轮对设计成立性的判断**

前四轮的判断是「路对、原语不够（R1）→ 边界变宽（R2）→ 读事实的缺口补齐（R3）→ 声明即契约（R4）」。
第五轮在 T28/T29 之后可以补两句：

1. **这套设计第一次做到了「旧实况可执行存活」的验证纪律。** T29 修改了第四轮验证者的用例，
   同时把旧脚本原文**逐字节**搬进引擎校验器并用断言复现旧结论——这正好满足 T27 定的安全前提。
   这不是天然成立的，是这一轮**专门补**的。
2. **「实现落后于声明」仍是主要风险形态，而且它对验证资产本身也适用。** 本轮实证了两条：
   `rule-dnd-proficiency` 的 @"mount" 声明漂移（没有工具能发现，靠人逐条比对）；
   `--check` 的源码子串能被注释满足（靠行为级兜底救场）。
   换句话说：**声明与实现之间没有强制约束，声明与验证断言之间也没有。**
   真正把它钉住的仍然是「真实引擎 + 行为断言 + 变异验证」这一套。

**下一轮若要继续加固，优先级建议（本轮不实施）**：
① 把判定类挂载点的「判定范围」变成**强制可结构化的 `signature_scope`**（连不读签名的那 4 条也写清楚，
   并让 `--check` 校验它）；② 让 `--check` 的基础模式也跑引擎行为断言（消除「子串通过、行为已死」的窗口）；
③ 给丢弃 WARN 加存档维度（或分层：首见 INFO / 复发 WARN），恢复「复发可见性」。

---

### R5-12. 本轮新增 / 改动文件

| 文件 | 作用 |
|---|---|
| `docs/lmop-verification-report.md` | 本报告（§0–§10 / §R2 / §R3 / §R4 **原样保留**，在 §R4-6 / §R4-10 加「⚠️ 第五轮订正」标注，追加 §R5） |
| `crates/octopus-api/tests/lmop_verification_round5.rs` | **新增**：9 条独立用例（旧实况复现 + harness 忠实性结构断言 + 3 个变异 + 伏击签名/fail-closed + force/scale 正交 3 组对照 + 丢弃可见化实时/重放 + 16 挂载点签名扫描） |
| `scripts/lmop-r5-mutation-audit.mjs` | **新增**：行为级兜底的变异审计（5 个变异，真实引擎校验器逐条判定） |

**未改动**：`crates/*/src/` 与 `frontend/src/` 的任何产品代码；
**未改动**任何既有测试文件（`lmop_verification.rs` / `_round2` / `_round3` / `_round4`，只读 + 跑 + 变异推理）；
**未改动** `story_example/lmop-storybook.json`（只读；所有变异都写在系统临时目录）。
