<script setup lang="ts">
// 新建游戏 = 先确认再开档（规格点 3，#25 Answer ⑤）
// 弹窗内：选故事书（只列已发布）+ 起名（默认书名、可一键确认）
// 呈现层：shadcn-vue Dialog / Label / Input / Select（Material 3 · 靛蓝主题）；逻辑不动。
import { computed, ref, watch } from 'vue'
import { getStorybook } from '@/api'
import type { CharacterDef, Storybook, StorybookListItem } from '@/types'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Label } from '@/components/ui/label'
import { Input } from '@/components/ui/input'
import StorybookCover from '@/components/StorybookCover.vue'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select'
import { IconBook2, IconLoader2, IconPlayerPlay, IconSparkles, IconUser } from '@tabler/icons-vue'

const props = defineProps<{
  open: boolean
  storybooks: StorybookListItem[]
  /** 打开时预选的故事书 id（来自故事书卡「新建游戏」） */
  preselectId?: string | null
  busy?: boolean
}>()
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'confirm', payload: { storybookId: string; title: string; controlledCharacterId?: string }): void
}>()

const selectedId = ref('')
const name = ref('')
const pcs = ref<CharacterDef[]>([])
const controlledId = ref('')
/** 选中故事书的已发布正文（用于预览卡的简介 / 规模统计） */
const detail = ref<Storybook | null>(null)

/** 取已发布故事书里的 PC 列表（#01 修订：kind = pc 才可被玩家控制） */
async function loadPcs(id: string) {
  pcs.value = []
  controlledId.value = ''
  detail.value = null
  if (!id) return
  try {
    const doc = await getStorybook(id)
    const src = doc.released ?? doc.draft
    detail.value = src
    pcs.value = src.characters.filter(c => c.kind === 'pc')
    controlledId.value = pcs.value[0]?.id ?? ''
  } catch { /* 忽略：仍可开档，主角默认由后端决定 */ }
}

const selectedBook = computed(() => props.storybooks.find(s => s.id === selectedId.value) ?? null)
/** 规模统计：让人在开档前就知道这本故事书有多大。 */
const scale = computed(() => {
  const d = detail.value
  if (!d) return null
  return { chars: d.characters.length, skills: d.skills.length, items: d.items.length }
})
const nameIsDefault = computed(() => name.value.trim() === (selectedBook.value?.title ?? ''))

// 每次打开时：默认选中第一本已发布书（或预选），名字默认书名，可一键确认
watch(() => props.open, (open) => {
  if (!open) return
  const pre = props.preselectId && props.storybooks.some(s => s.id === props.preselectId) ? props.preselectId : null
  const sb = props.storybooks.find(s => s.id === (pre ?? props.storybooks[0]?.id))
  if (!sb) return
  selectedId.value = sb.id
  name.value = sb.title
  void loadPcs(sb.id)
}, { immediate: true })

function pickStorybook() {
  const sb = props.storybooks.find(s => s.id === selectedId.value)
  if (sb) name.value = sb.title
  void loadPcs(selectedId.value)
}

function close() { if (!props.busy) emit('update:open', false) }

function confirm() {
  if (!selectedBook.value) return
  const title = name.value.trim() || selectedBook.value.title
  emit('confirm', { storybookId: selectedBook.value.id, title, controlledCharacterId: controlledId.value || undefined })
}
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => close()">
    <DialogContent class="sm:max-w-lg gap-0 p-0 border-border/80 bg-card/95 backdrop-blur-md" :show-close-button="!busy">
      <DialogHeader class="border-b border-border/70 px-5 py-4">
        <DialogTitle class="flex items-center gap-2 font-serif text-xl text-foreground">
          <div class="flex size-7 items-center justify-center rounded-lg bg-primary/15 text-primary border border-primary/25">
            <IconSparkles aria-hidden="true" class="size-4" />
          </div>
          新建游戏
        </DialogTitle>
        <DialogDescription class="text-xs text-muted-foreground/85">
          基于一本已发布故事书的当前版次开档，进入后即开始游玩。
        </DialogDescription>
      </DialogHeader>

      <div class="max-h-[68vh] space-y-4 overflow-y-auto px-5 py-4">
        <template v-if="storybooks.length">
          <!-- ① 选故事书 + 选中预览 -->
          <div class="space-y-2">
            <Label for="ng-sb" class="text-xs font-semibold text-muted-foreground">故事书设定</Label>
            <Select :model-value="selectedId" :disabled="busy" @update:model-value="(value) => { if (typeof value === 'string') { selectedId = value; pickStorybook() } }">
              <SelectTrigger id="ng-sb" class="w-full">
                <SelectValue placeholder="选择一本已发布故事书" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem v-for="sb in storybooks" :key="sb.id" :value="sb.id">
                    <span class="inline-flex items-center gap-2">
                      <IconBook2 aria-hidden="true" class="size-4 text-primary/80" />
                      <span class="font-medium">{{ sb.title }}</span>
                      <span class="font-mono text-[11px] text-muted-foreground">rev {{ sb.revision }}</span>
                    </span>
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>

            <div v-if="selectedBook" class="flex gap-3 rounded-xl border border-border/70 bg-muted/20 p-3">
              <StorybookCover
                :seed="selectedBook.id"
                :title="selectedBook.title"
                :cover="selectedBook.cover"
                size="lg"
                class="h-18 w-14 shrink-0 rounded-lg"
              />
              <div class="min-w-0 flex-1 space-y-1.5">
                <div class="flex items-center gap-2">
                  <span class="truncate text-[13.5px] font-bold text-foreground">{{ selectedBook.title }}</span>
                  <Badge variant="outline" class="shrink-0 px-1.5 font-mono text-[10.5px] font-normal text-muted-foreground">
                    rev {{ selectedBook.revision }}
                  </Badge>
                </div>
                <p v-if="selectedBook.description" class="line-clamp-2 text-[11.5px] leading-relaxed text-muted-foreground/80">
                  {{ selectedBook.description }}
                </p>
                <p v-else class="text-[11.5px] text-muted-foreground/50">这本故事书还没有简介</p>
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10.5px] text-muted-foreground/70">
                  <span v-if="scale">{{ scale.chars }} 位人物 · {{ scale.skills }} 项技能 · {{ scale.items }} 件物品</span>
                  <span v-if="pcs.length" class="text-primary/80">{{ pcs.length }} 位可扮演主角</span>
                </div>
              </div>
            </div>
          </div>

          <!-- ② 存档名 -->
          <div class="space-y-2">
            <div class="flex items-center justify-between">
              <Label for="ng-name" class="text-xs font-semibold text-muted-foreground">存档名称</Label>
              <button
                v-if="!nameIsDefault"
                type="button"
                class="cursor-pointer text-[11px] text-primary/80 transition-colors hover:text-primary"
                @click="name = selectedBook?.title ?? ''"
              >用书名</button>
            </div>
            <Input
              id="ng-name"
              v-model="name"
              type="text"
              :placeholder="selectedBook?.title ?? '给这次冒险起个名字'"
              :disabled="busy"
              @keyup.enter="confirm"
            />
          </div>

          <!-- ③ 主角 -->
          <div v-if="pcs.length" class="space-y-2">
            <div class="flex items-center justify-between">
              <Label for="ng-pc" class="text-xs font-semibold text-muted-foreground">主角（受控角色）</Label>
              <span class="text-[11px] text-muted-foreground/70">游戏中可随时切换</span>
            </div>
            <Select :model-value="controlledId" :disabled="busy" @update:model-value="(value) => { if (typeof value === 'string') controlledId = value }">
              <SelectTrigger id="ng-pc" class="w-full">
                <SelectValue placeholder="选择一位主角" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem v-for="pc in pcs" :key="pc.id" :value="pc.id">
                    <span class="inline-flex items-center gap-2">
                      <IconUser aria-hidden="true" class="size-3.5 text-muted-foreground" />
                      <span class="font-medium">{{ pc.name }}</span>
                    </span>
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
        </template>

        <!-- 一本已发布故事书都没有：开档无对象，给出明确出路（草稿需先发布） -->
        <div v-else class="rounded-xl border border-dashed border-border/80 bg-muted/20 px-4 py-6 text-center text-[12.5px] leading-relaxed text-muted-foreground">
          还没有已发布的故事书，暂时无法开档。<br />
          请先在「我的故事书」里创作一本并<strong class="text-foreground/85">发布</strong>，之后即可开档游玩。
        </div>
      </div>

      <DialogFooter class="gap-2 border-t border-border/70 px-5 py-3 sm:justify-end">
        <Button variant="ghost" :disabled="busy" @click="close">取消</Button>
        <Button :disabled="busy || !selectedBook" class="shadow-sm transition-all" @click="confirm">
          <IconLoader2 v-if="busy" data-icon="inline-start" class="size-4 animate-spin" />
          <IconPlayerPlay v-else data-icon="inline-start" class="size-4" />
          {{ busy ? '开档中…' : '开档并开始游玩' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
