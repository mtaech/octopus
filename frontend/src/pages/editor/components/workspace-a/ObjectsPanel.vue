<script setup lang="ts">
// ObjectsPanel —— A「物件」tab：#01 修订 objects（场景内可交互物件）
// 名称 / 描述 / 所属地点 / 动作列表 / 可挂技能；interact 的 object_id + action 引用这里
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { ObjectDef } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { IconBox, IconPlus, IconX } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

const objects = computed<ObjectDef[]>(() => d.value?.objects ?? [])
const locOptions = computed(() => (d.value?.world.locations ?? []).map(l => ({ value: l.id, label: l.name })))
const skillOptions = computed(() => (d.value?.skills ?? []).map(s => ({ value: s.id, label: s.name })))

function addObject(): void {
  d.value?.objects.push({ id: uid('obj'), name: '新物件', description: '', actions: [], skills: [] })
}
function removeObject(o: ObjectDef): void {
  const arr = d.value?.objects
  if (!arr) return
  const i = arr.indexOf(o)
  if (i >= 0) arr.splice(i, 1)
}

function addAction(o: ObjectDef): void {
  if (!o.actions) o.actions = []
  o.actions.push({ key: 'interact', label: '交互' })
}
function removeAction(o: ObjectDef, i: number): void {
  o.actions?.splice(i, 1)
}

/** 追加一个尚未挂载的技能；技能都挂过时按钮置灰，不再补空选择 */
function addSkill(o: ObjectDef): void {
  if (!o.skills) o.skills = []
  const used = new Set(o.skills)
  const next = skillOptions.value.find(s => !used.has(s.value))
  if (next) o.skills.push(next.value)
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4 flex items-start justify-between gap-3">
      <div>
        <h3 class="font-serif text-lg text-foreground">物件</h3>
        <p class="mt-0.5 text-xs text-muted-foreground/70">场景内可交互物件：动作列表供 interact 的 action 引用，可挂技能与条件（#01 修订）。</p>
      </div>
      <Button size="sm" class="gap-1" @click="addObject">
        <IconPlus data-icon="inline-start" />
        物件
      </Button>
    </div>
    <div v-if="!objects.length" class="py-3 text-xs text-muted-foreground/70">还没有物件。</div>

    <div v-for="o in objects" :key="o.id" class="mb-3">
      <EntityCard :title="o.name" sub="物件" :icon="IconBox" @remove="removeObject(o)">
        <FieldText label="名称" :model-value="o.name" @update:model-value="o.name = $event" />
        <FieldText label="id" :model-value="o.id" mono hint="interact 的 object_id 引用此 id" @update:model-value="o.id = $event" />
        <FieldArea label="描述" :model-value="o.description ?? ''" md placeholder="看起来是什么样…" @update:model-value="o.description = $event" />
        <FieldSelect
          label="所属地点"
          :model-value="o.location_id ?? ''"
          placeholder="（不限定地点）"
          :options="locOptions"
          hint="删地点时自动清空此引用"
          @update:model-value="o.location_id = $event || undefined"
        />

        <div class="mt-3 mb-1.5 flex items-center justify-between">
          <span class="text-[11px] font-bold tracking-wider text-muted-foreground">可交互动作</span>
          <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" @click="addAction(o)">
            <IconPlus data-icon="inline-start" />
            动作
          </Button>
        </div>
        <div v-if="!o.actions?.length" class="py-1 text-xs text-muted-foreground/70">还没有动作 —— interact 至少需要一个动作 key。</div>
        <div v-for="(a, i) in o.actions ?? []" :key="i" class="mb-1.5 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-1.5">
          <Input class="h-7 w-full font-mono text-xs" :model-value="a.key" placeholder="动作 key，如 read" @update:model-value="a.key = String($event)" />
          <Input class="h-7 w-full text-[13px]" :model-value="a.label" placeholder="展示名，如 查看" @update:model-value="a.label = String($event)" />
          <Button variant="ghost" size="icon-xs" class="size-6 text-destructive hover:bg-destructive/10 hover:text-destructive" aria-label="移除动作" @click="removeAction(o, i)">
            <IconX />
          </Button>
        </div>

        <div class="mt-3 mb-1.5 flex items-center justify-between">
          <span class="text-[11px] font-bold tracking-wider text-muted-foreground">可挂技能</span>
          <Button
            v-if="skillOptions.length"
            variant="ghost"
            size="sm"
            class="h-7 gap-1 text-xs text-muted-foreground"
            :disabled="(o.skills?.length ?? 0) >= skillOptions.length"
            @click="addSkill(o)"
          >
            <IconPlus data-icon="inline-start" />
            技能
          </Button>
        </div>
        <div v-if="!skillOptions.length" class="py-1 text-xs text-muted-foreground/70">先在「技能」tab 建技能，再回来挂载。</div>
        <div v-for="(sid, i) in o.skills ?? []" :key="i" class="mb-1.5 flex items-center gap-1.5">
          <Select :model-value="sid" @update:model-value="o.skills![i] = String($event ?? '')">
            <SelectTrigger class="h-7 w-full flex-1 text-[13px]">
              <SelectValue :placeholder="sid ? undefined : '（选技能）'" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="s in skillOptions" :key="s.value" :value="s.value">{{ s.label }}</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <Button variant="ghost" size="icon-xs" class="size-6 text-destructive hover:bg-destructive/10 hover:text-destructive" aria-label="移除技能" @click="o.skills!.splice(i, 1)">
            <IconX />
          </Button>
        </div>
      </EntityCard>
    </div>
  </div>
</template>
