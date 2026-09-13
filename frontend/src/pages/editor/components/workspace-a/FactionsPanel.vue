<script setup lang="ts">
// FactionsPanel —— A「势力」tab：#01 factions 组织 / 阵营
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { FactionDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import { IconBuilding } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('factions')
function select(id: string): void { selected.value = id }

const list = computed<FactionDef[]>(() => d.value?.factions ?? [])
const items = computed<WorkbenchItem[]>(() => list.value.map(f => ({
  id: entityKey(f),
  title: f.name,
  sub: f.id,
  badge: (f.goals?.length ?? 0) ? (f.goals?.length ?? 0) + ' 目标' : undefined,
})))

const current = computed(() => list.value.find(f => entityKey(f) === selected.value) ?? null)

function add(): void {
  const arr = d.value?.factions
  if (!arr) return
  const f: FactionDef = { id: uid('fac'), name: '新势力', description: '', goals: [], default_attitude: 0 }
  arr.unshift(f)
  selected.value = entityKey(f)
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
  <WorkbenchLayout
    title="势力"
    hint="组织 / 阵营（#01 ⑤）。人物或势力之间可建立关系边（见「关系」tab）。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="势力"
    search-placeholder="搜索势力…"
    empty-hint="还没有势力。点右上「势力」新增。"
    @update:selected="select"
    @add="add"
  >
    <template v-if="current">
      <EntityFormHeader :title="current.name" sub="势力 · 组织或阵营" :icon="IconBuilding" :meta="current.id" @remove="remove(current)" />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="current.name" @update:model-value="current.name = $event" />
        <FieldText label="id" mono hint="关系边的 from / to 引用此 id" :model-value="current.id" @update:model-value="current.id = $event" />
        <FieldNum label="默认态度" :min="-100" :max="100" hint="对陌生人的初始关系值（-100 敌意 … 100 友善）" :model-value="current.default_attitude ?? 0" @update:model-value="current.default_attitude = $event" />
        <FieldArea label="描述" :model-value="current.description" @update:model-value="current.description = $event" />
        <FieldArea label="目标（每行一个）" :rows="3" :model-value="goalText(current)" @update:model-value="setGoals(current, $event)" />
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个势力开始编辑。</div>
  </WorkbenchLayout>
</template>
