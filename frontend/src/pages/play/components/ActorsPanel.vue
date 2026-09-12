<script setup lang="ts">
// 演员栏（A 模板右侧列，#08 ②/⑤）：在场角色紧凑卡列表，受控角色高亮
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import ActorCard from './ActorCard.vue'
import { IconUsers } from '@tabler/icons-vue'

const store = usePlayStore()
const controlledId = computed(() => store.controlledId)
const defs = computed(() => {
  const d = store.detail
  return d ? d.storybook.world.resources : []
})
const panelActors = computed(() => store.presentChars)
function onSwitch(id: string) { void store.switchTo(id) }
</script>

<template>
  <aside class="w-[280px] shrink-0 overflow-y-auto border-l border-border bg-card/50 p-3 backdrop-blur-sm">
    <div class="mb-3 flex items-center justify-between border-b border-border/80 pb-2 px-0.5">
      <div class="flex items-center gap-1.5 text-[11px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">
        <IconUsers class="size-3.5 text-primary" />
        <span>在场角色</span>
      </div>
      <span class="font-mono rounded-full border border-border bg-muted/60 px-2 py-0.5 text-[10.5px] font-bold text-muted-foreground">{{ panelActors.length }}</span>
    </div>
    <div class="flex flex-col gap-2.5">
      <ActorCard
        v-for="a in panelActors"
        :key="a.instance_id"
        :actor="a"
        :controlled="a.instance_id === controlledId || a.template_id === controlledId"
        :resource-defs="defs"
        @switch="onSwitch"
      />
    </div>
  </aside>
</template>
