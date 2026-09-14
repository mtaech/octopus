<script setup lang="ts">
// 故事书卡（规格点 5）：从"纯文字卡"改为"封面卡"。
// 一张卡 = 一个世界：封面纹样 + 首字印鉴 + 设定摘要 + 版次/更新时间；
// 已发布 = 新建游戏（主）/ 编辑器（次）；草稿 = 继续编辑（主），开档不可用（未发布 409）。
// 封面与存档行共享同一种子。
import type { StorybookListItem } from '@/types'
import { relativeTime } from '../utils/relativeTime'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardFooter } from '@/components/ui/card'
import StorybookCover from '@/components/StorybookCover.vue'
import { IconBook2, IconDownload, IconPencil, IconTrash } from '@tabler/icons-vue'

defineProps<{ storybook: StorybookListItem }>()
defineEmits<{
  (e: 'new-game', storybookId: string): void
  (e: 'edit', storybookId: string): void
  (e: 'export', storybookId: string): void
  (e: 'delete', storybookId: string): void
}>()
</script>

<template>
  <Card class="group relative gap-0 overflow-hidden p-0 transition-[transform,box-shadow,border-color] duration-300 hover:-translate-y-1 hover:shadow-lg hover:shadow-primary/10 hover:border-primary/50 focus-within:ring-2 focus-within:ring-ring/60 motion-reduce:transition-none motion-reduce:hover:translate-y-0">
    <div class="relative border-b border-border">
      <StorybookCover :seed="storybook.id" :title="storybook.title" :cover="storybook.cover" size="lg" class="h-28 w-full" />
      <div class="absolute top-3 right-3 flex items-center gap-1.5">
        <!-- 内容评级（P3）：仅徽标，不做过滤 / 年龄验证；SFW 是缺省，不显示角标 -->
        <Badge v-if="storybook.rating === 'nsfw'" variant="outline" class="border-destructive/50 bg-destructive/15 text-[11px] text-destructive backdrop-blur-sm shadow-2xs">
          NSFW
        </Badge>
        <Badge v-if="!storybook.published" variant="outline" class="border-warning/50 bg-warning/15 text-[11px] text-warning backdrop-blur-sm shadow-2xs">
          草稿
        </Badge>
        <Badge variant="outline" class="border-border/80 bg-card/90 font-mono text-[11px] text-muted-foreground backdrop-blur-sm shadow-2xs">
          rev {{ storybook.revision }}
        </Badge>
      </div>
    </div>

    <div class="flex min-h-0 flex-1 flex-col gap-2 px-4 pt-3.5 pb-4">
      <h3
        class="line-clamp-1 font-serif text-[17px] leading-snug font-semibold tracking-wide text-foreground transition-colors group-hover:text-primary"
        :title="storybook.title"
      >
        {{ storybook.title }}
      </h3>
      <p class="line-clamp-2 min-h-[2.6rem] text-[13px] leading-relaxed text-muted-foreground">
        {{ storybook.description || '尚未填写简介。进编辑器补全世界设定、人物与骨架。' }}
      </p>

      <div class="inline-flex items-center gap-1.5 text-[11.5px] text-muted-foreground/80">
        <span class="size-1.5 rounded-full" :class="storybook.published ? 'bg-success' : 'bg-warning'" />
        {{ storybook.published ? '已发布' : '草稿 · 未发布' }} · 更新于 {{ relativeTime(storybook.updated_at) }}
      </div>

      <CardFooter class="mt-auto gap-2 border-t border-border/70 pt-3 pb-0 px-0">
        <template v-if="storybook.published">
          <Button size="sm" class="flex-1 shadow-sm shadow-primary/15" @click="$emit('new-game', storybook.id)">
            <IconBook2 data-icon="inline-start" />
            新建游戏
          </Button>
          <Button size="sm" variant="outline" class="border-border/80 text-muted-foreground hover:border-border hover:text-foreground" @click="$emit('edit', storybook.id)">
            <IconPencil data-icon="inline-start" />
            编辑器
          </Button>
        </template>
        <template v-else>
          <Button size="sm" class="flex-1 shadow-sm shadow-primary/15" @click="$emit('edit', storybook.id)">
            <IconPencil data-icon="inline-start" />
            继续编辑
          </Button>
        </template>
        <Button
          size="sm"
          variant="ghost"
          class="shrink-0 text-muted-foreground hover:bg-primary/10 hover:text-primary"
          title="导出为 .octopus-book.zip（草稿 + 已发布版次 + 全部图片）"
          aria-label="导出故事书"
          @click="$emit('export', storybook.id)"
        >
          <IconDownload class="size-3.5" />
        </Button>
        <Button
          size="sm"
          variant="ghost"
          class="shrink-0 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
          :title="storybook.published ? '删除故事书（已内嵌副本的存档不受影响）' : '删除草稿'"
          @click="$emit('delete', storybook.id)"
        >
          <IconTrash class="size-3.5" />
        </Button>
      </CardFooter>
    </div>
  </Card>
</template>
