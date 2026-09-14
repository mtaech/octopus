import '@fontsource-variable/geist'
import '@fontsource-variable/noto-serif-sc'
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import { initTheme, initFont } from './theme'
import { setUnauthorizedHandler } from './lib/auth'
import './style.css'

initTheme()
initFont()

// 任何请求拿到 401（令牌过期 / 被吊销）都由这里收口：清登录态 + 回登录页。
setUnauthorizedHandler(() => {
  const current = router.currentRoute.value
  if (current.name === 'login' || current.name === 'register') return
  void router.replace({ name: 'login', query: { redirect: current.fullPath } })
})

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')
