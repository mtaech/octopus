<script setup lang="ts">
// ============================================================
// 游玩页 /play/:saveId（蓝图 #08/#17/#18/#21/#24）
// 三演出模板（A 聊天流 / B 剧本式 / C 沉浸式）共用同一演出流与 store，
// 模板切换纯前端、不重置状态（#08 ① / #18 ①）；免确认为引擎侧开关，点击即发
// 元指令 /免确认，本地态由 system 事件回显纠正（#17）。
// 迁移：顶栏 Button/Tabs+Switch，升级横幅 <Alert>，布局 Tailwind 语义类。
// ============================================================
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePlayStore } from './stores/play'
import { useDrawerStore } from './stores/drawer'
import FeedChat from './components/feed/FeedChat.vue'
import FeedScript from './components/feed/FeedScript.vue'
import FeedImmersive from './components/feed/FeedImmersive.vue'
import ActorsPanel from './components/ActorsPanel.vue'
import SaveDrawer from './components/SaveDrawer.vue'
import InputBar from './components/InputBar.vue'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Alert, AlertAction, AlertDescription } from '@/components/ui/alert'
import { Separator } from '@/components/ui/separator'
import {
  IconArrowLeft,
  IconDeviceFloppy,
  IconX,
  IconAlertTriangle,
  IconRefresh,
  IconMessageDots,
  IconFileText,
  IconSparkles,
} from '@tabler/icons-vue'

const route = useRoute()
const router = useRouter()
const store = usePlayStore()
const drawer = useDrawerStore()

// ---------- 模板偏好（#08 ① localStorage 记忆，默认 A） ----------
type TplId = 'A' | 'B' | 'C'
const TPL_KEY = 'octopus.play.template'
const TEMPLATES: { id: TplId; label: string; short: string; desc: string; icon: any }[] = [
  { id: 'A', label: '聊天流', short: 'IM', desc: '对话气泡', icon: IconMessageDots },
  { id: 'B', label: '剧本式', short: 'SCR', desc: '纸面脚本', icon: IconFileText },
  { id: 'C', label: '沉浸式', short: 'VN', desc: '逐行演出', icon: IconSparkles }
]
const tpl = ref<TplId>('A')
function pickTpl(id: TplId) {
  tpl.value = id
  try { localStorage.setItem(TPL_KEY, id) } catch { /* noop */ }
}
function initTpl() {
  try {
    const saved = localStorage.getItem(TPL_KEY) as TplId | null
    if (saved && TEMPLATES.some(t => t.id === saved)) tpl.value = saved
  } catch { /* noop */ }
}

// ---------- 顶栏派生 ----------
const sceneTitle = computed(() => store.sceneTitle)
const needsUpgrade = computed(() => store.projection?.meta.needs_upgrade ?? false)
const rev = computed(() => store.projection?.meta.revision)
const storybookTitle = computed(() => store.projection?.meta.storybook_title ?? store.detail?.storybook_title ?? '')
const saveTitle = computed(() => store.projection?.meta.save_title ?? '')
const showBanner = computed(() => needsUpgrade.value && !drawer.bannerDismissed)
const autoConfirm = computed(() => store.autoConfirm)
const phaseChip = computed(() => store.phaseLabel)

// ---------- 免确认开关：直调存档级设置端点（#24 修订，状态类元指令结构化） ----------
const togglingConfirm = ref(false)
async function toggleAutoConfirm() {
  if (store.busy || togglingConfirm.value) return
  togglingConfirm.value = true
  try { await store.setAutoConfirm(!autoConfirm.value) } finally { togglingConfirm.value = false }
}

// ---------- 自动滚动 ----------
const feedRoot = ref<HTMLElement | null>(null)
async function scrollToBottom(smooth = false) {
  await nextTick()
  const el = feedRoot.value
  if (el) el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
}
watch(() => [store.revealPulse, store.entries.length, tpl.value], () => void scrollToBottom())
watch(() => store.phase, () => { if (store.phase === 'idle') void scrollToBottom(true) })

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
  initTpl()
  await boot()
  stopWatchRoute = watch(saveIdParam, () => { void boot() })
})
onUnmounted(() => { stopWatchRoute?.(); store.teardown() })

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
  <div class="flex h-screen flex-col overflow-hidden bg-background text-foreground">
    <!-- ============ 顶栏 ============ -->
    <header class="z-20 flex h-13 shrink-0 items-center gap-2.5 border-b border-border/80 bg-card/85 px-3.5 backdrop-blur-md">
      <Button variant="ghost" size="icon-sm" class="size-8 text-muted-foreground hover:text-foreground" title="返回故事书列表" @click="goBack()">
        <IconArrowLeft class="size-4" />
      </Button>

      <div class="h-4 w-px bg-border/60" />

      <div class="flex min-w-0 flex-col leading-tight">
        <span class="max-w-64 truncate font-serif text-[14px] font-bold text-foreground" :title="saveTitle">{{ saveTitle || '存档' }}</span>
        <span class="text-[11px] text-muted-foreground/80 whitespace-nowrap">{{ storybookTitle }} · rev {{ rev ?? '–' }}</span>
      </div>

      <span v-if="sceneTitle" class="hidden max-w-56 truncate rounded-full border border-border/70 bg-muted/40 px-2.5 py-0.5 text-xs text-muted-foreground sm:inline-block">
        {{ sceneTitle }}
      </span>

      <!-- 管线阶段徽标（#08 Q6） -->
      <span v-if="phaseChip" class="inline-flex items-center gap-1.5 rounded-full border border-warning/45 bg-warning/15 px-2.5 py-0.5 text-[11px] font-bold whitespace-nowrap text-warning shadow-2xs">
        <span class="size-1.5 animate-pulse rounded-full bg-warning"></span>{{ phaseChip }}
      </span>

      <div class="flex-1"></div>

      <!-- 🎭 模板切换（segmented Tabs） -->
      <Tabs :model-value="tpl" @update:model-value="(v: unknown) => pickTpl(v as TplId)">
        <TabsList class="h-8 gap-0.5 rounded-lg border border-border/80 bg-background/80 p-0.5 shadow-inner">
          <TabsTrigger v-for="t in TEMPLATES" :key="t.id" :value="t.id" class="h-7 gap-1.5 px-2.5 text-xs font-medium" :title="t.label + ' · ' + t.desc">
            <component :is="t.icon" class="size-3.5" />
            <span>{{ t.label }}</span>
          </TabsTrigger>
        </TabsList>
      </Tabs>

      <Separator orientation="vertical" class="h-5 bg-border/60" />

      <!-- 免确认开关（引擎侧：点击发 /免确认 元指令） -->
      <label class="flex cursor-pointer items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors" title="免确认：存档级设置，直接调用引擎端点">
        <Switch size="sm" :model-value="autoConfirm" :disabled="store.busy || togglingConfirm" @update:model-value="toggleAutoConfirm" />
        <span class="whitespace-nowrap text-[11.5px]">免确认: {{ autoConfirm ? '开' : '关' }}</span>
      </label>

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

    <!-- ============ 主舞台 ============ -->
    <main v-if="store.ready && !store.error" class="flex min-h-0 flex-1 items-stretch" :class="'tpl-' + tpl">
      <!-- A：左主列 + 右演员卡 -->
      <template v-if="tpl === 'A'">
        <section class="col-main">
          <div ref="feedRoot" class="feed-scroll"><FeedChat :feed="store.entries" /></div>
          <InputBar />
        </section>
        <ActorsPanel class="actors-rail" />
      </template>

      <!-- B：居中主列（纸感子树） -->
      <template v-else-if="tpl === 'B'">
        <section class="col-main">
          <div ref="feedRoot" class="feed-scroll"><FeedScript :feed="store.entries" /></div>
          <InputBar />
        </section>
      </template>

      <!-- C：全宽沉浸 -->
      <template v-else>
        <section class="col-full">
          <div ref="feedRoot" class="feed-imms"><FeedImmersive :feed="store.entries" /></div>
          <InputBar />
        </section>
      </template>
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

    <!-- ============ 存档抽屉 ============ -->
    <SaveDrawer />
  </div>
</template>

<style scoped>
.col-main {
  flex: 1; max-width: 720px; min-width: 0;
  height: 100%; display: flex; flex-direction: column;
  margin-inline: auto;
}
.tpl-A .col-main { max-width: min(720px, calc(100vw - 300px)); }
.actors-rail { display: block; height: 100%; }
.col-full { flex: 1; min-width: 0; height: 100%; display: flex; flex-direction: column; }
.feed-scroll { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
.feed-scroll > :deep(*) { width: 100%; }
.feed-imms { flex: 1; min-height: 0; overflow: hidden; position: relative; }
</style>