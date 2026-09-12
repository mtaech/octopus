<script setup lang="ts">
// DynamicForm —— 元模型探针：按 KindDef.fields 渲染任意开放种类的表单。
// 关键主张：新增一个故事概念 = 故事书里加一条 KindDef + 若干 Definition，零新增 Vue 组件。
import { computed } from 'vue'
import type { AssetRef, Definition, FieldDef } from '@/types'
import { useEditorStore } from '../../stores/editor'
import { listEntityRefs } from '@/lib/entity-refs'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import ImageField from '../fields/ImageField.vue'

const props = defineProps<{ definition: Definition; fields: FieldDef[] }>()
const editor = useEditorStore()

const refs = computed(() => (editor.draft ? listEntityRefs(editor.draft) : []))

function fieldValue(f: FieldDef): unknown { return props.definition.fields?.[f.key] }
function setField(f: FieldDef, v: unknown): void {
  if (!props.definition.fields) props.definition.fields = {}
  props.definition.fields[f.key] = v
}
function asString(f: FieldDef): string { const v = fieldValue(f); return v == null ? '' : String(v) }
function asNumber(f: FieldDef): number { const n = Number(fieldValue(f)); return Number.isFinite(n) ? n : 0 }
function enumOptions(f: FieldDef): { value: string; label: string }[] {
  return (f.options ?? []).map(o => ({ value: o, label: o }))
}
function refOptions(f: FieldDef): { value: string; label: string }[] {
  return refs.value
    .filter(r => !f.ref_kind || r.kind === f.ref_kind)
    .map(r => ({ value: r.id ?? '', label: r.id ? r.name + ' · ' + r.id : r.name }))
}
function refListText(f: FieldDef): string {
  const v = fieldValue(f)
  return Array.isArray(v) ? v.join('\n') : ''
}
function setRefList(f: FieldDef, t: string): void {
  setField(f, t.split(/[\n,，]/).map(s => s.trim()).filter(Boolean))
}
function boolValue(f: FieldDef): string {
  const v = fieldValue(f)
  return v === true ? 'true' : v === false ? 'false' : ''
}
function setBool(f: FieldDef, v: string): void { setField(f, v === '' ? undefined : v === 'true') }
function assetValue(f: FieldDef): AssetRef | null { return (fieldValue(f) as AssetRef | null | undefined) ?? null }
</script>

<template>
  <FieldGrid>
    <FieldText label="名称" :model-value="definition.name" @update:model-value="definition.name = $event" />
    <FieldText label="id" mono hint="引用此条目时使用" :model-value="definition.id" @update:model-value="definition.id = $event" />
    <template v-for="f in fields" :key="f.key">
      <FieldText
        v-if="f.type === 'text'"
        :label="f.label" :hint="f.hint" :placeholder="f.placeholder"
        :model-value="asString(f)" @update:model-value="setField(f, $event)"
      />
      <FieldArea
        v-else-if="f.type === 'textarea'"
        :label="f.label" :hint="f.hint" :placeholder="f.placeholder"
        :model-value="asString(f)" @update:model-value="setField(f, $event)"
      />
      <FieldNum
        v-else-if="f.type === 'number'"
        :label="f.label" :hint="f.hint"
        :model-value="asNumber(f)" @update:model-value="setField(f, $event)"
      />
      <FieldSelect
        v-else-if="f.type === 'enum'"
        :label="f.label" :hint="f.hint" :options="enumOptions(f)"
        :model-value="asString(f)" @update:model-value="setField(f, $event)"
      />
      <FieldSelect
        v-else-if="f.type === 'boolean'"
        :label="f.label" :hint="f.hint"
        :options="[{ value: 'true', label: '是' }, { value: 'false', label: '否' }]"
        :model-value="boolValue(f)" @update:model-value="setBool(f, $event)"
      />
      <FieldSelect
        v-else-if="f.type === 'ref'"
        :label="f.label" :hint="f.hint" :options="refOptions(f)"
        :model-value="asString(f)" @update:model-value="setField(f, $event)"
      />
      <FieldArea
        v-else-if="f.type === 'ref_list'"
        :label="f.label" :rows="3"
        :hint="(f.hint ? f.hint + ' · ' : '') + '每行一个 id'"
        :model-value="refListText(f)" @update:model-value="setRefList(f, $event)"
      />
      <ImageField
        v-else-if="f.type === 'asset'"
        :label="f.label" :hint="f.hint"
        :model-value="assetValue(f)" @update:model-value="setField(f, $event)"
      />
    </template>
  </FieldGrid>
</template>
