<script setup lang="ts">
// DocMarkdown —— 叙事字段的只读渲染（极简 Markdown，见 markdown.ts）
// 只读展示用；编辑态仍是 DocText 的纯文本 Textarea。
import { computed } from 'vue'
import { renderMarkdown } from './markdown'

const props = withDefaults(defineProps<{ value?: string }>(), { value: '' })
const html = computed(() => renderMarkdown(props.value))
</script>

<template>
  <div class="md-body text-[13px] leading-[1.75] break-words text-foreground/90" v-html="html" />
</template>
