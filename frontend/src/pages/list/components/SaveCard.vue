<script setup lang="ts">
// 存档行（规格点 6）：世界封面缩略 + 标题/角标 + 元信息 + 继续游玩。
// 去掉旧的硬编码 hex 色带，改用共享封面 —— 与故事书卡是同一个世界的同一张封面。
import type { AssetRef, SaveListItem } from '@/types'
import { relativeTime } from '../utils/relativeTime'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import StorybookCover from '@/components/StorybookCover.vue'
import { IconBook2, IconCheck, IconClock, IconDeviceGamepad2, IconPlayerPlayFilled, IconSparkles, IconTrash } from '@tabler/icons-vue'

defineProps<{ save: SaveListItem; cover?: AssetRef | null; selectable?: boolean; selected?: boolean }>()
defineEmits<{
  (e: 'resume', saveId: string): void
  (e: 'delete', saveId: string): void
  (e: 'select', saveId: string): void
}>()
</script>

<template>
  <Card
    :role="selectable ? 'checkbox' : undefined"
    :aria-checked="selectable ? !!selected : undefined"
    :tabindex="selectable ? 0 : undefined"
    :class="[
      'group flex-row items-center gap-0 overflow-hidden p-0 transition-[transform,box-shadow,border-color] duration-300 hover:-translate-y-0.5 hover:shadow-md hover:shadow-primary/10 hover:border-primary/40 focus-within:ring-2 focus-within:ring-ring/60 motion-reduce:transition-none motion-reduce:hover:translate-y-0',
      selectable ? 'cursor-pointer select-none' : '',
      selectable && selected ? 'border-primary/60 bg-primary/[0.04] ring-2 ring-primary/25' : '',
    ]"
    @click="selectable && $emit('select', save.id)"
    @keydown.enter.prevent="selectable && $emit('select', save.id)"
    @keydown.space.prevent="selectable && $emit('select', save.id)"
  >
    <span
      v-if="selectable"
      aria-hidden="true"
      class="ml-3 flex size-5 flex-none items-center justify-center rounded-md border transition-colors"
      :class="selected ? 'border-primary bg-primary text-primary-foreground' : 'border-border/80 bg-background text-transparent'"
    >
      <IconCheck class="size-3.5" />
    </span>
    <StorybookCover
      :seed="save.storybook_id"
      :title="save.storybook_title"
      :cover="cover"
      size="sm"
      class="m-3 size-11 flex-none rounded-lg border border-border shadow-2xs"
    />

    <div class="min-w-0 flex-1 py-3 pr-2">
      <div class="flex flex-wrap items-center gap-2">
        <h3 class="truncate font-serif text-[15.5px] font-semibold text-card-foreground transition-colors group-hover:text-primary" :title="save.title">
          {{ save.title }}
        </h3>
        <Badge v-if="save.is_sandbox || save.title.startsWith('【沙箱试玩】')" class="gap-1 border-primary/40 bg-primary/10 text-[11px] text-primary">
          <IconDeviceGamepad2 aria-hidden="true" class="size-3" />
          沙箱
        </Badge>
        <Badge v-if="save.imported" class="gap-1 border-success/40 bg-success/10 text-[11px] text-success">
          <IconSparkles aria-hidden="true" class="size-3" />
          新导入
        </Badge>
        <Badge v-if="save.needs_upgrade" class="gap-1 border-warning/40 bg-warning/10 text-[11px] text-warning">
          新版次待升级
        </Badge>
      </div>
      <p class="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
        <span class="inline-flex items-center gap-1.5 text-muted-foreground/90">
          <IconBook2 aria-hidden="true" class="size-3.5 text-muted-foreground/60" />
          {{ save.storybook_title }}
        </span>
        <span class="text-muted-foreground/40">·</span>
        <span class="font-mono">
          版次 {{ save.embedded_revision }}<template v-if="save.needs_upgrade"> → <span class="font-semibold text-warning">{{ save.latest_revision }}</span></template>
        </span>
        <span class="text-muted-foreground/40">·</span>
        <span class="inline-flex items-center gap-1">
          <IconClock aria-hidden="true" class="size-3 text-muted-foreground/60" />
          {{ relativeTime(save.last_played_at) }}
        </span>
      </p>
    </div>

    <div v-if="!selectable" class="flex flex-none items-center gap-1.5 self-stretch border-l border-border/70 py-3.5 pr-4 pl-3.5">
      <Button
        size="icon-sm"
        variant="ghost"
        class="text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
        title="删除存档"
        aria-label="删除存档"
        @click="$emit('delete', save.id)"
      >
        <IconTrash class="size-4" />
      </Button>
      <Button
        size="sm"
        variant="outline"
        class="gap-1.5 border-border/80 transition-all duration-300 hover:border-primary/50 group-hover:border-primary/40 group-hover:bg-primary group-hover:text-primary-foreground group-hover:shadow-sm group-hover:shadow-primary/20"
        @click="$emit('resume', save.id)"
      >
        <IconPlayerPlayFilled data-icon="inline-start" class="size-3.5" />
        继续游玩
      </Button>
    </div>
  </Card>
</template>
