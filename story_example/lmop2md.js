#!/usr/bin/env node
// Convert 5etools adventure JSON + aux data files to clean Chinese markdown.
// 重新生成所需数据文件（下载到脚本同目录）：
//   curl -O https://5e.kiwee.top/data/adventure/adventure-lmop.json  (改名 lmop.json)
//   curl -O https://5e.kiwee.top/data/items.json
//   curl -O https://5e.kiwee.top/data/bestiary/bestiary-lmop.json
//   curl -O https://5e.kiwee.top/data/bestiary/bestiary-mm.json
//   curl -O https://5e.kiwee.top/data/bestiary/bestiary-dmg.json
//   curl -O https://5e.kiwee.top/data/bestiary/fluff-bestiary-lmop.json
//   curl -O https://5e.kiwee.top/data/bestiary/fluff-bestiary-mm.json
const fs = require('fs');
const J = p => JSON.parse(fs.readFileSync(p, 'utf8'));

const { data } = J('lmop.json');
const ITEMS = J('items.json').item || [];
const MONSTERS = [...J('bestiary-lmop.json').monster, ...J('bestiary-mm.json').monster, ...J('bestiary-dmg.json').monster];
const FLUFF = [...(J('fluff-lmop.json').monsterFluff || []), ...(J('fluff-mm.json').monsterFluff || [])];

// Prefer the book's own (LMoP) entry over later reprints when names collide.
const itemByName = new Map();
for (const i of [...ITEMS].sort((a, b) => (a.source === 'LMoP' ? -1 : 1) - (b.source === 'LMoP' ? -1 : 1))) {
  if (!itemByName.has(i.name)) itemByName.set(i.name, i);
}

const monsterByName = new Map(MONSTERS.map(m => [m.name, m]));
const fluffByName = new Map(FLUFF.map(f => [f.name, f]));

const IMG_BASE = 'https://5e.kiwee.top/img/';
const OUT = '凡戴尔的失落矿坑.md';

const out = [];
const push = s => out.push(s === undefined ? '' : s);

// ---------- data tables ----------
const XP = { '0': 10, '1/8': 25, '1/4': 50, '1/2': 100, '1': 200, '2': 450, '3': 700, '4': 1100, '5': 1800, '6': 2300, '7': 2900, '8': 3900, '9': 5000, '10': 5900, '11': 7200, '12': 8400, '13': 10000, '14': 11500, '15': 13000, '16': 15000, '17': 18000, '18': 20000, '19': 22000, '20': 25000, '21': 33000, '22': 41000, '23': 50000, '24': 62000, '25': 155000, '26': 155000, '27': 155000, '28': 155000, '29': 155000, '30': 155000 };
const SIZE = { T: '微型', S: '小型', M: '中型', L: '大型', H: '巨型', G: '超巨型' };
const TYPE = { aberration: '异怪', beast: '野兽', celestial: '天界生物', construct: '构装体', dragon: '龙', elemental: '元素', fey: '精类', fiend: '邪魔', giant: '巨人', humanoid: '类人生物', monstrosity: '怪兽', ooze: '泥怪', plant: '植物', undead: '不死生物' };
const ABIL = { str: '力量', dex: '敏捷', con: '体质', int: '智力', wis: '睿智', cha: '魅力' };
const SKILL = { acrobatics: '特技', animal: '驯兽', arcana: '奥秘', athletics: '运动', deception: '欺瞒', history: '历史', insight: '洞悉', intimidation: '威吓', investigation: '调查', medicine: '医药', nature: '自然', perception: '察觉', performance: '表演', persuasion: '说服', religion: '宗教', sleight: '巧手', stealth: '隐匿', survival: '求生' };
const ITEMTYPE = { A: '护甲', W: '武器', P: '药水', R: '戒指', RG: '戒指', RD: '权杖', ST: '法杖', SC: '卷轴', WND: '魔杖', WD: '奇物', S: '盾牌', T: '工具', M: '杂项', G: '通用物品', EXP: '消耗品' };
const RARITY = { common: '普通', uncommon: '非普通', rare: '稀有', 'very rare': '非常稀有', legendary: '传说', artifact: '神器', varies: '视情况', unknown: '特殊', none: '特殊' };
const MOD = s => { const m = Math.floor((s - 10) / 2); return (m >= 0 ? `+${m}` : `−${-m}`); };

// ---------- inline tag conversion ----------
const AREA_TAGS = { area: v => `第 ${v} 区`, dc: v => `DC ${v}`, dice: v => v, damage: v => v };
const ITALIC_TAGS = new Set(['item', 'spell']);
const BOLD_TAGS = new Set(['creature', 'hazard']);
const PLAIN_TAGS = new Set(['skill', 'condition', 'status', 'sense', 'quickref', 'language']);

function inline(str) {
  if (!str) return '';
  let s = String(str)
    .replace(/\n\n+/g, '\n\n')
    .replace(/\n/g, ' ');
  // Replace innermost tags first so nested {...} inside {@b ...} works.
  let prev;
  do {
    prev = s;
    s = s.replace(/\{@([a-zA-Z]+)\s*([^{}]*)\}/g, (m, tag, content) => {
      const value = content.split('|')[0].trim();
      if (tag === 'b') return `**${value}**`;
      if (tag === 'i') return `*${value}*`;
      if (tag === 'atk') return /rw/.test(value) && /mw/.test(value) ? '近战或远程武器攻击' : /mw/.test(value) ? '近战武器攻击' : /rw/.test(value) ? '远程武器攻击' : '武器攻击';
      if (tag === 'hit') return value.startsWith('-') || value.startsWith('+') ? value : `+${value}`;
      if (tag === 'h') return '命中：';
      if (tag === 'recharge') return `（充能 ${value}–6）`;
      if (tag === 'chance') return `${value}%`;
      if (AREA_TAGS[tag]) return AREA_TAGS[tag](value);
      if (ITALIC_TAGS.has(tag)) return `*${value}*`;
      if (BOLD_TAGS.has(tag)) return `**${value}**`;
      if (PLAIN_TAGS.has(tag)) return value;
      return value; // unknown tag: keep visible text
    });
  } while (s !== prev);
  return s;
}

// ---------- block conversion ----------
function blockLines(entry, depth, skipHeadingName, plainHeading) {
  const lines = [];
  const h = '#'.repeat(Math.min(depth, 4));

  if (typeof entry === 'string') { lines.push(inline(entry), ''); return lines; }
  const { type } = entry;
  const name = entry.name ? inline(entry.name) : null;

  switch (type) {
    case 'section':
    case 'entries':
      if (name) lines.push(plainHeading ? `**${name}**` : `${h} ${name}`, '');
      for (const e of entry.entries || []) lines.push(...blockLines(e, depth + 1, name, plainHeading));
      break;

    case 'inset': {
      const body = (entry.entries || []).map(e => blockLines(e, depth + 1).join('\n').trim()).filter(Boolean);
      lines.push('> ' + (name ? `**${name}**` : ''));
      for (const para of body) for (const line of para.split('\n')) lines.push('> ' + line);
      lines.push('');
      break;
    }

    case 'insetReadaloud': {
      const body = (entry.entries || []).map(e => blockLines(e, depth + 1).join('\n').trim()).filter(Boolean);
      lines.push('> **📖 朗读文本**');
      for (const para of body) for (const line of para.split('\n')) lines.push('> ' + line);
      lines.push('');
      break;
    }

    case 'list': {
      const items = entry.items || [];
      const hang = entry.style === 'list-hang-notitle';
      for (const it of items) {
        if (typeof it === 'string') lines.push(`- ${inline(it)}`);
        else if (it && typeof it === 'object') {
          const label = it.name ? inline(it.name) : null;
          const body = it.entry ? inline(it.entry) : (it.entries || []).map(inline).join(' ');
          lines.push(`- ${hang && label ? `**${label}**：${body}` : body}`);
        }
      }
      lines.push('');
      break;
    }

    case 'table': {
      const labels = (entry.colLabels || []).map(inline);
      const rows = entry.rows || [];
      const cols = Math.max(labels.length, ...rows.map(r => r.length), 1);
      lines.push('| ' + labels.concat(Array(cols - labels.length).fill('')).join(' | ') + ' |');
      lines.push('|' + Array(cols).fill(' --- ').join('|') + '|');
      for (const row of rows) {
        const cells = row.map(inline).concat(Array(Math.max(0, cols - row.length)).fill(''));
        lines.push('| ' + cells.join(' | ') + ' |');
      }
      lines.push('');
      break;
    }

    case 'gallery':
      for (const img of entry.images || []) lines.push(...blockLines(img, depth));
      break;

    case 'image': {
      const path = entry.href && entry.href.path ? entry.href.path : '';
      const url = path ? IMG_BASE + encodeURI(path) : '';
      const title = inline(entry.title || '地图');
      if (url) lines.push(`![${title}](${url})`);
      else lines.push(`> 地图：${title}`);
      if (entry.credit) lines.push(`> *来源：${inline(entry.credit)}*`);
      lines.push('');
      break;
    }

    case 'statblock':
      if (entry.prop === 'monsterFluff') lines.push(...fluffBlock(entry.name, depth));
      else if (entry.tag === 'creature') lines.push(...monsterBlock(entry.name, skipHeadingName === entry.name));
      else if (entry.tag === 'item') lines.push(...itemBlock(entry.name, depth, skipHeadingName === entry.name));
      else lines.push(...placeholderBlock(entry, name));
      break;

    default:
      if (name) lines.push(`**${name}**`, '');
      for (const e of entry.entries || []) lines.push(...blockLines(e, depth));
      for (const e of entry.items || []) lines.push(...blockLines(e, depth));
  }
  return lines;
}

// Flatten arbitrary nested entry (list/entries/inset/...) to one line of text.
function entryToText(e) {
  if (typeof e === 'string') return inline(e);
  if (!e || typeof e !== 'object') return '';
  if (e.type === 'list') return (e.items || []).map(i => typeof i === 'string' ? inline(i) : (i.name ? `${inline(i.name)}：` : '') + inline(i.entry || '')).join('；');
  const sub = (e.entries || []).map(entryToText).join(' ');
  return e.name ? `${inline(e.name)}：${sub}` : sub;
}

// ---------- stat block renderers ----------
function monsterBlock(name, skipName) {
  const m = monsterByName.get(name);
  const lines = [];
  if (!m) return lines.concat([`> **${name}** *(怪物数据，详见附录 B)*`, '']);
  if (!skipName) lines.push(`### ${inline(m.name)}`, '');

  // meta line: size type(，tags)，alignment
  const t = m.type;
  const typeStr = typeof t === 'string' ? (TYPE[t] || t) : t ? ((TYPE[t.type] || t.type) + (t.tags ? `（${t.tags.map(inline).join('、')}）` : '')) : '';
  const sizeStr = SIZE[m.size?.[0]] || '';
  lines.push(`> *${[sizeStr, typeStr].filter(Boolean).join(' ')}，${alignmentStr(m.alignment)}*`);
  lines.push('>');
  lines.push(`> **护甲等级** ${acStr(m.ac)}`);
  if (m.hp) lines.push(`> **生命值** ${m.hp.average}（${m.hp.formula}）`);
  if (m.speed) lines.push(`> **速度** ${speedStr(m.speed)}`);
  lines.push('>');
  const attrs = ['str', 'dex', 'con', 'int', 'wis', 'cha'].map(a => `${m[a]}（${MOD(m[a])}）`);
  lines.push('> | 力量 | 敏捷 | 体质 | 智力 | 睿智 | 魅力 |');
  lines.push('> |:---:|:---:|:---:|:---:|:---:|:---:|');
  lines.push('> | ' + attrs.join(' | ') + ' |');
  lines.push('>');
  if (m.save) lines.push(`> **豁免** ${Object.entries(m.save).map(([a, v]) => `${ABIL[a] || a}${inline(v)}`).join('、')}`);
  if (m.skill) lines.push(`> **技能** ${Object.entries(m.skill).map(([a, v]) => `${SKILL[a] || a}${inline(v)}`).join('、')}`);
  if (m.vulnerable) lines.push(`> **伤害易伤** ${dmgTypeList(m.vulnerable)}`);
  if (m.resist) lines.push(`> **伤害抗力** ${dmgTypeList(m.resist)}`);
  if (m.immune) lines.push(`> **伤害免疫** ${dmgTypeList(m.immune)}`);
  if (m.conditionImmune) lines.push(`> **状态免疫** ${m.conditionImmune.map(inline).join('、')}`);
  if (m.senses?.length) lines.push(`> **感官** ${m.senses.map(inline).join('、')}${m.passive ? `；被动察觉 ${m.passive}` : ''}`);
  if (m.languages) lines.push(`> **语言** ${m.languages.length ? m.languages.map(inline).join('、') : '—'}`);
  if (m.cr) lines.push(`> **挑战等级** ${m.cr}（${XP[m.cr] ?? '—'} XP）`);
  lines.push('');

  const spellcast = (m.spellcasting || []).map(sc => `**${inline(sc.name)}。** ${(sc.headerEntries || []).map(inline).join(' ')}\n${spellsStr(sc)}`);
  const group = (arr, label) => {
    if (!arr?.length) return;
    lines.push(`**${label}**`, '');
    for (const t of arr) {
      const head = t.name ? `**${inline(t.name)}。** ` : '';
      const textParts = [];
      const subLists = [];
      for (const e of t.entries || []) {
        if (typeof e === 'string') textParts.push(inline(e));
        else if (e?.type === 'list') {
          for (const it of e.items || []) {
            if (typeof it === 'string') subLists.push(`- ${inline(it)}`);
            else if (it && typeof it === 'object') {
              const label = it.name ? inline(it.name) : '';
              const body = it.entry ? inline(it.entry) : (it.entries || []).map(entryToText).join(' ');
              subLists.push(`- ${label ? `**${label}**：` : ''}${body}`);
            }
          }
        } else textParts.push(entryToText(e));
      }
      lines.push(head + textParts.join(' '), '');
      lines.push(...subLists, '');
    }
  };
  group(m.trait, '特性');
  for (const sc of spellcast) { lines.push(sc, ''); }
  group(m.action, '动作');
  group(m.bonus, '附赠动作');
  group(m.reaction, '反应');
  group(m.legendary, '传奇动作');
  if (m.lair?.length) {
    const intro = m.lair.find(x => !x.name);
    if (intro) lines.push((intro.entries || []).map(inline).join(' '), '');
    group(m.lair.filter(x => x.name), '巢穴动作');
  }
  return lines;
}

function spellsStr(sc) {
  const rows = [];
  for (const [lvl, data] of Object.entries(sc.spells || {})) {
    const list = (data.spells || []).map(inline).join('、');
    if (lvl === '0') rows.push(`戏法（随意）：${list}`);
    else if (lvl === 'constant') rows.push(`常驻效果：${list}`);
    else rows.push(`${lvl}环${data.slots ? `（${data.slots}个法术位）` : ''}：${list}`);
  }
  return rows.map(r => `  - ${r}`).join('\n');
}

function alignmentStr(a) {
  if (!a) return '';
  if (typeof a === 'string') return a === 'U' ? '无阵营' : (a === 'ANY' ? '任何阵营' : a);
  if (a[0] === 'U') return '无阵营';
  if (a[0] === 'ANY' || a[0] === 'A') return '任何阵营';
  const LAW = { L: '守序', N: '中立', C: '混乱' }, GOOD = { G: '善良', N: '中立', E: '邪恶' };
  if (a[0] === 'N' && (a[1] === 'N' || !a[1])) return '绝对中立';
  return `${LAW[a[0]] || ''}${GOOD[a[1]] || ''}` || '未知';
}

function acStr(ac) {
  if (Array.isArray(ac)) return ac.map(x => typeof x === 'number' ? x : (x.ac + (x.from ? `（${x.from.map(inline).join('、')}）` : ''))).join('、');
  return String(ac);
}

function speedStr(sp) {
  const order = ['walk', 'burrow', 'climb', 'fly', 'swim'];
  const label = { walk: '', burrow: '掘地 ', climb: '攀爬 ', fly: '飞行 ', swim: '游泳 ' };
  const parts = [];
  for (const k of order) {
    const v = sp[k];
    if (!v) continue;
    if (typeof v === 'number') parts.push(`${label[k]}${v}尺`);
    else parts.push(`${label[k]}${v.number}尺${v.condition ? v.condition : ''}`);
  }
  if (sp.canHover || sp.hover) parts.push('（悬停）');
  return parts.join('，') || '—';
}

function dmgTypeList(arr) {
  return (arr || []).map(x => typeof x === 'string' ? inline(x) : `${(x.resist || x.immune || x.vulnerable || []).map(inline).join('、')}${x.note ? `（${inline(x.note)}）` : ''}`).join('、');
}

function fluffBlock(name, depth) {
  const f = fluffByName.get(name);
  if (!f) return [];
  const lines = [];
  for (const e of f.entries || []) lines.push(...blockLines(e, depth + 1, null, true));
  return lines;
}

function itemBlock(name, depth, skipName) {
  const it = itemByName.get(name);
  const lines = [];
  if (!it) return lines.concat([`> **${name}** *(魔法物品数据本站未收录)*`, '']);
  if (!skipName) lines.push(`### ${inline(it.name)}`, '');
  const rawType = it.type ? String(it.type).split('|')[0] : (it.wondrous ? 'WD' : null);
  const typ = rawType ? (ITEMTYPE[rawType] || '魔法物品') : '魔法物品';
  const rar = RARITY[it.rarity] || (it.rarity || '特殊');
  const attune = it.reqAttune === true ? '，需同调' : (typeof it.reqAttune === 'string' ? `，需同调：${inline(it.reqAttune)}` : '');
  lines.push(`*${typ}，${rar}${attune}*`, '');
  for (const e of it.entries || []) lines.push(...blockLines(e, depth + 1, null, true));
  return lines;
}

function placeholderBlock(entry, name) {
  return name ? [`> **${name}** *(${entry.tag === 'item' ? '魔法物品，详见附录 A' : '怪物数据，详见附录 B'})*`, ''] : [''];
}

function block(entry, depth) {
  for (const l of blockLines(entry, depth)) push(l);
}

// ---------- top ----------
push(`# 《凡戴尔的失落矿坑》(Lost Mine of Phandelver)`);
push('');
push('> 本文档由 5etools 中文镜像（https://5e.kiwee.top/adventure.html#lmop）的冒险数据自动转换生成。');
push('> 原模组面向地下城主（DM），适合 4–5 名 1 级角色，冒险结束后角色提升至 5 级。');
push('');

for (const section of data) block(section, 1);

fs.writeFileSync(OUT, out.join('\n'));
console.log(`done: ${OUT}, ${out.join('\n').length} chars`);
