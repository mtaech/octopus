// ============================================================
// 结对请求的草稿上下文（带 id）：供模型引用既有实体做 update / delete 与建关系。
// 抽成共享函数，让「发送」与「侧栏 token 计量」用同一份口径。
// ============================================================
import type { Storybook } from '@/types'

export function buildStorybookContext(d: Storybook | null | undefined): Record<string, unknown> | undefined {
  if (!d) return undefined
  return {
    title: d.meta.title,
    description: d.meta.description,
    opening: d.world.opening,
    premise: d.world.premise,
    checker: d.world.check ?? null,
    characters: d.characters.map(c => ({
      id: c.id, name: c.name, kind: c.kind, background: c.background, personality: c.personality,
      skills: c.skills ?? [], inventory: c.inventory ?? [],
      // 开放挂接：解析成带定义的目标，模型才能看到种族/职业/特性等自定义内容
      attachments: Object.fromEntries(Object.entries(c.attachments ?? {}).map(([k, ids]) => [k, (ids ?? []).map(id => {
        const def = (d.definitions ?? []).find(x => x.id === id)
        return def ? { id, name: def.name, fields: def.fields ?? {} } : { id }
      })])),
    })),
    locations: d.world.locations.map(l => ({ id: l.id, name: l.name, description: l.description, parent_id: l.parent_id })),
    resources: d.world.resources.map(r => ({ id: r.id, name: r.name, type: r.type, default_max: r.default_max })),
    dimensions: d.attribute_dimensions.map(x => ({ key: x.key, label: x.label, type: x.type, options: x.options })),
    skeleton: d.skeleton.map(ch => ({
      id: ch.id,
      title: ch.title,
      scenes: ch.scenes.map(sc => ({
        id: sc.id,
        title: sc.title,
        location_id: sc.location_id,
        goals: sc.goals.map(g => ({ id: g.id, text: g.text, primary: g.primary })),
        triggers: sc.triggers.map(t => ({ id: t.id, title: t.title })),
      })),
    })),
    skills: d.skills.map(sk => ({ id: sk.id, name: sk.name, description: sk.description, category: sk.category })),
    items: d.items.map(it => ({ id: it.id, name: it.name, description: it.description, type: it.type })),
    objects: d.objects.map(o => ({ id: o.id, name: o.name, location_id: o.location_id })),
    factions: d.factions.map(f => ({ id: f.id, name: f.name, description: f.description })),
    relationships: d.relationships.map(r => ({ id: r.id, from_kind: r.from_kind, from_id: r.from_id, to_kind: r.to_kind, to_id: r.to_id, type: r.type, value: r.value })),
    statuses: (d.statuses ?? []).map(s => ({ id: s.id, name: s.name, duration: s.duration, unit: s.unit, stack: s.stack ?? null })),
    lore: (d.lore ?? []).map(l => ({ id: l.id, title: l.title, content: l.content, keys: l.keys ?? [], priority: l.priority ?? 0, constant: l.constant ?? false, recursive: l.recursive ?? false, enabled: l.enabled ?? true })),
    narrative: (d.narrative?.sections ?? []).map(s => ({ id: s.id, title: s.title, slot: s.slot, scope: s.scope, text: s.text ?? '', enabled: s.enabled !== false })),
    // 开放内容：种类 schema + 实例一并交给模型，模型无需预知种类
    kinds: (d.kinds ?? []).map(k => ({ key: k.key, label: k.label, fields: k.fields })),
    definitions: (d.definitions ?? []).map(x => ({ id: x.id, kind: x.kind, name: x.name, description: x.description, fields: x.fields ?? {} })),
    flags: d.flags,
    events: d.events,
    relationship_types: d.relationship_types,
    target_types: d.target_types,
  }
}
