<script setup lang="ts">
// FieldText —— 标签在上的单行文本字段（A 工作台统一样式；列宽由 FieldGrid 决定）
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'

withDefaults(defineProps<{
  label: string
  modelValue?: string
  placeholder?: string
  hint?: string
  mono?: boolean
  /** full = 跨整行 */
  span?: 'default' | 'full'
  /** 标签与输入框间距收紧一档 */
  dense?: boolean
}>(), { modelValue: '', placeholder: '', hint: '', mono: false, span: 'default', dense: false })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
</script>

<template>
  <label class="flex min-w-0 flex-col" :class="[dense ? 'gap-1' : 'gap-1.5', span === 'full' && 'col-span-full']">
    <span class="text-[11px] leading-4 font-medium text-muted-foreground">{{ label }}</span>
    <Input
      class="h-8 w-full text-[13px]"
      :class="cn(mono && 'font-mono text-xs')"
      :model-value="modelValue"
      :placeholder="placeholder"
      @update:model-value="emit('update:modelValue', String($event))"
    />
    <span v-if="hint" class="text-[10.5px] leading-4 text-muted-foreground/65">{{ hint }}</span>
  </label>
</template>
