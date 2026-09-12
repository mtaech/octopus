// ============================================================
// 命令式确认框（Promise 化）：替代浏览器原生 window.confirm。
// 用法：const ok = await confirm({ title: '删除？', destructive: true })
// 由 App 里挂载的 <ConfirmHost /> 渲染 shadcn AlertDialog。
// ============================================================
import { ref } from 'vue'

export interface ConfirmOptions {
  title: string
  description?: string
  confirmText?: string
  cancelText?: string
  /** 破坏性操作：确认按钮用 destructive 配色 */
  destructive?: boolean
}

export const confirmOpen = ref(false)
export const confirmOpts = ref<ConfirmOptions | null>(null)
let resolver: ((v: boolean) => void) | null = null

/** 弹出确认框，返回用户是否点了确认。 */
export function confirm(opts: ConfirmOptions | string): Promise<boolean> {
  // 已有未决确认：先按「取消」结掉，避免 Promise 悬挂。
  if (resolver) { resolver(false); resolver = null }
  confirmOpts.value = typeof opts === 'string' ? { title: opts } : opts
  confirmOpen.value = true
  return new Promise<boolean>(resolve => { resolver = resolve })
}

export function resolveConfirm(v: boolean): void {
  confirmOpen.value = false
  const r = resolver
  resolver = null
  if (r) r(v)
}
