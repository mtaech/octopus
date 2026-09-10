<script setup lang="ts">
// 确认门卡片（#08 Q3 / #17 pending ↔ confirmAction）：流内挂起卡，
// 确认后原位结算（状态由 play store 在 resolution 到达时改写为 confirmed/cancelled）。
// 迁移：挂起态 <Card>（warning 语义底）+ [确认 Button default][取消 Button ghost]；
// 结算态（confirmed/cancelled）原位保留。
import { ref } from 'vue'
import { usePlayStore } from '../stores/play'
import { Card, CardContent } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconCircleCheck, IconBan, IconAlertTriangle, IconClock, IconCheck, IconX } from '@tabler/icons-vue'

const props = defineProps<{
  actionId: string
  description: string
  impact?: string
  actorName: string
  state: 'pending' | 'confirmed' | 'cancelled'
  round: number
}>()

const store = usePlayStore()
const localBusy = ref(false)

async function decide(decision: 'confirm' | 'cancel') {
  if (localBusy.value || props.state !== 'pending') return
  localBusy.value = true
  await store.confirm(props.actionId, decision)
  localBusy.value = false
}
</script>

<template>
  <div class="my-2 w-full">
    <!-- 挂起确认门 -->
    <Card
      v-if="state === 'pending'"
      size="sm"
      class="overflow-hidden rounded-xl border-warning/50 bg-gradient-to-br from-warning/15 via-card/95 to-warning/5 py-0 shadow-md ring-1 ring-warning/30"
    >
      <CardContent class="p-4">
        <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
          <div class="flex items-center gap-2">
            <span class="relative flex size-2">
              <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-warning opacity-75"></span>
              <span class="relative inline-flex size-2 rounded-full bg-warning"></span>
            </span>
            <Badge variant="outline" class="border-warning/60 bg-warning/15 text-warning font-bold text-[11px]">行动决断</Badge>
            <span class="text-[13px] font-extrabold text-foreground">{{ actorName }}</span>
          </div>
          <span class="text-[11px] text-muted-foreground/80">第 {{ round }} 回合</span>
        </div>

        <div class="text-[14px] font-bold text-foreground leading-relaxed">{{ description }}</div>

        <div v-if="impact" class="mt-2.5 rounded-lg border border-warning/30 bg-warning/10 px-3 py-1.5 text-[12px] text-warning/90 leading-relaxed">
          <span class="font-bold">预估影响：</span>{{ impact }}
        </div>

        <div class="mt-3.5 flex flex-wrap items-center gap-2.5">
          <Button
            size="sm"
            class="h-8 rounded-lg font-bold shadow-xs transition-all"
            :disabled="localBusy"
            @click="decide('confirm')"
          >
            <IconCheck class="size-3.5 mr-1" />
            <span>确认执行</span>
          </Button>
          <Button
            size="sm"
            variant="outline"
            class="h-8 rounded-lg border-border/80 text-muted-foreground hover:text-foreground transition-all"
            :disabled="localBusy"
            @click="decide('cancel')"
          >
            <IconX class="size-3.5 mr-1" />
            <span>取消行动</span>
          </Button>

          <div class="ml-auto flex items-center gap-1 text-[10.5px] text-muted-foreground/70">
            <IconClock class="size-3 text-muted-foreground/60" />
            <span>20 秒未确认将自动取消</span>
          </div>
        </div>
      </CardContent>
    </Card>

    <!-- 已确认：原位结算态 -->
    <Card v-else-if="state === 'confirmed'" size="sm" class="rounded-xl border-success/40 bg-success/10 py-0 shadow-2xs">
      <CardContent class="flex items-center gap-2.5 px-4 py-2.5">
        <IconCircleCheck class="size-4 shrink-0 text-success" />
        <Badge variant="outline" class="border-success/60 bg-success/10 text-success text-[10.5px]">已确认</Badge>
        <span class="text-[12.5px] text-muted-foreground">{{ description }} — 正在结算行动…</span>
      </CardContent>
    </Card>

    <!-- 已取消 -->
    <Card v-else size="sm" class="rounded-xl border-border/60 bg-muted/30 py-0 opacity-70">
      <CardContent class="flex items-center gap-2.5 px-4 py-2.5">
        <IconBan class="size-4 shrink-0 text-muted-foreground" />
        <Badge variant="outline" class="border-border text-muted-foreground text-[10.5px]">已取消</Badge>
        <span class="text-[12.5px] text-muted-foreground">{{ description }} — 你收回了动作</span>
      </CardContent>
    </Card>
  </div>
</template>
