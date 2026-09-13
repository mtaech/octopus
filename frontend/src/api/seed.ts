// ============================================================
// 演示种子数据（内存 Mock 后端用，贴近蓝图 #01 形状）
// 世界：坠星谷 —— 一个边境小镇的奇幻开放设定
// ============================================================
import type { Storybook, CheckerDef } from '@/types'

export const DIMENSIONS = [
  { key: 'str', label: '力量', type: 'number' as const, min: 0, max: 100, baseline: 50 },
  { key: 'agi', label: '敏捷', type: 'number' as const, min: 0, max: 100, baseline: 50 },
  { key: 'wit', label: '机敏', type: 'number' as const, min: 0, max: 100, baseline: 50 },
  { key: 'cha', label: '魅力', type: 'number' as const, min: 0, max: 100, baseline: 50 },
  { key: 'trait', label: '特质', type: 'enum' as const, options: ['沉稳', '急躁', '好奇', '多疑', '慷慨', '记仇'] },
  { key: 'note', label: '备注', type: 'text' as const }
]

export const GLOBAL_CHECKER: CheckerDef = {
  dice: '1d20',
  mode: 'gte',
  modifier_formula: '(v - 50) / 5',
  degree_thresholds: [10, 0, -10]
}

/** 坠星谷·酒馆之夜：主打故事书（已发布 rev4，其存档 embedded rev3 → 升级演示） */
export function storybookFallingStar(): Storybook {
  return {
    schema_version: 3,
    meta: { id: 'sb-fallingstar', title: '坠星谷 · 酒馆之夜', author: '星尘旅人', language: 'zh-CN', description: '边境小镇坠星谷，一颗流星坠落后，夜晚变得不再平静。' },
    world: {
      premise: '坠星谷是群山环抱的边境小镇，三日前一颗流星坠落在镇外废矿坑，镇民开始做同一个怪梦。你是路过的旅人，被酒馆老板娘伊莎收留过夜。',
      opening: '夜里的雨敲打着碎星酒馆的窗。你推开木门，油灯光一晃，老板娘伊莎抬眼打量你：「稀客。先坐下来，喝一杯暖暖身子。」你抖落斗篷上的水珠，在她对面坐下——坠星谷的怪梦，刚刚开始。',
      locations: [
        { id: 'loc-tavern', name: '碎星酒馆', description: '镇中心的老酒馆，老板娘伊莎的领地，暖黄的油灯与麦酒味。' },
        { id: 'loc-square', name: '镇广场', description: '白天的集市与公告栏，夜晚空旷，只有风。' },
        { id: 'loc-mine', name: '废矿坑', description: '流星坠落处，被镇公所围了起来，禁止入内。' },
        { id: 'loc-church', name: '星辰教堂', description: '镇上的老教堂，执事奥登看守着地窖。' }
      ],
      resources: [
        { id: 'res-gold', name: '金币', type: 'numerical', default_max: 999, natural_recovery: { amount: 0, trigger: 'per_rest' } },
        { id: 'res-stamina', name: '体力', type: 'numerical', default_max: 100, natural_recovery: { amount: 20, trigger: 'per_rest' } }
      ],
      check: { ...GLOBAL_CHECKER }
    },
    attribute_dimensions: [...DIMENSIONS],
    skeleton: [
      {
        id: 'ch-1', title: '第一章 · 流星之夜', description: '在酒馆安顿，结识镇民，听闻怪梦传闻。',
        scenes: [
          {
            id: 'sc-tavern-night', title: '碎星酒馆的夜晚',
            location_id: 'loc-tavern',
            present_char_ids: ['char-mira', 'char-isa', 'char-oden', 'char-kael'],
            goals: [
              { id: 'g1', text: '在酒馆打听到怪梦的传闻', primary: true, condition: { op: 'flag_set', flag: 'heard_dreams' } },
              { id: 'g2', text: '结识至少两位镇民', primary: false, condition: { op: 'attribute_ge', attribute: 'cha', value: 1 } }
            ],
            triggers: [
              { id: 'b1', title: '流星坠落之夜', description: '老板娘提起流星坠落那晚', hint: '让伊莎讲述流星坠落那一夜发生了什么，注意她的措辞。', condition: { op: 'flag_set', flag: 'met_isa' } },
              { id: 'b2', title: '梦的怪象', description: '镇民提到同样的怪梦', hint: '镇民们说着同一个怪梦——在梦里他们都站在废矿坑边缘。', condition: { op: 'flag_set', flag: 'heard_dreams' }, repeatable: true }
            ]
          },
          {
            id: 'sc-mine-probe', title: '废矿坑调查',
            location_id: 'loc-mine',
            present_char_ids: ['char-mira', 'char-kael'],
            goals: [
              { id: 'g3', text: '查明废矿坑里的异常', primary: true, condition: { op: 'flag_set', flag: 'mine_probed' } }
            ],
            triggers: [
              { id: 'b3', title: '封条与告示', description: '矿坑入口被镇公所封条围住', hint: '封条上盖着镇公所的章，日期是流星坠落当天。', condition: { op: 'at_location', location_id: 'loc-mine' } }
            ]
          }
        ]
      }
    ],
    characters: [
      { id: 'char-mira', name: '米拉', kind: 'pc', background: '流浪的赏金猎人，为追查一桩旧案来到坠星谷。', personality: '冷静寡言，观察敏锐，习惯先确认退路。', attributes: { str: 55, agi: 70, wit: 75, cha: 45, trait: '多疑', note: '' }, resources: { 'res-gold': 32, 'res-stamina': 100 }, skills: ['sk-observe', 'sk-persuade'], inventory: [{ id: 'it-potion', quantity: 2 }] },
      { id: 'char-isa', name: '伊莎', kind: 'npc', background: '碎星酒馆老板娘，十年前从王都搬来，消息灵通。', personality: '热情圆滑，话里有话，护短。', attributes: { str: 40, agi: 50, wit: 80, cha: 85, trait: '慷慨', note: '' }, skills: ['sk-persuade'] },
      { id: 'char-oden', name: '奥登', kind: 'npc', background: '星辰教堂执事，看守地窖，笃信星象。', personality: '温和谨慎，忌讳谈论流星。', attributes: { str: 45, agi: 40, wit: 70, cha: 65, trait: '沉稳', note: '' }, skills: ['sk-observe'] },
      { id: 'char-kael', name: '凯尔', kind: 'npc', background: '镇上的年轻铁匠，流星夜后变得焦躁。', personality: '冲动直率，藏不住事。', attributes: { str: 78, agi: 60, wit: 40, cha: 50, trait: '急躁', note: '' }, skills: ['sk-quaff'] }
    ],
    skills: [
      { id: 'sk-observe', name: '观察', description: '仔细观察环境或人物，寻找线索。', cost: [], check: { dice: '1d20', mode: 'gte', degree_thresholds: [10, 0, -10] }, effect: {} },
      { id: 'sk-persuade', name: '说服', description: '用言语打动对方。', cost: [], check: 'sk-observe' },
      { id: 'sk-quaff', name: '痛饮', description: '喝下一大杯麦酒，恢复体力。', cost: [{ resource: 'res-gold', amount: 2 }], effect: { immediate: [{ kind: 'modify_resource', resource: 'res-stamina', amount: '15' }] } }
    ],
    items: [
      { id: 'it-map', name: '旧地图', description: '一张标注着坠星谷周边的羊皮地图，废矿坑被红笔圈过。', skills: [] },
      { id: 'it-potion', name: '药水', description: '恢复体力的粗制药水。', skills: ['sk-quaff'] },
      { id: 'it-lantern', name: '提灯', description: '旅人的旧提灯，油还够半晚。', skills: [] }
    ],
    objects: [
      { id: 'obj-notice', name: '公告牌', description: '钉着镇公所告示的旧木牌。', location_id: 'loc-tavern', actions: [{ key: 'read', label: '查看' }] },
      { id: 'obj-seal', name: '矿坑封条', description: '盖着镇公所印章的封条。', location_id: 'loc-mine', actions: [{ key: 'inspect', label: '检查' }] }
    ],
    factions: [
      { id: 'fac-town', name: '镇公所', description: '坠星谷的治理机构，镇长病后由文书代理。', goals: ['维持秩序', '压下流言'], default_attitude: 0 },
      { id: 'fac-church', name: '星辰教会', description: '供奉星象的老教会。', goals: ['守望星象', '看守地窖'], default_attitude: 20 }
    ],
    relationships: [
      { id: 'rel-1', from_kind: 'character', from: 'char-isa', to_kind: 'character', to: 'char-mira', type: '好感', value: 30 },
      { id: 'rel-2', from_kind: 'character', from: 'char-kael', to_kind: 'character', to: 'char-isa', type: '敬畏', value: 40 },
      { id: 'rel-3', from_kind: 'faction', from: 'fac-church', to_kind: 'character', to: 'char-oden', type: '隶属', value: 90 }
    ],
    statuses: [
      { id: 'st-burn', name: '灼烧', description: '被流星余烬灼伤，每回合流失体力。', duration: 3, unit: 'turns', stack: 'replace', effect: [{ kind: 'damage', amount: '3', resource: 'res-stamina' }] },
      { id: 'st-guard', name: '守望', description: '星辰教会的祝祷，短暂提升意志。', duration: 2, unit: 'turns', stack: 'replace' }
    ],
    flags: [
      { key: 'met_isa', label: '结识伊莎' },
      { key: 'heard_dreams', label: '听闻怪梦' },
      { key: 'mine_foreshadow', label: '矿坑伏笔' },
      { key: 'silver_powder', label: '银色粉末' },
      { key: 'mine_probed', label: '矿坑已探查' }
    ],
    events: [
      { key: 'scene', label: '场景切换' },
      { key: 'character_death', label: '角色死亡' }
    ],
    relationship_types: [
      { key: '好感', label: '好感' },
      { key: '敬畏', label: '敬畏' },
      { key: '隶属', label: '隶属' },
      { key: '敌意', label: '敌意' }
    ],
    target_types: [
      { key: 'self', label: '自身' },
      { key: 'single', label: '单体' },
      { key: 'area', label: '范围' }
    ],
    // 元模型探针：下面两个种类在代码里没有任何对应类型 / 面板 / 校验分支，
    // 却由通用面板、引用选择器、结对上下文一并支持。
    kinds: [
      {
        key: 'rumor', label: '传闻', group: '开放内容',
        fields: [
          { key: 'source', label: '来源', type: 'ref', ref_kind: 'character', hint: '谁传出的' },
          { key: 'place', label: '地点', type: 'ref', ref_kind: 'location' },
          { key: 'credibility', label: '可信度', type: 'number', hint: '0–100' },
          { key: 'verified', label: '已证实', type: 'boolean' },
          { key: 'detail', label: '内容', type: 'textarea' }
        ]
      },
      {
        key: 'prophecy', label: '预言', group: '开放内容',
        fields: [
          { key: 'speaker', label: '预言者', type: 'ref', ref_kind: 'character' },
          { key: 'fulfilled', label: '已应验', type: 'boolean' },
          { key: 'text', label: '预言', type: 'textarea' }
        ]
      }
    ],
    definitions: [
      { id: 'rumor-1', kind: 'rumor', name: '矿坑的怪声', fields: { source: 'char-kael', place: 'loc-mine', credibility: 60, verified: false, detail: '凯尔说夜里经过矿坑，听见底下有人念他的名字。' } },
      { id: 'rumor-2', kind: 'rumor', name: '教堂地窖的灯', fields: { source: 'char-oden', place: 'loc-church', credibility: 30, verified: false, detail: '奥登承认地窖的灯有时会自己亮起来，但他不肯多说。' } },
      { id: 'pro-1', kind: 'prophecy', name: '流星的低语', fields: { speaker: 'char-isa', fulfilled: false, text: '当第三颗星落下，睡着的镇子会醒来。' } }
    ]
  }
}

/** 雾镇疑云 · 调查档案：rev2 已发布 */
export function storybookMist(): Storybook {
  const base = storybookFallingStar()
  return {
    ...base,
    meta: { ...base.meta, id: 'sb-mist', title: '雾镇疑云 · 调查档案', author: '岸边灯塔', description: '常年大雾的海边小镇，档案员失踪，留下一本没写完的档案。' },
    characters: base.characters.map((c, i) => (i === 0 ? { ...c, id: 'char-eve', name: '艾芙' } : c))
  }
}

/** 灰烬行者：rev3 已发布（后启示录风） */
export function storybookAsh(): Storybook {
  const base = storybookFallingStar()
  return {
    ...base,
    meta: { ...base.meta, id: 'sb-ash', title: '灰烬行者', author: '余烬', description: '焚尽之城幸存的巡夜人，替无法安息者走完最后一程。' },
    world: {
      ...base.world,
      premise: '旧世界焚毁后，幸存者蜷缩在灰烬城的铁墙后。巡夜人每晚出城，为死在墙外的亡魂点上最后一盏灯。',
      opening: '入夜，铁门在你身后合拢。你提起那盏旧提灯，走出灰烬城的西门——今晚的名单上，又多了三个名字。墙外的灰像雪一样落着，远处有什么在等你。'
    },
    characters: base.characters.map((c, i) => (i === 0 ? { ...c, id: 'char-wren', name: '瑞恩', kind: 'pc', personality: '寡言，背负太多，对死亡习以为常。' } : c))
  }
}

/** 北境信使 · 初雪：只有草稿（未发布），编辑器演示 */
export function storybookDraft(): Storybook {
  const base = storybookFallingStar()
  return {
    ...base,
    meta: { ...base.meta, id: 'sb-courier', title: '北境信使 · 初雪', author: '', description: '' },
    world: { ...base.world, opening: '北境的第一场雪落下来时，你正走在通往初雪驿站的驿道上。怀里那封信被油布裹了三层，是镇里老邮差托付的——他说，这封信必须亲手交到掌柜手上。' },
    skeleton: [{ id: 'ch-1', title: '第一章 · 驿站', description: '', scenes: [{ id: 'sc-1', title: '初雪驿站', goals: [{ id: 'g1', text: '把信送到驿站掌柜手上', primary: true, condition: null }], triggers: [] }] }],
    characters: [{ id: 'char-rio', name: '里奥', kind: 'pc', background: '驿站信使，腿脚快，记性差。', personality: '开朗', attributes: { str: 50, agi: 80, wit: 45, cha: 60, trait: '好奇' }, resources: { 'res-stamina': 100 } }]
  }
}
