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
import { IconCircleCheck, IconBan, IconAlertTriangle } from '@tabler/icons-vue'

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
  <div class="w-full">
    <!-- 挂起确认 -->
    <Card v-if="state === 'pending'" size="sm" class="border-warning/45 bg-warning/10 py-0">
      <CardContent class="px-4 py-3">
        <div class="mb-1.5 flex flex-wrap items-center gap-2">
          <Badge variant="outline" class="border-warning/60 text-warning">待确认</Badge>
          <span class="text-[13px] font-bold">{{ actorName }}</span>
          <span class="ml-auto text-[11px] text-muted-foreground">确认期间回合保持开启</span>
        </div>
        <div class="font-semibold">{{ description }}</div>
        <div v-if="impact" class="mt-0.5 text-[12.5px] text-muted-foreground">影响：{{ impact }}</div>
        <div class="mt-2.5 flex gap-2">
          <Button size="sm" :disabled="localBusy" @click="decide('confirm')">确认执行</Button>
          <Button size="sm" variant="ghost" :disabled="localBusy" @click="decide('cancel')">取消</Button>
        </div>
        <div class="mt-2 text-[11px] text-muted-foreground/70">20 秒内未确认将自动取消</div>
      </CardContent>
    </Card>

    <!-- 已确认：原位结算态 -->
    <Card v-else-if="state === 'confirmed'" size="sm" class="border-success/40 bg-success/10 py-0">
      <CardContent class="flex items-center gap-2 px-4 py-3">
        <IconCircleCheck class="size-4 shrink-0 text-success" />
        <Badge variant="outline" class="border-success/60 text-success">已确认</Badge>
        <span class="text-[12.5px] text-muted-foreground">{{ description }} — 结算中…</span>
      </CardContent>
    </Card>

    <!-- 已取消 -->
    <Card v-else size="sm" class="border-border bg-muted/40 py-0 opacity-80">
      <CardContent class="flex items-center gap-2 px-4 py-3">
        <IconBan class="size-4 shrink-0 text-muted-foreground" />
        <Badge variant="outline">已取消</Badge>
        <span class="text-[12.5px] text-muted-foreground">{{ description }} — 你收回了动作</span>
      </CardContent>
    </Card>
  </div>
</template>
