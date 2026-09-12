<script setup lang="ts">
// ObjectsPanel —— A「物件」tab：#01 修订 objects（场景内可交互物件）
// 名称 / 描述 / 所属地点 / 动作列表 / 可挂技能；interact 的 object_id + action 引用这里
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { ObjectDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { IconBox, IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('objects')
function select(id: string): void { selected.value = id }

const list = computed<ObjectDef[]>(() => d.value?.objects ?? [])
const locOptions = computed(() => (d.value?.world.locations ?? []).map(l => ({ value: l.id, label: l.name })))
const skillOptions = computed(() => (d.value?.skills ?? []).map(s => ({ value: s.id, label: s.name })))

const locName = (id?: string): string => (id ? ((d.value?.world.locations ?? []).find(l => l.id === id)?.name ?? id) : '')

const items = computed<WorkbenchItem[]>(() => list.value.map(o => ({
  id: entityKey(o),
  title: o.name,
  sub: o.id,
  badge: locName(o.location_id) || '未定位',
})))

const current = computed(() => list.value.find(o => entityKey(o) === selected.value) ?? null)

function addObject(): void {
  const arr = d.value?.objects
  if (!arr) return
  const o: ObjectDef = { id: uid('obj'), name: '新物件', description: '', actions: [], skills: [] }
  arr.unshift(o)
  selected.value = entityKey(o)
}
function removeObject(o: ObjectDef): void {
  const arr = d.value?.objects
  if (!arr) return
  const i = arr.indexOf(o)
  if (i >= 0) arr.splice(i, 1)
}

function addAction(o: ObjectDef): void {
  if (!o.actions) o.actions = []
  o.actions.unshift({ key: 'interact', label: '交互' })
}
function removeAction(o: ObjectDef, i: number): void {
  o.actions?.splice(i, 1)
}

/** 追加一个尚未挂载的技能；技能都挂过时按钮置灰，不再补空选择 */
function addSkill(o: ObjectDef): void {
  if (!o.skills) o.skills = []
  const used = new Set(o.skills)
  const next = skillOptions.value.find(s => !used.has(s.value))
  if (next) o.skills.unshift(next.value)
}
</script>

<template>
  <WorkbenchLayout
    title="物件"
    hint="场景内可交互物件：动作列表供 interact 的 action 引用，可挂技能与条件（#01 修订）。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="物件"
    search-placeholder="搜索物件…"
    empty-hint="还没有物件。点右上「物件」新增。"
    @update:selected="select"
    @add="addObject"
  >
    <template v-if="current">
      <EntityFormHeader :title="current.name" sub="物件 · 场景内可交互" :icon="IconBox" :meta="current.id" @remove="removeObject(current)" />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="current.name" @update:model-value="current.name = $event" />
        <FieldText label="id" mono hint="interact 的 object_id 引用此 id" :model-value="current.id" @update:model-value="current.id = $event" />
        <FieldSelect
          label="所属地点"
          :model-value="current.location_id ?? ''"
          placeholder="（不限定地点）"
          :options="locOptions"
          hint="删地点时自动清空此引用"
          @update:model-value="current.location_id = $event || undefined"
        />
        <FieldArea label="描述" md placeholder="看起来是什么样…" :model-value="current.description ?? ''" @update:model-value="current.description = $event" />

        <!-- 可交互动作 -->
        <section class="col-span-full border-t border-border/60 pt-3.5">
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">可交互动作</span>
            <Button variant="ghost" size="sm" class="h-7 gap-1 text-xs text-muted-foreground" @click="addAction(current)">
              <IconPlus class="size-3.5" />
              动作
            </Button>
          </div>
          <p v-if="!current.actions?.length" class="text-xs text-muted-foreground/70">还没有动作 —— interact 至少需要一个动作 key。</p>
          <div v-for="(a, i) in current.actions ?? []" :key="i" class="mb-2 flex items-center gap-2">
            <Input class="h-8 w-40 flex-none font-mono text-xs" :model-value="a.key" placeholder="动作 key，如 read" @update:model-value="a.key = String($event)" />
            <Input class="h-8 w-full flex-1 text-[13px]" :model-value="a.label" placeholder="展示名，如 查看" @update:model-value="a.label = String($event)" />
            <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" aria-label="移除动作" @click="removeAction(current, i)">
              <IconX class="size-3.5" />
            </Button>
          </div>
        </section>

        <!-- 可挂技能 -->
        <section class="col-span-full border-t border-border/60 pt-3.5">
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">可挂技能</span>
            <Button
              v-if="skillOptions.length"
              variant="ghost"
              size="sm"
              class="h-7 gap-1 text-xs text-muted-foreground"
              :disabled="(current.skills?.length ?? 0) >= skillOptions.length"
              @click="addSkill(current)"
            >
              <IconPlus class="size-3.5" />
              技能
            </Button>
          </div>
          <p v-if="!skillOptions.length" class="text-xs text-muted-foreground/70">先在「技能」tab 建技能，再回来挂载。</p>
          <div v-for="(sid, i) in current.skills ?? []" :key="i" class="mb-2 flex items-center gap-2">
            <Select :model-value="sid" @update:model-value="current.skills![i] = String($event ?? '')">
              <SelectTrigger class="h-8 w-full flex-1 text-[13px]">
                <SelectValue :placeholder="sid ? undefined : '（选技能）'">
                  {{ (d?.skills ?? []).find(s => s.id === sid)?.name || sid }}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem v-for="s in skillOptions" :key="s.value" :value="s.value">{{ s.label }}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
            <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" aria-label="移除技能" @click="current.skills!.splice(i, 1)">
              <IconX class="size-3.5" />
            </Button>
          </div>
        </section>
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个物件开始编辑。</div>
  </WorkbenchLayout>
</template>
