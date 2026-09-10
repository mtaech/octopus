<script setup lang="ts">
// EditorPage —— /storybook/:id/edit（id=new = 新建草稿）
// #22：单路由 + 内部视图态（A 表单工作台 / C AI 结对 / B 文档 三平级）
// editor store = 草稿唯一主人；pair store = 结对会话（待审查建议不入草稿）
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { toast } from '@/api'
import { useEditorStore, PARADIGMS, type Paradigm } from './stores/editor'
import { usePairStore } from './stores/pair'
import WorkspaceA from './components/workspace-a/WorkspaceA.vue'
import PairC from './paradigms/pair-c/PairC.vue'
import DocumentB from './paradigms/document-b/DocumentB.vue'
import ValidationDock from './components/refs/ValidationDock.vue'
import PlaytestDialog from './components/PlaytestDialog.vue'
import { Button } from '@/components/ui/button'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Badge } from '@/components/ui/badge'
import {
  IconArrowLeft,
  IconX,
  IconCircleDot,
  IconRocket,
  IconCheck,
  IconLoader2,
  IconLayoutGrid,
  IconSparkles,
  IconFileText,
  IconPlayerPlay,
} from '@tabler/icons-vue'

const route = useRoute()
const router = useRouter()
const editor = useEditorStore()
const pair = usePairStore()

// 是否"新建"语义（id=new）：进 C 结对 + 首屏轻引导（#22 ①）
const isNewSession = ref(false)
const isNew = computed(() => isNewSession.value)

// 范式（Tabs 双向绑定）
const paradigmModel = computed<Paradigm>({
  get: () => editor.paradigm,
  set: (v: Paradigm) => editor.setParadigm(v),
})
const showIntro = computed(() => isNew.value && booted.value && pair.showWelcome() && editor.paradigm === 'C')

const paradigmIcons: Record<Paradigm, any> = {
  A: IconLayoutGrid,
  C: IconSparkles,
  B: IconFileText,
}

// 保存态文案
const saveLabel = computed(() => {
  if (editor.saving) return '保存中…'
  if (editor.dirty) return '未保存改动'
  return '已自动保存'
})

const errorCount = computed(() => editor.issues.filter(i => i.severity === 'error').length)
const pubBlockMsg = computed(() => {
  if (!editor.dirty && errorCount.value === 0) return ''
  if (errorCount.value > 0) {
    const first = editor.issues.find(i => i.severity === 'error')
    return '有 ' + errorCount.value + ' 个错误级校验问题（发布被拦下）：' + (first?.message ?? '')
  }
  return '草稿有未保存改动，发布前会自动保存'
})

async function onPublish(): Promise<void> {
  await editor.publish()
}

const playtestDialogOpen = ref(false)

function openPlaytestDialog(): void {
  playtestDialogOpen.value = true
}

async function onPlaytestConfirm(payload: { title?: string; controlledCharacterId?: string }): Promise<void> {
  const saveId = await editor.playtest(payload.title, payload.controlledCharacterId)
  if (saveId) {
    playtestDialogOpen.value = false
    void router.push('/play/' + saveId)
  }
}

function goBack(): void {
  void router.push('/')
}

// ---------- 挂载：load（或 create） ----------
const booted = ref(false)
onMounted(async () => {
  const raw = String(route.params.id ?? '')
  let ok = false
  if (raw === 'new') {
    isNewSession.value = true
    editor.setParadigm('C')
    pair.bannerDismissed = false
    const newId = await editor.createNew('北境信使 · 初雪（新草稿）')
    if (newId) {
      void router.replace({ path: '/storybook/' + newId + '/edit' })
      ok = true
    }
  } else {
    ok = await editor.load(raw)
    if (ok && !editor.paradigm) editor.setParadigm('A')
  }
  if (!ok) {
    toast('error', editor.loadError ?? '无法打开故事书')
  }
  booted.value = true
})

onUnmounted(() => {
  editor.dispose()
})
</script>

<template>
  <div class="flex h-full min-h-0 flex-col bg-background">
    <!-- ============ 应用顶栏 ============ -->
    <header class="flex h-13 flex-none items-center gap-3 border-b border-border/80 bg-card/85 px-3.5 backdrop-blur-md">
      <Button variant="ghost" size="sm" class="gap-1.5 text-xs text-muted-foreground hover:text-foreground" @click="goBack">
        <IconArrowLeft data-icon="inline-start" class="size-4" />
        <span class="font-medium">故事书列表</span>
      </Button>

      <div class="h-4 w-px bg-border/60" />

      <div class="flex min-w-0 items-center gap-2.5">
        <span class="truncate font-serif text-[15px] font-bold tracking-wide text-foreground" :title="editor.draft?.meta.title ?? ''">
          {{ editor.draft?.meta.title ?? '加载中…' }}
        </span>
        <span v-if="editor.dirty" class="inline-flex items-center gap-1 text-[11px] font-semibold text-warning" title="有未保存改动">
          <span class="size-2 rounded-full bg-warning animate-pulse shadow-[0_0_8px_var(--warning)]" />
        </span>
        <Badge v-if="isNew" variant="outline" class="border-warning/45 bg-warning/10 text-[10.5px] text-warning">新草稿</Badge>
        <Badge v-if="editor.published" variant="outline" class="border-success/45 bg-success/10 font-mono text-[10.5px] text-success">rev {{ editor.revision }}</Badge>
        <Badge v-else-if="editor.draft" variant="secondary" class="font-mono text-[10.5px] text-muted-foreground">未发布 · rev {{ editor.revision }}</Badge>
      </div>

      <!-- 自动保存状态指示 -->
      <div class="hidden items-center gap-1.5 pl-2 text-xs text-muted-foreground/75 lg:inline-flex">
        <IconLoader2 v-if="editor.saving" class="size-3.5 animate-spin text-primary" />
        <IconCircleDot v-else-if="editor.dirty" class="size-3 text-warning" />
        <IconCheck v-else class="size-3.5 text-success/80" />
        <span>{{ saveLabel }}</span>
      </div>

      <!-- 范式切换（A / C / B 视图态按钮组） -->
      <nav class="ml-auto flex items-center" aria-label="编辑范式">
        <Tabs :model-value="paradigmModel" @update:model-value="paradigmModel = ($event as Paradigm)">
          <TabsList class="h-8 gap-0.5 rounded-lg border border-border/80 bg-background/80 p-0.5 shadow-inner">
            <TabsTrigger
              v-for="p in PARADIGMS"
              :key="p.key"
              :value="p.key"
              class="h-7 gap-1.5 rounded-md px-3 text-xs font-medium transition-all"
              :title="p.label + '：' + p.hint"
            >
              <component :is="paradigmIcons[p.key]" class="size-3.5" />
              <span>{{ p.label }}</span>
            </TabsTrigger>
          </TabsList>
        </Tabs>
      </nav>

      <div class="h-4 w-px bg-border/60" />

      <!-- 沙箱试玩按钮 -->
      <Button
        variant="outline"
        size="sm"
        :disabled="editor.publishing || editor.playtesting"
        title="直接使用当前草稿开档试玩，验证设定"
        class="gap-1.5 font-medium border-primary/40 text-primary hover:bg-primary/10 transition-all duration-200"
        @click="openPlaytestDialog"
      >
        <IconLoader2 v-if="editor.playtesting" class="size-4 animate-spin" />
        <IconPlayerPlay v-else data-icon="inline-start" class="size-4" />
        <span>沙箱试玩</span>
      </Button>

      <div class="h-4 w-px bg-border/60" />

      <!-- 发布按钮 -->
      <Button
        size="sm"
        :disabled="errorCount > 0 || editor.publishing"
        :title="pubBlockMsg || '发布当前草稿为新版次'"
        class="gap-1.5 font-semibold transition-all duration-200"
        :class="errorCount === 0 && !editor.publishing ? 'bg-gradient-to-r from-primary to-amber-500 hover:from-amber-500 hover:to-primary text-primary-foreground shadow-sm shadow-primary/30 hover:shadow-md hover:shadow-primary/40' : ''"
        @click="onPublish"
      >
        <IconLoader2 v-if="editor.publishing" class="size-4 animate-spin" />
        <IconRocket v-else data-icon="inline-start" class="size-4" />
        <span v-if="editor.publishing">发布中…</span>
        <template v-else>发布新版次{{ editor.dirty ? ' ●' : '' }}</template>
      </Button>
    </header>

    <!-- ============ 首屏轻引导（新建书 C 范式，#22 ① 可关不强制） ============ -->
    <div v-if="showIntro" class="absolute top-16 left-1/2 z-40 w-[min(560px,calc(100vw-2rem))] -translate-x-1/2">
      <div class="rounded-xl border border-primary/40 bg-popover p-4 shadow-xl shadow-black/40">
        <div class="mb-2 flex items-center">
          <strong class="font-serif text-[15px] text-foreground">开始创作新故事书</strong>
          <Button variant="ghost" size="icon-sm" class="ml-auto size-6 text-muted-foreground/60" aria-label="关掉" @click="pair.bannerDismissed = true">
            <IconX />
          </Button>
        </div>
        <p class="text-[13px] leading-6 text-muted-foreground">没有头绪？从 C AI 结对开始最顺——跟 Octo 聊聊设定，把建议「采纳」成实体。</p>
        <p class="mt-1 text-xs leading-5 text-muted-foreground/70">有明确目标？切到 A 表单工作台逐项精修；想自由书写就进 B 文档。三种范式操作同一草稿，切换不丢内容。</p>
        <div class="mt-2.5 flex gap-2">
          <Button size="sm" @click="editor.setParadigm('A')">去 A 表单工作台</Button>
          <Button variant="ghost" size="sm" @click="pair.bannerDismissed = true">知道了</Button>
        </div>
      </div>
    </div>

    <!-- ============ 冲突横幅（乐观并发，#23 ③） ============ -->
    <div v-if="editor.conflict" class="flex flex-none flex-wrap items-center gap-2.5 border-b border-destructive/40 bg-destructive/10 px-4 py-1.5 text-[13px] text-destructive">
      <span>草稿版本冲突：另一会话已保存过此故事书。</span>
      <Button size="sm" variant="outline" class="h-6 text-xs" @click="editor.conflictReload()">加载服务端版（丢弃本地改动）</Button>
      <Button size="sm" class="h-6 text-xs" @click="editor.conflictOverwrite()">保留我的并覆盖</Button>
    </div>

    <!-- ============ 主工作区 + 校验 dock ============ -->
    <div class="relative flex min-h-0 flex-1">
      <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
        <div v-if="editor.loading" class="m-auto p-10 text-center text-muted-foreground">加载草稿…</div>
        <div v-else-if="editor.loadError" class="m-auto p-10 text-center text-muted-foreground">
          <p class="mb-3">无法打开故事书：{{ editor.loadError }}</p>
          <Button variant="outline" size="sm" @click="goBack">返回列表</Button>
        </div>
        <WorkspaceA v-else-if="editor.draft && editor.paradigm === 'A'" />
        <PairC v-else-if="editor.draft && editor.paradigm === 'C'" />
        <DocumentB v-else-if="editor.draft && editor.paradigm === 'B'" />
      </div>
      <ValidationDock v-if="editor.draft" />
    </div>

    <!-- ============ 沙箱试玩弹窗 ============ -->
    <PlaytestDialog
      :open="playtestDialogOpen"
      :storybook-title="editor.draft?.meta.title ?? ''"
      :characters="editor.draft?.characters ?? []"
      :error-count="errorCount"
      :warning-count="editor.issues.filter(i => i.severity === 'warning').length"
      :busy="editor.playtesting"
      @update:open="playtestDialogOpen = $event"
      @confirm="onPlaytestConfirm"
    />
  </div>
</template>
