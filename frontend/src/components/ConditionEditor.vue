<script setup lang="ts">
// ConditionEditor —— 条件表达式树编辑（#13）：结构化 op（标记 / 触发点 / 地点 / 属性 / 关系 / 与或非）+ Lua 兜底。
// 递归组件：all_of / any_of 的子条件与 not 的子条件复用自身。
import { computed } from 'vue'
import type { CondExpr } from '@/types'
import { useEditorStore } from '@/pages/editor/stores/editor'
import { buildLuaContext } from '@/lib/lua-context'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import LuaHookEditor from '@/components/LuaHookEditor.vue'
import { IconPlus, IconTrash } from '@tabler/icons-vue'

const props = defineProps<{ modelValue: CondExpr | null }>()
const emit = defineEmits<{ (e: 'update:modelValue', value: CondExpr | null): void }>()

const editor = useEditorStore()
const d = computed(() => editor.draft)
const luaContext = computed(() => buildLuaContext(d.value))
// 判别联合，叶子字段按 op 区分；这里用 any 简化模板取值
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const cur = computed<any>(() => props.modelValue ?? {})
const op = computed(() => props.modelValue?.op ?? 'none')

const OPS = [
  { value: 'none', label: '无条件' },
  { value: 'all_of', label: '全部满足 (all_of)' },
  { value: 'any_of', label: '任一满足 (any_of)' },
  { value: 'not', label: '取反 (not)' },
  { value: 'flag_set', label: '标记已设 (flag_set)' },
  { value: 'trigger_fired', label: '触发点已触发 (trigger_fired)' },
  { value: 'at_location', label: '抵达地点 (at_location)' },
  { value: 'attribute_ge', label: '属性 ≥ (attribute_ge)' },
  { value: 'relationship_ge', label: '关系 ≥ (relationship_ge)' },
  { value: 'lua', label: 'Lua 脚本 (lua)' },
]

const flagOptions = computed(() => (d.value?.flags ?? []).map(x => ({ value: x.key, label: x.label ?? x.key })))
const triggerOptions = computed(() => {
  const out: { value: string; label: string }[] = []
  d.value?.skeleton.forEach(ch =>
    ch.scenes.forEach(sc =>
      sc.triggers.forEach(t => out.push({ value: t.id, label: t.title || t.id })),
    ),
  )
  return out
})
const locationOptions = computed(() => (d.value?.world.locations ?? []).map(l => ({ value: l.id, label: l.name })))
const dimOptions = computed(() =>
  (d.value?.attribute_dimensions ?? []).filter(x => x.type === 'number').map(x => ({ value: x.key, label: x.label })),
)
const actorOptions = computed(() => [
  ...(d.value?.characters ?? []).map(c => ({ value: c.id, label: c.name + '（人物）' })),
  ...(d.value?.factions ?? []).map(f => ({ value: f.id, label: f.name + '（势力）' })),
])
const relTypeOptions = computed(() =>
  (d.value?.relationship_types ?? []).map(x => ({ value: x.key, label: x.label ?? x.key })),
)

function patch(partial: Record<string, unknown>): void {
  emit('update:modelValue', { ...(props.modelValue as object), ...partial } as CondExpr)
}

function setOp(next: string): void {
  if (next === 'none') {
    emit('update:modelValue', null)
    return
  }
  const first = <T,>(arr: T[], pick: (x: T) => string, fallback: string) => (arr.length ? pick(arr[0]) : fallback)
  const nextCond: CondExpr = (() => {
    switch (next) {
      case 'all_of': return { op: 'all_of', children: [] }
      case 'any_of': return { op: 'any_of', children: [] }
      case 'not': return { op: 'not', child: { op: 'flag_set', flag: first(flagOptions.value, x => x.value, '') } }
      case 'flag_set': return { op: 'flag_set', flag: first(flagOptions.value, x => x.value, '') }
      case 'trigger_fired': return { op: 'trigger_fired', trigger_id: first(triggerOptions.value, x => x.value, '') }
      case 'at_location': return { op: 'at_location', location_id: first(locationOptions.value, x => x.value, '') }
      case 'attribute_ge': return { op: 'attribute_ge', attribute: first(dimOptions.value, x => x.value, ''), value: 50 }
      case 'relationship_ge': {
        const from = first(actorOptions.value, x => x.value, '')
        return { op: 'relationship_ge', from, to: from, type: first(relTypeOptions.value, x => x.value, ''), value: 0 }
      }
      case 'lua': return { op: 'lua', script: '' }
      default: return { op: 'flag_set', flag: '' }
    }
  })()
  emit('update:modelValue', nextCond)
}

function children(): CondExpr[] {
  const value = props.modelValue
  return value && (value.op === 'all_of' || value.op === 'any_of') ? value.children : []
}
function addChild(): void {
  patch({ children: [...children(), { op: 'flag_set', flag: flagOptions.value[0]?.value ?? '' } as CondExpr] })
}
function setChild(index: number, value: CondExpr | null): void {
  const list = [...children()]
  if (value) list[index] = value
  else list.splice(index, 1)
  patch({ children: list })
}
function setNotChild(value: CondExpr | null): void {
  patch({ child: value ?? { op: 'flag_set', flag: '' } })
}
</script>

<template>
  <div class="rounded-lg border border-border/70 bg-muted/15 p-2.5">
    <div class="flex items-center gap-2">
      <Select :model-value="op" @update:model-value="setOp(String($event))">
        <SelectTrigger class="h-7 w-52 text-xs"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in OPS" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
      <Button
        v-if="op !== 'none'"
        variant="ghost"
        size="icon-sm"
        class="size-7 text-muted-foreground/50 hover:bg-destructive/10 hover:text-destructive"
        title="清除条件"
        @click="emit('update:modelValue', null)"
      >
        <IconTrash class="size-3.5" />
      </Button>
    </div>

    <!-- 叶子 op -->
    <div v-if="op === 'flag_set'" class="mt-2">
      <Select :model-value="cur.flag" @update:model-value="patch({ flag: String($event) })">
        <SelectTrigger class="h-8 w-full text-xs"><SelectValue placeholder="选择标记" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in flagOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
    </div>

    <div v-else-if="op === 'trigger_fired'" class="mt-2">
      <Select :model-value="cur.trigger_id" @update:model-value="patch({ trigger_id: String($event) })">
        <SelectTrigger class="h-8 w-full text-xs"><SelectValue placeholder="选择触发点" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in triggerOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
    </div>

    <div v-else-if="op === 'at_location'" class="mt-2">
      <Select :model-value="cur.location_id" @update:model-value="patch({ location_id: String($event) })">
        <SelectTrigger class="h-8 w-full text-xs"><SelectValue placeholder="选择地点" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in locationOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
    </div>

    <div v-else-if="op === 'attribute_ge'" class="mt-2 flex items-center gap-2">
      <Select :model-value="cur.attribute" @update:model-value="patch({ attribute: String($event) })">
        <SelectTrigger class="h-8 flex-1 text-xs"><SelectValue placeholder="属性维度" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in dimOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
      <span class="text-xs text-muted-foreground">≥</span>
      <Input
        type="number"
        class="h-8 w-24 text-right font-mono text-xs"
        :model-value="cur.value ?? 0"
        @update:model-value="patch({ value: Number($event) })"
      />
    </div>

    <div v-else-if="op === 'relationship_ge'" class="mt-2 grid grid-cols-2 gap-2">
      <Select :model-value="cur.from" @update:model-value="patch({ from: String($event) })">
        <SelectTrigger class="h-8 text-xs"><SelectValue placeholder="主体" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in actorOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
      <Select :model-value="cur.to" @update:model-value="patch({ to: String($event) })">
        <SelectTrigger class="h-8 text-xs"><SelectValue placeholder="客体" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in actorOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
      <Select :model-value="cur.type" @update:model-value="patch({ type: String($event) })">
        <SelectTrigger class="h-8 text-xs"><SelectValue placeholder="关系类型" /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem v-for="o in relTypeOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
      <Input
        type="number"
        class="h-8 text-right font-mono text-xs"
        :model-value="cur.value ?? 0"
        @update:model-value="patch({ value: Number($event) })"
      />
    </div>

    <div v-else-if="op === 'lua'" class="mt-2 space-y-1.5">
      <LuaHookEditor
        :model-value="cur.script ?? ''"
        mode="condition"
        title="Lua 自定义条件（返回 true / false）"
        :storybook="d"
        @update:model-value="patch({ script: $event })"
      />
    </div>

    <!-- 复合 op -->
    <div v-else-if="op === 'all_of' || op === 'any_of'" class="mt-2 space-y-2">
      <ConditionEditor
        v-for="(child, i) in children()"
        :key="i"
        :model-value="child"
        @update:model-value="setChild(i, $event)"
      />
      <Button variant="ghost" size="sm" class="h-7 gap-1 text-xs text-primary hover:bg-primary/10" @click="addChild">
        <IconPlus class="size-3.5" />
        子条件
      </Button>
    </div>

    <div v-else-if="op === 'not'" class="mt-2">
      <ConditionEditor :model-value="cur.child ?? null" @update:model-value="setNotChild" />
    </div>
  </div>
</template>
