<script setup lang="ts">
// CharacterPanel —— A「角色库」tab：玩家角色 / NPC / 怪物同住一处的模板面板。
// 布局：左侧名册（按 kind 筛选）+ 右侧角色卡（立绘 + 身份 + 属性网格 + 资源槽）。
// kind='monster' 时改为怪物数据卡：statblock 展示字段 + 攻击技能绑定（图鉴 M1 / 地图 P1）。
import { computed, ref, onUnmounted } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { AttributeDimension, CharacterDef, Definition, FieldDef, ItemDef, KindDef, MonsterStatblock, SheetSection, SkillDef, StatusDef, AssetRef } from '@/types'
import { uid } from '@/types'
import { entityKey, useWorkbenchSelection } from './selection'
import WorkbenchLayout, { type WorkbenchItem } from './WorkbenchLayout.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import LuaHookEditor from '@/components/LuaHookEditor.vue'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { assetUrl, toast, uploadAsset } from '@/api'
import { importStFile } from '@/lib/st-import'
import { computeDerived } from '@/lib/derived'
import FieldText from '../fields/FieldText.vue'
import FieldArea from '../fields/FieldArea.vue'
import FieldNum from '../fields/FieldNum.vue'
import FieldGrid from '../fields/FieldGrid.vue'
import { nameTintClass, initial } from '@/pages/play/utils'
import {
  IconCrown,
  IconUsers,
  IconSwords,
  IconSparkles,
  IconBook2,
  IconUpload,
  IconTrash,
  IconCopy,
  IconCoins,
  IconLoader2,
  IconArrowUpRight,
  IconPlus,
  IconX,
  IconPackage,
  IconDice,
  IconFlame,
  IconHourglass,
  IconTarget,
  IconMaximize,
  IconArrowsExchange,
  IconEye,
  IconShield,
  IconLayoutGrid,
  IconPencil,
  IconArrowsMaximize,
  IconMapPin,
  IconMessageChatbot,
  IconNotes,
  IconEyeOff,
  IconCircleCheck,
  IconAlertTriangle,
  IconFileImport,
} from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const selected = useWorkbenchSelection('characters')
function select(id: string): void { selected.value = id }

const chars = computed<CharacterDef[]>(() => d.value?.characters ?? [])

// ---------------- 角色库：kind 筛选（玩家角色 / NPC / 怪物） ----------------
type KindKey = 'pc' | 'npc' | 'monster'
const KIND_TABS: { key: 'all' | KindKey; label: string }[] = [
  { key: 'all', label: '全部' },
  { key: 'pc', label: '玩家角色' },
  { key: 'npc', label: 'NPC' },
  { key: 'monster', label: '怪物' },
]
/** 名册徽标文案 */
const KIND_BADGE: Record<KindKey, string> = { pc: '👑 PC', npc: 'NPC', monster: '☠ 怪物' }
const kindFilter = ref<'all' | KindKey>('all')
/** 归一 kind：字段缺失按 pc（历史行为，旧故事书不受影响） */
function kindOf(c: CharacterDef): KindKey {
  return c.kind === 'monster' ? 'monster' : c.kind === 'npc' ? 'npc' : 'pc'
}
const kindCounts = computed<Record<'all' | KindKey, number>>(() => ({
  all: chars.value.length,
  pc: chars.value.filter(c => kindOf(c) === 'pc').length,
  npc: chars.value.filter(c => kindOf(c) === 'npc').length,
  monster: chars.value.filter(c => kindOf(c) === 'monster').length,
}))
/** 当前筛选下的角色（名册与右栏都只看这一份） */
const visibleChars = computed<CharacterDef[]>(() =>
  kindFilter.value === 'all' ? chars.value : chars.value.filter(c => kindOf(c) === kindFilter.value),
)
const dims = computed<AttributeDimension[]>(() => d.value?.attribute_dimensions ?? [])
const numDims = computed(() => dims.value.filter(dim => dim.type === 'number'))
const otherDims = computed(() => dims.value.filter(dim => dim.type !== 'number'))
const worldResources = computed(() => d.value?.world.resources ?? [])
const availableSkills = computed<SkillDef[]>(() => d.value?.skills ?? [])
const skillOptions = computed(() => availableSkills.value.map(s => ({ value: s.id, label: s.name })))
const availableItems = computed<ItemDef[]>(() => d.value?.items ?? [])
const itemOptions = computed(() => availableItems.value.map(i => ({ value: i.id, label: i.name })))

// ---------------- 开放种类挂接（把自定义种类接到人物上） ----------------
const kinds = computed<KindDef[]>(() => d.value?.kinds ?? [])
const definitions = computed<Definition[]>(() => d.value?.definitions ?? [])
/** 声明了「可挂接到人物」的开放种类 */
const attachableKinds = computed(() => kinds.value.filter(k => (k.applies_to ?? []).includes('character')))
function defsOfKind(kindKey: string): Definition[] { return definitions.value.filter(x => x.kind === kindKey) }
function attachmentsOf(c: CharacterDef, kindKey: string): string[] { return c.attachments?.[kindKey] ?? [] }
function attachDefinition(c: CharacterDef, kindKey: string, defId: string): void {
  const bag = c.attachments ?? (c.attachments = {})
  const list = bag[kindKey] ?? []
  if (!list.includes(defId)) bag[kindKey] = [...list, defId]
}
function detachDefinition(c: CharacterDef, kindKey: string, defId: string): void {
  const bag = c.attachments
  const list = bag?.[kindKey]
  if (!bag || !list) return
  bag[kindKey] = list.filter(x => x !== defId)
}
function defNameOf(id: string): string { return definitions.value.find(x => x.id === id)?.name ?? id }

// ---------------- 每日准备（#4）：从已掌握技能里选当日法术 ----------------
function isPrepared(c: CharacterDef, sid: string): boolean { return (c.prepared ?? []).includes(sid) }
function togglePrepared(c: CharacterDef, sid: string): void {
  const list = c.prepared ?? []
  c.prepared = list.includes(sid) ? list.filter(x => x !== sid) : [...list, sid]
}

// ---------------- 装备位（#5）：可装备物品的装备 / 卸下（同 slot 唯一） ----------------
function itemOf(id: string): ItemDef | undefined { return availableItems.value.find(x => x.id === id) }
function isEquipped(c: CharacterDef, id: string): boolean { return (c.equipped ?? []).includes(id) }
function toggleEquipped(c: CharacterDef, id: string): void {
  const list = c.equipped ?? []
  if (list.includes(id)) { c.equipped = list.filter(x => x !== id); return }
  const slot = itemOf(id)?.slot
  const drop = slot ? new Set(list.filter(x => itemOf(x)?.slot === slot)) : new Set<string>()
  c.equipped = [...list.filter(x => !drop.has(x)), id]
}

// ---------------- 卡面分区（#6）：由故事书声明的展示结构 ----------------
const sheetSections = computed<SheetSection[]>(() => d.value?.sheet ?? [])
const sheetOpen = ref(false)

/** 好卡自检（借鉴 SillyTavern 角色卡指南）：描写宜精不宜长，对话示例最关键。 */
const writingChecks = computed(() => {
  const c = current.value
  if (!c) return []
  const bg = (c.background ?? '').length
  const pers = (c.personality ?? '').length
  const ex = (c.example_dialogues ?? '').trim().length
  return [
    { label: `背景 ${bg} 字`, ok: bg > 0 && bg <= 800, hint: '200–500 字：只保留最关键的信息' },
    { label: `性格 ${pers} 字`, ok: pers > 0 && pers <= 500, hint: '2–3 个一致的核心特征，用行为而不是形容词' },
    { label: ex > 0 ? `对话示例 ${ex} 字` : '缺对话示例', ok: ex > 0, hint: '3–5 轮示范口吻的对话——改变 AI 语气最有效的手段' },
  ]
})
const writingScore = computed(() => writingChecks.value.filter(c => c.ok).length)

// ---------------- 叙事档案：放大到弹窗里写 ----------------
type NarrativeKey = 'background' | 'personality' | 'example_dialogues' | 'notes'
const expandedField = ref<NarrativeKey | null>(null)
const narrativeMeta: Record<NarrativeKey, { title: string; en: string; hint: string }> = {
  background: { title: '背景渊源', en: 'Background', hint: '详细描述角色的出身、过往经历、隐秘身世与核心动机…' },
  personality: { title: '性格特质', en: 'Personality', hint: '描述角色的行为习惯、言谈举止风格、处事准则与心理弱点…' },
  example_dialogues: { title: '对话示例', en: 'Example Dialogues', hint: '写 3-5 轮示范该角色口吻的对话（每轮一行，或「角色名：台词」）。AI 会模仿这里的语气、句长与用词——这是最有效的风格控制。' },
  notes: { title: '作者注释', en: "Creator's Notes", hint: '给使用者的备注：创作灵感、使用建议、已知问题。这里的内容不会发给 AI，可以放心写剧透。' },
}
const activeNarrative = computed(() => (expandedField.value ? narrativeMeta[expandedField.value] : null))
const expandedText = computed({
  get: () => (expandedField.value && current.value ? (current.value[expandedField.value] ?? '') : ''),
  set: (v: string) => {
    const f = expandedField.value
    const c = current.value
    if (f && c) c[f] = v
  },
})
function addSheetSection(): void {
  const sb = d.value
  if (!sb) return
  if (!sb.sheet) sb.sheet = []
  sb.sheet.push({ title: '新分区', kinds: [], derived: [], resources: [], narrative: [] })
}
function removeSheetSection(i: number): void { d.value?.sheet?.splice(i, 1) }
type SheetFieldKey = 'kinds' | 'derived' | 'resources' | 'narrative'
interface SheetFieldGroup { key: SheetFieldKey; label: string; options: { value: string; label: string }[] }
/** 卡面分区四类内容的可选项（直接点选，不用手抄 id）。 */
const sheetFieldGroups = computed<SheetFieldGroup[]>(() => [
  { key: 'kinds', label: '挂接种类', options: attachableKinds.value.map(k => ({ value: k.key, label: k.label })) },
  { key: 'derived', label: '派生值', options: sbDerived.value.map(d => ({ value: d.key, label: d.label || d.key })) },
  { key: 'resources', label: '资源', options: worldResources.value.map(r => ({ value: r.id, label: r.name })) },
  { key: 'narrative', label: '叙事字段', options: [
    { value: 'background', label: '背景' },
    { value: 'personality', label: '性格' },
    { value: 'appearance', label: '外观' },
  ] },
])
function toggleSheetValue(sec: SheetSection, field: SheetFieldKey, value: string): void {
  const cur = sec[field] ?? []
  sec[field] = cur.includes(value) ? cur.filter(x => x !== value) : [...cur, value]
}
/** 已存在但不在当前可选项里的值（如种类被删）——仍显示为可移除的 chip，避免静默丢失。 */
function extraValues(sec: SheetSection, field: SheetFieldKey, options: { value: string }[]): string[] {
  const known = new Set(options.map(o => o.value))
  return (sec[field] ?? []).filter(v => !known.has(v))
}
function attachedDefs(c: CharacterDef, kindKey: string): Definition[] {
  const ids = new Set(c.attachments?.[kindKey] ?? [])
  return definitions.value.filter(x => ids.has(x.id))
}
function derivedOf(key: string): { label: string; value: number | null } | undefined {
  return derivedValues.value.find(x => x.key === key)
}
function resourceOf(id: string): { name: string; max?: number; cur: number } | undefined {
  const def = worldResources.value.find(r => r.id === id)
  if (!def) return undefined
  return { name: def.name, max: def.default_max, cur: Number(current.value?.resources?.[id] ?? 0) }
}
/** 某开放种类的字段 schema（用于卡面按声明顺序、用字段标签渲染）。 */
function kindFields(kindKey: string): FieldDef[] {
  return kinds.value.find(k => k.key === kindKey)?.fields ?? []
}
/** 字段值 → 可读文本：ref 解析成定义名而不是甩 id；布尔转是/否；数组顿号连接。 */
function fieldValueText(kindKey: string, fieldKey: string, value: unknown): string {
  if (value == null || value === '') return '—'
  const f = kindFields(kindKey).find(x => x.key === fieldKey)
  const one = (v: unknown): string => {
    if (v == null || v === '') return ''
    if (f?.type === 'ref' || f?.type === 'ref_list') {
      return definitions.value.find(d => d.id === v)?.name ?? String(v)
    }
    if (typeof v === 'boolean') return v ? '是' : '否'
    return String(v)
  }
  if (Array.isArray(value)) {
    const parts = value.map(one).filter(Boolean)
    return parts.length ? parts.join('、') : '—'
  }
  return one(value) || '—'
}
/** 某定义里「有值」的字段（按 schema 顺序）。 */
function visibleFields(kindKey: string, def: Definition): FieldDef[] {
  return kindFields(kindKey).filter(f => fieldValueText(kindKey, f.key, def.fields?.[f.key]) !== '—')
}
/** 无字段种类的全部定义名（如语言：通用语、龙语、矮人语）。 */
function attachedNames(kindKey: string): string {
  return attachedDefs(current.value as CharacterDef, kindKey).map(d => d.name).join('、')
}
function derivedText(key: string): string {
  const dv = derivedValues.value.find(x => x.key === key)
  if (!dv || dv.value == null) return '—'
  return (dv.signed && dv.value >= 0 ? '+' : '') + dv.value
}
function narrativeLabel(key: string): string {
  if (key === 'background') return '背景'
  if (key === 'personality') return '性格'
  if (key === 'appearance') return '外观'
  return key
}
function narrativeOf(c: CharacterDef, key: string): string {
  if (key === 'background') return c.background ?? ''
  if (key === 'personality') return c.personality ?? ''
  if (key === 'appearance') return c.appearance ?? ''
  return ''
}

// ---------------- 派生值（#2）：属性 + 修正来源 → 调整值 / 豁免 / AC / 法术DC ----------------
const sbDerived = computed(() => d.value?.derived ?? [])
const derivedComputation = computed(() => (current.value && d.value ? computeDerived(d.value, current.value) : null))
const derivedValues = computed(() => derivedComputation.value?.derived ?? [])
/** 含修正来源（挂接定义 / 已装备物品）后的有效属性值；与引擎取的值一致。 */
function effectiveAttr(dim: AttributeDimension): number {
  const v = derivedComputation.value?.attrs?.[dim.key]
  return v != null ? v : statVal(current.value as CharacterDef, dim.key, dim.baseline ?? 10)
}
const derivedOpen = ref(false)
function addDerived(): void {
  const sb = d.value
  if (!sb) return
  if (!sb.derived) sb.derived = []
  sb.derived.push({ key: 'new_value', label: '新派生值', formula: '0' })
}
function removeDerived(i: number): void { d.value?.derived?.splice(i, 1) }
/** 派生值行的实时预览（当前选中角色）。 */
function derivedPreview(key: string): { text: string; tone: string } {
  const dv = derivedValues.value.find(x => x.key === key)
  if (!dv) return { text: '—', tone: 'text-muted-foreground/40' }
  if (dv.value == null) return { text: '错误', tone: 'text-destructive' }
  return { text: (dv.signed && dv.value >= 0 ? '+' : '') + dv.value, tone: 'text-primary' }
}

const items = computed<WorkbenchItem[]>(() => visibleChars.value.map(c => {
  const k = kindOf(c)
  return {
    id: entityKey(c),
    title: c.name,
    sub: c.id,
    badge: KIND_BADGE[k],
    tone: k === 'pc' ? 'ok' : k === 'monster' ? 'warn' : 'default',
  } as WorkbenchItem
}))

const current = computed(() => visibleChars.value.find(c => entityKey(c) === selected.value) ?? null)
const currentKind = computed<KindKey | null>(() => (current.value ? kindOf(current.value) : null))
/** 怪物：隐藏扮演向区块，改显示数据卡 */
const isMonster = computed(() => currentKind.value === 'monster')

/** 首个受控主角（开档默认主角） */
const firstPcId = computed(() => chars.value.find(c => c.kind === 'pc')?.id ?? '')

/** 新增角色：kind 跟随当前筛选（「全部」时首个角色为 PC，其余为 NPC） */
function addCharacter(): void {
  const arr = d.value?.characters
  if (!arr) return
  const k: KindKey = kindFilter.value !== 'all'
    ? kindFilter.value
    : (chars.value.some(c => kindOf(c) === 'pc') ? 'npc' : 'pc')
  const c: CharacterDef = {
    id: uid('char'),
    name: k === 'monster' ? '新怪物' : '新角色',
    kind: k,
    background: '',
    personality: '',
    attributes: {},
    resources: {},
    skills: [],
    ...(k === 'monster' ? { statblock: {} } : {}),
  }
  arr.unshift(c)
  selected.value = entityKey(c)
  if (kindFilter.value !== 'all' && kindFilter.value !== k) kindFilter.value = k
}

function removeCharacter(c: CharacterDef): void {
  const arr = d.value?.characters
  if (!arr) return
  const i = arr.indexOf(c)
  if (i >= 0) arr.splice(i, 1)
}

// ---------------- 怪物数据卡（图鉴 M1）：展示型字段 + 攻击技能绑定 ----------------
/** 写 statblock 展示字段：留空即删，保持 JSON 干净 */
function setStatField(c: CharacterDef, key: keyof MonsterStatblock, v: string): void {
  if (!c.statblock) c.statblock = {}
  if (v === '') delete c.statblock[key]
  else if (key === 'xp') c.statblock.xp = Number(v) || undefined
  else c.statblock[key] = v
}
/** 技能库里判定种类为「攻击」的技能（check.kind === 'attack'）——数据卡只从这里挑 */
const attackSkills = computed<SkillDef[]>(() =>
  availableSkills.value.filter(s => typeof s.check === 'object' && s.check?.kind === 'attack'),
)
function isSkillBound(c: CharacterDef, sid: string): boolean { return (c.skills ?? []).includes(sid) }
/** 攻击技能绑定开关：直接落进角色的 skills[]（引擎按它结算攻击） */
function toggleSkillBinding(c: CharacterDef, sid: string): void {
  const list = c.skills ?? []
  c.skills = list.includes(sid) ? list.filter(x => x !== sid) : [...list, sid]
}
const boundAttackCount = computed(() =>
  (current.value?.skills ?? []).filter(sid => attackSkills.value.some(s => s.id === sid)).length,
)
/** 怪物 AC 实时预览：派生值里的 ac（缺失回落 12，与引擎同口径） */
const acPreview = computed(() => derivedValues.value.find(x => x.key === 'ac') ?? null)

// ---------------- 常驻地 / 出没地（地图 P1）：引用地点 ----------------
const locationOptions = computed(() =>
  (d.value?.world.locations ?? []).map(l => ({ value: l.id, label: l.name + ' · ' + l.id })),
)
/** reka Select 不接受空串，用哨兵占位 */
const NO_LOCATION = '__no_location__'
function setLocation(c: CharacterDef, v: string): void {
  if (v && v !== NO_LOCATION) c.location_id = v
  else delete c.location_id
}

// ---- SillyTavern 角色卡导入（PNG 内嵌 JSON / V2 JSON）+ 内嵌角色书词条 ----
const stFileInput = ref<HTMLInputElement | null>(null)
const stImporting = ref(false)
function pickStFile(): void { stFileInput.value?.click() }
async function onStFile(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file || !d.value) return
  stImporting.value = true
  try {
    const res = await importStFile(file)
    const sb = d.value
    const onlyLore = res.character.name === '（仅世界书）'
    if (!onlyLore) {
      // ST 把立绘嵌在卡里：PNG 角色卡的头像一并入库为人物立绘
      if (file.type === 'image/png') {
        try {
          const up = await uploadAsset(file)
          const bmp = await createImageBitmap(file)
          res.character.portrait = { asset: up.asset, w: bmp.width, h: bmp.height }
          bmp.close?.()
        } catch { /* 头像导入失败不阻断人物导入 */ }
      }
      sb.characters.unshift(res.character)
      selected.value = entityKey(res.character)
    }
    if (res.lore.length) {
      if (!sb.lore) sb.lore = []
      sb.lore.unshift(...res.lore)
    }
    if (onlyLore) toast('ok', `已导入 ${res.lore.length} 条世界词条`)
    else toast('ok', `已导入人物「${res.character.name}」${res.lore.length ? `及 ${res.lore.length} 条词条` : ''}`)
  } catch (err) {
    toast('error', err instanceof Error ? err.message : '导入失败')
  } finally {
    stImporting.value = false
  }
}

function setAttr(c: CharacterDef, key: string, v: number | string): void {
  if (!c.attributes) c.attributes = {}
  c.attributes[key] = v
}

function setResource(c: CharacterDef, resId: string, val: number): void {
  if (!c.resources) c.resources = {}
  c.resources[resId] = val
}

/** 故事书中未被当前角色掌握的技能候选 */
const unassignedSkills = computed(() => {
  if (!current.value) return []
  const owned = new Set(current.value.skills ?? [])
  return availableSkills.value.filter(s => !owned.has(s.id))
})

/** 获取技能实体定义 */
function getSkill(sid: string): SkillDef | undefined {
  return availableSkills.value.find(s => s.id === sid)
}

/** 为角色绑定新技能 */
function bindSkillToChar(c: CharacterDef, sid: string): void {
  if (!c.skills) c.skills = []
  if (!c.skills.includes(sid)) {
    c.skills.push(sid)
    const sk = getSkill(sid)
    toast('ok', `已为「${c.name}」添加技能「${sk?.name ?? sid}」`)
  }
}

/** 从角色移除技能绑定 */
function unbindCharSkill(c: CharacterDef, index: number): void {
  if (!c.skills) return
  const sid = c.skills[index]
  const sk = getSkill(sid)
  c.skills.splice(index, 1)
  toast('ok', `已移除技能「${sk?.name ?? sid}」`)
}

/** 绑定一个尚未引用的技能（兼容调用） */
function addCharSkill(c: CharacterDef): void {
  if (unassignedSkills.value[0]) {
    bindSkillToChar(c, unassignedSkills.value[0].id)
  }
}

/** 故事书中当前角色尚未携带的物品候选 */
const unassignedItems = computed(() => {
  if (!current.value) return []
  const owned = new Set((current.value.inventory ?? []).map(e => e.id))
  return availableItems.value.filter(it => !owned.has(it.id))
})

/** 往角色物品栏放入一件物品（数量 1）；use_item 按此校验持有（#01） */
function bindItemToChar(c: CharacterDef, itemId: string): void {
  if (!c.inventory) c.inventory = []
  if (!c.inventory.some(e => e.id === itemId)) {
    c.inventory.push({ id: itemId, quantity: 1 })
    const it = availableItems.value.find(x => x.id === itemId)
    toast('ok', `已为「${c.name}」放入「${it?.name ?? itemId}」`)
  }
}

/** 技能消耗文本呈现 */
function skillCostText(s: SkillDef): string {
  if (!s.cost || s.cost.length === 0) return '无消耗'
  return s.cost.map(c => {
    const rName = worldResources.value.find(r => r.id === c.resource)?.name || c.resource
    return `${rName} ${c.amount}`
  }).join(' · ')
}

/** 按 id 取顶层状态声明（技能 effect.status 存的是引用） */
function statusOf(id: string): StatusDef | undefined {
  return (d.value?.statuses ?? []).find(x => x.id === id)
}

/** 技能判定规则呈现 */
function skillCheckText(s: SkillDef): string | null {
  if (!s.check) return null
  if (typeof s.check === 'string') return `检定: ${s.check}`
  if (s.check.dice) {
    const thr = s.check.degree_thresholds?.[0]
    return thr != null ? `${s.check.dice} (≥${thr})` : `${s.check.dice} 检定`
  }
  return '有检定'
}

/** 技能即时效果呈现 */
function skillEffectText(s: SkillDef): string | null {
  if (!s.effect?.immediate?.length) return null
  return s.effect.immediate.map(imm => {
    if (imm.kind === 'modify_resource') {
      const rName = worldResources.value.find(r => r.id === imm.resource)?.name || imm.resource
      const amt = Number(imm.amount)
      return `${rName} ${amt > 0 ? '+' + amt : amt}`
    }
    if (imm.kind === 'damage') return `伤害 ${imm.amount}`
    if (imm.kind === 'heal') return `治疗 ${imm.amount}`
    if (imm.kind === 'set_flag') return `设标记 ${imm.flag}`
    return ''
  }).filter(Boolean).join(' · ')
}

// ---------------- 技能卡展示弹窗（Inspector Modal） ----------------
const detailModalOpen = ref(false)
const inspectingSkillId = ref<string | null>(null)
const inspectingSkillIndex = ref<number>(-1)

const inspectingSkill = computed<SkillDef | null>(() => {
  if (!inspectingSkillId.value) return null
  return getSkill(inspectingSkillId.value) ?? null
})

function openSkillDetail(sid: string, index: number): void {
  inspectingSkillId.value = sid
  inspectingSkillIndex.value = index
  detailModalOpen.value = true
}

function closeSkillDetail(): void {
  detailModalOpen.value = false
}

function swapInspectingSkill(newSid: string): void {
  if (!current.value || inspectingSkillIndex.value < 0) return
  current.value.skills![inspectingSkillIndex.value] = newSid
  inspectingSkillId.value = newSid
  const sk = getSkill(newSid)
  toast('ok', `已更换技能为「${sk?.name ?? newSid}」`)
}

function removeInspectingSkill(): void {
  if (!current.value || inspectingSkillIndex.value < 0) return
  const idx = inspectingSkillIndex.value
  closeSkillDetail()
  unbindCharSkill(current.value, idx)
}

// ---------------- 立绘上传与压缩 ----------------
const fileInput = ref<HTMLInputElement | null>(null)
const busyUpload = ref(false)
const isDragging = ref(false)

async function compressImage(file: File, maxEdge = 1024): Promise<{ blob: Blob; w: number; h: number }> {
  const bitmap = await createImageBitmap(file)
  const scale = Math.min(1, maxEdge / Math.max(bitmap.width, bitmap.height))
  const w = Math.max(1, Math.round(bitmap.width * scale))
  const h = Math.max(1, Math.round(bitmap.height * scale))
  const canvas = document.createElement('canvas')
  canvas.width = w
  canvas.height = h
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('浏览器不支持 canvas')
  ctx.drawImage(bitmap, 0, 0, w, h)
  bitmap.close?.()
  const blob = (await new Promise<Blob | null>(res => canvas.toBlob(res, 'image/webp', 0.9)))
    ?? (await new Promise<Blob | null>(res => canvas.toBlob(res, 'image/jpeg', 0.9)))
  if (!blob) throw new Error('图片编码失败')
  return { blob, w, h }
}

async function handleFile(file: File | null | undefined, targetChar: CharacterDef): Promise<void> {
  if (!file) return
  if (!file.type.startsWith('image/')) {
    toast('warn', '请选择图片文件')
    return
  }
  busyUpload.value = true
  try {
    const { blob, w, h } = await compressImage(file)
    const up = await uploadAsset(blob)
    targetChar.portrait = { asset: up.asset, w, h }
    toast('ok', `立绘已更新（${w}×${h}）`)
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e))
  } finally {
    busyUpload.value = false
  }
}

function onPickFile(e: Event, targetChar: CharacterDef): void {
  const el = e.target as HTMLInputElement
  void handleFile(el.files?.[0], targetChar)
  el.value = ''
}

function onDrop(e: DragEvent, targetChar: CharacterDef): void {
  isDragging.value = false
  void handleFile(e.dataTransfer?.files?.[0], targetChar)
}

function clearPortrait(targetChar: CharacterDef): void {
  targetChar.portrait = undefined
  toast('ok', '已清除立绘')
}

// ---------------- 辅助：复制 ID 与两步删除 ----------------
async function copyId(id: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(id)
    toast('ok', `已复制 ID：${id}`)
  } catch {
    toast('info', id)
  }
}

const confirmingDeleteId = ref<string | null>(null)
let deleteTimer: ReturnType<typeof setTimeout> | null = null

function onDeleteClick(c: CharacterDef): void {
  if (confirmingDeleteId.value !== c.id) {
    confirmingDeleteId.value = c.id
    if (deleteTimer) clearTimeout(deleteTimer)
    deleteTimer = setTimeout(() => { confirmingDeleteId.value = null }, 3500)
    return
  }
  if (deleteTimer) clearTimeout(deleteTimer)
  confirmingDeleteId.value = null
  removeCharacter(c)
  toast('ok', `已删除角色「${c.name}」`)
}

onUnmounted(() => { if (deleteTimer) clearTimeout(deleteTimer) })

// ---------------- 游戏属性卡辅助 ----------------
function charByKey(key: string): CharacterDef | undefined {
  return chars.value.find(c => entityKey(c) === key)
}

function charKeyStats(c: CharacterDef): string {
  const attrs = c.attributes ?? {}
  const pairs: string[] = []
  for (const dim of numDims.value) {
    if (attrs[dim.key] != null) {
      pairs.push(`${dim.label} ${attrs[dim.key]}`)
      if (pairs.length >= 3) break
    }
  }
  return pairs.length ? pairs.join(' · ') : c.id
}

function statVal(c: CharacterDef, key: string, baseline = 10): number {
  const v = c.attributes?.[key]
  if (typeof v === 'number') return v
  const n = Number(v)
  return Number.isNaN(n) ? baseline : n
}

function statDiff(val: number, baseline = 10): { diff: number; text: string; tone: 'plus' | 'minus' | 'base' } {
  const diff = val - baseline
  if (diff > 0) return { diff, text: `+${diff}`, tone: 'plus' }
  if (diff < 0) return { diff, text: `${diff}`, tone: 'minus' }
  return { diff: 0, text: `0`, tone: 'base' }
}

/** 与引擎 resolve.rs 同款：floor((clamp(值) - 基线) / 步长)，夹到维度推出的修正范围。 */
function ruleModifier(dim: AttributeDimension, value: number): number {
  const baseline = dim.baseline ?? 50
  const step = dim.modifier_step && dim.modifier_step > 0 ? dim.modifier_step : 5
  let v = value
  if (dim.min != null) v = Math.max(v, dim.min)
  if (dim.max != null) v = Math.min(v, dim.max)
  const raw = Math.floor((v - baseline) / step)
  let lo = -10
  let hi = 10
  if (dim.min != null && dim.max != null) {
    const a = Math.floor((dim.min - baseline) / step)
    const b = Math.floor((dim.max - baseline) / step)
    lo = Math.min(a, b)
    hi = Math.max(a, b)
  }
  return Math.min(Math.max(raw, lo), hi)
}
function signed(n: number): string { return (n >= 0 ? '+' : '') + n }
function modTone(n: number): string {
  return n > 0 ? 'text-success' : n < 0 ? 'text-destructive' : 'text-muted-foreground/70'
}

function statPct(val: number): number {
  return Math.max(0, Math.min(100, val))
}

function gaugeClass(val: number, baseline = 10): string {
  if (val >= baseline + 20) return 'bg-success'
  if (val >= baseline) return 'bg-primary'
  if (val >= baseline - 10) return 'bg-warning'
  return 'bg-destructive'
}
</script>

<template>
  <WorkbenchLayout
    title="角色库"
    hint="角色模板集合：玩家角色 / NPC / 怪物同住一处。属性消费全局维度；kind=monster 改显示数据卡与攻击技能。"
    :count="items.length"
    :items="items"
    :selected="selected"
    add-label="角色"
    search-placeholder="搜索角色姓名、ID…"
    empty-hint="还没有角色模板。用右上「新增」创建，或从 SillyTavern 角色卡导入。"
    custom-actions
    @update:selected="select"
    @add="addCharacter"
  >
    <!-- 左栏：按 kind 筛选（玩家角色 / NPC / 怪物） -->
    <template #railFilter>
      <button
        v-for="t in KIND_TABS"
        :key="t.key"
        type="button"
        class="inline-flex cursor-pointer items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-colors"
        :class="kindFilter === t.key
          ? 'border-primary/50 bg-primary/10 font-semibold text-primary'
          : 'border-border text-muted-foreground hover:bg-muted/60'"
        :title="'只显示「' + t.label + '」的角色'"
        @click="kindFilter = t.key"
      >
        <span>{{ t.label }}</span>
        <span class="font-mono text-[10px] opacity-70">{{ kindCounts[t.key] }}</span>
      </button>
    </template>

    <template #railActions>
      <Button variant="outline" size="sm" class="h-7 gap-1 px-2 text-xs" :disabled="stImporting" title="导入 SillyTavern 角色卡（PNG / V2 JSON）" @click="pickStFile">
        <IconFileImport class="size-3.5" />
        {{ stImporting ? '导入中…' : '导入' }}
      </Button>
      <Button size="sm" class="h-7 gap-1 px-2 text-xs" title="按当前筛选分类新增角色" @click="addCharacter">
        <IconPlus class="size-3.5" />
        新增
      </Button>
      <input ref="stFileInput" type="file" accept=".png,.json,application/json,image/png" class="hidden" @change="onStFile" />
    </template>
    <!-- 左侧清单定制：游戏角色名册风格 -->
    <template #item="{ item, selected: isSelected, select: doSelect }">
      <button
        type="button"
        class="group mb-1.5 flex w-full cursor-pointer items-center gap-2.5 rounded-xl border p-2 text-left transition-all duration-200"
        :class="isSelected
          ? 'border-primary/60 bg-gradient-to-r from-primary/15 to-primary/5 shadow-2xs ring-1 ring-primary/30'
          : 'border-border/60 bg-card/60 hover:border-border hover:bg-muted/40'"
        @click="doSelect()"
      >
        <!-- 头像缩略图 -->
        <div class="relative size-8 shrink-0 overflow-hidden rounded-lg border border-border/80 bg-muted/40 shadow-inner">
          <img
            v-if="charByKey(item.id)?.portrait"
            :src="assetUrl(charByKey(item.id)!.portrait!.asset)"
            :alt="item.title"
            class="size-full object-cover"
          />
          <div
            v-else
            class="flex size-full items-center justify-center text-xs font-black"
            :class="nameTintClass(item.title)"
          >
            {{ initial(item.title) }}
          </div>
        </div>

        <!-- 角色基础信息 -->
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-1.5">
            <span
              class="truncate text-[13px] font-bold tracking-tight transition-colors"
              :class="isSelected ? 'text-primary' : 'text-foreground'"
            >
              {{ item.title || '（未命名）' }}
            </span>
          </div>
          <div class="truncate font-mono text-[10px] text-muted-foreground/75">
            {{ charByKey(item.id) ? charKeyStats(charByKey(item.id)!) : item.sub }}
          </div>
        </div>

        <!-- 身份徽标 -->
        <Badge
          variant="outline"
          class="shrink-0 px-1.5 py-0 text-[9.5px] font-mono font-bold"
          :class="charByKey(item.id)?.kind === 'pc'
            ? 'border-primary/40 bg-primary/15 text-primary shadow-2xs'
            : charByKey(item.id)?.kind === 'monster'
              ? 'border-warning/50 bg-warning/10 text-warning'
              : 'border-border/70 bg-muted/60 text-muted-foreground'"
        >
          {{ charByKey(item.id)?.kind === 'pc' ? '👑 PC' : charByKey(item.id)?.kind === 'monster' ? '☠ 怪物' : 'NPC' }}
        </Badge>
      </button>
    </template>

    <!-- 右侧编辑区：游戏人物卡呈现 -->
    <template v-if="current">
      <div
        class="relative mx-auto w-full max-w-5xl rounded-2xl border transition-all duration-300 shadow-md backdrop-blur-sm overflow-hidden"
        :class="currentKind === 'pc'
          ? 'border-primary/50 bg-gradient-to-b from-primary/10 via-card/95 to-card ring-1 ring-primary/20 shadow-primary/5'
          : currentKind === 'monster'
            ? 'border-warning/50 bg-gradient-to-b from-warning/10 via-card/95 to-card ring-1 ring-warning/20 shadow-warning/5'
            : 'border-border/80 bg-gradient-to-b from-muted/30 via-card/95 to-card shadow-sm'"
      >
        <!-- 卡片顶层控制栏：阵营切换 + 角色操作 -->
        <div class="flex flex-wrap items-center justify-between gap-3 border-b border-border/70 bg-card/60 px-5 py-3 backdrop-blur-xs">
          <!-- 身份模式分段开关（PC / NPC） -->
          <div class="flex items-center gap-3">
            <div class="inline-flex rounded-lg border border-border/80 bg-muted/40 p-0.5 shadow-inner">
              <button
                type="button"
                class="inline-flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-bold transition-all"
                :class="currentKind === 'pc'
                  ? 'bg-primary text-primary-foreground shadow-xs'
                  : 'text-muted-foreground hover:text-foreground'"
                title="玩家角色：开档后可被选为受控主角"
                @click="current.kind = 'pc'"
              >
                <IconCrown class="size-3.5" />
                <span>玩家角色</span>
              </button>
              <button
                type="button"
                class="inline-flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-bold transition-all"
                :class="currentKind === 'npc'
                  ? 'bg-secondary text-secondary-foreground shadow-xs'
                  : 'text-muted-foreground hover:text-foreground'"
                title="NPC：场景里可交互的故事角色"
                @click="current.kind = 'npc'"
              >
                <IconUsers class="size-3.5" />
                <span>NPC</span>
              </button>
              <button
                type="button"
                class="inline-flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-bold transition-all"
                :class="currentKind === 'monster'
                  ? 'bg-warning text-warning-foreground shadow-xs'
                  : 'text-muted-foreground hover:text-foreground'"
                title="怪物：遭遇里的敌方单位，只承载结算数据与数据卡"
                @click="current.kind = 'monster'"
              >
                <IconSwords class="size-3.5" />
                <span>怪物</span>
              </button>
            </div>

            <Badge
              v-if="currentKind === 'pc'"
              variant="outline"
              class="border-primary/30 bg-primary/10 text-[11px] font-medium text-primary hidden sm:inline-flex"
            >
              {{ current.id === firstPcId ? '默认开档主角' : '受控角色候选' }}
            </Badge>
            <span v-else-if="currentKind === 'monster'" class="text-xs text-warning/90 hidden sm:inline">
              怪物 · 遭遇中的敌方单位（不参与对话扮演）
            </span>
            <span v-else class="text-xs text-muted-foreground/75 hidden sm:inline">
              故事角色 · 场景中可交互
            </span>
          </div>

          <!-- 右侧操作栏：ID 快速复制 + 删除 -->
          <div class="flex items-center gap-2">
            <button
              type="button"
              class="group flex cursor-pointer items-center gap-1.5 rounded-md border border-border/70 bg-muted/40 px-2.5 py-1 font-mono text-xs text-muted-foreground/90 transition-colors hover:border-primary/40 hover:bg-muted/80 hover:text-foreground"
              title="点击复制人物 ID"
              @click="copyId(current.id)"
            >
              <span class="opacity-60">ID:</span>
              <span>{{ current.id }}</span>
              <IconCopy class="size-3.5 text-muted-foreground/60 transition-transform group-hover:scale-110" />
            </button>

            <Button
              variant="ghost"
              size="sm"
              class="h-8 gap-1.5 text-xs transition-colors"
              :class="confirmingDeleteId === current.id
                ? 'bg-destructive/15 text-destructive hover:bg-destructive/25'
                : 'text-muted-foreground hover:bg-destructive/10 hover:text-destructive'"
              @click="onDeleteClick(current)"
            >
              <IconTrash class="size-3.5" />
              <span>{{ confirmingDeleteId === current.id ? '确认删除？' : '删除' }}</span>
            </Button>
          </div>
        </div>

        <!-- 卡片主体：头部立绘身份 + 宽敞叙事档案 + 核心属性网格 -->
        <div class="p-5 sm:p-6 space-y-5">
          <!-- 头部区域：大气立绘框 + 角色姓名与属性速览 -->
          <div class="flex flex-col sm:flex-row items-start gap-5">
            <!-- 立绘卡槽：3:4 比例游戏人物立绘框（加大尺寸，舒展大气） -->
            <div
              class="group relative flex w-36 sm:w-44 flex-none cursor-pointer flex-col overflow-hidden rounded-xl border border-border/80 bg-muted/30 shadow-sm transition-all hover:border-primary/50 hover:shadow-md"
              :class="isDragging ? 'border-primary ring-2 ring-primary/40' : ''"
              style="aspect-ratio: 3 / 4;"
              @click="fileInput?.click()"
              @dragover.prevent="isDragging = true"
              @dragleave.prevent="isDragging = false"
              @drop.prevent="onDrop($event, current)"
            >
              <!-- 已有立绘 -->
              <template v-if="current.portrait">
                <img
                  :src="assetUrl(current.portrait.asset)"
                  :alt="current.name"
                  class="size-full object-cover transition-transform duration-300 group-hover:scale-105"
                />
                <div class="absolute inset-0 flex flex-col items-center justify-end bg-gradient-to-t from-black/80 via-black/20 to-transparent p-2.5 opacity-0 transition-opacity duration-200 group-hover:opacity-100">
                  <div class="flex w-full items-center justify-between gap-1">
                    <span class="text-xs font-medium text-white/90">更换立绘</span>
                    <button
                      type="button"
                      class="cursor-pointer rounded p-1 text-white/70 hover:bg-white/20 hover:text-white"
                      title="清除立绘"
                      @click.stop="clearPortrait(current)"
                    >
                      <IconTrash class="size-3.5 text-destructive-foreground" />
                    </button>
                  </div>
                </div>
              </template>

              <!-- 未上传立绘：首字游戏头像占位 + 点击上传提示 -->
              <div
                v-else
                class="flex size-full flex-col items-center justify-between p-4 text-center transition-colors"
                :class="nameTintClass(current.name)"
              >
                <span class="mt-3 text-4xl font-black tracking-wider opacity-85 select-none">
                  {{ initial(current.name) }}
                </span>
                <div class="flex items-center gap-1.5 rounded-full bg-background/85 px-2.5 py-1 text-xs font-semibold text-foreground backdrop-blur-xs shadow-2xs">
                  <IconUpload class="size-3.5" />
                  <span>上传立绘</span>
                </div>
              </div>

              <!-- 上传中遮罩 -->
              <div v-if="busyUpload" class="absolute inset-0 flex items-center justify-center bg-background/80 backdrop-blur-xs">
                <IconLoader2 class="size-6 animate-spin text-primary" />
              </div>
            </div>

            <!-- 右侧：姓名输入 + ID编辑 + 属性速览 -->
            <div class="flex min-w-0 flex-1 flex-col justify-between self-stretch gap-4">
              <div class="space-y-2">
                <!-- 角色姓名 -->
                <div class="flex items-center gap-2">
                  <input
                    v-model="current.name"
                    type="text"
                    placeholder="角色名称…"
                    class="w-full rounded-md border border-dashed border-border bg-muted/25 px-2 py-1 font-serif text-3xl font-black tracking-wide text-foreground transition-colors placeholder:text-muted-foreground/40 hover:border-solid hover:border-primary/60 hover:bg-primary/5 focus:border-solid focus:border-primary focus:bg-background focus:outline-none"
                    title="点击可修改角色名"
                  />
                  <IconPencil class="size-4 shrink-0 text-muted-foreground/45 transition-colors group-hover:text-primary/70" />
                </div>

                <!-- 唯一 ID 与 关系链提示 -->
                <div class="flex flex-wrap items-center gap-2 pt-1 text-xs text-muted-foreground">
                  <span class="font-mono text-xs font-semibold">内部标识:</span>
                  <span class="inline-flex items-center gap-1 rounded-md border border-dashed border-border bg-muted/25 px-2 py-1 transition-colors focus-within:border-solid focus-within:border-primary hover:border-solid hover:border-primary/60 hover:bg-muted/50">
                    <input
                      v-model="current.id"
                      type="text"
                      placeholder="唯一引用 ID"
                      title="点击可修改 ID"
                      class="h-5 w-40 bg-transparent font-mono text-xs text-foreground placeholder:text-muted-foreground/40 focus:outline-none"
                    />
                    <IconPencil class="size-3 shrink-0 text-muted-foreground/60" />
                  </span>
                  <span class="text-xs text-muted-foreground/70 hidden sm:inline">（场景在场名单 / 关系网连线引用）</span>
                </div>

                <!-- 常驻地 / 出没地（地图 P1）：引用 world.locations -->
                <div class="flex flex-wrap items-center gap-2 pt-1 text-xs text-muted-foreground">
                  <IconMapPin class="size-3.5 shrink-0 text-muted-foreground/60" />
                  <span class="font-semibold">{{ isMonster ? '出没地' : '常驻地' }}</span>
                  <Select
                    :model-value="current.location_id ?? NO_LOCATION"
                    @update:model-value="setLocation(current, String($event ?? ''))"
                  >
                    <SelectTrigger class="h-7 w-52 text-xs" :title="isMonster ? '怪物常出没的地点（遭遇继承它）' : 'NPC / PC 的常驻地点：场景按位置算在场'">
                      <SelectValue placeholder="（未指定地点）" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem :value="NO_LOCATION" class="text-muted-foreground">（未指定）</SelectItem>
                        <SelectItem v-for="o in locationOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  <span v-if="!locationOptions.length" class="text-[11px] text-muted-foreground/60">世界还没有地点，去「世界」新增</span>
                </div>
              </div>

              <!-- 角色能力速览条 / 标签卡 -->
              <div class="rounded-xl border border-border/70 bg-card/60 p-3.5 shadow-2xs">
                <div class="flex items-center justify-between text-[11px] font-bold text-muted-foreground uppercase tracking-wider mb-2">
                  <span>能力速览概况</span>
                  <span v-if="(current.skills ?? []).length" class="text-primary font-mono lowercase text-[10.5px]">
                    {{ current.skills!.length }} 项掌握技能
                  </span>
                </div>
                <div class="flex flex-wrap items-center gap-2">
                  <template v-for="dim in numDims" :key="dim.key">
                    <span class="inline-flex items-center gap-1.5 rounded-lg border border-border/70 bg-muted/40 px-2.5 py-1 font-mono text-xs">
                      <span class="text-muted-foreground">{{ dim.label }}</span>
                      <b class="text-foreground font-extrabold">{{ current.attributes?.[dim.key] ?? (dim.baseline ?? 10) }}</b>
                    </span>
                  </template>
                  <span v-if="!numDims.length" class="text-xs text-muted-foreground/60">尚未配置数值属性</span>
                </div>
              </div>
            </div>
          </div>

          <!-- 怪物数据卡（图鉴 M1）：kind='monster' 用数据卡替代扮演向区块 -->
          <section v-if="isMonster" class="rounded-xl border border-warning/40 bg-warning/5 p-4 shadow-2xs">
            <div class="mb-3 flex flex-wrap items-center gap-2">
              <IconSwords class="size-4 shrink-0 text-warning" />
              <span class="font-serif text-base font-bold text-foreground">怪物数据卡</span>
              <Badge variant="outline" class="border-warning/40 bg-warning/10 px-1.5 font-mono text-[10.5px] text-warning">statblock</Badge>
              <span
                class="inline-flex items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-2 py-0.5 font-mono text-[11px] text-foreground/90"
                title="派生值 AC：由属性维度 + 修正来源实时算出；缺失时引擎回落 12"
              >
                <IconShield class="size-3.5 text-primary/70" />
                AC {{ acPreview?.value ?? 12 }}
              </span>
              <span class="text-[11px] text-muted-foreground/70">展示型字段：引擎不读，也不进提示词</span>
            </div>
            <FieldGrid dense>
              <FieldText
                label="生物类型 / 阵营"
                :model-value="current.statblock?.creatureType ?? ''"
                placeholder="如「中型 不死生物，中立邪恶」"
                @update:model-value="setStatField(current, 'creatureType', $event)"
              />
              <FieldText
                label="挑战等级 CR"
                :model-value="current.statblock?.challenge ?? ''"
                placeholder="如 1/4"
                @update:model-value="setStatField(current, 'challenge', $event)"
              />
              <FieldNum
                label="经验值 XP"
                :min="0"
                :model-value="current.statblock?.xp ?? 0"
                hint="仅展示（XP 合计由 Lua 在击败事件里做）"
                @update:model-value="setStatField(current, 'xp', String($event))"
              />
              <FieldArea
                label="特性 / 感官 / 免疫"
                :rows="3"
                :model-value="current.statblock?.traits ?? ''"
                placeholder="如「伤害免疫：毒素；感官：黑暗视觉 18m；语言：理解通用语但不能说」"
                @update:model-value="setStatField(current, 'traits', $event)"
              />
              <FieldArea
                label="攻击动作描述"
                :rows="3"
                :model-value="current.statblock?.actionsNote ?? ''"
                placeholder="攻击动作的文字描述（真正结算走下方「攻击技能」）"
                @update:model-value="setStatField(current, 'actionsNote', $event)"
              />
            </FieldGrid>

            <!-- 攻击技能绑定：只列 check.kind = 'attack' 的技能 -->
            <div class="mt-4 border-t border-warning/25 pt-3">
              <div class="mb-2 flex flex-wrap items-center gap-2">
                <IconTarget class="size-4 shrink-0 text-warning" />
                <span class="text-xs font-bold text-foreground">攻击技能</span>
                <Badge variant="outline" class="px-1.5 font-mono text-[10.5px] text-muted-foreground">
                  {{ boundAttackCount }} / {{ attackSkills.length }}
                </Badge>
                <span class="text-[11px] text-muted-foreground/70">绑定后引擎按这些技能结算怪物攻击（判定种类 check.kind = attack）</span>
              </div>
              <div
                v-if="!attackSkills.length"
                class="rounded-lg border border-dashed border-border/80 bg-muted/20 p-3 text-[11.5px] text-muted-foreground"
              >
                技能库里还没有攻击类技能。
                <button type="button" class="ml-1 cursor-pointer font-bold text-primary underline underline-offset-2" @click="editor.activeTab = 'skills'">
                  前往「技能」配置 check.kind = attack
                </button>
              </div>
              <div v-else class="flex flex-wrap gap-1.5">
                <button
                  v-for="s in attackSkills"
                  :key="s.id"
                  type="button"
                  class="inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11.5px] transition-colors"
                  :class="isSkillBound(current, s.id)
                    ? 'border-warning/60 bg-warning/15 font-semibold text-warning'
                    : 'border-border text-muted-foreground hover:bg-muted/60'"
                  :title="isSkillBound(current, s.id) ? '点击解绑' : '点击绑定为怪物的攻击技能'"
                  @click="toggleSkillBinding(current, s.id)"
                >
                  <IconSwords class="size-3" />
                  {{ s.name }}
                  <span class="font-mono text-[10px] opacity-70">{{ isSkillBound(current, s.id) ? '已绑定' : '+ 绑定' }}</span>
                </button>
              </div>
            </div>
          </section>

          <!-- 叙事档案区域：背景渊源与性格特质（扮演向；怪物隐藏） -->
          <div v-if="!isMonster" class="grid grid-cols-1 gap-3 pt-1 md:grid-cols-2">
            <div class="flex flex-col gap-2 rounded-xl border border-border/70 bg-card/60 p-3.5 shadow-2xs transition-colors hover:border-border">
              <div class="flex items-center gap-1.5">
                <IconBook2 class="size-4 shrink-0 text-primary" />
                <span class="text-xs font-bold text-foreground">背景渊源</span>
                <span class="font-mono text-[10.5px] text-muted-foreground/50">Background</span>
                <span class="ml-auto font-mono text-[10.5px] text-muted-foreground/50">{{ current.background?.length ?? 0 }} 字</span>
                <Button variant="ghost" size="icon-sm" class="size-6 text-muted-foreground/45 hover:text-primary" title="放大编辑" aria-label="放大编辑" @click="expandedField = 'background'">
                  <IconArrowsMaximize class="size-3.5" />
                </Button>
              </div>
              <textarea
                v-model="current.background"
                rows="5"
                placeholder="详细描述角色的出身、过往经历、隐秘身世与核心动机…"
                class="min-h-[8.5rem] w-full resize-y rounded-lg border border-border/60 bg-background/60 p-2.5 text-[13px] leading-relaxed text-foreground transition-colors placeholder:text-muted-foreground/40 hover:border-border focus:border-primary focus:bg-background focus:outline-none"
              ></textarea>
            </div>

            <div class="flex flex-col gap-2 rounded-xl border border-border/70 bg-card/60 p-3.5 shadow-2xs transition-colors hover:border-border">
              <div class="flex items-center gap-1.5">
                <IconSparkles class="size-4 shrink-0 text-warning" />
                <span class="text-xs font-bold text-foreground">性格特质</span>
                <span class="font-mono text-[10.5px] text-muted-foreground/50">Personality</span>
                <span class="ml-auto font-mono text-[10.5px] text-muted-foreground/50">{{ current.personality?.length ?? 0 }} 字</span>
                <Button variant="ghost" size="icon-sm" class="size-6 text-muted-foreground/45 hover:text-primary" title="放大编辑" aria-label="放大编辑" @click="expandedField = 'personality'">
                  <IconArrowsMaximize class="size-3.5" />
                </Button>
              </div>
              <textarea
                v-model="current.personality"
                rows="5"
                placeholder="描述角色的行为习惯、言谈举止风格、处事准则与心理弱点…"
                class="min-h-[8.5rem] w-full resize-y rounded-lg border border-border/60 bg-background/60 p-2.5 text-[13px] leading-relaxed text-foreground transition-colors placeholder:text-muted-foreground/40 hover:border-border focus:border-primary focus:bg-background focus:outline-none"
              ></textarea>
            </div>
          </div>

          <!-- 对话示例（发给 AI 的风格样板）与作者注释（只给人看） -->
          <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
            <div v-if="!isMonster" class="flex flex-col gap-2 rounded-xl border border-border/70 bg-card/60 p-3.5 shadow-2xs transition-colors hover:border-border">
              <div class="flex items-center gap-1.5">
                <IconMessageChatbot class="size-4 shrink-0 text-primary" />
                <span class="text-xs font-bold text-foreground">对话示例</span>
                <span class="font-mono text-[10.5px] text-muted-foreground/50">Example Dialogues</span>
                <span class="ml-auto font-mono text-[10.5px] text-muted-foreground/50">{{ current.example_dialogues?.length ?? 0 }} 字</span>
                <Button variant="ghost" size="icon-sm" class="size-6 text-muted-foreground/45 hover:text-primary" title="放大编辑" aria-label="放大编辑" @click="expandedField = 'example_dialogues'">
                  <IconArrowsMaximize class="size-3.5" />
                </Button>
              </div>
              <textarea
                v-model="current.example_dialogues"
                rows="5"
                placeholder="3-5 轮示范该角色口吻的对话，AI 会模仿语气与句式（最有效的风格控制）…"
                class="min-h-[8.5rem] w-full resize-y rounded-lg border border-border/60 bg-background/60 p-2.5 text-[13px] leading-relaxed text-foreground transition-colors placeholder:text-muted-foreground/40 hover:border-border focus:border-primary focus:bg-background focus:outline-none"
              ></textarea>
            </div>

            <div
              class="flex flex-col gap-2 rounded-xl border border-dashed border-border/70 bg-muted/20 p-3.5 shadow-2xs transition-colors hover:border-border"
              :class="isMonster && 'md:col-span-2'"
            >
              <div class="flex items-center gap-1.5">
                <IconNotes class="size-4 shrink-0 text-muted-foreground" />
                <span class="text-xs font-bold text-foreground">作者注释</span>
                <span class="font-mono text-[10.5px] text-muted-foreground/50">Creator's Notes</span>
                <span class="ml-auto inline-flex items-center gap-1 rounded-full bg-muted/50 px-2 py-0.5 text-[10px] text-muted-foreground"><IconEyeOff class="size-3" />不发给 AI</span>
                <Button variant="ghost" size="icon-sm" class="size-6 text-muted-foreground/45 hover:text-primary" title="放大编辑" aria-label="放大编辑" @click="expandedField = 'notes'">
                  <IconArrowsMaximize class="size-3.5" />
                </Button>
              </div>
              <textarea
                v-model="current.notes"
                rows="5"
                placeholder="给使用者的备注：灵感来源、使用建议、已知问题…（不会发给 AI）"
                class="min-h-[8.5rem] w-full resize-y rounded-lg border border-border/60 bg-background/60 p-2.5 text-[13px] leading-relaxed text-foreground transition-colors placeholder:text-muted-foreground/40 hover:border-border focus:border-primary focus:bg-background focus:outline-none"
              ></textarea>
            </div>
          </div>

          <!-- 好卡自检（借鉴 SillyTavern 角色卡指南）：衡量扮演向字段，怪物不适用 -->
          <div v-if="!isMonster" class="flex flex-wrap items-center gap-x-4 gap-y-1.5 rounded-xl border border-border/60 bg-muted/20 px-3.5 py-2.5">
            <span class="flex items-center gap-1.5 text-[11px] font-bold text-muted-foreground">
              <IconCircleCheck class="size-3.5" :class="writingScore === writingChecks.length ? 'text-primary' : 'text-warning'" />
              好卡自检 {{ writingScore }}/{{ writingChecks.length }}
            </span>
            <span v-for="chk in writingChecks" :key="chk.label" class="flex items-center gap-1 text-[11px]" :class="chk.ok ? 'text-muted-foreground' : 'text-warning'" :title="chk.hint">
              <IconCircleCheck v-if="chk.ok" class="size-3.5 text-primary" />
              <IconAlertTriangle v-else class="size-3.5" />
              {{ chk.label }}
            </span>
          </div>

          <!-- 叙事档案放大编辑弹窗 -->
          <Dialog :open="!!expandedField" @update:open="(v: boolean) => { if (!v) expandedField = null }">
            <DialogContent v-if="activeNarrative" class="sm:max-w-2xl gap-0 p-0">
              <DialogHeader class="border-b border-border/70 px-5 py-4">
                <DialogTitle class="flex items-center gap-2 text-[15px]">
                  <IconBook2 v-if="expandedField === 'background'" class="size-4 text-primary" />
                  <IconMessageChatbot v-else-if="expandedField === 'example_dialogues'" class="size-4 text-primary" />
                  <IconNotes v-else-if="expandedField === 'notes'" class="size-4 text-muted-foreground" />
                  <IconSparkles v-else class="size-4 text-warning" />
                  {{ activeNarrative.title }}
                  <span class="font-mono text-[11px] font-normal text-muted-foreground/60">{{ activeNarrative.en }}</span>
                </DialogTitle>
                <DialogDescription class="text-[11.5px] leading-5">{{ activeNarrative.hint }}</DialogDescription>
              </DialogHeader>
              <div class="px-5 py-4">
                <textarea
                  v-model="expandedText"
                  :placeholder="activeNarrative.hint"
                  class="min-h-[48vh] w-full resize-none rounded-lg border border-border bg-background p-3 text-[13px] leading-[1.8] text-foreground transition-colors placeholder:text-muted-foreground/40 focus-visible:border-primary focus-visible:ring-2 focus-visible:ring-primary/20 focus-visible:outline-none"
                ></textarea>
              </div>
              <DialogFooter class="border-t border-border/70 px-5 py-3 sm:justify-between">
                <span class="font-mono text-[11px] text-muted-foreground/60">{{ expandedText.length }} 字 · 自动保存</span>
                <Button size="sm" @click="expandedField = null">完成</Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>

          <!-- 核心属性维度面板（Game Attributes Panel） -->
          <div class="pt-3 border-t border-border/70">
            <div class="mb-3 flex items-center justify-between">
              <div class="flex items-center gap-2">
                <IconSwords class="size-4 text-primary" />
                <span class="font-serif text-base font-bold text-foreground">核心属性维度</span>
                <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">
                  {{ dims.length }} 项已配置
                </Badge>
              </div>
              <div class="flex items-center gap-3">
                <button
                  type="button"
                  class="inline-flex cursor-pointer items-center gap-1 border-b border-dashed border-border/70 text-xs text-muted-foreground transition-colors hover:border-primary/60 hover:text-primary"
                  title="声明卡面分区：分区的标题与内容由故事书声明"
                  @click="sheetOpen = true"
                >
                  <span>卡面</span>
                  <IconLayoutGrid class="size-3.5" />
                </button>
                <button
                  type="button"
                  class="inline-flex cursor-pointer items-center gap-1 border-b border-dashed border-border/70 text-xs text-muted-foreground transition-colors hover:border-primary/60 hover:text-primary"
                  title="声明派生值：由属性维度 + 修正来源算出的调整值 / 豁免 / AC / 法术DC"
                  @click="derivedOpen = true"
                >
                  <span>派生值</span>
                  <IconDice class="size-3.5" />
                </button>
                <button
                  type="button"
                  class="inline-flex cursor-pointer items-center gap-1 border-b border-dashed border-border/70 text-xs text-muted-foreground transition-colors hover:border-primary/60 hover:text-primary"
                  title="去「维度设置」管理全局属性"
                  @click="editor.activeTab = 'dimensions'"
                >
                  <span>配置全局维度</span>
                  <IconArrowUpRight class="size-3.5" />
                </button>
              </div>
            </div>

            <!-- 尚无维度提示 -->
            <div
              v-if="!dims.length"
              class="rounded-xl border border-dashed border-border/80 bg-muted/20 p-5 text-center text-xs text-muted-foreground"
            >
              故事书尚未定义全局属性维度。
              <button
                type="button"
                class="ml-1 cursor-pointer font-bold text-primary underline underline-offset-2"
                @click="editor.activeTab = 'dimensions'"
              >
                前往「维度设置」新增
              </button>
            </div>

            <div v-else class="space-y-3">
              <!-- 数值属性格栅：自适应列数，6项属性在桌面上一行完美铺展！ -->
              <div v-if="numDims.length" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-3">
                <div
                  v-for="dim in numDims"
                  :key="dim.key"
                  class="group flex flex-col justify-between rounded-xl border border-border/70 bg-card/70 p-3 transition-all hover:border-primary/50 hover:bg-card hover:shadow-2xs"
                >
                  <div class="flex items-center justify-between gap-1">
                    <span class="truncate text-xs font-bold text-foreground" :title="dim.label">
                      {{ dim.label }}
                    </span>
                    <span class="font-mono text-[10px] text-muted-foreground uppercase">{{ dim.key }}</span>
                  </div>

                  <!-- 大数字输入框 -->
                  <div class="my-2 flex items-center justify-center gap-1.5">
                    <input
                      type="number"
                      :value="typeof current.attributes?.[dim.key] === 'number' ? current.attributes[dim.key] : (dim.baseline ?? 10)"
                      class="w-full cursor-pointer rounded-md border border-dashed border-border bg-muted/25 py-0.5 text-center font-mono text-2xl font-black text-foreground transition-colors select-all [appearance:textfield] hover:border-solid hover:border-primary/60 hover:bg-primary/5 focus-visible:cursor-text focus-visible:border-solid focus-visible:border-primary focus-visible:bg-background focus-visible:ring-2 focus-visible:ring-primary/25 focus-visible:outline-none [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                      title="点击数字直接编辑"
                      @input="setAttr(current, dim.key, Number(($event.target as HTMLInputElement).value))"
                    />
                    <IconPencil class="size-3.5 shrink-0 text-muted-foreground/45 transition-colors group-hover:text-primary/70" />
                  </div>

                  <!-- 基准与微型计量槽 -->
                  <div class="space-y-1.5">
                    <div class="flex items-center justify-between gap-1 text-[10.5px] font-mono text-muted-foreground">
                      <span>
                        基准 {{ dim.baseline ?? 10 }}
                        <span
                          v-if="effectiveAttr(dim) !== statVal(current, dim.key, dim.baseline ?? 10)"
                          class="text-primary/70"
                          title="含修正来源（挂接定义 / 装备）后的有效值"
                        >→ {{ effectiveAttr(dim) }}</span>
                      </span>
                      <span class="flex items-baseline gap-1">
                        <span
                          v-if="statDiff(statVal(current, dim.key, dim.baseline ?? 10), dim.baseline ?? 10).diff !== 0"
                          class="text-muted-foreground/40"
                          title="相对基线的偏移（不是判定修正）"
                        >{{ signed(statDiff(statVal(current, dim.key, dim.baseline ?? 10), dim.baseline ?? 10).diff) }}</span>
                        <span
                          class="font-bold"
                          :class="modTone(ruleModifier(dim, effectiveAttr(dim)))"
                          title="判定修正（与引擎同款，基于有效值）"
                        >修正 {{ signed(ruleModifier(dim, effectiveAttr(dim))) }}</span>
                      </span>
                    </div>
                    <div class="h-1 w-full overflow-hidden rounded-full bg-muted">
                      <div
                        class="h-full rounded-full transition-all duration-300"
                        :class="gaugeClass(statVal(current, dim.key, dim.baseline ?? 10), dim.baseline ?? 10)"
                        :style="{ width: statPct(statVal(current, dim.key, dim.baseline ?? 10)) + '%' }"
                      ></div>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 枚举与文本等补充特质格栅 -->
              <div v-if="otherDims.length" class="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 gap-3 pt-1">
                <template v-for="dim in otherDims" :key="dim.key">
                  <!-- 枚举特质卡（如阵营、性格取向） -->
                  <div
                    v-if="dim.type === 'enum'"
                    class="flex flex-col justify-between rounded-xl border border-border/70 bg-card/70 p-3 transition-all hover:border-primary/50 hover:bg-card hover:shadow-2xs"
                  >
                    <div class="flex items-center justify-between gap-1 mb-1.5">
                      <span class="truncate text-xs font-bold text-foreground" :title="dim.label">
                        {{ dim.label }}
                      </span>
                      <span class="font-mono text-[10px] text-muted-foreground/70">枚举特质</span>
                    </div>
                    <div>
                      <Select
                        :model-value="String(current.attributes?.[dim.key] ?? '')"
                        @update:model-value="setAttr(current, dim.key, String($event ?? ''))"
                      >
                        <SelectTrigger class="h-8 w-full text-xs font-semibold">
                          <SelectValue :placeholder="String(current.attributes?.[dim.key] ?? '') === '' ? '（未选）' : undefined" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem v-for="o in dim.options ?? []" :key="o" :value="o">{{ o }}</SelectItem>
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>

                  <!-- 文本特质卡 -->
                  <div
                    v-else
                    class="flex flex-col justify-between rounded-xl border border-border/70 bg-card/70 p-3 transition-all hover:border-primary/50 hover:bg-card hover:shadow-2xs"
                  >
                    <div class="flex items-center justify-between gap-1 mb-1.5">
                      <span class="truncate text-xs font-bold text-foreground" :title="dim.label">
                        {{ dim.label }}
                      </span>
                      <span class="font-mono text-[10px] text-muted-foreground/70">特质文本</span>
                    </div>
                    <div>
                      <Input
                        class="h-8 w-full text-xs"
                        :model-value="String(current.attributes?.[dim.key] ?? '')"
                        placeholder="自定义特质标签…"
                        @update:model-value="setAttr(current, dim.key, String($event))"
                      />
                    </div>
                  </div>
                </template>
              </div>
            </div>
          </div>

          <!-- 派生值（#2）：属性维度 + 修正来源 → 调整值 / 豁免 / AC / 法术DC / 被动察觉 -->
          <div v-if="derivedValues.length" class="pt-3 border-t border-border/70">
            <div class="mb-3 flex items-center gap-2">
              <IconDice class="size-4 text-primary" />
              <span class="font-serif text-base font-bold text-foreground">派生值</span>
              <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">{{ derivedValues.length }} 项</Badge>
              <span class="text-[11px] text-muted-foreground/60">由属性维度 + 修正来源算出</span>
            </div>
            <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-6">
              <div
                v-for="dv in derivedValues"
                :key="dv.key"
                class="rounded-xl border border-border/70 bg-card/50 p-3 text-center"
                :title="dv.error ?? (dv.key + ' = ' + (sbDerived.find(x => x.key === dv.key)?.formula ?? ''))"
              >
                <div class="truncate text-xs font-bold text-foreground">{{ dv.label }}</div>
                <div class="my-1 font-mono text-xl font-bold" :class="dv.value == null ? 'text-destructive' : 'text-primary'">
                  {{ dv.value == null ? '—' : (dv.signed && dv.value >= 0 ? '+' + dv.value : dv.value) }}
                </div>
                <div class="truncate font-mono text-[10px] text-muted-foreground/60">{{ dv.key }}</div>
              </div>
            </div>
          </div>

          <!-- 派生值编辑器（故事书全局） -->
          <Dialog v-model:open="derivedOpen">
            <DialogContent class="sm:max-w-5xl gap-0 p-0">
              <DialogHeader class="border-b border-border/70 px-5 py-4">
                <DialogTitle class="flex items-center gap-2 text-[15px]">
                  <IconDice class="size-4 text-primary" />
                  派生值
                  <Badge variant="outline" class="ml-1 px-1.5 font-mono text-[10.5px] font-normal text-muted-foreground">{{ sbDerived.length }} 项</Badge>
                </DialogTitle>
                <DialogDescription class="text-[11.5px] leading-5">
                  由属性维度与修正来源算出的二级数值。公式可引用属性维度 key、前面已声明的派生 key，以及 floor / ceil / round / min / max / abs。
                </DialogDescription>
              </DialogHeader>

              <div class="max-h-[58vh] overflow-y-auto px-5">
                <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,2fr)_4rem_1.75rem] items-center gap-2 border-b border-border/60 py-2 text-[10px] font-bold tracking-wider text-muted-foreground/55 uppercase">
                  <span>key</span>
                  <span>显示名</span>
                  <span>公式</span>
                  <span class="text-right">当前值</span>
                  <span></span>
                </div>
                <div
                  v-for="(dv, i) in sbDerived"
                  :key="i"
                  class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,2fr)_4rem_1.75rem] items-center gap-2 border-b border-border/40 py-1.5 last:border-0"
                >
                  <Input class="h-8 font-mono text-xs" :model-value="dv.key" @update:model-value="dv.key = String($event)" />
                  <Input class="h-8 text-[13px]" :model-value="dv.label" @update:model-value="dv.label = String($event)" />
                  <Input class="h-8 font-mono text-xs" :model-value="dv.formula" @update:model-value="dv.formula = String($event)" />
                  <span class="text-right font-mono text-[12px] font-bold" :class="derivedPreview(dv.key).tone">{{ derivedPreview(dv.key).text }}</span>
                  <Button variant="ghost" size="icon-sm" class="size-7 shrink-0 text-muted-foreground/45 hover:bg-destructive/10 hover:text-destructive" aria-label="删除派生值" @click="removeDerived(i)">
                    <IconTrash class="size-3.5" />
                  </Button>
                </div>
                <p v-if="!sbDerived.length" class="py-8 text-center text-xs text-muted-foreground/60">还没有派生值。</p>
              </div>

              <div class="flex flex-wrap items-center gap-2 border-t border-border/70 px-5 py-3">
                <Button size="sm" class="h-7 gap-1 px-2.5 text-xs" @click="addDerived">
                  <IconPlus class="size-3.5" />
                  新增派生值
                </Button>
                <span class="text-[11px] text-muted-foreground/60">
                  例 <code class="rounded bg-muted px-1 font-mono text-[10.5px]">floor((int - 10) / 2)</code>
                  <code class="ml-1 rounded bg-muted px-1 font-mono text-[10.5px]">8 + prof + int_mod</code>
                </span>
              </div>
            </DialogContent>
          </Dialog>

          <!-- 卡面分区（#6）：分区的标题与内容由故事书声明 -->
          <div v-if="sheetSections.length" class="pt-3 border-t border-border/70">
            <div class="mb-3 flex items-center gap-2">
              <IconLayoutGrid class="size-4 text-primary" />
              <span class="font-serif text-base font-bold text-foreground">卡面</span>
              <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">{{ sheetSections.length }} 区</Badge>
              <span class="text-[11px] text-muted-foreground/60">由故事书声明</span>
            </div>
            <div class="divide-y divide-border/60">
              <section v-for="(sec, si) in sheetSections" :key="si" class="py-3 first:pt-1">
                <div class="mb-2 text-[12.5px] font-bold text-foreground">{{ sec.title }}</div>
                <div class="grid grid-cols-1 gap-x-4 gap-y-3.5 @2xl:grid-cols-2">
                  <div v-for="k in sec.kinds ?? []" :key="k" class="space-y-1.5">
                    <div class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">{{ kinds.find(x => x.key === k)?.label ?? k }}</div>

                    <!-- 无字段的种类（如语言）：一行列名，不做盒子 -->
                    <p v-if="!kindFields(k).length && attachedDefs(current, k).length" class="text-[12.5px] text-foreground/85">{{ attachedNames(k) }}</p>

                    <!-- 有字段：名称 + 左侧竖线 + 字段网格 -->
                    <div
                      v-for="def in kindFields(k).length ? attachedDefs(current, k) : []"
                      :key="def.id"
                      class="space-y-0.5 border-l-2 border-border/70 pl-2.5"
                    >
                      <div class="text-[12.5px] font-semibold text-foreground">{{ def.name }}</div>
                      <dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-0.5">
                        <template v-for="f in visibleFields(k, def)" :key="f.key">
                          <dt class="whitespace-nowrap text-[10.5px] text-muted-foreground/60">{{ f.label || f.key }}</dt>
                          <dd class="min-w-0 break-words text-[11.5px] text-foreground/85">{{ fieldValueText(k, f.key, def.fields?.[f.key]) }}</dd>
                        </template>
                      </dl>
                    </div>

                    <span v-if="!attachedDefs(current, k).length" class="text-[11px] text-muted-foreground/50">未挂接</span>
                  </div>

                  <div v-if="(sec.derived ?? []).length" class="space-y-1.5">
                    <div class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">派生值</div>
                    <div class="flex flex-wrap gap-1.5">
                      <span v-for="dk in sec.derived" :key="dk" class="inline-flex items-baseline gap-1.5 rounded-full bg-muted/25 px-2.5 py-1 text-[11.5px]">
                        <span class="text-muted-foreground/70">{{ derivedOf(dk)?.label ?? dk }}</span>
                        <span class="font-mono font-bold text-primary">{{ derivedText(dk) }}</span>
                      </span>
                    </div>
                  </div>

                  <div v-if="(sec.resources ?? []).length" class="space-y-1.5">
                    <div class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">资源</div>
                    <div class="flex flex-wrap gap-1.5">
                      <span v-for="rid in sec.resources" :key="rid" class="inline-flex items-baseline gap-1.5 rounded-full bg-muted/25 px-2.5 py-1 text-[11.5px]">
                        <span class="text-muted-foreground/70">{{ resourceOf(rid)?.name ?? rid }}</span>
                        <span class="font-mono font-bold text-primary">{{ resourceOf(rid)?.cur ?? 0 }}<span v-if="resourceOf(rid)?.max != null" class="font-normal text-muted-foreground/50">/{{ resourceOf(rid)?.max }}</span></span>
                      </span>
                    </div>
                  </div>

                  <div v-if="(sec.narrative ?? []).length" class="col-span-full space-y-2">
                    <div v-for="nk in sec.narrative" :key="nk" class="space-y-1">
                      <div class="text-[10.5px] font-bold tracking-wider text-muted-foreground/55 uppercase">{{ narrativeLabel(nk) }}</div>
                      <p class="text-[11.5px] leading-relaxed whitespace-pre-wrap text-foreground/85">{{ narrativeOf(current, nk) }}</p>
                    </div>
                  </div>
                </div>
              </section>
            </div>
          </div>

          <!-- 卡面分区编辑器（故事书全局） -->
          <Dialog v-model:open="sheetOpen">
            <DialogContent class="sm:max-w-4xl gap-0 p-0">
              <DialogHeader class="border-b border-border/70 px-5 py-4">
                <DialogTitle class="flex items-center gap-2 text-[15px]">
                  <IconLayoutGrid class="size-4 text-primary" />
                  卡面分区
                  <Badge variant="outline" class="ml-1 px-1.5 font-mono text-[10.5px] font-normal text-muted-foreground">{{ sheetSections.length }} 区</Badge>
                </DialogTitle>
                <DialogDescription class="text-[11.5px] leading-5">
                  声明角色卡的展示结构：每区放哪些「挂接种类 / 派生值 / 资源 / 叙事字段」。直接点标签选择，再点一次取消；左侧序号就是它在卡面上的显示顺序。
                </DialogDescription>
              </DialogHeader>

              <div class="max-h-[58vh] space-y-3 overflow-y-auto px-5 py-4">
                <div v-for="(sec, i) in sheetSections" :key="i" class="overflow-hidden rounded-xl border border-border/70 bg-card/40">
                  <div class="flex items-center gap-2 border-b border-border/60 bg-muted/20 px-3 py-2">
                    <span class="flex size-5 shrink-0 items-center justify-center rounded-md bg-primary/10 font-mono text-[10.5px] font-bold text-primary">{{ i + 1 }}</span>
                    <label class="flex min-w-0 flex-1 items-center gap-1.5">
                      <span class="flex shrink-0 items-center gap-1 text-[10.5px] font-medium text-muted-foreground/70">
                        <IconPencil class="size-3.5" />
                        标题
                      </span>
                      <Input
                        class="h-8 flex-1 text-[13px] font-semibold"
                        :model-value="sec.title"
                        placeholder="如「战斗数据」"
                        @update:model-value="sec.title = String($event)"
                      />
                    </label>
                    <Button variant="ghost" size="icon-sm" class="size-7 shrink-0 text-muted-foreground/45 hover:bg-destructive/10 hover:text-destructive" aria-label="删除分区" @click="removeSheetSection(i)">
                      <IconTrash class="size-3.5" />
                    </Button>
                  </div>
                  <div class="space-y-2 px-3 py-2.5">
                    <div v-for="g in sheetFieldGroups" :key="g.key" class="flex items-start gap-2">
                      <span class="mt-1 w-20 shrink-0 text-[11px] text-muted-foreground">{{ g.label }}</span>
                      <div class="flex flex-wrap gap-1.5">
                        <button
                          v-for="opt in g.options"
                          :key="opt.value"
                          type="button"
                          class="cursor-pointer rounded-full border px-2 py-0.5 text-[11px] transition-colors"
                          :class="(sec[g.key] ?? []).includes(opt.value)
                            ? 'border-primary/50 bg-primary/10 text-primary'
                            : 'border-border text-muted-foreground hover:bg-muted/60'"
                          @click="toggleSheetValue(sec, g.key, opt.value)"
                        >{{ opt.label }}</button>
                        <button
                          v-for="ex in extraValues(sec, g.key, g.options)"
                          :key="'x-' + ex"
                          type="button"
                          class="cursor-pointer rounded-full border border-warning/50 bg-warning/10 px-2 py-0.5 text-[11px] text-warning"
                          :title="'该值已不在可选项里（可能被删）——点一下移除'"
                          @click="toggleSheetValue(sec, g.key, ex)"
                        >{{ ex }} ×</button>
                        <span v-if="!g.options.length && !extraValues(sec, g.key, g.options).length" class="text-[11px] text-muted-foreground/50">无可选项</span>
                      </div>
                    </div>
                  </div>
                </div>
                <p v-if="!sheetSections.length" class="py-8 text-center text-xs text-muted-foreground/60">还没有分区。</p>
              </div>

              <div class="flex flex-wrap items-center gap-2 border-t border-border/70 px-5 py-3">
                <Button size="sm" class="h-7 gap-1 px-2.5 text-xs" @click="addSheetSection">
                  <IconPlus class="size-3.5" />
                  新增分区
                </Button>
                <span class="text-[11px] text-muted-foreground/60">点标签选择内容；同一项再点一次取消</span>
              </div>
            </DialogContent>
          </Dialog>

          <!-- 开放种类挂接：种族/职业/背景/特性/熟练项… 把「扩展」里定义的内容接到人物上 -->
          <div v-if="attachableKinds.length" class="pt-3 border-t border-border/70">
            <div class="mb-3 flex items-center gap-2">
              <IconSparkles class="size-4 text-primary" />
              <span class="font-serif text-base font-bold text-foreground">开放挂接</span>
              <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">{{ attachableKinds.length }} 类</Badge>
              <span class="text-[11px] text-muted-foreground/60">引用「扩展 → 开放内容」里的定义</span>
            </div>
            <div class="grid grid-cols-1 gap-3 @2xl:grid-cols-2">
              <div v-for="k in attachableKinds" :key="k.key" class="rounded-xl border border-border/70 bg-card/40 p-3">
                <div class="mb-2 flex items-center gap-2">
                  <span class="text-[12.5px] font-semibold text-foreground">{{ k.label }}</span>
                  <span class="font-mono text-[10.5px] text-muted-foreground/60">{{ k.key }}</span>
                  <div class="ml-auto">
                    <DropdownMenu v-if="defsOfKind(k.key).length">
                      <DropdownMenuTrigger as-child>
                        <Button variant="ghost" size="sm" class="h-7 gap-1 text-xs text-muted-foreground">
                          <IconPlus class="size-3.5" /> 挂接
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" class="max-h-72 w-56 overflow-y-auto">
                        <DropdownMenuLabel>选择「{{ k.label }}」定义</DropdownMenuLabel>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          v-for="def in defsOfKind(k.key)"
                          :key="def.id"
                          :disabled="attachmentsOf(current, k.key).includes(def.id)"
                          @click="attachDefinition(current, k.key, def.id)"
                        >
                          {{ def.name }}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                    <span v-else class="text-[10.5px] text-muted-foreground/50">尚无定义</span>
                  </div>
                </div>
                <div class="flex flex-wrap gap-1.5">
                  <span
                    v-for="id in attachmentsOf(current, k.key)"
                    :key="id"
                    class="inline-flex items-center gap-1 rounded-full border border-primary/40 bg-primary/10 px-2 py-0.5 text-[11px] text-primary"
                  >
                    {{ defNameOf(id) }}
                    <button type="button" class="cursor-pointer text-primary/60 transition-colors hover:text-primary" title="移除" @click="detachDefinition(current, k.key, id)">×</button>
                  </span>
                  <span v-if="!attachmentsOf(current, k.key).length" class="text-[11px] text-muted-foreground/50">未挂接</span>
                </div>
              </div>
            </div>
          </div>

          <!-- 物品栏：开档时进背包，use_item 按此校验持有（#01） -->
          <div class="pt-3 border-t border-border/70">
            <div class="mb-3 flex items-center justify-between gap-2">
              <div class="flex items-center gap-2">
                <IconPackage class="size-4 text-primary" />
                <span class="font-serif text-base font-bold text-foreground">物品栏</span>
                <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">
                  {{ current.inventory?.length ?? 0 }} 项
                </Badge>
              </div>
              <DropdownMenu v-if="unassignedItems.length">
                <DropdownMenuTrigger as-child>
                  <Button variant="ghost" size="sm" class="h-7 gap-1 text-xs text-muted-foreground">
                    <IconPlus class="size-3.5" /> 添加物品
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="max-h-72 w-56 overflow-y-auto">
                  <DropdownMenuLabel>选择要放入背包的物品</DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem v-for="it in unassignedItems" :key="it.id" @click="bindItemToChar(current, it.id)">
                    {{ it.name }}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <p v-if="!itemOptions.length" class="rounded-xl border border-dashed border-border/80 bg-muted/20 p-4 text-center text-xs text-muted-foreground">
              故事书尚未定义物品。
              <button type="button" class="ml-1 cursor-pointer font-bold text-primary underline underline-offset-2" @click="editor.activeTab = 'items'">前往「物品」新增</button>
            </p>
            <div v-else class="space-y-2">
              <div v-for="(entry, i) in current.inventory ?? []" :key="i" class="flex items-center gap-2">
                <Select :model-value="entry.id" @update:model-value="current.inventory![i].id = String($event ?? '')">
                  <SelectTrigger class="h-8 w-full flex-1 text-[13px]"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem v-for="o in itemOptions" :key="o.value" :value="o.value">{{ o.label }}</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
                <Badge v-if="itemOf(entry.id)?.slot" variant="outline" class="shrink-0 border-border/70 px-1.5 text-[10px] text-muted-foreground">{{ itemOf(entry.id)?.slot }}</Badge>
                <Input type="number" class="h-8 w-20 shrink-0 text-right font-mono text-xs" :model-value="entry.quantity ?? 1" @update:model-value="current.inventory![i].quantity = Number($event)" />
                <Button
                  v-if="itemOf(entry.id)?.slot"
                  variant="ghost"
                  size="icon-sm"
                  class="size-8 shrink-0 transition-colors"
                  :class="isEquipped(current, entry.id) ? 'text-primary hover:bg-primary/10' : 'text-muted-foreground/60 hover:bg-muted/60'"
                  :title="isEquipped(current, entry.id) ? '卸下' : '装备'"
                  :aria-label="isEquipped(current, entry.id) ? '卸下' : '装备'"
                  @click="toggleEquipped(current, entry.id)"
                >
                  <IconShield class="size-3.5" :fill="isEquipped(current, entry.id) ? 'currentColor' : 'none'" />
                </Button>
                <Button variant="ghost" size="icon-sm" class="size-8 shrink-0 text-muted-foreground/60 hover:bg-destructive/10 hover:text-destructive" aria-label="移除物品" @click="current.inventory!.splice(i, 1)">
                  <IconX class="size-3.5" />
                </Button>
              </div>
              <p v-if="!(current.inventory?.length ?? 0)" class="text-xs text-muted-foreground/60">背包为空，点右上「添加物品」放入。</p>
            </div>
          </div>

          <!-- 初始资源配置（如果故事书定义了资源如 HP、MP、体力、金币） -->
          <div v-if="worldResources.length" class="pt-3 border-t border-border/70">
            <div class="mb-2.5 flex items-center justify-between gap-2">
              <div class="flex items-center gap-2">
                <IconCoins class="size-4 text-warning" />
                <span class="font-serif text-base font-bold text-foreground">初始资源槽位</span>
                <span class="text-xs text-muted-foreground/70 hidden sm:inline">（开档时注入该角色的初始数值）</span>
              </div>
              <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">
                {{ worldResources.length }} 项资源
              </Badge>
            </div>
            <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
              <div
                v-for="r in worldResources"
                :key="r.id"
                class="flex items-center justify-between rounded-xl border border-border/70 bg-card/60 px-3 py-2 transition-all hover:border-primary/40 hover:bg-card/90"
              >
                <div class="min-w-0 pr-2">
                  <div class="truncate text-xs font-bold text-foreground" :title="r.name">{{ r.name }}</div>
                  <div class="font-mono text-[10px] text-muted-foreground/70">{{ r.id }}</div>
                </div>
                <Input
                  type="number"
                  class="h-7 w-20 text-right font-mono text-xs font-bold"
                  :model-value="current.resources?.[r.id] ?? r.default_max ?? 0"
                  @update:model-value="setResource(current, r.id, Number($event))"
                />
              </div>
            </div>
          </div>

          <!-- 每日准备法术（#4）：从已掌握技能里挑选；长休后重挑（扮演向，怪物隐藏） -->
          <div v-if="!isMonster && (current.skills?.length ?? 0)" class="pt-3 border-t border-border/70">
            <div class="mb-3 flex items-center gap-2">
              <IconBook2 class="size-4 text-primary" />
              <span class="font-serif text-base font-bold text-foreground">每日准备</span>
              <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">{{ current.prepared?.length ?? 0 }} / {{ current.skills?.length ?? 0 }}</Badge>
              <span class="text-[11px] text-muted-foreground/60">长休后重新挑选；点选切换</span>
            </div>
            <div class="flex flex-wrap gap-1.5">
              <button
                v-for="sid in current.skills ?? []"
                :key="sid"
                type="button"
                class="cursor-pointer rounded-full border px-2.5 py-1 text-[11.5px] transition-colors"
                :class="isPrepared(current, sid) ? 'border-primary/50 bg-primary/10 text-primary' : 'border-border text-muted-foreground hover:bg-muted/60'"
                @click="togglePrepared(current, sid)"
              >
                {{ getSkill(sid)?.name ?? sid }}
              </button>
            </div>
          </div>

          <!-- 掌握技能：RPG 游戏技能卡（Skill Cards） -->
          <div class="pt-3 border-t border-border/70 space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex items-center gap-2">
                <IconSparkles class="size-4 text-primary" />
                <span class="font-serif text-base font-bold text-foreground">掌握技能</span>
                <Badge variant="outline" class="font-mono text-xs px-2 text-muted-foreground">
                  {{ current.skills?.length ?? 0 }} 项
                </Badge>
              </div>

              <!-- 右侧快捷操作：跳转技能库 + 绑定新技能下拉 -->
              <div class="flex items-center gap-2">
                <button
                  type="button"
                  class="inline-flex cursor-pointer items-center gap-1 border-b border-dashed border-border/70 text-xs text-muted-foreground transition-colors hover:border-primary/60 hover:text-primary"
                  title="去「技能与物品」管理故事书全局技能定义"
                  @click="editor.activeTab = 'skills'"
                >
                  <span>技能库</span>
                  <IconArrowUpRight class="size-3.5" />
                </button>

                <!-- 绑定技能下拉菜单 -->
                <DropdownMenu v-if="availableSkills.length">
                  <DropdownMenuTrigger as-child>
                    <Button
                      variant="outline"
                      size="sm"
                      class="h-7 gap-1.5 text-xs font-semibold hover:border-primary/50"
                      :disabled="unassignedSkills.length === 0"
                    >
                      <IconPlus class="size-3.5 text-primary" />
                      <span>{{ unassignedSkills.length === 0 ? '已掌握全部技能' : '绑定技能' }}</span>
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" class="w-64 max-h-72 overflow-y-auto p-1">
                    <DropdownMenuLabel class="text-xs text-muted-foreground px-2 py-1.5 font-normal">
                      选择要赋予「{{ current.name }}」的技能
                    </DropdownMenuLabel>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      v-for="s in unassignedSkills"
                      :key="s.id"
                      class="cursor-pointer flex items-center justify-between gap-2 p-2 rounded-lg hover:bg-muted/70 transition-colors"
                      @click="bindSkillToChar(current, s.id)"
                    >
                      <div class="min-w-0 flex-1">
                        <div class="flex items-center gap-1.5">
                          <span class="text-xs font-bold text-foreground truncate">{{ s.name }}</span>
                          <span class="text-[10px] font-mono text-muted-foreground/70">{{ s.id }}</span>
                        </div>
                        <div class="text-[11px] text-muted-foreground truncate">
                          {{ s.description || skillCostText(s) || '暂无描述' }}
                        </div>
                      </div>
                      <Badge variant="outline" class="shrink-0 text-[10px] px-1.5 py-0 font-normal">
                        {{ s.category || '技能' }}
                      </Badge>
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>

                <Button
                  v-else
                  variant="outline"
                  size="sm"
                  class="h-7 gap-1.5 text-xs text-muted-foreground"
                  @click="editor.activeTab = 'skills'"
                >
                  <IconPlus class="size-3.5" />
                  <span>新建技能</span>
                </Button>
              </div>
            </div>

            <!-- 空状态：故事书无技能 -->
            <div
              v-if="!availableSkills.length"
              class="rounded-xl border border-dashed border-border/80 bg-muted/20 p-5 text-center text-xs text-muted-foreground"
            >
              故事书尚未定义技能。
              <button
                type="button"
                class="ml-1 cursor-pointer font-bold text-primary underline underline-offset-2"
                @click="editor.activeTab = 'skills'"
              >
                前往「技能」新增
              </button>
            </div>

            <!-- 空状态：角色尚未掌握技能 -->
            <div
              v-else-if="!(current.skills?.length ?? 0)"
              class="rounded-xl border border-dashed border-border/80 bg-muted/15 p-5 text-center space-y-1.5"
            >
              <div class="text-xs font-medium text-muted-foreground">该角色尚未掌握任何技能</div>
              <div class="text-[11px] text-muted-foreground/70">
                点击右上角「绑定技能」，从故事书技能库赋予该角色施展能力。
              </div>
            </div>

            <!-- 技能卡片网格列表（RPG 游戏技能卡） -->
            <div v-else class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
              <template v-for="(sid, i) in current.skills ?? []" :key="sid + '-' + i">
                <!-- 已识别技能卡 -->
                <div
                  v-if="getSkill(sid)"
                  class="group relative flex flex-col justify-between rounded-xl border border-border/75 bg-card/85 p-3.5 shadow-2xs transition-all duration-200 hover:border-primary/60 hover:bg-card hover:shadow-md active:scale-[0.99] gap-3 cursor-pointer select-none"
                  @click="openSkillDetail(sid, i)"
                >
                  <!-- 卡片头部：图标、技能名、分类与快捷操作 -->
                  <div class="flex items-start justify-between gap-2">
                    <div class="flex items-center gap-2.5 min-w-0 flex-1">
                      <div class="flex size-7 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary border border-primary/20 group-hover:scale-105 group-hover:border-primary/40 transition-all">
                        <IconSparkles class="size-3.5" />
                      </div>
                      <div class="min-w-0">
                        <div class="flex items-center gap-1.5">
                          <span class="font-serif text-[13.5px] font-bold text-foreground truncate group-hover:text-primary transition-colors" :title="getSkill(sid)!.name">
                            {{ getSkill(sid)!.name }}
                          </span>
                          <Badge variant="outline" class="shrink-0 text-[9.5px] px-1.5 py-0 border-border/70 bg-muted/50 text-muted-foreground font-normal">
                            {{ getSkill(sid)!.category || '通用' }}
                          </Badge>
                        </div>
                        <div class="font-mono text-[10px] text-muted-foreground/60 truncate">
                          {{ sid }}
                        </div>
                      </div>
                    </div>

                    <!-- 操作栏：展开详情 / 更换技能 / 移除技能 -->
                    <div class="flex items-center gap-0.5 shrink-0" @click.stop>
                      <!-- 展开技能卡详情弹窗 -->
                      <button
                        type="button"
                        class="flex size-6 cursor-pointer items-center justify-center rounded text-muted-foreground/60 transition-colors hover:bg-primary/15 hover:text-primary"
                        title="展开技能卡详情弹窗"
                        @click.stop="openSkillDetail(sid, i)"
                      >
                        <IconMaximize class="size-3.5" />
                      </button>

                      <!-- 更换技能下拉 -->
                      <DropdownMenu v-if="skillOptions.length > 1">
                        <DropdownMenuTrigger as-child>
                          <button
                            type="button"
                            class="flex size-6 cursor-pointer items-center justify-center rounded text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
                            title="更换为此角色的其他技能"
                          >
                            <IconArrowsExchange class="size-3.5" />
                          </button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" class="w-56 max-h-64 overflow-y-auto p-1">
                          <DropdownMenuLabel class="text-xs text-muted-foreground px-2 py-1">更换技能为</DropdownMenuLabel>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            v-for="opt in skillOptions"
                            :key="opt.value"
                            :disabled="opt.value === sid"
                            class="cursor-pointer text-xs py-1.5"
                            @click="current.skills![i] = opt.value"
                          >
                            <div class="flex items-center justify-between w-full">
                              <span>{{ opt.label }}</span>
                              <span v-if="opt.value === sid" class="text-[10px] text-primary">当前</span>
                            </div>
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>

                      <!-- 解除绑定按钮 -->
                      <button
                        type="button"
                        class="flex size-6 cursor-pointer items-center justify-center rounded text-muted-foreground/60 transition-colors hover:bg-destructive/15 hover:text-destructive"
                        title="移除该技能"
                        aria-label="移除技能"
                        @click.stop="unbindCharSkill(current, i)"
                      >
                        <IconX class="size-3.5" />
                      </button>
                    </div>
                  </div>

                  <!-- 技能描述文本 -->
                  <p class="text-xs leading-relaxed text-muted-foreground line-clamp-2 min-h-[32px]">
                    {{ getSkill(sid)!.description || '（暂无技能描述，点击卡片展开详情）' }}
                  </p>

                  <!-- 卡片底部数值槽与规则徽标 -->
                  <div class="flex flex-wrap items-center gap-1.5 pt-1.5 border-t border-border/50 text-[10.5px]">
                    <!-- 消耗标签 -->
                    <span
                      class="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 font-mono font-semibold"
                      :class="getSkill(sid)!.cost?.length
                        ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border border-amber-500/20'
                        : 'bg-muted/50 text-muted-foreground border border-border/50'"
                    >
                      <IconCoins class="size-3 opacity-75" />
                      <span>{{ skillCostText(getSkill(sid)!) }}</span>
                    </span>

                    <!-- 检定判定 -->
                    <span
                      v-if="skillCheckText(getSkill(sid)!)"
                      class="inline-flex items-center gap-1 rounded-md border border-primary/25 bg-primary/10 px-1.5 py-0.5 font-mono text-primary"
                      :title="'判定规则: ' + skillCheckText(getSkill(sid)!)"
                    >
                      <IconDice class="size-3 opacity-80" />
                      <span>{{ skillCheckText(getSkill(sid)!) }}</span>
                    </span>

                    <!-- 效果标签 -->
                    <span
                      v-if="skillEffectText(getSkill(sid)!)"
                      class="inline-flex items-center gap-1 rounded-md border border-emerald-500/25 bg-emerald-500/10 px-1.5 py-0.5 font-mono text-emerald-600 dark:text-emerald-400"
                      :title="'即时效果: ' + skillEffectText(getSkill(sid)!)"
                    >
                      <IconFlame class="size-3 opacity-80" />
                      <span class="truncate max-w-[110px]">{{ skillEffectText(getSkill(sid)!) }}</span>
                    </span>

                    <!-- 冷却标签 -->
                    <span
                      v-if="getSkill(sid)!.cooldown?.turns"
                      class="inline-flex items-center gap-1 rounded-md border border-blue-500/25 bg-blue-500/10 px-1.5 py-0.5 font-mono text-blue-600 dark:text-blue-400"
                    >
                      <IconHourglass class="size-3 opacity-80" />
                      <span>CD {{ getSkill(sid)!.cooldown!.turns }}回合</span>
                    </span>

                    <!-- 目标标签 -->
                    <span
                      v-if="getSkill(sid)!.target"
                      class="inline-flex items-center gap-1 rounded-md border border-border/60 bg-muted/40 px-1.5 py-0.5 font-mono text-muted-foreground"
                    >
                      <IconTarget class="size-3 opacity-75" />
                      <span>{{ getSkill(sid)!.target }}</span>
                    </span>

                    <!-- 展开提示 -->
                    <span class="ml-auto text-[10px] text-primary/70 opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-0.5">
                      <span>详情</span>
                      <IconMaximize class="size-2.5" />
                    </span>
                  </div>
                </div>

                <!-- 悬空或未识别的技能引用（防御性呈现） -->
                <div
                  v-else
                  class="flex flex-col justify-between rounded-xl border border-destructive/40 bg-destructive/5 p-3.5 gap-2"
                >
                  <div class="flex items-start justify-between gap-2">
                    <div class="min-w-0">
                      <div class="font-bold text-xs text-destructive">未识别技能 ({{ sid }})</div>
                      <div class="text-[11px] text-muted-foreground/80 mt-0.5">
                        故事书技能库中不存在此 ID，可能已被删除。
                      </div>
                    </div>
                    <button
                      type="button"
                      class="flex size-6 cursor-pointer items-center justify-center rounded text-destructive/70 hover:bg-destructive/15 hover:text-destructive"
                      title="移除失效引用"
                      @click="unbindCharSkill(current, i)"
                    >
                      <IconX class="size-3.5" />
                    </button>
                  </div>
                </div>
              </template>
            </div>
          </div>
        </div>
      </div>
    </template>

    <div v-else class="py-12 text-center text-xs text-muted-foreground/60">
      从左侧名册选择一位角色开始编辑，或点击右上「新增」创建。
    </div>

    <!-- 隐藏式文件输入，用于立绘上传 -->
    <input
      ref="fileInput"
      type="file"
      accept="image/*"
      class="hidden"
      @change="current ? onPickFile($event, current) : null"
    />

    <!-- ================= 技能卡展示弹窗（Game Skill Card Inspector Dialog） ================= -->
    <Dialog :open="detailModalOpen" @update:open="(v: boolean) => detailModalOpen = v">
      <DialogContent class="sm:max-w-xl max-h-[88vh] overflow-y-auto border-border/80 bg-card/95 backdrop-blur-md p-0 overflow-hidden shadow-2xl">
        <template v-if="inspectingSkill">
          <!-- 弹窗顶栏：技能名、身份、分类与核心指示 -->
          <div class="relative border-b border-border/70 bg-gradient-to-r from-primary/15 via-muted/40 to-card p-5">
            <DialogHeader class="p-0 space-y-0">
              <div class="flex items-start justify-between gap-4">
                <div class="flex items-start gap-3.5 min-w-0">
                  <!-- 大尺寸技能图标徽章 -->
                  <div class="flex size-14 shrink-0 items-center justify-center rounded-2xl bg-gradient-to-br from-primary/20 to-primary/5 text-primary border border-primary/30 shadow-inner">
                    <IconSparkles class="size-7" />
                  </div>
                  <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-2">
                      <DialogTitle class="font-serif text-2xl font-black tracking-wide text-foreground">
                        {{ inspectingSkill.name }}
                      </DialogTitle>
                      <Badge variant="outline" class="border-primary/40 bg-primary/10 text-xs font-semibold text-primary">
                        {{ inspectingSkill.category || '通用技艺' }}
                      </Badge>
                    </div>
                    <DialogDescription class="flex flex-wrap items-center gap-2 pt-1 font-mono text-xs text-muted-foreground/80">
                      <span>唯一标识: {{ inspectingSkill.id }}</span>
                      <span>·</span>
                      <span>掌握者: {{ current?.name }}</span>
                    </DialogDescription>
                  </div>
                </div>
              </div>
            </DialogHeader>

            <!-- 核心参数状态槽 (3列网格) -->
            <div class="mt-4 grid grid-cols-3 gap-2 pt-3 border-t border-border/50 text-xs">
              <!-- 消耗 -->
              <div class="rounded-lg border border-border/60 bg-background/60 p-2 text-center">
                <div class="text-[10px] text-muted-foreground font-medium flex items-center justify-center gap-1">
                  <IconCoins class="size-3 text-warning" />
                  <span>消耗代价</span>
                </div>
                <div class="mt-0.5 font-mono font-bold text-foreground">
                  {{ skillCostText(inspectingSkill) }}
                </div>
              </div>

              <!-- 冷却 -->
              <div class="rounded-lg border border-border/60 bg-background/60 p-2 text-center">
                <div class="text-[10px] text-muted-foreground font-medium flex items-center justify-center gap-1">
                  <IconHourglass class="size-3 text-blue-500" />
                  <span>冷却机制</span>
                </div>
                <div class="mt-0.5 font-mono font-bold text-foreground">
                  {{ inspectingSkill.cooldown?.turns ? `${inspectingSkill.cooldown.turns} 回合` : '无冷却' }}
                </div>
              </div>

              <!-- 目标 -->
              <div class="rounded-lg border border-border/60 bg-background/60 p-2 text-center">
                <div class="text-[10px] text-muted-foreground font-medium flex items-center justify-center gap-1">
                  <IconTarget class="size-3 text-primary" />
                  <span>生效目标</span>
                </div>
                <div class="mt-0.5 font-mono font-bold text-foreground truncate">
                  {{ inspectingSkill.target || '自身 / 场景' }}
                </div>
              </div>
            </div>
          </div>

          <!-- 弹窗主体内容 -->
          <div class="p-5 space-y-4">
            <!-- 技能描述 -->
            <div>
              <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-1.5 flex items-center gap-1.5">
                <IconBook2 class="size-3.5 text-primary" />
                <span>技能描述与叙述</span>
              </div>
              <div class="rounded-xl border border-border/70 bg-muted/20 p-3.5 text-[13px] leading-relaxed text-foreground">
                {{ inspectingSkill.description || '该技能暂无详细文本描述，可在「技能与物品」库中补充完备。' }}
              </div>
            </div>

            <!-- 检定规则（如果有） -->
            <div v-if="inspectingSkill.check">
              <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-1.5 flex items-center gap-1.5">
                <IconDice class="size-3.5 text-primary" />
                <span>检定判定机制</span>
              </div>
              <div class="rounded-xl border border-primary/25 bg-primary/5 p-3.5 space-y-1.5 text-xs">
                <div v-if="typeof inspectingSkill.check === 'string'" class="font-mono text-primary font-bold">
                  引用判定器: {{ inspectingSkill.check }}
                </div>
                <div v-else class="space-y-1.5">
                  <div class="flex items-center justify-between font-mono">
                    <span class="text-muted-foreground">检定掷骰:</span>
                    <span class="font-bold text-primary">{{ inspectingSkill.check.dice }}</span>
                  </div>
                  <div v-if="inspectingSkill.check.mode" class="flex items-center justify-between font-mono">
                    <span class="text-muted-foreground">判定模式:</span>
                    <span class="font-bold text-foreground">
                      {{ inspectingSkill.check.mode === 'gte' ? '大于等于成功 (gte)' : inspectingSkill.check.mode === 'lte' ? '小于等于成功 (lte)' : '成功度阶梯判定 (degree)' }}
                    </span>
                  </div>
                  <div v-if="inspectingSkill.check.degree_thresholds?.length" class="flex items-center justify-between font-mono">
                    <span class="text-muted-foreground">成功度阶梯阈值:</span>
                    <span class="font-bold text-foreground">[{{ inspectingSkill.check.degree_thresholds.join(', ') }}]</span>
                  </div>
                </div>
              </div>
            </div>

            <!-- 效果规则（如果有） -->
            <div v-if="inspectingSkill.effect && (inspectingSkill.effect.immediate?.length || inspectingSkill.effect.status?.length || inspectingSkill.effect.modifiers?.length)">
              <div class="text-xs font-bold text-muted-foreground uppercase tracking-wider mb-1.5 flex items-center gap-1.5">
                <IconFlame class="size-3.5 text-warning" />
                <span>技能即时与持续效果</span>
              </div>
              <div class="rounded-xl border border-border/70 bg-card p-3.5 space-y-2.5 text-xs">
                <!-- 即时效果 -->
                <div v-if="inspectingSkill.effect.immediate?.length" class="space-y-1.5">
                  <div class="text-[11px] font-bold text-muted-foreground">即时效果 (Immediate):</div>
                  <div class="flex flex-wrap gap-1.5">
                    <span
                      v-for="(imm, idx) in inspectingSkill.effect.immediate"
                      :key="idx"
                      class="inline-flex items-center gap-1 rounded-md border border-emerald-500/25 bg-emerald-500/10 px-2 py-1 font-mono text-emerald-600 dark:text-emerald-400"
                    >
                      <template v-if="imm.kind === 'modify_resource'">
                        修改资源 {{ imm.resource }} ({{ Number(imm.amount) > 0 ? '+' : '' }}{{ imm.amount }})
                      </template>
                      <template v-else-if="imm.kind === 'damage'">
                        造成伤害 {{ imm.amount }}
                      </template>
                      <template v-else-if="imm.kind === 'heal'">
                        恢复生命 {{ imm.amount }}
                      </template>
                      <template v-else-if="imm.kind === 'set_flag'">
                        设置标记 {{ imm.flag }}
                      </template>
                    </span>
                  </div>
                </div>

                <!-- 持续状态（顶层声明，按 id 引用） -->
                <div v-if="inspectingSkill.effect.status?.length" class="space-y-1 pt-1.5 border-t border-border/50">
                  <div class="text-[11px] font-bold text-muted-foreground">持续状态 (Status):</div>
                  <div v-for="sid in inspectingSkill.effect.status" :key="sid" class="font-mono text-muted-foreground">
                    <template v-if="statusOf(sid)">
                      • {{ statusOf(sid)!.name }} (持续 {{ statusOf(sid)!.duration }} {{ statusOf(sid)!.unit === 'turns' ? '回合' : '场景' }})
                    </template>
                    <template v-else>• {{ sid }}（未声明）</template>
                  </div>
                </div>

                <!-- 属性修正 -->
                <div v-if="inspectingSkill.effect.modifiers?.length" class="space-y-1 pt-1.5 border-t border-border/50">
                  <div class="text-[11px] font-bold text-muted-foreground">属性修正 (Modifiers):</div>
                  <div class="flex flex-wrap gap-1.5">
                    <span v-for="m in inspectingSkill.effect.modifiers" :key="m.attribute" class="rounded bg-muted/60 px-1.5 py-0.5 font-mono">
                      {{ m.attribute }} {{ m.value > 0 ? '+' : '' }}{{ m.value }}
                    </span>
                  </div>
                </div>
              </div>
            </div>

            <!-- Lua 脚本钩子（如果有） -->
            <div v-if="inspectingSkill.lua" class="space-y-1.5">
              <LuaHookEditor
                :model-value="inspectingSkill.lua ?? ''"
                mode="hook"
                title="Lua 自定义脚本逻辑"
                :skill="inspectingSkill"
                :storybook="d"
                @update:model-value="inspectingSkill.lua = $event"
              />
            </div>
          </div>

          <!-- 弹窗底部操作栏 Footer -->
          <div class="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 bg-muted/30 px-5 py-3.5">
            <!-- 左侧：前往全局技能库编辑 -->
            <button
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 border-b border-dashed border-border/70 text-xs text-muted-foreground transition-colors hover:border-primary/60 hover:text-primary"
              @click="editor.activeTab = 'skills'; closeSkillDetail()"
            >
              <span>在技能库中编辑定义</span>
              <IconArrowUpRight class="size-3.5" />
            </button>

            <!-- 右侧：更换、移除、完成 -->
            <div class="flex items-center gap-2">
              <!-- 更换为其他技能 -->
              <DropdownMenu v-if="skillOptions.length > 1">
                <DropdownMenuTrigger as-child>
                  <Button variant="outline" size="sm" class="h-8 gap-1.5 text-xs">
                    <IconArrowsExchange class="size-3.5" />
                    <span>更换技能</span>
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="w-56 max-h-64 overflow-y-auto p-1">
                  <DropdownMenuLabel class="text-xs text-muted-foreground px-2 py-1">更换为此角色其他技能</DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    v-for="opt in skillOptions"
                    :key="opt.value"
                    :disabled="opt.value === inspectingSkill.id"
                    class="cursor-pointer text-xs py-1.5"
                    @click="swapInspectingSkill(opt.value)"
                  >
                    <div class="flex items-center justify-between w-full">
                      <span>{{ opt.label }}</span>
                      <span v-if="opt.value === inspectingSkill.id" class="text-[10px] text-primary">当前</span>
                    </div>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>

              <!-- 移除此技能 -->
              <Button
                variant="ghost"
                size="sm"
                class="h-8 gap-1 text-xs text-destructive hover:bg-destructive/10"
                @click="removeInspectingSkill()"
              >
                <IconTrash class="size-3.5" />
                <span>移除技能</span>
              </Button>

              <!-- 关闭弹窗 -->
              <Button
                variant="secondary"
                size="sm"
                class="h-8 text-xs"
                @click="closeSkillDetail()"
              >
                完成
              </Button>
            </div>
          </div>
        </template>
      </DialogContent>
    </Dialog>
  </WorkbenchLayout>
</template>


