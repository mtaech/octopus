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
  <Card class="group relative gap-0 overflow-hidden rounded-2xl border-border/75 bg-card/90 p-0 pl-4 backdrop-blur-sm transition-all duration-300 hover:-translate-y-1 hover:border-primary/50 hover:bg-card hover:shadow-[0_16px_40px_-10px_rgba(0,0,0,0.75)] focus-within:ring-2 focus-within:ring-ring/50">
    <!-- 左侧精装书脊色带：带微光与渐变 -->
    <div
      aria-hidden="true"
      class="absolute top-0 bottom-0 left-0 w-2 rounded-l-2xl transition-all duration-300 group-hover:w-2.5"
      :style="{ background: `linear-gradient(180deg, ${tint.from}, ${tint.to})` }"
    />

    <!-- 卡片内层微光边框 -->
    <div aria-hidden="true" class="pointer-events-none absolute inset-0 rounded-2xl ring-1 ring-inset ring-white/[0.04]" />

    <CardHeader class="gap-2.5 px-5 pt-5 pb-3">
      <div class="flex items-start gap-3.5">
        <!-- 典籍首字印鉴 -->
        <span
          aria-hidden="true"
          class="inline-flex size-10 flex-none items-center justify-center rounded-xl font-serif text-lg font-bold text-white shadow-md ring-1 ring-white/10 transition-transform duration-300 group-hover:scale-105"
          :style="{ background: `linear-gradient(135deg, ${tint.from}, ${tint.to})` }"
        >
          {{ glyph }}
        </span>
        <div class="min-w-0 flex-1 pt-0.5">
          <CardTitle class="truncate font-serif text-[17.5px] leading-snug font-semibold tracking-wide text-foreground transition-colors group-hover:text-primary" :title="storybook.title">
            {{ storybook.title }}
          </CardTitle>
          <div class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
            <span class="inline-flex items-center gap-1">
              <span class="size-1.5 rounded-full bg-success/80 shadow-[0_0_6px_var(--success)]" />
              已发布
            </span>
            <span class="text-muted-foreground/40">·</span>
            <span>版次 {{ storybook.revision }}</span>
          </div>
        </div>
      </div>
      <p v-if="storybook.description" class="line-clamp-2 pl-[3.35rem] text-[13px] leading-relaxed text-muted-foreground/85">
        {{ storybook.description }}
      </p>
    </CardHeader>

    <div class="flex items-center justify-between gap-3 px-5 pt-1.5 pb-5 pl-[3.35rem]">
      <Badge variant="outline" class="gap-1 border-border/70 bg-muted/40 font-mono text-[11px] text-muted-foreground/75">
        rev {{ storybook.revision }}
      </Badge>
      <CardFooter class="gap-2 p-0">
        <Button size="sm" class="font-medium shadow-sm transition-all duration-200 group-hover:shadow-primary/20" @click="$emit('new-game', storybook.id)">
          <IconBook2 data-icon="inline-start" />
          新建游戏
        </Button>
        <Button size="sm" variant="ghost" class="text-muted-foreground hover:bg-accent/70 hover:text-foreground" @click="$emit('edit', storybook.id)">
          <IconPencil data-icon="inline-start" />
          进编辑器
        </Button>
      </CardFooter>
    </div>
  </Card>
</template>
