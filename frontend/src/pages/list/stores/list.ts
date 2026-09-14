// 列表页数据 store（feature 一文件，CONTRACT.md 允许自建）
// 职责：拉取已发布故事书 / 存档列表，并暴露创建与导入动作；
// 页面只消费这里的派生数据，保持组件薄。
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import {
  listStorybooks, listSaves, createSave, importSave, exportStorybook, importStorybook,
  deleteStorybook, deleteSave
} from '@/api'
import type { SaveListItem, StorybookDocument, StorybookListItem, ValidationIssue } from '@/types'

export const useListStore = defineStore('list', () => {
  /** 我的故事书（含草稿）——书架只显示自己创作的。 */
  const storybooks = ref<StorybookListItem[]>([])
  /** 可开档库：**所有人**的已发布故事书（多账户下这是共享的那一份）。 */
  const playable = ref<StorybookListItem[]>([])
  const saves = ref<SaveListItem[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const loaded = ref(false)

  // 顶部动作带「继续游玩」：直达最近存档 = listSaves()[0]（后端已按 last_played_at 降序）
  const recentSave = computed<SaveListItem | null>(() => saves.value[0] ?? null)

  /**
   * 可开档子集：只有已发布故事书能开档（未发布会被后端 409）。
   * 来源是「所有人的已发布」（我的 + 别人的），这样玩别人的书、别人玩我的书都成立。
   */
  const publishedStorybooks = computed<StorybookListItem[]>(() => playable.value.filter(s => s.published))

  /**
   * 存档封面按 storybook_id 查：我可能玩的是别人的已发布书，
   * 所以必须合并「我的书 + 可开档库」，只看书架会查不到封面。
   */
  const knownStorybooks = computed<Map<string, StorybookListItem>>(() => {
    const map = new Map<string, StorybookListItem>()
    for (const b of playable.value) map.set(b.id, b)
    for (const b of storybooks.value) map.set(b.id, b)
    return map
  })

  /** 拉取三类数据（我的书 + 可开档库 + 我的存档）；失败时以 error 呈现并保留空态引导 */
  async function load() {
    loading.value = true
    error.value = null
    try {
      // 三类数据各自有归属语义：
      //   released_only=false → 我的（草稿必须能在首页找到，否则「我的故事书」名不副实）；
      //   released_only=true  → 所有人的已发布（开档弹窗的候选）；
      //   saves               → 我的存档（后端已按账户过滤）。
      const [sbList, playableList, saveList] = await Promise.all([
        listStorybooks(false),
        listStorybooks(true),
        listSaves(),
      ])
      storybooks.value = sbList
      playable.value = playableList
      saves.value = saveList
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      loading.value = false
      loaded.value = true
    }
  }

  /** 删除故事书：级联清掉它的草稿与结对会话；基于它的存档已内嵌冻结副本，不受影响。 */
  async function removeStorybook(id: string): Promise<void> {
    await deleteStorybook(id)
    await load()
  }

  /** 删除存档：删命令日志 / 快照并刷新列表；后端会同时丢弃内存会话。 */
  async function removeSave(id: string): Promise<void> {
    await deleteSave(id)
    await load()
  }

  /**
   * 批量删除存档：后端只有单档端点，这里逐个删；全部尝试完再统一刷新一次。
   * 有失败则抛错（已成功的部分已删除并已刷新）。
   */
  async function removeSaves(ids: string[]): Promise<void> {
    const failed: string[] = []
    for (const id of ids) {
      try {
        await deleteSave(id)
      } catch {
        failed.push(id)
      }
    }
    await load()
    if (failed.length) throw new Error(`有 ${failed.length} 个存档删除失败，请刷新后重试`)
  }

  /** 开档（新建游戏）：确认弹窗 → createSave → 返回新存档 id */
  async function startNewGame(storybookId: string, title: string, controlledCharacterId: string | undefined): Promise<string> {
    const detail = await createSave(storybookId, title, controlledCharacterId)
    // 新档已产生，本地即时刷新（last_played_at 最新 → 排到最近存档）
    await load()
    return detail.id
  }

  /** 导入存档：传存档包文件本身（当前格式为 zip：save.json + assets/） */
  async function importFromFile(file: Blob): Promise<SaveListItem> {
    const item = await importSave(file)
    await load()
    return item
  }

  /** 导入故事书：传故事书包文件本身（zip：storybook.json + assets/），随后刷新书架 */
  async function importBookFromFile(
    file: Blob
  ): Promise<{ doc: StorybookDocument; issues: ValidationIssue[] }> {
    const res = await importStorybook(file)
    await load()
    return res
  }

  /** 导出故事书：返回文件名与字节，由页面触发下载（下载是纯 UI 动作，不进 store） */
  async function exportBook(id: string): Promise<{ filename: string; blob: Blob }> {
    return exportStorybook(id)
  }

  return { storybooks, playable, publishedStorybooks, knownStorybooks, saves, loading, loaded, error, recentSave, load, startNewGame, importFromFile, importBookFromFile, exportBook, removeStorybook, removeSave, removeSaves }
})