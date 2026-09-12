<script setup lang="ts">
// EntityPicker —— 「引用」选择器（结对编辑器）：分类 tab + 跨分类搜索。
// 解决「模型靠猜、用户说不清」：点这里精确指定要改的实体，模型拿到其完整定义。
import { computed, ref, watch } from 'vue'
import type { EntityRef } from '@/types'
import { refKindLabel } from '@/lib/entity-refs'
import { Input } from '@/components/ui/input'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { IconSearch } from '@tabler/icons-vue'

const props = defineProps<{ open: boolean; items: EntityRef[] }>()
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'select', ref: EntityRef): void
}>()

const query = ref('')
const tab = ref('')
watch(() => props.open, (v) => { if (v) { query.value = ''; tab.value = '' } })

/** 分类 tab：把细粒度 kind 归到人看得懂的大类 */
const TABS: { key: string; label: string; kinds: string[] }[] = [
  { key: 'plot', label: '剧情', kinds: ['chapter', 'scene', 'goal', 'trigger'] },
  { key: 'people', label: '人物', kinds: ['character', 'faction'] },
  { key: 'content', label: '内容', kinds: ['skill', 'item', 'object', 'status'] },
  { key: 'world', label: '世界', kinds: ['meta', 'world', 'location', 'resource', 'dimension', 'flag', 'event'] },
  { key: 'relation', label: '关系', kinds: ['relationship', 'relationship_type', 'target_type'] },
]
function tabOf(kind: string): string {
  return TABS.find(x => x.kinds.includes(kind))?.key ?? 'content'   // 开放内容归「内容」
}

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  return props.items.filter(r => !q || r.name.toLowerCase().includes(q) || r.kind.toLowerCase().includes(q))
})
const groups = computed(() =>
  TABS
    .map(t => ({ key: t.key, label: t.label, items: filtered.value.filter(r => tabOf(r.kind) === t.key) }))
    .filter(g => g.items.length),
)
/** 当前分类（被搜索过滤空时回落到第一个有内容的分类） */
const activeTab = computed(() => {
  if (tab.value && groups.value.some(g => g.key === tab.value)) return tab.value
  return groups.value[0]?.key ?? ''
})
const tabItems = computed(() => groups.value.find(g => g.key === activeTab.value)?.items ?? [])

function pick(r: EntityRef): void {
  emit('select', r)
  emit('update:open', false)
}
</script>

<template>
  <Dialog :open="props.open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogContent class="sm:max-w-2xl gap-0 p-0">
      <DialogHeader class="border-b border-border/70 px-5 py-4">
        <DialogTitle class="text-[15px]">引用实体</DialogTitle>
        <DialogDescription class="text-[11.5px] leading-5">
          选中后，模型会拿到该实体的完整定义，并把本次改动聚焦在它身上。
        </DialogDescription>
      </DialogHeader>

      <div class="px-5 pt-3">
        <div class="relative">
          <IconSearch class="absolute top-2.5 left-2.5 size-4 text-muted-foreground" />
          <Input v-model="query" class="pl-8" placeholder="跨分类搜索名称或类型…" />
        </div>
      </div>

      <!-- 分类 tab -->
      <div class="mt-3 flex flex-wrap gap-1 border-b border-border/70 px-5 pb-2">
        <button
          v-for="g in groups"
          :key="g.key"
          type="button"
          class="cursor-pointer rounded-full px-2.5 py-0.5 text-[11.5px] whitespace-nowrap transition-colors"
          :class="g.key === activeTab ? 'bg-primary/15 font-bold text-primary' : 'text-muted-foreground hover:bg-muted/70 hover:text-foreground'"
          @click="tab = g.key"
        >
          {{ g.label }}<span class="ml-1 font-mono text-[10px] opacity-70">{{ g.items.length }}</span>
        </button>
      </div>

      <div class="max-h-[48vh] overflow-y-auto px-5 py-2">
        <button
          v-for="it in tabItems"
          :key="it.kind + ':' + (it.id ?? '') + ':' + (it.parent_id ?? '')"
          type="button"
          class="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[12.5px] transition-colors hover:bg-primary/10"
          @click="pick(it)"
        >
          <span class="min-w-0 flex-1 truncate text-foreground">{{ it.name }}</span>
          <span class="shrink-0 rounded border border-border px-1.5 py-px text-[10px] text-muted-foreground/60">{{ refKindLabel(it.kind) }}</span>
          <span class="hidden shrink-0 font-mono text-[10px] text-muted-foreground/40 sm:inline">{{ it.id ?? '—' }}</span>
        </button>
        <p v-if="!tabItems.length" class="py-6 text-center text-xs text-muted-foreground/60">这个分类下没有匹配的实体</p>
      </div>

      <div class="flex items-center justify-between border-t border-border/70 px-5 py-3 text-[11px] text-muted-foreground/60">
        <span>共 {{ filtered.length }} 项</span>
        <span>共 {{ groups.length }} 个分类</span>
      </div>
    </DialogContent>
  </Dialog>
</template>
