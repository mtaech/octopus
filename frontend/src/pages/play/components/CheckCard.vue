<script setup lang="ts">
// 判定结算卡（#08 Q6/#17 check_result）：跟随故事书判定器，成功度按差值分档。
// 迁移：level 用语义色——great=text-success 淡底 / success=text-primary 淡琥珀 /
// barely=text-warning / fail=text-destructive。结构保留 dice/rolls/mod/total/margin。
// 对抗判定（判定 C3）：payload.opponent 由引擎在 opposed 时署名，双方各得一次值比大小；
// 引擎把**对手总值**写成 target（resolve::apply_opposed_target），margin = 己方 total − 对手总值。
import { computed } from 'vue'
import type { CheckResultPayload } from '@/types'
import { LEVEL_META, initial } from '../utils'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import {
  IconSparkles,
  IconCircleCheck,
  IconAlertTriangle,
  IconX,
  IconSwords,
} from '@tabler/icons-vue'

const props = defineProps<{ payload: CheckResultPayload }>()

/** 对抗的另一方：为空 = 单方判定（与静态难度比），布局逐字沿用旧版。 */
const opponent = computed(() => props.payload.opponent ?? null)
const level = computed(() => props.payload.level)
const levelLabel = computed(() => LEVEL_META[level.value].label)
const levelHint = computed(() => LEVEL_META[level.value].hint)
const rollDice = computed(() => props.payload.rolls ?? [])
const exprText = computed(() => props.payload.expr ?? (rollDice.value.length ? '' : '无骰判定'))
const sign = (n: number) => (n > 0 ? '+' + n : String(n))
const KIND_LABEL: Record<string, string> = { attribute: '属性检定', attack: '攻击检定', save: '豁免', passive: '被动值' }
const kindText = computed(() => KIND_LABEL[props.payload.kind ?? 'attribute'] ?? '机制判定')

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
        <div class="flex items-center gap-1.5 text-[11px] text-muted-foreground/75 font-medium">
          <span>{{ kindText }}</span>
          <span v-if="opponent" class="inline-flex items-center gap-1 rounded-full border border-destructive/40 bg-destructive/10 px-1.5 text-[10px] font-bold text-destructive">
            <IconSwords class="size-3" />对抗判定
          </span>
        </div>
      </div>
      <div class="flex items-center gap-1.5 rounded-full px-2.5 py-1 border" :class="[lv.ring, lv.text]">
        <component :is="lv.icon" class="size-3.5" />
        <span class="text-xs font-extrabold tracking-wide">{{ levelLabel }}</span>
      </div>
    </div>

    <!-- 对抗双方（判定 C3）：主动方骰面 → 总值；对手总值 = 本次判定的 target -->
    <div v-if="opponent" class="mt-3 rounded-xl border border-border/60 bg-card/60 px-3 py-2.5">
      <div class="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-start gap-2">
        <!-- 主动方 -->
        <div class="min-w-0">
          <div class="flex items-center gap-1.5">
            <span class="truncate text-[12px] font-bold text-foreground">{{ payload.actor.name }}</span>
            <span class="shrink-0 rounded-full border border-primary/45 bg-primary/12 px-1.5 text-[9.5px] font-bold text-primary">主动方</span>
          </div>
          <div class="mt-1.5 flex flex-wrap items-center gap-1">
            <template v-if="rollDice.length">
              <template v-for="(r, i) in rollDice" :key="i">
                <span class="bg-primary text-primary-foreground font-mono flex size-6.5 items-center justify-center rounded-md font-black text-[13px] shadow-sm shadow-black/30 ring-1 ring-white/15">{{ r }}</span>
                <span v-if="i < rollDice.length - 1" class="text-[10.5px] text-muted-foreground/60">+</span>
              </template>
            </template>
            <span v-else-if="exprText" class="font-mono text-[10.5px] text-muted-foreground/80">{{ exprText }}</span>
            <span v-else class="text-[10.5px] text-muted-foreground/70">无骰</span>
            <span class="text-[10.5px] text-muted-foreground font-medium">修正 {{ sign(payload.mod) }}</span>
          </div>
          <div class="mt-1 flex items-baseline gap-1">
            <span class="text-[10px] text-muted-foreground/70">总值</span>
            <span class="font-mono text-[17px] font-black text-foreground">{{ payload.total }}</span>
          </div>
        </div>

        <div class="self-center px-0.5 font-mono text-[10.5px] font-bold text-muted-foreground/60">VS</div>

        <!-- 对手：引擎只在事件里回传署名与总值（骰面不进 payload），总值即 target -->
        <div class="min-w-0 text-right">
          <div class="flex items-center justify-end gap-1.5">
            <span class="shrink-0 rounded-full border border-destructive/45 bg-destructive/10 px-1.5 text-[9.5px] font-bold text-destructive">对手</span>
            <span class="truncate text-[12px] font-bold text-foreground">{{ opponent.name }}</span>
          </div>
          <div class="mt-1.5 text-[10.5px] text-muted-foreground/70">对手掷出 / 被动值</div>
          <div class="mt-1 flex items-baseline justify-end gap-1">
            <span class="text-[10px] text-muted-foreground/70">总值</span>
            <span class="font-mono text-[17px] font-black text-foreground">{{ payload.target }}</span>
          </div>
        </div>
      </div>
      <p class="mt-2 border-t border-border/50 pt-1.5 text-[10.5px] leading-4 text-muted-foreground/70">
        对抗：双方各得一次值比大小——对手总值即本次判定的目标值，比大小看差值。
      </p>
    </div>

    <!-- 单方判定的掷骰结果行（无对手时逐字沿用旧版布局） -->
    <div v-else class="mt-3 flex flex-wrap items-center gap-2.5 rounded-xl bg-card/60 px-3 py-2 border border-border/60">
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
        <span class="inline-flex items-center gap-1 font-bold" :class="payload.result ? 'text-success' : 'text-destructive'">
          <IconCircleCheck v-if="payload.result" class="size-3.5" />
          <IconX v-else class="size-3.5" />
          {{ payload.result ? '判定达成' : '判定失败' }}
        </span>
      </div>
      <span class="font-mono text-[11.5px] text-muted-foreground">
        <template v-if="opponent">差值: {{ sign(payload.margin) }}（{{ payload.total }} − {{ payload.target }}）</template>
        <template v-else>差值: {{ sign(payload.margin) }}</template>
      </span>
    </div>
  </div>
</template>
