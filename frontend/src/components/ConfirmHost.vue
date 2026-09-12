<script setup lang="ts">
// 全局确认框宿主：读 lib/confirm 的命令式状态，渲染 shadcn AlertDialog。
// 用普通 Button + 显式 resolve：不依赖 AlertDialogAction 的自动关闭，避免与
// @update:open 处理器竞态（先关后 resolve 会把「确认」吞成「取消」）。
import { computed } from 'vue'
import { Button } from '@/components/ui/button'
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { confirmOpen, confirmOpts, resolveConfirm } from '@/lib/confirm'

const opts = computed(() => confirmOpts.value)
</script>

<template>
  <AlertDialog :open="confirmOpen" @update:open="(v: boolean) => { if (!v) resolveConfirm(false) }">
    <AlertDialogContent v-if="opts">
      <AlertDialogHeader>
        <AlertDialogTitle>{{ opts.title }}</AlertDialogTitle>
        <AlertDialogDescription v-if="opts.description" class="whitespace-pre-line">{{ opts.description }}</AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <Button variant="outline" @click="resolveConfirm(false)">{{ opts.cancelText ?? '取消' }}</Button>
        <Button :variant="opts.destructive ? 'destructive' : 'default'" @click="resolveConfirm(true)">
          {{ opts.confirmText ?? '确定' }}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
</template>
