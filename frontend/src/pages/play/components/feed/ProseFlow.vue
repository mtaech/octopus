<script setup lang="ts">
// 演出混排：台词走气泡（同一说话人的连续台词合并成一个气泡），旁白 / 神态 / 导演裁定走无气泡正文。
// 头像与名字只出现在气泡开头；气泡内多行台词按行排（store 已用 \n 合并同说话人连续台词），动作行独占一行。
// 夹在同一角色两句台词之间的旁白会被判为该角色的动作并收进气泡（引擎这类旁白不带 actor，只能用结构判断）。
import { computed } from 'vue'
import { assetUrl } from '@/api'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { renderMarkdownInline } from '@/lib/markdown'
import type { FeedEntry } from '../../stores/play'
import { usePlayStore } from '../../stores/play'
import { initial, nameTintClass, portraitOf } from '../../utils'

type ContentEntry = Extract<FeedEntry, { kind: 'content' }>

const props = defineProps<{ items: ContentEntry[] }>()
const store = usePlayStore()

interface BubbleLine {
  key: string
  /** speech = 台词正文；action = 收进气泡的动作行（神态，或被判定为角色动作的旁白） */
  kind: 'speech' | 'action'
  html: string
  actionBefore?: string
  actionAfter?: string
}
type Part =
  | { kind: 'prose'; key: string; tone: 'narrate' | 'emote' | 'story'; html: string; emotion?: string }
  | {
      kind: 'speech'
      key: string
      speaker: string
      portrait: string | null
      tint: string
      initial: string
      lines: BubbleLine[]
    }

/** 说话人名字：仅当是角色的台词时给出（导演裁定 / 无归属台词不算）。 */
function speakerOf(e: ContentEntry): string | null {
  return e.actorId !== '__story__' && e.type === 'dialogue' && e.actorName ? e.actorName : null
}

/**
 * 段落主语是否指向该角色——**引擎没给归属时的兜底判断**（引擎给了归属就直接采信）。
 * 以角色名起句：是；以「她 / 它」起句：可能是（仅当本段只有一个说话人时才认）；
 * 以「他」起句：保守跳过——「他」既可能是玩家也可能是男性角色，认错比不认更糟。
 * 场景描写（「车子拐进枫树街…」）不以人称作主语，自然落选。
 */
function looksLikeActorAction(text: string, speaker: string, singleSpeaker: boolean): boolean {
  const t = text.trim()
  if (!t) return false
  if (t.startsWith(speaker)) return true
  return singleSpeaker && /^[她它]/.test(t)
}

/**
 * 归类规则：
 *  1. 同一说话人的连续台词 → 同一个气泡；
 *  2. 神态（引擎带 actor 的动作）→ 收进气泡内的动作行；
 *  3. **夹在同一角色两句台词之间的旁白 → 判为该角色的动作**，一并收进气泡（引擎这类旁白不带 actor，只能用结构判断：前后都是同一人的台词，才认）。
 *  4. 其余旁白 / 导演裁定 → 无气泡正文段。
 */
const parts = computed<Part[]>(() => {
  const out: Part[] = []
  const items = props.items
  // 本批内容里有几个人在说话：只有一个时才敢用「她 / 它」判断归属
  const speakers = new Set(items.map(speakerOf).filter((s): s is string => !!s))
  const singleSpeaker = speakers.size <= 1
  let run: Extract<Part, { kind: 'speech' }> | null = null
  const flush = () => { if (run) { out.push(run); run = null } }
  for (let i = 0; i < items.length; i++) {
    const it = items[i]
    const isStory = it.actorId === '__story__'
    const isSpeech = !isStory && it.type === 'dialogue' && !!it.actorName

    // 规则 3：旁白夹在同一角色的前后两句台词之间，且**属于该角色** → 判为动作行，收进气泡
    //（保持气泡敞开，让后一句并进来）。纯场景描写与玩家动作不并入。
    // 归属优先看引擎给的事件归属（#17：叙事事件必带 actor）；引擎没给时才用主语启发式兜底。
    if (!isSpeech && !isStory && it.type === 'narrate' && run) {
      const nextSpeaker = i + 1 < items.length ? speakerOf(items[i + 1]) : null
      const attributed = it.actorId && it.actorId !== '__story__' ? (it.actorName ?? null) : null
      const belongs = attributed
        ? attributed === run.speaker
        : looksLikeActorAction(it.text, run.speaker, singleSpeaker)
      if (nextSpeaker && nextSpeaker === run.speaker && belongs) {
        run.lines.push({ key: it.key, kind: 'action', html: renderMarkdownInline(it.text) })
        continue
      }
    }

    if (isSpeech) {
      const speaker = it.actorName as string
      const line: BubbleLine = {
        key: it.key,
        kind: 'speech',
        html: renderMarkdownInline(it.text),
        actionBefore: it.actionBefore,
        actionAfter: it.actionAfter
      }
      if (run && run.speaker === speaker) { run.lines.push(line); continue }
      flush()
      const portrait = portraitOf(store.detail?.storybook, it.actorId, speaker)
      run = {
        kind: 'speech',
        key: 'speech:' + it.key,
        speaker,
        portrait: portrait ? assetUrl(portrait.asset) : null,
        tint: nameTintClass(speaker),
        initial: initial(speaker),
        lines: [line]
      }
      continue
    }
    flush()
    out.push({
      kind: 'prose',
      key: it.key,
      tone: isStory ? 'story' : it.type === 'emote' ? 'emote' : 'narrate',
      html: renderMarkdownInline(it.text),
      emotion: it.emotion
    })
  }
  flush()
  return out
})
</script>

<template>
  <div class="flex flex-col gap-2.5">
    <template v-for="p in parts" :key="p.key">
      <!-- 台词：同一说话人的连续台词合并成一个气泡；头像与名字只出现一次 -->
      <div v-if="p.kind === 'speech'" class="flex items-start gap-2.5">
        <Avatar class="size-9 shrink-0 rounded-xl shadow-2xs ring-1 ring-border/70">
          <img
            v-if="p.portrait"
            :src="p.portrait"
            :alt="p.speaker"
            class="size-full object-cover"
            loading="lazy"
            decoding="async"
          />
          <AvatarFallback v-else class="text-[14px] font-extrabold" :class="p.tint">{{ p.initial }}</AvatarFallback>
        </Avatar>
        <div class="flex min-w-0 flex-col items-start gap-1">
          <span class="text-[12.5px] font-semibold text-foreground/85">{{ p.speaker }}</span>
          <div class="flex max-w-[62ch] flex-col gap-1.5 rounded-2xl rounded-tl-xs border border-border/70 bg-background px-3.5 py-2.5 shadow-2xs">
            <template v-for="line in p.lines" :key="line.key">
              <!-- 角色动作行（神态，或被判定为该角色动作的旁白）：独占一行、斜体 -->
              <p
                v-if="line.kind === 'action'"
                class="font-serif text-[13px] italic leading-relaxed text-muted-foreground/75"
                v-html="line.html"
              ></p>
              <p v-else class="font-serif text-[14.5px] leading-[1.85] text-foreground/90">
                <span v-if="line.actionBefore" class="mb-1 block text-[13px] italic text-muted-foreground/75">{{ line.actionBefore }}</span><span v-html="line.html"></span><span v-if="line.actionAfter" class="mt-1 block text-[13px] italic text-muted-foreground/75">{{ line.actionAfter }}</span>
              </p>
            </template>
          </div>
        </div>
      </div>

      <!-- 旁白 / 情节发展：无气泡，直接成段 -->
      <p
        v-else-if="p.tone === 'narrate'"
        class="font-serif text-[14.5px] leading-[1.9] tracking-normal text-foreground/85"
        v-html="p.html"
      ></p>

      <!-- 神态：无气泡，斜体 + 情绪小标 -->
      <p v-else-if="p.tone === 'emote'" class="font-serif text-[13.5px] italic leading-relaxed text-muted-foreground/85">
        <span v-if="p.emotion" class="mr-1.5 rounded-full border border-warning/40 bg-warning/10 px-1.5 py-px align-middle font-sans text-[10px] not-italic text-warning/90">{{ p.emotion }}</span><span v-html="p.html"></span>
      </p>

      <!-- 导演裁定：无气泡，段首小标 -->
      <p v-else class="font-serif text-[14.5px] leading-[1.9] text-foreground/85">
        <span class="mr-1.5 align-middle font-sans text-[10px] font-bold tracking-wider text-info/80">导演裁定</span><span v-html="p.html"></span>
      </p>
    </template>
  </div>
</template>
