<script setup lang="ts">
// RelationshipsPanel —— A「关系」tab：#01 关系 = 有向边，人物/势力两两之间
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { RelationshipDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import type { SelectOption } from '../fields/FieldSelect.vue'
import { IconLink } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('relationships')
function select(id: string): void { selected.value = id }

const charOptions = computed<SelectOption[]>(() => (d.value?.characters ?? []).map(c => ({ value: c.id, label: '人物 · ' + c.name })))
const facOptions = computed<SelectOption[]>(() => (d.value?.factions ?? []).map(f => ({ value: f.id, label: '势力 · ' + f.name })))

/** 边类型候选：优先取声明区 relationship_types（#10 决议：编辑器给下拉 + 校验） */
const relTypeOptions = computed<SelectOption[]>(() => (d.value?.relationship_types ?? []).map(t => ({ value: t.key, label: t.label ?? t.key })))

function entityOptions(kind: 'character' | 'faction'): SelectOption[] {
  return kind === 'character' ? charOptions.value : facOptions.value
}
function nameOf(kind: 'character' | 'faction', id: string): string {
  const pool = kind === 'character' ? d.value?.characters ?? [] : d.value?.factions ?? []
  return pool.find(e => e.id === id)?.name ?? id
}

const list = computed<RelationshipDef[]>(() => d.value?.relationships ?? [])
const items = computed<WorkbenchItem[]>(() => list.value.map(r => ({
  id: entityKey(r),
  title: nameOf(r.from_kind, r.from_id) + ' → ' + nameOf(r.to_kind, r.to_id),
  sub: r.type,
  badge: (r.value > 0 ? '+' : '') + r.value,
  tone: r.value > 0 ? 'ok' : r.value < 0 ? 'warn' : 'default',
})))

const current = computed(() => list.value.find(r => entityKey(r) === selected.value) ?? null)

function add(): void {
  const arr = d.value?.relationships
  if (!arr) return
  const rel: RelationshipDef = {
    id: uid('rel'), from_kind: 'character', from_id: charOptions.value[0]?.value ?? '',
    to_kind: 'character', to_id: charOptions.value[0]?.value ?? '', type: relTypeOptions.value[0]?.value ?? '好感', value: 0,
  }
  arr.unshift(rel)
  selected.value = entityKey(rel)
}
function remove(rel: RelationshipDef): void {
  const arr = d.value?.relationships
  if (!arr) return
  const i = arr.indexOf(rel)
  if (i >= 0) arr.splice(i, 1)
}
</script>

<template>
  <WorkbenchLayout
    title="关系"
    hint="主体 → 客体：类型 + 数值强度（-100 … 100）；可连 人物 / 势力 两两之间（#01 ⑤）。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="关系"
    search-placeholder="搜索关系…"
    empty-hint="还没有关系边。先在「人物」「势力」tab 添加实体，再回来连线。"
    @update:selected="select"
    @add="add"
  >
    <template v-if="current">
      <EntityFormHeader
        :title="nameOf(current.from_kind, current.from_id) + ' → ' + nameOf(current.to_kind, current.to_id)"
        :sub="'关系边 · ' + current.type"
        :icon="IconLink"
        :meta="(current.value > 0 ? '+' : '') + current.value"
        :tone="current.value > 0 ? 'ok' : current.value < 0 ? 'warn' : 'default'"
        @remove="remove(current)"
      />
      <FieldGrid class="mt-4">
        <FieldSelect label="起点类型" :model-value="current.from_kind" :options="[{ value: 'character', label: '人物' }, { value: 'faction', label: '势力' }]" @update:model-value="current.from_kind = $event as 'character' | 'faction'" />
        <FieldSelect label="起点实体" :model-value="current.from_id" :options="entityOptions(current.from_kind)" @update:model-value="current.from_id = $event" />
        <FieldSelect label="终点类型" :model-value="current.to_kind" :options="[{ value: 'character', label: '人物' }, { value: 'faction', label: '势力' }]" @update:model-value="current.to_kind = $event as 'character' | 'faction'" />
        <FieldSelect label="终点实体" :model-value="current.to_id" :options="entityOptions(current.to_kind)" @update:model-value="current.to_id = $event" />
        <FieldSelect v-if="relTypeOptions.length" label="类型" :model-value="current.type" :options="relTypeOptions" hint="来自「声明」tab 的关系类型" @update:model-value="current.type = $event" />
        <FieldText v-else label="类型" placeholder="好感 / 敌意 / 同盟…" hint="可在「声明」tab 定义候选" :model-value="current.type" @update:model-value="current.type = $event" />
        <FieldNum label="强度" :min="-100" :max="100" hint="正 = 亲近，负 = 敌对" :model-value="current.value" @update:model-value="current.value = $event" />
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一条关系开始编辑。</div>
  </WorkbenchLayout>
</template>
