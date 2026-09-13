// ============================================================
// Octopus 前端 API 层 —— 页面唯一的数据入口（#23/#24 契约面）
// 模式：优先直连真实 axum 后端（/api 经 Vite 代理转发到 127.0.0.1:8787）；
//       通过 URL ?mock=1 或 localStorage['octopus:force_mock']='1' 可随时切回 Mock。
// ============================================================
import type {
  Storybook, SaveListItem, SaveDetail, SavePackage, StorybookListItem, StorybookDocument,
  ValidateResult, ValidationIssue, UpgradeReport, Disposition,
  PairSuggestion, WorldProjection, PlayEvent, StreamStatus, PhaseStage,
  HistoryPage, SaveSettings, MaintenanceRow, AppConfig, ProviderTestResult, ProviderConfig, ProbeResult,
  EntityRef, FocusEntity
} from '@/types'
import * as mock from './mock/backend'
import { delay } from './mock/backend'

// ---- 轻量全局提示 ----
type ToastKind = 'info' | 'ok' | 'warn' | 'error'
export interface Toast { id: number; kind: ToastKind; text: string }
const toasts: Toast[] = []
const toastSinks = new Set<(t: Toast[]) => void>()
let toastId = 0
export function subscribeToasts(fn: (t: Toast[]) => void): () => void { toastSinks.add(fn); fn([...toasts]); return () => toastSinks.delete(fn) }
export function toast(kind: ToastKind, text: string) {
  const t: Toast = { id: ++toastId, kind, text }
  toasts.push(t)
  toastSinks.forEach(fn => fn([...toasts]))
  setTimeout(() => { const i = toasts.indexOf(t); if (i >= 0) { toasts.splice(i, 1); toastSinks.forEach(fn => fn([...toasts])) } }, 3200)
}

export interface ApiError extends Error { code: string; detail?: Record<string, unknown> }
function mkErr(code: string, message: string, detail?: Record<string, unknown>): ApiError {
  const e = new Error(message) as ApiError; e.code = code; e.detail = detail; return e
}
function wrapErr(e: unknown): ApiError {
  if (e && typeof e === 'object' && 'code' in e && typeof (e as { code?: unknown }).code === 'string') return e as ApiError
  const code = (e as { code?: string })?.code ?? 'INTERNAL'
  const msg = (e as Error)?.message ?? String(e)
  return mkErr(code, msg)
}
async function run<T>(fn: () => T | Promise<T>): Promise<T> {
  try { return await fn() } catch (e) { throw wrapErr(e) }
}

// ---- 运行模式判断 ----
export function isMockMode(): boolean {
  if (typeof window === 'undefined') return true
  const params = new URLSearchParams(window.location.search)
  if (params.get('mock') === '1') return true
  if (localStorage.getItem('octopus:force_mock') === '1') return true
  return false
}

export function setMockMode(forceMock: boolean): void {
  if (forceMock) {
    localStorage.setItem('octopus:force_mock', '1')
  } else {
    localStorage.removeItem('octopus:force_mock')
  }
  window.location.reload()
}

// ---- HTTP 工具函数 ----
async function fetchJson<T>(url: string, init?: RequestInit): Promise<T> {
  let res: Response
  try {
    res = await fetch(url, {
      ...init,
      headers: {
        'Content-Type': 'application/json',
        ...init?.headers,
      },
    })
  } catch (e) {
    throw mkErr('NETWORK_ERROR', `无法连接到服务器 (${(e as Error).message})`)
  }

  if (!res.ok) {
    let errBody: { code?: string; message?: string; detail?: Record<string, unknown> } | undefined
    try {
      errBody = await res.json()
    } catch {}
    const code = errBody?.code ?? `HTTP_${res.status}`
    const msg = errBody?.message ?? res.statusText ?? '请求失败'
    throw mkErr(code, msg, errBody?.detail)
  }

  return res.json() as Promise<T>
}

async function fetchNoContent(url: string, init?: RequestInit): Promise<void> {
  let res: Response
  try {
    res = await fetch(url, {
      ...init,
      headers: {
        'Content-Type': 'application/json',
        ...init?.headers,
      },
    })
  } catch (e) {
    throw mkErr('NETWORK_ERROR', `无法连接到服务器 (${(e as Error).message})`)
  }

  if (!res.ok) {
    let errBody: { code?: string; message?: string; detail?: Record<string, unknown> } | undefined
    try {
      errBody = await res.json()
    } catch {}
    const code = errBody?.code ?? `HTTP_${res.status}`
    const msg = errBody?.message ?? res.statusText ?? '请求失败'
    throw mkErr(code, msg, errBody?.detail)
  }
  // 消费响应体（这些端点多为 202 空体）：不读的话，浏览器会把「未读 body 被丢弃」
  // 记为 net::ERR_ABORTED，污染 Network 面板 / 自动化断言。功能本身不受影响，但顺手清掉。
  await res.text().catch(() => {})
}

// ---- 模拟网络延迟 ----
const NET = 60
function net<T>(p: Promise<T> | T, ms = NET + Math.random() * 90): Promise<T> {
  return delay(ms).then(() => p)
}

// ================= 资产库（#28） =================

export interface UploadedAsset { asset: string; size: number; url: string }

/**
 * 上传图片。服务端按内容 sha256 命名，因此**同一张图重复上传会命中同一资产**（自动去重）。
 * 调用方应先用 canvas 压到目标尺寸再传（见 ImageField）。
 */
export async function uploadAsset(blob: Blob): Promise<UploadedAsset> {
  if (isMockMode()) {
    // Mock 模式没有后端资产库：退化成 data URL，保证离线也能预览与走通链路
    const url = await new Promise<string>((resolve, reject) => {
      const fr = new FileReader()
      fr.onload = () => resolve(String(fr.result))
      fr.onerror = () => reject(mkErr('UPLOAD_FAILED', '读取图片失败'))
      fr.readAsDataURL(blob)
    })
    return { asset: url, size: blob.size, url }
  }
  let res: Response
  try {
    res = await fetch('/api/assets', {
      method: 'POST',
      headers: { 'Content-Type': blob.type || 'application/octet-stream' },
      body: blob,
    })
  } catch (e) {
    throw mkErr('NETWORK_ERROR', `无法连接到服务器 (${(e as Error).message})`)
  }
  if (!res.ok) {
    const err = await res.json().catch(() => null)
    throw mkErr(err?.code ?? `HTTP_${res.status}`, err?.message ?? '图片上传失败')
  }
  return res.json() as Promise<UploadedAsset>
}

/** 资产名 → <img src>；兼容 Mock 模式下的 data URL */
export function assetUrl(name: string): string {
  if (!name) return ''
  return name.startsWith('data:') ? name : `/api/assets/${name}`
}

// ================= 故事书（#23） =================

export function listStorybooks(releasedOnly = false): Promise<StorybookListItem[]> {
  if (isMockMode()) {
    return net(mock.listStorybooks(releasedOnly))
  }
  return fetchJson<StorybookListItem[]>(`/api/storybooks?released_only=${releasedOnly}`)
}

export function getStorybook(id: string): Promise<StorybookDocument> {
  if (isMockMode()) {
    return net(run(async () => {
      const d = mock.getStorybook(id)
      if (!d) throw mkErr('NOT_FOUND', '故事书不存在')
      return d
    }))
  }
  return fetchJson<StorybookDocument>(`/api/storybooks/${encodeURIComponent(id)}`)
}

export function createStorybookDraft(title?: string): Promise<StorybookDocument> {
  if (isMockMode()) {
    return net(run(async () => mock.createStorybookDraft(title)))
  }
  return fetchJson<StorybookDocument>('/api/storybooks', {
    method: 'POST',
    body: JSON.stringify({ title }),
  })
}

/** 删除故事书：级联删掉它的草稿/结对会话；已内嵌冻结副本的存档不受影响。 */
export function deleteStorybook(id: string): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.deleteStorybook(id) }), 180)
  }
  return fetchNoContent(`/api/storybooks/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export function saveDraft(
  id: string,
  draft: Storybook,
  baseVersion: number
): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
  if (isMockMode()) {
    return net(run(async () => mock.saveDraft(id, draft, baseVersion)))
  }
  return fetchJson<{ doc: StorybookDocument; issues: ValidationIssue[] }>(
    `/api/storybooks/${encodeURIComponent(id)}`,
    {
      method: 'PUT',
      body: JSON.stringify({ draft, base_version: baseVersion }),
    }
  )
}

export function publishDraft(
  id: string,
  baseVersion: number
): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
  if (isMockMode()) {
    return net(run(async () => mock.publishDraft(id, baseVersion)), 220)
  }
  return fetchJson<{ doc: StorybookDocument; issues: ValidationIssue[] }>(
    `/api/storybooks/${encodeURIComponent(id)}/publish`,
    {
      method: 'POST',
      body: JSON.stringify({ base_version: baseVersion }),
    }
  )
}

export function playtestStorybook(
  id: string,
  req?: { title?: string; controlledCharacterId?: string }
): Promise<SaveDetail> {
  if (isMockMode()) {
    return net(run(async () => {
      return mock.createSave(id, req?.title, req?.controlledCharacterId).detail
    }), 200)
  }
  return fetchJson<SaveDetail>(`/api/storybooks/${encodeURIComponent(id)}/sandbox`, {
    method: 'POST',
    body: JSON.stringify({
      title: req?.title,
      controlled_character_id: req?.controlledCharacterId,
    }),
  })
}

export function validateStorybook(sb: Storybook): Promise<ValidateResult> {
  if (isMockMode()) {
    return net(mock.validate(sb), 30)
  }
  return fetchJson<ValidateResult>('/api/validate', {
    method: 'POST',
    body: JSON.stringify(sb),
  })
}

// ================= Lua 试跑（#23 / #05） =================

export interface LuaRunRequest {
  script: string
  mode?: 'check' | 'condition' | 'hook'
  mount?: string
  actor?: unknown
  target?: unknown
  skill?: unknown
  difficulty?: number
  scene_id?: string
  round?: number
  relationships?: unknown[]
}

export interface LuaRunResult {
  ok: boolean
  mode?: string
  result?: Record<string, unknown>
  error?: string
  requests?: unknown[]
}

const LUA_FORBIDDEN = ['io.', 'os.', 'package.', 'debug.', 'require', 'dofile', 'loadfile', 'math.random', 'collectgarbage', 'coroutine.']

/** 编辑期 Lua 试跑：真实后端走沙箱执行；mock 模式只做越权 API 扫描。 */
export function runLua(req: LuaRunRequest): Promise<LuaRunResult> {
  if (isMockMode()) {
    const hit = LUA_FORBIDDEN.find(t => req.script.includes(t))
    if (hit) return net<LuaRunResult>({ ok: false, error: `Lua 使用了白名单之外的 API：${hit}` }, 30)
    return net<LuaRunResult>({ ok: false, error: 'mock 模式不执行 Lua，请连接后端试跑' }, 30)
  }
  return fetchJson<LuaRunResult>('/api/lua/run', {
    method: 'POST',
    body: JSON.stringify(req),
  })
}

// ================= 存档（#24） =================

export function listSaves(): Promise<SaveListItem[]> {
  if (isMockMode()) {
    return net(mock.listSaves())
  }
  return fetchJson<SaveListItem[]>('/api/saves')
}

export function getSave(id: string): Promise<SaveDetail> {
  if (isMockMode()) {
    return net(run(async () => {
      const s = mock.getSave(id)
      if (!s) throw mkErr('NOT_FOUND', '存档不存在')
      return s
    }))
  }
  return fetchJson<SaveDetail>(`/api/saves/${encodeURIComponent(id)}`)
}

export function createSave(storybookId: string, title?: string, controlledCharacterId?: string): Promise<SaveDetail> {
  if (isMockMode()) {
    return net(run(async () => mock.createSave(storybookId, title, controlledCharacterId).detail), 260)
  }
  return fetchJson<SaveDetail>('/api/saves', {
    method: 'POST',
    body: JSON.stringify({
      storybook_id: storybookId,
      title,
      controlled_character_id: controlledCharacterId
    })
  })
}

export function upgradeDryRun(saveId: string): Promise<UpgradeReport> {
  if (isMockMode()) {
    return net(run(async () => {
      const r = mock.upgradeDryRun(saveId)
      if (!r) throw mkErr('NOT_FOUND', '存档不存在')
      return r
    }), 300)
  }
  return fetchJson<UpgradeReport>(`/api/saves/${encodeURIComponent(saveId)}/upgrade/dry-run`, { method: 'POST' })
}

export function upgradeExecute(saveId: string, dispositions: { character_id: string; disposition: Disposition }[]): Promise<{ detail: SaveDetail; backupName: string }> {
  if (isMockMode()) {
    return net(run(async () => mock.upgradeExecute(saveId, dispositions)), 500)
  }
  return fetchJson<{ detail: SaveDetail; backup_name: string }>(`/api/saves/${encodeURIComponent(saveId)}/upgrade`, {
    method: 'POST',
    body: JSON.stringify({ dispositions })
  }).then(r => ({ detail: r.detail, backupName: r.backup_name }))
}

export function manualSave(saveId: string): Promise<SaveListItem> {
  if (isMockMode()) {
    return net(mock.manualSave(saveId), 180)
  }
  return fetchJson<SaveListItem>(`/api/saves/${encodeURIComponent(saveId)}/save`, { method: 'POST' })
}

/**
 * 导入存档。当前格式是 **zip 存档包**（save.json + assets/），直接传文件字节；
 * 也接受旧版单 JSON 对象（历史导出，无资产），后端自动识别。
 */
export async function importSave(pkgOrBlob: Blob | ArrayBuffer | SavePackage): Promise<SaveListItem> {
  const isBinary = pkgOrBlob instanceof Blob || pkgOrBlob instanceof ArrayBuffer
  if (isMockMode()) {
    const title = pkgOrBlob instanceof File ? pkgOrBlob.name : '导入存档'
    return net(run(async () => mock.importSaveFile(title)), 400)
  }
  return fetchJson<SaveListItem>('/api/saves/import', {
    method: 'POST',
    headers: isBinary
      ? { 'Content-Type': pkgOrBlob instanceof Blob ? (pkgOrBlob.type || 'application/zip') : 'application/zip' }
      : undefined,
    body: isBinary ? (pkgOrBlob as BodyInit) : JSON.stringify(pkgOrBlob),
  })
}

export async function exportSave(saveId: string): Promise<{ filename: string; blob: Blob }> {
  if (isMockMode()) {
    return net(run(async () => {
      const s = mock.getSave(saveId)
      if (!s) throw mkErr('NOT_FOUND', '存档不存在')
      const blob = new Blob([JSON.stringify({ format: 'octopus-save-package', version: 1, exported_at: new Date().toISOString(), save: s, commands: [], archived_commands: [], maintenance: [] })], { type: 'application/json' })
      return { filename: s.id + '.octopus.zip', blob }
    }), 200)
  }
  const res = await fetch(`/api/saves/${encodeURIComponent(saveId)}/export`)
  if (!res.ok) {
    const err = await res.json().catch(() => null)
    throw new Error(err?.message ?? '导出失败')
  }
  const blob = await res.blob()
  const contentDisp = res.headers.get('content-disposition')
  const match = contentDisp?.match(/filename="?([^"]+)"?/)
  const filename = match?.[1] ?? `${saveId}.octopus.zip`
  return { filename, blob }
}

/** 叙事历史分页（#17/#24 修订）：进页 / 重连 / 换模板回读故事 */
export async function getHistory(saveId: string, beforeSeq?: number, limit = 50): Promise<HistoryPage> {
  if (isMockMode()) {
    return net(run(async () => mock.getHistory(saveId, beforeSeq, limit)))
  }
  const q = new URLSearchParams()
  if (beforeSeq !== undefined) q.set('before_seq', String(beforeSeq))
  q.set('limit', String(limit))
  const raw = await fetchJson<{ events: PlayEvent[]; has_more?: boolean; hasMore?: boolean }>(
    `/api/saves/${encodeURIComponent(saveId)}/history?${q.toString()}`
  )
  return {
    events: raw.events,
    hasMore: raw.hasMore ?? raw.has_more ?? false,
  }
}

export function renameSave(saveId: string, title: string): Promise<SaveListItem> {
  if (isMockMode()) {
    return net(run(async () => mock.renameSave(saveId, title)), 120)
  }
  return fetchJson<SaveListItem>(`/api/saves/${encodeURIComponent(saveId)}`, {
    method: 'PATCH',
    body: JSON.stringify({ title })
  })
}

export function deleteSave(saveId: string): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.deleteSave(saveId) }), 180)
  }
  return fetchNoContent(`/api/saves/${encodeURIComponent(saveId)}`, { method: 'DELETE' })
}

export function getMaintenance(saveId: string): Promise<MaintenanceRow[]> {
  if (isMockMode()) {
    return net(run(async () => mock.getMaintenance(saveId)))
  }
  return fetchJson<MaintenanceRow[]>(`/api/saves/${encodeURIComponent(saveId)}/maintenance`)
}

export function getSaveSettings(saveId: string): Promise<SaveSettings> {
  if (isMockMode()) {
    return net(run(async () => mock.getSaveSettings(saveId)))
  }
  return fetchJson<SaveSettings>(`/api/saves/${encodeURIComponent(saveId)}/settings`)
}

export function setSaveSettings(saveId: string, settings: SaveSettings): Promise<SaveSettings> {
  if (isMockMode()) {
    return net(run(async () => mock.setSaveSettings(saveId, settings)), 120)
  }
  return fetchJson<SaveSettings>(`/api/saves/${encodeURIComponent(saveId)}/settings`, {
    method: 'PUT',
    body: JSON.stringify(settings)
  })
}

/** 新原点（#24 修订：玩家侧 v1） */
export async function newOrigin(saveId: string): Promise<{ detail: SaveDetail; archivedCount: number }> {
  if (isMockMode()) {
    return net(run(async () => mock.newOrigin(saveId)), 320)
  }
  const res = await fetchJson<{ ok: boolean; archived_count?: number }>(
    `/api/saves/${encodeURIComponent(saveId)}/origin`,
    { method: 'POST' }
  )
  const detail = await getSave(saveId)
  return { detail, archivedCount: res.archived_count ?? 0 }
}

/** 切换受控角色（#24 修订：状态类元指令直调） */
export function switchCharacter(saveId: string, characterId: string): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.switchCharacter(saveId, characterId) }), 100)
  }
  return fetchNoContent(`/api/saves/${encodeURIComponent(saveId)}/character`, {
    method: 'POST',
    body: JSON.stringify({ character_id: characterId })
  })
}

// ================= 应用配置（#26 AI Provider） =================

export async function getAppConfig(): Promise<AppConfig> {
  if (!isMockMode()) {
    try {
      const cfg = await fetchJson<AppConfig>('/api/config')
      if (typeof localStorage !== 'undefined') {
        localStorage.setItem('octopus:app_config', JSON.stringify(cfg))
      }
      return cfg
    } catch (e) {
      console.warn('获取服务端配置失败，尝试本地缓存', e)
    }
  }
  if (typeof localStorage !== 'undefined') {
    const cached = localStorage.getItem('octopus:app_config')
    if (cached) {
      try { return JSON.parse(cached) } catch {}
    }
  }
  return mock.getAppConfig()
}

export async function saveAppConfig(config: AppConfig): Promise<AppConfig> {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('octopus:app_config', JSON.stringify(config))
  }
  mock.saveAppConfig(config)
  if (!isMockMode()) {
    try {
      return await fetchJson<AppConfig>('/api/config', {
        method: 'PUT',
        body: JSON.stringify(config),
      })
    } catch (e) {
      console.warn('保存到服务端失败，已暂存本地', e)
    }
  }
  return config
}

export function testProvider(provider: ProviderConfig): Promise<ProviderTestResult> {
  if (!isMockMode()) {
    return fetchJson<ProviderTestResult>('/api/providers/test', {
      method: 'POST',
      body: JSON.stringify(provider),
    }).catch(e => ({ ok: false, message: (e as Error)?.message ?? '测试失败' }))
  }
  return net(run(async () => mock.testProvider(provider)), 400 + Math.random() * 400)
}

/** 探测模型：GET {base_url}/models（真后端执行，避免浏览器暴露 key） */
export function probeProviderModels(provider: Pick<ProviderConfig, 'base_url' | 'api_key' | 'kind'>): Promise<ProbeResult> {
  if (isMockMode()) {
    return net(run(async () => mock.probeProviderModels(provider)), 700 + Math.random() * 500)
  }
  return fetchJson<ProbeResult>('/api/providers/probe', {
    method: 'POST',
    body: JSON.stringify(provider)
  })
}

// ================= 结对（#23 ④，编辑器 C 范式） =================

/** 结对消息的文本附件：前端读文件后随消息一起送（后端折叠进正文给模型） */
export interface PairAttachment {
  name: string
  size?: number
  text: string
}

/** 送给结对对话的历史消息：支持 assistant.tool_calls 与 role=tool 结果回灌 */
export interface PairHistoryMessage {
  role: 'user' | 'assistant' | 'tool' | 'system'
  content: string
  tool_calls?: { id: string; type: 'function'; function: { name: string; arguments: string } }[]
  tool_call_id?: string
  /** 用户消息的附件 */
  attachments?: PairAttachment[]
}

export interface PairToolCall {
  id: string
  name: string
  arguments: string
}

/** 模型本轮真实用量（后端从流末尾的 Final 帧取回） */
export interface PairUsage {
  input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cached_input_tokens?: number
  cache_creation_input_tokens?: number
  tool_use_prompt_tokens?: number
  reasoning_tokens?: number
}

export interface PairChatResult {
  text: string
  deltas: string[]
  suggestions: PairSuggestion[]
  toolCalls: PairToolCall[]
  finishReason?: string
  /** 真实用量：有值时优先于前端估算 */
  usage?: PairUsage
  /** 本轮思考（reasoning）字符数：为 0 且文本为空时，多半是模型空手而归 */
  reasoningChars?: number
  /** 本轮思考正文（reasoning_content） */
  reasoning?: string
  /** 各类流事件计数，用于事后诊断 */
  counts?: Record<string, number>
}

/** 故事书实体的结构化引用（精准指向「要改这个」） */
export type { EntityRef }

/** 发给模型的目标实体：带完整定义 */
export interface PairFocusEntity {
  kind: string
  id?: string | null
  name: string
  entity: unknown
}

export interface PairChatOptions {
  provider_id?: string
  model?: string
  storybook?: Record<string, unknown>
  /** OpenAI 风格 function calling 工具定义；传入即进入工具模式 */
  tools?: unknown[]
  /** 本轮显式引用的目标实体（完整定义） */
  focus?: PairFocusEntity[]
  onDelta?: (delta: string) => void
  /** 思考流（reasoning_content）；replace=true 表示整块替换此前的增量 */
  onReasoning?: (text: string, replace?: boolean) => void
  onSuggestion?: (sug: PairSuggestion) => void
  onToolCall?: (tc: PairToolCall) => void
}

export async function pairChat(
  messages: PairHistoryMessage[],
  options?: PairChatOptions
): Promise<PairChatResult> {
  if (isMockMode()) {
    return net(run(async () => {
      const out = mock.pairStreamChat(messages as { role: 'user' | 'assistant'; content: string }[], options)
      return { text: out.text, deltas: typeof out.deltas === 'string' ? [out.deltas] : out.deltas, suggestions: out.suggestions, toolCalls: [] as PairToolCall[], finishReason: 'stop' }
    }), 500 + Math.random() * 400)
  }

  try {
    const res = await fetch('/api/pair/chat/stream', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        provider_id: options?.provider_id,
        model: options?.model,
        messages,
        storybook: options?.storybook,
        tools: options?.tools,
        focus: options?.focus,
      }),
    })

    if (!res.ok) {
      let errBody: { code?: string; message?: string } | undefined
      try { errBody = await res.json() } catch {}
      const msg = errBody?.message ?? `服务响应异常 (HTTP ${res.status})`
      throw mkErr(errBody?.code ?? `HTTP_${res.status}`, msg)
    }

    if (!res.body) {
      throw mkErr('EMPTY_RESPONSE', '服务未返回数据流')
    }

    const reader = res.body.getReader()
    const decoder = new TextDecoder('utf-8')
    let buffer = ''
    let fullText = ''
    const suggestions: PairSuggestion[] = []
    const toolCalls: PairToolCall[] = []
    let finishReason: string | undefined
    let usage: PairUsage | undefined
    let reasoningChars: number | undefined
    let reasoning = ''
    let counts: Record<string, number> | undefined
    let currentEvent = 'message'

    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })

      const lines = buffer.split('\n')
      buffer = lines.pop() ?? ''

      for (const line of lines) {
        const trimmed = line.trim()
        if (!trimmed) {
          currentEvent = 'message'
          continue
        }
        if (trimmed.startsWith('event:')) {
          currentEvent = trimmed.slice(6).trim()
          continue
        }
        if (trimmed.startsWith('data:')) {
          const dataStr = trimmed.slice(5).trim()
          try {
            const parsed = JSON.parse(dataStr)
            if (currentEvent === 'delta') {
              if (parsed.text) {
                fullText += parsed.text
                options?.onDelta?.(parsed.text)
              }
            } else if (currentEvent === 'reasoning') {
              if (typeof parsed.text === 'string' && parsed.text) {
                reasoning = parsed.replace ? parsed.text : reasoning + parsed.text
                options?.onReasoning?.(parsed.text, !!parsed.replace)
              }
            } else if (currentEvent === 'suggestion') {
              suggestions.push(parsed as PairSuggestion)
              options?.onSuggestion?.(parsed as PairSuggestion)
            } else if (currentEvent === 'tool_call') {
              const tc = parsed as PairToolCall
              if (tc && tc.name) {
                toolCalls.push(tc)
                options?.onToolCall?.(tc)
              }
            } else if (currentEvent === 'usage') {
              if (parsed.usage) usage = parsed.usage as PairUsage
              if (parsed.finish_reason) finishReason = String(parsed.finish_reason)
              if (typeof parsed.reasoning_chars === 'number') reasoningChars = parsed.reasoning_chars
              if (parsed.counts) counts = parsed.counts as Record<string, number>
            } else if (currentEvent === 'done') {
              if (parsed.full_text) fullText = parsed.full_text
              if (parsed.finish_reason) finishReason = String(parsed.finish_reason)
              if (parsed.usage) usage = parsed.usage as PairUsage
              if (typeof parsed.reasoning_chars === 'number') reasoningChars = parsed.reasoning_chars
              if (typeof parsed.reasoning === 'string' && parsed.reasoning) reasoning = parsed.reasoning
              if (parsed.counts) counts = parsed.counts as Record<string, number>
              if (Array.isArray(parsed.tool_calls) && parsed.tool_calls.length) {
                for (const tc of parsed.tool_calls as PairToolCall[]) {
                  if (!toolCalls.some(ex => ex.id === tc.id)) {
                    toolCalls.push(tc)
                    options?.onToolCall?.(tc)
                  }
                }
              }
              if (Array.isArray(parsed.suggestions) && parsed.suggestions.length) {
                for (const s of parsed.suggestions) {
                  if (!suggestions.some(ex => ex.id === s.id)) {
                    suggestions.push(s)
                    options?.onSuggestion?.(s)
                  }
                }
              }
            } else if (currentEvent === 'error') {
              throw mkErr('STREAM_ERROR', parsed.message || '推理中断')
            }
          } catch (err) {
            if ((err as Error)?.message?.includes('推理中断')) throw err
          }
        }
      }
    }

    return {
      text: fullText,
      deltas: [fullText],
      suggestions,
      toolCalls,
      finishReason,
      usage,
      reasoningChars,
      reasoning: reasoning || undefined,
      counts,
    }
  } catch (err) {
    throw wrapErr(err)
  }
}

// ---- 结对会话线程持久化（#23 ④）：一本故事书多条会话，刷新 / 重进即恢复 ----

export interface PairThreadRecord {
  id: string
  storybook_id: string
  title: string
  message_count: number
  created_at: string
  updated_at: string
  /** 尚未处理的「待审查改动」（前端形状，落库后可恢复） */
  pending_suggestions?: unknown
}

export interface PairMessageRecord {
  seq: number
  role: 'user' | 'assistant'
  content: string
  reasoning?: string | null
  model?: string | null
  is_error?: boolean
  tools?: unknown
  refs?: EntityRef[] | null
  attachments?: PairAttachment[] | null
}

export async function listPairThreads(storybookId: string): Promise<PairThreadRecord[]> {
  if (isMockMode()) {
    return net(run(async () => mock.listPairThreads(storybookId)), 120)
  }
  return fetchJson<PairThreadRecord[]>(
    `/api/storybooks/${encodeURIComponent(storybookId)}/pair/threads`
  )
}

export async function createPairThread(storybookId: string, title?: string): Promise<PairThreadRecord> {
  if (isMockMode()) {
    return net(run(async () => mock.createPairThread(storybookId, title)), 80)
  }
  return fetchJson<PairThreadRecord>(
    `/api/storybooks/${encodeURIComponent(storybookId)}/pair/threads`,
    { method: 'POST', body: JSON.stringify({ title }) }
  )
}

export async function renamePairThread(threadId: string, title: string): Promise<PairThreadRecord> {
  if (isMockMode()) {
    return net(run(async () => mock.renamePairThread(threadId, title)), 60)
  }
  return fetchJson<PairThreadRecord>(
    `/api/pair/threads/${encodeURIComponent(threadId)}`,
    { method: 'PATCH', body: JSON.stringify({ title }) }
  )
}

/** 覆盖线程的待审查改动（刷新 / 切会话后恢复；null 表示清空）。 */
export async function setPairThreadPending(threadId: string, suggestions: unknown): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.setPairThreadPending(threadId, suggestions) }), 40)
  }
  await fetchNoContent(`/api/pair/threads/${encodeURIComponent(threadId)}/pending`, {
    method: 'PUT',
    body: JSON.stringify({ suggestions }),
  })
}

export async function deletePairThread(threadId: string): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.deletePairThread(threadId) }), 60)
  }
  await fetchNoContent(`/api/pair/threads/${encodeURIComponent(threadId)}`, { method: 'DELETE' })
}

export async function getPairMessages(threadId: string): Promise<PairMessageRecord[]> {
  if (isMockMode()) {
    return net(run(async () => mock.getPairMessages(threadId)), 120)
  }
  return fetchJson<PairMessageRecord[]>(
    `/api/pair/threads/${encodeURIComponent(threadId)}/messages`
  )
}

export async function appendPairMessages(threadId: string, messages: PairMessageRecord[]): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.appendPairMessages(threadId, messages) }), 60)
  }
  await fetchNoContent(
    `/api/pair/threads/${encodeURIComponent(threadId)}/messages`,
    { method: 'POST', body: JSON.stringify({ messages }) }
  )
}

export async function clearPairMessages(threadId: string): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { mock.clearPairMessages(threadId) }), 60)
  }
  await fetchNoContent(
    `/api/pair/threads/${encodeURIComponent(threadId)}/messages`,
    { method: 'DELETE' }
  )
}

// ================= 游玩（#24/#17） =================

export function hydrate(saveId: string): Promise<WorldProjection> {
  if (isMockMode()) {
    return net(run(async () => {
      const p = mock.currentProjection(saveId)
      if (!p) throw mkErr('NOT_FOUND', '存档不存在')
      return p
    }))
  }
  return fetchJson<WorldProjection>(`/api/saves/${encodeURIComponent(saveId)}/state`)
}

export interface RoundSubmitHandle { onEvent: (e: PlayEvent) => void; onProgress?: (p: { stage: PhaseStage; detail?: string }) => void }

export async function submitRound(
  saveId: string,
  channel: 'character' | 'meta' | 'gm',
  text: string,
  requestId: string,
  h: RoundSubmitHandle,
  refs?: EntityRef[],
  focus?: FocusEntity[]
): Promise<void> {
  if (isMockMode()) {
    return mock.submitRound(saveId, channel, text, requestId, h.onProgress, refs).then(() => {})
  }
  await fetchNoContent(`/api/saves/${encodeURIComponent(saveId)}/rounds`, {
    method: 'POST',
    body: JSON.stringify({ channel, text, request_id: requestId, refs, focus })
  })
}

/**
 * 重跑最后一个回合：后端归档旧回合命令、把会话回滚到回合前，再用原输入重跑。
 * focus 是前端重解析出来的引用实体完整定义（日志里只存轻量 refs）。
 */
export function rerunRound(saveId: string, focus?: FocusEntity[], text?: string): Promise<void> {
  if (isMockMode()) {
    return net(run(async () => { await mock.rerunRound(saveId, focus, text) }), 260)
  }
  return fetchNoContent(`/api/saves/${encodeURIComponent(saveId)}/rerun`, {
    method: 'POST',
    body: JSON.stringify({ focus, text })
  })
}

export interface StreamHandlers {
  onStatus?: (s: StreamStatus) => void
  /** 断线重连成功后触发：调用方按自己的 seq 水位线补拉错过的事件 */
  onResync?: () => void
}

/** 订阅演出流。内置指数退避重连；重连成功后回调 onResync 由调用方补拉。 */
export function subscribe(
  saveId: string,
  sink: (e: PlayEvent) => void,
  handlers: StreamHandlers = {}
): () => void {
  if (isMockMode()) {
    handlers.onStatus?.('open')
    return mock.subscribe(saveId, sink)
  }
  let closed = false
  let es: EventSource | null = null
  let retry = 0
  let timer: ReturnType<typeof setTimeout> | null = null

  const attach = () => {
    if (closed) return
    handlers.onStatus?.('connecting')
    es = new EventSource(`/api/saves/${encodeURIComponent(saveId)}/stream`)
    es.addEventListener('play', (ev) => {
      try {
        const parsed = JSON.parse((ev as MessageEvent).data) as PlayEvent
        sink(parsed)
      } catch (err) {
        console.warn('解析 SSE 演出流事件失败', err)
      }
    })
    es.onopen = () => {
      handlers.onStatus?.('open')
      // 首次与重连都补拉：关闭「拉历史」到「建立 SSE」之间的窗口期丢事件。
      handlers.onResync?.()
      retry = 0
    }
    es.onerror = () => {
      handlers.onStatus?.('error')
      es?.close()
      es = null
      if (closed) return
      retry += 1
      const wait = Math.min(1000 * 2 ** Math.min(retry - 1, 4), 15000)
      timer = setTimeout(attach, wait)
    }
  }
  attach()
  return () => {
    closed = true
    if (timer) clearTimeout(timer)
    es?.close()
    es = null
  }
}

export async function confirmAction(
  saveId: string,
  roundId: number,
  actionId: string,
  decision: 'confirm' | 'cancel'
): Promise<void> {
  if (isMockMode()) {
    await mock.confirmAction(saveId, roundId, actionId, decision)
    return
  }
  await fetchNoContent(`/api/saves/${encodeURIComponent(saveId)}/rounds/${roundId}/confirmation`, {
    method: 'POST',
    body: JSON.stringify({ action_id: actionId, decision })
  })
}

export function setAutoConfirm(v: boolean): void {
  mock.setAutoConfirmDefault(v)
}

export function streamStatus(): StreamStatus {
  return 'open'
}

// 确保种子初始化（供 mock 模式随时可用）
mock.ensureSeeded()
