<script setup lang="ts">
// DocText —— 叙事文本字段的可编辑块（Textarea 自适应行高，CSS field-sizing）
// 直接 v-model 进 editor store 草稿（deep watch 驱动 dirty + 自动保存）
// 存储格式 = Markdown（#01/#22 决议 12）：纯文本编辑，只读渲染见 DocMarkdown.vue
import { Textarea } from '@/components/ui/textarea'

const props = withDefaults(defineProps<{
  modelValue: string
  placeholder?: string
  /** 初始最小行数 */
  rows?: number
  /** 暗调小字提示 */
  hint?: string
}>(), { modelValue: '', placeholder: '留空…', rows: 3 })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const minH = (props.rows ?? 3) * 22 + 14
</script>

<template>
  <div class="flex flex-col gap-0.5">
    <Textarea
      class="w-full resize-none rounded-md border-border/70 bg-transparent px-3 py-2.5 text-sm leading-[1.75] text-foreground transition-colors hover:border-border focus:border-ring focus:bg-card"
      :style="{ minHeight: minH + 'px' }"
      :model-value="modelValue"
      :placeholder="placeholder"
      spellcheck="false"
      @update:model-value="emit('update:modelValue', String($event))"
    />
    <p class="mt-0.5 flex items-center gap-1.5 text-[11px] text-muted-foreground/60">
      <span v-if="hint">{{ hint }}</span>
      <span v-if="hint" class="text-muted-foreground/30">·</span>
      <span class="tracking-wide text-muted-foreground/50">支持 Markdown</span>
    </p>
  </div>
</template>
