// ============================================================
// editor store —— 草稿唯一主人（#22 ③ / #23）
// 整本故事书草稿 + dirty 追踪 + debounced 自动保存 + 发布门 + 校验刷新
// A / C / B 三范式共享同一草稿；范式偏好 localStorage 持久化（#22 ①）。
// ============================================================
import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { createStorybookDraft, getStorybook, playtestStorybook, publishDraft, saveDraft, toast, validateStorybook } from '@/api'
import type { ChapterDef, DeclarationDef, Definition, EntityRef, FocusEntity, GoalDef, KindDef, NarrativeSection, PairSuggestion, SceneDef, Storybook, StorybookDocument, TriggerDef, ValidationIssue } from '@/types'
import { listEntityRefs as listRefsFrom, resolveEntityRef as resolveRefFrom, resolveFocus as resolveFocusFrom } from '@/lib/entity-refs'
import { describeActionTitle, describePatch } from '@/lib/describe-patch'
import { normalizeRelationshipFields, uid } from '@/types'

// ---------- 范式（视图态，#22 ①） ----------
export type Paradigm = 'A' | 'B' | 'C'
const PARADIGM_LS_KEY = 'octopus.editor.paradigm'

export interface ParadigmMeta {
  key: Paradigm
  label: string
  desc: string
  /** key 仍是 A/C/B（#22 的范式标识）；label 面向玩家，不带字母前缀 */
  hint: string
}
export const PARADIGMS: ParadigmMeta[] = [
  { key: 'A', label: '表单工作台', desc: '表单工作台', hint: '目标明确、逐项精修' },
  { key: 'C', label: 'AI 结对', desc: 'AI 结对', hint: '没有头绪时的启发式编辑' },
  { key: 'B', label: '文档', desc: '文档视图', hint: '叙事自由书写' }
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
/** 返回某顶层 kind 在草稿里的实体数组（读写同一引用）；嵌套 / 未知 kind 返回 null */
function kindList(d: Storybook, kind: string): Row[] | null {
  switch (kind) {
    case 'location': return d.world.locations as unknown as Row[]
    case 'resource': return d.world.resources as unknown as Row[]
    case 'dimension': return d.attribute_dimensions as unknown as Row[]
    case 'character': return d.characters as unknown as Row[]
    case 'map': {
      const w = d.world
      if (!w.maps) w.maps = []
      return w.maps as unknown as Row[]
    }
    case 'skill': return d.skills as unknown as Row[]
    case 'item': return d.items as unknown as Row[]
    case 'object': return d.objects as unknown as Row[]
    case 'faction': return d.factions as unknown as Row[]
    case 'chapter': return d.skeleton as unknown as Row[]
    case 'relationship': return d.relationships as unknown as Row[]
    case 'status': return (d.statuses ?? []) as unknown as Row[]
    case 'lore': return (d.lore ?? []) as unknown as Row[]
    default: {
      // 开放内容：kind 由数据声明，按实例的 kind 字段过滤
      const defs = (d.definitions ?? []).filter(x => x.kind === kind)
      return defs.length ? (defs as unknown as Row[]) : null
    }
  }
}

/** 声明区四类（按 key 寻址） */
function declList(d: Storybook, kind: string): DeclarationDef[] | null {
  switch (kind) {
    case 'flag': return d.flags
    case 'event': return d.events
    case 'relationship_type': return d.relationship_types
    case 'target_type': return d.target_types
    default: return null
  }
}

interface SceneRef { chapter: ChapterDef; scene: SceneDef; sceneIndex: number }
/** 跨章节定位场景；给了 parentId（章节 id）则限定在该章节内 */
function findScene(d: Storybook, sceneId: string, parentId?: string): SceneRef | null {
  for (const chapter of d.skeleton) {
    if (parentId && chapter.id !== parentId) continue
    const sceneIndex = chapter.scenes.findIndex(sc => sc.id === sceneId)
    if (sceneIndex >= 0) return { chapter, scene: chapter.scenes[sceneIndex], sceneIndex }
  }
  return null
}

/** 模型给的 kind 可能是别名（attribute_dimension / characters…），归一到内部 kind */
function normalizeKind(raw: string): string {
  const k = raw.trim().toLowerCase()
  const alias: Record<string, string> = {
    attribute_dimension: 'dimension', attribute_dimensions: 'dimension', attribute: 'dimension',
    attributes: 'dimension', dimensions: 'dimension', characters: 'character',
    locations: 'location', resources: 'resource', skills: 'skill', items: 'item',
    objects: 'object', factions: 'faction', relationships: 'relationship',
    chapters: 'chapter', scenes: 'scene', goals: 'goal', triggers: 'trigger',
    statuses: 'status', lores: 'lore', maps: 'map',
  }
  return alias[k] ?? k
}

/** 实体是否已存在（upsert 判定用；scene/goal/trigger 走嵌套查找） */
function entityExists(d: Storybook, kind: string, id: string): boolean {
  if (!id) return false
  if (kind === 'scene') return !!findScene(d, id)
  if (kind === 'goal' || kind === 'trigger') {
    return d.skeleton.some(ch =>
      ch.scenes.some(sc => (kind === 'goal' ? sc.goals : sc.triggers).some(x => x.id === id)),
    )
  }
  if (kind === 'kind') return (d.kinds ?? []).some(x => x.key === id)
  if (kind === 'definition') return (d.definitions ?? []).some(x => x.id === id)
  const arr = kindList(d, kind)
  return !!arr && arr.some(e => rowKey(e) === id)
}

function kindLabel(kind: string): string {
  return {
    meta: '元信息', world: '世界设定',
    location: '地点', map: '地图', resource: '资源', dimension: '属性维度', character: '角色库条目',
    skill: '技能', item: '物品', object: '物件', faction: '势力', chapter: '章节',
    scene: '场景', goal: '目标', trigger: '触发点', relationship: '关系',
    status: '状态', lore: '词条', flag: '标记', event: '事件', relationship_type: '关系类型', target_type: '目标类型',
    kind: '种类', definition: '开放内容',
  }[kind] ?? kind
}

/** 读取工具参数里的 patch：容忍字符串化 JSON / 模型把字段平铺到顶层 */
function readPatch(args: Row): Row {
  let p: unknown = args.patch
  if (typeof p === 'string') {
    try { p = JSON.parse(p) } catch { p = {} }
  }
  if (p && typeof p === 'object' && !Array.isArray(p)) {
    return { ...(p as Row) }
  }
  const skip = new Set(['patch', 'id', 'parent_id', 'kind', 'action', 'section'])
  const out: Row = {}
  for (const [k, v] of Object.entries(args)) {
    if (!skip.has(k) && v !== undefined && v !== null) out[k] = v
  }
  return out
}

/** 实体的寻址字段值：维度 = key，其余 = id */
function rowKey(e: Row): string { return String(e.id ?? e.key ?? '') }
/** 实体的展示名 */
function rowName(e: Row): string { return String(e.name ?? e.title ?? rowKey(e)) }
const KIND_PREFIX: Record<string, string> = {
  location: 'loc', map: 'map', resource: 'res', dimension: 'dim', character: 'char', skill: 'sk',
  item: 'it', object: 'obj', faction: 'fac', chapter: 'ch', scene: 'sc', goal: 'g', trigger: 'b', relationship: 'rel',
  status: 'st', definition: 'def', lore: 'lore'
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
  const playtesting = ref(false)
  const loadError = ref<string | null>(null)
  const conflict = ref(false)
  const paradigm = ref<Paradigm>(restoreParadigm())
  const activeTab = ref('world')
  const activeEntityId = ref<string | null>(null)

  // ---------- B：工具编辑撤销栈（工具直接落稿，可逐步 / 整轮撤销） ----------
  const undoDepth = ref(0)
  const undoStack: Storybook[] = []

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
      if (replaceDraft) {
        // 读取侧兼容（决策 #11）：旧草稿的关系边可能写作 from_id/to_id，
        // 载入时归一为规范字段 from/to，之后编辑器只写规范字段。
        const cloned = structuredClone(doc.draft)
        normalizeRelationshipFields(cloned)
        draft.value = cloned
        undoStack.length = 0
        undoDepth.value = 0
      }
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

  // ---------- 沙箱试玩（Playground：未发布草稿即开即玩） ----------
  async function playtest(title?: string, controlledCharacterId?: string): Promise<string | null> {
    const id = docId.value
    if (!id || playtesting.value) return null
    playtesting.value = true
    try {
      if (dirty.value) {
        const ok = await save()
        if (!ok) return null
      }
      if (conflict.value) {
        toast('warn', '试玩中止：请先解决草稿版本冲突')
        return null
      }
      if (errorCount() > 0) {
        const first = issues.value.find(i => i.severity === 'error')
        toast('error', '草稿存在阻断性错误，无法开始试玩：' + (first?.message ?? ''))
        return null
      }
      const saveDetail = await playtestStorybook(id, { title, controlledCharacterId })
      toast('ok', `沙箱存档「${saveDetail.title}」已就绪，进入试玩`)
      return saveDetail.id
    } catch (e) {
      const code = (e as { code?: string })?.code
      const msg = (e as Error)?.message ?? String(e)
      if (code === 'VALIDATION_FAILED') {
        const detail = (e as { detail?: { issues?: ValidationIssue[] } })?.detail
        if (detail?.issues) replaceIssues(detail.issues)
        const errs = detail?.issues?.filter(i => i.severity === 'error') ?? []
        toast('error', '试玩被校验拦下：' + (errs[0]?.message ?? '存在错误级问题'))
      } else {
        toast('error', '进入沙箱试玩失败：' + msg)
      }
      return null
    } finally {
      playtesting.value = false
    }
  }

  // ---------- 校验问题定位联动 ----------
  function navigateToIssue(iss: ValidationIssue): void {
    if (paradigm.value !== 'A') {
      setParadigm('A')
    }
    if (!iss.target) return
    // 协议问题（target = narrative.protocol）定位到「协议」tab。
    if (iss.target.startsWith('narrative.protocol')) {
      activeTab.value = 'protocol'
      return
    }
    const [kind, targetId] = iss.target.split(':')
    switch (kind) {
      case 'character':
        activeTab.value = 'characters'
        break
      case 'location':
      case 'map':
      case 'resource':
        activeTab.value = 'world'
        break
      case 'skill':
        activeTab.value = 'skills'
        break
      case 'item':
        activeTab.value = 'items'
        break
      case 'object':
        activeTab.value = 'objects'
        break
      case 'faction':
        activeTab.value = 'factions'
        break
      case 'relationship':
        activeTab.value = 'relationships'
        break
      case 'dimension':
        activeTab.value = 'dimensions'
        break
      case 'status':
        activeTab.value = 'statuses'
        break
      case 'chapter':
      case 'scene':
      case 'goal':
      case 'trigger':
      case 'skeleton':
        activeTab.value = 'skeleton'
        break
      case 'meta':
        activeTab.value = 'world'
        break
      default:
        break
    }
    if (targetId) {
      activeEntityId.value = targetId
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

  /** 新建实体时补齐各 kind 必需嵌套字段，避免半成品落稿 */
  function withDefaults(kind: string, patch: Row): Row {
    switch (kind) {
      case 'chapter': return { scenes: [], ...patch }
      case 'character': return { background: '', personality: '', attributes: {}, skills: [], ...patch }
      case 'map': return { pins: [], ...patch }
      case 'location': return { description: '', ...patch }
      case 'skill': return { description: '', ...patch }
      case 'item': return { description: '', ...patch }
      case 'faction': return { description: '', ...patch }
      case 'object': return { description: '', actions: [], skills: [], ...patch }
      case 'resource': return { type: 'numerical', ...patch }
      case 'status': return { description: '', duration: 1, unit: 'turns', ...patch }
      case 'lore': return { title: '', content: '', keys: [], priority: 0, enabled: true, ...patch }
      case 'dimension': {
        const rawType = String(patch.type ?? 'number').toLowerCase()
        const type = rawType === 'enum' || rawType === '枚举' ? 'enum'
          : rawType === 'text' || rawType === 'string' || rawType === '文本' ? 'text'
            : 'number'
        const out: Row = { ...patch, type }
        out.label = String(patch.label ?? patch.key ?? '新维度')
        if (type === 'number') {
          if (out.min == null) out.min = 0
          if (out.max == null) out.max = 100
          if (out.baseline == null) out.baseline = 50
        }
        if (type === 'enum') {
          let opts: unknown = patch.options
          if (typeof opts === 'string') opts = opts.split(/[,，、|/]/).map(s => s.trim()).filter(Boolean)
          out.options = Array.isArray(opts) ? opts : (opts == null ? [] : [String(opts)])
        }
        return out
      }
      default: return patch
    }
  }

  /** 应用单元：把一条建议写入**给定**草稿（纯应用，不含撤销快照）。
   *  支持：meta / world 单例、声明区、scene / goal / trigger 嵌套实体、其余顶层实体 */
  function applySuggestionTo(d: Storybook | null, s: PairSuggestion): { ok: boolean; message: string } {
    if (!d) return { ok: false, message: '草稿尚未就绪' }
    const kind = s.target?.kind ?? ''
    const action = s.action
    const patch = { ...(s.patch ?? {}) } as Row

    // ---- 单例：元信息 / 世界设定（仅 update） ----
    if (kind === 'meta') {
      Object.assign(d.meta, patch)
      return { ok: true, message: '已更新故事书元信息' }
    }
    if (kind === 'world') {
      Object.assign(d.world, patch)
      return { ok: true, message: '已更新世界设定' }
    }

    // ---- 声明区四类（按 key 寻址） ----
    const decl = declList(d, kind)
    if (decl) {
      if (action === 'create') {
        const key = String(patch.key ?? '')
        if (!key) return { ok: false, message: '声明条目缺少 key' }
        if (decl.some(x => x.key === key)) return { ok: false, message: '已存在同名' + kindLabel(kind) + '：' + key }
        decl.unshift({ key, label: patch.label == null ? undefined : String(patch.label) })
        return { ok: true, message: '已新增' + kindLabel(kind) + '「' + key + '」' }
      }
      const wantKey = s.target.id ?? ''
      const di = decl.findIndex(x => x.key === wantKey)
      if (di < 0) return { ok: false, message: '找不到' + kindLabel(kind) + '：' + wantKey }
      if (action === 'delete') {
        decl.splice(di, 1)
        return { ok: true, message: '已删除' + kindLabel(kind) + '「' + wantKey + '」' }
      }
      Object.assign(decl[di], patch)
      if (patch.key != null) decl[di].key = String(patch.key)
      return { ok: true, message: '已更新' + kindLabel(kind) + '「' + String(patch.key ?? wantKey) + '」' }
    }

    // ---- 场景（嵌套于章节；create 需 parent_id = 章节 id） ----
    if (kind === 'scene') {
      if (action === 'create') {
        const chapter = d.skeleton.find(c => c.id === s.target.parent_id) ?? d.skeleton[0]
        if (!chapter) return { ok: false, message: '还没有章节，无法新增场景' }
        const scene = {
          id: String(patch.id ?? uid('sc')),
          title: String(patch.title ?? '新场景'),
          description: patch.description == null ? undefined : String(patch.description),
          location_id: patch.location_id == null ? undefined : String(patch.location_id),
          present_char_ids: (patch.present_char_ids as string[] | undefined) ?? [],
          goals: (patch.goals as GoalDef[] | undefined) ?? [],
          triggers: (patch.triggers as TriggerDef[] | undefined) ?? [],
        } as SceneDef
        chapter.scenes.push(scene)
        return { ok: true, message: '已在章节「' + chapter.title + '」新增场景「' + scene.title + '」' }
      }
      const ref = findScene(d, s.target.id ?? '', s.target.parent_id)
      if (!ref) return { ok: false, message: '找不到场景：' + (s.target.id ?? '') }
      if (action === 'delete') {
        ref.chapter.scenes.splice(ref.sceneIndex, 1)
        return { ok: true, message: '已删除场景「' + ref.scene.title + '」' }
      }
      delete patch.id
      Object.assign(ref.scene, patch)
      return { ok: true, message: '已更新场景「' + ref.scene.title + '」' }
    }

    // ---- 目标 / 触发点（嵌套于场景；create 需 parent_id = 场景 id） ----
    if (kind === 'goal' || kind === 'trigger') {
      const sceneRef = s.target.parent_id ? findScene(d, s.target.parent_id) : null
      if (action === 'create') {
        if (!sceneRef) return { ok: false, message: '新增' + kindLabel(kind) + '需先指定所属场景（parent_id）' }
        const sc = sceneRef.scene
        if (kind === 'goal') {
          const goal: GoalDef = {
            id: String(patch.id ?? uid('g')),
            text: String(patch.text ?? '新目标'),
            primary: Boolean(patch.primary ?? false),
            hidden: Boolean(patch.hidden ?? false),
            condition: (patch.condition as GoalDef['condition']) ?? null,
          }
          sc.goals.push(goal)
          return { ok: true, message: '已在场景「' + sc.title + '」新增目标' }
        }
        const trigger: TriggerDef = {
          id: String(patch.id ?? uid('b')),
          title: String(patch.title ?? '新触发点'),
          description: patch.description == null ? undefined : String(patch.description),
          hint: String(patch.hint ?? ''),
          repeatable: Boolean(patch.repeatable ?? false),
          condition: (patch.condition as TriggerDef['condition']) ?? null,
        }
        sc.triggers.push(trigger)
        return { ok: true, message: '已在场景「' + sc.title + '」新增触发点「' + trigger.title + '」' }
      }
      const pools: SceneDef[] = sceneRef ? [sceneRef.scene] : d.skeleton.flatMap(c => c.scenes)
      for (const sc of pools) {
        const arr = (kind === 'goal' ? sc.goals : sc.triggers) as unknown as Row[]
        const idx = arr.findIndex(x => rowKey(x) === (s.target.id ?? ''))
        if (idx < 0) continue
        if (action === 'delete') {
          const [removed] = arr.splice(idx, 1)
          return { ok: true, message: '已删除' + kindLabel(kind) + '「' + rowName(removed) + '」' }
        }
        delete patch.id
        Object.assign(arr[idx], patch)
        return { ok: true, message: '已更新' + kindLabel(kind) + '「' + rowName(arr[idx]) + '」' }
      }
      return { ok: false, message: '找不到' + kindLabel(kind) + '：' + (s.target.id ?? '') }
    }

    // ---- 开放种类（KindDef，按 key 寻址；upsert 语义） ----
    if (kind === 'kind') {
      if (!d.kinds) d.kinds = []
      const kinds = d.kinds
      const wantKey = String(patch.key ?? s.target.id ?? '')
      if (action === 'delete') {
        const di = kinds.findIndex(x => x.key === wantKey)
        if (di < 0) return { ok: false, message: '找不到种类：' + wantKey }
        kinds.splice(di, 1)
        d.definitions = (d.definitions ?? []).filter(x => x.kind !== wantKey)
        return { ok: true, message: '已删除种类「' + wantKey + '」及其内容' }
      }
      if (!wantKey) return { ok: false, message: '种类缺少 key' }
      const fields = Array.isArray(patch.fields) ? (patch.fields as KindDef['fields']) : undefined
      const existing = kinds.find(x => x.key === wantKey)
      if (existing) {
        if (fields) existing.fields = fields
        if (patch.label != null) existing.label = String(patch.label)
        if (patch.group != null) existing.group = String(patch.group)
        if (patch.singleton != null) existing.singleton = Boolean(patch.singleton)
        return { ok: true, message: '已更新种类「' + wantKey + '」' }
      }
      kinds.push({
        key: wantKey,
        label: String(patch.label ?? wantKey),
        group: patch.group == null ? undefined : String(patch.group),
        singleton: Boolean(patch.singleton ?? false),
        fields: fields ?? [],
      })
      return { ok: true, message: '已新增种类「' + wantKey + '」' }
    }

    // ---- 开放内容（Definition，按 id 寻址） ----
    if (kind === 'definition') {
      if (!d.definitions) d.definitions = []
      const defs = d.definitions
      const want = s.target.id ?? ''
      if (action === 'delete') {
        const di = defs.findIndex(e => e.id === want)
        if (di < 0) return { ok: false, message: '找不到开放内容：' + want }
        const [removed] = defs.splice(di, 1)
        return { ok: true, message: '已删除开放内容「' + String(removed.name ?? want) + '」' }
      }
      if (action === 'create') {
        const defKind = String(patch.kind ?? '')
        if (!defKind) return { ok: false, message: '开放内容缺少 kind（先指定所属种类）' }
        if (!(d.kinds ?? []).some(x => x.key === defKind)) {
          return { ok: false, message: '尚未定义种类「' + defKind + '」——请先用 upsert_kind 建立它' }
        }
        const def: Definition = {
          id: String(patch.id ?? uid(defKind.slice(0, 3) || 'def')),
          kind: defKind,
          name: String(patch.name ?? '新' + defKind),
          description: patch.description == null ? undefined : String(patch.description),
          fields: (patch.fields as Record<string, unknown> | undefined) ?? {},
          ...(patch.mechanics && typeof patch.mechanics === 'object'
            ? { mechanics: patch.mechanics as Definition['mechanics'] }
            : {}),
        }
        defs.unshift(def)
        return { ok: true, message: '已新增开放内容「' + def.name + '」' }
      }
      const di = defs.findIndex(e => e.id === want)
      if (di < 0) return { ok: false, message: '找不到开放内容：' + want }
      delete patch.id
      delete patch.kind
      Object.assign(defs[di], patch)
      return { ok: true, message: '已更新开放内容「' + String(patch.name ?? defs[di].name ?? want) + '」' }
    }

    // ---- 顶层实体（含 chapter；scene 已在上面单独处理） ----
    const arr = kindList(d, kind)
    if (!arr) return { ok: false, message: '该建议类型暂不支持：' + kindLabel(kind) }

    if (action === 'create') {
      ensureKeyField(kind, patch)
      const entity: Row = { ...withDefaults(kind, patch) }
      if (kind === 'chapter') arr.push(entity) // 章节按故事顺序追加（#29：倒序没必要）
      else arr.unshift(entity)
      return { ok: true, message: '已新增' + kindLabel(kind) + '「' + rowName(entity) + '」' }
    }

    if (action === 'delete') {
      const want = s.target.id ?? ''
      const idx = arr.findIndex(e => rowKey(e) === want)
      if (idx < 0) return { ok: false, message: '找不到待删除的' + kindLabel(kind) + '：' + want }
      const [removed] = arr.splice(idx, 1)
      cascadeCleanup(d, kind, rowKey(removed))
      return { ok: true, message: '已删除' + kindLabel(kind) + '「' + rowName(removed) + '」' }
    }

    const want = s.target.id ?? ''
    const idx = arr.findIndex(e => rowKey(e) === want)
    if (idx < 0) return { ok: false, message: '找不到待更新的' + kindLabel(kind) + '：' + want }
    if (kind === 'dimension') delete patch.id
    Object.assign(arr[idx], patch)
    return { ok: true, message: '已更新' + kindLabel(kind) + '「' + String(patch.name ?? patch.title ?? rowName(arr[idx])) + '」' }
  }

  function pushUndo(snapshot: Storybook): void {
    undoStack.push(snapshot)
    if (undoStack.length > 60) undoStack.shift()
    undoDepth.value = undoStack.length
  }

  /** 单条应用（含一次撤销快照）。 */
  function applySuggestion(s: PairSuggestion): { ok: boolean; message: string } {
    const d = draft.value
    if (!d) return { ok: false, message: '草稿尚未就绪' }
    const snapshot = JSON.parse(JSON.stringify(d)) as Storybook
    const r = applySuggestionTo(d, s)
    if (r.ok) pushUndo(snapshot)
    return r
  }

  /** 批量应用（整批只压一次撤销快照），返回成功项 id 与失败原因。 */
  function applySuggestions(list: PairSuggestion[]): { appliedIds: string[]; failed: string[] } {
    const d = draft.value
    if (!d || !list.length) return { appliedIds: [], failed: [] }
    const snapshot = JSON.parse(JSON.stringify(d)) as Storybook
    const appliedIds: string[] = []
    const failed: string[] = []
    for (const s of list) {
      const r = applySuggestionTo(d, s)
      if (r.ok) appliedIds.push(s.id)
      else failed.push(r.message)
    }
    if (appliedIds.length) pushUndo(snapshot)
    return { appliedIds, failed }
  }

  /**
   * 采纳 ST 预设导入的叙述段（P3）：整批只压一次撤销快照，追加到 narrative.sections 头部。
   * id 冲突时补后缀，避免与既有段撞 id 导致发布校验报错。
   */
  function adoptNarrativeSections(sections: NarrativeSection[], rating?: 'sfw' | 'nsfw'): void {
    const d = draft.value
    if (!d || (!sections.length && !rating)) return
    const snapshot = JSON.parse(JSON.stringify(d)) as Storybook
    if (!d.narrative) d.narrative = { sections: [] }
    if (!d.narrative.sections) d.narrative.sections = []
    const used = new Set(d.narrative.sections.map(s => s.id))
    for (const s of sections) {
      let id = s.id
      let n = 2
      while (used.has(id)) id = s.id + '-' + n++
      used.add(id)
      d.narrative.sections.unshift({ ...s, id })
    }
    if (rating) d.meta.rating = rating
    pushUndo(snapshot)
  }

  // ---------- B：function calling 工具执行（先暂存，批准后才合并） ----------
  /** 工具名 + 参数 → 内部建议形状 */
  function mapToolCall(name: string, args: Row): PairSuggestion | null {
    const patch = readPatch(args)
    if (name === 'set_meta') {
      return { id: uid('tool'), action: 'update', target: { kind: 'meta' }, patch, label: '更新元信息', summary: '' }
    }
    if (name === 'set_world') {
      return { id: uid('tool'), action: 'update', target: { kind: 'world' }, patch, label: '更新世界设定', summary: '' }
    }
    if (name === 'upsert_entity') {
      const kind = normalizeKind(String(args.kind ?? ''))
      const id = args.id == null || String(args.id).trim() === '' ? undefined : String(args.id)
      const parentId = args.parent_id == null || String(args.parent_id).trim() === '' ? undefined : String(args.parent_id)
      return { id: uid('tool'), action: id ? 'update' : 'create', target: { kind, id, parent_id: parentId }, patch, label: '', summary: '' }
    }
    if (name === 'delete_entity') {
      const kind = normalizeKind(String(args.kind ?? ''))
      const id = args.id == null ? undefined : String(args.id)
      const parentId = args.parent_id == null ? undefined : String(args.parent_id)
      return { id: uid('tool'), action: 'delete', target: { kind, id, parent_id: parentId }, patch, label: '', summary: '' }
    }
    if (name === 'set_declarations') {
      const map: Record<string, string> = { flags: 'flag', events: 'event', relationship_types: 'relationship_type', target_types: 'target_type' }
      const kind = map[String(args.section ?? '')]
      if (!kind) return null
      const action = (String(args.action ?? 'create') || 'create') as PairSuggestion['action']
      const key = args.key == null || String(args.key).trim() === '' ? undefined : String(args.key)
      if (action === 'create' && !patch.key) patch.key = key ?? ''
      return { id: uid('tool'), action, target: { kind, id: action === 'create' ? undefined : key }, patch, label: '', summary: '' }
    }
    if (name === 'upsert_kind') {
      const key = args.key != null && String(args.key).trim() !== ''
        ? String(args.key)
        : (patch.key != null ? String(patch.key) : undefined)
      return { id: uid('tool'), action: 'update', target: { kind: 'kind', id: key }, patch, label: '', summary: '' }
    }
    if (name === 'delete_kind') {
      const key = args.key == null ? undefined : String(args.key)
      return { id: uid('tool'), action: 'delete', target: { kind: 'kind', id: key }, patch, label: '', summary: '' }
    }
    if (name === 'upsert_definition') {
      const defKind = String(args.kind ?? '').trim()
      const id = args.id == null || String(args.id).trim() === '' ? undefined : String(args.id)
      if (defKind && patch.kind == null) patch.kind = defKind
      return { id: uid('tool'), action: id ? 'update' : 'create', target: { kind: 'definition', id }, patch, label: '', summary: '' }
    }
    if (name === 'delete_definition') {
      const id = args.id == null ? undefined : String(args.id)
      return { id: uid('tool'), action: 'delete', target: { kind: 'definition', id }, patch, label: '', summary: '' }
    }
    return null
  }

  // ---------- 结对暂存：AI 改动只落暂存草稿，批准后才合并（草稿零污染） ----------
  let stagingDraft: Storybook | null = null
  let stagedSuggestions: PairSuggestion[] = []

  /** 开始一轮工具暂存：克隆当前草稿，工具只改克隆体。 */
  function beginToolTurn(): number {
    stagingDraft = draft.value ? (JSON.parse(JSON.stringify(draft.value)) as Storybook) : null
    stagedSuggestions = []
    return undoStack.length
  }

  /** 结束一轮工具暂存：交出本轮拟改动，不合并。 */
  function finishToolTurn(): PairSuggestion[] {
    const out = stagedSuggestions
    stagedSuggestions = []
    stagingDraft = null
    return out
  }

  /** 放弃本轮暂存（真草稿本来就未被改动）。 */
  function discardStagedTurn(): void {
    stagedSuggestions = []
    stagingDraft = null
  }

  /** 当前有效草稿：暂存态优先（供下一轮上下文带上已暂存实体，保证链式引用）。 */
  function stagedOrLive(): Storybook | null {
    return stagingDraft ?? draft.value
  }

  // ---------- 实体引用（#23 ④ 精准指向）：枚举 / 解析 / 生成目标 ----------
  // 实现抽到 @/lib/entity-refs（游玩页共用同一套口径，保证解析结果一致）。
  function listEntityRefs(): EntityRef[] {
    return draft.value ? listRefsFrom(draft.value) : []
  }

  /** 把一条引用解析为草稿里的完整实体定义（找不到返回 undefined）。 */
  function resolveEntityRef(ref: EntityRef): unknown {
    return draft.value ? resolveRefFrom(draft.value, ref) : undefined
  }

  /** 发送前：把引用的实体解析成带完整定义的目标列表（给模型）。 */
  function resolveFocus(refs: EntityRef[]): FocusEntity[] {
    return draft.value ? resolveFocusFrom(draft.value, refs) : []
  }

  /** 在暂存草稿上执行一次工具调用：不触碰真草稿，结果回灌给模型继续链式调用。 */
  function runToolCall(name: string, args: Row): { ok: boolean; message: string; data?: unknown } {
    const target = stagingDraft
    if (!target) return { ok: false, message: '未处于暂存态：工具改动必须先经 beginToolTurn' }
    const suggestion = mapToolCall(name, args)
    if (!suggestion) return { ok: false, message: '未知工具：' + name }
    const kind = suggestion.target.kind
    const patch = suggestion.patch as Row
    // 真正的 upsert 语义：upsert_entity 带了 id 但草稿里不存在 → 当作新建
    if (name === 'upsert_entity' && suggestion.action === 'update' && suggestion.target.id && !entityExists(target, kind, suggestion.target.id)) {
      const want = suggestion.target.id
      suggestion.action = 'create'
      suggestion.target.id = undefined
      if (kind === 'dimension') {
        if (patch.key == null) patch.key = want
      } else if (patch.id == null) {
        patch.id = want
      }
    }
    // 种类同理：upsert_kind 的 key 不存在 → 当作新建（卡片标题才显示「新增」）
    if (name === 'upsert_kind' && suggestion.target.id && !entityExists(target, 'kind', suggestion.target.id)) {
      suggestion.action = 'create'
    }
    const isKeyAddressed = kind === 'flag' || kind === 'event' || kind === 'relationship_type' || kind === 'target_type' || kind === 'kind'
    if (suggestion.action === 'create' && !isKeyAddressed) ensureKeyField(kind, patch)
    const r = applySuggestionTo(target, suggestion)
    if (!r.ok) return r
    // 卡片文案：标题用人话（动作 + 中文类型 + 友好名），正文是字段列表——不展示 JSON。
    const friendly = describeActionTitle(kind, suggestion.action, patch)
    suggestion.label = friendly.includes('「') ? friendly : r.message
    suggestion.summary = ''
    suggestion.details = describePatch(patch)
    stagedSuggestions.push(suggestion)
    const assigned = isKeyAddressed
      ? (patch.key == null ? undefined : String(patch.key))
      : (patch.key != null ? String(patch.key) : (patch.id != null ? String(patch.id) : undefined))
    const resultId = suggestion.action === 'delete' ? suggestion.target.id : (suggestion.target.id ?? assigned)
    return { ok: true, message: r.message, data: resultId ? { id: resultId } : undefined }
  }

  /** 撤销最后一次工具编辑 */
  function undoLastTool(): boolean {
    if (!undoStack.length) return false
    draft.value = undoStack.pop() as Storybook
    undoDepth.value = undoStack.length
    return true
  }

  /** 连续撤销到指定 mark（mark = 本轮开始前的深度），返回撤销步数 */
  function undoToMark(mark: number): number {
    let n = 0
    while (undoStack.length > mark) {
      const snap = undoStack.pop() as Storybook
      n++
      if (undoStack.length === mark) draft.value = snap
    }
    undoDepth.value = undoStack.length
    return n
  }

  /** 删除实体后的基础级联清理（关系边 + 场景在场人物 + 物品/物件技能 + 人物维度键 + 场景地点/物件所属地点） */
  function cascadeCleanup(d: Storybook, kind: string, id: string): void {
    if (kind === 'character' || kind === 'faction') {
      const relKind = kind === 'faction' ? 'faction' : 'character'
      d.relationships = d.relationships.filter(r => !(r.from_kind === relKind && r.from === id) && !(r.to_kind === relKind && r.to === id))
      if (kind === 'character') {
        d.skeleton.forEach(ch => ch.scenes.forEach(sc => { sc.present_char_ids = (sc.present_char_ids ?? []).filter(c => c !== id) }))
      }
    }
    if (kind === 'skill') {
      d.items.forEach(it => { it.skills = (it.skills ?? []).filter(sk => sk !== id) })
      d.objects.forEach(o => { o.skills = (o.skills ?? []).filter(sk => sk !== id) })
      d.characters.forEach(c => { c.skills = (c.skills ?? []).filter(sk => sk !== id) })
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
      // 地图锚点指向地点：地点没了就摘掉锚点，别留悬空引用（校验会报 dangling_map_pin_location_ref）
      d.world.maps?.forEach(m => { if (m.pins) m.pins = m.pins.filter(p => p.location_id !== id) })
    }
    if (kind === 'map') {
      // 子图 parent_id 指向被删地图 → 摘掉，避免 dangling_map_parent_ref
      d.world.maps?.forEach(m => { if (m.parent_id === id) delete m.parent_id })
    }
    if (kind === 'status') {
      d.skills.forEach(sk => {
        if (sk.effect?.status) sk.effect.status = sk.effect.status.filter(sid => sid !== id)
      })
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
    dirty, issues, saved, loading, saving, publishing, playtesting, loadError, conflict, paradigm,
    activeTab, activeEntityId,
    load, createNew, save, publish, playtest, navigateToIssue, setParadigm, runValidate, scheduleValidate,
    conflictReload, conflictOverwrite, applySuggestion, applySuggestions, adoptNarrativeSections, errorCount, warningCount, dispose,
    runToolCall, beginToolTurn, finishToolTurn, discardStagedTurn, stagedOrLive,
    listEntityRefs, resolveEntityRef, resolveFocus, undoLastTool, undoToMark, undoDepth
  }
})

// pair-tool executor (B) — 直接落稿 + 撤销栈