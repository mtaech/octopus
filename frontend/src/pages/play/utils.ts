// ============================================================
// 游玩页小工具（头像色散列 / 判定成功度中文映射 / 格式化）
// ============================================================
import type { AssetRef, ChapterDef, CharacterInstance, EncounterView, QuestView, SuccessLevel, WorldProjection } from '@/types'

/**
 * 投影里 characters 以 template_id 为 key，而 instance_id 是带前缀的另一个值
 * （inst-char-xxx）。凡是判断「这个角色是不是受控角色」都必须同时认两个 id。
 */
export function isControlledChar(controlledId: string, c: CharacterInstance): boolean {
  return !!controlledId && (c.template_id === controlledId || c.instance_id === controlledId)
}

/** 按任意一种 id（投影 key / template_id / instance_id）找角色实例。 */
export function findCharacter(p: WorldProjection | null, id: string): CharacterInstance | null {
  if (!p || !id) return null
  const direct = p.characters[id]
  if (direct) return direct
  return Object.values(p.characters).find(c => c.template_id === id || c.instance_id === id) ?? null
}

/**
 * 从存档内嵌的冻结故事书里查人物立绘（#28）。
 * 先按 id 精确匹配，其次按名字兜底（事件里的 actor.id 有时是实例 id）。
 */
export function portraitOf(storybook: unknown, id?: string, name?: string): AssetRef | null {
  const chars = (storybook as { characters?: { id?: string; name?: string; portrait?: AssetRef }[] } | undefined)?.characters
  if (!chars?.length) return null
  return (id ? chars.find(c => c.id === id)?.portrait : undefined)
    ?? (name ? chars.find(c => c.name === name)?.portrait : undefined)
    ?? null
}

function hash(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) { h = (h * 31 + s.charCodeAt(i)) | 0 }
  return Math.abs(h)
}
// 柔和语义底：按角色散列取一组语义色淡底（只用主题令牌，不写死 hex）
const TINT_IDX = ['bg-primary/20 text-primary', 'bg-info/20 text-info', 'bg-success/20 text-success', 'bg-warning/25 text-warning', 'bg-secondary text-secondary-foreground', 'bg-accent/60 text-accent-foreground']
const BORDER_IDX = ['border-l-primary/60', 'border-l-info/60', 'border-l-success/60', 'border-l-warning/60', 'border-l-border', 'border-l-accent-foreground/40']
function tintIndex(name: string): number { return hash(name) % TINT_IDX.length }
export function nameTintClass(name: string): string { return TINT_IDX[tintIndex(name)] }
export function nameBorderClass(name: string): string { return BORDER_IDX[tintIndex(name)] }

export function initial(name: string): string { return name.trim().slice(0, 1) || '?' }

/** 单位类别标签（PC / NPC / 怪物）：地点栏、地图面板、数据卡共用一套说法。 */
export function kindLabel(kind: string): string {
  return kind === 'monster' ? '怪物' : kind === 'npc' ? 'NPC' : 'PC'
}

// ---------- 按位置聚合（地图与在场 P3/P6 §6.7） ----------

/** 未落实例的临时敌人条目（图鉴 M1 之前 / AI 现编）：只存名字，不可点开数据卡。 */
export interface LooseEnemy { id: string; name: string; encounter: string }

/** 某个地点上的单位分桶：PC / NPC / 怪物实例，外加尚未落实例的临时敌人。 */
export interface LocationUnits {
  pc: CharacterInstance[]
  npc: CharacterInstance[]
  /** kind='monster' 的怪物实例（遭遇克隆产物，图鉴 M2） */
  monster: CharacterInstance[]
  /** 没有 instance_id 的临时敌人：按敌人自身的位置归位 */
  loose: LooseEnemy[]
}

/**
 * 怪物实例 id 到遭遇快照地点的索引（地图 P5 §6.4）。
 *
 * 遭遇创建时把 location_id 快照下来，克隆出的怪物实例继承它；但旧日志 / 触发点预置路径
 * 可能只留下遭遇这一侧的地点，实例自身没有 location_id。前端不能因此把它丢掉——
 * 这张表就是「实例缺失时用 EncounterView.location_id 兜底」的索引（T7 衔接点）。
 */
function encounterLocationIndex(p: WorldProjection | null): Map<string, string> {
  const out = new Map<string, string>()
  for (const enc of p?.encounters ?? []) {
    const loc = enc.location_id?.trim()
    if (!loc) continue
    for (const en of enc.enemies) {
      // 遭遇内的敌人自身地点更精确，优先；否则记遭遇地点。
      const own = en.location_id?.trim()
      const key = en.instance_id
      if (key && !out.has(key)) out.set(key, own || loc)
    }
  }
  return out
}

/**
 * 按**实例位置**聚合某地点上的单位——位置是唯一归属依据（地图与在场 §6.7）。
 * 不再有「当前场景地点 = 所有在场人物都在这里」的近似。
 *
 * 归属回落（T7 衔接点）：**实例 location_id 优先**；实例没写时用所属遭遇快照的
 * location_id 兜底。临时敌人（没有实例）同理：自身 location_id 优先，其次遭遇地点。
 * 两处都没有 = 不属于任何地点。
 */
export function unitsAtLocation(p: WorldProjection | null, locId: string): LocationUnits {
  const out: LocationUnits = { pc: [], npc: [], monster: [], loose: [] }
  if (!p || !locId) return out
  const fallback = encounterLocationIndex(p)
  for (const c of Object.values(p.characters)) {
    if (!c) continue
    const loc = c.location_id?.trim() || fallback.get(c.instance_id) || ''
    if (loc !== locId) continue
    if (c.kind === 'monster') out.monster.push(c)
    else if (c.kind === 'npc') out.npc.push(c)
    else out.pc.push(c)
  }
  for (const enc of p.encounters ?? []) {
    const encLoc = enc.location_id?.trim() ?? ''
    for (const en of enc.enemies) {
      // 有 instance_id 的敌人已在 characters 里按位置归位（含遭遇兜底），这里再计一次会重复。
      if (en.instance_id) continue
      const loc = en.location_id?.trim() || encLoc
      if (loc !== locId) continue
      out.loose.push({ id: en.id, name: en.name, encounter: enc.name })
    }
  }
  return out
}

export interface UnitTally { people: number; monsters: number; total: number }

/** 地点上的人数（PC + NPC）与怪物数（怪物实例 + 临时敌人）。 */
export function unitTally(u: LocationUnits): UnitTally {
  const people = u.pc.length + u.npc.length
  const monsters = u.monster.length + u.loose.length
  return { people, monsters, total: people + monsters }
}

/** 地点上的单位总数。 */
export function unitCount(u: LocationUnits): number { return unitTally(u).total }

// ---------- 地点上的剧情标记（地图 P6 §6.7：地图即剧情视图） ----------

/** 该地点的任务：QuestView 的 location_id 由引擎按所属场景推导（地图 P5 §6.2）。 */
export function questsAtLocation(p: WorldProjection | null, locId: string): QuestView[] {
  if (!p || !locId) return []
  return (p.quests ?? []).filter(q => (q.location_id ?? '') === locId)
}

/** 该地点的遭遇：创建时快照的 location_id（缺省继承场景，地图 P5 §6.4）。 */
export function encountersAtLocation(p: WorldProjection | null, locId: string): EncounterView[] {
  if (!p || !locId) return []
  return (p.encounters ?? []).filter(e => (e.location_id ?? '') === locId)
}

/** 骨架里 scene 所属的章节（无匹配返回 null）。 */
export function chapterOfScene(skeleton: ChapterDef[] | null | undefined, sceneId: string): ChapterDef | null {
  if (!sceneId) return null
  for (const ch of skeleton ?? []) {
    if (ch.scenes.some(sc => sc.id === sceneId)) return ch
  }
  return null
}

/**
 * 当前章节范围：该章全部场景的 location_id **并集**（去重，保持骨架顺序）。
 * 引擎不额外投影章节——`storybook.skeleton` 的 chapters 到 scenes 到 location_id 就是推导入口。
 */
export function chapterLocationIds(chapter: ChapterDef | null): string[] {
  const out: string[] = []
  for (const sc of chapter?.scenes ?? []) {
    const id = sc.location_id?.trim()
    if (id && !out.includes(id)) out.push(id)
  }
  return out
}

// ---------- 生命资源（与引擎 vital_resource_key 同口径） ----------

/**
 * 实例的生命资源键：`hp` 优先；否则取 id 以 `-hp` / `_hp` 结尾的那一个
 * （LMoP 导入的图鉴用 `res-hp`）。与引擎 `vital_resource_key` 逐条对齐——
 * 引擎按排序后的键取第一个命中，这里同样排序，避免键序不同导致两边指向不同的资源。
 * 返回 null = 这个实例没有生命资源。
 */
export function vitalResourceKey(resources: Record<string, number> | null | undefined): string | null {
  if (!resources) return null
  const keys = Object.keys(resources)
  if (!keys.length) return null
  if (keys.includes('hp')) return 'hp'
  return [...keys].sort().find(k => k.endsWith('-hp') || k.endsWith('_hp')) ?? null
}

export const LEVEL_META: Record<SuccessLevel, { label: string; hint: string }> = {
  great: { label: '大成功', hint: '差值 ≥ 10' },
  success: { label: '成功', hint: '达成目标' },
  barely: { label: '勉强', hint: '差一点达成' },
  fail: { label: '失败', hint: '未达成目标' }
}

export function fmtTime(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  const p = (n: number) => String(n).padStart(2, '0')
  return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) + ' ' + p(d.getHours()) + ':' + p(d.getMinutes())
}

/** 仅时钟：HH:mm（聊天条目时间戳） */
export function fmtClock(iso?: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return p(d.getHours()) + ':' + p(d.getMinutes())
}

export function relTime(iso: string): string {
  const d = new Date(iso).getTime()
  if (Number.isNaN(d)) return iso
  const s = Math.max(1, Math.floor((Date.now() - d) / 1000))
  if (s < 60) return s + ' 秒前'
  const m = Math.floor(s / 60)
  if (m < 60) return m + ' 分钟前'
  const h = Math.floor(m / 60)
  if (h < 24) return h + ' 小时前'
  const dd = Math.floor(h / 24)
  return dd + ' 天前'
}