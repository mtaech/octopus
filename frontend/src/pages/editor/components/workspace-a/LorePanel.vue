<script setup lang="ts">
// LorePanel —— A「词条」tab：世界词条（关键词触发注入，控制上下文膨胀）
// 核心思路（借鉴 SillyTavern 世界书）：条目只在命中触发词时注入，每个条目 3-5 句最有效。
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { LoreDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { IconBook, IconPlus, IconFileImport } from '@tabler/icons-vue'
import { Button } from '@/components/ui/button'
import { toast } from '@/api'
import { importStFile } from '@/lib/st-import'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('lore')
function select(id: string): void { selected.value = id }

const list = computed<LoreDef[]>(() => d.value?.lore ?? [])
const items = computed<WorkbenchItem[]>(() => list.value.map(l => ({
  id: entityKey(l),
  title: l.title || l.id,
  sub: l.constant ? '常驻' : (l.keys ?? []).slice(0, 3).join(' / '),
  badge: l.enabled === false ? '停用' : (l.constant ? '常驻' : undefined),
  tone: l.enabled === false ? 'warn' : 'default',
  group: l.constant ? '常驻词条' : '关键词词条',
})))

const current = computed(() => list.value.find(l => entityKey(l) === selected.value) ?? null)

function add(): void {
  const arr = d.value?.lore
  if (!arr) {
    if (!d.value) return
    d.value.lore = []
  }
  const bag = d.value!.lore!
  const l: LoreDef = { id: uid('lore'), title: '新词条', content: '', keys: [], priority: 0, enabled: true }
  bag.unshift(l)
  selected.value = entityKey(l)
}
function remove(l: LoreDef): void {
  const arr = d.value?.lore
  if (!arr) return
  const i = arr.indexOf(l)
  if (i >= 0) arr.splice(i, 1)
}
function keysText(l: LoreDef): string { return (l.keys ?? []).join(String.fromCharCode(10)) }
function setKeys(l: LoreDef, t: string): void {
  l.keys = t.split(String.fromCharCode(10)).map(s => s.trim()).filter(Boolean)
}
const YESNO = [{ value: 'no', label: '否' }, { value: 'yes', label: '是' }]
function yn(v: boolean | undefined): 'yes' | 'no' { return v ? 'yes' : 'no' }

// ---- SillyTavern 世界书导入（JSON） ----
const fileInput = ref<HTMLInputElement | null>(null)
const importing = ref(false)
function pickFile(): void { fileInput.value?.click() }
async function onFile(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  importing.value = true
  try {
    const res = await importStFile(file)
    if (!res.lore.length) {
      toast('warn', '这个文件里没有可导入的世界书词条')
      return
    }
    if (!d.value) return
    if (!d.value.lore) d.value.lore = []
    d.value.lore.unshift(...res.lore)
    selected.value = entityKey(res.lore[0])
    toast('ok', `已导入 ${res.lore.length} 条世界词条`)
  } catch (err) {
    toast('error', err instanceof Error ? err.message : '导入失败')
  } finally {
    importing.value = false
  }
}
</script>

<template>
  <WorkbenchLayout
    title="词条"
    hint="世界词条：命中触发词才注入给 AI（常驻词条每回合都注入）。每个条目 3-5 句最有效——少即是多。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="词条"
    search-placeholder="搜索词条 / 触发词…"
    empty-hint="还没有词条。点右上「词条」新增，或从 SillyTavern 世界书导入。"
    custom-actions
    @update:selected="select"
    @add="add"
  >
    <template #railActions>
      <Button variant="outline" size="sm" class="h-7 gap-1 px-2 text-xs" :disabled="importing" title="导入 SillyTavern 世界书 / 角色书 JSON" @click="pickFile">
        <IconFileImport class="size-3.5" />
        {{ importing ? '导入中…' : '导入' }}
      </Button>
      <Button size="sm" class="h-7 gap-1 px-2 text-xs" @click="add">
        <IconPlus class="size-3.5" />
        词条
      </Button>
      <input ref="fileInput" type="file" accept=".json,application/json" class="hidden" @change="onFile" />
    </template>
    <template v-if="current">
      <EntityFormHeader :title="current.title || current.id" sub="世界词条 · 关键词触发注入" :icon="IconBook" :meta="current.id" @remove="remove(current)" />
      <FieldGrid class="mt-4">
        <FieldText label="标题" :model-value="current.title" @update:model-value="current.title = $event" />
        <FieldText label="id" mono :model-value="current.id" @update:model-value="current.id = $event" />
        <FieldArea
          label="触发词（每行一个）"
          :rows="3"
          :model-value="keysText(current)"
          placeholder="暗影森林"
          hint="中/英/简称/昵称都写上，命中任意一个即注入；常驻词条可留空"
          @update:model-value="setKeys(current, $event)"
        />
        <FieldArea
          label="词条内容"
          :rows="6"
          md
          :model-value="current.content"
          placeholder="3-5 句最核心的事实，只回答一个问题……"
          hint="注入给 AI 的正文：只放当下需要知道的关键信息，长篇设定留在别处"
          @update:model-value="current.content = $event"
        />
        <FieldNum label="优先级" :min="-99" :max="99" hint="越大越靠前注入（核心世界观调高）" :model-value="current.priority ?? 0" @update:model-value="current.priority = $event" />
        <FieldSelect label="常驻" :model-value="yn(current.constant)" :options="YESNO" hint="常驻：无需触发词，每回合都注入" @update:model-value="current.constant = $event === 'yes'" />
        <FieldSelect label="递归扫描" :model-value="yn(current.recursive)" :options="YESNO" hint="命中后其内容里再出现其它词条的触发词时继续连锁命中" @update:model-value="current.recursive = $event === 'yes'" />
        <FieldSelect label="启用" :model-value="current.enabled === false ? 'no' : 'yes'" :options="YESNO" @update:model-value="current.enabled = $event === 'yes'" />
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个词条开始编辑。</div>
  </WorkbenchLayout>
</template>
