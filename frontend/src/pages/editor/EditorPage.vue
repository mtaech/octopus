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
import { Button } from '@/components/ui/button'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Badge } from '@/components/ui/badge'
import { IconArrowLeft, IconX, IconCircleDot, IconRocket } from '@tabler/icons-vue'

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
    <header class="flex h-12 flex-none items-center gap-3 border-b border-border bg-card/60 px-3">
      <Button variant="ghost" size="sm" class="gap-1 text-muted-foreground" @click="goBack">
        <IconArrowLeft data-icon="inline-start" />
        列表
      </Button>

      <div class="flex min-w-0 items-center gap-2">
        <span class="truncate text-sm font-bold text-foreground">{{ editor.draft?.meta.title ?? '…' }}</span>
        <IconCircleDot v-if="editor.dirty" class="size-2.5 shrink-0 text-warning" title="有未保存改动" />
        <Badge v-if="isNew" variant="outline" class="border-warning/40 text-[10px] text-warning">新草稿</Badge>
        <Badge v-if="editor.published" variant="outline" class="border-success/40 text-[10px] text-success">rev {{ editor.revision }}</Badge>
        <Badge v-else-if="editor.draft" variant="secondary" class="text-[10px]">未发布 · rev {{ editor.revision }}</Badge>
      </div>

      <span class="hidden text-xs whitespace-nowrap text-muted-foreground/60 lg:inline">{{ saveLabel }}</span>

      <!-- 范式切换（A / C / B 视图态按钮组） -->
      <nav class="ml-auto flex items-center" aria-label="编辑范式">
        <Tabs :model-value="paradigmModel" @update:model-value="paradigmModel = ($event as Paradigm)">
          <TabsList class="gap-1 border border-border/60 bg-background/60 p-0.5">
            <TabsTrigger v-for="p in PARADIGMS" :key="p.key" :value="p.key" class="h-7 rounded-md px-3 text-xs" :title="p.label + '：' + p.hint">
              {{ p.short }}
            </TabsTrigger>
          </TabsList>
        </Tabs>
      </nav>

      <Button
        variant="default"
        size="sm"
        :disabled="errorCount > 0 || editor.publishing"
        :title="pubBlockMsg || '发布当前草稿为新版次'"
        class="gap-1.5"
        @click="onPublish"
      >
        <IconRocket data-icon="inline-start" />
        <span v-if="editor.publishing">发布中…</span>
        <template v-else>发布{{ editor.dirty ? ' ●' : '' }}</template>
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
  </div>
</template>
