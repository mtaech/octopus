<script setup lang="ts">
// CheckerEditor —— 判定器编辑（#12）：骰子表达式 / 比较模式 / 分档阈值 / 属性修正 / Lua 判定脚本。
import { computed } from 'vue'
import type { CheckerDef } from '@/types'
import type { LuaCompletionContext } from '@/lib/lua-context'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import LuaHookEditor from '@/components/LuaHookEditor.vue'

const props = defineProps<{ modelValue: CheckerDef | null; context?: LuaCompletionContext }>()
const emit = defineEmits<{ (e: 'update:modelValue', value: CheckerDef | null): void }>()

const checker = computed<CheckerDef>(() => props.modelValue ?? {})

function patch(partial: Partial<CheckerDef>): void {
  emit('update:modelValue', { ...checker.value, ...partial })
}

const thresholdsText = computed({
  get: () => (checker.value.degree_thresholds ?? []).join(', '),
  set: (value: string) => {
    const arr = value
      .split(/[,，\s]+/)
      .map(s => s.trim())
      .filter(Boolean)
      .map(Number)
      .filter(n => !Number.isNaN(n))
    patch({ degree_thresholds: arr.length ? arr : undefined })
  },
})

const attrModText = computed({
  get: () => Object.entries(checker.value.attribute_modifier ?? {}).map(([k, v]) => k + ':' + v).join(', '),
  set: (value: string) => {
    const map: Record<string, number> = {}
    value
      .split(/[,，\n]+/)
      .map(s => s.trim())
      .filter(Boolean)
      .forEach(pair => {
        const [key, raw] = pair.split(/[:=]/).map(s => s.trim())
        const num = Number(raw)
        if (key && !Number.isNaN(num)) map[key] = num
      })
    patch({ attribute_modifier: Object.keys(map).length ? map : undefined })
  },
})

const MODES = [
  { value: 'gte', label: '最终值 ≥ 难度 (gte)' },
  { value: 'lte', label: '最终值 ≤ 目标 (lte)' },
  { value: 'opposed', label: '对抗 (opposed)' },
]

const KINDS = [
  { value: 'attribute', label: '属性检定（本人掷骰）' },
  { value: 'attack', label: '攻击检定（本人 vs 目标防御）' },
  { value: 'save', label: '豁免（目标掷骰，失败才结算）' },
  { value: 'passive', label: '被动值（不掷骰）' },
]
</script>

<template>
  <div class="space-y-3">
    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
      <label class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">骰子表达式</span>
        <Input
          :model-value="checker.dice ?? ''"
          placeholder="1d20 / 2d6+3；留空 = 无骰，由 AI 叙事裁决"
          @update:model-value="patch({ dice: String($event) || undefined })"
        />
      </label>
      <label class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">判定种类</span>
        <Select :model-value="checker.kind ?? 'attribute'" @update:model-value="patch({ kind: $event as CheckerDef['kind'] })">
          <SelectTrigger class="h-8 w-full text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem v-for="k in KINDS" :key="k.value" :value="k.value">{{ k.label }}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </label>
      <label v-if="(checker.kind ?? 'attribute') === 'passive'" class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">被动基数</span>
        <Input
          type="number"
          :model-value="checker.passive_base ?? 10"
          placeholder="10"
          @update:model-value="patch({ passive_base: Number($event) })"
        />
      </label>
      <label class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">比较模式</span>
        <Select :model-value="checker.mode ?? 'gte'" @update:model-value="patch({ mode: $event as CheckerDef['mode'] })">
          <SelectTrigger class="h-8 w-full text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem v-for="m in MODES" :key="m.value" :value="m.value">{{ m.label }}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </label>
      <label class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">成功度阈值（按差值）</span>
        <Input
          :model-value="thresholdsText"
          placeholder="10, 0, -10（默认四档）"
          @update:model-value="thresholdsText = String($event)"
        />
      </label>
      <label class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">属性修正映射</span>
        <Input
          :model-value="attrModText"
          placeholder="str:2, agi:-1（留空用默认 (v-50)/5）"
          @update:model-value="attrModText = String($event)"
        />
      </label>
    </div>

    <div class="space-y-1.5 pt-1">
      <LuaHookEditor
        :model-value="checker.lua ?? ''"
        mode="check"
        title="Lua 判定算法脚本（计算 total 与 margin）"
        :context="context"
        @update:model-value="patch({ lua: $event || undefined })"
      />
    </div>
  </div>
</template>
