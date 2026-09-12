<script setup lang="ts">
// CodeEditor —— 通用代码编辑器（CodeMirror 6）。
// Lua 语法高亮 + 行号 + 括号匹配 + 撤销历史；外部 v-model 读写文本。
// Lua 场景额外提供：host.* API 自动补全、按当前故事书补全资源/属性/标记等 id、
// 以及「可用 API 与资源」参考面板（点击即插入）。配色走应用 CSS 变量，明暗自动适配。
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { basicSetup } from 'codemirror'
import { Compartment, EditorState, Prec, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { HighlightStyle, StreamLanguage, syntaxHighlighting } from '@codemirror/language'
import { autocompletion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete'
import { tags as t } from '@lezer/highlight'
import { lua } from '@codemirror/legacy-modes/mode/lua'
import { LUA_API_REFERENCE, type LuaApiEntry, type LuaCompletionContext, type LuaIdOption } from '@/lib/lua-context'

const props = withDefaults(defineProps<{
  modelValue: string
  language?: 'lua'
  readonly?: boolean
  minHeight?: string
  maxHeight?: string
  placeholder?: string
  context?: LuaCompletionContext
  showReference?: boolean
  fontSize?: string
}>(), {
  language: 'lua',
  readonly: false,
  minHeight: '7rem',
  maxHeight: '24rem',
  showReference: false,
  fontSize: '13.5px',
})

const emit = defineEmits<{ (e: 'update:modelValue', value: string): void }>()

const host = ref<HTMLElement | null>(null)
const refOpen = ref(false)
let view: EditorView | null = null
const readOnlyCompartment = new Compartment()

const luaLanguage = StreamLanguage.define(lua)

const highlight = HighlightStyle.define([
  { tag: t.keyword, color: 'var(--primary)', fontWeight: '600' },
  { tag: [t.string, t.special(t.string)], color: 'var(--success)' },
  { tag: [t.number, t.bool, t.null], color: 'var(--warning)' },
  { tag: [t.comment, t.lineComment, t.blockComment], color: 'var(--muted-foreground)', fontStyle: 'italic' },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: 'var(--info)' },
  { tag: [t.operator, t.punctuation, t.bracket], color: 'var(--muted-foreground)' },
  { tag: [t.propertyName, t.definition(t.variableName), t.variableName], color: 'var(--foreground)' },
])

const theme = EditorView.theme({
  '&': {
    color: 'var(--foreground)',
    backgroundColor: 'color-mix(in oklab, var(--muted) 45%, transparent)',
    border: '1px solid var(--border)',
    borderRadius: '0.5rem',
    fontSize: '13.5px',
    overflow: 'hidden',
  },
  '&.cm-focused': { outline: 'none', borderColor: 'var(--primary)' },
  '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.68', overflow: 'auto' },
  '.cm-content': { padding: '8px 0', caretColor: 'var(--primary)' },
  '.cm-gutters': { backgroundColor: 'transparent', color: 'var(--muted-foreground)', border: 'none' },
  '.cm-activeLine': { backgroundColor: 'color-mix(in oklab, var(--primary) 7%, transparent)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--foreground)' },
  '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': {
    backgroundColor: 'color-mix(in oklab, var(--primary) 22%, transparent)',
  },
  '.cm-cursor': { borderLeftColor: 'var(--foreground)' },
  '.cm-tooltip': { border: '1px solid var(--border)', backgroundColor: 'var(--background)', borderRadius: '0.5rem' },
  '.cm-tooltip-autocomplete ul li[aria-selected]': { backgroundColor: 'color-mix(in oklab, var(--primary) 18%, transparent)', color: 'var(--foreground)' },
})

/** host. 之后的补全内容 = 完整片段去掉 host. 前缀（host. 已由用户输入）。 */
function applyFor(entry: LuaApiEntry): string {
  return entry.snippet.startsWith('host.') ? entry.snippet.slice(5) : entry.snippet
}

const LUA_KEYWORDS = [
  'return', 'local', 'if', 'then', 'elseif', 'else', 'end', 'for', 'while', 'do', 'function',
  'and', 'or', 'not', 'nil', 'true', 'false', 'break', 'repeat', 'until', 'in',
  'pairs', 'ipairs', 'tostring', 'tonumber', 'type', 'math.floor', 'math.min', 'math.max',
]

function luaCompletionSource(ctx: CompletionContext): CompletionResult | null {
  const line = ctx.state.doc.lineAt(ctx.pos)
  const before = line.text.slice(0, ctx.pos - line.from)
  // 字符串参数内：按 API 的首参类型补全当前故事书的 id
  const argMatch = before.match(/host\.(\w+)\s*\(\s*(['"])([^'"]*)$/)
  if (argMatch) {
    const entry = LUA_API_REFERENCE.find(e => e.name === argMatch[1])
    const group = entry?.argContext
    const list: LuaIdOption[] = group && props.context ? props.context[group] : []
    const partial = argMatch[3]
    if (!list.length) return null
    return {
      from: ctx.pos - partial.length,
      options: list.map(o => ({ label: o.id, detail: o.label, type: 'constant' })),
      validFor: /^[^'"]*$/,
    }
  }
  const word = ctx.matchBefore(/[\w.$]*/)
  if (!word || (word.from === word.to && !ctx.explicit)) return null
  const text = word.text
  if (text.startsWith('host.')) {
    return {
      from: word.from + 'host.'.length,
      options: LUA_API_REFERENCE.map(e => ({
        label: e.name,
        detail: e.signature,
        info: e.desc,
        type: e.group === '环境' ? 'property' : 'function',
        apply: applyFor(e),
      })),
      validFor: /^[\w]*$/,
    }
  }
  if (text.includes('.')) return null
  return {
    from: word.from,
    options: [
      { label: 'host', type: 'variable', detail: '引擎注入的宿主 API', apply: 'host' },
      ...LUA_KEYWORDS.map(k => ({ label: k, type: 'keyword' })),
    ],
    validFor: /^[\w]*$/,
  }
}

const apiGroups = computed(() => {
  const groups: { label: string; items: LuaApiEntry[] }[] = []
  for (const key of ['只读', '可写', '环境'] as const) {
    const items = LUA_API_REFERENCE.filter(e => e.group === key)
    if (items.length) groups.push({ label: key + ' API', items })
  }
  return groups
})

const contextGroups = computed(() => {
  const c = props.context
  if (!c) return []
  const rows: { label: string; list: LuaIdOption[] }[] = [
    { label: '属性维度', list: c.attributes },
    { label: '资源', list: c.resources },
    { label: '标记', list: c.flags },
    { label: '事件', list: c.events },
    { label: '状态', list: c.statuses },
    { label: '人物', list: c.characters },
    { label: '地点', list: c.locations },
    { label: '关系类型', list: c.relationshipTypes },
  ]
  return rows.filter(r => r.list.length)
})

function insertText(text: string): void {
  if (!view) return
  view.dispatch(view.state.replaceSelection(text))
  view.focus()
}

function extensions(): Extension[] {
  return [
    basicSetup,
    luaLanguage,
    Prec.highest(syntaxHighlighting(highlight)),
    autocompletion({ override: [luaCompletionSource], activateOnTyping: true }),
    readOnlyCompartment.of(EditorState.readOnly.of(props.readonly)),
    theme,
    EditorView.lineWrapping,
    EditorView.theme({
      '&': { height: '100%' },
      '.cm-scroller': {
        minHeight: props.minHeight,
        maxHeight: props.maxHeight,
        height: props.maxHeight === '100%' ? '100%' : null,
      },
    }),
    EditorView.updateListener.of(update => {
      if (update.docChanged) emit('update:modelValue', update.state.doc.toString())
    }),
  ]
}

onMounted(() => {
  if (!host.value) return
  view = new EditorView({
    state: EditorState.create({ doc: props.modelValue ?? '', extensions: extensions() }),
    parent: host.value,
  })
})

watch(() => props.modelValue, value => {
  if (!view) return
  const current = view.state.doc.toString()
  if (value !== current) {
    view.dispatch({ changes: { from: 0, to: current.length, insert: value ?? '' } })
  }
})

watch(() => props.readonly, value => {
  view?.dispatch({ effects: readOnlyCompartment.reconfigure(EditorState.readOnly.of(value)) })
})

onBeforeUnmount(() => {
  view?.destroy()
  view = null
})

defineExpose({
  insertText,
})
</script>

<template>
  <div class="w-full h-full flex flex-col min-h-0">
    <div ref="host" class="code-editor flex-1 min-h-0"></div>

    <div v-if="showReference" class="mt-1.5">
      <button
        type="button"
        class="text-[11px] font-semibold text-muted-foreground transition-colors hover:text-primary"
        @click="refOpen = !refOpen"
      >
        {{ refOpen ? '收起 API 参考' : '查看可用 API 与资源' }}
      </button>
      <div v-if="refOpen" class="mt-1.5 max-h-72 space-y-2.5 overflow-auto rounded-lg border border-border/60 bg-muted/20 p-2.5">
        <div v-for="group in apiGroups" :key="group.label">
          <div class="mb-1 text-[10.5px] font-bold tracking-wider text-muted-foreground/70 uppercase">{{ group.label }}</div>
          <div class="flex flex-col gap-0.5">
            <button
              v-for="entry in group.items"
              :key="entry.name"
              type="button"
              class="flex flex-wrap items-baseline gap-x-2 rounded px-1 py-0.5 text-left transition-colors hover:bg-primary/10"
              :title="'点击插入：' + entry.desc"
              @click="insertText(entry.snippet)"
            >
              <code class="font-mono text-[11px] text-info">{{ entry.signature }}</code>
              <span class="text-[10.5px] text-muted-foreground">{{ entry.desc }}</span>
            </button>
          </div>
        </div>

        <div v-for="group in contextGroups" :key="group.label">
          <div class="mb-1 text-[10.5px] font-bold tracking-wider text-muted-foreground/70 uppercase">{{ group.label }}</div>
          <div class="flex flex-wrap gap-1">
            <button
              v-for="option in group.list"
              :key="option.id"
              type="button"
              class="rounded border border-border/60 bg-background/60 px-1.5 py-0.5 font-mono text-[10.5px] transition-colors hover:border-primary/40 hover:text-primary"
              :title="option.label"
              @click="insertText(option.id)"
            >
              {{ option.label ?? option.id }}
            </button>
          </div>
        </div>

        <p v-if="!contextGroups.length" class="text-[11px] text-muted-foreground/60">
          当前故事书还没有可引用的资源 / 属性 / 标记 —— 先去相应 tab 建好，再回来写脚本。
        </p>
      </div>
    </div>
  </div>
</template>
