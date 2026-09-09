<script setup lang="ts">
// 新建游戏 = 先确认再开档（规格点 3，#25 Answer ⑤）
// 弹窗内：选故事书（只列已发布）+ 起名（默认书名、可一键确认）
// 呈现层：shadcn-vue Dialog / Label / Input / Select（夜行手记主题）；逻辑不动。
import { computed, ref, watch } from 'vue'
import { getStorybook } from '@/api'
import type { CharacterDef, StorybookListItem } from '@/types'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Input } from '@/components/ui/input'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select'
import { IconBook2, IconPlayerPlay, IconSparkles } from '@tabler/icons-vue'

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

/** 取已发布故事书里的 PC 列表（#01 修订：kind = pc 才可被玩家控制） */
async function loadPcs(id: string) {
  pcs.value = []
  controlledId.value = ''
  if (!id) return
  try {
    const doc = await getStorybook(id)
    const src = doc.released ?? doc.draft
    pcs.value = src.characters.filter(c => c.kind === 'pc')
    controlledId.value = pcs.value[0]?.id ?? ''
  } catch { /* 忽略：仍可开档，主角默认由后端决定 */ }
}

const selectedBook = computed(() => props.storybooks.find(s => s.id === selectedId.value) ?? null)

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
    <DialogContent class="sm:max-w-md" :show-close-button="!busy">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-xl">
          <IconSparkles aria-hidden="true" class="size-5 text-primary" />
          新建游戏
        </DialogTitle>
        <DialogDescription>
          基于一本已发布故事书的当前版次开档，进入后即开始游玩。
        </DialogDescription>
      </DialogHeader>

      <template v-if="storybooks.length">
        <div class="flex flex-col gap-2">
          <Label for="ng-sb" class="text-xs font-semibold text-muted-foreground">故事书</Label>
          <Select :model-value="selectedId" :disabled="busy" @update:model-value="(value) => { if (typeof value === 'string') { selectedId = value; pickStorybook() } }">
            <SelectTrigger id="ng-sb" class="w-full">
              <SelectValue placeholder="选择一本已发布故事书" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="sb in storybooks" :key="sb.id" :value="sb.id">
                  <span class="inline-flex items-center gap-2">
                    <IconBook2 aria-hidden="true" class="size-4 text-muted-foreground" />
                    {{ sb.title }}
                    <span class="text-muted-foreground">· 版次 {{ sb.revision }}</span>
                  </span>
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>

        <div class="flex flex-col gap-2">
          <Label for="ng-name" class="text-xs font-semibold text-muted-foreground">存档名称（可改）</Label>
          <Input
            id="ng-name"
            v-model="name"
            type="text"
            :placeholder="selectedBook?.title ?? '给这次冒险起个名字'"
            :disabled="busy"
            @keyup.enter="confirm"
          />
        </div>

        <div v-if="pcs.length" class="flex flex-col gap-2">
          <Label for="ng-pc" class="text-xs font-semibold text-muted-foreground">主角（受控角色）</Label>
          <Select :model-value="controlledId" :disabled="busy" @update:model-value="(value) => { if (typeof value === 'string') controlledId = value }">
            <SelectTrigger id="ng-pc" class="w-full">
              <SelectValue placeholder="选择一位主角" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="pc in pcs" :key="pc.id" :value="pc.id">{{ pc.name }}</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <p class="text-[11px] text-muted-foreground/70">进入游戏后也可随时切换受控角色。</p>
        </div>
      </template>

      <DialogFooter class="gap-2 sm:justify-end">
        <Button variant="ghost" :disabled="busy" @click="close">取消</Button>
        <Button :disabled="busy || !selectedBook" @click="confirm">
          <IconPlayerPlay data-icon="inline-start" />
          {{ busy ? '开档中…' : '开档并进入游玩' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
