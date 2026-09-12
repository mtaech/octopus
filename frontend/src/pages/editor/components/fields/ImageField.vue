<script setup lang="ts">
// ImageField —— 图片字段（#28）：拖拽 / 点击 / 粘贴上传。
// 上传前先用 canvas 压到目标最长边（避免把 10MB 原图直传），再交给内容寻址的资产库。
import { computed, ref } from 'vue'
import { assetUrl, toast, uploadAsset } from '@/api'
import type { AssetRef } from '@/types'
import { Button } from '@/components/ui/button'
import { IconLoader2, IconPhoto, IconTrash, IconUpload } from '@tabler/icons-vue'

const props = withDefaults(defineProps<{
  label: string
  modelValue?: AssetRef | null
  hint?: string
  /** 压缩目标：最长边像素（封面 1600 / 立绘 1024 / 图标 512） */
  maxEdge?: number
  /** 预览框比例：'16/9' | '1/1' | '3/4' */
  ratio?: string
  /** 预览框最大宽度 */
  maxWidth?: string
}>(), { modelValue: null, hint: '', maxEdge: 1280, ratio: '16/9', maxWidth: '18rem' })

const emit = defineEmits<{ 'update:modelValue': [value: AssetRef | null] }>()

const inputEl = ref<HTMLInputElement | null>(null)
const busy = ref(false)
const dragging = ref(false)

const preview = computed(() => (props.modelValue?.asset ? assetUrl(props.modelValue.asset) : ''))

/** canvas 压缩：等比缩到 maxEdge，优先 webp */
async function compress(file: File): Promise<{ blob: Blob; w: number; h: number }> {
  const bitmap = await createImageBitmap(file)
  const scale = Math.min(1, props.maxEdge / Math.max(bitmap.width, bitmap.height))
  const w = Math.max(1, Math.round(bitmap.width * scale))
  const h = Math.max(1, Math.round(bitmap.height * scale))
  const canvas = document.createElement('canvas')
  canvas.width = w
  canvas.height = h
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('浏览器不支持 canvas，无法压缩图片')
  ctx.drawImage(bitmap, 0, 0, w, h)
  bitmap.close?.()
  const blob = (await new Promise<Blob | null>(res => canvas.toBlob(res, 'image/webp', 0.9)))
    ?? (await new Promise<Blob | null>(res => canvas.toBlob(res, 'image/jpeg', 0.9)))
  if (!blob) throw new Error('图片编码失败')
  return { blob, w, h }
}

async function accept(file: File | null | undefined): Promise<void> {
  if (!file) return
  if (!file.type.startsWith('image/')) {
    toast('warn', '请选择图片文件')
    return
  }
  busy.value = true
  try {
    const { blob, w, h } = await compress(file)
    const up = await uploadAsset(blob)
    emit('update:modelValue', { asset: up.asset, w, h })
    toast('ok', `图片已上传（${w}×${h}）`)
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  } finally {
    busy.value = false
  }
}

function onPick(e: Event): void {
  const el = e.target as HTMLInputElement
  void accept(el.files?.[0])
  el.value = ''
}
function onDrop(e: DragEvent): void {
  dragging.value = false
  void accept(e.dataTransfer?.files?.[0])
}
function onPaste(e: ClipboardEvent): void {
  const item = Array.from(e.clipboardData?.items ?? []).find(i => i.type.startsWith('image/'))
  if (item) void accept(item.getAsFile())
}
function clear(): void { emit('update:modelValue', null) }
</script>

<template>
  <div class="flex min-w-0 flex-col gap-1.5">
    <span class="text-[11px] leading-4 font-medium text-muted-foreground">{{ label }}</span>

    <div
      class="group relative flex cursor-pointer items-center justify-center overflow-hidden rounded-lg border border-dashed border-border/70 bg-muted/25 transition-colors hover:border-primary/45 hover:bg-primary/5 focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
      :class="dragging && 'border-primary/60 bg-primary/10'"
      :style="{ aspectRatio: ratio, maxWidth }"
      role="button"
      tabindex="0"
      :aria-label="label"
      @click="inputEl?.click()"
      @keydown.enter.prevent="inputEl?.click()"
      @keydown.space.prevent="inputEl?.click()"
      @paste="onPaste"
      @dragover.prevent="dragging = true"
      @dragleave.prevent="dragging = false"
      @drop.prevent="onDrop"
    >
      <img v-if="preview" :src="preview" alt="" class="size-full object-cover" />
      <div v-else class="flex flex-col items-center gap-1.5 px-3 text-center">
        <IconPhoto class="size-5 text-muted-foreground/50" />
        <span class="text-[11px] leading-4 text-muted-foreground/70">拖拽 / 点击 / 粘贴图片</span>
      </div>

      <div v-if="busy" class="absolute inset-0 flex items-center justify-center bg-background/70 backdrop-blur-sm">
        <IconLoader2 class="size-5 animate-spin text-primary" />
      </div>
    </div>

    <div class="flex flex-wrap items-center gap-2">
      <Button variant="outline" size="sm" class="h-7 gap-1 text-xs" :disabled="busy" @click.stop="inputEl?.click()">
        <IconUpload class="size-3.5" />
        {{ modelValue ? '替换' : '上传' }}
      </Button>
      <Button
        v-if="modelValue"
        variant="ghost"
        size="sm"
        class="h-7 gap-1 text-xs text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
        @click.stop="clear"
      >
        <IconTrash class="size-3.5" />
        移除
      </Button>
      <span v-if="modelValue" class="font-mono text-[10.5px] text-muted-foreground/60">{{ modelValue.w }}×{{ modelValue.h }}</span>
    </div>

    <span v-if="hint" class="text-[10.5px] leading-4 text-muted-foreground/65">{{ hint }}</span>
    <span v-if="busy" class="text-[10.5px] leading-4 text-primary/80">压缩并上传中…</span>
    <input ref="inputEl" type="file" accept="image/*" class="hidden" @change="onPick" />
  </div>
</template>
