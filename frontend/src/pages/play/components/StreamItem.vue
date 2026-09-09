<script setup lang="ts">
// 叙事组件宿主：把各类演出流条目渲染为模板无关卡片/行
// —— 判定结算卡 / 确认门卡 / 系统行 / 目标节拍进度 / 意图结算
// 迁移：system info/warn/error → 徽标 + text-muted-foreground / text-warning / text-destructive。
import { computed } from 'vue'
import CheckCard from './CheckCard.vue'
import PendingCard from './PendingCard.vue'
import FeedText from './FeedText.vue'
import type { FeedEntry } from '../stores/play'

const props = withDefaults(defineProps<{ entry: FeedEntry; showSpeaker?: boolean }>(), { showSpeaker: true })

const sysLevel = computed(() => (props.entry.kind === 'system' ? props.entry.level : 'info'))
const SYS = {
  info: { badge: 'border-info/50 text-info', row: 'border-info/30 bg-info/10', dot: 'bg-info' },
  warn: { badge: 'border-warning/55 text-warning', row: 'border-warning/35 bg-warning/10', dot: 'bg-warning' },
  error: { badge: 'border-destructive/55 text-destructive', row: 'border-destructive/40 bg-destructive/10', dot: 'bg-destructive' }
}
const sysMeta = computed(() => SYS[sysLevel.value])
const sysLabel = computed(() => (sysLevel.value === 'info' ? '系统' : sysLevel.value === 'warn' ? '注意' : '错误'))

const progTone = computed(() => (props.entry.kind === 'progress' ? props.entry.tone : 'goal'))
const PROG = {
  goal: { badge: 'border-success/60 text-success', row: 'border-success/35 bg-success/8' },
  beat: { badge: 'border-primary/60 text-primary', row: 'border-primary/35 bg-primary/8' }
}
const progMeta = computed(() => PROG[progTone.value])

const resOk = computed(() => props.entry.kind === 'resolution' && props.entry.status === 'ok')
</script>

<template>
  <div v-if="entry.kind === 'check'" class="w-full"><CheckCard :payload="entry.payload" /></div>
  <div v-else-if="entry.kind === 'pending'" class="w-full">
    <PendingCard :action-id="entry.actionId" :description="entry.description" :impact="entry.impact" :actor-name="entry.actorName" :state="entry.state" :round="entry.round" />
  </div>

  <!-- 系统行：info/warn/error 徽标行 -->
  <div v-else-if="entry.kind === 'system'" class="mx-auto flex w-[92%] max-w-[92%] items-center gap-2 rounded-xl border border-dashed px-3 py-1.5 text-center text-[12.5px]" :class="sysMeta.row">
    <span class="rounded-full border px-2 py-px text-[10px] font-extrabold tracking-wide whitespace-nowrap" :class="sysMeta.badge">{{ sysLabel }}</span>
    <span v-if="entry.code" class="font-mono text-[10px] opacity-70">{{ entry.code }}</span>
    <span class="min-w-0 text-left" :class="sysLevel === 'info' ? 'text-muted-foreground' : sysLevel === 'warn' ? 'text-warning' : 'text-destructive'">{{ entry.text }}</span>
  </div>

  <!-- 目标/节拍进度 -->
  <div v-else-if="entry.kind === 'progress'" class="mx-auto flex w-[92%] max-w-[92%] items-center gap-2 rounded-xl border px-3 py-1.5" :class="progMeta.row">
    <span class="rounded-full border px-2 py-px text-[10px] font-extrabold whitespace-nowrap" :class="progMeta.badge">{{ entry.tone === 'goal' ? '目标' : '节拍' }}</span>
    <span class="text-[12.5px] text-muted-foreground">{{ entry.label }}</span>
  </div>

  <!-- 意图结算 -->
  <div v-else-if="entry.kind === 'resolution'" class="mx-auto flex w-[92%] max-w-[92%] items-center gap-2 rounded-lg px-1.5 py-1 text-[12.5px] text-muted-foreground">
    <span class="rounded-full border px-2 py-px text-[10px] font-extrabold whitespace-nowrap" :class="resOk ? 'border-success/60 text-success' : 'border-destructive/55 text-destructive'">{{ entry.status === 'ok' ? '结算' : '驳回' }}</span>
    <span v-if="entry.text" class="min-w-0">{{ entry.text }}</span>
    <span v-else>{{ entry.status === 'ok' ? '动作已结算。' : '意图被驳回。' }}</span>
  </div>

  <template v-else-if="entry.kind === 'content'"><FeedText :entry="entry" :show-speaker="showSpeaker" /></template>
</template>
