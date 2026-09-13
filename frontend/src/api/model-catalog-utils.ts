// 模型目录的查询/反查工具（基于 model-catalog.ts 的静态快照）。
import { MODEL_CATALOG, PROVIDER_PRESETS } from './model-catalog'
import type { CatalogModel, CatalogProvider, ModelEntry } from '@/types'

const providerById = new Map<string, CatalogProvider>(MODEL_CATALOG.map(p => [p.id, p]))

/** 模型 id → 收录它的供应商 id 列表（同一 id 可能出现在多家） */
const modelToProviders = new Map<string, string[]>()
for (const p of MODEL_CATALOG) {
  for (const m of p.models) {
    const arr = modelToProviders.get(m.id) ?? []
    if (!arr.includes(p.id)) arr.push(p.id)
    modelToProviders.set(m.id, arr)
  }
}

/** base_url 主机 → 供应商 id（用于「自动探测」） */
const hostToProvider = new Map<string, string>()
for (const [id, preset] of Object.entries(PROVIDER_PRESETS)) {
  if (!preset.base_url) continue
  try {
    const host = new URL(preset.base_url).host.toLowerCase()
    if (!hostToProvider.has(host)) hostToProvider.set(host, id)
  } catch { /* 忽略非法 URL */ }
}

export function catalogProvider(id: string): CatalogProvider | undefined {
  return providerById.get(id)
}

export function catalogModels(id: string): string[] {
  return providerById.get(id)?.models.map(m => m.id) ?? []
}

/** 目录里某供应商的模型条目（id + 展示名），用于「恢复默认模型」 */
export function catalogEntries(id: string): ModelEntry[] {
  return providerById.get(id)?.models.map(m => ({ id: m.id, name: m.name })) ?? []
}

export function findModelProviders(modelId: string): string[] {
  return modelToProviders.get(modelId) ?? []
}

export function allModelIds(): string[] {
  return [...modelToProviders.keys()]
}

/** 由 Base URL 反推供应商（命中预设主机，退化到子串匹配）。 */
export function detectProviderByUrl(url: string): string | null {
  const raw = (url ?? '').trim()
  if (!raw) return null
  try {
    const host = new URL(raw).host.toLowerCase()
    if (hostToProvider.has(host)) return hostToProvider.get(host)!
    for (const id of providerById.keys()) {
      const stem = id.split('-')[0]
      if (stem.length >= 4 && host.includes(stem)) return id
    }
  } catch { /* 非法 URL，走子串兜底 */ }
  const lower = raw.toLowerCase()
  for (const id of providerById.keys()) {
    const stem = id.split('-')[0]
    if (stem.length >= 4 && lower.includes(stem)) return id
  }
  return null
}

/** 目录里某供应商某模型的元数据（上下文 / 思考能力）；探测来的自定义模型可能没有。 */
export function catalogModelMeta(providerId: string, modelId: string): CatalogModel | undefined {
  const exact = providerById.get(providerId)?.models.find(m => m.id === modelId)
  if (exact) return exact
  // 供应商 id 可能与目录不同（用户自定义命名）：按模型 id 全局兜底
  for (const p of MODEL_CATALOG) {
    const hit = p.models.find(m => m.id === modelId)
    if (hit) return hit
  }
  return undefined
}


/** 合并「目录元数据」与「用户自定义覆盖」：用户填了哪个字段就覆盖哪个。 */
export function mergeModelMeta(base: CatalogModel | undefined, entry: ModelEntry | undefined): CatalogModel | undefined {
  if (!base && !entry) return undefined
  return {
    id: entry?.id ?? base?.id ?? '',
    name: entry?.name ?? base?.name ?? '',
    ctx: entry?.ctx ?? base?.ctx,
    // 目录快照是 camelCase（maxOut），落盘的 ModelEntry 与 AppConfig 其余字段一致是 snake_case（max_out）
    maxOut: entry?.max_out ?? base?.maxOut,
    reasoning: entry?.reasoning ?? base?.reasoning,
    tl: entry?.tl ?? base?.tl,
  }
}

/** 思考等级的稳定展示顺序。 */
const LEVEL_ORDER = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

/** 该模型支持的思考等级；无元数据时回落到通用档位。 */
export function thinkingLevels(meta?: CatalogModel): string[] {
  if (!meta?.tl || !Object.keys(meta.tl).length) return ['default', 'minimal', 'low', 'medium', 'high']
  const keys = Object.keys(meta.tl).sort((a, b) => {
    const ia = LEVEL_ORDER.indexOf(a), ib = LEVEL_ORDER.indexOf(b)
    return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib)
  })
  // 目录里有些 reasoning 模型没声明 off 档（pi.dev 的数据不全），但用户仍需要能关掉思考；
  // 这里补一个「关闭」兜底，实际下发值见 levelToWire。
  const ordered = keys.includes('off') ? keys : ['off', ...keys]
  return ['default', ...ordered]
}

/** 等级 → 实际下发值；default / off（wire 为 null）都表示不发送该参数。 */
export function levelToWire(meta: CatalogModel | undefined, level: string): string | undefined {
  if (level === 'default') return undefined
  if (level === 'off') {
    if (!meta?.tl) return undefined
    // 目录声明了 off：按其 wire 值下发（null = 不发送，即交给供应商默认）
    if ('off' in meta.tl) return meta.tl.off ?? undefined
    // 目录没声明 off（数据不全）：显式下发 none —— 目录里 132 个模型就是这么标的
    return 'none'
  }
  const wire = meta?.tl?.[level]
  return wire == null ? undefined : wire
}

/** 实际下发值 → 等级（回显用）。 */
export function wireToLevel(meta: CatalogModel | undefined, wire?: string): string {
  if (!wire) return 'default'
  if (meta?.tl) {
    const hit = Object.entries(meta.tl).find(([, v]) => v === wire)
    if (hit) return hit[0]
  }
  // 我们为「关闭」补的显式值：回显成 off 档
  if (wire === 'none') return 'off'
  return wire
}

/** 思考等级的展示标签。 */
export const LEVEL_LABEL: Record<string, string> = {
  default: '默认', off: '关闭', minimal: 'Minimal', low: 'Low', medium: 'Medium', high: 'High', xhigh: 'XHigh', max: 'Max',
}

/** u32 上界：超过它后端反序列化会 422，整份配置都存不下去。 */
const TOKEN_MAX = 4_294_967_295

/**
 * token 数的容错解析：`131072` / `128K` / `1M` / `1,000,000` / `100万` 都认。
 * 认不出（含超出 u32）返回 null —— 调用方据此提示用户，而不是把输入悄悄丢掉。
 */
export function parseTokenCount(text: string): number | null {
  const raw = (text ?? '').trim().replace(/[\s,，_]/g, '')
  const m = /^(\d+(?:\.\d+)?)([kKmM万])?$/.exec(raw)
  if (!m) return null
  const n = Number(m[1])
  if (!Number.isFinite(n) || n <= 0) return null
  const unit = (m[2] ?? '').toLowerCase()
  const scale = unit === 'k' ? 1_000 : unit === 'm' ? 1_000_000 : unit === '万' ? 10_000 : 1
  const out = Math.round(n * scale)
  return out > 0 && out <= TOKEN_MAX ? out : null
}

/** 上下文窗口的可读展示：1000000 → 1M，128000 → 128K。 */
export function fmtContext(n?: number): string {
  if (!n) return ''
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1) + 'M'
  if (n >= 1000) return Math.round(n / 1000) + 'K'
  return String(n)
}
