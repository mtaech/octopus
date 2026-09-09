<script setup lang="ts">
// EntityCard —— A 表单工作台的实体编辑卡：头部（图标+名称/徽标/折叠/删除）+ 字段区插槽
import { ref, useSlots, type Component } from 'vue'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconChevronDown, IconChevronUp, IconTrash } from '@tabler/icons-vue'

const props = withDefaults(defineProps<{
  title: string
  sub?: string
  /** tabler 图标组件（避免 emoji） */
  icon?: Component
  tone?: 'ok' | 'warn'
  meta?: string
}>(), { sub: '', meta: '' })

const emit = defineEmits<{ remove: [] }>()

const slots = useSlots()
const collapsed = ref(false)
const hasBody = !!(slots.default && slots.default().length)
const badgeCls = props.tone === 'ok' ? 'text-success border-success/40' : props.tone === 'warn' ? 'text-warning border-warning/40' : ''
</script>

<template>
  <Card class="gap-0 rounded-lg border-border/60 py-0 shadow-none ring-0">
    <header class="flex items-center gap-2.5 px-3.5 py-2">
      <component :is="icon" v-if="icon" class="size-4 shrink-0 text-primary/90" />
      <div class="min-w-0 flex-1">
        <div class="truncate text-[13.5px] leading-5 font-semibold text-foreground">{{ title }}</div>
        <div v-if="sub" class="truncate text-[11px] leading-4 text-muted-foreground/70">{{ sub }}</div>
      </div>
      <Badge v-if="meta" variant="outline" class="shrink-0 border-border/60 text-[11px] font-normal" :class="badgeCls">{{ meta }}</Badge>
      <div class="flex shrink-0 items-center gap-0.5">
        <Button v-if="hasBody" variant="ghost" size="sm" class="h-6 px-2 text-xs font-normal text-muted-foreground" @click="collapsed = !collapsed">
          <component :is="collapsed ? IconChevronDown : IconChevronUp" data-icon="inline-start" />
          {{ collapsed ? '展开' : '收起' }}
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          class="size-7 text-destructive hover:bg-destructive/10 hover:text-destructive"
          title="删除"
          aria-label="删除 {{ title }}"
          @click="emit('remove')"
        >
          <IconTrash />
        </Button>
      </div>
    </header>
    <div v-show="!collapsed" class="px-2.5 pb-2.5">
      <slot />
    </div>
  </Card>
</template>
