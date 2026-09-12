<script setup lang="ts">
// PlaytestDialog —— 沙箱试玩弹窗（免发布草稿即开即玩）
import { computed, ref, watch } from 'vue'
import type { CharacterDef } from '@/types'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Input } from '@/components/ui/input'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select'
import { IconAlertCircle, IconCircleCheck, IconDeviceGamepad2, IconLoader2, IconPlayerPlay, IconUser } from '@tabler/icons-vue'

const props = defineProps<{
  open: boolean
  storybookTitle: string
  characters: CharacterDef[]
  errorCount: number
  warningCount: number
  busy?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'confirm', payload: { title?: string; controlledCharacterId?: string }): void
}>()

const name = ref('')
const controlledId = ref('')

watch(() => props.open, (open) => {
  if (!open) return
  name.value = `【沙箱试玩】${props.storybookTitle || '新故事书'}`
  // 优先选第一个 PC，没有则选第一个角色
  const defaultPc = props.characters.find(c => c.kind === 'pc') ?? props.characters[0]
  controlledId.value = defaultPc?.id ?? ''
}, { immediate: true })

const selectedChar = computed(() => props.characters.find(c => c.id === controlledId.value) ?? null)

function close() {
  if (!props.busy) emit('update:open', false)
}

function confirm() {
  if (props.errorCount > 0 || props.busy) return
  emit('confirm', {
    title: name.value.trim() || undefined,
    controlledCharacterId: controlledId.value || undefined,
  })
}
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => close()">
    <DialogContent class="sm:max-w-md border-border/80 bg-card/95 backdrop-blur-md" :show-close-button="!busy">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-xl text-foreground">
          <div class="flex size-7 items-center justify-center rounded-lg bg-primary/15 text-primary border border-primary/25">
            <IconDeviceGamepad2 aria-hidden="true" class="size-4" />
          </div>
          沙箱试玩 (Playground)
        </DialogTitle>
        <DialogDescription class="text-xs text-muted-foreground/85">
          直接使用当前草稿开档试玩，无需正式发布。验证剧情分支、人物属性与判定机制。
        </DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-4 py-1">
        <!-- 校验健康状态横幅 -->
        <div
          v-if="errorCount > 0"
          class="flex items-start gap-2.5 rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive"
        >
          <IconAlertCircle class="mt-0.5 size-4 shrink-0" />
          <div class="min-w-0">
            <div class="font-semibold">草稿存在 {{ errorCount }} 个阻断性错误</div>
            <div class="mt-0.5 opacity-90">为避免游戏运行时异常，请先在右侧校验面板中修复错误后再试玩。</div>
          </div>
        </div>

        <div
          v-else
          class="flex items-center gap-2 rounded-lg border border-success/30 bg-success/10 px-3 py-2 text-xs text-success"
        >
          <IconCircleCheck class="size-4 shrink-0" />
          <span>草稿校验通过<span v-if="warningCount > 0" class="text-muted-foreground">（有 {{ warningCount }} 条警告，不影响试玩）</span>，随时可以开跑。</span>
        </div>

        <!-- 试玩存档标题 -->
        <div class="flex flex-col gap-2">
          <Label for="sbx-name" class="text-xs font-semibold text-muted-foreground">试玩存档名称</Label>
          <Input
            id="sbx-name"
            v-model="name"
            type="text"
            placeholder="试玩存档名称"
            :disabled="busy"
            @keyup.enter="confirm"
          />
        </div>

        <!-- 受控主角选择 -->
        <div v-if="characters.length" class="flex flex-col gap-2">
          <div class="flex items-center justify-between">
            <Label for="sbx-pc" class="text-xs font-semibold text-muted-foreground">试玩主角（受控角色）</Label>
            <span v-if="selectedChar && selectedChar.kind !== 'pc'" class="text-[10.5px] text-warning">
              临时作为 PC 受控
            </span>
          </div>
          <Select :model-value="controlledId" :disabled="busy" @update:model-value="(value) => { if (typeof value === 'string') controlledId = value }">
            <SelectTrigger id="sbx-pc" class="w-full">
              <SelectValue placeholder="选择一位受控角色" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="c in characters" :key="c.id" :value="c.id">
                  <span class="inline-flex items-center gap-2">
                    <IconUser aria-hidden="true" class="size-3.5 text-muted-foreground" />
                    {{ c.name }}
                    <span class="text-[11px] font-mono text-muted-foreground/70">
                      ({{ c.kind === 'pc' ? 'PC' : 'NPC' }})
                    </span>
                  </span>
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
      </div>

      <DialogFooter class="gap-2 sm:justify-end pt-2">
        <Button variant="ghost" :disabled="busy" @click="close">取消</Button>
        <Button
          :disabled="busy || errorCount > 0"
          class="shadow-sm transition-all"
          :class="errorCount === 0 ? 'bg-primary text-primary-foreground hover:bg-primary/90' : ''"
          @click="confirm"
        >
          <IconLoader2 v-if="busy" data-icon="inline-start" class="size-4 animate-spin" />
          <IconPlayerPlay v-else data-icon="inline-start" class="size-4" />
          {{ busy ? '正在生成沙箱…' : '开始沙箱试玩' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
