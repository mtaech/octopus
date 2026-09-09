<script setup lang="ts">
// 判定结算卡（#08 Q6/#17 check_result）：跟随故事书判定器，成功度按差值分档。
// 迁移：level 用语义色——great=text-success 淡底 / success=text-primary 淡琥珀 /
// barely=text-warning / fail=text-destructive。结构保留 dice/rolls/mod/total/margin。
import { computed } from 'vue'
import type { CheckResultPayload } from '@/types'
import { LEVEL_META, initial } from '../utils'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'

const props = defineProps<{ payload: CheckResultPayload }>()

const level = computed(() => props.payload.level)
const levelLabel = computed(() => LEVEL_META[level.value].label)
const levelHint = computed(() => LEVEL_META[level.value].hint)
const rollDice = computed(() => props.payload.rolls ?? [])
const exprText = computed(() => props.payload.expr ?? (rollDice.value.length ? '' : '无骰判定'))
const sign = (n: number) => (n > 0 ? '+' + n : String(n))

/** level → 语义态：色类 + 底（淡） */
const LV = {
  great:   { text: 'text-success',   ring: 'border-success/40 bg-success/10',   chip: 'bg-success/15' },
  success: { text: 'text-primary',   ring: 'border-primary/35 bg-primary/10',   chip: 'bg-primary/15' },
  barely:  { text: 'text-warning',   ring: 'border-warning/45 bg-warning/10',   chip: 'bg-warning/15' },
  fail:    { text: 'text-destructive', ring: 'border-destructive/45 bg-destructive/10', chip: 'bg-destructive/15' }
} as const
const lv = computed(() => LV[level.value])
</script>

<template>
  <div class="w-full rounded-xl border px-4 py-3 text-[13px]" :class="lv.ring">
    <div class="flex items-center gap-2.5">
      <Avatar class="size-7">
        <AvatarFallback class="text-xs font-extrabold" :class="lv.chip + ' ' + lv.text">{{ initial(payload.actor.name) }}</AvatarFallback>
      </Avatar>
      <div class="min-w-0 flex-1">
        <div class="font-bold leading-tight">{{ payload.actor.name }} · {{ payload.attribute }}</div>
        <div class="text-[10.5px] text-muted-foreground">判定</div>
      </div>
      <span class="text-xs font-extrabold tracking-wide" :class="lv.text">{{ levelLabel }}</span>
    </div>

    <div class="mt-2.5 flex flex-wrap items-center gap-2">
      <span v-if="exprText" class="font-mono text-xs text-muted-foreground">{{ exprText }}</span>
      <span v-if="rollDice.length" class="inline-flex items-center gap-1.5">
        <template v-for="(r, i) in rollDice" :key="i">
          <span class="bg-primary text-primary-foreground font-mono flex size-7 items-center justify-center rounded-lg border border-primary/60 font-extrabold text-[14px]">{{ r }}</span>
          <span v-if="i < rollDice.length - 1" class="text-muted-foreground">+</span>
        </template>
      </span>
      <span class="text-xs text-muted-foreground">修正 {{ sign(payload.mod) }}</span>
      <span class="text-muted-foreground">=</span>
      <span class="font-mono text-lg font-black text-foreground">{{ payload.total }}</span>
      <span class="text-xs text-muted-foreground">目标 {{ payload.target }}（{{ levelHint }}）</span>
    </div>

    <div class="mt-1.5 flex items-center gap-3 border-t border-dashed border-border/60 pt-1.5">
      <span class="text-[11.5px] font-bold" :class="payload.result ? 'text-success' : 'text-destructive'">{{ payload.result ? '达成' : '未达成' }}</span>
      <span class="font-mono text-[11.5px] text-muted-foreground">差值 {{ sign(payload.margin) }}</span>
    </div>
  </div>
</template>