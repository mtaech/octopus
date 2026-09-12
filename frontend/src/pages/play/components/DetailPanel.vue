<script setup lang="ts">
// 详情栏（A 模板右侧列）：显示左栏选中对象的信息；未选中时显示受控角色。
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import CharacterSheet from './CharacterSheet.vue'
import type { CharacterInstance, EncounterView, GoalDef, LocationDef, SceneDef, TriggerDef } from '@/types'
import type { WorldSelection } from '../selection'
import { findCharacter, isControlledChar } from '../utils'
import { IconTarget, IconFlag, IconMapPin, IconInfoCircle, IconUser, IconCheck, IconSwords } from '@tabler/icons-vue'

const props = defineProps<{ selected: WorldSelection | null }>()
const emit = defineEmits<{ (e: 'switch', characterId: string): void }>()

const store = usePlayStore()
/** 当前场景（骨架里按 scene_id 找） */
const scene = computed<SceneDef | null>(() => {
  const p = store.projection
  const sk = store.detail?.storybook.skeleton
  if (!p || !sk) return null
  for (const ch of sk) for (const sc of ch.scenes) if (sc.id === p.scene_id) return sc
  return null
})

type Detail =
  | { kind: 'character'; actor: CharacterInstance }
  | { kind: 'goal'; goal: GoalDef; done: boolean }
  | { kind: 'trigger'; trigger: TriggerDef; done: boolean }
  | { kind: 'location'; loc: LocationDef; current: boolean; chars: CharacterInstance[] }
  | { kind: 'encounter'; enc: EncounterView }
  | null

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
    const current = l.id === (scene.value?.location_id ?? '')
    // 当前场景地点：在场人物都在这里（实例的 location_id 可能没写）。
    // 其它地点按实例的 location_id 归属。
    return {
      kind: 'location',
      loc: l,
      current,
      chars: current ? store.presentChars : store.presentChars.filter(c => c.location_id === l.id),
    }
  }
  const id = sel?.kind === 'character' ? sel.id : store.controlledId
  const a = findCharacter(p, id)
  return a ? { kind: 'character', actor: a } : null
})

const kindLabel = computed(() => {
  const d = detail.value
  if (!d) return ''
  return d.kind === 'character' ? '人物'
    : d.kind === 'goal' ? '任务'
    : d.kind === 'trigger' ? '触发点'
    : d.kind === 'encounter' ? '遭遇'
    : '地点'
})
</script>

<template>
  <aside class="w-[300px] shrink-0 overflow-y-auto border-l border-border bg-card/50 p-3">
    <div class="mb-3 flex items-center gap-1.5 border-b border-border/80 px-0.5 pb-2">
      <IconInfoCircle class="size-3.5 text-primary" />
      <span class="text-[11px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">详情</span>
      <span v-if="kindLabel" class="ml-auto rounded-full border border-border bg-muted/60 px-2 py-0.5 text-[10.5px] font-bold text-muted-foreground">{{ kindLabel }}</span>
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
        </div>
        <p v-if="detail.enc.note" class="mt-2 text-[12px] leading-relaxed text-foreground/85">{{ detail.enc.note }}</p>
      </div>
      <div class="mt-3">
        <div class="mb-1.5 text-[10.5px] font-bold tracking-wider text-muted-foreground/60 uppercase">敌方单位 · {{ detail.enc.enemies.length }}</div>
        <p v-if="!detail.enc.enemies.length" class="text-[11.5px] text-muted-foreground/60">这个遭遇没有登记敌人。</p>
        <div class="space-y-2">
          <div v-for="(en, i) in detail.enc.enemies" :key="i">
            <div class="flex items-baseline justify-between gap-2 text-[12px]">
              <span class="truncate text-foreground/90">{{ en.name }}<span class="ml-1.5 font-mono text-[10px] text-muted-foreground/50">{{ en.id }}</span></span>
              <span class="shrink-0 font-mono font-bold" :class="en.hp <= 0 ? 'text-muted-foreground/50 line-through' : 'text-destructive'">
                {{ en.hp }}<span class="font-normal text-muted-foreground/50">/{{ en.max }}</span>
              </span>
            </div>
            <div class="mt-0.5 h-1 overflow-hidden rounded-full bg-muted">
              <div class="h-full rounded-full bg-destructive transition-all" :style="{ width: en.max > 0 ? Math.max(0, Math.min(100, (en.hp / en.max) * 100)) + '%' : '0%' }"></div>
            </div>
          </div>
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
        <div class="mb-1.5 text-[10.5px] font-bold tracking-wider text-muted-foreground/60 uppercase">此处人物 · {{ detail.chars.length }}</div>
        <p v-if="!detail.chars.length" class="text-[11.5px] text-muted-foreground/60">这里暂时没有人。</p>
        <div v-for="c in detail.chars" :key="c.instance_id" class="flex items-center gap-2 py-0.5 text-[12px]">
          <IconUser class="size-3.5 shrink-0 text-muted-foreground/60" />
          <span class="truncate text-foreground/90">{{ c.name }}</span>
          <span v-if="isControlledChar(store.controlledId, c)" class="ml-auto shrink-0 rounded-full bg-primary/12 px-1.5 text-[10px] font-bold text-primary">你</span>
        </div>
      </div>
    </template>

    <p v-else class="px-1 text-[12px] leading-relaxed text-muted-foreground/70">
      从左栏选择任务 / 触发点 / 地点 / 人物，这里会显示详细信息。
    </p>
  </aside>
</template>
