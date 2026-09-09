// ============================================================
// Octopus 前端 API 层 —— 页面唯一的数据入口（#23/#24 契约面）
// 现在：内存 Mock 后端（api/mock/backend.ts）
// 将来：换真 axum 后端 = 改本文件内部实现，签名不变，页面零改动。
// ============================================================
import type {
  Storybook, SaveListItem, SaveDetail, StorybookListItem, StorybookDocument,
  ValidateResult, ValidationIssue, UpgradeReport, Disposition,
  PairSuggestion, WorldProjection, PlayEvent, StreamStatus, PhaseStage,
  HistoryPage, SaveSettings, MaintenanceRow, AppConfig, ProviderTestResult, ProviderConfig
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

// ---- 模拟网络延迟 ----
const NET = 60
function net<T>(p: Promise<T> | T, ms = NET + Math.random() * 90): Promise<T> {
  return delay(ms).then(() => p)
}

// ================= 故事书（#23） =================

export function listStorybooks(releasedOnly = false): Promise<StorybookListItem[]> {
  return net(mock.listStorybooks(releasedOnly))
}
export function getStorybook(id: string): Promise<StorybookDocument> {
  return net(run(async () => {
    const d = mock.getStorybook(id)
    if (!d) throw mkErr('NOT_FOUND', '故事书不存在')
    return d
  }))
}
export function createStorybookDraft(title?: string): Promise<StorybookDocument> {
  return net(run(async () => mock.createStorybookDraft(title)))
}
export function saveDraft(id: string, draft: Storybook, baseVersion: number): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
  return net(run(async () => mock.saveDraft(id, draft, baseVersion)))
}
export function publishDraft(id: string, baseVersion: number): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
  return net(run(async () => mock.publishDraft(id, baseVersion)), 220)
}
export function validateStorybook(sb: Storybook): Promise<ValidateResult> {
  return net(mock.validate(sb), 30)
}

// ================= 存档（#24） =================

export function listSaves(): Promise<SaveListItem[]> {
  return net(mock.listSaves())
}
export function getSave(id: string): Promise<SaveDetail> {
  return net(run(async () => {
    const s = mock.getSave(id)
    if (!s) throw mkErr('NOT_FOUND', '存档不存在')
    return s
  }))
}
export function createSave(storybookId: string, title?: string, controlledCharacterId?: string): Promise<SaveDetail> {
  return net(run(async () => mock.createSave(storybookId, title, controlledCharacterId).detail), 260)
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
  return net(mock.manualSave(saveId), 180)
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
export function getHistory(saveId: string, beforeSeq?: number, limit = 50): Promise<HistoryPage> {
  return net(run(async () => mock.getHistory(saveId, beforeSeq, limit)))
}
export function renameSave(saveId: string, title: string): Promise<SaveListItem> {
  return net(run(async () => mock.renameSave(saveId, title)), 120)
}
export function deleteSave(saveId: string): Promise<void> {
  return net(run(async () => { mock.deleteSave(saveId) }), 180)
}
export function getMaintenance(saveId: string): Promise<MaintenanceRow[]> {
  return net(run(async () => mock.getMaintenance(saveId)))
}
export function getSaveSettings(saveId: string): Promise<SaveSettings> {
  return net(run(async () => mock.getSaveSettings(saveId)))
}
export function setSaveSettings(saveId: string, settings: SaveSettings): Promise<SaveSettings> {
  return net(run(async () => mock.setSaveSettings(saveId, settings)), 120)
}
/** 新原点（#24 修订：玩家侧 v1） */
export function newOrigin(saveId: string): Promise<{ detail: SaveDetail; archivedCount: number }> {
  return net(run(async () => mock.newOrigin(saveId)), 320)
}
/** 切换受控角色（#24 修订：状态类元指令直调） */
export function switchCharacter(saveId: string, characterId: string): Promise<void> {
  return net(run(async () => { mock.switchCharacter(saveId, characterId) }), 100)
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

// ================= 结对（#23 ④，编辑器 C 范式） =================

export function pairChat(messages: { role: 'user' | 'assistant'; content: string }[]): Promise<{ deltas: string[]; suggestions: PairSuggestion[] }> {
  return net(run(async () => {
    const out = mock.pairStreamChat(messages)
    return { deltas: typeof out.deltas === 'string' ? [out.deltas] : out.deltas, suggestions: out.suggestions }
  }), 500 + Math.random() * 400)
}

// ================= 游玩（#24/#17） =================

export function hydrate(saveId: string): Promise<WorldProjection> {
  return net(run(async () => {
    const p = mock.currentProjection(saveId)
    if (!p) throw mkErr('NOT_FOUND', '存档不存在')
    return p
  }))
}
export interface RoundSubmitHandle { onEvent: (e: PlayEvent) => void; onProgress?: (p: { stage: PhaseStage; detail?: string }) => void }
export function submitRound(saveId: string, channel: 'character' | 'meta', text: string, requestId: string, h: RoundSubmitHandle): Promise<void> {
  return mock.submitRound(saveId, channel, text, requestId, h.onProgress).then(() => {})
}
export function subscribe(saveId: string, sink: (e: PlayEvent) => void): () => void {
  return mock.subscribe(saveId, sink)
}
export function confirmAction(saveId: string, roundId: number, actionId: string, decision: 'confirm' | 'cancel'): Promise<void> {
  return net(run(async () => { await mock.confirmAction(saveId, roundId, actionId, decision) }), 120)
}
export function setAutoConfirm(v: boolean): void { mock.setAutoConfirmDefault(v) }
export function streamStatus(): StreamStatus { return 'open' }

// 确保种子初始化（真实场景由后端启动时完成）
mock.ensureSeeded()
