<script setup lang="ts">
// SkeletonView —— A 工作台「骨架」tab：独立大纲式（#07 ② / #22 ②）
// 左场景树（章节 → 场景） + 右 goal / beat 卡片
// 目标 goal：编辑 text / primary；节拍 beat：title / description / hint
import { computed, ref } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { BeatDef, ChapterDef, GoalDef, SceneDef } from '@/types'
import { uid } from '@/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { IconChevronRight, IconCircleDot, IconPlus, IconTrash, IconTarget, IconFlag } from '@tabler/icons-vue'

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
  const sc: SceneDef = { id: uid('sc'), title: '新场景', goals: [], beats: [], present_char_ids: [] }
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

// ---------- goal / beat CRUD（都在选中场景上） ----------
function addGoal(sc: SceneDef): void {
  sc.goals.push({ id: uid('g'), text: '新目标', primary: false, condition: null })
}
function removeGoal(sc: SceneDef, g: GoalDef): void {
  const i = sc.goals.indexOf(g)
  if (i >= 0) sc.goals.splice(i, 1)
}
function addBeat(sc: SceneDef): void {
  sc.beats.push({ id: uid('b'), title: '新节拍', description: '', hint: '', condition: null })
}
function removeBeat(sc: SceneDef, b: BeatDef): void {
  const i = sc.beats.indexOf(b)
  if (i >= 0) sc.beats.splice(i, 1)
}
</script>

<template>
  <div class="flex h-full min-h-0">
    <!-- 左：场景树 -->
    <div class="flex w-76 flex-none flex-col border-r border-border/80 bg-card/25 backdrop-blur-xs">
      <div class="flex items-center justify-between border-b border-border/80 px-3.5 py-2.5">
        <span class="text-xs font-bold tracking-wider text-foreground/80 uppercase">章节 / 场景</span>
        <Button variant="ghost" size="xs" class="gap-1 text-xs text-primary hover:bg-primary/10" @click="addChapter">
          <IconPlus data-icon="inline-start" class="size-3.5" />
          章节
        </Button>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto p-2">
        <div v-if="!chapters.length" class="px-3 py-6 text-center text-xs leading-relaxed text-muted-foreground/70">
          <p>场景树在这里组织故事的结构化骨架：章节 → 场景 → 目标 / 节拍。</p>
          <Button size="sm" class="mt-3 shadow-xs" @click="addChapter">新增第一章</Button>
        </div>
        <div v-for="(ch, chIdx) in chapters" :key="ch.id" class="mb-1">
          <!-- 章节行 -->
          <div class="group flex cursor-pointer items-center gap-1.5 rounded-lg px-2 py-1.5 transition-colors hover:bg-muted/50" @click="toggleChapter(chIdx)">
            <IconChevronRight class="size-3.5 shrink-0 text-muted-foreground/60 transition-transform duration-200" :class="openChapter.has(chIdx) ? 'rotate-90' : ''" />
            <Input
              class="h-7 flex-1 rounded border border-transparent bg-transparent px-1.5 text-[13px] font-bold text-foreground shadow-none hover:border-border/60 focus:border-ring focus:bg-background"
              :model-value="ch.title"
              @update:model-value="ch.title = String($event)"
              @click.stop
            />
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
            <div
              v-for="(sc, scIdx) in ch.scenes"
              :key="sc.id"
              class="group/scene flex cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 transition-all"
              :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'bg-primary/12 text-primary font-semibold border-l-2 border-primary shadow-2xs' : 'text-foreground/80 hover:bg-muted/40'"
              @click="selectScene(chIdx, scIdx)"
            >
              <IconCircleDot class="size-2.5 shrink-0" :class="selChapterIdx === chIdx && selSceneIdx === scIdx ? 'text-primary' : 'text-muted-foreground/50'" />
              <span class="min-w-0 flex-1 truncate text-[12.5px]">{{ sc.title }}</span>
              <span class="shrink-0 text-[10px] font-mono text-muted-foreground/70">{{ sc.goals.length }}目·{{ sc.beats.length }}拍</span>
            </div>
            <button class="flex w-full items-center gap-1 cursor-pointer rounded-lg px-2 py-1 text-left text-xs text-muted-foreground/70 hover:bg-muted/30 hover:text-primary transition-colors" type="button" @click="addScene(chIdx)">
              <IconPlus class="size-3" />
              新增场景
            </button>
          </div>
        </div>
      </div>
    </div>

    <!-- 右：选中场景的 goal / beat 卡片 -->
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
          <div v-if="!selScene.goals.length" class="rounded-xl border border-dashed border-border/70 p-4 text-center text-xs leading-5 text-muted-foreground/60">
            该场景还没有目标 —— 引擎在回合末求值，达成会提示主线 AI。
          </div>
          <div v-for="g in selScene.goals" :key="g.id" class="mb-3 rounded-xl border border-border/75 bg-card/75 p-3.5 shadow-xs transition-all hover:border-border">
            <div class="flex items-start gap-2.5">
              <Badge :variant="g.primary ? 'default' : 'outline'" class="mt-1 shrink-0 text-[10.5px] font-semibold">{{ g.primary ? '主要目标' : '次要' }}</Badge>
              <Textarea
                class="min-h-12 w-full flex-1 resize-none rounded-lg border-border/50 bg-background/50 p-2 text-[13px] leading-relaxed shadow-none focus:border-ring focus:bg-background"
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
          </div>
        </section>

        <section>
          <div class="mb-3 flex items-center justify-between">
            <span class="flex items-center gap-1.5 text-xs font-bold tracking-widest text-foreground uppercase">
              <IconFlag class="size-4 text-primary" />
              节拍 Beat
            </span>
            <Button variant="ghost" size="xs" class="gap-1 text-xs text-primary hover:bg-primary/10" @click="addBeat(selScene)">
              <IconPlus data-icon="inline-start" class="size-3.5" />
              节拍
            </Button>
          </div>
          <div v-if="!selScene.beats.length" class="rounded-xl border border-dashed border-border/70 p-4 text-center text-xs leading-5 text-muted-foreground/60">
            该场景还没有节拍 —— 关键剧情事件节点，触发后提示主线 AI 即兴演绎。
          </div>
          <div v-for="b in selScene.beats" :key="b.id" class="mb-3 rounded-xl border border-border/75 bg-card/75 p-3.5 shadow-xs transition-all hover:border-border">
            <div class="mb-2 flex items-center gap-2">
              <Input
                class="h-8 flex-1 rounded-md border-border/60 bg-background/50 px-2.5 text-[13px] font-bold text-foreground shadow-none hover:border-border focus:border-ring focus:bg-background"
                :model-value="b.title"
                placeholder="节拍标题"
                @update:model-value="b.title = String($event)"
              />
              <Button
                variant="ghost" size="icon-xs"
                class="size-6 shrink-0 text-muted-foreground/50 hover:text-destructive transition-colors"
                title="删除节拍" aria-label="删除节拍"
                @click="removeBeat(selScene, b)"
              >
                <IconTrash class="size-3.5" />
              </Button>
            </div>
            <Textarea
              class="mb-2 min-h-12 w-full resize-none rounded-lg border-border/50 bg-background/50 p-2 text-[13px] leading-relaxed shadow-none focus:border-ring focus:bg-background"
              :model-value="b.hint"
              rows="2"
              spellcheck="false"
              placeholder="触发后给主线 AI 的提示建议…"
              @update:model-value="b.hint = String($event)"
            />
            <Input
              class="h-7 rounded border-transparent bg-transparent px-1.5 text-xs text-muted-foreground/80 shadow-none hover:border-border/60 focus:border-ring focus:bg-background"
              :model-value="b.description ?? ''"
              placeholder="补充叙事描述（可选）"
              @update:model-value="b.description = String($event)"
            />
          </div>
        </section>
      </template>

      <div v-else class="px-6 py-10 text-center">
        <p class="text-[13.5px] text-muted-foreground">从左侧选择一个场景，编辑它的目标与节拍。</p>
        <p class="mt-2 text-xs text-muted-foreground/60">骨架 = 章节 / 场景 / 目标 / 节拍：主线 AI 在结构化骨架内即兴（见 CONTEXT 术语）。</p>
      </div>
    </div>
  </div>
</template>
