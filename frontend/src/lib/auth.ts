// ============================================================
// 账户与登录态（多账户）
//
// 令牌存哪：默认 sessionStorage —— **每个标签页一份**，所以同一个浏览器可以同时
// 登两个账号（这正是「多账户」在客户端最实际的形态）；只有勾了「记住我」才落
// localStorage（关掉标签页也还在）。读取顺序 session 优先。
//
// 失效判定只有一条：`/api/auth/me` 返回 401 → 清空令牌并回登录页。
// 本模块直接 fetch，不经 `@/api`——`@/api` 反过来要用这里的令牌，避免循环依赖。
// ============================================================
import { reactive } from 'vue'
import { isMockMode } from './mock-mode'
import type { Account } from '@/types'

const TOKEN_KEY = 'octopus:token'
const USER_KEY = 'octopus:user'

/** Mock 模式下的本地账户：没有后端也要能进页面看 UI。 */
const MOCK_USER: Account = {
  id: 'mock-user',
  username: 'octopus',
  display_name: '本地演示',
  is_admin: true,
  must_change_password: false,
  created_at: '',
  last_login_at: null,
}

function safeGet(store: Storage, key: string): string | null {
  try { return store.getItem(key) } catch { return null }
}
function safeSet(store: Storage, key: string, value: string): void {
  try { store.setItem(key, value) } catch { /* 隐私模式下不可写：退化为仅当前内存态 */ }
}
function safeRemove(store: Storage, key: string): void {
  try { store.removeItem(key) } catch { /* ignore */ }
}

/** 令牌：session 优先，其次「记住我」写下的 localStorage。 */
export function getToken(): string | null {
  if (typeof window === 'undefined') return null
  return safeGet(sessionStorage, TOKEN_KEY) ?? safeGet(localStorage, TOKEN_KEY)
}

/** 请求头（无令牌时为空对象；公开端点不需要它）。 */
export function authHeaders(): Record<string, string> {
  const token = getToken()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

/** EventSource 无法自定义请求头：演出流只能把令牌挂在查询串上。 */
export function withTokenQuery(url: string): string {
  const token = getToken()
  if (!token) return url
  return url + (url.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token)
}

function readCachedUser(): Account | null {
  if (typeof window === 'undefined') return null
  const raw = safeGet(sessionStorage, USER_KEY) ?? safeGet(localStorage, USER_KEY)
  if (!raw) return null
  try { return JSON.parse(raw) as Account } catch { return null }
}

function cacheUser(user: Account): void {
  const store = safeGet(sessionStorage, TOKEN_KEY) ? sessionStorage : localStorage
  safeSet(store, USER_KEY, JSON.stringify(user))
}

export function clearSession(): void {
  if (typeof window === 'undefined') return
  for (const store of [sessionStorage, localStorage]) {
    safeRemove(store, TOKEN_KEY)
    safeRemove(store, USER_KEY)
  }
}

function persist(token: string, user: Account, remember: boolean): void {
  clearSession()
  const store = remember ? localStorage : sessionStorage
  safeSet(store, TOKEN_KEY, token)
  safeSet(store, USER_KEY, JSON.stringify(user))
}

// ---- 响应式登录态（页面只读它） ----
const state = reactive({
  user: readCachedUser() as Account | null,
  /** 是否已向服务端确认过登录态（避免每次路由跳转都打一次 /me）。 */
  confirmed: false,
  /** 仍在使用初始口令：页面据此常驻提醒改密。 */
  mustChangePassword: readCachedUser()?.must_change_password ?? false,
})
export const auth = state

export function currentUser(): Account | null {
  return state.user
}
export function isAdmin(): boolean {
  return state.user?.is_admin ?? false
}

let bootstrap: Promise<boolean> | null = null

/** 401 时的统一出口：清令牌 + 交给路由跳登录页（由 main.ts 注册，避免这里依赖 router）。 */
let onUnauthorized: (() => void) | null = null
export function setUnauthorizedHandler(fn: (() => void) | null): void {
  onUnauthorized = fn
}

/** 任意请求拿到 401 都调它：清空登录态并通知跳转。 */
export function handleUnauthorized(): void {
  if (!state.user && !getToken()) return
  clearSession()
  state.user = null
  state.confirmed = true
  state.mustChangePassword = false
  bootstrap = Promise.resolve(false)
  onUnauthorized?.()
}

// ---- 与后端对话（只处理账户相关端点） ----
interface ApiFailure extends Error { code?: string }

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response
  try {
    res = await fetch(path, {
      ...init,
      headers: { 'Content-Type': 'application/json', ...authHeaders(), ...init?.headers },
    })
  } catch (e) {
    const err = new Error(`无法连接到服务器（${(e as Error).message}）`) as ApiFailure
    err.code = 'NETWORK_ERROR'
    throw err
  }
  const text = await res.text()
  let body: unknown = null
  try { body = text ? JSON.parse(text) : null } catch { /* 非 JSON 响应 */ }
  if (!res.ok) {
    const detail = body as { code?: string; message?: string } | null
    const err = new Error(detail?.message ?? res.statusText ?? '请求失败') as ApiFailure
    err.code = detail?.code ?? `HTTP_${res.status}`
    throw err
  }
  return body as T
}

interface LoginResponse { token: string; expires_at: string; account: Account }

function acceptSession(session: LoginResponse, remember: boolean): void {
  persist(session.token, session.account, remember)
  state.user = session.account
  state.confirmed = true
  state.mustChangePassword = session.account.must_change_password
  bootstrap = Promise.resolve(true)
}

export async function login(username: string, password: string, remember = false): Promise<Account> {
  if (isMockMode()) return acceptMock()
  const session = await call<LoginResponse>('/api/auth/login', {
    method: 'POST',
    body: JSON.stringify({ username, password }),
  })
  acceptSession(session, remember)
  return session.account
}

export async function register(
  username: string,
  password: string,
  displayName?: string,
  remember = false,
): Promise<Account> {
  if (isMockMode()) return acceptMock()
  const session = await call<LoginResponse>('/api/auth/register', {
    method: 'POST',
    body: JSON.stringify({
      username,
      password,
      display_name: displayName?.trim() ? displayName.trim() : null,
    }),
  })
  acceptSession(session, remember)
  return session.account
}

function acceptMock(): Account {
  state.user = MOCK_USER
  state.confirmed = true
  state.mustChangePassword = false
  bootstrap = Promise.resolve(true)
  return MOCK_USER
}

export async function logout(): Promise<void> {
  if (!isMockMode() && getToken()) {
    // 登出失败（令牌已失效 / 网络不通）也要把本地清干净。
    await call<void>('/api/auth/logout', { method: 'POST' }).catch(() => undefined)
  }
  clearSession()
  state.user = null
  state.confirmed = true
  state.mustChangePassword = false
  bootstrap = Promise.resolve(false)
}

export async function changePassword(currentPassword: string, newPassword: string): Promise<void> {
  if (isMockMode()) {
    state.mustChangePassword = false
    return
  }
  await call<void>('/api/auth/password', {
    method: 'POST',
    body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
  })
  state.mustChangePassword = false
  if (state.user) {
    const next = { ...state.user, must_change_password: false }
    state.user = next
    cacheUser(next)
  }
}

/**
 * 路由守卫用：确认当前是否已登录。首次调用会打一次 `/api/auth/me`，
 * 之后走内存缓存（登录 / 登出会重置缓存）。
 */
export function ensureAuth(): Promise<boolean> {
  if (isMockMode()) {
    if (!state.user) state.user = MOCK_USER
    state.confirmed = true
    return Promise.resolve(true)
  }
  if (state.confirmed) return Promise.resolve(state.user !== null)
  if (bootstrap) return bootstrap
  bootstrap = (async () => {
    if (!getToken()) {
      state.confirmed = true
      return false
    }
    try {
      const me = await call<Account>('/api/auth/me')
      state.user = me
      state.confirmed = true
      state.mustChangePassword = me.must_change_password
      cacheUser(me)
      return true
    } catch (e) {
      // 401：令牌真的无效 → 清；其它错误（后端没起）也当作未登录，登录页会再报一次。
      if ((e as ApiFailure).code === 'unauthorized' || (e as ApiFailure).code === 'HTTP_401') {
        clearSession()
      }
      state.user = null
      state.confirmed = true
      return false
    }
  })()
  return bootstrap
}
