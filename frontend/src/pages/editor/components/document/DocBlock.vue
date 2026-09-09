<script setup lang="ts">
// DocBlock —— 文档视图的内嵌数据块：标题 + 若干只读行 + 自由内容插槽
withDefaults(defineProps<{
  title: string
  rows?: { label?: string; value: string; icon?: string }[]
  aside?: string
}>(), { rows: () => [], aside: '' })
</script>

<template>
  <section class="overflow-hidden rounded-lg border border-border bg-card">
    <header class="flex items-center gap-2 border-b border-border px-3.5 py-1.5">
      <span class="truncate text-[13px] font-bold text-foreground">{{ title }}</span>
      <span v-if="aside" class="ml-auto shrink-0 text-[11px] text-muted-foreground/60">{{ aside }}</span>
    </header>
    <div v-if="rows.length" class="flex flex-col">
      <div v-for="(r, i) in rows" :key="i" class="flex gap-2.5 border-b border-border/40 px-3.5 py-1 text-xs">
        <span v-if="r.label" class="w-16 flex-none text-[11px] text-muted-foreground/70">{{ r.label }}</span>
        <span class="min-w-0 flex-1 break-words text-foreground/90">{{ r.icon }} {{ r.value }}</span>
      </div>
    </div>
    <div class="px-3.5 py-1.5">
      <slot />
    </div>
  </section>
</template>
