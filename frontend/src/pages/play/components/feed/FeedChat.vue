<script setup lang="ts">
// 演出模板 A · 聊天流（#08 ①）：IM 气泡 —— narrate=居中旁白窄行、
// dialogue=演员名+头像色气泡、emote=斜体神态行、scene=场景分隔卡。
import type { FeedEntry } from '../../stores/play'
import StreamItem from '../StreamItem.vue'

defineProps<{ feed: FeedEntry[] }>()
</script>

<template>
  <div class="flex flex-col gap-3.5 px-4 pt-4 pb-6">
    <template v-for="e in feed" :key="e.key">
      <!-- 回合头 / 场景头 -->
      <template v-if="e.kind === 'round'">
        <div class="mt-2 flex flex-wrap items-center gap-2 border-t border-dashed border-border/70 pt-3.5 text-xs text-muted-foreground">
          <span
            class="rounded-full px-2.5 py-0.5 text-[10.5px] font-bold tracking-wide whitespace-nowrap border"
            :class="e.channel === 'meta'
              ? 'border-warning/40 bg-warning/15 text-warning'
              : 'border-primary/40 bg-primary/10 text-primary'"
          >
            {{ e.channel === 'meta' ? '元指令' : '玩家 · ' + (e.actorName ?? '你') }}
          </span>
          <span class="text-[13.5px] font-semibold text-foreground">「{{ e.text }}」</span>
        </div>
      </template>
      <template v-else-if="e.kind === 'scene'">
        <div class="my-3 flex flex-col items-center gap-1.5 py-1">
          <div class="flex w-full items-center gap-3">
            <span class="h-px flex-1 bg-gradient-to-r from-transparent via-primary/30 to-transparent"></span>
            <span class="font-serif text-[13.5px] font-bold tracking-[0.2em] text-foreground uppercase px-2">
              {{ e.title }}
            </span>
            <span class="h-px flex-1 bg-gradient-to-r from-transparent via-primary/30 to-transparent"></span>
          </div>
          <p v-if="e.description" class="text-muted-foreground/75 text-center text-xs max-w-md italic">
            {{ e.description }}
          </p>
        </div>
      </template>
      <StreamItem v-else :entry="e" />
    </template>
  </div>
</template>
