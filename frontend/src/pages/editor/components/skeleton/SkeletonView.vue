<script setup lang="ts">
// SkeletonView —— A 工作台「骨架」tab：独立大纲式（#07 ② / #22 ②）
// 左场景树（章节 → 场景） + 右 goal / trigger 卡片
// 目标 goal：编辑 text / primary；触发点 trigger：title / description / hint
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { ChapterDef, GoalDef, SceneDef, TriggerDef } from '@/types'
import { uid } from '@/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { IconChevronRight, IconCircleDot, IconPlus, IconTrash, IconTarget, IconFlag, IconPencil } from '@tabler/icons-vue'
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
          <!-- 场景列表 -->
          <div v-if="openChapter.has(chIdx)" class="ml-4.5 mt-0.5 border-l border-border/70 pl-2 space-y-0.5">
            <button class="flex w-full cursor-pointer items-center gap-1 rounded-lg border border-dashed border-border/60 px-2 py-1 text-left text-xs text-muted-foreground/70 transition-colors hover:border-primary/40 hover:bg-primary/5 hover:text-primary" type="button" @click="addScene(chIdx)">
              <IconPlus class="size-3" />
              新增场景
            </button>
            <div
              v-for="(sc, scIdx) in ch.scenes"
              :key="sc.id"
              class="group/scene flex cursor-pointer items-center gap-2 rounded-lg border px-2 py-1.5 transition-all"
              :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'border-primary/40 bg-primary/10 font-semibold text-primary' : 'border-transparent text-foreground/80 hover:bg-muted/40'"
              @click="selectScene(chIdx, scIdx)"
            >
              <IconCircleDot class="size-2.5 shrink-0" :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'text-primary' : 'text-muted-foreground/50'" />
              <span class="min-w-0 flex-1 truncate text-[12.5px]">{{ sc.title }}</span>
              <span class="shrink-0 text-[10px] font-mono text-muted-foreground/70">{{ sc.goals.length }} 目标 · {{ sc.triggers.length }} 触发点</span>
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
            class="h-9 flex-1 rounded-md bg-transparent text-lg font-bold"
            :model-value="selScene.title"
            @update:model-value="selScene.title = String($event)"
          />
          <Button variant="ghost" size="sm" class="shrink-0 gap-1 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive" @click="removeScene(selScene)">
            <IconTrash data-icon="inline-start" />
            删除场景
          </Button>
        </div>
        <div class="mt-1.5 mb-6 text-xs text-muted-foreground/70">{{ selScene.present_char_ids?.length ?? 0 }} 名在场人物{{ selScene.location_id ? ' · 地点 ' + selScene.location_id : '' }}</div>

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
            <div class="mt-2.5 flex items-center gap-4 pl-1 text-xs">
              <label class="flex cursor-pointer items-center gap-2 text-muted-foreground">
                <Switch size="sm" :model-value="!!g.primary" @update:model-value="g.primary = Boolean($event)" />
                <span>设为场景主要目标</span>
              </label>
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
            该场景还没有剧情触发点 —— 满足条件即触发，并提示主线 AI 即兴演绎。
          </div>
          <div v-for="t in selScene.triggers" :key="t.id" class="mb-3 rounded-xl border border-border bg-card/85 p-3.5 shadow-xs transition-all hover:border-primary/40">
            <div class="mb-2 flex items-center gap-2">
              <Input
                class="h-8 flex-1 rounded-md border border-border bg-background/60 px-2.5 text-[13px] font-bold text-foreground shadow-none hover:border-border focus:border-ring focus:bg-background"
                :model-value="t.title"
                placeholder="触发点标题"
                @update:model-value="t.title = String($event)"
              />
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
              class="h-7 rounded border border-transparent bg-transparent px-1.5 text-xs text-muted-foreground/80 shadow-none hover:border-border/60 focus:border-ring focus:bg-background"
              :model-value="t.description ?? ''"
              placeholder="补充叙事描述（可选）"
              @update:model-value="t.description = String($event)"
            />
            <div class="mt-2.5">
              <div class="mb-1 text-[11px] font-bold uppercase tracking-wider text-muted-foreground">触发条件</div>
              <ConditionEditor :model-value="t.condition ?? null" @update:model-value="t.condition = $event" />
            </div>
          </div>
        </section>
      </template>

      <div v-else class="px-6 py-10 text-center">
        <p class="text-[13.5px] text-muted-foreground">从左侧选择一个场景，编辑它的目标与剧情触发点。</p>
        <p class="mt-2 text-xs text-muted-foreground/60">骨架 = 章节 / 场景 / 目标 / 剧情触发点：主线 AI 在结构化骨架内即兴（见 CONTEXT 术语）。</p>
      </div>
    </div>
  </div>
</template>
