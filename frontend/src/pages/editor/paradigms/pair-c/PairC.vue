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

      <div ref="chatEl" class="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-5">
        <div v-if="!pair.messages.length" class="rounded-2xl border border-border/80 bg-card/60 p-5 text-[13px] leading-relaxed text-muted-foreground backdrop-blur-xs">
          <div class="flex items-center gap-2 text-foreground font-semibold font-serif text-base mb-2">
            <IconSparkles class="size-4 text-primary" />
            与 Octo 一起构思世界
          </div>
          <p class="text-xs text-muted-foreground/85">没有头绪？直接告诉 Octo 你的想法，它会为你即兴生成角色、地点、技能等结构化建议。</p>
          <div class="mt-4 flex flex-col gap-2">
            <span class="text-[11px] font-semibold text-muted-foreground/60 uppercase tracking-wider">点击快捷尝试：</span>
            <div class="flex flex-wrap gap-2">
              <button
                type="button"
                class="rounded-lg border border-border/80 bg-muted/40 px-3 py-1.5 text-xs text-foreground/90 transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-primary cursor-pointer text-left"
                @click="pair.input = '我想在这个世界里加一个暴风雪夜的藏身酒馆，里面有位神秘老板'"
              >
                🍸 「想加一个暴风雪夜的藏身酒馆」
              </button>
              <button
                type="button"
                class="rounded-lg border border-border/80 bg-muted/40 px-3 py-1.5 text-xs text-foreground/90 transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-primary cursor-pointer text-left"
                @click="pair.input = '帮我设计两个性格迥异的镇民，一个热心老猎人，一个怀揣禁书的年轻学者'"
              >
                👥 「设计两个性格迥异的镇民人物」
              </button>
              <button
                type="button"
                class="rounded-lg border border-border/80 bg-muted/40 px-3 py-1.5 text-xs text-foreground/90 transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-primary cursor-pointer text-left"
                @click="pair.input = '为这本故事书增加一幕高潮场景：在旧神遗迹揭开古老封印'"
              >
                ⚡ 「增加一幕高潮场景与关键节拍」
              </button>
            </div>
          </div>
        </div>

        <div v-for="(m, i) in pair.messages" :key="i" class="flex max-w-[84%] flex-col gap-1.5" :class="m.role === 'user' ? 'ml-auto items-end' : 'items-start'">
          <div class="flex items-center gap-1.5 px-1 text-[11px] font-medium tracking-wide text-muted-foreground/75">
            <IconUser v-if="m.role === 'user'" class="size-3 text-muted-foreground" />
            <IconSparkles v-else class="size-3 text-primary" />
            {{ fmtRole(m.role) }}
          </div>
          <div
            class="rounded-2xl px-4 py-3 text-[13.5px] leading-relaxed break-words whitespace-pre-wrap shadow-xs"
            :class="m.role === 'user'
              ? 'rounded-br-xs bg-secondary text-secondary-foreground border border-border/60'
              : 'rounded-bl-xs border border-border/80 bg-card/90 text-foreground border-l-2 border-l-primary/60'"
          >{{ m.content }}</div>
        </div>

        <div v-if="pair.sending" class="flex max-w-[84%] flex-col items-start gap-1.5">
          <div class="flex items-center gap-1.5 px-1 text-[11px] font-medium tracking-wide text-muted-foreground/75">
            <IconSparkles class="size-3 text-primary" />
            Octo 结对
          </div>
          <div class="flex items-center gap-2 rounded-2xl rounded-bl-xs border border-border/80 bg-card/90 px-4 py-3 text-[13px] text-muted-foreground">
            <IconLoader2 class="size-4 animate-spin text-primary" />
            <span>Octo 正在推演设定…</span>
          </div>
        </div>
      </div>

      <div class="flex flex-none items-end gap-2 border-t border-border/80 bg-card/40 px-4 py-3 backdrop-blur-xs">
        <Textarea
          v-model="pair.input"
          class="min-h-12 max-h-32 w-full flex-1 resize-none rounded-xl text-[13.5px] leading-relaxed"
          rows="2"
          placeholder="向 Octo 描述你的想法或要求（Enter 发送，Shift+Enter 换行）…"
          @keydown="onKey"
        />
        <Button ref="sendBtn" size="sm" class="h-10 gap-1.5 px-4 font-semibold shadow-xs" :disabled="canSend" @click="onSend">
          <IconLoader2 v-if="pair.sending" class="size-4 animate-spin" />
          <IconSend v-else data-icon="inline-start" class="size-4" />
          {{ pair.sending ? '发送中' : '发送' }}
        </Button>
      </div>
    </div>

    <!-- 右：待审查建议区（与对话区并列） -->
    <aside class="flex w-84 flex-none flex-col border-l border-border/80 bg-card/30 backdrop-blur-xs">
      <div class="flex flex-none items-center gap-2 border-b border-border/80 px-4 py-3">
        <span class="flex items-center gap-1.5 text-xs font-extrabold tracking-wide text-foreground">
          <IconListCheck class="size-4 text-primary" />
          待审查建议
        </span>
        <Badge v-if="pair.suggestions.length" variant="outline" class="border-primary/40 bg-primary/10 px-1.5 text-[11px] text-primary">{{ pair.suggestions.length }}</Badge>
        <Button v-if="pair.suggestions.length > 1" variant="ghost" size="sm" class="ml-auto h-7 gap-1 px-2.5 text-xs font-medium text-primary hover:bg-primary/10" @click="adoptAll">
          <IconCheck data-icon="inline-start" class="size-3.5" />
          全部采纳
        </Button>
      </div>
      <div class="min-h-0 flex-1 space-y-2.5 overflow-y-auto p-3.5">
        <p v-if="!pair.suggestions.length" class="px-2 py-6 text-center text-xs leading-5 text-muted-foreground/60">
          结对对话中生成的建议卡将出现在这里。<br />
          审阅后点「采纳」直接写入故事书草稿。
        </p>
        <Card v-for="s in pair.suggestions" :key="s.id" class="gap-2 overflow-hidden rounded-xl border-border/75 bg-card/90 p-3.5 shadow-xs transition-all hover:border-primary/40">
          <div class="flex items-center gap-1.5">
            <Badge variant="outline" class="border-border/60 px-1.5 py-0 text-[10px] font-semibold" :class="actionBadgeCls(s.action)">{{ actionLabel(s.action) }}</Badge>
            <span class="font-mono text-[10.5px] uppercase tracking-wider text-muted-foreground/70">{{ s.target.kind }}</span>
            <Button
              size="xs"
              class="ml-auto gap-1 px-2.5 font-medium shadow-2xs"
              :disabled="!!s.applying"
              @click="adopt(s)"
            >
              <IconLoader2 v-if="s.applying" class="size-3 animate-spin" />
              <IconCheck v-else data-icon="inline-start" class="size-3" />
              采纳
            </Button>
          </div>
          <div class="truncate text-[13.5px] font-bold text-foreground">{{ s.label }}</div>
          <div class="text-xs leading-relaxed text-muted-foreground/85">{{ s.summary }}</div>
        </Card>
      </div>
    </aside>
  </div>
</template>
