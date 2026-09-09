<script setup lang="ts">
// PairC —— C AI 结对范式（#07 反馈修订：无头绪启发式编辑的主范式）
// 会话流 + 建议卡（待审查区，不入草稿）+ 采纳 = editor store 落稿（可多选采纳）
import { computed, nextTick, ref, watch } from 'vue'
import { useEditorStore } from '../../stores/editor'
import { usePairStore, type PendingSuggestion } from '../../stores/pair'
import type { PairSuggestion } from '@/types'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { IconSend, IconSparkles, IconUser, IconCheck, IconLoader2, IconListCheck } from '@tabler/icons-vue'

const editor = useEditorStore()
const pair = usePairStore()

const chatEl = ref<HTMLElement | null>(null)
const sendBtn = ref<HTMLButtonElement | null>(null)

// 自动滚到底
watch(() => pair.messages.length, async () => {
  await nextTick()
  const el = chatEl.value
  if (el) el.scrollTop = el.scrollHeight
})

/** 发送 */
async function onSend(): Promise<void> {
  await pair.send(editor.draft)
}
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void onSend()
  }
}

/** 采纳：editor store 落稿 + 校验刷新；成功 toast */
function adopt(s: PendingSuggestion): void {
  const ok = pair.adopt(s, (sug: PairSuggestion) => editor.applySuggestion(sug))
  if (!ok) {
    import('@/api').then(({ toast }) => toast('warn', '采纳失败：建议与草稿状态不匹配'))
    return
  }
  import('@/api').then(({ toast }) => toast('ok', '已采纳建议并写入草稿'))
}

/** 一键全部采纳（当前待审查区的实体建议） */
async function adoptAll(): Promise<void> {
  const list = [...pair.suggestions]
  let n = 0
  for (const s of list) {
    const ok = pair.adopt(s, (sug: PairSuggestion) => editor.applySuggestion(sug))
    if (ok) n++
  }
  const { toast } = await import('@/api')
  toast(n ? 'ok' : 'warn', n ? '已采纳 ' + n + ' 条建议' : '建议与草稿状态不匹配，无采纳')
}
function fmtRole(r: string): string { return r === 'user' ? '你' : 'Octo 结对' }

const actionLabel = (a: string): string => a === 'create' ? '新建' : a === 'update' ? '更新' : '删除'
const actionBadgeCls = (a: string): string => a === 'delete' ? 'text-destructive border-destructive/40' : a === 'update' ? 'text-warning border-warning/40' : 'text-success border-success/40'
const canSend = computed(() => pair.sending || !pair.input.trim())
</script>

<template>
  <div class="flex h-full min-h-0">
    <!-- 左：会话流 -->
    <div class="flex min-w-0 flex-1 flex-col">
      <div class="flex flex-none items-baseline gap-2.5 border-b border-border px-4 py-2.5">
        <span class="flex items-center gap-1.5 text-[13px] font-extrabold tracking-wide text-primary">
          <IconSparkles class="size-4" />
          Octo · AI 结对
        </span>
        <span class="text-xs text-muted-foreground/70">边聊边成型 —— 建议先入「待审查」，采纳才落草稿</span>
      </div>

      <div ref="chatEl" class="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-4">
        <div v-if="!pair.messages.length" class="rounded-xl border border-border/60 bg-card/40 px-4 py-3.5 text-[13px] leading-6 text-muted-foreground">
          <p>没有头绪？跟 Octo 聊聊你的想法。</p>
          <p class="mt-1.5 text-xs leading-5 text-muted-foreground/70">试试问：<br />· 「想加一个酒馆 / 夜晚的隐藏地点」<br />· 「我缺几个镇民人物」<br />回答会带来可采纳的建议卡。</p>
        </div>

        <div v-for="(m, i) in pair.messages" :key="i" class="flex max-w-[82%] flex-col gap-1" :class="m.role === 'user' ? 'ml-auto items-end' : 'items-start'">
          <div class="flex items-center gap-1 px-1 text-[10.5px] tracking-wide text-muted-foreground/60">
            <IconUser v-if="m.role === 'user'" class="size-3" />
            <IconSparkles v-else class="size-3 text-primary/70" />
            {{ fmtRole(m.role) }}
          </div>
          <div
            class="rounded-xl px-3.5 py-2.5 text-[13.5px] leading-relaxed break-words whitespace-pre-wrap"
            :class="m.role === 'user'
              ? 'rounded-br-sm bg-secondary text-secondary-foreground'
              : 'rounded-bl-sm border border-border bg-card text-foreground'"
          >{{ m.content }}</div>
        </div>

        <div v-if="pair.sending" class="flex max-w-[82%] flex-col items-start gap-1">
          <div class="flex items-center gap-1 px-1 text-[10.5px] tracking-wide text-muted-foreground/60">
            <IconSparkles class="size-3 text-primary/70" />
            Octo 结对
          </div>
          <div class="flex items-center gap-1.5 rounded-xl rounded-bl-sm border border-border bg-card px-3.5 py-2.5 text-[13px] text-muted-foreground/70">
            <IconLoader2 class="size-3.5 animate-spin" />
            正在想…
          </div>
        </div>
      </div>

      <div class="flex flex-none items-end gap-2 border-t border-border bg-card/40 px-3 py-2.5">
        <Textarea
          v-model="pair.input"
          class="min-h-11 max-h-32 w-full flex-1 resize-none text-[13.5px] leading-relaxed"
          rows="2"
          placeholder="描述你的想法 / 要求（Enter 发送，Shift+Enter 换行）…"
          @keydown="onKey"
        />
        <Button ref="sendBtn" size="sm" class="h-9 gap-1 px-3" :disabled="canSend" @click="onSend">
          <IconSend data-icon="inline-start" />
          {{ pair.sending ? '发送中' : '发送' }}
        </Button>
      </div>
    </div>

    <!-- 右：待审查建议区（与对话区并列） -->
    <aside class="flex w-80 flex-none flex-col border-l border-border bg-card/25">
      <div class="flex flex-none items-center gap-2 border-b border-border px-3.5 py-2.5">
        <span class="flex items-center gap-1.5 text-xs font-extrabold tracking-wide text-muted-foreground">
          <IconListCheck class="size-4" />
          待审查建议
        </span>
        <Badge v-if="pair.suggestions.length" variant="outline" class="px-1.5 text-[11px] text-primary">{{ pair.suggestions.length }}</Badge>
        <Button v-if="pair.suggestions.length > 1" variant="ghost" size="sm" class="ml-auto h-6 gap-1 px-2 text-xs" @click="adoptAll">
          <IconCheck data-icon="inline-start" />
          全部采纳
        </Button>
      </div>
      <div class="min-h-0 flex-1 space-y-2 overflow-y-auto p-3">
        <p v-if="!pair.suggestions.length" class="px-1 py-2 text-xs leading-5 text-muted-foreground/60">建议会出现在这里。审阅后点「采纳」写入草稿；未采纳的不会改动草稿。</p>
        <Card v-for="s in pair.suggestions" :key="s.id" class="gap-1.5 rounded-lg border-border/70 px-3 py-2.5 shadow-none ring-0">
          <div class="flex items-center gap-1.5">
            <Badge variant="outline" class="border-border/50 px-1.5 text-[10px] font-normal" :class="actionBadgeCls(s.action)">{{ actionLabel(s.action) }}</Badge>
            <span class="truncate text-[10px] uppercase tracking-wider text-muted-foreground/60">{{ s.target.kind }}</span>
            <Button
              size="sm"
              class="ml-auto h-6 shrink-0 gap-1 px-2 text-xs"
              :disabled="!!s.applying"
              @click="adopt(s)"
            >
              <IconLoader2 v-if="s.applying" class="animate-spin" />
              <IconCheck v-else data-icon="inline-start" />
              采纳
            </Button>
          </div>
          <div class="truncate pt-0.5 text-[13px] font-bold text-foreground">{{ s.label }}</div>
          <div class="text-xs leading-5 text-muted-foreground">{{ s.summary }}</div>
        </Card>
      </div>
    </aside>
  </div>
</template>
