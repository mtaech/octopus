<script setup lang="ts">
// FieldNum —— 标签在上的数字字段
import { Input } from '@/components/ui/input'

withDefaults(defineProps<{
  label: string
  modelValue?: number
  min?: number
  max?: number
  step?: number
  hint?: string
  span?: 'default' | 'full'
}>(), { modelValue: 0, hint: '', step: 1, span: 'default' })

const emit = defineEmits<{ 'update:modelValue': [value: number] }>()

function onInput(v: string | number): void {
  const n = Number(v)
  emit('update:modelValue', Number.isNaN(n) ? 0 : n)
}
</script>

<template>
  <label class="flex min-w-0 flex-col gap-1.5" :class="span === 'full' && 'col-span-full'">
    <span class="text-[11px] leading-4 font-medium text-muted-foreground">{{ label }}</span>
    <Input
      class="h-8 w-full text-[13px]"
      type="number"
      :model-value="modelValue"
      :min="min"
      :max="max"
      :step="step"
      @update:model-value="onInput"
    />
    <span v-if="hint" class="text-[10.5px] leading-4 text-muted-foreground/65">{{ hint }}</span>
  </label>
</template>
