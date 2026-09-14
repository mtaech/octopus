<script setup lang="ts">
// 详情栏（A 模板右侧列）：显示左栏选中对象的信息；未选中时显示受控角色。
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import CharacterSheet from './CharacterSheet.vue'
import { assetUrl } from '@/api'
import type { CharacterInstance, EncounterView, EnemyView, GoalDef, LocationDef, SceneDef, Storybook, TriggerDef } from '@/types'
import type { WorldSelection } from '../selection'
import { findCharacter, initial, isControlledChar, nameTintClass, portraitOf, unitCount, unitsAtLocation } from '../utils'
import type { LooseEnemy, LocationUnits } from '../utils'
import { IconTarget, IconFlag, IconMapPin, IconInfoCircle, IconUser, IconCheck, IconSwords, IconShield, IconChevronRight, IconBook2, IconSkull } from '@tabler/icons-vue'

const props = defineProps<{ selected: WorldSelection | null }>()
const emit = defineEmits<{
  (e: 'switch', characterId: string): void
  (e: 'select', s: WorldSelection): void
}>()

const store = usePlayStore()
/** 当前场景（骨架里按 scene_id 找） */
const scene = computed<SceneDef | null>(() => {
  const p = store.projection
  const sk = store.detail?.storybook.skeleton
  if (!p || !sk) return null
  for (const ch of sk) for (const sc of ch.scenes) if (sc.id === p.scene_id) return sc
  return null
})

/** 地点详情里的一行单位（PC / NPC / 怪物排成一列，只靠 kind 区分）。 */
interface LocUnitRow { instanceId: string; name: string; kind: string; controlled: boolean; present: boolean }

function locRows(u: LocationUnits): LocUnitRow[] {
  const rows: LocUnitRow[] = []
  const push = (list: CharacterInstance[]): void => {
    for (const c of list) {
      rows.push({ instanceId: c.instance_id, name: c.name, kind: c.kind, controlled: isControlledChar(store.controlledId, c), present: c.present })
    }
  }
  push(u.pc); push(u.npc); push(u.monster)
  return rows
}

type Detail =
  | { kind: 'character'; actor: CharacterInstance }
  | { kind: 'goal'; goal: GoalDef; done: boolean }
  | { kind: 'trigger'; trigger: TriggerDef; done: boolean }
  | { kind: 'location'; loc: LocationDef; current: boolean; rows: LocUnitRow[]; loose: LooseEnemy[]; total: number }
  | { kind: 'encounter'; enc: EncounterView }
  | null

/** 存档内嵌的冻结故事书：立绘 / 怪物数据卡都从这里取 */
const sb = computed<Storybook | null>(() => store.detail?.storybook ?? null)

const detail = computed<Detail>(() => {
  const p = store.projection
  if (!p) return null
  const sel = props.selected
  if (sel?.kind === 'goal') {
    const g = scene.value?.goals.find(x => x.id === sel.id)
    if (g) return { kind: 'goal', goal: g, done: Boolean(p.progress.goals[g.id]) }
    // 导演运行时新增的任务（不在骨架里）
    const q = (p.quests ?? []).find(x => x.id === sel.id)
    if (q) return { kind: 'goal', goal: { id: q.id, text: q.text, primary: q.primary, hidden: q.hidden }, done: q.done }
    return null
  }
  if (sel?.kind === 'trigger') {
    const t = scene.value?.triggers.find(x => x.id === sel.id)
    return t ? { kind: 'trigger', trigger: t, done: Boolean(p.progress.triggers[t.id]) } : null
  }
  if (sel?.kind === 'encounter') {
    const e = (p.encounters ?? []).find(x => x.id === sel.id)
    return e ? { kind: 'encounter', enc: e } : null
  }
  if (sel?.kind === 'location') {
    const l = p.locations.find(x => x.id === sel.id)
    if (!l) return null
    // 归属按实例 location_id（地图与在场 §6.7），实例没写时用遭遇快照的地点兜底（见 unitsAtLocation）
    // ——不再给「当前场景地点」留近似后门。
    const units = unitsAtLocation(p, l.id)
    return {
      kind: 'location',
      loc: l,
      current: l.id === (scene.value?.location_id ?? ''),
      rows: locRows(units),
      loose: units.loose,
      total: unitCount(units),
    }
  }
  const id = sel?.kind === 'character' ? sel.id : store.controlledId
  const a = findCharacter(p, id)
  return a ? { kind: 'character', actor: a } : null
})

/** 骨架里按 id 找场景（遭遇快照的 scene_id 可能已经不是当前场景）。 */
function sceneById(id?: string | null): SceneDef | null {
  if (!id) return null
  for (const ch of store.detail?.storybook.skeleton ?? []) {
    for (const sc of ch.scenes) if (sc.id === id) return sc
  }
  return null
}
/** 遭遇快照的场景标题：骨架优先，当前场景用投影标题兜底，其余回落 id。 */
function sceneTitleOf(id?: string | null): string {
  if (!id) return ''
  const sc = sceneById(id)
  if (sc) return sc.title
  return id === store.projection?.scene_id ? (store.projection?.scene_title ?? id) : id
}
function locNameOf(id?: string | null): string {
  if (!id) return ''
  return store.projection?.locations.find(l => l.id === id)?.name ?? id
}
function locKnown(id?: string | null): boolean {
  return !!id && (store.projection?.locations ?? []).some(l => l.id === id)
}
/** 关联任务（goal_id）：运行时任务优先，其次骨架目标，最后回落 id。 */
function goalOf(id?: string | null): { text: string; done: boolean } | null {
  if (!id) return null
  const q = (store.projection?.quests ?? []).find(x => x.id === id)
  if (q) return { text: q.text, done: q.done }
  for (const ch of store.detail?.storybook.skeleton ?? []) {
    for (const sc of ch.scenes) {
      const g = sc.goals.find(x => x.id === id)
      if (g) return { text: g.text, done: Boolean(store.projection?.progress.goals[id]) }
    }
  }
  return { text: id, done: false }
}
/** 遭遇引用的图鉴条目（template_ids）：名字与是否怪物来自冻结故事书。 */
function templateOf(id: string): { id: string; name: string; monster: boolean } {
  const c = sb.value?.characters.find(x => x.id === id)
  return { id, name: c?.name ?? id, monster: c?.kind === 'monster' }
}

const kindLabelText = computed(() => {
  const d = detail.value
  if (!d) return ''
  return d.kind === 'character' ? '人物'
    : d.kind === 'goal' ? '任务'
    : d.kind === 'trigger' ? '触发点'
    : d.kind === 'encounter' ? '遭遇'
    : '地点'
})

// ---- 遭遇里的敌方单位：有实例的走数据卡，临时敌人（无 instance_id）只读 ----
function enemyActor(en: EnemyView): CharacterInstance | null {
  if (!en.instance_id) return null
  return findCharacter(store.projection, en.instance_id)
}
function enemySelected(en: EnemyView): boolean {
  const a = enemyActor(en)
  return !!a && props.selected?.kind === 'character' && props.selected.id === a.instance_id
}
function openEnemy(en: EnemyView): void {
  const a = enemyActor(en)
  if (a) emit('select', { kind: 'character', id: a.instance_id })
}
function portraitUrl(en: EnemyView): string {
  const p = portraitOf(sb.value, en.template_id ?? undefined, en.name)
  return p ? assetUrl(p.asset) : ''
}
function hpWidth(en: EnemyView): string {
  return en.max > 0 ? Math.max(0, Math.min(100, (en.hp / en.max) * 100)) + '%' : '0%'
}
</script>

<template>
  <aside class="w-[300px] shrink-0 overflow-y-auto border-l border-border bg-card/50 p-3">
    <div class="mb-3 flex items-center gap-1.5 border-b border-border/80 px-0.5 pb-2">
      <IconInfoCircle class="size-3.5 text-primary" />
      <span class="text-[11px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">详情</span>
      <span v-if="kindLabelText" class="ml-auto rounded-full border border-border bg-muted/60 px-2 py-0.5 text-[10.5px] font-bold text-muted-foreground">{{ kindLabelText }}</span>
    </div>

    <!-- 人物：只读角色卡 -->
    <template v-if="detail?.kind === 'character'">
      <CharacterSheet
        :actor="detail.actor"
        :controlled="isControlledChar(store.controlledId, detail.actor)"
        @switch="(id: string) => emit('switch', id)"
      />
    </template>

    <!-- 任务 -->
    <template v-else-if="detail?.kind === 'goal'">
      <div class="rounded-xl border border-border/80 bg-card/80 p-3">
        <div class="flex items-start gap-2">
          <IconTarget class="mt-0.5 size-4 shrink-0 text-primary" />
          <span class="text-[13px] leading-relaxed font-bold text-foreground">{{ detail.goal.text }}</span>
        </div>
        <div class="mt-2.5 flex flex-wrap gap-1.5">
          <span class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10.5px] font-bold" :class="detail.done ? 'bg-success/15 text-success' : 'bg-warning/15 text-warning'">
            <IconCheck v-if="detail.done" class="size-3" />
            {{ detail.done ? '已完成' : '进行中' }}
          </span>
          <span v-if="detail.goal.primary" class="rounded-full bg-primary/12 px-2 py-0.5 text-[10.5px] font-bold text-primary">主线</span>
          <span v-if="detail.goal.hidden" class="rounded-full bg-muted px-2 py-0.5 text-[10.5px] font-bold text-muted-foreground">隐藏目标</span>
        </div>
        <p v-if="scene" class="mt-2.5 text-[11.5px] text-muted-foreground/70">所属场景 · {{ scene.title }}</p>
      </div>
    </template>

    <!-- 触发点 -->
    <template v-else-if="detail?.kind === 'trigger'">
      <div class="rounded-xl border border-border/80 bg-card/80 p-3">
        <div class="flex items-start gap-2">
          <IconFlag class="mt-0.5 size-4 shrink-0" :class="detail.done ? 'text-success' : 'text-warning'" />
          <span class="text-[13px] leading-relaxed font-bold text-foreground">{{ detail.trigger.title }}</span>
        </div>
        <span class="mt-2.5 inline-flex rounded-full px-2 py-0.5 text-[10.5px] font-bold" :class="detail.done ? 'bg-success/15 text-success' : 'bg-muted text-muted-foreground'">
          {{ detail.done ? '已触发' : '未触发' }}
        </span>
        <p v-if="detail.trigger.description" class="mt-2.5 text-[12px] leading-relaxed text-foreground/85">{{ detail.trigger.description }}</p>
        <p v-if="detail.trigger.hint" class="mt-2 text-[11.5px] leading-relaxed text-muted-foreground/80">
          <span class="font-bold">演绎提示 · </span>{{ detail.trigger.hint }}
        </p>
        <p v-if="scene" class="mt-2.5 text-[11.5px] text-muted-foreground/70">所属场景 · {{ scene.title }}</p>
      </div>
    </template>

    <!-- 遭遇 -->
    <template v-else-if="detail?.kind === 'encounter'">
      <div class="rounded-xl border border-border/80 bg-card/80 p-3">
        <div class="flex items-start gap-2">
          <IconSwords class="mt-0.5 size-4 shrink-0 text-destructive" />
          <span class="text-[13px] leading-relaxed font-bold text-foreground">{{ detail.enc.name }}</span>
          <span class="ml-auto shrink-0 rounded-full px-1.5 text-[10px] font-bold" :class="detail.enc.active ? 'bg-destructive/15 text-destructive' : 'bg-muted text-muted-foreground/70'">
            {{ detail.enc.active ? '进行中' : '已结束' }}
          </span>
        </div>
        <p v-if="detail.enc.note" class="mt-2 text-[12px] leading-relaxed text-foreground/85">{{ detail.enc.note }}</p>

        <!-- 叙事与空间锚（地图 P5 §6.4）：创建时的**快照**，不在运行时重新推导 -->
        <div v-if="detail.enc.scene_id || detail.enc.location_id || detail.enc.goal_id" class="mt-2.5 space-y-1 border-t border-border/50 pt-2">
          <div v-if="detail.enc.scene_id" class="flex items-start gap-1.5 text-[11.5px]">
            <IconBook2 class="mt-0.5 size-3.5 shrink-0 text-muted-foreground/60" />
            <span class="text-muted-foreground/70">所属场景</span>
            <span class="min-w-0 flex-1 truncate font-semibold text-foreground/90" :title="detail.enc.scene_id">{{ sceneTitleOf(detail.enc.scene_id) }}</span>
          </div>
          <div v-if="detail.enc.location_id" class="flex items-start gap-1.5 text-[11.5px]">
            <IconMapPin class="mt-0.5 size-3.5 shrink-0 text-info" />
            <span class="text-muted-foreground/70">发生地点</span>
            <button
              v-if="locKnown(detail.enc.location_id)"
              type="button"
              class="min-w-0 max-w-full cursor-pointer truncate font-semibold text-info transition-colors hover:text-primary hover:underline"
              title="点开该地点的详情"
              @click="emit('select', { kind: 'location', id: detail.enc.location_id })"
            >{{ locNameOf(detail.enc.location_id) }}</button>
            <span v-else class="min-w-0 flex-1 truncate font-semibold text-muted-foreground/70" title="地点已不存在">{{ locNameOf(detail.enc.location_id) }}（地点已不存在）</span>
          </div>
          <div v-if="detail.enc.goal_id" class="flex items-start gap-1.5 text-[11.5px]">
            <IconTarget class="mt-0.5 size-3.5 shrink-0 text-primary" />
            <span class="text-muted-foreground/70">关联任务</span>
            <button
              type="button"
              class="min-w-0 max-w-full cursor-pointer truncate text-left font-semibold text-foreground/90 transition-colors hover:text-primary"
              title="点开该任务的详情"
              @click="emit('select', { kind: 'goal', id: detail.enc.goal_id })"
            >{{ goalOf(detail.enc.goal_id)?.text }}</button>
            <span v-if="goalOf(detail.enc.goal_id)?.done" class="shrink-0 rounded-full bg-success/15 px-1.5 text-[10px] font-bold text-success">已完成</span>
          </div>
        </div>

        <!-- 涉及的图鉴条目（template_ids）：临时敌人不计入 -->
        <div v-if="detail.enc.template_ids?.length" class="mt-2 flex flex-wrap items-center gap-1">
          <span class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">图鉴</span>
          <span
            v-for="tid in detail.enc.template_ids"
            :key="tid"
            class="inline-flex max-w-full items-center gap-1 rounded-full border px-1.5 py-px text-[10.5px] font-semibold"
            :class="templateOf(tid).monster ? 'border-warning/45 bg-warning/10 text-warning' : 'border-border bg-muted/60 text-foreground/80'"
            :title="tid"
          >
            <IconSkull v-if="templateOf(tid).monster" class="size-3 shrink-0" />
            <span class="truncate">{{ templateOf(tid).name }}</span>
          </span>
        </div>
      </div>
      <div class="mt-3">
        <div class="mb-1.5 text-[10.5px] font-bold tracking-wider text-muted-foreground/60 uppercase">敌方单位 · {{ detail.enc.enemies.length }}</div>
        <p v-if="!detail.enc.enemies.length" class="text-[11.5px] text-muted-foreground/60">这个遭遇没有登记敌人。</p>
        <div class="space-y-2">
          <!-- 带 instance_id 的敌方单位：立绘 + AC + HP 血条，点开就是它的数据卡（图鉴 M2 后数据完整） -->
          <button
            v-for="en in detail.enc.enemies"
            :key="en.id"
            type="button"
            :disabled="!enemyActor(en)"
            :title="enemyActor(en) ? '查看 ' + en.name + ' 的数据卡' : en.name + '（临时敌人：没有实例，不能查看数据卡）'"
            class="w-full rounded-lg border p-2 text-left transition-colors"
            :class="enemyActor(en)
              ? [enemySelected(en) ? 'border-primary/60 bg-primary/10' : 'border-border/70 bg-card/60 hover:border-primary/40 hover:bg-muted/40', 'cursor-pointer']
              : 'cursor-default border-border/60 bg-muted/25'"
            @click="openEnemy(en)"
          >
            <div class="flex items-center gap-2">
              <span class="flex size-9 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-border/70">
                <img v-if="portraitUrl(en)" :src="portraitUrl(en)" :alt="en.name" class="size-full object-cover" loading="lazy" decoding="async" />
                <span v-else class="text-[13px] font-extrabold" :class="nameTintClass(en.name)">{{ initial(en.name) }}</span>
              </span>
              <span class="min-w-0 flex-1">
                <span class="flex items-baseline gap-1.5">
                  <span class="truncate text-[12.5px] font-semibold" :class="en.hp <= 0 ? 'text-muted-foreground/60 line-through' : 'text-foreground/95'">{{ en.name }}</span>
                  <span class="font-mono text-[10px] text-muted-foreground/50">{{ en.id }}</span>
                  <span v-if="en.template_id" class="shrink-0 rounded-full bg-warning/15 px-1.5 text-[9.5px] font-bold text-warning">怪物</span>
                  <span v-else class="shrink-0 rounded-full bg-muted px-1.5 text-[9.5px] font-bold text-muted-foreground/70">临时</span>
                </span>
                <span class="mt-1 flex items-center gap-1.5">
                  <span class="h-1 min-w-0 flex-1 overflow-hidden rounded-full bg-muted">
                    <span class="block h-full rounded-full bg-destructive transition-all" :style="{ width: hpWidth(en) }"></span>
                  </span>
                  <span class="shrink-0 font-mono text-[10.5px] font-bold" :class="en.hp <= 0 ? 'text-muted-foreground/50' : 'text-destructive'">
                    {{ en.hp }}<span class="font-normal text-muted-foreground/50">/{{ en.max }}</span>
                  </span>
                </span>
              </span>
              <span class="flex shrink-0 flex-col items-end gap-1">
                <span class="inline-flex items-center gap-1 rounded-full border border-border bg-muted/50 px-1.5 py-px text-[10px] font-bold text-muted-foreground" :title="'护甲等级 AC ' + en.ac">
                  <IconShield class="size-3" />AC {{ en.ac }}
                </span>
                <span v-if="enemyActor(en)" class="inline-flex items-center text-[10px] font-bold text-primary">数据卡<IconChevronRight class="size-3" /></span>
                <span v-else class="text-[10px] text-muted-foreground/50">无实例</span>
              </span>
            </div>
          </button>
        </div>
      </div>
    </template>

    <!-- 地点 -->
    <template v-else-if="detail?.kind === 'location'">
      <div class="rounded-xl border border-border/80 bg-card/80 p-3">
        <div class="flex items-start gap-2">
          <IconMapPin class="mt-0.5 size-4 shrink-0 text-info" />
          <span class="text-[13px] leading-relaxed font-bold text-foreground">{{ detail.loc.name }}</span>
          <span v-if="detail.current" class="ml-auto shrink-0 rounded-full border border-primary/45 bg-primary/12 px-1.5 text-[10px] font-bold text-primary">当前</span>
        </div>
        <p v-if="detail.loc.description" class="mt-2.5 text-[12px] leading-relaxed text-foreground/85">{{ detail.loc.description }}</p>
        <p v-else class="mt-2.5 text-[11.5px] text-muted-foreground/60">这个地点还没有描述。</p>
        <p class="mt-2 font-mono text-[10.5px] text-muted-foreground/50">{{ detail.loc.id }}</p>
      </div>
      <div class="mt-3">
        <div class="mb-1.5 text-[10.5px] font-bold tracking-wider text-muted-foreground/60 uppercase">此处角色 · {{ detail.total }}</div>
        <p v-if="!detail.total" class="text-[11.5px] text-muted-foreground/60">按实例位置统计，这里暂时没有角色。</p>
        <div v-for="r in detail.rows" :key="r.instanceId" class="flex items-center gap-2 py-0.5 text-[12px]">
          <IconUser class="size-3.5 shrink-0" :class="r.kind === 'monster' ? 'text-destructive/70' : 'text-muted-foreground/60'" />
          <span class="truncate" :class="r.present ? 'text-foreground/90' : 'text-muted-foreground/60'">{{ r.name }}</span>
          <span v-if="r.kind === 'monster'" class="shrink-0 rounded-full bg-destructive/12 px-1.5 text-[10px] font-bold text-destructive">怪物</span>
          <span v-else-if="r.kind === 'npc'" class="shrink-0 rounded-full bg-muted px-1.5 text-[10px] font-bold text-muted-foreground/70">NPC</span>
          <span v-if="r.controlled" class="ml-auto shrink-0 rounded-full bg-primary/12 px-1.5 text-[10px] font-bold text-primary">你</span>
        </div>
        <div v-for="e in detail.loose" :key="e.id" class="flex items-center gap-2 py-0.5 text-[12px]">
          <IconSwords class="size-3.5 shrink-0 text-destructive/70" />
          <span class="truncate text-muted-foreground/80">{{ e.name }}</span>
          <span class="shrink-0 rounded-full border border-dashed border-destructive/50 px-1.5 text-[10px] font-bold text-destructive/90">临时</span>
        </div>
      </div>
    </template>

    <p v-else class="px-1 text-[12px] leading-relaxed text-muted-foreground/70">
      从左栏选择任务 / 触发点 / 地点 / 人物，这里会显示详细信息。
    </p>
  </aside>
</template>
