<script setup lang="ts">
// 角色卡（游玩页详情栏，只读）：身份 / 属性 / 分区内容 / 技能 / 物品。
// 数据源：存档内嵌的冻结故事书（模板侧）+ 投影里的角色实例（随游戏进程变化）。
// 分区结构优先用故事书声明的 sheet（#6）；未声明时回落内置顺序。
import { computed } from 'vue'
import { usePlayStore } from '../stores/play'
import { assetUrl, toast } from '@/api'
import { initial, nameTintClass, portraitOf } from '../utils'
import { computeDerived } from '@/lib/derived'
import type {
  AttributeDimension, CharacterDef, CharacterInstance, Definition, FieldDef, ItemDef,
  ResourceDef, SheetSection, SkillDef, Storybook,
} from '@/types'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconUserCheck, IconSwords, IconPackage, IconHeartbeat } from '@tabler/icons-vue'

const props = defineProps<{ actor: CharacterInstance; controlled: boolean }>()
const emit = defineEmits<{ (e: 'switch', characterId: string): void }>()

const store = usePlayStore()
const sb = computed<Storybook | null>(() => (store.detail?.storybook as Storybook | undefined) ?? null)
const dims = computed<AttributeDimension[]>(() => sb.value?.attribute_dimensions ?? [])
const resourceDefs = computed<ResourceDef[]>(() => (sb.value?.world as { resources?: ResourceDef[] } | undefined)?.resources ?? [])
const definitions = computed<Definition[]>(() => sb.value?.definitions ?? [])
const kinds = computed(() => sb.value?.kinds ?? [])
const items = computed<ItemDef[]>(() => sb.value?.items ?? [])
const skills = computed<SkillDef[]>(() => sb.value?.skills ?? [])
const sheet = computed<SheetSection[]>(() => sb.value?.sheet ?? [])

/** 模板只用于出身信息（立绘 / 挂接 / 掌握技能） */
const template = computed<CharacterDef | null>(() =>
  sb.value?.characters.find(c => c.id === props.actor.template_id) ?? null,
)
const portrait = computed(() => portraitOf(sb.value, props.actor.template_id, props.actor.name))
const tint = computed(() => nameTintClass(props.actor.name))

// ---- 资源：实例当前值 + 故事书声明的上限 ----
const resources = computed(() =>
  Object.entries(props.actor.resources).map(([id, v]) => resourceRow(id, Number(v))).filter((r): r is ResourceRow => !!r),
)
interface ResourceRow { id: string; name: string; value: number; max?: number }
function resourceRow(id: string, value: number): ResourceRow | null {
  if (!(id in props.actor.resources) && value === 0) return null
  const def = resourceDefs.value.find(r => r.id === id)
  return { id, name: def?.name ?? id, value, max: def?.type === 'numerical' ? def.default_max : undefined }
}
function pct(value: number, max?: number): string {
  if (max == null || max <= 0) return '0%'
  return Math.max(0, Math.min(100, (value / max) * 100)) + '%'
}

// ---- 派生值：与编辑器同口径（有效属性 + 修正来源） ----
const derivedValues = computed(() => {
  if (!sb.value || !template.value) return []
  const pseudo: CharacterDef = { ...template.value, attributes: props.actor.attributes as Record<string, number> }
  return computeDerived(sb.value, pseudo).derived
})
function derivedOf(key: string) {
  return derivedValues.value.find(d => d.key === key)
}
function derivedText(key: string): string {
  const d = derivedOf(key)
  if (!d || d.value == null) return '—'
  return (d.signed && d.value >= 0 ? '+' : '') + d.value
}

// ---- 挂接内容（种族 / 职业 / 背景 / 特性 / 语言…） ----
interface KindGroup { kindKey: string; label: string; defs: Definition[]; fieldless: boolean }
function kindFields(kindKey: string): FieldDef[] {
  return kinds.value.find(k => k.key === kindKey)?.fields ?? []
}
function fieldValueText(kindKey: string, fieldKey: string, value: unknown): string {
  if (value == null || value === '') return '—'
  const f = kindFields(kindKey).find(x => x.key === fieldKey)
  const one = (v: unknown): string => {
    if (v == null || v === '') return ''
    if (f?.type === 'ref' || f?.type === 'ref_list') return definitions.value.find(d => d.id === v)?.name ?? String(v)
    if (typeof v === 'boolean') return v ? '是' : '否'
    return String(v)
  }
  if (Array.isArray(value)) {
    const parts = value.map(one).filter(Boolean)
    return parts.length ? parts.join('、') : '—'
  }
  return one(value) || '—'
}
function visibleFields(kindKey: string, def: Definition): FieldDef[] {
  return kindFields(kindKey).filter(f => fieldValueText(kindKey, f.key, def.fields?.[f.key]) !== '—')
}
function buildGroup(kindKey: string): KindGroup {
  const att: Record<string, string[]> = template.value?.attachments ?? {}
  return {
    kindKey,
    label: kinds.value.find(k => k.key === kindKey)?.label ?? kindKey,
    defs: (att[kindKey] ?? []).map(id => definitions.value.find(d => d.id === id)).filter((d): d is Definition => !!d),
    fieldless: kindFields(kindKey).length === 0,
  }
}
/** 内置顺序：全部挂接 */
function attachedGroups(): KindGroup[] {
  const att: Record<string, string[]> = template.value?.attachments ?? {}
  return Object.keys(att).map(buildGroup).filter(g => g.defs.length)
}
/** 声明分区：只列该区声明的种类（没声明就不显示挂接） */
function groupsOf(kindKeys?: string[]): KindGroup[] {
  return (kindKeys ?? []).map(buildGroup).filter(g => g.defs.length)
}
function narrativeText(key: string): string {
  const t = template.value
  if (!t) return ''
  if (key === 'background') return t.background ?? ''
  if (key === 'personality') return t.personality ?? ''
  if (key === 'appearance') return t.appearance ?? ''
  return ''
}
function narrativeLabel(key: string): string {
  return key === 'background' ? '背景' : key === 'personality' ? '性格' : key === 'appearance' ? '外观' : key
}

// ---- 物品 / 技能 ----
const inventory = computed(() =>
  Object.entries(props.actor.inventory ?? {}).map(([id, qty]) => {
    const def = items.value.find(i => i.id === id)
    return { id, qty: Number(qty), name: def?.name ?? id, slot: def?.slot }
  }),
)
const knownSkills = computed(() =>
  (template.value?.skills ?? []).map(id => ({ id, name: skills.value.find(s => s.id === id)?.name ?? id })),
)

const secHead = 'mb-1.5 text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase'
async function copyId() {
  try { await navigator.clipboard.writeText(props.actor.instance_id); toast('ok', '已复制实例 id') } catch { /* noop */ }
}
</script>

<template>
  <div class="space-y-3.5">
    <!-- 身份 -->
    <div class="flex items-start gap-3">
      <Avatar class="size-12 shrink-0">
        <img v-if="portrait" :src="assetUrl(portrait.asset)" :alt="actor.name" class="size-full object-cover" loading="lazy" decoding="async" />
        <AvatarFallback v-else class="text-[16px] font-extrabold" :class="tint">{{ initial(actor.name) }}</AvatarFallback>
      </Avatar>
      <div class="min-w-0 flex-1">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="truncate text-[15px] leading-tight font-extrabold text-foreground">{{ actor.name }}</span>
          <Badge v-if="controlled" class="h-4 bg-primary px-1.5 text-[9.5px] font-extrabold text-primary-foreground">你</Badge>
          <span class="font-mono text-[9.5px] font-semibold text-muted-foreground/70 uppercase">{{ actor.kind === 'pc' ? 'PC' : 'NPC' }}</span>
        </div>
        <div class="mt-1 flex flex-wrap gap-1">
          <span v-for="s in actor.statuses" :key="s.id" class="rounded-full border border-warning/40 bg-warning/15 px-1.5 py-px text-[10px] text-warning">
            {{ s.name }}<span v-if="s.turns_left != null" class="opacity-70"> · {{ s.turns_left }} 回合</span><span v-else-if="s.scenes_left != null" class="opacity-70"> · {{ s.scenes_left }} 场景</span>
          </span>
          <span v-if="!actor.statuses.length" class="text-[10.5px] text-muted-foreground/60">状态如常</span>
        </div>
        <button type="button" class="mt-1 font-mono text-[10px] text-muted-foreground/45 transition-colors hover:text-muted-foreground" title="点击复制实例 id" @click="copyId">
          {{ actor.instance_id }}
        </button>
      </div>
    </div>

    <Button
      v-if="!controlled && actor.kind === 'pc'"
      size="sm"
      variant="outline"
      class="w-full justify-center border-dashed border-primary/40 text-primary hover:bg-primary/10"
      @click="emit('switch', actor.template_id)"
    >
      <IconUserCheck class="mr-1 size-3.5" />
      切换为受控角色
    </Button>

    <!-- 属性（分区声明里没有属性，始终显示） -->
    <section v-if="dims.length">
      <h4 :class="secHead">属性</h4>
      <dl class="grid grid-cols-[auto_1fr] items-baseline gap-x-3 gap-y-0.5">
        <template v-for="d in dims" :key="d.key">
          <dt class="text-[11.5px] text-muted-foreground/70">{{ d.label }}</dt>
          <dd class="font-mono text-[12px] font-bold text-foreground">{{ actor.attributes[d.key] ?? '—' }}</dd>
        </template>
      </dl>
    </section>

    <!-- 故事书声明的卡面分区（#6） -->
    <template v-if="sheet.length">
      <section v-for="(sec, si) in sheet" :key="si">
        <h4 :class="secHead">{{ sec.title }}</h4>
        <div class="space-y-2">
          <!-- 挂接内容 -->
          <div v-for="g in groupsOf(sec.kinds)" :key="g.kindKey" class="space-y-1">
            <div class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">{{ g.label }}</div>
            <p v-if="g.fieldless" class="text-[12px] text-foreground/85">{{ g.defs.map(d => d.name).join('、') }}</p>
            <div v-else class="space-y-2">
              <div v-for="def in g.defs" :key="def.id" class="space-y-0.5 border-l-2 border-border/70 pl-2.5">
                <div class="text-[12px] font-semibold text-foreground">{{ def.name }}</div>
                <dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-0.5">
                  <template v-for="f in visibleFields(g.kindKey, def)" :key="f.key">
                    <dt class="text-[10.5px] whitespace-nowrap text-muted-foreground/60">{{ f.label || f.key }}</dt>
                    <dd class="min-w-0 text-[11.5px] break-words text-foreground/85">{{ fieldValueText(g.kindKey, f.key, def.fields?.[f.key]) }}</dd>
                  </template>
                </dl>
              </div>
            </div>
          </div>

          <!-- 派生值 -->
          <div v-if="(sec.derived ?? []).length" class="flex flex-wrap gap-1.5">
            <span v-for="k in sec.derived" :key="k" class="inline-flex items-baseline gap-1.5 rounded-full bg-muted/50 px-2.5 py-1 text-[11.5px]">
              <span class="text-muted-foreground/70">{{ derivedOf(k)?.label ?? k }}</span>
              <span class="font-mono font-bold text-primary">{{ derivedText(k) }}</span>
            </span>
          </div>

          <!-- 资源 -->
          <div v-if="(sec.resources ?? []).length" class="space-y-1.5">
            <div v-for="rid in sec.resources" :key="rid">
              <div class="flex items-baseline justify-between gap-2 text-[11.5px]">
                <span class="truncate text-foreground/85">{{ resourceRow(rid, Number(actor.resources[rid] ?? 0))?.name ?? rid }}</span>
                <span class="shrink-0 font-mono font-bold text-primary">
                  {{ Number(actor.resources[rid] ?? 0) }}<span v-if="resourceRow(rid, Number(actor.resources[rid] ?? 0))?.max != null" class="font-normal text-muted-foreground/50">/{{ resourceRow(rid, Number(actor.resources[rid] ?? 0))?.max }}</span>
                </span>
              </div>
              <div v-if="resourceRow(rid, Number(actor.resources[rid] ?? 0))?.max != null" class="mt-0.5 h-1 overflow-hidden rounded-full bg-muted">
                <div class="h-full rounded-full bg-primary transition-all" :style="{ width: pct(Number(actor.resources[rid] ?? 0), resourceRow(rid, Number(actor.resources[rid] ?? 0))?.max) }"></div>
              </div>
            </div>
          </div>

          <!-- 叙事字段 -->
          <div v-for="nk in sec.narrative ?? []" :key="nk" class="space-y-0.5">
            <div class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">{{ narrativeLabel(nk) }}</div>
            <p class="text-[11.5px] leading-relaxed whitespace-pre-wrap text-foreground/85">{{ narrativeText(nk) || '—' }}</p>
          </div>
        </div>
      </section>
    </template>

    <!-- 未声明分区：内置顺序（资源 + 全部挂接） -->
    <template v-else>
      <section v-if="resources.length">
        <h4 :class="secHead"><IconHeartbeat class="mr-1 inline size-3 align-[-2px]" />资源</h4>
        <div class="space-y-1.5">
          <div v-for="r in resources" :key="r.id">
            <div class="flex items-baseline justify-between gap-2 text-[11.5px]">
              <span class="truncate text-foreground/85">{{ r.name }}</span>
              <span class="shrink-0 font-mono font-bold text-primary">{{ r.value }}<span v-if="r.max != null" class="font-normal text-muted-foreground/50">/{{ r.max }}</span></span>
            </div>
            <div v-if="r.max != null" class="mt-0.5 h-1 overflow-hidden rounded-full bg-muted">
              <div class="h-full rounded-full bg-primary transition-all" :style="{ width: pct(r.value, r.max) }"></div>
            </div>
          </div>
        </div>
      </section>

      <section v-for="g in attachedGroups()" :key="g.kindKey">
        <h4 :class="secHead">{{ g.label }}</h4>
        <p v-if="g.fieldless" class="text-[12px] text-foreground/85">{{ g.defs.map(d => d.name).join('、') }}</p>
        <div v-else class="space-y-2">
          <div v-for="def in g.defs" :key="def.id" class="space-y-0.5 border-l-2 border-border/70 pl-2.5">
            <div class="text-[12px] font-semibold text-foreground">{{ def.name }}</div>
            <dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-0.5">
              <template v-for="f in visibleFields(g.kindKey, def)" :key="f.key">
                <dt class="text-[10.5px] whitespace-nowrap text-muted-foreground/60">{{ f.label || f.key }}</dt>
                <dd class="min-w-0 text-[11.5px] break-words text-foreground/85">{{ fieldValueText(g.kindKey, f.key, def.fields?.[f.key]) }}</dd>
              </template>
            </dl>
          </div>
        </div>
      </section>
    </template>

    <!-- 技能 -->
    <section v-if="knownSkills.length">
      <h4 :class="secHead"><IconSwords class="mr-1 inline size-3 align-[-2px]" />掌握技能 · {{ knownSkills.length }}</h4>
      <div class="flex flex-wrap gap-1">
        <span v-for="s in knownSkills" :key="s.id" class="rounded-full bg-muted/60 px-2 py-0.5 text-[11px] text-foreground/85">{{ s.name }}</span>
      </div>
    </section>

    <!-- 物品 -->
    <section v-if="inventory.length">
      <h4 :class="secHead"><IconPackage class="mr-1 inline size-3 align-[-2px]" />物品 · {{ inventory.length }}</h4>
      <div class="space-y-0.5">
        <div v-for="it in inventory" :key="it.id" class="flex items-baseline gap-2 text-[11.5px]">
          <span class="min-w-0 flex-1 truncate text-foreground/85">{{ it.name }}</span>
          <span v-if="it.slot" class="shrink-0 rounded bg-muted px-1 text-[10px] text-muted-foreground/70">{{ it.slot }}</span>
          <span class="shrink-0 font-mono text-muted-foreground/70">×{{ it.qty }}</span>
        </div>
      </div>
    </section>
  </div>
</template>
