<script setup lang="ts">
// CharacterPanel —— A「人物」tab：#01 characters 模板 + #07 ④ 人物表单消费全局维度
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { AttributeDimension, CharacterDef } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { IconUser, IconPlus } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)

const dims = computed<AttributeDimension[]>(() => d.value?.attribute_dimensions ?? [])

function addCharacter(): void {
  d.value?.characters.push({
    id: uid('char'), name: '新人物', kind: 'npc', background: '', personality: '', attributes: {}
  })
}
function removeCharacter(c: CharacterDef): void {
  const arr = d.value?.characters
  if (!arr) return
  const i = arr.indexOf(c)
  if (i >= 0) arr.splice(i, 1)
}
function setAttr(c: CharacterDef, key: string, v: number | string): void {
  if (!c.attributes) c.attributes = {}
  c.attributes[key] = v
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4 flex items-start justify-between gap-3">
      <div>
        <h3 class="font-serif text-lg text-foreground">人物模板</h3>
        <p class="mt-0.5 text-xs text-muted-foreground/70">运行时实例化为角色实例；属性值来自全局属性维度（在「维度设置」tab 定义，人物表单只消费）。</p>
      </div>
      <Button size="sm" class="gap-1" @click="addCharacter">
        <IconPlus data-icon="inline-start" />
        人物
      </Button>
    </div>

    <div v-if="!d.characters.length" class="py-3 text-xs text-muted-foreground/70">还没有人物模板。</div>

    <div v-for="c in d.characters" :key="c.id" class="mb-3">
      <EntityCard :title="c.name" :sub="'人物 · ' + (c.kind === 'pc' ? '受控' : 'NPC')" :icon="IconUser" @remove="removeCharacter(c)">
        <FieldText label="名称" :model-value="c.name" @update:model-value="c.name = $event" />
        <FieldText label="id" :model-value="c.id" mono hint="场景在场人物 / 关系边引用此 id" @update:model-value="c.id = $event" />
        <FieldSelect label="种类" :model-value="c.kind ?? 'npc'" :options="[{ value: 'pc', label: '受控人物（PC）' }, { value: 'npc', label: 'NPC' }]" hint="开档默认选第一个 PC 为主角，游玩中可切换（#01/#25）" @update:model-value="c.kind = $event as 'pc' | 'npc'" />
        <FieldArea label="背景" :model-value="c.background" md placeholder="出身、经历、秘密…" @update:model-value="c.background = $event" />
        <FieldArea label="性格" :model-value="c.personality" md placeholder="行为倾向、说话方式…" @update:model-value="c.personality = $event" />
        <!-- 属性区：维度由全局定义，人物只消费（#07 ④） -->
        <div class="mt-3 mb-1.5 text-[11px] font-bold tracking-wider text-muted-foreground">属性（{{ dims.length }} 个全局维度）</div>
        <div v-if="!dims.length" class="text-xs text-muted-foreground/70">尚无全局维度 —— 到「维度设置」tab 新增，人物表单会自动出现对应输入项。</div>
        <template v-else>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-x-3 gap-y-2">
            <template v-for="dim in dims" :key="dim.key">
              <div v-if="dim.type === 'number'" class="flex flex-col gap-1">
                <label class="text-[11px] text-muted-foreground/80">{{ dim.label }}</label>
                <Input
                  class="h-7 w-full text-[13px]"
                  type="number"
                  :value="typeof c.attributes?.[dim.key] === 'number' ? c.attributes[dim.key] : ''"
                  :placeholder="'基线 ' + (dim.baseline ?? 50)"
                  @change="setAttr(c, dim.key, Number(($event.target as HTMLInputElement).value))"
                />
              </div>
              <div v-else-if="dim.type === 'enum'" class="flex flex-col gap-1">
                <label class="text-[11px] text-muted-foreground/80">{{ dim.label }}</label>
                <Select :model-value="String(c.attributes?.[dim.key] ?? '')" @update:model-value="setAttr(c, dim.key, String($event ?? ''))">
                  <SelectTrigger class="h-7 w-full text-[13px]">
                    <SelectValue :placeholder="String(c.attributes?.[dim.key] ?? '') === '' ? '（未选）' : undefined" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem v-for="o in dim.options ?? []" :key="o" :value="o">{{ o }}</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </div>
              <div v-else class="col-span-full flex flex-col gap-1">
                <label class="text-[11px] text-muted-foreground/80">{{ dim.label }}</label>
                <Input
                  class="h-7 w-full text-[13px]"
                  :value="String(c.attributes?.[dim.key] ?? '')"
                  placeholder="文本备注…"
                  @change="setAttr(c, dim.key, ($event.target as HTMLInputElement).value)"
                />
              </div>
            </template>
          </div>
          <p class="mt-2 text-[11px] leading-4 text-muted-foreground/70">数值型维度参与判定（0–100 基线 50，中心偏移公式见 #12）；枚举 / 文本仅作角色刻画。</p>
        </template>
      </EntityCard>
    </div>
  </div>
</template>
