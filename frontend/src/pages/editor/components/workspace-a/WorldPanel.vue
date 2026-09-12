<script setup lang="ts">
// WorldPanel —— A「世界」tab：#01 world = premise + locations + resources（左清单 / 右表单）
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { AssetRef, CheckerDef, LocationDef, ResourceDef } from '@/types'
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
import CheckerEditor from '@/components/CheckerEditor.vue'
import { buildLuaContext } from '@/lib/lua-context'
import { Button } from '@/components/ui/button'
import { IconDiamond, IconDice, IconMapPin, IconPhoto, IconPlus, IconWorld } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('world')
function select(id: string): void { selected.value = id }

/** 常驻伪实体（没有 id，用固定键） */
const OPENING = '__opening__'
const PREMISE = '__premise__'
const COVER = '__cover__'
const RATING = '__rating__'
const CHECK = '__check__'

const luaContext = computed(() => buildLuaContext(d.value))
const locations = computed<LocationDef[]>(() => d.value?.world.locations ?? [])
const resources = computed<ResourceDef[]>(() => d.value?.world.resources ?? [])

const items = computed<WorkbenchItem[]>(() => [
  { id: OPENING, title: '故事 · 开头', sub: 'opening', badge: '叙事', group: '世界观' },
  { id: PREMISE, title: '世界 · 前提', sub: 'premise', badge: '叙事', group: '世界观' },
  {
    id: COVER,
    title: '故事书封面',
    sub: 'meta.cover',
    badge: d.value?.meta.cover ? '已设' : '未设',
    tone: d.value?.meta.cover ? 'ok' : 'default',
    group: '世界观',
  },
  {
    id: RATING,
    title: '内容评级',
    sub: 'meta.rating',
    badge: d.value?.meta.rating === 'nsfw' ? 'NSFW' : 'SFW',
    tone: d.value?.meta.rating === 'nsfw' ? 'warn' : 'default',
    group: '世界观',
  },
  {
    id: CHECK,
    title: '全局判定器',
    sub: 'world.check',
    badge: d.value?.world.check ? '已设' : '默认',
    group: '规则',
  },
  ...locations.value.map(l => ({ id: entityKey(l), title: l.name, sub: l.id, badge: '地点', group: '地点' })),
  ...resources.value.map(r => ({ id: entityKey(r), title: r.name, sub: r.id, badge: r.type === 'binary' ? '二元' : '数值', group: '资源' })),
])

const currentLoc = computed(() => locations.value.find(l => entityKey(l) === selected.value) ?? null)
const currentRes = computed(() => resources.value.find(r => entityKey(r) === selected.value) ?? null)

function setOpening(v: string): void { if (d.value) d.value.world.opening = v }
function setPremise(v: string): void { if (d.value) d.value.world.premise = v }

/** 封面可移除：清空时删字段而不是留 null，保持 JSON 干净 */
function setCover(v: AssetRef | null): void {
  if (!d.value) return
  if (v) d.value.meta.cover = v
  else delete d.value.meta.cover
}

/** 内容评级：缺省 sfw；选 sfw 时删字段保持 JSON 干净。仅列表徽标用，不做过滤。 */
function setRating(v: 'sfw' | 'nsfw'): void {
  if (!d.value) return
  if (v === 'nsfw') d.value.meta.rating = 'nsfw'
  else delete d.value.meta.rating
}

function setWorldCheck(v: CheckerDef | null): void {
  if (!d.value) return
  if (v) d.value.world.check = v
  else delete d.value.world.check
}

function addLocation(): void {
  const w = d.value?.world
  if (!w) return
  const loc: LocationDef = { id: uid('loc'), name: '新地点', description: '' }
  w.locations.unshift(loc)
  selected.value = entityKey(loc)
}
function removeLocation(loc: LocationDef): void {
  const arr = d.value?.world.locations
  if (!arr) return
  const i = arr.indexOf(loc)
  if (i >= 0) arr.splice(i, 1)
}
function addResource(): void {
  const w = d.value?.world
  if (!w) return
  const r: ResourceDef = { id: uid('res'), name: '新资源', type: 'numerical', default_max: 100 }
  w.resources.unshift(r)
  selected.value = entityKey(r)
}
function removeResource(r: ResourceDef): void {
  const arr = d.value?.world.resources
  if (!arr) return
  const i = arr.indexOf(r)
  if (i >= 0) arr.splice(i, 1)
}
</script>

<template>
  <WorkbenchLayout
    title="世界"
    hint="背景叙事（premise）、地点与资源。判定器沿用全局默认（#12）。"
    :count="items.length"
    :items="items"
    :selected="selected"
    search-placeholder="搜索地点 / 资源…"
    empty-hint="世界观还是空的。用右上「地点」「资源」开始搭建。"
    custom-actions
    @update:selected="select"
  >
    <template #railActions>
      <Button variant="outline" size="sm" class="h-7 gap-1 px-2 text-xs" @click="addLocation">
        <IconPlus class="size-3.5" />
        地点
      </Button>
      <Button variant="outline" size="sm" class="h-7 gap-1 px-2 text-xs" @click="addResource">
        <IconPlus class="size-3.5" />
        资源
      </Button>
    </template>

    <template v-if="selected === OPENING">
      <EntityFormHeader title="故事 · 开头" sub="opening — 开档后玩家读到的第一段叙事" :icon="IconWorld" hide-remove />
      <FieldGrid class="mt-4">
        <FieldArea
          label="开场旁白"
          :model-value="d?.world.opening ?? ''"
          :rows="8"
          md
          placeholder="开档后，玩家看到的第一段文字……"
          hint="缺省时回落为「世界 · 前提」；留空则不显示。文档范式也会渲染这里"
          @update:model-value="setOpening"
        />
      </FieldGrid>
    </template>

    <template v-else-if="selected === PREMISE">
      <EntityFormHeader title="世界 · 前提" sub="premise — 故事发生的世界是什么样" :icon="IconWorld" hide-remove />
      <FieldGrid class="mt-4">
        <FieldArea
          label="背景叙事"
          :model-value="d?.world.premise ?? ''"
          :rows="8"
          md
          placeholder="世界基调、常识、通用规则…"
          hint="文档范式也会渲染这里"
          @update:model-value="setPremise"
        />
      </FieldGrid>
    </template>

    <template v-else-if="selected === COVER">
      <EntityFormHeader title="故事书封面" sub="meta.cover — 首页书架与编辑器顶栏使用" :icon="IconPhoto" hide-remove />
      <FieldGrid class="mt-4">
        <div class="col-span-full">
          <ImageField
            label="封面图"
            :model-value="d?.meta.cover ?? null"
            :max-edge="1600"
            ratio="16/9"
            max-width="24rem"
            hint="不上传时回落为按标题生成的 CSS 封面（色相 + 首字印鉴），所以封面是可选的"
            @update:model-value="setCover"
          />
        </div>
      </FieldGrid>
    </template>

    <template v-else-if="selected === RATING">
      <EntityFormHeader title="内容评级" sub="meta.rating — 仅列表徽标；不做内容过滤、不做年龄验证" :icon="IconWorld" hide-remove />
      <FieldGrid class="mt-4">
        <FieldSelect
          label="评级"
          :model-value="d?.meta.rating === 'nsfw' ? 'nsfw' : 'sfw'"
          :options="[{ value: 'sfw', label: 'SFW（全年龄，默认）' }, { value: 'nsfw', label: 'NSFW（成人向）' }]"
          :allow-empty="false"
          hint="创作者自报，供玩家在列表页识别；引擎、提示词与校验都不读它"
          @update:model-value="setRating($event as 'sfw' | 'nsfw')"
        />
      </FieldGrid>
    </template>

    <template v-else-if="selected === CHECK">
      <EntityFormHeader title="全局判定器" sub="world.check — 技能 check 可引用或覆盖（#12）" :icon="IconDice" hide-remove />
      <CheckerEditor class="mt-4" :model-value="d?.world.check ?? null" :context="luaContext" @update:model-value="setWorldCheck" />
    </template>

    <template v-else-if="currentLoc">
      <EntityFormHeader :title="currentLoc.name" sub="地点 · 场景可指向此 id" :icon="IconMapPin" @remove="removeLocation(currentLoc)" />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="currentLoc.name" @update:model-value="currentLoc.name = $event" />
        <FieldText label="id" mono hint="引用完整性：场景指向此 id" :model-value="currentLoc.id" @update:model-value="currentLoc.id = $event" />
        <FieldArea label="描述" :model-value="currentLoc.description ?? ''" placeholder="这里是什么样…" @update:model-value="currentLoc.description = $event" />
        <div class="col-span-full">
          <ImageField
            label="插图"
            :model-value="currentLoc.image ?? null"
            :max-edge="1280"
            ratio="16/9"
            max-width="20rem"
            @update:model-value="currentLoc.image = $event ?? undefined"
          />
        </div>
      </FieldGrid>
    </template>

    <template v-else-if="currentRes">
      <EntityFormHeader :title="currentRes.name" sub="资源 · 技能消耗引用此 id" :icon="IconDiamond" @remove="removeResource(currentRes)" />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="currentRes.name" @update:model-value="currentRes.name = $event" />
        <FieldText label="id" mono :model-value="currentRes.id" @update:model-value="currentRes.id = $event" />
        <FieldSelect label="类型" :model-value="currentRes.type" :options="[{ value: 'numerical', label: '数值' }, { value: 'binary', label: '二元' }]" @update:model-value="currentRes.type = $event as 'numerical' | 'binary'" />
        <FieldNum v-if="currentRes.type === 'numerical'" label="上限" :min="0" :model-value="currentRes.default_max ?? 0" @update:model-value="currentRes.default_max = $event" />
      </FieldGrid>
    </template>

    <div v-else class="py-10 text-center text-xs text-muted-foreground/60">从左侧选择一个条目开始编辑。</div>
  </WorkbenchLayout>
</template>
