<script setup lang="ts">
// FactionsPanel —— A「势力」tab：#01 factions 组织 / 阵营
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { FactionDef } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import { Button } from '@/components/ui/button'
import { IconBuilding, IconPlus } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

function add(): void {
  d.value?.factions.push({ id: uid('fac'), name: '新势力', description: '', goals: [], default_attitude: 0 })
}
function remove(f: FactionDef): void {
  const arr = d.value?.factions
  if (!arr) return
  const i = arr.indexOf(f)
  if (i >= 0) arr.splice(i, 1)
}
function goalText(f: FactionDef): string { return (f.goals ?? []).join(String.fromCharCode(10)) }
function setGoals(f: FactionDef, t: string): void {
  f.goals = t.split(String.fromCharCode(10)).map(s => s.trim()).filter(Boolean)
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4 flex items-start justify-between gap-3">
      <div>
        <h3 class="font-serif text-lg text-foreground">势力</h3>
        <p class="mt-0.5 text-xs text-muted-foreground/70">组织 / 阵营（#01 ⑤）。人物或势力之间可建立关系边（见「关系」tab）。</p>
      </div>
      <Button size="sm" class="gap-1" @click="add">
        <IconPlus data-icon="inline-start" />
        势力
      </Button>
    </div>
    <div v-if="!d.factions.length" class="py-3 text-xs text-muted-foreground/70">还没有势力。</div>
    <div v-for="f in d.factions" :key="f.id" class="mb-3">
      <EntityCard :title="f.name" sub="势力" :icon="IconBuilding" @remove="remove(f)">
        <FieldText label="名称" :model-value="f.name" @update:model-value="f.name = $event" />
        <FieldText label="id" :model-value="f.id" mono @update:model-value="f.id = $event" />
        <FieldArea label="描述" :model-value="f.description" @update:model-value="f.description = $event" />
        <FieldArea label="目标（每行一个）" :model-value="goalText(f)" :rows="3" @update:model-value="setGoals(f, $event)" />
        <FieldNum label="默认态度" :model-value="f.default_attitude ?? 0" :min="-100" :max="100" hint="对陌生人的初始关系值（-100 敌意 … 100 友善）" @update:model-value="f.default_attitude = $event" />
      </EntityCard>
    </div>
  </div>
</template>
