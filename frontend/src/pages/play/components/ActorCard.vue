<script setup lang="ts">
// 演员卡（#08 Q5）：常驻紧凑（名/kind/关键属性/资源条/statuses chips），点开全表。
// 受控角色高亮并标注「你」——迁移：<Badge> 或 ring。
// 资源条自画 2px 细条（不引 Progress 组件）。
import { ref, computed } from 'vue'
import type { CharacterInstance, ResourceDef } from '@/types'
import { usePlayStore } from '../stores/play'
import { kindLabel, nameTintClass, initial, portraitOf } from '../utils'
import { assetUrl } from '@/api'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconChevronDown, IconChevronUp, IconMapPin, IconUserCheck } from '@tabler/icons-vue'

const props = defineProps<{
  actor: CharacterInstance
  controlled: boolean
  resourceDefs: ResourceDef[]
}>()

const emit = defineEmits<{ (e: 'switch', characterId: string): void }>()
const expanded = ref(false)
const tint = computed(() => nameTintClass(props.actor.name))
const store = usePlayStore()
const portrait = computed(() => portraitOf(store.detail?.storybook, props.actor.template_id, props.actor.name))
const keyAttrs = computed(() => {
  const dims = ['str', 'agi', 'wit', 'cha', 'trait', 'note']
  return Object.entries(props.actor.attributes).filter(([k]) => dims.includes(k))
})
const fullAttrs = computed(() => Object.entries(props.actor.attributes))
const locationName = computed(() => props.actor.location_id ?? '')
const resourceName = (id: string) => props.resourceDefs.find(r => r.id === id)?.name ?? id
const resRows = computed(() =>
  Object.entries(props.actor.resources).map(([id, value]) => {
    const def = props.resourceDefs.find(r => r.id === id)
    return { id, name: def?.name ?? id, value, max: def?.type === 'numerical' ? def.default_max : undefined }
  })
)
const pct = (r: { value: number; max?: number }) => {
  if (r.max == null || r.max <= 0) return '0%'
  return Math.max(0, Math.min(100, (r.value / r.max) * 100)) + '%'
}
const actorKind = computed(() => kindLabel(props.actor.kind))

/** 资源条：按资源语义映射主题语义色（实色即可，条高仅 1.5px），与全局调色一致 */
function resBarClass(id: string) {
  const low = id.toLowerCase()
  if (low.includes('hp') || low.includes('health') || low.includes('life') || low.includes('血')) return 'bg-destructive'
  if (low.includes('mp') || low.includes('san') || low.includes('mana') || low.includes('蓝') || low.includes('理智')) return 'bg-info'
  if (low.includes('sta') || low.includes('energy') || low.includes('体')) return 'bg-success'
  return 'bg-primary'
}
</script>

<template>
  <div
    class="group relative rounded-xl border p-3 transition-all duration-200"
    :class="controlled
      ? 'border-primary/60 bg-gradient-to-b from-primary/10 via-primary/5 to-card/90 ring-1 ring-primary/30 shadow-xs'
      : 'border-border/80 bg-card/80 hover:border-border hover:bg-card/95 hover:shadow-2xs'"
  >
    <button
      class="flex w-full cursor-pointer items-center gap-2.5 border-0 bg-transparent p-0 text-left text-inherit"
      :aria-expanded="expanded"
      @click="expanded = !expanded"
    >
      <Avatar
        class="size-8.5 shrink-0 transition-transform group-hover:scale-105"
        :class="controlled ? 'ring-2 ring-primary/60 ring-offset-1 ring-offset-background' : ''"
      >
        <img
          v-if="portrait"
          :src="assetUrl(portrait.asset)"
          :alt="actor.name"
          class="size-full object-cover"
          loading="lazy"
          decoding="async"
        />
        <AvatarFallback v-else class="text-[13px] font-extrabold shadow-inner" :class="tint">
          {{ initial(actor.name) }}
        </AvatarFallback>
      </Avatar>

      <span class="min-w-0 flex-1">
        <span class="flex flex-wrap items-center gap-1.5">
          <span class="truncate text-[13.5px] font-extrabold">{{ actor.name }}</span>
          <Badge v-if="controlled" variant="default" class="h-4 px-1.5 text-[9.5px] font-extrabold bg-primary text-primary-foreground shadow-2xs">你</Badge>
          <span class="font-mono text-[9.5px] font-semibold text-muted-foreground/70 uppercase">{{ actorKind }}</span>
        </span>
        <span class="mt-0.5 flex flex-wrap items-center gap-1">
          <template v-if="actor.statuses.length">
            <span
              v-for="s in actor.statuses"
              :key="s.id"
              class="rounded-full border border-warning/40 bg-warning/15 px-1.5 py-px text-[10px] font-medium text-warning"
            >
              {{ s.name }}
            </span>
          </template>
          <span v-else class="text-[10.5px] text-muted-foreground/60">状态如常</span>
        </span>
      </span>

      <div class="flex size-5 items-center justify-center rounded-md text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground">
        <IconChevronUp v-if="expanded" class="size-3.5" />
        <IconChevronDown v-else class="size-3.5" />
      </div>
    </button>

    <Button
      v-if="!controlled && actor.kind === 'pc'"
      size="xs"
      variant="outline"
      class="mt-2 w-full justify-center border-dashed border-primary/40 text-primary hover:bg-primary/10 hover:text-primary transition-colors"
      @click="emit('switch', actor.template_id)"
    >
      <IconUserCheck class="size-3 mr-1" />
      <span>切换为受控角色</span>
    </Button>

    <!-- 关键属性 -->
    <div v-if="keyAttrs.length" class="mt-2.5 flex flex-wrap gap-1">
      <span
        v-for="[k, v] in keyAttrs"
        :key="k"
        class="font-mono rounded-md border border-border/70 bg-muted/60 px-1.5 py-0.5 text-[10.5px] text-muted-foreground"
        :title="k"
      >
        <span>{{ k }}</span> <b class="font-bold text-foreground">{{ v }}</b>
      </span>
    </div>

    <!-- 资源槽位进度条 -->
    <div v-if="resRows.length" class="mt-2.5 flex flex-col gap-1.5">
      <div v-for="r in resRows" :key="r.id" class="space-y-0.5">
        <div class="flex justify-between text-[10.5px] text-muted-foreground">
          <span>{{ r.name }}</span>
          <span class="font-mono text-foreground/90 font-medium">
            {{ r.value }}<template v-if="r.max != null"> <span class="text-muted-foreground/60">/ {{ r.max }}</span></template>
          </span>
        </div>
        <div class="h-1.5 w-full overflow-hidden rounded-full border border-border/40 bg-muted/70">
          <i
            class="block h-full rounded-full transition-all duration-300"
            :class="resBarClass(r.id)"
            :style="{ width: pct(r) }"
          ></i>
        </div>
      </div>
    </div>

    <!-- 展开详情面板 -->
    <div v-if="expanded" class="mt-2.5 border-t border-dashed border-border/80 pt-2.5 space-y-2.5">
      <div>
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">属性全表</div>
        <div v-if="fullAttrs.length" class="mt-1.5 grid grid-cols-2 gap-1.5">
          <div
            v-for="[k, v] in fullAttrs"
            :key="k"
            class="flex items-center justify-between rounded-md border border-border/60 bg-muted/40 px-2 py-1 text-[11px]"
          >
            <span class="text-muted-foreground font-mono">{{ k }}</span>
            <span class="font-bold text-foreground">{{ v }}</span>
          </div>
        </div>
        <div v-else class="mt-1 text-[11px] text-muted-foreground/60">无属性数据</div>
      </div>

      <div>
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">资源概览</div>
        <div v-if="Object.keys(actor.resources).length" class="mt-1.5 flex flex-wrap gap-1.5">
          <span
            v-for="[id, v] in Object.entries(actor.resources)"
            :key="id"
            class="rounded-md border border-border/60 bg-muted/40 px-2 py-0.5 text-[11px]"
          >
            <span class="text-muted-foreground">{{ resourceName(id) }}</span>
            <b class="font-mono ml-1.5 text-foreground">{{ v }}</b>
          </span>
        </div>
        <div v-else class="mt-1 text-[11px] text-muted-foreground/60">无资源</div>
      </div>

      <div>
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">当前状态</div>
        <div v-if="actor.statuses.length" class="mt-1.5 flex flex-wrap gap-1.5">
          <span
            v-for="s in actor.statuses"
            :key="s.id"
            class="rounded-full border border-warning/40 bg-warning/10 px-2 py-0.5 text-[10.5px] text-warning font-medium"
          >
            {{ s.name }}
            <template v-if="s.turns_left != null">（{{ s.turns_left }} 回合）</template>
            <template v-else-if="s.scenes_left != null">（{{ s.scenes_left }} 场景）</template>
          </span>
        </div>
        <div v-else class="mt-1 text-[11px] text-muted-foreground/60">无状态影响</div>
      </div>

      <div v-if="actor.location_id">
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">所处位置</div>
        <div class="mt-1 flex items-center gap-1 text-[11.5px] text-muted-foreground">
          <IconMapPin class="size-3 text-primary" />
          <span>{{ locationName }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
