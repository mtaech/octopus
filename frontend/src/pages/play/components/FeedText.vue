<script setup lang="ts">
// 打字机文本条目（#08 Q6）：narrate/dialogue/emote 就地逐字 reveal，
// 点击/空格可跳过。模板 B 用自带说话人列，故支持 showSpeaker=false 去重。
// 迁移：聊天气泡 bg-secondary、旁白 text-muted-foreground、神态 text-warning；
// 头像用 Avatar + AvatarFallback（柔和语义底，角色名散列）。
import { computed } from 'vue'
import { usePlayStore, type FeedEntry } from '../stores/play'
import { nameTintClass, nameBorderClass, initial } from '../utils'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'

const props = withDefaults(defineProps<{
  entry: Extract<FeedEntry, { kind: 'content' }>
  showSpeaker?: boolean
}>(), { showSpeaker: true })

const store = usePlayStore()
const shown = computed(() => props.entry.text.slice(0, props.entry.reveal))
const isStreaming = computed(() => !props.entry.done && props.entry.reveal < props.entry.text.length)
const tint = computed(() => props.entry.actorName ? nameTintClass(props.entry.actorName) : 'bg-secondary text-muted-foreground')
const bubbleAccent = computed(() => props.entry.actorName ? nameBorderClass(props.entry.actorName) : 'border-l-border')

function onTap() {
  if (isStreaming.value) { store.skipEntry(props.entry.key) }
  else { store.emitSkip() }
}
</script>

<template>
  <div class="flex w-full flex-col">
    <!-- dialogue：演员头像 + 气泡（B 模板自带说话人列时隐藏名字） -->
    <template v-if="entry.type === 'dialogue'">
      <div v-if="showSpeaker" class="mb-1 ml-1 flex items-center gap-2">
        <Avatar size="sm" class="size-5 ring-1 ring-border/80">
          <AvatarFallback class="text-[9px] font-extrabold" :class="tint">{{ entry.actorName ? initial(entry.actorName) : '?' }}</AvatarFallback>
        </Avatar>
        <span class="text-[12px] font-semibold text-foreground/90">{{ entry.actorName ?? '角色' }}</span>
      </div>
      <button
        type="button"
        class="text-foreground bg-card/90 hover:bg-card/100 cursor-pointer self-start max-w-[82%] rounded-2xl rounded-tl-xs border border-border/75 border-l-[3.5px] px-4 py-2.5 text-left text-[14px] leading-relaxed shadow-xs transition-colors backdrop-blur-xs"
        :class="[bubbleAccent, { 'ring-1 ring-primary/40': isStreaming }]"
        @click="onTap"
      >
        <span>{{ shown }}</span>
        <span v-if="isStreaming" class="ml-1 inline-block text-primary font-bold animate-pulse">▎</span>
        <span v-if="isStreaming" class="ml-2 text-[10px] text-muted-foreground/50 tracking-wider">点击快进</span>
      </button>
    </template>

    <!-- narrate：旁白窄行（A 居中窄行 / B 纸面段左对齐） -->
    <button
      v-else-if="entry.type === 'narrate'"
      type="button"
      class="text-muted-foreground/90 cursor-pointer self-center max-w-[88%] px-3 py-1.5 text-center font-serif text-[14px] italic leading-relaxed tracking-wide transition-colors hover:text-foreground"
      :class="{ '!self-stretch !max-w-none !text-left !not-italic font-sans': !showSpeaker, 'ring-1 ring-primary/30 rounded-lg': isStreaming }"
      @click="onTap"
    >
      <span>{{ shown }}</span>
      <span v-if="isStreaming" class="ml-1 inline-block text-primary font-bold animate-pulse">▎</span>
    </button>

    <!-- emote：斜体神态行 -->
    <button
      v-else
      type="button"
      class="text-warning/90 cursor-pointer self-center max-w-[88%] px-3 py-1.5 text-center font-serif text-[13.5px] italic leading-relaxed transition-colors hover:text-warning"
      :class="{ '!self-stretch !max-w-none !text-left font-sans': !showSpeaker, 'ring-1 ring-warning/30 rounded-lg': isStreaming }"
      @click="onTap"
    >
      <span v-if="entry.emotion" class="text-warning/80 mr-1.5 inline-flex items-center rounded-full border border-warning/40 bg-warning/10 px-2 py-0.2 text-[10.5px] not-italic font-sans">{{ entry.emotion }}</span>
      <span>{{ shown }}</span>
      <span v-if="isStreaming" class="ml-1 inline-block text-primary font-bold animate-pulse">▎</span>
    </button>
  </div>
</template>