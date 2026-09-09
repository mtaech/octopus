// 模型目录的查询/反查工具（基于 model-catalog.ts 的静态快照）。
import { MODEL_CATALOG, PROVIDER_PRESETS } from './model-catalog'
import type { CatalogProvider } from '@/types'

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
