<script setup lang="ts">
// DeclarationsPanel —— A「声明」tab：#01 修订 声明区
// flags / events / relationship_types / target_types 四条列表，每条 key + 可选 label
// 供引用校验使用（关系类型 / 技能目标 / flag_set 条件 / 触发事件），Lua 兜底可绕过
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { DeclarationDef } from '@/types'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

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
  { key: 'target_types', title: '目标类型 target_types', desc: '技能 target 引用这些 key。', keyPlaceholder: 'single', labelPlaceholder: '单体' }
]

function listOf(key: DeclKey): DeclarationDef[] {
  return d.value ? d.value[key] : []
}
function add(key: DeclKey): void {
  if (!d.value) return
  d.value[key].push({ key: '', label: '' })
}
function remove(key: DeclKey, i: number): void {
  d.value?.[key]?.splice(i, 1)
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4">
      <h3 class="font-serif text-lg text-foreground">声明区</h3>
      <p class="mt-0.5 text-xs text-muted-foreground/70">集中声明可引用的标识；引用处只做存在性校验（Lua 兜底可绕过，见 #01 修订 / #12）。</p>
    </div>

    <div class="flex flex-col gap-3">
      <Card v-for="g in GROUPS" :key="g.key" class="gap-0 rounded-lg border-border/60 py-0 shadow-none ring-0">
        <header class="flex items-start gap-2.5 px-3.5 py-2">
          <div class="min-w-0 flex-1">
            <div class="text-[13.5px] leading-5 font-semibold text-foreground">{{ g.title }}</div>
            <div class="mt-0.5 text-[11px] leading-4 text-muted-foreground/70">{{ g.desc }}</div>
          </div>
          <Button variant="outline" size="sm" class="h-7 flex-none gap-1 text-xs" @click="add(g.key)">
            <IconPlus data-icon="inline-start" />
            条目
          </Button>
        </header>
        <div class="px-3.5 pb-3">
          <div v-if="!listOf(g.key).length" class="py-1 text-xs text-muted-foreground/70">还没有声明条目。</div>
          <div v-for="(e, i) in listOf(g.key)" :key="i" class="mb-1.5 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-1.5">
            <Input class="h-7 w-full font-mono text-xs" :model-value="e.key" :placeholder="g.keyPlaceholder" @update:model-value="e.key = String($event)" />
            <Input class="h-7 w-full text-[13px]" :model-value="e.label ?? ''" :placeholder="g.labelPlaceholder" @update:model-value="e.label = String($event)" />
            <Button variant="ghost" size="icon-xs" class="size-6 text-destructive hover:bg-destructive/10 hover:text-destructive" aria-label="删除条目" @click="remove(g.key, i)">
              <IconX />
            </Button>
          </div>
        </div>
      </Card>
    </div>
  </div>
</template>
