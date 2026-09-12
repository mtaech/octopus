<script setup lang="ts">
// EntityFormHeader —— 右侧编辑区头部：实体名 + 类型/元信息 + 删除（两步确认，避免误删）
import { ref, onUnmounted, type Component } from 'vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconTrash } from '@tabler/icons-vue'

withDefaults(defineProps<{
  title: string
  sub?: string
  icon?: Component
  meta?: string
  tone?: 'default' | 'warn' | 'ok'
  /** 伪实体（如世界前提）没有删除语义 */
  hideRemove?: boolean
}>(), { sub: '', meta: '', tone: 'default', hideRemove: false })

const emit = defineEmits<{ remove: [] }>()

const confirming = ref(false)
let timer: ReturnType<typeof setTimeout> | null = null

function onRemove(): void {
  if (!confirming.value) {
    confirming.value = true
    timer = setTimeout(() => { confirming.value = false }, 3000)
    return
  }
  if (timer) clearTimeout(timer)
  confirming.value = false
  emit('remove')
}
onUnmounted(() => { if (timer) clearTimeout(timer) })
</script>

<template>
  <header class="flex items-start gap-3 border-b border-border pb-3">
    <div v-if="icon" class="mt-0.5 flex size-8 flex-none items-center justify-center rounded-lg border border-primary/20 bg-primary/10 text-primary">
      <component :is="icon" class="size-4" />
    </div>
    <div class="min-w-0 flex-1">
      <h4 class="truncate font-serif text-[15px] leading-6 font-semibold text-foreground">{{ title || '（未命名）' }}</h4>
      <p v-if="sub" class="mt-0.5 truncate text-[11.5px] leading-4 text-muted-foreground/75">{{ sub }}</p>
    </div>
    <Badge
      v-if="meta"
      variant="outline"
      class="mt-0.5 shrink-0 text-[10.5px] font-normal"
      :class="tone === 'warn' ? 'border-warning/45 text-warning' : tone === 'ok' ? 'border-success/45 text-success' : 'border-border/70 text-muted-foreground'"
    >
      {{ meta }}
    </Badge>
    <Button
      v-if="!hideRemove"
      variant="ghost"
      size="sm"
      class="h-7 shrink-0 gap-1 text-xs transition-colors"
      :class="confirming ? 'bg-destructive/10 text-destructive' : 'text-muted-foreground/70 hover:bg-destructive/10 hover:text-destructive'"
      @click="onRemove"
    >
      <IconTrash class="size-3.5" />
      {{ confirming ? '确认删除' : '删除' }}
    </Button>
  </header>
</template>
