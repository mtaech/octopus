<script setup lang="ts">
// 演出模板 B · 剧本式（#08 ①）：浅色纸感；说话人=角色的 script 排版；
// 场景切 = 分隔线；判定/确认门为页边批注式卡片。
// 迁移：纸感用作用域语义 token 换肤（--background/--card/--foreground/--primary 等），
// 子组件（气泡/判定卡/系统行）随纸感自动换肤，不写死 hex。
import type { FeedEntry } from '../../stores/play'
import StreamItem from '../StreamItem.vue'

defineProps<{ feed: FeedEntry[] }>()
const stageDir = (e: Extract<FeedEntry, { kind: 'content' }>) => (e.type === 'narrate' ? '旁白' : e.type === 'emote' ? '神态' : '')
</script>

<template>
  <div class="feed-b">
    <template v-for="e in feed" :key="e.key">
      <!-- 场景切 = 剧本分隔线 -->
      <div v-if="e.kind === 'scene'" class="my-4 flex items-center gap-3">
        <span class="h-0.5 flex-1" style="background: var(--primary)"></span>
        <div class="text-[19px] font-extrabold tracking-[2px] whitespace-nowrap">{{ e.title }}</div>
        <span class="h-0.5 flex-1" style="background: var(--primary)"></span>
      </div>

      <!-- 玩家回合行 -->
      <div v-else-if="e.kind === 'round'" class="my-2.5 pl-[118px] text-[13.5px]" :class="e.channel === 'meta' ? 'text-warning' : 'text-muted-foreground'">
        <span class="font-bold italic">{{ e.channel === 'meta' ? '（元指令）' : e.actorName + '：' }}</span>
        <span class="text-foreground not-italic font-normal">{{ e.text }}</span>
      </div>

      <!-- 剧本对白：说话人列 + 台词列 -->
      <div v-else-if="e.kind === 'content'" class="my-2 grid grid-cols-[118px_1fr] gap-3">
        <div v-if="e.type === 'dialogue'" class="pt-1 text-right text-[13.5px] font-extrabold">{{ e.actorName }}</div>
        <div v-else class="text-muted-foreground pt-1 text-right text-xs italic font-normal">{{ stageDir(e) }}</div>
        <StreamItem :entry="e" :show-speaker="false" />
      </div>

      <!-- 判定 / 确认门 / 系统等：页边批注式卡片 -->
      <div v-else class="my-2.5 ml-[130px]">
        <StreamItem :entry="e" />
      </div>
    </template>
  </div>
</template>

<style scoped>
/* 纸感换肤：仅在 B 子树内改写语义 token —— 子 shadcn 组件经 CSS 变量自动换肤 */
.feed-b {
  min-height: 100%;
  border-radius: var(--radius-sm);
  padding: 10px 22px 26px;
  font-family: Georgia, 'Noto Serif SC', 'Songti SC', serif;
  background: #f6f1e6;
  color: #23252b;
  --background: #f6f1e6;
  --foreground: #23252b;
  --card: #fffdf6;
  --card-foreground: #23252b;
  --secondary: #efe7d6;
  --secondary-foreground: #23252b;
  --muted: #efe7d6;
  --muted-foreground: #7a7466;
  --border: #d8cbb0;
  --input: #d8cbb0;
  --primary: #b45309;
  --primary-foreground: #fff7ea;
  --accent: #e9ddc6;
  --accent-foreground: #23252b;
  --destructive: #b3352b;
  --success: #3f7a33;
  --warning: #a16207;
  --info: #4a6a8a;
  --popover: #fffdf6;
  --popover-foreground: #23252b;
}
</style>
