<script setup lang="ts">
// 存档抽屉宿主（#18 ④ / #21）：<Sheet side="right"> 右滑抽屉，标题随视图切换。
import { computed } from 'vue'
import { useDrawerStore } from '../stores/drawer'
import DrawerView from './drawer/DrawerView.vue'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'

const drawer = useDrawerStore()
const title = computed(() => (drawer.wizardOpen ? '版次升级' : '存读档菜单'))
</script>

<template>
  <Sheet :open="drawer.open" @update:open="(v: boolean) => { if (!v) drawer.closeDrawer() }">
    <SheetContent side="right" class="flex h-full w-[min(88vw,360px)] flex-col gap-0 p-0 sm:max-w-sm" :show-close-button="true">
      <SheetHeader class="border-b border-border px-4 py-2.5">
        <SheetTitle class="text-[14px] font-extrabold">{{ title }}</SheetTitle>
      </SheetHeader>
      <div class="min-h-0 flex-1 overflow-y-auto">
        <DrawerView />
      </div>
    </SheetContent>
  </Sheet>
</template>
