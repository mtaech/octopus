<script setup lang="ts">
// LuaHookEditor —— 专业级嵌入式 Lua 脚本编辑器卡片（#02 / #12 / #23）
// 支持：挂载时机配置、快捷宏插入、沙箱合规检测、快速模板套用、内联沙箱试跑、一键开启全屏 LuaWorkbench 深度工作台。
import { computed, ref } from 'vue'
import type { SkillDef, Storybook } from '@/types'
import { buildLuaContext, lintLuaSource, type LuaCompletionContext } from '@/lib/lua-context'
import { LUA_TEMPLATES, type LuaTemplate } from '@/lib/lua-templates'
import CodeEditor from '@/components/CodeEditor.vue'
import LuaWorkbenchDialog from '@/components/LuaWorkbenchDialog.vue'
import { runLua, type LuaRunRequest, type LuaRunResult } from '@/api'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  IconCode,
  IconPlayerPlay,
  IconArrowUpRight,
  IconSparkles,
  IconShieldCheck,
  IconAlertTriangle,
  IconCoins,
  IconFlame,
  IconDice,
  IconTerminal2,
  IconLoader2,
  IconChevronDown,
  IconWand,
} from '@tabler/icons-vue'

const props = withDefaults(defineProps<{
  modelValue?: string
  mount?: string
  mode?: 'hook' | 'check' | 'condition'
  title?: string
  skill?: SkillDef | unknown
  storybook?: Storybook | null
  context?: LuaCompletionContext
  readonly?: boolean
}>(), {
  modelValue: '',
  mount: 'pre_resolve',
  mode: 'hook',
  title: 'Lua 自定义脚本逻辑',
  storybook: null,
  readonly: false,
})

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'update:mount', value: string): void
}>()

const editorRef = ref<InstanceType<typeof CodeEditor> | null>(null)
const workbenchOpen = ref(false)

// 故事书上下文实体（补全与引用）
const luaContext = computed<LuaCompletionContext>(() => props.context ?? buildLuaContext(props.storybook))

// 静态安全合规检测
const lintViolations = computed(() => lintLuaSource(props.modelValue))

// 代码行数与大小
const lineCount = computed(() => {
  const code = props.modelValue ?? ''
  if (!code.trim()) return 0
  return code.split('\n').length
})

// 适用模式的快速模板库
const availableTemplates = computed<LuaTemplate[]>(() => {
  return LUA_TEMPLATES.filter(t => t.category === props.mode)
})

function applyTemplate(tpl: LuaTemplate): void {
  emit('update:modelValue', tpl.code)
  if (tpl.mount && props.mode === 'hook') {
    emit('update:mount', tpl.mount)
  }
}

function fixForbidden(term: string): void {
  if (term === 'math.random') {
    const fixed = (props.modelValue ?? '').replace(/math\.random\s*\(([^)]*)\)/g, 'host.engine_rng($1)')
    emit('update:modelValue', fixed)
  }
}

// 快速宏插入
function insertSnippet(text: string): void {
  if (editorRef.value) {
    editorRef.value.insertText(text)
  } else {
    emit('update:modelValue', props.modelValue ? `${props.modelValue}\n${text}` : text)
  }
}

const HOOK_SNIPPETS = [
  { label: '扣除消耗', icon: IconCoins, iconClass: 'text-amber-500', code: "host.request_cost('mp', 10)" },
  { label: '施加状态', icon: IconFlame, iconClass: 'text-rose-500', code: "host.apply_status('target', 'poison', 3, 'turns')" },
  { label: '确定性RNG', icon: IconDice, iconClass: 'text-indigo-500', code: 'local roll = host.engine_rng(1, 20)' },
  { label: '触发事件', icon: IconSparkles, iconClass: 'text-primary', code: "host.trigger_event('spell_cast')" },
  { label: '跨回合存储', icon: IconTerminal2, iconClass: 'text-emerald-500', code: "host.storage.combo = (host.storage.combo or 0) + 1" },
  { label: '调试日志', icon: IconTerminal2, iconClass: 'text-muted-foreground', code: "host.log('info', 'Executing lua script...')" },
]

// 内联试跑沙箱
const inlineRunning = ref(false)
const inlineResult = ref<LuaRunResult | null>(null)
const inlineTimeMs = ref<number | null>(null)

async function runInlineSandbox(): Promise<void> {
  const code = props.modelValue?.trim()
  if (!code || inlineRunning.value) return
  inlineRunning.value = true
  inlineResult.value = null
  const t0 = performance.now()
  try {
    const actor = props.storybook?.characters?.[0]
    const target = props.storybook?.characters?.[1] ?? actor
    const req: LuaRunRequest = {
      script: code,
      mode: props.mode,
      mount: props.mount,
      skill: props.skill,
      actor: actor ? {
        id: actor.id,
        name: actor.name,
        attributes: actor.attributes ?? {},
        resources: actor.resources ?? {},
        statuses: [],
      } : undefined,
      target: target ? {
        id: target.id,
        name: target.name,
        kind: target.kind ?? 'npc',
      } : undefined,
      difficulty: 12,
      round: 1,
    }
    const res = await runLua(req)
    inlineResult.value = res
  } catch (e) {
    inlineResult.value = {
      ok: false,
      error: e instanceof Error ? e.message : String(e),
    }
  } finally {
    inlineTimeMs.value = Math.round(performance.now() - t0)
    inlineRunning.value = false
  }
}
</script>

<template>
  <div class="rounded-xl border border-border/80 bg-card/60 shadow-xs overflow-hidden">
    <!-- 头部工具栏 -->
    <div class="flex flex-wrap items-center justify-between gap-2.5 border-b border-border/70 bg-muted/30 px-3.5 py-2.5">
      <!-- 左侧：标题与挂载点/状态徽章 -->
      <div class="flex flex-wrap items-center gap-2">
        <div class="flex items-center gap-1.5 font-semibold text-sm text-foreground">
          <IconCode class="size-4 text-primary" />
          <span>{{ title }}</span>
        </div>

        <!-- 挂载时机（仅在 hook 模式提供切换） -->
        <div v-if="mode === 'hook'" class="flex items-center gap-1">
          <Select :model-value="mount" @update:model-value="emit('update:mount', String($event))">
            <SelectTrigger class="h-7 text-xs font-mono px-2.5 py-0 border-border/70 bg-background/80">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="pre_resolve">pre_resolve（结算前钩子）</SelectItem>
                <SelectItem value="post_resolve">post_resolve（结算后钩子）</SelectItem>
                <SelectItem value="check_pre_roll">check_pre_roll（掷骰前修正）</SelectItem>
                <SelectItem value="check_post_roll">check_post_roll（掷骰后修正）</SelectItem>
                <SelectItem value="event">event（通用事件响应）</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>

        <!-- 沙箱合规徽章 -->
        <Badge
          v-if="lintViolations.length === 0"
          variant="outline"
          class="h-6 gap-1 border-emerald-500/30 bg-emerald-500/10 px-2 text-xs text-emerald-600 dark:text-emerald-400"
        >
          <IconShieldCheck class="size-3.5" />
          <span>沙箱合规</span>
        </Badge>
        <Badge
          v-else
          variant="destructive"
          class="h-6 gap-1 px-2 text-xs animate-pulse"
        >
          <IconAlertTriangle class="size-3.5" />
          <span>{{ lintViolations.length }} 处越权警告</span>
        </Badge>

        <span v-if="lineCount > 0" class="font-mono text-xs text-muted-foreground/70">
          {{ lineCount }} 行
        </span>
      </div>

      <!-- 右侧：模板与全屏工作台 -->
      <div class="flex items-center gap-1.5">
        <!-- 快速模板套用 -->
        <DropdownMenu v-if="availableTemplates.length > 0 && !readonly">
          <DropdownMenuTrigger as-child>
            <Button variant="outline" size="sm" class="h-7 gap-1 px-2.5 text-xs text-muted-foreground hover:text-foreground">
              <IconSparkles class="size-3.5 text-amber-500" />
              <span>预设模板</span>
              <IconChevronDown class="size-3 opacity-60" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-64 max-h-72 overflow-y-auto p-1.5 text-xs">
            <DropdownMenuLabel class="text-xs text-muted-foreground">快速套用 {{ mode }} 模板</DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              v-for="tpl in availableTemplates"
              :key="tpl.id"
              class="cursor-pointer flex flex-col items-start gap-0.5 py-1.5 text-xs"
              @click="applyTemplate(tpl)"
            >
              <div class="font-medium text-foreground">{{ tpl.title }}</div>
              <div class="text-xs text-muted-foreground line-clamp-1">{{ tpl.description }}</div>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>

        <!-- 打开全屏工作台 -->
        <Button
          variant="default"
          size="sm"
          class="h-7 gap-1.5 px-3 text-xs shadow-xs cursor-pointer font-medium"
          @click="workbenchOpen = true"
        >
          <IconArrowUpRight class="size-3.5" />
          <span>全屏工作台</span>
        </Button>
      </div>
    </div>

    <!-- 快捷宏注入栏（Macro snippet strip） -->
    <div v-if="!readonly" class="flex flex-wrap items-center gap-1.5 border-b border-border/50 bg-background/50 px-3.5 py-2 text-xs">
      <span class="text-muted-foreground/70 mr-1 text-xs select-none">常用注入:</span>
      <button
        v-for="s in HOOK_SNIPPETS"
        :key="s.label"
        type="button"
        class="inline-flex cursor-pointer items-center gap-1 rounded border border-border/60 bg-muted/40 px-2 py-1 font-mono text-xs text-muted-foreground hover:border-primary/50 hover:bg-primary/10 hover:text-primary transition-colors"
        @click="insertSnippet(s.code)"
      >
        <component :is="s.icon" class="size-3.5" :class="s.iconClass" />
        <span>{{ s.label }}</span>
      </button>
    </div>

    <!-- 静态安全警告条（若检测到非白名单 API） -->
    <div
      v-if="lintViolations.length > 0"
      class="border-b border-destructive/30 bg-destructive/10 px-3.5 py-2.5 text-xs text-destructive space-y-1.5"
    >
      <div class="flex items-center justify-between font-semibold">
        <div class="flex items-center gap-1.5">
          <IconAlertTriangle class="size-4 shrink-0" />
          <span>检测到白名单外或违规 Lua API（将无法通过引擎预检）：</span>
        </div>
        <Button
          v-if="lintViolations.some(v => v.term === 'math.random')"
          variant="outline"
          size="sm"
          class="h-6 px-2.5 text-xs border-destructive/40 bg-background/80 hover:bg-destructive/20 text-destructive gap-1"
          @click="fixForbidden('math.random')"
        >
          <IconWand class="size-3" />
          <span>一键修复 math.random 为 host.engine_rng</span>
        </Button>
      </div>
      <div class="space-y-1 pl-5">
        <div v-for="v in lintViolations" :key="v.term" class="font-mono text-xs">
          • <span class="font-bold underline">{{ v.term }}</span>: {{ v.tip }}
        </div>
      </div>
    </div>

    <!-- CodeEditor 编辑区 -->
    <div class="p-2 bg-background/30">
      <CodeEditor
        ref="editorRef"
        :model-value="modelValue ?? ''"
        language="lua"
        min-height="7rem"
        max-height="20rem"
        :context="luaContext"
        show-reference
        :readonly="readonly"
        :placeholder="mode === 'check'
          ? 'local r = host.engine_rng(1, 20)\nreturn { total = r, margin = r - (host.difficulty or 12) }'
          : mode === 'condition'
          ? 'return host.get_attribute(\'str\') >= 60 and host.get_resource(\'mp\') >= 10'
          : '-- 可选 Lua 钩子：host.request_cost / host.apply_status / host.trigger_event / host.engine_rng ...'"
        @update:model-value="emit('update:modelValue', $event)"
      />
    </div>

    <!-- 底部：内联试跑与沙箱说明 -->
    <div class="border-t border-border/60 bg-muted/20 px-3.5 py-2.5 text-xs space-y-2">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            class="h-7 gap-1.5 px-2.5 text-xs cursor-pointer border-border/70 hover:border-primary/50 hover:bg-primary/5"
            :disabled="inlineRunning || !modelValue?.trim()"
            @click="runInlineSandbox"
          >
            <IconLoader2 v-if="inlineRunning" class="size-3.5 animate-spin text-primary" />
            <IconPlayerPlay v-else class="size-3.5 text-primary" />
            <span>{{ inlineRunning ? '沙箱执行中…' : '试跑沙箱' }}</span>
          </Button>
          <span class="text-xs text-muted-foreground/80">
            {{ inlineRunning ? '正在连接 Rust 引擎沙箱…' : '即时预检 + 沙箱仿真' }}
          </span>
        </div>

        <div class="text-xs text-muted-foreground/70">
          沙箱白名单：无 io / os / package / debug，RNG 走 host.engine_rng；保存后由引擎做语法 + 越权预检。
        </div>
      </div>

      <!-- 内联试跑输出 -->
      <div v-if="inlineResult" class="rounded-lg border border-border/70 bg-background/80 p-2.5 text-xs space-y-2 font-mono">
        <div class="flex items-center justify-between text-xs">
          <div class="flex items-center gap-2">
            <span
              class="inline-block size-2.5 rounded-full"
              :class="inlineResult.ok ? 'bg-emerald-500 shadow-xs shadow-emerald-500/50' : 'bg-destructive shadow-xs shadow-destructive/50'"
            />
            <span :class="inlineResult.ok ? 'font-bold text-emerald-600 dark:text-emerald-400' : 'font-bold text-destructive'">
              {{ inlineResult.ok ? '执行成功' : '执行失败' }}
            </span>
            <span v-if="inlineTimeMs !== null" class="text-muted-foreground">({{ inlineTimeMs }}ms)</span>
          </div>

          <!-- 写请求数量指示 -->
          <div v-if="inlineResult.requests && inlineResult.requests.length > 0" class="text-muted-foreground">
            产生了 {{ inlineResult.requests.length }} 个写请求
          </div>
        </div>

        <!-- 错误信息 -->
        <div v-if="!inlineResult.ok && inlineResult.error" class="text-destructive text-xs whitespace-pre-wrap bg-destructive/10 p-2.5 rounded">
          {{ inlineResult.error }}
        </div>

        <!-- 成功返回结果 -->
        <div v-if="inlineResult.ok" class="space-y-1.5">
          <div v-if="inlineResult.result && Object.keys(inlineResult.result).length > 0" class="text-xs text-foreground">
            <span class="text-muted-foreground">返回值: </span>
            <span class="text-primary font-semibold">{{ JSON.stringify(inlineResult.result) }}</span>
          </div>

          <!-- 写请求卡片组 -->
          <div v-if="inlineResult.requests && inlineResult.requests.length > 0" class="flex flex-wrap gap-2 pt-1">
            <span
              v-for="(req, idx) in (inlineResult.requests as any[])"
              :key="idx"
              class="inline-flex items-center gap-1.5 rounded bg-muted/70 px-2 py-1 text-xs border border-border/60"
            >
              <template v-if="req.Cost">
                <IconCoins class="size-3.5 text-amber-500" />
                <span>Cost: {{ req.Cost.resource }} {{ req.Cost.amount }}</span>
              </template>
              <template v-else-if="req.ApplyStatus">
                <IconFlame class="size-3.5 text-rose-500" />
                <span>Apply: {{ req.ApplyStatus.status }} ({{ req.ApplyStatus.duration }}{{ req.ApplyStatus.unit }})</span>
              </template>
              <template v-else-if="req.TriggerEvent">
                <IconSparkles class="size-3.5 text-primary" />
                <span>Event: {{ req.TriggerEvent.event }}</span>
              </template>
              <template v-else>
                <span>{{ JSON.stringify(req) }}</span>
              </template>
            </span>
          </div>
        </div>
      </div>
    </div>

    <!-- 全屏工作台弹窗 -->
    <LuaWorkbenchDialog
      v-model:open="workbenchOpen"
      :model-value="modelValue"
      :mount="mount"
      :mode="mode"
      :title="title"
      :storybook="storybook"
      :context="luaContext"
      :skill="skill"
      @update:model-value="emit('update:modelValue', $event)"
      @update:mount="emit('update:mount', $event)"
    />
  </div>
</template>
