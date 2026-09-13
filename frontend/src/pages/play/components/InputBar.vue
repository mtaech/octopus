<script setup lang="ts">
// 输入区（#08 ④）：同一输入框双通道 —— 普通输入=角色输入（受控角色名义），
// / 开头=元指令（channel:'meta'）。常驻快捷按钮直接发元指令文本。
// 迁移：<Textarea> 自动高 + 发送 <Button>；快捷按钮 <Button size=sm variant=ghost>。
import { ref, computed, watch, nextTick, onMounted } from 'vue'
import { usePlayStore } from '../stores/play'
import { useSettingsStore } from '@/pages/list/stores/settings'
import SettingsDialog from '@/pages/list/components/SettingsDialog.vue'
import type { EntityRef, Storybook, SaveModelChoice } from '@/types'
import { refKindLabel } from '@/lib/entity-refs'
import { estimateTokens, estimateTokensOf, fmtTokens } from '@/lib/tokens'
import RefList from './RefList.vue'
import { filterRefItems, groupRefItems, playRefItems, toEntityRef, type RefItem } from '../ref-items'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { IconSend, IconPlayerStopFilled, IconTerminal2, IconUser, IconDeviceFloppy, IconHelp, IconArrowsExchange, IconCheck, IconAt, IconWand, IconSwords, IconRobot, IconSettings, IconChartHistogram } from '@tabler/icons-vue'
import SaveModelPicker from '@/components/SaveModelPicker.vue'
import { catalogModelMeta, mergeModelMeta, wireToLevel, LEVEL_LABEL } from '@/api/model-catalog-utils'
import { toast } from '@/api'

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
/** AI 正在推理：此时「发送」换成「停止」（后端会真正取消在途调用）。 */
const aiThinking = computed(() => store.phase === 'story_thinking')
/** 停止按钮已点：等事件流收尾（System round_cancelled + RoundEnd）期间不再重复点。 */
const stopRequested = ref(false)
watch(aiThinking, v => { if (!v) stopRequested.value = false })
async function onStop(): Promise<void> {
  if (stopRequested.value) return
  stopRequested.value = true
  await store.stopRound()
}
const placeholder = computed(() => {
  if (store.waitingConfirm) return '有待确认动作 — 请先在上方确认或取消…'
  return '输入你想做的事（/ 开头 = 元指令，如 /存档 /帮助）…'
})

// ---------- 上下文用量 ----------
const settingsStore = useSettingsStore()
const settingsOpen = ref(false)
const usageOpen = ref(false)
onMounted(() => { if (!settingsStore.config) void settingsStore.load() })

// ---------- 本存档模型 / 思考强度（单一 AI） ----------
/** 可用供应商（来自全局配置）。 */
const providers = computed(() => settingsStore.config?.providers ?? [])
/** 模型弹层开关。 */
const modelDialogOpen = ref(false)
/** 全局 AI 默认（本存档未指定时继承）。 */
const globalModel = computed<SaveModelChoice>(() => {
  const gs = settingsStore.config?.roles.story
  return {
    provider_id: gs?.provider_id ?? '',
    model: gs?.model ?? '',
    reasoning_effort: gs?.reasoning_effort,
  }
})
/** 本存档是否在继承全局默认（没有存档级覆盖）。 */
const isModelInherited = computed(() => !store.saveModel)
/** 本存档生效的模型 = 存档覆盖 ?? 全局默认。 */
const effectiveModel = computed<SaveModelChoice>(() => store.saveModel ?? globalModel.value)
function modelMetaOf(m: SaveModelChoice) {
  if (!m.provider_id || !m.model) return undefined
  const entry = providers.value.find(p => p.id === m.provider_id)?.models.find(x => x.id === m.model)
  return mergeModelMeta(catalogModelMeta(m.provider_id, m.model), entry)
}
function modelDisplayName(pid?: string, mid?: string): string {
  if (!pid || !mid) return '未配置'
  const p = providers.value.find(x => x.id === pid)
  // 自定义条目常只有 id：显示名回落到目录快照。
  return p?.models.find(x => x.id === mid)?.name || catalogModelMeta(pid, mid)?.name || mid
}
/** 「模型名 · 思考档位」简报（默认档不显示强度）。 */
function modelSummaryOf(m: SaveModelChoice): string {
  const lv = wireToLevel(modelMetaOf(m), m.reasoning_effort)
  const effort = lv === 'default' ? '' : ' · ' + (LEVEL_LABEL[lv] ?? lv)
  return modelDisplayName(m.provider_id, m.model) + effort
}
/** 按钮上的简报。 */
const modelSummary = computed(() => modelSummaryOf(effectiveModel.value))
/** 继承状态里展示的全局默认。 */
const globalModelSummary = computed(() => modelSummaryOf(globalModel.value))
function onModelChange(v: SaveModelChoice) {
  void store.setModel(v)
}
function onUseGlobal() {
  void store.clearModel()
}

/** 系统提示词模板粗估：对应 Rust 侧 SYSTEM_PREAMBLE 大致长度 */
const PREAMBLE_TOKENS = 880

/** 本回合发给模型的新增上下文估算（不含按存档追加的会话历史；真实总量见 actual） */
const estimate = computed(() => {
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

/** 上一回合 AI 实际用量（供应商回传；null = 本存档还没跑过回合） */
const actual = computed(() => store.lastAiUsage)
/** 缓存命中率（%）：cached 是检验「缓存是否吃满」的关键指标 */
const cachePct = computed(() => {
  const a = actual.value
  return a && a.input > 0 ? Math.round((a.cached / a.input) * 100) : 0
})
/** 精确计数：192,971（与估算的 k/M 缩写区分） */
function fmtExact(n: number): string { return n.toLocaleString() }

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
          class="mention-editor max-h-[300px] min-h-[5rem] overflow-y-auto px-4 py-2.5 text-[15px] leading-relaxed whitespace-pre-wrap outline-none"
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
        v-if="aiThinking"
        variant="destructive"
        class="h-11 shrink-0 gap-1.5 rounded-xl px-5 text-[15px] font-bold shadow-sm transition-all"
        :disabled="stopRequested"
        title="取消这一回合的 AI 推理（在途请求会被真正中断，不再消耗 token）"
        @click="onStop"
      >
        <IconPlayerStopFilled class="size-4 mr-1" />
        <span>{{ stopRequested ? '停止中…' : '停止' }}</span>
      </Button>
      <Button
        v-else
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

        <!-- 上下文用量：有真实回传后显示上回合实际输入，否则显示本地粗估 -->
        <button
          type="button"
          class="inline-flex shrink-0 cursor-pointer items-center gap-1 rounded-full border border-border/80 px-2 py-0.5 text-[10.5px] text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
          :class="usageOpen ? 'border-primary/50 text-foreground' : ''"
          :title="actual ? '上回合 AI 实际用量（供应商回传，含会话历史），点击查看明细' : '本回合上下文用量（估算，不含会话历史）'"
          @click="usageOpen = !usageOpen"
        >
          <IconChartHistogram class="size-3 text-primary/70" />
          {{ actual ? '上回合 ' + fmtTokens(actual.input) : '本回合 ≈' + fmtTokens(estimate.total) }}
        </button>

        <Button
          variant="outline"
          size="sm"
          class="h-8 max-w-[220px] shrink-0 gap-1.5 border border-border/80 bg-card/80 px-2 text-[10.5px] font-semibold shadow-2xs hover:bg-muted/50"
          title="本存档模型与思考强度"
          @click="modelDialogOpen = true"
        >
          <IconRobot class="size-3.5 shrink-0 text-primary" />
          <span class="min-w-0 truncate">{{ modelSummary }}</span>
        </Button>

        <Button
          variant="ghost"
          size="icon-xs"
          class="size-6 shrink-0 text-muted-foreground hover:text-foreground"
          title="模型与 Provider 设置"
          @click="settingsOpen = true"
        >
          <IconSettings class="size-3.5" />
        </Button>

        <!-- 用量明细（贴右下角弹出）：真实回传 + 本地粗估 -->
        <div v-if="usageOpen" class="absolute right-0 bottom-full z-40 mb-2 w-64 rounded-xl border border-border bg-popover p-3 text-[11px] shadow-xl">
          <div class="flex items-center justify-between">
            <span class="font-semibold text-foreground">上下文用量</span>
            <span class="font-mono font-semibold text-foreground/85">{{ actual ? fmtTokens(actual.input) : '≈' + fmtTokens(estimate.total) }} tok</span>
          </div>

          <!-- 上回合实际：ai_call 事件回传的真实用量（含会话历史） -->
          <template v-if="actual">
            <div class="mt-1.5 text-[10px] text-muted-foreground/55">
              上回合实际 · 第 {{ actual.round }} 回合 · {{ modelDisplayName(actual.provider, actual.model) }}<template v-if="actual.calls > 1"> · {{ actual.calls }} 次调用</template>
            </div>
            <div class="mt-1 space-y-1">
              <div class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground/80">输入（含会话历史）</span>
                <span class="font-mono text-foreground/85">{{ fmtExact(actual.input) }}</span>
              </div>
              <div class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground/80">其中缓存命中</span>
                <span class="font-mono text-muted-foreground/70">{{ fmtExact(actual.cached) }} · {{ cachePct }}%</span>
              </div>
              <div v-if="actual.cacheWrite > 0" class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground/80">写入缓存</span>
                <span class="font-mono text-muted-foreground/70">{{ fmtExact(actual.cacheWrite) }}</span>
              </div>
              <div class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground/80">输出</span>
                <span class="font-mono text-muted-foreground/70">{{ fmtExact(actual.output) }}</span>
              </div>
              <div class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground/80">延迟</span>
                <span class="font-mono text-muted-foreground/70">{{ (actual.latencyMs / 1000).toFixed(1) }}s</span>
              </div>
            </div>
            <div class="mt-1.5 text-[10px] text-muted-foreground/55" title="按本页已加载的事件日志累计；向前翻页会继续累加">
              累计 {{ store.aiUsageTotal.calls }} 次调用 · 输入 {{ fmtTokens(store.aiUsageTotal.input) }} · 输出 {{ fmtTokens(store.aiUsageTotal.output) }}
            </div>
          </template>

          <!-- 本回合新增估算：下次调用将在历史之上多带的上下文 -->
          <div :class="actual ? 'mt-2 border-t border-border/60 pt-2' : 'mt-2'">
            <div class="flex items-center justify-between">
              <span class="text-muted-foreground/80">本回合新增估算</span>
              <span class="font-mono text-muted-foreground/70">≈{{ fmtTokens(estimate.total) }}</span>
            </div>
            <div class="mt-1 space-y-1">
              <div v-for="row in estimate.rows" :key="row.label" class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground/80">{{ row.label }}</span>
                <span class="font-mono text-muted-foreground/70">{{ fmtTokens(row.tokens) }}</span>
              </div>
            </div>
          </div>
          <p class="mt-2 border-t border-border/60 pt-2 text-[10px] leading-relaxed text-muted-foreground/55">
            实际值由供应商回传：输入 = 会话历史（前缀被缓存复用）+ 本回合新增。估算按 CJK 1 tok 粗估，仅作参考。
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

    <!-- 本存档模型 / 思考强度（单一 AI） -->
    <Dialog v-model:open="modelDialogOpen">
      <DialogContent class="gap-0 p-0 sm:max-w-md">
        <DialogHeader class="border-b border-border/70 px-5 py-4">
          <DialogTitle class="flex items-center gap-2 text-[15px]">
            <IconRobot class="size-4 text-primary" />
            模型与思考强度
          </DialogTitle>
          <DialogDescription class="text-[11.5px] leading-5">
            本存档使用；改动写回存档、只影响之后的回合。未指定时继承全局 AI 默认。
          </DialogDescription>
        </DialogHeader>
        <div class="px-5 py-4">
          <!-- 继承状态：明确现在用的是「本存档指定」还是「全局默认」，并给出全局默认内容 -->
          <div class="mb-4 flex items-center justify-between gap-3 rounded-lg border border-border/70 bg-muted/30 px-3 py-2">
            <div class="min-w-0">
              <div class="text-[12px] font-semibold">
                {{ isModelInherited ? '继承全局默认' : '本存档指定' }}
              </div>
              <div class="mt-0.5 truncate text-[11px] text-muted-foreground">
                全局默认：{{ globalModelSummary }}
              </div>
            </div>
            <Button
              v-if="!isModelInherited"
              size="sm"
              variant="outline"
              class="h-7 shrink-0 px-2 text-[11px]"
              @click="onUseGlobal"
            >恢复继承</Button>
          </div>

          <SaveModelPicker
            :model-value="effectiveModel"
            :providers="providers"
            @update:model-value="onModelChange"
          />
        </div>
        <DialogFooter class="border-t border-border/70 px-5 py-3 sm:justify-end">
          <Button size="sm" @click="modelDialogOpen = false">完成</Button>
        </DialogFooter>
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