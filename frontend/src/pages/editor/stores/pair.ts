// ============================================================
// pair store（#22 ③ / #23 ④）—— C AI 结对会话
// 会话为临时态：messages + suggestions「待审查」不入草稿；
// 采纳 = 调 editor store 落稿并校验刷新，不留来源标记。
// ============================================================
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { pairChat } from '@/api'
import type { PairSuggestion, Storybook } from '@/types'

export interface PairMessage {
  role: 'user' | 'assistant'
  content: string
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

export const usePairStore = defineStore('editorPair', () => {
  const messages = ref<PairMessage[]>([])
  const suggestions = ref<PendingSuggestion[]>([])
  const sending = ref(false)
  const input = ref('')
  const bannerDismissed = ref(false)

  /** 新建书默认进 C：首屏轻引导（可关，不强制） */
  function showWelcome(): boolean { return !bannerDismissed.value && messages.value.length === 0 }

  function push(msg: PairMessage): void { messages.value.push(msg) }

  function clear(): void { messages.value = []; suggestions.value = [] }

  /** 发送一轮：user 消息入流 → pairChat；建议进待审查区 */
  async function send(draft: Storybook | null): Promise<boolean> {
    const t = input.value.trim()
    if (!t || sending.value) return false
    sending.value = true
    push({ role: 'user', content: t })
    input.value = ''
    try {
      // 历史裁剪：只带最近 12 条（#23 ④：窗口裁剪为客户端职责）
      const history = messages.value.slice(-12).map(m => ({ role: m.role, content: m.content }))
      const res = await pairChat(history)
      const deltas = Array.isArray(res.deltas) ? res.deltas : []
      const text = deltas.join('') || (res.suggestions.length ? '我按这个方向拟了几条建议，可以先看看合不合口味。' : '这个方向我还没太想好，换个说法再聊聊？')
      push({ role: 'assistant', content: text })
      const fresh = (res.suggestions ?? []).filter(s => !sampleConflicts(s, draft))
      if (res.suggestions.length > 0 && fresh.length === 0) {
        push({ role: 'assistant', content: '（这些实体你似乎已经写过或采纳过了，换个方向聊聊？）' })
      }
      suggestions.value.push(...fresh.map(s => ({ ...s })))
      return true
    } catch (e) {
      push({ role: 'assistant', content: '结对服务暂时不可用：' + ((e as Error)?.message ?? String(e)) })
      return false
    } finally {
      sending.value = false
    }
  }

  /** 采纳一条：editor 落稿 + 校验刷新（applyFn 由 EditorPage 注入） */
  function adopt(s: PendingSuggestion, applyFn: (sug: PairSuggestion) => { ok: boolean; message: string }): boolean {
    if (s.applying) return false
    s.applying = true
    try {
      const r = applyFn(s)
      if (!r.ok) return false
      const i = suggestions.value.indexOf(s)
      if (i >= 0) suggestions.value.splice(i, 1)
      return true
    } finally {
      s.applying = false
    }
  }

  return { messages, suggestions, sending, input, bannerDismissed, showWelcome, push, clear, send, adopt }
})
