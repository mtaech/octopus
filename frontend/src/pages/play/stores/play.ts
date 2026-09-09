// ============================================================
// 游玩页 store（#18 ② 全量共享 store + #17 事件消费）
// 职责：世界投影（本地镜像，经 hydrate + state_changes delta 增量更新）、
// 演出流条目数组、管线阶段、打字机调度、回合提交与确认门编排。
// ============================================================
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import type {
  WorldProjection, PlayEvent, PhaseStage, SaveDetail, CheckResultPayload, StateDelta, CharacterInstance
} from '@/types'
import { hydrate, subscribe, submitRound, confirmAction, getSave, getHistory, switchCharacter, setSaveSettings, toast } from '@/api'
import { uid } from '@/types'

export type ContentType = 'narrate' | 'dialogue' | 'emote'

/** 演出流条目（渲染器数据源；kind 判别联合） */
export type FeedEntry =
  | { key: string; kind: 'scene'; round: number; sceneId: string; title: string; description?: string }
  | { key: string; kind: 'content'; round: number; type: ContentType; text: string; actorId?: string; actorName?: string; emotion?: string; reveal: number; done: boolean }
  | { key: string; kind: 'round'; round: number; channel: 'character' | 'meta'; text: string; actorName?: string; actorId?: string }
  | { key: string; kind: 'system'; round: number; level: 'info' | 'warn' | 'error'; code?: string; text: string }
  | { key: string; kind: 'check'; round: number; payload: CheckResultPayload }
  | { key: string; kind: 'pending'; round: number; state: 'pending' | 'confirmed' | 'cancelled'; actionId: string; description: string; impact?: string; actorName: string }
  | { key: string; kind: 'resolution'; round: number; status: 'ok' | 'rejected'; text?: string }
  | { key: string; kind: 'progress'; round: number; tone: 'goal' | 'beat'; label: string }

const TYPE_SPEED_MS = 20

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
  const beatTexts = ref<Record<string, string>>({})

  let unsub: (() => void) | null = null
  let keySeq = 0
  let ticker: ReturnType<typeof setInterval> | null = null
  let watermark = 0

  // ---------- 派生 ----------
  const autoConfirm = computed<boolean>(() => projection.value?.meta.auto_confirm ?? false)
  const sceneTitle = computed(() => projection.value?.scene_title ?? '')
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
      } else if (d.domain === 'beat') {
        if (d.op === 'remove') delete p.progress.beats[d.entity_id]
        else p.progress.beats[d.entity_id] = !!d.value
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
  function pushContent(type: ContentType, text: string, actor: { id: string; name: string } | null | undefined, round: number, emotion?: string, instant = false) {
    const entry: Extract<FeedEntry, { kind: 'content' }> = {
      key: nextKey(), kind: 'content', round, type, text,
      actorId: actor?.id, actorName: actor?.name, emotion,
      reveal: instant ? text.length : 0, done: instant
    }
    entries.value.push(entry)
    if (!instant) ensureTicker()
  }

  function settlePendings(round: number, status: 'ok' | 'rejected') {
    entries.value.forEach(e => {
      if (e.kind === 'pending' && e.round === round && e.state === 'pending') {
        e.state = status === 'ok' ? 'confirmed' : 'cancelled'
      }
    })
  }

  function progressLines(deltas: StateDelta[]): FeedEntry[] {
    const out: FeedEntry[] = []
    for (const d of deltas) {
      if (d.domain === 'goal' && d.value) out.push({ key: nextKey(), kind: 'progress', round: 0, tone: 'goal', label: goalTexts.value[d.entity_id] ?? d.entity_id })
      else if (d.domain === 'beat' && d.value) out.push({ key: nextKey(), kind: 'progress', round: 0, tone: 'beat', label: beatTexts.value[d.entity_id] ?? d.entity_id })
    }
    return out
  }

  function onEvent(e: PlayEvent) {
    if (e.seq <= watermark) return // 丢弃水合前迟到事件（#17 seq 水位线）
    switch (e.type) {
      case 'round_start': {
        flushAll()
        entries.value.push({
          key: nextKey(), kind: 'round', round: e.round,
          channel: e.payload.input.channel, text: e.payload.input.text,
          actorName: e.actor?.name, actorId: e.actor?.id
        })
        break
      }
      case 'scene': {
        const p = projection.value
        if (p) { p.scene_id = e.payload.scene_id; p.scene_title = e.payload.title }
        entries.value.push({ key: nextKey(), kind: 'scene', round: e.round, sceneId: e.payload.scene_id, title: e.payload.title, description: e.payload.description })
        break
      }
      case 'narrate': pushContent('narrate', e.payload.content, e.actor, e.round); break
      case 'dialogue': pushContent('dialogue', e.payload.content, e.actor, e.round); break
      case 'emote': pushContent('emote', e.payload.content, e.actor, e.round, e.payload.emotion); break
      case 'pending': {
        if (phase.value !== 'waiting_confirm') phase.value = 'waiting_confirm'
        entries.value.push({
          key: nextKey(), kind: 'pending', round: e.round, state: 'pending',
          actionId: e.payload.action_id, description: e.payload.description,
          impact: e.payload.impact, actorName: e.payload.actor.name
        })
        break
      }
      case 'check_result': {
        entries.value.push({ key: nextKey(), kind: 'check', round: e.round, payload: e.payload })
        break
      }
      case 'resolution': {
        settlePendings(e.round, e.payload.status)
        applyDeltas(e.payload.state_changes)
        const lines = progressLines(e.payload.state_changes)
        if (lines.length) entries.value.push(...lines)
        if (e.payload.status === 'ok' && e.payload.narrative) {
          entries.value.push({ key: nextKey(), kind: 'resolution', round: e.round, status: 'ok', text: e.payload.narrative })
        } else if (e.payload.status === 'rejected') {
          entries.value.push({ key: nextKey(), kind: 'resolution', round: e.round, status: 'rejected', text: e.payload.narrative ?? (e.payload.rejection_code ? '意图被驳回（' + e.payload.rejection_code + '）' : '意图被驳回') })
        }
        break
      }
      case 'state_update': {
        applyDeltas(e.payload.changes)
        const lines = progressLines(e.payload.changes)
        if (lines.length) entries.value.push(...lines)
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
        entries.value.push({ key: nextKey(), kind: 'system', round: e.round, level: e.payload.level, code: e.payload.code, text: e.payload.text })
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
    try {
      const [p, d, hist] = await Promise.all([hydrate(id), getSave(id), getHistory(id)])
      projection.value = structuredClone(p)
      detail.value = d
      // 骨架文案索引（goal/beat 触发时给出友好名）
      const g: Record<string, string> = {}
      const b: Record<string, string> = {}
      d.storybook.skeleton.forEach(ch => ch.scenes.forEach(sc => {
        sc.goals.forEach(go => { g[go.id] = go.text })
        sc.beats.forEach(be => { b[be.id] = be.title })
      }))
      goalTexts.value = g
      beatTexts.value = b
      // 回放叙事历史（#17/#24 修订：进页 / 刷新 / 重连 / 换模板不丢故事）
      watermark = 0
      hist.events.forEach(e => onEvent(e))
      watermark = p.seq
      if (hist.events.length === 0) {
        entries.value.push({ key: nextKey(), kind: 'scene', round: 0, sceneId: p.scene_id, title: p.scene_title })
        const premise = d.storybook.world.premise
        if (premise) pushContent('narrate', premise, null, 0, undefined, true)
        entries.value.push({ key: nextKey(), kind: 'system', round: 0, level: 'info', text: '直接输入想做的事（如「打听怪梦的传闻」）；以 / 开头发送元指令（/存档 /免确认 /帮助）。' })
      }
      ready.value = true
      // 先订阅再允许回合：保证 submitRound 事件能送达本 sink（#24 时序）
      unsub = subscribe(id, onEvent)
    } catch (err) {
      error.value = (err as Error)?.message ?? String(err)
      toast('error', '加载存档失败：' + error.value)
    }
  }

  /** 提交玩家回合：/ 前缀 → 元指令通道，其余 → 角色通道 */
  async function send(raw: string) {
    const text = raw.trim()
    if (!text) return
    if (busy.value) { toast('info', waitingConfirm.value ? '有待确认动作，请先确认或取消。' : '回合结算中，请稍候…'); return }
    if (!ready.value || !saveId.value) return
    sending.value = true
    const channel = text.startsWith('/') ? 'meta' : 'character'
    const requestId = uid('req')
    try {
      await submitRound(saveId.value, channel, text, requestId, {
        onEvent: () => { /* 事件经 subscribe sink 推送（#24 时序：先订阅再提交） */ },
        onProgress: p => { phase.value = p.stage; phaseDetail.value = p.detail ?? '' }
      })
    } catch (err) {
      const code = (err as { code?: string })?.code
      const msg = (err as Error)?.message ?? String(err)
      if (code === 'ROUND_IN_PROGRESS') toast('info', '回合仍在进行中，请稍候。')
      else toast('error', msg)
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
    if (!saveId.value || characterId === controlledId.value) return
    try {
      await switchCharacter(saveId.value, characterId)
      if (projection.value) projection.value.controlled = [characterId]
    } catch (err) { toast('error', (err as Error)?.message ?? '切换角色失败') }
  }

  /** 免确认开关（#24 修订：存档级设置端点） */
  async function setAutoConfirm(v: boolean) {
    if (!saveId.value) return
    try {
      const s = await setSaveSettings(saveId.value, { auto_confirm: v })
      if (projection.value) projection.value.meta.auto_confirm = s.auto_confirm
    } catch (err) { toast('error', (err as Error)?.message ?? '设置失败') }
  }

  function teardown() {
    if (unsub) { unsub(); unsub = null }
    stopTicker()
    watermark = 0
    projection.value = null
    ready.value = false
  }

  return {
    saveId, ready, error, projection, detail, entries, phase, phaseDetail, sending, confirmBusy,
    revealPulse, advancePulse, goalTexts, beatTexts,
    autoConfirm, sceneTitle, controlledId, controlled, presentChars, allChars, busy, waitingConfirm,
    streamingEntry, phaseLabel,
    init, send, confirm, switchTo, setAutoConfirm, skipEntry, skipCurrent, emitSkip, flushAll, applyUpgrade, teardown
  }
})
