<script setup lang="ts">
// StatusesPanel —— A「状态」tab（#12 ③）：故事书顶层持续状态声明
// 技能 effect.status 与 Lua host.apply_status 均按 id 引用此处的状态；删状态会级联清理技能引用。
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { ImmediateEffect, StatusDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import FieldArea from '../fields/FieldArea.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconFlame, IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('statuses')
function select(id: string): void { selected.value = id }

const statusesList = computed<StatusDef[]>(() => d.value?.statuses ?? [])

const UNIT_OPTIONS = [
  { value: 'turns', label: '回合 turns' },
  { value: 'scenes', label: '场景 scenes' },
]
const STACK_OPTIONS = [
  { value: 'replace', label: '替换 replace' },
  { value: 'add', label: '叠加 add' },
  { value: 'max', label: '取较大 max' },
]
const EFFECT_KIND_OPTIONS = [
  { value: 'damage', label: '伤害' },
  { value: 'heal', label: '治疗' },
  { value: 'modify_resource', label: '改资源' },
  { value: 'set_flag', label: '设标记' },
]

/** 被哪些技能引用（删除前提示） */
function usedBy(x: StatusDef): string[] {
  return (d.value?.skills ?? []).filter(sk => (sk.effect?.status ?? []).includes(x.id)).map(sk => sk.name)
}

const items = computed<WorkbenchItem[]>(() => statusesList.value.map(x => {
  const n = usedBy(x).length
  return {
    id: entityKey(x),
    title: x.name || x.id,
    sub: x.id,
    badge: n ? '被 ' + n + ' 技能引用' : '未引用',
    tone: n ? 'default' : 'warn',
  }
}))

const current = computed(() => statusesList.value.find(x => entityKey(x) === selected.value) ?? null)
const currentUsed = computed(() => (current.value ? usedBy(current.value) : []))

function addStatus(): void {
  const arr = d.value?.statuses
  if (!arr) return
  const x: StatusDef = { id: uid('st'), name: '新状态', description: '', duration: 1, unit: 'turns' }
  arr.unshift(x)
  selected.value = entityKey(x)
}
function removeStatus(x: StatusDef): void {
  const arr = d.value?.statuses
  if (!arr) return
  const i = arr.indexOf(x)
  if (i >= 0) arr.splice(i, 1)
  // 级联清理技能引用
  d.value?.skills.forEach(sk => {
    if (sk.effect?.status) sk.effect.status = sk.effect.status.filter(sid => sid !== x.id)
  })
}

function addEffect(s: StatusDef): void {
  if (!s.effect) s.effect = []
  s.effect.push({ kind: 'damage', amount: '1' })
}
function removeEffect(s: StatusDef, i: number): void { s.effect?.splice(i, 1) }
function setEffectKind(s: StatusDef, i: number, kind: string): void {
  const next: ImmediateEffect =
    kind === 'modify_resource' ? { kind: 'modify_resource', resource: 'hp', amount: '1' }
      : kind === 'set_flag' ? { kind: 'set_flag', flag: '' }
        : { kind: kind === 'heal' ? 'heal' : 'damage', amount: '1' }
  if (s.effect) s.effect[i] = next
}
function amountOf(e: ImmediateEffect): string { return e.kind === 'set_flag' ? '' : e.amount }
function setAmount(s: StatusDef, i: number, v: string): void {
  const e = s.effect?.[i]
  if (e && e.kind !== 'set_flag') e.amount = v
}
function resourceOf(e: ImmediateEffect): string { return e.kind === 'modify_resource' ? e.resource : '' }
function setResource(s: StatusDef, i: number, v: string): void {
  const e = s.effect?.[i]
  if (e && e.kind === 'modify_resource') e.resource = v
}
function flagOf(e: ImmediateEffect): string { return e.kind === 'set_flag' ? e.flag : '' }
function setFlag(s: StatusDef, i: number, v: string): void {
  const e = s.effect?.[i]
  if (e && e.kind === 'set_flag') e.flag = v
}
</script>

<template>
  <WorkbenchLayout
    title="状态"
    hint="全局声明持续状态（时长 / 单位 / 叠加策略 / 每 tick 效果）。技能与 Lua 按 id 引用；未声明的 id 会被校验挡下。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="状态"
    search-placeholder="搜索状态…"
    empty-hint="还没有持续状态。点右上「状态」新增第一个。"
    @update:selected="select"
    @add="addStatus"
  >
    <template v-if="current">
      <EntityFormHeader
        :title="current.name || current.id"
        sub="持续状态 · 全局声明"
        :icon="IconFlame"
        :meta="currentUsed.length ? '被 ' + currentUsed.length + ' 技能引用' : '未引用'"
        :tone="currentUsed.length ? 'default' : 'warn'"
        @remove="removeStatus(current)"
      />
      <FieldGrid class="mt-4">
        <FieldText label="id" mono hint="技能 effect.status 与 Lua apply_status 引用它" :model-value="current.id" @update:model-value="current.id = $event" />
        <FieldText label="名称" :model-value="current.name" @update:model-value="current.name = $event" />
        <FieldNum label="持续时长" :min="0" :model-value="current.duration" @update:model-value="current.duration = $event" />
        <FieldSelect label="单位" :model-value="current.unit" :options="UNIT_OPTIONS" @update:model-value="current.unit = $event as StatusDef['unit']" />
        <FieldSelect label="叠加策略" allow-empty placeholder="（默认替换）" hint="同名状态再次施加时的处理" :model-value="current.stack ?? ''" :options="STACK_OPTIONS" @update:model-value="current.stack = ($event || undefined) as StatusDef['stack']" />
        <FieldArea label="说明" :rows="2" :model-value="current.description ?? ''" @update:model-value="current.description = $event" />

        <section class="col-span-full border-t border-border/60 pt-3.5">
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">每 tick 效果</span>
            <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" @click="addEffect(current)">
              <IconPlus class="size-3.5" />
              效果
            </Button>
          </div>
          <p class="mb-2 text-[10.5px] leading-4 text-muted-foreground/65">状态持续期间，每经过一个 duration 单位结算一次。</p>
          <div v-for="(e, i) in current.effect ?? []" :key="i" class="mb-2 flex flex-wrap items-center gap-2">
            <FieldSelect label="效果" class="w-32" :model-value="e.kind" :options="EFFECT_KIND_OPTIONS" @update:model-value="setEffectKind(current, i, String($event))" />
            <Input v-if="e.kind === 'modify_resource'" class="h-8 w-40 font-mono text-xs" placeholder="资源 id" :model-value="resourceOf(e)" @update:model-value="setResource(current, i, String($event))" />
            <Input v-if="e.kind !== 'set_flag'" class="h-8 w-28 text-[13px]" placeholder="数量 / 骰式" :model-value="amountOf(e)" @update:model-value="setAmount(current, i, String($event))" />
            <Input v-if="e.kind === 'set_flag'" class="h-8 w-40 font-mono text-xs" placeholder="标记 key" :model-value="flagOf(e)" @update:model-value="setFlag(current, i, String($event))" />
            <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" title="删除效果" aria-label="删除效果" @click="removeEffect(current, i)">
              <IconX class="size-3.5" />
            </Button>
          </div>
          <p v-if="!(current.effect ?? []).length" class="rounded-lg border border-dashed border-border/70 py-5 text-center text-xs text-muted-foreground/60">
            没有每 tick 效果：该状态仅作为标记存在。
          </p>
        </section>

        <p v-if="currentUsed.length" class="col-span-full text-[10.5px] leading-4 text-muted-foreground/65">
          引用技能：{{ currentUsed.join('、') }}
        </p>
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个状态开始编辑。</div>
  </WorkbenchLayout>
</template>