// ============================================================
// 游玩页 store（#18 ② 全量共享 store + #17 事件消费）
// 职责：世界投影（本地镜像，经 hydrate + state_changes delta 增量更新）、
// 演出流条目数组、管线阶段、打字机调度、回合提交与确认门编排。
// ============================================================
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import type {
  WorldProjection, PlayEvent, PhaseStage, SaveDetail, CheckResultPayload, StateDelta, CharacterInstance,
  EntityRef, FocusEntity, Storybook
} from '@/types'
import { hydrate, subscribe, submitRound, rerunRound, confirmAction, getSave, getHistory, getSaveSettings, switchCharacter, setSaveSettings, toast } from '@/api'
import { uid } from '@/types'
import { listEntityRefs as listRefsFrom, resolveFocus as resolveFocusFrom } from '@/lib/entity-refs'

export type ContentType = 'narrate' | 'dialogue' | 'emote'

/** 演出流条目（渲染器数据源；kind 判别联合） */
export type FeedEntry =
  | { key: string; kind: 'scene'; round: number; sceneId: string; title: string; description?: string; ts?: string }
  | { key: string; kind: 'content'; round: number; type: ContentType; text: string; actorId?: string; actorName?: string; emotion?: string; reveal: number; done: boolean; ts?: string; /** 气泡内台词前的动作/神态行（同说话人的 emote 收进来，避免另起一行割裂） */ actionBefore?: string; /** 气泡内台词后的动作/神态行 */ actionAfter?: string }
  | { key: string; kind: 'round'; round: number; channel: 'character' | 'meta' | 'gm'; text: string; actorName?: string; actorId?: string; ts?: string; /** 玩家显式引用的实体 */ refs?: EntityRef[]; /** 乐观追加、尚未与 round_start 事件对账 */ pending?: boolean }
  | { key: string; kind: 'system'; round: number; level: 'info' | 'warn' | 'error'; code?: string; text: string; ts?: string }
  | { key: string; kind: 'check'; round: number; payload: CheckResultPayload; ts?: string }
  | { key: string; kind: 'pending'; round: number; state: 'pending' | 'confirmed' | 'cancelled'; actionId: string; description: string; impact?: string; actorName: string; ts?: string }
  | { key: string; kind: 'resolution'; round: number; status: 'ok' | 'rejected'; text?: string; ts?: string }
  | { key: string; kind: 'progress'; round: number; tone: 'goal' | 'trigger' | 'quest'; label: string; ts?: string }
  | { key: string; kind: 'reasoning'; round: number; stage: string; text: string; /** P3：provider = 供应商 reasoning_content；model = think 意图 */ source: 'provider' | 'model'; ts?: string }

const TYPE_SPEED_MS = 20
/** 每次拉取的历史页大小（后端上限 200） */
const HISTORY_PAGE = 200

export const usePlayStore = defineStore('play', () => {
  const saveId = ref('')
  const ready = ref(false)
  const error = ref('')
  const projection = ref<WorldProjection | null>(null)
  const detail = ref<SaveDetail | null>(null)
  const entries = ref<FeedEntry[]>([])
  const phase = ref<PhaseStage>('idle')
  const phaseDetail = ref('')
  const sending = ref(false)
  const confirmBusy = ref(false)
  /** 打字机每揭示一字符 +1，供自动滚动/光标闪烁监听 */
  const revealPulse = ref(0)
  /** C 沉浸式「点击前进」脉冲：揭示完当前条目后点击 → +1（外层渲染器据此推进） */
  const advancePulse = ref(0)
  const goalTexts = ref<Record<string, string>>({})
  const triggerTexts = ref<Record<string, string>>({})
  /** 输入框待发送的实体引用（本次行动的目标） */
  const pendingRefs = ref<EntityRef[]>([])
  /** 本存档覆盖的模型（None = 用全局角色默认） */
  const saveModel = ref<{ provider_id: string; model: string; reasoning_effort?: string } | null>(null)
  /** 叙述段玩家偏好（叙事契约 P1）：section id → 开关(bool) | 变体 key(string)；只影响之后的回合 */
  const narrativePrefs = ref<Record<string, boolean | string>>({})
  /** 已加载的最旧事件 seq（分页游标） */
  const oldestSeq = ref<number | null>(null)
  const hasMoreOlder = ref(false)
  const loadingOlder = ref(false)

  let unsub: (() => void) | null = null
  let keySeq = 0
  let ticker: ReturnType<typeof setInterval> | null = null
  let watermark = 0
  /** 非空时，事件产生的条目写入该缓冲而非 entries（用于向前分页的批量重放） */
  let entrySink: FeedEntry[] | null = null

  // ---------- 派生 ----------
  const autoConfirm = computed<boolean>(() => projection.value?.meta.auto_confirm ?? false)
  const sceneTitle = computed(() => projection.value?.scene_title ?? '')
  /** 思考草稿展示策略（P3）：storybook.narrative.display.draft，缺省 folded；仅前端展示用。 */
  const draftDisplay = computed<'folded' | 'hidden'>(() => {
    const sb = detail.value?.storybook as Storybook | undefined
    return sb?.narrative?.display?.draft ?? 'folded'
  })
  const controlledId = computed(() => projection.value?.controlled[0] ?? '')
  const controlled = computed<CharacterInstance | null>(() => {
    const p = projection.value
    if (!p) return null
    const c = p.characters[controlledId.value]
    return c ?? null
  })
  /** 在场角色：受控角色置顶，其后按出现顺序 */
  const presentChars = computed(() => {
    const p = projection.value
    if (!p) return []
    const ids = controlledId.value ? [controlledId.value, ...Object.keys(p.characters).filter(k => k !== controlledId.value)] : Object.keys(p.characters)
    return ids.map(id => p.characters[id]).filter((c): c is CharacterInstance => !!c && c.present)
  })
  const allChars = computed(() => {
    const p = projection.value
    if (!p) return []
    const ids = controlledId.value ? [controlledId.value, ...Object.keys(p.characters).filter(k => k !== controlledId.value)] : Object.keys(p.characters)
    return ids.map(id => p.characters[id]).filter((c): c is CharacterInstance => !!c)
  })
  const hasOpenPending = computed(() => {
    for (const e of entries.value) {
      if (e.kind === 'pending' && e.state === 'pending') return true
    }
    return false
  })
  const busy = computed(() => sending.value || confirmBusy.value || hasOpenPending.value)
  const waitingConfirm = computed(() => phase.value === 'waiting_confirm' || hasOpenPending.value)
  /** 是否可以重跑本轮：存在一个已完成、非元指令的玩家回合 */
  const canRerun = computed(() =>
    entries.value.some(e => e.kind === 'round' && !e.pending && e.channel !== 'meta')
  )
  /** 正在打字机揭示的条目（渲染器据此画光标） */
  const streamingEntry = computed<FeedEntry | null>(() => {
    for (const en of entries.value) {
      if (en.kind === 'content' && !en.done && en.reveal < en.text.length) return en
    }
    return null
  })
  const phaseLabel = computed(() => {
    switch (phase.value) {
      case 'story_thinking': return '主线思考中'
      case 'character_thinking': return '角色回应中'
      case 'resolving': return '结算中'
      case 'waiting_confirm': return '等你确认'
      default: return ''
    }
  })

  // ---------- 工具 ----------
  function nextKey(): string { keySeq++; return 'f' + keySeq }
  function isContent(e: FeedEntry): e is Extract<FeedEntry, { kind: 'content' }> { return e.kind === 'content' }
  /** 统一入口：向前分页时写入缓冲，其余写入 entries。 */
  function addEntry(e: FeedEntry): void { (entrySink ?? entries.value).push(e) }

  // ---------- 实体引用（本次行动的目标） ----------
  /** 存档内冻结的故事书：引用枚举与解析的数据源。 */
  function storybook(): Storybook | null {
    return (detail.value?.storybook as Storybook | undefined) ?? null
  }
  /** 供引用选择器枚举。 */
  function listRefs(): EntityRef[] {
    const sb = storybook()
    return sb ? listRefsFrom(sb) : []
  }
  /** 把引用解析成带完整定义的目标（给 AI）。 */
  function focusFor(refs: EntityRef[]): FocusEntity[] {
    const sb = storybook()
    return sb ? resolveFocusFrom(sb, refs) : []
  }
  function addRef(ref: EntityRef): void {
    const key = ref.kind + ':' + (ref.id ?? '') + ':' + (ref.parent_id ?? '')
    if (pendingRefs.value.some(r => r.kind + ':' + (r.id ?? '') + ':' + (r.parent_id ?? '') === key)) return
    pendingRefs.value = [...pendingRefs.value, ref]
  }
  function removeRef(ref: EntityRef): void {
    pendingRefs.value = pendingRefs.value.filter(r =>
      !(r.kind === ref.kind && (r.id ?? '') === (ref.id ?? '') && (r.parent_id ?? '') === (ref.parent_id ?? ''))
    )
  }
  function clearRefs(): void {
    pendingRefs.value = []
  }

  function ensureTicker() {
    if (!ticker) ticker = setInterval(tick, TYPE_SPEED_MS)
  }
  function stopTicker() {
    if (ticker) { clearInterval(ticker); ticker = null }
  }
  function findStreaming(): Extract<FeedEntry, { kind: 'content' }> | null {
    for (const e of entries.value) {
      if (e.kind === 'content' && !e.done && e.reveal < e.text.length) return e
    }
    return null
  }
  function tick() {
    const cur = findStreaming()
    if (!cur) { stopTicker(); return }
    cur.reveal = Math.min(cur.text.length, cur.reveal + 1)
    revealPulse.value++
    if (cur.reveal >= cur.text.length) cur.done = true
  }
  /** 立即揭示全部流条目（回合开始/模板操作时清空打字机队列） */
  function flushAll() {
    for (const e of entries.value) {
      if (e.kind === 'content' && !e.done) { e.reveal = e.text.length; e.done = true }
    }
    stopTicker()
  }
  /** 跳过某个仍在揭示的条目 */
  function skipEntry(key: string) {
    for (const e of entries.value) {
      if (e.key === key && e.kind === 'content' && !e.done) { e.reveal = e.text.length; e.done = true; return }
    }
  }
  function skipCurrent() {
    const cur = streamingEntry.value
    if (cur) skipEntry(cur.key)
  }
  /** 点击/空格语义：正在揭示 → 跳过；已揭示完 → 触发前进脉冲（供 C 逐行演出推进） */
  function emitSkip() {
    const cur = streamingEntry.value
    if (cur) { skipEntry(cur.key) } else { advancePulse.value++ }
  }

  // ---------- 投影增量应用（#17 共享 state_changes delta） ----------
  function applyDeltas(deltas: StateDelta[]) {
    const p = projection.value
    if (!p) return
    for (const d of deltas) {
      if (d.domain === 'flag') {
        if (d.op === 'remove') delete p.flags[d.entity_id]
        else p.flags[d.entity_id] = !!d.value
      } else if (d.domain === 'goal') {
        if (d.op === 'remove') delete p.progress.goals[d.entity_id]
        else p.progress.goals[d.entity_id] = !!d.value
      } else if (d.domain === 'trigger') {
        if (d.op === 'remove') delete p.progress.triggers[d.entity_id]
        else p.progress.triggers[d.entity_id] = !!d.value
      } else if (d.domain === 'character') {
        const c = p.characters[d.entity_id]
        if (!c) continue
        if (d.field === 'present') { c.present = !!d.value }
        else if (d.field.startsWith('attributes.')) {
          const k = d.field.slice('attributes.'.length)
          if (d.op === 'remove') delete c.attributes[k]
          else if (d.op === 'add') c.attributes[k] = (Number(c.attributes[k] ?? 0)) + Number(d.value)
          else c.attributes[k] = d.value as number | string
        } else if (d.field.startsWith('resources.')) {
          const k = d.field.slice('resources.'.length)
          if (d.op === 'remove') delete c.resources[k]
          else if (d.op === 'add') c.resources[k] = (c.resources[k] ?? 0) + Number(d.value)
          else c.resources[k] = Number(d.value)
        }
      }
      // location / relationship 在 v1 游玩页暂无独立面板，忽略
    }
  }

  // ---------- 事件消费（#17 类型表） ----------
  function pushContent(
    type: ContentType,
    text: string,
    actor: { id: string; name: string } | null | undefined,
    round: number,
    emotion?: string,
    instant = false,
    key?: string,
    ts?: string
  ) {
    // 合并规则（把一段连续演出收成更少的块，避免「同一个人连发 / 旁白被切碎」的割裂感）：
    //   1. 同说话人的连续台词 → 同一个气泡；
    //   2. 神态紧贴在台词前/后（同说话人）→ 收进气泡内的动作行；
    //   3. 连续旁白 → 合并成一段。
    const sink = entrySink ?? entries.value
    const last = sink[sink.length - 1]
    const sameSpeaker =
      last && last.kind === 'content' && last.round === round &&
      (last.actorId ?? '') === (actor?.id ?? '')
    if (sameSpeaker && actor?.id) {
      const le = last as Extract<FeedEntry, { kind: 'content' }>
      if (type === 'dialogue') {
        // 神态在前：把它收进气泡作为动作行，本条台词作为正文
        if (le.type === 'emote') {
          le.type = 'dialogue'
          le.actionBefore = le.text
          le.text = text
          le.reveal = instant ? text.length : 0
          le.done = instant
          if (!le.actorName && actor.name) le.actorName = actor.name
          if (!le.emotion && emotion) le.emotion = emotion
          if (!le.ts && ts) le.ts = ts
          if (!instant) ensureTicker()
          return
        }
        // 连续台词：并进同一个气泡
        if (le.type === 'dialogue') {
          le.text = le.text ? le.text + '\n' + text : text
          if (instant) { le.reveal = le.text.length; le.done = true }
          else { le.done = false }
          if (!le.emotion && emotion) le.emotion = emotion
          if (!le.ts && ts) le.ts = ts
          if (!instant) ensureTicker()
          return
        }
      }
      if (type === 'emote') {
        // 神态在台词后：并进气泡的动作行
        if (le.type === 'dialogue') {
          le.actionAfter = le.actionAfter ? le.actionAfter + '\n' + text : text
          if (!instant) ensureTicker()
          return
        }
        // 连续神态：合并
        if (le.type === 'emote') {
          le.text = le.text ? le.text + '\n' + text : text
          if (instant) { le.reveal = le.text.length; le.done = true }
          else { le.done = false }
          if (!le.emotion && emotion) le.emotion = emotion
          if (!instant) ensureTicker()
          return
        }
      }
    }
    // 连续旁白合并成一段
    if (
      type === 'narrate' && last && last.kind === 'content' &&
      last.type === 'narrate' && last.round === round
    ) {
      last.text = last.text ? last.text + '\n' + text : text
      if (instant) { last.reveal = last.text.length; last.done = true }
      else { last.done = false }
      if (!instant) ensureTicker()
      return
    }
    const entry: Extract<FeedEntry, { kind: 'content' }> = {
      key: key ?? nextKey(), kind: 'content', round, type, text,
      actorId: actor?.id, actorName: actor?.name, emotion,
      reveal: instant ? text.length : 0, done: instant, ts
    }
    addEntry(entry)
    if (!instant) ensureTicker()
  }

  function settlePendings(round: number, status: 'ok' | 'rejected') {
    (entrySink ?? entries.value).forEach(e => {
      if (e.kind === 'pending' && e.round === round && e.state === 'pending') {
        e.state = status === 'ok' ? 'confirmed' : 'cancelled'
      }
    })
  }

  function progressLines(deltas: StateDelta[], baseKey: string, ts?: string): FeedEntry[] {
    const out: FeedEntry[] = []
    deltas.forEach((d, i) => {
      if (d.domain === 'goal') {
        const v = d.value
        // 导演新增/更新的任务：value 是对象（带 text），不是骨架目标的布尔
        const quest = v && typeof v === 'object' ? (v as { text?: string; done?: boolean }) : null
        if (quest?.text) {
          out.push({
            key: baseKey + ':q' + i, kind: 'progress', round: 0, tone: 'quest',
            label: quest.text + (quest.done ? '（已完成）' : ''), ts,
          })
        } else if (v) {
          out.push({ key: baseKey + ':g' + i, kind: 'progress', round: 0, tone: 'goal', label: goalTexts.value[d.entity_id] ?? d.entity_id, ts })
        }
      } else if (d.domain === 'trigger' && d.value) {
        out.push({ key: baseKey + ':b' + i, kind: 'progress', round: 0, tone: 'trigger', label: triggerTexts.value[d.entity_id] ?? d.entity_id, ts })
      }
    })
    return out
  }

  /**
   * 消费一条演出事件。
   * `replay` = true 时只重建 feed 条目：**不改本地投影（权威由后端重放给出）**、
   * 内容立即完整显示（历史不打字机）。实时事件才做增量与动画（规范铁律 4）。
   */
  function onEvent(e: PlayEvent, replay = false) {
    if (e.seq <= watermark) return // 丢弃已消费 / 水合前迟到事件（#17 seq 水位线）
    // 实时事件也要推进水位线：否则 SSE 重连的补拉（backfill）会把已消费的事件重放一遍，
    // 表现为同一句话出现两张回合卡 / 内容重复。
    if (!replay) watermark = e.seq
    const key = e.id
    const ts = e.ts
    switch (e.type) {
      case 'round_start': {
        flushAll()
        // 与乐观追加的玩家回合对账，避免出现两条。
        const opt = entries.value.find(
          (en): en is Extract<FeedEntry, { kind: 'round' }> =>
            en.kind === 'round' && !!en.pending && en.text === e.payload.input.text && en.channel === e.payload.input.channel
        )
        const refs = e.payload.input.refs?.length ? e.payload.input.refs : undefined
        if (opt) {
          opt.key = key
          opt.round = e.round
          opt.pending = false
          opt.ts = ts
          if (refs) opt.refs = refs
          if (!replay && e.actor) { opt.actorName = e.actor.name; opt.actorId = e.actor.id }
        } else if (!entries.value.some(en => en.key === key)) {
          // 同一事件重复到达时不要重复插卡（幂等兜底）
          addEntry({
            key, kind: 'round', round: e.round,
            channel: e.payload.input.channel, text: e.payload.input.text,
            actorName: e.actor?.name ?? (e.payload.input.channel === 'gm' ? '导演' : undefined),
            actorId: e.actor?.id, refs, ts
          })
        }
        break
      }
      case 'scene': {
        const p = projection.value
        if (p && !replay) { p.scene_id = e.payload.scene_id; p.scene_title = e.payload.title }
        addEntry({ key, kind: 'scene', round: e.round, sceneId: e.payload.scene_id, title: e.payload.title, description: e.payload.description, ts })
        break
      }
      case 'narrate': pushContent('narrate', e.payload.content, e.actor, e.round, undefined, replay, key, ts); break
      case 'dialogue': pushContent('dialogue', e.payload.content, e.actor, e.round, undefined, replay, key, ts); break
      case 'emote': pushContent('emote', e.payload.content, e.actor, e.round, e.payload.emotion, replay, key, ts); break
      case 'pending': {
        if (phase.value !== 'waiting_confirm') phase.value = 'waiting_confirm'
        addEntry({
          key, kind: 'pending', round: e.round, state: 'pending',
          actionId: e.payload.action_id, description: e.payload.description,
          impact: e.payload.impact, actorName: e.payload.actor.name, ts
        })
        break
      }
      case 'check_result': {
        addEntry({ key, kind: 'check', round: e.round, payload: e.payload, ts })
        break
      }
      case 'resolution': {
        settlePendings(e.round, e.payload.status)
        if (!replay) applyDeltas(e.payload.state_changes)
        const lines = progressLines(e.payload.state_changes, key, ts)
        if (lines.length) lines.forEach(addEntry)
        if (e.payload.status === 'ok' && e.payload.narrative) {
          addEntry({ key: key + ':res', kind: 'resolution', round: e.round, status: 'ok', text: e.payload.narrative, ts })
        } else if (e.payload.status === 'rejected') {
          addEntry({ key: key + ':res', kind: 'resolution', round: e.round, status: 'rejected', text: e.payload.narrative ?? (e.payload.rejection_code ? '意图被驳回（' + e.payload.rejection_code + '）' : '意图被驳回'), ts })
        }
        break
      }
      case 'state_update': {
        if (!replay) applyDeltas(e.payload.changes)
        const lines = progressLines(e.payload.changes, key, ts)
        if (lines.length) lines.forEach(addEntry)
        break
      }
      case 'phase': {
        phase.value = e.payload.stage
        phaseDetail.value = e.payload.detail ?? ''
        break
      }
      case 'round_end': {
        phase.value = 'idle'
        phaseDetail.value = ''
        break
      }
      case 'system': {
        // 免确认切换回显：以 system 事件文本为准同步本地投影
        if (e.payload.code === 'confirm_toggle' && projection.value) {
          projection.value.meta.auto_confirm = /开启/.test(e.payload.text)
        }
        addEntry({ key, kind: 'system', round: e.round, level: e.payload.level, code: e.payload.code, text: e.payload.text, ts })
        break
      }
      case 'reasoning': {
        // AI 思考：provider = 供应商 reasoning_content（始终折叠）；model = think 意图，
        // 按故事书 narrative.display.draft 决定折叠 / 隐藏（hidden 时不渲染，不入 feed）。
        const source = e.payload.source === 'model' ? 'model' : 'provider'
        if (source === 'model' && draftDisplay.value === 'hidden') break
        addEntry({ key, kind: 'reasoning', round: e.round, stage: e.payload.stage, text: e.payload.text, source, ts })
        break
      }
    }
  }

  // ---------- 对外动作 ----------
  async function init(id: string) {
    teardown()
    saveId.value = id
    error.value = ''
    ready.value = false
    entries.value = []
    pendingRefs.value = []
    oldestSeq.value = null
    hasMoreOlder.value = false
    loadingOlder.value = false
    try {
      const [p, d, hist, settings] = await Promise.all([hydrate(id), getSave(id), getHistory(id, undefined, HISTORY_PAGE), getSaveSettings(id)])
      projection.value = structuredClone(p)
      detail.value = d
      saveModel.value = settings.model_provider_id && settings.model
        ? { provider_id: settings.model_provider_id, model: settings.model, reasoning_effort: settings.reasoning_effort }
        : null
      narrativePrefs.value = { ...(settings.narrative ?? {}) }
      // 骨架文案索引（goal/trigger 触发时给出友好名）
      const g: Record<string, string> = {}
      const b: Record<string, string> = {}
      d.storybook.skeleton.forEach(ch => ch.scenes.forEach(sc => {
        sc.goals.forEach(go => { g[go.id] = go.text })
        sc.triggers.forEach(t => { b[t.id] = t.title })
      }))
      goalTexts.value = g
      triggerTexts.value = b
      // 回放叙事历史：只重建 feed，不改投影（投影由后端重放权威给出），历史不打字机。
      watermark = 0
      hist.events.forEach(e => onEvent(e, true))
      // 水位线取「投影 seq」与「历史最大 seq」的较大者：二者并发拉取，投影可能比历史旧；
      // 只取投影 seq 会让 SSE 首次 onopen 的补拉把已水合的事件再放一遍。
      watermark = hist.events.reduce((m, e) => Math.max(m, e.seq), p.seq)
      if (hist.events.length) {
        oldestSeq.value = hist.events[0].seq
        hasMoreOlder.value = hist.hasMore
      } else {
        entries.value.push({ key: 'init:scene', kind: 'scene', round: 0, sceneId: p.scene_id, title: p.scene_title })
        // 故事开头优先，缺省回落为世界前提（#29 opening）
        const opening = d.storybook.world.opening?.trim() || d.storybook.world.premise
        if (opening) pushContent('narrate', opening, null, 0, undefined, true, 'init:opening')
        entries.value.push({ key: 'init:hint', kind: 'system', round: 0, level: 'info', text: '直接输入想做的事（如「打听怪梦的传闻」）；以 / 开头发送元指令（/存档 /免确认 /帮助）。' })
      }
      ready.value = true
      // 先订阅再允许回合（#24 时序）；onResync：断线重连后按水位线补拉错过的事件。
      unsub = subscribe(id, onEvent, { onResync: () => { void backfill() } })
    } catch (err) {
      error.value = (err as Error)?.message ?? String(err)
      toast('error', '加载存档失败：' + error.value)
    }
  }

  /** 向前分页：拉取比当前最旧事件更早的一页，重放后前插。 */
  async function loadOlder() {
    if (!saveId.value || !hasMoreOlder.value || loadingOlder.value || oldestSeq.value === null) return
    loadingOlder.value = true
    const savedWatermark = watermark
    try {
      const hist = await getHistory(saveId.value, oldestSeq.value, HISTORY_PAGE)
      if (!hist.events.length) { hasMoreOlder.value = false; return }
      const buf: FeedEntry[] = []
      entrySink = buf
      watermark = -1 // 历史事件 seq 小于当前水位，临时放开过滤
      try {
        hist.events.forEach(e => onEvent(e, true))
      } finally {
        watermark = savedWatermark
        entrySink = null
      }
      entries.value = [...buf, ...entries.value]
      oldestSeq.value = hist.events[0].seq
      hasMoreOlder.value = hist.hasMore
    } catch (err) {
      toast('error', (err as Error)?.message ?? '加载更早历史失败')
    } finally {
      watermark = savedWatermark
      entrySink = null
      loadingOlder.value = false
    }
  }

  /** 断线重连补拉：拉最新一页，只应用水位线之后的事件。 */
  async function backfill() {
    if (!saveId.value) return
    try {
      const hist = await getHistory(saveId.value, undefined, HISTORY_PAGE)
      let maxSeq = watermark
      for (const e of hist.events) {
        if (e.seq > watermark) onEvent(e, false)
        if (e.seq > maxSeq) maxSeq = e.seq
      }
      watermark = maxSeq
    } catch {
      // noop：下一次重连会再试
    }
  }

  /**
   * 提交回合。返回是否受理。
   * 通道：/ 前缀 → 元指令；显式传 'gm' → 导演（人代替 GM）；否则 → 角色。
   */
  async function send(raw: string, explicit?: 'character' | 'gm'): Promise<boolean> {
    const text = raw.trim()
    if (!text) return false
    if (busy.value) { toast('info', waitingConfirm.value ? '有待确认动作，请先确认或取消。' : '回合结算中，请稍候…'); return false }
    if (!ready.value || !saveId.value) return false
    sending.value = true
    const channel: 'character' | 'meta' | 'gm' = text.startsWith('/')
      ? 'meta'
      : (explicit === 'gm' ? 'gm' : 'character')
    const requestId = uid('req')
    // 引用：本次行动的目标（轻量 refs 落库，完整定义 focus 仅本轮透传给 AI）。
    const refs = [...pendingRefs.value]
    const focus = focusFor(refs)
    pendingRefs.value = []
    // 乐观追加玩家回合：提交到 round_start 事件回来之间不再空窗（P0-4）。
    const optKey = 'opt:' + requestId
    entries.value.push({
      key: optKey, kind: 'round', round: 0, channel, text, pending: true,
      actorName: channel === 'character' ? controlled.value?.name : channel === 'gm' ? '导演' : undefined,
      refs: refs.length ? refs : undefined,
      ts: new Date().toISOString()
    })
    try {
      await submitRound(saveId.value, channel, text, requestId, {
        onEvent: () => { /* 事件经 subscribe sink 推送（#24 时序：先订阅再提交） */ },
        onProgress: p => { phase.value = p.stage; phaseDetail.value = p.detail ?? '' }
      }, refs.length ? refs : undefined, focus.length ? focus : undefined)
      return true
    } catch (err) {
      // 提交失败：撤回乐观条目，避免留下不存在的回合。
      const i = entries.value.findIndex(e => e.key === optKey)
      if (i >= 0) entries.value.splice(i, 1)
      const code = (err as { code?: string })?.code
      const msg = (err as Error)?.message ?? String(err)
      if (code === 'ROUND_IN_PROGRESS') toast('info', '回合仍在进行中，请稍候。')
      else toast('error', msg)
      return false
    } finally {
      sending.value = false
    }
  }

  /** 确认门往返（#17/#24） */
  async function confirm(actionId: string, decision: 'confirm' | 'cancel') {
    if (confirmBusy.value) return
    confirmBusy.value = true
    try {
      const pe = entries.value.find((e): e is Extract<FeedEntry, { kind: 'pending' }> => e.kind === 'pending' && e.actionId === actionId)
      await confirmAction(saveId.value, pe?.round ?? 0, actionId, decision)
    } catch (err) {
      const code = (err as { code?: string })?.code
      toast('warn', code === 'expired' ? '该确认已超时过期，动作已自动取消。' : ((err as Error)?.message ?? '确认失败'))
      // 过期即视为取消，settle 由后续 system/round_end 兜底
    } finally {
      confirmBusy.value = false
    }
  }

  /** 升级执行完成后同步本地投影的版次元信息 */
  function applyUpgrade(rev: number, needs: boolean) {
    const p = projection.value
    if (p) { p.meta.revision = rev; p.meta.needs_upgrade = needs }
    const d = detail.value
    if (d) { d.embedded_revision = rev; d.needs_upgrade = needs }
  }

  /** 切换受控角色（#24 修订：状态类元指令直调结构化端点） */
  async function switchTo(characterId: string) {
    const p = projection.value
    if (!saveId.value || !p) return
    // controlled 存的是存档内寻址键（角色实例 id）；调用方可能传模板 id。
    const key = p.characters[characterId]
      ? characterId
      : Object.keys(p.characters).find(k => {
          const c = p.characters[k]
          return c.template_id === characterId || c.instance_id === characterId
        })
    if (!key || key === controlledId.value) return
    try {
      await switchCharacter(saveId.value, characterId)
      p.controlled = [key]
    } catch (err) { toast('error', (err as Error)?.message ?? '切换角色失败') }
  }

  /** 免确认开关（#24 修订：存档级设置端点）；带模型字段一起提交，避免把本存档模型清掉 */
  async function setAutoConfirm(v: boolean) {
    if (!saveId.value) return
    try {
      const s = await setSaveSettings(saveId.value, {
        auto_confirm: v,
        model_provider_id: saveModel.value?.provider_id,
        model: saveModel.value?.model,
        reasoning_effort: saveModel.value?.reasoning_effort,
        narrative: narrativePrefs.value,
      })
      if (projection.value) projection.value.meta.auto_confirm = s.auto_confirm
      narrativePrefs.value = { ...(s.narrative ?? {}) }
    } catch (err) { toast('error', (err as Error)?.message ?? '设置失败') }
  }

  /** 切换本存档使用的模型（story/character 一起），即时生效并持久化到该存档。 */
  async function setModel(providerId: string, model: string, reasoningEffort?: string) {
    if (!saveId.value) return
    try {
      await setSaveSettings(saveId.value, {
        auto_confirm: autoConfirm.value,
        model_provider_id: providerId,
        model,
        reasoning_effort: reasoningEffort,
        narrative: narrativePrefs.value,
      })
      saveModel.value = { provider_id: providerId, model, reasoning_effort: reasoningEffort }
      toast('ok', '本存档模型已切换为 ' + model)
    } catch (err) { toast('error', (err as Error)?.message ?? '切换模型失败') }
  }

  /**
   * 叙述段玩家偏好（叙事契约 P1）：切换某个 playerEditable 段的开关 / 变体。
   * 偏好写存档设置、只影响之后的回合；不重写命令日志，也不改变已播放的叙事。
   */
  async function setNarrativeOverride(sectionId: string, value: boolean | string) {
    if (!saveId.value) return
    const next = { ...narrativePrefs.value, [sectionId]: value }
    try {
      const s = await setSaveSettings(saveId.value, {
        auto_confirm: autoConfirm.value,
        model_provider_id: saveModel.value?.provider_id,
        model: saveModel.value?.model,
        reasoning_effort: saveModel.value?.reasoning_effort,
        narrative: next,
      })
      narrativePrefs.value = { ...(s.narrative ?? next) }
    } catch (err) { toast('error', (err as Error)?.message ?? '叙述偏好保存失败') }
  }

  /**
   * 重跑本轮：后端归档旧回合、把会话回滚到回合前，再用原输入重跑；
   * 完成后整页重新水合，旧回合条目被新事件替换。
   */
  async function rerunLastRound(editedText?: string): Promise<boolean> {
    if (!saveId.value || !ready.value) return false
    if (busy.value) {
      toast('info', waitingConfirm.value ? '有待确认动作，请先确认或取消。' : '回合结算中，请稍候…')
      return false
    }
    // 日志只存轻量 refs；focus 需要前端重解析成完整定义再透传给 AI。
    const last = [...entries.value].reverse().find(
      (e): e is Extract<FeedEntry, { kind: 'round' }> => e.kind === 'round' && !e.pending
    )
    const refs = last?.refs ?? []
    const focus = focusFor(refs)
    sending.value = true
    try {
      // 带上编辑后的文本：后端用它替换原回合输入，其余（渠道 / 引用）沿用。
      const text = editedText?.trim() || undefined
      await rerunRound(saveId.value, focus.length ? focus : undefined, text)
      await init(saveId.value)
      toast('ok', text ? '已按修改后的话重跑本轮' : '已重跑本轮')
      return true
    } catch (err) {
      const code = (err as { code?: string })?.code
      if (code === 'round_in_progress') toast('info', '回合仍在进行中，请稍候。')
      else toast('error', (err as Error)?.message ?? '重跑失败')
      return false
    } finally {
      sending.value = false
    }
  }

  function teardown() {
    if (unsub) { unsub(); unsub = null }
    stopTicker()
    watermark = 0
    projection.value = null
    ready.value = false
    pendingRefs.value = []
  }

  return {
    saveId, ready, error, projection, detail, entries, phase, phaseDetail, sending, confirmBusy,
    revealPulse, advancePulse, goalTexts, triggerTexts,
    pendingRefs, addRef, removeRef, clearRefs, listRefs, saveModel, narrativePrefs,
    oldestSeq, hasMoreOlder, loadingOlder,
    autoConfirm, sceneTitle, controlledId, controlled, presentChars, allChars, busy, waitingConfirm, canRerun,
    streamingEntry, phaseLabel,
    init, loadOlder, send, rerunLastRound, confirm, switchTo, setAutoConfirm, setModel, setNarrativeOverride, skipEntry, skipCurrent, emitSkip, flushAll, applyUpgrade, teardown
  }
})
