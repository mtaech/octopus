// ============================================================
// Lua 脚本预设模板与代码片段库（#02 / #12 / #23）
// 供故事书编辑器中的 Lua 钩子、判定器、条件表达式快速套用与参考。
// ============================================================

export interface LuaTemplate {
  id: string
  title: string
  category: 'hook' | 'check' | 'condition'
  mount?: 'pre_resolve' | 'post_resolve' | 'check_pre_roll' | 'check_post_roll' | 'event'
  description: string
  tags: string[]
  code: string
}

export const LUA_TEMPLATES: LuaTemplate[] = [
  // ---------- 技能钩子类 ----------
  {
    id: 'skill_cost_and_status',
    title: '资源消耗与概率状态施加',
    category: 'hook',
    mount: 'pre_resolve',
    description: '安全检查角色资源，资源足够扣减体力/魔力，并使用确定性 RNG 概率给目标施加持续状态。',
    tags: ['资源消耗', '施加状态', 'RNG'],
    code: `-- 技能钩子：检查并扣除消耗，概率对目标施加状态
local cost_amount = 5
local stamina = host.get_resource('res-stamina') or 0

if stamina < cost_amount then
    -- 资源不足时可拦截或转化为惩罚代价
    host.log('体力不足，改为过载扣除生命')
    host.request_cost('res-hp', 8)
else
    host.request_cost('res-stamina', cost_amount)
end

-- 75% 概率对目标施加持续 3 回合的异常状态
local roll = host.engine_rng(1, 100)
if roll <= 75 and host.target then
    host.apply_status(host.target.id, 'burn', 3, 'turns')
    host.log('成功施加灼烧状态，判定骰: ' .. roll)
end
`,
  },
  {
    id: 'skill_combo_storage',
    title: '跨回合连击与爆发蓄能（host.storage）',
    category: 'hook',
    mount: 'post_resolve',
    description: '利用沙箱提供的独立跨回合持久化存储 host.storage，记录技能施放次数，满层触发爆发。',
    tags: ['跨回合存储', '连击机制', '事件触发'],
    code: `-- 技能钩子：跨回合连击蓄能机制（利用 host.storage 独立持久化）
host.storage.combo = (host.storage.combo or 0) + 1
host.log('当前技能连续累积层数: ' .. host.storage.combo)

if host.storage.combo >= 3 then
    -- 达到 3 层连击：触发暴击广播，赋予强化状态并清空计数
    host.trigger_event('combo_burst')
    host.apply_status(host.actor.id, 'empowered', 2, 'turns')
    host.storage.combo = 0
    host.log('★ 连击爆发！获得强化状态，重置连击计数')
else
    -- 蓄力期标准体力消耗
    host.request_cost('res-stamina', 2)
end
`,
  },
  {
    id: 'skill_target_counter',
    title: '目标属性交互与防御穿透',
    category: 'hook',
    mount: 'pre_resolve',
    description: '读取施法者属性与目标防御/敏捷属性进行对抗计算，并根据优劣发起动态写请求。',
    tags: ['目标对抗', '属性交互', '破甲'],
    code: `-- 技能钩子：施法者力量 vs 目标体质对抗
if host.target then
    local my_str = host.get_attribute('str') or 10
    -- 查询世界中目标实体的体质属性（若无则默认为 10）
    local target_con = 10
    
    if my_str >= target_con + 10 then
        -- 完全压制：施加破防虚弱状态
        host.apply_status(host.target.id, 'vulnerable', 2, 'turns')
        host.trigger_event('shield_break')
        host.log('力量压制成功，施加易伤虚弱状态')
    else
        -- 势均力敌：额外消耗体力抵消反冲
        host.request_cost('res-stamina', 3)
    end
end
`,
  },
  {
    id: 'skill_narrative_event',
    title: '血线濒死广播与特定场景触发',
    category: 'hook',
    mount: 'post_resolve',
    description: '根据角色当前生命值或所在场景，触发剧情事件（供引擎与主线 AI 叙事响应）。',
    tags: ['叙事事件', '场景感应', '血线机制'],
    code: `-- 技能钩子：检测生命值与场景，触发关键叙事分支
local hp = host.get_resource('res-hp') or 100

-- 当生命值低于 25% 极限触发背水一战
if hp <= 25 then
    host.apply_status(host.actor.id, 'desperation', 2, 'turns')
    host.trigger_event('character_critical')
    host.log('触发背水一战状态！')
end

-- 若在特定场景施展，触发场景隐藏机关
if host.scene_id == 'loc-mine' then
    host.trigger_event('mine_resonated')
    host.log('在废矿坑中施展引发共鸣')
end
`,
  },

  // ---------- 判定器类 ----------
  {
    id: 'check_d20_attribute',
    title: '标准 D20 + 属性修正检定（D&D 风格）',
    category: 'check',
    description: '经典的 1d20 掷骰，加上基于属性偏离中心值的修正值，归一化输出 total 与 margin。',
    tags: ['D20', '属性检定', '难度比对'],
    code: `-- 自定义判定器：必须归一化返回 { total = 最终值, margin = 难度差值 }
local d20 = host.engine_rng(1, 20)
local attr = host.get_attribute('wit') or 10

-- 以 10 为基准线，每高 2 点获得 +1 修正
local modifier = math.floor((attr - 10) / 2)
local total = d20 + modifier
local diff = host.difficulty or 15
local margin = total - diff

host.log(string.format('D20结果=%d, 修正值=%+d, 最终总值=%d, 目标难度=%d, 差值=%+d', d20, modifier, total, diff, margin))

return {
    total = total,
    margin = margin
}
`,
  },
  {
    id: 'check_3d6_bell_curve',
    title: '3D6 钟形曲线检定（GURPS 风格向下判定）',
    category: 'check',
    description: '掷 3 颗 6 面骰，计算趋于正态分布的平稳数值，以小于等于技能值为成功。',
    tags: ['3D6', '正态分布', '技能值比对'],
    code: `-- 自定义判定器：3d6 钟形正态分布检定
local r1 = host.engine_rng(1, 6)
local r2 = host.engine_rng(1, 6)
local r3 = host.engine_rng(1, 6)
local total = r1 + r2 + r3

-- 技能或属性目标值（例如敏捷 12）
local target_val = host.get_attribute('agi') or 12
-- roll <= target_val 为成功，差值 margin = target - total
local margin = target_val - total

host.log(string.format('3D6骰值=%d( %d+%d+%d ), 目标值=%d, 差值=%+d', total, r1, r2, r3, target_val, margin))

return {
    total = total,
    margin = margin
}
`,
  },
  {
    id: 'check_d100_percentile',
    title: 'D100 百分比技能值判定（CoC / BRP 风格）',
    category: 'check',
    description: '掷 1d100，判断是否在角色掌握的百分比技能值以内，差值大于零为成功。',
    tags: ['D100', '百分比', '克苏鲁风格'],
    code: `-- 自定义判定器：D100 百分比检定
local d100 = host.engine_rng(1, 100)
local skill_rate = host.get_attribute('wit') or 50
local margin = skill_rate - d100

-- 大成功 (≤5) 或 大失败 (≥96)
if d100 <= 5 then
    margin = margin + 50
    host.log('★ D100 大成功！')
elseif d100 >= 96 then
    margin = margin - 50
    host.log('⚠ D100 大失败！')
end

return {
    total = d100,
    margin = margin
}
`,
  },

  // ---------- 条件表达式类 ----------
  {
    id: 'condition_multi_resource_attr',
    title: '复合属性与资源门槛检测',
    category: 'condition',
    description: '用于目标达成或场景触发条件，当角色智力、金币同时满足或带有特定状态时返回 true。',
    tags: ['条件判断', '复合门槛', '布尔返回'],
    code: `-- 条件表达式：必须返回 boolean (true=成立, false=不成立)
local wit = host.get_attribute('wit') or 0
local gold = host.get_resource('res-gold') or 0
local has_insight = host.has_status('scholar_insight')

-- 满足条件：智力 ≥ 50 且持有金币 ≥ 20；或拥有特殊洞察状态
if (wit >= 50 and gold >= 20) or has_insight then
    return true
end

return false
`,
  },
  {
    id: 'condition_relationship_bond',
    title: 'NPC 关系好感度羁绊检测',
    category: 'condition',
    description: '用于检查当前角色与场景关键人物之间的好感度或阵营信任度是否达到剧情分水岭。',
    tags: ['好感度', '关系网络', '剧情分水岭'],
    code: `-- 条件表达式：检查角色与酒馆老板娘伊莎的好感度羁绊
local favor = host.relationship(host.actor.id, 'char-isa', '好感') or 0

-- 好感度达到 30 且未处于敌对状态
local hostility = host.relationship(host.actor.id, 'char-isa', '敌意') or 0
if favor >= 30 and hostility == 0 then
    return true
end

return false
`,
  },
]
