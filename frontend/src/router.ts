import { createRouter, createWebHistory } from 'vue-router'
import { ensureAuth, isAdmin } from '@/lib/auth'
import { toast } from '@/api'

// 路由（蓝图 #18 ④）：/login · /register 账户入口；/ 列表 · /storybook/:id/edit 编辑器
// （id=new 表示新建草稿）· /play/:saveId 游玩页 · /admin 管理后台（仅管理员）
const router = createRouter({
  history: createWebHistory(),
  routes: [
    // 公开页：未登录也能进（登录 / 注册共用同一组件，路由名区分模式）
    { path: '/login', name: 'login', component: () => import('@/pages/auth/AuthPage.vue'), meta: { public: true } },
    { path: '/register', name: 'register', component: () => import('@/pages/auth/AuthPage.vue'), meta: { public: true } },
    { path: '/', name: 'list', component: () => import('@/pages/list/ListPage.vue') },
    { path: '/storybook/:id/edit', name: 'editor', component: () => import('@/pages/editor/EditorPage.vue') },
    { path: '/play/:saveId', name: 'play', component: () => import('@/pages/play/PlayPage.vue') },
    // 管理后台：meta.admin 表示「还要过管理员这一关」（服务端同样有 guard_admin 兜底）
    { path: '/admin', name: 'admin', component: () => import('@/pages/admin/AdminPage.vue'), meta: { admin: true } },
    { path: '/:pathMatch(.*)*', redirect: '/' }
  ]
})

/**
 * 全局守卫：多账户的唯一门禁。
 *
 * - 未登录访问受保护页 → 跳登录页，并带上 `redirect` 回跳；
 * - 已登录访问登录 / 注册页 → 直接回应用（避免出现「已登录还在登录页」的迷惑态）；
 * - 非管理员访问 `meta.admin` 的页面 → 弹回列表页（不停在「你没权限」的空页上）；
 * - `ensureAuth()` 首次会向 `/api/auth/me` 确认令牌，之后走内存缓存（登录 / 登出会重置）。
 */
router.beforeEach(async (to) => {
  const authed = await ensureAuth()
  if (to.meta.public) {
    return authed ? { name: 'list' } : true
  }
  if (!authed) return { name: 'login', query: to.fullPath === '/' ? {} : { redirect: to.fullPath } }
  if (to.meta.admin && !isAdmin()) {
    toast('warn', '仅管理员可访问管理后台')
    return { name: 'list' }
  }
  return true
})

export default router
