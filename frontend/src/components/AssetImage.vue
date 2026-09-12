<script setup lang="ts">
// AssetImage —— 渲染资产引用（#28）。无图时把控制权交回调用方（插槽），
// 以便各处回落到自己的兜底呈现（如 CSS 生成的故事书封面、首字头像）。
import { computed } from 'vue'
import { assetUrl } from '@/api'
import type { AssetRef } from '@/types'

const props = withDefaults(defineProps<{
  value?: AssetRef | null
  alt?: string
  /** 图片填充方式 */
  fit?: 'cover' | 'contain'
}>(), { value: null, alt: '', fit: 'cover' })

const src = computed(() => (props.value?.asset ? assetUrl(props.value.asset) : ''))
</script>

<template>
  <img
    v-if="src"
    :src="src"
    :alt="alt"
    :class="fit === 'contain' ? 'object-contain' : 'object-cover'"
    class="size-full"
    loading="lazy"
    decoding="async"
  />
  <slot v-else />
</template>
