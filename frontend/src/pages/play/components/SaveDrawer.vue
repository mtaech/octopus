<script setup lang="ts">
// 存档抽屉宿主（#18 ④ / #21）：<Sheet side="right"> 右滑抽屉，标题随视图切换。
import { computed } from 'vue'
import { useDrawerStore } from '../stores/drawer'
import DrawerView from './drawer/DrawerView.vue'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { IconBookmark } from '@tabler/icons-vue'

const drawer = useDrawerStore()
const title = computed(() => (drawer.wizardOpen ? '版次升级' : '存读档中心'))
</script>

<template>
  <Sheet :open="drawer.open" @update:open="(v: boolean) => { if (!v) drawer.closeDrawer() }">
    <SheetContent
      side="right"
      class="flex h-full w-[min(92vw,400px)] flex-col gap-0 border-l border-border/80 bg-background/95 p-0 backdrop-blur-xl sm:max-w-md shadow-2xl"
      :show-close-button="true"
    >
      <SheetHeader class="border-b border-border/80 px-4 py-3 bg-card/50">
        <SheetTitle class="flex items-center gap-2 text-[14.5px] font-extrabold text-foreground">
          <IconBookmark class="size-4 text-primary" />
          <span>{{ title }}</span>
        </SheetTitle>
      </SheetHeader>
      <div class="min-h-0 flex-1 overflow-y-auto">
        <DrawerView />
      </div>
    </SheetContent>
  </Sheet>
</template>
