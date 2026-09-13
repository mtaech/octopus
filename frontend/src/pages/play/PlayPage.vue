<script setup lang="ts">
// ============================================================
// 游玩页 /play/:saveId（蓝图 #08/#17/#18/#21/#24）
// 单一演出模板（A 聊天流）：左栏世界 / 中列演出 / 右栏详情。
// 免确认为引擎侧开关，点击即发元指令 /免确认，本地态由 system 事件回显纠正（#17）。
// ============================================================
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePlayStore } from './stores/play'
import { useDrawerStore } from './stores/drawer'
import FeedChat from './components/feed/FeedChat.vue'
import FlowLog from './components/FlowLog.vue'
import WorldRail from './components/WorldRail.vue'
import DetailPanel from './components/DetailPanel.vue'
import type { WorldSelection } from './selection'
import SaveDrawer from './components/SaveDrawer.vue'
import InputBar from './components/InputBar.vue'
import NarrativePrefsDialog from './components/NarrativePrefsDialog.vue'
import ThemeToggle from '@/components/ThemeToggle.vue'
import StorybookCover from '@/components/StorybookCover.vue'
import type { AssetRef } from '@/types'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Alert, AlertAction, AlertDescription } from '@/components/ui/alert'
import { Separator } from '@/components/ui/separator'
import {
  IconArrowLeft,
  IconDeviceFloppy,
  IconX,
  IconAlertTriangle,
  IconRefresh,
  IconDeviceGamepad2,
  IconPencil,
  IconArrowDown,
  IconAdjustments,
} from '@tabler/icons-vue'

const route = useRoute()
const router = useRouter()
const store = usePlayStore()
const drawer = useDrawerStore()

// ---------- 左栏世界列表 / 右栏详情 ----------
const selection = ref<WorldSelection | null>(null)

// ---------- 顶栏派生 ----------
const sceneTitle = computed(() => store.sceneTitle)
const needsUpgrade = computed(() => store.projection?.meta.needs_upgrade ?? false)
const rev = computed(() => store.projection?.meta.revision)
const storybookTitle = computed(() => store.projection?.meta.storybook_title ?? store.detail?.storybook_title ?? '')
const saveTitle = computed(() => store.projection?.meta.save_title ?? '')
const showBanner = computed(() => needsUpgrade.value && !drawer.bannerDismissed)
const autoConfirm = computed(() => store.autoConfirm)
const phaseChip = computed(() => store.phaseLabel)
const isSandbox = computed(() => Boolean(store.detail?.is_sandbox || store.detail?.title?.startsWith('【沙箱试玩】')))
const storybookId = computed(() => store.detail?.storybook_id)
/** 是否有作者开放给玩家的叙述段：没有就不显示「叙述偏好」入口，避免死按钮 */
const hasNarrativePrefs = computed(() => {
  const sb = store.detail?.storybook as { narrative?: { sections?: { playerEditable?: boolean }[] } } | undefined
  return (sb?.narrative?.sections ?? []).some(s => s.playerEditable === true)
})
const narrativeOpen = ref(false)
/** 顶栏封面取自存档内嵌的冻结故事书（发布时冻结，游玩中不随草稿变动） */
const cover = computed(() => {
  const sb = store.detail?.storybook as { meta?: { cover?: AssetRef } } | undefined
  return sb?.meta?.cover ?? null
})

function goEditStorybook() {
  if (storybookId.value) {
    void router.push(`/storybook/${storybookId.value}/edit`)
  }
}

// ---------- 免确认开关：直调存档级设置端点（#24 修订，状态类元指令结构化） ----------
const togglingConfirm = ref(false)
async function toggleAutoConfirm() {
  if (store.busy || togglingConfirm.value) return
  togglingConfirm.value = true
  try { await store.setAutoConfirm(!autoConfirm.value) } finally { togglingConfirm.value = false }
}

// ---------- 对话 / 流程日志 切换 ----------
// 「日志」保留全部原始事件（含被驳回的意图），用于在游玩时复盘整个流程。
const TAB_KEY = 'octopus:play-tab'
const tab = ref<'chat' | 'log'>(localStorage.getItem(TAB_KEY) === 'log' ? 'log' : 'chat')
async function switchTab(next: 'chat' | 'log') {
  tab.value = next
  try { localStorage.setItem(TAB_KEY, next) } catch { /* 隐私模式忽略 */ }
  await nextTick()
  atBottom.value = true
  void scrollToBottom(false, true)
}

// ---------- 滚动行为（P1-4）：上滚时暂停自动滚底，可选回到底部 ----------
const feedRoot = ref<HTMLElement | null>(null)
const atBottom = ref(true)
const NEAR_BOTTOM_PX = 96
function computeAtBottom() {
  const el = feedRoot.value
  if (!el) return
  atBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX
}
async function scrollToBottom(smooth = false, force = false) {
  await nextTick()
  const el = feedRoot.value
  if (!el) return
  if (!force && !atBottom.value) return
  el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
  atBottom.value = true
}
watch(() => [store.revealPulse, store.entries.length], () => {
  if (store.loadingOlder) return
  void scrollToBottom()
})
watch(() => store.phase, () => { if (store.phase === 'idle') void scrollToBottom(true) })
function onFeedScroll() { computeAtBottom() }
function jumpToBottom() { void scrollToBottom(true, true) }

// ---------- 贴底保持 ----------
// 卡片内容是异步渲染/合并的（markdown、字体、流式片段并进已有卡片、instant 推送不产生 revealPulse），
// 单次 nextTick 的 scrollTo 会落空，最后一张卡（含「重发 / 编辑 / 重跑本轮」那行）就被输入栏压住。
// 所以在「内容变化」与「可视区变化」之后，只要玩家本来在底部，就重新贴底。
let feedObserver: MutationObserver | null = null
let sizeObserver: ResizeObserver | null = null
let pinFrame = 0
function pinToBottomIfNeeded(): void {
  const el = feedRoot.value
  if (!el || !atBottom.value) return
  // 同步贴底（读取 scrollHeight 会强制布局，拿到的是最新高度）；
  // 再补一个宏任务兜底，覆盖字体 / markdown 二次回流导致的再增长。
  el.scrollTop = el.scrollHeight
  if (pinFrame) return
  pinFrame = window.setTimeout(() => {
    pinFrame = 0
    const cur = feedRoot.value
    if (cur && atBottom.value) cur.scrollTop = cur.scrollHeight
  }, 0)
}
function startBottomPin(): void {
  const el = feedRoot.value
  if (!el) return
  feedObserver?.disconnect()
  sizeObserver?.disconnect()
  feedObserver = new MutationObserver(pinToBottomIfNeeded)
  feedObserver.observe(el, { childList: true, subtree: true, characterData: true })
  if (typeof ResizeObserver !== 'undefined') {
    sizeObserver = new ResizeObserver(pinToBottomIfNeeded)
    sizeObserver.observe(el)
  }
}

/** 加载更早历史并保持视口锚点（前插内容不跳位）。 */
async function loadOlder() {
  const el = feedRoot.value
  const prevHeight = el?.scrollHeight ?? 0
  const prevTop = el?.scrollTop ?? 0
  await store.loadOlder()
  await nextTick()
  if (el) el.scrollTop = prevTop + (el.scrollHeight - prevHeight)
  computeAtBottom()
}

// ---------- 生命周期 ----------
const saveIdParam = computed(() => String(route.params.saveId ?? ''))
let stopWatchRoute: (() => void) | null = null
async function boot() {
  if (!saveIdParam.value) return
  drawer.bannerDismissed = false
  await store.init(saveIdParam.value)
  if (store.error) return
  drawer.closeDrawer()
}
onMounted(async () => {
  await boot()
  await nextTick()
  startBottomPin()
  stopWatchRoute = watch(saveIdParam, async () => {
    await boot()
    await nextTick()
    startBottomPin()
  })
})
onUnmounted(() => {
  stopWatchRoute?.()
  store.teardown()
  feedObserver?.disconnect()
  sizeObserver?.disconnect()
  if (pinFrame) clearTimeout(pinFrame)
})

// 全局空格 = 跳过打字机 / 快进（不拦截输入框内空格）
function globalKey(e: KeyboardEvent) {
  const tag = (e.target as HTMLElement | null)?.tagName
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
  if (e.code === 'Space' || e.key === ' ') {
    e.preventDefault()
    store.emitSkip()
  }
}
onMounted(() => window.addEventListener('keydown', globalKey))
onUnmounted(() => window.removeEventListener('keydown', globalKey))

function goBack() { void router.push('/') }
</script>

<template>
  <div class="flex h-dvh flex-col overflow-hidden bg-background text-foreground">
    <!-- ============ 顶栏 ============ -->
    <header class="z-20 flex h-13 shrink-0 items-center gap-2.5 border-b border-border bg-card/90 px-3.5 backdrop-blur-md">
      <Button variant="ghost" size="icon-sm" class="size-8 text-muted-foreground hover:text-foreground" title="返回故事书列表" @click="goBack()">
        <IconArrowLeft class="size-4" />
      </Button>

      <div class="h-4 w-px bg-border" />

      <!-- 世界封面：与列表 / 编辑器同源 -->
      <StorybookCover
        :seed="storybookId ?? saveIdParam"
        :title="storybookTitle || '存档'"
        :cover="cover"
        size="sm"
        class="size-8 flex-none rounded-lg border border-border/80 shadow-2xs"
      />

      <div class="flex min-w-0 flex-col leading-tight">
        <span class="max-w-64 truncate font-serif text-[14px] font-bold text-foreground" :title="saveTitle">{{ saveTitle || '存档' }}</span>
        <span class="text-[11px] text-muted-foreground/80 whitespace-nowrap">{{ storybookTitle }} · rev {{ rev ?? '–' }}</span>
      </div>

      <span v-if="sceneTitle" class="hidden max-w-56 truncate rounded-full border border-border bg-muted/50 px-2.5 py-0.5 text-xs text-muted-foreground sm:inline-block">
        {{ sceneTitle }}
      </span>

      <!-- 管线阶段徽标（#08 Q6） -->
      <span v-if="phaseChip" class="inline-flex items-center gap-1.5 rounded-full border border-warning/45 bg-warning/15 px-2.5 py-0.5 text-[11px] font-bold whitespace-nowrap text-warning shadow-2xs">
        <span class="size-1.5 animate-pulse rounded-full bg-warning"></span>{{ phaseChip }}
      </span>

      <!-- 沙箱试玩徽标与返回编辑器入口 -->
      <span v-if="isSandbox" class="inline-flex items-center gap-1.5 rounded-full border border-primary/45 bg-primary/15 px-2.5 py-0.5 text-[11px] font-bold whitespace-nowrap text-primary shadow-2xs">
        <IconDeviceGamepad2 class="size-3.5" />
        <span>沙箱试玩</span>
      </span>

      <Button
        v-if="isSandbox && storybookId"
        variant="ghost"
        size="sm"
        class="h-7 gap-1 text-xs text-primary hover:bg-primary/10"
        title="返回当前故事书编辑器继续调整设定"
        @click="goEditStorybook"
      >
        <IconPencil class="size-3.5" />
        <span>返回编辑器</span>
      </Button>

      <div class="flex-1"></div>

      <!-- 主题切换 -->
      <ThemeToggle />

      <Separator orientation="vertical" class="h-5 bg-border" />

      <!-- 免确认开关（引擎侧：点击发 /免确认 元指令） -->
      <label class="flex cursor-pointer items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors" title="免确认：存档级设置，直接调用引擎端点">
        <Switch size="sm" :model-value="autoConfirm" :disabled="store.busy || togglingConfirm" @update:model-value="toggleAutoConfirm" />
        <span class="whitespace-nowrap text-[11.5px]">免确认: {{ autoConfirm ? '开' : '关' }}</span>
      </label>

      <!-- 叙述偏好（叙事契约 P1）：只对故事书 playerEditable 段开放 -->
      <Button
        v-if="hasNarrativePrefs"
        variant="outline"
        size="sm"
        class="gap-1.5 font-medium shadow-xs"
        title="调整作者开放给你的叙述段（开关 / 变体）；只影响之后的回合"
        @click="narrativeOpen = true"
      >
        <IconAdjustments data-icon="inline-start" class="size-3.5" />
        <span>叙述偏好</span>
      </Button>

      <!-- 存档抽屉 -->
      <Button
        variant="outline"
        size="sm"
        class="relative gap-1.5 font-medium shadow-xs"
        :class="{ 'border-warning/60 text-warning bg-warning/10': needsUpgrade }"
        @click="drawer.openDrawer()"
      >
        <IconDeviceFloppy data-icon="inline-start" class="size-3.5" />
        <span>存档</span>
        <span v-if="needsUpgrade" class="absolute -top-1 -right-1 size-2 rounded-full border-2 border-card bg-warning" title="有可升级的新版次"></span>
      </Button>
    </header>

    <!-- ============ 需升级横幅（可关，#21 ② 打开存档提示、不自动） ============ -->
    <Alert v-if="showBanner" class="relative rounded-none border-x-0 border-t-0 border-warning/35 bg-warning/10 py-2.5 pr-14 text-warning" variant="default">
      <IconAlertTriangle class="text-warning size-4" />
      <AlertDescription class="flex items-center gap-3 text-[12.5px]">
        <span>故事书已发布新版次 {{ store.detail?.latest_revision ?? '' }}（当前 rev {{ rev }}）</span>
        <Button size="xs" variant="outline" class="h-6 border-warning/45 text-warning hover:bg-warning/20" @click="drawer.openDrawer(); drawer.enterUpgrade()">查看迁移报告</Button>
      </AlertDescription>
      <AlertAction class="top-2 right-2.5">
        <Button variant="ghost" size="icon-xs" class="text-warning/80 hover:bg-warning/20 hover:text-warning" @click="drawer.dismissBanner()"><IconX class="size-3.5" /></Button>
      </AlertAction>
    </Alert>

    <!-- ============ 主舞台：左栏世界 / 中列演出 / 右栏详情 ============ -->
    <main v-if="store.ready && !store.error" class="flex min-h-0 flex-1 items-stretch">
      <WorldRail :selected="selection" @select="(s: WorldSelection) => (selection = s)" />
      <section class="col-main">
        <!-- 视图切换：对话 / 流程日志（日志含引擎内部事件，如被驳回的意图） -->
        <div class="flex shrink-0 items-center gap-1 border-b border-border/60 bg-card/60 px-2.5 py-1.5">
          <button
            type="button"
            class="cursor-pointer rounded-md px-2.5 py-0.5 text-[11.5px] font-semibold transition-colors"
            :class="tab === 'chat' ? 'bg-primary/15 text-primary' : 'text-muted-foreground hover:text-foreground'"
            @click="switchTab('chat')"
          >对话</button>
          <button
            type="button"
            class="cursor-pointer rounded-md px-2.5 py-0.5 text-[11.5px] font-semibold transition-colors"
            :class="tab === 'log' ? 'bg-primary/15 text-primary' : 'text-muted-foreground hover:text-foreground'"
            title="显示本会话全部原始事件（含被驳回的意图 / 阶段 / 思考 / 状态变更）"
            @click="switchTab('log')"
          >日志<span class="ml-1 font-mono text-[10px] opacity-70">{{ store.flowLog.length }}</span></button>
          <span class="ml-auto truncate text-[10.5px] text-muted-foreground/60">
            {{ tab === 'log' ? '引擎内部事件也在这里' : '' }}
          </span>
        </div>
        <div class="relative flex min-h-0 flex-1 flex-col">
          <div ref="feedRoot" class="feed-scroll" @scroll.passive="onFeedScroll">
            <template v-if="tab === 'chat'">
              <div v-if="store.hasMoreOlder" class="flex justify-center pt-3">
                <Button variant="ghost" size="xs" class="h-6 text-[11px] text-muted-foreground" :disabled="store.loadingOlder" @click="loadOlder">
                  {{ store.loadingOlder ? '加载中…' : '加载更早' }}
                </Button>
              </div>
              <FeedChat :feed="store.entries" />
            </template>
            <FlowLog v-else :lines="store.flowLog" />
          </div>
          <Button
            v-if="!atBottom"
            variant="outline"
            size="icon-sm"
            class="absolute bottom-3 left-1/2 z-10 -translate-x-1/2 rounded-full shadow-lg"
            title="回到底部"
            @click="jumpToBottom"
          >
            <IconArrowDown class="size-4" />
          </Button>
        </div>
        <InputBar />
      </section>
      <DetailPanel :selected="selection" @switch="(id: string) => store.switchTo(id)" />
    </main>

    <!-- 加载/错误态 -->
    <main v-else class="flex flex-1 items-center justify-center">
      <div v-if="store.error" class="flex flex-col items-center gap-3 text-[13px]">
        <div class="text-[15px] font-extrabold text-destructive">无法打开存档</div>
        <div class="text-[12.5px] text-muted-foreground">{{ store.error }}</div>
        <Button variant="outline" @click="goBack()">返回列表</Button>
      </div>
      <div v-else class="flex flex-col items-center gap-3 text-[13px] text-muted-foreground">
        <IconRefresh class="size-5 animate-spin text-primary" />
        <span>水合世界中…</span>
      </div>
    </main>

    <!-- ============ 叙述偏好（叙事契约 P1） ============ -->
    <NarrativePrefsDialog v-model="narrativeOpen" />

    <!-- ============ 存档抽屉 ============ -->
    <SaveDrawer />
  </div>
</template>

<style scoped>
/* 中列填满左右栏之间的空间：不再用 max-width + auto margin 居中，
   否则宽屏会在三栏之间留下大块空白。阅读宽度由 FeedChat 内部限制。 */
.col-main {
  flex: 1; min-width: 0;
  height: 100%; display: flex; flex-direction: column;
  background-color: color-mix(in oklab, var(--card) 30%, transparent);
}
.feed-scroll { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
.feed-scroll > :deep(*) { width: 100%; }
</style>
