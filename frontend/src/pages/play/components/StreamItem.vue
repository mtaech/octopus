<script setup lang="ts">
// 叙事组件宿主：把各类演出流条目渲染为模板无关卡片/行
// —— 判定结算卡 / 确认门卡 / 系统行 / 目标节拍进度 / 意图结算
// 迁移：system info/warn/error → 徽标 + text-muted-foreground / text-warning / text-destructive。
import { computed } from 'vue'
import CheckCard from './CheckCard.vue'
import PendingCard from './PendingCard.vue'
import FeedText from './FeedText.vue'
import type { FeedEntry } from '../stores/play'
import {
  IconInfoCircle,
  IconAlertTriangle,
  IconAlertCircle,
  IconTarget,
  IconSparkles,
  IconCircleCheck,
  IconCircleX
} from '@tabler/icons-vue'

const props = withDefaults(defineProps<{ entry: FeedEntry; showSpeaker?: boolean }>(), { showSpeaker: true })

const sysLevel = computed(() => (props.entry.kind === 'system' ? props.entry.level : 'info'))
const SYS = {
  info: { badge: 'border-info/40 bg-info/10 text-info', row: 'border-info/25 bg-info/5', icon: IconInfoCircle },
  warn: { badge: 'border-warning/45 bg-warning/10 text-warning', row: 'border-warning/30 bg-warning/5', icon: IconAlertTriangle },
  error: { badge: 'border-destructive/45 bg-destructive/10 text-destructive', row: 'border-destructive/30 bg-destructive/5', icon: IconAlertCircle }
}
const sysMeta = computed(() => SYS[sysLevel.value])
const sysLabel = computed(() => (sysLevel.value === 'info' ? '系统' : sysLevel.value === 'warn' ? '注意' : '错误'))

const progTone = computed(() => (props.entry.kind === 'progress' ? props.entry.tone : 'goal'))
const PROG = {
  goal: { badge: 'border-success/50 bg-success/10 text-success', row: 'border-success/30 bg-success/5', icon: IconTarget },
  beat: { badge: 'border-primary/50 bg-primary/10 text-primary', row: 'border-primary/30 bg-primary/5', icon: IconSparkles }
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
  <div v-else-if="entry.kind === 'system'" class="mx-auto my-1 flex w-[94%] max-w-[94%] items-center gap-2 rounded-xl border px-3.5 py-2 text-[12px] shadow-2xs backdrop-blur-xs transition-all" :class="sysMeta.row">
    <component :is="sysMeta.icon" class="size-3.5 shrink-0 opacity-85" />
    <span class="rounded-full border px-2 py-0.5 text-[9.5px] font-extrabold tracking-wide whitespace-nowrap" :class="sysMeta.badge">{{ sysLabel }}</span>
    <span v-if="entry.code" class="font-mono text-[10px] text-muted-foreground/60">{{ entry.code }}</span>
    <span class="min-w-0 text-left leading-relaxed" :class="sysLevel === 'info' ? 'text-muted-foreground' : sysLevel === 'warn' ? 'text-warning' : 'text-destructive'">{{ entry.text }}</span>
  </div>

  <!-- 目标/节拍进度 -->
  <div v-else-if="entry.kind === 'progress'" class="mx-auto my-1 flex w-[94%] max-w-[94%] items-center gap-2 rounded-xl border px-3.5 py-2 shadow-2xs backdrop-blur-xs transition-all" :class="progMeta.row">
    <component :is="progMeta.icon" class="size-3.5 shrink-0 opacity-85" />
    <span class="rounded-full border px-2 py-0.5 text-[9.5px] font-extrabold whitespace-nowrap" :class="progMeta.badge">{{ entry.tone === 'goal' ? '目标达成' : '剧情节拍' }}</span>
    <span class="text-[12.5px] text-foreground/90 font-medium">{{ entry.label }}</span>
  </div>

  <!-- 意图结算 -->
  <div v-else-if="entry.kind === 'resolution'" class="mx-auto my-1 flex w-[94%] max-w-[94%] items-center gap-2 rounded-lg px-2 py-1 text-[12px] text-muted-foreground">
    <IconCircleCheck v-if="resOk" class="size-3.5 text-success shrink-0" />
    <IconCircleX v-else class="size-3.5 text-destructive shrink-0" />
    <span class="rounded-full border px-2 py-0.5 text-[9.5px] font-extrabold whitespace-nowrap" :class="resOk ? 'border-success/50 bg-success/10 text-success' : 'border-destructive/45 bg-destructive/10 text-destructive'">{{ entry.status === 'ok' ? '行动结算' : '意图驳回' }}</span>
    <span v-if="entry.text" class="min-w-0 text-foreground/80 font-medium">{{ entry.text }}</span>
    <span v-else>{{ entry.status === 'ok' ? '动作已顺利结算。' : '意图被驳回。' }}</span>
  </div>

  <template v-else-if="entry.kind === 'content'"><FeedText :entry="entry" :show-speaker="showSpeaker" /></template>
</template>
