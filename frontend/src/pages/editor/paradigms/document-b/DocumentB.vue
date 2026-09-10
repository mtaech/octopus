<script setup lang="ts">
// DocumentB —— B 文档视图（#07 反馈修订：叙事自由书写的载体，非主壳）
// 把整本草稿渲染成长文：premise / 人物 / 技能 / 物品 / 世界 区块
// 关键叙事字段局部可编辑（premise、人物 background/personality、技能/物品描述…），
// 其余为只读展示；所有编辑直写 editor store 草稿（B = 纯视图容器，无独立状态）
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import DocText from '../../components/document/DocText.vue'
import DocBlock from '../../components/document/DocBlock.vue'
import DocMarkdown from '../../components/document/DocMarkdown.vue'
import type { ObjectDef } from '@/types'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'

const editor = useEditorStore()
const d = computed(() => editor.draft)

const errCount = computed(() => editor.issues.filter(i => i.severity === 'error').length)
const title = computed(() => d.value?.meta.title ?? '')

function locName(id?: string): string {
  if (!id) return '（未指定）'
  return d.value?.world.locations.find(l => l.id === id)?.name ?? id
}
function actionText(o: ObjectDef): string {
  const acts = o.actions ?? []
  if (!acts.length) return '（无动作）'
  return acts.map(a => a.label + '(' + a.key + ')').join(' · ')
}
</script>

<template>
  <div v-if="d" class="h-full overflow-y-auto">
    <div class="mx-auto max-w-3xl px-6 pt-8 pb-20">
      <div class="rounded-2xl border border-border/75 bg-card/50 p-7 sm:p-9 shadow-xs backdrop-blur-xs">
        <!-- 封面 -->
        <header class="border-b border-border/80 pb-6">
          <Input
            class="h-auto rounded-lg border border-transparent bg-transparent px-2 py-1 font-serif text-[28px] leading-snug font-extrabold shadow-none hover:border-border/80 focus:border-ring focus:bg-card text-foreground"
            :model-value="title"
            placeholder="故事书标题"
            @update:model-value="d.meta.title = String($event)"
          />
          <div class="mt-2.5 flex flex-wrap gap-2 px-2">
            <Badge v-if="d.meta.author" variant="secondary" class="border-border/60 text-[11px] font-normal text-muted-foreground">作者 {{ d.meta.author }}</Badge>
            <Badge variant="secondary" class="border-border/60 text-[11px] font-normal text-muted-foreground">{{ d.meta.language ?? 'zh-CN' }}</Badge>
            <Badge variant="secondary" class="border-border/60 text-[11px] font-normal text-muted-foreground font-mono">schema v{{ d.schema_version }}</Badge>
          </div>
        </header>

        <!-- 世界 · 前提 -->
        <section class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border/70 pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">世界 · 前提</h2>
          </div>
          <DocText :model-value="d.world.premise" placeholder="（空白）写这个世界的样子…" :rows="5" @update:model-value="d.world.premise = $event" />
        </section>

        <!-- 地点 -->
        <section v-if="d.world.locations.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border/70 pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">地点</h2>
          </div>
          <div class="flex flex-col gap-2.5">
            <DocBlock v-for="loc in d.world.locations" :key="loc.id" :title="loc.name" :rows="[{ label: 'id', value: loc.id }]" :aside="'地点'">
              <DocMarkdown v-if="loc.description" :value="loc.description" />
              <span v-else class="text-xs text-muted-foreground/60">（无描述）</span>
            </DocBlock>
          </div>
        </section>

        <!-- 人物（可编辑叙事字段） -->
        <section v-if="d.characters.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border/70 pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">人物</h2>
          </div>
          <div class="flex flex-col gap-3">
            <div v-for="c in d.characters" :key="c.id" class="rounded-xl border border-border/75 bg-card/80 px-4.5 py-4 shadow-xs">
              <div class="mb-2.5 flex items-center gap-2.5">
                <strong class="font-serif text-base text-foreground font-semibold">{{ c.name }}</strong>
                <Badge :variant="c.kind === 'pc' ? 'default' : 'outline'" class="text-[10px] font-medium">{{ c.kind === 'pc' ? '受控主角' : 'NPC' }}</Badge>
              </div>
              <label class="mb-0.5 mt-1 block text-[11px] font-medium tracking-wider text-muted-foreground/75">背景故事</label>
              <DocText :model-value="c.background" placeholder="（空白）出身、过往、动机…" :rows="2" @update:model-value="c.background = $event" />
              <label class="mb-0.5 mt-2.5 block text-[11px] font-medium tracking-wider text-muted-foreground/75">性格特质</label>
              <DocText :model-value="c.personality" placeholder="（空白）性格特质与行为准则…" :rows="2" @update:model-value="c.personality = $event" />
              <div v-if="Object.keys(c.attributes ?? {}).length" class="mt-2.5 rounded-lg bg-muted/30 px-2.5 py-1 text-[11.5px] text-muted-foreground">
                <span class="font-semibold text-foreground/80 mr-1.5">属性：</span>{{ Object.entries(c.attributes).map(([k, v]) => k + ' ' + v).join(' · ') }}
              </div>
            </div>
          </div>
        </section>

        <!-- 技能 -->
        <section v-if="d.skills.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border/70 pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">技能</h2>
          </div>
          <div class="flex flex-col gap-2.5">
            <DocBlock v-for="s in d.skills" :key="s.id" :title="s.name" :aside="s.category ?? '技能'">
              <DocText :model-value="s.description" :rows="1" @update:model-value="s.description = $event" />
            </DocBlock>
          </div>
        </section>

        <!-- 物品 -->
        <section v-if="d.items.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border/70 pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">物品</h2>
          </div>
          <div class="flex flex-col gap-2.5">
            <DocBlock v-for="it in d.items" :key="it.id" :title="it.name" :aside="it.type ?? '物品'">
              <DocText :model-value="it.description" :rows="1" @update:model-value="it.description = $event" />
            </DocBlock>
          </div>
        </section>

        <!-- 物件（可编辑描述） -->
        <section v-if="d.objects.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border/70 pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">场景物件</h2>
          </div>
          <div class="flex flex-col gap-2.5">
            <DocBlock v-for="o in d.objects" :key="o.id" :title="o.name" :rows="[{ label: '地点', value: locName(o.location_id) }, { label: '动作', value: actionText(o) }]" aside="物件">
              <DocText :model-value="o.description ?? ''" placeholder="（空白）" :rows="1" @update:model-value="o.description = $event" />
            </DocBlock>
          </div>
        </section>

        <footer class="mt-12 border-t border-dashed border-border/70 pt-4 text-[11.5px] leading-relaxed text-muted-foreground/70">
          📜 B 文档视图 = 叙事自由书写载体：编辑即写草稿（其他范式同步可见）。叙事字段存储为 Markdown，结构化字段可到 A 表单工作台精修。
        </footer>
      </div>
    </div>
  </div>
</template>
