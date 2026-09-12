import '@fontsource-variable/geist'
import '@fontsource-variable/noto-serif-sc'
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import { initTheme, initFont } from './theme'
import './style.css'

initTheme()
initFont()

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')
