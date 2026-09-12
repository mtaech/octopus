<script setup lang="ts">
// ============================================================
// 列表页（/ 路由）—— 故事书与存档的入口首页（蓝图 #18 ④ / #25 决议 A 分区并置）
//
// 信息架构（本次重构）：
//   ① 继续 / 开始带：最近存档锚点 + 「开始新故事」动作面板（新建 / 导入并置一处）
//   ② 书架：故事书封面卡网格（封面 = 世界的视觉指纹）
//   ③ 最近存档：世界封面缩略行，按最近游玩排序
//
// 呈现层：shadcn-vue + Tailwind 语义类（Material 3 · 靛蓝主题）。
// 封面为纯 CSS 确定性生成（见 StorybookCover），不引入图片资产。
// ============================================================
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { storeToRefs } from 'pinia'
import { toast, createStorybookDraft, saveDraft, uploadAsset } from '@/api'
import { importStFile } from '@/lib/st-import'
import { confirm } from '@/lib/confirm'
import { useListStore } from './stores/list'
import NewGameDialog from './components/NewGameDialog.vue'
import { relativeTime } from './utils/relativeTime'
import StorybookCard from './components/StorybookCard.vue'
import StorybookCover from '@/components/StorybookCover.vue'
import SaveCard from './components/SaveCard.vue'
import SettingsDialog from './components/SettingsDialog.vue'
import ThemeToggle from '@/components/ThemeToggle.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Alert, AlertAction, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  IconAlertTriangleFilled,
  IconBook2,
  IconCheck,
  IconChevronRight,
  IconDiamondFilled,
  IconListCheck,
  IconPlayerPlayFilled,
  IconPlus,
  IconRefresh,
  IconSettings,
  IconSparkles,
  IconTrash,
  IconUpload,
  IconFileImport,
} from '@tabler/icons-vue'

const router = useRouter()
const store = useListStore()
// storeToRefs 保响应性；recentSave 为派生计算（最近存档）
const { storybooks, publishedStorybooks, saves, loading, loaded, error, recentSave } = storeToRefs(store)

// ---- 新建游戏弹窗状态 ----
const dialogOpen = ref(false)
const settingsOpen = ref(false)
const dialogPreselect = ref<string | null>(null)
const creating = ref(false)

// ---- 故事书 / 最近存档 Tab（默认最近存档） ----
const activeTab = ref('saves')

// ---- 多选删除状态 ----
const manageSaves = ref(false)
const selectedSaveIds = ref<string[]>([])

/**
 * 存档行的封面：用其所属故事书的封面（存档内嵌冻结副本里也有，但列表接口不返回）。
 * 故事书已下架时自然回落为 CSS 生成封面。
 */
function coverOf(storybookId: string) {
  return storybooks.value.find(b => b.id === storybookId)?.cover ?? null
}

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
    // 直接传文件字节：后端识别 zip 存档包（save.json + assets/），也兼容旧版单 JSON
    const item = await store.importFromFile(file)
    toast('ok', `已导入「${item.title}」`)
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  } finally {
    importing.value = false
  }
}

// ---- SillyTavern 卡片导入：以角色卡为蓝本新建一本故事书，随后进入编辑器 ----
const stFileInput = ref<HTMLInputElement | null>(null)
const stImporting = ref(false)
function triggerStImport() { stFileInput.value?.click() }

async function onStImportChange(evt: Event) {
  const input = evt.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 允许重复选择同一文件
  if (!file) return
  stImporting.value = true
  try {
    const res = await importStFile(file)
    const onlyLore = res.character.name === '（仅世界书）'
    const title = onlyLore ? 'SillyTavern 世界书' : res.character.name
    const doc = await createStorybookDraft(title)
    const draft = doc.draft
    draft.meta.title = title
    if (onlyLore) {
      draft.lore = res.lore
    } else {
      draft.characters = [res.character]
      draft.lore = res.lore
      // 新书以角色卡为蓝本：清掉空模板里可能残留的引用，避免悬空
      draft.skeleton = []
      draft.relationships = []
      if (res.opening) draft.world.opening = res.opening
      // PNG 卡的头像一并入库为立绘
      if (file.type === 'image/png') {
        try {
          const up = await uploadAsset(file)
          const bmp = await createImageBitmap(file)
          res.character.portrait = { asset: up.asset, w: bmp.width, h: bmp.height }
          bmp.close?.()
        } catch { /* 头像失败不阻断导入 */ }
      }
    }
    await saveDraft(doc.id, draft, doc.draft_version)
    toast('ok', `已用「${title}」创建故事书，进入编辑器`)
    router.push('/storybook/' + doc.id + '/edit')
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  } finally {
    stImporting.value = false
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

/** 删除故事书：破坏性操作，二次确认（与「删除存档」同风格）。 */
async function onDeleteStorybook(id: string) {
  const sb = storybooks.value.find(s => s.id === id)
  if (!sb) return
  const related = saves.value.filter(s => s.storybook_id === id).length
  const saveNote = related
    ? '\n\n已有 ' + related + ' 个存档基于它；这些存档内嵌了故事书冻结副本，删除后仍可继续游玩。'
    : ''
  const pubNote = sb.published ? '\n\n注意：这是已发布的故事书，删除后无法再基于它开新档。' : ''
  const confirmed = await confirm({
    title: '删除故事书「' + sb.title + '」？',
    description: '它的草稿与结对会话会一并删除，且不可恢复。' + saveNote + pubNote,
    confirmText: '删除',
    destructive: true,
  })
  if (!confirmed) return
  try {
    await store.removeStorybook(id)
    toast('ok', '已删除故事书')
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  }
}

/** 删除存档：破坏性操作，二次确认（与游玩页抽屉里的删除是同一后端动作）。 */
async function onDeleteSave(id: string) {
  const s = saves.value.find(x => x.id === id)
  if (!s) return
  const confirmed = await confirm({
    title: '删除存档「' + s.title + '」？',
    description: '删除后不可恢复；已导出的备份包不受影响。',
    confirmText: '删除',
    destructive: true,
  })
  if (!confirmed) return
  try {
    await store.removeSave(id)
    toast('ok', '已删除存档')
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  }
}

/** 进入 / 退出多选（退出即清空已选） */
function toggleManageSaves() {
  manageSaves.value = !manageSaves.value
  selectedSaveIds.value = []
}

/** 勾选 / 取消单个存档 */
function toggleSelectSave(id: string) {
  selectedSaveIds.value = selectedSaveIds.value.includes(id)
    ? selectedSaveIds.value.filter(x => x !== id)
    : [...selectedSaveIds.value, id]
}

/** 全选 / 取消全选 */
function toggleSelectAllSaves() {
  selectedSaveIds.value = selectedSaveIds.value.length === saves.value.length
    ? []
    : saves.value.map(s => s.id)
}

/** 批量删除：逐个删，二次确认；部分失败会提示刷新重试 */
async function onDeleteSelectedSaves() {
  const ids = selectedSaveIds.value
  if (!ids.length) return
  const confirmed = await confirm({
    title: `删除选中的 ${ids.length} 个存档？`,
    description: '删除后不可恢复；已导出的备份包不受影响。',
    confirmText: '删除',
    destructive: true,
  })
  if (!confirmed) return
  try {
    await store.removeSaves(ids)
    toast('ok', `已删除 ${ids.length} 个存档`)
    selectedSaveIds.value = []
    manageSaves.value = false
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  }
}
</script>

<template>
  <div class="relative min-h-dvh overflow-x-hidden bg-background text-foreground">
    <!-- 环境光晕：主色低透明径向，给首屏一点纵深（不参与交互、不遮内容） -->
    <div aria-hidden="true" class="pointer-events-none absolute inset-x-0 top-0 h-[380px] overflow-hidden">
      <div class="absolute top-[-200px] left-1/2 h-[420px] w-[760px] max-w-[130vw] -translate-x-1/2 rounded-full bg-primary/10 blur-3xl dark:bg-primary/15" />
    </div>

    <!-- 顶栏 -->
    <header class="sticky top-0 z-20 border-b border-border bg-background/85 backdrop-blur-md supports-[backdrop-filter]:bg-background/70">
      <div class="mx-auto flex h-14 w-full max-w-[1120px] items-center gap-3 px-4 sm:px-6">
        <div class="inline-flex select-none items-center gap-2.5">
          <div class="flex size-7 items-center justify-center rounded-lg border border-primary/30 bg-primary/12 text-primary shadow-sm shadow-primary/20">
            <IconDiamondFilled aria-hidden="true" class="size-3.5" />
          </div>
          <span class="font-serif text-lg font-bold tracking-[0.04em] text-foreground">Octopus</span>
        </div>
        <span class="hidden text-xs text-muted-foreground sm:inline">通用 AI RPG · 你自己的故事书</span>
        <div class="flex-1"></div>
        <ThemeToggle />
        <Button variant="ghost" size="sm" class="gap-1.5 text-xs text-muted-foreground hover:text-foreground" title="设置 · AI Provider 与模型配置" @click="settingsOpen = true">
          <IconSettings class="size-4" />
          <span class="hidden sm:inline">设置</span>
        </Button>
      </div>
    </header>

    <main class="relative z-10 mx-auto w-full max-w-[1120px] px-4 pt-8 pb-20 sm:px-6 lg:pt-10">
      <!-- 载入中：与终态同形的骨架 -->
      <div v-if="!loaded" class="flex flex-col gap-4">
        <div class="grid gap-4 lg:grid-cols-[minmax(0,1fr)_19rem]">
          <Skeleton class="h-44 rounded-xl" />
          <Skeleton class="h-44 rounded-xl" />
        </div>
        <div class="mt-8 grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <Skeleton v-for="i in 3" :key="i" class="h-60 rounded-xl" />
        </div>
        <div class="mt-8 flex flex-col gap-2.5">
          <Skeleton v-for="i in 3" :key="i" class="h-[4.25rem] rounded-xl" />
        </div>
      </div>

      <!-- 全页空态 -->
      <Empty v-else-if="storybooks.length === 0" class="animate-in fade-in duration-500 rounded-2xl border border-dashed border-border/80 bg-card/40 py-16 backdrop-blur-sm motion-reduce:animate-none">
        <EmptyMedia variant="icon" :class="error ? 'bg-warning/10 text-warning' : 'bg-primary/10 text-primary'">
          <IconAlertTriangleFilled v-if="error" aria-hidden="true" />
          <IconBook2 v-else aria-hidden="true" />
        </EmptyMedia>
        <EmptyHeader>
          <EmptyTitle class="font-serif text-xl">{{ error ? '加载失败' : '书架还空着' }}</EmptyTitle>
          <EmptyDescription class="max-w-md text-[13.5px] leading-relaxed">
            {{ error ?? '开档需要一本已发布的故事书作为设定基石。去编辑器写出你的第一个世界，发布后即可开档游玩。' }}
          </EmptyDescription>
        </EmptyHeader>
        <Button v-if="error" variant="outline" @click="store.load()">
          <IconRefresh data-icon="inline-start" />
          重试
        </Button>
        <div v-else class="flex flex-wrap items-center justify-center gap-2.5">
          <Button size="lg" class="shadow-md shadow-primary/20" @click="goCreateNew">
            <IconPlus data-icon="inline-start" />
            创作新故事书
          </Button>
          <Button size="lg" variant="outline" :disabled="stImporting" @click="triggerStImport">
            <IconFileImport data-icon="inline-start" />
            导入 SillyTavern 角色卡
          </Button>
        </div>
      </Empty>

      <template v-else>
        <Alert v-if="error" variant="destructive" class="mb-6">
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

        <!-- ===== ① 继续 / 开始带 ===== -->
        <section class="grid gap-4 lg:grid-cols-[minmax(0,1fr)_19rem]">
          <!-- 继续游玩大卡（有最近存档） -->
          <Card
            v-if="recentSave"
            role="button"
            tabindex="0"
            class="group animate-in fade-in slide-in-from-bottom-2 duration-500 motion-reduce:animate-none flex-row gap-0 overflow-hidden p-0 transition-[transform,box-shadow,border-color] duration-300 hover:-translate-y-0.5 hover:shadow-xl hover:shadow-primary/10 hover:border-primary/50 focus-visible:ring-2 focus-visible:ring-ring/70 focus-visible:outline-none motion-reduce:transition-none motion-reduce:hover:translate-y-0 cursor-pointer"
            @click="resumeRecent()"
            @keydown.enter.prevent="resumeRecent()"
            @keydown.space.prevent="resumeRecent()"
          >
            <StorybookCover
              :seed="recentSave.storybook_id"
              :title="recentSave.storybook_title"
              :cover="coverOf(recentSave.storybook_id)"
              size="hero"
              class="w-28 flex-none self-stretch sm:w-36 border-r border-border"
            />
            <div class="flex min-w-0 flex-1 flex-col justify-center gap-2 px-4 py-5 sm:px-7 sm:py-6">
              <div class="flex flex-wrap items-center gap-2">
                <span class="inline-flex items-center gap-1.5 rounded-full border border-primary/35 bg-primary/10 px-2.5 py-0.5 text-[11px] font-semibold tracking-wide text-primary">
                  <IconPlayerPlayFilled aria-hidden="true" class="size-3" />
                  继续游玩
                </span>
                <Badge v-if="recentSave.needs_upgrade" class="border-warning/45 bg-warning/15 text-[11px] text-warning">
                  新版次待升级
                </Badge>
              </div>
              <h2 class="line-clamp-1 font-serif text-2xl leading-tight font-semibold tracking-wide text-foreground sm:text-[1.75rem]" :title="recentSave.title">
                {{ recentSave.title }}
              </h2>
              <div class="flex flex-wrap items-center gap-x-3.5 gap-y-1.5 text-[13px] text-muted-foreground">
                <span class="inline-flex items-center gap-1.5 text-foreground/80">
                  <IconBook2 aria-hidden="true" class="size-3.5 text-primary/70" />
                  {{ recentSave.storybook_title }}
                </span>
                <span class="text-muted-foreground/40">·</span>
                <span class="font-mono text-xs">版次 {{ recentSave.embedded_revision }}</span>
                <span class="text-muted-foreground/40">·</span>
                <span>最近游玩 · {{ relativeTime(recentSave.last_played_at) }}</span>
              </div>
            </div>
            <!-- 视觉按钮（整卡即按钮，此处仅作反馈，非独立焦点）；窄屏隐藏，避免挤压标题 -->
            <div class="hidden flex-none items-center pr-5 sm:flex sm:pr-7 pl-4 border-l border-border/70 self-stretch my-3">
              <span
                aria-hidden="true"
                class="inline-flex h-10 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground shadow-sm shadow-primary/25 transition-transform duration-300 group-hover:scale-[1.04] motion-reduce:transition-none motion-reduce:group-hover:scale-100"
              >
                <IconPlayerPlayFilled class="size-4" />
                继续
              </span>
            </div>
          </Card>

          <!-- 无存档引导卡 -->
          <Card v-else class="animate-in fade-in slide-in-from-bottom-2 duration-500 motion-reduce:animate-none flex-row gap-0 overflow-hidden p-0">
            <StorybookCover
              :seed="storybooks[0]?.id ?? 'octopus'"
              :title="storybooks[0]?.title ?? '书'"
              :cover="storybooks[0]?.cover ?? null"
              size="hero"
              class="w-28 flex-none self-stretch sm:w-36 border-r border-border"
            />
            <div class="flex min-w-0 flex-1 flex-col justify-center gap-2 px-4 py-5 sm:px-7 sm:py-6">
              <span class="text-[11px] font-semibold tracking-[0.18em] text-primary/80 uppercase">还没有存档</span>
              <h2 class="font-serif text-2xl font-semibold tracking-wide text-foreground">开启第一段冒险</h2>
              <p class="max-w-[46ch] text-sm leading-relaxed text-muted-foreground">
                从下方故事书开一档，或导入一份自包含存档包继续上次的旅程。
              </p>
            </div>
          </Card>

          <!-- 开始新故事：动作面板 -->
          <Card class="animate-in fade-in slide-in-from-bottom-2 duration-500 delay-100 motion-reduce:animate-none flex flex-col gap-0 overflow-hidden p-0">
            <div class="border-b border-border bg-muted/25 px-4 py-2.5">
              <p class="text-[11px] font-semibold tracking-[0.16em] text-muted-foreground uppercase">开始新故事</p>
            </div>
            <div class="flex min-h-0 flex-1 flex-col divide-y divide-border">
              <button
                type="button"
                class="group/row flex flex-1 cursor-pointer items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none focus-visible:ring-inset"
                @click="openNewGame()"
              >
                <span class="flex size-9 flex-none items-center justify-center rounded-xl border border-primary/25 bg-primary/12 text-primary transition-all duration-300 group-hover/row:bg-primary group-hover/row:text-primary-foreground motion-reduce:transition-none">
                  <IconPlus aria-hidden="true" class="size-4" />
                </span>
                <span class="min-w-0 flex-1">
                  <span class="block text-sm font-semibold text-foreground">新建游戏</span>
                  <span class="block text-xs text-muted-foreground">基于已发布故事书开档</span>
                </span>
                <IconChevronRight aria-hidden="true" class="size-4 flex-none text-muted-foreground/50 transition-transform duration-300 group-hover/row:translate-x-0.5 group-hover/row:text-primary motion-reduce:transition-none" />
              </button>

              <button
                type="button"
                class="group/row flex flex-1 cursor-pointer items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none focus-visible:ring-inset disabled:pointer-events-none disabled:opacity-50"
                :disabled="importing"
                @click="triggerImport"
              >
                <span class="flex size-9 flex-none items-center justify-center rounded-xl border border-primary/25 bg-primary/12 text-primary transition-all duration-300 group-hover/row:bg-primary group-hover/row:text-primary-foreground motion-reduce:transition-none">
                  <IconUpload aria-hidden="true" class="size-4" />
                </span>
                <span class="min-w-0 flex-1">
                  <span class="block text-sm font-semibold text-foreground">导入存档</span>
                  <span class="block text-xs text-muted-foreground">从 .octopus.zip 恢复</span>
                </span>
                <IconChevronRight aria-hidden="true" class="size-4 flex-none text-muted-foreground/50 transition-transform duration-300 group-hover/row:translate-x-0.5 group-hover/row:text-primary motion-reduce:transition-none" />
              </button>
              <button
                type="button"
                class="group/row flex flex-1 cursor-pointer items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none focus-visible:ring-inset disabled:pointer-events-none disabled:opacity-50"
                :disabled="stImporting"
                @click="triggerStImport"
              >
                <span class="flex size-9 flex-none items-center justify-center rounded-xl border border-primary/25 bg-primary/12 text-primary transition-all duration-300 group-hover/row:bg-primary group-hover/row:text-primary-foreground motion-reduce:transition-none">
                  <IconFileImport aria-hidden="true" class="size-4" />
                </span>
                <span class="min-w-0 flex-1">
                  <span class="block truncate text-sm font-semibold text-foreground">导入角色卡</span>
                  <span class="block truncate text-xs text-muted-foreground">{{ stImporting ? '导入中…' : 'SillyTavern PNG / JSON' }}</span>
                </span>
                <IconChevronRight aria-hidden="true" class="size-4 flex-none text-muted-foreground/50 transition-transform duration-300 group-hover/row:translate-x-0.5 group-hover/row:text-primary motion-reduce:transition-none" />
              </button>
            </div>
          </Card>
        </section>

        <!-- ===== ② / ③ 我的故事书 与 最近存档（Tab 切换，默认最近存档） ===== -->
        <section class="mt-12">
          <Tabs v-model="activeTab" class="gap-0 flex-col">
            <TabsList variant="line" class="h-10 w-full justify-start gap-1 rounded-none border-b border-border/80 px-0">
              <TabsTrigger value="books" class="h-10 flex-none gap-2 px-3 text-[13.5px] font-medium">
                我的故事书
                <Badge variant="outline" class="border-border/70 px-1.5 font-mono text-[10.5px] font-normal text-muted-foreground">{{ storybooks.length }}</Badge>
              </TabsTrigger>
              <TabsTrigger value="saves" class="h-10 flex-none gap-2 px-3 text-[13.5px] font-medium">
                最近存档
                <Badge variant="outline" class="border-border/70 px-1.5 font-mono text-[10.5px] font-normal text-muted-foreground">{{ saves.length }}</Badge>
              </TabsTrigger>
            </TabsList>

            <TabsContent value="books" class="mt-5">
              <div class="mb-4 flex flex-wrap items-end justify-between gap-3">
                <p class="text-xs text-muted-foreground">
                  {{ publishedStorybooks.length }} 本已发布 · 草稿点「继续编辑」，已发布点「新建游戏」开档
                </p>
                <Button size="sm" variant="outline" class="gap-1.5 border-border/80 hover:border-border" @click="goCreateNew">
                  <IconPlus data-icon="inline-start" class="size-3.5" />
                  创作新故事书
                </Button>
              </div>
          <div class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
            <StorybookCard
              v-for="(sb, i) in storybooks"
              :key="sb.id"
              :storybook="sb"
              class="animate-in fade-in slide-in-from-bottom-2 duration-500 motion-reduce:animate-none"
              :style="{ animationDelay: `${i * 45}ms` }"
              @new-game="openNewGame($event)"
              @edit="goEditor($event)"
              @delete="onDeleteStorybook($event)"
            />
          </div>
            </TabsContent>

            <TabsContent value="saves" class="mt-5">
              <div class="mb-4 flex flex-wrap items-end justify-between gap-3">
                <p class="text-xs text-muted-foreground">
                  共 {{ saves.length }} 个 · 按最近游玩排序<template v-if="manageSaves"> · 已选 {{ selectedSaveIds.length }}</template>
                </p>
            <div v-if="saves.length" class="flex flex-wrap items-center gap-2">
              <template v-if="manageSaves">
                <Button size="sm" variant="outline" class="border-border/80" @click="toggleSelectAllSaves">
                  <IconCheck data-icon="inline-start" class="size-3.5" />
                  {{ selectedSaveIds.length === saves.length ? '取消全选' : '全选' }}
                </Button>
                <Button size="sm" variant="destructive" :disabled="!selectedSaveIds.length" @click="onDeleteSelectedSaves">
                  <IconTrash data-icon="inline-start" class="size-3.5" />
                  删除选中<span v-if="selectedSaveIds.length">（{{ selectedSaveIds.length }}）</span>
                </Button>
                <Button size="sm" variant="ghost" class="text-muted-foreground" @click="toggleManageSaves">完成</Button>
              </template>
              <Button v-else size="sm" variant="outline" class="border-border/80" title="多选删除存档" @click="toggleManageSaves">
                <IconListCheck data-icon="inline-start" class="size-3.5" />
                多选
              </Button>
            </div>
          </div>

          <Empty v-if="saves.length === 0" class="rounded-2xl border border-dashed border-border/80 bg-card/30 py-14">
            <EmptyContent>
              <EmptyMedia variant="icon" class="bg-muted text-muted-foreground">
                <IconBook2 aria-hidden="true" />
              </EmptyMedia>
              <EmptyTitle class="text-base">还没有存档</EmptyTitle>
              <EmptyDescription class="max-w-md text-sm leading-relaxed">
                从上方故事书开一档开始新的冒险，或导入一份自包含存档包继续。
              </EmptyDescription>
              <div class="flex flex-wrap items-center justify-center gap-2.5 pt-2">
                <Button size="sm" @click="openNewGame()">
                  <IconPlus data-icon="inline-start" />
                  新建游戏
                </Button>
                <Button size="sm" variant="outline" :disabled="importing" @click="triggerImport">
                  <IconUpload data-icon="inline-start" />
                  导入存档
                </Button>
                <Button size="sm" variant="outline" :disabled="stImporting" @click="triggerStImport">
                  <IconFileImport data-icon="inline-start" />
                  导入角色卡
                </Button>
              </div>
            </EmptyContent>
          </Empty>

          <div v-else class="flex flex-col gap-2.5">
            <SaveCard
              v-for="(s, i) in saves"
              :key="s.id"
              :save="s"
              :cover="coverOf(s.storybook_id)"
              :selectable="manageSaves"
              :selected="selectedSaveIds.includes(s.id)"
              class="animate-in fade-in slide-in-from-bottom-1 duration-500 motion-reduce:animate-none"
              :style="{ animationDelay: `${i * 35}ms` }"
              @resume="resume($event)"
              @select="toggleSelectSave($event)"
              @delete="onDeleteSave($event)"
            />
          </div>

          <p class="mt-6 inline-flex items-center gap-2 text-xs text-muted-foreground/70">
            <IconSparkles aria-hidden="true" class="size-3.5 text-primary/60" />
            存档管理（升级 / 新原点 / 分享）在游玩页右侧抽屉内，本页作入口直达。
          </p>
            </TabsContent>
          </Tabs>
        </section>
      </template>
    </main>

    <input ref="fileInput" type="file" accept=".zip,.octopus.zip,.json,.octopus.json" class="hidden" @change="onImportChange" />
        <input ref="stFileInput" type="file" accept=".png,.json,application/json,image/png" class="hidden" @change="onStImportChange" />

    <SettingsDialog v-model:open="settingsOpen" />

    <NewGameDialog
      v-model:open="dialogOpen"
      :storybooks="publishedStorybooks"
      :preselect-id="dialogPreselect"
      :busy="creating"
      @confirm="onNewGameConfirm"
    />
  </div>
</template>
