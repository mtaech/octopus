// 管理后台数据 store（feature 一文件，与列表页同款约定）
// 职责：拉概览 + 账户列表，并把「改完就刷新」收在一处，页面只消费派生数据。
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import {
  getAdminOverview, listAdminUsers, createAdminUser, updateAdminUser,
  resetAdminUserPassword, revokeAdminUserSessions, deleteAdminUser,
} from '@/api'
import type {
  AdminOverview, AdminUserRow, AdminCreateUserRequest, AdminUpdateUserRequest,
} from '@/types'

export const useAdminStore = defineStore('admin', () => {
  const overview = ref<AdminOverview | null>(null)
  const users = ref<AdminUserRow[]>([])
  const loading = ref(false)
  const loaded = ref(false)
  const error = ref<string | null>(null)

  /** 搜索词（用户名 / 显示名，大小写不敏感）。本地过滤：账户数量级很小。 */
  const query = ref('')

  const filteredUsers = computed<AdminUserRow[]>(() => {
    const q = query.value.trim().toLowerCase()
    if (!q) return users.value
    return users.value.filter(
      u => u.username.toLowerCase().includes(q) || u.display_name.toLowerCase().includes(q)
    )
  })

  /** 管理员数量：前端据此预告「不能降级最后一个管理员」。 */
  const adminCount = computed(() => users.value.filter(u => u.is_admin).length)

  async function load(): Promise<void> {
    loading.value = true
    error.value = null
    try {
      const [ov, list] = await Promise.all([getAdminOverview(), listAdminUsers()])
      overview.value = ov
      users.value = list
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      loading.value = false
      loaded.value = true
    }
  }

  async function create(req: AdminCreateUserRequest): Promise<AdminUserRow> {
    const row = await createAdminUser(req)
    await load()
    return row
  }

  async function update(id: string, req: AdminUpdateUserRequest): Promise<AdminUserRow> {
    const row = await updateAdminUser(id, req)
    await load()
    return row
  }

  async function resetPassword(id: string, newPassword: string): Promise<void> {
    await resetAdminUserPassword(id, newPassword)
    await load()
  }

  async function revokeSessions(id: string): Promise<number> {
    const res = await revokeAdminUserSessions(id)
    await load()
    return res.revoked
  }

  async function remove(id: string, purge: boolean): Promise<{ saves_deleted: number; storybooks_deleted: number }> {
    const res = await deleteAdminUser(id, purge)
    await load()
    return res
  }

  return {
    overview, users, filteredUsers, adminCount, query, loading, loaded, error,
    load, create, update, resetPassword, revokeSessions, remove,
  }
})
