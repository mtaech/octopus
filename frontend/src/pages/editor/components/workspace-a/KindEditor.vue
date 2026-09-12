<script setup lang="ts">
// KindEditor —— 开放种类（KindDef）的字段 schema 编辑器。
// 这是「新概念 = 数据」的另一半：创作者在 UI 里声明种类与字段，无需改任何代码。
import { computed } from 'vue'
import type { FieldDef, FieldType, KindDef } from '@/types'
import { useEditorStore } from '../../stores/editor'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { IconPlus, IconTrash } from '@tabler/icons-vue'

const props = defineProps<{ kindDef: KindDef }>()
const editor = useEditorStore()

const FIELD_TYPES: { value: FieldType; label: string }[] = [
  { value: 'text', label: '单行文本' },
  { value: 'textarea', label: '多行文本' },
  { value: 'number', label: '数字' },
  { value: 'boolean', label: '是 / 否' },
  { value: 'enum', label: '枚举' },
  { value: 'ref', label: '实体引用' },
  { value: 'ref_list', label: '实体引用（多）' },
  { value: 'asset', label: '图片' },
]
const typeOptions = FIELD_TYPES.map(t => ({ value: t.value, label: t.label }))
/** 可挂接的宿主实体（目前只有人物；后续可扩到 scene / item） */
const ATTACH_OPTIONS = [{ value: 'character', label: '人物' }]
function toggleAttach(key: string): void {
  const cur = props.kindDef.applies_to ?? []
  props.kindDef.applies_to = cur.includes(key) ? cur.filter(k => k !== key) : [...cur, key]
}
const yesNo = [{ value: 'true', label: '是' }, { value: 'false', label: '否' }]
const kindOptions = computed(() =>
  (editor.draft?.kinds ?? []).map(k => ({ value: k.key, label: k.label + ' (' + k.key + ')' })),
)

function addField(): void {
  props.kindDef.fields.push({ key: 'field' + (props.kindDef.fields.length + 1), label: '新字段', type: 'text' })
}
function removeField(i: number): void { props.kindDef.fields.splice(i, 1) }
function setType(f: FieldDef, v: string): void { f.type = v as FieldType }
function setOptions(f: FieldDef, t: string): void {
  f.options = t.split(/[,，、|/]/).map(s => s.trim()).filter(Boolean)
}
function optionsText(f: FieldDef): string { return (f.options ?? []).join('、') }
function needsRef(f: FieldDef): boolean { return f.type === 'ref' || f.type === 'ref_list' }
function needsOptions(f: FieldDef): boolean { return f.type === 'enum' }
</script>

<template>
  <FieldGrid>
    <FieldText label="key" mono hint="英文标识（如 rumor），存储与引用用它" :model-value="kindDef.key" @update:model-value="kindDef.key = $event" />
    <FieldText label="显示名" :model-value="kindDef.label" @update:model-value="kindDef.label = $event" />
    <FieldText label="分组" hint="左栏分组标题，可空" :model-value="kindDef.group ?? ''" @update:model-value="kindDef.group = $event" />
    <FieldSelect label="单例" :options="yesNo" :allow-empty="false" :model-value="kindDef.singleton ? 'true' : 'false'" @update:model-value="kindDef.singleton = $event === 'true'" />
  </FieldGrid>

  <div class="mt-3 flex flex-col gap-1.5">
    <span class="text-[11px] leading-4 font-medium text-muted-foreground">可挂接到</span>
    <div class="flex flex-wrap gap-1.5">
      <button
        v-for="opt in ATTACH_OPTIONS"
        :key="opt.value"
        type="button"
        class="cursor-pointer rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
        :class="(kindDef.applies_to ?? []).includes(opt.value)
          ? 'border-primary/50 bg-primary/10 text-primary'
          : 'border-border text-muted-foreground hover:bg-muted/60'"
        @click="toggleAttach(opt.value)"
      >
        {{ opt.label }}
      </button>
    </div>
    <span class="text-[10.5px] leading-4 text-muted-foreground/65">声明后，该实体的编辑面板会出现「{{ kindDef.label }}」挂接区，可引用本种类的定义。</span>
    <span class="text-[10.5px] leading-4 text-warning/80">开放种类只装「规则书内容」（种族 / 职业 / 背景 / 特性 / 语言…）；属性维度、派生值、资源、装备各自有专用面板，别在这里重复定义。</span>
  </div>

  <div class="mt-5 flex items-center gap-2 border-t border-border pt-4">
    <h5 class="text-[12px] font-semibold text-foreground">字段 schema</h5>
    <span class="text-[11px] text-muted-foreground/70">{{ kindDef.fields.length }} 项 · 表单按此生成</span>
    <Button size="sm" class="ml-auto h-7 gap-1 px-2 text-xs" @click="addField">
      <IconPlus class="size-3.5" />
      加字段
    </Button>
  </div>

  <p v-if="!kindDef.fields.length" class="mt-3 text-[11.5px] leading-5 text-muted-foreground/70">
    这个种类还没有字段。点「加字段」声明它需要填什么。
  </p>

  <div v-for="(f, i) in kindDef.fields" :key="i" class="mt-2 rounded-lg border border-border/70 bg-card/30 p-3">
    <div class="mb-2 flex items-center gap-2">
      <span class="text-[11px] font-semibold text-muted-foreground">字段 {{ i + 1 }}</span>
      <Button
        variant="ghost" size="sm"
        class="ml-auto h-6 gap-1 px-2 text-[11px] text-muted-foreground/70 hover:bg-destructive/10 hover:text-destructive"
        @click="removeField(i)"
      >
        <IconTrash class="size-3" />
        移除
      </Button>
    </div>
    <FieldGrid dense>
      <FieldText dense label="key" mono :model-value="f.key" @update:model-value="f.key = $event" />
      <FieldText dense label="显示名" :model-value="f.label" @update:model-value="f.label = $event" />
      <FieldSelect dense label="类型" :options="typeOptions" :allow-empty="false" :model-value="f.type" @update:model-value="setType(f, $event)" />
      <FieldText
        v-if="needsOptions(f)" dense label="枚举值（、分隔）"
        :model-value="optionsText(f)" @update:model-value="setOptions(f, $event)"
      />
      <FieldSelect
        v-else-if="needsRef(f)" dense label="引用目标种类"
        :options="kindOptions" :allow-empty="true"
        :model-value="f.ref_kind ?? ''" @update:model-value="f.ref_kind = $event || undefined"
      />
      <FieldText dense label="提示" :model-value="f.hint ?? ''" @update:model-value="f.hint = $event" />
    </FieldGrid>
  </div>
</template>
