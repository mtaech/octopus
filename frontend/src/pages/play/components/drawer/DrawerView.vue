<script setup lang="ts">
// 存档抽屉主视图：存读档菜单（#21 ① B 形态）—— 存档卡片列 + 点开内联展开
// 展开面板内分节：手动存档 / 升级入口（needs_upgrade）/ 导出下载 / 维护历史 / 新原点说明
import { computed } from 'vue'
import { usePlayStore } from '../../stores/play'
import { useDrawerStore } from '../../stores/drawer'
import SaveMain from './SaveMain.vue'
import WizardView from './WizardView.vue'

const store = usePlayStore()
const drawer = useDrawerStore()

const playSave = computed(() => {
  const p = store.projection
  if (!p) return null
  return {
    id: p.meta.save_id, title: p.meta.save_title, storybookTitle: p.meta.storybook_title,
    revision: p.meta.revision, needsUpgrade: p.meta.needs_upgrade,
    latestRevision: store.detail?.latest_revision ?? p.meta.revision
  }
})
</script>

<template>
  <!-- 升级向导为聚焦视图：进入后隐藏存读档主视图（面包屑可返回） -->
  <WizardView v-if="drawer.wizardOpen" />
  <SaveMain v-else :play-save="playSave" />
</template>
