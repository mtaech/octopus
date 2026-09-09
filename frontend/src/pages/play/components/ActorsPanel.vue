<script setup lang="ts">
// 演员栏（A 模板右侧列，#08 ②/⑤）：在场角色紧凑卡列表，受控角色高亮
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import ActorCard from './ActorCard.vue'

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
  <aside class="w-[264px] shrink-0 overflow-y-auto border-l border-border bg-card p-3">
    <div class="mb-2.5 flex items-center gap-1.5">
      <span class="text-xs font-extrabold tracking-widest text-muted-foreground">在场角色</span>
      <span class="rounded-full border border-border bg-muted px-1.5 text-[10.5px]">{{ panelActors.length }}</span>
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
