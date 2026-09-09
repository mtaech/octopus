// 应用设置 store（#26 AI Provider 配置）：provider 清单 + 三类模型分工 + 成本护栏
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getAppConfig, saveAppConfig, testProvider, toast } from '@/api'
import type { AppConfig, ProviderTestResult, ProviderConfig } from '@/types'

export const useSettingsStore = defineStore('settings', () => {
  const config = ref<AppConfig | null>(null)
  const loading = ref(false)
  const saving = ref(false)
  const tests = ref<Record<string, ProviderTestResult & { testing?: boolean }>>({})

  async function load() {
    loading.value = true
    try { config.value = await getAppConfig() }
    catch (e) { toast('error', '读取配置失败：' + ((e as Error)?.message ?? String(e))) }
    finally { loading.value = false }
  }

  async function save(): Promise<boolean> {
    if (!config.value) return false
    saving.value = true
    try {
      // 去掉 Vue 响应式代理再提交（structuredClone 对 proxy 会抛错）
      const plain = JSON.parse(JSON.stringify(config.value)) as AppConfig
      config.value = await saveAppConfig(plain)
      toast('ok', '配置已保存到本地配置文件（0600）')
      return true
    } catch (e) {
      toast('error', '保存失败：' + ((e as Error)?.message ?? String(e)))
      return false
    } finally { saving.value = false }
  }

  async function test(provider: ProviderConfig) {
    const id = provider.id
    tests.value[id] = { ...(tests.value[id] ?? { ok: false, message: '' }), testing: true }
    try { tests.value[id] = await testProvider(JSON.parse(JSON.stringify(provider)) as ProviderConfig) }
    catch (e) { tests.value[id] = { ok: false, message: (e as Error)?.message ?? '测试失败' } }
  }

  return { config, loading, saving, tests, load, save, test }
})
