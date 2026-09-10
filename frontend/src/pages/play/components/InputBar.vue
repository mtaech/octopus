<script setup lang="ts">
// 输入区（#08 ④）：同一输入框双通道 —— 普通输入=角色输入（受控角色名义），
// / 开头=元指令（channel:'meta'）。常驻快捷按钮直接发元指令文本。
// 迁移：<Textarea> 自动高 + 发送 <Button>；快捷按钮 <Button size=sm variant=ghost>。
import { ref, computed, watch, nextTick } from 'vue'
import { usePlayStore } from '../stores/play'
import { Textarea } from '@/components/ui/textarea'
import { Button } from '@/components/ui/button'
import { IconSend, IconTerminal2, IconUser, IconDeviceFloppy, IconHelp, IconArrowsExchange, IconCheck } from '@tabler/icons-vue'

const store = usePlayStore()
const input = ref('')
const boxEl = ref<HTMLElement | null>(null)
const metaHint = computed(() => {
  const t = input.value.trim()
  return t.startsWith('/') ? '元指令通道' : '角色通道 · ' + (store.controlled?.name ?? '')
})
const isMeta = computed(() => metaHint.value.startsWith('元'))
const disabled = computed(() => !store.ready || store.sending)
const placeholder = computed(() => {
  if (store.waitingConfirm) return '有待确认动作 — 请先在上方确认或取消…'
  return '输入你想做的事（/ 开头 = 元指令，如 /存档 /帮助）…'
})

type Quick = { label: string; kind: 'meta' | 'toggle' | 'switch'; value?: string }
const QUICK: Quick[] = [
  { label: '/存档', kind: 'meta', value: '/存档' },
  { label: '免确认', kind: 'toggle' },
  { label: '换角色', kind: 'switch' },
  { label: '/帮助', kind: 'meta', value: '/帮助' },
]

function cycleCharacter() {
  const pcs = store.allChars.filter(c => c.kind === 'pc')
  if (pcs.length < 2) return
  const i = pcs.findIndex(c => c.template_id === store.controlledId)
  const next = pcs[(i + 1) % pcs.length]
  if (next) void store.switchTo(next.template_id)
}

function runQuick(q: Quick) {
  if (q.kind === 'toggle') { void store.setAutoConfirm(!store.autoConfirm); return }
  if (q.kind === 'switch') { cycleCharacter(); return }
  quickRun(q.value ?? '')
}

function autoGrow() {
  const ta = boxEl.value?.querySelector('textarea')
  if (ta) { ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, 120) + 'px' }
}

async function submit() {
  const t = input.value.trim()
  if (!t) return
  input.value = ''
  await nextTick(autoGrow)
  await store.send(t)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void submit()
  }
}
function quickRun(label: string) {
  if (disabled.value || store.waitingConfirm) return
  input.value = label
  void submit()
}
watch(input, () => autoGrow())
</script>

<template>
  <div class="border-t border-border/80 bg-card/85 px-4 pt-2.5 pb-3 backdrop-blur-md transition-colors">
    <div class="mb-2 flex items-center justify-between gap-2">
      <!-- 模式指示胶囊 -->
      <div
        class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[11px] font-bold shadow-2xs transition-all"
        :class="isMeta
          ? 'border-warning/40 bg-warning/10 text-warning'
          : 'border-primary/30 bg-primary/10 text-primary'"
      >
        <IconTerminal2 v-if="isMeta" class="size-3 text-warning" />
        <IconUser v-else class="size-3 text-primary" />
        <span>{{ metaHint }}</span>
      </div>

      <!-- 快捷动作芯片 -->
      <div class="flex items-center gap-1.5">
        <Button
          v-for="q in QUICK"
          :key="q.label"
          size="xs"
          variant="outline"
          class="h-6 rounded-full border-border/70 px-2 text-[11px] font-medium transition-all hover:border-primary/40 hover:bg-muted/80"
          :class="{ 'border-primary/50 bg-primary/15 text-primary font-bold': q.kind === 'toggle' && store.autoConfirm }"
          :disabled="disabled || store.waitingConfirm"
          @click="runQuick(q)"
        >
          <template v-if="q.kind === 'meta' && q.label === '/存档'">
            <IconDeviceFloppy class="size-2.5 mr-0.5 text-muted-foreground" />
          </template>
          <template v-else-if="q.kind === 'meta' && q.label === '/帮助'">
            <IconHelp class="size-2.5 mr-0.5 text-muted-foreground" />
          </template>
          <template v-else-if="q.kind === 'switch'">
            <IconArrowsExchange class="size-2.5 mr-0.5 text-muted-foreground" />
          </template>
          <template v-else-if="q.kind === 'toggle' && store.autoConfirm">
            <IconCheck class="size-2.5 mr-0.5 text-primary" />
          </template>
          <span>{{ q.label }}</span>
        </Button>
      </div>
    </div>

    <!-- 输入区与操作按钮 -->
    <div class="flex items-end gap-2.5">
      <div
        ref="boxEl"
        class="min-w-0 flex-1 rounded-xl border border-border/80 bg-background/90 shadow-inner transition-all focus-within:border-primary/60 focus-within:ring-2 focus-within:ring-primary/20"
      >
        <Textarea
          v-model="input"
          rows="1"
          class="min-h-10 max-h-[120px] resize-none border-0 bg-transparent px-3.5 py-2 text-[13.5px] leading-relaxed shadow-none focus-visible:ring-0"
          :placeholder="placeholder"
          :disabled="!store.ready"
          @keydown="onKeydown"
        />
      </div>
      <Button
        class="h-10 shrink-0 rounded-xl px-4 font-bold shadow-sm transition-all"
        :disabled="disabled || !input.trim()"
        @click="submit"
      >
        <IconSend class="size-4 mr-1 transition-transform group-hover:translate-x-0.5" />
        <span>发送</span>
      </Button>
    </div>

    <div class="mt-1.5 flex items-center justify-between text-[10.5px] text-muted-foreground/70">
      <span class="inline-flex items-center gap-1">
        以角色 <b class="font-semibold text-foreground/90">{{ store.controlled?.name ?? '未选择' }}</b> 采取行动
      </span>
      <span class="font-mono text-[10px] text-muted-foreground/50">Enter 发送 · Shift+Enter 换行</span>
    </div>
  </div>
</template>