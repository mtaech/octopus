<script setup lang="ts">
// 演出 · 合并叙事流（#08 三模板里的纸面/剧本路线，取代此前的 A 聊天流气泡版）。
// 一轮 = 右侧玩家气泡 + 混排正文（台词成气泡、旁白/神态/导演裁定无气泡）+ 机器事件块 + 回合操作。
// 同一说话人的连续台词合并成一个气泡；判定卡/确认门/系统行仍是独立块（机器事件不并进正文）。
import { computed, nextTick, onUnmounted, ref, watch } from 'vue'
import { toast } from '@/api'
import type { FeedEntry } from '../../stores/play'
import { usePlayStore } from '../../stores/play'
import StreamItem from '../StreamItem.vue'
import ProseFlow from './ProseFlow.vue'
import SceneDivider from './SceneDivider.vue'
import { fmtClock } from '../../utils'
import { refKindLabel } from '@/lib/entity-refs'
import { IconRepeat, IconRotateClockwise, IconLoader2, IconPencil, IconCheck, IconCopy } from '@tabler/icons-vue'

const props = defineProps<{ feed: FeedEntry[] }>()
const store = usePlayStore()

type RoundEntry = Extract<FeedEntry, { kind: 'round' }>
type ContentEntry = Extract<FeedEntry, { kind: 'content' }>
type SceneEntry = Extract<FeedEntry, { kind: 'scene' }>
/** 块内部：连续 content 连成一个散文段；场景与机器事件各自独立。 */
type Segment =
  | { kind: 'prose'; key: string; items: ContentEntry[] }
  | { kind: 'scene'; key: string; entry: SceneEntry }
  | { kind: 'block'; key: string; entry: FeedEntry }
/** round = 玩家回合卡；loose = 回合外的散段（开场 / 幕间旁白）。 */
type Block =
  | { kind: 'loose'; key: string; segments: Segment[] }
  | { kind: 'round'; key: string; entry: RoundEntry; segments: Segment[] }

function pushSegment(segs: Segment[], e: FeedEntry) {
  if (e.kind === 'content') {
    const last = segs[segs.length - 1]
    if (last && last.kind === 'prose') { last.items.push(e); return }
    segs.push({ kind: 'prose', key: 'prose:' + e.key, items: [e] })
    return
  }
  if (e.kind === 'scene') {
    segs.push({ kind: 'scene', key: e.key, entry: e })
    return
  }
  segs.push({ kind: 'block', key: e.key, entry: e })
}

/** 扁平事件流 → 块：每个玩家回合吸收其后的条目；首个回合之前的正文成为「回合外散段」。 */
const blocks = computed<Block[]>(() => {
  const out: Block[] = []
  for (const e of props.feed) {
    if (e.kind === 'round') {
      out.push({ kind: 'round', key: e.key, entry: e, segments: [] })
      continue
    }
    const last = out[out.length - 1]
    if (last && last.kind === 'round') { pushSegment(last.segments, e); continue }
    let loose = last && last.kind === 'loose' ? last : null
    if (!loose) {
      loose = { kind: 'loose', key: 'loose:' + e.key, segments: [] }
      out.push(loose)
    }
    pushSegment(loose.segments, e)
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

// ---- 玩家输入行：通道色与文案（你 / 导演 / 元指令） ----
function channelLabel(e: RoundEntry): string {
  if (e.channel === 'meta') return '元指令'
  if (e.channel === 'gm') return '导演'
  return e.actorName ? '你 · ' + e.actorName : '你'
}
function channelBadge(e: RoundEntry): string {
  return e.channel === 'meta'
    ? 'border-warning/40 bg-warning/15 text-warning'
    : e.channel === 'gm'
      ? 'border-info/45 bg-info/10 text-info'
      : 'border-primary/40 bg-primary/10 text-primary'
}
/** 玩家气泡着色：元指令 / 导演 / 角色 三通道各自的边框与浅底 */
function channelBubble(e: RoundEntry): string {
  return e.channel === 'meta'
    ? 'border-warning/35 bg-warning/8'
    : e.channel === 'gm'
      ? 'border-info/35 bg-info/8'
      : 'border-primary/25 bg-primary/8'
}

/** 重发提示：导演回合保持导演通道；角色回合若中途切过受控角色，诚实提示将以当前角色执行
 *（后端 submitRound 不接受 actor 覆盖，见 api/index.ts，故不能假装沿用原角色）。 */
function resendHint(e: RoundEntry): string {
  if (e.channel === 'gm') return '用同样的导演指令再开一个新回合（保留这一回合）'
  const cur = store.controlled?.name
  if (e.actorName && cur && e.actorName !== cur) {
    return `将以当前受控角色「${cur}」重发（原输入属于「${e.actorName}」，保留这一回合）`
  }
  return '用同样的输入再开一个新回合（保留这一回合）'
}
function resend(e: RoundEntry) {
  if (store.busy) return
  // 显式带上原回合通道：否则导演回合会被 send() 的默认值变成角色发言
  void store.send(e.text, e.channel === 'gm' ? 'gm' : undefined)
}
async function rerun() {
  if (store.busy) return
  await store.rerunLastRound()
}

// ---- 复制整轮（条目级 hover 复制行已随气泡退场，改为回合级可见按钮） ----
function roundText(b: Block): string {
  if (b.kind !== 'round') return ''
  const lines = [`${channelLabel(b.entry)}：${b.entry.text}`]
  for (const s of b.segments) {
    if (s.kind === 'prose') for (const it of s.items) lines.push(it.text)
    else if (s.entry.kind === 'system' || s.entry.kind === 'resolution') lines.push(s.entry.text ?? '')
  }
  return lines.filter(Boolean).join('\n\n')
}
async function copyRound(b: Block) {
  try {
    await navigator.clipboard.writeText(roundText(b))
    toast('ok', '已复制本轮')
  } catch {
    toast('warn', '复制失败')
  }
}

// ---- 重跑确认（P0）：重跑会归档旧回合并回滚，不可撤销，故单击只开启确认态 ----
const confirmingKey = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null
function clearConfirmTimer() { if (confirmTimer) { clearTimeout(confirmTimer); confirmTimer = null } }
function armRerun(b: Block) {
  if (b.kind !== 'round' || store.busy) return
  cancelEdit()
  confirmingKey.value = b.key
  clearConfirmTimer()
  confirmTimer = setTimeout(() => { confirmingKey.value = null; confirmTimer = null }, 3000)
  // 接管焦点：被点击的「重跑本轮」随分支切换被移除，不接管焦点会掉到 body（键盘用户失联）。
  // preventScroll：只接管焦点，别让浏览器把阅读位置拽到底部。
  void nextTick(() => document.querySelector<HTMLButtonElement>('[data-confirm-rerun]')?.focus({ preventScroll: true }))
}
/** 显式取消时把焦点还给触发按钮(超时路径不抢焦点，避免打断已经转移注意力的用户) */
function cancelRerun(restoreFocus = false) {
  clearConfirmTimer()
  confirmingKey.value = null
  if (restoreFocus) void nextTick(() => document.querySelector<HTMLButtonElement>('[data-rerun-last]')?.focus({ preventScroll: true }))
}
async function confirmRerun() { clearConfirmTimer(); confirmingKey.value = null; await rerun() }
onUnmounted(clearConfirmTimer)

// ---- 修改后重跑：只有最后一轮可编辑（引擎只能回滚最后一轮） ----
const editingKey = ref<string | null>(null)
const draft = ref('')
function startEdit(b: Block) {
  if (b.kind !== 'round') return
  cancelRerun()
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
// 最后一轮变化（新回合 / 重跑完成）时收起编辑态与确认态，避免残留
watch(lastRoundKey, () => {
  if (editingKey.value && editingKey.value !== lastRoundKey.value) cancelEdit()
  if (confirmingKey.value && confirmingKey.value !== lastRoundKey.value) cancelRerun()
})

const BTN = 'inline-flex cursor-pointer items-center gap-1 rounded-full border border-border/80 px-2.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:border-primary/50 hover:bg-muted/70 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:pointer-events-none disabled:opacity-40'
</script>

<template>
  <div class="mx-auto flex w-full max-w-3xl flex-col gap-3.5 px-4 pt-4 pb-6">
    <template v-for="b in blocks" :key="b.key">
      <!-- 玩家回合：右对齐玩家气泡 + 台词气泡/无气泡正文混排 + 回合操作 -->
      <section v-if="b.kind === 'round'" class="group rounded-2xl border border-border/60 bg-card/35 px-4 py-3.5">
        <!-- 玩家输入：右对齐气泡（通道色保留：你 / 导演 / 元指令） -->
        <div class="flex items-start justify-end gap-2.5">
          <div class="flex min-w-0 flex-col items-end gap-1">
            <span
              class="shrink-0 rounded-full border px-2 py-0.5 text-[10.5px] font-bold tracking-wide whitespace-nowrap"
              :class="channelBadge(b.entry)"
            >
              {{ channelLabel(b.entry) }}
            </span>
            <p
              class="max-w-[62ch] rounded-2xl rounded-br-xs border px-3.5 py-2.5 text-left font-serif text-[14.5px] leading-[1.85] text-foreground/90"
              :class="channelBubble(b.entry)"
            >
              {{ b.entry.text }}
            </p>
            <div class="flex flex-wrap items-center justify-end gap-1.5">
              <span
                v-for="(r, ri) in b.entry.refs ?? []"
                :key="ri"
                class="inline-flex items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-[10.5px] text-primary"
              >
                {{ refKindLabel(r.kind) }} · {{ r.name }}
              </span>
              <span v-if="b.entry.pending" class="inline-flex items-center gap-1 text-[10.5px] text-muted-foreground/70">
                <IconLoader2 class="size-3 animate-spin" />发送中
              </span>
              <span v-if="b.entry.ts" class="text-[10.5px] text-muted-foreground/50">{{ fmtClock(b.entry.ts) }}</span>
            </div>
          </div>
        </div>

        <!-- 回合内容：连续散文段 + 机器事件块 -->
        <div v-if="b.segments.length" class="mt-3 flex flex-col gap-3">
          <template v-for="s in b.segments" :key="s.key">
            <ProseFlow v-if="s.kind === 'prose'" :items="s.items" />
            <SceneDivider v-else-if="s.kind === 'scene'" :entry="s.entry" />
            <StreamItem v-else :entry="s.entry" />
          </template>
        </div>

        <!-- 只有最后一轮常驻操作;历史回合降到 40% 不透明度,悬停/聚焦/触屏恢复,把阅读面还给正文 -->
        <footer
          v-if="!b.entry.pending && b.entry.channel !== 'meta'"
          class="mt-3 flex flex-col gap-2 transition-opacity"
          :class="b.key === lastRoundKey
            ? ''
            : 'opacity-40 group-hover:opacity-100 focus-within:opacity-100 [@media(hover:none)]:opacity-100'"
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
              <button type="button" :class="BTN" :disabled="store.busy" @click="cancelEdit">取消</button>
              <button
                type="button"
                class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-primary/50 bg-primary/10 px-2.5 py-1 text-[11px] font-medium text-primary transition-colors hover:bg-primary/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:pointer-events-none disabled:opacity-40"
                :disabled="store.busy || !draft.trim()"
                title="丢弃本回合演绎，按修改后的输入重跑"
                @click="saveEdit"
              >
                <IconLoader2 v-if="store.busy" class="size-3 animate-spin" />
                <IconCheck v-else class="size-3" />保存并重跑
              </button>
            </div>
          </div>

          <!-- 重跑确认（P0）：重跑不可撤销，单击只开启确认态；文案报出真实代价 -->
          <div
            v-else-if="confirmingKey === b.key"
            class="flex flex-wrap items-center justify-end gap-1.5"
            @keydown.esc="cancelRerun(true)"
          >
            <p class="mr-auto text-[11px] text-muted-foreground">
              将丢弃本轮的旁白与台词，且无法恢复。
            </p>
            <button type="button" :class="BTN" @click="cancelRerun(true)">取消</button>
            <button
              data-confirm-rerun
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-warning/60 bg-warning/15 px-2.5 py-1 text-[11px] font-medium text-warning transition-colors hover:bg-warning/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:pointer-events-none disabled:opacity-40"
              :disabled="store.busy"
              title="丢弃本轮演绎并用同样的输入重新生成"
              @click="confirmRerun"
            >
              <IconRotateClockwise class="size-3" />确认重跑
            </button>
          </div>

          <div v-else class="flex flex-wrap items-center justify-end gap-1.5">
            <!-- 重跑期间画面保留旧内容（不再闪「水合世界中」），所以这里要给一个「确实在跑」的状态 -->
            <span v-if="store.rerunning" class="mr-auto inline-flex items-center gap-1 text-[11px] text-muted-foreground">
              <IconLoader2 class="size-3 animate-spin text-primary" />重跑中，生成完成后原地替换本轮…
            </span>
            <button type="button" :class="BTN" title="复制整轮文本" @click="copyRound(b)">
              <IconCopy class="size-3" />复制
            </button>
            <button type="button" :class="BTN" :disabled="store.busy" :title="resendHint(b.entry)" @click="resend(b.entry)">
              <IconRepeat class="size-3" />重发
            </button>
            <button
              v-if="b.key === lastRoundKey"
              type="button"
              :class="BTN"
              :disabled="store.busy"
              title="修改这次发送的话，然后重跑本轮"
              @click="startEdit(b)"
            >
              <IconPencil class="size-3" />编辑
            </button>
            <button
              v-if="b.key === lastRoundKey"
              data-rerun-last
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 rounded-full border border-warning/45 bg-warning/10 px-2.5 py-1 text-[11px] font-medium text-warning transition-colors hover:bg-warning/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:pointer-events-none disabled:opacity-40"
              :disabled="store.busy"
              title="丢弃这一回合的演绎，用同样的输入重新生成"
              @click="armRerun(b)"
            >
              <IconRotateClockwise class="size-3" />重跑本轮
            </button>
          </div>
        </footer>
      </section>

      <!-- 回合外散段（开场 / 幕间旁白）：与回合内共用同一套合并渲染 -->
      <div v-else class="flex flex-col gap-3">
        <template v-for="s in b.segments" :key="s.key">
          <ProseFlow v-if="s.kind === 'prose'" :items="s.items" />
          <SceneDivider v-else-if="s.kind === 'scene'" :entry="s.entry" />
          <StreamItem v-else :entry="s.entry" />
        </template>
      </div>
    </template>

    <!-- AI 正在输入（P1-3）：管线阶段进行中（正文到达前占位） -->
    <div
      v-if="store.phase === 'story_thinking' || store.phase === 'character_thinking' || store.phase === 'resolving'"
      class="flex items-center gap-2 px-1 text-[12px] text-muted-foreground/70"
    >
      <IconLoader2 class="size-3.5 animate-spin text-primary" />{{ store.phaseLabel }}…
    </div>
  </div>
</template>
