<script setup lang="ts">
// NarrativePrefsDialog —— 游玩页「叙述偏好」（叙事契约 P1）：
// 只列出故事书里 playerEditable=true 的段；玩家可开关 / 选变体，写入存档设置。
// 偏好只影响之后的回合，不回写命令日志 / 历史。
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import type { NarrativeSection, Storybook } from '@/types'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Switch } from '@/components/ui/switch'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Label } from '@/components/ui/label'

const open = defineModel<boolean>({ default: false })
const store = usePlayStore()

const sections = computed<NarrativeSection[]>(() => {
  const sb = store.detail?.storybook as Storybook | undefined
  return (sb?.narrative?.sections ?? []).filter(s => s.playerEditable === true && s.enabled !== false)
})

/** 段是否生效：玩家显式关闭才是 false，否则沿用故事书缺省（启用）。 */
function enabledOf(s: NarrativeSection): boolean {
  return store.narrativePrefs[s.id] !== false
}
/** 当前变体：玩家选择优先，否则 defaultVariant / 首个变体。 */
function variantOf(s: NarrativeSection): string {
  const v = store.narrativePrefs[s.id]
  if (typeof v === 'string') return v
  return s.defaultVariant ?? s.variants?.[0]?.key ?? ''
}
function toggle(s: NarrativeSection, on: boolean): void {
  void store.setNarrativeOverride(s.id, on)
}
function pickVariant(s: NarrativeSection, key: string): void {
  void store.setNarrativeOverride(s.id, key)
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-w-lg">
      <DialogHeader>
        <DialogTitle>叙述偏好</DialogTitle>
        <DialogDescription>
          作者开放给你的叙述段：可以开关，或选择表达方式。只影响之后的回合，不会改写已经发生的剧情。
        </DialogDescription>
      </DialogHeader>
      <div class="max-h-[60vh] overflow-y-auto pr-1">
        <div v-if="!sections.length" class="py-8 text-center text-xs text-muted-foreground/70">
          这本故事书没有开放给玩家调整的叙述段。
        </div>
        <div v-for="s in sections" :key="s.id" class="mb-2.5 rounded-xl border border-border/80 bg-card/90 p-3 shadow-2xs">
          <div class="flex items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="truncate text-[13px] font-bold text-foreground">{{ s.title || s.id }}</div>
              <div class="font-mono text-[10.5px] text-muted-foreground/70">{{ s.id }}</div>
            </div>
            <Switch
              v-if="!(s.variants?.length)"
              :model-value="enabledOf(s)"
              title="开启 / 关闭该叙述段"
              @update:model-value="(v: boolean) => toggle(s, v)"
            />
          </div>

          <RadioGroup
            v-if="s.variants?.length"
            class="mt-2.5 grid gap-2"
            :model-value="variantOf(s)"
            :disabled="!enabledOf(s)"
            @update:model-value="(v: unknown) => pickVariant(s, String(v))"
          >
            <div
              v-for="v in (s.variants ?? [])"
              :key="v.key"
              class="flex cursor-pointer items-start gap-2 rounded-lg border p-2.5 text-[11.5px] transition-all"
              :class="variantOf(s) === v.key ? 'border-primary bg-primary/10 ring-1 ring-primary/30' : 'border-border/70 hover:border-border'"
            >
              <RadioGroupItem :id="s.id + '-' + v.key" :value="v.key" class="mt-0.5 size-4" />
              <Label :for="s.id + '-' + v.key" class="flex cursor-pointer flex-col items-start gap-0.5 font-normal">
                <span class="font-bold text-foreground">{{ v.label || v.key }}</span>
                <span class="line-clamp-2 text-[10px] text-muted-foreground">{{ v.text }}</span>
              </Label>
            </div>
          </RadioGroup>
          <div v-if="s.variants?.length" class="mt-2 flex items-center gap-2">
            <Switch :model-value="enabledOf(s)" title="开启 / 关闭该叙述段" @update:model-value="(v: boolean) => toggle(s, v)" />
            <span class="text-[10.5px] text-muted-foreground">启用该段</span>
          </div>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
