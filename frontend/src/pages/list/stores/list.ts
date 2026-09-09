// 列表页数据 store（feature 一文件，CONTRACT.md 允许自建）
// 职责：拉取已发布故事书 / 存档列表，并暴露创建与导入动作；
// 页面只消费这里的派生数据，保持组件薄。
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import {
  listStorybooks, listSaves, createSave, importSave
} from '@/api'
import type { SaveListItem, StorybookListItem } from '@/types'

export const useListStore = defineStore('list', () => {
  const storybooks = ref<StorybookListItem[]>([])
  const saves = ref<SaveListItem[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const loaded = ref(false)

  // 顶部动作带「继续游玩」：直达最近存档 = listSaves()[0]（后端已按 last_played_at 降序）
  const recentSave = computed<SaveListItem | null>(() => saves.value[0] ?? null)

  /** 拉取两类数据（已发布故事书 + 全部存档）；失败时以 error 呈现并保留空态引导 */
  async function load() {
    loading.value = true
    error.value = null
    try {
      // 本页只列已发布故事书（规格点 5）
      const [sbList, saveList] = await Promise.all([listStorybooks(true), listSaves()])
      storybooks.value = sbList
      saves.value = saveList
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      loading.value = false
      loaded.value = true
    }
  }

  /** 开档（新建游戏）：确认弹窗 → createSave → 返回新存档 id */
  async function startNewGame(storybookId: string, title: string, controlledCharacterId?: string): Promise<string> {
    const detail = await createSave(storybookId, title, controlledCharacterId)
    // 新档已产生，本地即时刷新（last_played_at 最新 → 排到最近存档）
    await load()
    return detail.id
  }

  /** 导入存档：传文件名即可，mock 返回带 imported 标记的存档 */
  async function importFromFile(fileName: string): Promise<SaveListItem> {
    const item = await importSave(fileName)
    await load()
    return item
  }

  return { storybooks, saves, loading, loaded, error, recentSave, load, startNewGame, importFromFile }
})