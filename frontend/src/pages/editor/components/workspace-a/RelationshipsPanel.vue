<script setup lang="ts">
// RelationshipsPanel —— A「关系」tab：#01 关系 = 有向边，人物/势力两两之间
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { RelationshipDef } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import type { SelectOption } from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { IconLink, IconPlus } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

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

function add(): void {
  d.value?.relationships.push({
    id: uid('rel'), from_kind: 'character', from_id: charOptions.value[0]?.value ?? '',
    to_kind: 'character', to_id: charOptions.value[0]?.value ?? '', type: relTypeOptions.value[0]?.value ?? '好感', value: 0
  })
}
function remove(rel: RelationshipDef): void {
  const arr = d.value?.relationships
  if (!arr) return
  const i = arr.indexOf(rel)
  if (i >= 0) arr.splice(i, 1)
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4 flex items-start justify-between gap-3">
      <div>
        <h3 class="font-serif text-lg text-foreground">关系（有向边）</h3>
        <p class="mt-0.5 text-xs text-muted-foreground/70">主体 → 客体：类型 + 数值强度（-100 … 100）；可连 人物 / 势力 两两之间（#01 ⑤）。</p>
      </div>
      <Button size="sm" class="gap-1" @click="add">
        <IconPlus data-icon="inline-start" />
        关系
      </Button>
    </div>
    <div v-if="!d.relationships.length" class="py-3 text-xs text-muted-foreground/70">还没有关系边。先在「人物」「势力」tab 添加实体，再来这里连线。</div>
    <div v-for="rel in d.relationships" :key="rel.id" class="mb-3">
      <EntityCard
        :title="nameOf(rel.from_kind, rel.from_id) + ' → ' + nameOf(rel.to_kind, rel.to_id)"
        :sub="'关系 · ' + rel.type"
        :icon="IconLink"
        :meta="String(rel.value)"
        @remove="remove(rel)"
      >
        <div class="grid grid-cols-2 gap-x-4">
          <FieldSelect label="起点类型" :model-value="rel.from_kind" :options="[{ value: 'character', label: '人物' }, { value: 'faction', label: '势力' }]" @update:model-value="rel.from_kind = $event as 'character' | 'faction'" />
          <FieldSelect label="起点实体" :model-value="rel.from_id" :options="entityOptions(rel.from_kind)" @update:model-value="rel.from_id = $event" />
          <FieldSelect label="终点类型" :model-value="rel.to_kind" :options="[{ value: 'character', label: '人物' }, { value: 'faction', label: '势力' }]" @update:model-value="rel.to_kind = $event as 'character' | 'faction'" />
          <FieldSelect label="终点实体" :model-value="rel.to_id" :options="entityOptions(rel.to_kind)" @update:model-value="rel.to_id = $event" />
        </div>
        <FieldSelect v-if="relTypeOptions.length" label="类型" :model-value="rel.type" :options="relTypeOptions" hint="来自「声明」tab 的关系类型" @update:model-value="rel.type = $event" />
        <FieldText v-else label="类型" :model-value="rel.type" placeholder="好感 / 敌意 / 同盟…（可在「声明」tab 定义候选）" @update:model-value="rel.type = $event" />
        <FieldNum label="强度" :model-value="rel.value" :min="-100" :max="100" hint="正 = 亲近，负 = 敌对" @update:model-value="rel.value = $event" />
      </EntityCard>
    </div>
  </div>
</template>
