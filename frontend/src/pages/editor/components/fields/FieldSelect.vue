<script setup lang="ts">
// FieldSelect —— 带标签的下拉选择（shadcn Select）
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

export interface SelectOption { value: string; label: string }

const props = withDefaults(defineProps<{
  label: string
  modelValue?: string
  options?: SelectOption[]
  placeholder?: string
  hint?: string
  allowEmpty?: boolean
}>(), { modelValue: '', options: () => [], placeholder: '（未选择）', hint: '', allowEmpty: true })

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

/** 空值哨兵：reka Select 不接受 value=''，用哨兵占位、对外仍发空串 */
const EMPTY = '__oct_select_empty__'

function onPick(v: unknown): void {
  emit('update:modelValue', v === EMPTY ? '' : String(v ?? ''))
}
</script>

<template>
  <div class="grid grid-cols-[124px_minmax(0,1fr)] items-start gap-x-4 px-0.5 py-1.5">
    <label class="pt-2 text-xs leading-4 text-muted-foreground">{{ label }}</label>
    <div class="flex min-w-0 flex-col gap-1">
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
      <p v-if="hint" class="text-[11px] leading-4 text-muted-foreground/70">{{ hint }}</p>
    </div>
  </div>
</template>
