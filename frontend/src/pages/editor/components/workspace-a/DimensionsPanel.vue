<script setup lang="ts">
// DimensionsPanel —— A「维度设置」tab（#07 ④）：全局属性维度
// 人物表单只消费不内联新建；被引用维度删除由校验 dock 提示（#01 ④）
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { AttributeDimension, DimensionType } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconSettings, IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('dimensions')
function select(id: string): void { selected.value = id }

const DIM_TYPE_OPTIONS = [
  { value: 'number', label: '数值（参与判定）' },
  { value: 'enum', label: '枚举（候选标签）' },
  { value: 'text', label: '文本（备注刻画）' },
]

const dimsList = computed<AttributeDimension[]>(() => d.value?.attribute_dimensions ?? [])

/** 被哪些人物引用（删除提示） */
function usedBy(x: AttributeDimension): string[] {
  return (d.value?.characters ?? []).filter(c => c.attributes && x.key in c.attributes).map(c => c.name)
}

const items = computed<WorkbenchItem[]>(() => dimsList.value.map(x => {
  const n = usedBy(x).length
  return {
    id: entityKey(x),
    title: x.label || x.key,
    sub: x.key,
    badge: n ? '被 ' + n + ' 人引用' : '未引用',
    tone: n ? 'warn' : 'default',
  }
}))

const current = computed(() => dimsList.value.find(x => entityKey(x) === selected.value) ?? null)
const currentUsed = computed(() => (current.value ? usedBy(current.value) : []))

function addDim(): void {
  const arr = d.value?.attribute_dimensions
  if (!arr) return
  const x: AttributeDimension = { key: uid('dim'), label: '新维度', type: 'number', min: 0, max: 100, baseline: 50, modifier_step: 5 }
  arr.unshift(x)
  selected.value = entityKey(x)
}
function removeDim(x: AttributeDimension): void {
  const arr = d.value?.attribute_dimensions
  if (!arr) return
  const i = arr.indexOf(x)
  if (i >= 0) arr.splice(i, 1)
}
function setType(x: AttributeDimension, t: DimensionType): void {
  x.type = t
  if (t === 'number' && x.min == null) { x.min = 0; x.max = 100; x.baseline = 50; x.modifier_step = x.modifier_step ?? 5 }
  if (t === 'enum' && !x.options?.length) x.options = ['标签一', '标签二']
}
function setOption(x: AttributeDimension, i: number, v: string): void {
  if (!x.options) x.options = []
  x.options[i] = v
}
function addOption(x: AttributeDimension): void {
  if (!x.options) x.options = []
  x.options.unshift('新标签')
}
function removeOption(x: AttributeDimension, i: number): void {
  x.options?.splice(i, 1)
}
</script>

<template>
  <WorkbenchLayout
    title="维度"
    hint="故事书全局定义：键 + 标签 + 类型（数值 / 枚举 / 文本）。判定只用数值型；人物表单在此消费这些维度。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="维度"
    search-placeholder="搜索维度…"
    empty-hint="还没有属性维度。点右上「维度」新增第一个。"
    @update:selected="select"
    @add="addDim"
  >
    <template v-if="current">
      <EntityFormHeader
        :title="current.label || current.key"
        sub="属性维度 · 全局定义"
        :icon="IconSettings"
        :meta="currentUsed.length ? '被 ' + currentUsed.length + ' 人物引用' : ''"
        :tone="currentUsed.length ? 'warn' : 'default'"
        @remove="removeDim(current)"
      />
      <FieldGrid class="mt-4">
        <FieldText label="键 key" mono hint="人物 attributes 以此寻址；改键需同步人物（校验会兜底）" :model-value="current.key" @update:model-value="current.key = $event" />
        <FieldText label="标签 label" :model-value="current.label" @update:model-value="current.label = $event" />
        <FieldSelect label="类型" :model-value="current.type" :options="DIM_TYPE_OPTIONS" @update:model-value="setType(current, $event as DimensionType)" />

        <div v-if="current.type === 'number'" class="col-span-full grid grid-cols-2 gap-x-4 @lg:grid-cols-4">
          <FieldNum label="最小值" :model-value="current.min ?? 0" hint="修正范围也由此推出" @update:model-value="current.min = $event" />
          <FieldNum label="最大值" :model-value="current.max ?? 100" @update:model-value="current.max = $event" />
          <FieldNum label="基线" :model-value="current.baseline ?? 50" hint="修正中心" @update:model-value="current.baseline = $event" />
          <FieldNum label="修正步长" :model-value="current.modifier_step ?? 5" hint="默认 5；D&D 用 2" @update:model-value="current.modifier_step = $event" />
        </div>

        <section v-else-if="current.type === 'enum'" class="col-span-full">
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="text-[11px] font-medium text-muted-foreground">候选标签</span>
            <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" @click="addOption(current)">
              <IconPlus class="size-3.5" />
              标签
            </Button>
          </div>
          <div v-for="(o, i) in current.options ?? []" :key="i" class="mb-2 flex items-center gap-2">
            <Input class="h-8 w-full flex-1 text-[13px]" :model-value="o" @update:model-value="setOption(current, i, String($event))" />
            <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" title="删除标签" aria-label="删除标签" @click="removeOption(current, i)">
              <IconX class="size-3.5" />
            </Button>
          </div>
        </section>

        <p v-if="currentUsed.length" class="col-span-full text-[10.5px] leading-4 text-muted-foreground/65">
          引用人物：{{ currentUsed.join('、') }}
        </p>
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个维度开始编辑。</div>
  </WorkbenchLayout>
</template>
