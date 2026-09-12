<script setup lang="ts">
// 演出模板 A · 聊天流（#08 ①）：IM 气泡 —— narrate=居中旁白窄行、
// dialogue=演员名+头像色气泡、emote=斜态神态行、scene=场景分隔卡。
// 每个玩家回合聚成一张卡片：回合头（玩家输入）+ 回合内容 + 底部回合操作（重发 / 重跑本轮）。
import { computed, ref, watch } from 'vue'
import { toast } from '@/api'
import type { FeedEntry } from '../../stores/play'
import { usePlayStore } from '../../stores/play'
import StreamItem from '../StreamItem.vue'
import { fmtClock } from '../../utils'
import { refKindLabel } from '@/lib/entity-refs'
import { IconRepeat, IconRotateClockwise, IconLoader2, IconPencil, IconCheck } from '@tabler/icons-vue'

const props = defineProps<{ feed: FeedEntry[] }>()
const store = usePlayStore()

type RoundEntry = Extract<FeedEntry, { kind: 'round' }>
type Block =
  | { kind: 'single'; key: string; entry: FeedEntry }
  | { kind: 'round'; key: string; entry: RoundEntry; items: FeedEntry[] }

/** 把扁平事件流按「玩家回合」聚成卡片块：回合头 + 直到下一个回合的条目。 */
const blocks = computed<Block[]>(() => {
  const out: Block[] = []
  for (const e of props.feed) {
    if (e.kind === 'round') {
      out.push({ kind: 'round', key: e.key, entry: e, items: [] })
      continue
    }
    const last = out[out.length - 1]
    if (last && last.kind === 'round') last.items.push(e)
    else out.push({ kind: 'single', key: e.key, entry: e })
  }
  return out
})

/** 最后一个非元指令回合的 key：只有它显示「重跑本轮」（引擎只能回滚最后一轮）。 */
const lastRoundKey = computed(() => {
  for (let i = blocks.value.length - 1; i >= 0; i--) {
    const b = blocks.value[i]
    if (b.kind === 'round' && b.entry.channel !== 'meta') return b.key
  }
  return ''
})

function resend(text: string) {
  if (store.busy) return
  void store.send(text)
}
async function rerun() {
  if (store.busy) return
  await store.rerunLastRound()
}

// ---- 修改后重跑：只有最后一轮可编辑（引擎只能回滚最后一轮） ----
const editingKey = ref<string | null>(null)
const draft = ref('')
function startEdit(b: Block) {
  if (b.kind !== 'round') return
  editingKey.value = b.key
  draft.value = b.entry.text
}
function cancelEdit() {
  editingKey.value = null
  draft.value = ''
}
async function saveEdit(): Promise<void> {
  if (store.busy) return
  const text = draft.value.trim()
  if (!text) { toast('warn', '内容不能为空'); return }
  const ok = await store.rerunLastRound(text)
  if (ok) cancelEdit()
}
// 最后一轮变化（新回合 / 重跑完成）时收起编辑态，避免残留
watch(lastRoundKey, () => { if (editingKey.value && editingKey.value !== lastRoundKey.value) cancelEdit() })
</script>

<template>
  <div class="mx-auto flex w-full max-w-[1024px] flex-col gap-3.5 px-4 pt-4 pb-6">
    <template v-for="b in blocks" :key="b.key">
      <!-- 玩家回合卡：头 + 内容 + 底部操作 -->
      <section
        v-if="b.kind === 'round'"
        class="overflow-hidden rounded-2xl border border-border/70 bg-card/45 shadow-2xs"
      >
        <header class="flex flex-wrap items-center gap-2 border-b border-border/60 bg-muted/25 px-3.5 py-2.5 text-xs text-muted-foreground">
          <span
            class="rounded-full px-2.5 py-0.5 text-[10.5px] font-bold tracking-wide whitespace-nowrap border"
            :class="b.entry.channel === 'meta'
              ? 'border-warning/40 bg-warning/15 text-warning'
              : b.entry.channel === 'gm'
                ? 'border-info/45 bg-info/10 text-info'
                : 'border-primary/40 bg-primary/10 text-primary'"
          >
            {{ b.entry.channel === 'meta' ? '元指令' : b.entry.channel === 'gm' ? '导演' : '玩家 · ' + (b.entry.actorName ?? '你') }}
          </span>
          <span class="text-[13.5px] font-semibold text-foreground">「{{ b.entry.text }}」</span>
          <span v-if="b.entry.refs && b.entry.refs.length" class="inline-flex flex-wrap items-center gap-1">
            <span
              v-for="(r, ri) in b.entry.refs"
              :key="ri"
              class="inline-flex items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-[10.5px] text-primary"
            >
              {{ refKindLabel(r.kind) }} · {{ r.name }}
            </span>
          </span>
          <span v-if="b.entry.pending" class="inline-flex items-center gap-1 text-[10.5px] text-muted-foreground/70">
            <IconLoader2 class="size-3 animate-spin" />发送中
          </span>
          <span v-if="b.entry.ts" class="ml-auto text-[10.5px] text-muted-foreground/50">{{ fmtClock(b.entry.ts) }}</span>
        </header>

        <div class="flex flex-col gap-3 px-3.5 py-3.5">
          <StreamItem v-for="it in b.items" :key="it.key" :entry="it" />
        </div>

        <footer
          v-if="!b.entry.pending && b.entry.channel !== 'meta'"
          class="flex flex-col gap-2 border-t border-border/60 bg-muted/15 px-3 py-2"
        >
          <!-- 编辑这次发送的话 → 保存后丢弃本回合演绎并按新输入重跑 -->
          <div v-if="editingKey === b.key" class="flex flex-col gap-2">
            <p class="text-[10.5px] text-muted-foreground">修改这次发送的话：保存后丢弃本回合演绎，并按新输入重跑。</p>
            <textarea
              v-model="draft"
              rows="2"
              class="w-full resize-y rounded-lg border border-border/70 bg-background px-2.5 py-2 text-[13px] leading-relaxed text-foreground transition-colors placeholder:text-muted-foreground/40 hover:border-border focus-visible:border-primary focus-visible:ring-2 focus-visible:ring-primary/20 focus-visible:outline-none"
              placeholder="修改这次要说的话…"
              @keydown.esc="cancelEdit"
            ></textarea>
            <div class="flex flex-wrap items-center justify-end gap-1.5">
              <button
                type="button"
                class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-border/80 px-2.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:border-primary/50 hover:bg-muted/70 hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
                :disabled="store.busy"
                @click="cancelEdit"
              >
                取消
              </button>
              <button
                type="button"
                class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-primary/50 bg-primary/10 px-2.5 py-1 text-[11px] font-medium text-primary transition-colors hover:bg-primary/20 disabled:pointer-events-none disabled:opacity-40"
                :disabled="store.busy || !draft.trim()"
                title="丢弃本回合演绎，按修改后的输入重跑"
                @click="saveEdit"
              >
                <IconLoader2 v-if="store.busy" class="size-3 animate-spin" />
                <IconCheck v-else class="size-3" />保存并重跑
              </button>
            </div>
          </div>

          <div v-else class="flex flex-wrap items-center justify-end gap-1.5">
            <button
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-border/80 px-2.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:border-primary/50 hover:bg-muted/70 hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
              :disabled="store.busy"
              title="用同样的输入再开一个新回合（保留这一回合）"
              @click="resend(b.entry.text)"
            >
              <IconRepeat class="size-3" />重发
            </button>
            <button
              v-if="b.key === lastRoundKey"
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-border/80 px-2.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:border-primary/50 hover:bg-muted/70 hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
              :disabled="store.busy"
              title="修改这次发送的话，然后重跑本轮"
              @click="startEdit(b)"
            >
              <IconPencil class="size-3" />编辑
            </button>
            <button
              v-if="b.key === lastRoundKey"
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-warning/45 bg-warning/10 px-2.5 py-1 text-[11px] font-medium text-warning transition-colors hover:bg-warning/20 disabled:pointer-events-none disabled:opacity-40"
              :disabled="store.busy"
              title="丢弃这一回合的演绎，用同样的输入重新生成"
              @click="rerun"
            >
              <IconRotateClockwise class="size-3" />重跑本轮
            </button>
          </div>
        </footer>
      </section>

      <!-- 场景分隔卡 -->
      <div v-else-if="b.entry.kind === 'scene'" class="my-3 flex flex-col items-center gap-1.5 py-1">
        <div class="flex w-full items-center gap-3">
          <span class="h-px flex-1 bg-border"></span>
          <span class="font-serif text-[13.5px] font-bold tracking-[0.2em] text-foreground uppercase px-2">
            {{ b.entry.title }}
          </span>
          <span class="h-px flex-1 bg-border"></span>
        </div>
        <p v-if="b.entry.description" class="text-muted-foreground/75 text-center text-xs max-w-md italic">
          {{ b.entry.description }}
        </p>
      </div>

      <StreamItem v-else :entry="b.entry" />
    </template>

    <!-- AI 正在输入（P1-3）：管线阶段进行中且当前没有打字机内容 -->
    <div
      v-if="!store.streamingEntry && (store.phase === 'story_thinking' || store.phase === 'character_thinking' || store.phase === 'resolving')"
      class="flex items-center gap-2 px-1 text-[12px] text-muted-foreground/70"
    >
      <IconLoader2 class="size-3.5 animate-spin text-primary" />{{ store.phaseLabel }}…
    </div>
  </div>
</template>
