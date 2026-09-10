<script setup lang="ts">
// A 表单工作台 —— 实体 tab 容器
// #22 模块树：A = 世界/骨架/人物/技能/物品/物件/势力/关系/声明/维度设置 一级 tab
// 骨架 tab = SkeletonView；其余 = 各面板组件
import { ref, type Component } from 'vue'
import { useEditorStore } from '../../stores/editor'
import WorldPanel from './WorldPanel.vue'
import DimensionsPanel from './DimensionsPanel.vue'
import CharacterPanel from './CharacterPanel.vue'
import SkillsItemsPanel from './SkillsItemsPanel.vue'
import ObjectsPanel from './ObjectsPanel.vue'
import FactionsPanel from './FactionsPanel.vue'
import DeclarationsPanel from './DeclarationsPanel.vue'
import RelationshipsPanel from './RelationshipsPanel.vue'
import SkeletonView from '../skeleton/SkeletonView.vue'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { IconWorld, IconListTree, IconUser, IconSparkles, IconPackage, IconBox, IconBuilding, IconLink, IconFlag, IconSettings } from '@tabler/icons-vue'

const editor = useEditorStore()

const A_TABS: { key: string; label: string; icon: Component; comp: string }[] = [
  { key: 'world', label: '世界', icon: IconWorld, comp: 'world' },
  { key: 'skeleton', label: '骨架', icon: IconListTree, comp: 'skeleton' },
  { key: 'characters', label: '人物', icon: IconUser, comp: 'characters' },
  { key: 'skills', label: '技能', icon: IconSparkles, comp: 'skills' },
  { key: 'items', label: '物品', icon: IconPackage, comp: 'items' },
  { key: 'objects', label: '物件', icon: IconBox, comp: 'objects' },
  { key: 'factions', label: '势力', icon: IconBuilding, comp: 'factions' },
  { key: 'relationships', label: '关系', icon: IconLink, comp: 'relationships' },
  { key: 'declarations', label: '声明', icon: IconFlag, comp: 'declarations' },
  { key: 'dimensions', label: '维度设置', icon: IconSettings, comp: 'dimensions' }
]

const active = ref('world')

const countOf = (key: string): number => {
  const d = editor.draft
  if (!d) return 0
  switch (key) {
    case 'world': return d.world.locations.length + d.world.resources.length
    case 'skeleton': return d.skeleton.length
    case 'characters': return d.characters.length
    case 'skills': return d.skills.length
    case 'items': return d.items.length
    case 'objects': return d.objects.length
    case 'factions': return d.factions.length
    case 'relationships': return d.relationships.length
    case 'declarations': return d.flags.length + d.events.length + d.relationship_types.length + d.target_types.length
    case 'dimensions': return d.attribute_dimensions.length
    default: return 0
  }
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <Tabs v-model="active" class="flex min-h-0 flex-1 flex-col">
      <TabsList variant="line" class="h-11 w-full shrink-0 justify-start overflow-x-auto rounded-none border-b border-border/80 bg-card/40 px-3 py-0 backdrop-blur-xs">
        <TabsTrigger
          v-for="t in A_TABS"
          :key="t.key"
          :value="t.key"
          class="group h-11 flex-none gap-2 rounded-none px-3.5 text-[13px] font-medium transition-colors"
        >
          <component :is="t.icon" class="size-4 shrink-0 transition-transform group-hover:scale-110" />
          <span>{{ t.label }}</span>
          <span class="rounded-full bg-muted/60 px-1.5 py-0.5 text-[10.5px] font-mono font-normal text-muted-foreground/80 transition-colors group-data-[state=active]:bg-primary/20 group-data-[state=active]:text-primary">
            {{ countOf(t.key) }}
          </span>
        </TabsTrigger>
      </TabsList>

      <TabsContent value="world" class="min-h-0 flex-1 overflow-y-auto"><WorldPanel /></TabsContent>
      <TabsContent value="skeleton" class="min-h-0 flex-1 overflow-y-auto"><SkeletonView /></TabsContent>
      <TabsContent value="characters" class="min-h-0 flex-1 overflow-y-auto"><CharacterPanel /></TabsContent>
      <TabsContent value="skills" class="min-h-0 flex-1 overflow-y-auto"><SkillsItemsPanel kind="skill" /></TabsContent>
      <TabsContent value="items" class="min-h-0 flex-1 overflow-y-auto"><SkillsItemsPanel kind="item" /></TabsContent>
      <TabsContent value="objects" class="min-h-0 flex-1 overflow-y-auto"><ObjectsPanel /></TabsContent>
      <TabsContent value="factions" class="min-h-0 flex-1 overflow-y-auto"><FactionsPanel /></TabsContent>
      <TabsContent value="relationships" class="min-h-0 flex-1 overflow-y-auto"><RelationshipsPanel /></TabsContent>
      <TabsContent value="declarations" class="min-h-0 flex-1 overflow-y-auto"><DeclarationsPanel /></TabsContent>
      <TabsContent value="dimensions" class="min-h-0 flex-1 overflow-y-auto"><DimensionsPanel /></TabsContent>
    </Tabs>
  </div>
</template>
