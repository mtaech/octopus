// ============================================================
// Octopus 前端 API 层 —— 页面唯一的数据入口（#23/#24 契约面）
// 模式：优先直连真实 axum 后端（/api 经 Vite 代理转发到 127.0.0.1:8787）；
//       通过 URL ?mock=1 或 localStorage['octopus:force_mock']='1' 可随时切回 Mock。
// ============================================================
import type {
  Storybook, SaveListItem, SaveDetail, StorybookListItem, StorybookDocument,
  ValidateResult, ValidationIssue, UpgradeReport, Disposition,
  PairSuggestion, WorldProjection, PlayEvent, StreamStatus, PhaseStage,
  HistoryPage, SaveSettings, MaintenanceRow, AppConfig, ProviderTestResult, ProviderConfig, ProbeResult
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
}

// ---- 模拟网络延迟 ----
const NET = 60
function net<T>(p: Promise<T> | T, ms = NET + Math.random() * 90): Promise<T> {
  return delay(ms).then(() => p)
}

// ================= 故事书（#23） =================

export function listStorybooks(releasedOnly = false): Promise<StorybookListItem[]> {
  if (isMockMode()) {
    return net(mock.listStorybooks(releasedOnly))
  }
  return fetchJson<StorybookListItem[]>('/api/storybooks')
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
  // 后端故事书草稿接口待 #23 接入，此处暂走 mock
  return net(run(async () => mock.createStorybookDraft(title)))
}

export function saveDraft(id: string, draft: Storybook, baseVersion: number): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
  // 后端草稿保存待 #23 接入，此处暂走 mock
  return net(run(async () => mock.saveDraft(id, draft, baseVersion)))
}

export function publishDraft(id: string, baseVersion: number): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
  // 后端发布待 #23 接入，此处暂走 mock
  return net(run(async () => mock.publishDraft(id, baseVersion)), 220)
}

export function validateStorybook(sb: Storybook): Promise<ValidateResult> {
  return net(mock.validate(sb), 30)
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
  return net(run(async () => {
    const r = mock.upgradeDryRun(saveId)
    if (!r) throw mkErr('NOT_FOUND', '存档不存在')
    return r
  }), 300)
}

export function upgradeExecute(saveId: string, dispositions: { character_id: string; disposition: Disposition }[]): Promise<{ detail: SaveDetail; backupName: string }> {
  return net(run(async () => mock.upgradeExecute(saveId, dispositions)), 500)
}

export function manualSave(saveId: string): Promise<SaveListItem> {
  if (isMockMode()) {
    return net(mock.manualSave(saveId), 180)
  }
  return fetchJson<SaveListItem>(`/api/saves/${encodeURIComponent(saveId)}/save`, { method: 'POST' })
}

export function importSave(fileName: string): Promise<SaveListItem> {
  return net(run(async () => mock.importSaveFile(fileName)), 400)
}

export function exportSave(saveId: string): Promise<{ filename: string; blob: Blob }> {
  return net(run(async () => {
    const s = mock.getSave(saveId)
    if (!s) throw mkErr('NOT_FOUND', '存档不存在')
    const blob = new Blob([JSON.stringify({ save: { id: s.id, storybook: s.storybook.meta.title, revision: s.embedded_revision }, exported: true })], { type: 'application/octet-stream' })
    return { filename: s.id + '.sqlite', blob }
  }), 200)
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

export function getAppConfig(): Promise<AppConfig> {
  return net(run(async () => mock.getAppConfig()))
}

export function saveAppConfig(config: AppConfig): Promise<AppConfig> {
  return net(run(async () => mock.saveAppConfig(config)), 220)
}

export function testProvider(provider: ProviderConfig): Promise<ProviderTestResult> {
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

export function pairChat(messages: { role: 'user' | 'assistant'; content: string }[]): Promise<{ deltas: string[]; suggestions: PairSuggestion[] }> {
  return net(run(async () => {
    const out = mock.pairStreamChat(messages)
    return { deltas: typeof out.deltas === 'string' ? [out.deltas] : out.deltas, suggestions: out.suggestions }
  }), 500 + Math.random() * 400)
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
  channel: 'character' | 'meta',
  text: string,
  requestId: string,
  h: RoundSubmitHandle
): Promise<void> {
  if (isMockMode()) {
    return mock.submitRound(saveId, channel, text, requestId, h.onProgress).then(() => {})
  }
  await fetchNoContent(`/api/saves/${encodeURIComponent(saveId)}/rounds`, {
    method: 'POST',
    body: JSON.stringify({ channel, text, request_id: requestId })
  })
}

export function subscribe(saveId: string, sink: (e: PlayEvent) => void): () => void {
  if (isMockMode()) {
    return mock.subscribe(saveId, sink)
  }
  const es = new EventSource(`/api/saves/${encodeURIComponent(saveId)}/stream`)
  es.addEventListener('play', (ev) => {
    try {
      const parsed = JSON.parse(ev.data) as PlayEvent
      sink(parsed)
    } catch (err) {
      console.warn('解析 SSE 演出流事件失败', err)
    }
  })
  es.onerror = (err) => {
    console.warn('SSE 演出流连接异常/重连中', err)
  }
  return () => {
    es.close()
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
