<script setup lang="ts">
// 演出模板 A · 聊天流（#08 ①）：IM 气泡 —— narrate=居中旁白窄行、
// dialogue=演员名+头像色气泡、emote=斜体神态行、scene=场景分隔卡。
import type { FeedEntry } from '../../stores/play'
import StreamItem from '../StreamItem.vue'

defineProps<{ feed: FeedEntry[] }>()
</script>

<template>
  <div class="flex flex-col gap-3 px-4 pt-2.5 pb-4">
    <template v-for="e in feed" :key="e.key">
      <!-- 回合头 / 场景头 -->
      <template v-if="e.kind === 'round'">
        <div class="mt-1.5 flex flex-wrap items-baseline gap-2 border-t border-dashed border-border/60 pt-3.5 text-xs text-muted-foreground" :class="{ 'border-info/30': e.channel === 'meta' }">
          <span class="rounded-full px-2 py-px text-[10.5px] font-extrabold tracking-wide whitespace-nowrap" :class="e.channel === 'meta' ? 'bg-warning/15 text-warning' : 'bg-info/15 text-info'">{{ e.channel === 'meta' ? '元指令' : '你 · ' + (e.actorName ?? '') }}</span>
          <span class="text-[13px] font-semibold text-foreground">「{{ e.text }}」</span>
        </div>
      </template>
      <template v-else-if="e.kind === 'scene'">
        <div class="my-1 flex items-center gap-3">
          <span class="h-px flex-1 bg-gradient-to-r from-transparent via-border to-transparent"></span>
          <span class="text-muted-foreground text-[12.5px] font-extrabold tracking-[2px] whitespace-nowrap">{{ e.title }}</span>
          <span class="h-px flex-1 bg-gradient-to-r from-transparent via-border to-transparent"></span>
        </div>
        <p v-if="e.description" class="text-muted-foreground/70 -mt-0.5 text-center text-xs">{{ e.description }}</p>
      </template>
      <StreamItem v-else :entry="e" />
    </template>
  </div>
</template>
