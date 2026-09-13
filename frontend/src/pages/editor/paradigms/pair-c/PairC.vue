<script setup lang="ts">
// PairC —— C AI 结对范式（#07 反馈修订：无头绪启发式编辑的主范式）
// 会话流 + 建议卡（待审查区，不入草稿）+ 采纳 = editor store 落稿（可多选采纳）
import { computed, onMounted, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import { usePairStore, type PendingSuggestion } from '../../stores/pair'
import { useSettingsStore } from '@/pages/list/stores/settings'
import SettingsDialog from '@/pages/list/components/SettingsDialog.vue'
import EntityPicker from '@/components/EntityPicker.vue'
import type { EntityRef } from '@/types'
import { refKindLabel } from '@/lib/entity-refs'
import { renderMarkdown } from '@/lib/markdown'
import { Conversation, ConversationContent, ConversationScrollButton } from '@/components/ai-elements/conversation'
import { MessageResponse } from '@/components/ai-elements/message'
import { Reasoning, ReasoningContent, ReasoningTrigger } from '@/components/ai-elements/reasoning'
import CodeEditor from '@/components/CodeEditor.vue'
import { estimateTokens, estimateTokensOf, fmtTokens } from '@/lib/tokens'
import { buildStorybookContext } from '@/lib/pair-context'
import { PAIR_TOOLS } from '../../stores/pair-tools'
import { confirm } from '@/lib/confirm'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  IconSend, IconSparkles, IconUser, IconCheck, IconLoader2,
  IconListCheck, IconGlassFull, IconUsers, IconBolt, IconRobot, IconSettings,
  IconArrowBackUp, IconTool, IconPlus, IconAt, IconPaperclip, IconFileText,
  IconAlertTriangle
} from '@tabler/icons-vue'
import type { PairAttachment } from '@/api'

const editor = useEditorStore()
const pair = usePairStore()
const settingsStore = useSettingsStore()
const settingsOpen = ref(false)

// ---------- 实体引用（精准指向要改的东西） ----------
const pickerOpen = ref(false)
function onPickRef(r: EntityRef): void { pair.addRef(r) }
function openPicker(): void { pickerOpen.value = true }
const pickerItems = computed<EntityRef[]>(() => editor.listEntityRefs())

// ---------- 上下文计量：与发送口径一致（@/lib/pair-context + PAIR_TOOLS） ----------
const tokenStats = computed(() => {
  const dialog = estimateTokens(pair.messages.map(m => m.role + '\n' + m.content).join('\n'))
  const storybook = estimateTokensOf(buildStorybookContext(editor.draft) ?? {})
  const tools = estimateTokensOf(PAIR_TOOLS)
  return { dialog, storybook, tools, total: dialog + storybook + tools }
})

/** 上一轮真实用量（后端 usage 优先）：出字 / 速度 / 缓存命中 / 思考量 */
const lastTurnLine = computed(() => {
  const s = pair.lastTurnStats
  if (!s) return ''
  const parts: string[] = []
  parts.push((s.measured ? '上轮出字 ' : '上轮估算出字 ') + s.outTokens + ' tok')
  parts.push(s.tps + ' tok/s')
  if (s.cachedTokens != null && s.cachedTokens > 0) {
    const rate = s.inputTokens ? Math.round((s.cachedTokens / s.inputTokens) * 100) : null
    parts.push('缓存命中 ' + (rate != null ? rate + '%' : fmtTokens(s.cachedTokens) + ' tok'))
  }
  if (s.reasoningChars) parts.push('思考 ' + s.reasoningChars + ' 字')
  if (s.reasoningTokens) parts.push('思考 ' + fmtTokens(s.reasoningTokens) + ' tok')
  if (s.finishReason === 'length') parts.push('输出预算已用尽，回答被截断')
  return parts.join(' · ')
})

/** 截断提示里那句「本轮预算」：优先报真实用量，其次报设置里的预算 */
const truncationBudgetLabel = computed(() => {
  const s = pair.lastTurnStats
  const used = s?.maxTokens ?? s?.reasoningTokens ?? 0
  return used > 0 ? fmtTokens(used) + ' tok' : ''
})

/** 继续写：填好续写指令后走与输入框同一条发送链路 */
async function onResumeTruncated(): Promise<void> {
  pair.seedResumePrompt()
  await onSend()
}

// ---------- 多会话栏（一本故事书多条线程，按主题隔离上下文） ----------
const renamingId = ref('')
const renameText = ref('')

function selectThread(id: string): void {
  void pair.switchThread(id)
}

function startRename(t: { id: string; title: string }): void {
  renamingId.value = t.id
  renameText.value = t.title
}

async function confirmRename(id: string): Promise<void> {
  const ok = await pair.renameThread(id, renameText.value)
  if (ok) renamingId.value = ''
}

async function onDeleteThread(id: string): Promise<void> {
  const t = pair.threads.find(x => x.id === id)
  const confirmed = await confirm({
    title: '删除会话「' + (t?.title ?? '') + '」？',
    description: '该会话的全部对话将被移除。',
    confirmText: '删除',
    destructive: true,
  })
  if (!confirmed) return
  await pair.deleteThread(id)
}

onMounted(async () => {
  if (!settingsStore.config) {
    await settingsStore.load()
  }
  const defaultProvider = settingsStore.config?.roles.pair?.provider_id || settingsStore.config?.roles.story?.provider_id || settingsStore.config?.providers[0]?.id || 'deepseek'
  const defaultModel = settingsStore.config?.roles.pair?.model || settingsStore.config?.roles.story?.model || settingsStore.config?.providers[0]?.models[0]?.id || 'deepseek-chat'
  pair.initModel(defaultProvider, defaultModel)
})

// 可用供应商及模型
const providers = computed(() => settingsStore.config?.providers ?? [])

// 当前选中的模型完整 key (格式: provider_id::model_id)
const currentModelValue = computed(() => {
  if (pair.selectedProviderId && pair.selectedModel) {
    return `${pair.selectedProviderId}::${pair.selectedModel}`
  }
  const role = settingsStore.config?.roles.pair || settingsStore.config?.roles.story
  if (role?.provider_id && role?.model) {
    return `${role.provider_id}::${role.model}`
  }
  return ''
})

// 当前选中模型的展示名称
const currentModelDisplay = computed(() => {
  const pId = pair.selectedProviderId || settingsStore.config?.roles.pair?.provider_id || settingsStore.config?.roles.story?.provider_id
  const mId = pair.selectedModel || settingsStore.config?.roles.pair?.model || settingsStore.config?.roles.story?.model
  if (!pId || !mId) return { providerName: '默认', modelName: 'deepseek-chat' }
  const p = providers.value.find(x => x.id === pId)
  const m = p?.models.find(x => x.id === mId)
  return {
    providerName: p?.label || pId,
    modelName: m?.name || mId,
  }
})

function onModelChange(val: unknown) {
  if (typeof val !== 'string') return
  const [providerId, modelId] = val.split('::')
  if (providerId && modelId) {
    pair.setModel(providerId, modelId)
    if (settingsStore.config) {
      if (!settingsStore.config.roles.pair) {
        settingsStore.config.roles.pair = {
          provider_id: providerId,
          model: modelId,
          temperature: 0.8,
          max_tokens: 4096,
        }
      } else {
        settingsStore.config.roles.pair.provider_id = providerId
        settingsStore.config.roles.pair.model = modelId
      }
      void settingsStore.save()
    }
  }
}

const sendBtn = ref<HTMLButtonElement | null>(null)

// ---------- 文件附件（文本类）：随消息交给 AI ----------
const pendingAttachments = ref<PairAttachment[]>([])
const attachInput = ref<HTMLInputElement | null>(null)
const ATTACH_ACCEPT = '.txt,.md,.markdown,.json,.yaml,.yml,.csv,.tsv,.html,.htm,.xml,.log,.ts,.js,.py,.lua,.rs,.toml,.ini,.conf,.css,.sql'
const ATTACH_EXT = /\.(txt|md|markdown|json|ya?ml|csv|tsv|html?|xml|log|ts|js|py|lua|rs|toml|ini|conf|css|sql)$/i
const ATTACH_MAX_BYTES = 512 * 1024
function pickAttach(): void { attachInput.value?.click() }
function removeAttachment(i: number): void { pendingAttachments.value.splice(i, 1) }
async function onAttach(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement
  const files = Array.from(input.files ?? [])
  input.value = ''
  const { toast } = await import('@/api')
  for (const f of files) {
    if (!ATTACH_EXT.test(f.name)) { toast('warn', `跳过「${f.name}」：暂只支持文本类文件`); continue }
    if (f.size > ATTACH_MAX_BYTES) { toast('warn', `跳过「${f.name}」：超过 512KB`); continue }
    if (pendingAttachments.value.some(a => a.name === f.name)) { toast('warn', `已有同名附件「${f.name}」`); continue }
    const text = await f.text()
    if (!text.trim()) { toast('warn', `跳过「${f.name}」：内容为空`); continue }
    pendingAttachments.value.push({ name: f.name, size: f.size, text })
  }
}

// 自动滚到底交给 ai-elements 的 Conversation（底层 vue-stick-to-bottom），
// 切换会话 / 流式出字都自动保持在底部，并带「回到底部」按钮。

/** 发送：接入编辑器工具执行器；工具只写暂存草稿，批准后才合并 */
async function onSend(): Promise<void> {
  const atts = pendingAttachments.value
  const ok = await pair.send(editor.draft, {
    beginTurn: () => editor.beginToolTurn(),
    currentDraft: () => editor.stagedOrLive(),
    focus: (refs) => editor.resolveFocus(refs),
    runTool: (name, args) => editor.runToolCall(name, args),
    finishTurn: () => editor.finishToolTurn(),
    discardTurn: () => editor.discardStagedTurn(),
    apply: (list) => editor.applySuggestions(list),
  }, atts.length ? [...atts] : undefined)
  if (ok) pendingAttachments.value = []
}
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void onSend()
    return
  }
  // @ 作为引用选择器的快捷键（中文输入法下也走同一个按钮入口）
  if (e.key === '@') {
    e.preventDefault()
    openPicker()
  }
}

/** 新开会话：新建一条独立线程（不再销毁旧会话），自动激活（#22 ①） */
function onNewSession(): void {
  void pair.createThread()
}

// ---------- 批准门：默认全选，逐张可取消 ----------
const deselected = ref<Record<string, boolean>>({})
function isSelected(id: string): boolean { return !deselected.value[id] }
function toggleSelected(id: string): void { deselected.value = { ...deselected.value, [id]: !deselected.value[id] } }
const selectedList = computed(() => pair.suggestions.filter(s => isSelected(s.id)))

/** 应用单张：批准后才合并进草稿 */
function adopt(s: PendingSuggestion): void {
  const ok = pair.adopt(s)
  if (ok) import('@/api').then(({ toast }) => toast('ok', '已应用并写入草稿'))
  else import('@/api').then(({ toast }) => toast('warn', '应用失败：该改动与当前草稿不匹配'))
}

/** 应用所有勾选的改动（整批一次撤销快照） */
async function applySelectedNow(): Promise<void> {
  const list = selectedList.value
  if (!list.length) return
  const r = pair.applySelected(list.map(s => s.id))
  const { toast } = await import('@/api')
  if (r.applied) {
    toast('ok', '已应用 ' + r.applied + ' 处改动' + (r.failed.length ? '，' + r.failed.length + ' 处失败' : ''))
  } else {
    toast('warn', r.failed[0] ?? '没有可应用的改动')
  }
}

/** 放弃全部待审查改动：草稿保持不变 */
function discardAll(): void {
  pair.discardPending()
  import('@/api').then(({ toast }) => toast('info', '已放弃本轮改动，草稿未变'))
}
/** 撤销本轮已应用的改动（批准之后才存在） */
function onUndoTurn(): void {
  const n = editor.undoToMark(pair.lastTurnMark ?? 0)
  pair.turnToolCount = 0
  if (n > 0) {
    import('@/api').then(({ toast }) => toast('ok', '已撤销本轮 ' + n + ' 处改动'))
  }
}

function fmtRole(r: string): string { return r === 'user' ? '你' : 'Octo 结对' }

const actionLabel = (a: string): string => a === 'create' ? '新建' : a === 'update' ? '更新' : '删除'
const actionBadgeCls = (a: string): string => a === 'delete' ? 'text-destructive border-destructive/40' : a === 'update' ? 'text-warning border-warning/40' : 'text-success border-success/40'
const canSend = computed(() => pair.sending || !pair.input.trim())
</script>

<template>
  <div class="flex h-full min-h-0">
    <!-- 会话栏：一本故事书多条会话，按主题隔离上下文 -->
    <aside class="flex w-52 flex-none flex-col border-r border-border bg-card/20">
      <div class="flex h-10 flex-none items-center justify-between border-b border-border/70 px-3">
        <span class="text-[11px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">会话</span>
        <Button variant="ghost" size="icon-xs" class="size-6 text-muted-foreground hover:text-foreground" title="新建会话" @click="onNewSession">
          <IconPlus class="size-3.5" />
        </Button>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto p-2">
        <div
          v-for="t in pair.threads"
          :key="t.id"
          class="group mb-1 cursor-pointer rounded-lg border px-2.5 py-2 transition-colors"
          :class="t.id === pair.activeThreadId
            ? 'border-primary/45 bg-primary/10'
            : 'border-transparent hover:border-border/70 hover:bg-muted/40'"
          @click="selectThread(t.id)"
        >
          <div v-if="renamingId === t.id" class="flex items-center gap-1">
            <input
              v-model="renameText"
              class="h-6 w-full rounded border border-border bg-background px-1.5 text-[12px] text-foreground outline-none focus:border-primary/60"
              @click.stop
              @keyup.enter="confirmRename(t.id)"
              @keyup.esc="renamingId = ''"
            />
          </div>
          <template v-else>
            <div class="flex items-center gap-1.5">
              <span class="min-w-0 flex-1 truncate text-[12.5px] font-medium" :class="t.id === pair.activeThreadId ? 'text-primary' : 'text-foreground/85'">{{ t.title }}</span>
              <span class="flex-none text-[10px] text-muted-foreground/60">{{ t.message_count }}</span>
            </div>
            <div class="mt-0.5 flex items-center gap-2 opacity-0 transition-opacity group-hover:opacity-100">
              <button type="button" class="text-[10px] text-muted-foreground/70 transition-colors hover:text-foreground" @click.stop="startRename(t)">重命名</button>
              <button type="button" class="text-[10px] text-muted-foreground/70 transition-colors hover:text-destructive" @click.stop="onDeleteThread(t.id)">删除</button>
            </div>
          </template>
        </div>
        <p v-if="!pair.threads.length" class="px-1 py-3 text-center text-[11px] text-muted-foreground/60">暂无会话</p>
      </div>
      <!-- 上下文计量：直接报 token（对话 + 故事书 + 工具）；不再有固定条数窗口 -->
      <div class="flex-none border-t border-border/70 px-3 py-2 text-[10.5px] text-muted-foreground/70">
        <div class="flex items-center justify-between">
          <span>上下文估算</span>
          <span class="font-mono font-semibold text-foreground/85">{{ fmtTokens(tokenStats.total) }} tok</span>
        </div>
        <p class="mt-0.5 font-mono text-[10px] text-muted-foreground/55">
          {{ pair.messages.length }} 条 · 对话 {{ fmtTokens(tokenStats.dialog) }} · 故事书 {{ fmtTokens(tokenStats.storybook) }} · 工具 {{ fmtTokens(tokenStats.tools) }}
        </p>
        <p v-if="lastTurnLine" class="mt-0.5 font-mono text-[10px] text-muted-foreground/55">
          {{ lastTurnLine }}
        </p>
      </div>
    </aside>

    <!-- 左：会话流 -->
    <div class="flex min-w-0 flex-1 flex-col">
      <div class="flex h-10 flex-none items-center gap-2 border-b border-border bg-card/30 px-3 backdrop-blur-xs">
        <span
          class="flex shrink-0 items-center gap-1.5 whitespace-nowrap text-[13px] font-extrabold tracking-wide text-primary"
          title="边聊边成型：AI 的改动先入「待审查」，批准后才写入草稿"
        >
          <IconSparkles class="size-4" />
          Octo · AI 结对
        </span>

        <!-- 结对 AI 模型选择与快速配置（窄列下必须可收缩，不能把文字挤成竖排） -->
        <div class="ml-auto flex min-w-0 items-center gap-1">
          <Select :model-value="currentModelValue" @update:model-value="onModelChange">
            <SelectTrigger
              size="sm"
              class="min-w-0 max-w-[190px] gap-1.5 border border-border bg-card/80 px-2 text-xs font-semibold shadow-2xs focus:ring-0 hover:bg-muted/50 cursor-pointer"
              :title="'模型：' + currentModelDisplay.modelName + ' · ' + currentModelDisplay.providerName"
            >
              <IconRobot class="size-3.5 shrink-0 text-primary" />
              <span class="min-w-0 flex-1 truncate text-left text-foreground">
                {{ currentModelDisplay.modelName }}
              </span>
            </SelectTrigger>
              <SelectContent class="z-50 max-h-72">
                <template v-if="providers.length">
                  <SelectGroup v-for="p in providers" :key="p.id">
                    <SelectLabel class="text-[10.5px] font-bold text-muted-foreground uppercase px-2 py-1">
                      {{ p.label }}
                    </SelectLabel>
                    <SelectItem
                      v-for="m in p.models"
                      :key="p.id + '::' + m.id"
                      :value="p.id + '::' + m.id"
                      class="text-xs cursor-pointer"
                    >
                      <div class="flex items-center justify-between gap-4 w-full">
                        <span>{{ m.name || m.id }}</span>
                        <span class="font-mono text-[10px] text-muted-foreground/70">{{ m.id }}</span>
                      </div>
                    </SelectItem>
                  </SelectGroup>
                </template>
                <div v-else class="p-2 text-center text-xs text-muted-foreground">
                  尚未配置供应商，点击右侧设置添加
                </div>
              </SelectContent>
          </Select>

          <Button
            variant="ghost"
            size="icon-xs"
            class="size-7 shrink-0 text-muted-foreground hover:text-foreground cursor-pointer"
            title="打开模型与 Provider 设置"
            @click="settingsOpen = true"
          >
            <IconSettings class="size-3.5" />
          </Button>
        </div>
      </div>

      <Conversation class="min-h-0 flex-1">
        <ConversationContent class="gap-4 px-5 py-5">
        <div v-if="!pair.messages.length" class="rounded-2xl border border-border/80 bg-card/60 p-5 text-[13px] leading-relaxed text-muted-foreground backdrop-blur-xs">
          <div class="flex items-center gap-2 text-foreground font-semibold text-base mb-2">
            <IconSparkles class="size-4 text-primary" />
            与 Octo 一起构思世界
          </div>
          <p class="text-xs text-muted-foreground/85">没有头绪？直接告诉 Octo 你的想法，它会为你即兴生成角色、地点、技能等结构化建议。</p>
          <div class="mt-4 flex flex-col gap-2">
            <span class="text-[11px] font-semibold text-muted-foreground/60 uppercase tracking-wider">点击快捷尝试：</span>
            <div class="flex flex-wrap gap-2">
              <button
                type="button"
                class="inline-flex items-center gap-1.5 rounded-lg border border-border/80 bg-muted/40 px-3 py-1.5 text-left text-xs text-foreground/90 transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-primary cursor-pointer"
                @click="pair.input = '我想在这个世界里加一个暴风雪夜的藏身酒馆，里面有位神秘老板'"
              >
                <IconGlassFull class="size-3.5 flex-none text-primary" />
                <span>「想加一个暴风雪夜的藏身酒馆」</span>
              </button>
              <button
                type="button"
                class="inline-flex items-center gap-1.5 rounded-lg border border-border/80 bg-muted/40 px-3 py-1.5 text-left text-xs text-foreground/90 transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-primary cursor-pointer"
                @click="pair.input = '帮我设计两个性格迥异的镇民，一个热心老猎人，一个怀揣禁书的年轻学者'"
              >
                <IconUsers class="size-3.5 flex-none text-primary" />
                <span>「设计两个性格迥异的镇民人物」</span>
              </button>
              <button
                type="button"
                class="inline-flex items-center gap-1.5 rounded-lg border border-border/80 bg-muted/40 px-3 py-1.5 text-left text-xs text-foreground/90 transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-primary cursor-pointer"
                @click="pair.input = '为这本故事书增加一幕高潮场景：在旧神遗迹揭开古老封印'"
              >
                <IconBolt class="size-3.5 flex-none text-primary" />
                <span>「增加一幕高潮场景与剧情触发点」</span>
              </button>
            </div>
          </div>
        </div>

        <div v-for="(m, i) in pair.messages" :key="i" class="flex max-w-[84%] flex-col gap-1.5" :class="m.role === 'user' ? 'ml-auto items-end' : 'items-start'">
          <div class="flex items-center gap-1.5 px-1 text-[11px] font-medium tracking-wide text-muted-foreground/75">
            <IconUser v-if="m.role === 'user'" class="size-3 text-muted-foreground" />
            <IconSparkles v-else class="size-3 text-primary" />
            {{ fmtRole(m.role) }}
            <span v-if="m.role === 'assistant'" class="text-[10px] text-muted-foreground/50 font-mono">· {{ m.model || currentModelDisplay.modelName }}</span>
          </div>
          <!-- 用户消息的实体引用（本次改动的目标） -->
          <div v-if="m.refs && m.refs.length" class="flex flex-wrap justify-end gap-1">
            <span
              v-for="(r, ri) in m.refs"
              :key="ri"
              class="inline-flex items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-2 py-0.5 text-[10.5px] text-primary"
            >
              {{ refKindLabel(r.kind) }} · {{ r.name }}
            </span>
          </div>
          <!-- 用户消息的附件 -->
          <div v-if="m.attachments && m.attachments.length" class="flex flex-wrap justify-end gap-1">
            <span
              v-for="(a, ai) in m.attachments"
              :key="ai"
              class="inline-flex items-center gap-1 rounded-full border border-border/80 bg-muted/50 px-2 py-0.5 text-[10.5px] text-foreground/75"
              :title="a.name"
            >
              <IconFileText class="size-3 shrink-0 text-muted-foreground" />
              <span class="max-w-[10rem] truncate">{{ a.name }}</span>
            </span>
          </div>
          <div
            class="rounded-2xl px-4 py-3 text-[13.5px] leading-relaxed break-words shadow-xs"
            :class="[
              m.role === 'user'
                ? 'bubble-user rounded-br-xs bg-primary text-primary-foreground border border-primary/40'
                : m.isError
                  ? 'rounded-bl-xs border border-destructive/50 bg-destructive/10 text-foreground'
                  : 'rounded-bl-xs border border-border/80 bg-card/90 text-foreground'
            ]"
          >
            <!-- 思考流（reasoning_content）：流式时展开，回答开始后自动收起 -->
            <Reasoning
              v-if="m.role === 'assistant' && m.reasoning"
              :is-streaming="!!(pair.sending && !m.content)"
              class="mb-3"
            >
              <ReasoningTrigger />
              <ReasoningContent :content="m.reasoning" />
            </Reasoning>
            <template v-if="m.role === 'assistant' && !m.content && pair.sending && i === pair.messages.length - 1">
              <span class="inline-flex items-center gap-2 text-muted-foreground text-xs">
                <IconLoader2 class="size-3.5 animate-spin text-primary" />
                Octo 正在推演设定…
              </span>
            </template>
            <MessageResponse v-else :content="m.content" class="text-[13.5px] leading-relaxed" />
            <div v-if="m.tools && m.tools.length" class="mt-2.5 space-y-1 border-t border-border/50 pt-2">
              <div
                v-for="(tc, ti) in m.tools"
                :key="ti"
                class="flex items-start gap-1.5 text-[11px] leading-4"
                :class="tc.ok ? 'text-muted-foreground' : 'text-destructive'"
              >
                <IconTool class="mt-0.5 size-3 shrink-0" />
                <span>{{ tc.ok ? (m.turnState === 'staged' ? '待批准 · ' : m.turnState === 'discarded' ? '已放弃 · ' : m.turnState === 'applied' ? '已应用 · ' : '') + tc.label : '失败 · ' + tc.label }}</span>
              </div>
              <div v-if="m.turnState === 'discarded'" class="pt-1 text-[11px] text-muted-foreground/70">本轮改动已放弃，草稿未变。</div>
            </div>
            <!-- 输出预算用尽：正文断在半句话上。必须显式说明，并给一条继续写的出路。 -->
            <div
              v-if="m.role === 'assistant' && m.truncated"
              class="mt-2.5 flex flex-wrap items-center gap-2 border-t border-warning/30 pt-2"
            >
              <span class="flex items-center gap-1.5 text-[11px] leading-4 text-warning">
                <IconAlertTriangle class="size-3.5 shrink-0" />
                回答被输出预算截断（{{ truncationBudgetLabel }}）——上面可能停在半句话上。思考与正文共享这份预算。
              </span>
              <Button
                v-if="i === pair.messages.length - 1"
                size="xs"
                variant="outline"
                class="h-6 gap-1 border-warning/50 text-[11px] text-foreground hover:bg-warning/10"
                :disabled="pair.sending"
                @click="onResumeTruncated()"
              >
                <IconArrowBackUp class="size-3" />
                继续写完
              </Button>
              <button
                type="button"
                class="text-[11px] text-muted-foreground/70 underline decoration-dotted hover:text-foreground"
                @click="settingsOpen = true"
              >
                调大输出预算
              </button>
            </div>
            <div v-if="m.isError" class="mt-2.5 pt-2 border-t border-destructive/20 flex items-center justify-between gap-2">
              <span class="text-xs text-muted-foreground">未配置有效密钥或服务连接失败</span>
              <Button size="xs" variant="outline" class="border-destructive/40 text-foreground hover:bg-destructive/10 cursor-pointer" @click="settingsOpen = true">
                <IconSettings class="size-3 mr-1" />
                打开设置
              </Button>
            </div>
          </div>
        </div>
        </ConversationContent>
        <ConversationScrollButton />
      </Conversation>

      <div
        v-if="pair.stagedTurn || editor.undoDepth > 0"
        class="flex flex-none flex-wrap items-center gap-2 border-t border-border/60 px-4 py-1.5"
        :class="pair.stagedTurn ? 'bg-primary/5' : 'bg-muted/30'"
      >
        <span v-if="pair.stagedTurn" class="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
          <IconListCheck class="size-3.5 text-primary" />
          本轮 AI 拟改动 {{ pair.turnToolCount }} 处，<strong class="text-foreground">尚未写入草稿</strong> — 请在右侧审查后批准
        </span>
        <span v-else class="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
          <IconArrowBackUp class="size-3.5" />
          草稿已写入改动，可逐步或整轮撤销
        </span>
        <div class="ml-auto flex items-center gap-1.5">
          <template v-if="pair.stagedTurn">
            <Button variant="outline" size="xs" class="h-6 gap-1 text-[11px]" @click="discardAll()">放弃本轮</Button>
            <Button size="xs" class="h-6 gap-1 text-[11px]" :disabled="!selectedList.length" @click="applySelectedNow()">
              <IconCheck class="size-3" />
              应用选中（{{ selectedList.length }}）
            </Button>
          </template>
          <template v-else>
            <Button variant="outline" size="xs" class="h-6 gap-1 text-[11px]" :disabled="editor.undoDepth === 0" @click="editor.undoLastTool()">
              <IconArrowBackUp class="size-3" />
              撤销一步
            </Button>
            <Button v-if="editor.undoDepth > 0" variant="outline" size="xs" class="h-6 gap-1 text-[11px]" @click="onUndoTurn()">
              <IconArrowBackUp class="size-3" />
              撤销本轮
            </Button>
          </template>
        </div>
      </div>

      <div class="flex flex-none flex-col gap-2 border-t border-border bg-card/40 px-4 py-3 backdrop-blur-xs">
        <!-- 本次目标（实体引用 chips） -->
        <div v-if="pair.pendingRefs.length" class="flex flex-wrap items-center gap-1.5">
          <span class="text-[11px] font-medium text-muted-foreground">本次目标：</span>
          <span
            v-for="(r, ri) in pair.pendingRefs"
            :key="ri"
            class="inline-flex items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-2 py-0.5 text-[11px] text-primary"
          >
            {{ refKindLabel(r.kind) }} · {{ r.name }}
            <button type="button" class="cursor-pointer text-primary/60 transition-colors hover:text-primary" title="移除" @click="pair.removeRef(r)">×</button>
          </span>
          <button type="button" class="cursor-pointer text-[10.5px] text-muted-foreground/70 transition-colors hover:text-foreground" @click="pair.clearRefs()">清空</button>
        </div>
        <!-- 附件 chips（文本类文件，随消息交给 AI） -->
        <div v-if="pendingAttachments.length" class="flex flex-wrap items-center gap-1.5">
          <span class="text-[11px] font-medium text-muted-foreground">附件：</span>
          <span
            v-for="(a, ai) in pendingAttachments"
            :key="ai"
            class="inline-flex items-center gap-1 rounded-full border border-border/80 bg-muted/50 px-2 py-0.5 text-[11px] text-foreground/80"
            :title="a.name"
          >
            <IconFileText class="size-3 shrink-0 text-muted-foreground" />
            <span class="max-w-[10rem] truncate">{{ a.name }}</span>
            <span class="font-mono text-[9.5px] text-muted-foreground/60">{{ a.size ? Math.max(1, Math.round(a.size / 1024)) : a.text.length }}K</span>
            <button type="button" class="cursor-pointer text-muted-foreground/60 transition-colors hover:text-destructive" title="移除" @click="removeAttachment(ai)">×</button>
          </span>
        </div>
        <div class="rounded-xl border border-border bg-background/80 transition-[border-color,box-shadow] focus-within:border-primary/60 focus-within:ring-2 focus-within:ring-primary/15">
          <Textarea
            v-model="pair.input"
            rows="3"
            class="min-h-[3.5rem] max-h-40 w-full resize-none border-0 bg-transparent px-3 py-2 text-[13.5px] leading-relaxed shadow-none focus-visible:ring-0"
            placeholder="向 Octo 描述你的想法或要求…"
            @keydown="onKey"
          />
          <div class="flex items-center gap-1 border-t border-border/60 px-1.5 py-1">
            <Button
              variant="ghost"
              size="xs"
              class="h-6 shrink-0 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground"
              title="引用故事书实体（也可直接输入 @）"
              @click="openPicker()"
            >
              <IconAt class="size-3.5" />
              引用
            </Button>
            <Button
              variant="ghost"
              size="xs"
              class="h-6 shrink-0 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground"
              title="上传文本文件（.txt/.md/.json/代码等）作为附件，随消息交给 AI"
              @click="pickAttach()"
            >
              <IconPaperclip class="size-3.5" />
              附件
            </Button>
            <input ref="attachInput" type="file" multiple class="hidden" :accept="ATTACH_ACCEPT" @change="onAttach" />
            <span class="hidden min-w-0 flex-1 truncate text-[10.5px] text-muted-foreground/55 lg:inline">
              Enter 发送 · Shift+Enter 换行
            </span>
            <Button ref="sendBtn" size="sm" class="ml-auto h-7 shrink-0 gap-1 px-3 font-semibold shadow-xs" :disabled="canSend" @click="onSend">
              <IconLoader2 v-if="pair.sending" class="size-3.5 animate-spin" />
              <IconSend v-else class="size-3.5" />
              {{ pair.sending ? '发送中' : '发送' }}
            </Button>
          </div>
        </div>
      </div>
    </div>

    <!-- 右：待审查建议区（与对话区并列） -->
    <aside class="flex w-84 flex-none flex-col border-l border-border bg-card/30 backdrop-blur-xs">
      <div class="flex h-10 flex-none items-center gap-2 border-b border-border px-4">
        <span class="flex items-center gap-1.5 text-xs font-extrabold tracking-wide text-foreground">
          <IconListCheck class="size-4 text-primary" />
          待审查改动
        </span>
        <Badge v-if="pair.suggestions.length" variant="outline" class="border-primary/40 bg-primary/10 px-1.5 text-[11px] text-primary">{{ pair.suggestions.length }}</Badge>
        <div v-if="pair.suggestions.length" class="ml-auto flex items-center gap-1">
          <Button variant="ghost" size="sm" class="h-7 gap-1 px-2.5 text-xs font-medium text-muted-foreground hover:text-foreground" @click="discardAll()">
            放弃
          </Button>
          <Button variant="ghost" size="sm" class="h-7 gap-1 px-2.5 text-xs font-medium text-primary hover:bg-primary/10" :disabled="!selectedList.length" @click="applySelectedNow()">
            <IconCheck data-icon="inline-start" class="size-3.5" />
            应用选中（{{ selectedList.length }}）
          </Button>
        </div>
      </div>
      <div class="min-h-0 flex-1 space-y-2.5 overflow-y-auto p-3.5">
        <p v-if="!pair.suggestions.length" class="px-2 py-6 text-center text-xs leading-5 text-muted-foreground/60">
          AI 的改动会先出现在这里，<strong class="text-foreground/80">批准后才写入草稿</strong>。<br />
          可逐张应用，或勾选后整批应用。
        </p>
        <Card v-for="s in pair.suggestions" :key="s.id" class="gap-2 overflow-hidden rounded-xl border-border bg-card/90 p-3.5 shadow-xs transition-all hover:border-primary/50">
          <div class="flex items-center gap-1.5">
            <input
              type="checkbox"
              class="size-3.5 shrink-0 cursor-pointer accent-primary"
              :checked="isSelected(s.id)"
              @change="toggleSelected(s.id)"
            />
            <Badge variant="outline" class="border-border/60 px-1.5 py-0 text-[10px] font-semibold" :class="actionBadgeCls(s.action)">{{ actionLabel(s.action) }}</Badge>
            <span class="truncate text-[10.5px] font-medium tracking-wide text-muted-foreground/70">{{ refKindLabel(s.target.kind) }}</span>
            <div class="ml-auto flex shrink-0 items-center gap-1">
              <Button
                size="xs"
                variant="ghost"
                class="gap-1 px-2 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                :disabled="!!s.applying"
                title="不采纳这条改动（草稿不变）"
                @click="pair.discardOne(s.id)"
              >
                弃用
              </Button>
              <Button
                size="xs"
                variant="outline"
                class="gap-1 px-2.5 font-medium shadow-2xs"
                :disabled="!!s.applying"
                @click="adopt(s)"
              >
                <IconLoader2 v-if="s.applying" class="size-3 animate-spin" />
                <IconCheck v-else data-icon="inline-start" class="size-3" />
                应用
              </Button>
            </div>
          </div>
          <div class="text-[13px] leading-snug font-bold text-foreground">{{ s.label }}</div>
          <!-- 可读字段列表：只给人话，不给 JSON -->
          <div v-if="s.details?.length" class="flex flex-col gap-0.5">
            <div v-for="(d, di) in s.details" :key="di" class="flex gap-1.5 text-[11.5px] leading-relaxed">
              <span class="w-14 shrink-0 text-muted-foreground/55">{{ d.label }}</span>
              <span class="min-w-0 flex-1 break-words text-foreground/85">{{ d.value }}</span>
            </div>
          </div>
          <div v-else-if="s.summary" class="md-body text-[11.5px] leading-relaxed text-muted-foreground/85" v-html="renderMarkdown(s.summary)"></div>
          <div v-if="typeof s.patch?.lua === 'string' && s.patch.lua" class="mt-1.5">
            <div class="mb-1 text-[11px] font-semibold tracking-wider text-muted-foreground/70">Lua 脚本（只读预览）</div>
            <CodeEditor :model-value="String(s.patch.lua)" language="lua" readonly min-height="3.5rem" max-height="12rem" />
          </div>
        </Card>
      </div>
    </aside>

    <!-- 设置弹窗 -->
    <SettingsDialog v-model:open="settingsOpen" />

    <!-- 实体引用选择器 -->
    <EntityPicker v-model:open="pickerOpen" :items="pickerItems" @select="onPickRef" />
  </div>
</template>
