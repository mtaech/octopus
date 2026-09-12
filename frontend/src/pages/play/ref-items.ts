// 游玩页引用项：在 EntityRef 上补「归属分组 + 卡片简介 + 小标签」，解决
// 「分不清我的技能 / 别人的技能 / 地图物品 / 世界规则」和「看不到基本信息」的问题。
import type { CharacterInstance, EntityRef, Storybook } from '@/types'
import { listEntityRefs } from '@/lib/entity-refs'

export interface RefItem extends EntityRef {
  /** 选择器里的分组标题 */
  group: string
  /** 右侧标注：技能归属人、物品数量… */
  tag?: string
  /** 卡片正文：一行简介 */
  desc?: string
  /** 卡片小标签：类型 / 消耗 / 目标 / 主线… */
  meta?: string[]
}

const GROUP_OF: Record<string, string> = {
  location: '地点', character: '人物',
  status: '规则 · 状态', dimension: '规则 · 属性维度', resource: '规则 · 资源',
  chapter: '剧情 · 章节', scene: '剧情 · 场景', goal: '剧情 · 目标', trigger: '剧情 · 触发点',
  world: '世界设定', meta: '世界设定',
}

/** 分组展示顺序（未列出的分组排在最后） */
const GROUP_ORDER = [
  '我的 · 技能', '我的 · 物品', '我的 · 状态',
  '他人 · 技能', '人物',
  '地点', '物品', '技能',
  '规则 · 状态', '规则 · 属性维度', '规则 · 资源',
  '剧情 · 章节', '剧情 · 场景', '剧情 · 目标', '剧情 · 触发点',
  '世界设定',
]

const STACK_LABEL: Record<string, string> = { replace: '替换', add: '层数相加', max: '取较高' }

export function playRefItems(
  sb: Storybook,
  controlled: CharacterInstance | null,
  present: CharacterInstance[],
): RefItem[] {
  const templates = new Map(sb.characters.map(c => [c.id, c]))
  const resourceName = (id: string) => sb.world?.resources?.find(r => r.id === id)?.name ?? id
  const mine = controlled ? templates.get(controlled.template_id) ?? null : null
  const sceneTitle = new Map<string, string>()
  for (const ch of sb.skeleton) for (const sc of ch.scenes) sceneTitle.set(sc.id, ch.title + ' · ' + sc.title)
  const sceneOf = (id?: string | null) => (id ? sb.skeleton.flatMap(c => c.scenes).find(s => s.id === id) : undefined)

  const items: RefItem[] = []
  const push = (r: RefItem) => items.push(r)

  const skillRef = (id: string, group: string, tag?: string): RefItem => {
    const s = sb.skills.find(x => x.id === id)
    const meta: string[] = []
    if (s?.category) meta.push(s.category)
    if (s?.cost?.length) meta.push('消耗 ' + s.cost.map(c => c.amount + ' ' + resourceName(c.resource)).join('、'))
    if (s?.target) meta.push('目标 ' + s.target)
    if (s?.cooldown?.turns) meta.push('冷却 ' + s.cooldown.turns)
    return { kind: 'skill', id, name: s?.name ?? id, parent_id: null, group, tag, desc: s?.description, meta }
  }
  const itemRef = (id: string, group: string, tag?: string): RefItem => {
    const i = sb.items.find(x => x.id === id)
    const meta: string[] = []
    if (i?.type) meta.push(i.type)
    if (i?.slot) meta.push('装备位 ' + i.slot)
    if (i?.damage) meta.push('伤害 ' + i.damage)
    return { kind: 'item', id, name: i?.name ?? id, parent_id: null, group, tag, desc: i?.description, meta }
  }
  const statusRef = (id: string, group: string, tag?: string): RefItem => {
    const s = (sb.statuses ?? []).find(x => x.id === id)
    const meta: string[] = []
    if (s) meta.push(s.duration + ' ' + (s.unit === 'turns' ? '回合' : '场景'))
    if (s?.stack) meta.push(STACK_LABEL[s.stack] ?? s.stack)
    return { kind: 'status', id, name: s?.name ?? id, parent_id: null, group, tag, desc: s?.description, meta }
  }

  // 我的：技能 / 物品 / 状态（来自受控角色的实例与模板）
  if (mine && controlled) {
    for (const id of mine.skills ?? []) push(skillRef(id, '我的 · 技能'))
    for (const [id, qty] of Object.entries(controlled.inventory ?? {})) push(itemRef(id, '我的 · 物品', '×' + qty))
    for (const s of controlled.statuses ?? []) push(statusRef(s.id, '我的 · 状态'))
  }

  // 他人的技能：按人标注「谁的」
  for (const c of present) {
    if (controlled && c.instance_id === controlled.instance_id) continue
    const t = templates.get(c.template_id)
    if (!t) continue
    for (const id of t.skills ?? []) push(skillRef(id, '他人 · 技能', c.name))
  }

  // 世界 / 规则 / 剧情（技能与物品单独处理，好带上归属标注）
  for (const r of listEntityRefs(sb)) {
    if (r.kind === 'skill' || r.kind === 'item') continue
    const group = GROUP_OF[r.kind]
    if (!group) continue
    let desc: string | undefined
    let meta: string[] | undefined
    switch (r.kind) {
      case 'location':
        desc = sb.world?.locations?.find(x => x.id === r.id)?.description
        break
      case 'status': {
        const s = (sb.statuses ?? []).find(x => x.id === r.id)
        desc = s?.description
        if (s) meta = [s.duration + ' ' + (s.unit === 'turns' ? '回合' : '场景')]
        break
      }
      case 'goal': {
        const g = sceneOf(r.parent_id)?.goals.find(x => x.id === r.id)
        desc = sceneTitle.get(r.parent_id ?? '')
        meta = [g?.primary ? '主线' : '支线', ...(g?.hidden ? ['隐藏'] : [])]
        break
      }
      case 'trigger': {
        const t = sceneOf(r.parent_id)?.triggers.find(x => x.id === r.id)
        desc = t?.description || t?.hint || sceneTitle.get(r.parent_id ?? '')
        if (t?.repeatable) meta = ['可重复']
        break
      }
      case 'scene': {
        desc = sceneOf(r.id)?.description
        const st = sceneTitle.get(r.id ?? '')
        if (st) meta = [st]
        break
      }
      case 'chapter':
        desc = sb.skeleton.find(c => c.id === r.id)?.description
        break
      case 'character': {
        const c = templates.get(r.id ?? '')
        desc = c?.background
        if (c?.kind) meta = [c.kind === 'pc' ? '玩家角色' : 'NPC']
        break
      }
      case 'dimension': {
        const d = sb.attribute_dimensions.find(x => x.key === r.id)
        const range = d && d.type === 'number' && d.min != null && d.max != null ? d.min + '–' + d.max : undefined
        desc = [d?.type, range].filter(Boolean).join(' · ')
        break
      }
      case 'resource': {
        const res = sb.world?.resources?.find(x => x.id === r.id)
        if (res) desc = (res.type === 'binary' ? '开关型' : '数值型') + (res.default_max != null ? ' · 上限 ' + res.default_max : '')
        break
      }
    }
    push({ ...r, group, desc, meta })
  }

  // 全部技能 / 物品：补归属标注
  for (const s of sb.skills) {
    const owners = present.filter(c => (templates.get(c.template_id)?.skills ?? []).includes(s.id))
    const tag = owners.length
      ? owners.map(o => (controlled && o.instance_id === controlled.instance_id ? '我' : o.name)).join('、')
      : undefined
    push(skillRef(s.id, '技能', tag))
  }
  for (const i of sb.items) push(itemRef(i.id, '物品'))

  return items
}

/** 按声明顺序组织成分组（组名 → 项）。 */
export function groupRefItems(items: RefItem[]): { group: string; items: RefItem[] }[] {
  const by = new Map<string, RefItem[]>()
  for (const it of items) {
    const list = by.get(it.group) ?? []
    list.push(it)
    by.set(it.group, list)
  }
  const out: { group: string; items: RefItem[] }[] = []
  for (const g of GROUP_ORDER) if (by.has(g)) out.push({ group: g, items: by.get(g)! })
  for (const [g, list] of by) if (!GROUP_ORDER.includes(g)) out.push({ group: g, items: list })
  return out
}

/** 过滤（名称 / 类型 / 分组 / 标注 / 简介 / 小标签） */
export function filterRefItems(items: RefItem[], query: string): RefItem[] {
  const q = query.trim().toLowerCase()
  if (!q) return items
  return items.filter(it =>
    it.name.toLowerCase().includes(q) ||
    it.kind.toLowerCase().includes(q) ||
    it.group.toLowerCase().includes(q) ||
    (it.tag ?? '').toLowerCase().includes(q) ||
    (it.desc ?? '').toLowerCase().includes(q) ||
    (it.meta ?? []).some(m => m.toLowerCase().includes(q)),
  )
}

/** 只把纯 EntityRef 交给 store / 后端 */
export function toEntityRef(it: RefItem): EntityRef {
  return { kind: it.kind, id: it.id, parent_id: it.parent_id ?? null, name: it.name }
}
