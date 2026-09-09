<script setup lang="ts">
// 模型选择器：自动探测（按 Base URL 反查目录）+ 搜索勾选 + 自定义添加。
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconX, IconWand, IconPlus } from '@tabler/icons-vue'
import { allModelIds, catalogModels, catalogProvider, detectProviderByUrl } from '@/api/model-catalog-utils'

const props = defineProps<{ modelValue: string[]; providerUrl?: string }>()
const emit = defineEmits<{
  (e: 'update:modelValue', v: string[]): void
  (e: 'pick', modelId: string): void
  (e: 'detected', providerId: string): void
}>()

const query = ref('')
const open = ref(false)
const tried = ref(false)

const selected = computed(() => props.modelValue ?? [])
// 始终由 Base URL 实时推导，改了地址立刻同步（不缓存手动探测结果）
const detectedId = computed(() => detectProviderByUrl(props.providerUrl ?? ''))
const detectedName = computed(() => (detectedId.value ? catalogProvider(detectedId.value)?.label ?? detectedId.value : ''))

const pool = computed<string[]>(() => (detectedId.value ? catalogModels(detectedId.value) : []))
const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  const base = pool.value.length ? pool.value : (q ? allModelIds() : [])
  return base
    .filter(id => !selected.value.includes(id) && (!q || id.toLowerCase().includes(q)))
    .slice(0, 60)
})

function add(id: string) {
  const v = id.trim()
  if (!v || selected.value.includes(v)) return
  emit('update:modelValue', [...selected.value, v])
  emit('pick', v)
  query.value = ''
  open.value = true
}
function remove(id: string) { emit('update:modelValue', selected.value.filter(x => x !== id)) }
function clear() { emit('update:modelValue', []) }

function detect() {
  tried.value = true
  open.value = true
  if (detectedId.value) emit('detected', detectedId.value)
}
function onEnter() { if (query.value.trim()) add(query.value.trim()) }
</script>

<template>
  <div class="w-full">
    <div class="mb-1.5 flex items-center gap-2">
      <Button size="xs" variant="outline" :disabled="!providerUrl" @click="detect">
        <IconWand data-icon="inline-start" />自动探测
      </Button>
      <span v-if="detectedId" class="text-[11px] text-success">已识别：{{ detectedName }} · {{ pool.length }} 个模型</span>
      <span v-else-if="tried" class="text-[11px] text-warning">未能从 Base URL 识别，可直接搜索或输入模型名</span>
      <span v-if="selected.length" class="ml-auto flex items-center gap-2">
        <span class="text-[11px] text-muted-foreground">已选 {{ selected.length }}</span>
        <Button size="xs" variant="ghost" @click="clear">清空</Button>
      </span>
    </div>

    <!-- 已选 chips -->
    <div v-if="selected.length" class="mb-1.5 flex flex-wrap gap-1">
      <span v-for="m in selected" :key="m" class="inline-flex items-center gap-1 rounded-md border border-border bg-muted/50 py-0.5 pr-1 pl-1.5 font-mono text-[11px]">
        {{ m }}
        <button type="button" class="rounded p-0.5 text-muted-foreground hover:bg-destructive/15 hover:text-destructive" :aria-label="'移除 ' + m" @click="remove(m)">
          <IconX class="size-3" />
        </button>
      </span>
    </div>

    <!-- 搜索 + 候选 -->
    <div class="relative">
      <Input
        v-model="query"
        placeholder="搜索模型并点击添加，回车也可添加自定义模型…"
        class="h-9"
        @focus="open = true"
        @keyup.enter="onEnter"
      />
      <div
        v-if="open"
        class="absolute z-50 mt-1 max-h-56 w-full overflow-y-auto rounded-lg border border-border bg-popover p-1 shadow-lg shadow-black/40"
        @mousedown.prevent
      >
        <button
          v-for="id in filtered"
          :key="id"
          type="button"
          class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-accent"
          @mousedown.prevent="add(id)"
        >
          <IconPlus class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate font-mono">{{ id }}</span>
        </button>
        <button
          v-if="query.trim() && !filtered.includes(query.trim())"
          type="button"
          class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] text-primary hover:bg-accent"
          @mousedown.prevent="add(query.trim())"
        >
          <IconPlus class="size-3.5 shrink-0" />
          添加自定义模型「{{ query.trim() }}」
        </button>
        <div v-if="!filtered.length && !query.trim()" class="px-2.5 py-3 text-center text-[11.5px] text-muted-foreground">
          点「自动探测」按 Base URL 拉取候选，或直接输入模型名
        </div>
        <div v-else-if="!filtered.length" class="px-2.5 py-3 text-center text-[11.5px] text-muted-foreground">没有匹配的模型</div>
      </div>
    </div>
  </div>
</template>
