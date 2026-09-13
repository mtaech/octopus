<script setup lang="ts">
// 流程日志视图：按 seq 顺序展示本会话的原始事件。
// 与「对话」视图的区别：这里保留**全部**事件——包括不渲染进对话的引擎内部事件
// （被驳回的意图 / 阶段 / 思考 / 状态变更 / 系统消息），用于对演出流程做整体复盘。
import type { FlowLogLine } from '../stores/play'

defineProps<{ lines: FlowLogLine[] }>()

const TONE_CLASS: Record<FlowLogLine['tone'], string> = {
  info: 'text-foreground/85',
  warn: 'text-warning',
  error: 'text-destructive',
  muted: 'text-muted-foreground/60',
}
const TYPE_CLASS: Record<string, string> = {
  resolution: 'text-warning',
  check_result: 'text-primary',
  system: 'text-muted-foreground',
}
function hhmmss(ts?: string): string {
  if (!ts) return ''
  const d = new Date(ts)
  return isNaN(d.getTime()) ? '' : d.toLocaleTimeString('zh-CN', { hour12: false })
}
</script>

<template>
  <div class="mx-auto w-[96%] max-w-[980px] py-3 font-mono text-[11.5px] leading-relaxed">
    <div v-if="!lines.length" class="py-10 text-center text-[12px] text-muted-foreground">
      本会话还没有事件。
    </div>
    <div
      v-for="l in lines"
      :key="l.seq"
      class="flex items-start gap-2 border-b border-border/40 px-1 py-[3px] hover:bg-muted/30"
    >
      <span class="w-9 shrink-0 text-right tabular-nums text-muted-foreground/45">{{ l.seq }}</span>
      <span class="w-10 shrink-0 tabular-nums text-muted-foreground/45">r{{ l.round }}</span>
      <span
        class="w-[104px] shrink-0 truncate"
        :class="TYPE_CLASS[l.type] ?? 'text-primary/70'"
        :title="l.type"
      >{{ l.type }}</span>
      <span class="w-[62px] shrink-0 tabular-nums text-muted-foreground/45">{{ hhmmss(l.ts) }}</span>
      <span class="min-w-0 flex-1 break-words whitespace-pre-wrap" :class="TONE_CLASS[l.tone]">{{ l.text }}</span>
    </div>
  </div>
</template>
