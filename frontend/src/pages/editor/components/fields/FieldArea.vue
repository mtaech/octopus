<script setup lang="ts">
// FieldArea —— 带标签的多行文本域（shadcn Textarea，field-sizing 自适应行高）
import { Textarea } from '@/components/ui/textarea'

const props = withDefaults(defineProps<{
  label?: string
  modelValue?: string
  placeholder?: string
  rows?: number
  hint?: string
  /** 叙事字段：附加「支持 Markdown」说明（#01/#22 决议 12） */
  md?: boolean
}>(), { label: '', modelValue: '', placeholder: '', rows: 3, hint: '', md: false })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const minH = (props.rows ?? 3) * 22 + 14
</script>

<template>
  <div class="grid grid-cols-[124px_minmax(0,1fr)] items-start gap-x-4 px-0.5 py-1.5">
    <label v-if="label" class="pt-2 text-xs leading-4 text-muted-foreground">{{ label }}</label>
    <div class="flex min-w-0 flex-col gap-1" :class="label ? '' : 'col-span-2'">
      <Textarea
        :model-value="modelValue"
        :placeholder="placeholder"
        :style="{ minHeight: minH + 'px' }"
        spellcheck="false"
        class="w-full resize-none text-[13px] leading-relaxed"
        @update:model-value="emit('update:modelValue', String($event))"
      />
      <p v-if="hint || md" class="text-[11px] leading-4 text-muted-foreground/70">
        <span v-if="hint">{{ hint }}</span>
        <span v-if="hint && md" class="text-muted-foreground/40"> · </span>
        <span v-if="md" class="text-muted-foreground/60">支持 Markdown</span>
      </p>
    </div>
  </div>
</template>
