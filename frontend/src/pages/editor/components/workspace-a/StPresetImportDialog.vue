<script setup lang="ts">
// StPresetImportDialog —— SillyTavern 预设导入的「待审查提案」对话框（P3 §B）。
// 原则：导入不直接改草稿；先在这里逐段展示「源名 / 建议槽位 / 预览」，创作者勾选 + 改槽位后采纳。
// 正则脚本与占位符只列出「不导入」的原因；推断 rating 作为默认值，可改。
import { computed, ref, watch } from 'vue'
import type { NarrativeSection } from '@/types'
import type { ProposedSection, StPresetProposal, StRating, StSectionSlot } from '@/lib/st-preset'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import FieldSelect from '../fields/FieldSelect.vue'
import { IconCircleCheck, IconEyeOff, IconFileImport } from '@tabler/icons-vue'

const open = defineModel<boolean>('open', { default: false })

const props = defineProps<{ proposal: StPresetProposal | null }>()

const emit = defineEmits<{
  (e: 'adopt', payload: { sections: NarrativeSection[]; rating: StRating | null }): void
}>()

const SLOT_OPTIONS = [
  { value: 'style', label: '文风' },
  { value: 'behavior', label: '行为约束' },
  { value: 'world', label: '世界（前提旁）' },
  { value: 'closing', label: '收尾（输入之后）' },
]
const RATING_OPTIONS = [
  { value: 'sfw', label: 'SFW（全年龄）' },
  { value: 'nsfw', label: 'NSFW（成人向）' },
]
const KIND_LABEL: Record<string, string> = {
  placeholder: '占位符',
  regex: '正则脚本',
  empty: '空内容',
  header: '分组标题',
  config: '内部配置',
  sampling: '采样参数',
}

interface Row {
  key: string
  selected: boolean
  slot: StSectionSlot
  section: ProposedSection
}

const rows = ref<Row[]>([])
const rating = ref<StRating>('sfw')
const applyRating = ref(true)

// proposal 变化 / 每次打开时重置为「全部勾选 + 建议槽位」。
watch(
  () => [open.value, props.proposal] as const,
  () => {
    const p = props.proposal
    rows.value = (p?.sections ?? []).map((s, i) => ({ key: 'r' + i, selected: true, slot: s.slot, section: s }))
    rating.value = p?.rating ?? 'sfw'
    applyRating.value = true
  },
  { immediate: true },
)

const selectedCount = computed(() => rows.value.filter(r => r.selected).length)
const droppedCount = computed(() => props.proposal?.dropped.length ?? 0)

/** 预览文本：普通段显示正文；变体组显示各选项标签。 */
function preview(s: ProposedSection): string {
  if (s.variants?.length) return s.variants.map(v => '· ' + (v.label || v.key) + '：' + v.text).join(String.fromCharCode(10))
  return s.text ?? ''
}

/** 提案段 → 可写入草稿的叙述段（丢弃来源信息，缺省启用、仅作者可改）。 */
function toSection(s: ProposedSection, slot: StSectionSlot): NarrativeSection {
  const out: NarrativeSection = { id: s.id, title: s.title, slot, scope: 'both', enabled: true }
  if (s.variants?.length) {
    out.variants = s.variants.map(v => ({ ...v }))
    out.defaultVariant = s.defaultVariant
  } else {
    out.text = s.text ?? ''
  }
  return out
}

function adopt(): void {
  const sections = rows.value.filter(r => r.selected).map(r => toSection(r.section, r.slot))
  emit('adopt', { sections, rating: applyRating.value ? rating.value : null })
  open.value = false
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-w-2xl">
      <DialogHeader>
        <DialogTitle>导入 SillyTavern 预设</DialogTitle>
        <DialogDescription>
          这是待审查提案：逐段确认后才会写入叙事契约。ST 的分组与顺序机制不保留，只做<strong>语义映射</strong>；
          正则脚本与占位符不会导入。
        </DialogDescription>
      </DialogHeader>

      <div class="max-h-[62vh] overflow-y-auto pr-1">
        <!-- 推断评级 -->
        <div class="mb-3 flex flex-wrap items-end gap-3 rounded-xl border border-border/80 bg-card/70 p-3">
          <FieldSelect :model-value="rating" label="内容评级" class="w-48" :options="RATING_OPTIONS" :allow-empty="false" hint="从 NSFW 关键词推断，可改" @update:model-value="rating = $event as StRating" />
          <label class="mb-1 flex cursor-pointer items-center gap-2 text-xs text-muted-foreground">
            <input v-model="applyRating" type="checkbox" class="size-3.5 cursor-pointer rounded border-border accent-primary" />
            <span>采纳时一并设置 meta.rating</span>
          </label>
        </div>

        <!-- 不导入项 -->
        <div v-if="droppedCount" class="mb-3 rounded-xl border border-border/70 bg-muted/30 p-3">
          <div class="mb-1.5 flex items-center gap-1.5 text-[11px] font-bold text-muted-foreground">
            <IconEyeOff class="size-3.5" />
            <span>以下 {{ droppedCount }} 项不会导入</span>
          </div>
          <ul class="flex flex-col gap-1">
            <li v-for="(d, i) in (proposal?.dropped ?? [])" :key="i" class="flex items-start gap-2 text-[11px] leading-4">
              <Badge variant="outline" class="mt-px shrink-0 border-border/80 px-1.5 text-[10px] font-normal text-muted-foreground">{{ KIND_LABEL[d.kind] ?? d.kind }}</Badge>
              <span class="font-medium text-foreground/85">{{ d.name }}</span>
              <span class="text-muted-foreground/75">— {{ d.reason }}</span>
            </li>
          </ul>
        </div>

        <!-- 逐段提案 -->
        <div v-if="!rows.length" class="py-8 text-center text-xs text-muted-foreground/70">
          没有可导入的叙述段（这个预设只有占位符 / 正则脚本）。
        </div>
        <div v-for="r in rows" :key="r.key" class="mb-2.5 rounded-xl border border-border/80 bg-card/90 p-3 shadow-2xs">
          <div class="flex items-start gap-3">
            <input v-model="r.selected" type="checkbox" class="mt-0.5 size-4 shrink-0 cursor-pointer rounded border-border accent-primary" />
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-1.5">
                <span class="truncate text-[13px] font-bold text-foreground">{{ r.section.title }}</span>
                <Badge v-if="r.section.variants?.length" variant="outline" class="border-primary/40 px-1.5 text-[10px] text-primary">变体组 {{ r.section.variants.length }} 选 1</Badge>
                <Badge v-if="r.section.needsConfirm" variant="outline" class="border-warning/50 px-1.5 text-[10px] text-warning">需要你确认</Badge>
              </div>
              <div class="mt-0.5 flex items-center gap-2 font-mono text-[10.5px] text-muted-foreground/70">
                <IconFileImport class="size-3" />
                <span class="truncate">{{ r.section.sourceIdentifier }}</span>
              </div>
              <pre class="mt-1.5 max-h-24 overflow-y-auto whitespace-pre-wrap break-words rounded-lg bg-muted/40 px-2 py-1.5 font-sans text-[11px] leading-4 text-muted-foreground">{{ preview(r.section) }}</pre>
            </div>
            <div class="w-32 shrink-0">
              <FieldSelect
                :model-value="r.slot"
                label="建议槽位"
                dense
                :options="SLOT_OPTIONS"
                :allow-empty="false"
                @update:model-value="r.slot = $event as StSectionSlot"
              />
            </div>
          </div>
        </div>
      </div>

      <DialogFooter class="mt-1">
        <div class="mr-auto flex items-center gap-1.5 text-[11px] text-muted-foreground">
          <IconCircleCheck class="size-3.5 text-success" />
          <span>将采纳 {{ selectedCount }} 段</span>
        </div>
        <Button variant="outline" size="sm" @click="open = false">取消</Button>
        <Button size="sm" :disabled="!selectedCount" @click="adopt">采纳选中段</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
