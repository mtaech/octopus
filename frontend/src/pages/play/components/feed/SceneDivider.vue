<script setup lang="ts">
// 场景分隔：有标题才画横线；无标题退化为纯间距，不渲染「两条孤线夹空白」。
import type { FeedEntry } from '../../stores/play'

type SceneEntry = Extract<FeedEntry, { kind: 'scene' }>

defineProps<{ entry: SceneEntry }>()
</script>

<template>
  <div class="my-3 flex flex-col items-center gap-1.5 py-1">
    <div v-if="entry.title" class="flex w-full items-center gap-3">
      <span class="h-px flex-1 bg-border"></span>
      <span class="font-serif text-[13.5px] font-bold tracking-[0.2em] text-foreground uppercase px-2">
        {{ entry.title }}
      </span>
      <span class="h-px flex-1 bg-border"></span>
    </div>
    <p v-if="entry.description" class="max-w-md text-center text-xs text-muted-foreground/75 italic">
      {{ entry.description }}
    </p>
  </div>
</template>
