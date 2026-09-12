<script setup lang="ts">
// NarrativePanel —— A「叙事」tab：故事书声明的「怎么讲」（按槽位注入的叙述段）
// 与世界词条分工：词条讲「世界有什么」，叙述段讲「怎么讲」。
// P1：when 条件段 + 变体组 + 玩家可否调整。
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { NarrativeSection } from '@/types'
import { uid } from '@/types'
import { toast } from '@/api'
import { convertStPreset, type StPresetProposal } from '@/lib/st-preset'
import StPresetImportDialog from './StPresetImportDialog.vue'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import ConditionEditor from '@/components/ConditionEditor.vue'
import { Button } from '@/components/ui/button'
import { IconEye, IconFileImport, IconMessage2, IconPlus, IconTrash } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('narrative')
function select(id: string): void { selected.value = id }

const SLOT_LABEL: Record<string, string> = { world: '世界', style: '文风', behavior: '行为', closing: '收尾' }
const SLOT_OPTIONS = [
  { value: 'world', label: '世界（前提旁）' },
  { value: 'style', label: '文风' },
  { value: 'behavior', label: '行为约束' },
  { value: 'closing', label: '收尾（输入之后）' },
]

const list = computed<NarrativeSection[]>(() => d.value?.narrative?.sections ?? [])

/** 故事书级的显示策略伪条目（不属于 sections，但放在同一工作台里编辑）。 */
const DISPLAY = '__display__'
const DRAFT_OPTIONS = [
  { value: 'folded', label: '折叠展示（默认）' },
  { value: 'hidden', label: '完全隐藏' },
]

/** 思考草稿展示策略：缺省 folded（引擎与校验都不读，纯前端展示）。 */
const draftDisplay = computed<'folded' | 'hidden'>(() => d.value?.narrative?.display?.draft ?? 'folded')
function setDraftDisplay(v: 'folded' | 'hidden'): void {
  const nar = d.value?.narrative
  if (!nar) return
  if (v === 'folded') {
    // 缺省就是 folded：删字段保持 JSON 干净
    if (nar.display) {
      delete nar.display.draft
      if (!Object.keys(nar.display).length) delete nar.display
    }
  } else {
    if (!nar.display) nar.display = {}
    nar.display.draft = v
  }
}

const items = computed<WorkbenchItem[]>(() => {
  const rows: WorkbenchItem[] = list.value.map(s => {
    const row: WorkbenchItem = {
      id: entityKey(s),
      title: s.title || s.id,
      sub: s.id,
      badge: SLOT_LABEL[s.slot] ?? s.slot,
      tone: s.enabled === false ? 'warn' : 'default',
      group: SLOT_LABEL[s.slot] ?? s.slot,
    }
    return row
  })
  // 伪条目放末尾：默认选中仍是第一条叙述段，不被契约设置抢走。
  rows.push({
    id: DISPLAY,
    title: '思考展示策略',
    sub: 'narrative.display.draft',
    badge: draftDisplay.value === 'hidden' ? '隐藏' : '折叠',
    tone: draftDisplay.value === 'hidden' ? 'warn' : 'default',
    group: '契约设置',
  })
  return rows
})

const current = computed(() => list.value.find(s => entityKey(s) === selected.value) ?? null)
const chars = computed(() => d.value?.characters ?? [])

// ---------- P3：ST 预设导入（提案 → 逐段采纳） ----------
const presetInput = ref<HTMLInputElement | null>(null)
const presetOpen = ref(false)
const proposal = ref<StPresetProposal | null>(null)

function pickPreset(): void { presetInput.value?.click() }
async function onPresetFile(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  try {
    const json = JSON.parse(await file.text()) as unknown
    const p = convertStPreset(json)
    if (!p.sections.length && !p.dropped.length) {
      toast('warn', '这个文件里没有可识别的 SillyTavern 预设内容')
      return
    }
    proposal.value = p
    presetOpen.value = true
  } catch (err) {
    toast('error', err instanceof Error ? err.message : '导入失败：不是合法的 JSON')
  }
}
function onAdopt(payload: { sections: NarrativeSection[]; rating: 'sfw' | 'nsfw' | null }): void {
  editor.adoptNarrativeSections(payload.sections, payload.rating ?? undefined)
  const first = payload.sections[0]
  if (first) selected.value = entityKey(first)
  toast('ok', '已采纳 ' + payload.sections.length + ' 个叙述段' + (payload.rating ? '，评级设为 ' + payload.rating.toUpperCase() : ''))
}

function add(): void {
  const sb = d.value
  if (!sb) return
  if (!sb.narrative) sb.narrative = { sections: [] }
  const s: NarrativeSection = { id: uid('nar'), title: '新叙述段', slot: 'style', scope: 'both', text: '', enabled: true }
  sb.narrative.sections.unshift(s)
  selected.value = entityKey(s)
}
function remove(s: NarrativeSection): void {
  const arr = d.value?.narrative?.sections
  if (!arr) return
  const i = arr.indexOf(s)
  if (i >= 0) arr.splice(i, 1)
}

/** scope 选择值：字符串 story/character/both，或 character:<人物id> */
const scopeValue = computed<string>({
  get: () => {
    const s = current.value?.scope
    if (s && typeof s === 'object') return 'character:' + s.characterId
    return s ?? 'both'
  },
  set: (v: string) => {
    const c = current.value
    if (!c) return
    c.scope = v.startsWith('character:')
      ? { characterId: v.slice('character:'.length) }
      : (v as 'story' | 'character' | 'both')
  },
})
const scopeOptions = computed(() => [
  { value: 'both', label: '两者' },
  { value: 'story', label: '仅主线 AI（旁白/世界）' },
  { value: 'character', label: '仅角色 AI（全部 NPC）' },
  ...chars.value.map(c => ({ value: 'character:' + c.id, label: '仅 ' + c.name })),
])

// ---------- P1：变体组 ----------
// text 与 variants 互斥（引擎发布校验）；UI 用一个开关在两种形态间切换，切换时搬运内容，
// 避免用户已写的文本被无声丢掉。
const isVariantGroup = computed(() => (current.value?.variants?.length ?? 0) > 0)
const defaultOptions = computed(() =>
  (current.value?.variants ?? []).map(v => ({ value: v.key, label: v.label ? v.label + '（' + v.key + '）' : v.key })),
)

function toVariantGroup(): void {
  const c = current.value
  if (!c || isVariantGroup.value) return
  c.variants = [{ key: 'v1', label: '默认', text: c.text ?? '' }]
  c.defaultVariant = 'v1'
  c.text = undefined
}
function toPlainText(): void {
  const c = current.value
  if (!c) return
  const def = c.variants?.find(v => v.key === c.defaultVariant) ?? c.variants?.[0]
  c.text = def?.text ?? c.text ?? ''
  c.variants = undefined
  c.defaultVariant = undefined
}
function addVariant(): void {
  const c = current.value
  if (!c) return
  if (!c.variants) c.variants = []
  const key = 'v' + (c.variants.length + 1)
  c.variants.push({ key, label: '', text: '' })
  if (!c.defaultVariant) c.defaultVariant = key
}
function removeVariant(i: number): void {
  const c = current.value
  const arr = c?.variants
  if (!c || !arr) return
  const removed = arr[i]
  arr.splice(i, 1)
  if (removed && c.defaultVariant === removed.key) c.defaultVariant = arr[0]?.key
  if (arr.length === 0) { c.variants = undefined; c.defaultVariant = undefined }
}
/** 改 key 时同步默认指向，避免 defaultVariant 悬空。 */
function renameVariant(i: number, key: string): void {
  const c = current.value
  const v = c?.variants?.[i]
  if (!c || !v) return
  if (c.defaultVariant === v.key) c.defaultVariant = key
  v.key = key
}
</script>

<template>
  <WorkbenchLayout
    title="叙事"
    hint="故事书声明的「怎么讲」：按槽位（世界/文风/行为/收尾）注入提示词；scope 决定只对主线 AI、只对角色 AI、还是两者生效。可加 when 条件、用变体组给出互斥选项。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="叙述段"
    search-placeholder="搜索叙述段…"
    empty-hint="还没有叙述段。点右上「叙述段」新增，或从 SillyTavern 预设导入。"
    custom-actions
    @update:selected="select"
    @add="add"
  >
    <template #railActions>
      <Button variant="outline" size="sm" class="h-7 gap-1 px-2 text-xs" title="从 SillyTavern 预设导入叙述段（先审查提案，再逐段采纳）" @click="pickPreset">
        <IconFileImport class="size-3.5" />
        <span>导入预设</span>
      </Button>
      <Button size="sm" class="h-7 gap-1 px-2 text-xs" @click="add">
        <IconPlus class="size-3.5" />
        <span>叙述段</span>
      </Button>
      <input ref="presetInput" type="file" accept=".json,application/json" class="hidden" @change="onPresetFile" />
    </template>

    <!-- 故事书级显示策略（非某一段） -->
    <template v-if="selected === DISPLAY">
      <EntityFormHeader title="思考展示策略" sub="narrative.display — 模型写在正文里的思考" :icon="IconEye" hide-remove />
      <FieldGrid class="mt-4">
        <FieldSelect
          label="思考草稿"
          :model-value="draftDisplay"
          :options="DRAFT_OPTIONS"
          :allow-empty="false"
          hint="模型用 think 意图写在正文里的思考怎么显示；供应商 reasoning_content 始终折叠"
          @update:model-value="setDraftDisplay($event as 'folded' | 'hidden')"
        />
      </FieldGrid>
      <p class="mt-3 max-w-2xl text-[11.5px] leading-5 text-muted-foreground/75">
        默认「折叠」：玩家点开才看到，与供应商思考链共用同一个组件。「完全隐藏」时前端不渲染这类草稿，
        适合不想暴露模型推演过程的故事书。此设置只影响展示，不改动任何历史事件。
      </p>
    </template>

    <template v-else-if="current">
      <EntityFormHeader :title="current.title || current.id" sub="叙述段 · 叙事契约" :icon="IconMessage2" :meta="current.id" @remove="remove(current)" />
      <FieldGrid class="mt-4">
        <FieldText label="标题" :model-value="current.title" @update:model-value="current.title = $event" />
        <FieldText label="id" mono :model-value="current.id" @update:model-value="current.id = $event" />
        <FieldSelect label="槽位" :model-value="current.slot" :options="SLOT_OPTIONS" hint="决定注入位置：世界=前提旁，文风/行为=输入前的「叙事要求」，收尾=输入之后" @update:model-value="current.slot = $event as NarrativeSection['slot']" />
        <FieldSelect label="作用范围" :model-value="scopeValue" :options="scopeOptions" hint="只影响你用哪个 AI 演绎，不影响引擎结算" @update:model-value="scopeValue = $event" />
      </FieldGrid>

      <!-- 内容形态：单文本 / 变体组（互斥） -->
      <div class="mt-5">
        <div class="mb-2 flex items-center justify-between gap-2">
          <span class="text-[11px] font-bold uppercase tracking-wider text-muted-foreground">内容形态</span>
          <Button v-if="!isVariantGroup" variant="outline" size="xs" class="h-7 gap-1 text-xs" title="把这行文本改为多个互斥选项" @click="toVariantGroup">
            <IconPlus class="size-3.5" /><span>改为变体组</span>
          </Button>
          <Button v-else variant="outline" size="xs" class="h-7 gap-1 text-xs" title="取默认变体的文本，改回单段常驻文本" @click="toPlainText">
            <IconTrash class="size-3.5" /><span>改回单文本</span>
          </Button>
        </div>

        <FieldArea
          v-if="!isVariantGroup"
          label="内容"
          :rows="7"
          md
          :model-value="current.text ?? ''"
          placeholder="如：用冷硬派笔触，短句为主；不要替玩家做决定…"
          hint="发给 AI 的原文；建议正向表述、控制篇幅"
          @update:model-value="current.text = $event"
        />

        <div v-else class="flex flex-col gap-3">
          <FieldSelect
            label="默认变体"
            :model-value="current.defaultVariant ?? ''"
            :options="defaultOptions"
            placeholder="（选择默认变体）"
            hint="玩家未在游玩页选择时使用；存档里的玩家选择优先于它"
            @update:model-value="current.defaultVariant = $event"
          />
          <div v-for="(v, i) in (current.variants ?? [])" :key="i" class="rounded-lg border border-border/70 bg-card/40 p-3">
            <div class="mb-2 flex items-center justify-between">
              <span class="text-[11px] font-bold text-muted-foreground">变体 {{ i + 1 }}<span v-if="current.defaultVariant === v.key" class="ml-1.5 rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] text-primary">默认</span></span>
              <Button variant="ghost" size="icon-xs" class="text-muted-foreground hover:text-destructive" title="删除该变体" @click="removeVariant(i)">
                <IconTrash class="size-3.5" />
              </Button>
            </div>
            <FieldGrid>
              <FieldText label="key" mono :model-value="v.key" placeholder="v1" @update:model-value="renameVariant(i, $event)" />
              <FieldText label="显示名" :model-value="v.label" placeholder="如：第一人称" @update:model-value="v.label = $event" />
              <FieldArea label="文本" :rows="3" md :model-value="v.text" placeholder="该变体注入的原文…" @update:model-value="v.text = $event" />
            </FieldGrid>
          </div>
          <Button variant="outline" size="sm" class="self-start gap-1 text-xs" @click="addVariant">
            <IconPlus class="size-3.5" /><span>新增变体</span>
          </Button>
        </div>
      </div>

      <!-- 注入条件 when：复用骨架条件编辑器 -->
      <div class="mt-5">
        <div class="mb-1 text-[11px] font-bold uppercase tracking-wider text-muted-foreground">注入条件 when（可选）</div>
        <p class="mb-2 text-[10.5px] leading-4 text-muted-foreground/70">世界状态满足条件时才注入该段；不设则每回合都注入。条件支持标记 / 触发点 / 地点 / 属性 / 关系 / Lua。</p>
        <ConditionEditor :model-value="current.when ?? null" @update:model-value="current.when = $event" />
      </div>

      <FieldGrid class="mt-5">
        <FieldSelect label="启用" :model-value="current.enabled === false ? 'no' : 'yes'" :options="[{ value: 'yes', label: '启用' }, { value: 'no', label: '停用' }]" @update:model-value="current.enabled = $event === 'yes'" />
        <FieldSelect
          label="玩家可调"
          :model-value="current.playerEditable ? 'yes' : 'no'"
          :options="[{ value: 'yes', label: '允许玩家开关 / 选变体' }, { value: 'no', label: '仅作者决定' }]"
          hint="开启后玩家可在游玩页调整；只影响之后的回合，不回写历史"
          @update:model-value="current.playerEditable = $event === 'yes'"
        />
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一条叙述段开始编辑。</div>
  </WorkbenchLayout>

  <StPresetImportDialog v-model:open="presetOpen" :proposal="proposal" @adopt="onAdopt" />
</template>
