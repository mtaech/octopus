<script setup lang="ts">
// FieldArea —— 标签在上的多行文本域（默认跨整行；叙事字段标注支持 Markdown）
import { Textarea } from '@/components/ui/textarea'

const props = withDefaults(defineProps<{
  label?: string
  modelValue?: string
  placeholder?: string
  rows?: number
  hint?: string
  /** 叙事字段：附加「支持 Markdown」说明（#01/#22 决议 12） */
  md?: boolean
  span?: 'default' | 'full'
  /** 标签与文本域间距收紧一档 */
  dense?: boolean
}>(), { label: '', modelValue: '', placeholder: '', rows: 3, hint: '', md: false, span: 'full', dense: false })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const minH = (props.rows ?? 3) * 22 + 14
</script>

<template>
  <label v-if="label" class="flex min-w-0 flex-col" :class="[dense ? 'gap-1' : 'gap-1.5', span === 'full' && 'col-span-full']">
    <span class="text-[11px] leading-4 font-medium text-muted-foreground">{{ label }}</span>
    <Textarea
      :model-value="modelValue"
      :placeholder="placeholder"
      :style="{ minHeight: minH + 'px' }"
      spellcheck="false"
      class="w-full resize-none text-[13px] leading-relaxed"
      @update:model-value="emit('update:modelValue', String($event))"
    />
    <span v-if="hint || md" class="text-[10.5px] leading-4 text-muted-foreground/65">
      <span v-if="hint">{{ hint }}</span>
      <span v-if="hint && md" class="text-muted-foreground/40"> · </span>
      <span v-if="md">支持 Markdown</span>
    </span>
  </label>
  <div v-else class="flex min-w-0 flex-col" :class="[dense ? 'gap-1' : 'gap-1.5', span === 'full' && 'col-span-full']">
    <Textarea
      :model-value="modelValue"
      :placeholder="placeholder"
      :style="{ minHeight: minH + 'px' }"
      spellcheck="false"
      class="w-full resize-none text-[13px] leading-relaxed"
      @update:model-value="emit('update:modelValue', String($event))"
    />
  </div>
</template>
