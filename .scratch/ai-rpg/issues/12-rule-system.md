# 规则系统设计（判定 / 资源 / 效果机制）

Type: grilling
Status: resolved

> **修订（产品定位通用化）**：判定机制去 DnD 私有语义——d20 从「唯一随机预设」降级为常用参数组合之一；成功度改按差值分档，删除 nat 20 / nat 1 特例；schema 字段 `world.rules.resolution` 更名 `world.rules.check`（「结算 (Resolution)」一词已被命令结算与演出流事件占用）。

## Question

定义核心规则机制，为引擎结算与 Lua 脚本提供规则语义。需要拍板：
① 判定机制：用什么骰子系统（d20 / d100 / 骰池 / 无骰），判定难度与属性如何结合（属性值 vs 阈值 vs 对抗）。
② 资源经济：world.resources 如何产生 / 消耗 / 上限（技能 cost 引用的语义）。
③ 效果机制：skill.effect 如何声明与叠加（加成 / 状态 / 触发 / 持续）。
④ 随机性与确定性：随机数种子与可回放性的关系。
（与「引擎职责边界」的分工：本票定「规则是什么」，那票定「规则逻辑放在 Rust 还是 Lua」。）

## Answer

### 架构前提

引擎提供**判定器**，判定方式全部由故事书声明，引擎不内置任何特定骰系。判定器有两种来源：**声明式配置**（骰子表达式、比较模式、属性修正映射、成功度阈值皆为参数，d20、d100、骰池只是常用参数组合；声明无骰时由 AI 依属性值叙事裁决），或 **Lua 判定脚本**（自定义逻辑，须归一化输出最终值与差值，成功度仍由引擎分档）。判定器双作用域：`world.rules.check` 全局声明，单个技能可用 `skill.check` 引用或覆盖。

### ① 判定机制

- **判定器参数**（故事书声明，引擎不预设骰系）：
  - `dice`：骰子表达式（`"1d20"` / `"1d100"` / `"3d6"` / 骰池 `"5d6"` 等）或 `null`——无随机，AI 依属性值叙事裁决。
  - `mode`：比较模式——`gte`（最终值 ≥ 难度）/ `lte`（最终值 ≤ 目标值，百分比技能手感）/ `opposed`（对抗，双方各掷比差值）。
  - `attribute_modifier`：属性→修正映射，默认中心偏移公式 `(属性值 − 50) / 5`（向下取整），50 为基线 +0，范围 −10~+10；故事书可覆盖。
  - `degree_thresholds`：成功度分档阈值列表，按「最终值 − 目标值」差值落档；档位数由列表导出，默认 `[+10, 0, −10]` 划四档（大成功 / 成功 / 勉强 / 失败，对齐 #17 的 `level` 枚举）。
- **成功度**：只按差值分档，与骰面本身无关（无 nat 20 / nat 1 之类特例）；同时返回原始差值供 Lua/AI 自行解读。档位显示名（如「圆满 / 勉强」）归演出模板映射，不入 schema。
- **故事书配置点**：`world.rules.check` 下声明 `dice` / `mode` / `attribute_modifier` / `degree_thresholds`；`skill.check` 可引用全局判定器或声明技能自己的判定器（配置或 Lua 脚本）。
- **自定义判定（Lua）的归一化契约**：判定脚本只输出最终值 `total` 与差值 `margin`，引擎按 `degree_thresholds` 分档成 `level`（对齐 #17 枚举），不能直接返回自定义档位；差值同时供叙事与 Lua/AI 自行解读。RNG 必须走引擎 `engine_rng` 确定性序列，命令日志记录消耗。

### ② 资源经济

- 资源定义在 `world.resources`，类型 = `numerical`（数值）或 `binary`（有无）。
- 每条资源可声明 `default_max` 和可选 `natural_recovery`（amount + trigger: per_turn | per_scene | per_rest）。
- 技能 cost 直接引用资源 id：`cost: [{ resource: "mana", amount: 10 }]`。
- 引擎结算阶段校验余额→扣减→写回。

### ③ 效果机制

- **分层声明**：声明式核心（immediate / status / triggers / modifiers）+ Lua 钩子兜底。
- **immediate**：即时效果（damage/heal/modify_resource/set_flag），支持骰子表达式如 `"2d6+3"`。
- **status**：持续效果，duration 按 turns 或 scenes，stack 策略 = replace | add | max。
- **triggers**：条件触发（event + condition + effects），事件名由引擎定义。
- **modifiers**：静态属性修正，同名取最高值（max），不同名独立生效。
- **叠加规则**：同名 modifier 取 max；同名 status 按 stack 字段；不同名效果全部独立。

### ④ 随机性与确定性

- 存档创建时生成全局种子，存入存档 meta。
- 每次判定与效果中的骰子表达式求值消耗 RNG 值，命令日志记录 `rng_consume` 条目（含 count + values 数组）。
- 重放时直接使用命令日志中已记录的随机值，跳过 RNG 调用，保证确定可复现。
- v1 持续时间单位：turns（该角色回合开始时 tick）和 scenes（场景切换时到期）。
