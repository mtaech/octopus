<script setup lang="ts">
// SaveModelPicker —— 单存档「单一 AI」的模型 / 思考强度选择器（无共用 / 分角色分叉）。
// 值形状 = SaveModelChoice；思考档位来自模型目录的 thinkingLevelMap，缺省回落通用档。
import { computed } from 'vue'
import type { ModelEntry, ProviderConfig, SaveModelChoice } from '@/types'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectGroup, SelectItem } from '@/components/ui/select'
import {
  catalogEntries, catalogModelMeta, mergeModelMeta, thinkingLevels, levelToWire, wireToLevel, LEVEL_LABEL,
} from '@/api/model-catalog-utils'

const props = withDefaults(defineProps<{
  modelValue: SaveModelChoice
  providers: ProviderConfig[]
  disabled?: boolean
}>(), { disabled: false })
const emit = defineEmits<{ (e: 'update:modelValue', v: SaveModelChoice): void }>()

/** 该供应商的候选模型：以归档里的自定义清单为准，空了回落到目录条目。 */
const models = computed<{ id: string; name: string; entry?: ModelEntry }[]>(() => {
  const pid = props.modelValue.provider_id
  const list = props.providers.find(x => x.id === pid)?.models ?? []
  const src = list.length ? list : catalogEntries(pid)
  // 归档里的自定义条目常只有 id：显示名回落到目录快照，避免下拉里全是裸 id。
  return src.map(m => ({ id: m.id, name: m.name || catalogModelMeta(pid, m.id)?.name || m.id, entry: m }))
})

function metaOf(providerId: string, modelId: string) {
  if (!providerId || !modelId) return undefined
  const entry = props.providers.find(p => p.id === providerId)?.models.find(m => m.id === modelId)
  return mergeModelMeta(catalogModelMeta(providerId, modelId), entry)
}
const meta = computed(() => metaOf(props.modelValue.provider_id, props.modelValue.model))
const levels = computed(() => thinkingLevels(meta.value))
const currentLevel = computed(() => wireToLevel(meta.value, props.modelValue.reasoning_effort))
const supportsReasoning = computed(() => meta.value?.reasoning !== false)

function patch(v: Partial<SaveModelChoice>) {
  emit('update:modelValue', { ...props.modelValue, ...v })
}
function onProvider(id: string) {
  const first = props.providers.find(p => p.id === id)?.models?.[0]?.id ?? catalogEntries(id)[0]?.id ?? ''
  patch({ provider_id: id, model: first, reasoning_effort: undefined })
}
function onModel(id: string) {
  // 换模型后，原思考档位若不在新模型支持的档里，回落「默认」。
  const next = metaOf(props.modelValue.provider_id, id)
  const lv = wireToLevel(next, props.modelValue.reasoning_effort)
  patch({ model: id, reasoning_effort: thinkingLevels(next).includes(lv) ? props.modelValue.reasoning_effort : undefined })
}
function onLevel(level: string) {
  patch({ reasoning_effort: levelToWire(meta.value, level) })
}
</script>

<template>
  <div class="space-y-3">
    <div class="flex items-center justify-between gap-6">
      <div class="min-w-0">
        <div class="text-[13px] font-semibold">供应商</div>
        <div class="mt-0.5 text-xs text-muted-foreground">本存档使用的运行后端</div>
      </div>
      <div class="w-52 shrink-0">
        <Select :model-value="modelValue.provider_id" :disabled="disabled" @update:model-value="(v) => typeof v === 'string' && onProvider(v)">
          <SelectTrigger class="w-full"><SelectValue placeholder="选择供应商" /></SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem v-for="p in providers" :key="p.id" :value="p.id">{{ p.label }}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </div>
    </div>

    <div class="flex items-center justify-between gap-6">
      <div class="min-w-0">
        <div class="text-[13px] font-semibold">模型</div>
        <div class="mt-0.5 text-xs text-muted-foreground">单一 AI 调用的具体模型</div>
      </div>
      <div class="w-52 shrink-0">
        <Select :model-value="modelValue.model" :disabled="disabled" @update:model-value="(v) => typeof v === 'string' && onModel(v)">
          <SelectTrigger class="w-full"><SelectValue placeholder="选择模型" /></SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem v-for="m in models" :key="m.id" :value="m.id">{{ m.name }}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </div>
    </div>

    <div v-if="supportsReasoning" class="flex items-center justify-between gap-6">
      <div class="min-w-0">
        <div class="text-[13px] font-semibold">思考强度</div>
        <div class="mt-0.5 text-xs text-muted-foreground">reasoning_effort：缺省用供应商默认</div>
      </div>
      <div class="w-52 shrink-0">
        <Select :model-value="currentLevel" :disabled="disabled" @update:model-value="(v) => typeof v === 'string' && onLevel(v)">
          <SelectTrigger class="w-full"><SelectValue placeholder="默认" /></SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem v-for="lv in levels" :key="lv" :value="lv">{{ LEVEL_LABEL[lv] ?? lv }}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </div>
    </div>
  </div>
</template>
