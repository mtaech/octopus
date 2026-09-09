<script setup lang="ts">
// FieldNum —— 带标签的数字字段（shadcn Input type=number）
import { Input } from '@/components/ui/input'

withDefaults(defineProps<{
  label: string
  modelValue?: number
  min?: number
  max?: number
  step?: number
  hint?: string
}>(), { modelValue: 0, hint: '', step: 1 })

const emit = defineEmits<{ 'update:modelValue': [value: number] }>()

function onInput(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value)
  emit('update:modelValue', Number.isNaN(v) ? 0 : v)
}
</script>

<template>
  <div class="grid grid-cols-[124px_minmax(0,1fr)] items-start gap-x-4 px-0.5 py-1.5">
    <label class="pt-2 text-xs leading-4 text-muted-foreground">{{ label }}</label>
    <div class="flex min-w-0 flex-col gap-1">
      <Input
        class="h-8 w-full text-[13px]"
        type="number"
        :value="modelValue"
        :min="min"
        :max="max"
        :step="step"
        @input="onInput"
      />
      <p v-if="hint" class="text-[11px] leading-4 text-muted-foreground/70">{{ hint }}</p>
    </div>
  </div>
</template>
