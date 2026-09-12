// ============================================================
// 游玩页小工具（头像色散列 / 判定成功度中文映射 / 格式化）
// ============================================================
import type { AssetRef, CharacterInstance, SuccessLevel, WorldProjection } from '@/types'

/**
 * 投影里 characters 以 template_id 为 key，而 instance_id 是带前缀的另一个值
 * （inst-char-xxx）。凡是判断「这个角色是不是受控角色」都必须同时认两个 id。
 */
export function isControlledChar(controlledId: string, c: CharacterInstance): boolean {
  return !!controlledId && (c.template_id === controlledId || c.instance_id === controlledId)
}

/** 按任意一种 id（投影 key / template_id / instance_id）找角色实例。 */
export function findCharacter(p: WorldProjection | null, id: string): CharacterInstance | null {
  if (!p || !id) return null
  const direct = p.characters[id]
  if (direct) return direct
  return Object.values(p.characters).find(c => c.template_id === id || c.instance_id === id) ?? null
}

/**
 * 从存档内嵌的冻结故事书里查人物立绘（#28）。
 * 先按 id 精确匹配，其次按名字兜底（事件里的 actor.id 有时是实例 id）。
 */
export function portraitOf(storybook: unknown, id?: string, name?: string): AssetRef | null {
  const chars = (storybook as { characters?: { id?: string; name?: string; portrait?: AssetRef }[] } | undefined)?.characters
  if (!chars?.length) return null
  return (id ? chars.find(c => c.id === id)?.portrait : undefined)
    ?? (name ? chars.find(c => c.name === name)?.portrait : undefined)
    ?? null
}

function hash(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) { h = (h * 31 + s.charCodeAt(i)) | 0 }
  return Math.abs(h)
}
// 柔和语义底：按角色散列取一组语义色淡底（只用主题令牌，不写死 hex）
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

/** 仅时钟：HH:mm（聊天条目时间戳） */
export function fmtClock(iso?: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return p(d.getHours()) + ':' + p(d.getMinutes())
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