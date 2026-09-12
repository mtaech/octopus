<script setup lang="ts">
// WorkbenchLayout —— A 表单工作台外壳（#22 重做：左实体清单 + 右编辑表单）
// 只负责呈现与选中态；数据仍由各面板直接读写 editor.draft。
import { computed, ref, watch } from 'vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconPlus, IconSearch } from '@tabler/icons-vue'

export interface WorkbenchItem {
  id: string
  title: string
  sub?: string
  badge?: string
  tone?: 'default' | 'warn' | 'ok'
  group?: string
}

const props = withDefaults(defineProps<{
  title: string
  hint?: string
  count: number
  items: WorkbenchItem[]
  selected: string
  addLabel?: string
  searchPlaceholder?: string
  emptyHint?: string
  /** 为 true 时不渲染默认「新增」按钮（改用 #railActions 插槽） */
  customActions?: boolean
}>(), {
  hint: '',
  addLabel: '新增',
  searchPlaceholder: '搜索…',
  emptyHint: '还没有内容。',
  customActions: false,
})

const emit = defineEmits<{
  (e: 'update:selected', id: string): void
  (e: 'add'): void
}>()

const q = ref('')
const filtered = computed(() => {
  const kw = q.value.trim().toLowerCase()
  if (!kw) return props.items
  return props.items.filter(i => (i.title + ' ' + (i.sub ?? '') + ' ' + (i.badge ?? '')).toLowerCase().includes(kw))
})

/** 扁平渲染序列：group 变化处插入分组标题 */
const rows = computed(() => {
  const out: { kind: 'group' | 'item'; key: string; label: string; item: WorkbenchItem | null }[] = []
  let last = ''
  for (const it of filtered.value) {
    const g = it.group ?? ''
    if (g && g !== last) out.push({ kind: 'group', key: 'g-' + g, label: g, item: null })
    last = g
    out.push({ kind: 'item', key: it.id, label: '', item: it })
  }
  return out
})

/** 选中项被删除时回落到第一条（首个 item 键） */
watch(
  () => props.items.map(i => i.id).join('|'),
  () => {
    if (!props.items.length) return
    if (!props.items.some(i => i.id === props.selected)) emit('update:selected', props.items[0].id)
  },
  { immediate: true },
)

const toneCls = (t?: string) =>
  t === 'warn' ? 'border-warning/45 text-warning'
    : t === 'ok' ? 'border-success/45 text-success'
      : 'border-border/70 text-muted-foreground'
</script>

<template>
  <div class="flex h-full min-h-0">
    <!-- 左：实体清单 -->
    <aside class="flex w-72 flex-none flex-col border-r border-border bg-card/25">
      <div class="flex-none border-b border-border px-3 py-3">
        <div class="flex items-center gap-2">
          <h3 class="text-[13px] font-semibold text-foreground">{{ title }}</h3>
          <Badge variant="outline" class="border-border/80 px-1.5 font-mono text-[10.5px] text-muted-foreground">{{ count }}</Badge>
          <div class="ml-auto flex items-center gap-1">
            <slot name="railActions">
              <Button v-if="!customActions" size="sm" class="h-7 gap-1 px-2 text-xs" @click="emit('add')">
                <IconPlus class="size-3.5" />
                {{ addLabel }}
              </Button>
            </slot>
          </div>
        </div>
        <div class="relative mt-2">
          <IconSearch class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground/60" />
          <Input v-model="q" :placeholder="searchPlaceholder" class="h-7 pl-7 text-xs" />
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto p-2">
        <p v-if="!items.length" class="px-2 py-4 text-[11.5px] leading-5 text-muted-foreground/70">{{ emptyHint }}</p>
        <p v-else-if="!filtered.length" class="px-2 py-4 text-[11.5px] text-muted-foreground/70">没有匹配的条目。</p>
        <template v-for="r in rows" :key="r.key">
          <div v-if="r.kind === 'group'" class="px-2 pt-2.5 pb-1 text-[10.5px] font-bold tracking-[0.14em] text-muted-foreground/60 uppercase">
            {{ r.label }}
          </div>
          <template v-else-if="r.item">
            <slot name="item" :item="r.item" :selected="r.item.id === selected" :select="() => emit('update:selected', r.item!.id)">
              <button
                type="button"
                class="mb-1 flex w-full cursor-pointer flex-col gap-0.5 rounded-lg border px-2.5 py-1.5 text-left transition-colors"
                :class="r.item.id === selected ? 'border-primary/50 bg-primary/10 shadow-2xs' : 'border-transparent hover:border-border hover:bg-muted/50'"
                @click="emit('update:selected', r.item.id)"
              >
                <span class="flex items-center gap-1.5">
                  <span class="truncate text-[12.5px] font-semibold" :class="r.item.id === selected ? 'text-primary' : 'text-foreground'">
                    {{ r.item.title || '（未命名）' }}
                  </span>
                  <Badge v-if="r.item.badge" variant="outline" class="ml-auto shrink-0 px-1.5 text-[10px] font-normal" :class="toneCls(r.item.tone)">
                    {{ r.item.badge }}
                  </Badge>
                </span>
                <span v-if="r.item.sub" class="truncate font-mono text-[10.5px] text-muted-foreground/70">{{ r.item.sub }}</span>
              </button>
            </slot>
          </template>
        </template>
      </div>
    </aside>

    <!-- 右：编辑区 -->
    <section class="flex min-w-0 flex-1 flex-col">
      <div
        v-if="hint"
        class="flex-none truncate border-b border-border bg-card/20 px-5 py-1.5 text-[11px] leading-5 text-muted-foreground/75"
        :title="hint"
      >
        {{ hint }}
      </div>
      <div class="@container min-h-0 flex-1 overflow-y-auto px-5 py-4">
        <div class="mx-auto max-w-5xl">
          <slot v-if="items.length" />
          <div v-else class="flex min-h-64 items-center justify-center px-6 text-center text-xs leading-6 text-muted-foreground/60">
            <slot name="empty" />
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
