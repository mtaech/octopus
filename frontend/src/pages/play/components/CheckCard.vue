<script setup lang="ts">
// 判定结算卡（#08 Q6/#17 check_result）：跟随故事书判定器，成功度按差值分档。
// 迁移：level 用语义色——great=text-success 淡底 / success=text-primary 淡琥珀 /
// barely=text-warning / fail=text-destructive。结构保留 dice/rolls/mod/total/margin。
import { computed } from 'vue'
import type { CheckResultPayload } from '@/types'
import { LEVEL_META, initial } from '../utils'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import {
  IconSparkles,
  IconCircleCheck,
  IconAlertTriangle,
  IconX,
} from '@tabler/icons-vue'

const props = defineProps<{ payload: CheckResultPayload }>()

const level = computed(() => props.payload.level)
const levelLabel = computed(() => LEVEL_META[level.value].label)
const levelHint = computed(() => LEVEL_META[level.value].hint)
const rollDice = computed(() => props.payload.rolls ?? [])
const exprText = computed(() => props.payload.expr ?? (rollDice.value.length ? '' : '无骰判定'))
const sign = (n: number) => (n > 0 ? '+' + n : String(n))

/** level → 语义态：色类 + 底（淡） */
const LV = {
  great:   { text: 'text-success',   ring: 'border-success/50 bg-success/10',   chip: 'bg-success/20 text-success', icon: IconSparkles },
  success: { text: 'text-primary',   ring: 'border-primary/45 bg-primary/10',   chip: 'bg-primary/20 text-primary', icon: IconCircleCheck },
  barely:  { text: 'text-warning',   ring: 'border-warning/50 bg-warning/10',   chip: 'bg-warning/20 text-warning', icon: IconAlertTriangle },
  fail:    { text: 'text-destructive', ring: 'border-destructive/50 bg-destructive/10', chip: 'bg-destructive/20 text-destructive', icon: IconX }
} as const
const lv = computed(() => LV[level.value])
</script>

<template>
  <div class="w-full rounded-2xl border px-4.5 py-3.5 text-[13px] shadow-xs backdrop-blur-xs transition-all" :class="lv.ring">
    <div class="flex items-center gap-3">
      <Avatar class="size-7.5 ring-1 ring-border/80">
        <AvatarFallback class="text-xs font-bold" :class="lv.chip">{{ initial(payload.actor.name) }}</AvatarFallback>
      </Avatar>
      <div class="min-w-0 flex-1">
        <div class="font-bold leading-tight text-foreground">{{ payload.actor.name }} · {{ payload.attribute }}</div>
        <div class="text-[11px] text-muted-foreground/75 font-medium">机制判定</div>
      </div>
      <div class="flex items-center gap-1.5 rounded-full px-2.5 py-1 border" :class="[lv.ring, lv.text]">
        <component :is="lv.icon" class="size-3.5" />
        <span class="text-xs font-extrabold tracking-wide">{{ levelLabel }}</span>
      </div>
    </div>

    <!-- 掷骰结果行 -->
    <div class="mt-3 flex flex-wrap items-center gap-2.5 rounded-xl bg-card/60 px-3 py-2 border border-border/60">
      <span v-if="exprText" class="font-mono text-xs text-muted-foreground/85 font-medium">{{ exprText }}</span>
      <div v-if="rollDice.length" class="inline-flex items-center gap-1.5">
        <template v-for="(r, i) in rollDice" :key="i">
          <span class="bg-primary text-primary-foreground font-mono flex size-7.5 items-center justify-center rounded-lg font-black text-[14.5px] shadow-sm shadow-black/40 ring-1 ring-white/15">
            {{ r }}
          </span>
          <span v-if="i < rollDice.length - 1" class="text-muted-foreground/60 text-xs">+</span>
        </template>
      </div>
      <span class="text-xs text-muted-foreground font-medium">修正 {{ sign(payload.mod) }}</span>
      <span class="text-muted-foreground/60 font-mono">=</span>
      <span class="font-mono text-lg font-black text-foreground">{{ payload.total }}</span>
      <span class="text-xs text-muted-foreground ml-auto">目标 {{ payload.target }}（{{ levelHint }}）</span>
    </div>

    <!-- 最终达成度 -->
    <div class="mt-2 flex items-center justify-between border-t border-border/50 pt-2 text-xs">
      <div class="flex items-center gap-2">
        <span class="font-bold" :class="payload.result ? 'text-success' : 'text-destructive'">
          {{ payload.result ? '✓ 判定达成' : '✗ 判定失败' }}
        </span>
      </div>
      <span class="font-mono text-[11.5px] text-muted-foreground">
        差值: {{ sign(payload.margin) }}
      </span>
    </div>
  </div>
</template>