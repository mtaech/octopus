// ============================================================
// 故事书实体引用：枚举 / 解析 / 生成 focus。编辑器（结对）与游玩页共用。
// 编辑器的草稿是活的 Storybook；游玩页的是存档内冻结 Storybook——同一套形状。
// ============================================================
import type { EntityRef, FocusEntity, Storybook } from '@/types'
import { relationshipEnd } from '@/types'

/** 实体类型的中文标签（引用 chip 显示用）。 */
export const REF_KIND_LABEL: Record<string, string> = {
  meta: '元信息', world: '世界设定', character: '人物', location: '地点', resource: '资源',
  dimension: '属性维度', skill: '技能', item: '物品', object: '物件', faction: '势力',
  relationship: '关系', chapter: '章节', scene: '场景', goal: '目标', trigger: '触发点',
  status: '状态', flag: '标记', event: '事件', relationship_type: '关系类型', target_type: '目标类型',
  kind: '种类', definition: '开放内容',
}
export function refKindLabel(kind: string): string { return REF_KIND_LABEL[kind] ?? kind }

type Row = Record<string, unknown>

function rowKey(e: Row): string {
  return String(e.id ?? e.key ?? '')
}

interface SceneRef {
  chapter: { id: string; scenes: unknown[] }
  scene: Row
  sceneIndex: number
}

function findScene(d: Storybook, sceneId: string, parentId?: string): SceneRef | null {
  for (const chapter of d.skeleton) {
    if (parentId && chapter.id !== parentId) continue
    const sceneIndex = chapter.scenes.findIndex(sc => sc.id === sceneId)
    if (sceneIndex >= 0) {
      return { chapter: chapter as unknown as SceneRef['chapter'], scene: chapter.scenes[sceneIndex] as unknown as Row, sceneIndex }
    }
  }
  return null
}

/** 顶层 kind → 草稿里的实体数组（与 editor 的寻址保持一致）。 */
function kindList(d: Storybook, kind: string): Row[] | null {
  switch (kind) {
    case 'location': return d.world.locations as unknown as Row[]
    case 'resource': return d.world.resources as unknown as Row[]
    case 'dimension': return d.attribute_dimensions as unknown as Row[]
    case 'character': return d.characters as unknown as Row[]
    case 'skill': return d.skills as unknown as Row[]
    case 'item': return d.items as unknown as Row[]
    case 'object': return d.objects as unknown as Row[]
    case 'faction': return d.factions as unknown as Row[]
    case 'chapter': return d.skeleton as unknown as Row[]
    case 'relationship': return d.relationships as unknown as Row[]
    case 'status': return (d.statuses ?? []) as unknown as Row[]
    case 'lore': return (d.lore ?? []) as unknown as Row[]
    default: {
      const defs = (d.definitions ?? []).filter(x => x.kind === kind)
      return defs.length ? (defs as unknown as Row[]) : null
    }
  }
}

/** 声明区四类（按 key 寻址）。 */
function declList(d: Storybook, kind: string): { key: string; label?: string }[] | null {
  switch (kind) {
    case 'flag': return d.flags
    case 'event': return d.events
    case 'relationship_type': return d.relationship_types
    case 'target_type': return d.target_types
    default: return null
  }
}

/** 枚举故事书里所有可被引用的实体。 */
export function listEntityRefs(d: Storybook): EntityRef[] {
  const out: EntityRef[] = []
  const push = (kind: string, name: unknown, id?: string | null, parent_id?: string | null) => {
    const label = String(name ?? '').trim()
    if (!label) return
    out.push({ kind, name: label, id: id ?? null, parent_id: parent_id ?? null })
  }
  push('meta', d.meta?.title || '故事书元信息')
  push('world', '世界设定')
  d.characters?.forEach(c => push('character', c.name, c.id))
  d.world?.locations?.forEach(l => push('location', l.name, l.id))
  d.world?.resources?.forEach(r => push('resource', r.name, r.id))
  d.attribute_dimensions?.forEach(x => push('dimension', x.label || x.key, x.key))
  d.skills?.forEach(s => push('skill', s.name, s.id))
  d.items?.forEach(i => push('item', i.name, i.id))
  d.objects?.forEach(o => push('object', o.name, o.id))
  d.factions?.forEach(f => push('faction', f.name, f.id))
  d.relationships?.forEach(r => push('relationship', relationshipEnd(r, 'from') + ' → ' + relationshipEnd(r, 'to') + ' · ' + r.type, r.id))
  d.skeleton?.forEach(ch => {
    push('chapter', ch.title, ch.id)
    ch.scenes?.forEach(sc => {
      push('scene', sc.title, sc.id, ch.id)
      sc.goals?.forEach(g => push('goal', g.text, g.id, sc.id))
      sc.triggers?.forEach(t => push('trigger', t.title, t.id, sc.id))
    })
  })
  d.statuses?.forEach(s => push('status', s.name, s.id))
  d.lore?.forEach(l => push('lore', l.title, l.id))
  // 开放内容：kind 由数据声明，这里无需逐 kind 登记
  d.definitions?.forEach(x => push(x.kind, x.name, x.id))
  d.flags?.forEach(x => push('flag', x.label || x.key, x.key))
  d.events?.forEach(x => push('event', x.label || x.key, x.key))
  d.relationship_types?.forEach(x => push('relationship_type', x.label || x.key, x.key))
  d.target_types?.forEach(x => push('target_type', x.label || x.key, x.key))
  return out
}

/** 把一条引用解析为完整实体定义（找不到返回 undefined）。 */
export function resolveEntityRef(d: Storybook, ref: EntityRef): unknown {
  switch (ref.kind) {
    case 'meta': return d.meta
    case 'world': return d.world
    case 'scene':
      return findScene(d, ref.id ?? '', ref.parent_id ?? undefined)?.scene
    case 'goal':
    case 'trigger': {
      const scene = findScene(d, ref.parent_id ?? '')?.scene
      if (!scene) return undefined
      const pool = (ref.kind === 'goal' ? scene.goals : scene.triggers) as unknown as Row[] | undefined
      return pool?.find(x => rowKey(x) === ref.id)
    }
    case 'flag':
    case 'event':
    case 'relationship_type':
    case 'target_type':
      return declList(d, ref.kind)?.find(x => x.key === ref.id)
    default:
      return kindList(d, ref.kind)?.find(e => rowKey(e) === ref.id)
  }
}

/** 把引用列表解析成带完整定义的 focus（给 AI）。 */
export function resolveFocus(d: Storybook, refs: EntityRef[]): FocusEntity[] {
  const out: FocusEntity[] = []
  for (const r of refs) {
    const entity = resolveEntityRef(d, r)
    if (entity !== undefined) out.push({ kind: r.kind, id: r.id ?? null, name: r.name, entity })
  }
  return out
}
