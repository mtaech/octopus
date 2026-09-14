<script setup lang="ts">
// WorldPanel —— A「世界」tab：#01 world = premise + locations + resources + maps（左清单 / 右表单）
// 地图（地图 P1）：底图 + 地点锚点，锚点坐标归一化 0..1（换分辨率 / 换底图不破版）。
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { AssetRef, ChapterDef, CheckerDef, LocationDef, MapDef, MapPin, ResourceDef, SceneDef } from '@/types'
import { uid } from '@/types'
import { assetUrl } from '@/api'
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
import { IconDiamond, IconDice, IconMap, IconMapPin, IconPhoto, IconPlus, IconSwords, IconTarget, IconTrash, IconUsers, IconWorld } from '@tabler/icons-vue'

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
const maps = computed<MapDef[]>(() => d.value?.world.maps ?? [])

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
  ...maps.value.map(m => ({ id: entityKey(m), title: m.name, sub: m.id, badge: '地图', tone: 'ok' as const, group: '地图' })),
  ...locations.value.map(l => ({ id: entityKey(l), title: l.name, sub: l.id, badge: '地点', group: '地点' })),
  ...resources.value.map(r => ({ id: entityKey(r), title: r.name, sub: r.id, badge: r.type === 'binary' ? '二元' : '数值', group: '资源' })),
])

const currentLoc = computed(() => locations.value.find(l => entityKey(l) === selected.value) ?? null)
const currentRes = computed(() => resources.value.find(r => entityKey(r) === selected.value) ?? null)
const currentMap = computed(() => maps.value.find(m => entityKey(m) === selected.value) ?? null)

// ---------------- 地点的剧情关联（地图 P6 §6.7）：全部由既有字段推导，不落字段 ----------------
/** 该地点上的场景（chapter → scenes 推导） */
const locatedScenes = computed<{ scene: SceneDef; chapter: ChapterDef }[]>(() => {
  const id = currentLoc.value?.id
  if (!id) return []
  const out: { scene: SceneDef; chapter: ChapterDef }[] = []
  for (const ch of d.value?.skeleton ?? []) {
    for (const sc of ch.scenes) if (sc.location_id === id) out.push({ scene: sc, chapter: ch })
  }
  return out
})
/** 该地点的任务 = 这些场景的 goals（地点继承自场景，见设计 §6.2） */
const locatedGoals = computed(() => locatedScenes.value.flatMap(x => x.scene.goals.map(g => ({ goal: g, scene: x.scene }))))
/** 常驻 / 出没此地点的人与怪物（CharacterDef.location_id） */
const locatedChars = computed(() => (d.value?.characters ?? []).filter(c => c.location_id === currentLoc.value?.id))
/** 把此地点锚定到底图上的地图 */
const locatedMaps = computed(() => maps.value.filter(m => (m.pins ?? []).some(p => p.location_id === currentLoc.value?.id)))

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

// ---------------- 地图（地图 P1）：底图 + 地点锚点（归一化 0..1） ----------------
/** 地图可层级（世界图 > 区域图 > 地牢图）；不得指向自己 */
const parentMapOptions = computed(() =>
  maps.value
    .filter(m => m !== currentMap.value)
    .map(m => ({ value: m.id, label: m.name + ' · ' + m.id })),
)
const locationOptions = computed(() => locations.value.map(l => ({ value: l.id, label: l.name + ' · ' + l.id })))
const knownLocationIds = computed(() => new Set(locations.value.map(l => l.id)))
/** 底图预览框比例：用上传时记录的像素宽高，图像填满容器 = 归一化坐标与图像严格对应 */
const mapAspect = computed(() => {
  const img = currentMap.value?.image
  return img && img.w && img.h ? img.w + ' / ' + img.h : '16 / 9'
})
const canvasEl = ref<HTMLElement | null>(null)
const draggingPin = ref<number | null>(null)
/** 拖拽落点后紧接着的 click 不该再新增锚点 */
let suppressCanvasClick = false

function addMap(): void {
  const w = d.value?.world
  if (!w) return
  if (!w.maps) w.maps = []
  const m: MapDef = { id: uid('map'), name: '新地图', pins: [] }
  w.maps.unshift(m)
  selected.value = entityKey(m)
}
function removeMap(m: MapDef): void {
  const w = d.value?.world
  const arr = w?.maps
  if (!arr) return
  const i = arr.indexOf(m)
  if (i >= 0) arr.splice(i, 1)
  // 子图 parent_id 悬空 → 摘掉（校验会报 dangling_map_parent_ref）
  arr.forEach(x => { if (x.parent_id === m.id) delete x.parent_id })
}
function setMapImage(m: MapDef, v: AssetRef | null): void {
  if (v) m.image = v
  else delete m.image
}
function setMapParent(m: MapDef, v: string): void {
  if (v) m.parent_id = v
  else delete m.parent_id
}
function clamp01(n: number): number {
  return Math.min(1, Math.max(0, Number.isFinite(n) ? n : 0))
}
function round4(n: number): number {
  return Math.round(clamp01(n) * 10000) / 10000
}
/** 底图上第一个还没被占用的地点 */
function freeLocationId(): string {
  const used = new Set((currentMap.value?.pins ?? []).map(p => p.location_id))
  return locations.value.find(l => !used.has(l.id))?.id ?? locations.value[0]?.id ?? ''
}
function addPin(): void {
  const m = currentMap.value
  if (!m) return
  if (!m.pins) m.pins = []
  m.pins.push({ location_id: freeLocationId(), x: 0.5, y: 0.5 })
}
function removePin(i: number): void {
  currentMap.value?.pins?.splice(i, 1)
}
function setPinLocation(pin: MapPin, v: string): void {
  pin.location_id = v
}
function setPinCoord(pin: MapPin, axis: 'x' | 'y', v: number): void {
  pin[axis] = round4(v)
}
function pinDangling(pin: MapPin): boolean {
  return !!pin.location_id && !knownLocationIds.value.has(pin.location_id)
}
const danglingPinCount = computed(() => (currentMap.value?.pins ?? []).filter(p => pinDangling(p)).length)
const duplicatePinCount = computed(() => {
  const list = (currentMap.value?.pins ?? []).map(p => p.location_id).filter(Boolean)
  return list.length - new Set(list).size
})
function pinLabel(pin: MapPin): string {
  if (pin.label) return pin.label
  return locations.value.find(l => l.id === pin.location_id)?.name ?? (pin.location_id || '未绑定地点')
}
/** 画布坐标 → 归一化 0..1 */
function pinFromEvent(e: PointerEvent): { x: number; y: number } | null {
  const el = canvasEl.value
  if (!el) return null
  const r = el.getBoundingClientRect()
  if (!r.width || !r.height) return null
  return { x: round4((e.clientX - r.left) / r.width), y: round4((e.clientY - r.top) / r.height) }
}
function startDrag(e: PointerEvent, i: number): void {
  const m = currentMap.value
  if (!m?.pins) return
  e.preventDefault()
  draggingPin.value = i
  suppressCanvasClick = true
  const p = pinFromEvent(e)
  if (p) { m.pins[i].x = p.x; m.pins[i].y = p.y }
  canvasEl.value?.setPointerCapture?.(e.pointerId)
}
function onDragMove(e: PointerEvent): void {
  const m = currentMap.value
  const i = draggingPin.value
  if (i == null || !m?.pins?.[i]) return
  const p = pinFromEvent(e)
  if (!p) return
  m.pins[i].x = p.x
  m.pins[i].y = p.y
}
function endDrag(e: PointerEvent): void {
  if (draggingPin.value == null) return
  draggingPin.value = null
  canvasEl.value?.releasePointerCapture?.(e.pointerId)
}
function onCanvasClick(e: MouseEvent): void {
  if (suppressCanvasClick) { suppressCanvasClick = false; return }
  const m = currentMap.value
  const el = canvasEl.value
  if (!m || !el) return
  const r = el.getBoundingClientRect()
  if (!r.width || !r.height) return
  if (!m.pins) m.pins = []
  m.pins.push({ location_id: freeLocationId(), x: round4((e.clientX - r.left) / r.width), y: round4((e.clientY - r.top) / r.height) })
}
</script>

<template>
  <WorkbenchLayout
    title="世界"
    hint="背景叙事（premise）、地图、地点与资源。地图是地点的可视投影：底图 + 归一化锚点；判定器沿用全局默认（#12）。"
    :count="items.length"
    :items="items"
    :selected="selected"
    search-placeholder="搜索地图 / 地点 / 资源…"
    empty-hint="世界观还是空的。用右上「地图」「地点」「资源」开始搭建。"
    custom-actions
    @update:selected="select"
  >
    <template #railActions>
      <Button variant="outline" size="sm" class="h-7 gap-1 px-2 text-xs" title="新增地图：底图 + 地点锚点" @click="addMap">
        <IconMap class="size-3.5" />
        地图
      </Button>
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

    <template v-else-if="currentMap">
      <EntityFormHeader
        :title="currentMap.name"
        sub="地图 · 地点的可视投影（引擎不读，纯视图）"
        :icon="IconMap"
        @remove="removeMap(currentMap)"
      />
      <FieldGrid class="mt-4">
        <FieldText label="名称" :model-value="currentMap.name" @update:model-value="currentMap.name = $event" />
        <FieldText label="id" mono hint="子图 parent_id 指向此 id" :model-value="currentMap.id" @update:model-value="currentMap.id = $event" />
        <FieldSelect
          label="上层地图"
          :model-value="currentMap.parent_id ?? ''"
          :options="parentMapOptions"
          placeholder="（无 · 顶层地图）"
          hint="世界图 > 区域图 > 地牢图；不得成环"
          @update:model-value="setMapParent(currentMap, $event)"
        />
        <div class="col-span-full">
          <ImageField
            label="底图"
            :model-value="currentMap.image ?? null"
            :max-edge="2048"
            ratio="16/9"
            max-width="28rem"
            hint="锚点坐标存归一化的 0..1，换分辨率 / 换底图都不破版"
            @update:model-value="setMapImage(currentMap, $event)"
          />
        </div>
      </FieldGrid>

      <!-- 地点锚点：点底图或按钮新增，拖动圆点改位置 -->
      <div class="mt-5 rounded-xl border border-border/70 bg-card/40 p-3.5">
        <div class="flex flex-wrap items-center gap-2">
          <IconMapPin class="size-4 shrink-0 text-primary" />
          <span class="font-serif text-base font-bold text-foreground">地点锚点</span>
          <span class="rounded-full bg-muted/60 px-2 py-0.5 font-mono text-[10.5px] text-muted-foreground">
            {{ currentMap.pins?.length ?? 0 }} 个
          </span>
          <Button
            variant="outline"
            size="sm"
            class="ml-auto h-7 gap-1 px-2 text-xs"
            title="新增一个锚点（默认放在底图中央，再拖动调整）"
            @click="addPin"
          >
            <IconPlus class="size-3.5" />
            添加锚点
          </Button>
        </div>
        <p class="mt-1 text-[11px] leading-5 text-muted-foreground/70">
          点击底图空白处也能落一个锚点；按住圆点拖动调整位置。坐标以 <span class="font-mono">0..1</span> 归一化存储。
        </p>

        <div
          v-if="currentMap.image"
          ref="canvasEl"
          class="relative mt-2 w-full cursor-crosshair touch-none overflow-hidden rounded-lg border border-dashed border-border/70 bg-muted/20 select-none"
          :style="{ aspectRatio: mapAspect }"
          @pointermove="onDragMove"
          @pointerup="endDrag"
          @pointercancel="endDrag"
          @click="onCanvasClick"
        >
          <img
            :src="assetUrl(currentMap.image.asset)"
            alt=""
            draggable="false"
            class="pointer-events-none absolute inset-0 size-full object-fill"
          />
          <button
            v-for="(pin, i) in currentMap.pins ?? []"
            :key="i"
            type="button"
            class="absolute z-10 inline-flex -translate-x-1/2 -translate-y-1/2 cursor-grab items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-semibold shadow-sm transition-colors active:cursor-grabbing"
            :class="pinDangling(pin)
              ? 'border-destructive/60 bg-destructive/15 text-destructive'
              : 'border-primary/60 bg-primary/15 text-primary hover:bg-primary/25'"
            :style="{ left: (pin.x * 100) + '%', top: (pin.y * 100) + '%' }"
            :title="pinLabel(pin) + ' · x=' + pin.x + ' y=' + pin.y + '（拖动调整位置）'"
            @pointerdown.stop="startDrag($event, i)"
            @click.stop
          >
            <IconMapPin class="size-3" />
            {{ pinLabel(pin) }}
          </button>
        </div>
        <div
          v-else
          class="mt-2 rounded-lg border border-dashed border-border/80 bg-muted/20 p-6 text-center text-[11.5px] leading-6 text-muted-foreground"
        >
          先上传底图，再在图上放置地点锚点。
        </div>

        <!-- 锚点列表：绑定地点 + 坐标 + 标签 -->
        <div v-if="(currentMap.pins?.length ?? 0)" class="mt-3 space-y-2">
          <div
            v-for="(pin, i) in currentMap.pins ?? []"
            :key="i"
            class="grid grid-cols-1 items-end gap-2 rounded-lg border border-border/60 bg-background/50 p-2 @lg:grid-cols-[minmax(0,2fr)_6.5rem_6.5rem_minmax(0,1fr)_2rem]"
          >
            <FieldSelect
              dense
              label="绑定地点"
              :model-value="pin.location_id"
              :options="locationOptions"
              placeholder="（未绑定地点）"
              @update:model-value="setPinLocation(pin, $event)"
            />
            <FieldNum label="x（0–1）" :min="0" :max="1" :step="0.01" :model-value="pin.x" @update:model-value="setPinCoord(pin, 'x', $event)" />
            <FieldNum label="y（0–1）" :min="0" :max="1" :step="0.01" :model-value="pin.y" @update:model-value="setPinCoord(pin, 'y', $event)" />
            <FieldText dense label="标签（可选）" :model-value="pin.label ?? ''" placeholder="覆盖地点名" @update:model-value="pin.label = ($event || undefined)" />
            <Button
              variant="ghost"
              size="icon-sm"
              class="size-8 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive"
              :title="'删除锚点 ' + (i + 1)"
              :aria-label="'删除锚点 ' + (i + 1)"
              @click="removePin(i)"
            >
              <IconTrash class="size-3.5" />
            </Button>
            <p v-if="pinDangling(pin)" class="col-span-full text-[11px] text-destructive">
              这个锚点指向的地点「{{ pin.location_id }}」已不存在，发布校验会拦截。
            </p>
          </div>
          <p v-if="danglingPinCount" class="text-[11px] text-destructive">
            有 {{ danglingPinCount }} 个锚点引用了不存在的地点。
          </p>
          <p v-if="duplicatePinCount" class="text-[11px] text-warning">
            有 {{ duplicatePinCount }} 个地点在同一张地图上重复锚定（同一地点可以多锚点，确认是否有意）。
          </p>
        </div>
      </div>
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

      <!-- 剧情关联：地点是「这一章发生在哪、这里有什么任务、谁在这儿」的聚合点（地图即剧情视图 §6.7） -->
      <div class="mt-5 rounded-xl border border-border/70 bg-card/40 p-3.5">
        <div class="flex flex-wrap items-center gap-2">
          <IconMapPin class="size-4 shrink-0 text-primary" />
          <span class="font-serif text-base font-bold text-foreground">剧情关联</span>
          <span class="rounded-full bg-muted/60 px-2 py-0.5 font-mono text-[10.5px] text-muted-foreground">推导 · 不落字段</span>
        </div>
        <div class="mt-2.5 grid grid-cols-1 gap-3 @lg:grid-cols-2">
          <div>
            <div class="flex items-center gap-1.5 text-[11px] font-bold tracking-wider text-muted-foreground uppercase">
              <IconMap class="size-3.5 text-muted-foreground/70" />
              此地的场景
            </div>
            <div class="mt-1 flex flex-wrap gap-1.5">
              <span
                v-for="x in locatedScenes"
                :key="x.chapter.id + '/' + x.scene.id"
                class="inline-flex items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-1.5 py-0.5 text-[11px] text-foreground/90"
                :title="'章节：' + x.chapter.title + ' · 场景 id：' + x.scene.id"
              >
                {{ x.chapter.title }} / {{ x.scene.title }}
              </span>
              <span v-if="!locatedScenes.length" class="text-[11px] text-muted-foreground/60">还没有场景声明这个地点（去「骨架」给场景选地点）</span>
            </div>
          </div>
          <div>
            <div class="flex items-center gap-1.5 text-[11px] font-bold tracking-wider text-muted-foreground uppercase">
              <IconTarget class="size-3.5 text-muted-foreground/70" />
              此地的任务
            </div>
            <div class="mt-1 flex flex-wrap gap-1.5">
              <span
                v-for="x in locatedGoals"
                :key="x.goal.id"
                class="inline-flex max-w-full items-center gap-1 truncate rounded-md border border-border/70 bg-muted/40 px-1.5 py-0.5 text-[11px] text-foreground/90"
                :title="'继承自场景「' + x.scene.title + '」' + (x.goal.primary ? ' · 主要目标' : '')"
              >
                {{ x.goal.text }}
              </span>
              <span v-if="!locatedGoals.length" class="text-[11px] text-muted-foreground/60">还没有任务 —— 任务的地点继承自场景</span>
            </div>
          </div>
          <div>
            <div class="flex items-center gap-1.5 text-[11px] font-bold tracking-wider text-muted-foreground uppercase">
              <IconUsers class="size-3.5 text-muted-foreground/70" />
              常驻 / 出没
            </div>
            <div class="mt-1 flex flex-wrap gap-1.5">
              <span
                v-for="c in locatedChars"
                :key="c.id"
                class="inline-flex items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-1.5 py-0.5 text-[11px] text-foreground/90"
                :title="'角色 id：' + c.id + '（' + (c.kind === 'monster' ? '怪物 · 遭遇会继承它的出没地' : c.kind === 'pc' ? 'PC' : 'NPC') + '）'"
              >
                <IconSwords v-if="c.kind === 'monster'" class="size-3 text-warning/80" />
                {{ c.name }}
              </span>
              <span v-if="!locatedChars.length" class="text-[11px] text-muted-foreground/60">没有人物 / 怪物把这里当常驻地</span>
            </div>
          </div>
          <div>
            <div class="flex items-center gap-1.5 text-[11px] font-bold tracking-wider text-muted-foreground uppercase">
              <IconMapPin class="size-3.5 text-muted-foreground/70" />
              锚定此地的地图
            </div>
            <div class="mt-1 flex flex-wrap gap-1.5">
              <span
                v-for="m in locatedMaps"
                :key="m.id"
                class="inline-flex items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-1.5 py-0.5 text-[11px] text-foreground/90"
              >
                {{ m.name }}
              </span>
              <span v-if="!locatedMaps.length" class="text-[11px] text-muted-foreground/60">还没有地图锚定这个地点</span>
            </div>
          </div>
        </div>
      </div>
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
