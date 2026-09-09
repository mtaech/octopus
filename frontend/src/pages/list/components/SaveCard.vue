<script setup lang="ts">
// 存档卡（规格点 6）：标题 / 故事书名 / 版次 / 时间 / 需升级角标（warn 色）
// + imported「新导入」角标 + 常驻「继续游玩」。
// 精致版：书脊色带（按故事书名散列）、清晰排版节奏、hover 微提升。
import { computed } from 'vue'
import type { SaveListItem } from '@/types'
import { relativeTime } from '../utils/relativeTime'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { IconBook2, IconPlayerPlay, IconSparkles } from '@tabler/icons-vue'

const props = defineProps<{ save: SaveListItem }>()
defineEmits<{ (e: 'resume', saveId: string): void }>()

/** 按故事书名确定性散列暖色（与 StorybookCard 同族但不强求一致） */
const tint = computed(() => {
  const TINTS = ['#e8a44c', '#c9a06b', '#a8975f', '#b8856f', '#8a9a6f', '#7fa0a8']
  let h = 0
  for (const c of props.save.storybook_title) h = (h * 31 + c.charCodeAt(0)) | 0
  return TINTS[Math.abs(h) % TINTS.length]
})
</script>

<template>
  <Card class="group flex-row items-center gap-0 overflow-hidden rounded-xl border-border/80 p-0 transition-all duration-300 hover:-translate-y-0.5 hover:border-primary/40 hover:shadow-[0_10px_30px_-8px_rgba(0,0,0,0.55)]">
    <!-- 左侧色带 -->
    <div aria-hidden="true" class="w-1 flex-none self-stretch" :style="{ background: tint }" />
    <!-- 内容列 -->
    <div class="min-w-0 flex-1 px-5 py-4">
      <div class="flex flex-wrap items-center gap-2">
        <h3 class="truncate font-serif text-[15px] font-semibold text-card-foreground" :title="save.title">
          {{ save.title }}
        </h3>
        <Badge v-if="save.imported" class="gap-1 border-success/40 bg-success/10 text-success">
          <IconSparkles aria-hidden="true" class="size-3" />
          新导入
        </Badge>
        <Badge v-if="save.needs_upgrade" class="gap-1 border-warning/40 bg-warning/10 text-warning">
          新版次待升级
        </Badge>
      </div>
      <p class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
        <span class="inline-flex items-center gap-1.5">
          <IconBook2 aria-hidden="true" class="size-3.5 text-muted-foreground/60" />
          {{ save.storybook_title }}
        </span>
        <span class="text-muted-foreground/50">·</span>
        <span>
          版次 {{ save.embedded_revision }}<template v-if="save.needs_upgrade"> → <span class="text-warning">{{ save.latest_revision }}</span></template>
        </span>
        <span class="text-muted-foreground/50">·</span>
        <span>{{ relativeTime(save.last_played_at) }}</span>
      </p>
    </div>
    <!-- 操作列 -->
    <div class="flex-none py-4 pr-5">
      <Button size="sm" class="transition-all duration-300 group-hover:bg-primary group-hover:text-primary-foreground" variant="outline" @click="$emit('resume', save.id)">
        <IconPlayerPlay data-icon="inline-start" />
        继续游玩
      </Button>
    </div>
  </Card>
</template>
