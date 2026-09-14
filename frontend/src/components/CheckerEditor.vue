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
  { value: 'opposed', label: '对抗 (opposed) · 双方各掷一次' },
]

/** 当前比较模式（缺省 gte）；mode === 'opposed' 时才放「对手判定属性」 */
const mode = computed(() => checker.value.mode ?? 'gte')

/** reka Select 不接受空串：用哨兵表示「未声明 / 同主动属性」，对外仍写 undefined */
const NO_ATTR = '__checker_no_attribute__'
const SAME_AS_ACTOR = '__checker_same_as_actor__'

/** 属性维度可选项来自故事书（经 context 传入）；没有维度声明时回落为自由输入 */
const attrOptions = computed(() =>
  (props.context?.attributes ?? []).map(a => ({ value: a.id, label: a.label ? a.label + ' · ' + a.id : a.id })),
)

function setAttribute(v: unknown): void {
  patch({ attribute: v === NO_ATTR ? undefined : String(v) })
}
function setOpposedAttribute(v: unknown): void {
  patch({ opposed_attribute: v === SAME_AS_ACTOR ? undefined : String(v) })
}

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
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">判定属性</span>
        <Select
          v-if="attrOptions.length"
          :model-value="checker.attribute ?? NO_ATTR"
          @update:model-value="setAttribute"
        >
          <SelectTrigger class="h-8 w-full text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem :value="NO_ATTR" class="text-muted-foreground">（缺省 · 回落 str）</SelectItem>
              <SelectItem v-for="a in attrOptions" :key="a.value" :value="a.value">{{ a.label }}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
        <Input
          v-else
          :model-value="checker.attribute ?? ''"
          placeholder="属性维度 key（如 str）"
          @update:model-value="setAttribute"
        />
        <span class="text-[10.5px] leading-4 text-muted-foreground/65">技能未声明时用它掷骰；必须命中属性维度 key，否则校验报错</span>
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
      <label v-if="mode === 'opposed'" class="space-y-1">
        <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">对手判定属性</span>
        <template v-if="attrOptions.length">
          <Select
            :model-value="checker.opposed_attribute ?? SAME_AS_ACTOR"
            @update:model-value="setOpposedAttribute"
          >
            <SelectTrigger class="h-8 w-full text-xs"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem :value="SAME_AS_ACTOR" class="text-muted-foreground">（同主动属性 · 缺省）</SelectItem>
                <SelectItem v-for="a in attrOptions" :key="a.value" :value="a.value">{{ a.label }}</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </template>
        <Input
          v-else
          :model-value="checker.opposed_attribute ?? ''"
          placeholder="留空 = 同主动属性"
          @update:model-value="setOpposedAttribute"
        />
        <span class="text-[10.5px] leading-4 text-muted-foreground/65">对手用它掷骰；留空与主动方同属性</span>
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

    <div
      v-if="mode === 'opposed'"
      class="rounded-lg border border-dashed border-primary/40 bg-primary/5 px-3 py-2 text-[11.5px] leading-5 text-muted-foreground"
    >
      <span class="font-semibold text-foreground">对抗 = 双方各掷一次比大小：</span>
      只有在意图给出对手（<span class="font-mono">opponent_id</span>）时才走对抗，目标是<b>对手的掷骰总值</b>；
      没给对手会被引擎明确驳回，<b>不再静默降级</b>成「对固定难度」。所以上面选了「对抗」还必须在玩法侧指到对手。
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
