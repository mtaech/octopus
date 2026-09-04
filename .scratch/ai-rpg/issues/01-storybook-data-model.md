# 故事书数据模型

Type: grilling
Status: resolved

## Question

定义故事书的完整数据模型，作为引擎、AI 和编辑器的单一事实来源。需要拍板：
① 顶层结构：章节/场景/世界设定如何组织（结构化骨架的静态侧）。
② 人物：背景/性格/属性维度的字段，「属性维度」自定义的精确 schema（键 + 数值/标签？数值范围？）。
③ 技能/法术：声明式定义（名称/消耗/冷却/目标/判定）+ Lua 钩子的精确形态。
④ 物品与物品技能：物品字段，物品技能与角色技能的关系（同一个技能实体被物品引用，还是独立类型）。
⑤ 势力与关系：关系是数值好感度还是图结构；势力如何影响 AI 与剧情。
⑥ 静态设定 vs 运行时状态的分界。
⑦ 序列化/持久化格式与版本迁移策略。

## Answer

故事书 = 静态模板的单一事实来源，JSON 序列化（带 `schema_version` + 迁移路径）。

```
故事书 Storybook
├── meta            标题 / 版本 / 作者 / 语言
├── world           世界设定
│   ├── premise       背景叙事、基调、通用规则
│   ├── locations     地点/区域
│   └── resources     资源/货币定义
├── skeleton        结构化骨架：chapters → scenes → goals + beats（章节→场景→目标+关键事件）
├── characters      人物（模板）：{ id, name, background, personality, attributes }
├── skills          技能/法术：{ id, name, description, category, cost, cooldown, target, check, effect, lua? }
├── items           物品：{ id, name, description, type, quantity, properties, skills:[skill_ref] }
├── factions        势力：{ id, name, description, goals, default_attitude }
└── relationships   关系：有向边 { from, to, type, value }
```

- 属性维度：故事书全局定义（键 + 标签 + 类型），类型 = 数值（0–100 可设 min/max）/ 枚举（标签）/ 文本；判定只用数值型。
- 技能是单一 Skill 实体；「物品技能」= 物品引用同一 Skill（载体是物品）。
- 关系 = 有向边，可连 人物/势力 两两之间；type（好感/敌意/同盟/亲属/…）+ value（数值强度）。
- 静态 vs 运行时：故事书只存模板；角色实例 = 人物模板 + 当前属性/位置/状态；物品栏 = 物品模板 + 数量/实例态；关系值可偏离初始。
