// ============================================================
// 由 pi.dev 的模型目录重新生成 frontend/src/api/model-catalog.ts。
// 用法：node scripts/gen-model-catalog.mjs   （或 pnpm gen:catalog）
// 数据源：https://pi.dev/api/models —— 含 contextWindow / maxTokens /
// reasoning（是否支持思考）/ thinkingLevelMap（思考等级 → 供应商实际值）。
// ============================================================
import { writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const SRC = 'https://pi.dev/api/models'
const OUT = resolve(dirname(fileURLToPath(import.meta.url)), '../src/api/model-catalog.ts')

// 已知供应商的默认端点与协议类型（「从目录添加」时预填；未收录者留空由用户填）
const PROVIDER_PRESETS = {
  openai: { base_url: 'https://api.openai.com/v1', kind: 'openai' },
  anthropic: { base_url: 'https://api.anthropic.com', kind: 'anthropic' },
  deepseek: { base_url: 'https://api.deepseek.com', kind: 'openai-compatible' },
  google: { base_url: 'https://generativelanguage.googleapis.com/v1beta', kind: 'openai-compatible' },
  'google-vertex': { base_url: '', kind: 'openai-compatible' },
  groq: { base_url: 'https://api.groq.com/openai/v1', kind: 'openai-compatible' },
  mistral: { base_url: 'https://api.mistral.ai/v1', kind: 'openai-compatible' },
  xai: { base_url: 'https://api.x.ai/v1', kind: 'openai-compatible' },
  moonshotai: { base_url: 'https://api.moonshot.ai/v1', kind: 'openai-compatible' },
  'moonshotai-cn': { base_url: 'https://api.moonshot.cn/v1', kind: 'openai-compatible' },
  zai: { base_url: 'https://api.z.ai/api/paas/v4', kind: 'openai-compatible' },
  'zai-coding-cn': { base_url: 'https://open.bigmodel.cn/api/paas/v4', kind: 'openai-compatible' },
  minimax: { base_url: 'https://api.minimax.chat/v1', kind: 'openai-compatible' },
  'minimax-cn': { base_url: 'https://api.minimaxi.com/v1', kind: 'openai-compatible' },
  together: { base_url: 'https://api.together.xyz/v1', kind: 'openai-compatible' },
  cerebras: { base_url: 'https://api.cerebras.ai/v1', kind: 'openai-compatible' },
  nvidia: { base_url: 'https://integrate.api.nvidia.com/v1', kind: 'openai-compatible' },
  openrouter: { base_url: 'https://openrouter.ai/api/v1', kind: 'openai-compatible' },
  fireworks: { base_url: 'https://api.fireworks.ai/inference/v1', kind: 'openai-compatible' },
  huggingface: { base_url: 'https://api-inference.huggingface.co/v1', kind: 'openai-compatible' },
  baseten: { base_url: 'https://inference.baseten.co/v1', kind: 'openai-compatible' },
  'cloudflare-workers-ai': { base_url: '', kind: 'openai-compatible' },
  'cloudflare-ai-gateway': { base_url: '', kind: 'openai-compatible' },
  ollama: { base_url: 'http://127.0.0.1:11434', kind: 'ollama' },
}

const res = await fetch(SRC)
if (!res.ok) throw new Error(`拉取 ${SRC} 失败：${res.status}`)
const catalog = await res.json()

const providers = Object.keys(catalog).sort()
let modelCount = 0
let reasoningCount = 0
let levelCount = 0

const q = (s) => JSON.stringify(s)
function modelLine(m) {
  const parts = [`id: ${q(m.id)}`, `name: ${q(m.name ?? m.id)}`]
  if (typeof m.contextWindow === 'number') parts.push(`ctx: ${m.contextWindow}`)
  if (typeof m.maxTokens === 'number' && m.maxTokens !== m.contextWindow) parts.push(`maxOut: ${m.maxTokens}`)
  if (m.reasoning === true) parts.push('reasoning: true')
  const tl = m.thinkingLevelMap
  if (tl && typeof tl === 'object' && Object.keys(tl).length) {
    parts.push(`tl: ${JSON.stringify(tl)}`)
    levelCount += 1
  }
  return `    { ${parts.join(', ')} },`
}

const body = providers.map((pid) => {
  const models = Object.values(catalog[pid] ?? {}).sort((a, b) => a.id.localeCompare(b.id))
  modelCount += models.length
  for (const m of models) if (m.reasoning === true) reasoningCount += 1
  const rows = models.map(modelLine).join('\n')
  return `  { id: ${q(pid)}, label: ${q(pid)}, models: [\n${rows}\n  ] },`
}).join('\n')

const presetRows = Object.entries(PROVIDER_PRESETS)
  .map(([id, p]) => `  ${q(id)}: { base_url: ${q(p.base_url)}, kind: ${q(p.kind)} },`)
  .join('\n')

const out = `// ============================================================
// 模型目录快照（自动生成，勿手改）
// 来源：https://pi.dev/models
// 生成时间：${new Date().toISOString()} · 供应商 ${providers.length} / 模型 ${modelCount}
// 重新生成：pnpm -C frontend gen:catalog
// ============================================================
import type { CatalogProvider, ProviderKind } from '@/types'

export const CATALOG_SOURCE = 'https://pi.dev/models'
export const CATALOG_FETCHED_AT = '${new Date().toISOString()}'

/** 已知供应商的默认端点与协议类型（「从目录添加」时预填；未收录者留空由用户填） */
export const PROVIDER_PRESETS: Record<string, { base_url: string; kind: ProviderKind }> = {
${presetRows}
}

export const MODEL_CATALOG: CatalogProvider[] = [
${body}
]
`

await writeFile(OUT, out, 'utf8')
console.log(`已生成 ${OUT}`)
console.log(`供应商 ${providers.length} · 模型 ${modelCount} · reasoning ${reasoningCount} · 带思考等级 ${levelCount}`)
