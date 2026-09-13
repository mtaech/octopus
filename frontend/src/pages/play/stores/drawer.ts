// ============================================================
// 存档抽屉 store（#21：存读档菜单窄抽屉 + 升级独立向导步进）
// 职责：抽屉开关、当前存档卡展开、维护历史、升级 wizard 状态机
// (report → adjudicate → confirm → result)、导出下载。
// ============================================================
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import type { SaveDetail, SaveListItem, UpgradeReport, Disposition, MaintenanceRow } from '@/types'
import { listSaves, getSave, getMaintenance, manualSave, exportSave, renameSave, deleteSave, newOrigin, upgradeDryRun, upgradeExecute, toast } from '@/api'
import { usePlayStore } from './play'

export type WizardStep = 0 | 1 | 2 | 3
/** 0=未进入 1=迁移报告 2=确认执行 3=执行结果 */

export type HistoryRow = MaintenanceRow & { isNew?: boolean }

export interface GoneCharacterPick {
  character_id: string
  name: string
  reason?: string
  disposition: Disposition | null
}

export const useDrawerStore = defineStore('playDrawer', () => {
  const open = ref(false)
  const bannerDismissed = ref(false)
  const list = ref<SaveListItem[]>([])
  const expandedId = ref<string | null>(null)
  const currentDetail = ref<SaveDetail | null>(null)
  const history = ref<HistoryRow[]>([])
  const busySave = ref(false)

  // wizard 状态机
  const wizard = ref<{
    step: WizardStep
    report: UpgradeReport | null
    reportLoading: boolean
    picks: GoneCharacterPick[]
    executing: boolean
    backupName: string
    error: string
  }>({ step: 0, report: null, reportLoading: false, picks: [], executing: false, backupName: '', error: '' })

  const wizardOpen = computed(() => wizard.value.step > 0)
  const allAdjudicated = computed(() => {
    const w = wizard.value
    if (!w.report) return false
    return w.report.groups.gone_characters.length > 0
      ? w.report.groups.gone_characters.every(gc => w.picks.some(p => p.character_id === gc.character_id && p.disposition !== null))
      : true
  })

  function openDrawer() { open.value = true }
  function closeDrawer() {
    open.value = false
    resetWizard()
    expandedId.value = null
  }
  function dismissBanner() { bannerDismissed.value = true }

  async function loadList() {
    try { list.value = await listSaves() } catch { /* 列表失败静默 */ }
  }

  function toggleExpand(id: string) {
    expandedId.value = expandedId.value === id ? null : id
    if (expandedId.value) void loadDetail(id)
  }

  async function loadDetail(id: string) {
    const [detail, maint] = await Promise.all([getSave(id), getMaintenance(id)])
    currentDetail.value = detail
    // 维护历史改为从后端读取（#21/#24 修订）
    history.value = maint.map((m, i) => ({ ...m, isNew: i === 0 }))
  }

  /** 手动存档（存档卡展开内按钮 / 游玩页操作） */
  async function doManualSave() {
    const play = usePlayStore()
    if (busySave.value || !play.saveId) return
    busySave.value = true
    try {
      const item = await manualSave(play.saveId)
      toast('ok', '已手动存档：' + item.title)
      history.value = (await getMaintenance(play.saveId)).map((m, i) => ({ ...m, isNew: i === 0 }))
      // 同步回 play detail 的时间
      const d = play.detail
      if (d) { d.updated_at = item.updated_at }
    } catch (err) {
      toast('error', (err as Error)?.message ?? '存档失败')
    } finally {
      busySave.value = false
    }
  }

  /** 导出通用自包含存档包（.octopus.json）下载（#21 ④ / #27） */
  async function doExport() {
    const play = usePlayStore()
    if (!play.saveId) return
    try {
      const { filename, blob } = await exportSave(play.saveId)
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = filename
      document.body.appendChild(a)
      a.click()
      a.remove()
      setTimeout(() => URL.revokeObjectURL(url), 4000)
      toast('ok', '已导出 ' + filename)
    } catch (err) {
      toast('error', (err as Error)?.message ?? '导出失败')
    }
  }

  /** 重命名存档（#24 修订） */
  async function doRename(id: string, title: string): Promise<boolean> {
    try {
      const item = await renameSave(id, title)
      const play = usePlayStore()
      if (play.projection?.meta.save_id === id) play.projection.meta.save_title = item.title
      if (currentDetail.value?.id === id) currentDetail.value.title = item.title
      await loadList()
      toast('ok', '已重命名为「' + item.title + '」')
      return true
    } catch (err) { toast('error', (err as Error)?.message ?? '重命名失败'); return false }
  }

  /** 删除存档（#24 修订） */
  async function doDelete(id: string): Promise<boolean> {
    try {
      await deleteSave(id)
      await loadList()
      toast('ok', '已删除存档')
      return true
    } catch (err) { toast('error', (err as Error)?.message ?? '删除失败'); return false }
  }

  /** 压缩为新原点（#24 修订：玩家侧 v1；旧日志归档只读） */
  async function doNewOrigin() {
    const play = usePlayStore()
    if (!play.saveId) return
    try {
      const { archivedCount } = await newOrigin(play.saveId)
      history.value = (await getMaintenance(play.saveId)).map((m, i) => ({ ...m, isNew: i === 0 }))
      toast('ok', `已压缩为新原点，归档 ${archivedCount} 条历史`)
    } catch (err) { toast('error', (err as Error)?.message ?? '压缩失败') }
  }

  // ---------- 升级向导 ----------
  function enterUpgrade() {
    wizard.value = { step: 1, report: null, reportLoading: true, picks: [], executing: false, backupName: '', error: '' }
    void runDryRun()
  }
  async function runDryRun() {
    const play = usePlayStore()
    wizard.value.reportLoading = true
    wizard.value.error = ''
    try {
      const rep = await upgradeDryRun(play.saveId)
      wizard.value.report = rep
      wizard.value.picks = rep.groups.gone_characters.map(gc => ({
        character_id: gc.character_id, name: gc.name, reason: gc.reason, disposition: null
      }))
      if (rep.groups.gone_characters.length === 0) wizard.value.step = 2
    } catch (err) {
      wizard.value.error = (err as Error)?.message ?? '迁移报告加载失败'
      toast('error', wizard.value.error)
    } finally {
      wizard.value.reportLoading = false
    }
  }
  function pick(characterId: string, disposition: Disposition) {
    const p = wizard.value.picks.find(pk => pk.character_id === characterId)
    if (p) p.disposition = disposition
  }
  function goConfirm() { wizard.value.step = 2 }
  function backToReport() { wizard.value.step = 1 }
  function resetWizard() { wizard.value = { step: 0, report: null, reportLoading: false, picks: [], executing: false, backupName: '', error: '' } }

  /** 执行升级（#14/#21 step3：自动备份 → 执行 → 维护历史落账） */
  async function doExecute() {
    const play = usePlayStore()
    const w = wizard.value
    if (w.executing || !allAdjudicated.value) return
    w.executing = true
    w.error = ''
    try {
      const dispositions = w.picks.map(pk => ({ character_id: pk.character_id, disposition: pk.disposition as Disposition }))
      const { detail, backupName } = await upgradeExecute(play.saveId, dispositions)
      w.backupName = backupName
      w.step = 3
      play.applyUpgrade(detail.embedded_revision, false)
      play.detail = detail
      // 本地投影镜像：departure 角色标记离场（present=false），freeze 维持封存
      for (const d of dispositions) {
        if (d.disposition === 'departure') {
          const p = play.projection
          const inst = p?.characters[d.character_id]
          if (inst) {
            inst.present = false
            if (!inst.statuses.some(s => s.id === 'departed')) inst.statuses.push({ id: 'departed', name: '已离场' })
          }
        }
      }
      history.value = (await getMaintenance(play.saveId)).map((m, i) => ({ ...m, isNew: i === 0 }))
      toast('ok', '升级完成，已自动备份')
    } catch (err) {
      const code = (err as { code?: string })?.code
      w.error = code === 'missing_dispositions' || code === 'MISSING_DISPOSITIONS' ? '还有人物未裁决处置方式。' : ((err as Error)?.message ?? '升级执行失败')
      toast('error', w.error)
    } finally {
      w.executing = false
    }
  }

  return {
    open, bannerDismissed, list, expandedId, currentDetail, history, busySave, wizard, wizardOpen,
    allAdjudicated,
    openDrawer, closeDrawer, dismissBanner, loadList, toggleExpand, loadDetail,
    doManualSave, doExport, doRename, doDelete, doNewOrigin, enterUpgrade, runDryRun, pick, goConfirm, backToReport, resetWizard, doExecute
  }
})
