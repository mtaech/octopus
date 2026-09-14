<script setup lang="ts">
// A 表单工作台 —— 两级导航：#22 模块树
// 一级 = 领域分组（设定 / 剧情 / 角色库 / 内容 / 关系 / 扩展）；
// 二级 = 组内分类。editor.activeTab 仍是「分类」级 key，
// 校验定位（navigateToIssue）与各面板的选中态寻址都不受影响。
import { computed, type Component } from 'vue'
import { useEditorStore } from '../../stores/editor'
import WorldPanel from './WorldPanel.vue'
import LorePanel from './LorePanel.vue'
import NarrativePanel from './NarrativePanel.vue'
import ProtocolPanel from './ProtocolPanel.vue'
import DimensionsPanel from './DimensionsPanel.vue'
import CharacterPanel from './CharacterPanel.vue'
import SkillsItemsPanel from './SkillsItemsPanel.vue'
import ObjectsPanel from './ObjectsPanel.vue'
import FactionsPanel from './FactionsPanel.vue'
import StatusesPanel from './StatusesPanel.vue'
import OpenKindsPanel from './OpenKindsPanel.vue'
import DeclarationsPanel from './DeclarationsPanel.vue'
import RelationshipsPanel from './RelationshipsPanel.vue'
import SkeletonView from '../skeleton/SkeletonView.vue'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { IconWorld, IconListTree, IconUser, IconSparkles, IconPackage, IconBox, IconBuilding, IconLink, IconFlag, IconFlame, IconSettings, IconLayoutGrid, IconBook, IconMessage2, IconCode } from '@tabler/icons-vue'

interface LeafTab { key: string; label: string; icon: Component; comp: string }
interface TabGroup { key: string; label: string; icon: Component; leaves: LeafTab[] }

// 一级分组：把 12 个分类收成 6 组，顶部不再随分类增多而变挤。
const GROUPS: TabGroup[] = [
  { key: 'setting', label: '设定', icon: IconWorld, leaves: [
    { key: 'world', label: '世界', icon: IconWorld, comp: 'world' },
    { key: 'lore', label: '词条', icon: IconBook, comp: 'lore' },
    { key: 'narrative', label: '叙事', icon: IconMessage2, comp: 'narrative' },
    { key: 'protocol', label: '协议', icon: IconCode, comp: 'protocol' },
    { key: 'dimensions', label: '维度', icon: IconSettings, comp: 'dimensions' },
    { key: 'declarations', label: '声明', icon: IconFlag, comp: 'declarations' }
  ] },
  { key: 'story', label: '剧情', icon: IconListTree, leaves: [
    { key: 'skeleton', label: '骨架', icon: IconListTree, comp: 'skeleton' }
  ] },
  { key: 'character', label: '角色库', icon: IconUser, leaves: [
    { key: 'characters', label: '角色库', icon: IconUser, comp: 'characters' }
  ] },
  { key: 'content', label: '内容', icon: IconSparkles, leaves: [
    { key: 'skills', label: '技能', icon: IconSparkles, comp: 'skills' },
    { key: 'items', label: '物品', icon: IconPackage, comp: 'items' },
    { key: 'objects', label: '物件', icon: IconBox, comp: 'objects' },
    { key: 'statuses', label: '状态', icon: IconFlame, comp: 'statuses' }
  ] },
  { key: 'relation', label: '关系', icon: IconLink, leaves: [
    { key: 'factions', label: '势力', icon: IconBuilding, comp: 'factions' },
    { key: 'relationships', label: '关系', icon: IconLink, comp: 'relationships' }
  ] },
  { key: 'extension', label: '扩展', icon: IconLayoutGrid, leaves: [
    { key: 'open', label: '开放内容', icon: IconLayoutGrid, comp: 'open' }
  ] }
]
const ALL_LEAVES: LeafTab[] = GROUPS.flatMap(g => g.leaves)

const editor = useEditorStore()

const active = computed<string>({
  get: () => editor.activeTab,
  set: (v: string) => { editor.activeTab = v }
})

const activeGroup = computed<TabGroup>(() =>
  GROUPS.find(g => g.leaves.some(l => l.key === editor.activeTab)) ?? GROUPS[0]
)

/** 切换分组：若当前分类不在该组内，落到该组第一个分类 */
function selectGroup(g: TabGroup): void {
  if (!g.leaves.some(l => l.key === editor.activeTab)) editor.activeTab = g.leaves[0].key
}

const countOf = (key: string): number => {
  const d = editor.draft
  if (!d) return 0
  switch (key) {
    case 'world': return d.world.locations.length + d.world.resources.length + (d.world.maps?.length ?? 0)
    case 'lore': return (d.lore ?? []).length
    case 'narrative': return (d.narrative?.sections ?? []).length
    case 'protocol': {
      const p = d.narrative?.protocol
      return p && p.mode !== 'default' ? 1 : 0
    }
    case 'skeleton': return d.skeleton.length
    case 'characters': return d.characters.length
    case 'skills': return d.skills.length
    case 'items': return d.items.length
    case 'objects': return d.objects.length
    case 'factions': return d.factions.length
    case 'relationships': return d.relationships.length
    case 'statuses': return (d.statuses ?? []).length
    case 'declarations': return d.flags.length + d.events.length + d.relationship_types.length + d.target_types.length
    case 'dimensions': return d.attribute_dimensions.length
    case 'open': return (d.definitions ?? []).length
    default: return 0
  }
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <!-- 一级：领域分组 -->
    <div class="flex h-10 flex-none items-center gap-0.5 overflow-x-auto border-b border-border bg-card/40 px-3">
      <button
        v-for="g in GROUPS"
        :key="g.key"
        type="button"
        class="flex flex-none cursor-pointer items-center gap-1.5 rounded-md px-3 py-1.5 text-[12.5px] font-medium whitespace-nowrap transition-colors"
        :class="activeGroup.key === g.key ? 'bg-primary/12 text-primary' : 'text-muted-foreground hover:bg-muted/60 hover:text-foreground'"
        @click="selectGroup(g)"
      >
        <component :is="g.icon" class="size-4 shrink-0" />
        <span>{{ g.label }}</span>
      </button>
    </div>

    <Tabs v-model="active" class="flex min-h-0 flex-1 flex-col">
      <!-- 二级：组内分类（单分类的组不渲染此行） -->
      <TabsList
        v-if="activeGroup.leaves.length > 1"
        variant="line"
        class="h-10 w-full shrink-0 justify-start gap-0.5 overflow-x-auto rounded-none border-b border-border bg-card/20 px-3 py-0"
      >
        <TabsTrigger
          v-for="t in activeGroup.leaves"
          :key="t.key"
          :value="t.key"
          :title="t.label"
          class="group h-10 flex-none gap-1.5 rounded-none px-2.5 text-[12.5px] font-medium whitespace-nowrap transition-colors"
        >
          <component :is="t.icon" class="size-3.5 shrink-0 transition-transform group-hover:scale-110" />
          <span>{{ t.label }}</span>
          <span
            v-if="countOf(t.key) > 0"
            class="rounded-full bg-muted/60 px-1.5 py-0.5 text-[10px] font-mono font-normal text-muted-foreground/80 transition-colors group-data-[state=active]:bg-primary/20 group-data-[state=active]:text-primary"
          >
            {{ countOf(t.key) }}
          </span>
        </TabsTrigger>
      </TabsList>

      <TabsContent v-for="t in ALL_LEAVES" :key="t.key" :value="t.key" class="min-h-0 flex-1 overflow-y-auto">
        <WorldPanel v-if="t.comp === 'world'" />
        <LorePanel v-else-if="t.comp === 'lore'" />
        <NarrativePanel v-else-if="t.comp === 'narrative'" />
        <ProtocolPanel v-else-if="t.comp === 'protocol'" />
        <SkeletonView v-else-if="t.comp === 'skeleton'" />
        <CharacterPanel v-else-if="t.comp === 'characters'" />
        <SkillsItemsPanel v-else-if="t.comp === 'skills'" kind="skill" />
        <SkillsItemsPanel v-else-if="t.comp === 'items'" kind="item" />
        <ObjectsPanel v-else-if="t.comp === 'objects'" />
        <FactionsPanel v-else-if="t.comp === 'factions'" />
        <RelationshipsPanel v-else-if="t.comp === 'relationships'" />
        <StatusesPanel v-else-if="t.comp === 'statuses'" />
        <DeclarationsPanel v-else-if="t.comp === 'declarations'" />
        <DimensionsPanel v-else-if="t.comp === 'dimensions'" />
        <OpenKindsPanel v-else-if="t.comp === 'open'" />
      </TabsContent>
    </Tabs>
  </div>
</template>
