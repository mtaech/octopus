<script setup lang="ts">
// 机器事件宿主：判定结算卡 / 确认门卡 / 系统行 / 目标与触发点进度 / 意图结算 / 思考折叠。
// 正文（旁白/台词/神态）已由 feed/ProseFlow.vue 合并渲染，不经过这里。
import { computed } from 'vue'
import CheckCard from './CheckCard.vue'
import PendingCard from './PendingCard.vue'
import { Reasoning, ReasoningContent, ReasoningTrigger } from '@/components/ai-elements/reasoning'
import type { FeedEntry } from '../stores/play'
import {
  IconInfoCircle,
  IconAlertTriangle,
  IconAlertCircle,
  IconTarget,
  IconSparkles,
  IconCircleCheck,
  IconCircleX,
  IconBook2
} from '@tabler/icons-vue'

const props = defineProps<{ entry: FeedEntry }>()

const sysLevel = computed(() => (props.entry.kind === 'system' ? props.entry.level : 'info'))
const SYS = {
  info: { badge: 'border-border bg-muted text-muted-foreground', row: 'bg-muted/40', icon: IconInfoCircle },
  warn: { badge: 'border-warning/45 bg-warning/10 text-warning', row: 'bg-warning/10', icon: IconAlertTriangle },
  error: { badge: 'border-destructive/45 bg-destructive/10 text-destructive', row: 'bg-destructive/10', icon: IconAlertCircle }
}
const sysMeta = computed(() => SYS[sysLevel.value])
const sysLabel = computed(() => (sysLevel.value === 'info' ? '系统' : sysLevel.value === 'warn' ? '注意' : '错误'))

const progTone = computed(() => (props.entry.kind === 'progress' ? props.entry.tone : 'goal'))
const PROG_LABEL: Record<string, string> = { goal: '目标达成', trigger: '触发点', quest: '新增任务' }
const PROG = {
  goal: { badge: 'border-success/50 bg-success/10 text-success', row: 'border-success/30 bg-success/5', icon: IconTarget },
  trigger: { badge: 'border-primary/50 bg-primary/10 text-primary', row: 'border-primary/30 bg-primary/5', icon: IconSparkles },
  quest: { badge: 'border-info/50 bg-info/10 text-info', row: 'border-info/30 bg-info/5', icon: IconBook2 },
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
  <div v-else-if="entry.kind === 'system'" class="mx-auto my-1 flex w-[94%] max-w-[94%] items-center gap-2 rounded-lg px-3 py-1.5 text-[11.5px] transition-colors" :class="sysMeta.row">
    <component :is="sysMeta.icon" class="size-3.5 shrink-0 opacity-85" />
    <span class="rounded-full border px-2 py-0.5 text-[9.5px] font-extrabold tracking-wide whitespace-nowrap" :class="sysMeta.badge">{{ sysLabel }}</span>
    <span v-if="entry.code" class="font-mono text-[10px] text-muted-foreground/60">{{ entry.code }}</span>
    <span class="min-w-0 text-left leading-relaxed" :class="sysLevel === 'info' ? 'text-muted-foreground' : sysLevel === 'warn' ? 'text-warning' : 'text-destructive'">{{ entry.text }}</span>
  </div>

  <!-- 目标/触发点进度 -->
  <div v-else-if="entry.kind === 'progress'" class="mx-auto my-1 flex w-[94%] max-w-[94%] items-center gap-2 rounded-lg px-3 py-1.5 transition-colors" :class="progMeta.row">
    <component :is="progMeta.icon" class="size-3.5 shrink-0 opacity-85" />
    <span class="rounded-full border px-2 py-0.5 text-[9.5px] font-extrabold whitespace-nowrap" :class="progMeta.badge">{{ PROG_LABEL[entry.tone] ?? '进度' }}</span>
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

  <!-- AI 思考：provider reasoning_content 与 think 意图共用同一个折叠组件（P3）；
       source=model 时在触发器上方标「模型草稿」，让玩家知道这是正文里的思考而非供应商思考链 -->
  <div v-else-if="entry.kind === 'reasoning'" class="mx-auto w-[94%] max-w-[94%]">
    <div v-if="entry.source === 'model'" class="mb-0.5 text-[10px] font-semibold tracking-wide text-muted-foreground/60">模型草稿</div>
    <Reasoning :default-open="false" class="mb-1 rounded-lg border border-border/60 bg-muted/20 px-3 py-2">
      <ReasoningTrigger />
      <ReasoningContent :content="entry.text" />
    </Reasoning>
  </div>
</template>
