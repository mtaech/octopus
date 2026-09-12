// ============================================================
// pair store（#22 ③ / #23 ④）—— C AI 结对会话
// 会话为临时态：messages + suggestions「待审查」不入草稿；
// 采纳 = 调 editor store 落稿并校验刷新，不留来源标记。
// ============================================================
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import {
  pairChat,
  listPairThreads,
  createPairThread,
  renamePairThread,
  deletePairThread,
  getPairMessages,
  appendPairMessages,
  setPairThreadPending,
} from '@/api'
import type { EntityRef, PairAttachment, PairFocusEntity, PairHistoryMessage, PairToolCall, PairMessageRecord, PairThreadRecord, PairUsage } from '@/api'
import { PAIR_TOOLS } from './pair-tools'
import { estimateTokens } from '@/lib/tokens'
import { buildStorybookContext } from '@/lib/pair-context'
import type { PairSuggestion, Storybook } from '@/types'

/** 一次工具调用的展示轨迹 */
export interface PairToolTrace {
  name: string
  label: string
  ok: boolean
}

/** 由 EditorPage 注入：让结对对话能直接执行工具并支持撤销 */
export interface PairToolExecutor {
  /** 开始一轮暂存：返回撤销栈 mark */
  beginTurn(): number
  /** 当前有效草稿（暂存态优先）：供下一轮上下文带上本轮已暂存的实体 */
  currentDraft(): Storybook | null
  /** 把引用解析成带完整定义的目标实体 */
  focus(refs: EntityRef[]): PairFocusEntity[]
  /** 在暂存草稿上执行一次工具调用，返回结果供模型链式继续 */
  runTool(name: string, args: Record<string, unknown>): { ok: boolean; message: string; data?: unknown }
  /** 本轮结束：交出暂存的拟改动（尚未写入草稿） */
  finishTurn(): PairSuggestion[]
  /** 放弃本轮暂存 */
  discardTurn(): void
  /** 把选中的拟改动合并进草稿（整批一次撤销快照） */
  apply(suggestions: PairSuggestion[]): { appliedIds: string[]; failed: string[] }
}

export interface PairMessage {
  role: 'user' | 'assistant'
  content: string
  model?: string
  isError?: boolean
  /** assistant 消息上：本轮工具轨迹（暂存态为「待批准」） */
  tools?: PairToolTrace[]
  /** 仅 assistant：本轮思考（reasoning_content）；可折叠展示 */
  reasoning?: string
  /** 仅 assistant：本轮 AI 改动的审查状态 */
  turnState?: 'staged' | 'applied' | 'discarded'
  /** 用户消息：显式引用的故事书实体（引用 chips） */
  refs?: EntityRef[]
  /** 用户消息：随消息一起交给 AI 的文件附件（读出的文本） */
  attachments?: PairAttachment[]
}

export interface PendingSuggestion extends PairSuggestion {
  /** 采纳中（调 applySuggestion） */
  applying?: boolean
}

/** mock 建议实体是否与当前草稿重复（演示：建议区不堆同名人/地点） */
function sampleConflicts(s: PairSuggestion, draft: Storybook | null): boolean {
  if (!draft) return false
  if (s.target.kind === 'location') return draft.world.locations.some(l => l.name === (s.patch as { name?: string }).name)
  if (s.target.kind === 'character') return draft.characters.some(c => c.name === (s.patch as { name?: string }).name)
  return false
}

export const HIDE_EDITOR_INTRO_KEY = 'octopus-hide-editor-intro'

export function isEditorIntroHidden(): boolean {
  try {
    if (typeof localStorage !== 'undefined') {
      return localStorage.getItem(HIDE_EDITOR_INTRO_KEY) === 'true'
    }
  } catch {
    // noop
  }
  return false
}

export function setEditorIntroHidden(hidden: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') {
      if (hidden) {
        localStorage.setItem(HIDE_EDITOR_INTRO_KEY, 'true')
      } else {
        localStorage.removeItem(HIDE_EDITOR_INTRO_KEY)
      }
    }
  } catch {
    // noop
  }
}

export const usePairStore = defineStore('editorPair', () => {
  const messages = ref<PairMessage[]>([])
  const suggestions = ref<PendingSuggestion[]>([])
  const sending = ref(false)
  const input = ref('')
  const bannerDismissed = ref(false)
  const selectedProviderId = ref('')
  const selectedModel = ref('')
  /** 最近一轮开始前的撤销栈深度（null = 无可撤销轮次） */
  const lastTurnMark = ref<number | null>(null)
  /** 当前「待审查改动」条数 */
  const turnToolCount = ref(0)
  /** 是否仍有待批准改动：直接由待审列表推导——**落库后刷新依然成立** */
  const stagedTurn = computed(() => suggestions.value.length > 0)
  /** 最近一次暂存的 assistant 消息（用于回写审查状态；仅内存） */
  let stagedMessage: PairMessage | null = null
  /** 发送时注入的执行器，供批准 / 放弃复用 */
  let activeExecutor: PairToolExecutor | null = null
  /** 输入框待发送的实体引用（引用 chips） */
  const pendingRefs = ref<EntityRef[]>([])
  /** 当前故事书 id（空 = 未绑定） */
  const threadStorybookId = ref('')
  /** 当前故事书下的会话线程列表（按最后更新倒序） */
  const threads = ref<PairThreadRecord[]>([])
  /** 当前激活的会话线程 id */
  const activeThreadId = ref('')
  const activeThread = computed(() => threads.value.find(t => t.id === activeThreadId.value) ?? null)
  /**
   * 上一轮统计：后端返回真实 usage 时以真实值为准（measured=true），
   * 否则退回前端估算。附结束原因 / 思考字数，供空返回时自证清白。
   */
  const lastTurnStats = ref<{
    outTokens: number
    ms: number
    tps: number
    /** true = 后端真实 usage；false = 前端估算 */
    measured: boolean
    inputTokens?: number
    cachedTokens?: number
    totalTokens?: number
    reasoningChars?: number
    finishReason?: string
  } | null>(null)

  function setModel(providerId: string, model: string) {
    selectedProviderId.value = providerId
    selectedModel.value = model
    try {
      localStorage.setItem('octopus-pair-model', JSON.stringify({ provider_id: providerId, model }))
    } catch {
      // noop
    }
  }

  function initModel(defaultProviderId = '', defaultModel = '') {
    try {
      const saved = localStorage.getItem('octopus-pair-model')
      if (saved) {
        const parsed = JSON.parse(saved)
        if (parsed.provider_id && parsed.model) {
          selectedProviderId.value = parsed.provider_id
          selectedModel.value = parsed.model
          return
        }
      }
    } catch {
      // noop
    }
    if (defaultProviderId) selectedProviderId.value = defaultProviderId
    if (defaultModel) selectedModel.value = defaultModel
  }

  /** 新建书默认进 C：首屏轻引导（可关，支持持久化不再提示） */
  function showWelcome(): boolean {
    if (isEditorIntroHidden()) return false
    return !bannerDismissed.value && messages.value.length === 0
  }

  function dismissBanner(permanent = false): void {
    bannerDismissed.value = true
    if (permanent) {
      setEditorIntroHidden(true)
    }
  }

  function push(msg: PairMessage): void { messages.value.push(msg) }

  /** 仅清空本地会话态（不触碰后端）。 */
  function resetLocal(): void {
    messages.value = []
    suggestions.value = []
    input.value = ''
    pendingRefs.value = []
    lastTurnMark.value = null
    turnToolCount.value = 0
    // 上轮用量属于单条会话：切走 / 重进后不能把别的会话的数字留在状态行
    lastTurnStats.value = null
  }

  /** 添加一条实体引用（按 kind + id 去重）。 */
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

  /** 进入编辑器时先本地清空；真正的历史恢复由 openStorybook 负责。 */
  function clear(): void {
    threadStorybookId.value = ''
    activeThreadId.value = ''
    threads.value = []
    resetLocal()
  }

  /** 从线程记录恢复待审查改动（落库值，刷新 / 切会话都据此恢复）。 */
  function restorePending(threadId: string): void {
    const raw = threads.value.find(t => t.id === threadId)?.pending_suggestions
    suggestions.value = Array.isArray(raw) ? (raw as PendingSuggestion[]) : []
    turnToolCount.value = suggestions.value.length
  }

  let pendingTimer: ReturnType<typeof setTimeout> | null = null
  /** 把当前待审查改动落库到线程：刷新 / 切会话后都能恢复，应用或放弃后同步收敛。 */
  function persistPending(): void {
    const tid = activeThreadId.value
    if (!tid) return
    const payload = suggestions.value.map(s => ({
      id: s.id,
      action: s.action,
      target: s.target,
      patch: s.patch,
      label: s.label,
      summary: s.summary,
      details: s.details ?? null,
    }))
    const t = threads.value.find(x => x.id === tid)
    if (t) t.pending_suggestions = payload.length ? payload : undefined
    turnToolCount.value = payload.length
    if (pendingTimer) clearTimeout(pendingTimer)
    pendingTimer = setTimeout(() => {
      pendingTimer = null
      void setPairThreadPending(tid, payload.length ? payload : null).catch(() => { /* noop */ })
    }, 150)
  }

  function toDisplay(rows: PairMessageRecord[]): PairMessage[] {
    return rows
      .filter(m => m.role === 'user' || m.role === 'assistant')
      .map(m => ({
        role: m.role as 'user' | 'assistant',
        content: m.content,
        reasoning: m.reasoning ?? undefined,
        model: m.model ?? undefined,
        isError: m.is_error,
        tools: Array.isArray(m.tools) ? (m.tools as PairToolTrace[]) : undefined,
        refs: m.refs ?? undefined,
        attachments: m.attachments ?? undefined,
      }))
  }

  let threadToken = 0

  /** 切到某条会话线程：换掉消息与建议，加载该线程历史。 */
  async function switchThread(threadId: string): Promise<void> {
    if (threadId === activeThreadId.value) return
    const token = ++threadToken
    activeThreadId.value = threadId
    resetLocal()
    if (!threadId) return
    restorePending(threadId)
    try {
      const rows = await getPairMessages(threadId)
      if (token !== threadToken) return
      messages.value = toDisplay(rows)
    } catch {
      // 恢复失败不阻塞编辑，对话从空开始。
    }
  }

  /** 打开某本故事书：加载会话列表，激活最近一条；一条都没有就新建。 */
  async function openStorybook(storybookId: string): Promise<void> {
    const token = ++threadToken
    threadStorybookId.value = storybookId
    activeThreadId.value = ''
    threads.value = []
    resetLocal()
    if (!storybookId) return
    try {
      let list = await listPairThreads(storybookId)
      if (token !== threadToken) return
      if (!list.length) {
        const created = await createPairThread(storybookId)
        if (token !== threadToken) return
        list = [created]
      }
      threads.value = list
      await switchThread(list[0].id)
    } catch {
      // 恢复失败不阻塞编辑，对话从空开始。
    }
  }

  /** 新建一条会话线程并激活。 */
  async function createThread(): Promise<void> {
    const sb = threadStorybookId.value
    if (!sb) return
    try {
      const created = await createPairThread(sb)
      threads.value = [created, ...threads.value]
      activeThreadId.value = ''
      await switchThread(created.id)
    } catch {
      // noop
    }
  }

  /** 重命名会话线程。 */
  async function renameThread(threadId: string, title: string): Promise<boolean> {
    const next = title.trim()
    if (!next) return false
    try {
      const updated = await renamePairThread(threadId, next)
      const i = threads.value.findIndex(t => t.id === threadId)
      if (i >= 0) threads.value[i] = updated
      return true
    } catch {
      return false
    }
  }

  /** 删除会话线程。删当前线程时自动切到剩余最近一条，没有则新建。 */
  async function deleteThread(threadId: string): Promise<boolean> {
    try {
      await deletePairThread(threadId)
    } catch {
      return false
    }
    const wasActive = activeThreadId.value === threadId
    threads.value = threads.value.filter(t => t.id !== threadId)
    if (!wasActive) return true
    activeThreadId.value = ''
    resetLocal()
    const next = threads.value[0]
    if (next) await switchThread(next.id)
    else await createThread()
    return true
  }

  /** 把消息追加进当前会话线程（失败只记录，不回滚已展示内容）。 */
  async function persist(msgs: PairMessage[]): Promise<void> {
    const id = activeThreadId.value
    if (!id || !msgs.length) return
    try {
      await appendPairMessages(id, msgs.map(m => ({
        seq: 0,
        role: m.role,
        content: m.content,
        reasoning: m.reasoning ?? null,
        model: m.model ?? null,
        is_error: !!m.isError,
        tools: m.tools ?? null,
        refs: m.refs && m.refs.length ? m.refs : null,
        attachments: m.attachments && m.attachments.length ? m.attachments : null,
      })))
      // 本地同步消息数与标题（标题由后端按首条用户消息自动派生，这里乐观更新）。
      const t = threads.value.find(x => x.id === id)
      if (t) {
        t.message_count += msgs.length
        if (t.title === '新会话' || t.title.startsWith('对话 ')) {
          const firstUser = msgs.find(m => m.role === 'user')
          if (firstUser) {
            const line = (firstUser.content.split('\n').find(l => l.trim()) ?? '').trim()
            if (line) t.title = line.length > 20 ? line.slice(0, 20) + '…' : line
          }
        }
        t.updated_at = new Date().toISOString()
      }
    } catch {
      // noop：后端会记日志，UI 不回滚
    }
  }

  /** 发送一轮：user 消息入流 → function calling agent 循环（工具直接落稿，可撤销） */
  async function send(draft: Storybook | null, executor?: PairToolExecutor, attachments?: PairAttachment[]): Promise<boolean> {
    const t = input.value.trim()
    if (!t || sending.value) return false
    sending.value = true
    activeExecutor = executor ?? activeExecutor
    // 引用：本轮的显式目标（chips → 结构化 refs + 完整定义 focus）
    const refs = [...pendingRefs.value]
    pendingRefs.value = []
    const focus = executor && refs.length ? executor.focus(refs) : []
    const userMsg: PairMessage = {
      role: 'user',
      content: t,
      refs: refs.length ? refs : undefined,
      attachments: attachments && attachments.length ? attachments : undefined,
    }
    push(userMsg)
    input.value = ''
    void persist([userMsg])

    const assistantMsg = ref<PairMessage>({
      role: 'assistant',
      content: '',
      model: selectedModel.value || undefined,
      tools: [],
    })
    messages.value.push(assistantMsg.value)

    try {
      // 普通上下文处理：带上**完整**历史，不再有固定条数窗口
      const history: PairHistoryMessage[] = messages.value
        .slice(0, -1) // 排除当前占位 assistant 消息
        .map(m => ({ role: m.role, content: m.content, attachments: m.attachments && m.attachments.length ? m.attachments : undefined }))

      // 提取草稿上下文（带 id：供模型引用既有实体做 update / delete 与建关系）
      // 用闭包而非快照：同一轮内工具改动落在暂存草稿，下一轮上下文要能带上新实体 id。
      const contextDraft = (): Storybook | null => executor?.currentDraft?.() ?? draft
      // 与侧栏 token 计量共用同一份口径（@/lib/pair-context）
      const buildSbContext = (): Record<string, unknown> | undefined => buildStorybookContext(contextDraft())

      // 多步 agent 循环：模型调工具 → **只改暂存草稿** → 结果回灌 → 继续
      const t0 = Date.now()
      const working: PairHistoryMessage[] = history.map(m => ({ role: m.role, content: m.content, attachments: m.attachments && m.attachments.length ? m.attachments : undefined }))
      const traces: PairToolTrace[] = []
      const mark = executor ? executor.beginTurn() : 0
      const MAX_ROUNDS = 6
      let finalText = ''
      // 多步循环里收集最后一轮的真实用量与诊断信息（供状态行 / 空返回提示）
      let lastUsage: PairUsage | undefined
      let lastFinishReason: string | undefined
      let lastReasoningChars = 0
      let lastCounts: Record<string, number> | undefined
      let streamedText = ''
      let totalReasoning = ''

      for (let round = 0; round < MAX_ROUNDS; round++) {
        let roundReasoning = ''
        const res = await pairChat(working, {
          provider_id: selectedProviderId.value || undefined,
          model: selectedModel.value || undefined,
          storybook: buildSbContext(),
          tools: PAIR_TOOLS,
          focus: focus.length ? focus : undefined,
          onReasoning: (text: string, replace?: boolean) => {
            roundReasoning = replace ? text : roundReasoning + text
            assistantMsg.value.reasoning = totalReasoning + roundReasoning
          },
          onDelta: (delta: string) => {
            finalText += delta
            assistantMsg.value.content = finalText
          },
          onSuggestion: (sug: PairSuggestion) => {
            if (!sampleConflicts(sug, draft) && !suggestions.value.some(s => s.id === sug.id || (s.label === sug.label && s.summary === sug.summary))) {
              suggestions.value.push({ ...sug })
            }
          },
        })

        totalReasoning += roundReasoning
        if (roundReasoning) assistantMsg.value.reasoning = totalReasoning
        if (res.text && !finalText.trim()) {
          finalText = res.text
          assistantMsg.value.content = finalText
        }
        lastUsage = res.usage
        lastFinishReason = res.finishReason
        lastReasoningChars += res.reasoningChars ?? 0
        if (res.counts) lastCounts = res.counts
        if (res.text) streamedText += res.text

        const calls: PairToolCall[] = res.toolCalls ?? []
        if (!calls.length) {
          for (const sug of res.suggestions) {
            if (!sampleConflicts(sug, draft) && !suggestions.value.some(s => s.id === sug.id || (s.label === sug.label && s.summary === sug.summary))) {
              suggestions.value.push({ ...sug })
            }
          }
          break
        }

        // 把本轮 assistant 的 tool_calls 记入上下文，再逐个执行并回灌结果
        working.push({
          role: 'assistant',
          content: res.text || '',
          tool_calls: calls.map(c => ({ id: c.id, type: 'function', function: { name: c.name, arguments: c.arguments } })),
        })

        for (const c of calls) {
          let args: Record<string, unknown> = {}
          if (c.arguments && c.arguments.trim()) {
            try { args = JSON.parse(c.arguments) } catch { args = {} }
          }
          let r: { ok: boolean; message: string; data?: unknown }
          if (executor) {
            try { r = executor.runTool(c.name, args) }
            catch (err) { r = { ok: false, message: '工具执行异常：' + ((err as Error)?.message ?? String(err)) } }
          } else {
            r = { ok: false, message: '当前未接入草稿编辑器' }
          }
          traces.push({ name: c.name, label: r.message, ok: r.ok })
          working.push({
            role: 'tool',
            tool_call_id: c.id,
            content: JSON.stringify({ ok: r.ok, message: r.message, ...(r.data && typeof r.data === 'object' ? r.data : {}) }),
          })
        }

        assistantMsg.value.tools = [...traces]
      }

      // 本轮工具改动全部落在暂存草稿：交出来作为「待审查」，真草稿零污染。
      const staged = executor ? executor.finishTurn() : []
      if (staged.length) {
        for (const s of staged) {
          if (!suggestions.value.some(x => x.id === s.id)) suggestions.value.push(s)
        }
        assistantMsg.value.turnState = 'staged'
        stagedMessage = assistantMsg.value
      }

      lastTurnMark.value = mark
      assistantMsg.value.tools = traces
      // 待审查改动落库：刷新 / 切会话后仍能恢复（这是「右侧卡片会丢」的根治）。
      persistPending()

      if (!assistantMsg.value.content.trim()) {
        if (staged.length) {
          assistantMsg.value.content = '我拟好了 ' + staged.length + ' 处改动，请在右侧审查后批准写入草稿。'
        } else if (suggestions.value.length) {
          assistantMsg.value.content = '我按这个方向拟了建议，可以先在右侧审查区查看。'
        } else {
          // 模型这次什么都没返回（空流 / 限流 / 网络抖动）。绝不能谎报「拟了建议」——
          // 那会让用户对着空的审查区找卡片。附上诊断，让「为什么」有据可查。
          const diag: string[] = []
          if (lastFinishReason) diag.push('结束原因 ' + lastFinishReason)
          if (lastUsage?.output_tokens) diag.push('输出 ' + lastUsage.output_tokens + ' tok')
          if (lastReasoningChars) diag.push('思考 ' + lastReasoningChars + ' 字')
          const toolFrames = lastCounts?.tool_call_delta ?? 0
          if (toolFrames) diag.push('工具增量 ' + toolFrames + ' 帧')
          let hint = ''
          if (lastFinishReason === 'length') {
            hint = '这一轮的长度被用完了（常见于思考过程过长），把要求拆小一点再试。'
          }
          assistantMsg.value.content =
            '模型这次没有返回任何内容，请重试一次，或把要求说得更具体些。' +
            (hint ? ' ' + hint : '') +
            (diag.length ? '（诊断：' + diag.join('，') + '）' : '')
        }
      }

      // 上一轮出字统计：优先用后端真实 usage，缺省才退回估算
      const ms = Date.now() - t0
      const measuredOut = lastUsage?.output_tokens
      const hasMeasured = typeof measuredOut === 'number' && measuredOut > 0
      const outTokens = hasMeasured ? (measuredOut as number) : estimateTokens(streamedText || finalText)
      lastTurnStats.value = {
        outTokens,
        ms,
        tps: ms > 0 ? Math.round(outTokens / (ms / 1000)) : 0,
        measured: hasMeasured,
        inputTokens: lastUsage?.input_tokens,
        cachedTokens: lastUsage?.cached_input_tokens,
        totalTokens: lastUsage?.total_tokens,
        reasoningChars: lastReasoningChars || undefined,
        finishReason: lastFinishReason,
      }

      await persist([assistantMsg.value])
      return true
    } catch (e) {
      const msg = (e as Error)?.message ?? String(e)
      executor?.discardTurn()
      assistantMsg.value.content = '结对服务暂时不可用：' + msg
      assistantMsg.value.isError = true
      await persist([assistantMsg.value])
      return false
    } finally {
      sending.value = false
    }
  }

  /** 应用选中的待审查改动（整批一次撤销快照）；成功后移除对应卡片。 */
  function applySelected(ids: string[]): { applied: number; failed: string[] } {
    const ex = activeExecutor
    const list = suggestions.value.filter(s => ids.includes(s.id))
    if (!ex || !list.length) return { applied: 0, failed: ['当前未接入草稿编辑器'] }
    const r = ex.apply(list)
    if (r.appliedIds.length) {
      const done = new Set(r.appliedIds)
      suggestions.value = suggestions.value.filter(s => !done.has(s.id))
      if (!suggestions.value.length && stagedMessage) stagedMessage.turnState = 'applied'
    }
    persistPending()
    return { applied: r.appliedIds.length, failed: r.failed }
  }

  /** 弃用单条：从待审查列表移除，草稿不变。 */
  function discardOne(id: string): void {
    suggestions.value = suggestions.value.filter(s => s.id !== id)
    if (!suggestions.value.length && stagedMessage) stagedMessage.turnState = 'discarded'
    persistPending()
  }

  /** 放弃全部待审查改动（含本轮暂存），草稿保持不变。 */
  function discardPending(): void {
    activeExecutor?.discardTurn()
    if (suggestions.value.length && stagedMessage) stagedMessage.turnState = 'discarded'
    suggestions.value = []
    persistPending()
  }

  /** 应用单张卡片。 */
  function adopt(s: PendingSuggestion): boolean {
    if (s.applying) return false
    s.applying = true
    try {
      const r = applySelected([s.id])
      return r.applied > 0
    } finally {
      s.applying = false
    }
  }

  return {
    messages, suggestions, sending, input,
    bannerDismissed, showWelcome, dismissBanner,
    selectedProviderId, selectedModel, setModel, initModel,
    lastTurnMark, turnToolCount, stagedTurn, threadStorybookId,
    threads, activeThreadId, activeThread,
    lastTurnStats,
    push, clear, openStorybook, switchThread, createThread, renameThread, deleteThread,
    pendingRefs, addRef, removeRef, clearRefs,
    send, adopt, applySelected, discardOne, discardPending
  }
})
