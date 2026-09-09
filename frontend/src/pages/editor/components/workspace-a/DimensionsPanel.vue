<script setup lang="ts">
// DimensionsPanel —— A「维度设置」tab（#07 ④）：全局属性维度
// 人物表单只消费不内联新建；被引用维度删除由校验 dock 提示（#01 ④）
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { AttributeDimension, DimensionType } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconSettings, IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

const DIM_TYPE_OPTIONS = [
  { value: 'number', label: '数值（参与判定）' },
  { value: 'enum', label: '枚举（候选标签）' },
  { value: 'text', label: '文本（备注刻画）' }
]

function addDim(): void {
  d.value?.attribute_dimensions.push({ key: uid('dim'), label: '新维度', type: 'number', min: 0, max: 100, baseline: 50 })
}
function removeDim(x: AttributeDimension): void {
  const arr = d.value?.attribute_dimensions
  if (!arr) return
  const i = arr.indexOf(x)
  if (i >= 0) arr.splice(i, 1)
}
function setType(x: AttributeDimension, t: DimensionType): void {
  x.type = t
  if (t === 'number' && x.min == null) { x.min = 0; x.max = 100; x.baseline = 50 }
  if (t === 'enum' && !x.options?.length) x.options = ['标签一', '标签二']
}
function setOption(x: AttributeDimension, i: number, v: string): void {
  if (!x.options) x.options = []
  x.options[i] = v
}
function addOption(x: AttributeDimension): void {
  if (!x.options) x.options = []
  x.options.push('新标签')
}
function removeOption(x: AttributeDimension, i: number): void {
  x.options?.splice(i, 1)
}
/** 被哪些人物引用（删除提示） */
function usedBy(x: AttributeDimension): string[] {
  return (d.value?.characters ?? []).filter(c => c.attributes && x.key in c.attributes).map(c => c.name)
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4 flex items-start justify-between gap-3">
      <div>
        <h3 class="font-serif text-lg text-foreground">属性维度（全局）</h3>
        <p class="mt-0.5 text-xs text-muted-foreground/70">故事书全局定义：键 + 标签 + 类型（数值 / 枚举 / 文本）。判定只用数值型；人物表单在下拉/输入中消费这些维度。</p>
      </div>
      <Button size="sm" class="gap-1" @click="addDim">
        <IconPlus data-icon="inline-start" />
        维度
      </Button>
    </div>
    <div v-if="!d.attribute_dimensions.length" class="py-3 text-xs text-muted-foreground/70">还没有属性维度。先定义人物可填的属性项。</div>

    <div v-for="x in d.attribute_dimensions" :key="x.key" class="mb-3">
      <EntityCard
        :title="x.label || x.key"
        sub="属性维度"
        :icon="IconSettings"
        :meta="usedBy(x).length ? '被 ' + usedBy(x).length + ' 人物引用' : ''"
        tone="warn"
        @remove="removeDim(x)"
      >
        <FieldText label="键 key" :model-value="x.key" mono hint="人物 attributes 以此寻址；改键需同步人物（校验会兜底）" @update:model-value="x.key = $event" />
        <FieldText label="标签 label" :model-value="x.label" @update:model-value="x.label = $event" />
        <FieldSelect label="类型" :model-value="x.type" :options="DIM_TYPE_OPTIONS" @update:model-value="setType(x, $event as DimensionType)" />
        <template v-if="x.type === 'number'">
          <div class="grid grid-cols-3 gap-x-4">
            <FieldNum label="最小值" :model-value="x.min ?? 0" @update:model-value="x.min = $event" />
            <FieldNum label="最大值" :model-value="x.max ?? 100" @update:model-value="x.max = $event" />
            <FieldNum label="基线" :model-value="x.baseline ?? 50" hint="默认 50（中心偏移公式 (v-50)/5）" @update:model-value="x.baseline = $event" />
          </div>
        </template>
        <template v-else-if="x.type === 'enum'">
          <div class="mt-2 mb-1 text-[11px] text-muted-foreground/70">候选标签</div>
          <div v-for="(o, i) in x.options ?? []" :key="i" class="mb-1.5 flex items-center gap-1.5">
            <Input class="h-7 w-full flex-1 text-[13px]" :value="o" @change="setOption(x, i, ($event.target as HTMLInputElement).value)" />
            <Button variant="ghost" size="icon-xs" class="size-6 text-destructive hover:bg-destructive/10 hover:text-destructive" title="删除标签" aria-label="删除标签" @click="removeOption(x, i)">
              <IconX />
            </Button>
          </div>
          <Button variant="ghost" size="sm" class="h-7 gap-1 text-xs text-muted-foreground" @click="addOption(x)">
            <IconPlus data-icon="inline-start" />
            标签
          </Button>
        </template>
      </EntityCard>
    </div>
  </div>
</template>
