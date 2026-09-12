<script setup lang="ts">
// DeclarationsPanel —— A「声明」tab：#01 修订 声明区
// 左侧 = 四类声明（flags / events / relationship_types / target_types）；右侧 = 该类条目的 key + label 编辑。
// 供引用校验使用（关系类型 / 技能目标 / flag_set 条件 / 触发事件），Lua 兜底可绕过。
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { DeclarationDef } from '@/types'
import { useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconFlag, IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('declarations')
function select(id: string): void { selected.value = id }

type DeclKey = 'flags' | 'events' | 'relationship_types' | 'target_types'
interface Group {
  key: DeclKey
  title: string
  desc: string
  keyPlaceholder: string
  labelPlaceholder: string
}

const GROUPS: Group[] = [
  { key: 'flags', title: '标记 flags', desc: '条件 flag_set 与「设标记」效果引用这些 key。', keyPlaceholder: 'met_isa', labelPlaceholder: '结识伊莎' },
  { key: 'events', title: '事件 events', desc: '效果触发器的 event 引用这些 key。', keyPlaceholder: 'scene_change', labelPlaceholder: '场景切换' },
  { key: 'relationship_types', title: '关系类型 relationship_types', desc: '关系边的 type 引用这些 key。', keyPlaceholder: '好感', labelPlaceholder: '好感' },
  { key: 'target_types', title: '目标类型 target_types', desc: '技能 target 引用这些 key。', keyPlaceholder: 'single', labelPlaceholder: '单体' },
]

function listOf(key: DeclKey): DeclarationDef[] {
  return d.value ? d.value[key] : []
}

const items = computed<WorkbenchItem[]>(() => GROUPS.map(g => ({
  id: g.key,
  title: g.title,
  sub: g.key,
  badge: listOf(g.key).length + ' 条',
})))

const current = computed(() => GROUPS.find(g => g.key === selected.value) ?? null)

function add(key: DeclKey): void {
  if (!d.value) return
  d.value[key].unshift({ key: '', label: '' })
}
function remove(key: DeclKey, i: number): void {
  d.value?.[key]?.splice(i, 1)
}
</script>

<template>
  <WorkbenchLayout
    title="声明"
    hint="集中声明可引用的标识；引用处只做存在性校验（Lua 兜底可绕过，见 #01 修订 / #12）。"
    :count="items.length"
    :items="items"
    :selected="selected"
    search-placeholder="搜索声明类型…"
    empty-hint="没有可用的声明类型。"
    @update:selected="select"
  >
    <template v-if="current">
      <EntityFormHeader :title="current.title" :sub="current.desc" :icon="IconFlag" />

      <div class="mt-4">
        <div class="mb-2 flex items-center justify-between gap-2">
          <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">条目</span>
          <div class="flex items-center gap-2">
            <Badge variant="outline" class="border-border/70 text-[10.5px] font-normal text-muted-foreground">{{ listOf(current.key).length }} 条</Badge>
            <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" @click="add(current.key)">
              <IconPlus class="size-3.5" />
              条目
            </Button>
          </div>
        </div>
        <div v-if="listOf(current.key).length" class="mb-1.5 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_2rem] gap-2 px-0.5">
          <span class="text-[11px] leading-4 font-medium text-muted-foreground">key（引用处填这个）</span>
          <span class="text-[11px] leading-4 font-medium text-muted-foreground">显示标签（给玩家看）</span>
          <span></span>
        </div>
        <div v-for="(e, i) in listOf(current.key)" :key="i" class="mb-2 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_2rem] items-center gap-2">
          <Input class="h-8 w-full font-mono text-xs" :model-value="e.key" :placeholder="current.keyPlaceholder" @update:model-value="e.key = String($event)" />
          <Input class="h-8 w-full text-[13px]" :model-value="e.label ?? ''" :placeholder="current.labelPlaceholder" @update:model-value="e.label = String($event)" />
          <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" aria-label="删除条目" @click="remove(current.key, i)">
            <IconX class="size-3.5" />
          </Button>
        </div>

        <p v-if="!listOf(current.key).length" class="rounded-lg border border-dashed border-border/70 py-6 text-center text-xs text-muted-foreground/60">
          该类还没有声明条目。引用处会因「找不到 key」被校验拦下。
        </p>
      </div>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一类声明。</div>
  </WorkbenchLayout>
</template>
