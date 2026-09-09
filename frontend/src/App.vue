<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { subscribeToasts, type Toast } from '@/api'

const toasts = ref<Toast[]>([])
let unsub: (() => void) | null = null
onMounted(() => { unsub = subscribeToasts(t => { toasts.value = t }) })
onUnmounted(() => { unsub?.() })

const kindClass = (k: Toast['kind']) => 'toast--' + k
</script>

<template>
  <router-view />
  <div class="toast-stack" aria-live="polite">
    <transition-group name="toast">
      <div v-for="t in toasts" :key="t.id" class="oct-toast" :class="kindClass(t.kind)">{{ t.text }}</div>
    </transition-group>
  </div>
</template>

<style scoped>
.toast-stack { position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%); z-index: 200; display: flex; flex-direction: column; align-items: center; gap: 8px; pointer-events: none; }
.oct-toast {
  background: var(--popover); border: 1px solid var(--border);
  color: var(--foreground); padding: 8px 16px; border-radius: 999px; font-size: 13px;
  box-shadow: 0 6px 24px rgba(0,0,0,.4); max-width: min(560px, 90vw);
}
.toast--ok { border-color: color-mix(in oklab, var(--success) 50%, transparent); color: var(--success); }
.toast--warn { border-color: color-mix(in oklab, var(--warning) 50%, transparent); color: var(--warning); }
.toast--error { border-color: color-mix(in oklab, var(--destructive) 50%, transparent); color: var(--destructive); }
.toast-enter-active, .toast-leave-active { transition: all 200ms cubic-bezier(0.2, 0, 0, 1); }
.toast-enter-from, .toast-leave-to { opacity: 0; transform: translateY(8px); }
</style>
