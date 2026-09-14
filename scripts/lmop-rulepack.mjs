#!/usr/bin/env node
// scripts/lmop-rulepack.mjs — LMoP 的 D&D 规则包（docs/rules-via-lua.md §7 / §9 L5）
//
// 做什么：把 LMoP 需要的 D&D 规则**全部**用 storybook.lua_mounts 脚本 + 开放内容表达，
// 与 scripts/import-bestiary.mjs 的图鉴 / 地点 / 地图 / 骨架**合并成一本可直接导入的故事书**。
//
// 硬边界（不可越过）：
//   1. 只用引擎已落地的通用原语：host.modify_check / scale_effect / apply_effect / modify_resource / engine_rng
//      + 只读 host.get_attribute / has_status / check.* / status.* / definition / actor / target
//      + T21 世界事实 host.get_character / get_flag / list_flags / get_encounter / list_encounters
//      + T22 PostResolve 只读 host.resolved_effects。
//   2. 不新增任何封闭字段，不改引擎。表达不了的规则**原样报告为缺口**（见 --check 的 gap 列表
//      与 story_example/lmop-import.md），绝不用「看起来齐全」的假规则糊过去。
//   3. 图鉴数据**不复制**：只引用 scripts/import-bestiary.mjs 产出的 id（模板 id / XP 值从草稿读）。
//   4. 确定性：同输入同输出，无时间戳、无随机、无环境依赖。
//
// 用法：
//   node scripts/lmop-rulepack.mjs                  # 生成 story_example/lmop-storybook.json
//   node scripts/lmop-rulepack.mjs --check          # 自断言（不写文件）
//   node scripts/lmop-rulepack.mjs --check --engine <bin>   # 额外跑真实引擎发布校验
//   node scripts/lmop-rulepack.mjs --out <file>
//
// 输入：story_example/lmop-storybook.draft.json（缺失时自动调用 import-bestiary.mjs 重建）

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '..')
const F = {
  draft: path.join(ROOT, 'story_example/lmop-storybook.draft.json'),
  out: path.join(ROOT, 'story_example/lmop-storybook.json'),
  importer: path.join(ROOT, 'scripts/import-bestiary.mjs')
}

// ---------------------------------------------------------------------------
// 0. 撤回的封闭字段名（deep-scan 断言用）
// ---------------------------------------------------------------------------
// docs/rules-via-lua.md §5 的「上一轮写死、本轮全部撤回」清单。整本故事书里
// **任何位置**（键、文本、Lua 源码）都不允许出现这些名字——出现即说明规则集语义漏回了数据模型。
const RETRACTED_TOKENS = [
  'advantage',
  'disadvantage',
  'on_save',
  'ConditionalModifier',
  'save_ends',
  'crit',
  'EncounterTable',
  'extends'
]

// 引擎挂载点白名单（lua_host.rs LuaMount::DECLARABLE）。
// 注意：protocol 只能在 narrative.protocol 走，不在故事书挂载点表里。
const DECLARABLE_MOUNTS = [
  'check_pre_roll',
  'check_post_roll',
  'check',
  'pre_resolve',
  'post_resolve',
  'event',
  'condition',
  'status_tick',
  'turn_end',
  'scene_end'
]

// host.* API（lua_host.rs make_env 注册的全部读写口）。脚本里出现白名单外的 host.* 即断言失败。
//
// T18 新增（本轮规则包开始实际使用）：
//   · 只读开放内容：get_attachments / get_attachment / get_definition / list_definitions（闭合 GAP-C）
//   · 清标记：set_flag / clear_flag（闭合 GAP-H；clear_flag 是 set_flag(flag, false) 的具名写法）
//   · 事件挂载点事实：event_name / event_data（闭合 GAP-F：enemy_defeated / encounter_cleared）
//   · check_pre_roll 签名：check_kind / check_attribute / check_target 掷骰前即有值（闭合 GAP-D）
//
// T21 / T22 新增（本轮规则包开始实际使用）：
//   · 只读世界事实：get_character / get_flag / list_flags / get_encounter / list_encounters
//     （闭合 GAP-A；host.target 同时升级为与 host.actor 同级完整，含 statuses —— 闭合 GAP-L）
//   · 效果缩放 scale_effect（只在 check_post_roll / pre_resolve 存在）+ PostResolve 只读快照
//     resolved_effects（闭合 GAP-E）
const HOST_API = [
  'script_id', 'mount', 'event', 'scene_id', 'round', 'difficulty', 'controlled', 'present',
  'storage', 'actor', 'target', 'definition',
  'event_name', 'event_data',
  'status', 'status_id', 'status_name', 'status_turns_left', 'status_scenes_left',
  'status_unit', 'status_remaining',
  'check', 'check_total', 'check_target', 'check_margin', 'check_result', 'check_attribute',
  'check_expr', 'check_level', 'check_kind',
  'get_attribute', 'get_resource', 'has_status', 'relationship',
  'get_attachments', 'get_attachment', 'get_definition', 'list_definitions',
  'get_character', 'get_flag', 'list_flags', 'get_encounter', 'list_encounters',
  'resolved_effects',
  'engine_rng', 'request_cost', 'apply_status', 'remove_status', 'trigger_event', 'query_world',
  'modify_check', 'scale_effect', 'apply_effect', 'modify_resource', 'set_flag', 'clear_flag', 'log'
]

// Lua 预检禁用 API（lua_lint.rs FORBIDDEN 同口径）。
const FORBIDDEN_LUA = [
  'io.', 'os.', 'package.', 'debug.', 'require', 'dofile', 'loadfile',
  'math.random', 'collectgarbage', 'coroutine.'
]

// ---------------------------------------------------------------------------
// 1. 规则包词汇（单一事实来源：同一个 JS 结构同时生成「Lua 脚本」与「开放内容」）
// ---------------------------------------------------------------------------
// ⚠️ 缺口 C（见 --check 输出）：引擎**不向 Lua 暴露挂接定义 / 开放内容**
// （docs §7 用的 host.attachment_bonus 在引擎里不存在）。所以「哪个模板有什么特性」
// 必须在生成期从开放内容**烘进 Lua 表**。这里就是那个烘制点：改词汇只改这一处。

const RULEPACK = {
  /** 优势来源状态（规则包自己的词汇；引擎只认识 keep_high）。 */
  advantage_statuses: { 'dnd-inspired': '激励' },
  /** 劣势来源状态。 */
  disadvantage_statuses: { 'dnd-eruption-penalty': '灰烬呛咳（灰烬喷发失败的后续）' },
  /** 日照敏感：阳光下攻击检定劣势（缚灵·墨梅斯克，附录 B 原文）。 */
  sunlight_sensitive: { 'mon-mormesk-the-wraith': '缚灵·墨梅斯克' },
  /** 集群战术：狼（需要「盟友在目标 5 尺内」这个事实——引擎没有位置概念，见缺口 P）。 */
  pack_tactics: { 'mon-wolf': '狼' },
  /** 伏击：变形怪在战斗第一轮对「被突袭者」的攻击检定有优势（附录 B 原文）。 */
  ambusher: { 'mon-doppelganger': '变形怪' },
  /** 熟练加值：只在 kind=attribute 的检定上生效（避免误加攻击骰 / 豁免骰）。 */
  proficiency: {
    'pc-lmop-talin': { dex: { bonus: 5, label: '隐匿（敏捷）' }, wis: { bonus: 3, label: '察觉（睿智）' } }
  },
  /** 重复豁免（save-ends）：回合末再掷一次豁免，成功即提前终止。 */
  save_ends: {
    'dnd-eruption-penalty': { dc: 10, attribute: 'con' },
    'dnd-poisoned': { dc: 11, attribute: 'con' }
  }
}

// 野外遭遇表（附录 B 前文「野外遭遇」两行表，第 1225 / 1235 行原样录入）。
// ⚠️ 缺口 Q：表里的数量是骰式（1d8+2 只蚊蝠），而 EncounterPresetEnemy.count 是**静态整数**，
// 规则包只能把每行定成一个固定数量（取骰式下界），并在 note 里写明原始骰式。
const WANDER_TABLES = [
  {
    key: 'day',
    label: '野外遭遇（白昼）',
    flagPrefix: 'dnd-wander-day',
    sceneId: 'sc-lmop-0b7',
    locationId: 'loc-lmop-0b7',
    night: false,
    rows: [
      { die: [1, 2], name: '蚊蝠', dice: '1d8+2' },
      { die: [3, 4], name: '食人魔', dice: '1' },
      { die: [5, 6], name: '地精', dice: '1d6+3' },
      { die: [7, 8], name: '大地精', dice: '1d4+2' },
      { die: [9, 10], name: '兽人', dice: '1d4+2' },
      { die: [11, 11], name: '狼', dice: '1d4+2' },
      { die: [12, 12], name: '枭熊', dice: '1' }
    ]
  },
  {
    key: 'night',
    label: '野外遭遇（黑夜）',
    flagPrefix: 'dnd-wander-night',
    sceneId: 'sc-lmop-0b7',
    locationId: 'loc-lmop-0b7',
    night: true,
    rows: [
      { die: [1, 3], name: '蚊蝠', dice: '1d8+2' },
      { die: [4, 4], name: '食尸鬼', dice: '1d4+1' },
      { die: [5, 5], name: '地精', dice: '1d6+3' },
      { die: [6, 6], name: '大地精', dice: '1d4+2' },
      { die: [7, 8], name: '兽人', dice: '1d4+2' },
      { die: [9, 10], name: '狼', dice: '1d4+2' },
      { die: [11, 12], name: '枭熊', dice: '1' }
    ]
  }
]

// 规则包新增的常驻标记（作者 / 导演 / AI 置位；规则脚本用 when 闸门读它们）。
// T21 之后 Lua 也能读标记（host.get_flag / host.list_flags），本包暂未改用——口径不变。
const RULE_FLAGS = [
  { key: 'dnd-in-wilderness', label: '野外行进（开启遭遇检定）' },
  { key: 'dnd-night', label: '夜间' },
  { key: 'dnd-sunlight', label: '阳光直射' },
  { key: 'dnd-in-combat', label: '战斗中（战斗轮计数开闸）' },
  { key: 'dnd-surprised', label: '本场战斗出现了被突袭者' },
  { key: 'dnd-battle-round-1', label: '战斗第 1 轮' },
  { key: 'dnd-battle-round-2', label: '战斗第 2 轮' },
  { key: 'dnd-battle-round-3', label: '战斗第 3 轮' }
]

// 规则包新增的状态（激励 / 灰烬呛咳 / 受突袭 / 倒地 / 中毒）。
const RULE_STATUSES = [
  {
    id: 'dnd-inspired',
    name: '激励',
    description: '规则包：一次属性检定 / 攻击检定 / 豁免检定获得优势（掷两次取高），用掉即消失。',
    duration: 5,
    unit: 'turns'
  },
  {
    id: 'dnd-eruption-penalty',
    name: '灰烬呛咳',
    description: '规则包：灰烬喷发（DC 10 体质豁免）失败后，攻击 / 豁免 / 属性检定获得劣势；每回合末可重复豁免。',
    duration: 10,
    unit: 'turns'
  },
  {
    id: 'dnd-surprised',
    name: '受突袭',
    description: '规则包：战斗第一轮失去其回合（引擎没有先攻与行动经济，「失去回合」由叙事层落实）。',
    duration: 1,
    unit: 'turns'
  },
  {
    id: 'dnd-prone',
    name: '倒地',
    description: '规则包：倒地（近战攻击有优势、起身消耗移动；引擎不结算这两条，属叙事层）。',
    duration: 1,
    unit: 'turns'
  },
  {
    id: 'dnd-poisoned',
    name: '中毒',
    description: '规则包：中毒，每回合末 DC 11 体质豁免，成功即终止。',
    duration: 10,
    unit: 'turns'
  }
]

// 规则包新增的技能（PC 的四个 + 供规则消费的两个豁免类技能）。
// 豁免类技能：check.kind = 'save' → 引擎只在**豁免失败**时结算 effect（豁免成功伤害减半由 Lua 补）。
const RULE_SKILLS = [
  {
    id: 'sk-lmop-stealth',
    name: '隐匿',
    description: '敏捷（隐匿）检定。熟练加值由规则包 dnd-proficiency 追加（+5）。',
    category: '检定',
    target: 'self',
    attribute: 'dex',
    check: { dice: '1d20', kind: 'attribute' }
  },
  {
    id: 'sk-lmop-perception',
    name: '察觉',
    description: '睿智（察觉）检定。熟练加值由规则包 dnd-proficiency 追加（+3）。',
    category: '检定',
    target: 'self',
    attribute: 'wis',
    check: { dice: '1d20', kind: 'attribute' }
  },
  {
    id: 'sk-lmop-shortsword',
    name: '短剑',
    description: '近战武器攻击 命中+5，触及5尺，单一目标。命中：6（1d6 + 3）点穿刺伤害。',
    category: '攻击',
    target: 'single',
    attribute: 'dex',
    check: { dice: '1d20', kind: 'attack' },
    effect: { immediate: [{ kind: 'damage', amount: '1d6+3', resource: 'res-hp' }] }
  },
  {
    id: 'sk-lmop-inspiration',
    name: '消耗激励',
    description: '消耗 1 点激励，获得「激励」状态：下一次属性检定 / 攻击检定 / 豁免检定获得优势。',
    category: '特性',
    target: 'self',
    cost: [{ resource: 'res-insp', amount: 1 }],
    effect: { status: ['dnd-inspired'] }
  },
  {
    id: 'sk-lmop-rubble-collapse',
    name: '坠落瓦砾',
    description: 'DC 10 敏捷豁免：失败受 3d6 钝击伤害并倒地，成功则只受一半伤害（dnd-save-half 声明 scale_effect(0.5)，引擎只掷一次效果骰）。',
    category: '陷阱',
    target: 'single',
    attribute: 'dex',
    check: { dice: '1d20', kind: 'save', mode: 'gte', default_dc: 10 },
    // ⚠️ 「失败才倒地」不写在这里：scale_effect 的声明会**整体打开**引擎的效果门
    // （成功豁免也照常结算），而缩放只作用于数值型 delta，状态会原样施加。
    // 所以附加状态放进开放内容 savehalf-rubble.fields.fail_status，由 dnd-save-half
    // 在失败分支补；成功分支只剩「引擎自己掷出、按 0.5 缩放」的那份伤害。
    effect: {
      immediate: [{ kind: 'damage', amount: '3d6', resource: 'res-hp' }]
    }
  },
  {
    id: 'sk-lmop-ash-eruption',
    name: '灰烬喷发',
    description: 'DC 10 体质豁免：失败获得「灰烬呛咳」（攻击 / 豁免 / 属性检定劣势，每回合末可重复豁免）。',
    category: '特性',
    target: 'single',
    attribute: 'con',
    check: { dice: '1d20', kind: 'save', mode: 'gte', default_dc: 10 },
    effect: { status: ['dnd-eruption-penalty'] }
  }
]

// 规则包新增的开放内容种类（引擎不读，编辑器 / 作者 / AI 读）。
const RULE_KINDS = [
  {
    key: 'dnd-rule',
    label: 'D&D 规则条目',
    group: '规则包',
    applies_to: ['storybook'],
    fields: [
      { key: 'mount', label: '挂载点', type: 'text', hint: 'check_pre_roll / check_post_roll / status_tick / turn_end …' },
      { key: 'coverage', label: '覆盖能力', type: 'text' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-proficiency',
    label: '熟练加值',
    group: '规则包',
    applies_to: ['character'],
    fields: [
      { key: 'character_id', label: '角色', type: 'ref', ref_kind: 'character' },
      { key: 'check_attribute', label: '判定属性', type: 'text', hint: '属性维度 key，如 dex' },
      { key: 'bonus', label: '加值', type: 'text' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-sunlight-sensitivity',
    label: '日照敏感',
    group: '规则包',
    applies_to: ['character'],
    fields: [
      { key: 'creature_id', label: '生物', type: 'ref', ref_kind: 'character' },
      { key: 'note', label: '原文', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-pack-tactics',
    label: '集群战术',
    group: '规则包',
    applies_to: ['character'],
    fields: [
      { key: 'creature_id', label: '生物', type: 'ref', ref_kind: 'character' },
      { key: 'ally_proxy', label: '同伴近似口径', type: 'text' },
      { key: 'range_proxy', label: '目标附近近似口径', type: 'text' },
      { key: 'note', label: '原文', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-ambusher',
    label: '伏击',
    group: '规则包',
    applies_to: ['character'],
    fields: [
      { key: 'creature_id', label: '生物', type: 'ref', ref_kind: 'character' },
      { key: 'target_status', label: '目标状态', type: 'text', hint: '目标带该状态即视为「受其突袭」' },
      { key: 'note', label: '原文', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-save-ends',
    label: '重复豁免',
    group: '规则包',
    applies_to: ['status'],
    fields: [
      { key: 'status_id', label: '状态', type: 'ref', ref_kind: 'status' },
      { key: 'dc', label: '豁免 DC', type: 'text' },
      { key: 'attribute', label: '豁免属性', type: 'text' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-save-half',
    label: '豁免成功伤害减半',
    group: '规则包',
    applies_to: ['skill'],
    fields: [
      { key: 'skill_id', label: '技能', type: 'ref', ref_kind: 'skill' },
      { key: 'fail_status', label: '失败附加状态', type: 'ref', ref_kind: 'status', hint: '只有豁免失败才施加；成功只减半伤害' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-encounter-table',
    label: '遭遇掷表',
    group: '规则包',
    applies_to: ['storybook'],
    fields: [
      { key: 'table', label: '表名', type: 'text' },
      { key: 'trigger_die', label: '触发骰', type: 'text', hint: '如 1d20 ≥ 17' },
      { key: 'row_die', label: '查表骰', type: 'text' },
      { key: 'rows', label: '表项', type: 'textarea' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-xp-award',
    label: '经验值合计',
    group: '规则包',
    applies_to: ['storybook'],
    fields: [
      { key: 'formula', label: '合计口径', type: 'text' },
      { key: 'table', label: '每模板 XP', type: 'textarea' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-damage-type',
    label: '伤害类型',
    group: '规则包',
    applies_to: ['storybook'],
    fields: [
      { key: 'damage_type', label: '类型', type: 'text' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'dnd-battle-round',
    label: '战斗轮次',
    group: '规则包',
    applies_to: ['storybook'],
    fields: [
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  }
]

// 覆盖矩阵：验收 1 的八条能力 → 规则 id + 开放内容 id。--check 逐条断言。
const COVERAGE = [
  {
    key: 'advantage_inspiration',
    label: '优势 · 激励',
    rules: ['dnd-status-keep-high', 'dnd-consume-inspiration'],
    definitions: ['rule-dnd-status-keep-high', 'rule-dnd-consume-inspiration', 'prof-pc-lmop-talin-dex']
  },
  {
    key: 'disadvantage_sunlight',
    label: '劣势 · 日照敏感',
    rules: ['dnd-sunlight-sensitivity'],
    definitions: ['rule-dnd-sunlight-sensitivity', 'sunlight-mormesk-the-wraith']
  },
  {
    key: 'advantage_pack_tactics',
    label: '优势 · 集群战术',
    rules: ['dnd-pack-tactics'],
    definitions: ['rule-dnd-pack-tactics', 'pack-wolf']
  },
  {
    key: 'save_half',
    label: '豁免成功伤害减半',
    rules: ['dnd-save-half'],
    definitions: ['rule-dnd-save-half', 'savehalf-rubble']
  },
  {
    key: 'skill_dc',
    label: '技能声明的豁免 DC',
    rules: ['dnd-skill-dc'],
    definitions: ['rule-dnd-skill-dc']
  },
  {
    key: 'proficiency',
    label: '熟练加值',
    rules: ['dnd-proficiency'],
    definitions: ['rule-dnd-proficiency', 'prof-pc-lmop-talin-dex', 'prof-pc-lmop-talin-wis']
  },
  {
    key: 'save_ends',
    label: '重复豁免',
    rules: ['dnd-save-ends'],
    definitions: ['rule-dnd-save-ends', 'saveends-eruption-penalty', 'saveends-poisoned']
  },
  {
    key: 'xp_total',
    label: '经验值合计（enemy_defeated 逐只发放）',
    rules: ['dnd-xp-award'],
    rulePrefixes: [],
    definitions: ['rule-dnd-xp-award', 'xp-lmop-table', 'xp-mon-wolf', 'xp-mon-stirge']
  },
  {
    key: 'encounter_table',
    label: '掷表遭遇（d20 触发 + d12 查表 + 可反复）',
    rules: ['dnd-wander-day', 'dnd-wander-night', 'dnd-wander-reset'],
    definitions: ['rule-dnd-wander-day', 'rule-dnd-wander-night', 'table-lmop-day', 'table-lmop-night']
  },
  {
    key: 'surprise_first_round',
    label: '突袭与战斗第一轮',
    rules: ['dnd-surprise', 'dnd-battle-round', 'dnd-ambusher-keep-high'],
    definitions: ['rule-dnd-surprise', 'rule-dnd-battle-round', 'rule-dnd-ambusher-keep-high']
  }
]

// ---------------------------------------------------------------------------
// 2. 表达不了的缺口（证伪职责：原样登记，不假装解决）
// ---------------------------------------------------------------------------
const GAPS = [
  {
    id: 'GAP-A-no-cross-entity-read',
    rule: '集群战术（狼）/ 任何「对手或盟友」类规则',
    stuck_at: 'Lua 只读得到**当前 actor**：host.get_attribute / get_resource / has_status 全部只作用于 actor；'
      + 'host.target 只带 id / name / kind；没有 get_character / get_encounter / get_flag。',
    need: '一个「按 id 读任意角色实例（属性 / 资源 / 状态 / 位置）」的原语，或把遭遇快照交给 Lua。'
      + '现状：规则包只能用 dnd-flanked 标记代替「盟友在目标 5 尺内」这个事实，由叙事层置位。'
  },
  {
    id: 'GAP-B-no-positioning',
    rule: '集群战术的 5 尺判定 / 任何距离、区域、掩体规则',
    stuck_at: '引擎没有位置、距离、区域概念（只有 location_id 这种地点归属）。',
    need: '位置或「交战关系」的封闭结构（超出规则包范围）；当前只能靠标记。'
  },
  {
    id: 'GAP-C-no-open-content-in-lua',
    rule: '熟练加值 / 日照敏感 / 集群战术 / 伏击 / 重复豁免的**数据来源**',
    stuck_at: 'docs/rules-via-lua.md §7 用了 host.attachment_bonus 读开放内容挂接——引擎里**没有这个 API**，'
      + '也没有任何「读挂接定义 / 读 kinds / 读 definitions」的口子。',
    need: 'host.attachment(...) 一类只读口子，让规则真正由开放内容驱动；'
      + '现状：生成期把开放内容烘进 Lua 表（本脚本的 RULEPACK 常量），开放内容只能给人看。'
  },
  {
    id: 'GAP-D-no-check-signature-pre-roll',
    rule: '按判定种类 / 判定属性给优势（日照敏感的「依赖视力的感知（察觉）检定」那一半）',
    stuck_at: 'check_pre_roll 拿不到判定签名：host.check / host.check_kind / host.check_attribute 只在'
      + '**掷骰后**才有值；而「取高/取低」只有掷骰前有意义（引擎 apply_post_roll_adjustments 注释明说后置取高无效）。',
    need: 'check_pre_roll 也下发判定签名（attribute / kind / difficulty）。'
      + '现状：攻击检定那半靠 host.definition.check.kind 绕过去；感知检定那半表达不了。'
  },
  {
    id: 'GAP-E-effect-roll-not-exposed',
    rule: '豁免成功「伤害减半」的数值等价',
    stuck_at: 'Lua 读不到本次效果已掷出的伤害（没有 pending_effect / last_effect 快照）。',
    need: '把待结算效果的交涉值暴露给 check_post_roll。'
      + '现状：规则包按 host.definition 的骰式**重新掷一次**再取半——期望值对，但与规则书的同一颗骰不等价（方差更大、多消耗 RNG）。'
  },
  {
    id: 'GAP-F-no-defeat-event',
    rule: 'XP 合计的触发时机（「合计角色们克服的每个怪物的经验值」）',
    stuck_at: '引擎只在场景切换派发 event（scene / scene_change）；没有「敌人被击败 / 遭遇结束」事件，'
      + 'Lua 也读不到遭遇内容（encounters 只在 when 闸门的 CondExpr 快照里）。',
    need: '击败事件（带 enemy / template_id / xp）或遭遇快照只读口。'
      + '现状：只用 when: encounter_cleared + 掷表行标记回补 XP；导演即兴创建的遭遇（LMoP 里的大多数）拿不到 XP。'
  },
  {
    id: 'GAP-G-encounter-count-static',
    rule: '野外遭遇表的数量（1d8+2 只蚊蝠）',
    stuck_at: 'EncounterPresetEnemy.count 是静态整数，没有骰式的落点。',
    need: '预置遭遇支持骰式数量（或让 Lua 参与建遭遇）。'
      + '现状：规则包把每行固定成骰式下界，原始骰式写进 note。'
  },
  {
    id: 'GAP-H-no-unset-flag',
    rule: '战斗轮次标记 / 掷表行标记的清理',
    stuck_at: 'ImmediateEffect 只有 set_flag，没有 unset_flag；Lua 也没有清标记口子。',
    need: '清标记原语（或按轮次命名并按序号收敛）。'
      + '现状：dnd-battle-round-1..3 只置不清，闸门必须按具体轮次取值。'
  },
  {
    id: 'GAP-I-no-turn-economy',
    rule: '突袭的「在战斗第一轮失去其回合」',
    stuck_at: '引擎没有先攻、轮次、行动经济——没有「回合」可以失去。',
    need: '战斗轮次结构（超出规则包范围）。'
      + '现状：只落 dnd-surprised 状态 + dnd-battle-round-1 标记，失去回合由叙事层落实。'
  },
  {
    id: 'GAP-J-no-opposed-passive',
    rule: '突袭对抗「与地精的被动感知（察觉）属性值对抗」',
    stuck_at: '挂载点拿不到对手的被动值；CondExpr 也没有「掷骰 vs 对手被动」的入口'
      + '（CondExpr::AttributeGe 只看 actor 自己的属性值）。',
    need: '对抗判定 / 被动值作为对手（crates 侧进行中）。'
      + '现状：难度由导演按数据卡被动值给（地精被动察觉 9），规则只按判定成败落突袭标记。'
  },
  {
    id: 'GAP-K-status-not-seeded',
    rule: '怪物固有特性用状态表达（把「日照敏感」做成模板自带状态）',
    stuck_at: 'build_state 建实例时 statuses 恒为空数组（octopus-api/src/lib.rs），character 模板没有初始状态声明。',
    need: '模板级初始状态（或初始挂接 → 状态的桥）。'
      + '现状：模板级特性只能写进 Lua 表（见 GAP-C）。'
  },
  {
    id: 'GAP-L-target-status-unreadable',
    rule: '伏击的「对受其突袭的生物发动攻击」',
    stuck_at: 'host.target 不含 statuses / attributes / resources（只有 id / name / kind）。',
    need: '目标的只读快照（至少 statuses）。'
      + '现状：用 dnd-surprised 标记近似「本场有人被突袭」。'
  },
  {
    id: 'GAP-M-no-encounter-active-cond',
    rule: '战斗轮次计数的开闸条件',
    stuck_at: 'CondExpr 只有 encounter_cleared（「有遭遇」没有对应条件；「无遭遇」时 encounter_cleared 也不成立，取反得不到「有遭遇」）。',
    need: 'encounter_active 条件。'
      + '现状：靠 dnd-in-combat 标记开闸（导演 / AI 置位）。'
  },
  {
    id: 'GAP-N-trigger-one-shot',
    rule: '掷表遭遇的可重复性（遭遇表本该反复生效）',
    stuck_at: 'evaluate_skeleton 对每个触发点做进度去重（progress.triggers），且 TriggerDef.repeatable 只在种子数据里出现、**引擎从未实现**；'
      + '掷表行标记又没有清除原语（GAP-H），所以每行表项整局只能触发一次。',
    need: '可重复触发点（或「触发点 fired 后可复位」的表达）。'
      + '现状：24 行表项 = 整局最多 24 场掷表遭遇，之后三猪小径不再出事。'
  }
]

// ---------------------------------------------------------------------------
// 2b. 缺口现状（T17 / T18 原语落地后的复核）
// ---------------------------------------------------------------------------
// 每条 GAP 保留**历史原文**（stuck_at / need 是当时的实况，不改写历史），另附本轮复核结论：
//   status     closed = 已用新原语真正闭合（规则包已改用，且 --check / 引擎校验断言）
//              open   = 仍然存在（说明卡在哪个原语上）
//   closed_by  闭合用到的原语
//   evidence   规则包里的落地位置 / 断言
const GAP_RESOLUTION = {
  'GAP-A-no-cross-entity-read': {
    status: 'partial',
    closed_by: 'host.get_character(id) / host.get_flag / host.list_flags / host.get_encounter / host.list_encounters'
      + '（host.target 同时升级为与 host.actor 同级完整）',
    evidence: '读任意实体的能力**真正落地并已用上**：集群战术用 host.list_encounters 找同遭遇同伴、'
      + 'host.get_character 读同伴 HP；伏击用 host.target.statuses；XP 用 host.get_encounter 兜底回查 template_id。'
      + '近似口径由开放内容声明（pack-*.fields.ally_proxy / range_proxy），Lua 里没有名单常量。',
    now: '按「近似提高」登记而不是「已闭合」：这条缺口当初登记的是**集群战术规则**，'
      + '而「目标 5 尺内」这个事实仍受 GAP-B 限制——只能近似成「同遭遇 + 同 location_id + HP>0」，'
      + '判定仍不精确。跨实体读取这一**能力**本身已闭合（见 closed_by / evidence），但规则结果不是精确的。',
  },
  'GAP-B-no-positioning': {
    status: 'open',
    now: '仍然存在：引擎没有位置 / 距离 / 区域概念（只有 location_id 这种地点归属）。'
      + 'GAP-A 近似提高后集群战术能读同伴了，但「5 尺内」只能用「同一场遭遇 + 同一 location_id」近似，'
      + 'HP>0 也只是「未失能」的近似——近似程度提高，判定仍不精确。'
  },
  'GAP-C-no-open-content-in-lua': {
    status: 'closed',
    closed_by: 'host.get_attachments / host.get_attachment(kind) / host.get_definition(id) / host.list_definitions(kind)',
    evidence: '熟练加值 / 日照敏感 / 集群战术 / 伏击 / 重复豁免 / XP 数值全部改成运行时读开放内容；'
      + '脚本里不再有加值 / 名单 / DC / XP 常量，--check 的 open_content.data_driven 断言逐条检查。'
  },
  'GAP-D-no-check-signature-pre-roll': {
    status: 'closed',
    closed_by: 'check_pre_roll 下发判定签名（host.check_attribute / host.check_kind / host.check_target）',
    evidence: 'dnd-sunlight-sensitivity 按签名区分「攻击骰」与「依赖视力的感知（wis）检定」；'
      + 'dnd-proficiency 也回到 check_pre_roll 用签名选属性。'
  },
  'GAP-E-effect-roll-not-exposed': {
    status: 'closed',
    closed_by: 'host.scale_effect(factor)（只在 check_post_roll / pre_resolve）+ PostResolve 只读 host.resolved_effects',
    evidence: 'dnd-save-half 改为在 check_post_roll 声明 host.scale_effect(0.5)：引擎照常结算一次、'
      + '只掷一次效果骰，数值型 delta 按 0.5 向零取整（多段效果 / modifiers 一并覆盖），不再由 Lua 重掷同一骰式。'
      + '引擎校验断言 save_half.engine_scales_own_roll（同种子下 0.5 的结果恰为 1.0 结果的一半）与'
      + ' save_half.resolved_effects_is_engine_snapshot（PostResolve 读到的 factor / rng_consumed / delta 与提交值一致）。'
      + '配套：技能的失败附加状态移入开放内容 fail_status，由脚本在失败分支补（缩放不覆盖状态）。'
  },
  'GAP-F-no-defeat-event': {
    status: 'closed',
    closed_by: 'event 挂载点 + host.event_name / host.event_data（enemy_defeated / encounter_cleared）',
    evidence: 'dnd-xp-award 挂在 event 上，按 data.enemy.template_id 逐只发 XP；'
      + '与遭遇来源无关，导演即兴建的遭遇同样覆盖（引擎校验断言 xp.improvised_encounter）。'
      + 'T21 之后还能用 host.get_encounter 读遭遇快照：事件缺 template_id 时按 instance_id 回查'
      + '（引擎校验 xp.encounter_snapshot_fallback）——常规路径不变，这是兜底。'
      + '边界：非 strike 击杀（Lua 直接把资源打到 0）不派发该事件，那种路径不发 XP。'
  },
  'GAP-G-encounter-count-static': {
    status: 'open',
    now: '仍然存在：EncounterPresetEnemy.count 是静态整数，没有骰式落点；'
      + '规则包把每行固定成骰式下界，原始骰式写进触发点 hint 与定义 rows。'
  },
  'GAP-H-no-unset-flag': {
    status: 'closed',
    closed_by: "host.set_flag(flag[, value]) / host.clear_flag(flag)",
    evidence: 'dnd-battle-round 每轮清 1..3 再置当前轮；dnd-wander-reset 在 encounter_cleared 时清行标记。'
  },
  'GAP-I-no-turn-economy': {
    status: 'open',
    now: '仍然存在：引擎没有先攻 / 轮次 / 行动经济，「在战斗第一轮失去其回合」只能靠状态 + 标记 + 叙事层。'
  },
  'GAP-J-no-opposed-passive': {
    status: 'open',
    now: 'CondExpr 仍没有「掷骰 vs 对手被动」的入口（AttributeGe 只看 actor 自己的属性值）；'
      + '对抗判定结构（Intent::Check.opponent_id）已在引擎侧，规则脚本仍读不到对手被动值。'
  },
  'GAP-K-status-not-seeded': {
    status: 'open',
    now: '仍然存在：character 模板没有初始状态声明，建实例时 statuses 恒为空数组；'
      + '「模板自带状态」还缺一个声明入口（挂接读得到，但挂接 → 状态的桥没有）。'
  },
  'GAP-L-target-status-unreadable': {
    status: 'closed',
    closed_by: 'host.target 与 host.actor 同级完整（含 statuses / attributes / resources / location_id）',
    evidence: 'dnd-ambusher-keep-high 直接读 host.target.statuses 判断目标是否带 dnd-surprised'
      + '（期望状态 id 来自开放内容 fields.target_status，脚本无状态名常量）；when 闸门不再要求 dnd-surprised 标记。'
      + '引擎校验断言 ambusher.reads_target_statuses（带状态 → keep_high；不带 → 零请求）。'
  },
  'GAP-M-no-encounter-active-cond': {
    status: 'open',
    now: '仍然存在：CondExpr 仍只有 encounter_cleared，没有 encounter_active；'
      + '战斗轮计数继续靠 dnd-in-combat 标记开闸。'
  },
  'GAP-N-trigger-one-shot': {
    status: 'closed',
    closed_by: 'TriggerDef.repeatable（边沿语义）+ host.clear_flag 复位',
    evidence: '24 个掷表触发点全部 repeatable: true，遭遇结束时 dnd-wander-reset 清行标记复位；'
      + '引擎校验断言 encounter_table.refires（同一表项在同一局里触发 ≥ 2 次）。'
  }
}
for (const gap of GAPS) {
  const resolution = GAP_RESOLUTION[gap.id]
  if (!resolution) throw new Error('缺口 ' + gap.id + ' 没有复核结论')
  Object.assign(gap, resolution)
}

// ---------------------------------------------------------------------------
// 3. Lua 脚本生成
// ---------------------------------------------------------------------------

function luaStringMap(map, valueMapper) {
  const entries = Object.keys(map).map(k => {
    const v = valueMapper ? valueMapper(map[k]) : 'true'
    return "  ['" + k + "'] = " + v
  })
  return '{\n' + entries.join(',\n') + '\n}'
}

function luaNumberMap(map) {
  return luaStringMap(map, v => String(v.bonus))
}

/** 优势：actor 身上有「优势来源状态」→ 掷两次取高。 */
function srcStatusAdvantage() {
  return [
    '-- 规则包：优势（规则集词汇）＝ 引擎通用动作 keep_high（掷两次取高）。',
    'local KEEP_HIGH_STATUSES = ' + luaStringMap(RULEPACK.advantage_statuses),
    'local function has_any(list)',
    '  for id in pairs(list) do',
    '    if host.has_status(id) then return true end',
    '  end',
    '  return false',
    'end',
    'if has_any(KEEP_HIGH_STATUSES) then',
    "  host.modify_check('keep_high')",
    'end'
  ].join('\n')
}

/** 劣势：actor 身上有「劣势来源状态」→ 掷两次取低。 */
function srcStatusDisadvantage() {
  return [
    '-- 规则包：劣势（规则集词汇）＝ 引擎通用动作 keep_low（掷两次取低）。',
    'local KEEP_LOW_STATUSES = ' + luaStringMap(RULEPACK.disadvantage_statuses),
    'local function has_any(list)',
    '  for id in pairs(list) do',
    '    if host.has_status(id) then return true end',
    '  end',
    '  return false',
    'end',
    'if has_any(KEEP_LOW_STATUSES) then',
    "  host.modify_check('keep_low')",
    'end'
  ].join('\n')
}

/** 日照敏感：**按判定签名**区分（GAP-D 闭合）+ 从开放内容读（GAP-C 闭合）。
 *
 *  附录 B 原文（缚灵·墨梅斯克）：「在阳光下，该生物在攻击骰上处于劣势，并且在**依赖视力的
 *  感知（察觉）检定**上也处于劣势」。判定签名（kind / attribute）在 check_pre_roll 就有值，
 *  所以两条分支都能精确表达，不再只覆盖攻击那半。 */
function srcSunlightSensitivity() {
  return [
    '-- 规则包：日照敏感（缚灵·墨梅斯克，附录 B 原文）。',
    '-- 数据来源：开放内容（角色模板的 dnd-sunlight-sensitivity 挂接 → definition）。',
    '-- 判定签名在 check_pre_roll 就有（GAP-D 闭合）：',
    "--   kind == 'attack'                          → 攻击骰劣势（原文第一半）",
    "--   kind == 'attribute' and attribute == 'wis' → 依赖视力的感知（察觉）检定劣势（原文第二半）",
    '-- 其它判定（力量检定 / 体质豁免 / 敏捷豁免 …）一律不受影响。',
    "local ids = host.get_attachment('dnd-sunlight-sensitivity')",
    'if type(ids) ~= "table" or #ids == 0 then return end',
    'local kind = host.check_kind',
    'local attribute = host.check_attribute',
    "local attack = kind == 'attack'",
    "local sight = kind == 'attribute' and attribute == 'wis'",
    'if attack or sight then',
    "  host.modify_check('keep_low')",
    'end'
  ].join('\n')
}

/** 集群战术（狼）：目标 5 尺内有未失能的盟友 → 攻击检定优势。
 *
 *  T21 之后规则包能读运行时事实了（host.get_character / host.list_encounters），
 *  不再是「导演置位 dnd-flanked」的纯标记近似。**但仍受 GAP-B 限制**：
 *  引擎没有位置 / 距离 / 交战关系，所以：
 *    · 「我方同伴」= 与我同处一场活跃遭遇的其它敌人（遭遇的 enemies 数组即分组），
 *      HP > 0 是「未失能」能做到的最接近近似（引擎没有失能状态声明）；
 *    · 「在目标附近」= 目标与我在同一 location_id（缺任一侧时不做判断）。
 *  仍然表达不了：精确到 5 尺、以及「失能」的完整语义（麻痹 / 震慑 / 昏迷 …）。
 *  数据来源：开放内容（角色模板的 dnd-pack-tactics 挂接），脚本无生物名单。 */
function srcPackTactics() {
  return [
    '-- 规则包：集群战术（狼，附录 B 原文）。',
    '-- 数据来源：开放内容（角色模板的 dnd-pack-tactics 挂接），脚本无生物名单。',
    '-- T21：读运行时事实——同一场遭遇的同伴（list_encounters + get_character）。',
    '-- 仍受 GAP-B（没有位置 / 距离）：同伴按「同一场遭遇」近似，目标按「同一 location_id」近似。',
    "local ids = host.get_attachment('dnd-pack-tactics')",
    'if type(ids) ~= "table" or #ids == 0 then return end',
    'local me = host.actor',
    'local target = host.target',
    'if type(me) ~= "table" or type(target) ~= "table" then return end',
    'local my_key = me.id or me.instance_id',
    'local function alive(inst)',
    "  local hp = inst and inst.resources and inst.resources['res-hp']",
    '  return type(hp) == "number" and hp > 0',
    'end',
    'local ally_found = false',
    'for _, enc in ipairs(host.list_encounters()) do',
    '  local enemies = enc.enemies',
    '  if type(enemies) == "table" then',
    '    local mine = false',
    '    for _, e in ipairs(enemies) do',
    '      if (e.instance_id or e.id) == my_key then mine = true end',
    '    end',
    '    if mine then',
    '      for _, e in ipairs(enemies) do',
    '        local key = e.instance_id or e.id',
    '        if key and key ~= my_key then',
    '          if alive(host.get_character(key)) then ally_found = true end',
    '        end',
    '      end',
    '    end',
    '  end',
    'end',
    'if not ally_found then return end',
    '-- 「在目标 5 尺内」的可用近似：双方都知道地点时必须同地点（GAP-B 仍然存在）。',
    'local my_place = me.location_id',
    'local target_place = target.location_id',
    'if my_place and target_place and my_place ~= target_place then return end',
    "host.modify_check('keep_high')"
  ].join('\n')
}

/** 伏击：战斗第一轮里，**直接读目标的状态**（host.target.statuses，GAP-L 闭合）
 *  判断「这个目标是否受我突袭」，不再用 dnd-surprised 标记近似。
 *  期望的状态 id 来自开放内容 dnd-ambusher.fields.target_status（脚本无状态名常量）。 */
function srcAmbusherAdvantage() {
  return [
    '-- 规则包：伏击（变形怪，附录 B 原文）：战斗开始的第一轮里，',
    '-- 对任何成功受其突袭的生物所发动的攻击检定具有优势。',
    '-- T21：host.target 与 host.actor 同级完整（含 statuses）——直接按目标状态判定（GAP-L 闭合）。',
    '-- 数据来源：开放内容（角色模板的 dnd-ambusher 挂接 → fields.target_status），脚本无状态名常量。',
    "local ids = host.get_attachment('dnd-ambusher')",
    'if type(ids) ~= "table" or #ids == 0 then return end',
    'local target = host.target',
    'local statuses = target and target.statuses',
    'if type(statuses) ~= "table" then return end',
    'for _, def_id in ipairs(ids) do',
    '  local def = host.get_definition(def_id)',
    '  local wanted = def and def.fields and def.fields.target_status',
    '  if wanted and wanted ~= "" then',
    '    for _, st in ipairs(statuses) do',
    '      if st.id == wanted or st.name == wanted then',
    "        host.modify_check('keep_high')",
    '        return',
    '      end',
    '    end',
    '  end',
    'end'
  ].join('\n')
}

/** 技能声明的检定 / 豁免 DC：引擎的 use_skill 路径只取 world.check.default_dc，
 *  不读技能自己的 check.default_dc —— 规则包把它翻译成通用原语 dc 修正。 */
function srcSkillDc() {
  return [
    '-- 规则包：技能声明的豁免 DC（模块第 1727 行「DC 10 敏捷豁免」）。',
    '-- 引擎实况：use_skill 路径的难度只取 world.check.default_dc，技能的 check.default_dc 不参与；',
    '-- 规则包在掷骰前用通用原语 dc 把难度对齐到技能声明的值。',
    'local definition = host.definition',
    'local check = definition and definition.check',
    'local want = nil',
    'if type(check) == "table" then want = check.default_dc end',
    'local current = host.difficulty',
    'if want and current and want ~= current then',
    "  host.modify_check('dc', want - current)",
    'end'
  ].join('\n')
}

/** 熟练加值：**从开放内容读**（GAP-C 闭合），掷骰前按判定签名补加值（GAP-D 闭合）。
 *
 *  链路：character.attachments['dnd-proficiency'] → definition id → fields.bonus
 *  脚本里**没有任何加值数字**：改故事书里 definition 的 fields.bonus，检定总值随之变。 */
function srcProficiency() {
  return [
    '-- 规则包：熟练加值（模块第 194 行「隐匿技能调整值：+6」的同一条机制）。',
    '-- 数据来源：开放内容（角色模板的 dnd-proficiency 挂接 → definition.fields）。',
    '-- 本脚本不含任何加值常量：加多少由故事书数据决定（改数据即改结果）。',
    '-- 判定签名在 check_pre_roll 就有（attribute / kind），所以加值走**掷骰前**修正。',
    "if host.check_kind ~= 'attribute' then return end",
    "local ids = host.get_attachment('dnd-proficiency')",
    'if type(ids) ~= "table" then return end',
    'local attribute = host.check_attribute',
    'for _, id in ipairs(ids) do',
    '  local def = host.get_definition(id)',
    '  local fields = def and def.fields',
    '  if fields and fields.check_attribute == attribute then',
    '    local bonus = tonumber(fields.bonus)',
    '    if bonus and bonus ~= 0 then',
    "      host.modify_check('add', bonus)",
    '    end',
    '  end',
    'end'
  ].join('\n')
}

/** 豁免成功伤害减半（GAP-E 闭合）：声明 host.scale_effect(0.5)，让**引擎自己**
 *  掷一次效果骰、按 0.5 缩放数值型 delta —— 不再由 Lua 重掷同一骰式。
 *
 *  为什么失败分支要补状态：scale_effect 的声明会覆盖「豁免成功 = 不结算」那道门
 *  （效果照常结算一次），而缩放只作用于数值 delta；状态会原样施加。所以技能里
 *  只留伤害，附加状态（倒地）由本脚本在失败分支按开放内容补。
 *
 *  规则只对**登记在开放内容里**的技能生效（dnd-save-half 定义的 fields.skill_id），
 *  脚本里没有技能名 / 状态名 / 骰式常量。 */
function srcSaveHalf() {
  return [
    '-- 规则包：豁免成功伤害减半（模块第 1727 行原文）。',
    '-- T22：scale_effect 只在 check_post_roll / pre_resolve 存在；效果照常由引擎结算一次，',
    '--       数值型 delta 按因子缩放（向零取整、逐条）——全程只掷一次效果骰。',
    '-- 数据来源：开放内容 dnd-save-half 定义（fields.skill_id / fields.fail_status）。',
    "if host.check_kind ~= 'save' then return end",
    'local definition = host.definition',
    'local skill_id = definition and definition.id',
    'local rule = nil',
    "for _, def in ipairs(host.list_definitions('dnd-save-half')) do",
    '  local fields = def.fields',
    '  if fields and fields.skill_id == skill_id then rule = fields end',
    'end',
    'if not rule then return end',
    'if host.check_result == true then',
    '  -- 豁免成功：引擎按本次实际掷出的效果骰结算一次，数值减半。',
    '  host.scale_effect(0.5)',
    'elseif rule.fail_status and rule.fail_status ~= "" then',
    '  -- 豁免失败：默认全量结算；失败才附加的状态由本脚本补（缩放不覆盖状态）。',
    '  local target = host.target and (host.target.id or host.target.instance_id)',
    '  if target then',
    "    host.apply_status(target, rule.fail_status, 1, 'turns')",
    '  end',
    'end'
  ].join('\n')
}

/** 重复豁免：回合末（status_tick）再掷一次豁免，成功即移除该状态。
 *  数据来源：开放内容 dnd-save-ends 定义（fields.status_id / dc / attribute），脚本无 DC 常量。 */
function srcSaveEnds() {
  return [
    '-- 规则包：重复豁免（模块第 2541 行「生物可以在其每回合结束时重复进行豁免检定，若成功则提前终止效应」）。',
    '-- 时机原语 status_tick：引擎只给「谁 / 哪个状态 / 还剩多久」，要不要结束由规则包掷骰决定。',
    '-- 数据来源：开放内容（dnd-save-ends 定义），本脚本不含 DC / 属性常量。',
    'local rule = nil',
    "local defs = host.list_definitions('dnd-save-ends')",
    'for _, def in ipairs(defs) do',
    '  local fields = def.fields',
    '  if fields and fields.status_id == host.status_id then rule = fields end',
    'end',
    'if rule then',
    '  local score = host.get_attribute(rule.attribute)',
    '  local modifier = 0',
    '  if score then modifier = math.floor((score - 10) / 2) end',
    '  local roll = host.engine_rng(1, 20) + modifier',
    '  if roll >= tonumber(rule.dc) then',
    '    host.remove_status(host.actor.id, host.status_id)',
    '  end',
    'end'
  ].join('\n')
}

/** 突袭：敏捷（隐匿）检定失败 → 受突袭标记 + 战斗第一轮有人被突袭。 */
function srcSurprise() {
  return [
    '-- 规则包：突袭（模块第 195 / 309 行原文）。',
    '-- 判定成败由引擎给（难度由导演按地精被动感知 9 给，见 GAP-J）；',
    '-- 「失去其回合」引擎没有回合可以失去（GAP-I），只落状态与标记。',
    "if host.check_kind == 'attribute' and host.check_attribute == 'dex' and host.check_result == false then",
    "  host.apply_status(host.actor.id, 'dnd-surprised', 1, 'turns')",
    "  host.apply_effect(host.actor.id, { kind = 'set_flag', flag = 'dnd-surprised' })",
    'end'
  ].join('\n')
}

/** 战斗轮次：turn_end + dnd-in-combat 闸门 → 自增、**清掉上一轮标记**、置本轮标记。
 *  清标记原语（GAP-H 闭合）让「第 N 轮」始终只有一个为真，闸门不必再按具体轮次收敛。 */
function srcBattleRound() {
  const clears = [1, 2, 3].map(n => "host.clear_flag('dnd-battle-round-" + n + "')").join('\n')
  return [
    '-- 规则包：战斗轮次（突袭需要的「战斗第一轮」标记，模块第 195 行）。',
    '-- CondExpr 没有「当前有遭遇」这个条件（GAP-M），开闸靠 dnd-in-combat 标记。',
    '-- 清标记原语（GAP-H 闭合）：每轮先清 1..3，再置当前轮——任何时刻至多一个轮次标记为真。',
    'local round = (host.storage.dnd_battle_round or 0) + 1',
    'host.storage.dnd_battle_round = round',
    clears,
    'if round <= 3 then',
    "  host.apply_effect(host.actor.id, { kind = 'set_flag', flag = 'dnd-battle-round-' .. tostring(round) })",
    'end'
  ].join('\n')
}

/** 掷表遭遇：d20 触发 + d12 查表 → set_flag（触发点 condition: flag_set + encounter 预置）。
 *
 *  可重复（GAP-N 闭合）：触发点标 repeatable: true，边沿语义由**复位**保证——
 *  某行表项对应的遭遇结束时（encounter_cleared 事件），srcWanderReset 清掉该行标记，
 *  该行下一次被掷中就是一次新的「假 → 真」边沿，于是可以反复出。
 *  遭遇未结束期间同一行再被掷中不产生边沿 → 不会重复堆同名遭遇（刻意如此）。 */
function srcWanderTable(table) {
  const flags = table.rolls.map(r => "  [" + r.die + "] = '" + r.flag + "'").join(',\n')
  return [
    '-- 规则包：野外遭遇掷表（模块第 1221 行原文）。',
    '-- 链路：Lua 掷骰 → set_flag → 触发点 condition: flag_set（repeatable）+ encounter 预置',
    '--       → 回合末骨架求值在边沿建遭遇；遭遇结束时由 dnd-wander-reset 清标记复位。',
    '-- 引擎不认识「遭遇表」，表只活在规则包 Lua 里。',
    'local check = host.engine_rng(1, 20)',
    'if check >= 17 then',
    '  local row = host.engine_rng(1, 12)',
    '  local FLAGS_BY_ROW = {\n' + flags + '\n  }',
    '  local flag = FLAGS_BY_ROW[row]',
    '  if flag then',
    '    host.set_flag(flag)',
    '  end',
    'end'
  ].join('\n')
}

/** XP 合计：**在 enemy_defeated 事件按被击败敌人的模板发**（GAP-F 闭合）。
 *
 *  模块第 1987 行「合计角色们克服的每个怪物的经验值」——「每个怪物」正是逐只结算的口径。
 *  挂在 event 挂载点后，只要引擎派发 enemy_defeated（strike 击杀路径）就发 XP，
 *  **与遭遇从哪来无关**：掷表建的行、导演即兴建的、AI 用 Intent::Encounter 建的，一视同仁。
 *
 *  XP 数值来自**开放内容**：dnd-xp-award 定义的 fields.template_id / fields.xp
 *  （由 buildOpenContent 从 T8 图鉴草稿的 statblock.xp 生成，脚本不复制数值）。
 *
 *  边界（原样登记）：Lua 直接把怪物资源打到 0（非 strike 路径）**不派发该事件**，
 *  那种击杀拿不到 XP——奖励只挂在引擎认得的「击败」事实上。 */
function srcXpAward() {
  return [
    '-- 规则包：经验值合计（模块第 1987 行「合计角色们克服的每个怪物的经验值」）。',
    '-- 时机：event 挂载点 + enemy_defeated（data.enemy.template_id / data.killer）。',
    '-- 数值：开放内容 dnd-xp-award 定义（fields.template_id → fields.xp），脚本无 XP 常量。',
    "if host.event_name ~= 'enemy_defeated' then return end",
    'local data = host.event_data',
    'local enemy = data and data.enemy',
    'local template_id = enemy and enemy.template_id',
    '-- T21：遭遇快照可读（get_encounter）——事件缺 template_id 时按 instance_id 回查。',
    '-- 这是**兜底**，不改变常规路径（strike 派发的 data.enemy 本就带 template_id）。',
    'if (not template_id or template_id == "") and enemy and enemy.instance_id then',
    '  local enc = data and data.encounter and data.encounter.id',
    '  local snapshot = enc and host.get_encounter(enc)',
    '  local list = snapshot and snapshot.enemies',
    '  if type(list) == "table" then',
    '    for _, e in ipairs(list) do',
    '      if e.instance_id == enemy.instance_id and e.template_id then template_id = e.template_id end',
    '    end',
    '  end',
    'end',
    'if not template_id or template_id == "" then return end',
    'local xp = 0',
    "local defs = host.list_definitions('dnd-xp-award')",
    'for _, def in ipairs(defs) do',
    '  local fields = def.fields',
    '  if fields and fields.template_id == template_id then xp = tonumber(fields.xp) or 0 end',
    'end',
    'if xp > 0 then',
    "  host.modify_resource(host.actor.id, 'res-xp', xp)",
    'end'
  ].join('\n')
}

/** 掷表标记复位：某行表项对应的遭遇结束时清掉该行标记（GAP-N 的边沿复位，GAP-H 的清标记原语）。
 *
 *  遭遇名字由规则包生成（srcEncounterName），在表项之间**唯一**，所以「遭遇 → 标记」可精确反查；
 *  多场掷表遭遇同时进行时，只复位真正结束的那一场。 */
function srcWanderReset(tables) {
  const names = []
  for (const table of tables) {
    for (const roll of table.rolls) {
      names.push("  ['" + encounterName(table, roll) + "'] = '" + roll.flag + "'")
    }
  }
  return [
    '-- 规则包：掷表标记复位（repeatable 触发点的边沿复位）。',
    '-- 触发点 repeatable 只在条件「假 → 真」时再触发；遭遇结束后把行标记清掉，',
    '-- 下一次掷中同一行才是一次新的边沿——这就是「同一表项能反复出」的全部机制。',
    "if host.event_name ~= 'encounter_cleared' then return end",
    'local data = host.event_data',
    'local name = data and data.encounter and data.encounter.name',
    'if not name then return end',
    'local FLAG_BY_ENCOUNTER = {\n' + names.join(',\n') + '\n}',
    'local flag = FLAG_BY_ENCOUNTER[name]',
    'if flag then',
    '  host.clear_flag(flag)',
    'end'
  ].join('\n')
}

/** 掷表预置遭遇的名字：表项之间唯一（含 d12 行号），供 srcWanderReset 精确反查标记。 */
function encounterName(table, roll) {
  return table.label + ' d12=' + roll.die + '：' + roll.monsterName
}

/** 消耗激励：掷骰后移除（5e 的激励是「一次检定」，引擎没有消耗式状态）。 */
function srcConsumeInspiration() {
  return [
    '-- 规则包：激励用掉即消失（5e：一次属性检定 / 攻击检定 / 豁免检定）。',
    '-- 引擎的状态只有时长，没有「被一次判定消耗」的语义——用 check_post_roll + remove_status 表达。',
    "if host.has_status('dnd-inspired') then",
    "  host.remove_status(host.actor.id, 'dnd-inspired')",
    'end'
  ].join('\n')
}

// ---------------------------------------------------------------------------
// 4. 构建
// ---------------------------------------------------------------------------

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'))
}

function ensureDraft() {
  if (!fs.existsSync(F.draft)) {
    execFileSync(process.execPath, [F.importer], { cwd: ROOT, stdio: 'inherit' })
  }
  return readJson(F.draft)
}

function indexMonsters(draft) {
  const byName = new Map()
  for (const c of draft.characters) {
    if (c.kind === 'monster') byName.set(c.name, c)
  }
  return byName
}

/** 骰式下界：'1d8+2' → 3；'1' → 1（规则包对「数量是骰式」的降级取法，见 GAP-G）。 */
function diceFloor(expr) {
  const m = /^(\d+)d(\d+)([+-]\d+)?$/.exec(expr.trim())
  if (!m) return Number(expr) || 1
  return Number(m[1]) + (m[3] ? Number(m[3]) : 0)
}

function expandTable(table, monsters) {
  const out = []
  for (const row of table.rows) {
    const monster = monsters.get(row.name)
    if (!monster) throw new Error('遭遇表引用了图鉴里没有的怪物：' + row.name)
    const count = Math.max(1, diceFloor(row.dice))
    for (let die = row.die[0]; die <= row.die[1]; die++) {
      out.push({
        die,
        flag: table.flagPrefix + '-' + die,
        monsterId: monster.id,
        monsterName: monster.name,
        dice: row.dice,
        count,
        xp: (monster.statblock && monster.statblock.xp ? monster.statblock.xp : 0) * count
      })
    }
  }
  return out.sort((a, b) => a.die - b.die)
}

function buildRules(draft) {
  const monsters = indexMonsters(draft)
  const tables = WANDER_TABLES.map(t => ({ ...t, rolls: expandTable(t, monsters) }))
  const rules = []
  const add = r => { rules.push(r); return r }

  add({
    id: 'dnd-status-keep-high',
    mount: 'check_pre_roll',
    source: srcStatusAdvantage(),
    coverage: ['advantage_inspiration'],
    note: '激励等优势来源状态 → 掷两次取高'
  })
  add({
    id: 'dnd-status-keep-low',
    mount: 'check_pre_roll',
    source: srcStatusDisadvantage(),
    coverage: ['disadvantage_sunlight'],
    note: '灰烬呛咳等劣势来源状态 → 掷两次取低'
  })
  add({
    id: 'dnd-sunlight-sensitivity',
    mount: 'check_pre_roll',
    source: srcSunlightSensitivity(),
    coverage: ['disadvantage_sunlight'],
    when: { op: 'flag_set', flag: 'dnd-sunlight' },
    note: '阳光下（dnd-sunlight 标记）攻击检定劣势'
  })
  add({
    id: 'dnd-pack-tactics',
    mount: 'check_pre_roll',
    source: srcPackTactics(),
    coverage: ['advantage_pack_tactics'],
    // GAP-A 闭合：不再用 when: dnd-flanked 开闸——规则自己读运行时事实（同遭遇同伴 + 同地点）。
    // 精确「目标 5 尺内」仍做不到（GAP-B），近似口径见 srcPackTactics 注释，不假装准确。
    note: '集群战术：同遭遇同伴（get_character）+ 同 location_id 近似「目标 5 尺内有盟友」'
  })
  add({
    id: 'dnd-ambusher-keep-high',
    mount: 'check_pre_roll',
    source: srcAmbusherAdvantage(),
    coverage: ['surprise_first_round'],
    // GAP-L 闭合：闸门只留「战斗第一轮」——「这个目标是否受我突袭」由脚本读
    // host.target.statuses 判定，不再要求 dnd-surprised 全场标记。
    when: { op: 'flag_set', flag: 'dnd-battle-round-1' },
    note: '伏击：战斗第一轮，对 host.target.statuses 带受突袭状态的目标攻击检定优势'
  })
  add({
    id: 'dnd-skill-dc',
    mount: 'check_pre_roll',
    source: srcSkillDc(),
    coverage: ['skill_dc'],
    note: '把技能声明的豁免 / 检定 DC 翻译成 dc 原语'
  })
  add({
    id: 'dnd-proficiency',
    mount: 'check_pre_roll',
    source: srcProficiency(),
    coverage: ['proficiency'],
    note: '熟练加值：从开放内容读（模板挂接 → definition.fields.bonus），按判定签名只在属性检定上'
  })
  add({
    id: 'dnd-save-half',
    mount: 'check_post_roll',
    source: srcSaveHalf(),
    coverage: ['save_half'],
    note: '豁免成功伤害减半'
  })
  add({
    id: 'dnd-consume-inspiration',
    mount: 'check_post_roll',
    source: srcConsumeInspiration(),
    coverage: ['advantage_inspiration'],
    note: '激励用掉即消失（一次检定）'
  })
  add({
    id: 'dnd-surprise',
    mount: 'check_post_roll',
    source: srcSurprise(),
    coverage: ['surprise_first_round'],
    note: '敏捷（隐匿）检定失败 → 受突袭 + 战斗第一轮标记'
  })
  add({
    id: 'dnd-save-ends',
    mount: 'status_tick',
    source: srcSaveEnds(),
    coverage: ['save_ends'],
    note: '每回合末重复豁免，成功即终止状态'
  })
  add({
    id: 'dnd-battle-round',
    mount: 'turn_end',
    source: srcBattleRound(),
    coverage: ['surprise_first_round'],
    when: { op: 'flag_set', flag: 'dnd-in-combat' },
    note: '战斗轮次计数 + 第 1 轮标记'
  })

  for (const table of tables) {
    const gate = {
      op: 'all_of',
      children: [
        {
          op: 'any_of',
          children: [
            { op: 'at_location', location_id: table.locationId },
            { op: 'flag_set', flag: 'dnd-in-wilderness' }
          ]
        },
        table.night
          ? { op: 'flag_set', flag: 'dnd-night' }
          : { op: 'not', child: { op: 'flag_set', flag: 'dnd-night' } }
      ]
    }
    add({
      id: 'dnd-wander-' + table.key,
      mount: 'turn_end',
      source: srcWanderTable(table),
      coverage: ['encounter_table'],
      when: gate,
      note: table.label + '：d20 ≥ 17 触发，d12 查表（置行标记，触发点 repeatable）'
    })
  }

  // 掷表行标记的复位（GAP-N / GAP-H）：遭遇结束时清标记，同一表项才能再次触发。
  add({
    id: 'dnd-wander-reset',
    mount: 'event',
    source: srcWanderReset(tables),
    coverage: ['encounter_table'],
    note: 'encounter_cleared → clear_flag：把掷表行标记复位，触发点 repeatable 的边沿才成立'
  })

  // XP（GAP-F）：逐只按 enemy_defeated 发，与遭遇来源无关。
  add({
    id: 'dnd-xp-award',
    mount: 'event',
    source: srcXpAward(),
    coverage: ['xp_total'],
    note: 'enemy_defeated → 按 data.enemy.template_id 查开放内容 dnd-xp-award 定义发 XP（导演即兴遭遇同样覆盖）'
  })

  return { rules, tables }
}

function buildOpenContent(draft, tables) {
  const monsters = indexMonsters(draft)
  const kinds = RULE_KINDS.slice()
  const definitions = []

  const ruleMeta = {
    'dnd-status-keep-high': ['check_pre_roll', '优势（激励）'],
    'dnd-status-keep-low': ['check_pre_roll', '劣势（灰烬呛咳）'],
    'dnd-xp-award': ['event', '经验值合计（每个被击败的怪物一条）'],
    'dnd-wander-reset': ['event', '掷表标记复位（遭遇结束清标记）'],
    'dnd-sunlight-sensitivity': ['check_pre_roll', '劣势（日照敏感）'],
    'dnd-pack-tactics': ['check_pre_roll', '优势（集群战术）'],
    'dnd-ambusher-keep-high': ['check_pre_roll', '优势（伏击，战斗第一轮）'],
    'dnd-skill-dc': ['check_pre_roll', '技能声明的豁免 DC'],
    'dnd-proficiency': ['check_post_roll', '熟练加值'],
    'dnd-save-half': ['check_post_roll', '豁免成功伤害减半'],
    'dnd-consume-inspiration': ['check_post_roll', '激励用掉即消失'],
    'dnd-surprise': ['check_post_roll', '突袭'],
    'dnd-save-ends': ['status_tick', '重复豁免'],
    'dnd-battle-round': ['turn_end', '战斗轮次'],
    'dnd-wander-day': ['turn_end', '掷表遭遇（白昼）'],
    'dnd-wander-night': ['turn_end', '掷表遭遇（黑夜）']
  }
  for (const id of Object.keys(ruleMeta)) {
    definitions.push({
      id: 'rule-' + id,
      kind: 'dnd-rule',
      name: '规则 · ' + ruleMeta[id][1],
      description: 'lua_mounts:' + id + '（挂载点 ' + ruleMeta[id][0] + '）',
      fields: {
        mount: ruleMeta[id][0],
        coverage: ruleMeta[id][1],
        note: '规则语义全在 Lua；引擎只提供通用原语。'
      }
    })
  }

  for (const pcId of Object.keys(RULEPACK.proficiency)) {
    for (const attr of Object.keys(RULEPACK.proficiency[pcId])) {
      const p = RULEPACK.proficiency[pcId][attr]
      definitions.push({
        id: 'prof-' + pcId + '-' + attr,
        kind: 'dnd-proficiency',
        name: '熟练 · ' + p.label,
        description: p.label + ' 检定 +' + p.bonus,
        fields: {
          character_id: pcId,
          check_attribute: attr,
          bonus: String(p.bonus),
          note: 'lua_mounts:dnd-proficiency 在 check_post_roll 追加（引擎只在掷骰后给判定属性）。'
        }
      })
    }
  }

  for (const id of Object.keys(RULEPACK.sunlight_sensitive)) {
    definitions.push({
      id: 'sunlight-' + id.replace(/^mon-/, ''),
      kind: 'dnd-sunlight-sensitivity',
      name: '日照敏感 · ' + RULEPACK.sunlight_sensitive[id],
      description: '阳光下攻击检定劣势，依赖视力的感知（察觉）检定也劣势（按判定签名区分）',
      fields: {
        creature_id: id,
        signature_scope: 'kind == attack ∨ (kind == attribute ∧ attribute == wis)',
        note:
          '附录 B 原文：在阳光下，该生物在攻击骰上处于劣势，并且在依赖视力的感知（察觉）检定上也处于劣势。'
          + '两条分支都由 check_pre_roll 的判定签名区分；其它判定不受影响。'
      }
    })
  }

  for (const id of Object.keys(RULEPACK.pack_tactics)) {
    definitions.push({
      id: 'pack-' + id.replace(/^mon-/, '').replace(/-/g, ''),
      kind: 'dnd-pack-tactics',
      name: '集群战术 · ' + RULEPACK.pack_tactics[id],
      description: '至少一个未失能的盟友在目标 5 尺内时，攻击检定有优势',
      fields: {
        creature_id: id,
        ally_proxy: '同一场活跃遭遇里的其它敌人（HP>0 视为未失能）',
        range_proxy: '与目标同一 location_id',
        note:
          'GAP-A 已闭合：规则用 host.list_encounters + host.get_character 读同遭遇同伴，不再靠 dnd-flanked 标记；'
          + '但精确「目标 5 尺内」仍做不到（GAP-B：引擎无位置 / 距离），HP>0 只是「未失能」的近似。'
          + '「哪只生物有集群战术」由本定义驱动（Lua 读模板挂接，不烘名单）。'
      }
    })
  }

  for (const id of Object.keys(RULEPACK.ambusher)) {
    definitions.push({
      id: 'ambush-' + id.replace(/^mon-/, ''),
      kind: 'dnd-ambusher',
      name: '伏击 · ' + RULEPACK.ambusher[id],
      description: '战斗第一轮对受其突袭的生物的攻击检定有优势',
      fields: {
        creature_id: id,
        target_status: 'dnd-surprised',
        note:
          '附录 B 原文：战斗开始的第一轮里，该生物对任何成功受其突袭的生物所发动的攻击检定具有优势。'
          + 'GAP-L 已闭合：Lua 直接读 host.target.statuses 里有没有 target_status 这个状态，不用全场标记近似。'
          + '「哪只生物有伏击」由本定义驱动（Lua 读模板挂接，不烘名单）。'
      }
    })
  }

  for (const id of Object.keys(RULEPACK.save_ends)) {
    const r = RULEPACK.save_ends[id]
    definitions.push({
      id: 'saveends-' + id.replace(/^dnd-/, ''),
      kind: 'dnd-save-ends',
      name: '重复豁免 · ' + id,
      description: '每回合末 DC ' + r.dc + ' ' + r.attribute + ' 豁免，成功终止',
      fields: {
        status_id: id,
        dc: String(r.dc),
        attribute: r.attribute,
        note: 'lua_mounts:dnd-save-ends 在 status_tick 掷骰并 remove_status。'
      }
    })
  }

  definitions.push({
    id: 'savehalf-rubble',
    kind: 'dnd-save-half',
    name: '减半 · 坠落瓦砾',
    description: 'DC 10 敏捷豁免：失败 3d6 钝击 + 倒地，成功减半',
    fields: {
      skill_id: 'sk-lmop-rubble-collapse',
      fail_status: 'dnd-prone',
      note:
        'lua_mounts:dnd-save-half 在 check_post_roll 声明 host.scale_effect(0.5)（GAP-E 已闭合）：'
        + '引擎照常结算一次、只掷一次效果骰，数值型 delta 按 0.5 向零取整；'
        + '失败才附加的状态（fail_status）由同一脚本在失败分支补——缩放不覆盖状态。'
    }
  })

  for (const table of tables) {
    definitions.push({
      id: 'rule-dnd-wander-' + table.key,
      kind: 'dnd-rule',
      name: '规则 · 掷表遭遇（' + table.label + '）',
      description: 'lua_mounts:dnd-wander-' + table.key,
      fields: {
        mount: 'turn_end',
        coverage: '掷表遭遇（d20 触发 + d12 查表）',
        note: '表本身只活在 Lua 里；引擎不认识「遭遇表」。'
      }
    })
    definitions.push({
      id: 'table-lmop-' + table.key,
      kind: 'dnd-encounter-table',
      name: '遭遇表 · ' + table.label,
      description: 'd20 ≥ 17 触发，d12 查表',
      fields: {
        table: table.label,
        trigger_die: '1d20，17-20 触发',
        row_die: '1d12',
        rows: table.rolls
          .map(r => r.die + ' → ' + r.monsterName + '（原表 ' + r.dice + '，本规则包固定 ' + r.count + '）')
          .join('\n'),
        note:
          '触发点 repeatable，遭遇结束时由 lua_mounts:dnd-wander-reset 清行标记复位——同一表项可反复出。'
          + '仍存在的缺口：GAP-G（预置遭遇的数量是静态整数，骰式数量只能取骰式下界）。'
      }
    })
  }

  // XP 表（GAP-F 闭合）：逐模板一条定义，Lua 在 enemy_defeated 时按 template_id 反查。
  // 数值来源是 T8 图鉴草稿的 statblock.xp，**不另抄一份**；改图鉴 → 定义跟着变 → 发放跟着变。
  const xpTable = draft.characters
    .filter(c => c.kind === 'monster')
    .map(c => c.name + '=' + ((c.statblock && c.statblock.xp) || 0))
    .join('；')
  definitions.push({
    id: 'xp-lmop-table',
    kind: 'dnd-xp-award',
    name: '经验值合计（按模板）',
    description: '模板 XP 取自 T8 图鉴的 statblock.xp，不另抄一份',
    fields: {
      formula: '合计 = Σ(模板 statblock.xp × 遭遇内该模板数量)',
      table: xpTable,
      note:
        'lua_mounts:dnd-xp-award 在 event: enemy_defeated 时按 data.enemy.template_id 发放；'
        + '导演出即兴遭遇也一样。逐模板值见同 kind 的 xp-<模板 id> 定义。'
    }
  })
  for (const m of draft.characters.filter(c => c.kind === 'monster')) {
    definitions.push({
      id: 'xp-' + m.id,
      kind: 'dnd-xp-award',
      name: 'XP · ' + m.name,
      description: m.name + ' 被击败时发放 ' + ((m.statblock && m.statblock.xp) || 0) + ' 点经验值',
      fields: {
        template_id: m.id,
        creature_name: m.name,
        xp: String((m.statblock && m.statblock.xp) || 0),
        note: 'lua_mounts:dnd-xp-award 在 enemy_defeated 时按 fields.template_id 命中本条并 modify_resource。'
      }
    })
  }

  for (const t of ['钝击', '穿刺', '毒素', '光耀', '黯蚀']) {
    definitions.push({
      id: 'damage-' + (t === '钝击' ? 'bludgeoning' : t === '穿刺' ? 'piercing' : t === '毒素' ? 'poison' : t === '光耀' ? 'radiant' : 'necrotic'),
      kind: 'dnd-damage-type',
      name: '伤害类型 · ' + t,
      description: '作者词汇（引擎不认识伤害类型，见 GAP：伤害无类型）',
      fields: {
        damage_type: t,
        note: '引擎只有「资源扣减」这一个动作；免疫 / 抗性由故事书文本与叙事层承担。'
      }
    })
  }

  definitions.push({
    id: 'battle-round-lmop',
    kind: 'dnd-battle-round',
    name: '战斗轮次（LMoP）',
    description: 'dnd-in-combat 标记开闸，turn_end 自增，清 1..3 后置当前轮',
    fields: {
      note:
        '每轮先 clear_flag(dnd-battle-round-1..3) 再置当前轮：任何时刻至多一个轮次标记为真。'
        + '仍存在的缺口：GAP-M（没有「有遭遇」条件，靠 dnd-in-combat 标记开闸）/ GAP-I（没有先攻与行动经济）。'
    }
  })

  return { kinds, definitions }
}

function buildPc() {
  return {
    id: 'pc-lmop-talin',
    name: '塔林·银溪',
    kind: 'pc',
    background:
      '凡达林长大的半精灵，替商队跑腿时学会了在树线后走路、在酒馆里听墙角。'
      + '刚铎·寻岩者雇她护送那车补给去凡达林，她答应了——现在刚铎没到，地精箭矢倒是先到了。',
    personality: '务实、嘴硬、不轻易相信人，但会为一个承诺走到黑。',
    example_dialogues:
      '「刚铎付了定金。定金没退，我就还在路上。」\n'
      + '「别问我为什么蹲着走路。你先看看地上那串脚印，有三趾的。」\n'
      + '「我不是英雄。我只是不太喜欢半路把话说一半的人。」',
    attributes: { str: 10, dex: 16, con: 14, int: 12, wis: 13, cha: 11 },
    resources: { 'res-hp': 24, 'res-insp': 1, 'res-xp': 0 },
    location_id: 'loc-lmop-040',
    skills: ['sk-lmop-stealth', 'sk-lmop-perception', 'sk-lmop-shortsword', 'sk-lmop-inspiration'],
    attachments: { 'monster-armor': ['armor-lmop-talin-leather'] },
    notes:
      'LMoP 规则包开局 PC（3 级游荡者取向）：HP 24 = 3d8+6 取平均；AC 14 = 10 + 敏捷调整值 +3 + 皮甲 +1（挂接修正）；'
      + '六维取自 5e 标准数组。常驻地：凡达林。'
  }
}

function buildPcArmor() {
  return {
    id: 'armor-lmop-talin-leather',
    kind: 'monster-armor',
    name: '塔林 · 皮甲',
    description: '皮甲：AC = 10 + 敏捷调整值 3 + 护甲修正 1 = 14',
    fields: {
      source: '皮甲（12 AC = 11 + 敏捷调整值）',
      note: 'AC 由 derived.ac（10 + 敏捷调整值）加本挂接修正得到'
    },
    modifiers: [{ target: 'ac', value: 1, op: 'add' }]
  }
}

/** 规则挂接：把「哪只生物有什么特性」做成角色模板的 attachments（开放内容 → Lua 的唯一入口）。
 *
 *  GAP-C 闭合的关键：规则脚本运行时用 host.get_attachment(kind) + host.get_definition(id)
 *  读这里的引用，所以加值 / 名单 / 范围都在**数据**里，不在 Lua 里。
 *  挂接只**新增**规则包自己的 kind，不动 T8 图鉴写下的 monster-variant / monster-armor。 */
function buildRuleAttachments() {
  const map = new Map()
  const link = (charId, kind, defId) => {
    if (!map.has(charId)) map.set(charId, {})
    const kinds = map.get(charId)
    if (!kinds[kind]) kinds[kind] = []
    if (!kinds[kind].includes(defId)) kinds[kind].push(defId)
  }
  for (const pcId of Object.keys(RULEPACK.proficiency)) {
    for (const attr of Object.keys(RULEPACK.proficiency[pcId])) {
      link(pcId, 'dnd-proficiency', 'prof-' + pcId + '-' + attr)
    }
  }
  for (const id of Object.keys(RULEPACK.sunlight_sensitive)) {
    link(id, 'dnd-sunlight-sensitivity', 'sunlight-' + id.replace(/^mon-/, ''))
  }
  for (const id of Object.keys(RULEPACK.pack_tactics)) {
    link(id, 'dnd-pack-tactics', 'pack-' + id.replace(/^mon-/, '').replace(/-/g, ''))
  }
  for (const id of Object.keys(RULEPACK.ambusher)) {
    link(id, 'dnd-ambusher', 'ambush-' + id.replace(/^mon-/, ''))
  }
  return map
}

function buildRuleFlags(tables) {
  const flags = RULE_FLAGS.slice()
  for (const table of tables) {
    for (const roll of table.rolls) {
      flags.push({
        key: roll.flag,
        label: table.label + ' d12=' + roll.die + '：' + roll.monsterName + ' ×' + roll.count
      })
    }
  }
  return flags
}

function pickTriggerScene(draft, sceneId) {
  for (const chapter of draft.skeleton) {
    for (const scene of chapter.scenes) {
      if (scene.id === sceneId) return scene
    }
  }
  return null
}

function buildStorybook(draft) {
  const book = JSON.parse(JSON.stringify(draft))
  const { rules, tables } = buildRules(book)
  const { kinds, definitions } = buildOpenContent(book, tables)

  // --- lua_mounts：规则包的交付形态（docs/rules-via-lua.md §4 ①）---
  book.lua_mounts = rules.map(r => {
    const out = { id: r.id, mount: r.mount, source: r.source }
    if (r.when) out.when = r.when
    return out
  })

  // --- 规则包新增技能 ---
  const skillIds = new Set(book.skills.map(s => s.id))
  for (const skill of RULE_SKILLS) {
    if (!skillIds.has(skill.id)) {
      book.skills.push(JSON.parse(JSON.stringify(skill)))
      skillIds.add(skill.id)
    }
  }
  // 灰烬丧尸使用「灰烬喷发」这条豁免技能（模块第 2541 行原文：使用丧尸资料卡 + 额外特性）。
  const ashZombie = book.characters.find(c => c.id === 'mon-ash-zombie')
  if (ashZombie && !ashZombie.skills.includes('sk-lmop-ash-eruption')) {
    ashZombie.skills = ashZombie.skills.concat(['sk-lmop-ash-eruption'])
  }

  // --- 规则包新增状态 / 资源 / 标记 / 开放内容 ---
  const statusIds = new Set(book.statuses.map(s => s.id))
  for (const status of RULE_STATUSES) {
    if (!statusIds.has(status.id)) {
      book.statuses.push(JSON.parse(JSON.stringify(status)))
      statusIds.add(status.id)
    }
  }
  const resourceIds = new Set(book.world.resources.map(r => r.id))
  if (!resourceIds.has('res-insp')) {
    book.world.resources.push({ id: 'res-insp', name: '激励', type: 'numerical', default_max: 1 })
  }
  const flagKeys = new Set(book.flags.map(f => f.key))
  for (const flag of buildRuleFlags(tables)) {
    if (!flagKeys.has(flag.key)) {
      book.flags.push(flag)
      flagKeys.add(flag.key)
    }
  }
  const kindKeys = new Set(book.kinds.map(k => k.key))
  for (const kind of kinds) {
    if (!kindKeys.has(kind.key)) {
      book.kinds.push(kind)
      kindKeys.add(kind.key)
    }
  }
  const defIds = new Set(book.definitions.map(d => d.id))
  for (const def of definitions) {
    if (!defIds.has(def.id)) {
      book.definitions.push(def)
      defIds.add(def.id)
    }
  }
  book.definitions.push(buildPcArmor())
  book.definitions.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))

  // --- 开局 PC（没有受控角色就没法试玩）---
  if (!book.characters.some(c => c.id === 'pc-lmop-talin')) {
    book.characters.push(buildPc())
  }

  // --- 规则挂接：把规则数据挂到角色模板上（GAP-C：Lua 只读得到挂接与定义）---
  for (const [charId, kinds] of buildRuleAttachments()) {
    const character = book.characters.find(c => c.id === charId)
    if (!character) throw new Error('规则挂接引用了不存在的角色 ' + charId)
    if (!character.attachments) character.attachments = {}
    for (const kind of Object.keys(kinds)) {
      const existing = character.attachments[kind] || []
      const merged = existing.slice()
      for (const id of kinds[kind]) if (!merged.includes(id)) merged.push(id)
      character.attachments[kind] = merged
    }
  }
  const missingAttachmentRefs = []
  const defIdSet = new Set(book.definitions.map(d => d.id))
  for (const character of book.characters) {
    for (const kind of Object.keys(character.attachments || {})) {
      for (const id of character.attachments[kind]) {
        if (!defIdSet.has(id)) missingAttachmentRefs.push(character.id + '.' + kind + ' → ' + id)
      }
    }
  }
  if (missingAttachmentRefs.length) {
    throw new Error('规则挂接引用了未声明的定义：' + missingAttachmentRefs.join(' ; '))
  }

  // --- 卡面分区：让规则包词汇在编辑器里可见 ---
  book.sheet.push({
    title: '规则包（D&D 5e）',
    kinds: RULE_KINDS.filter(k => k.key !== 'dnd-rule').map(k => k.key),
    resources: ['res-insp', 'res-xp']
  })

  // --- 掷表遭遇的触发点（condition: flag_set + encounter 预置）---
  for (const table of tables) {
    const scene = pickTriggerScene(book, table.sceneId)
    if (!scene) throw new Error('骨架里找不到场景 ' + table.sceneId)
    if (!Array.isArray(scene.triggers)) scene.triggers = []
    for (const roll of table.rolls) {
      scene.triggers.push({
        id: 'tr-lmop-wander-' + table.key + '-' + roll.die,
        title: table.label + ' d12=' + roll.die + '：' + roll.monsterName,
        hint:
          '规则包掷表：d20 ≥ 17 触发，d12=' + roll.die + ' → ' + roll.monsterName
          + '（原表数量 ' + roll.dice + '，本规则包固定 ' + roll.count + '）。'
          + '可重复：行标记在遭遇结束时复位（dnd-wander-reset）。',
        // 可重复触发点（GAP-N 闭合）：条件由假变真（边沿）时再触发。
        repeatable: true,
        condition: { op: 'flag_set', flag: roll.flag },
        encounter: {
          // 名字含 d12 行号且在表项间唯一：dnd-wander-reset 用它精确反查该行的标记。
          name: encounterName(table, roll),
          note: '规则包预置遭遇 · 原表 ' + roll.dice + ' → 固定 ' + roll.count,
          location_id: table.locationId,
          enemies: [{ template_id: roll.monsterId, count: roll.count }]
        }
      })
    }
  }

  return { book, rules, tables }
}

// ---------------------------------------------------------------------------
// 5. 自检
// ---------------------------------------------------------------------------

function* walk(value, pathStr) {
  yield { value, path: pathStr }
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) yield* walk(value[i], pathStr + '[' + i + ']')
  } else if (value && typeof value === 'object') {
    for (const key of Object.keys(value)) yield* walk(value[key], pathStr + '.' + key)
  }
}

function deepScanRetracted(book) {
  const hits = []
  for (const { value, path: p } of walk(book, '$')) {
    if (typeof value === 'string') {
      const lower = value.toLowerCase()
      for (const token of RETRACTED_TOKENS) {
        if (lower.includes(token.toLowerCase())) hits.push(p + ' ← ' + token)
      }
    } else if (value && typeof value === 'object' && !Array.isArray(value)) {
      for (const key of Object.keys(value)) {
        for (const token of RETRACTED_TOKENS) {
          if (key.toLowerCase().includes(token.toLowerCase())) hits.push(p + '.' + key + ' ← ' + token)
        }
      }
    }
  }
  return hits
}

function checkLuaSources(book) {
  const problems = []
  const seen = new Set()
  for (const mount of book.lua_mounts || []) {
    if (!mount.id || seen.has(mount.id)) problems.push('lua_mounts id 缺失或重复：' + JSON.stringify(mount.id))
    seen.add(mount.id)
    if (!DECLARABLE_MOUNTS.includes(mount.mount)) {
      problems.push(mount.id + ' 用了不可声明的挂载点 ' + mount.mount)
    }
    if (!mount.source || !mount.source.trim()) problems.push(mount.id + ' source 为空')
    for (const token of FORBIDDEN_LUA) {
      if (mount.source.includes(token)) problems.push(mount.id + ' 用了 lua_lint 禁用 API：' + token)
    }
    for (const call of mount.source.matchAll(/host\.([A-Za-z_][A-Za-z0-9_]*)/g)) {
      if (!HOST_API.includes(call[1])) problems.push(mount.id + ' 调用了不存在的 host API：host.' + call[1])
    }
    for (const token of RETRACTED_TOKENS) {
      if (mount.source.toLowerCase().includes(token.toLowerCase())) {
        problems.push(mount.id + ' 源码里出现撤回词：' + token)
      }
    }
  }
  return problems
}

function runChecks(build) {
  const { book, rules, tables } = build
  const results = []
  const ok = (name, detail) => results.push({ pass: true, name, detail })
  const fail = (name, detail) => results.push({ pass: false, name, detail })
  const expect = (cond, name, detail) => (cond ? ok(name, detail) : fail(name, detail))

  // 1. 规则包覆盖（八条能力）
  const ruleIds = new Set(rules.map(r => r.id))
  const defIds = new Set(book.definitions.map(d => d.id))
  for (const item of COVERAGE) {
    const missingRules = (item.rules || []).filter(id => !ruleIds.has(id))
      .concat((item.rulePrefixes || []).filter(p => ![...ruleIds].some(id => id.startsWith(p))).map(p => p + '*'))
    const missingDefs = item.definitions.filter(id => !defIds.has(id))
    expect(
      missingRules.length === 0 && missingDefs.length === 0,
      'coverage.' + item.key,
      item.label + ' → 规则 ' + ((item.rules || []).length + (item.rulePrefixes || []).length) + ' 组 / 开放内容 ' + item.definitions.length + ' 条'
        + (missingRules.length ? ' 缺规则:' + missingRules.join(',') : '')
        + (missingDefs.length ? ' 缺定义:' + missingDefs.join(',') : '')
    )
  }

  // 2. 开局 PC
  const pc = book.characters.find(c => c.kind === 'pc')
  expect(!!pc, 'pc.exists', pc ? pc.id + ' / ' + pc.name : '没有 kind=pc 的角色（开档后没有受控角色）')
  if (pc) {
    const attrs = Object.keys(pc.attributes || {})
    const dims = book.attribute_dimensions.map(d => d.key)
    expect(
      dims.every(k => attrs.includes(k)),
      'pc.attributes',
      '六维 ' + dims.map(k => k + '=' + pc.attributes[k]).join(' ')
    )
    expect(
      typeof pc.resources['res-hp'] === 'number' && pc.resources['res-hp'] > 0,
      'pc.hp',
      'res-hp=' + pc.resources['res-hp'] + '（LMoP 用 res-hp，不是 hp）'
    )
    const skillIds = new Set(book.skills.map(s => s.id))
    expect(
      Array.isArray(pc.skills) && pc.skills.length > 0 && pc.skills.every(id => skillIds.has(id)),
      'pc.skills',
      (pc.skills || []).join('、')
    )
    const locIds = new Set(book.world.locations.map(l => l.id))
    expect(
      typeof pc.location_id === 'string' && locIds.has(pc.location_id),
      'pc.home',
      'location_id=' + pc.location_id
    )
  }

  // 3. deep-scan 撤回字段
  const hits = deepScanRetracted(book)
  expect(
    hits.length === 0,
    'no_retracted_fields',
    hits.length === 0
      ? '整本故事书（键 / 文本 / Lua 源码）零命中：' + RETRACTED_TOKENS.join(' / ')
      : hits.slice(0, 8).join(' ; ')
  )

  // 4. lua_mounts 过预检（形状 + 白名单 + 禁用 API + host API 存在性）
  const luaProblems = checkLuaSources(book)
  expect(
    luaProblems.length === 0,
    'lua_mounts.lint',
    luaProblems.length === 0
      ? (book.lua_mounts || []).length + ' 条脚本；挂载点名 / source / host API / 禁用 API 全部通过'
      : luaProblems.slice(0, 6).join(' ; ')
  )

  // 5. 图鉴数据不复制：T8 条目的数值逐字未改
  //    attachments 是唯一的放宽点：规则包可以**追加自己的 kind**（开放内容 → Lua 的唯一入口），
  //    但 T8 写的每个 kind 与每条引用必须逐字保留——图鉴数据本身一个字节也不许改。
  const RULEPACK_ATTACHMENT_KINDS = ['dnd-proficiency', 'dnd-sunlight-sensitivity', 'dnd-pack-tactics', 'dnd-ambusher']
  const byId = new Map(book.characters.map(c => [c.id, c]))
  const drift = []
  let addedAttachments = 0
  for (const m of build.draft.characters.filter(c => c.kind === 'monster')) {
    const now = byId.get(m.id)
    if (!now) { drift.push(m.id + ' 丢失'); continue }
    const sameData = JSON.stringify(now.attributes) === JSON.stringify(m.attributes)
      && JSON.stringify(now.resources) === JSON.stringify(m.resources)
      && JSON.stringify(now.statblock) === JSON.stringify(m.statblock)
    if (!sameData) drift.push(m.id + ' 数据被改动')
    // 图鉴自带的挂接必须原样保留（1:1 前缀，且不许多出 T8 之外的键）。
    for (const kind of Object.keys(m.attachments || {})) {
      if (JSON.stringify((now.attachments || {})[kind]) !== JSON.stringify(m.attachments[kind])) {
        drift.push(m.id + ' 的图鉴挂接 ' + kind + ' 被改动')
      }
    }
    for (const kind of Object.keys(now.attachments || {})) {
      if (m.attachments && kind in m.attachments) continue
      if (!RULEPACK_ATTACHMENT_KINDS.includes(kind)) drift.push(m.id + ' 多出来路不明的挂接 ' + kind)
      else addedAttachments++
    }
    const added = now.skills.filter(s => !m.skills.includes(s))
    if (added.length && !(m.id === 'mon-ash-zombie' && added.length === 1 && added[0] === 'sk-lmop-ash-eruption')) {
      drift.push(m.id + ' 技能被改动：' + added.join(','))
    }
  }
  expect(
    drift.length === 0,
    'bestiary.untouched',
    drift.length === 0
      ? build.draft.characters.filter(c => c.kind === 'monster').length + ' 条图鉴数值与 T8 草稿逐字一致'
        + '（只追加 1 条豁免技能 + ' + addedAttachments + ' 个规则挂接 kind）'
      : drift.join(' ; ')
  )

  // 6. 掷表链路：Lua → set_flag → trigger(condition flag_set + encounter 预置)
  const monsterIds = new Set(book.characters.filter(c => c.kind === 'monster').map(c => c.id))
  const triggerById = new Map()
  for (const chapter of book.skeleton) {
    for (const scene of chapter.scenes) {
      for (const trigger of scene.triggers || []) triggerById.set(trigger.id, { trigger, scene })
    }
  }
  let tableProblems = []
  for (const table of tables) {
    const wander = book.lua_mounts.find(m => m.id === 'dnd-wander-' + table.key)
    if (!wander || !wander.source.includes('host.engine_rng(1, 20)') || !wander.source.includes('host.engine_rng(1, 12)')) {
      tableProblems.push(table.key + ' 缺少 d20 触发 / d12 查表')
    }
    for (const roll of table.rolls) {
      const hit = triggerById.get('tr-lmop-wander-' + table.key + '-' + roll.die)
      if (!hit) { tableProblems.push(roll.flag + ' 没有对应触发点'); continue }
      const enc = hit.trigger.encounter
      if (!enc || !Array.isArray(enc.enemies) || enc.enemies.length === 0) {
        tableProblems.push(roll.flag + ' 触发点没有 encounter 预置')
        continue
      }
      const enemy = enc.enemies[0]
      if (!monsterIds.has(enemy.template_id)) tableProblems.push(roll.flag + ' 的 template_id 不是图鉴条目')
      if (enemy.count !== roll.count) tableProblems.push(roll.flag + ' 数量与表不一致')
      if (hit.trigger.condition.op !== 'flag_set' || hit.trigger.condition.flag !== roll.flag) {
        tableProblems.push(roll.flag + ' 触发条件不是 flag_set')
      }
      // 可重复（GAP-N）：触发点必须显式标 repeatable: true，否则整局只出一次。
      if (hit.trigger.repeatable !== true) tableProblems.push(roll.flag + ' 触发点没有 repeatable: true')
      // 复位（GAP-H）：行标记必须能被清掉，否则 repeatable 永远等不到「假 → 真」边沿。
      const reset = book.lua_mounts.find(m => m.id === 'dnd-wander-reset')
      if (!reset || !reset.source.includes("host.clear_flag(flag)") || !reset.source.includes(encounterName(table, roll))) {
        tableProblems.push(roll.flag + ' 没有对应的 clear_flag 复位')
      }
    }
  }
  // XP 不再按行回补，改由 enemy_defeated 逐只发放（GAP-F）。
  const xpMount = book.lua_mounts.find(m => m.id === 'dnd-xp-award')
  if (!xpMount || xpMount.mount !== 'event') tableProblems.push('XP 规则没有挂在 event 挂载点')
  if (xpMount && !xpMount.source.includes("host.event_name ~= 'enemy_defeated'")) {
    tableProblems.push('XP 规则没有按 enemy_defeated 开闸')
  }
  if (book.lua_mounts.some(m => /^dnd-xp-/.test(m.id) && m.id !== 'dnd-xp-award')) {
    tableProblems.push('仍残留按掷表行回补的 XP 规则（应全部改走 enemy_defeated）')
  }
  expect(
    tableProblems.length === 0,
    'encounter_table.chain',
    tableProblems.length === 0
      ? tables.length + ' 张表 / ' + tables.reduce((n, t) => n + t.rolls.length, 0)
        + ' 行：Lua 掷骰 → set_flag → repeatable 触发点（flag_set + encounter 预置）'
        + ' → 回合末边沿建遭遇 → 遭遇结束 clear_flag 复位 → 可反复出；XP 走 enemy_defeated 逐只发'
      : tableProblems.slice(0, 6).join(' ; ')
  )

  // 7. 开放内容引用自洽（status / skill / resource / kind / flag）
  const problems = []
  const bookSkillIds = new Set(book.skills.map(s => s.id))
  const bookStatusIds = new Set(book.statuses.map(s => s.id))
  const bookResourceIds = new Set(book.world.resources.map(r => r.id))
  const bookKindKeys = new Set(book.kinds.map(k => k.key))
  const bookFlagKeys = new Set(book.flags.map(f => f.key))
  const bookLocIds = new Set(book.world.locations.map(l => l.id))
  for (const skill of book.skills) {
    if (!bookSkillIds.has(skill.id)) problems.push('skill ' + skill.id)
    for (const st of (skill.effect && skill.effect.status) || []) {
      if (!bookStatusIds.has(st)) problems.push('技能 ' + skill.id + ' 引用了未声明状态 ' + st)
    }
    for (const imm of (skill.effect && skill.effect.immediate) || []) {
      if (imm.resource && !bookResourceIds.has(imm.resource)) {
        problems.push('技能 ' + skill.id + ' 引用了未声明资源 ' + imm.resource)
      }
    }
    for (const cost of skill.cost || []) {
      if (!bookResourceIds.has(cost.resource)) problems.push('技能 ' + skill.id + ' 消耗了未声明资源 ' + cost.resource)
    }
  }
  for (const c of book.characters) {
    for (const sid of c.skills || []) {
      if (!bookSkillIds.has(sid)) problems.push('角色 ' + c.id + ' 引用了未声明技能 ' + sid)
    }
    for (const kind of Object.keys(c.attachments || {})) {
      if (!bookKindKeys.has(kind)) problems.push('角色 ' + c.id + ' 挂接了未声明种类 ' + kind)
    }
  }
  for (const def of book.definitions) {
    if (!bookKindKeys.has(def.kind)) problems.push('定义 ' + def.id + ' 的种类未声明 ' + def.kind)
  }
  for (const m of book.lua_mounts) {
    const flags = JSON.stringify(m.when || {}).match(/"flag":"([^"]+)"/g) || []
    for (const f of flags) {
      const key = f.split(':')[1].replace(/"/g, '')
      if (!bookFlagKeys.has(key)) problems.push('规则 ' + m.id + ' 的 when 引用了未声明标记 ' + key)
    }
  }
  for (const chapter of book.skeleton) {
    for (const scene of chapter.scenes) {
      for (const trigger of scene.triggers || []) {
        const flag = trigger.condition && trigger.condition.flag
        if (flag && !bookFlagKeys.has(flag)) problems.push('触发点 ' + trigger.id + ' 引用了未声明标记 ' + flag)
        const loc = trigger.encounter && trigger.encounter.location_id
        if (loc && !bookLocIds.has(loc)) problems.push('触发点 ' + trigger.id + ' 引用了未声明地点 ' + loc)
      }
    }
  }
  expect(
    problems.length === 0,
    'references.self_consistent',
    problems.length === 0
      ? '技能 / 状态 / 资源 / 种类 / 标记 / 地点引用全部命中'
      : problems.slice(0, 6).join(' ; ')
  )

  // 8. 数据驱动（GAP-C 闭合的验收 1）：规则数值只活在开放内容里，Lua 里没有烘死的常量
  const srcOf = id => (book.lua_mounts.find(m => m.id === id) || {}).source || ''
  const dataDriven = [
    {
      id: 'dnd-proficiency',
      reads: ["host.get_attachment('dnd-proficiency')", 'host.get_definition('],
      forbids: ['pc-lmop-talin']
    },
    {
      id: 'dnd-sunlight-sensitivity',
      reads: ["host.get_attachment('dnd-sunlight-sensitivity')"],
      forbids: ['mon-mormesk']
    },
    {
      id: 'dnd-pack-tactics',
      reads: ["host.get_attachment('dnd-pack-tactics')", 'host.list_encounters', 'host.get_character'],
      forbids: ['mon-wolf', 'dnd-flanked']
    },
    {
      id: 'dnd-ambusher-keep-high',
      reads: ["host.get_attachment('dnd-ambusher')", 'host.target', 'host.get_definition('],
      forbids: ['mon-doppelganger', 'dnd-surprised']
    },
    {
      id: 'dnd-save-ends',
      reads: ["host.list_definitions('dnd-save-ends')"],
      forbids: ['dnd-eruption-penalty', 'dnd-poisoned']
    },
    {
      id: 'dnd-xp-award',
      reads: ["host.list_definitions('dnd-xp-award')", 'host.event_data', 'host.get_encounter'],
      forbids: ['mon-']
    },
    {
      id: 'dnd-save-half',
      reads: ["host.list_definitions('dnd-save-half')", 'host.scale_effect(0.5)'],
      forbids: ['savehalf-rubble', 'host.engine_rng']
    }
  ]
  const dataDrivenProblems = []
  for (const item of dataDriven) {
    const source = srcOf(item.id)
    if (!source) { dataDrivenProblems.push(item.id + ' 脚本缺失'); continue }
    for (const needle of item.reads) {
      if (!source.includes(needle)) dataDrivenProblems.push(item.id + ' 没有从开放内容读：' + needle)
    }
    for (const needle of item.forbids) {
      if (source.includes(needle)) dataDrivenProblems.push(item.id + ' 源码里烘死了标识：' + needle)
    }
  }
  // 加值本身也不许出现在 Lua 里：definition.fields.bonus 是唯一事实来源。
  const profSource = srcOf('dnd-proficiency')
  const bakedBonuses = []
  for (const def of book.definitions.filter(d => d.kind === 'dnd-proficiency')) {
    const bonus = String(def.fields.bonus)
    if (new RegExp('\\b' + bonus + '\\b').test(profSource)) bakedBonuses.push(def.id + '=' + bonus)
  }
  if (bakedBonuses.length) dataDrivenProblems.push('熟练加值的数值被烘进 Lua：' + bakedBonuses.join(','))
  expect(
    dataDrivenProblems.length === 0,
    'open_content.data_driven',
    dataDrivenProblems.length === 0
      ? dataDriven.length + ' 条规则全部运行时读开放内容（挂接 / 定义），Lua 里没有标识与数值常量'
      : dataDrivenProblems.slice(0, 6).join(' ; ')
  )

  // 9. 日照敏感按判定签名（GAP-D 闭合的验收 2）
  const sunSource = srcOf('dnd-sunlight-sensitivity')
  const sunProblems = []
  if (!sunSource.includes("host.check_kind")) sunProblems.push('没有读判定种类（check_kind）')
  if (!sunSource.includes("host.check_attribute")) sunProblems.push('没有读判定属性（check_attribute）')
  if (!/kind == 'attack'/.test(sunSource)) sunProblems.push('缺少「攻击骰」分支')
  if (!/attribute == 'wis'/.test(sunSource)) sunProblems.push('缺少「依赖视力的感知（wis）检定」分支')
  if (sunSource.includes('host.definition')) sunProblems.push('仍在用 host.definition 绕判定签名（GAP-D 的旧写法）')
  const sunDef = book.definitions.find(d => d.kind === 'dnd-sunlight-sensitivity')
  if (!sunDef || !/signature_scope/.test(JSON.stringify(sunDef.fields))) sunProblems.push('日照敏感定义没有声明签名范围')
  expect(
    sunProblems.length === 0,
    'sunlight.by_signature',
    sunProblems.length === 0
      ? '日照敏感按判定签名区分：attack ∨ (attribute ∧ wis)，其它判定不受影响；定义里带 signature_scope'
      : sunProblems.slice(0, 6).join(' ; ')
  )

  // 9b. GAP-E：豁免减半 = 声明引擎缩放，Lua 不再重掷效果骰
  const halfSource = srcOf('dnd-save-half')
  const halfProblems = []
  if (!halfSource.includes('host.scale_effect(0.5)')) halfProblems.push('没有声明 host.scale_effect(0.5)')
  if (halfSource.includes('host.engine_rng')) halfProblems.push('仍在 Lua 里重掷效果骰（GAP-E 的旧写法）')
  if (!halfSource.includes("host.check_kind ~= 'save'")) halfProblems.push('没有先按 check_kind == save 收敛')
  if (!halfSource.includes('host.check_result == true')) halfProblems.push('没有按豁免结果分支')
  if (!halfSource.includes("host.list_definitions('dnd-save-half')")) halfProblems.push('规则没有从开放内容取 skill_id / fail_status')
  const halfDefs = book.definitions.filter(d => d.kind === 'dnd-save-half')
  if (!halfDefs.length || !halfDefs.every(d => d.fields && d.fields.fail_status)) halfProblems.push('dnd-save-half 定义缺少 fail_status')
  const rubble = book.skills.find(s => s.id === 'sk-lmop-rubble-collapse')
  if (rubble && rubble.effect && Array.isArray(rubble.effect.status) && rubble.effect.status.length) {
    halfProblems.push('技能的失败附加状态仍写在 effect.status（缩放门会连成功豁免也施加）')
  }
  expect(
    halfProblems.length === 0,
    'save_half.scale_effect',
    halfProblems.length === 0
      ? 'dnd-save-half 只声明 scale_effect(0.5)（Lua 无效果骰重掷）；失败附加状态走开放内容 fields.fail_status，技能 effect 不含状态'
      : halfProblems.slice(0, 6).join(' ; ')
  )

  // 9c. GAP-L：伏击读 host.target.statuses（不再靠 dnd-surprised 标记）
  const ambSource = srcOf('dnd-ambusher-keep-high')
  const ambProblems = []
  if (!ambSource.includes('host.target')) ambProblems.push('没有读 host.target')
  if (!ambSource.includes('statuses')) ambProblems.push('没有读目标的 statuses')
  if (ambSource.includes('dnd-surprised')) ambProblems.push('仍把受突袭状态名烘进 Lua（应读开放内容 fields.target_status）')
  const ambDefs = book.definitions.filter(d => d.kind === 'dnd-ambusher')
  if (!ambDefs.length || !ambDefs.every(d => d.fields && d.fields.target_status)) ambProblems.push('伏击定义缺少 target_status')
  const ambMount = book.lua_mounts.find(m => m.id === 'dnd-ambusher-keep-high')
  if (JSON.stringify((ambMount && ambMount.when) || {}).includes('dnd-surprised')) ambProblems.push('when 闸门仍要求 dnd-surprised 标记')
  expect(
    ambProblems.length === 0,
    'ambusher.target_status',
    ambProblems.length === 0
      ? '伏击读 host.target.statuses，期望状态 id 由开放内容 fields.target_status 给出；闸门只留战斗第一轮'
      : ambProblems.slice(0, 6).join(' ; ')
  )

  // 9d. GAP-A：集群战术读运行时事实（近似程度提高，不假装精确）
  const packSource = srcOf('dnd-pack-tactics')
  const packProblems = []
  if (!packSource.includes('host.list_encounters')) packProblems.push('没有读活跃遭遇')
  if (!packSource.includes('host.get_character')) packProblems.push('没有读同伴实例')
  if (!packSource.includes('location_id')) packProblems.push('没有用 location_id 做「目标附近」的近似')
  if (packSource.includes('dnd-flanked')) packProblems.push('仍靠 dnd-flanked 标记')
  const packDefs = book.definitions.filter(d => d.kind === 'dnd-pack-tactics')
  if (!packDefs.length || !packDefs.every(d => d.fields && d.fields.range_proxy)) packProblems.push('集群战术定义缺少 range_proxy（近似口径必须写明）')
  expect(
    packProblems.length === 0,
    'pack_tactics.world_facts',
    packProblems.length === 0
      ? '集群战术用 host.list_encounters + host.get_character 读同遭遇同伴（HP>0），目标按同一 location_id 近似；5 尺仍做不到（GAP-B）'
      : packProblems.slice(0, 6).join(' ; ')
  )

  // 10. 规则挂接自洽（开放内容 → Lua 的入口必须真的指得到定义）
  const attachProblems = []
  const defsById = new Map(book.definitions.map(d => [d.id, d]))
  const attachedKinds = new Set()
  for (const character of book.characters) {
    for (const kind of Object.keys(character.attachments || {})) {
      if (!RULEPACK_ATTACHMENT_KINDS.includes(kind)) continue
      attachedKinds.add(kind)
      for (const id of character.attachments[kind]) {
        const def = defsById.get(id)
        if (!def) attachProblems.push(character.id + ' 挂接了不存在的定义 ' + id)
        else if (def.kind !== kind) attachProblems.push(character.id + ' 的 ' + kind + ' 挂接指向 ' + def.kind + ' 定义 ' + id)
      }
    }
  }
  for (const kind of RULEPACK_ATTACHMENT_KINDS) {
    if (!attachedKinds.has(kind)) attachProblems.push('没有任何角色挂接 ' + kind + '（Lua 读不到数据）')
  }
  expect(
    attachProblems.length === 0,
    'attachments.resolve',
    attachProblems.length === 0
      ? RULEPACK_ATTACHMENT_KINDS.length + ' 类规则挂接全部指向同 kind 的定义：' + RULEPACK_ATTACHMENT_KINDS.join(' / ')
      : attachProblems.slice(0, 6).join(' ; ')
  )

  // 11. 缺口登记（证伪职责：清单非空、每条有 rule / stuck_at / need / status）
  const badGaps = GAPS.filter(
    g => !g.rule || !g.stuck_at || !g.need || !['closed', 'partial', 'open'].includes(g.status)
      || ((g.status === 'closed' || g.status === 'partial') && (!g.closed_by || !g.evidence))
      || ((g.status === 'partial' || g.status === 'open') && !g.now)
  )
  const closed = GAPS.filter(g => g.status === 'closed')
  const partial = GAPS.filter(g => g.status === 'partial')
  expect(
    badGaps.length === 0 && GAPS.length > 0,
    'gaps.registered',
    GAPS.length + ' 条缺口已逐条复核：闭合 ' + closed.length + ' 条 / 近似提高 ' + partial.length + ' 条 / 仍存在 '
      + (GAPS.length - closed.length - partial.length) + ' 条'
      + (badGaps.length ? '（缺字段：' + badGaps.map(g => g.id).join(',') + '）' : '')
  )

  return results
}

function checkMode(engineBin) {
  console.log('== LMoP 规则包自检（node scripts/lmop-rulepack.mjs --check）==')
  const draft = ensureDraft()
  const build = buildStorybook(draft)
  build.draft = draft

  const results = runChecks(build)
  let failed = 0
  let engineChecks = 0
  for (const r of results) {
    console.log('[' + (r.pass ? 'PASS' : 'FAIL') + '] ' + r.name + ' — ' + r.detail)
    if (!r.pass) failed++
  }

  // 确定性：同输入两次构建逐字节一致
  const again = buildStorybook(ensureDraft())
  const a = JSON.stringify(build.book)
  const b = JSON.stringify(again.book)
  const deterministic = a === b
  console.log('[' + (deterministic ? 'PASS' : 'FAIL') + '] determinism.same_output — 同输入两次构建逐字节一致')
  if (!deterministic) failed++

  // 真实引擎发布校验（可选）：--engine <bin> 指向能跑 validate_storybook + lint_storybook 的二进制
  if (engineBin) {
    const tmp = path.join(ROOT, 'story_example/.lmop-rulepack.engine-check.json')
    fs.writeFileSync(tmp, JSON.stringify(build.book, null, 2) + '\n')
    try {
      const raw = execFileSync(engineBin, [tmp], { encoding: 'utf8' })
      const parsed = JSON.parse(raw)
      const gatePass = parsed.error_count === 0 && (parsed.lua_issues || []).length === 0
      console.log(
        '[' + (gatePass ? 'PASS' : 'FAIL') + '] engine.publish_gate — validate_storybook 错误 '
        + parsed.error_count + ' / lua_lint 问题 ' + (parsed.lua_issues || []).length
        + '（警告 ' + parsed.warning_count + '）'
      )
      if (!gatePass) {
        failed++
        console.log(JSON.stringify({ errors: parsed.errors, lua_issues: parsed.lua_issues }, null, 2))
      }
      // 真实引擎的**运行时**规则断言（同一支校验器顺带跑；没有该段就只算发布门）。
      if (Array.isArray(parsed.rule_assertions)) {
        for (const item of parsed.rule_assertions) {
          console.log('[' + (item.pass ? 'PASS' : 'FAIL') + '] engine.' + item.name + ' — ' + item.detail)
          engineChecks++
          if (!item.pass) failed++
        }
      }
      if (parsed.rule_failures) failed += parsed.rule_failures
    } catch (err) {
      console.log('[FAIL] engine.publish_gate — 调引擎校验器失败：' + (err && err.message ? err.message : String(err)))
      failed++
    } finally {
      if (fs.existsSync(tmp)) fs.unlinkSync(tmp)
    }
  }

  console.log('')
  const closedCount = GAPS.filter(g => g.status === 'closed').length
  const partialCount = GAPS.filter(g => g.status === 'partial').length
  console.log(
    '—— 缺口清单现状：闭合 ' + closedCount + ' 条 / 近似提高 ' + partialCount + ' 条 / 仍存在 '
    + (GAPS.length - closedCount - partialCount)
    + ' 条（共 ' + GAPS.length + ' 条；原样报告，不新增封闭字段）——'
  )
  for (const gap of GAPS) {
    if (gap.status === 'closed' || gap.status === 'partial') {
      console.log('· [' + (gap.status === 'closed' ? '已闭合' : '近似提高') + '] [' + gap.id + '] ' + gap.rule)
      console.log('    原缺口：' + gap.stuck_at)
      console.log('    ' + (gap.status === 'closed' ? '闭合原语' : '提高用到的原语') + '：' + gap.closed_by)
      console.log('    落地：' + gap.evidence)
      if (gap.now) console.log('    残余 / 复核：' + gap.now)
    } else {
      console.log('· [仍存在] [' + gap.id + '] ' + gap.rule)
      console.log('    卡在：' + gap.stuck_at)
      console.log('    需要：' + gap.need)
      console.log('    本轮复核：' + gap.now)
    }
  }
  console.log('')
  const totalChecks = results.length + 1 + (engineBin ? 1 + engineChecks : 0)
  console.log(
    '总结：' + totalChecks + ' 项断言，' + failed + ' 项失败'
    + (engineBin ? '（含真实引擎发布门 + ' + engineChecks + ' 条运行时规则断言）' : '')
  )
  return failed === 0 ? 0 : 1
}

// ---------------------------------------------------------------------------
// 6. CLI
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const out = { check: false, help: false, out: null, engine: null }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--check') out.check = true
    else if (a === '--help' || a === '-h') out.help = true
    else if (a === '--out') out.out = argv[++i]
    else if (a === '--engine') out.engine = argv[++i]
    else throw new Error('未知参数 ' + a)
  }
  return out
}

function usage() {
  console.log('用法：node scripts/lmop-rulepack.mjs [--check] [--out <file>] [--engine <bin>]')
  console.log('  （无参数）        生成 story_example/lmop-storybook.json（T8 草稿 + 规则包）')
  console.log('  --check           自检并断言覆盖 / PC / 撤回字段 / 掷表链路 / 确定性，不写文件')
  console.log('  --engine <bin>    额外跑一次真实引擎发布校验（bin 接收故事书路径，输出 JSON）')
}

function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.help) { usage(); return 0 }
  if (args.check) return checkMode(args.engine)
  const draft = ensureDraft()
  const { book, rules } = buildStorybook(draft)
  const target = args.out ? path.resolve(process.cwd(), args.out) : F.out
  fs.writeFileSync(target, JSON.stringify(book, null, 2) + '\n')
  console.log('已写入故事书 ' + path.relative(ROOT, target))
  console.log(
    '  图鉴 ' + book.characters.filter(c => c.kind === 'monster').length
    + ' 条 / 角色 ' + book.characters.filter(c => c.kind !== 'monster').length
    + ' 条 / 地点 ' + book.world.locations.length
    + ' 个 / 地图 ' + book.world.maps.length
    + ' 张 / 章节 ' + book.skeleton.length
    + ' 章 / 技能 ' + book.skills.length
    + ' 条 / 状态 ' + book.statuses.length
    + ' 条 / 开放内容定义 ' + book.definitions.length
    + ' 条 / 规则挂载点 ' + rules.length + ' 条'
  )
  console.log('  只写 JSON；不写数据库。规则包 = lua_mounts 脚本 + 开放内容，引擎一行未改。')
  return 0
}

try {
  process.exitCode = main()
} catch (err) {
  console.error('[lmop-rulepack] ' + (err && err.message ? err.message : String(err)))
  process.exitCode = 1
}
