<script setup lang="ts">
// ============================================================
// 地图面板（地图与在场 P3/P6 §6.7）：地点之上的**可视层**——
// 底图 + 锚点 + 当前地点高亮 + 按实例位置聚合的单位（PC / NPC / 怪物可区分）。
// 地图不是地点，是地点的投影：锚点属于地图，坐标是归一化的 0..1，换分辨率不破版。
//
// P6「地图即剧情视图」：章节范围（该章 scenes 的 location_id 并集）与任务 / 遭遇标记
// 一并画在地点上——这一章发生在哪、哪里有事，一屏可见。
// ============================================================
import { computed, ref, watch } from 'vue'
import { usePlayStore } from '../stores/play'
import { assetUrl } from '@/api'
import {
  chapterLocationIds, chapterOfScene, encountersAtLocation, initial, kindLabel,
  nameTintClass, portraitOf, questsAtLocation, unitTally, unitsAtLocation,
} from '../utils'
import type { LocationUnits, UnitTally } from '../utils'
import type { ChapterDef, CharacterInstance, MapDef, MapPin, SceneDef, Storybook } from '@/types'
import type { WorldSelection } from '../selection'
import { IconMap, IconMapPin, IconUsers, IconSwords, IconChevronRight, IconTarget, IconNotes } from '@tabler/icons-vue'

const props = defineProps<{ selected: WorldSelection | null }>()
const emit = defineEmits<{ (e: 'select', s: WorldSelection): void }>()

const store = usePlayStore()
const sb = computed<Storybook | null>(() => store.detail?.storybook ?? null)
const maps = computed<MapDef[]>(() => sb.value?.world.maps ?? [])

/** 当前展示的地图：地图可层级（世界图 > 区域图 > 地牢图），v1 平铺切换。 */
const activeId = ref('')
watch(maps, (list) => {
  if (!list.some(m => m.id === activeId.value)) activeId.value = list[0]?.id ?? ''
}, { immediate: true })
const activeMap = computed<MapDef | null>(() => maps.value.find(m => m.id === activeId.value) ?? null)

/** 当前场景（骨架里按 scene_id 找）：它落地的地点就是「当前地点」。 */
const scene = computed<SceneDef | null>(() => {
  const p = store.projection
  const sk = store.detail?.storybook.skeleton
  if (!p || !sk) return null
  for (const ch of sk) for (const sc of ch.scenes) if (sc.id === p.scene_id) return sc
  return null
})
const currentLoc = computed(() => scene.value?.location_id ?? '')

// ---- 当前章节范围（地图 P6 §6.7）：该章全部场景的 location_id 并集 ----
const skeleton = computed<ChapterDef[]>(() => sb.value?.skeleton ?? [])
const chapter = computed<ChapterDef | null>(() => chapterOfScene(skeleton.value, store.projection?.scene_id ?? ''))
const chapterLocs = computed<string[]>(() => chapterLocationIds(chapter.value))
const inChapter = (id: string): boolean => chapterLocs.value.includes(id)

function locName(id: string): string {
  return store.projection?.locations.find(l => l.id === id)?.name ?? id
}
function locExists(id: string): boolean {
  return (store.projection?.locations ?? []).some(l => l.id === id)
}
function pinLabel(pin: MapPin): string { return pin.label?.trim() || locName(pin.location_id) }
function isSelLoc(id: string): boolean { return props.selected?.kind === 'location' && props.selected.id === id }
function isSelChar(instanceId: string): boolean { return props.selected?.kind === 'character' && props.selected.id === instanceId }
function openLoc(id: string): void { emit('select', { kind: 'location', id }) }
function openChar(c: CharacterInstance): void { emit('select', { kind: 'character', id: c.instance_id }) }

// ---- 地点上的剧情标记（地图 P6）：任务（引擎按场景推导 location_id）+ 遭遇（创建快照） ----
interface LocStory { quests: number; openQuests: number; encounters: number; enemies: number }
function storyAt(id: string): LocStory {
  if (!id) return { quests: 0, openQuests: 0, encounters: 0, enemies: 0 }
  const qs = questsAtLocation(store.projection, id)
  const es = encountersAtLocation(store.projection, id)
  return {
    quests: qs.length,
    openQuests: qs.filter(q => !q.done).length,
    encounters: es.length,
    enemies: es.reduce((n, e) => n + e.enemies.length, 0),
  }
}
function tallyAt(id: string): UnitTally { return unitTally(unitsAtLocation(store.projection, id)) }
function storyTitle(id: string): string {
  const s = storyAt(id)
  const parts: string[] = []
  if (s.quests) parts.push(s.quests + ' 个任务（未完成 ' + s.openQuests + '）')
  if (s.encounters) parts.push(s.encounters + ' 场遭遇（' + s.enemies + ' 敌）')
  return parts.join(' · ')
}

// ---- 底图 ----
const imageUrl = computed(() => {
  const img = activeMap.value?.image
  return img ? assetUrl(img.asset) : ''
})
const aspect = computed(() => {
  const img = activeMap.value?.image
  return img && img.w > 0 && img.h > 0 ? img.w + ' / ' + img.h : '16 / 9'
})

// ---- 锚点：地点在底图上的位置 + 该地点的单位统计 ----
interface PinnedLoc { id: string; label: string; x: number; y: number; current: boolean; selected: boolean; chapter: boolean; missing: boolean; units: LocationUnits; tally: UnitTally; story: LocStory }
const pins = computed<PinnedLoc[]>(() => (activeMap.value?.pins ?? []).map(pin => {
  const units = unitsAtLocation(store.projection, pin.location_id)
  return {
    id: pin.location_id,
    label: pinLabel(pin),
    x: pin.x,
    y: pin.y,
    current: pin.location_id === currentLoc.value,
    selected: isSelLoc(pin.location_id),
    chapter: inChapter(pin.location_id),
    missing: !locExists(pin.location_id),
    units,
    tally: unitTally(units),
    story: storyAt(pin.location_id),
  }
}))

// ---- 按地点聚合：锚点优先，其次「有单位 / 有剧情」的地点，最后兜底当前地点 ----
interface LocAgg { id: string; name: string; current: boolean; chapter: boolean; pinned: boolean; selected: boolean; missing: boolean; units: LocationUnits; tally: UnitTally; story: LocStory }
const aggregates = computed<LocAgg[]>(() => {
  const p = store.projection
  const out = new Map<string, LocAgg>()
  const add = (id: string, label: string | undefined, pinned: boolean): void => {
    if (!id || out.has(id)) return
    const units = unitsAtLocation(p, id)
    out.set(id, {
      id,
      name: label?.trim() || locName(id),
      current: id === currentLoc.value,
      chapter: inChapter(id),
      pinned,
      selected: isSelLoc(id),
      missing: !locExists(id),
      units,
      tally: unitTally(units),
      story: storyAt(id),
    })
  }
  for (const pin of activeMap.value?.pins ?? []) add(pin.location_id, pin.label, true)
  for (const l of p?.locations ?? []) {
    if (unitTally(unitsAtLocation(p, l.id)).total || storyAt(l.id).quests || storyAt(l.id).encounters) add(l.id, undefined, false)
  }
  add(currentLoc.value, undefined, false)
  return [...out.values()].sort((a, b) =>
    Number(b.current) - Number(a.current)
    || Number(b.chapter) - Number(a.chapter)
    || (b.story.openQuests + b.story.encounters) - (a.story.openQuests + a.story.encounters)
    || b.tally.total - a.tally.total
    || Number(b.pinned) - Number(a.pinned))
})

const grandTotal = computed(() => aggregates.value.reduce((n, a) => n + a.tally.total, 0))

// ---- 单位外观：PC / NPC / 怪物三色区分 ----
function chipClass(kind: string): string {
  return kind === 'monster' ? 'border-destructive/50 bg-destructive/12 text-destructive'
    : kind === 'npc' ? 'border-info/45 bg-info/10 text-info'
    : 'border-primary/45 bg-primary/12 text-primary'
}
function portraitUrl(c: CharacterInstance): string {
  const p = portraitOf(sb.value, c.template_id, c.name)
  return p ? assetUrl(p.asset) : ''
}
/**
 * 锚点定位：左半图用 left、右半图用 right 锚定，纵向贴边时翻转对齐。
 * 绝对定位盒的**可用宽度受定位侧限制**——锚点贴在右边缘却用 left: 82% 时只剩 18% 可用宽度，
 * 标签会被挤成省略号；换成 right 锚定（再 flex-row-reverse 让图钉朝向锚点）就不挤了，
 * 坐标本身不动。
 */
function pinStyle(x: number, y: number): Record<string, string> {
  const dy = y > 0.78 ? '-100%' : y < 0.22 ? '0%' : '-50%'
  const s: Record<string, string> = { top: (y * 100) + '%', transform: 'translateY(' + dy + ')' }
  if (x > 0.5) s.right = ((1 - x) * 100) + '%'
  else s.left = (x * 100) + '%'
  return s
}
function isControlled(c: CharacterInstance): boolean {
  return !!store.controlledId && (c.template_id === store.controlledId || c.instance_id === store.controlledId)
}
/** 章节范围 chip：当前地点 > 本章其它地点 > 锚点已失效。 */
function chapterChipClass(id: string): string {
  if (!locExists(id)) return 'cursor-default border-dashed border-destructive/50 bg-destructive/10 text-destructive'
  if (id === currentLoc.value) return 'border-primary bg-primary/15 text-primary ring-1 ring-primary/30'
  if (storyAt(id).encounters) return 'border-destructive/45 bg-destructive/10 text-destructive hover:bg-destructive/20'
  if (storyAt(id).quests) return 'border-primary/45 bg-primary/10 text-primary hover:bg-primary/20'
  return 'border-border bg-card/80 text-foreground/80 hover:border-primary/40 hover:bg-muted/50'
}
const headBase = 'flex flex-wrap items-center gap-2 border-b border-border/60 bg-card/60 px-3 py-2'
</script>

<template>
  <div class="@container flex min-h-0 flex-1 flex-col">
    <!-- 头部：地图切换 + 聚合口径 -->
    <div :class="headBase">
      <IconMap class="size-3.5 shrink-0 text-info" />
      <span class="text-[11px] font-extrabold tracking-[1.2px] text-muted-foreground uppercase">地图</span>
      <div v-if="maps.length > 1" class="flex flex-wrap items-center gap-1">
        <button
          v-for="m in maps"
          :key="m.id"
          type="button"
          class="cursor-pointer rounded-full border px-2 py-0.5 text-[11px] font-semibold transition-colors"
          :class="m.id === activeId ? 'border-primary/50 bg-primary/12 text-primary' : 'border-border bg-muted/40 text-muted-foreground hover:text-foreground'"
          @click="activeId = m.id"
        >{{ m.name }}</button>
      </div>
      <span v-else-if="activeMap" class="truncate text-[11.5px] font-bold text-foreground/80">{{ activeMap.name }}</span>
      <span v-if="maps.length" class="ml-auto text-[10.5px] text-muted-foreground/60">
        按实例位置聚合 · {{ aggregates.length }} 地点 / {{ grandTotal }} 单位<span v-if="chapterLocs.length"> · 本章 {{ chapterLocs.length }} 地点</span>
      </span>
    </div>

    <!-- 当前章节范围（地图 P6 §6.7）：该章 scenes 的 location_id 并集，一处不漏 -->
    <div v-if="maps.length && chapter" class="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border/60 bg-card/35 px-3 py-2">
      <IconNotes class="size-3.5 shrink-0 text-warning" />
      <span class="text-[10.5px] font-extrabold tracking-[1.2px] text-muted-foreground uppercase">本章范围</span>
      <span class="max-w-48 truncate text-[11.5px] font-bold text-foreground/85" :title="chapter.title">{{ chapter.title }}</span>
      <span class="text-[10.5px] text-muted-foreground/60">{{ chapterLocs.length }} 地点</span>
      <div class="flex flex-wrap items-center gap-1">
        <button
          v-for="id in chapterLocs"
          :key="id"
          type="button"
          class="inline-flex max-w-56 items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-semibold transition-colors"
          :class="[chapterChipClass(id), locExists(id) ? 'cursor-pointer' : '']"
          :title="locExists(id)
            ? locName(id) + ' · ' + tallyAt(id).total + ' 单位' + (storyTitle(id) ? ' · ' + storyTitle(id) : '') + ' · ' + id
            : id + '（地点已不存在）'"
          @click="locExists(id) && openLoc(id)"
        >
          <IconMapPin class="size-3 shrink-0" />
          <span class="truncate">{{ locName(id) }}</span>
          <span v-if="id === currentLoc" class="shrink-0 rounded-full bg-primary/25 px-1 text-[9.5px] font-bold">当前</span>
          <span v-if="!locExists(id)" class="shrink-0 text-[9.5px] font-bold">缺失</span>
          <span v-if="storyAt(id).quests" class="inline-flex shrink-0 items-center gap-0.5 text-[9.5px] font-bold"><IconTarget class="size-2.5" />{{ storyAt(id).quests }}</span>
          <span v-if="storyAt(id).encounters" class="inline-flex shrink-0 items-center gap-0.5 text-[9.5px] font-bold"><IconSwords class="size-2.5" />{{ storyAt(id).encounters }}</span>
        </button>
        <span v-if="!chapterLocs.length" class="text-[10.5px] text-muted-foreground/60">这一章的场景都没有绑定地点</span>
      </div>
    </div>

    <!-- 没有地图：纯视图，缺了不影响游玩 -->
    <div v-if="!maps.length" class="m-3 rounded-xl border border-dashed border-border/80 bg-muted/20 p-8 text-center text-[12px] leading-6 text-muted-foreground">
      这本故事书还没有地图。<br />
      作者可在编辑器「世界 → 地图」里加上底图与地点锚点，锚点与单位聚合会自动出现在这里。
    </div>

    <div v-else class="min-h-0 flex-1 overflow-y-auto p-3">
      <div class="@2xl:grid @2xl:grid-cols-[minmax(0,1fr)_20rem] @2xl:gap-3">
        <!-- 底图 + 锚点 -->
        <div>
          <div
            class="relative w-full overflow-hidden rounded-xl border border-border/70 bg-muted/20"
            :style="{ aspectRatio: aspect }"
          >
            <img
              v-if="imageUrl"
              :src="imageUrl"
              :alt="activeMap?.name ?? '地图'"
              class="pointer-events-none absolute inset-0 size-full object-fill"
              loading="lazy"
              decoding="async"
            />
            <div v-else class="absolute inset-0 flex items-center justify-center">
              <div
                class="absolute inset-0 opacity-50"
                :style="{
                  backgroundImage: 'linear-gradient(to right, var(--border) 1px, transparent 1px), linear-gradient(to bottom, var(--border) 1px, transparent 1px)',
                  backgroundSize: '2.5rem 2.5rem',
                }"
              ></div>
              <span class="relative rounded-full border border-border/70 bg-card/90 px-2 py-0.5 text-[10.5px] font-semibold text-muted-foreground shadow-2xs">未设置底图 · 锚点仍按归一化坐标显示</span>
            </div>

            <button
              v-for="pin in pins"
              :key="pin.id"
              type="button"
              class="absolute z-10 inline-flex max-w-[75%] cursor-pointer items-center gap-1 rounded-full border px-2 py-0.5 text-[10.5px] font-semibold shadow-sm backdrop-blur-sm transition-colors"
              :class="[pin.x > 0.5 ? 'flex-row-reverse' : '', pin.missing
                ? 'border-dashed border-destructive/60 bg-destructive/15 text-destructive'
                : pin.current
                  ? 'border-primary bg-primary/20 text-primary ring-2 ring-primary/35'
                  : pin.selected
                    ? 'border-info bg-info/15 text-info ring-2 ring-info/35'
                    : pin.chapter
                      ? 'border-warning/55 bg-warning/15 text-warning hover:border-warning hover:bg-warning/25'
                      : 'border-border bg-card/95 text-foreground/85 hover:border-primary/50 hover:bg-primary/10']"
              :style="pinStyle(pin.x, pin.y)"
              :title="pin.label + (pin.missing ? '（锚点指向的地点已不存在）' : '') + ' · ' + pin.tally.people + ' 人 / ' + pin.tally.monsters + ' 怪' + (storyTitle(pin.id) ? ' · ' + storyTitle(pin.id) : '')"
              @click="openLoc(pin.id)"
            >
              <IconMapPin class="size-3 shrink-0" />
              <span class="truncate">{{ pin.label }}</span>
              <span v-if="pin.current" class="shrink-0 rounded-full bg-primary/25 px-1 text-[9.5px] font-bold">当前</span>
              <span v-if="pin.tally.people" class="shrink-0 rounded-full bg-info/20 px-1 font-mono text-[9.5px] font-bold text-info">{{ pin.tally.people }}人</span>
              <span v-if="pin.tally.monsters" class="shrink-0 rounded-full bg-destructive/20 px-1 font-mono text-[9.5px] font-bold text-destructive">{{ pin.tally.monsters }}怪</span>
              <span v-if="pin.story.quests" class="inline-flex shrink-0 items-center gap-0.5 rounded-full bg-primary/20 px-1 font-mono text-[9.5px] font-bold text-primary"><IconTarget class="size-2.5" />{{ pin.story.quests }}</span>
              <span v-if="pin.story.encounters" class="inline-flex shrink-0 items-center gap-0.5 rounded-full bg-destructive/20 px-1 font-mono text-[9.5px] font-bold text-destructive"><IconSwords class="size-2.5" />{{ pin.story.encounters }}</span>
            </button>
          </div>
          <p class="mt-1.5 text-[10.5px] leading-5 text-muted-foreground/60">
            锚点坐标是相对底图的归一化 <span class="font-mono">0..1</span>：换分辨率 / 换底图都不破版。点锚点看该地点的详情。
          </p>
        </div>

        <!-- 按地点聚合的单位 -->
        <div class="mt-3 space-y-2 @2xl:mt-0">
          <p v-if="!aggregates.length" class="rounded-xl border border-dashed border-border/80 bg-muted/20 p-4 text-center text-[11.5px] text-muted-foreground">
            这张地图还没有锚点。作者绑定地点后，这里会按地点列出单位。
          </p>
          <div
            v-for="a in aggregates"
            :key="a.id"
            class="rounded-xl border p-2.5 transition-colors"
            :class="a.current ? 'border-primary/50 bg-primary/10' : a.selected ? 'border-info/50 bg-info/10' : a.chapter ? 'border-warning/45 bg-warning/5' : 'border-border/70 bg-card/50'"
          >
            <div class="flex flex-wrap items-center gap-1.5">
              <IconMapPin class="size-3.5 shrink-0" :class="a.current ? 'text-primary' : a.chapter ? 'text-warning' : 'text-muted-foreground/60'" />
              <button type="button" class="min-w-0 cursor-pointer truncate text-[12.5px] font-bold text-foreground transition-colors hover:text-primary" @click="openLoc(a.id)">{{ a.name }}</button>
              <span v-if="a.current" class="shrink-0 rounded-full border border-primary/45 bg-primary/12 px-1.5 text-[10px] font-bold text-primary">当前</span>
              <span v-if="a.chapter" class="shrink-0 rounded-full border border-warning/45 bg-warning/12 px-1.5 text-[10px] font-bold text-warning" title="属于当前章节范围（该章场景的 location_id 并集）">本章</span>
              <span v-if="!a.pinned" class="shrink-0 rounded-full bg-muted px-1.5 text-[10px] font-medium text-muted-foreground/70">无锚点</span>
              <span class="ml-auto shrink-0 font-mono text-[10.5px] text-muted-foreground/70">{{ a.tally.total }}</span>
            </div>

            <!-- 剧情标记：任务（场景推导） / 遭遇（创建快照） -->
            <div v-if="a.story.quests || a.story.encounters" class="mt-1.5 flex flex-wrap items-center gap-1">
              <span v-if="a.story.quests" class="inline-flex items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-1.5 py-px text-[10.5px] font-bold text-primary" :title="storyTitle(a.id)">
                <IconTarget class="size-3" />任务 {{ a.story.quests }}<span class="font-normal text-primary/80">· 未完成 {{ a.story.openQuests }}</span>
              </span>
              <span v-if="a.story.encounters" class="inline-flex items-center gap-1 rounded-full border border-destructive/40 bg-destructive/10 px-1.5 py-px text-[10.5px] font-bold text-destructive" :title="storyTitle(a.id)">
                <IconSwords class="size-3" />遭遇 {{ a.story.encounters }}<span class="font-normal text-destructive/80">· {{ a.story.enemies }} 敌</span>
              </span>
            </div>

            <p v-if="!a.tally.total" class="mt-1.5 text-[11px] text-muted-foreground/60">此处暂无单位（没有实例把位置写在这里）。</p>
            <div v-else class="mt-1.5 flex flex-wrap gap-1">
              <button
                v-for="c in a.units.pc.concat(a.units.npc).concat(a.units.monster)"
                :key="c.instance_id"
                type="button"
                class="inline-flex max-w-full cursor-pointer items-center gap-1 rounded-full border px-1.5 py-0.5 text-[11px] font-medium transition-shadow hover:shadow-sm"
                :class="[chipClass(c.kind), isSelChar(c.instance_id) ? 'ring-2 ring-primary/40' : '', c.present ? '' : 'opacity-60']"
                :title="kindLabel(c.kind) + ' · ' + c.name + (c.present ? '（在场）' : '（不在场）') + ' · 点开数据卡'"
                @click="openChar(c)"
              >
                <span class="flex size-4 shrink-0 items-center justify-center overflow-hidden rounded-full text-[9px] font-extrabold" :class="nameTintClass(c.name)">
                  <img v-if="portraitUrl(c)" :src="portraitUrl(c)" :alt="c.name" class="size-full object-cover" loading="lazy" decoding="async" />
                  <template v-else>{{ initial(c.name) }}</template>
                </span>
                <span class="truncate">{{ c.name }}</span>
                <span class="shrink-0 text-[9.5px] opacity-80">{{ kindLabel(c.kind) }}</span>
                <span v-if="isControlled(c)" class="shrink-0 text-[9.5px] font-bold">你</span>
                <IconChevronRight class="size-3 shrink-0 opacity-60" />
              </button>
              <!-- 没有实例的临时敌人：自身地点优先，缺省用遭遇地点；只能看不能点开 -->
              <span
                v-for="e in a.units.loose"
                :key="e.id"
                class="inline-flex max-w-full items-center gap-1 rounded-full border border-dashed border-destructive/50 bg-destructive/10 px-1.5 py-0.5 text-[11px] font-medium text-destructive"
                :title="'临时敌人（没有实例）· 来自遭遇「' + e.encounter + '」'"
              >
                <IconSwords class="size-3 shrink-0" />
                <span class="truncate">{{ e.name }}</span>
                <span class="shrink-0 text-[9.5px] opacity-80">临时</span>
              </span>
            </div>
          </div>
        </div>
      </div>

      <p class="mt-3 flex items-center gap-1.5 text-[10.5px] text-muted-foreground/60">
        <IconUsers class="size-3.5 shrink-0" />
        <span>单位按角色实例的 <span class="font-mono">location_id</span> 归位（实例没写时用遭遇快照的地点兜底）：PC / NPC / 怪物三色区分，怪物只在遭遇与这里出现。</span>
      </p>
    </div>
  </div>
</template>
