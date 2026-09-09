// ============================================================
// editor store —— 草稿唯一主人（#22 ③ / #23）
// 整本故事书草稿 + dirty 追踪 + debounced 自动保存 + 发布门 + 校验刷新
// A / C / B 三范式共享同一草稿；范式偏好 localStorage 持久化（#22 ①）。
// ============================================================
import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { createStorybookDraft, getStorybook, publishDraft, saveDraft, toast, validateStorybook } from '@/api'
import type { PairSuggestion, Storybook, StorybookDocument, ValidationIssue } from '@/types'
import { uid } from '@/types'

// ---------- 范式（视图态，#22 ①） ----------
export type Paradigm = 'A' | 'B' | 'C'
const PARADIGM_LS_KEY = 'octopus.editor.paradigm'

export interface ParadigmMeta {
  key: Paradigm
  label: string
  short: string
  desc: string
  /** A 表单工作台 = 有计划的详细编辑；C = 无头绪启发式编辑（#07 反馈修订） */
  hint: string
}
export const PARADIGMS: ParadigmMeta[] = [
  { key: 'A', label: 'A 表单工作台', short: 'A', desc: '表单工作台', hint: '目标明确、逐项精修' },
  { key: 'C', label: 'C AI 结对', short: 'C', desc: 'AI 结对', hint: '没有头绪时的启发式编辑' },
  { key: 'B', label: 'B 文档', short: 'B', desc: '文档视图', hint: '叙事自由书写' }
]
export function restoreParadigm(): Paradigm {
  try {
    const v = localStorage.getItem(PARADIGM_LS_KEY)
    if (v === 'A' || v === 'B' || v === 'C') return v
  } catch { /* 忽略 */ }
  return 'A'
}
export function rememberParadigm(p: Paradigm): void {
  try { localStorage.setItem(PARADIGM_LS_KEY, p) } catch { /* 忽略 */ }
}

// ---------- 实体寻址 ----------
/** 宽泛行类型：大部分实体按 id 寻址；维度（AttributeDimension）按 key 寻址（#01 ④ / #07 ④） */
type Row = Record<string, unknown>
/** 返回某 kind 在草稿里的实体数组（读写同一引用）；未知 kind 返回 null */
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
    case 'scene': return d.skeleton.flatMap(ch => ch.scenes) as unknown as Row[]
    default: return null
  }
}
function kindLabel(kind: string): string {
  return { location: '地点', resource: '资源', dimension: '属性维度', character: '人物', skill: '技能', item: '物品', object: '物件', faction: '势力', chapter: '章节', scene: '场景', relationship: '关系', goal: '目标', beat: '节拍' }[kind] ?? kind
}
/** 实体的寻址字段值：维度 = key，其余 = id */
function rowKey(e: Row): string { return String(e.id ?? e.key ?? '') }
/** 实体的展示名 */
function rowName(e: Row): string { return String(e.name ?? e.title ?? rowKey(e)) }
const KIND_PREFIX: Record<string, string> = {
  location: 'loc', resource: 'res', dimension: 'dim', character: 'char', skill: 'sk',
  item: 'it', object: 'obj', faction: 'fac', chapter: 'ch', scene: 'sc', goal: 'g', beat: 'b', relationship: 'rel'
}

export const useEditorStore = defineStore('editor', () => {
  // ---------- state ----------
  const docId = ref<string | null>(null)
  const draft = ref<Storybook | null>(null)
  const baseVersion = ref(0)
  const revision = ref(0)
  const published = ref(false)
  const releasedAt = ref<string | null>(null)
  const updatedAt = ref<string | null>(null)
  const dirty = ref(false)
  const issues = ref<ValidationIssue[]>([])
  const saved = ref(false)
  const loading = ref(false)
  const saving = ref(false)
  const publishing = ref(false)
  const loadError = ref<string | null>(null)
  const conflict = ref(false)
  const paradigm = ref<Paradigm>(restoreParadigm())

  // 内部：抑制 dirty 的赋值（load / create / publish 元信息同步）
  // 用计数器而非布尔：deep watch 默认 pre flush 在微任务里跑，布尔会在同步块后被复位 → 误判；
  // flush:'sync' 让 watcher 在同一同步块内（计数仍 >0）执行。
  let suppressDirty = 0
  const epoch = ref(0)          // 每次真实内容变更 +1，用于跨请求识别"保存期间又有新编辑"
  let saveTimer: ReturnType<typeof setTimeout> | null = null
  let validateTimer: ReturnType<typeof setTimeout> | null = null
  let pendingSave = false
  let validateSeq = 0

  // 深监听：任何字段被编辑 → dirty + 调度自动保存与校验
  watch(draft, () => {
    if (suppressDirty > 0 || !draft.value) return
    epoch.value++
    dirty.value = true
    scheduleSave()
    scheduleValidate()
  }, { deep: true, flush: 'sync' })

  // ---------- 校验（debounced 实时刷新） ----------
  function scheduleValidate(delay = 450): void {
    if (validateTimer) clearTimeout(validateTimer)
    validateTimer = setTimeout(() => { void runValidate() }, delay)
  }
  async function runValidate(): Promise<void> {
    const d = draft.value
    if (!d || loading.value) return
    const seq = ++validateSeq
    const res = await validateStorybook(d)
    if (seq === validateSeq) issues.value = res.issues
  }
  function replaceIssues(list: ValidationIssue[]): void { issues.value = list }

  // ---------- 自动保存（debounced ~1.2s，不 bump 版次） ----------
  function scheduleSave(delay = 1200): void {
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = setTimeout(() => {
      if (saving.value) { pendingSave = true; return }
      void save()
    }, delay)
  }
  async function save(): Promise<boolean> {
    const d = draft.value
    const id = docId.value
    if (!d || !id || saving.value || conflict.value) return false
    saving.value = true
    const e0 = epoch.value
    try {
      // JSON 往返深拷贝：去除嵌套 reactive proxy（structuredClone 对 proxy 抛 DataCloneError；
      // toRaw 只解最外层。原型数据均为 JSON-safe，直接 JSON 序列化最稳）
      const snapshot = JSON.parse(JSON.stringify(d)) as Storybook
      const { doc, issues: serverIssues } = await saveDraft(id, snapshot, baseVersion.value)
      syncDocMeta(doc, false)
      replaceIssues(serverIssues)
      if (epoch.value === e0) {
        dirty.value = false
        saved.value = true
      } else {
        scheduleSave()
      }
      return true
    } catch (e) {
      const code = (e as { code?: string })?.code
      const msg = (e as Error)?.message ?? String(e)
      if (code === 'CONFLICT' || /CONFLICT/i.test(msg)) {
        conflict.value = true
        toast('warn', '草稿版本冲突：已被其他会话修改')
      } else {
        toast('error', '自动保存失败：' + msg)
      }
      return false
    } finally {
      saving.value = false
      if (pendingSave) { pendingSave = false; scheduleSave(50) }
    }
  }

  // ---------- 元信息同步（不触碰草稿内容，也不标 dirty） ----------
  function syncDocMeta(doc: StorybookDocument, replaceDraft: boolean): void {
    suppressDirty++
    try {
      docId.value = doc.id
      baseVersion.value = doc.draft_version
      revision.value = doc.revision
      published.value = doc.published
      releasedAt.value = doc.released_at ?? null
      updatedAt.value = doc.updated_at ?? null
      if (replaceDraft) draft.value = structuredClone(doc.draft)
    } finally {
      suppressDirty--
    }
  }

  // ---------- 加载 / 新建 ----------
  async function load(id: string): Promise<boolean> {
    loading.value = true
    loadError.value = null
    try {
      const doc = await getStorybook(id)
      syncDocMeta(doc, true)
      dirty.value = false
      saved.value = true
      return true
    } catch (e) {
      loadError.value = (e as Error)?.message ?? String(e)
      return false
    } finally {
      loading.value = false
      // 必须在 loading=false 后触发，否则 runValidate 因 loading 直接短路
      void runValidate()
    }
  }

  /** 新建草稿：mock 建 doc（revision 0、released 空），返回 docId 供路由替换 */
  async function createNew(title?: string): Promise<string | null> {
    loading.value = true
    loadError.value = null
    try {
      const doc = await createStorybookDraft(title)
      syncDocMeta(doc, true)
      dirty.value = false
      saved.value = true
      return doc.id
    } catch (e) {
      loadError.value = (e as Error)?.message ?? String(e)
      return null
    } finally {
      loading.value = false
      void runValidate()
    }
  }

  // ---------- 范式（视图态切换，不重载草稿） ----------
  function setParadigm(p: Paradigm): void {
    paradigm.value = p
    rememberParadigm(p)
  }

  // ---------- 发布门（#23 ①：校验通过才 bump 版次；先落盘再发布） ----------
  async function publish(): Promise<boolean> {
    const id = docId.value
    if (!id || publishing.value) return false
    publishing.value = true
    try {
      // 有未落盘改动先保存，保证"发布我所见的版本"
      if (dirty.value) {
        const ok = await save()
        if (!ok) return false
      }
      if (conflict.value) { toast('warn', '发布中止：请先解决草稿版本冲突'); return false }
      const { doc } = await publishDraft(id, baseVersion.value)
      syncDocMeta(doc, false)
      dirty.value = false
      saved.value = true
      issues.value = []
      toast('ok', '已发布 · 版次 ' + doc.revision)
      return true
    } catch (e) {
      const code = (e as { code?: string })?.code
      const msg = (e as Error)?.message ?? String(e)
      if (code === 'VALIDATION_FAILED') {
        const detail = (e as { detail?: { issues?: ValidationIssue[] } })?.detail
        if (detail?.issues) replaceIssues(detail.issues)
        const errs = detail?.issues?.filter(i => i.severity === 'error') ?? []
        toast('error', '发布被校验拦下：' + (errs[0]?.message ?? '存在错误级问题'))
      } else if (code === 'CONFLICT' || /CONFLICT/i.test(msg)) {
        conflict.value = true
        toast('warn', '发布冲突：草稿版本已变化')
      } else {
        toast('error', '发布失败：' + msg)
      }
      return false
    } finally {
      publishing.value = false
    }
  }

  // ---------- 冲突解决（#23 ③ 乐观并发） ----------
  /** 丢弃本地，重载服务端版 */
  async function conflictReload(): Promise<void> {
    const id = docId.value
    if (!id) return
    await load(id)
    conflict.value = false
  }
  /** 强制覆盖：取服务端当前 draft_version 作 base 重发（服务端无 force 标志） */
  async function conflictOverwrite(): Promise<void> {
    const id = docId.value
    const d = draft.value
    if (!id || !d) return
    try {
      const fresh = await getStorybook(id)
      suppressDirty++
      baseVersion.value = fresh.draft_version
      suppressDirty--
      conflict.value = false
      const ok = await save()
      if (ok) toast('ok', '已强制覆盖服务端草稿')
    } catch (e) {
      toast('error', '强制覆盖失败：' + ((e as Error)?.message ?? String(e)))
    }
  }

  // ---------- 结对建议采纳（#23 ④：patch 与实体形状同构 → 落稿） ----------
  /** 建议 id / 维度的 key 归一化为实体的寻址字段 */
  function ensureKeyField(kind: string, patch: Row): void {
    if (kind === 'dimension') {
      // AttributeDimension 用 key 寻址；patch 若带了 id 则转存为 key
      if (typeof patch.id === 'string' && patch.id && typeof patch.key !== 'string') patch.key = patch.id
      delete patch.id
      if (typeof patch.key !== 'string' || !patch.key) patch.key = uid('dim')
    } else if (typeof patch.id !== 'string' || !patch.id) {
      patch.id = uid(KIND_PREFIX[kind] ?? kind)
    }
  }

  /** 返回 { ok, message }；成功则草稿已变更（deep watch 自动触发保存/校验） */
  function applySuggestion(s: PairSuggestion): { ok: boolean; message: string } {
    const d = draft.value
    if (!d) return { ok: false, message: '草稿尚未就绪' }
    const kind = s.target?.kind ?? ''
    const arr = kindList(d, kind)
    if (!arr) return { ok: false, message: '该建议类型暂不支持：' + kindLabel(kind) }
    const patch = { ...(s.patch ?? {}) } as Row

    if (s.action === 'create') {
      ensureKeyField(kind, patch)
      const entity: Row = { ...patch }
      arr.push(entity)
      return { ok: true, message: '已新增 ' + kindLabel(kind) + '「' + rowName(entity) + '」' }
    }

    if (s.action === 'delete') {
      const want = s.target.id ?? ''
      const idx = arr.findIndex(e => rowKey(e) === want)
      if (idx < 0) return { ok: false, message: '找不到待删除的 ' + kindLabel(kind) + '：' + want }
      const [removed] = arr.splice(idx, 1)
      cascadeCleanup(d, kind, rowKey(removed))
      return { ok: true, message: '已删除 ' + kindLabel(kind) + '「' + rowName(removed) + '」' }
    }

    // update：浅合并字段
    const want = s.target.id ?? ''
    const idx = arr.findIndex(e => rowKey(e) === want)
    if (idx < 0) return { ok: false, message: '找不到待更新的 ' + kindLabel(kind) + '：' + want }
    if (kind === 'dimension') delete patch.id
    Object.assign(arr[idx], patch)
    return { ok: true, message: '已更新 ' + kindLabel(kind) + '「' + String(patch.name ?? patch.title ?? rowName(arr[idx])) + '」' }
  }

  /** 删除实体后的基础级联清理（关系边 + 场景在场人物 + 物品/物件技能 + 人物维度键 + 场景地点/物件所属地点） */
  function cascadeCleanup(d: Storybook, kind: string, id: string): void {
    if (kind === 'character' || kind === 'faction') {
      const relKind = kind === 'faction' ? 'faction' : 'character'
      d.relationships = d.relationships.filter(r => !(r.from_kind === relKind && r.from_id === id) && !(r.to_kind === relKind && r.to_id === id))
      if (kind === 'character') {
        d.skeleton.forEach(ch => ch.scenes.forEach(sc => { sc.present_char_ids = (sc.present_char_ids ?? []).filter(c => c !== id) }))
      }
    }
    if (kind === 'skill') {
      d.items.forEach(it => { it.skills = (it.skills ?? []).filter(sk => sk !== id) })
      d.objects.forEach(o => { o.skills = (o.skills ?? []).filter(sk => sk !== id) })
    }
    if (kind === 'dimension') {
      d.characters.forEach(c => {
        const a = c.attributes
        if (a && id in a) {
          const next: Record<string, number | string> = {}
          Object.keys(a).forEach(k => { if (k !== id) next[k] = a[k] })
          c.attributes = next
        }
      })
    }
    if (kind === 'location') {
      d.skeleton.forEach(ch => ch.scenes.forEach(sc => { if (sc.location_id === id) sc.location_id = undefined }))
      d.objects.forEach(o => { if (o.location_id === id) o.location_id = undefined })
    }
  }

  /** 计算校验阻塞数（dock / 发布按钮用） */
  function errorCount(): number { return issues.value.filter(i => i.severity === 'error').length }
  function warningCount(): number { return issues.value.filter(i => i.severity === 'warning').length }

  // 页面离开时清理定时器
  function dispose(): void {
    if (saveTimer) clearTimeout(saveTimer)
    if (validateTimer) clearTimeout(validateTimer)
  }

  return {
    docId, draft, baseVersion, revision, published, releasedAt, updatedAt,
    dirty, issues, saved, loading, saving, publishing, loadError, conflict, paradigm,
    load, createNew, save, publish, setParadigm, runValidate, scheduleValidate,
    conflictReload, conflictOverwrite, applySuggestion, errorCount, warningCount, dispose
  }
})