<script setup lang="ts">
// 演员卡（#08 Q5）：常驻紧凑（名/kind/关键属性/资源条/statuses chips），点开全表。
// 受控角色高亮并标注「你」——迁移：<Badge> 或 ring。
// 资源条自画 2px 细条（不引 Progress 组件）。
import { ref, computed } from 'vue'
import type { CharacterInstance, ResourceDef } from '@/types'
import { nameTintClass, initial } from '../utils'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconChevronDown, IconChevronUp } from '@tabler/icons-vue'

const props = defineProps<{
  actor: CharacterInstance
  controlled: boolean
  resourceDefs: ResourceDef[]
}>()

const emit = defineEmits<{ (e: 'switch', characterId: string): void }>()
const expanded = ref(false)
const tint = computed(() => nameTintClass(props.actor.name))
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
const kindLabel = computed(() => (props.actor.kind === 'pc' ? 'PC' : 'NPC'))
</script>

<template>
  <div
    class="rounded-xl border bg-card px-3 py-2"
    :class="controlled ? 'border-primary/60 bg-primary/5 ring-1 ring-primary/25' : 'border-border'"
  >
    <button class="flex w-full cursor-pointer items-center gap-2.5 border-0 bg-transparent p-0 text-left text-inherit" :aria-expanded="expanded" @click="expanded = !expanded">
      <Avatar class="size-8 shrink-0">
        <AvatarFallback class="text-[13px] font-extrabold" :class="tint">{{ initial(actor.name) }}</AvatarFallback>
      </Avatar>
      <span class="min-w-0 flex-1">
        <span class="flex flex-wrap items-center gap-1.5">
          <span class="truncate text-[13.5px] font-extrabold">{{ actor.name }}</span>
          <Badge v-if="controlled" variant="default" class="text-[9px]">你</Badge>
          <span class="text-[10px] font-semibold text-muted-foreground">{{ kindLabel }}</span>
        </span>
        <span class="mt-0.5 flex flex-wrap items-center gap-1">
          <template v-if="actor.statuses.length">
            <span v-for="s in actor.statuses" :key="s.id" class="border-warning/40 bg-warning/10 text-warning rounded-full border px-1.5 text-[10px]">{{ s.name }}</span>
          </template>
          <span v-else class="text-[10.5px] text-muted-foreground/60">状态如常</span>
        </span>
      </span>
      <IconChevronUp v-if="expanded" class="size-3.5 text-muted-foreground/60" />
      <IconChevronDown v-else class="size-3.5 text-muted-foreground/60" />
    </button>

    <Button v-if="!controlled && actor.kind === 'pc'" size="xs" variant="ghost" class="mt-1.5 w-full justify-center" @click="emit('switch', actor.template_id)">切换为受控角色</Button>

    <div v-if="keyAttrs.length" class="mt-2 flex flex-wrap gap-1">
      <span v-for="[k, v] in keyAttrs" :key="k" class="font-mono rounded-md border border-border bg-muted px-1.5 py-px text-[10.5px] text-muted-foreground" :title="k">{{ k }} <b class="text-foreground font-bold">{{ v }}</b></span>
    </div>

    <div v-if="resRows.length" class="mt-2 flex flex-col gap-1.5">
      <div v-for="r in resRows" :key="r.id">
        <div class="flex justify-between text-[10.5px] text-muted-foreground">
          <span>{{ r.name }}</span><span class="font-mono text-foreground">{{ r.value }}<template v-if="r.max != null"> / {{ r.max }}</template></span>
        </div>
        <!-- 2px 资源条（自画） -->
        <div class="bg-muted mt-0.5 h-0.5 overflow-hidden rounded-full">
          <i class="bg-primary block h-full rounded-full" :style="{ width: pct(r) }"></i>
        </div>
      </div>
    </div>

    <div v-if="expanded" class="mt-2.5 border-t border-dashed border-border pt-2">
      <div>
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground">属性全表</div>
        <div v-if="fullAttrs.length" class="mt-1 grid grid-cols-2 gap-1">
          <div v-for="[k, v] in fullAttrs" :key="k" class="flex justify-between rounded-md border border-border bg-muted/50 px-1.5 py-0.5 text-[11px]">
            <span class="text-muted-foreground">{{ k }}</span><span class="font-bold">{{ v }}</span>
          </div>
        </div>
        <div v-else class="text-[11px] text-muted-foreground/60">无属性数据</div>
      </div>
      <div class="mt-2">
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground">资源</div>
        <div v-if="Object.keys(actor.resources).length" class="mt-1 flex flex-wrap gap-1">
          <span v-for="[id, v] in Object.entries(actor.resources)" :key="id" class="rounded-md border border-border bg-muted/50 px-1.5 py-0.5 text-[11px]"><span class="text-muted-foreground">{{ resourceName(id) }}</span><b class="font-mono ml-1">{{ v }}</b></span>
        </div>
        <div v-else class="text-[11px] text-muted-foreground/60">无资源</div>
      </div>
      <div class="mt-2">
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground">状态</div>
        <div v-if="actor.statuses.length" class="mt-1 flex flex-wrap gap-1">
          <span v-for="s in actor.statuses" :key="s.id" class="border-warning/40 bg-warning/10 text-warning rounded-full border px-1.5 py-px text-[10.5px]">{{ s.name }}<template v-if="s.turns_left != null">（{{ s.turns_left }} 回合）</template><template v-else-if="s.scenes_left != null">（{{ s.scenes_left }} 场景）</template></span>
        </div>
        <div v-else class="text-[11px] text-muted-foreground/60">无</div>
      </div>
      <div v-if="actor.location_id" class="mt-2">
        <div class="text-[9.5px] font-extrabold tracking-[1.5px] text-muted-foreground">位置</div>
        <div class="text-[11.5px] text-muted-foreground/70">{{ locationName }}</div>
      </div>
    </div>
  </div>
</template>
