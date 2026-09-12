<script setup lang="ts">
// FieldSelect —— 标签在上的下拉选择
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

export interface SelectOption { value: string; label: string }

withDefaults(defineProps<{
  label: string
  modelValue?: string
  options?: SelectOption[]
  placeholder?: string
  hint?: string
  allowEmpty?: boolean
  span?: 'default' | 'full'
  /** 标签与控件间距收紧一档 */
  dense?: boolean
}>(), { modelValue: '', options: () => [], placeholder: '（未选择）', hint: '', allowEmpty: true, span: 'default', dense: false })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

/** 空值哨兵：reka Select 不接受 value=''，用哨兵占位、对外仍发空串 */
const EMPTY = '__oct_select_empty__'

function onPick(v: unknown): void {
  emit('update:modelValue', v === EMPTY ? '' : String(v ?? ''))
}
</script>

<template>
  <div class="flex min-w-0 flex-col" :class="[dense ? 'gap-1' : 'gap-1.5', span === 'full' && 'col-span-full']">
    <span class="text-[11px] leading-4 font-medium text-muted-foreground">{{ label }}</span>
    <Select :model-value="modelValue" @update:model-value="onPick">
      <SelectTrigger class="h-8 w-full text-[13px]">
        <SelectValue :placeholder="placeholder" />
      </SelectTrigger>
      <SelectContent>
        <SelectGroup>
          <SelectItem v-if="allowEmpty && placeholder" :value="EMPTY" class="text-muted-foreground">
            {{ placeholder }}
          </SelectItem>
          <SelectItem v-for="o in options" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
        </SelectGroup>
      </SelectContent>
    </Select>
    <span v-if="hint" class="text-[10.5px] leading-4 text-muted-foreground/65">{{ hint }}</span>
  </div>
</template>
