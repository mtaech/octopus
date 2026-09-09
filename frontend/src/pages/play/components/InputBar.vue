<script setup lang="ts">
// 输入区（#08 ④）：同一输入框双通道 —— 普通输入=角色输入（受控角色名义），
// / 开头=元指令（channel:'meta'）。常驻快捷按钮直接发元指令文本。
// 迁移：<Textarea> 自动高 + 发送 <Button>；快捷按钮 <Button size=sm variant=ghost>。
import { ref, computed, watch, nextTick } from 'vue'
import { usePlayStore } from '../stores/play'
import { Textarea } from '@/components/ui/textarea'
import { Button } from '@/components/ui/button'

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
  { label: '切换角色', kind: 'switch' },
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
  <div class="border-t border-border bg-card px-4 pt-2 pb-2.5">
    <div class="mb-1.5 flex items-center gap-2.5">
      <span class="inline-flex items-center gap-1.5 text-[11px] font-bold" :class="isMeta ? 'text-warning' : 'text-primary'">
        <span class="size-1.5 rounded-full" :class="isMeta ? 'bg-warning' : 'bg-primary'"></span>{{ metaHint }}
      </span>
      <div class="ml-auto flex gap-1.5">
        <Button v-for="q in QUICK" :key="q.label" size="xs" variant="ghost" :disabled="disabled || store.waitingConfirm" @click="runQuick(q)">{{ q.label }}</Button>
      </div>
    </div>
    <div class="flex items-end gap-2">
      <div ref="boxEl" class="min-w-0 flex-1">
      <Textarea
        v-model="input"
        rows="1"
        class="min-h-10 max-h-[120px] resize-none px-3 py-2 text-[13.5px] leading-relaxed"
        :placeholder="placeholder"
        :disabled="!store.ready"
        @keydown="onKeydown"
      />
      </div>
      <Button class="h-10 shrink-0" :disabled="disabled || !input.trim()" @click="submit">发送</Button>
    </div>
    <div class="mt-1 text-[10.5px] text-muted-foreground/60">以受控角色 {{ store.controlled?.name ?? '' }} 发言 · Enter 发送 / Shift+Enter 换行</div>
  </div>
</template>