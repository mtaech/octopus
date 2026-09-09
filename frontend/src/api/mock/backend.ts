// ============================================================
// 内存 Mock 后端 —— 前端原型的"假服务器"
// 实现蓝图后端契约（#23 故事书单资源两态 / #24 存档与回合 /
// #17 演出流事件）的可注入替身；日后换真 axum 后端时替换 api 层即可。
// ============================================================
import type {
  Storybook, SaveListItem, SaveDetail, StorybookListItem, StorybookDocument,
  ValidateResult, ValidationIssue, CharacterInstance, WorldProjection, PlayEvent,
  UpgradeReport, Disposition, PairSuggestion, PhaseStage, StateDelta,
  CondExpr, HistoryPage, SaveSettings, MaintenanceRow, AppConfig, ProviderTestResult, ProviderConfig
} from '@/types'
import { uid } from '@/types'
import { storybookFallingStar, storybookMist, storybookAsh, storybookDraft } from '../seed'

// ---------- 小工具 ----------

export function delay(ms: number): Promise<void> { return new Promise(r => setTimeout(r, ms)) }

function hashStr(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619) }
  return h >>> 0
}
/** 确定性伪随机（0..1）：同一输入同一结果，方便演示可复现 */
export function seededRand(seed: string | number): number {
  const n = typeof seed === 'number' ? seed : hashStr(seed)
  const x = Math.sin(n + 1) * 10000
  return x - Math.floor(x)
}

// ---------- 校验器（#23 ② 编辑期：schema + 引用完整性，纯函数形） ----------

function validateStorybook(sb: Storybook): ValidationIssue[] {
  const issues: ValidationIssue[] = []
  if (!sb.meta?.title?.trim()) issues.push({ severity: 'error', code: 'title_empty', target: 'meta.title', message: '故事书需要一个标题' })
  const dimKeys = new Set(sb.attribute_dimensions.map(d => d.key))
  const charIds = new Set(sb.characters.map(c => c.id))
  const skillIds = new Set(sb.skills.map(s => s.id))
  const itemIds = new Set(sb.items.map(i => i.id))
  const locIds = new Set(sb.world.locations.map(l => l.id))
  const facIds = new Set(sb.factions.map(f => f.id))
  const allIds = new Set([...charIds, ...skillIds, ...itemIds, ...locIds, ...facIds])
  // id 唯一性
  const seen = new Map<string, string>()
  const checkId = (kind: string, id: string) => {
    if (!id) return
    if (seen.has(id)) issues.push({ severity: 'error', code: 'dup_id', target: `${kind}:${id}`, message: `id "${id}" 重复（${seen.get(id)} 与 ${kind}）` })
    seen.set(id, kind)
  }
  sb.characters.forEach(c => checkId('character', c.id))
  sb.skills.forEach(s => checkId('skill', s.id))
  sb.items.forEach(i => checkId('item', i.id))
  sb.factions.forEach(f => checkId('faction', f.id))
  sb.world.locations.forEach(l => checkId('location', l.id))
  // 人物属性引用全局维度
  sb.characters.forEach(c => {
    for (const k of Object.keys(c.attributes)) {
      if (!dimKeys.has(k)) issues.push({ severity: 'error', code: 'unknown_dim', target: `character:${c.id}`, message: `人物「${c.name}」引用了未定义的属性维度 "${k}"` })
    }
  })
  // 引用完整性：技能/物品/人物/地点/势力
  sb.items.forEach(it => (it.skills ?? []).forEach(sid => { if (!skillIds.has(sid)) issues.push({ severity: 'error', code: 'dangling_ref', target: `item:${it.id}`, message: `物品「${it.name}」引用了不存在的技能 "${sid}"`, related_refs: [] }) }))
  sb.relationships.forEach(rel => {
    const fromOk = rel.from_kind === 'faction' ? facIds.has(rel.from_id) : charIds.has(rel.from_id)
    const toOk = rel.to_kind === 'faction' ? facIds.has(rel.to_id) : charIds.has(rel.to_id)
    if (!fromOk) issues.push({ severity: 'error', code: 'dangling_ref', target: `relationship:${rel.id}`, message: `关系 ${rel.id} 的起点实体不存在` })
    if (!toOk) issues.push({ severity: 'error', code: 'dangling_ref', target: `relationship:${rel.id}`, message: `关系 ${rel.id} 的终点实体不存在` })
  })
  // 骨架
  sb.skeleton.forEach(ch => {
    ch.scenes.forEach(sc => {
      if (sc.location_id && !locIds.has(sc.location_id)) issues.push({ severity: 'warning', code: 'dangling_ref', target: `scene:${sc.id}`, message: `场景「${sc.title}」指向不存在的地点` })
      ;(sc.present_char_ids ?? []).forEach(cid => { if (!charIds.has(cid)) issues.push({ severity: 'warning', code: 'dangling_ref', target: `scene:${sc.id}`, message: `场景「${sc.title}」的在场人物 ${cid} 不存在` }) })
      sc.goals.forEach(g => { if (!g.text?.trim()) issues.push({ severity: 'warning', code: 'empty_goal', target: `goal:${g.id}`, message: '存在没有文本的目标' }) })
    })
  })
  // 人物/技能/物品空名
  ;(['characters', 'skills', 'items', 'factions'] as const).forEach(kind => {
    sb[kind].forEach((e: { id: string; name: string }) => { if (!e.name?.trim()) issues.push({ severity: 'warning', code: 'empty_name', target: `${kind.slice(0, -1)}:${e.id}`, message: '有实体缺少名称' }) })
  })
  // 声明区与物件引用（#01 修订）
  const flagKeys = new Set((sb.flags ?? []).map(d => d.key))
  const eventKeys = new Set((sb.events ?? []).map(d => d.key))
  const relTypeKeys = new Set((sb.relationship_types ?? []).map(d => d.key))
  const targetKeys = new Set((sb.target_types ?? []).map(d => d.key))
  ;(sb.objects ?? []).forEach(o => {
    if (o.location_id && !locIds.has(o.location_id)) issues.push({ severity: 'warning', code: 'dangling_ref', target: `object:${o.id}`, message: `物件「${o.name}」指向不存在的地点` })
    ;(o.skills ?? []).forEach(sid => { if (!skillIds.has(sid)) issues.push({ severity: 'error', code: 'dangling_ref', target: `object:${o.id}`, message: `物件「${o.name}」引用了不存在的技能 "${sid}"` }) })
  })
  sb.relationships.forEach(rel => { if (relTypeKeys.size && !relTypeKeys.has(rel.type)) issues.push({ severity: 'warning', code: 'undeclared_type', target: `relationship:${rel.id}`, message: `关系类型 "${rel.type}" 未在声明区定义` }) })
  sb.skills.forEach(sk => { if (sk.target && targetKeys.size && !targetKeys.has(sk.target)) issues.push({ severity: 'warning', code: 'undeclared_target', target: `skill:${sk.id}`, message: `技能「${sk.name}」的目标类型 "${sk.target}" 未在声明区定义` }) })
  const walkCond = (c: CondExpr | null | undefined, target: string): void => {
    if (!c) return
    if (c.op === 'flag_set') { if (flagKeys.size && !flagKeys.has(c.flag)) issues.push({ severity: 'warning', code: 'undeclared_flag', target, message: `条件引用了未声明的 flag "${c.flag}"` }) }
    else if (c.op === 'all_of' || c.op === 'any_of') c.children.forEach(ch => walkCond(ch, target))
    else if (c.op === 'not') walkCond(c.child, target)
  }
  sb.skeleton.forEach(ch => ch.scenes.forEach(sc => {
    sc.goals.forEach(g => walkCond(g.condition, `goal:${g.id}`))
    sc.beats.forEach(b => walkCond(b.condition, `beat:${b.id}`))
  }))
  sb.skills.forEach(sk => (sk.effect?.triggers ?? []).forEach(t => { if (eventKeys.size && !eventKeys.has(t.event)) issues.push({ severity: 'warning', code: 'undeclared_event', target: `skill:${sk.id}`, message: `触发事件 "${t.event}" 未在声明区定义` }) }))

  return issues
}

// ---------- 内部库 ----------

interface StorybookRow {
  doc: StorybookDocument
}
interface SaveRow {
  item: SaveListItem
  detail: SaveDetail
  // 运行时态（#06 投影源 + #17 引擎模拟）
  projection: WorldProjection
  seq: number
  round: number
  phase: PhaseStage
  maintenance: MaintenanceRow[]
  pendingAction: { action_id: string; description: string; impact?: string; resolve: (decision: 'confirm' | 'cancel') => void } | null
  // 引擎发事件的订阅
  sinks: Set<(e: PlayEvent) => void>
  /** 当前回合是否开着（确认门挂起时保持 open） */
  roundOpen: boolean
  latestText: string
  aiBusy: boolean
  autoConfirm: boolean
  /** 叙事历史（#17/#24 修订）：权威事件流，供 GET /history 分页回读 */
  eventLog: PlayEvent[]
  requestIds: Set<string>
  archivedCount: number
}

const sbStore = new Map<string, StorybookRow>()
const saveStore = new Map<string, SaveRow>()

export type EngineSink = (e: PlayEvent) => void

// ---------- 基础校验/引用索引导出 ----------

export const validate = (sb: Storybook): ValidateResult => ({ issues: validateStorybook(sb) })

// ---------- 故事书 CRUD（#23 单资源两态） ----------

let _sbSeq = 0
function initStorybook(sb: Storybook, opts: { published?: boolean } = {}): StorybookRow {
  _sbSeq++
  const now = new Date(Date.now() + _sbSeq * 60_000).toISOString()
  const base: StorybookDocument = {
    id: sb.meta.id, revision: 0, draft_version: 0, updated_at: now, released_at: null,
    published: false, draft: sb, released: null
  }
  if (opts.published) {
    base.revision = 1; base.published = true; base.released_at = now; base.released = sb
  }
  const row: StorybookRow = { doc: base }
  sbStore.set(sb.meta.id, row)
  return row
}

// 初始数据（storybookFallingStar 已发布 rev4、雾镇 rev2、灰烬 rev3、北境草稿）
function bumpPublished(row: StorybookRow, times: number) {
  const d = row.doc
  for (let i = 0; i < times; i++) { d.revision++; d.draft_version++; d.released = d.draft; d.released_at = new Date().toISOString(); d.published = true }
  d.updated_at = new Date().toISOString()
}
function initSeeds() {
  const sb1 = initStorybook(storybookFallingStar(), { published: true }); bumpPublished(sb1, 3)
  const sb2 = initStorybook(storybookMist(), { published: true }); bumpPublished(sb2, 1)
  const sb3 = initStorybook(storybookAsh(), { published: true }); bumpPublished(sb3, 2)
  const sb4 = initStorybook(storybookDraft())
  return { sb1, sb2, sb3, sb4 }
}

// 存档开档（#24 ①：POST /api/saves { storybook_id } → 内嵌冻结当前已发布版次）
let _saveSeq = 0
export function createSave(storybookId: string, title?: string, controlledCharacterId?: string): SaveRow {
  const sbRow = sbStore.get(storybookId)
  if (!sbRow) throw new Error('STORYBOOK_NOT_FOUND')
  const d = sbRow.doc
  if (!d.published || !d.released) throw new Error('STORYBOOK_UNPUBLISHED')
  _saveSeq++
  const now = new Date(Date.now() + _saveSeq * 30_000).toISOString()
  const saveId = 'sv-' + String(_saveSeq).padStart(2, '0')
  const embedded = structuredClone(d.released)
  const item: SaveListItem = {
    id: saveId, title: title || (d.draft?.meta.title ?? ''), storybook_id: d.id, storybook_title: (d.draft?.meta.title ?? ''),
    embedded_revision: d.revision, latest_revision: d.revision, needs_upgrade: false,
    created_at: now, updated_at: now, last_played_at: now
  }
  const detail: SaveDetail = { ...item, storybook: embedded }
  const row: SaveRow = { item, detail, seq: 0, round: 0, phase: 'idle', maintenance: [], pendingAction: null, sinks: new Set(), roundOpen: false, latestText: '', aiBusy: false, autoConfirm: false, eventLog: [], requestIds: new Set(), archivedCount: 0, projection: buildProjection(detail, 0, item, 0, 0) }
  if (controlledCharacterId && row.projection.characters[controlledCharacterId]) row.projection.controlled = [controlledCharacterId]
  saveStore.set(saveId, row)
  return row
}

function buildProjection(detail: SaveDetail, seq: number, meta: SaveListItem, round: number, sceneIdx: number): WorldProjection {
  const sb = detail.storybook
  const scene = sb.skeleton[0]?.scenes[sceneIdx % Math.max(1, sb.skeleton[0]?.scenes.length ?? 1)]
  const controlledTpl = sb.characters[0]
  const chars: Record<string, CharacterInstance> = {}
  sb.characters.forEach((c, i) => {
    chars[c.id] = {
      instance_id: `inst-${c.id}`, template_id: c.id, name: c.name, kind: c.kind ?? (i === 0 ? 'pc' : 'npc'),
      attributes: { ...c.attributes }, resources: { ...(c.resources ?? {}) },
      location_id: scene?.location_id, present: true, statuses: []
    }
  })
  return {
    seq, scene_id: scene?.id ?? '', scene_title: scene?.title ?? '', characters: chars,
    controlled: controlledTpl ? [controlledTpl.id] : [], flags: {}, progress: { goals: {}, beats: {}, abandoned_scenes: [] },
    locations: sb.world.locations, meta: {
      save_id: meta.id, save_title: meta.title, storybook_title: meta.storybook_title,
      revision: meta.embedded_revision, needs_upgrade: meta.needs_upgrade, auto_confirm: rowConfirmDefault
    }
  }
}

let rowConfirmDefault = false
export function setAutoConfirmDefault(v: boolean) { rowConfirmDefault = v }

export function initSeedsAll() {
  const { sb1 } = initSeeds()
  // 建一个"当前在玩"的存档：基于坠星谷 rev4 → 让 embedded=rev3 制造 needs_upgrade
  const save = createSave('sb-fallingstar', '坠星谷 · 酒馆之夜')
  save.item.embedded_revision = 3
  save.item.needs_upgrade = true
  save.detail.embedded_revision = 3
  save.detail.needs_upgrade = true
  save.item.updated_at = new Date(Date.now() - 36e5 * 2).toISOString()
  save.item.last_played_at = new Date(Date.now() - 18e5).toISOString()
  save.projection.meta.revision = 3
  save.projection.meta.needs_upgrade = true
  const sb1Doc = sbStore.get('sb-fallingstar')!.doc
  save.item.latest_revision = sb1Doc.revision
  save.detail.latest_revision = sb1Doc.revision
  // 再建两个存档：一个同书无需升级、一个雾镇
  const s2 = createSave('sb-fallingstar', '米拉 · 废矿坑')
  const s3 = createSave('sb-mist', '雾镇疑云 · 调查档案')
  s3.item.updated_at = new Date(Date.now() - 36e5 * 26).toISOString()
  s3.item.last_played_at = new Date(Date.now() - 36e5 * 26).toISOString()
  return { save, s2, s3 }
}

// ---------- 存档 API（#24） ----------

export function listStorybooks(releasedOnly: boolean): StorybookListItem[] {
  return [...sbStore.values()].map(r => {
    const d = r.doc
    return { id: d.id, title: d.draft.meta.title, revision: d.revision, draft_version: d.draft_version, updated_at: d.updated_at, released_at: d.released_at, published: d.published, description: d.released?.meta.description ?? d.draft.meta.description ?? '' }
  }).filter(s => !releasedOnly || s.published)
}

export function getStorybook(id: string): StorybookDocument | null {
  return sbStore.get(id)?.doc ?? null
}

export function createStorybookDraft(title = '未命名故事书'): StorybookDocument {
  _sbSeq++
  const id = 'sb-' + String(Date.now().toString(36))
  const sb: Storybook = { schema_version: 1, meta: { id, title }, world: { premise: '', locations: [], resources: [] }, attribute_dimensions: [], skeleton: [], characters: [], skills: [], items: [], objects: [], factions: [], relationships: [], flags: [], events: [], relationship_types: [], target_types: [] }
  return initStorybook(sb).doc
}

export function saveDraft(id: string, draft: Storybook, baseVersion: number): { doc: StorybookDocument; issues: ValidationIssue[] } {
  const row = sbStore.get(id)
  if (!row) throw new Error('STORYBOOK_NOT_FOUND')
  if (baseVersion !== row.doc.draft_version) throw new Error('CONFLICT')
  row.doc.draft = structuredClone(draft)
  // 内容级幂等：无论是否同内容都 bump（真后端做 no-op 检测，这里简化）
  row.doc.draft_version++
  row.doc.updated_at = new Date().toISOString()
  return { doc: row.doc, issues: validateStorybook(row.doc.draft) }
}

export function publishDraft(id: string, baseVersion: number): { doc: StorybookDocument; issues: ValidationIssue[] } {
  const row = sbStore.get(id)
  if (!row) throw new Error('STORYBOOK_NOT_FOUND')
  const issues = validateStorybook(row.doc.draft)
  const errs = issues.filter(i => i.severity === 'error')
  if (errs.length) throw { code: 'VALIDATION_FAILED', detail: { issues } }
  if (baseVersion !== row.doc.draft_version) throw new Error('CONFLICT')
  row.doc.revision++; row.doc.released = structuredClone(row.doc.draft); row.doc.released_at = new Date().toISOString()
  row.doc.published = true; row.doc.draft_version++; row.doc.updated_at = new Date().toISOString()
  return { doc: row.doc, issues: [] }
}

export function listSaves(): SaveListItem[] {
  return [...saveStore.values()].map(r => ({ ...r.item })).sort((a, b) => (a.last_played_at < b.last_played_at ? 1 : -1))
}

export function getSave(id: string): SaveDetail | null {
  const r = saveStore.get(id); return r ? { ...r.detail } : null
}

// 版本升级（#14 两段式）
export function upgradeDryRun(saveId: string): UpgradeReport | null {
  const r = saveStore.get(saveId); if (!r) return null
  const sbCurrent = sbStore.get(r.item.storybook_id)?.doc
  const curRev = sbCurrent?.revision ?? r.item.embedded_revision
  return {
    from_revision: r.item.embedded_revision, to_revision: curRev,
    groups: { changes: [{ kind: 'skill', id: 'sk-quaff', label: '痛饮', action: '调整了消耗描述（2 金币 → 1 金币）' }], gone_characters: [{ character_id: 'char-kael', name: '凯尔', reason: '新版故事书中凯尔已离开坠星谷' }] }
  }
}

export function upgradeExecute(saveId: string, dispositions: { character_id: string; disposition: Disposition }[]): { detail: SaveDetail; backupName: string } {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  const gone = upgradeDryRun(saveId)!.groups.gone_characters
  if (!gone.every(g => dispositions.some(d => d.character_id === g.character_id && d.disposition))) throw new Error('MISSING_DISPOSITIONS')
  const from = r.item.embedded_revision, to = upgradeDryRun(saveId)!.to_revision
  // 内嵌模板升到新版
  const current = sbStore.get(r.item.storybook_id)!.doc.released!
  r.detail.storybook = structuredClone(current)
  r.item.embedded_revision = to; r.detail.embedded_revision = to; r.item.needs_upgrade = false; r.detail.needs_upgrade = false
  r.projection.meta.revision = to; r.projection.meta.needs_upgrade = false
  // 逐项裁决：departure 的角色标记离场
  dispositions.forEach(d => {
    if (d.disposition === 'departure') {
      const inst = r.projection.characters[d.character_id]
      if (inst) { inst.present = false; inst.statuses.push({ id: 'departed', name: '已离场', scenes_left: undefined }) }
    }
  })
  const backupName = `${saveId}-backup-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.sqlite`
  const now = new Date().toISOString()
  r.item.updated_at = now
  r.maintenance.unshift({ at: now, op: `升级 rev${from} → rev${to}`, summary: `备份 ${backupName}；${dispositions.filter(d => d.disposition === 'departure').length} 名人物离场，${dispositions.filter(d => d.disposition === 'freeze').length} 名遗留冻结` })
  return { detail: { ...r.detail }, backupName }
}

export function manualSave(saveId: string): SaveListItem {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  const now = new Date().toISOString()
  r.item.updated_at = now; r.item.last_played_at = now
  r.maintenance.unshift({ at: now, op: '手动存档', summary: '快照已写入' })
  return { ...r.item }
}


export function importSaveFile(fileName: string): SaveListItem {
  // mock：能打开即通过（#21 ④）。文件名 → 新建存档，标记 imported
  const sbPool = ['sb-fallingstar', 'sb-mist', 'sb-ash']
  const useSb = sbPool[Math.floor(seededRand(fileName) * sbPool.length)]
  const baseTitle = fileName.replace(/\.sqlite$/i, '').replace(/[-_]/g, ' ')
  const row = createSave(useSb, '导入 · ' + baseTitle)
  row.item.imported = true
  row.detail.imported = true
  const now = new Date().toISOString()
  row.item.created_at = now; row.item.updated_at = now; row.item.last_played_at = now
  row.maintenance.unshift({ at: now, op: '导入存档', summary: '来自单文件 ' + fileName })
  return { ...row.item }
}

// ---------- 叙事历史 / 存档维护（设计复审 2026-09-09 修订） ----------

export function getHistory(saveId: string, beforeSeq?: number, limit = 50): HistoryPage {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  const all = r.eventLog.filter(e => beforeSeq === undefined || e.seq < beforeSeq)
  const slice = all.slice(Math.max(0, all.length - limit))
  return { events: slice.map(e => ({ ...e })), hasMore: all.length > slice.length }
}

export function renameSave(saveId: string, title: string): SaveListItem {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  const t = title.trim(); if (!t) throw new Error('EMPTY_TITLE')
  r.item.title = t; r.detail.title = t
  r.item.updated_at = new Date().toISOString()
  r.projection.meta.save_title = t
  return { ...r.item }
}

export function deleteSave(saveId: string): void {
  if (!saveStore.delete(saveId)) throw new Error('SAVE_NOT_FOUND')
}

export function getMaintenance(saveId: string): MaintenanceRow[] {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  return r.maintenance.map(m => ({ ...m }))
}

export function getSaveSettings(saveId: string): SaveSettings {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  return { auto_confirm: r.autoConfirm }
}

export function setSaveSettings(saveId: string, settings: SaveSettings): SaveSettings {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  r.autoConfirm = !!settings.auto_confirm
  r.projection.meta.auto_confirm = r.autoConfirm
  emit(r, mkEvent(r, 'system', { level: 'info', code: 'confirm_toggle', text: `免确认模式已${r.autoConfirm ? '开启' : '关闭'}` }))
  return { auto_confirm: r.autoConfirm }
}

/** 新原点（#24 修订：玩家侧 v1；旧日志归档只读） */
export function newOrigin(saveId: string): { detail: SaveDetail; archivedCount: number } {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  const count = r.eventLog.length
  r.archivedCount += count
  r.eventLog = []
  const now = new Date().toISOString()
  r.maintenance.unshift({ at: now, op: '压缩为新原点', summary: `以 seq ${r.seq} 为新起点；归档 ${count} 条历史（只读）` })
  emit(r, mkEvent(r, 'system', { level: 'info', code: 'new_origin', text: `已压缩为新原点，归档 ${count} 条历史。` }))
  return { detail: { ...r.detail }, archivedCount: count }
}

/** 切换受控角色（#24 修订：状态类元指令直调） */
export function switchCharacter(saveId: string, characterId: string): void {
  const r = saveStore.get(saveId); if (!r) throw new Error('SAVE_NOT_FOUND')
  const inst = r.projection.characters[characterId]
  if (!inst) throw new Error('CHARACTER_NOT_FOUND')
  if (inst.kind !== 'pc') throw new Error('NOT_CONTROLLABLE')
  r.projection.controlled = [characterId]
  emit(r, mkEvent(r, 'system', { level: 'info', code: 'switch_character', text: `已切换受控角色：${inst.name}` }))
}

// ---------- 结对建议生成（#23 ④ mock：确定性模板建议） ----------

export function genSuggestions(question: string): PairSuggestion[] {
  const list: PairSuggestion[] = []
  if (/酒馆|tavern|夜晚/i.test(question)) {
    list.push({ id: uid('sug'), action: 'create', target: { kind: 'location' }, patch: { id: 'loc-backroom', name: '酒馆后间', description: '堆着旧木箱与空酒桶，伊莎不让客人进。' }, label: '新增地点 · 酒馆后间', summary: '给酒馆加一个藏秘密的后间' })
  }
  if (/人物|角色|npc|镇民/i.test(question) || list.length === 0) {
    list.push({ id: uid('sug'), action: 'create', target: { kind: 'character' }, patch: { id: 'char-hugo', name: '雨果', kind: 'npc', background: '镇上守夜人，流星夜后开始说梦话。', personality: '疲惫、警觉', attributes: { str: 55, agi: 60, wit: 50, cha: 45, trait: '沉稳' } }, label: '新增人物 · 守夜人雨果', summary: '一个见证了流星夜真相的边缘人' })
    list.push({ id: uid('sug'), action: 'update', target: { kind: 'character', id: 'char-isa' }, patch: { background: '碎星酒馆老板娘，十年前从王都搬来；传闻她曾是天象局学徒。', personality: '热情圆滑，话里有话，护短；对流星的话题异常回避。' }, label: '丰富人物 · 伊莎', summary: '给老板娘加一层与教会相关的旧身份' })
  }
  return list
}

export function pairStreamChat(messages: { role: 'user' | 'assistant'; content: string }[]): { deltas: string[]; suggestions: PairSuggestion[]; text: string } {
  const last = [...messages].reverse().find(m => m.role === 'user')?.content ?? ''
  const suggestions = genSuggestions(last)
  const head = suggestions.length ? '我按这个方向拟了几条建议，可以先看看合不合口味。' : '这个方向我还没太想好，换个说法再聊聊？'
  const text = head
  return { deltas: [text], suggestions, text }
}

// ---------- 引擎回合模拟（#17 事件 + 确认门） ----------

function emit(row: SaveRow, e: PlayEvent) { row.eventLog.push(e); row.sinks.forEach(s => { try { s(e) } catch { /* noop */ } }) }

export function subscribe(saveId: string, sink: EngineSink): () => void {
  const r = saveStore.get(saveId)
  if (!r) throw new Error('SAVE_NOT_FOUND')
  r.sinks.add(sink)
  return () => { r.sinks.delete(sink) }
}

function nextSeq(row: SaveRow): number { return ++row.seq }

function mkEvent(row: SaveRow, type: PlayEvent['type'], payload: Record<string, unknown>, extra: Partial<PlayEvent> = {}): PlayEvent {
  const base = { id: uid('evt'), seq: nextSeq(row), round: row.round, ts: new Date().toISOString(), actor: extra.actor ?? null, intent_id: extra.intent_id ?? null } as Record<string, unknown>
  return { ...base, type, payload, ...extra } as unknown as PlayEvent
}

const sleep = (ms: number) => new Promise<void>(res => setTimeout(res, ms))

/** 把玩家文本映射为若干叙事事件（demo 脚本，关键字驱动） */
function narrativeScript(text: string, row: SaveRow): { narrate: string[]; dialogue: { actor: string; content: string }[]; emote?: string; scene?: { scene_id: string; title: string; description: string; present: string[] }; goal?: string; beats?: string[]; flag?: string } {
  const sb = row.detail.storybook
  const char = (id: string) => sb.characters.find(c => c.id === id)
  const mira = char('char-mira')!
  const isa = char('char-isa')!
  const oden = char('char-oden')!
  const kael = char('char-kael')!
  const taverngo = /酒馆|伊莎|老板娘|麦酒|住宿/.test(text)
  const askDream = /梦|流星|传闻|怪/.test(text)
  const probe = /观察|调查|看看|查看/.test(text)
  const mine = /矿坑|废矿|封条/.test(text)
  const leave = /离开|出门|去|广场|教堂|睡觉/.test(text)
  const greet = /你好|嗨|打招呼|请问/.test(text)

  const narrate: string[] = []
  const dialogue: { actor: string; content: string }[] = []
  const out: NonNullable<ReturnType<typeof narrativeScript>> = { narrate, dialogue }

  if (!row.latestText && /观察|查看/.test(text) === false) {
    // 开场：第一次输入时先垫一段场景描写（若还没有 flag met_isa）
  }
  if (greet) {
    dialogue.push({ actor: 'char-isa', content: '稀客呀。坠星谷这半个月没什么生面孔——坐下来喝一杯，第一杯算我的。' })
    narrate.push('伊莎擦着杯子打量你，语气熟稔得像招呼老友。')
    out.flag = 'met_isa'
  } else if (askDream) {
    dialogue.push({ actor: 'char-isa', content: '你也听说了？实话讲，这三日镇上没几个睡踏实的。' })
    dialogue.push({ actor: 'char-isa', content: '都梦见同一件事：站在废矿坑边沿，底下有东西在发光，像是……在叫我们下去。' })
    narrate.push('伊莎压低声音，油灯的火苗晃了一下。')
    out.flag = 'heard_dreams'
    out.goal = 'g1'
    out.beats = ['b1', 'b2']
    out.emote = '伊莎说着，下意识摸了摸手腕上一道旧疤。'
  } else if (mine) {
    dialogue.push({ actor: 'char-kael', content: '封条是镇公所下的，可谁都知道那儿不对劲。我铁匠铺的伙计说，夜里能听见矿坑底下有——像心跳一样的声音。' })
    narrate.push('凯尔攥紧了拳头，指节发白。')
    out.beats = ['b3']
    out.emote = '凯尔说到一半停住，像是怕被谁听见。'
  } else if (taverngo || leave) {
    dialogue.push({ actor: 'char-isa', content: '楼上空房多的是，随便挑一间。夜里要是听见什么动静——别开窗。' })
    narrate.push('伊莎把一把黄铜钥匙推到你面前，笑意没到眼底。')
    out.flag = 'met_isa'
  } else {
    // 泛化回应：换个人说话
    const who = [isa, kael, oden][Math.floor(seededRand(row.round * 7 + text.length) * 3)]
    dialogue.push({ actor: who.id, content: `「${text}」这事啊……${who.name}想了想，&quot;说来话长，不过你算是问对人了。&quot;` })
  }
  out.narrate = narrate
  out.dialogue = dialogue
  return out
}

// 判定脚本：检查总要先走 pending（确认门）再 check_result + resolution
function rollFor(text: string, charId: string): { total: number; target: number; margin: number; mod: number; rolls: number[]; level: 'great' | 'success' | 'barely' | 'fail' } {
  const roll = Math.floor(seededRand(text + charId) * 20) + 1
  const mod = Math.floor((55 - 50) / 5)
  const target = 12
  const total = roll + mod
  const margin = total - target
  const level = margin >= 10 ? 'great' : margin >= 0 ? 'success' : margin >= -10 ? 'barely' : 'fail'
  return { total, target, margin, mod, rolls: [roll], level }
}

/** 提交玩家回合（#24 ① POST /rounds）→ 模拟引擎管线并流式发事件 */
export async function submitRound(saveId: string, channel: 'character' | 'meta', text: string, requestId?: string, onProgress?: (p: { stage: PhaseStage; detail?: string }) => void) {
  const row = saveStore.get(saveId)
  if (!row) throw new Error('SAVE_NOT_FOUND')
  if (row.aiBusy) throw new Error('ROUND_IN_PROGRESS')
  if (!text.trim()) throw new Error('EMPTY_INPUT')
  if (requestId && row.requestIds.has(requestId)) return
  if (requestId) row.requestIds.add(requestId)
  row.aiBusy = true
  row.round++
  row.latestText = text
  row.roundOpen = true
  const playerCharId = row.projection.controlled[0]

  const prog = (stage: PhaseStage, detail?: string) => { row.phase = stage; if (onProgress) onProgress({ stage, detail }); emit(row, mkEvent(row, 'phase', { stage, detail })) }

  try {
    emit(row, mkEvent(row, 'round_start', { input: { channel, text } }, { actor: { id: playerCharId, name: row.projection.characters[playerCharId]?.name ?? '玩家' } }))
    prog('story_thinking')
    await sleep(350 + seededRand(text) * 350)

    // 元指令走独立语义
    if (channel === 'meta') {
      if (/存档|保存/.test(text)) {
        prog('resolving')
        await sleep(150)
        const item = manualSave(saveId)
        emit(row, mkEvent(row, 'system', { level: 'info', code: 'saved', text: `已手动存档：${item.title}` }))
        emit(row, mkEvent(row, 'resolution', { intent_id: uid('int'), status: 'ok', narrative: '存档完成。', state_changes: [] }))
        emit(row, mkEvent(row, 'round_end', { round: row.round }))
        return
      }
      if (/免确认|自动确认/.test(text)) {
        row.autoConfirm = !row.autoConfirm
        row.projection.meta.auto_confirm = row.autoConfirm
        emit(row, mkEvent(row, 'system', { level: 'info', code: 'confirm_toggle', text: `免确认模式已${row.autoConfirm ? '开启' : '关闭'}` }))
        emit(row, mkEvent(row, 'round_end', { round: row.round }))
        return
      }
      if (/帮助|help|怎么玩/.test(text)) {
        emit(row, mkEvent(row, 'system', { level: 'info', text: '输入你想做的事即可。试试「观察一下酒馆」「打听怪梦的事」「去废矿坑看看」。元指令：/存档 /免确认 /切换角色 /帮' }))
        emit(row, mkEvent(row, 'round_end', { round: row.round }))
        return
      }
      emit(row, mkEvent(row, 'system', { level: 'warn', text: `未知元指令：${text}` }))
      emit(row, mkEvent(row, 'round_end', { round: row.round }))
      return
    }

    // ===== 角色输入 → 判定类动作（观察/调查/说服）=====
    const isCheck = /观察|调查|查看|说服|搜|侦察|聆听|嗅/.test(text)
    if (isCheck) {
      prog('resolving')
      const actionId = uid('act')
      const attr = /说服|搭话/.test(text) ? 'cha' : 'wit'
      const attrName = attr === 'cha' ? '魅力' : '机敏'
      const description = /说服|搭话/.test(text) ? '尝试说服对方' : `用${attrName}观察当前环境`
      const roll = rollFor(text + '/chk', playerCharId)
      // pending 确认门（免确认关闭时）
      if (!row.autoConfirm) {
        prog('waiting_confirm')
        const decision = await new Promise<'confirm' | 'cancel' | 'timeout'>(resolve => {
          // 先挂起再发 pending 事件，保证事件订阅者（页面）能立即 confirmAction
          row.pendingAction = { action_id: actionId, description, resolve: d => resolve(d) }
          emit(row, mkEvent(row, 'pending', { action_id: actionId, actor: { id: playerCharId, name: row.projection.characters[playerCharId].name }, description, impact: '会消耗一次行动机会', timeout_ms: 20000 }, { actor: { id: playerCharId, name: row.projection.characters[playerCharId].name } }))
          // 超时默认取消
          setTimeout(() => { if (row.pendingAction?.action_id === actionId) { row.pendingAction = null; resolve('timeout') } }, 20000)
        })
        if (decision === 'cancel' || decision === 'timeout') {
          row.pendingAction = null
          if (decision === 'timeout') emit(row, mkEvent(row, 'system', { level: 'warn', code: 'confirm_timeout', text: '确认超时，已默认取消。' }))
          emit(row, mkEvent(row, 'resolution', { intent_id: uid('int'), status: 'rejected', rejection_code: 'cancelled', narrative: '你收回了动作。', state_changes: [] }))
          emit(row, mkEvent(row, 'round_end', { round: row.round }))
          return
        }
        row.pendingAction = null
      }
      prog('resolving')
      await sleep(220)
      emit(row, mkEvent(row, 'check_result', { actor: { id: playerCharId, name: row.projection.characters[playerCharId].name }, attribute: attrName, expr: '1d20', rolls: roll.rolls, mod: roll.mod, total: roll.total, target: roll.target, margin: roll.margin, result: roll.total >= roll.target, level: roll.level }, { actor: { id: playerCharId, name: row.projection.characters[playerCharId].name } }))
      await sleep(180)
      const changes: StateDelta[] = []
      const charInst = row.projection.characters[playerCharId]
      const ok = roll.total >= roll.target
      let narrativeText: string
      if (/观察|查看/.test(text)) {
        if (ok) {
          narrativeText = '你环顾酒馆：吧台后方的帘子微微晃动，像有人刚躲进去。墙上的告示牌钉着一张盖了镇公所章的纸——废矿坑，禁止入内。'
          row.projection.flags.mine_foreshadow = true
          changes.push({ domain: 'flag', entity_id: 'mine_foreshadow', field: 'flag', op: 'set', value: true })
        } else { narrativeText = '油灯的光影晃得厉害，你什么也没看清。' }
      } else if (/调查/.test(text)) {
        if (ok) { narrativeText = '你注意到地板上有细碎的银白色粉末，从吧台一直延伸到后门。'; changes.push({ domain: 'flag', entity_id: 'silver_powder', field: 'flag', op: 'set', value: true }) }
        else { narrativeText = '酒气太浓，你打了个喷嚏，什么线索也没找到。' }
      } else { narrativeText = ok ? '对方被你说动了，态度明显软化。' : '对方不为所动，只是皱眉看着你。' }
      emit(row, mkEvent(row, 'narrate', { content: narrativeText }, { actor: { id: 'char-isa', name: '伊莎' } }))
      emit(row, mkEvent(row, 'resolution', { status: 'ok', narrative: ok ? '动作成功。' : '动作未达成目标。', state_changes: changes }, { actor: { id: playerCharId, name: row.projection.characters[playerCharId].name } }))
      emit(row, mkEvent(row, 'round_end', { round: row.round }))
      return
    }

    // ===== 角色输入 → 一般叙事 =====
    prog('character_thinking')
    await sleep(400 + seededRand(text + 'c') * 500)
    const script = narrativeScript(text, row)
    if (script.emote) { emit(row, mkEvent(row, 'emote', { content: script.emote, emotion: 'concern', gesture: 'gesture' }, { actor: { id: 'char-isa', name: '伊莎' } })) }
    if (script.narrate.length) script.narrate.forEach(n => emit(row, mkEvent(row, 'narrate', { content: n }, { actor: { id: 'char-isa', name: '伊莎' } })))
    script.dialogue.forEach(d => {
      const who = row.projection.characters[d.actor]
      emit(row, mkEvent(row, 'dialogue', { content: d.content }, { actor: { id: d.actor, name: who?.name ?? d.actor } }))
    })
    if (script.narrate.length === 0 && script.dialogue.length === 0) {
      emit(row, mkEvent(row, 'narrate', { content: '酒馆里的喧闹照旧，你的话像投进湖面的石子，短暂地起了涟漪。' }, { actor: { id: 'char-isa', name: '伊莎' } }))
    }
    // 状态更新：goal / beat / flag
    const changes: StateDelta[] = []
    if (script.flag) {
      row.projection.flags[script.flag] = true
      changes.push({ domain: 'flag', entity_id: script.flag, field: 'flag', op: 'set', value: true })
    }
    if (script.goal) { row.projection.progress.goals[script.goal] = true; changes.push({ domain: 'goal', entity_id: script.goal, field: 'achieved', op: 'set', value: true }) }
    ;(script.beats ?? []).forEach(b => { row.projection.progress.beats[b] = true; changes.push({ domain: 'beat', entity_id: b, field: 'fired', op: 'set', value: true }) })
    if (changes.length) { row.projection.seq = row.seq; emit(row, mkEvent(row, 'state_update', { changes })) }
    await sleep(200)
    emit(row, mkEvent(row, 'round_end', { round: row.round }))
  } finally {
    row.aiBusy = false
    row.roundOpen = false
    row.phase = 'idle'
  }
}

/** 确认门（#17/#24） */
export async function confirmAction(saveId: string, roundId: number, actionId: string, decision: 'confirm' | 'cancel'): Promise<void> {
  const row = saveStore.get(saveId)
  if (!row) throw new Error('SAVE_NOT_FOUND')
  if (!row.pendingAction || row.pendingAction.action_id !== actionId) throw new Error('NOT_PENDING')
  row.pendingAction.resolve(decision)
}

export function currentProjection(saveId: string): WorldProjection | null {
  const r = saveStore.get(saveId)
  if (!r) return null
  r.projection.seq = r.seq
  return r.projection
}

// ---------- 应用配置（#26 AI Provider 配置与凭据管理） ----------

const DEFAULT_CONFIG: AppConfig = {
  providers: [
    { id: 'deepseek', label: 'DeepSeek', kind: 'deepseek', base_url: 'https://api.deepseek.com', api_key: '', models: ['deepseek-chat', 'deepseek-reasoner'] },
    { id: 'openai', label: 'OpenAI', kind: 'openai', base_url: 'https://api.openai.com/v1', api_key: '', models: ['gpt-4o', 'gpt-4o-mini'] },
    { id: 'ollama', label: '本地 Ollama', kind: 'ollama', base_url: 'http://127.0.0.1:11434', api_key: '', models: ['qwen2.5:7b', 'llama3.1:8b'] },
    { id: 'fastembed', label: '本地 Embedding', kind: 'openai-compatible', api_key: '', models: ['bge-small-zh-v1.5', 'bge-m3'] }
  ],
  roles: {
    story: { provider_id: 'deepseek', model: 'deepseek-chat', temperature: 0.8, max_tokens: 4096 },
    character: { provider_id: 'deepseek', model: 'deepseek-chat', temperature: 0.7, max_tokens: 2048 },
    embedding: { provider_id: 'fastembed', model: 'bge-small-zh-v1.5' }
  },
  turn_token_budget: 0
}
let appConfig: AppConfig = structuredClone(DEFAULT_CONFIG)

export function getAppConfig(): AppConfig { return structuredClone(appConfig) }
export function saveAppConfig(cfg: AppConfig): AppConfig { appConfig = JSON.parse(JSON.stringify(cfg)) as AppConfig; return structuredClone(appConfig) }
export function testProvider(provider: ProviderConfig): ProviderTestResult {
  const p = provider
  if (!p) return { ok: false, message: '未找到该 Provider' }
  const needKey = p.kind === 'openai' || p.kind === 'anthropic' || p.kind === 'deepseek'
  if (needKey && !p.api_key) return { ok: false, message: '未填写 API Key' }
  return { ok: true, message: '连通正常', latency_ms: Math.round(80 + seededRand(p.id) * 400) }
}

// ---------- 初始化 ----------

let inited = false
export function ensureSeeded() {
  if (inited) return
  inited = true
  initSeedsAll()
}
