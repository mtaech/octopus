<script setup lang="ts">
// SkeletonView —— A 工作台「骨架」tab：独立大纲式（#07 ② / #22 ②）
// 左场景树（章节 → 场景） + 右 goal / trigger 卡片
// 目标 goal：编辑 text / primary；触发点 trigger：title / description / hint / 预置遭遇
//
// 剧情关联（地图 P5 §6.2 / §6.3 / §6.7）：
// - 场景声明 location_id → 决定「按位置会来哪些人」与遭遇地点的缺省继承
// - 章节的地点 = 其 scenes 的地点并集（推导，不存字段）
// - 任务（goal）的地点继承自所属场景（推导，不存字段）
// - 触发点可声明预置遭遇 EncounterPreset：引用 kind='monster' 的图鉴条目 + 数量 + 可选地点
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { ChapterDef, CharacterDef, GoalDef, LocationDef, SceneDef, TriggerDef } from '@/types'
import { uid } from '@/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import {
  IconChevronRight, IconCircleDot, IconPlus, IconTrash, IconTarget, IconFlag, IconPencil,
  IconMapPin, IconSwords, IconUsers,
} from '@tabler/icons-vue'
import ConditionEditor from '@/components/ConditionEditor.vue'

const editor = useEditorStore()

// ---------- 选中态（页面局部） ----------
const selSceneIdx = ref<number | null>(null)
const selChapterIdx = ref<number | null>(null)
const openChapter = ref<Set<number>>(new Set([0]))

const chapters = computed<ChapterDef[]>(() => editor.draft?.skeleton ?? [])
function chAt(chIdx: number): ChapterDef | null { return chapters.value[chIdx] ?? null }
function sceneAt(chIdx: number, scIdx: number): SceneDef | null {
  return chAt(chIdx)?.scenes[scIdx] ?? null
}
function selectScene(chIdx: number, scIdx: number): void {
  selChapterIdx.value = chIdx
  selSceneIdx.value = scIdx
}
const selScene = computed<SceneDef | null>(() => {
  if (selChapterIdx.value == null || selSceneIdx.value == null) return null
  return sceneAt(selChapterIdx.value, selSceneIdx.value)
})

function toggleChapter(i: number): void {
  const s = new Set(openChapter.value)
  if (s.has(i)) s.delete(i); else s.add(i)
  openChapter.value = s
}

// ---------- 地点（world.locations）：关联视图共用 ----------
const locations = computed<LocationDef[]>(() => editor.draft?.world.locations ?? [])
const locationById = computed(() => new Map(locations.value.map(l => [l.id, l])))
/** 地点 id → 显示名；悬空引用回落为 id 本身（校验会另行拦截） */
function locationName(id?: string | null): string {
  if (!id) return ''
  return locationById.value.get(id)?.name ?? id
}
const locationOptions = computed(() => locations.value.map(l => ({ value: l.id, label: l.name + ' · ' + l.id })))

/** reka Select 不接受空串：用哨兵表示「不指定地点」 */
const NO_LOCATION = '__skeleton_no_location__'

/** 场景的地点选择：置空 = 不参与位置在场（在场名单仍由 present_char_ids 决定） */
function setSceneLocation(sc: SceneDef, v: string): void {
  if (v && v !== NO_LOCATION) sc.location_id = v
  else delete sc.location_id
}

/** 章节的地点 = 其 scenes 的地点并集（推导，不落字段 → 不会不同步） */
function chapterLocationNames(ch: ChapterDef): string[] {
  const ids = new Set<string>()
  ch.scenes.forEach(sc => { if (sc.location_id) ids.add(sc.location_id) })
  return [...ids].map(id => locationName(id) || id)
}

/** 位置匹配：地点相同且未被作者点名（present_char_ids 优先，见设计 §4.1）的人 */
const sceneAutoPresent = computed<CharacterDef[]>(() => {
  const sc = selScene.value
  if (!sc?.location_id) return []
  const declared = new Set(sc.present_char_ids ?? [])
  return (editor.draft?.characters ?? [])
    .filter(c => c.location_id === sc.location_id && !declared.has(c.id))
})

// ---------- 图鉴（kind = 'monster'）：预置遭遇的引用源 ----------
const monsterLibrary = computed<CharacterDef[]>(() => (editor.draft?.characters ?? []).filter(c => c.kind === 'monster'))
const monsterIds = computed(() => new Set(monsterLibrary.value.map(m => m.id)))
function monsterName(id: string): string {
  return monsterLibrary.value.find(m => m.id === id)?.name ?? id
}
const NO_TEMPLATE = '__skeleton_no_template__'
/** 引用了非 monster / 已删除的图鉴条目：校验会拦，这里先给作者看见 */
function danglingTemplate(id: string): boolean {
  return !!id && !monsterIds.value.has(id)
}

// ---------- 章节 / 场景 CRUD ----------
function addChapter(): void {
  const sk = editor.draft?.skeleton
  if (!sk) return
  const ch: ChapterDef = { id: uid('ch'), title: '新章节', description: '', scenes: [] }
  sk.push(ch)
  const chIdx = sk.length - 1
  openChapter.value = new Set([...openChapter.value, chIdx])
  selChapterIdx.value = chIdx
  selSceneIdx.value = null
}
function removeChapter(chIdx: number): void {
  editor.draft?.skeleton.splice(chIdx, 1)
  if (selChapterIdx.value === chIdx) { selChapterIdx.value = null; selSceneIdx.value = null }
}
function addScene(chIdx: number): void {
  const ch = chAt(chIdx)
  if (!ch) return
  const sc: SceneDef = { id: uid('sc'), title: '新场景', goals: [], triggers: [], present_char_ids: [] }
  ch.scenes.push(sc)
  selectScene(chIdx, ch.scenes.length - 1)
}
function removeScene(sc: SceneDef): void {
  const ch = chAt(selChapterIdx.value ?? -1)
  if (!ch) return
  const i = ch.scenes.indexOf(sc)
  if (i >= 0) ch.scenes.splice(i, 1)
  selSceneIdx.value = null
}

// ---------- goal / trigger CRUD（都在选中场景上） ----------
function addGoal(sc: SceneDef): void {
  sc.goals.push({ id: uid('g'), text: '新目标', primary: false, condition: null })
}
function removeGoal(sc: SceneDef, g: GoalDef): void {
  const i = sc.goals.indexOf(g)
  if (i >= 0) sc.goals.splice(i, 1)
}
function addTrigger(sc: SceneDef): void {
  sc.triggers.push({ id: uid('b'), title: '新触发点', description: '', hint: '', condition: null })
}
function removeTrigger(sc: SceneDef, t: TriggerDef): void {
  const i = sc.triggers.indexOf(t)
  if (i >= 0) sc.triggers.splice(i, 1)
}

// ---------- 预置遭遇（TriggerDef.encounter / EncounterPreset，地图 P5 §6.3） ----------
/** 声明预置遭遇：种子一条怪物引用（有图鉴就用第一条，没有留空等作者补） */
function enableEncounter(t: TriggerDef): void {
  t.encounter = { enemies: [{ template_id: monsterLibrary.value[0]?.id ?? '', count: 1 }] }
}
function disableEncounter(t: TriggerDef): void {
  delete t.encounter
}
function addEnemy(t: TriggerDef): void {
  if (!t.encounter) return
  t.encounter.enemies.push({ template_id: monsterLibrary.value[0]?.id ?? '', count: 1 })
}
function removeEnemy(t: TriggerDef, i: number): void {
  t.encounter?.enemies.splice(i, 1)
}
/** 遭遇地点：置空 = 继承触发时所在场景的 location_id（与 Intent::Encounter 同口径） */
function setEncounterLocation(t: TriggerDef, v: string): void {
  if (!t.encounter) return
  if (v && v !== NO_LOCATION) t.encounter.location_id = v
  else delete t.encounter.location_id
}
function setEncounterName(t: TriggerDef, v: string): void {
  if (!t.encounter) return
  if (v) t.encounter.name = v
  else delete t.encounter.name
}
function setEncounterNote(t: TriggerDef, v: string): void {
  if (!t.encounter) return
  if (v) t.encounter.note = v
  else delete t.encounter.note
}
/** 预置遭遇展开的敌人总数（作者速览用） */
function encounterCount(t: TriggerDef): number {
  return (t.encounter?.enemies ?? []).reduce((n, e) => n + (e.count ?? 1), 0)
}
function encounterLabel(t: TriggerDef): string {
  const names = (t.encounter?.enemies ?? []).filter(e => e.template_id).map(e => monsterName(e.template_id))
  return names.length ? names.join('、') : '未指定怪物'
}
</script>

<template>
  <div class="flex h-full min-h-0">
    <!-- 左：场景树 -->
    <div class="flex w-72 flex-none flex-col border-r border-border bg-card/25">
      <div class="flex flex-none items-center gap-2 border-b border-border px-3 py-3">
        <h3 class="text-[13px] font-semibold text-foreground">章节 / 场景</h3>
        <Badge variant="outline" class="border-border/80 px-1.5 font-mono text-[10.5px] text-muted-foreground">{{ chapters.length }}</Badge>
        <Button size="sm" class="ml-auto h-7 gap-1 px-2 text-xs" @click="addChapter">
          <IconPlus class="size-3.5" />
          章节
        </Button>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto p-2">
        <div v-if="!chapters.length" class="px-3 py-6 text-center text-xs leading-relaxed text-muted-foreground/70">
          <p>场景树在这里组织故事的结构化骨架：章节 → 场景 → 目标 / 剧情触发点。</p>
          <Button size="sm" class="mt-3 shadow-xs" @click="addChapter">新增第一章</Button>
        </div>
        <div v-for="(ch, chIdx) in chapters" :key="ch.id" class="mb-1">
          <!-- 章节行 -->
          <div class="group flex cursor-pointer items-center gap-1.5 rounded-lg px-2 py-1.5 transition-colors hover:bg-muted/50" @click="toggleChapter(chIdx)">
            <IconChevronRight class="size-3.5 shrink-0 text-muted-foreground/60 transition-transform duration-200" :class="openChapter.has(chIdx) ? 'rotate-90' : ''" />
            <Input
              class="h-7 flex-1 rounded border border-dashed border-border bg-muted/25 px-1.5 text-[13px] font-bold text-foreground shadow-none transition-colors hover:border-solid hover:border-primary/60 hover:bg-primary/5 focus:border-solid focus:border-ring focus:bg-background"
              title="点击可修改标题"
              :model-value="ch.title"
              @update:model-value="ch.title = String($event)"
              @click.stop
            />
            <IconPencil class="size-3 shrink-0 text-muted-foreground/40 transition-colors group-hover:text-primary/70" />
            <Button
              variant="ghost" size="icon-xs"
              class="size-5 shrink-0 text-muted-foreground/40 opacity-0 hover:text-destructive group-hover:opacity-100 transition-opacity"
              title="删除章节" aria-label="删除章节"
              @click.stop="removeChapter(chIdx)"
            >
              <IconTrash class="size-3" />
            </Button>
          </div>
          <!-- 章节地点：其 scenes 的地点并集（推导，不存字段） -->
          <div class="ml-6 flex items-center gap-1 pb-1 pl-0.5 text-[10.5px] text-muted-foreground/70">
            <IconMapPin class="size-3 shrink-0 text-muted-foreground/50" />
            <span v-if="chapterLocationNames(ch).length" class="truncate" :title="'章节地点 = 各场景地点的并集：' + chapterLocationNames(ch).join('、')">
              地点并集：{{ chapterLocationNames(ch).join('、') }}
            </span>
            <span v-else-if="ch.scenes.length" class="text-muted-foreground/50" title="章节地点由各场景推导；这些场景都还没指定地点">
              地点并集：未指定（{{ ch.scenes.length }} 个场景都没有地点）
            </span>
            <span v-else class="text-muted-foreground/50">还没有场景</span>
          </div>
          <!-- 场景列表 -->
          <div v-if="openChapter.has(chIdx)" class="ml-4.5 mt-0.5 border-l border-border/70 pl-2 space-y-0.5">
            <button class="flex w-full cursor-pointer items-center gap-1 rounded-lg border border-dashed border-border/60 px-2 py-1 text-left text-xs text-muted-foreground/70 transition-colors hover:border-primary/40 hover:bg-primary/5 hover:text-primary" type="button" @click="addScene(chIdx)">
              <IconPlus class="size-3" />
              新增场景
            </button>
            <div
              v-for="(sc, scIdx) in ch.scenes"
              :key="sc.id"
              class="group/scene flex cursor-pointer flex-col gap-0.5 rounded-lg border px-2 py-1.5 transition-all"
              :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'border-primary/40 bg-primary/10 font-semibold text-primary' : 'border-transparent text-foreground/80 hover:bg-muted/40'"
              @click="selectScene(chIdx, scIdx)"
            >
              <div class="flex items-center gap-2">
                <IconCircleDot class="size-2.5 shrink-0" :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'text-primary' : 'text-muted-foreground/50'" />
                <span class="min-w-0 flex-1 truncate text-[12.5px]">{{ sc.title }}</span>
                <span class="shrink-0 text-[10px] font-mono text-muted-foreground/70">{{ sc.goals.length }} 目标 · {{ sc.triggers.length }} 触发点</span>
              </div>
              <div class="flex items-center gap-1 pl-4.5 text-[10.5px]" :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'text-primary/80' : 'text-muted-foreground/70'">
                <IconMapPin class="size-2.5 shrink-0" />
                <span class="truncate" :title="sc.location_id ? '场景地点：' + locationName(sc.location_id) : '未指定地点：位置在场与遭遇地点继承都不会生效'">
                  {{ locationName(sc.location_id) || '未指定地点' }}
                </span>
                <span v-if="sc.triggers.some(t => t.encounter)" class="inline-flex shrink-0 items-center gap-0.5 rounded-full bg-warning/15 px-1.5 font-mono text-[9.5px] text-warning" title="有触发点声明了预置遭遇">
                  <IconSwords class="size-2.5" />
                  {{ sc.triggers.filter(t => t.encounter).length }}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 右：选中场景的 goal / trigger 卡片 -->
    <div class="min-w-0 flex-1 overflow-y-auto px-6 py-4">
      <template v-if="selScene">
        <div class="flex items-center gap-2">
          <Input
            class="h-9 flex-1 rounded-md border border-dashed border-border bg-muted/25 text-lg font-bold text-foreground shadow-none transition-colors hover:border-solid hover:border-primary/60 hover:bg-primary/5 focus:border-solid focus:border-primary focus:bg-background"
            title="点击可修改场景标题"
            :model-value="selScene.title"
            @update:model-value="selScene.title = String($event)"
          />
          <IconPencil class="size-4 shrink-0 text-muted-foreground/45" />
          <Button variant="ghost" size="sm" class="shrink-0 gap-1 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive" @click="removeScene(selScene)">
            <IconTrash data-icon="inline-start" />
            删除场景
          </Button>
        </div>

        <!-- 场景的地点：决定自动在场与遭遇地点继承 -->
        <div class="mt-2 mb-5 flex flex-wrap items-center gap-x-3 gap-y-2 text-xs text-muted-foreground/80">
          <span class="inline-flex items-center gap-1.5">
            <IconUsers class="size-3.5 text-muted-foreground/60" />
            {{ selScene.present_char_ids?.length ?? 0 }} 名在场人物
          </span>
          <span class="inline-flex items-center gap-1.5">
            <IconMapPin class="size-3.5 text-muted-foreground/60" />
            <span class="font-semibold text-foreground/80">场景地点</span>
            <Select :model-value="selScene.location_id ?? NO_LOCATION" @update:model-value="(v: unknown) => setSceneLocation(selScene!, String(v))">
              <SelectTrigger
                class="h-7 w-56 text-xs"
                title="本场景发生的地点：决定「按位置会来哪些人」，也是预置遭遇地点的缺省来源"
              >
                <SelectValue placeholder="（未指定地点）" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem :value="NO_LOCATION" class="text-muted-foreground">（未指定 · 只按点名在场）</SelectItem>
                  <SelectItem v-for="o in locationOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </span>
          <span v-if="!locationOptions.length" class="text-[11px] text-muted-foreground/60">世界还没有地点，去「世界」新增</span>
        </div>

        <!-- 按位置会来哪些人（设计 §7：让作者看见自动在场的后果再决定是否覆盖） -->
        <div v-if="selScene.location_id" class="mb-6 rounded-lg border border-dashed border-border/70 bg-muted/20 px-3 py-2 text-[11.5px] leading-5 text-muted-foreground">
          <span class="font-semibold text-foreground/80">按位置会到场：</span>
          <template v-if="sceneAutoPresent.length">
            {{ sceneAutoPresent.map(c => c.name + (c.kind === 'monster' ? '（怪物）' : '')).join('、') }}
            <span class="text-muted-foreground/60">（{{ sceneAutoPresent.length }} 人常驻「{{ locationName(selScene.location_id) }}」；作者点名的人优先，见在场三层优先级）</span>
          </template>
          <span v-else class="text-muted-foreground/60">没有常驻此地点的人物 —— 进入本场景时不会自动到场。</span>
        </div>

        <section class="mb-8">
          <div class="mb-3 flex items-center justify-between">
            <span class="flex items-center gap-1.5 text-xs font-bold tracking-widest text-foreground uppercase">
              <IconTarget class="size-4 text-primary" />
              目标 Goal
            </span>
            <Button variant="ghost" size="xs" class="gap-1 text-xs text-primary hover:bg-primary/10" @click="addGoal(selScene)">
              <IconPlus data-icon="inline-start" class="size-3.5" />
              目标
            </Button>
          </div>
          <div v-if="!selScene.goals.length" class="rounded-xl border border-dashed border-border p-4 text-center text-xs leading-5 text-muted-foreground/60">
            该场景还没有目标 —— 引擎在回合末求值，达成会提示主线 AI。
          </div>
          <div v-for="g in selScene.goals" :key="g.id" class="mb-3 rounded-xl border border-border bg-card/85 p-3.5 shadow-xs transition-all hover:border-primary/40">
            <div class="flex items-start gap-2.5">
              <Badge :variant="g.primary ? 'default' : 'outline'" class="mt-1 shrink-0 text-[10.5px] font-semibold">{{ g.primary ? '主要目标' : '次要' }}</Badge>
              <Textarea
                class="min-h-12 w-full flex-1 resize-none rounded-lg border border-border bg-background/60 p-2 text-[13px] leading-relaxed shadow-none focus:border-ring focus:bg-background"
                :model-value="g.text"
                rows="2"
                spellcheck="false"
                placeholder="目标描述…"
                @update:model-value="g.text = String($event)"
              />
              <Button
                variant="ghost" size="icon-xs"
                class="mt-0.5 size-6 shrink-0 text-muted-foreground/50 hover:text-destructive transition-colors"
                title="删除目标" aria-label="删除目标"
                @click="removeGoal(selScene, g)"
              >
                <IconTrash class="size-3.5" />
              </Button>
            </div>
            <div class="mt-2.5 flex flex-wrap items-center gap-x-4 gap-y-1.5 pl-1 text-xs">
              <label class="flex cursor-pointer items-center gap-2 text-muted-foreground">
                <Switch size="sm" :model-value="!!g.primary" @update:model-value="g.primary = Boolean($event)" />
                <span>设为场景主要目标</span>
              </label>
              <span class="inline-flex items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-1.5 py-0.5 text-[11px] text-muted-foreground/80" :title="'任务地点继承自所属场景（推导，不落字段，不可直接编辑）'">
                <IconMapPin class="size-3 text-muted-foreground/60" />
                地点：{{ locationName(selScene.location_id) || '未指定' }}
                <span class="text-muted-foreground/50">· 继承自场景</span>
              </span>
              <span class="font-mono text-[11px] text-muted-foreground/60">判据：{{ g.condition ? '已定义' : '默认自动' }}</span>
            </div>
            <div class="mt-2.5">
              <div class="mb-1 text-[11px] font-bold uppercase tracking-wider text-muted-foreground">完成条件</div>
              <ConditionEditor :model-value="g.condition ?? null" @update:model-value="g.condition = $event" />
            </div>
          </div>
        </section>

        <section>
          <div class="mb-3 flex items-center justify-between">
            <span class="flex items-center gap-1.5 text-xs font-bold tracking-widest text-foreground uppercase">
              <IconFlag class="size-4 text-primary" />
              剧情触发点 Trigger
            </span>
            <Button variant="ghost" size="xs" class="gap-1 text-xs text-primary hover:bg-primary/10" @click="addTrigger(selScene)">
              <IconPlus data-icon="inline-start" class="size-3.5" />
              触发点
            </Button>
          </div>
          <div v-if="!selScene.triggers.length" class="rounded-xl border border-dashed border-border p-4 text-center text-xs leading-5 text-muted-foreground/60">
            该场景还没有剧情触发点 —— 满足条件即触发，并提示主线 AI 即兴演绎；也可以直接预置一场遭遇。
          </div>
          <div v-for="t in selScene.triggers" :key="t.id" class="mb-3 rounded-xl border border-border bg-card/85 p-3.5 shadow-xs transition-all hover:border-primary/40">
            <div class="mb-2 flex items-center gap-2">
              <Input
                class="h-8 flex-1 rounded-md border border-border bg-background/60 px-2.5 text-[13px] font-bold text-foreground shadow-none hover:border-border focus:border-ring focus:bg-background"
                :model-value="t.title"
                placeholder="触发点标题"
                @update:model-value="t.title = String($event)"
              />
              <Badge v-if="t.encounter" variant="outline" class="shrink-0 gap-1 border-warning/40 bg-warning/10 text-[10.5px] text-warning" :title="'预置遭遇：' + encounterLabel(t) + '（共 ' + encounterCount(t) + ' 个敌人）'">
                <IconSwords class="size-3" />
                遭遇 {{ encounterCount(t) }}
              </Badge>
              <Button
                variant="ghost" size="icon-xs"
                class="size-6 shrink-0 text-muted-foreground/50 hover:text-destructive transition-colors"
                title="删除触发点" aria-label="删除触发点"
                @click="removeTrigger(selScene, t)"
              >
                <IconTrash class="size-3.5" />
              </Button>
            </div>
            <Textarea
              class="mb-2 min-h-12 w-full resize-none rounded-lg border border-border bg-background/60 p-2 text-[13px] leading-relaxed shadow-none focus:border-ring focus:bg-background"
              :model-value="t.hint"
              rows="2"
              spellcheck="false"
              placeholder="触发后给主线 AI 的提示建议…"
              @update:model-value="t.hint = String($event)"
            />
            <Input
              class="h-7 rounded border border-dashed border-border/70 bg-muted/20 px-1.5 text-xs text-muted-foreground/80 shadow-none transition-colors hover:border-solid hover:border-primary/50 hover:bg-primary/5 focus:border-solid focus:border-ring focus:bg-background"
              title="点击可补充这个触发点的叙事描述"
              :model-value="t.description ?? ''"
              placeholder="补充叙事描述（可选）"
              @update:model-value="t.description = String($event)"
            />
            <div class="mt-2.5">
              <div class="mb-1 text-[11px] font-bold uppercase tracking-wider text-muted-foreground">触发条件</div>
              <ConditionEditor :model-value="t.condition ?? null" @update:model-value="t.condition = $event" />
            </div>

            <!-- 预置遭遇（地图 P5 §6.3）：触发点 fired 后自动建遭遇，不再靠导演 AI 即兴 spawn -->
            <div class="mt-3 rounded-lg border border-dashed border-border/70 bg-muted/20 p-2.5">
              <div class="flex flex-wrap items-center gap-2">
                <IconSwords class="size-3.5 shrink-0 text-warning" />
                <span class="text-[11px] font-bold uppercase tracking-wider text-muted-foreground">触发时预置遭遇</span>
                <span class="rounded-full bg-muted/60 px-1.5 font-mono text-[10px] text-muted-foreground/70">encounter</span>
                <Button
                  v-if="!t.encounter"
                  variant="outline" size="xs"
                  class="ml-auto gap-1 border-dashed text-xs"
                  title="声明后，触发点被标记 fired 时引擎自动建这场遭遇"
                  @click="enableEncounter(t)"
                >
                  <IconPlus class="size-3" />
                  声明预置遭遇
                </Button>
                <Button
                  v-else
                  variant="ghost" size="xs"
                  class="ml-auto gap-1 text-xs text-muted-foreground/70 hover:bg-destructive/10 hover:text-destructive"
                  title="移除预置遭遇（触发点仍会触发，只是不再自动建遭遇）"
                  @click="disableEncounter(t)"
                >
                  <IconTrash class="size-3" />
                  移除遭遇
                </Button>
              </div>
              <p v-if="!t.encounter" class="mt-1 text-[11px] leading-5 text-muted-foreground/70">
                触发点被标记 fired 后，引擎按这里声明的图鉴条目（kind = monster）自动建一场遭遇，走导演 encounter 的同一条创建路径（实例克隆 + 地点继承）。
              </p>

              <div v-else class="mt-2 space-y-2">
                <div class="grid grid-cols-1 gap-2 @lg:grid-cols-2">
                  <label class="flex min-w-0 flex-col gap-1">
                    <span class="text-[11px] text-muted-foreground">遭遇名（可选）</span>
                    <Input
                      class="h-8 text-[13px]"
                      :model-value="t.encounter.name ?? ''"
                      :placeholder="'缺省用触发点标题：' + (t.title || '未命名')"
                      @update:model-value="setEncounterName(t, String($event))"
                    />
                  </label>
                  <label class="flex min-w-0 flex-col gap-1">
                    <span class="text-[11px] text-muted-foreground">发生地点</span>
                    <Select :model-value="t.encounter.location_id ?? NO_LOCATION" @update:model-value="(v: unknown) => setEncounterLocation(t, String(v))">
                      <SelectTrigger class="h-8 w-full text-xs" title="缺省继承触发时所在场景的地点">
                        <SelectValue placeholder="（继承场景地点）" />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem :value="NO_LOCATION" class="text-muted-foreground">
                            继承场景地点{{ selScene.location_id ? '（' + locationName(selScene.location_id) + '）' : '（场景未指定）' }}
                          </SelectItem>
                          <SelectItem v-for="o in locationOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </label>
                </div>
                <label class="flex min-w-0 flex-col gap-1">
                  <span class="text-[11px] text-muted-foreground">遭遇备注（可选 · 只给作者看）</span>
                  <Textarea
                    class="min-h-10 w-full resize-none text-[13px] leading-relaxed"
                    :model-value="t.encounter.note ?? ''"
                    rows="2"
                    spellcheck="false"
                    placeholder="为什么这里会打起来、伏击还是正面冲突…"
                    @update:model-value="setEncounterNote(t, String($event))"
                  />
                </label>

                <div>
                  <div class="flex items-center gap-2">
                    <span class="text-[11px] font-semibold text-muted-foreground">怪物（引用图鉴 kind = monster）</span>
                    <span class="font-mono text-[10.5px] text-muted-foreground/60">共 {{ encounterCount(t) }} 个</span>
                    <Button variant="ghost" size="xs" class="ml-auto gap-1 text-xs text-primary hover:bg-primary/10" @click="addEnemy(t)">
                      <IconPlus class="size-3" />
                      加一条
                    </Button>
                  </div>
                  <p v-if="!monsterLibrary.length" class="mt-1 text-[11px] leading-5 text-warning">
                    图鉴里还没有怪物：去「人物」新增一条 kind = monster 的条目，这里才有可引用的模板。
                  </p>
                  <div v-for="(e, i) in t.encounter.enemies" :key="i" class="mt-1.5 flex items-end gap-2">
                    <label class="flex min-w-0 flex-1 flex-col gap-1">
                      <Select
                        :model-value="e.template_id || NO_TEMPLATE"
                        @update:model-value="(v: unknown) => e.template_id = v === NO_TEMPLATE ? '' : String(v)"
                      >
                        <SelectTrigger class="h-8 w-full text-xs" :class="danglingTemplate(e.template_id) ? 'border-destructive/60' : ''">
                          <SelectValue placeholder="（选择图鉴条目）" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem :value="NO_TEMPLATE" class="text-muted-foreground">（未选择）</SelectItem>
                            <SelectItem v-for="m in monsterLibrary" :key="m.id" :value="m.id">{{ m.name }} · {{ m.id }}</SelectItem>
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                    </label>
                    <label class="flex w-24 shrink-0 flex-col gap-1">
                      <span class="text-[11px] text-muted-foreground">数量</span>
                      <Input
                        type="number" min="1" max="99"
                        class="h-8 text-[13px]"
                        :model-value="e.count ?? 1"
                        @update:model-value="e.count = Math.max(1, Math.min(99, Number($event) || 1))"
                      />
                    </label>
                    <Button
                      variant="ghost" size="icon-sm"
                      class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive"
                      title="删除这条怪物引用" aria-label="删除这条怪物引用"
                      @click="removeEnemy(t, i)"
                    >
                      <IconTrash class="size-3.5" />
                    </Button>
                  </div>
                  <p v-if="t.encounter.enemies.some(e => danglingTemplate(e.template_id))" class="mt-1 text-[11px] text-destructive">
                    有怪物引用指向不存在（或不是 kind = monster）的图鉴条目，发布校验会拦截。
                  </p>
                  <p v-if="!t.encounter.enemies.length" class="mt-1 text-[11px] text-warning">
                    还没有怪物引用 —— 触发时会建出一场空遭遇，请至少加一条。
                  </p>
                </div>
              </div>
            </div>
          </div>
        </section>
      </template>

      <div v-else class="px-6 py-10 text-center">
        <p class="text-[13.5px] text-muted-foreground">从左侧选择一个场景，编辑它的目标、剧情触发点与预置遭遇。</p>
        <p class="mt-2 text-xs text-muted-foreground/60">骨架 = 章节 / 场景 / 目标 / 剧情触发点：主线 AI 在结构化骨架内即兴（见 CONTEXT 术语）。</p>
      </div>
    </div>
  </div>
</template>
