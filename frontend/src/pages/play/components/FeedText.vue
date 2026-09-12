<script setup lang="ts">
// 打字机文本条目（#08 Q6）：narrate/dialogue/emote 就地逐字 reveal，
// 点击/空格可跳过。模板 B 用自带说话人列，故支持 showSpeaker=false 去重。
// 迁移：聊天气泡 bg-secondary、旁白 text-muted-foreground、神态 text-warning；
// 头像用 Avatar + AvatarFallback（柔和语义底，角色名散列）。
// C 面补齐：Markdown 渲染、复制、时间戳（P1-1/2/5）。
import { computed } from 'vue'
import { usePlayStore, type FeedEntry } from '../stores/play'
import { nameTintClass, initial, portraitOf, fmtClock } from '../utils'
import { renderMarkdownInline } from '@/lib/markdown'
import { assetUrl, toast } from '@/api'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { IconCopy, IconBook2 } from '@tabler/icons-vue'

const props = withDefaults(defineProps<{
  entry: Extract<FeedEntry, { kind: 'content' }>
  showSpeaker?: boolean
}>(), { showSpeaker: true })

const store = usePlayStore()
const shown = computed(() => props.entry.text.slice(0, props.entry.reveal))
const html = computed(() => renderMarkdownInline(shown.value))
const isStreaming = computed(() => !props.entry.done && props.entry.reveal < props.entry.text.length)
const tint = computed(() => props.entry.actorName ? nameTintClass(props.entry.actorName) : 'bg-secondary text-muted-foreground')
const portrait = computed(() => portraitOf(store.detail?.storybook, props.entry.actorId, props.entry.actorName))
const clock = computed(() => fmtClock(props.entry.ts))
/** 归属「故事本身」的事件（导演裁定）：单独渲染，不当角色台词 */
const isStory = computed(() => props.entry.actorId === '__story__')

function onTap() {
  if (isStreaming.value) { store.skipEntry(props.entry.key) }
  else { store.emitSkip() }
}

async function copy() {
  try {
    await navigator.clipboard.writeText(props.entry.text)
    toast('ok', '已复制本条')
  } catch {
    toast('warn', '复制失败')
  }
}
</script>

<template>
  <div class="group flex w-full flex-col">
    <!-- 故事裁定（导演通道）：不是谁说的话，是世界本身发生变化 -->
    <div v-if="isStory" class="flex gap-2 rounded-lg border-l-2 border-info/60 bg-info/5 py-2 pr-2.5 pl-2.5">
      <IconBook2 class="mt-0.5 size-3.5 shrink-0 text-info" />
      <div class="min-w-0 flex-1">
        <div class="mb-0.5 text-[10.5px] font-bold tracking-wider text-info uppercase">故事 · 导演裁定</div>
        <div class="text-[13px] leading-relaxed text-foreground/90" v-html="html"></div>
      </div>
    </div>

    <!-- dialogue：演员头像 + 气泡（B 模板自带说话人列时隐藏名字）
         没有归属的台词按旁白渲染——宁可不署名，也不挂一个假名字。 -->
    <template v-else-if="entry.type === 'dialogue' && entry.actorName">
      <!-- IM 式：头像在左、名字与气泡在右 -->
      <div class="flex items-start gap-2.5">
        <Avatar v-if="showSpeaker" class="size-14 shrink-0 rounded-2xl ring-1 ring-border/80 shadow-2xs">
          <img
            v-if="portrait"
            :src="assetUrl(portrait.asset)"
            :alt="entry.actorName ?? ''"
            class="size-full object-cover"
            loading="lazy"
            decoding="async"
          />
          <AvatarFallback v-else class="text-[17px] font-extrabold" :class="tint">{{ entry.actorName ? initial(entry.actorName) : '?' }}</AvatarFallback>
        </Avatar>
        <div class="flex min-w-0 flex-1 flex-col items-start">
          <span v-if="showSpeaker" class="mb-1 ml-1 text-[12px] font-semibold text-foreground/90">{{ entry.actorName ?? '角色' }}</span>
          <button
            type="button"
            class="text-foreground bg-card hover:bg-card/100 cursor-pointer self-start max-w-[82%] rounded-2xl rounded-tl-xs border border-border px-4 py-2.5 text-left text-[14px] leading-relaxed shadow-xs transition-colors backdrop-blur-xs"
            :class="{ 'ring-1 ring-primary/40': isStreaming }"
            @click="onTap"
          >
            <span v-if="entry.actionBefore" class="mb-1.5 block font-serif text-[13px] italic leading-relaxed text-muted-foreground/80">{{ entry.actionBefore }}</span>
            <span v-html="html"></span>
            <span v-if="isStreaming" class="ml-1 inline-block text-primary font-bold animate-pulse">▎</span>
            <span v-if="isStreaming" class="ml-2 text-[10px] text-muted-foreground/50 tracking-wider">点击快进</span>
            <span v-if="entry.actionAfter" class="mt-1.5 block font-serif text-[13px] italic leading-relaxed text-muted-foreground/80">{{ entry.actionAfter }}</span>
          </button>
        </div>
      </div>
    </template>

    <!-- narrate：旁白窄行（A 居中窄行 / B 纸面段左对齐） -->
    <button
      v-else-if="entry.type === 'narrate' || entry.type === 'dialogue'"
      type="button"
      class="text-foreground/85 cursor-pointer self-stretch rounded-lg px-1.5 py-1.5 text-left font-serif text-[14.5px] leading-[1.9] tracking-normal transition-colors hover:text-foreground"
      :class="{ 'font-sans text-[13.5px] leading-relaxed': !showSpeaker, 'ring-1 ring-primary/30': isStreaming }"
      @click="onTap"
    >
      <span v-html="html"></span>
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
      <span v-html="html"></span>
      <span v-if="isStreaming" class="ml-1 inline-block text-primary font-bold animate-pulse">▎</span>
    </button>

    <!-- 消息操作 / 时间戳（悬停显示） -->
    <div
      class="mt-0.5 flex items-center gap-2 px-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100"
      :class="entry.type === 'dialogue' ? (showSpeaker ? 'self-start ml-[4.125rem]' : 'self-start ml-1') : 'self-center'"
    >
      <button type="button" class="inline-flex items-center gap-1 text-[10.5px] text-muted-foreground/60 transition-colors hover:text-foreground" @click="copy">
        <IconCopy class="size-3" />复制
      </button>
      <span v-if="clock" class="text-[10.5px] text-muted-foreground/50">{{ clock }}</span>
    </div>
  </div>
</template>
