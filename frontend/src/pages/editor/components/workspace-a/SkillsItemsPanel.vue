<script setup lang="ts">
// SkillsItemsPanel —— A「技能」/「物品」共用 tab（props.kind）
// #01：技能 Skill 单一实体；物品可引用技能成为「物品技能」
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { EffectDef, ItemDef, SkillDef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import EntityFormHeader from './EntityFormHeader.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import ImageField from '../fields/ImageField.vue'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import CheckerEditor from '@/components/CheckerEditor.vue'
import LuaHookEditor from '@/components/LuaHookEditor.vue'
import { Input } from '@/components/ui/input'
import { buildLuaContext } from '@/lib/lua-context'
import { IconSparkles, IconPackage, IconPlus, IconX } from '@tabler/icons-vue'

const props = defineProps<{ kind: 'skill' | 'item' }>()
const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection(props.kind)
function select(id: string): void { selected.value = id }

const isSkill = computed(() => props.kind === 'skill')
const skills = computed<SkillDef[]>(() => d.value?.skills ?? [])
const itemsArr = computed<ItemDef[]>(() => d.value?.items ?? [])
const list = computed<(SkillDef | ItemDef)[]>(() => (isSkill.value ? skills.value : itemsArr.value))

const skillOptions = computed(() => skills.value.map(s => ({ value: s.id, label: s.name })))
const resOptions = computed(() => (d.value?.world.resources ?? []).map(r => ({ value: r.id, label: r.name })))
/** 目标类型候选：来自声明区 target_types（#10 决议） */
const targetOptions = computed(() => (d.value?.target_types ?? []).map(t => ({ value: t.key, label: t.label ?? t.key })))

const items = computed<WorkbenchItem[]>(() => list.value.map(e => ({
  id: entityKey(e),
  title: e.name,
  sub: e.id,
  badge: isSkill.value ? ((e as SkillDef).category || undefined) : (((e as ItemDef).skills?.length ?? 0) ? (e as ItemDef).skills!.length + ' 技能' : undefined),
})))

const currentSkill = computed(() => (isSkill.value ? skills.value.find(s => entityKey(s) === selected.value) ?? null : null))
const currentItem = computed(() => (!isSkill.value ? itemsArr.value.find(i => entityKey(i) === selected.value) ?? null : null))

function add(): void {
  if (isSkill.value) {
    const arr = d.value?.skills
    if (!arr) return
    const s: SkillDef = { id: uid('sk'), name: '新技能', description: '', cost: [], category: '', check: undefined, effect: {} }
    arr.unshift(s)
    selected.value = entityKey(s)
  } else {
    const arr = d.value?.items
    if (!arr) return
    const it: ItemDef = { id: uid('it'), name: '新物品', description: '', type: '', quantity: 1, skills: [] }
    arr.unshift(it)
    selected.value = entityKey(it)
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

function addItemSkill(it: ItemDef): void {
  if (!it.skills) it.skills = []
  const used = new Set(it.skills)
  const next = skillOptions.value.find(s => !used.has(s.value))
  if (next) it.skills.unshift(next.value)
}

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

/** 写入即时效果时保留 effect 里的其他字段（尤其 status 引用）。 */
function setEffectText(s: SkillDef, text: string): void {
  const parsed = parseEffect(text)
  const next: EffectDef = { ...(s.effect ?? {}) }
  if (parsed.immediate && parsed.immediate.length) next.immediate = parsed.immediate
  else delete next.immediate
  s.effect = next
}

// ---------------- 持续状态引用 ----------------
const statusOptions = computed(() => (d.value?.statuses ?? []).map(s => ({ id: s.id, name: s.name })))
function statusRefsOf(s: SkillDef): string[] { return s.effect?.status ?? [] }
function toggleStatus(s: SkillDef, id: string): void {
  const cur = new Set(statusRefsOf(s))
  if (cur.has(id)) cur.delete(id)
  else cur.add(id)
  s.effect = { ...(s.effect ?? {}), status: Array.from(cur) }
}
function statusName(id: string): string { return statusOptions.value.find(x => x.id === id)?.name ?? id }

// ---------------- Lua 判定器 ----------------
const luaContext = computed(() => buildLuaContext(d.value))

/** 判定器来源：无 / 引用全局（字符串）/ 内联自定义（CheckerDef，#12） */
const checkMode = computed<'none' | 'ref' | 'inline'>(() => {
  const c = currentSkill.value?.check
  if (!c) return 'none'
  return typeof c === 'string' ? 'ref' : 'inline'
})
function setCheckMode(mode: string): void {
  const s = currentSkill.value
  if (!s) return
  if (mode === 'none') s.check = undefined
  else if (mode === 'ref') s.check = 'world'
  else s.check = {}
}
</script>

<template>
  <WorkbenchLayout
    :title="isSkill ? '技能' : '物品'"
    :hint="isSkill
      ? '声明式能力定义：消耗 / 目标 / 判定 / 效果（#01）。物品可引用同一技能实体。'
      : '可携带物件；可引用技能成为「物品技能」，载体是物品。'"
    :count="items.length"
    :items="items"
    :selected="selected"
    :add-label="isSkill ? '技能' : '物品'"
    :search-placeholder="isSkill ? '搜索技能…' : '搜索物品…'"
    :empty-hint="isSkill ? '还没有技能。点右上「技能」新增。' : '还没有物品。点右上「物品」新增。'"
    @update:selected="select"
    @add="add"
  >
    <!-- 技能表单 -->
    <template v-if="currentSkill">
      <EntityFormHeader :title="currentSkill.name" sub="技能 · 声明式能力" :icon="IconSparkles" :meta="currentSkill.id" @remove="remove(currentSkill)" />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="currentSkill.name" @update:model-value="currentSkill.name = $event" />
        <FieldText label="id" mono :model-value="currentSkill.id" @update:model-value="currentSkill.id = $event" />
        <FieldText label="分类" placeholder="法术 / 技艺 / …" :model-value="currentSkill.category ?? ''" @update:model-value="currentSkill.category = $event" />
        <FieldSelect
          v-if="targetOptions.length"
          label="目标"
          allow-empty
          placeholder="（未指定）"
          :model-value="currentSkill.target ?? ''"
          :options="targetOptions"
          hint="来自「声明」tab 的目标类型"
          @update:model-value="currentSkill.target = $event"
        />
        <FieldText
          v-else
          label="目标"
          placeholder="单体 / 自身 / 友方…"
          hint="可在「声明」tab 定义候选"
          :model-value="currentSkill.target ?? ''"
          @update:model-value="currentSkill.target = $event"
        />
        <FieldSelect label="消耗资源" allow-empty placeholder="（无消耗）" :model-value="costResOf(currentSkill)" :options="resOptions" @update:model-value="setCost(currentSkill, $event, costAmtOf(currentSkill))" />
        <FieldNum label="消耗量" :min="0" :model-value="costAmtOf(currentSkill)" @update:model-value="setCost(currentSkill, costResOf(currentSkill), $event)" />
        <FieldArea label="描述" md :model-value="currentSkill.description" @update:model-value="currentSkill.description = $event" />
        <FieldArea
          label="即时效果"
          :rows="2"
          hint="声明式即时效果：改资源 res-gold +15 / 伤害 N / 治疗 N / 设标记 flag"
          :model-value="effectText(currentSkill)"
          @update:model-value="setEffectText(currentSkill, String($event))"
        />

        <section class="col-span-full border-t border-border/60 pt-3.5">
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">施加持续状态</span>
            <span class="text-[10.5px] text-muted-foreground/60">定义在「状态」tab</span>
          </div>
          <div v-if="statusOptions.length" class="flex flex-wrap gap-1.5">
            <button
              v-for="o in statusOptions"
              :key="o.id"
              type="button"
              class="rounded-full border px-2.5 py-1 text-xs transition-colors"
              :class="statusRefsOf(currentSkill).includes(o.id)
                ? 'border-primary/50 bg-primary/15 text-primary'
                : 'border-border/70 bg-muted/40 text-muted-foreground hover:border-primary/40'"
              @click="toggleStatus(currentSkill, o.id)"
            >
              {{ o.name }}
            </button>
          </div>
          <p v-else class="text-xs text-muted-foreground/60">还没有状态声明：去「状态」tab 新建，或技能仍可用 Lua 施加。</p>
          <p v-if="statusRefsOf(currentSkill).length" class="mt-2 text-[10.5px] leading-4 text-muted-foreground/65">
            结算时施加：{{ statusRefsOf(currentSkill).map(statusName).join('、') }}
          </p>
        </section>

        <section class="col-span-full border-t border-border/60 pt-3.5">
          <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
            <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">判定器（#12）</span>
            <Select :model-value="checkMode" @update:model-value="setCheckMode(String($event))">
              <SelectTrigger class="h-7 w-40 text-xs"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="none">无判定</SelectItem>
                  <SelectItem value="ref">引用全局判定器</SelectItem>
                  <SelectItem value="inline">自定义判定器</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
          <Input
            v-if="checkMode === 'ref'"
            :model-value="typeof currentSkill.check === 'string' ? currentSkill.check : ''"
            placeholder="全局判定器引用名（如 world）"
            @update:model-value="currentSkill.check = String($event)"
          />
          <CheckerEditor
            v-else-if="checkMode === 'inline'"
            :model-value="typeof currentSkill.check === 'object' ? currentSkill.check : null"
            :context="luaContext"
            @update:model-value="currentSkill.check = $event ?? undefined"
          />
          <p v-else class="text-xs text-muted-foreground/60">未声明判定器：使用技能效果或由 AI 叙事裁决。</p>
        </section>

        <section class="col-span-full border-t border-border/60 pt-3.5">
          <LuaHookEditor
            :model-value="currentSkill.lua ?? ''"
            mode="hook"
            title="Lua 技能扩展钩子（高级效果与自定义结算）"
            :skill="currentSkill"
            :storybook="d"
            @update:model-value="currentSkill.lua = $event"
          />
        </section>
      </FieldGrid>
    </template>

    <!-- 物品表单 -->
    <template v-else-if="currentItem">
      <EntityFormHeader :title="currentItem.name" sub="物品 · 可携带物件" :icon="IconPackage" :meta="currentItem.id" @remove="remove(currentItem)" />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="currentItem.name" @update:model-value="currentItem.name = $event" />
        <FieldText label="id" mono :model-value="currentItem.id" @update:model-value="currentItem.id = $event" />
        <FieldText label="类型" placeholder="消耗品 / 装备 / …" :model-value="currentItem.type ?? ''" @update:model-value="currentItem.type = $event" />
        <FieldNum label="数量" :min="0" :model-value="currentItem.quantity ?? 1" @update:model-value="currentItem.quantity = $event" />
        <FieldText
          label="武器伤害"
          mono
          placeholder="1d6 / 1d8+2"
          hint="玩家点名这件武器却没挂技能时，引擎按它结算伤害（命中加值用修饰器 target=attack）"
          :model-value="currentItem.damage ?? ''"
          @update:model-value="currentItem.damage = $event"
        />
        <FieldArea label="描述" md :model-value="currentItem.description" @update:model-value="currentItem.description = $event" />

        <div class="col-span-full">
          <ImageField
            label="图标"
            :model-value="currentItem.icon ?? null"
            :max-edge="512"
            ratio="1/1"
            max-width="7rem"
            @update:model-value="currentItem.icon = $event ?? undefined"
          />
        </div>

        <section class="col-span-full border-t border-border/60 pt-3.5">
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="text-[11px] font-bold tracking-wider text-muted-foreground uppercase">物品技能</span>
            <Button
              v-if="skillOptions.length"
              variant="ghost"
              size="sm"
              class="h-7 gap-1 text-xs text-muted-foreground"
              :disabled="(currentItem.skills?.length ?? 0) >= skillOptions.length"
              @click="addItemSkill(currentItem)"
            >
              <IconPlus class="size-3.5" />
              绑定技能
            </Button>
          </div>
          <p v-if="!skillOptions.length" class="text-xs text-muted-foreground/70">先在「技能」tab 建技能，再回来绑定。</p>
          <div v-for="(sid, i) in currentItem.skills ?? []" :key="i" class="mb-2 flex items-center gap-2">
            <Select :model-value="sid" @update:model-value="currentItem.skills![i] = String($event ?? '')">
              <SelectTrigger class="h-8 w-full flex-1 text-[13px]">
                <SelectValue placeholder="（选择技能）">
                  {{ skills.find(s => s.id === sid)?.name || sid }}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem v-for="o in skillOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
            <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" aria-label="移除技能" @click="currentItem.skills!.splice(i, 1)">
              <IconX class="size-3.5" />
            </Button>
          </div>
        </section>
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个条目开始编辑。</div>
  </WorkbenchLayout>
</template>
