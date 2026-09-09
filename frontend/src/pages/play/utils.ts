// ============================================================
// 游玩页小工具（头像色散列 / 判定成功度中文映射 / 格式化）
// ============================================================
import type { SuccessLevel } from '@/types'

const PALETTE = [
  '#6ea8ff', '#4ade80', '#fbbf24', '#f87171', '#c084fc', '#22d3ee',
  '#fb923c', '#f472b6', '#a3e635', '#818cf8'
]
function hash(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) { h = (h * 31 + s.charCodeAt(i)) | 0 }
  return Math.abs(h)
}
export function nameColor(name: string): string { return PALETTE[hash(name) % PALETTE.length] }
// 柔和语义底（夜行手记）：按角色散列取一组语义色淡底，避免亮色 hex
const TINT_IDX = ['bg-primary/20 text-primary', 'bg-info/20 text-info', 'bg-success/20 text-success', 'bg-warning/25 text-warning', 'bg-secondary text-secondary-foreground', 'bg-accent/60 text-accent-foreground']
const BORDER_IDX = ['border-l-primary/60', 'border-l-info/60', 'border-l-success/60', 'border-l-warning/60', 'border-l-border', 'border-l-accent-foreground/40']
function tintIndex(name: string): number { return hash(name) % TINT_IDX.length }
export function nameTintClass(name: string): string { return TINT_IDX[tintIndex(name)] }
export function nameBorderClass(name: string): string { return BORDER_IDX[tintIndex(name)] }

export function initial(name: string): string { return name.trim().slice(0, 1) || '?' }

export const LEVEL_META: Record<SuccessLevel, { label: string; hint: string }> = {
  great: { label: '大成功', hint: '差值 ≥ 10' },
  success: { label: '成功', hint: '达成目标' },
  barely: { label: '勉强', hint: '差一点达成' },
  fail: { label: '失败', hint: '未达成目标' }
}

export function fmtTime(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  const p = (n: number) => String(n).padStart(2, '0')
  return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) + ' ' + p(d.getHours()) + ':' + p(d.getMinutes())
}

export function relTime(iso: string): string {
  const d = new Date(iso).getTime()
  if (Number.isNaN(d)) return iso
  const s = Math.max(1, Math.floor((Date.now() - d) / 1000))
  if (s < 60) return s + ' 秒前'
  const m = Math.floor(s / 60)
  if (m < 60) return m + ' 分钟前'
  const h = Math.floor(m / 60)
  if (h < 24) return h + ' 小时前'
  const dd = Math.floor(h / 24)
  return dd + ' 天前'
}