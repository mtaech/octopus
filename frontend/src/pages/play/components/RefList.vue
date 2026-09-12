<script setup lang="ts">
// 引用列表（分类 tab + 键盘活动项 + 卡片式信息）：引用弹窗与输入框 @ 联想共用。
// 每项是一张卡：类型图标（按类型着色）+ 名称/归属/类型 + 两行简介 + 类型·消耗·目标等小标签。
// tab 之间互斥，避免「一个超长分组列表里翻找」。
import { computed, nextTick, ref, watch } from 'vue'
import { groupRefItems, type RefItem } from '../ref-items'
import { refKindLabel } from '@/lib/entity-refs'
import {
  IconWand, IconPackage, IconTarget, IconFlag, IconMapPin, IconUser,
  IconActivity, IconBook2, IconNotes, IconRulerMeasure, IconCoin, IconTag,
} from '@tabler/icons-vue'

const props = defineProps<{
  items: RefItem[]
  /** 当前分类 */
  tab?: string
  /** 键盘活动项（当前分类内的扁平下标） */
  active?: number
  emptyText?: string
}>()
const emit = defineEmits<{
  (e: 'pick', it: RefItem): void
  (e: 'update:tab', tab: string): void
}>()

const KIND_ICON: Record<string, unknown> = {
  skill: IconWand, item: IconPackage, goal: IconTarget, trigger: IconFlag,
  location: IconMapPin, character: IconUser, status: IconActivity,
  scene: IconBook2, chapter: IconNotes, dimension: IconRulerMeasure, resource: IconCoin,
}
const KIND_TONE: Record<string, string> = {
  skill: 'border-primary/30 bg-primary/10 text-primary',
  item: 'border-warning/35 bg-warning/10 text-warning',
  goal: 'border-success/35 bg-success/10 text-success',
  trigger: 'border-info/35 bg-info/10 text-info',
  location: 'border-info/35 bg-info/10 text-info',
  character: 'border-primary/30 bg-primary/10 text-primary',
  status: 'border-destructive/35 bg-destructive/10 text-destructive',
  scene: 'border-info/35 bg-info/10 text-info',
  chapter: 'border-info/35 bg-info/10 text-info',
  dimension: 'border-primary/30 bg-primary/10 text-primary',
  resource: 'border-warning/35 bg-warning/10 text-warning',
}
function iconOf(kind: string): unknown { return KIND_ICON[kind] ?? IconTag }
function toneOf(kind: string, active: boolean): string {
  if (active) return 'border-primary/50 bg-primary/15 text-primary'
  return KIND_TONE[kind] ?? 'border-border/80 bg-muted/50 text-muted-foreground'
}

const groups = computed(() => groupRefItems(props.items))
/** 当前分类：选中分类为空（被搜索过滤掉）时回落到第一个有内容的分类 */
const activeTab = computed(() => {
  if (props.tab && groups.value.some(g => g.group === props.tab)) return props.tab
  return groups.value[0]?.group ?? ''
})
const tabItems = computed(() => groups.value.find(g => g.group === activeTab.value)?.items ?? [])
function isActive(it: RefItem): boolean {
  return tabItems.value[props.active ?? -1] === it
}

// ---------- 键盘移动时把活动项滚进视野 ----------
const listEl = ref<HTMLElement | null>(null)
const rows = ref<(HTMLElement | null)[]>([])
function setRow(el: unknown, i: number): void {
  rows.value[i] = el instanceof HTMLElement ? el : null
}
/** 只在列表容器内滚动，不触发页面级 scrollIntoView */
function ensureActiveVisible(): void {
  const box = listEl.value
  const row = rows.value[props.active ?? -1]
  if (!box || !row) return
  const r = row.getBoundingClientRect()
  const b = box.getBoundingClientRect()
  if (r.top < b.top) box.scrollTop -= b.top - r.top
  else if (r.bottom > b.bottom) box.scrollTop += r.bottom - b.bottom
}
watch(
  () => [props.active, props.tab, props.items.length],
  async () => { await nextTick(); ensureActiveVisible() },
)
</script>

<template>
  <div class="flex min-h-0 flex-col">
    <!-- 分类 tab -->
    <div class="flex shrink-0 flex-wrap gap-1 border-b border-border/70 px-2 py-2">
      <button
        v-for="g in groups"
        :key="g.group"
        type="button"
        class="cursor-pointer rounded-full px-2 py-0.5 text-[11px] whitespace-nowrap transition-colors"
        :class="g.group === activeTab
          ? 'bg-primary/15 font-bold text-primary'
          : 'text-muted-foreground hover:bg-muted/70 hover:text-foreground'"
        @click="emit('update:tab', g.group)"
      >
        {{ g.group }}<span class="ml-1 font-mono text-[10px] opacity-70">{{ g.items.length }}</span>
      </button>
    </div>

    <div ref="listEl" class="min-h-0 flex-1 overflow-y-auto p-2">
      <button
        v-for="(it, ti) in tabItems"
        :key="it.kind + ':' + (it.id ?? '') + ':' + (it.parent_id ?? '')"
        :ref="(el) => setRow(el, ti)"
        type="button"
        class="mb-1.5 flex w-full cursor-pointer items-start gap-3 rounded-xl border p-2.5 text-left transition-colors last:mb-0"
        :class="isActive(it)
          ? 'border-primary/50 bg-primary/[0.06] ring-1 ring-primary/20'
          : 'border-border/70 bg-card/40 hover:border-primary/35 hover:bg-muted/50'"
        @mousedown.prevent="emit('pick', it)"
      >
        <span class="flex size-8 flex-none items-center justify-center rounded-lg border" :class="toneOf(it.kind, isActive(it))">
          <component :is="iconOf(it.kind)" class="size-4" />
        </span>

        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-1.5">
            <span class="truncate text-[13.5px] font-semibold text-foreground">{{ it.name }}</span>
            <span v-if="it.tag" class="shrink-0 rounded-full bg-muted px-1.5 py-px text-[10px] text-muted-foreground/80">{{ it.tag }}</span>
            <span class="ml-auto shrink-0 text-[10px] text-muted-foreground/55">{{ refKindLabel(it.kind) }}</span>
          </div>
          <p v-if="it.desc" class="mt-1 line-clamp-2 text-[12px] leading-relaxed text-muted-foreground">{{ it.desc }}</p>
          <div v-if="it.meta && it.meta.length" class="mt-1.5 flex flex-wrap gap-1">
            <span
              v-for="(m, mi) in it.meta"
              :key="mi"
              class="rounded bg-muted/70 px-1.5 py-px text-[10px] text-muted-foreground/80"
            >{{ m }}</span>
          </div>
        </div>
      </button>
      <p v-if="!tabItems.length" class="py-6 text-center text-xs text-muted-foreground/60">{{ emptyText ?? '这个分类下没有匹配项' }}</p>
    </div>
  </div>
</template>
