<script setup lang="ts">
// 世界栏（A 模板左侧列）：任务 / 触发点 / 地点 / 在场人物 —— 点一行，右栏看详情。
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import type { SceneDef } from '@/types'
import type { WorldSelection } from '../selection'
import { isControlledChar, unitTally, unitsAtLocation } from '../utils'
import { IconTarget, IconFlag, IconMapPin, IconUsers, IconCheck, IconCircle, IconSwords } from '@tabler/icons-vue'

const props = defineProps<{ selected: WorldSelection | null }>()
const emit = defineEmits<{ (e: 'select', s: WorldSelection): void }>()

const store = usePlayStore()

/** 当前场景（骨架里按 scene_id 找）：任务与触发点来自它 */
const scene = computed<SceneDef | null>(() => {
  const p = store.projection
  const sk = store.detail?.storybook.skeleton
  if (!p || !sk) return null
  for (const ch of sk) for (const sc of ch.scenes) if (sc.id === p.scene_id) return sc
  return null
})
const goals = computed(() => scene.value?.goals ?? [])
const triggers = computed(() => scene.value?.triggers ?? [])
const locations = computed(() => store.projection?.locations ?? [])
/** 左栏「在场人物」**不含怪物**：怪物只在遭遇卡片 / 地图面板里出现（图鉴 §5.3、地图 §7）。 */
const chars = computed(() => store.presentChars.filter(c => c.kind !== 'monster'))
const currentLoc = computed(() => scene.value?.location_id ?? '')
const goalDone = (id: string) => Boolean(store.projection?.progress.goals[id])
const triggerDone = (id: string) => Boolean(store.projection?.progress.triggers[id])
/** 导演（人当 GM）运行时新增的任务 */
const gmQuests = computed(() => (store.projection?.quests ?? []).filter(q => q.source === 'gm'))
/** 进行中的结构化遭遇（导演创建） */
const encounters = computed(() => (store.projection?.encounters ?? []).filter(e => e.active))
const doneGm = computed(() => gmQuests.value.filter(q => q.done).length)
const doneGoals = computed(() => goals.value.filter(g => goalDone(g.id)).length)
const doneTriggers = computed(() => triggers.value.filter(t => triggerDone(t.id)).length)
const isSel = (kind: WorldSelection['kind'], id: string) =>
  props.selected?.kind === kind && props.selected?.id === id
/**
 * 地点栏人数：按**实例位置**（characters[].location_id）真实统计——替换原先按在场列表过滤
 * 的写法（NPC 实例过去没有 location_id，那里恒为 0）。怪物实例与未落实例的临时敌人分开计。
 */
const EMPTY_STAT = { people: 0, monsters: 0, total: 0 }
const locStats = computed<Record<string, { people: number; monsters: number; total: number }>>(() => {
  const out: Record<string, { people: number; monsters: number; total: number }> = {}
  for (const l of locations.value) out[l.id] = unitTally(unitsAtLocation(store.projection, l.id))
  return out
})
function stat(locId: string): { people: number; monsters: number; total: number } {
  return locStats.value[locId] ?? EMPTY_STAT
}

const rowBase = 'flex w-full cursor-pointer items-center gap-2 rounded-lg px-1.5 py-1.5 text-left text-[12.5px] text-foreground transition-colors hover:bg-muted/60'
const rowOn = 'bg-primary/12 text-primary hover:bg-primary/15'
const headBase = 'mb-1.5 flex items-center gap-1.5 border-b border-border/80 px-0.5 pb-1.5 text-[11px] font-extrabold tracking-[1.2px] text-muted-foreground uppercase'
const countBase = 'ml-auto font-mono text-[10.5px] font-bold text-muted-foreground/80'
const tagBase = 'shrink-0 rounded-full border border-primary/45 bg-primary/12 px-1.5 text-[10px] font-bold text-primary'
const textBase = 'min-w-0 flex-1 leading-[1.35]'
</script>

<template>
  <aside class="w-[240px] shrink-0 overflow-y-auto border-r border-border bg-card/45 p-3">
    <!-- 任务 -->
    <section class="mb-3.5">
      <header :class="headBase">
        <IconTarget class="size-3.5 text-primary" />
        <span>任务</span>
        <span :class="countBase">{{ doneGoals + doneGm }}/{{ goals.length + gmQuests.length }}</span>
      </header>
      <p v-if="!goals.length && !gmQuests.length" class="px-1 py-1.5 text-[11.5px] text-muted-foreground/70">本场景暂无任务</p>
      <button
        v-for="g in goals"
        :key="g.id"
        type="button"
        :class="[rowBase, isSel('goal', g.id) ? rowOn : '']"
        @click="emit('select', { kind: 'goal', id: g.id })"
      >
        <IconCheck v-if="goalDone(g.id)" class="size-3.5 shrink-0 text-success" />
        <IconCircle v-else class="size-3.5 shrink-0 text-muted-foreground/50" />
        <span :class="[textBase, goalDone(g.id) ? 'text-muted-foreground/80 line-through' : '']">{{ g.text }}</span>
        <span v-if="g.primary" :class="tagBase">主线</span>
      </button>

      <!-- 导演新增的运行时任务 -->
      <button
        v-for="q in gmQuests"
        :key="q.id"
        type="button"
        :class="[rowBase, isSel('goal', q.id) ? rowOn : '']"
        @click="emit('select', { kind: 'goal', id: q.id })"
      >
        <IconCheck v-if="q.done" class="size-3.5 shrink-0 text-success" />
        <IconCircle v-else class="size-3.5 shrink-0 text-info" />
        <span :class="[textBase, q.done ? 'text-muted-foreground/80 line-through' : '']">{{ q.text }}</span>
        <span class="shrink-0 rounded-full border border-info/45 bg-info/12 px-1.5 text-[10px] font-bold text-info">导演</span>
      </button>
    </section>

    <!-- 触发点 -->
    <section v-if="triggers.length" class="mb-3.5">
      <header :class="headBase">
        <IconFlag class="size-3.5 text-warning" />
        <span>触发点</span>
        <span :class="countBase">{{ doneTriggers }}/{{ triggers.length }}</span>
      </header>
      <button
        v-for="t in triggers"
        :key="t.id"
        type="button"
        :class="[rowBase, isSel('trigger', t.id) ? rowOn : '']"
        @click="emit('select', { kind: 'trigger', id: t.id })"
      >
        <IconFlag class="size-3.5 shrink-0" :class="triggerDone(t.id) ? 'text-success' : 'text-muted-foreground/50'" />
        <span :class="[textBase, triggerDone(t.id) ? 'text-muted-foreground/80' : '']">{{ t.title }}</span>
      </button>
    </section>

    <!-- 遭遇 -->
    <section v-if="encounters.length" class="mb-3.5">
      <header :class="headBase">
        <IconSwords class="size-3.5 text-destructive" />
        <span>遭遇</span>
        <span :class="countBase">{{ encounters.length }}</span>
      </header>
      <button
        v-for="e in encounters"
        :key="e.id"
        type="button"
        :class="[rowBase, isSel('encounter', e.id) ? rowOn : '']"
        @click="emit('select', { kind: 'encounter', id: e.id })"
      >
        <IconSwords class="size-3.5 shrink-0 text-destructive" />
        <span :class="textBase">{{ e.name }}</span>
        <span :class="countBase">{{ e.enemies.length }} 敌</span>
      </button>
    </section>

    <!-- 地点 -->
    <section class="mb-3.5">
      <header :class="headBase">
        <IconMapPin class="size-3.5 text-info" />
        <span>地点</span>
        <span :class="countBase">{{ locations.length }}</span>
      </header>
      <p v-if="!locations.length" class="px-1 py-1.5 text-[11.5px] text-muted-foreground/70">暂无地点</p>
      <button
        v-for="l in locations"
        :key="l.id"
        type="button"
        :class="[rowBase, isSel('location', l.id) ? rowOn : '']"
        @click="emit('select', { kind: 'location', id: l.id })"
      >
        <IconMapPin class="size-3.5 shrink-0" :class="l.id === currentLoc ? 'text-primary' : 'text-muted-foreground/50'" />
        <span :class="textBase">{{ l.name }}</span>
        <span v-if="l.id === currentLoc" :class="tagBase">当前</span>
        <span
          v-if="stat(l.id).total"
          :class="countBase"
          :title="'此地点：' + stat(l.id).people + ' 人 · ' + stat(l.id).monsters + ' 怪（按实例位置统计）'"
        ><template v-if="stat(l.id).people">{{ stat(l.id).people }} 人</template><span v-if="stat(l.id).monsters" :class="stat(l.id).people ? 'ml-0.5 text-destructive' : 'text-destructive'">{{ stat(l.id).monsters }} 怪</span></span>
      </button>
    </section>

    <!-- 在场人物 -->
    <section>
      <header :class="headBase">
        <IconUsers class="size-3.5 text-primary" />
        <span>在场人物</span>
        <span :class="countBase">{{ chars.length }}</span>
      </header>
      <p v-if="!chars.length" class="px-1 py-1.5 text-[11.5px] text-muted-foreground/70">现场没有人物</p>
      <button
        v-for="c in chars"
        :key="c.instance_id"
        type="button"
        :class="[rowBase, isSel('character', c.instance_id) ? rowOn : '']"
        @click="emit('select', { kind: 'character', id: c.instance_id })"
      >
        <span class="flex size-[18px] shrink-0 items-center justify-center rounded-full bg-primary/15 text-[10px] font-extrabold text-primary">{{ c.name.slice(0, 1) }}</span>
        <span :class="textBase">{{ c.name }}</span>
        <span v-if="isControlledChar(store.controlledId, c)" :class="tagBase">你</span>
        <span v-else-if="c.kind === 'npc'" :class="countBase">NPC</span>
      </button>
    </section>
  </aside>
</template>
