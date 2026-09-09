<script setup lang="ts">
// 故事书卡（规格点 5）：标题 / 版次 / 发布态 + 常驻「新建游戏 · 进编辑器」
// 精致版：左侧书脊色带（绝对定位，稳）+ 首字徽章 + 设定摘要 + 衬线标题
import { computed } from 'vue'
import type { StorybookListItem } from '@/types'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardFooter, CardHeader, CardTitle } from '@/components/ui/card'
import { IconBook2, IconCircleCheck, IconPencil } from '@tabler/icons-vue'

const props = defineProps<{ storybook: StorybookListItem }>()
defineEmits<{
  (e: 'new-game', storybookId: string): void
  (e: 'edit', storybookId: string): void
}>()

/** 标题首字（去掉常见前缀符号） */
const glyph = computed(() => {
  const t = props.storybook.title.replace(/^[·•\s]+/, '')
  return t.charAt(0) || '书'
})

/** 书脊暖色带：按标题确定性散列到 6 种暖色之一（不刺眼，与主题同族） */
const tint = computed(() => {
  const TINTS = [
    { from: '#e8a44c', to: '#b87a3a' }, // 琥珀
    { from: '#c9a06b', to: '#96714a' }, // 浅棕
    { from: '#a8975f', to: '#7d7047' }, // 枯叶金
    { from: '#b8856f', to: '#8d5f4f' }, // 赤陶
    { from: '#8a9a6f', to: '#6a7a55' }, // 苔绿
    { from: '#7fa0a8', to: '#5f7f88' }, // 青瓷
  ]
  let h = 0
  for (const c of props.storybook.id) h = (h * 31 + c.charCodeAt(0)) | 0
  return TINTS[Math.abs(h) % TINTS.length]
})
</script>

<template>
  <Card class="group relative gap-0 overflow-hidden rounded-2xl border-border/80 p-0 pl-4 transition-all duration-300 hover:-translate-y-1 hover:border-primary/40 hover:shadow-[0_14px_40px_-10px_rgba(0,0,0,0.65)]">
    <!-- 左侧书脊色带：绝对定位钉在卡片左缘，不受内部布局影响 -->
    <div
      aria-hidden="true"
      class="absolute top-0 bottom-0 left-0 w-1.5 rounded-l-2xl"
      :style="{ background: `linear-gradient(180deg, ${tint.from}, ${tint.to})` }"
    />

    <CardHeader class="gap-2.5 px-5 pt-5 pb-3">
      <div class="flex items-start gap-3">
        <span
          aria-hidden="true"
          class="inline-flex size-9 flex-none items-center justify-center rounded-lg font-serif text-lg font-bold text-white/90 shadow-inner"
          :style="{ background: `linear-gradient(135deg, ${tint.from}, ${tint.to})` }"
        >
          {{ glyph }}
        </span>
        <CardTitle class="min-w-0 flex-1 truncate pt-1 font-serif text-[17px] leading-snug font-semibold tracking-wide" :title="storybook.title">
          {{ storybook.title }}
        </CardTitle>
      </div>
      <p v-if="storybook.description" class="line-clamp-2 pl-12 text-[13px] leading-relaxed text-muted-foreground">
        {{ storybook.description }}
      </p>
    </CardHeader>

    <div class="flex items-center justify-between gap-3 px-5 pt-1 pb-5 pl-12">
      <div class="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-xs text-muted-foreground">
        <Badge class="gap-1 border-success/40 bg-success/10 text-success">
          <IconCircleCheck aria-hidden="true" class="size-3" />
          已发布
        </Badge>
        <span>版次 {{ storybook.revision }}</span>
      </div>
      <CardFooter class="gap-2 p-0">
        <Button size="sm" @click="$emit('new-game', storybook.id)">
          <IconBook2 data-icon="inline-start" />
          新建游戏
        </Button>
        <Button size="sm" variant="ghost" class="text-muted-foreground hover:text-foreground" @click="$emit('edit', storybook.id)">
          <IconPencil data-icon="inline-start" />
          进编辑器
        </Button>
      </CardFooter>
    </div>
  </Card>
</template>
