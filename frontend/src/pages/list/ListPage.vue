<script setup lang="ts">
// ============================================================
// 列表页（/ 路由）—— 故事书与存档的入口首页（蓝图 #18 ④ / #25 决议 A 分区并置）
// 分区：顶部动作带（继续游玩 / 新建游戏 / 导入）→ 故事书网格 → 存档列表
// 呈现层：shadcn-vue + Tailwind 语义类（夜行手记主题 · 墨黑+琥珀炉火）。
// 精致版：环境光晕 + 大卡锚点 + 封面感故事书卡 + 节奏留白。
// ============================================================
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { storeToRefs } from 'pinia'
import { toast } from '@/api'
import { useListStore } from './stores/list'
import NewGameDialog from './components/NewGameDialog.vue'
import { relativeTime } from './utils/relativeTime'
import StorybookCard from './components/StorybookCard.vue'
import SaveCard from './components/SaveCard.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Alert, AlertAction, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  IconAlertTriangleFilled,
  IconArrowUpRight,
  IconBook2,
  IconDiamondFilled,
  IconLoader2,
  IconPlayerPlayFilled,
  IconPlus,
  IconRefresh,
  IconSparkles,
  IconUpload,
} from '@tabler/icons-vue'

const router = useRouter()
const store = useListStore()
// storeToRefs 保响应性；recentSave 为派生计算（最近存档）
const { storybooks, saves, loading, loaded, error, recentSave } = storeToRefs(store)

// ---- 新建游戏弹窗状态 ----
const dialogOpen = ref(false)
const dialogPreselect = ref<string | null>(null)
const creating = ref(false)

// ---- 隐藏文件输入（导入） ----
const fileInput = ref<HTMLInputElement | null>(null)
const importing = ref(false)

onMounted(() => { store.load() })

function openNewGame(storybookId?: string) {
  dialogPreselect.value = storybookId ?? null
  dialogOpen.value = true
}

/** 继续游玩：直达最近存档（listSaves()[0]）。需升级存档也直达（升级提示归游玩页横幅） */
function resume(saveId: string) {
  router.push('/play/' + saveId)
}

/** 继续游玩大卡：点击直达最近存档 */
function resumeRecent() {
  const s = recentSave.value
  if (s) router.push('/play/' + s.id)
}

/** 新建游戏确认（规格点 3）：createSave → toast ok → 进游玩页 */
async function onNewGameConfirm(payload: { storybookId: string; title: string; controlledCharacterId?: string }) {
  creating.value = true
  try {
    const saveId = await store.startNewGame(payload.storybookId, payload.title, payload.controlledCharacterId)
    toast('ok', `已开档「${payload.title}」，进入游玩页`)
    dialogOpen.value = false
    router.push('/play/' + saveId)
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  } finally {
    creating.value = false
  }
}

/** 导入（规格点 4）：取文件名 → importSave → toast ok → 列表刷新（新存档带「新导入」角标） */
function triggerImport() { fileInput.value?.click() }

async function onImportChange(evt: Event) {
  const input = evt.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 允许重复选择同一文件再次触发
  if (!file) return
  importing.value = true
  try {
    const item = await store.importFromFile(file.name)
    toast('ok', `已导入「${item.title}」`)
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  } finally {
    importing.value = false
  }
}

/** 进编辑器（规格点 5） */
function goEditor(storybookId: string) {
  router.push('/storybook/' + storybookId + '/edit')
}

/** 全页空态唯一出路：去编辑器新建（#25 Answer ③） */
function goCreateNew() {
  router.push('/storybook/new/edit')
}
</script>

<template>
  <div class="relative min-h-dvh overflow-x-hidden bg-background text-foreground">
    <!-- 环境光晕：炉火从左上漫下来 + 右下余晖，夜行手记的氛围底（纯呈现，不挡交互） -->
    <div aria-hidden="true" class="pointer-events-none fixed inset-0 z-0">
      <div class="absolute -top-40 -left-24 h-[30rem] w-[38rem] rounded-full bg-primary/7 blur-[110px]" />
      <div class="absolute top-1/3 -right-40 h-[26rem] w-[30rem] rounded-full bg-warning/4 blur-[120px]" />
      <div class="absolute inset-0 bg-[radial-gradient(ellipse_80%_60%_at_50%_-10%,transparent,transparent_55%,rgba(0,0,0,0.28))]" />
    </div>

    <!-- 顶栏 -->
    <header class="sticky top-0 z-20 border-b border-border/70 bg-background/80 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div class="mx-auto flex h-14 w-full max-w-[1120px] items-center gap-4 px-4 sm:px-6">
        <span class="inline-flex select-none items-center gap-2">
          <IconDiamondFilled aria-hidden="true" class="size-3.5 text-primary" />
          <span class="font-serif text-lg font-bold tracking-[0.03em]">Octopus</span>
        </span>
        <Badge variant="outline" class="hidden gap-1 border-border/70 bg-muted/30 font-normal text-muted-foreground sm:inline-flex">
          列表 · 故事书与存档
        </Badge>
      </div>
    </header>

    <main class="relative z-10 mx-auto w-full max-w-[1120px] px-4 pt-12 pb-16 sm:px-6 lg:pt-16">
      <!-- 载入中 -->
      <div v-if="!loaded" class="flex flex-col items-center gap-8 pt-4">
        <p class="inline-flex items-center gap-2 text-sm text-muted-foreground">
          <IconLoader2 aria-hidden="true" class="size-4 animate-spin text-primary/70" />
          正在载入…
        </p>
        <div class="flex w-full flex-col gap-4 md:flex-row">
          <Skeleton class="h-48 flex-1 rounded-2xl" />
          <div class="grid w-full grid-cols-2 gap-4 md:w-64 md:flex-none md:flex-col">
            <Skeleton class="h-24 rounded-2xl" />
            <Skeleton class="h-24 rounded-2xl" />
          </div>
        </div>
        <div class="grid w-full gap-4 sm:grid-cols-2">
          <Skeleton class="h-40 rounded-2xl" />
          <Skeleton class="h-40 rounded-2xl" />
        </div>
      </div>

      <!-- 全页空态 -->
      <Empty v-else-if="storybooks.length === 0" class="rounded-2xl border border-dashed border-border">
        <EmptyMedia variant="icon" :class="error ? 'bg-warning/10 text-warning' : 'bg-primary/10 text-primary'">
          <IconAlertTriangleFilled v-if="error" aria-hidden="true" />
          <IconBook2 v-else aria-hidden="true" />
        </EmptyMedia>
        <EmptyHeader>
          <EmptyTitle>{{ error ? '加载失败' : '还没有已发布的故事书' }}</EmptyTitle>
          <EmptyDescription>
            {{ error ?? '开档需要一个已发布的故事书作为设定。去编辑器新建一份故事书，或把已有草稿发布。' }}
          </EmptyDescription>
        </EmptyHeader>
        <Button v-if="error" variant="outline" @click="store.load()">
          <IconRefresh data-icon="inline-start" />
          重试
        </Button>
        <Button v-else @click="goCreateNew">
          <IconPlus data-icon="inline-start" />
          去编辑器新建故事书
        </Button>
      </Empty>

      <template v-else>
        <Alert v-if="error" variant="destructive" class="mb-8">
          <IconAlertTriangleFilled aria-hidden="true" />
          <AlertTitle>加载失败</AlertTitle>
          <AlertDescription>{{ error }}</AlertDescription>
          <AlertAction>
            <Button size="sm" variant="ghost" @click="store.load()">
              <IconRefresh data-icon="inline-start" />
              重试
            </Button>
          </AlertAction>
        </Alert>

        <!-- ===== 顶部动作带 ===== -->
        <section class="flex flex-col gap-5 md:flex-row md:items-stretch">
          <!-- 继续游玩大卡：今夜冒险锚点 -->
          <Card
            v-if="recentSave"
            role="button"
            tabindex="0"
            class="group/resume relative flex-1 cursor-pointer overflow-hidden rounded-2xl border-border/80 p-0 transition-all duration-300 hover:-translate-y-0.5 hover:border-primary/50 hover:shadow-[0_16px_48px_-12px_rgba(0,0,0,0.7)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70"
            @click="resumeRecent()"
            @keydown.enter.prevent="resumeRecent()"
            @keydown.space.prevent="resumeRecent()"
          >
            <!-- 大卡内部：左侧炉火光斑 + 底部细线引导视线 -->
            <div aria-hidden="true" class="pointer-events-none absolute -top-20 -left-16 h-64 w-64 rounded-full bg-primary/15 blur-3xl transition-opacity duration-300 group-hover/resume:opacity-100" />
            <div aria-hidden="true" class="pointer-events-none absolute right-0 bottom-0 h-40 w-56 translate-x-1/4 translate-y-1/4 rounded-full bg-warning/8 blur-3xl" />
            <div class="relative flex h-full min-h-[11.5rem] flex-col justify-center gap-2 px-6 py-7 sm:px-8">
              <p class="inline-flex items-center gap-1.5 text-[11px] font-bold tracking-[0.24em] text-primary uppercase">
                <IconPlayerPlayFilled aria-hidden="true" class="size-3.5" />
                继续游玩
              </p>
              <h2 class="mt-1.5 line-clamp-1 font-serif text-2xl leading-tight font-semibold tracking-wide text-card-foreground sm:text-[1.7rem]" :title="recentSave.title">
                {{ recentSave.title }}
              </h2>
              <div class="mt-2.5 flex flex-wrap items-center gap-x-3.5 gap-y-1.5 text-[13px] text-muted-foreground">
                <Badge v-if="recentSave.needs_upgrade" class="border-warning/40 bg-warning/10 text-warning">
                  新版次待升级
                </Badge>
                <span class="inline-flex items-center gap-1.5">
                  <IconBook2 aria-hidden="true" class="size-3.5 text-muted-foreground/70" />
                  {{ recentSave.storybook_title }}
                </span>
                <span class="text-muted-foreground/60">·</span>
                <span>版次 {{ recentSave.embedded_revision }}</span>
                <span class="text-muted-foreground/60">·</span>
                <span>最近游玩 · {{ relativeTime(recentSave.last_played_at) }}</span>
              </div>
            </div>
            <!-- 右侧悬停箭头 -->
            <div aria-hidden="true" class="absolute top-1/2 right-6 flex size-10 -translate-y-1/2 items-center justify-center rounded-full border border-primary/25 bg-primary/8 text-primary transition-all duration-300 group-hover/resume:scale-110 group-hover/resume:border-primary/50 group-hover/resume:bg-primary/15">
              <IconArrowUpRight class="size-4 transition-transform duration-300 group-hover/resume:translate-x-0.5 group-hover/resume:-translate-y-0.5" />
            </div>
          </Card>

          <!-- 无存档引导卡 -->
          <Card v-else class="relative flex-1 overflow-hidden rounded-2xl border-border/80 p-0">
            <div aria-hidden="true" class="pointer-events-none absolute -top-16 -left-14 h-56 w-56 rounded-full bg-primary/8 blur-3xl" />
            <div class="relative flex h-full min-h-[11.5rem] flex-col justify-center gap-2 px-6 py-7 sm:px-8">
              <p class="inline-flex items-center gap-1.5 text-[11px] font-bold tracking-[0.24em] text-primary/70 uppercase">
                <IconPlayerPlayFilled aria-hidden="true" class="size-3.5" />
                继续游玩
              </p>
              <h2 class="mt-1.5 font-serif text-2xl font-semibold tracking-wide text-card-foreground sm:text-[1.7rem]">还没有存档</h2>
              <p class="mt-2 max-w-[52ch] text-sm leading-relaxed text-muted-foreground">
                从下方已发布故事书「新建游戏」开一局，或「导入」已有的 .sqlite 存档继续。
              </p>
            </div>
          </Card>

          <!-- 右侧入口动作 -->
          <div class="grid grid-cols-2 gap-4 md:w-[15.5rem] md:flex-none md:flex-col">
            <button
              type="button"
              class="group flex cursor-pointer flex-col items-start justify-center gap-2 rounded-2xl border border-border/80 bg-card/60 px-5 py-4 text-left transition-all duration-300 hover:-translate-y-0.5 hover:border-primary/40 hover:bg-card hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70"
              @click="openNewGame()"
            >
              <span class="inline-flex size-8 items-center justify-center rounded-lg bg-primary/12 text-primary transition-colors duration-300 group-hover:bg-primary/20">
                <IconPlus aria-hidden="true" class="size-4" />
              </span>
              <span class="mt-1 text-sm font-semibold">新建游戏</span>
              <span class="text-xs leading-relaxed text-muted-foreground">基于已发布故事书开档</span>
            </button>
            <button
              type="button"
              class="group flex cursor-pointer flex-col items-start justify-center gap-2 rounded-2xl border border-border/80 bg-card/60 px-5 py-4 text-left transition-all duration-300 hover:-translate-y-0.5 hover:border-primary/40 hover:bg-card hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70 disabled:pointer-events-none disabled:opacity-50"
              :disabled="importing"
              @click="triggerImport"
            >
              <span class="inline-flex size-8 items-center justify-center rounded-lg bg-primary/12 text-primary transition-colors duration-300 group-hover:bg-primary/20">
                <IconUpload aria-hidden="true" class="size-4" />
              </span>
              <span class="mt-1 text-sm font-semibold">导入存档</span>
              <span class="text-xs leading-relaxed text-muted-foreground">选择 .sqlite 文件</span>
            </button>
          </div>
        </section>

        <!-- ===== 故事书网格 ===== -->
        <section class="mt-14">
          <div class="mb-5 flex items-baseline justify-between gap-3 px-0.5">
            <h2 class="inline-flex items-center gap-2.5 text-[13px] font-semibold tracking-[0.2em] text-foreground/90 uppercase">
              <span aria-hidden="true" class="size-1.5 rounded-full bg-primary shadow-[0_0_8px_var(--primary)]" />
              我的故事书
            </h2>
            <span class="text-xs text-muted-foreground">{{ storybooks.length }} 本已发布</span>
          </div>
          <div class="grid grid-cols-1 gap-5 sm:grid-cols-2">
            <StorybookCard
              v-for="sb in storybooks"
              :key="sb.id"
              :storybook="sb"
              @new-game="openNewGame($event)"
              @edit="goEditor($event)"
            />
          </div>
        </section>

        <!-- ===== 存档列表 ===== -->
        <section class="mt-14 pb-2">
          <div class="mb-5 flex items-baseline justify-between gap-3 px-0.5">
            <h2 class="inline-flex items-center gap-2.5 text-[13px] font-semibold tracking-[0.2em] text-foreground/90 uppercase">
              <span aria-hidden="true" class="size-1.5 rounded-full bg-primary shadow-[0_0_8px_var(--primary)]" />
              最近存档
            </h2>
            <span class="text-xs text-muted-foreground">{{ saves.length }} 个 · 按最近游玩排序</span>
          </div>

          <Empty v-if="saves.length === 0" class="rounded-2xl border border-dashed border-border py-14">
            <EmptyContent>
              <IconBook2 aria-hidden="true" class="size-6 text-muted-foreground/60" />
              <EmptyDescription class="max-w-md">
                还没有存档。从上方故事书「新建游戏」开始一段新冒险，或导入一个 .sqlite 存档继续。
              </EmptyDescription>
              <div class="flex flex-wrap items-center justify-center gap-2">
                <Button size="sm" @click="openNewGame()">
                  <IconPlus data-icon="inline-start" />
                  新建游戏
                </Button>
                <Button size="sm" variant="outline" :disabled="importing" @click="triggerImport">
                  <IconUpload data-icon="inline-start" />
                  导入存档
                </Button>
              </div>
            </EmptyContent>
          </Empty>

          <div v-else class="flex flex-col gap-3.5">
            <SaveCard v-for="s in saves" :key="s.id" :save="s" @resume="resume($event)" />
          </div>

          <p class="mt-8 inline-flex items-center gap-1.5 px-0.5 text-xs text-muted-foreground/60">
            <IconSparkles aria-hidden="true" class="size-3.5 text-primary/50" />
            存档管理（升级 / 新原点 / 分享）在游玩页抽屉内 —— 本页仅作入口直达。
          </p>
        </section>
      </template>
    </main>

    <input ref="fileInput" type="file" accept=".sqlite" class="hidden" @change="onImportChange" />

    <NewGameDialog
      v-model:open="dialogOpen"
      :storybooks="storybooks"
      :preselect-id="dialogPreselect"
      :busy="creating"
      @confirm="onNewGameConfirm"
    />
  </div>
</template>
