// ============================================================
// Material 3 主题管理器 · 支持明亮 / 暗色 / 跟随系统
// ============================================================
import { ref } from 'vue'

export type ThemeMode = 'light' | 'dark' | 'auto'

export const STORAGE_KEY = 'octopus-theme'

function getSavedMode(): ThemeMode {
  try {
    const saved = localStorage.getItem(STORAGE_KEY) as ThemeMode | null
    if (saved === 'light' || saved === 'dark' || saved === 'auto') {
      return saved
    }
  } catch {
    // noop
  }
  // 默认使用明亮色调（满足用户偏好）
  return 'light'
}

export const themeMode = ref<ThemeMode>(getSavedMode())
export const isDark = ref(false)

function applyDarkClass(dark: boolean) {
  isDark.value = dark
  if (typeof document !== 'undefined') {
    const root = document.documentElement
    if (dark) {
      root.classList.add('dark')
      root.style.colorScheme = 'dark'
    } else {
      root.classList.remove('dark')
      root.style.colorScheme = 'light'
    }
  }
}

function updateFromSystem() {
  if (typeof window === 'undefined') return
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches
  if (themeMode.value === 'auto') {
    applyDarkClass(prefersDark)
  }
}

export function setTheme(mode: ThemeMode) {
  themeMode.value = mode
  try {
    localStorage.setItem(STORAGE_KEY, mode)
  } catch {
    // noop
  }
  resolveAndApply()
}

export function toggleTheme() {
  // 快速在明亮与暗色之间切换
  const next = isDark.value ? 'light' : 'dark'
  setTheme(next)
}

export function resolveAndApply() {
  if (typeof window === 'undefined') return
  if (themeMode.value === 'dark') {
    applyDarkClass(true)
  } else if (themeMode.value === 'light') {
    applyDarkClass(false)
  } else {
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches
    applyDarkClass(prefersDark)
  }
}

export function initTheme() {
  if (typeof window === 'undefined') return
  resolveAndApply()
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  try {
    mq.addEventListener('change', updateFromSystem)
  } catch {
    (mq as any).addListener?.(updateFromSystem)
  }
}
