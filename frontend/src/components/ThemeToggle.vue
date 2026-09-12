<script setup lang="ts">
import { computed } from 'vue'
import { isDark, toggleTheme, themeMode } from '@/theme'
import { Button } from '@/components/ui/button'
import { IconSun, IconMoon } from '@tabler/icons-vue'

const tooltip = computed(() => {
  if (themeMode.value === 'auto') {
    return isDark.value ? '主题：跟随系统（当前深色，点击切换明亮）' : '主题：跟随系统（当前浅色，点击切换深色）'
  }
  return isDark.value ? '主题：暗色模式（点击切换明亮）' : '主题：明亮模式（点击切换暗色）'
})
</script>

<template>
  <Button
    type="button"
    variant="ghost"
    size="icon-sm"
    class="relative size-8 rounded-full text-muted-foreground hover:text-foreground hover:bg-muted/80 transition-all duration-200"
    :title="tooltip"
    :aria-label="tooltip"
    @click="toggleTheme()"
  >
    <transition name="theme-fade" mode="out-in">
      <IconSun
        v-if="!isDark"
        key="sun"
        class="size-4.5 text-amber-600 dark:text-amber-400 transition-transform duration-300 hover:rotate-45"
      />
      <IconMoon
        v-else
        key="moon"
        class="size-4.5 text-primary transition-transform duration-300 hover:-rotate-12"
      />
    </transition>
  </Button>
</template>

<style scoped>
.theme-fade-enter-active,
.theme-fade-leave-active {
  transition: opacity 0.15s ease, transform 0.15s ease;
}

.theme-fade-enter-from {
  opacity: 0;
  transform: scale(0.7) rotate(-30deg);
}

.theme-fade-leave-to {
  opacity: 0;
  transform: scale(0.7) rotate(30deg);
}
</style>
