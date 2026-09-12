<script setup lang="ts">
// LuaWorkbenchDialog —— 沉浸式 Lua 脚本开发与沙箱调试工作台（#02 / #12 / #23）
// 提供：专业级代码编辑、预设模板库一键套用、host.* 交互式 API 手册、
// 当前故事书实体（属性/资源/标记）快速插入、多角色参数配置以及真机沙箱试跑调试。
import { computed, ref, watch } from 'vue'
import type { CharacterDef, Storybook } from '@/types'
import { runLua, type LuaRunRequest, type LuaRunResult } from '@/api'
import { LUA_TEMPLATES, type LuaTemplate } from '@/lib/lua-templates'
import { LUA_API_REFERENCE, buildLuaContext, type LuaCompletionContext, type LuaApiEntry } from '@/lib/lua-context'
import CodeEditor from '@/components/CodeEditor.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/components/ui/tabs'
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  IconPlayerPlay,
  IconCode,
  IconBook2,
  IconSparkles,
  IconCheck,
  IconAlertTriangle,
  IconFlame,
  IconCoins,
  IconDice,
  IconTrash,
  IconCopy,
  IconArrowRight,
  IconShieldCheck,
  IconTerminal2,
  IconHelpCircle,
  IconArrowsExchange,
  IconTarget,
  IconHourglass,
  IconLoader2,
  IconX,
} from '@tabler/icons-vue'

const props = withDefaults(defineProps<{
  open: boolean
  title?: string
  modelValue: string
  mode?: 'hook' | 'check' | 'condition'
  mount?: string
  storybook?: Storybook | null
  context?: LuaCompletionContext
  skill?: unknown
}>(), {
  title: 'Lua 扩展脚本',
  mode: 'hook',
  mount: 'pre_resolve',
  storybook: null,
})

const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
  (e: 'update:modelValue', value: string): void
  (e: 'update:mount', value: string): void
}>()

const editorRef = ref<InstanceType<typeof CodeEditor> | null>(null)
const scriptCode = ref(props.modelValue ?? '')
const currentMount = ref(props.mount ?? 'pre_resolve')
const assistantTab = ref<'templates' | 'api' | 'entities' | 'rules'>('templates')

watch(() => props.modelValue, (val) => {
  scriptCode.value = val ?? ''
})

watch(() => props.mount, (val) => {
  currentMount.value = val ?? 'pre_resolve'
})

function updateCode(val: string): void {
  scriptCode.value = val
  emit('update:modelValue', val)
}

function updateMount(val: string): void {
  currentMount.value = val
  emit('update:mount', val)
}

// ---------------- 故事书上下文实体 ----------------
const luaContext = computed<LuaCompletionContext>(() => props.context ?? buildLuaContext(props.storybook))

// ---------------- 模板库过滤 ----------------
const templateCategory = ref<'all' | 'hook' | 'check' | 'condition'>(props.mode ?? 'all')
const templateSearch = ref('')

const filteredTemplates = computed<LuaTemplate[]>(() => {
  return LUA_TEMPLATES.filter(t => {
    const matchCat = templateCategory.value === 'all' || t.category === templateCategory.value
    const matchSearch = !templateSearch.value.trim() ||
      t.title.includes(templateSearch.value) ||
      t.description.includes(templateSearch.value) ||
      t.tags.some(tag => tag.includes(templateSearch.value))
    return matchCat && matchSearch
  })
})

function applyTemplate(tpl: LuaTemplate, append = false): void {
  if (append) {
    const newCode = scriptCode.value.trim() ? `${scriptCode.value}\n\n${tpl.code}` : tpl.code
    updateCode(newCode)
  } else {
    updateCode(tpl.code)
  }
  if (tpl.mount && props.mode === 'hook') {
    updateMount(tpl.mount)
  }
}

// ---------------- API 参考手册 ----------------
const apiSearch = ref('')
const filteredApis = computed(() => {
  if (!apiSearch.value.trim()) return LUA_API_REFERENCE
  const q = apiSearch.value.toLowerCase()
  return LUA_API_REFERENCE.filter(a =>
    a.name.toLowerCase().includes(q) ||
    a.desc.toLowerCase().includes(q) ||
    a.signature.toLowerCase().includes(q)
  )
})

function insertSnippet(text: string): void {
  if (editorRef.value) {
    editorRef.value.insertText(text)
  } else {
    updateCode(`${scriptCode.value}\n${text}`)
  }
}

const WORKBENCH_SNIPPETS = [
  { label: '扣除消耗', icon: IconCoins, iconClass: 'text-warning', code: "host.request_cost('res-stamina', 2)" },
  { label: '施加状态', icon: IconSparkles, iconClass: 'text-purple-500', code: "host.apply_status(host.target.id, 'burn', 3, 'turns')" },
  { label: '确定性骰点', icon: IconDice, iconClass: 'text-primary', code: "local roll = host.engine_rng(1, 20)" },
  { label: '触发事件', icon: IconFlame, iconClass: 'text-emerald-500', code: "host.trigger_event('scene_change')" },
  { label: '跨回合存储', icon: IconHourglass, iconClass: 'text-blue-500', code: "host.storage.combo = (host.storage.combo or 0) + 1" },
  { label: '调试输出', icon: IconTerminal2, iconClass: 'text-muted-foreground', code: "host.log('调试信息')" },
]

// ---------------- 静态安全检测 ----------------
const LUA_FORBIDDEN = [
  { term: 'math.random', tip: '需替换为确定性随机源 host.engine_rng(min, max)' },
  { term: 'io.', tip: '沙箱禁止系统 IO 操作' },
  { term: 'os.', tip: '沙箱禁止操作系统级调用' },
  { term: 'package.', tip: '沙箱禁止动态模块包管理' },
  { term: 'require', tip: '沙箱禁止动态加载外部库' },
  { term: 'debug.', tip: '沙箱禁止调试反射 API' },
  { term: 'collectgarbage', tip: '沙箱托管垃圾回收' },
  { term: 'coroutine.', tip: '沙箱禁止未受控协程' },
]

const lintViolations = computed(() => {
  const hits: { term: string; tip: string }[] = []
  for (const f of LUA_FORBIDDEN) {
    if (scriptCode.value.includes(f.term)) {
      hits.push(f)
    }
  }
  return hits
})

function fixForbiddenApi(term: string): void {
  if (term === 'math.random') {
    updateCode(scriptCode.value.replace(/math\.random\s*\(([^)]*)\)/g, 'host.engine_rng($1)'))
  }
}

// ---------------- 交互式沙箱试跑参数 ----------------
const characters = computed<CharacterDef[]>(() => props.storybook?.characters ?? [])
const selectedActorId = ref<string>('')
const selectedTargetId = ref<string>('')
const testDifficulty = ref<number>(15)
const testRound = ref<number>(1)
const testSceneId = ref<string>('')

watch(() => props.open, (open) => {
  if (open && characters.value.length && !selectedActorId.value) {
    selectedActorId.value = characters.value[0]?.id ?? ''
    selectedTargetId.value = characters.value[1]?.id ?? ''
    testSceneId.value = props.storybook?.world?.locations?.[0]?.id ?? 'loc-tavern'
  }
}, { immediate: true })

const selectedActor = computed(() => characters.value.find(c => c.id === selectedActorId.value) ?? null)
const selectedTarget = computed(() => characters.value.find(c => c.id === selectedTargetId.value) ?? null)

const running = ref(false)
const runResult = ref<LuaRunResult | null>(null)
const runTimeMs = ref<number | null>(null)

async function executeSandbox(): Promise<void> {
  if (!scriptCode.value.trim() || running.value) return
  running.value = true
  runResult.value = null
  const t0 = performance.now()
  try {
    const req: LuaRunRequest = {
      script: scriptCode.value,
      mode: props.mode,
      mount: currentMount.value,
      skill: props.skill,
      actor: selectedActor.value ? {
        id: selectedActor.value.id,
        name: selectedActor.value.name,
        attributes: selectedActor.value.attributes ?? {},
        resources: selectedActor.value.resources ?? {},
        statuses: [],
      } : undefined,
      target: selectedTarget.value ? {
        id: selectedTarget.value.id,
        name: selectedTarget.value.name,
        kind: selectedTarget.value.kind ?? 'npc',
      } : undefined,
      difficulty: testDifficulty.value,
      round: testRound.value,
      scene_id: testSceneId.value,
    }
    const res = await runLua(req)
    runResult.value = res
  } catch (e) {
    runResult.value = {
      ok: false,
      error: e instanceof Error ? e.message : String(e),
    }
  } finally {
    runTimeMs.value = Math.round((performance.now() - t0) * 10) / 10
    running.value = false
  }
}

function handleKeydown(e: KeyboardEvent): void {
  if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
    e.preventDefault()
    void executeSandbox()
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogContent
      :show-close-button="false"
      class="!w-[96vw] !max-w-[1380px] sm:!max-w-[96vw] md:!max-w-[1380px] h-[92vh] max-h-[92vh] flex flex-col p-0 gap-0 overflow-hidden border-border/80 bg-card/95 backdrop-blur-md shadow-2xl"
      @keydown="handleKeydown"
    >
      <!-- 工作台顶栏 Header -->
      <div class="flex flex-wrap items-center justify-between gap-3 border-b border-border/70 bg-gradient-to-r from-primary/10 via-card to-card px-5 py-3.5 shrink-0">
        <div class="flex items-center gap-3 min-w-0">
          <div class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/15 text-primary border border-primary/30">
            <IconCode class="size-5" />
          </div>
          <div class="min-w-0 flex flex-col">
            <div class="flex items-center gap-2.5">
              <h3 class="font-serif text-lg font-bold text-foreground truncate">
                {{ title || 'Lua 脚本工作台' }}
              </h3>
              <Badge variant="outline" class="text-xs font-mono px-2.5 py-0.5 border-primary/30 bg-primary/10 text-primary shrink-0">
                {{ mode === 'hook' ? '技能钩子' : mode === 'check' ? '自定义判定器' : '条件表达式' }}
              </Badge>
            </div>
            <div class="text-xs text-muted-foreground/80 truncate hidden sm:block mt-0.5">
              mlua 5.4 确定性沙箱宿主 · 指令上限 2,000,000 条 · 独立跨回合存储 host.storage
            </div>
          </div>
        </div>

        <!-- 顶部操作与快捷试跑 -->
        <div class="flex items-center gap-2.5 shrink-0">
          <!-- 挂载时机下拉（仅 hook 模式显示） -->
          <div v-if="mode === 'hook'" class="flex items-center gap-1.5 mr-1">
            <span class="text-xs text-muted-foreground font-medium hidden sm:inline">挂载时机:</span>
            <Select :model-value="currentMount" @update:model-value="updateMount(String($event))">
              <SelectTrigger class="h-8 w-40 text-xs font-mono">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="pre_resolve">结算前 pre_resolve</SelectItem>
                  <SelectItem value="post_resolve">结算后 post_resolve</SelectItem>
                  <SelectItem value="check_pre_roll">掷骰前 pre_roll</SelectItem>
                  <SelectItem value="check_post_roll">掷骰后 post_roll</SelectItem>
                  <SelectItem value="event">事件响应 event</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>

          <!-- 合规状态徽标 -->
          <Badge
            v-if="lintViolations.length"
            variant="destructive"
            class="text-xs gap-1.5 px-2.5 py-1 font-mono cursor-pointer shrink-0"
            :title="lintViolations.map(l => `${l.term}: ${l.tip}`).join('；')"
          >
            <IconAlertTriangle class="size-3.5" />
            <span>{{ lintViolations.length }} 项越权警告</span>
          </Badge>
          <Badge
            v-else
            variant="outline"
            class="text-xs gap-1.5 px-2.5 py-1 font-mono border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 shrink-0"
          >
            <IconShieldCheck class="size-4" />
            <span>沙箱合规</span>
          </Badge>

          <!-- 快捷执行按钮 -->
          <Button
            variant="default"
            size="sm"
            class="h-8 gap-1.5 text-xs font-bold shadow-xs cursor-pointer px-3"
            :disabled="running || !scriptCode.trim()"
            @click="executeSandbox"
          >
            <IconLoader2 v-if="running" class="size-3.5 animate-spin" />
            <IconPlayerPlay v-else class="size-3.5" />
            <span>沙箱试跑</span>
            <kbd class="hidden md:inline-block font-mono text-[10px] opacity-75 bg-black/25 px-1.5 py-0.5 rounded">Ctrl+Enter</kbd>
          </Button>

          <!-- 完成 / 关闭按钮 -->
          <Button
            variant="outline"
            size="sm"
            class="h-8 text-xs cursor-pointer ml-1 px-3"
            @click="emit('update:open', false)"
          >
            完成
          </Button>

          <!-- 关闭 X 图标按钮 -->
          <Button
            variant="ghost"
            size="icon-sm"
            class="size-8 text-muted-foreground hover:text-foreground cursor-pointer rounded-lg ml-0.5"
            @click="emit('update:open', false)"
          >
            <IconX class="size-4.5" />
            <span class="sr-only">关闭</span>
          </Button>
        </div>
      </div>

      <!-- 常用代码片段快速插入条 (Quick Snippet Strip) -->
      <div class="flex items-center gap-2 px-5 py-2 border-b border-border/50 bg-muted/25 overflow-x-auto text-xs">
        <span class="text-muted-foreground/80 font-medium shrink-0 text-xs">快捷注入:</span>
        <button
          v-for="s in WORKBENCH_SNIPPETS"
          :key="s.label"
          type="button"
          class="inline-flex items-center gap-1.5 rounded border border-border/70 bg-background/60 px-2.5 py-1 font-mono text-xs text-muted-foreground transition-colors hover:border-primary/50 hover:text-primary shrink-0 cursor-pointer"
          @click="insertSnippet(s.code)"
        >
          <component :is="s.icon" class="size-3.5" :class="s.iconClass" />
          <span>{{ s.label }}</span>
        </button>
      </div>

      <!-- 越权 API 提示条（仅在发现越权时显示） -->
      <div v-if="lintViolations.length" class="flex flex-wrap items-center justify-between gap-2 px-5 py-2.5 border-b border-destructive/30 bg-destructive/10 text-xs">
        <div class="flex items-center gap-2">
          <IconAlertTriangle class="size-4 text-destructive shrink-0" />
          <span class="text-destructive font-semibold">检测到沙箱禁用 API：</span>
          <span v-for="v in lintViolations" :key="v.term" class="font-mono text-destructive underline font-medium" :title="v.tip">
            {{ v.term }}
          </span>
        </div>
        <Button
          v-if="lintViolations.some(v => v.term === 'math.random')"
          variant="outline"
          size="sm"
          class="h-6.5 text-xs border-destructive/40 text-destructive hover:bg-destructive/15"
          @click="fixForbiddenApi('math.random')"
        >
          一键将 math.random 替换为 host.engine_rng
        </Button>
      </div>

      <!-- 主体区域：左侧代码编辑器 + 右侧辅助面板 -->
      <div class="flex-1 min-h-0 grid grid-cols-1 md:grid-cols-12 divide-y md:divide-y-0 md:divide-x divide-border/60">
        <!-- 左侧：代码编辑区 -->
        <div class="md:col-span-7 flex flex-col min-h-0 bg-background/50">
          <div class="flex-1 min-h-0 p-3 overflow-hidden">
            <CodeEditor
              ref="editorRef"
              :model-value="scriptCode"
              language="lua"
              min-height="100%"
              max-height="100%"
              :context="luaContext"
              :placeholder="'-- 在此编写 Lua 脚本...\n-- 支持 host.get_attribute / host.request_cost / host.apply_status / host.engine_rng'"
              @update:model-value="updateCode"
            />
          </div>

          <!-- 编辑器底端状态栏 -->
          <div class="flex items-center justify-between px-4 py-2 border-t border-border/50 bg-muted/20 font-mono text-xs text-muted-foreground">
            <div class="flex items-center gap-4">
              <span>{{ scriptCode.split('\n').length }} 行</span>
              <span>{{ scriptCode.length }} 字符</span>
            </div>
            <div class="flex items-center gap-3">
              <span class="text-muted-foreground/75">指令预算: 2,000,000</span>
              <span class="text-muted-foreground/75">内存限制: 8MB</span>
            </div>
          </div>
        </div>

        <!-- 右侧：多功能辅助面板 -->
        <div class="md:col-span-5 flex flex-col min-h-0 bg-card/60">
          <Tabs v-model="assistantTab" class="flex flex-col flex-1 min-h-0">
            <!-- Tab 标头 -->
            <div class="border-b border-border/60 px-3 pt-2 bg-muted/20 shrink-0">
              <TabsList class="grid grid-cols-4 h-9 w-full">
                <TabsTrigger value="templates" class="text-xs sm:text-sm font-medium whitespace-nowrap px-1">
                  <IconSparkles class="size-3.5 mr-1 shrink-0" />
                  <span class="truncate">预设模板</span>
                </TabsTrigger>
                <TabsTrigger value="api" class="text-xs sm:text-sm font-medium whitespace-nowrap px-1">
                  <IconBook2 class="size-3.5 mr-1 shrink-0" />
                  <span class="truncate">API 手册</span>
                </TabsTrigger>
                <TabsTrigger value="entities" class="text-xs sm:text-sm font-medium whitespace-nowrap px-1">
                  <IconCoins class="size-3.5 mr-1 shrink-0" />
                  <span class="truncate">实体变量</span>
                </TabsTrigger>
                <TabsTrigger value="rules" class="text-xs sm:text-sm font-medium whitespace-nowrap px-1">
                  <IconHelpCircle class="size-3.5 mr-1 shrink-0" />
                  <span class="truncate">沙箱规则</span>
                </TabsTrigger>
              </TabsList>
            </div>

            <!-- Tab 1: 常用模板库 -->
            <TabsContent value="templates" class="flex-1 min-h-0 p-3.5 space-y-3 overflow-y-auto m-0">
              <div class="flex items-center gap-2">
                <Input
                  v-model="templateSearch"
                  placeholder="搜索模板名称、标签…"
                  class="h-8 text-xs flex-1"
                />
                <Select :model-value="templateCategory" @update:model-value="templateCategory = $event as any">
                  <SelectTrigger class="h-8 w-28 text-xs font-mono"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="all">全部分类</SelectItem>
                      <SelectItem value="hook">技能钩子</SelectItem>
                      <SelectItem value="check">自定义判定</SelectItem>
                      <SelectItem value="condition">条件逻辑</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </div>

              <div class="space-y-3">
                <div
                  v-for="tpl in filteredTemplates"
                  :key="tpl.id"
                  class="rounded-xl border border-border/70 bg-card/80 p-3.5 hover:border-primary/40 transition-all space-y-2.5"
                >
                  <div class="flex items-start justify-between gap-2">
                    <div>
                      <div class="text-sm font-bold text-foreground">{{ tpl.title }}</div>
                      <div class="text-xs text-muted-foreground leading-relaxed mt-1">
                        {{ tpl.description }}
                      </div>
                    </div>
                    <Badge variant="outline" class="text-xs px-2 py-0.5 shrink-0 font-mono">
                      {{ tpl.category }}
                    </Badge>
                  </div>

                  <div class="flex flex-wrap gap-1.5">
                    <span v-for="tag in tpl.tags" :key="tag" class="rounded bg-muted/70 px-2 py-0.5 text-xs text-muted-foreground font-mono">
                      #{{ tag }}
                    </span>
                  </div>

                  <div class="flex items-center justify-end gap-2 pt-2 border-t border-border/50">
                    <Button
                      variant="ghost"
                      size="sm"
                      class="h-7 text-xs text-muted-foreground hover:text-foreground"
                      @click="applyTemplate(tpl, true)"
                    >
                      追加到末尾
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      class="h-7 text-xs border-primary/30 text-primary hover:bg-primary/10 font-medium"
                      @click="applyTemplate(tpl, false)"
                    >
                      替换当前代码
                    </Button>
                  </div>
                </div>
              </div>
            </TabsContent>

            <!-- Tab 2: host.* 交互式 API 手册 -->
            <TabsContent value="api" class="flex-1 min-h-0 p-3.5 space-y-3 overflow-y-auto m-0">
              <Input
                v-model="apiSearch"
                placeholder="搜索 API 名称、参数或用途…"
                class="h-8 text-xs w-full"
              />

              <div class="space-y-2.5">
                <div
                  v-for="entry in filteredApis"
                  :key="entry.name"
                  class="group flex flex-col gap-1.5 rounded-lg border border-border/60 bg-muted/20 p-3 hover:border-primary/40 hover:bg-card transition-all cursor-pointer"
                  @click="insertSnippet(entry.snippet)"
                >
                  <div class="flex items-center justify-between">
                    <code class="font-mono text-sm font-bold text-primary">{{ entry.signature }}</code>
                    <Badge variant="outline" class="text-xs px-2 py-0.5 font-mono">
                      {{ entry.group }}
                    </Badge>
                  </div>
                  <div class="text-xs text-muted-foreground leading-relaxed flex items-center justify-between">
                    <span>{{ entry.desc }}</span>
                    <span class="opacity-0 group-hover:opacity-100 text-xs text-primary transition-opacity flex items-center gap-1 font-medium">
                      <span>插入代码</span>
                      <IconArrowRight class="size-3" />
                    </span>
                  </div>
                </div>
              </div>
            </TabsContent>

            <!-- Tab 3: 当前故事书实体注入 -->
            <TabsContent value="entities" class="flex-1 min-h-0 p-3.5 space-y-4 overflow-y-auto m-0">
              <p class="text-xs text-muted-foreground/85 leading-relaxed">
                点击下列故事书中已声明的标识符，即可将其精确插入到当前代码光标处，避免手写 ID 拼写错误。
              </p>

              <!-- 属性维度 -->
              <div v-if="luaContext.attributes.length">
                <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-2">属性维度 (Attributes)</div>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="a in luaContext.attributes"
                    :key="a.id"
                    type="button"
                    class="rounded-md border border-border/70 bg-card px-2.5 py-1 font-mono text-xs transition-colors hover:border-primary hover:text-primary cursor-pointer"
                    :title="a.label"
                    @click="insertSnippet(`'${a.id}'`)"
                  >
                    <span class="font-semibold">{{ a.label }}</span>
                    <span class="ml-1 opacity-60 text-[11px]">({{ a.id }})</span>
                  </button>
                </div>
              </div>

              <!-- 资源 -->
              <div v-if="luaContext.resources.length">
                <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-2">资源槽位 (Resources)</div>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="r in luaContext.resources"
                    :key="r.id"
                    type="button"
                    class="rounded-md border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 font-mono text-xs text-amber-700 dark:text-amber-400 transition-colors hover:border-amber-500 cursor-pointer"
                    :title="r.label"
                    @click="insertSnippet(`'${r.id}'`)"
                  >
                    <span class="font-semibold">{{ r.label }}</span>
                    <span class="ml-1 opacity-60 text-[11px]">({{ r.id }})</span>
                  </button>
                </div>
              </div>

              <!-- 剧情标记 -->
              <div v-if="luaContext.flags.length">
                <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-2">剧情标记 (Flags)</div>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="f in luaContext.flags"
                    :key="f.id"
                    type="button"
                    class="rounded-md border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 font-mono text-xs text-emerald-700 dark:text-emerald-400 transition-colors hover:border-emerald-500 cursor-pointer"
                    :title="f.label"
                    @click="insertSnippet(`'${f.id}'`)"
                  >
                    <span>{{ f.label || f.id }}</span>
                  </button>
                </div>
              </div>

              <!-- 注册事件 -->
              <div v-if="luaContext.events.length">
                <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-2">注册事件 (Events)</div>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="e in luaContext.events"
                    :key="e.id"
                    type="button"
                    class="rounded-md border border-blue-500/30 bg-blue-500/10 px-2.5 py-1 font-mono text-xs text-blue-700 dark:text-blue-400 transition-colors hover:border-blue-500 cursor-pointer"
                    :title="e.label"
                    @click="insertSnippet(`'${e.id}'`)"
                  >
                    <span>{{ e.label || e.id }}</span>
                  </button>
                </div>
              </div>

              <!-- 状态 -->
              <div v-if="luaContext.statuses.length">
                <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-2">状态定义 (Statuses)</div>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="s in luaContext.statuses"
                    :key="s.id"
                    type="button"
                    class="rounded-md border border-purple-500/30 bg-purple-500/10 px-2.5 py-1 font-mono text-xs text-purple-700 dark:text-purple-400 transition-colors hover:border-purple-500 cursor-pointer"
                    :title="s.label"
                    @click="insertSnippet(`'${s.id}'`)"
                  >
                    <span>{{ s.label || s.id }}</span>
                  </button>
                </div>
              </div>

              <!-- 故事人物 -->
              <div v-if="characters.length">
                <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-2">故事人物 (Characters)</div>
                <div class="flex flex-wrap gap-1.5">
                  <button
                    v-for="c in characters"
                    :key="c.id"
                    type="button"
                    class="rounded-md border border-border/70 bg-card px-2.5 py-1 font-mono text-xs transition-colors hover:border-primary hover:text-primary cursor-pointer"
                    @click="insertSnippet(`'${c.id}'`)"
                  >
                    <span class="font-semibold">{{ c.name }}</span>
                    <span class="ml-1 opacity-60 text-[11px]">({{ c.id }})</span>
                  </button>
                </div>
              </div>
            </TabsContent>

            <!-- Tab 4: 沙箱规则说明 -->
            <TabsContent value="rules" class="flex-1 min-h-0 p-4 space-y-3.5 text-xs leading-relaxed overflow-y-auto m-0 text-muted-foreground">
              <div class="font-bold text-foreground text-sm">Octopus Lua 5.4 沙箱架构</div>
              <p class="text-xs leading-relaxed">
                引擎基于 mlua 构建受控沙箱。为保证游戏确定性重放与安全性，脚本采用「只读查询 + 异步写请求」双相模式：
              </p>
              <div class="space-y-3 rounded-xl border border-border/60 bg-muted/20 p-3.5">
                <div>
                  <div class="font-bold text-foreground text-xs mb-1">1. 状态只读原则</div>
                  <p class="text-xs leading-relaxed">脚本通过 <code class="font-mono text-primary">host.get_attribute</code>、<code class="font-mono text-primary">host.get_resource</code> 等直接读取角色与世界数据，但不能直接改写内存状态。</p>
                </div>

                <div>
                  <div class="font-bold text-foreground text-xs mb-1">2. 写请求 Outbox 队列</div>
                  <p class="text-xs leading-relaxed">所有的变更行为（扣减金币、施加状态、广播事件）必须通过 <code class="font-mono text-primary">host.request_cost</code>、<code class="font-mono text-primary">host.apply_status</code> 发起请求，由引擎校验通过后统一在事务中落盘。</p>
                </div>

                <div>
                  <div class="font-bold text-foreground text-xs mb-1">3. 确定性随机源</div>
                  <p class="text-xs leading-relaxed">禁用系统自带的 <code class="font-mono text-destructive">math.random</code>，强制使用 <code class="font-mono text-primary">host.engine_rng(min, max)</code>，保证与引擎事件流一致并支持完整复盘重放。</p>
                </div>

                <div>
                  <div class="font-bold text-foreground text-xs mb-1">4. 隔离存储 host.storage</div>
                  <p class="text-xs leading-relaxed">每个脚本享有隔离独立的持久化 key-value 存储 <code class="font-mono text-primary">host.storage</code>，跨回合依然保留，非常适合连击、蓄能计数器。</p>
                </div>
              </div>
            </TabsContent>
          </Tabs>
        </div>
      </div>

      <!-- 底栏：交互式沙箱试跑控制台 -->
      <div class="border-t border-border/70 bg-card/90 shrink-0">
        <!-- 试跑参数工具栏 -->
        <div class="flex flex-wrap items-center justify-between gap-3 px-5 py-2.5 border-b border-border/50 bg-muted/30">
          <div class="flex flex-wrap items-center gap-3.5">
            <div class="flex items-center gap-1.5">
              <IconTerminal2 class="size-4 text-primary" />
              <span class="text-sm font-bold text-foreground">沙箱调试台</span>
            </div>

            <!-- 施法者选择 -->
            <div v-if="characters.length" class="flex items-center gap-1.5">
              <span class="text-xs text-muted-foreground font-medium">施法者:</span>
              <Select v-model="selectedActorId">
                <SelectTrigger class="h-8 w-36 text-xs font-semibold"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem v-for="c in characters" :key="c.id" :value="c.id">{{ c.name }}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </div>

            <!-- 目标选择 -->
            <div v-if="characters.length > 1" class="flex items-center gap-1.5">
              <span class="text-xs text-muted-foreground font-medium">目标:</span>
              <Select v-model="selectedTargetId">
                <SelectTrigger class="h-8 w-36 text-xs font-semibold"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem v-for="c in characters" :key="c.id" :value="c.id">{{ c.name }}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </div>

            <!-- 判定难度 -->
            <div v-if="mode === 'check'" class="flex items-center gap-1.5">
              <span class="text-xs text-muted-foreground font-medium">检定难度:</span>
              <Input
                v-model.number="testDifficulty"
                type="number"
                class="h-8 w-18 text-xs text-center font-mono"
              />
            </div>

            <!-- 测试回合 -->
            <div class="flex items-center gap-1.5">
              <span class="text-xs text-muted-foreground font-medium">回合:</span>
              <Input
                v-model.number="testRound"
                type="number"
                min="1"
                class="h-8 w-16 text-xs text-center font-mono"
              />
            </div>
          </div>

          <!-- 执行按钮与耗时 -->
          <div class="flex items-center gap-2.5">
            <span v-if="runTimeMs != null" class="font-mono text-xs text-muted-foreground">
              耗时: {{ runTimeMs }}ms
            </span>
            <Button
              variant="default"
              size="sm"
              class="h-8 gap-1.5 text-xs font-bold cursor-pointer shadow-xs px-3"
              :disabled="running || !scriptCode.trim()"
              @click="executeSandbox"
            >
              <IconLoader2 v-if="running" class="size-4 animate-spin" />
              <IconPlayerPlay v-else class="size-4" />
              <span>运行测试</span>
              <kbd class="hidden md:inline-block font-mono text-[10px] opacity-75 bg-black/25 px-1.5 py-0.5 rounded">Ctrl+Enter</kbd>
            </Button>
          </div>
        </div>

        <!-- 试跑输出结果显示区域 -->
        <div class="px-5 py-3 max-h-44 overflow-y-auto">
          <!-- 尚未执行 -->
          <div v-if="!runResult && !running" class="text-xs text-muted-foreground/75 py-1.5 flex items-center gap-2">
            <span>点击「运行测试」或按 Ctrl+Enter，即时在沙箱中执行该脚本并观测写请求与返回值。</span>
          </div>

          <!-- 执行中 -->
          <div v-else-if="running" class="flex items-center gap-2 py-2 text-xs font-medium text-primary">
            <IconLoader2 class="size-4 animate-spin" />
            <span>沙箱正在执行脚本…</span>
          </div>

          <!-- 发生错误 -->
          <div v-else-if="runResult && !runResult.ok" class="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive font-mono space-y-1.5">
            <div class="font-bold flex items-center gap-2 text-sm">
              <IconAlertTriangle class="size-4 shrink-0" />
              <span>执行失败：{{ runResult.error }}</span>
            </div>
            <p class="text-xs opacity-85">
              请检查语法、函数签名是否匹配 host.* 白名单，或查看左侧合规警告。
            </p>
          </div>

          <!-- 执行成功 -->
          <div v-else-if="runResult && runResult.ok" class="space-y-2.5">
            <div class="flex flex-wrap items-center gap-2.5">
              <Badge variant="outline" class="border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 font-mono text-xs px-2.5 py-0.5 font-semibold">
                ✓ 沙箱执行成功
              </Badge>

              <!-- 判定模式输出 -->
              <span v-if="mode === 'check' && runResult.result" class="font-mono text-xs sm:text-sm font-bold text-foreground">
                最终值: {{ runResult.result.total }} · 差值: {{ Number(runResult.result.margin) > 0 ? '+' : '' }}{{ runResult.result.margin }}
              </span>

              <!-- 条件模式输出 -->
              <span v-if="mode === 'condition' && runResult.result" class="font-mono text-xs sm:text-sm font-bold" :class="runResult.result.value ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600'">
                条件判定: {{ runResult.result.value ? '成立 (true)' : '未达成 (false)' }}
              </span>

              <!-- 写请求数量 -->
              <span class="font-mono text-xs text-muted-foreground">
                产生写请求: {{ (runResult.requests ?? []).length }} 项
              </span>
            </div>

            <!-- 写请求卡片展示 -->
            <div v-if="(runResult.requests ?? []).length" class="flex flex-wrap gap-2 pt-1">
              <template v-for="(req, idx) in (runResult.requests as any[])" :key="idx">
                <!-- 扣减资源 -->
                <div v-if="req.Cost" class="inline-flex items-center gap-1.5 rounded-md border border-amber-500/30 bg-amber-500/10 px-2.5 py-1.5 font-mono text-xs text-amber-700 dark:text-amber-300">
                  <IconCoins class="size-4" />
                  <span>扣除资源 {{ req.Cost.resource }}</span>
                  <b class="font-extrabold">{{ req.Cost.amount }}</b>
                </div>

                <!-- 施加状态 -->
                <div v-else-if="req.ApplyStatus" class="inline-flex items-center gap-1.5 rounded-md border border-purple-500/30 bg-purple-500/10 px-2.5 py-1.5 font-mono text-xs text-purple-700 dark:text-purple-300">
                  <IconSparkles class="size-4" />
                  <span>对 {{ req.ApplyStatus.target }} 施加 {{ req.ApplyStatus.status }} ({{ req.ApplyStatus.duration }} {{ req.ApplyStatus.unit }})</span>
                </div>

                <!-- 移除状态 -->
                <div v-else-if="req.RemoveStatus" class="inline-flex items-center gap-1.5 rounded-md border border-border/70 bg-muted/60 px-2.5 py-1.5 font-mono text-xs text-muted-foreground">
                  <IconTrash class="size-4" />
                  <span>移除 {{ req.RemoveStatus.target }} 的 {{ req.RemoveStatus.status }} 状态</span>
                </div>

                <!-- 触发事件 -->
                <div v-else-if="req.TriggerEvent" class="inline-flex items-center gap-1.5 rounded-md border border-blue-500/30 bg-blue-500/10 px-2.5 py-1.5 font-mono text-xs text-blue-700 dark:text-blue-300">
                  <IconFlame class="size-4" />
                  <span>触发事件 {{ req.TriggerEvent.event }}</span>
                </div>

                <!-- 其他 -->
                <div v-else class="inline-flex items-center gap-1.5 rounded-md border border-border/70 bg-card px-2.5 py-1.5 font-mono text-xs text-foreground">
                  <span>{{ JSON.stringify(req) }}</span>
                </div>
              </template>
            </div>

            <!-- 无副作用提示 -->
            <div v-else class="text-xs text-muted-foreground/80 font-mono">
              （未产生任何外部写请求，脚本仅作为只读计算或返回结果）
            </div>
          </div>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
