<script setup lang="ts">
// FieldText —— 带标签的单行文本字段（shadcn Input）
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'

withDefaults(defineProps<{
  label: string
  modelValue?: string
  placeholder?: string
  hint?: string
  mono?: boolean
}>(), { modelValue: '', placeholder: '', hint: '', mono: false })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
</script>

<template>
  <div class="grid grid-cols-[124px_minmax(0,1fr)] items-start gap-x-4 px-0.5 py-1.5">
    <label class="pt-2 text-xs leading-4 text-muted-foreground">{{ label }}</label>
    <div class="flex min-w-0 flex-col gap-1">
      <Input
        class="h-8 w-full text-[13px]"
        :class="cn(mono && 'font-mono text-xs')"
        :model-value="modelValue"
        :placeholder="placeholder"
        @update:model-value="emit('update:modelValue', String($event))"
      />
      <p v-if="hint" class="text-[11px] leading-4 text-muted-foreground/70">{{ hint }}</p>
    </div>
  </div>
</template>
