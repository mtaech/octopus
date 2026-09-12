// ============================================================
// 字体管理器 · 支持浏览器本地字体查询（Local Font Access API）+ LocalStorage 持久化
// ============================================================
import { ref } from 'vue'

export const FONT_STORAGE_KEY = 'octopus-font-family'
export const FONT_CACHED_LIST_KEY = 'octopus-cached-fonts'

export interface LocalFontItem {
  family: string
  fullName: string
}

/** 常用常见预设字体 */
export const PRESET_FONTS: { label: string; value: string }[] = [
  { label: '系统默认', value: '' },
  { label: '苹方 (PingFang SC)', value: 'PingFang SC' },
  { label: '微软雅黑 (Microsoft YaHei)', value: 'Microsoft YaHei' },
  { label: '思源黑体 (Source Han Sans)', value: 'Source Han Sans SC' },
  { label: '思源宋体 (Source Han Serif)', value: 'Source Han Serif SC' },
  { label: '霞鹜文楷 (LXGW WenKai)', value: 'LXGW WenKai' },
  { label: '楷体 (KaiTi)', value: 'KaiTi' },
  { label: '仿宋 (FangSong)', value: 'FangSong' },
  { label: '鸿蒙黑体 (HarmonyOS Sans)', value: 'HarmonyOS Sans SC' },
  { label: 'JetBrains Mono', value: 'JetBrains Mono' },
  { label: 'Fira Code', value: 'Fira Code' },
]

function getSavedFont(): string {
  try {
    if (typeof localStorage !== 'undefined') {
      return localStorage.getItem(FONT_STORAGE_KEY) || ''
    }
  } catch {
    // noop
  }
  return ''
}

function getCachedFontList(): LocalFontItem[] {
  try {
    if (typeof localStorage !== 'undefined') {
      const raw = localStorage.getItem(FONT_CACHED_LIST_KEY)
      if (raw) {
        const parsed = JSON.parse(raw)
        if (Array.isArray(parsed)) {
          return parsed.map((item: unknown) =>
            typeof item === 'string' ? { family: item, fullName: item } : (item as LocalFontItem)
          )
        }
      }
    }
  } catch {
    // noop
  }
  return []
}

export const currentFont = ref<string>(getSavedFont())
export const availableFonts = ref<LocalFontItem[]>(getCachedFontList())
export const isQueryingFonts = ref<boolean>(false)
export const isFontApiSupported = ref<boolean>(typeof window !== 'undefined' && 'queryLocalFonts' in window)

/**
 * 将字体设置应用到页面 DOM
 */
export function applyFont(fontName: string) {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  const trimmed = fontName.trim()
  if (trimmed && trimmed !== 'default') {
    const fontStack = `"${trimmed}", "Geist", ui-sans-serif, system-ui, -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif`
    root.style.setProperty('--app-font-family', fontStack)
    root.style.setProperty('--font-sans', fontStack)
    root.style.setProperty('--font-serif', fontStack)
    root.style.setProperty('--font-heading', fontStack)
  } else {
    root.style.removeProperty('--app-font-family')
    root.style.removeProperty('--font-sans')
    root.style.removeProperty('--font-serif')
    root.style.removeProperty('--font-heading')
  }
}

/**
 * 设置字体并保存至 localStorage
 */
export function setFont(fontName: string) {
  const trimmed = fontName.trim()
  currentFont.value = trimmed
  try {
    if (typeof localStorage !== 'undefined') {
      if (trimmed) {
        localStorage.setItem(FONT_STORAGE_KEY, trimmed)
      } else {
        localStorage.removeItem(FONT_STORAGE_KEY)
      }
    }
  } catch {
    // noop
  }
  applyFont(trimmed)
}

/**
 * 调用浏览器的本地字体查询 API (window.queryLocalFonts)
 */
export async function queryBrowserFonts(): Promise<{ ok: boolean; count: number; error?: string }> {
  if (typeof window === 'undefined') {
    return { ok: false, count: 0, error: '非浏览器环境' }
  }

  if (!('queryLocalFonts' in window)) {
    return {
      ok: false,
      count: 0,
      error: '当前浏览器不支持本地字体查询 API（Local Font Access API）。建议使用 Chrome/Edge 103+ 或从下方常用预设中选择。',
    }
  }

  try {
    isQueryingFonts.value = true
    // 调用浏览器 queryLocalFonts API（可能需要用户在弹窗中授权）
    const rawFonts = await (window as any).queryLocalFonts()

    // 按 family 去重
    const familyMap = new Map<string, LocalFontItem>()
    for (const f of rawFonts) {
      if (f.family && !familyMap.has(f.family)) {
        familyMap.set(f.family, {
          family: f.family,
          fullName: f.fullName || f.family,
        })
      }
    }

    const sorted = Array.from(familyMap.values()).sort((a, b) =>
      a.family.localeCompare(b.family, 'zh-Hans-CN', { sensitivity: 'base' })
    )

    availableFonts.value = sorted
    try {
      localStorage.setItem(FONT_CACHED_LIST_KEY, JSON.stringify(sorted))
    } catch {
      // noop
    }

    return { ok: true, count: sorted.length }
  } catch (err: any) {
    const isDenied = err?.name === 'NotAllowedError' || String(err?.message || '').toLowerCase().includes('denied')
    const msg = isDenied
      ? '未获得访问本机字体权限（请在浏览器地址栏或提示中允许访问）。'
      : (err?.message || '读取本地字体失败')
    return { ok: false, count: 0, error: msg }
  } finally {
    isQueryingFonts.value = false
  }
}

/**
 * 初始化字体设置（在 main.ts 中调用）
 */
export function initFont() {
  if (typeof window === 'undefined') return
  isFontApiSupported.value = 'queryLocalFonts' in window
  const saved = getSavedFont()
  if (saved) {
    applyFont(saved)
  }
}
