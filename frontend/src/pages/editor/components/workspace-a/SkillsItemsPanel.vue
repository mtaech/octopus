<script setup lang="ts">
// SkillsItemsPanel —— A「技能」/「物品」共用 tab（props.kind）
// #01：技能 Skill 单一实体；物品可引用技能成为「物品技能」
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { EffectDef, ItemDef, SkillDef } from '@/types'
import { uid } from '@/types'
import EntityCard from '../fields/EntityCard.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { IconSparkles, IconPackage, IconPlus, IconX } from '@tabler/icons-vue'

const props = defineProps<{ kind: 'skill' | 'item' }>()
const editor = useEditorStore()
const d = computed(() => editor.draft)

const list = computed<(SkillDef | ItemDef)[]>(() => props.kind === 'skill' ? (d.value?.skills ?? []) : (d.value?.items ?? []))
const skillOptions = computed(() => (d.value?.skills ?? []).map(s => ({ value: s.id, label: s.name })))
const resOptions = computed(() => (d.value?.world.resources ?? []).map(r => ({ value: r.id, label: r.name })))
/** 目标类型候选：来自声明区 target_types（#10 决议） */
const targetOptions = computed(() => (d.value?.target_types ?? []).map(t => ({ value: t.key, label: t.label ?? t.key })))

function add(): void {
  if (props.kind === 'skill') {
    d.value?.skills.push({ id: uid('sk'), name: '新技能', description: '', cost: [], category: '', check: undefined, effect: {} })
  } else {
    d.value?.items.push({ id: uid('it'), name: '新物品', description: '', type: '', quantity: 1, skills: [] })
  }
}
function remove(e: SkillDef | ItemDef): void {
  const arr = list.value
  const i = arr.indexOf(e)
  if (i >= 0) arr.splice(i, 1)
}

function setCost(s: SkillDef, res: string, amount: number): void {
  if (!res) { s.cost = []; return }
  s.cost = [{ resource: res, amount: amount ?? 0 }]
}
function costResOf(s: SkillDef): string { return s.cost?.[0]?.resource ?? '' }
function costAmtOf(s: SkillDef): number { return s.cost?.[0]?.amount ?? 0 }

/** EffectDef 即时效果 → 一行文本（v1 简化，仅作展示/改写入口） */
function effectText(s: SkillDef): string {
  const fx: EffectDef | undefined = s.effect
  const imm = fx?.immediate ?? []
  if (!imm.length) return ''
  return imm.map(i => {
    if (i.kind === 'damage') return '伤害 ' + i.amount
    if (i.kind === 'heal') return '治疗 ' + i.amount
    if (i.kind === 'modify_resource') return '改资源 ' + i.resource + ' ' + i.amount
    return '设标记 ' + i.flag
  }).join('；')
}
/** 文本 → EffectDef（极简解析；无法解析则空效果不报错） */
function parseEffect(t: string): EffectDef {
  const text = t.trim()
  if (!text) return {}
  const mRes = text.match(/改资源\s+(\S+)\s+([+-]?\d+)/)
  if (mRes) return { immediate: [{ kind: 'modify_resource', resource: mRes[1], amount: mRes[2] }] }
  return {}
}
</script>

<template>
  <div v-if="d" class="mx-auto max-w-3xl px-5 py-4">
    <div class="mb-4 flex items-start justify-between gap-3">
      <div>
        <h3 class="font-serif text-lg text-foreground">{{ props.kind === 'skill' ? '技能' : '物品' }}</h3>
        <p v-if="props.kind === 'skill'" class="mt-0.5 text-xs text-muted-foreground/70">声明式能力定义：消耗 / 目标 / 判定 / 效果（#01）。物品技能 = 物品引用同一技能实体。</p>
        <p v-else class="mt-0.5 text-xs text-muted-foreground/70">可携带物件；可引用技能成为「物品技能」，载体是物品。</p>
      </div>
      <Button size="sm" class="gap-1" @click="add">
        <IconPlus data-icon="inline-start" />
        {{ props.kind === 'skill' ? '技能' : '物品' }}
      </Button>
    </div>
    <div v-if="!list.length" class="py-3 text-xs text-muted-foreground/70">还没有{{ props.kind === 'skill' ? '技能' : '物品' }}。</div>

    <div v-for="e in list" :key="e.id" class="mb-3">
      <EntityCard v-if="props.kind === 'skill'" :title="(e as SkillDef).name" sub="技能" :icon="IconSparkles" @remove="remove(e)">
        <FieldText label="名称" :model-value="(e as SkillDef).name" @update:model-value="(e as SkillDef).name = $event" />
        <FieldText label="id" :model-value="e.id" mono @update:model-value="e.id = $event" />
        <FieldText label="分类" :model-value="(e as SkillDef).category ?? ''" placeholder="法术 / 技艺 / …" @update:model-value="(e as SkillDef).category = $event" />
        <FieldArea label="描述" :model-value="(e as SkillDef).description" md @update:model-value="(e as SkillDef).description = $event" />
        <FieldSelect v-if="targetOptions.length" label="目标" allow-empty placeholder="（未指定）" :model-value="(e as SkillDef).target ?? ''" :options="targetOptions" hint="来自「声明」tab 的目标类型" @update:model-value="(e as SkillDef).target = $event" />
        <FieldText v-else label="目标" :model-value="(e as SkillDef).target ?? ''" placeholder="单体 / 自身 / 友方…（可在「声明」tab 定义候选）" @update:model-value="(e as SkillDef).target = $event" />
        <div class="grid grid-cols-2 gap-x-4">
          <FieldSelect label="消耗资源" :model-value="costResOf(e as SkillDef)" allow-empty placeholder="（无消耗）" :options="resOptions" @update:model-value="setCost(e as SkillDef, $event, costAmtOf(e as SkillDef))" />
          <FieldNum label="消耗量" :model-value="costAmtOf(e as SkillDef)" :min="0" @update:model-value="setCost(e as SkillDef, costResOf(e as SkillDef), $event)" />
        </div>
        <FieldArea label="即时效果" :model-value="effectText(e as SkillDef)" :rows="2" hint="声明式即时效果：改资源 res-gold +15 / 伤害 N / 治疗 N / 设标记 flag" @update:model-value="(e as SkillDef).effect = parseEffect($event)" />
      </EntityCard>

      <EntityCard v-else :title="(e as ItemDef).name" sub="物品" :icon="IconPackage" @remove="remove(e)">
        <FieldText label="名称" :model-value="(e as ItemDef).name" @update:model-value="(e as ItemDef).name = $event" />
        <FieldText label="id" :model-value="e.id" mono @update:model-value="e.id = $event" />
        <FieldText label="类型" :model-value="(e as ItemDef).type ?? ''" placeholder="消耗品 / 装备 / …" @update:model-value="(e as ItemDef).type = $event" />
        <FieldNum label="数量" :model-value="(e as ItemDef).quantity ?? 1" :min="0" @update:model-value="(e as ItemDef).quantity = $event" />
        <FieldArea label="描述" :model-value="(e as ItemDef).description" md @update:model-value="(e as ItemDef).description = $event" />
        <div class="mt-3 mb-1 text-[11px] text-muted-foreground/70">物品技能（引用技能实体）</div>
        <div v-for="(sid, i) in (e as ItemDef).skills ?? []" :key="i" class="mb-1.5 flex items-center gap-1.5">
          <Select :model-value="sid" @update:model-value="(e as ItemDef).skills![i] = String($event ?? '')">
            <SelectTrigger class="h-7 w-full flex-1 text-[13px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="o in skillOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <Button variant="ghost" size="icon-xs" class="size-6 text-destructive hover:bg-destructive/10 hover:text-destructive" aria-label="移除技能" @click="(e as ItemDef).skills!.splice(i, 1)">
            <IconX />
          </Button>
        </div>
        <Button v-if="skillOptions.length && !(e as ItemDef).skills?.length" variant="ghost" size="sm" class="h-7 gap-1 text-xs text-muted-foreground" @click="(e as ItemDef).skills = [skillOptions[0].value]">
          <IconPlus data-icon="inline-start" />
          绑定技能
        </Button>
      </EntityCard>
    </div>
  </div>
</template>
