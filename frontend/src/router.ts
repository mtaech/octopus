import { createRouter, createWebHistory } from 'vue-router'

// 三路由（蓝图 #18 ④）：/ 列表 · /storybook/:id/edit 编辑器（id=new 表示新建草稿）· /play/:saveId 游玩页
const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'list', component: () => import('@/pages/list/ListPage.vue') },
    { path: '/storybook/:id/edit', name: 'editor', component: () => import('@/pages/editor/EditorPage.vue') },
    { path: '/play/:saveId', name: 'play', component: () => import('@/pages/play/PlayPage.vue') },
    { path: '/:pathMatch(.*)*', redirect: '/' }
  ]
})

export default router
