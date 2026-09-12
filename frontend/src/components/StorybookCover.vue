<script setup lang="ts">
// 世界封面（跨页共享：列表 / 编辑器 / 游玩）：纯 CSS 确定性生成 —— 语义色混出的色调 + 无方向性点状细纹 + 首字印鉴。
// 无图片资产；同一本故事书在「继续」卡 / 书架卡 / 存档行三处呈现同一封面，形成世界记忆。
import { computed } from 'vue'
import { assetUrl } from '@/api'
import { worldGlyph, worldPattern, worldTint } from '@/lib/worldTint'
import type { AssetRef } from '@/types'

const props = withDefaults(defineProps<{
  /** 世界种子：故事书用 id，存档用 storybook_id */
  seed: string
  /** 取首字作印鉴 */
  title: string
  /** 真实封面（#28）；缺省时回落为 CSS 生成封面 */
  cover?: AssetRef | null
  size?: 'sm' | 'md' | 'lg' | 'hero'
  class?: string
}>(), { cover: null, size: 'md' })

const coverSrc = computed(() => (props.cover?.asset ? assetUrl(props.cover.asset) : ''))
const tint = computed(() => worldTint(props.seed))
const pattern = computed(() => worldPattern(props.seed))
const glyph = computed(() => worldGlyph(props.title))

const glyphClass = computed(() => ({
  sm: 'text-lg',
  md: 'text-3xl',
  lg: 'text-5xl',
  hero: 'text-6xl',
}[props.size]))
</script>

<template>
  <div
    class="oct-cover"
    :class="props.class"
    :data-pattern="pattern"
    :style="{ '--tint': tint }"
    aria-hidden="true"
  >
    <img v-if="coverSrc" :src="coverSrc" alt="" class="absolute inset-0 size-full object-cover" decoding="async" />
    <span v-else class="oct-cover__glyph" :class="glyphClass">{{ glyph }}</span>
  </div>
</template>

<style scoped>
.oct-cover {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  isolation: isolate;
  color: color-mix(in oklab, var(--tint) 78%, var(--foreground));
  background:
    radial-gradient(130% 130% at 12% -12%, color-mix(in oklab, var(--tint) 38%, transparent), transparent 58%),
    linear-gradient(155deg, color-mix(in oklab, var(--tint) 22%, var(--card)), color-mix(in oklab, var(--tint) 4%, var(--card)));
  box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--tint) 20%, transparent);
}

.oct-cover::before {
  content: '';
  position: absolute;
  inset: 0;
  pointer-events: none;
}
.oct-cover[data-pattern='dots']::before {
  background-image: radial-gradient(color-mix(in oklab, var(--tint) 26%, transparent) 1px, transparent 1.4px);
  background-size: 14px 14px;
  -webkit-mask-image: linear-gradient(200deg, #000, transparent 94%);
  mask-image: linear-gradient(200deg, #000, transparent 94%);
}

.oct-cover__glyph {
  position: relative;
  font-family: var(--font-serif);
  font-weight: 700;
  line-height: 1;
  text-shadow: 0 1px 0 color-mix(in oklab, var(--card) 60%, transparent);
  transition: transform 320ms cubic-bezier(0.16, 1, 0.3, 1);
}
.group:hover .oct-cover__glyph {
  transform: scale(1.08);
}
@media (prefers-reduced-motion: reduce) {
  .oct-cover__glyph,
  .group:hover .oct-cover__glyph {
    transition: none;
    transform: none;
  }
}
</style>