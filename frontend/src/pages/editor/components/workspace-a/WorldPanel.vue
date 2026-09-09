<script setup lang="ts">
// WorldPanel —— A「世界」tab：#01 world = premise + locations + resources (+ check 全局)
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { LocationDef, ResourceDef } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { IconMapPin, IconDiamond, IconPlus } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

function addLocation(): void {
  d.value?.world.locations.push({ id: uid('loc'), name: '新地点', description: '' })
}
function removeLocation(loc: LocationDef): void {
  const arr = d.value?.world.locations
  if (!arr) return
  const i = arr.indexOf(loc)
  if (i >= 0) arr.splice(i, 1)
}
function addResource(): void {
  d.value?.world.resources.push({ id: uid('res'), name: '新资源', type: 'numerical', default_max: 100 })
}
function removeResource(r: ResourceDef): void {
  const arr = d.value?.world.resources
  if (!arr) return
  const i = arr.indexOf(r)
  if (i >= 0) arr.splice(i, 1)
}
const locOptions = computed(() => d.value?.world.locations.map(l => ({ value: l.id, label: l.name })) ?? [])
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4">
      <h3 class="font-serif text-lg text-foreground">世界设定</h3>
      <p class="mt-0.5 text-xs text-muted-foreground/70">背景叙事（premise）、地点与资源。判定器沿用全局默认（#12）。</p>
    </div>

    <FieldArea label="背景叙事" :model-value="d.world.premise" md :rows="6" placeholder="故事发生的世界是什么样？基调、常识、通用规则…" hint="叙事自由书写区：B 文档范式也会渲染这里" @update:model-value="d.world.premise = $event" />

    <div class="mt-5 mb-1.5 flex items-center justify-between">
      <span class="text-xs font-bold tracking-wider text-muted-foreground">地点</span>
      <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" @click="addLocation">
        <IconPlus data-icon="inline-start" />
        地点
      </Button>
    </div>
    <div v-if="!d.world.locations.length" class="py-2 text-xs text-muted-foreground/70">还没有地点。</div>
    <div class="flex flex-col gap-2.5">
      <EntityCard v-for="loc in d.world.locations" :key="loc.id" :title="loc.name" sub="地点" :icon="IconMapPin" @remove="removeLocation(loc)">
        <FieldText label="名称" :model-value="loc.name" @update:model-value="loc.name = $event" />
        <FieldText label="id" :model-value="loc.id" mono hint="引用完整性：场景指向此 id" @update:model-value="loc.id = $event" />
        <FieldArea label="描述" :model-value="loc.description ?? ''" placeholder="这里是什么样…" @update:model-value="loc.description = $event" />
      </EntityCard>
    </div>

    <div class="mt-5 mb-1.5 flex items-center justify-between">
      <span class="text-xs font-bold tracking-wider text-muted-foreground">资源</span>
      <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" @click="addResource">
        <IconPlus data-icon="inline-start" />
        资源
      </Button>
    </div>
    <div v-if="!d.world.resources.length" class="py-2 text-xs text-muted-foreground/70">还没有资源。技能消耗会引用资源 id。</div>
    <div class="flex flex-col gap-2.5">
      <EntityCard v-for="r in d.world.resources" :key="r.id" :title="r.name" sub="资源" :icon="IconDiamond" @remove="removeResource(r)">
        <FieldText label="名称" :model-value="r.name" @update:model-value="r.name = $event" />
        <FieldText label="id" :model-value="r.id" mono @update:model-value="r.id = $event" />
        <FieldSelect label="类型" :model-value="r.type" :options="[{ value: 'numerical', label: '数值' }, { value: 'binary', label: '二元' }]" @update:model-value="r.type = $event as 'numerical' | 'binary'" />
        <FieldNum v-if="r.type === 'numerical'" label="上限" :model-value="r.default_max ?? 0" @update:model-value="r.default_max = $event" />
      </EntityCard>
    </div>
  </div>
</template>
