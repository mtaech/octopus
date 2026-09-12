<script setup lang="ts">
import { defineAsyncComponent, ref, onMounted, onUnmounted } from 'vue'
import { subscribeToasts, type Toast } from '@/api'
// 异步引入：确认框的 shadcn/reka-ui 代码不进入口 chunk，避免拖慢首屏。
const ConfirmHost = defineAsyncComponent(() => import('@/components/ConfirmHost.vue'))
import {
  IconCircleCheckFilled,
  IconAlertTriangleFilled,
  IconAlertCircleFilled,
  IconInfoCircleFilled,
} from '@tabler/icons-vue'

const toasts = ref<Toast[]>([])
let unsub: (() => void) | null = null
onMounted(() => { unsub = subscribeToasts(t => { toasts.value = t }) })
onUnmounted(() => { unsub?.() })

const kindClass = (k: Toast['kind']) => 'toast--' + k
</script>

<template>
  <router-view />
  <!-- 全局命令式确认框（替代 window.confirm） -->
  <ConfirmHost />
  <div class="toast-stack" aria-live="polite">
    <transition-group name="toast">
      <div v-for="t in toasts" :key="t.id" class="oct-toast" :class="kindClass(t.kind)">
        <IconCircleCheckFilled v-if="t.kind === 'ok'" class="toast-icon text-success shrink-0 size-4" />
        <IconAlertTriangleFilled v-else-if="t.kind === 'warn'" class="toast-icon text-warning shrink-0 size-4" />
        <IconAlertCircleFilled v-else-if="t.kind === 'error'" class="toast-icon text-destructive shrink-0 size-4" />
        <IconInfoCircleFilled v-else class="toast-icon text-primary shrink-0 size-4" />
        <span class="leading-snug">{{ t.text }}</span>
      </div>
    </transition-group>
  </div>
</template>

<style scoped>
.toast-stack {
  position: fixed;
  bottom: 24px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 200;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  pointer-events: none;
}
.oct-toast {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  background: color-mix(in oklab, var(--popover) 85%, transparent);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  border: 1px solid color-mix(in oklab, var(--border) 90%, transparent);
  color: var(--foreground);
  padding: 8px 18px;
  border-radius: 9999px;
  font-size: 13px;
  font-weight: 500;
  box-shadow: 0 10px 25px -4px rgba(0, 0, 0, 0.12), 0 4px 8px -2px rgba(0, 0, 0, 0.06);
  max-width: min(560px, 90vw);
}
:is(.dark) .oct-toast {
  box-shadow: 0 10px 30px -4px rgba(0, 0, 0, 0.55), 0 0 0 1px rgba(255, 255, 255, 0.05);
}
.toast--ok {
  border-color: color-mix(in oklab, var(--success) 45%, transparent);
  color: var(--foreground);
}
.toast--warn {
  border-color: color-mix(in oklab, var(--warning) 45%, transparent);
  color: var(--foreground);
}
.toast--error {
  border-color: color-mix(in oklab, var(--destructive) 45%, transparent);
  color: var(--foreground);
}
.toast-enter-active, .toast-leave-active {
  transition: all 250ms cubic-bezier(0.16, 1, 0.3, 1);
}
.toast-enter-from, .toast-leave-to {
  opacity: 0;
  transform: translateY(12px) scale(0.96);
}
</style>
