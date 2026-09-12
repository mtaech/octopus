<script setup lang="ts">
// OpenKindsPanel —— 元模型探针：一个面板承载「开放种类」与「开放内容」。
// 种类（KindDef）＝ 内容模板；内容＝按模板填的条目。必须先有种类才能加内容，
// 因此空态直接落到「种类」并给出可点的入口，避免「不知要先建种类」。
import { computed, ref, watch } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { Definition, KindDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import DynamicForm from './DynamicForm.vue'
import KindEditor from './KindEditor.vue'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { IconLayoutGrid, IconPlus } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

const mode = ref<'content' | 'kind'>('content')
const selectedContent = useWorkbenchSelection('open:content')
const selectedKind = useWorkbenchSelection('open:kind')

const kinds = computed<KindDef[]>(() => d.value?.kinds ?? [])
const defs = computed<Definition[]>(() => d.value?.definitions ?? [])

// 没有任何种类时，没有内容可填——直接进入「种类」，让第一步显式可见。
watch(() => kinds.value.length, (n) => { if (!n) mode.value = 'kind' }, { immediate: true })

function kindLabel(key: string): string { return kinds.value.find(k => k.key === key)?.label ?? key }

const contentItems = computed<WorkbenchItem[]>(() => defs.value.map(x => ({
  id: entityKey(x), title: x.name, sub: x.id, badge: kindLabel(x.kind), group: kindLabel(x.kind),
})))
const kindItems = computed<WorkbenchItem[]>(() => kinds.value.map(k => ({
  id: entityKey(k), title: k.label, sub: k.key, badge: (k.fields?.length ?? 0) + ' 字段', group: k.group || '未分组',
})))

const items = computed(() => (mode.value === 'kind' ? kindItems.value : contentItems.value))
const selected = computed(() => (mode.value === 'kind' ? selectedKind.value : selectedContent.value))

const currentDef = computed(() => defs.value.find(x => entityKey(x) === selectedContent.value) ?? null)
const currentKind = computed(() => kinds.value.find(k => entityKey(k) === selectedKind.value) ?? null)
const currentDefKind = computed(() => kinds.value.find(k => k.key === currentDef.value?.kind) ?? null)
const currentFields = computed(() => currentDefKind.value?.fields ?? [])

const addKind = ref('')
const activeAddKind = computed({
  get: () => addKind.value || kinds.value[0]?.key || '',
  set: (v: string) => { addKind.value = v },
})

function onSelect(id: string): void {
  if (mode.value === 'kind') selectedKind.value = id
  else selectedContent.value = id
}

function addDefinition(): void {
  const kind = kinds.value.find(k => k.key === activeAddKind.value)
  const draft = d.value
  if (!kind || !draft) return
  if (!draft.definitions) draft.definitions = []
  const def: Definition = {
    id: uid(kind.key.slice(0, 3) || 'def'), kind: kind.key, name: '新' + kind.label, description: '', fields: {},
  }
  draft.definitions.unshift(def)
  selectedContent.value = entityKey(draft.definitions[0])
}
function removeDefinition(x: Definition): void {
  const arr = d.value?.definitions
  if (!arr) return
  const i = arr.indexOf(x)
  if (i >= 0) arr.splice(i, 1)
}

function addKindDef(): void {
  const draft = d.value
  if (!draft) return
  if (!draft.kinds) draft.kinds = []
  const k: KindDef = {
    key: 'kind' + (draft.kinds.length + 1), label: '新种类',
    group: '自定义', fields: [{ key: 'name', label: '名称', type: 'text' }],
  }
  draft.kinds.push(k)
  selectedKind.value = entityKey(draft.kinds[draft.kinds.length - 1])
  mode.value = 'kind'
}
function removeKindDef(k: KindDef): void {
  const draft = d.value
  const ks = draft?.kinds
  if (!draft || !ks) return
  const i = ks.indexOf(k)
  if (i >= 0) ks.splice(i, 1)
  draft.definitions = (draft.definitions ?? []).filter(x => x.kind !== k.key)
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <!-- 步骤切换：种类（模板）→ 内容（条目） -->
    <div class="flex flex-none items-center gap-3 border-b border-border bg-card/40 px-3 py-1.5">
      <div class="flex items-center rounded-lg border border-border bg-background p-0.5">
        <button
          type="button"
          class="flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-1 text-xs font-medium transition-colors"
          :class="mode === 'kind' ? 'bg-primary text-primary-foreground shadow-2xs' : 'text-muted-foreground hover:bg-muted/70 hover:text-foreground'"
          @click="mode = 'kind'"
        >
          <span class="font-mono text-[10px] opacity-70">1</span>
          定义种类
        </button>
        <button
          type="button"
          class="flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-1 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-45"
          :class="mode === 'content' ? 'bg-primary text-primary-foreground shadow-2xs' : 'text-muted-foreground hover:bg-muted/70 hover:text-foreground'"
          :disabled="!kinds.length"
          :title="kinds.length ? '按种类填写内容' : '先定义至少一个种类'"
          @click="mode = 'content'"
        >
          <span class="font-mono text-[10px] opacity-70">2</span>
          填写内容
        </button>
      </div>
      <span class="truncate text-[11px] leading-5 text-muted-foreground/75">
        {{ mode === 'kind'
          ? '种类是内容的模板：声明它有哪些字段，内容按这些字段填。'
          : '按种类的字段填写具体条目。' }}
      </span>
    </div>

    <div class="min-h-0 flex-1">
      <WorkbenchLayout
        :title="mode === 'kind' ? '开放种类' : '开放内容'"
        :hint="mode === 'kind' ? '种类与字段由数据声明：新增一个故事概念，就是在这里加一条种类、声明它的字段。' : '表单按种类 schema 生成；新增概念无需改代码。'"
        :count="items.length"
        :items="items"
        :selected="selected"
        custom-actions
        :search-placeholder="mode === 'kind' ? '搜索种类…' : '搜索名称或 id…'"
        :empty-hint="mode === 'kind' ? '还没有种类。点右上「新建种类」。' : '还没有内容。点右上「新增」。'"
        @update:selected="onSelect"
      >
        <template #railActions>
          <template v-if="mode === 'content' && kinds.length">
            <Select v-model="activeAddKind">
              <SelectTrigger class="h-7 w-20 text-[11px]">
                <SelectValue placeholder="种类" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="k in kinds" :key="k.key" :value="k.key">{{ k.label }}</SelectItem>
              </SelectContent>
            </Select>
            <Button size="sm" class="h-7 gap-1 px-2 text-xs" @click="addDefinition">
              <IconPlus class="size-3.5" />
              新增
            </Button>
          </template>
          <Button v-else size="sm" class="h-7 gap-1 px-2 text-xs" @click="addKindDef">
            <IconPlus class="size-3.5" />
            新建种类
          </Button>
        </template>

        <template v-if="mode === 'kind' && currentKind">
          <EntityFormHeader
            :title="currentKind.label"
            :sub="'种类 · ' + currentKind.key"
            :icon="IconLayoutGrid"
            :meta="(currentKind.fields?.length ?? 0) + ' 字段'"
            @remove="removeKindDef(currentKind)"
          />
          <KindEditor class="mt-4" :kind-def="currentKind" />
        </template>
        <template v-else-if="mode === 'content' && currentDef">
          <EntityFormHeader
            :title="currentDef.name"
            :sub="kindLabel(currentDef.kind) + ' · 开放内容'"
            :icon="IconLayoutGrid"
            :meta="currentDef.id"
            @remove="removeDefinition(currentDef)"
          />
          <DynamicForm class="mt-4" :definition="currentDef" :fields="currentFields" />
        </template>

        <!-- 列表非空但未选中 -->
        <div v-else class="py-10 text-center text-xs text-muted-foreground/60">
          从左侧选择一项开始编辑。
        </div>

        <!-- 列表为空：给出可点的第一步（WorkbenchLayout 的空态走 #empty 插槽） -->
        <template #empty>
          <div class="mx-auto flex max-w-md flex-col items-center gap-3 py-2 text-center">
            <div class="flex size-10 items-center justify-center rounded-xl border border-primary/25 bg-primary/10 text-primary">
              <IconLayoutGrid class="size-5" />
            </div>
            <template v-if="mode === 'kind'">
              <p class="text-[12.5px] leading-6 text-muted-foreground">
                还没有种类。种类是内容的模板——比如「传闻」需要<strong class="text-foreground/85">来源 / 地点 / 可信度</strong>。
              </p>
              <Button size="sm" class="gap-1" @click="addKindDef">
                <IconPlus class="size-3.5" />
                新建第一个种类
              </Button>
            </template>
            <template v-else-if="!kinds.length">
              <p class="text-[12.5px] leading-6 text-muted-foreground">
                还没有种类，无法添加内容。<br />
                先定义一个种类，它决定内容要填哪些字段。
              </p>
              <Button size="sm" class="gap-1" @click="mode = 'kind'">
                <IconPlus class="size-3.5" />
                去定义第一个种类
              </Button>
            </template>
            <template v-else>
              <p class="text-[12.5px] leading-6 text-muted-foreground">还没有内容。</p>
              <Button size="sm" class="gap-1" @click="addDefinition">
                <IconPlus class="size-3.5" />
                新增第一条内容
              </Button>
            </template>
          </div>
        </template>
      </WorkbenchLayout>
    </div>
  </div>
</template>
