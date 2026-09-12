// 世界色调（跨页共享）：故事书 / 存档 / 编辑器顶栏共用的封面色调与印鉴。
// 按种子把每本故事书 / 存档确定性地映射到「色调 + 封面纹样」，
// 只用主题语义令牌做 color-mix（var(--primary) / --info / --chart-5 / --success），
// 不写死 hex，随明暗双态自适应 —— 符合 CONTRACT-MIGRATE 第 2 条。

export const WORLD_TINTS = [
  'var(--primary)',
  'color-mix(in oklab, var(--primary) 68%, var(--info))',
  'color-mix(in oklab, var(--primary) 68%, var(--chart-5))',
  'color-mix(in oklab, var(--primary) 74%, var(--success))',
] as const

// 仅保留无方向性的点状细纹：斜纹 stripe / 放射 rays / 网格 grid 等线状纹样已按反馈移除
export const WORLD_PATTERNS = ['dots'] as const

function hash(seed: string): number {
  let h = 0
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) | 0
  return Math.abs(h)
}

/** 世界主色调（CSS 颜色表达式，可赋给 --tint） */
export function worldTint(seed: string): string {
  return WORLD_TINTS[hash(seed) % WORLD_TINTS.length]
}

/** 封面纹样：同一种子稳定复现，用作世界的视觉指纹 */
export function worldPattern(seed: string): string {
  return WORLD_PATTERNS[hash(seed + '::pattern') % WORLD_PATTERNS.length]
}

/** 标题首字（去掉常见前缀符号）作为封面印鉴；无字时回落到「书」 */
export function worldGlyph(title: string): string {
  const t = title.replace(/^[·•\s【（(]+/, '')
  return t.charAt(0) || '书'
}