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
import CodeEditor from '@/components/CodeEditor.vue'
import { buildLuaContext } from '@/lib/lua-context'
import type { ObjectDef } from '@/types'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { IconFileText, IconPencil } from '@tabler/icons-vue'

import { assetUrl } from '@/api'
import { nameTintClass, initial } from '@/pages/play/utils'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const luaContext = computed(() => buildLuaContext(d.value))

const errCount = computed(() => editor.issues.filter(i => i.severity === 'error').length)
const title = computed(() => d.value?.meta.title ?? '')

function dimLabel(key: string): string {
  return d.value?.attribute_dimensions.find(x => x.key === key)?.label ?? key
}

function locName(id?: string): string {
  if (!id) return '（未指定）'
  return d.value?.world.locations.find(l => l.id === id)?.name ?? id
}
function actionText(o: ObjectDef): string {
  const acts = o.actions ?? []
  if (!acts.length) return '（无动作）'
  return acts.map(a => a.label + '(' + a.key + ')').join(' · ')
}
/** 人物技能 id → 技能显示名（找不到回落为 id） */
function skillNames(ids?: string[]): string[] {
  return (ids ?? []).map(id => d.value?.skills.find(s => s.id === id)?.name ?? id)
}
</script>

<template>
  <div v-if="d" class="h-full overflow-y-auto">
    <div class="mx-auto max-w-3xl px-6 pt-8 pb-20">
      <div class="rounded-2xl border border-border bg-card/50 p-7 sm:p-9 shadow-xs backdrop-blur-xs">
        <!-- 封面 -->
        <header class="border-b border-border pb-6">
          <div class="flex items-center gap-2">
            <Input
              class="h-auto flex-1 rounded-lg border border-dashed border-border bg-muted/25 px-2 py-1 font-serif text-[28px] leading-snug font-extrabold text-foreground shadow-none transition-colors hover:border-solid hover:border-primary/60 hover:bg-primary/5 focus:border-solid focus:border-ring focus:bg-card"
              title="点击可修改标题"
              :model-value="title"
              placeholder="故事书标题"
              @update:model-value="d.meta.title = String($event)"
            />
            <IconPencil class="size-4 shrink-0 text-muted-foreground/45" />
          </div>
          <div class="mt-2.5 flex flex-wrap gap-2 px-2">
            <Badge v-if="d.meta.author" variant="outline" class="border-border text-[11px] font-normal text-muted-foreground">作者 {{ d.meta.author }}</Badge>
            <Badge variant="outline" class="border-border text-[11px] font-normal text-muted-foreground">{{ d.meta.language ?? 'zh-CN' }}</Badge>
            <Badge variant="outline" class="border-border text-[11px] font-normal text-muted-foreground font-mono">schema v{{ d.schema_version }}</Badge>
          </div>
        </header>

        <!-- 故事 · 开头 -->
        <section class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">故事 · 开头</h2>
          </div>
          <DocText :model-value="d.world.opening ?? ''" placeholder="（空白）开档后玩家读到的第一段文字；缺省回落为世界前提…" :rows="4" @update:model-value="d.world.opening = $event" />
        </section>

        <!-- 世界 · 前提 -->
        <section class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">世界 · 前提</h2>
          </div>
          <DocText :model-value="d.world.premise" placeholder="（空白）写这个世界的样子…" :rows="5" @update:model-value="d.world.premise = $event" />
        </section>

        <!-- 地点 -->
        <section v-if="d.world.locations.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
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
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">人物</h2>
          </div>
          <div class="flex flex-col gap-3">
            <div
              v-for="c in d.characters"
              :key="c.id"
              class="rounded-xl border transition-all duration-200 p-4 shadow-xs"
              :class="c.kind === 'pc' ? 'border-primary/40 bg-gradient-to-b from-primary/5 via-card/90 to-card' : 'border-border bg-card/80'"
            >
              <!-- 头部：头像 + 姓名 + 身份标签 + ID -->
              <div class="mb-3 flex items-center gap-3">
                <div class="relative size-10 shrink-0 overflow-hidden rounded-lg border border-border/80 bg-muted/30 shadow-2xs">
                  <img
                    v-if="c.portrait"
                    :src="assetUrl(c.portrait.asset)"
                    :alt="c.name"
                    class="size-full object-cover"
                    loading="lazy"
                  />
                  <div
                    v-else
                    class="flex size-full items-center justify-center text-sm font-extrabold shadow-inner"
                    :class="nameTintClass(c.name)"
                  >
                    {{ initial(c.name) }}
                  </div>
                </div>

                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2">
                    <strong class="truncate font-serif text-base font-bold text-foreground">{{ c.name || '（未命名人物）' }}</strong>
                    <Badge
                      :variant="c.kind === 'pc' ? 'default' : 'outline'"
                      class="text-[10px] font-semibold"
                      :class="c.kind === 'pc' ? 'bg-primary text-primary-foreground shadow-2xs' : 'border-border/70 text-muted-foreground'"
                    >
                      {{ c.kind === 'pc' ? '👑 受控主角' : 'NPC' }}
                    </Badge>
                  </div>
                  <div class="mt-0.5 font-mono text-[10.5px] text-muted-foreground/70">{{ c.id }}</div>
                </div>
              </div>

              <!-- 叙事字段：背景与性格紧凑并列 -->
              <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
                <div class="rounded-lg border border-border/60 bg-muted/20 p-2.5">
                  <span class="mb-1 block text-[11px] font-semibold tracking-wider text-muted-foreground">📜 背景故事</span>
                  <DocText :model-value="c.background" placeholder="（空白）出身、过往、动机…" :rows="2" @update:model-value="c.background = $event" />
                </div>
                <div class="rounded-lg border border-border/60 bg-muted/20 p-2.5">
                  <span class="mb-1 block text-[11px] font-semibold tracking-wider text-muted-foreground">🎭 性格特质</span>
                  <DocText :model-value="c.personality" placeholder="（空白）性格特质与行为准则…" :rows="2" @update:model-value="c.personality = $event" />
                </div>
              </div>

              <!-- 属性胶囊列 -->
              <div v-if="Object.keys(c.attributes ?? {}).length" class="mt-3 flex flex-wrap items-center gap-1.5 pt-2 border-t border-dashed border-border/60">
                <span class="text-[11px] font-medium text-muted-foreground/80 mr-1">属性：</span>
                <span
                  v-for="[k, v] in Object.entries(c.attributes)"
                  :key="k"
                  class="inline-flex items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-2 py-0.5 font-mono text-[11px]"
                >
                  <span class="text-muted-foreground">{{ dimLabel(k) }}</span>
                  <b class="font-bold text-foreground">{{ v }}</b>
                </span>
              </div>

              <!-- 掌握技能 -->
              <div v-if="(c.skills ?? []).length" class="mt-2 flex flex-wrap items-center gap-1.5">
                <span class="text-[11px] font-medium text-muted-foreground/80 mr-1">技能：</span>
                <span
                  v-for="(name, i) in skillNames(c.skills)"
                  :key="i"
                  class="inline-flex items-center rounded-md border border-primary/30 bg-primary/10 px-2 py-0.5 text-[11px] font-semibold text-primary"
                >
                  {{ name }}
                </span>
              </div>
            </div>
          </div>
        </section>

        <!-- 技能 -->
        <section v-if="d.skills.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">技能</h2>
          </div>
          <div class="flex flex-col gap-2.5">
            <DocBlock v-for="s in d.skills" :key="s.id" :title="s.name" :aside="s.category ?? '技能'">
              <DocText :model-value="s.description" :rows="1" @update:model-value="s.description = $event" />
              <div v-if="s.lua" class="mt-2">
                <div class="mb-1 text-[11px] font-semibold tracking-wider text-muted-foreground">Lua 钩子</div>
                <CodeEditor
                  :model-value="s.lua"
                  language="lua"
                  min-height="4rem"
                  max-height="14rem"
                  :context="luaContext"
                  show-reference
                  @update:model-value="s.lua = $event"
                />
              </div>
            </DocBlock>
          </div>
        </section>

        <!-- 物品 -->
        <section v-if="d.items.length" class="mt-8">
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
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
          <div class="mb-3 flex items-center gap-2 border-b border-border pb-2">
            <span class="size-1.5 rounded-full bg-primary" />
            <h2 class="font-serif text-[15px] font-extrabold tracking-wider text-foreground">场景物件</h2>
          </div>
          <div class="flex flex-col gap-2.5">
            <DocBlock v-for="o in d.objects" :key="o.id" :title="o.name" :rows="[{ label: '地点', value: locName(o.location_id) }, { label: '动作', value: actionText(o) }]" aside="物件">
              <DocText :model-value="o.description ?? ''" placeholder="（空白）" :rows="1" @update:model-value="o.description = $event" />
            </DocBlock>
          </div>
        </section>

        <footer class="mt-12 flex items-start gap-2 border-t border-dashed border-border pt-4 text-[11.5px] leading-relaxed text-muted-foreground/70">
          <IconFileText class="mt-0.5 size-3.5 flex-none" />
          <span>文档视图 = 叙事自由书写载体：编辑即写草稿（其他范式同步可见）。叙事字段存储为 Markdown，结构化字段可到表单工作台精修。</span>
        </footer>
      </div>
    </div>
  </div>
</template>
