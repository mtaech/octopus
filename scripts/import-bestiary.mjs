#!/usr/bin/env node
// scripts/import-bestiary.mjs
// LMoP 内容与导入：5e 图鉴 JSON -> kind=monster 图鉴条目；冒险数据 -> 地点树 / 地图锚点 / 章节骨架。
//
// 硬边界（docs/rules-via-lua.md 第 5 节）：规则集语义不落封闭字段。
//   六维 -> attributes · AC -> 顶层 derived 的 ac 公式 + 挂接 modifiers · HP -> resources.hp
//   攻击 -> skills[]（check = {dice:"1d20", kind:"attack"} + effect.immediate[damage]）
//   变体 -> 开放内容挂接（attachments + monster-variant definition），不新增 extends 字段
//   展示型数据 -> statblock（creatureType / traits / challenge / xp / actionsNote），引擎不读
//
// 输入（story_example/，均可由 --vendor-from 从官方 5e 数据重建）：
//   bestiary-lmop.json             官方 5e 图鉴文件（LMoP 部分），原样保存
//   bestiary-lmop-appendix-b.json  官方 5e 图鉴文件（MM/DMG）中附录 B 用到的条目子集，原样抽取
//   lmop.json                      5etools 冒险数据（地点 / 章节 / 地图清单）
//   凡戴尔的失落矿坑.md             中文译文：附录 B 数据卡，用于条目顺序与数值交叉核对
//
// 输出（默认）：lmop-storybook.draft.json —— 可导入的故事书草稿，脚本不写数据库。
//
// 用法：
//   node scripts/import-bestiary.mjs
//   node scripts/import-bestiary.mjs --check
//   node scripts/import-bestiary.mjs --vendor-from <dir>   # dir 内放官方 bestiary-lmop/-mm/-dmg.json
//   node scripts/import-bestiary.mjs --out <file>

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '..')
const STORY = path.join(ROOT, 'story_example')

const F = {
  adventure: path.join(STORY, 'lmop.json'),
  md: path.join(STORY, '凡戴尔的失落矿坑.md'),
  bestiaryLmop: path.join(STORY, 'bestiary-lmop.json'),
  bestiaryAppendix: path.join(STORY, 'bestiary-lmop-appendix-b.json'),
  bestiaryMm: path.join(STORY, 'bestiary-mm.json'),
  bestiaryDmg: path.join(STORY, 'bestiary-dmg.json'),
  draft: path.join(STORY, 'lmop-storybook.draft.json')
}

const MD_APPENDIX_HEADING = '# 附录B：怪物'

// ---------- 5e 对照表（与 lmop2md.js 同源） ----------

const XP = { '0': 10, '1/8': 25, '1/4': 50, '1/2': 100, '1': 200, '2': 450, '3': 700, '4': 1100, '5': 1800, '6': 2300, '7': 2900, '8': 3900, '9': 5000, '10': 5900, '11': 7200, '12': 8400, '13': 10000, '14': 11500, '15': 13000, '16': 15000, '17': 18000, '18': 20000, '19': 22000, '20': 25000, '21': 33000, '22': 41000, '23': 50000, '24': 62000, '25': 155000, '26': 155000, '27': 155000, '28': 155000, '29': 155000, '30': 155000 }
const SIZE = { T: '微型', S: '小型', M: '中型', L: '大型', H: '巨型', G: '超巨型' }
const TYPE = { aberration: '异怪', beast: '野兽', celestial: '天界生物', construct: '构装体', dragon: '龙', elemental: '元素', fey: '精类', fiend: '邪魔', giant: '巨人', humanoid: '类人生物', monstrosity: '怪兽', ooze: '泥怪', plant: '植物', undead: '不死生物' }
const ABIL = { str: '力量', dex: '敏捷', con: '体质', int: '智力', wis: '睿智', cha: '魅力' }
const ABIL_KEYS = ['str', 'dex', 'con', 'int', 'wis', 'cha']
const SKILL = { acrobatics: '特技', animal: '驯兽', arcana: '奥秘', athletics: '运动', deception: '欺瞒', history: '历史', insight: '洞悉', intimidation: '威吓', investigation: '调查', medicine: '医药', nature: '自然', perception: '察觉', performance: '表演', persuasion: '说服', religion: '宗教', sleight: '巧手', stealth: '隐匿', survival: '求生' }

const DIMENSIONS = ABIL_KEYS.map(function (k) {
  return { key: k, label: ABIL[k], type: 'number', min: 1, max: 30, baseline: 10, modifier_step: 2 }
})

// 顶层派生值声明：六维调整值 + AC。AC 用「无甲基准 + 挂接修正」表达，
// 于是「条目级常量 AC」不需要任何新封闭字段（护甲走 monster-armor 挂接的 modifiers）。
const DERIVED = ABIL_KEYS.map(function (k) {
  return { key: k + '_mod', label: ABIL[k] + '调整值', formula: 'floor((' + k + ' - 10) / 2)', group: '属性', signed: true }
}).concat([
  { key: 'ac', label: '护甲等级', formula: '10 + dex_mod', group: '战斗' }
])

const SHEET = [
  { title: '战斗', derived: ['ac'], resources: ['res-hp'] },
  { title: '属性', derived: ABIL_KEYS.map(function (k) { return k + '_mod' }) },
  { title: '数据卡挂接', kinds: ['monster-variant', 'monster-armor'] }
]

const RESOURCES = [
  { id: 'res-hp', name: '生命值', type: 'numerical' },
  { id: 'res-xp', name: '经验值', type: 'numerical' }
]

const WORLD_CHECK = {
  dice: '1d20',
  mode: 'gte',
  attribute: 'str',
  modifier_formula: 'floor((v - 10) / 2)',
  degree_thresholds: [10, 0, -10]
}

const KINDS = [
  {
    key: 'monster-armor', label: '怪物护甲', group: '图鉴', applies_to: ['character'],
    fields: [
      { key: 'source', label: '护甲来源', type: 'text', hint: '数据卡 ac[].from，如「兽皮甲、盾牌」' },
      { key: 'note', label: '说明', type: 'textarea' }
    ]
  },
  {
    key: 'monster-variant', label: '怪物变体', group: '图鉴', applies_to: ['character'],
    fields: [
      { key: 'base_creature', label: '基础数据卡', type: 'ref', ref_kind: 'character', hint: '变体沿用的资料卡（如灰烬丧尸沿用丧尸）' },
      { key: 'extra_traits', label: '额外特性', type: 'textarea' },
      { key: 'source', label: '出处', type: 'text' }
    ]
  }
]

// ---------- 冒险结构表（地点 / 章节 / 地图） ----------
// src = 5etools 冒险数据的 section id；areas:true 表示把紧跟其后的「N. 区域」收为子地点。

const LOCATION_PLAN = [
  { part: '00c', nodes: [{ src: '00e' }, { src: '012', areas: true }] },
  {
    part: '040',
    nodes: [
      { src: '040' },
      { src: '04a', parent: '040' }, { src: '053', parent: '040' }, { src: '056', parent: '040' },
      { src: '059', parent: '040' }, { src: '05c', parent: '040' }, { src: '05f', parent: '040' },
      { src: '062', parent: '040' }, { src: '065', parent: '040' }, { src: '066', parent: '040' },
      { src: '06c', parent: '040' },
      { src: '075', parent: '040', areas: true }
    ]
  },
  {
    part: '0b6',
    nodes: [
      { src: '0b7' }, { src: '0c1' }, { src: '0c9' },
      { src: '0ce', areas: true },
      { src: '106' }, { src: '10b', areas: true }
    ]
  },
  { part: '14b', nodes: [{ src: '14b', areas: true }] }
]

const PART_LABEL = { '00c': '第一部分', '040': '第二部分', '0b6': '第三部分', '14b': '第四部分' }

const MAP_PLAN = [
  {
    id: 'map-sword-coast', name: '剑湾（区域图）', file: 'adventure/LMoP/The Sword Coast.webp', credit: 'Mike Schley',
    pins: ['00e', '012', '040', '0b7', '0c1', '0c9', '0ce', '106', '10b', '14b']
  },
  { id: 'map-cragmaw-hideout', name: '克拉摩窝点', file: 'adventure/LMoP/Cragmaw Hideout.webp', credit: 'Mike Schley', root: '012' },
  {
    id: 'map-phandalin', name: '凡达林', file: 'adventure/LMoP/Phandalin.webp', credit: 'Mike Schley', root: '040',
    pins: ['040', '04a', '053', '056', '059', '05c', '05f', '062', '065', '066', '06c']
  },
  { id: 'map-redbrand-hideout', name: '红标帮窝点', file: 'adventure/LMoP/Redbrand Hideout.webp', credit: 'Mike Schley', root: '075' },
  { id: 'map-thundertree', name: '雷树废墟', file: 'adventure/LMoP/Ruins of Thundertree.webp', credit: 'Mike Schley', root: '0ce' },
  { id: 'map-cragmaw-castle', name: '克拉摩堡', file: 'adventure/LMoP/Cragmaw Castle.webp', credit: 'Mike Schley', root: '10b' },
  { id: 'map-wave-echo-cave', name: '回声洞', file: 'adventure/LMoP/Wave Echo Cave.webp', credit: 'Mike Schley', root: '14b' }
]

const FLAGS = [
  { key: 'gundren_rescued', label: '救出刚铎·寻岩者' },
  { key: 'map_recovered', label: '取回矿井地图' },
  { key: 'nezznar_defeated', label: '击败黑蜘蛛' },
  { key: 'wave_echo_cleared', label: '回声洞已肃清' }
]

const EVENTS = [{ key: 'scene', label: '场景切换' }, { key: 'character_death', label: '角色死亡' }]
const RELATIONSHIP_TYPES = [{ key: '敌对', label: '敌对' }, { key: '隶属', label: '隶属' }]
const TARGET_TYPES = [{ key: 'self', label: '自身' }, { key: 'single', label: '单体' }, { key: 'area', label: '范围' }]

// ---------- 小工具 ----------

function slug(s) {
  return String(s === undefined || s === null ? '' : s)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
}
function round4(n) { return Math.round(n * 10000) / 10000 }
function truncate(s, n) {
  const t = String(s || '').trim()
  return t.length <= n ? t : t.slice(0, n).trimEnd() + '…'
}
function stripMd(s) { return String(s === undefined || s === null ? '' : s).replace(/\*\*/g, '') }
function exists(p) { try { fs.accessSync(p); return true } catch (e) { return false } }
function readJson(p) { return JSON.parse(fs.readFileSync(p, 'utf8')) }
function readText(p) { return fs.readFileSync(p, 'utf8') }

// ---------- 行内标记还原（{@tag ...}） ----------

const ATK_LABEL = { mw: '近战武器攻击', rw: '远程武器攻击', ms: '近战法术攻击', rs: '远程法术攻击' }

function inline(input) {
  if (input === undefined || input === null) return ''
  let s = String(input).replace(/\n\n+/g, '\n\n').replace(/\n/g, ' ')
  let prev
  do {
    prev = s
    s = s.replace(/\{@([a-zA-Z0-9]+)\s*([^{}]*)\}/g, function (m, tag, content) {
      const value = content.split('|')[0].trim()
      const t = String(tag).toLowerCase()
      if (t === 'b' || t === 'bold') return '**' + value + '**'
      if (t === 'i' || t === 'italic') return '*' + value + '*'
      if (t === 'atk') {
        const parts = value.split(',').map(function (x) { return x.trim() }).filter(Boolean)
        const labels = parts.map(function (p) { return ATK_LABEL[p] || p })
        return labels.length > 1 ? labels.join('或') : (labels[0] || '武器攻击')
      }
      if (t === 'hit') return /^[+-]/.test(value) ? value : '+' + value
      if (t === 'h') return '命中：'
      if (t === 'dc') return 'DC ' + value
      if (t === 'recharge') return '（充能 ' + value + '–6）'
      if (t === 'chance') return value + '%'
      if (t === 'area') return '第 ' + value + ' 区'
      if (t === 'damage' || t === 'dice' || t === 'd20' || t === 'd100') return value
      if (t === 'scaledamage' || t === 'scaledice') return content.split('|').pop().trim() || value
      if (t === 'note') return '（' + value + '）'
      if (t === 'creature' || t === 'hazard') return '**' + value + '**'
      return value
    })
  } while (s !== prev)
  return s
}

function plainText(e) {
  if (typeof e === 'string') return inline(e)
  if (!e || typeof e !== 'object') return ''
  if (Array.isArray(e.items)) return e.items.map(plainText).filter(Boolean).join('；')
  if (e.entry !== undefined) return (e.name ? inline(e.name) + '：' : '') + inline(e.entry)
  const body = (e.entries || []).map(plainText).filter(Boolean).join(' ')
  return e.name ? inline(e.name) + '。' + body : body
}

function entriesText(entries) {
  return (entries || []).map(plainText).filter(Boolean).join(' ').replace(/\s+/g, ' ').trim()
}

function alignmentStr(a) {
  if (!a) return ''
  if (typeof a === 'string') {
    if (a === 'U') return '无阵营'
    if (a === 'ANY' || a === 'A') return '任何阵营'
    return a
  }
  if (a[0] === 'U') return '无阵营'
  if (a[0] === 'ANY' || a[0] === 'A') return '任何阵营'
  const LAW = { L: '守序', N: '中立', C: '混乱' }
  const GOOD = { G: '善良', N: '中立', E: '邪恶' }
  if (a[0] === 'N' && (a[1] === 'N' || !a[1])) return '绝对中立'
  return (LAW[a[0]] || '') + (GOOD[a[1]] || '') || '未知'
}

function speedStr(sp) {
  if (!sp) return ''
  const order = ['walk', 'burrow', 'climb', 'fly', 'swim']
  const label = { walk: '', burrow: '掘地 ', climb: '攀爬 ', fly: '飞行 ', swim: '游泳 ' }
  const parts = []
  for (const k of order) {
    const v = sp[k]
    if (!v) continue
    parts.push(label[k] + (typeof v === 'number' ? v + '尺' : v.number + '尺' + (v.condition || '')))
  }
  if (sp.canHover || sp.hover) parts.push('（悬停）')
  return parts.join('，')
}

function dmgTypeList(arr) {
  return (arr || []).map(function (x) {
    if (typeof x === 'string') return inline(x)
    return (x.resist || x.immune || x.vulnerable || []).map(inline).join('、') + (x.note ? '（' + inline(x.note) + '）' : '')
  }).join('、')
}

function acList(ac) {
  if (!Array.isArray(ac)) return String(ac === undefined ? '' : ac)
  return ac.map(function (x) {
    if (typeof x === 'number') return String(x)
    const from = x.from ? '（' + x.from.map(inline).join('、') + '）' : ''
    const cond = x.condition ? '（' + inline(x.condition) + '）' : ''
    return String(x.ac) + from + cond
  }).join('、')
}

function firstAc(ac) {
  if (typeof ac === 'number') return ac
  if (!Array.isArray(ac)) return null
  for (const x of ac) {
    if (typeof x === 'number') return x
    if (x && typeof x.ac === 'number') return x.ac
  }
  return null
}

// ---------- 附录 B markdown 解析（名称顺序 + 数值交叉核对） ----------

function parseAppendixB(mdText) {
  const lines = mdText.split('\n')
  let start = -1
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].indexOf(MD_APPENDIX_HEADING) === 0) { start = i; break }
  }
  if (start < 0) throw new Error('md 中未找到 ' + MD_APPENDIX_HEADING)
  let end = lines.length
  for (let i = start + 1; i < lines.length; i++) {
    if (/^# /.test(lines[i])) { end = i; break }
  }
  const body = lines.slice(start + 1, end)
  const sections = []
  let cur = null
  for (let i = 0; i < body.length; i++) {
    const line = body[i]
    const h = line.match(/^## (.+?)\s*$/)
    if (h) { cur = { name: h[1].trim(), lineNumber: start + 2 + i, lines: [] }; sections.push(cur); continue }
    if (cur) cur.lines.push(line)
  }
  for (const sec of sections) {
    const ls = sec.lines
    const acLine = ls.filter(function (l) { return /^\s*>\s*\*\*护甲等级\*\*/.test(l) })[0]
    const hpLine = ls.filter(function (l) { return /^\s*>\s*\*\*生命值\*\*/.test(l) })[0]
    const rowLine = ls.filter(function (l) { return /^\s*>\s*\|/.test(l) && l.indexOf('（') >= 0 })[0]
    const acMatch = acLine ? acLine.match(/(\d+)/) : null
    const hpMatch = hpLine ? hpLine.match(/(\d+)/) : null
    const cells = rowLine ? rowLine.split('|').map(function (s) { return s.trim() }).filter(function (c) { return /\d/.test(c) }) : []
    const abilities = cells.map(function (c) { const m = c.match(/(-?\d+)/); return m ? Number(m[1]) : null })
    const introLines = []
    for (const l of ls) { if (/^\s*>/.test(l)) break; introLines.push(l) }
    const intro = introLines.join('\n').trim()
    sec.ac = acMatch ? Number(acMatch[1]) : null
    sec.hp = hpMatch ? Number(hpMatch[1]) : null
    sec.abilities = abilities.length === 6 && abilities.every(function (n) { return n !== null }) ? abilities : null
    sec.intro = intro
    sec.numbers = sec.ac !== null && sec.hp !== null && sec.abilities !== null
  }
  return sections
}

function firstParagraph(text) {
  const paras = String(text || '').split(/\n\s*\n/).map(function (p) { return p.replace(/\*\*/g, '').replace(/\s+/g, ' ').trim() }).filter(Boolean)
  return paras[0] || ''
}

// ---------- 5e 图鉴加载与 _copy 解析 ----------

function buildBestiaryIndex(files) {
  const index = new Map()
  const origin = new Map()
  const order = []
  for (const f of files) {
    const list = (f.data && f.data.monster) || []
    for (const m of list) {
      if (!m || !m.name) continue
      const prev = index.get(m.name)
      if (!prev || (m.source === 'LMoP' && prev.source !== 'LMoP')) {
        index.set(m.name, m)
        origin.set(m.name, f.name)
      }
      if (order.indexOf(m.name) < 0) order.push(m.name)
    }
  }
  return { index: index, origin: origin, order: order }
}

function deepClone(v) {
  if (Array.isArray(v)) return v.map(deepClone)
  if (v && typeof v === 'object') {
    const o = {}
    for (const k of Object.keys(v)) o[k] = deepClone(v[k])
    return o
  }
  return v
}

function replaceTxtDeep(v, search, replacement) {
  if (typeof v === 'string') return v.split(search).join(replacement)
  if (Array.isArray(v)) return v.map(function (x) { return replaceTxtDeep(x, search, replacement) })
  if (v && typeof v === 'object') {
    const o = {}
    for (const k of Object.keys(v)) o[k] = replaceTxtDeep(v[k], search, replacement)
    return o
  }
  return v
}

/** 解析 5e 数据卡的 _copy / _mod：返回展开后的条目与继承信息（用于开放内容挂接）。 */
function resolveCopy(entry, bestiary, seen) {
  const guard = seen || new Set()
  const cp = entry._copy
  if (!cp || !cp.name) return { entry: entry, copyOf: null, templates: [], mod: null }
  if (guard.has(entry.name)) return { entry: entry, copyOf: null, templates: [], mod: null, cyclic: true }
  guard.add(entry.name)
  const base = bestiary.index.get(cp.name)
  if (!base) return { entry: entry, copyOf: null, templates: cp._templates || [], mod: entry._mod || null, missingBase: cp.name }
  const resolvedBase = resolveCopy(base, bestiary, guard)
  let merged = deepClone(resolvedBase.entry)
  const mod = entry._mod
  if (mod && mod['*'] && mod['*'].mode === 'replaceTxt' && typeof mod['*'].replace === 'string') {
    merged = replaceTxtDeep(merged, mod['*'].replace, mod['*'].with)
  }
  const out = Object.assign({}, merged, entry)
  delete out._copy
  delete out._mod
  return { entry: out, copyOf: cp.name, templates: cp._templates || [], mod: mod || null }
}

function creatureTypeLabel(m) {
  const size = SIZE[(m.size || [])[0]] || ''
  const t = m.type
  let typeStr = ''
  if (typeof t === 'string') typeStr = TYPE[t] || t
  else if (t) typeStr = (TYPE[t.type] || t.type) + (t.tags ? '（' + t.tags.map(inline).join('、') + '）' : '')
  const align = alignmentStr(m.alignment)
  const head = [size, typeStr].filter(Boolean).join(' ')
  return align ? head + '，' + align : head
}

function metaLines(m) {
  const out = []
  if (m.save) out.push('**豁免** ' + Object.keys(m.save).map(function (a) { return (ABIL[a] || a) + inline(m.save[a]) }).join('、'))
  if (m.skill) out.push('**技能** ' + Object.keys(m.skill).map(function (a) { return (SKILL[a] || a) + inline(m.skill[a]) }).join('、'))
  if (m.vulnerable) out.push('**伤害易伤** ' + dmgTypeList(m.vulnerable))
  if (m.resist) out.push('**伤害抗力** ' + dmgTypeList(m.resist))
  if (m.immune) out.push('**伤害免疫** ' + dmgTypeList(m.immune))
  if (m.conditionImmune) out.push('**状态免疫** ' + m.conditionImmune.map(inline).join('、'))
  if (m.senses && m.senses.length) out.push('**感官** ' + m.senses.map(inline).join('、') + (m.passive ? '；被动察觉 ' + m.passive : ''))
  if (m.languages) out.push('**语言** ' + (m.languages.length ? m.languages.map(inline).join('、') : '—'))
  const sp = speedStr(m.speed)
  if (sp) out.push('**速度** ' + sp)
  return out
}

function traitsText(m) {
  const parts = []
  for (const t of (m.trait || [])) parts.push('**' + inline(t.name) + '。** ' + entriesText(t.entries))
  for (const l of metaLines(m)) parts.push(l)
  return parts.join('\n')
}

function actionsText(m) {
  return (m.action || []).map(function (a) { return '**' + inline(a.name) + '。** ' + entriesText(a.entries) }).join('\n')
}

function rawOf(entries) { return JSON.stringify(entries || []) }

function attackProfile(action) {
  const raw = rawOf(action.entries)
  const atk = raw.match(/\{@atk\s+([^}]+)\}/)
  const tags = atk ? atk[1].split(',').map(function (s) { return s.trim() }) : []
  const hit = raw.match(/\{@hit\s+([^}]+)\}/)
  const isAttack = tags.length > 0 || !!hit
  let attribute
  if (tags.some(function (t) { return t === 'mw' || t === 'ms' })) attribute = 'str'
  else if (tags.indexOf('rw') >= 0) attribute = 'dex'
  else if (tags.indexOf('rs') >= 0) attribute = undefined
  else if (isAttack) attribute = 'str'
  return { isAttack: isAttack, attribute: attribute, tags: tags, hit: hit ? inline('{@hit ' + hit[1] + '}') : null }
}

function extractDamage(action) {
  const raw = rawOf(action.entries)
  const m = raw.match(/\{@damage\s+([^}]+)\}/) || raw.match(/\{@dice\s+([^}]+)\}/)
  if (!m) return null
  const expr = m[1].split('|')[0].trim().replace(/\s+/g, '')
  return expr || null
}

// ---------- 怪物条目构建 ----------

function buildBestiary(src) {
  const characters = []
  const skills = []
  const definitions = []
  const armorByMonster = {}
  const damageBySkill = {}
  const missing = []

  for (const sec of src.appendix) {
    const raw = src.bestiary.index.get(sec.name)
    if (!raw) { missing.push(sec.name); continue }
    const res = resolveCopy(raw, src.bestiary, new Set())
    const m = res.entry
    const eng = m.ENG_name || sec.name
    const id = 'mon-' + slug(eng)
    const hp = m.hp && typeof m.hp.average === 'number' ? m.hp.average : null

    // --- 攻击动作 -> skills[] ---
    const skillIds = []
    const actions = m.action || []
    for (let i = 0; i < actions.length; i++) {
      const a = actions[i]
      const aName = inline(a.name || ('动作 ' + (i + 1)))
      const prof = attackProfile(a)
      const damage = extractDamage(a)
      const skill = {
        id: 'sk-' + slug(eng) + '-' + (slug(a.ENG_name || a.name) || ('action-' + (i + 1))),
        name: aName,
        description: entriesText(a.entries),
        category: prof.isAttack ? '攻击' : '动作',
        target: 'single'
      }
      if (prof.attribute) skill.attribute = prof.attribute
      if (prof.isAttack) skill.check = { dice: '1d20', kind: 'attack' }
      if (damage) skill.effect = { immediate: [{ kind: 'damage', amount: damage, resource: 'res-hp' }] }
      skills.push(skill)
      skillIds.push(skill.id)
      damageBySkill[skill.id] = damage
    }

    // --- AC：无甲基准 + 挂接护甲修正 ---
    const ac = firstAc(m.ac)
    const dex = typeof m.dex === 'number' ? m.dex : 10
    const baseAc = 10 + Math.floor((dex - 10) / 2)
    const attachments = {}
    const statParts = []
    if (ac !== null && ac !== baseAc) {
      const from = Array.isArray(m.ac) ? acList(m.ac) : String(ac)
      const defId = 'armor-' + slug(eng)
      armorByMonster[id] = defId
      definitions.push({
        id: defId,
        kind: 'monster-armor',
        name: sec.name + ' · 护甲',
        description: '数据卡 AC ' + from + '（无甲基准 ' + baseAc + '，护甲修正 ' + (ac - baseAc) + '）',
        fields: { source: from, note: 'AC 由 derived.ac（10 + 敏捷调整值）加本挂接修正得到' },
        modifiers: [{ target: 'ac', value: ac - baseAc, op: 'add' }]
      })
      attachments['monster-armor'] = [defId]
      statParts.push('**护甲来源** ' + from + '（AC ' + ac + '）')
    }

    // --- 变体：开放内容挂接（不新增 extends） ---
    let variantDef = null
    let extraNote = ''
    if (res.copyOf) {
      const baseRaw = src.bestiary.index.get(res.copyOf)
      const baseEng = baseRaw && baseRaw.ENG_name ? baseRaw.ENG_name : res.copyOf
      const tpl = res.templates.length ? '（模板：' + res.templates.map(function (t) { return t.name }).join('、') + '）' : ''
      extraNote = '5e 数据卡 _copy：沿用「' + res.copyOf + '」数据卡' + tpl
      variantDef = {
        baseName: res.copyOf, baseEng: baseEng, baseId: 'mon-' + slug(baseEng),
        extra: '', source: extraNote
      }
    } else {
      const mm = String(sec.intro || '').match(/使用\s*([^\s，。；]{1,12}?)\s*资料卡/)
      if (mm && src.bestiary.index.get(mm[1])) {
        const baseRaw = src.bestiary.index.get(mm[1])
        const baseEng = baseRaw.ENG_name || mm[1]
        const own = (m.trait || []).map(function (t) { return inline(t.name) })
        const baseTraits = (baseRaw.trait || []).map(function (t) { return inline(t.name) })
        const extra = own.filter(function (n) { return baseTraits.indexOf(n) < 0 })
        extraNote = '附录 B 原文：使用' + mm[1] + '资料卡，并具有以下额外特性'
        variantDef = { baseName: mm[1], baseEng: baseEng, baseId: 'mon-' + slug(baseEng), extra: extra.join('、'), source: extraNote }
      }
    }
    if (variantDef) {
      const defId = 'variant-' + slug(eng)
      definitions.push({
        id: defId,
        kind: 'monster-variant',
        name: sec.name + ' · 变体',
        description: variantDef.source,
        fields: {
          base_creature: variantDef.baseId,
          extra_traits: variantDef.extra,
          source: variantDef.source
        }
      })
      attachments['monster-variant'] = [defId]
      statParts.push('**变体** ' + variantDef.source + (variantDef.extra ? '（额外特性：' + variantDef.extra + '）' : ''))
    }

    const traits = [traitsText(m)].concat(statParts).filter(Boolean).join('\n')
    const notes = '图鉴导入 · 5e 源 ' + (src.bestiary.origin.get(sec.name) || '?') + '（' + eng + (m.source ? '，' + m.source : '') + (m.page ? ' p' + m.page : '') + '）'

    const ch = {
      id: id,
      name: sec.name,
      kind: 'monster',
      background: truncate(stripMd(firstParagraph(sec.intro)), 240),
      personality: '',
      attributes: {
        str: m.str, dex: m.dex, con: m.con, int: m.int, wis: m.wis, cha: m.cha
      },
      resources: hp === null ? {} : { 'res-hp': hp },
      skills: skillIds,
      statblock: {
        creatureType: creatureTypeLabel(m),
        traits: traits,
        challenge: m.cr === undefined ? '' : String(m.cr),
        xp: XP[String(m.cr)] === undefined ? 0 : XP[String(m.cr)],
        actionsNote: actionsText(m)
      },
      notes: notes
    }
    if (Object.keys(attachments).length) ch.attachments = attachments
    characters.push(ch)
  }

  return {
    characters: characters,
    skills: skills,
    definitions: definitions,
    kinds: KINDS.slice(),
    missing: missing,
    armorByMonster: armorByMonster,
    damageBySkill: damageBySkill
  }
}

// ---------- 冒险：地点树 / 地图锚点 / 章节骨架 ----------

function indexAdventure(roots) {
  const byId = new Map()
  const parent = new Map()
  const order = new Map()
  let i = 0
  const walk = function (node, p) {
    if (!node || typeof node !== 'object' || typeof node === 'string') return
    order.set(node, i++)
    if (node.id) byId.set(node.id, node)
    parent.set(node, p)
    for (const c of (node.entries || [])) walk(c, node)
    for (const c of (node.items || [])) walk(c, node)
  }
  for (const r of roots) walk(r, null)
  return { byId: byId, parent: parent, order: order }
}

function isAreaNode(n) {
  return !!n && typeof n === 'object' && typeof n.name === 'string' && /^\d+[\.\s]/.test(n.name)
}

function areasOf(node, ctx) {
  const nested = (node.entries || []).filter(isAreaNode)
  if (nested.length) return nested
  const p = ctx.parent.get(node)
  if (!p || !Array.isArray(p.entries)) return []
  const idx = p.entries.indexOf(node)
  if (idx < 0) return []
  const out = []
  for (let i = idx + 1; i < p.entries.length; i++) {
    if (!isAreaNode(p.entries[i])) break
    out.push(p.entries[i])
  }
  return out
}

function nodeProse(node) {
  let found = ''
  const walk = function (n) {
    if (found || !n || typeof n !== 'object') return
    if (n.type === 'insetReadaloud' && Array.isArray(n.entries)) {
      const t = entriesText(n.entries)
      if (t) { found = t; return }
    }
    for (const c of (n.entries || [])) walk(c)
  }
  walk(node)
  if (!found) {
    for (const c of (node.entries || [])) {
      if (typeof c === 'string' && c.trim()) { found = inline(c); break }
    }
  }
  return found
}

function locationDesc(node) {
  const bits = []
  if (node.page) bits.push('第 ' + node.page + ' 页')
  if (node.ENG_name) bits.push(node.ENG_name)
  const prose = truncate(stripMd(nodeProse(node)), 160)
  return bits.join(' · ') + (prose ? ' · ' + prose : '')
}

function gridPins(ids) {
  const n = ids.length
  if (!n) return []
  const cols = Math.ceil(Math.sqrt(n))
  const rows = Math.ceil(n / cols)
  return ids.map(function (location_id, i) {
    return {
      location_id: location_id,
      x: round4(((i % cols) + 0.5) / cols),
      y: round4((Math.floor(i / cols) + 0.5) / rows)
    }
  })
}

function buildAdventure(src) {
  const ctx = indexAdventure((src.adventure.data || []))
  const locations = []
  const locIdOf = {}
  const chapterScenes = {}
  const locById = new Map()
  const partOfNode = new Map()

  const partIds = LOCATION_PLAN.map(function (p) { return p.part })
  const findPart = function (node) {
    let cur = node
    while (cur) {
      if (partIds.indexOf(cur.id) >= 0) return cur.id
      cur = ctx.parent.get(cur)
    }
    return null
  }

  for (const plan of LOCATION_PLAN) {
    chapterScenes[plan.part] = []
    for (const item of plan.nodes) {
      const node = ctx.byId.get(item.src)
      if (!node) throw new Error('冒险数据缺少 section id=' + item.src)
      const id = 'loc-lmop-' + item.src
      locIdOf[item.src] = id
      const loc = { id: id, name: node.name, description: locationDesc(node) }
      locations.push(loc)
      locById.set(id, loc)
      partOfNode.set(id, plan.part)
      chapterScenes[plan.part].push({ id: 'sc-lmop-' + item.src, title: node.name, description: truncate(stripMd(nodeProse(node)), 200), location_id: id, goals: [], triggers: [] })
      if (item.areas) {
        for (const area of areasOf(node, ctx)) {
          const aid = 'loc-lmop-' + area.id
          locIdOf[area.id] = aid
          const aloc = { id: aid, name: area.name, description: locationDesc(area), parent_id: id }
          locations.push(aloc)
          locById.set(aid, aloc)
          partOfNode.set(aid, plan.part)
          chapterScenes[plan.part].push({ id: 'sc-lmop-' + area.id, title: area.name, description: truncate(stripMd(nodeProse(area)), 200), location_id: aid, goals: [], triggers: [] })
        }
      }
    }
  }
  // 父地点：显式 parent 计划优先（凡达林的镇内地点 + 红标帮窝点）
  for (const plan of LOCATION_PLAN) {
    for (const item of plan.nodes) {
      if (!item.parent) continue
      const child = locById.get('loc-lmop-' + item.src)
      if (child) child.parent_id = 'loc-lmop-' + item.parent
    }
  }

  // 章节（骨架）
  const skeleton = []
  let ci = 0
  for (const plan of LOCATION_PLAN) {
    ci++
    const partNode = ctx.byId.get(plan.part)
    skeleton.push({
      id: 'ch-' + ci,
      title: PART_LABEL[plan.part] + ' · ' + (partNode ? partNode.name : plan.part),
      description: truncate(stripMd(nodeProse(partNode)), 200),
      scenes: chapterScenes[plan.part]
    })
  }

  // 地图 + 归一化锚点（0..1）
  const mapRoots = {}
  const maps = MAP_PLAN.map(function (plan) {
    let srcIds = plan.pins ? plan.pins.slice() : [plan.root]
    if (!plan.pins) {
      const node = ctx.byId.get(plan.root)
      const kids = node ? areasOf(node, ctx) : []
      srcIds = srcIds.concat(kids.map(function (k) { return k.id }))
    }
    const pinIds = srcIds.map(function (s) { return locIdOf[s] }).filter(Boolean)
    if (plan.root) mapRoots[plan.id] = locIdOf[plan.root]
    return {
      id: plan.id,
      name: plan.name,
      pins: gridPins(pinIds)
    }
  })

  return {
    locations: locations,
    maps: maps,
    skeleton: skeleton,
    mapFiles: MAP_PLAN.map(function (m) { return { id: m.id, name: m.name, file: m.file, credit: m.credit } }),
    mapRoots: mapRoots,
    locIdOf: locIdOf
  }
}

// ---------- 草稿组装 ----------

function buildDraft(src, report) {
  const bestiary = buildBestiary(src)
  const adventure = buildAdventure(src)
  if (report) { report.bestiary = bestiary; report.adventure = adventure }
  const overview = (src.adventure.data || [])[0]
  const premises = []
  const overviewNode = (function find(list) {
    for (const e of list || []) {
      if (typeof e === 'string' || !e) continue
      if (e.id === '008') return e
      const r = find(e.entries)
      if (r) return r
    }
    return null
  })(src.adventure.data)
  if (overviewNode) premises.push(truncate(stripMd(nodeProse(overviewNode)), 400))
  if (!premises.length && overview) premises.push(truncate(stripMd(nodeProse(overview)), 400))

  const openingNode = (function find(list) {
    for (const e of list || []) {
      if (typeof e === 'string' || !e) continue
      if (e.id === '00f') return e
      const r = find(e.entries)
      if (r) return r
    }
    return null
  })(src.adventure.data)

  const draft = {
    schema_version: 3,
    meta: {
      id: 'sb-lmop',
      title: '凡戴尔的失落矿坑（导入草稿）',
      author: 'scripts/import-bestiary.mjs',
      language: 'zh-CN',
      description: '由 5e 图鉴 JSON 与 5etools 冒险数据导入：' + bestiary.characters.length + ' 条 kind=monster 图鉴、'
        + adventure.locations.length + ' 个地点、' + adventure.maps.length + ' 张地图锚点、' + adventure.skeleton.length + ' 章骨架。'
        + '地图底图未随导入（image 留空），pins 为确定性占位布局，需人工按底图校准；本文件是草稿，脚本不写数据库。'
    },
    world: {
      premise: premises[0] || '凡达尔的失落矿坑：一支护送补给的队伍在三猪小径遭地精伏击，线索指向失落已久的法术锻造厂。',
      opening: openingNode ? truncate(entriesText(openingNode.entries), 600) : '',
      locations: adventure.locations,
      resources: RESOURCES.map(function (r) { return Object.assign({}, r) }),
      check: Object.assign({}, WORLD_CHECK),
      maps: adventure.maps
    },
    attribute_dimensions: DIMENSIONS.map(function (d) { return Object.assign({}, d) }),
    skeleton: adventure.skeleton,
    characters: bestiary.characters,
    skills: bestiary.skills,
    items: [],
    objects: [],
    factions: [],
    relationships: [],
    statuses: [],
    lore: [],
    kinds: bestiary.kinds,
    definitions: bestiary.definitions,
    derived: DERIVED.map(function (d) { return Object.assign({}, d) }),
    sheet: SHEET.map(function (s) { return Object.assign({}, s) }),
    flags: FLAGS.map(function (f) { return Object.assign({}, f) }),
    events: EVENTS.map(function (e) { return Object.assign({}, e) }),
    relationship_types: RELATIONSHIP_TYPES.map(function (r) { return Object.assign({}, r) }),
    target_types: TARGET_TYPES.map(function (t) { return Object.assign({}, t) })
  }
  return draft
}

// ---------- 派生值求值（与 frontend/src/lib/derived.ts 同语法同口径） ----------

function evalFormula(src, vars) {
  const s = src
  let i = 0
  const skip = function () { while (i < s.length && /\s/.test(s[i])) i++ }
  const peek = function () { skip(); return s[i] }
  function parseExpr() { return parseAdd() }
  function parseAdd() {
    let v = parseMul()
    for (;;) {
      const c = peek()
      if (c === '+') { i++; v += parseMul() } else if (c === '-') { i++; v -= parseMul() } else return v
    }
  }
  function parseMul() {
    let v = parseUnary()
    for (;;) {
      const c = peek()
      if (c === '*') { i++; v *= parseUnary() } else if (c === '/') { i++; v /= parseUnary() } else return v
    }
  }
  function parseUnary() {
    const c = peek()
    if (c === '-') { i++; return -parseUnary() }
    if (c === '+') { i++; return parseUnary() }
    return parsePrimary()
  }
  function applyFn(name, a) {
    if (name === 'floor') return Math.floor(a[0])
    if (name === 'ceil') return Math.ceil(a[0])
    if (name === 'round') return Math.round(a[0])
    if (name === 'abs') return Math.abs(a[0])
    if (name === 'min') return Math.min.apply(null, a)
    if (name === 'max') return Math.max.apply(null, a)
    throw new Error('未知函数 ' + name)
  }
  function parsePrimary() {
    const c = peek()
    if (c === '(') {
      i++
      const v = parseExpr()
      if (peek() !== ')') throw new Error('缺少右括号')
      i++
      return v
    }
    if (c !== undefined && /[0-9.]/.test(c)) {
      const start = i
      while (i < s.length && /[0-9.]/.test(s[i])) i++
      return Number(s.slice(start, i))
    }
    if (c !== undefined && /[A-Za-z_]/.test(c)) {
      const start = i
      while (i < s.length && /[A-Za-z0-9_.]/.test(s[i])) i++
      const name = s.slice(start, i)
      if (peek() === '(') {
        i++
        const args = []
        if (peek() !== ')') {
          args.push(parseExpr())
          while (peek() === ',') { i++; args.push(parseExpr()) }
        }
        if (peek() !== ')') throw new Error('缺少右括号')
        i++
        return applyFn(name, args)
      }
      if (!(name in vars)) throw new Error('未知变量 ' + name)
      return vars[name]
    }
    throw new Error('意外的字符 ' + String(c))
  }
  const out = parseExpr()
  skip()
  if (i < s.length) throw new Error('多余的输入 ' + s.slice(i))
  return out
}

function collectModifiers(draft, c) {
  const out = []
  const atts = c.attachments || {}
  for (const key of Object.keys(atts)) {
    for (const id of (atts[key] || [])) {
      const def = (draft.definitions || []).filter(function (x) { return x.id === id })[0]
      for (const m of ((def && def.modifiers) || [])) out.push(m)
    }
  }
  return out
}

function bucketModifiers(mods) {
  const add = {}
  const max = {}
  const set = {}
  for (const m of mods) {
    if (!m || typeof m.target !== 'string') continue
    if (m.op === 'set') set[m.target] = m.value
    else if (m.op === 'max') max[m.target] = Math.max(max[m.target] === undefined ? -Infinity : max[m.target], m.value)
    else add[m.target] = (add[m.target] || 0) + m.value
  }
  return { add: add, max: max, set: set }
}

function applyBuckets(base, key, b) {
  let v = base + (b.add[key] || 0)
  if (key in b.max) v = Math.max(v, b.max[key])
  if (key in b.set) v = b.set[key]
  return v
}

function computeDerived(draft, c) {
  const attrs = {}
  for (const dim of (draft.attribute_dimensions || [])) {
    if (dim.type !== 'number') continue
    const raw = (c.attributes || {})[dim.key]
    const n = typeof raw === 'number' ? raw : Number(raw)
    attrs[dim.key] = Number.isFinite(n) ? n : 0
  }
  const buckets = bucketModifiers(collectModifiers(draft, c))
  for (const k of Object.keys(attrs)) attrs[k] = applyBuckets(attrs[k], k, buckets)
  const vars = Object.assign({}, attrs)
  const derived = {}
  for (const def of (draft.derived || [])) {
    const v = applyBuckets(evalFormula(def.formula, vars), def.key, buckets)
    vars[def.key] = v
    derived[def.key] = v
  }
  return { attrs: attrs, derived: derived }
}

function findKeyDeep(value, key, hits, at) {
  const h = hits || []
  const p = at || '$'
  if (Array.isArray(value)) {
    value.forEach(function (v, i) { findKeyDeep(v, key, h, p + '[' + i + ']') })
    return h
  }
  if (value && typeof value === 'object') {
    for (const k of Object.keys(value)) {
      if (k === key) h.push(p + '.' + k)
      findKeyDeep(value[k], key, h, p + '.' + k)
    }
  }
  return h
}

// ---------- 自检 ----------

function checkMode(src) {
  const report = {}
  const draft = buildDraft(src, report)
  const built = report.bestiary
  const results = []
  const ok = function (id, cond, evidence) { results.push({ id: id, pass: !!cond, evidence: String(evidence === undefined ? '' : evidence) }) }
  const chars = draft.characters
  const byName = new Map(chars.map(function (c) { return [c.name, c] }))

  // 1. 图鉴条目
  ok('monsters.count', chars.length === src.appendix.length && chars.length > 0, chars.length + ' 条（附录 B 共 ' + src.appendix.length + ' 节）')
  ok('monsters.resolved', built.missing.length === 0, built.missing.length ? ('未解析：' + built.missing.join('、')) : '附录 B 全部条目在 5e 图鉴中命中')
  ok('monsters.kind', chars.every(function (c) { return c.kind === 'monster' }), '全部 kind=monster')
  ok('monsters.statblock', chars.every(function (c) { return c.statblock && typeof c.statblock.creatureType === 'string' && c.statblock.creatureType.length > 0 }), 'statblock.creatureType 齐备')
  ok('monsters.hp_resource', chars.every(function (c) { return typeof (c.resources || {})['res-hp'] === 'number' }), '每条都有 resources["res-hp"]')
  ok('world.hp_declared', (draft.world.resources || []).some(function (r) { return r.id === 'res-hp' }), 'world.resources 声明 res-hp')
  ok('world.ac_declared', (draft.derived || []).some(function (d) { return d.key === 'ac' }), 'derived 声明 ac：' + (draft.derived.filter(function (d) { return d.key === 'ac' })[0] || {}).formula)

  // 2. AC 由 derived 公式 + 挂接修正得到，并与数据卡逐条对齐
  let acFail = []
  for (const c of chars) {
    const raw = src.bestiary.index.get(c.name)
    const expected = firstAc(raw && raw.ac)
    if (expected === null) continue
    const got = computeDerived(draft, c).derived.ac
    if (got !== expected) acFail.push(c.name + '=' + got + '(期望 ' + expected + ')')
  }
  ok('monsters.ac_derived', acFail.length === 0, acFail.length ? acFail.join('；') : '全部条目的 derived.ac 等于数据卡 AC（无甲基准 + 挂接护甲修正）')

  // 3. 攻击动作 -> skills
  const attackSkills = draft.skills.filter(function (s) { return s.check && s.check.kind === 'attack' })
  ok('skills.attack_check', attackSkills.length > 0 && attackSkills.every(function (s) { return s.check.dice === '1d20' && s.check.kind === 'attack' }), attackSkills.length + ' 条攻击技能，check = {dice:"1d20", kind:"attack"}')
  const dmgBad = []
  for (const s of draft.skills) {
    const expected = built.damageBySkill[s.id] || null
    const list = (s.effect && s.effect.immediate) || []
    const got = list.filter(function (e) { return e.kind === 'damage' })[0]
    const gotAmount = got ? got.amount : null
    if (expected !== gotAmount) dmgBad.push(s.id + ' 期望 ' + expected + ' 实得 ' + gotAmount)
    if (got && got.resource !== 'res-hp') dmgBad.push(s.id + ' 伤害资源不是 res-hp')
  }
  ok('skills.damage_effect', dmgBad.length === 0 && draft.skills.some(function (s) { return s.effect && s.effect.immediate }), dmgBad.length ? dmgBad.join('；') : (attackSkills.length + ' 条攻击技能；命中攻击项的技能全部带 effect.immediate[damage] -> res-hp'))
  const refsOk = chars.every(function (c) { return (c.skills || []).every(function (id) { return draft.skills.some(function (s) { return s.id === id }) }) })
  ok('characters.skills_resolve', refsOk, 'characters[].skills 全部命中 storybook.skills')

  // 4. 变体：开放内容挂接，且不存在 extends
  const extendsHits = findKeyDeep(draft, 'extends', [], '$')
  ok('variants.no_extends', extendsHits.length === 0, extendsHits.length ? extendsHits.join('、') : '整份草稿不存在 extends 字段')
  const variants = (draft.definitions || []).filter(function (d) { return d.kind === 'monster-variant' })
  const variantOk = variants.every(function (d) {
    const host = chars.filter(function (c) { return c.attachments && (c.attachments['monster-variant'] || []).indexOf(d.id) >= 0 })[0]
    return !!host && !!d.fields && chars.some(function (c) { return c.id === d.fields.base_creature })
  })
  ok('variants.attached', variants.length > 0 && variantOk, variants.length + ' 条变体定义，全部经 attachments["monster-variant"] 挂接且基础数据卡存在')

  // 5. 抽样：LMoP 附录 B 灰烬丧尸
  const ash = byName.get('灰烬丧尸')
  const ashSec = src.appendix.filter(function (s) { return s.name === '灰烬丧尸' })[0]
  const ashDerived = ash ? computeDerived(draft, ash).derived.ac : null
  ok('sample.ash_zombie.exists', !!ash, ash ? ('id=' + ash.id + '，md 第 ' + ashSec.lineNumber + ' 行') : '缺失')
  ok('sample.ash_zombie.ac', ashDerived === 8 && ashSec.ac === 8, 'AC derived=' + ashDerived + ' / 附录 B=' + ashSec.ac)
  ok('sample.ash_zombie.hp', !!ash && ash.resources['res-hp'] === 22 && ashSec.hp === 22, 'HP resources.hp=' + (ash ? ash.resources['res-hp'] : '?') + ' / 附录 B=' + ashSec.hp)
  const ashAbil = ash ? ABIL_KEYS.map(function (k) { return ash.attributes[k] }) : []
  const ashMdAbil = ashSec.abilities || []
  ok('sample.ash_zombie.abilities', ashAbil.join(',') === '13,6,16,3,6,5' && ashMdAbil.join(',') === '13,6,16,3,6,5', '六维=' + ashAbil.join(',') + ' / 附录 B=' + ashMdAbil.join(','))
  const ashSlam = ash ? draft.skills.filter(function (s) { return (ash.skills || []).indexOf(s.id) >= 0 && s.check && s.check.kind === 'attack' })[0] : null
  ok('sample.ash_zombie.attack', !!ashSlam && ashSlam.check.dice === '1d20' && ashSlam.effect.immediate[0].amount === '1d6+1', ashSlam ? (ashSlam.name + ' check=' + JSON.stringify(ashSlam.check) + ' damage=' + ashSlam.effect.immediate[0].amount) : '缺攻击技能')
  const ashVariant = ash && ash.attachments && ash.attachments['monster-variant'] ? ash.attachments['monster-variant'][0] : null
  const ashVariantDef = ashVariant ? (draft.definitions || []).filter(function (d) { return d.id === ashVariant })[0] : null
  ok('sample.ash_zombie.variant', !!ashVariantDef && ashVariantDef.fields.base_creature === 'mon-zombie', ashVariantDef ? ('挂接 ' + ashVariantDef.id + ' -> 基础 ' + ashVariantDef.fields.base_creature + '，额外特性「' + ashVariantDef.fields.extra_traits + '」') : '缺变体挂接')

  // 6. 全书交叉核对：md 附录 B 中数值可解析的条目逐条对齐
  const mismatch = []
  let compared = 0
  for (const sec of src.appendix) {
    if (!sec.numbers) continue
    const c = byName.get(sec.name)
    if (!c) { mismatch.push(sec.name + ' 缺条目'); continue }
    compared++
    const raw = src.bestiary.index.get(sec.name)
    const jsonAc = firstAc(raw && raw.ac)
    const jsonHp = raw && raw.hp ? raw.hp.average : null
    const jsonAbil = ABIL_KEYS.map(function (k) { return raw ? raw[k] : null })
    const derivedAc = computeDerived(draft, c).derived.ac
    if (derivedAc !== sec.ac || jsonAc !== sec.ac) mismatch.push(sec.name + ' AC ' + derivedAc + '/' + jsonAc + ' vs ' + sec.ac)
    if (c.resources['res-hp'] !== sec.hp || jsonHp !== sec.hp) mismatch.push(sec.name + ' HP ' + c.resources['res-hp'] + '/' + jsonHp + ' vs ' + sec.hp)
    if (jsonAbil.join(',') !== sec.abilities.join(',')) mismatch.push(sec.name + ' 六维 ' + jsonAbil.join(',') + ' vs ' + sec.abilities.join(','))
  }
  ok('cross_check.appendix_b', mismatch.length === 0 && compared >= 25, compared + ' 条与附录 B 逐项对齐' + (mismatch.length ? ('；不一致：' + mismatch.join('；')) : ''))

  // 7. 地点 / 地图 / 骨架
  const locIds = new Set(draft.world.locations.map(function (l) { return l.id }))
  ok('locations.unique', locIds.size === draft.world.locations.length, draft.world.locations.length + ' 个地点，id 唯一')
  const badParent = draft.world.locations.filter(function (l) { return l.parent_id && !locIds.has(l.parent_id) })
  ok('locations.parent_tree', badParent.length === 0, badParent.length ? badParent.map(function (l) { return l.id }).join('、') : 'parent_id 全部命中（回声洞 > 第 N 区）')
  let cyclic = false
  for (const l of draft.world.locations) {
    const seen = new Set([l.id])
    let cur = l
    while (cur && cur.parent_id) {
      if (seen.has(cur.parent_id)) { cyclic = true; break }
      seen.add(cur.parent_id)
      cur = draft.world.locations.filter(function (x) { return x.id === cur.parent_id })[0]
    }
    if (cyclic) break
  }
  ok('locations.acyclic', !cyclic, '地点树无环')
  const maps = draft.world.maps || []
  const pins = maps.reduce(function (a, m) { return a.concat(m.pins || []) }, [])
  ok('maps.pins_normalized', maps.length > 0 && pins.length > 0 && pins.every(function (p) { return locIds.has(p.location_id) && p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1 }), maps.length + ' 张地图 / ' + pins.length + ' 个锚点，坐标全在 0..1 且地点存在')
  const mapRoots = (report.adventure && report.adventure.mapRoots) || {}
  const descendsFrom = function (locId, rootId) {
    const seen = new Set()
    let cur = draft.world.locations.filter(function (l) { return l.id === locId })[0]
    while (cur) {
      if (cur.id === rootId) return true
      if (seen.has(cur.id)) return false
      seen.add(cur.id)
      const pid = cur.parent_id
      cur = draft.world.locations.filter(function (l) { return l.id === pid })[0]
    }
    return false
  }
  const outOfScope = []
  for (const m of maps) {
    const root = mapRoots[m.id]
    if (!root) continue
    for (const p of (m.pins || [])) if (!descendsFrom(p.location_id, root)) outOfScope.push(m.id + ':' + p.location_id)
  }
  ok('maps.pins_in_scope', outOfScope.length === 0, outOfScope.length ? outOfScope.join('、') : '带 root 的地图锚点全部落在该地点子树内')
  const scenes = draft.skeleton.reduce(function (a, ch) { return a.concat(ch.scenes) }, [])
  ok('skeleton.scenes', draft.skeleton.length > 0 && scenes.length > 0 && scenes.every(function (s) { return locIds.has(s.location_id) }), draft.skeleton.length + ' 章 / ' + scenes.length + ' 场，场景地点全部存在')
  ok('draft.no_database_write', true, '脚本只读文件并写 JSON 草稿；--check 模式不写任何文件')

  // 8. 确定性
  const again = buildDraft(loadSources())
  ok('determinism.same_output', JSON.stringify(again) === JSON.stringify(draft), '同输入两次构建逐字节一致')

  const failed = results.filter(function (r) { return !r.pass })
  console.log('== LMoP 导入自检（node scripts/import-bestiary.mjs --check）==')
  for (const r of results) console.log((r.pass ? '[PASS] ' : '[FAIL] ') + r.id + ' — ' + r.evidence)
  console.log('总结：' + (results.length - failed.length) + '/' + results.length + ' 通过' + (failed.length ? '，失败：' + failed.map(function (f) { return f.id }).join('、') : ''))
  return failed.length ? 1 : 0
}

// ---------- vendored 源文件重建 ----------

function vendorFrom(dir) {
  const lmopSrc = path.join(dir, 'bestiary-lmop.json')
  const mmSrc = path.join(dir, 'bestiary-mm.json')
  const dmgSrc = path.join(dir, 'bestiary-dmg.json')
  if (!exists(lmopSrc)) throw new Error('缺少 ' + lmopSrc)
  const md = readText(F.md)
  const appendix = parseAppendixB(md)
  const lmop = readJson(lmopSrc)
  const lmopNames = new Set(((lmop.monster) || []).map(function (m) { return m.name }))
  const extra = []
  const extraFiles = [[mmSrc, 'bestiary-mm.json'], [dmgSrc, 'bestiary-dmg.json']]
  const pools = extraFiles.filter(function (p) { return exists(p[0]) }).map(function (p) { return { path: p[0], name: p[1], list: (readJson(p[0]).monster) || [] } })
  const origins = {}
  for (const sec of appendix) {
    if (lmopNames.has(sec.name)) continue
    let hit = null
    for (const pool of pools) {
      const m = pool.list.filter(function (x) { return x.name === sec.name })[0]
      if (m) { hit = { entry: m, file: pool.name }; break }
    }
    if (!hit) { console.error('[warn] 官方 5e 数据中未找到 ' + sec.name); continue }
    extra.push(hit.entry)
    origins[sec.name] = hit.file
  }
  fs.copyFileSync(lmopSrc, F.bestiaryLmop)
  const out = {
    _meta: {
      provenance: {
        note: '由 scripts/import-bestiary.mjs --vendor-from 从官方 5e 数据抽取：条目逐字段原样，未做改写。',
        sources: {
          'bestiary-lmop.json': 'https://5e.kiwee.top/data/bestiary/bestiary-lmop.json',
          'bestiary-mm.json': 'https://5e.kiwee.top/data/bestiary/bestiary-mm.json',
          'bestiary-dmg.json': 'https://5e.kiwee.top/data/bestiary/bestiary-dmg.json'
        },
        selection: '《凡戴尔的失落矿坑》附录 B 中不在 bestiary-lmop.json 内的条目（按附录顺序）',
        origins: origins
      },
      count: extra.length
    },
    monster: extra
  }
  fs.writeFileSync(F.bestiaryAppendix, JSON.stringify(out, null, '\t') + '\n')
  console.log('已写入 ' + path.relative(ROOT, F.bestiaryLmop) + '（原样复制）')
  console.log('已写入 ' + path.relative(ROOT, F.bestiaryAppendix) + '（' + extra.length + ' 条）')
  return 0
}

// ---------- CLI ----------

function loadSources() {
  const files = []
  if (exists(F.bestiaryLmop)) files.push({ name: 'bestiary-lmop.json', data: readJson(F.bestiaryLmop) })
  if (exists(F.bestiaryAppendix)) files.push({ name: 'bestiary-lmop-appendix-b.json', data: readJson(F.bestiaryAppendix) })
  if (exists(F.bestiaryMm)) files.push({ name: 'bestiary-mm.json', data: readJson(F.bestiaryMm) })
  if (exists(F.bestiaryDmg)) files.push({ name: 'bestiary-dmg.json', data: readJson(F.bestiaryDmg) })
  if (!files.length) throw new Error('缺少 5e 图鉴输入，请先运行 --vendor-from 或放入 bestiary-lmop.json')
  return {
    adventure: readJson(F.adventure),
    md: readText(F.md),
    appendix: parseAppendixB(readText(F.md)),
    bestiary: buildBestiaryIndex(files)
  }
}

function parseArgs(argv) {
  const out = { check: false, help: false, out: null, vendorFrom: null }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--check') out.check = true
    else if (a === '--help' || a === '-h') out.help = true
    else if (a === '--out') { out.out = argv[++i] }
    else if (a === '--vendor-from') { out.vendorFrom = argv[++i] }
    else throw new Error('未知参数 ' + a)
  }
  return out
}

function usage() {
  console.log('用法：node scripts/import-bestiary.mjs [--check] [--out <file>] [--vendor-from <dir>]')
  console.log('  （无参数）        生成 story_example/lmop-storybook.draft.json')
  console.log('  --check           自检并断言 LMoP 附录 B 数值，不写文件')
  console.log('  --vendor-from dir 从官方 5e 图鉴文件重建 story_example/bestiary-*.json')
}

function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.help) { usage(); return 0 }
  if (args.vendorFrom) return vendorFrom(args.vendorFrom)
  const src = loadSources()
  if (args.check) return checkMode(src)
  const draft = buildDraft(src)
  const target = args.out ? path.resolve(process.cwd(), args.out) : F.draft
  fs.writeFileSync(target, JSON.stringify(draft, null, 2) + '\n')
  console.log('已写入草稿 ' + path.relative(ROOT, target))
  console.log('  怪物 ' + draft.characters.length + ' 条 / 地点 ' + draft.world.locations.length + ' 个 / 地图 ' + draft.world.maps.length + ' 张 / 章节 ' + draft.skeleton.length + ' 章 / 技能 ' + draft.skills.length + ' 条 / 定义 ' + draft.definitions.length + ' 条')
  console.log('  未写数据库：脚本只产出 JSON 草稿，合并由人决定。')
  return 0
}

try {
  process.exitCode = main()
} catch (err) {
  console.error('[import-bestiary] ' + (err && err.message ? err.message : String(err)))
  process.exitCode = 1
}
