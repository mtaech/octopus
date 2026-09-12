<script setup lang="ts">
// 输入区（#08 ④）：同一输入框双通道 —— 普通输入=角色输入（受控角色名义），
// / 开头=元指令（channel:'meta'）。常驻快捷按钮直接发元指令文本。
// 迁移：<Textarea> 自动高 + 发送 <Button>；快捷按钮 <Button size=sm variant=ghost>。
import { ref, computed, watch, nextTick, onMounted } from 'vue'
import { usePlayStore } from '../stores/play'
import { useSettingsStore } from '@/pages/list/stores/settings'
import SettingsDialog from '@/pages/list/components/SettingsDialog.vue'
import type { EntityRef, Storybook } from '@/types'
import { refKindLabel } from '@/lib/entity-refs'
import { estimateTokens, estimateTokensOf, fmtTokens } from '@/lib/tokens'
import RefList from './RefList.vue'
import { filterRefItems, groupRefItems, playRefItems, toEntityRef, type RefItem } from '../ref-items'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger } from '@/components/ui/select'
import { IconSend, IconTerminal2, IconUser, IconDeviceFloppy, IconHelp, IconArrowsExchange, IconCheck, IconAt, IconWand, IconSwords, IconRobot, IconSettings, IconChartHistogram, IconBrain } from '@tabler/icons-vue'
import { toast } from '@/api'
import { catalogModelMeta, mergeModelMeta, thinkingLevels, levelToWire, wireToLevel, LEVEL_LABEL, fmtContext } from '@/api/model-catalog-utils'

const store = usePlayStore()
/** 序列化后的纯文本（chip → @名字）；发送用 */
const input = ref('')
const boxEl = ref<HTMLElement | null>(null)
/** contenteditable 编辑区：引用 chip 与文字混排 */
const editorEl = ref<HTMLElement | null>(null)

// ---------- 实体引用（精准指向本次行动的目标） ----------
const pickerOpen = ref(false)
const pickerQuery = ref('')
const pickerTab = ref('')
/** 全部可引用实体（带分组与归属标注：我的 / 他人的 / 世界规则 / 剧情） */
const refItems = computed<RefItem[]>(() => {
  const sb = store.detail?.storybook as Storybook | undefined
  if (!sb) return []
  return playRefItems(sb, store.controlled, store.presentChars)
})
const pickerItems = computed<RefItem[]>(() => filterRefItems(refItems.value, pickerQuery.value))
function onPickRef(r: RefItem): void { insertChip(toEntityRef(r)) }
function openPicker(): void { pickerQuery.value = ''; pickerOpen.value = true }

// ---------- 编辑区：文字与引用 chip 混排 ----------
/** 序列化编辑区：文本节点照抄，chip 记成 @名字，并收集引用 */
function serializeEditor(): { text: string; refs: EntityRef[] } {
  const el = editorEl.value
  if (!el) return { text: '', refs: [] }
  let text = ''
  const refs: EntityRef[] = []
  for (const n of Array.from(el.childNodes)) {
    if (n.nodeType === Node.TEXT_NODE) { text += n.textContent ?? ''; continue }
    if (!(n instanceof HTMLElement)) continue
    if (n.tagName === 'BR') { text += '\n'; continue }
    const raw = n.dataset.ref
    if (raw) {
      try { refs.push(JSON.parse(raw) as EntityRef) } catch { /* noop */ }
      text += n.dataset.mention ?? ''
      continue
    }
    if (n.tagName === 'DIV' || n.tagName === 'P') text += '\n'
    text += n.textContent ?? ''
  }
  return { text, refs }
}
/** 编辑区变化 → 同步纯文本与 store 中的引用列表 */
function syncEditor(): void {
  const { text, refs } = serializeEditor()
  input.value = text
  store.clearRefs()
  for (const r of refs) store.addRef(r)
}
function makeChip(ref: EntityRef): HTMLElement {
  const chip = document.createElement('span')
  chip.className = 'mention-chip'
  chip.contentEditable = 'false'
  chip.dataset.ref = JSON.stringify(ref)
  chip.dataset.mention = '@' + ref.name
  chip.textContent = '@' + ref.name
  chip.title = '点击移除引用'
  chip.addEventListener('click', () => { chip.remove(); syncEditor() })
  return chip
}
/** 在光标处插入引用 chip（后跟一个空格，便于继续打字） */
function insertChip(ref: EntityRef): void {
  const el = editorEl.value
  if (!el) return
  el.focus()
  const sel = window.getSelection()
  const range = sel && sel.rangeCount > 0 ? sel.getRangeAt(0) : null
  if (!sel || !range || !el.contains(range.startContainer)) {
    el.appendChild(makeChip(ref))
    el.appendChild(document.createTextNode('\u00A0'))
    syncEditor()
    return
  }
  range.deleteContents()
  const space = document.createTextNode('\u00A0')
  range.insertNode(space)
  range.insertNode(makeChip(ref))
  const after = document.createRange()
  after.setStart(space, 1)
  after.collapse(true)
  sel.removeAllRanges()
  sel.addRange(after)
  syncEditor()
}

// ---------- @ 联想（读光标前的 token 过滤） ----------
const mentionOpen = ref(false)
const mentionQuery = ref('')
const mentionActive = ref(0)
const mentionTab = ref('')
const mentionFiltered = computed<RefItem[]>(() => filterRefItems(refItems.value, mentionQuery.value))
const mentionGroups = computed(() => groupRefItems(mentionFiltered.value))
/** 当前分类（为空时回落到第一个有内容的分类） */
const mentionTabName = computed(() =>
  mentionGroups.value.some(g => g.group === mentionTab.value) ? mentionTab.value : (mentionGroups.value[0]?.group ?? ''),
)
const mentionItems = computed<RefItem[]>(
  () => (mentionGroups.value.find(g => g.group === mentionTabName.value)?.items ?? []).slice(0, 80),
)
function closeMention(): void {
  mentionOpen.value = false
  mentionQuery.value = ''
  mentionActive.value = 0
}
function syncMention(): void {
  const el = editorEl.value
  const sel = window.getSelection()
  if (!el || !sel || !sel.rangeCount) { closeMention(); return }
  const range = sel.getRangeAt(0)
  if (!el.contains(range.startContainer) || range.startContainer.nodeType !== Node.TEXT_NODE) { closeMention(); return }
  const text = (range.startContainer.textContent ?? '').slice(0, range.startOffset)
  const at = text.lastIndexOf('@')
  if (at < 0 || /\s/.test(text.slice(at + 1))) { closeMention(); return }
  const q = text.slice(at + 1)
  // 只有查询变化（或刚打开）才把活动项归零——否则 ↑↓ 选完会被 keyup 弹回第一项。
  const changed = q !== mentionQuery.value || !mentionOpen.value
  mentionQuery.value = q
  if (changed) mentionActive.value = 0
  mentionOpen.value = mentionFiltered.value.length > 0
}
/** 用 chip 替换光标前的 @查询 */
function pickMention(it: RefItem): void {
  const el = editorEl.value
  const sel = window.getSelection()
  if (!el || !sel || !sel.rangeCount) return
  const range = sel.getRangeAt(0)
  if (range.startContainer.nodeType !== Node.TEXT_NODE) { insertChip(toEntityRef(it)); closeMention(); return }
  const node = range.startContainer
  const caret = range.startOffset
  const text = node.textContent ?? ''
  const at = text.lastIndexOf('@', caret - 1)
  const del = document.createRange()
  del.setStart(node, at < 0 ? caret : at)
  del.setEnd(node, caret)
  del.deleteContents()
  const space = document.createTextNode('\u00A0')
  del.insertNode(space)
  del.insertNode(makeChip(toEntityRef(it)))
  const after = document.createRange()
  after.setStart(space, 1)
  after.collapse(true)
  sel.removeAllRanges()
  sel.addRange(after)
  closeMention()
  syncEditor()
}
function onPaste(e: ClipboardEvent): void {
  e.preventDefault()
  const t = e.clipboardData?.getData('text/plain') ?? ''
  if (!t) return
  document.execCommand('insertText', false, t)
  syncEditor()
}
/** 输入通道：角色（默认）或导演（人代替 GM 推进剧情）。「/」前缀仍走元指令。 */
const mode = ref<'character' | 'gm'>('character')
const CHANNELS: { id: 'character' | 'gm'; label: string; desc: string }[] = [
  { id: 'character', label: '角色', desc: '以受控角色的身份行动' },
  { id: 'gm', label: '导演', desc: '人代替 GM 推进剧情：制造冲突 / 新增任务 / 调整数值。归属「故事本身」，不会算在受控角色头上' },
]
/** 导演指令快捷模板：点了填进输入框，你补完句子再发（AI 演绎并落结构化部分）。 */
const GM_ACTS = [
  { label: '旁白', text: '旁白：', hint: '以世界口吻叙述一段既成事实' },
  { label: '制造冲突', text: '制造冲突：', hint: '引入敌人 / 意外 / 障碍，AI 会扩写成叙事' },
  { label: '新增任务', text: '新增任务：', hint: '新增一个任务；可注明「主线」或「隐藏」' },
  { label: '调整数值', text: '调整数值：', hint: '例：莱纳斯 生命值 -3（会钳制到合法区间）' },
  { label: '施加状态', text: '施加状态：', hint: '例：诺德罗 施加「中毒」' },
]
/** 把指令模板填进编辑区，光标落在末尾 */
function prefill(t: string): void {
  const el = editorEl.value
  if (!el) return
  el.textContent = t
  el.focus()
  const range = document.createRange()
  range.selectNodeContents(el)
  range.collapse(false)
  const sel = window.getSelection()
  sel?.removeAllRanges()
  sel?.addRange(range)
  syncEditor()
}

/** 战斗快捷：有进行中的遭遇时出现，点了只是把句子写好，不锁输入 */
const activeEncounter = computed(() => (store.projection?.encounters ?? []).find(e => e.active) ?? null)
const COMBAT_ACTS = [
  { label: '攻击', text: '攻击 ' },
  { label: '施法', text: '用法术攻击 ' },
  { label: '观察', text: '仔细观察 ' },
  { label: '撤退', text: '试着撤退' },
]

const metaHint = computed(() => {
  const t = input.value.trim()
  if (t.startsWith('/')) return '元指令通道'
  if (mode.value === 'gm') return '导演通道 · 代替 GM 推进剧情'
  return '角色通道 · ' + (store.controlled?.name ?? '')
})
const isMeta = computed(() => metaHint.value.startsWith('元'))
const disabled = computed(() => !store.ready || store.sending)
const placeholder = computed(() => {
  if (store.waitingConfirm) return '有待确认动作 — 请先在上方确认或取消…'
  return '输入你想做的事（/ 开头 = 元指令，如 /存档 /帮助）…'
})

// ---------- 模型切换（story / character 角色模型）与上下文用量 ----------
const settingsStore = useSettingsStore()
const settingsOpen = ref(false)
const usageOpen = ref(false)

const providers = computed(() => settingsStore.config?.providers ?? [])
/** 生效模型：本存档覆盖优先，否则回落到全局 story 角色默认。 */
const effectiveModel = computed(() => {
  const m = store.saveModel
  if (m) return m
  const r = settingsStore.config?.roles.story
  return r?.provider_id && r?.model ? { provider_id: r.provider_id, model: r.model } : null
})
const currentModelValue = computed(() => {
  const m = effectiveModel.value
  return m ? m.provider_id + '::' + m.model : ''
})
const currentModelDisplay = computed(() => {
  const m = effectiveModel.value
  if (!m) return { providerName: '默认', modelName: '未配置' }
  const p = providers.value.find(x => x.id === m.provider_id)
  const mod = p?.models.find(x => x.id === m.model)
  return { providerName: p?.label || m.provider_id, modelName: mod?.name || m.model }
})
/** 只覆盖「本存档」，不写全局配置（全局默认在设置弹窗里改）。 */
function onModelChange(val: unknown) {
  if (typeof val !== 'string') return
  const [providerId, modelId] = val.split('::')
  if (providerId && modelId) void store.setModel(providerId, modelId, store.saveModel?.reasoning_effort)
}

// ---------- 思考强度（reasoning_effort）：档位来自 pi.dev 目录的 thinkingLevelMap ----------
/** 当前生效模型在目录里的元数据（上下文 / 思考能力）；自定义模型可能没有。 */
const activeMeta = computed(() => {
  const m = effectiveModel.value
  if (!m) return undefined
  // 目录元数据 + 用户在供应商里对该模型的自定义覆盖（小中转站）
  const entry = providers.value.find(p => p.id === m.provider_id)?.models.find(x => x.id === m.model)
  return mergeModelMeta(catalogModelMeta(m.provider_id, m.model), entry)
})
const supportsReasoning = computed(() => activeMeta.value?.reasoning !== false)
const effortOptions = computed(() => thinkingLevels(activeMeta.value))
const currentEffort = computed(() => wireToLevel(activeMeta.value, store.saveModel?.reasoning_effort))
const effortLabel = computed(() => LEVEL_LABEL[currentEffort.value] ?? currentEffort.value)
const ctxLabel = computed(() => fmtContext(activeMeta.value?.ctx))
/** 目录里某模型的上下文标签（下拉项展示用） */
function modelCtx(providerId: string, modelId: string): string {
  return fmtContext(catalogModelMeta(providerId, modelId)?.ctx)
}
function onEffortChange(val: unknown) {
  if (typeof val !== 'string') return
  const m = effectiveModel.value
  if (!m) { toast('warn', '请先在设置里配置模型'); return }
  void store.setModel(m.provider_id, m.model, levelToWire(activeMeta.value, val))
}
onMounted(() => { if (!settingsStore.config) void settingsStore.load() })

/** 系统提示词模板粗估：对应 Rust 侧 STORY_PREAMBLE + CHARACTER_PREAMBLE 大致长度 */
const PREAMBLE_TOKENS = 880

/** 本轮实际发给模型的上下文估算（游玩 AI 每回合不带历史，故只有这些） */
const usage = computed(() => {
  const p = store.projection
  const sb = store.detail?.storybook as Storybook | undefined
  const rows: { label: string; tokens: number }[] = []
  rows.push({ label: '提示词模板', tokens: PREAMBLE_TOKENS })
  let sceneText = ''
  let scenesText = ''
  if (sb && p) {
    const all: string[] = []
    for (const ch of sb.skeleton) {
      for (const sc of ch.scenes) {
        all.push(ch.title + ' · ' + sc.title + '（' + sc.id + '）')
        if (sc.id === p.scene_id) sceneText = sc.title + '\n' + (sc.description ?? '')
      }
    }
    scenesText = all.join('\n')
  }
  rows.push({ label: '场景', tokens: estimateTokens(sceneText || (p?.scene_title ?? '')) })
  rows.push({ label: '任务', tokens: estimateTokensOf(p?.quests ?? []) })
  rows.push({ label: '可推进场景', tokens: estimateTokens(scenesText) })
  rows.push({ label: '在场角色', tokens: estimateTokens(store.presentChars.map(c => c.name + '(' + c.template_id + ')').join('、')) })
  rows.push({ label: '遭遇', tokens: estimateTokensOf(p?.encounters ?? []) })
  rows.push({ label: '玩家输入', tokens: estimateTokens(input.value) })
  return { rows, total: rows.reduce((a, b) => a + b.tokens, 0) }
})

type Quick = { label: string; kind: 'meta' | 'toggle' | 'switch'; value?: string }
const QUICK: Quick[] = [
  { label: '/存档', kind: 'meta', value: '/存档' },
  { label: '免确认', kind: 'toggle' },
  { label: '换角色', kind: 'switch' },
  { label: '/帮助', kind: 'meta', value: '/帮助' },
]

function cycleCharacter() {
  const pcs = store.allChars.filter(c => c.kind === 'pc')
  if (pcs.length < 2) return
  const i = pcs.findIndex(c => c.instance_id === store.controlledId || c.template_id === store.controlledId)
  const next = pcs[(i + 1) % pcs.length]
  if (next) void store.switchTo(next.template_id)
}

function runQuick(q: Quick) {
  if (q.kind === 'toggle') { void store.setAutoConfirm(!store.autoConfirm); return }
  if (q.kind === 'switch') { cycleCharacter(); return }
  quickRun(q.value ?? '')
}

async function submit() {
  const t = input.value.trim()
  if (!t) return
  // 只有被受理才清空输入；busy / 未就绪时保留，避免丢字（P0-5）。
  const accepted = await store.send(t, mode.value)
  if (!accepted) return
  input.value = ''
  if (editorEl.value) editorEl.value.innerHTML = ''
  store.clearRefs()
  closeMention()
}

function onKeydown(e: KeyboardEvent) {
  // @ 联想打开时：上下选择 / Enter 确认 / Esc 取消（Enter 不再发送）
  if (mentionOpen.value) {
    // Tab / ← → 切分类（仅在联想打开时接管，避免与焦点跳转、光标移动冲突）
    if (e.key === 'Tab' || e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault()
      const names = mentionGroups.value.map(g => g.group)
      if (names.length) {
        const back = e.key === 'ArrowLeft' || (e.key === 'Tab' && e.shiftKey)
        const i = Math.max(0, names.indexOf(mentionTabName.value))
        const next = names[(i + (back ? names.length - 1 : 1)) % names.length]
        if (next) mentionTab.value = next
        mentionActive.value = 0
      }
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      mentionActive.value = Math.min(mentionActive.value + 1, mentionItems.value.length - 1)
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      mentionActive.value = Math.max(mentionActive.value - 1, 0)
      return
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      const it = mentionItems.value[mentionActive.value]
      if (it) { e.preventDefault(); pickMention(it); return }
    }
    if (e.key === 'Escape') { e.preventDefault(); closeMention(); return }
  }
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void submit()
    return
  }
}
function quickRun(label: string) {
  if (disabled.value || store.waitingConfirm) return
  input.value = label
  void submit()
}
</script>

<template>
  <div class="border-t border-border bg-card/90 px-4 pt-2.5 pb-3 backdrop-blur-md transition-colors">
    <div class="mb-2 flex items-center justify-between gap-2">
      <!-- 模式指示胶囊 -->
      <div
        class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[11px] font-bold shadow-2xs transition-all"
        :class="isMeta
          ? 'border-warning/40 bg-warning/10 text-warning'
          : mode === 'gm'
            ? 'border-info/45 bg-info/10 text-info'
            : 'border-primary/30 bg-primary/10 text-primary'"
      >
        <IconTerminal2 v-if="isMeta" class="size-3 text-warning" />
        <IconWand v-else-if="mode === 'gm'" class="size-3 text-info" />
        <IconUser v-else class="size-3 text-primary" />
        <span>{{ metaHint }}</span>
      </div>

      <!-- 通道切换：角色 / 导演（/ 前缀仍是元指令） -->
      <div class="inline-flex shrink-0 rounded-lg border border-border bg-background/80 p-0.5 shadow-inner">
        <button
          v-for="m in CHANNELS"
          :key="m.id"
          type="button"
          class="cursor-pointer rounded-md px-2 py-0.5 text-[11px] font-medium transition-colors"
          :class="!isMeta && mode === m.id
            ? (m.id === 'gm' ? 'bg-info/15 font-bold text-info' : 'bg-primary/15 font-bold text-primary')
            : 'text-muted-foreground hover:text-foreground'"
          :title="m.desc"
          @click="mode = m.id"
        >{{ m.label }}</button>
      </div>

      <!-- 快捷动作芯片 -->
      <div class="flex items-center gap-1.5">
        <Button
          v-for="q in QUICK"
          :key="q.label"
          size="xs"
          variant="outline"
          class="h-6 rounded-full border-border/80 px-2 text-[11px] font-medium transition-all hover:border-primary/50 hover:bg-muted/80"
          :class="{ 'border-primary/60 bg-primary/15 text-primary font-bold': q.kind === 'toggle' && store.autoConfirm }"
          :disabled="disabled || store.waitingConfirm"
          @click="runQuick(q)"
        >
          <template v-if="q.kind === 'meta' && q.label === '/存档'">
            <IconDeviceFloppy class="size-2.5 mr-0.5 text-muted-foreground" />
          </template>
          <template v-else-if="q.kind === 'meta' && q.label === '/帮助'">
            <IconHelp class="size-2.5 mr-0.5 text-muted-foreground" />
          </template>
          <template v-else-if="q.kind === 'switch'">
            <IconArrowsExchange class="size-2.5 mr-0.5 text-muted-foreground" />
          </template>
          <template v-else-if="q.kind === 'toggle' && store.autoConfirm">
            <IconCheck class="size-2.5 mr-0.5 text-primary" />
          </template>
          <span>{{ q.label }}</span>
        </Button>
      </div>
    </div>

    <!-- 战斗快捷（有遭遇时显示；只是帮你把句子写好，输入仍然自由） -->
    <div v-if="activeEncounter && !isMeta" class="mb-2 flex flex-wrap items-center gap-1.5">
      <span class="inline-flex items-center gap-1 text-[11px] font-medium text-destructive">
        <IconSwords class="size-3" />
        {{ activeEncounter.name }}
        <span class="text-muted-foreground">· {{ activeEncounter.enemies.filter(e => e.hp > 0).length }} 敌</span>
      </span>
      <button
        v-for="a in COMBAT_ACTS"
        :key="a.label"
        type="button"
        class="cursor-pointer rounded-full border border-destructive/40 bg-destructive/5 px-2 py-0.5 text-[11px] font-medium text-destructive transition-colors hover:bg-destructive/15"
        :title="'点击填入输入框（命中与伤害由引擎结算）'"
        @click="prefill(a.text)"
      >{{ a.label }}</button>
    </div>

    <!-- 导演指令快捷（仅导演通道显示） -->
    <div v-if="mode === 'gm' && !isMeta" class="mb-2 flex flex-wrap items-center gap-1.5">
      <span class="text-[11px] font-medium text-muted-foreground">导演指令</span>
      <button
        v-for="a in GM_ACTS"
        :key="a.label"
        type="button"
        class="cursor-pointer rounded-full border border-info/40 bg-info/5 px-2 py-0.5 text-[11px] font-medium text-info transition-colors hover:bg-info/15"
        :title="a.hint"
        @click="prefill(a.text)"
      >{{ a.label }}</button>
    </div>

    <!-- 输入区与操作按钮 -->
    <div class="flex items-end gap-2.5">
      <Button
        variant="outline"
        class="h-11 shrink-0 gap-1.5 rounded-xl px-3.5 font-medium"
        title="引用故事书实体（也可直接输入 @）"
        @click="openPicker()"
      >
        <IconAt class="size-4" />
        <span class="hidden sm:inline">引用</span>
      </Button>
      <div
        ref="boxEl"
        class="relative min-w-0 flex-1 rounded-xl border border-border bg-background shadow-inner transition-all focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/20"
      >
        <!-- @ 联想面板（在输入框上方，不遮挡正文） -->
        <div
          v-if="mentionOpen"
          class="absolute bottom-full left-0 z-30 mb-1.5 w-[min(100%,720px)] overflow-hidden rounded-xl border border-border bg-popover shadow-xl"
        >
          <div class="flex items-center justify-between border-b border-border/70 px-2.5 py-1 text-[10px] text-muted-foreground/70">
            <span>引用实体{{ mentionQuery ? ' · “' + mentionQuery + '”' : '' }}</span>
            <span class="font-mono">←→ 切分类 · ↑↓ 选择 · Enter 确认 · Esc 取消</span>
          </div>
          <RefList
            :items="mentionFiltered"
            :tab="mentionTab"
            :active="mentionActive"
            class="max-h-[420px]"
            @update:tab="(t: string) => (mentionTab = t)"
            @pick="pickMention"
          />
        </div>
        <!-- 文字与引用 chip 混排（contenteditable） -->
        <div
          ref="editorEl"
          class="mention-editor max-h-[440px] min-h-[12rem] overflow-y-auto px-4 py-3 text-[15px] leading-relaxed whitespace-pre-wrap outline-none"
          :contenteditable="!disabled"
          :data-placeholder="placeholder"
          @keydown="onKeydown"
          @keyup="syncMention"
          @input="syncEditor(); syncMention()"
          @click="syncMention"
          @paste="onPaste"
        ></div>
      </div>
      <Button
        class="h-11 shrink-0 gap-1.5 rounded-xl px-5 text-[15px] font-bold shadow-sm transition-all"
        :disabled="disabled || !input.trim()"
        @click="submit"
      >
        <IconSend class="size-4 mr-1 transition-transform group-hover:translate-x-0.5" />
        <span>发送</span>
      </Button>
    </div>

    <div class="mt-1.5 flex items-center justify-between gap-2 text-[10.5px] text-muted-foreground/70">
      <span class="inline-flex shrink-0 items-center gap-1">
        以角色 <b class="font-semibold text-foreground/90">{{ store.controlled?.name ?? '未选择' }}</b> 采取行动
      </span>

      <span class="relative flex min-w-0 items-center gap-1.5">
        <span class="hidden font-mono text-[10px] text-muted-foreground/50 2xl:inline">Enter 发送 · Shift+Enter 换行</span>

        <!-- 上下文用量（估算） -->
        <button
          type="button"
          class="inline-flex shrink-0 cursor-pointer items-center gap-1 rounded-full border border-border/80 px-2 py-0.5 text-[10.5px] text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
          :class="usageOpen ? 'border-primary/50 text-foreground' : ''"
          title="本轮上下文用量（估算）"
          @click="usageOpen = !usageOpen"
        >
          <IconChartHistogram class="size-3 text-primary/70" />
          上下文 ≈{{ fmtTokens(usage.total) }}
        </button>

        <!-- 模型切换：story + character 一起切 -->
        <Select :model-value="currentModelValue" @update:model-value="onModelChange">
          <SelectTrigger
            size="sm"
            class="h-6 max-w-[170px] gap-1 border border-border/80 bg-card/80 px-2 text-[10.5px] font-semibold shadow-2xs hover:bg-muted/50"
            :title="'游玩模型：' + currentModelDisplay.modelName + ' · ' + currentModelDisplay.providerName"
          >
            <IconRobot class="size-3 shrink-0 text-primary" />
            <span class="min-w-0 flex-1 truncate text-left">{{ currentModelDisplay.modelName }}</span>
            <span v-if="ctxLabel" class="shrink-0 rounded-full bg-muted/70 px-1.5 font-mono text-[9.5px] font-normal text-muted-foreground/80">{{ ctxLabel }}</span>
          </SelectTrigger>
          <SelectContent class="z-50 max-h-72">
            <template v-if="providers.length">
              <SelectGroup v-for="pr in providers" :key="pr.id">
                <SelectLabel class="px-2 py-1 text-[10.5px] font-bold text-muted-foreground uppercase">{{ pr.label }}</SelectLabel>
                <SelectItem v-for="m in pr.models" :key="pr.id + '::' + m.id" :value="pr.id + '::' + m.id" class="text-xs">
                  <div class="flex w-full items-center justify-between gap-4">
                    <span>{{ m.name || m.id }}</span>
                    <span class="flex shrink-0 items-center gap-2">
                      <span v-if="modelCtx(pr.id, m.id)" class="rounded-full bg-muted/70 px-1.5 font-mono text-[9.5px] text-muted-foreground/80">{{ modelCtx(pr.id, m.id) }}</span>
                      <span class="font-mono text-[10px] text-muted-foreground/70">{{ m.id }}</span>
                    </span>
                  </div>
                </SelectItem>
              </SelectGroup>
            </template>
            <div v-else class="p-2 text-center text-xs text-muted-foreground">尚未配置供应商</div>
          </SelectContent>
        </Select>

        <!-- 思考强度（reasoning_effort）：档位来自该模型在目录里的 thinkingLevelMap -->
        <Select v-if="supportsReasoning" :model-value="currentEffort" @update:model-value="onEffortChange">
          <SelectTrigger
            size="sm"
            class="h-6 gap-1 border border-border/80 bg-card/80 px-2 text-[10.5px] font-semibold shadow-2xs hover:bg-muted/50"
            title="模型思考强度（reasoning_effort）：本存档随模型一起保存"
          >
            <IconBrain class="size-3 shrink-0 text-primary" />
            <span class="min-w-0 truncate text-left">{{ effortLabel }}</span>
          </SelectTrigger>
          <SelectContent class="z-50">
            <SelectItem v-for="lv in effortOptions" :key="lv" :value="lv" class="text-xs">{{ LEVEL_LABEL[lv] ?? lv }}</SelectItem>
          </SelectContent>
        </Select>

        <Button
          variant="ghost"
          size="icon-xs"
          class="size-6 shrink-0 text-muted-foreground hover:text-foreground"
          title="模型与 Provider 设置"
          @click="settingsOpen = true"
        >
          <IconSettings class="size-3.5" />
        </Button>

        <!-- 用量明细（贴右下角弹出） -->
        <div v-if="usageOpen" class="absolute right-0 bottom-full z-40 mb-2 w-60 rounded-xl border border-border bg-popover p-3 text-[11px] shadow-xl">
          <div class="flex items-center justify-between">
            <span class="font-semibold text-foreground">本轮上下文估算</span>
            <span class="font-mono font-semibold text-foreground/85">≈{{ fmtTokens(usage.total) }} tok</span>
          </div>
          <div class="mt-2 space-y-1">
            <div v-for="row in usage.rows" :key="row.label" class="flex items-center justify-between gap-3">
              <span class="text-muted-foreground/80">{{ row.label }}</span>
              <span class="font-mono text-muted-foreground/70">{{ fmtTokens(row.tokens) }}</span>
            </div>
          </div>
          <p class="mt-2 border-t border-border/60 pt-2 text-[10px] leading-relaxed text-muted-foreground/55">
            CJK 按 1 tok 粗估。游玩 AI 每回合不带历史，故总量偏小；实际每轮会跑主线 + 角色两次调用。
          </p>
        </div>
      </span>
    </div>

    <!-- 实体引用选择器：按「我的 / 他人的 / 世界规则 / 剧情」分组，可搜索 -->
    <Dialog v-model:open="pickerOpen">
      <DialogContent class="sm:max-w-3xl gap-0 p-0">
        <DialogHeader class="border-b border-border/70 px-5 py-4">
          <DialogTitle class="text-[15px]">引用实体</DialogTitle>
          <DialogDescription class="text-[11.5px] leading-5">
            选中后，模型会拿到该实体的完整定义，并把本次行动聚焦在它身上。也可直接在输入框里打 @ 唤起联想。
          </DialogDescription>
        </DialogHeader>
        <div class="px-5 pt-3">
          <Input v-model="pickerQuery" placeholder="搜索名称 / 类型 / 归属人…" />
        </div>
        <div class="px-5 py-3">
          <RefList
            :items="pickerItems"
            :tab="pickerTab"
            class="max-h-[62vh]"
            @update:tab="(t: string) => (pickerTab = t)"
            @pick="onPickRef"
          />
        </div>
        <div class="flex items-center justify-between border-t border-border/70 px-5 py-3 text-[11px] text-muted-foreground/60">
          <span>共 {{ pickerItems.length }} 项</span>
          <Button size="sm" variant="ghost" class="h-7 px-2.5 text-xs" @click="pickerOpen = false">关闭</Button>
        </div>
      </DialogContent>
    </Dialog>

    <SettingsDialog v-model:open="settingsOpen" />
  </div>
</template>

<style scoped>
/* contenteditable 没有原生 placeholder */
.mention-editor:empty::before {
  content: attr(data-placeholder);
  color: color-mix(in oklab, var(--muted-foreground) 70%, transparent);
  pointer-events: none;
}
/* 引用 chip：与普通文字同排混排；点击即移除 */
.mention-editor :deep(.mention-chip) {
  display: inline-flex;
  align-items: center;
  margin: 0 1px;
  border-radius: 999px;
  border: 1px solid color-mix(in oklab, var(--primary) 42%, transparent);
  background-color: color-mix(in oklab, var(--primary) 14%, transparent);
  padding: 0 5px 0 7px;
  font-size: 12px;
  line-height: 1.55;
  color: var(--primary);
  cursor: pointer;
  user-select: none;
}
.mention-editor :deep(.mention-chip)::after {
  content: '×';
  margin-left: 3px;
  opacity: 0.5;
}
.mention-editor :deep(.mention-chip:hover) {
  background-color: color-mix(in oklab, var(--primary) 24%, transparent);
}
.mention-editor :deep(.mention-chip:hover)::after { opacity: 1 }
</style>