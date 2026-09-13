<script setup lang="ts">
// 设置 · AI Provider（#26）：左分区导航 + 右行式配置。
// 供应商支持「列表 / 展开编辑 / 新增 / 删除」；密钥仅存本地配置文件（0600）。
import { computed, ref, watch } from 'vue'
import { useSettingsStore } from '../stores/settings'
import type { ModelEntry, ProviderConfig, ProviderKind, RoleConfig } from '@/types'
import { MODEL_CATALOG, PROVIDER_PRESETS } from '@/api/model-catalog'
import { catalogEntries, catalogProvider, findModelProviders, catalogModelMeta, mergeModelMeta, thinkingLevels, levelToWire, wireToLevel, LEVEL_LABEL } from '@/api/model-catalog-utils'
import ModelPicker from './ModelPicker.vue'
import PromptSettings from './PromptSettings.vue'
import { AI_PRESETS, applyPreset } from '@/lib/aiPresets'
import { confirm } from '@/lib/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import {
  themeMode, setTheme,
  currentFont, availableFonts, isQueryingFonts, isFontApiSupported,
  setFont, queryBrowserFonts, PRESET_FONTS,
} from '@/theme'
import {
  IconSettings, IconRobot, IconPlugConnected, IconCoin, IconInfoCircle,
  IconCircleCheck, IconAlertTriangle, IconLoader2, IconPlus, IconTrash, IconChevronRight,
  IconPalette, IconSun, IconMoon, IconDeviceDesktop,
  IconTypography, IconSearch, IconMessageChatbot,
} from '@tabler/icons-vue'
import { isEditorIntroHidden, setEditorIntroHidden } from '@/pages/editor/stores/pair'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>()
const store = useSettingsStore()

type RoleKey = 'story' | 'pair'
const ROLES: { key: RoleKey; label: string; desc: string }[] = [
  { key: 'story', label: 'AI（单一）', desc: '故事走向、旁白、世界响应并扮演所有 NPC' },
  { key: 'pair', label: '结对 AI', desc: '设定构思、启发与结构化建议（用于 AI 结对工作台）' },
]

/** 各协议类型的默认端点；选中类型时自动填入 Base URL */
const KIND_DEFAULT_URL: Record<ProviderKind, string> = {
  openai: 'https://api.openai.com/v1',
  anthropic: 'https://api.anthropic.com',
  ollama: 'http://127.0.0.1:11434',
  'openai-compatible': '',
}

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: 'openai', label: 'OpenAI' },
  { value: 'anthropic', label: 'Anthropic' },
  { value: 'ollama', label: 'Ollama（本地）' },
  { value: 'openai-compatible', label: 'OpenAI 兼容端点（DeepSeek / Moonshot / Groq…）' },
]

type SectionKey = 'roles' | 'providers' | 'budget' | 'prompts' | 'appearance' | 'about'
const NAV: { key: SectionKey; label: string; icon: unknown }[] = [
  { key: 'roles', label: 'AI 模型', icon: IconRobot },
  { key: 'providers', label: '供应商', icon: IconPlugConnected },
  { key: 'budget', label: '成本护栏', icon: IconCoin },
  { key: 'prompts', label: '提示词', icon: IconMessageChatbot },
  { key: 'appearance', label: '外观与主题', icon: IconPalette },
  { key: 'about', label: '关于', icon: IconInfoCircle },
]
const active = ref<SectionKey>('roles')

// 供应商：展开态 / 新增态
const expandedId = ref<string | null>(null)
const adding = ref(false)
const draft = ref<ProviderConfig>({ id: '', label: '', kind: 'openai', base_url: '', api_key: '', models: [] })
const catalogPick = ref('')
const catalogOpen = ref(false)
const catalogQuery = ref('')
const catalogFiltered = computed(() => {
  const q = catalogQuery.value.trim().toLowerCase()
  const list = q
    ? MODEL_CATALOG.filter(p => p.id.toLowerCase().includes(q) || p.label.toLowerCase().includes(q))
    : MODEL_CATALOG
  return list.slice(0, 80)
})
function closeCatalogSoon() { window.setTimeout(() => { catalogOpen.value = false }, 150) }
function selectCatalog(id: string) {
  catalogPick.value = id
  applyCatalog(id)
  catalogQuery.value = MODEL_CATALOG.find(x => x.id === id)?.label ?? id
  catalogOpen.value = false
}

// 字体设置与本地字体查询（Local Font Access API）
const fontFilter = ref(currentFont.value)
const fontDropdownOpen = ref(false)
const fontQueryFeedback = ref<{ type: 'ok' | 'err'; message: string } | null>(null)

const filteredFonts = computed(() => {
  const q = fontFilter.value.trim().toLowerCase()
  if (!q) return availableFonts.value.slice(0, 120)
  return availableFonts.value
    .filter(f => f.family.toLowerCase().includes(q) || f.fullName.toLowerCase().includes(q))
    .slice(0, 120)
})

async function onQueryBrowserFonts() {
  fontQueryFeedback.value = null
  const res = await queryBrowserFonts()
  if (res.ok) {
    fontQueryFeedback.value = { type: 'ok', message: `成功检索到 ${res.count} 个系统字体！` }
    fontDropdownOpen.value = true
  } else {
    fontQueryFeedback.value = { type: 'err', message: res.error || '检索失败' }
  }
}

function selectFont(fontFamily: string) {
  setFont(fontFamily)
  fontFilter.value = fontFamily
  fontDropdownOpen.value = false
  fontQueryFeedback.value = { type: 'ok', message: fontFamily ? `已应用并保存字体「${fontFamily}」` : '已恢复默认字体' }
}

function onCustomFontSubmit() {
  if (fontFilter.value.trim()) {
    selectFont(fontFilter.value.trim())
  }
}

function closeFontDropdownSoon() {
  window.setTimeout(() => { fontDropdownOpen.value = false }, 200)
}

// 新手创作引导开关
const editorIntroEnabled = ref(!isEditorIntroHidden())
function toggleEditorIntro(enabled: boolean) {
  editorIntroEnabled.value = enabled
  setEditorIntroHidden(!enabled)
}

watch(() => props.open, (v) => {
  if (v) {
    active.value = 'roles'
    adding.value = false
    expandedId.value = null
    fontFilter.value = currentFont.value
    fontQueryFeedback.value = null
    editorIntroEnabled.value = !isEditorIntroHidden()
    void store.load().then(() => {
      if (config.value && !config.value.roles.pair) {
        ensureRole('pair')
      }
    })
  }
})

const config = computed(() => store.config)
function modelsOf(providerId: string): ModelEntry[] {
  return config.value?.providers.find(p => p.id === providerId)?.models ?? []
}
function ensureRole(key: RoleKey): RoleConfig {
  const c = config.value
  if (!c) return { provider_id: '', model: '' }
  if (!c.roles[key]) {
    const fallback = c.roles.story?.provider_id || c.providers[0]?.id || ''
    c.roles[key] = {
      provider_id: fallback,
      model: modelsOf(fallback)[0]?.id || c.roles.story?.model || '',
      temperature: 0.8,
    }
  }
  return c.roles[key]!
}
/** AI 后端开关（config.ai 可能缺失，按需补默认） */
function ensureAi(): { provider: string } {
  const c = config.value
  if (!c) return { provider: 'auto' }
  if (!c.ai) c.ai = { provider: 'auto' }
  return c.ai
}
const PROVIDER_MODES = [
  { value: 'auto', label: '自动（rig 优先，失败回退脚本）' },
  { value: 'rig', label: 'rig（失败回退脚本）' },
  { value: 'scripted', label: 'scripted（确定性，离线 / 测试）' },
]
function onRoleProvider(key: RoleKey, providerId: string) {
  const role = ensureRole(key)
  role.provider_id = providerId
  role.model = modelsOf(providerId)[0]?.id ?? ''
}
/** 套用采样预设档；选「自定义」只清标签，保留当前值。 */
function onPreset(key: RoleKey, name: string) {
  const role = ensureRole(key)
  if (name === 'custom') {
    role.preset = undefined
    return
  }
  const p = AI_PRESETS.find(x => x.name === name)
  if (p) applyPreset(role, p)
}
/** 思考强度：档位来自该角色所选模型在 pi.dev 目录里的 thinkingLevelMap（未收录则回落通用档）。 */
function roleMeta(key: RoleKey) {
  const role = ensureRole(key)
  if (!role.provider_id || !role.model) return undefined
  // 目录元数据 + 用户自定义覆盖（小中转站）
  const entry = config.value?.providers.find(p => p.id === role.provider_id)?.models.find(x => x.id === role.model)
  return mergeModelMeta(catalogModelMeta(role.provider_id, role.model), entry)
}
function roleLevels(key: RoleKey): string[] {
  return thinkingLevels(roleMeta(key))
}
function roleSupportsReasoning(key: RoleKey): boolean {
  return roleMeta(key)?.reasoning !== false
}
function roleLevel(key: RoleKey): string {
  return wireToLevel(roleMeta(key), ensureRole(key).reasoning_effort)
}
function onEffort(key: RoleKey, level: string) {
  const role = ensureRole(key)
  role.reasoning_effort = levelToWire(roleMeta(key), level)
}
/** 选了模型但还没填端点时：按模型反查供应商，自动补 Base URL / 类型 / 名称 */
function autoFillFromModel(modelId: string, p: ProviderConfig) {
  if (p.base_url) return
  const [pid] = findModelProviders(modelId)
  if (!pid) return
  const preset = PROVIDER_PRESETS[pid]
  if (preset) {
    p.base_url = preset.base_url
    p.kind = preset.kind
  }
  if (!p.label) p.label = catalogProvider(pid)?.label ?? pid
}

function toggle(id: string) { expandedId.value = expandedId.value === id ? null : id }

/** 选中类型 → 自动填默认端点（若端点为空的或此前是自动填的） */
function applyKind(p: ProviderConfig, kind: ProviderKind) {
  p.kind = kind
  const url = KIND_DEFAULT_URL[kind]
  const autoUrls = Object.values(KIND_DEFAULT_URL)
  const current = (p.base_url ?? '').trim()
  if (url && (!current || autoUrls.includes(current))) p.base_url = url
  if (!p.label.trim()) p.label = KINDS.find(k => k.value === kind)?.label ?? kind
}

function slugify(s: string): string {
  const base = s.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
  return base || 'p-' + Date.now().toString(36)
}
function uniqueId(base: string): string {
  const taken = new Set((config.value?.providers ?? []).map(p => p.id))
  if (!taken.has(base)) return base
  let i = 2
  while (taken.has(`${base}-${i}`)) i++
  return `${base}-${i}`
}

/** 从目录选择供应商：预填名称 / 类型 / 端点 / 模型清单 */
function applyCatalog(id: string) {
  const p = MODEL_CATALOG.find(x => x.id === id)
  if (!p) return
  const preset = PROVIDER_PRESETS[id]
  draft.value.label = p.label
  draft.value.kind = (preset?.kind ?? 'openai-compatible') as ProviderKind
  draft.value.base_url = preset?.base_url ?? ''
  draft.value.models = catalogEntries(p.id)
}

function startAdd() {
  draft.value = { id: '', label: '', kind: 'openai', base_url: '', api_key: '', models: [] }
  catalogPick.value = ''
  catalogQuery.value = ''
  catalogOpen.value = false
  adding.value = true
  expandedId.value = null
}
function cancelAdd() { adding.value = false }
function saveAdd() {
  const c = config.value
  if (!c || !draft.value.label.trim()) return
  const p: ProviderConfig = {
    ...draft.value,
    id: uniqueId(slugify(draft.value.label)),
    label: draft.value.label.trim(),
    models: draft.value.models,
  }
  c.providers.push(p)
  adding.value = false
  expandedId.value = p.id
}
async function removeProvider(p: ProviderConfig) {
  const c = config.value
  if (!c) return
  const confirmed = await confirm({
    title: '删除供应商「' + p.label + '」？',
    description: '引用了它的模型配置将回退到第一个供应商。',
    confirmText: '删除',
    destructive: true,
  })
  if (!confirmed) return
  c.providers = c.providers.filter(x => x.id !== p.id)
  const fallback = c.providers[0]?.id ?? ''
  ;(['story', 'pair'] as RoleKey[]).forEach(k => {
    if (c.roles[k]?.provider_id === p.id) {
      c.roles[k]!.provider_id = fallback
      c.roles[k]!.model = modelsOf(fallback)[0]?.id ?? ''
    }
  })
  if (expandedId.value === p.id) expandedId.value = null
}

function close() { if (!store.saving) emit('update:open', false) }
async function save() { if (await store.save()) close() }
</script>

<template>
  <Dialog :open="open" @update:open="() => close()">
    <DialogContent class="gap-0 overflow-hidden p-0 sm:max-w-3xl">
      <DialogHeader class="flex-row items-center border-b border-border px-5 py-3.5 pr-12">
        <DialogTitle class="flex items-center gap-2 text-[15px] font-extrabold">
          <IconSettings aria-hidden="true" class="size-4.5 text-primary" />
          设置
        </DialogTitle>
        <DialogDescription class="sr-only">AI Provider 与模型配置</DialogDescription>
      </DialogHeader>

      <div v-if="!config" class="flex h-64 flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
        <IconLoader2 aria-hidden="true" class="size-5 animate-spin text-primary/70" />
        正在读取配置…
      </div>

      <div v-else class="flex h-[min(70vh,620px)] min-h-0 flex-col sm:flex-row">
        <!-- ============ 左：分区导航 ============ -->
        <nav class="flex shrink-0 gap-1 overflow-x-auto border-b border-border p-2 sm:w-44 sm:flex-col sm:border-r sm:border-b-0 sm:p-2.5" aria-label="设置分区">
          <button
            v-for="n in NAV"
            :key="n.key"
            type="button"
            class="flex shrink-0 items-center gap-2.5 rounded-lg px-3 py-2.5 text-left text-[13px] font-medium transition-all sm:w-full cursor-pointer"
            :class="active === n.key ? 'bg-primary/12 text-primary font-semibold shadow-xs' : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground'"
            @click="active = n.key"
          >
            <component :is="n.icon" aria-hidden="true" class="size-4" :class="active === n.key ? 'text-primary' : 'text-muted-foreground/70'" />
            {{ n.label }}
          </button>
        </nav>

        <!-- ============ 右：内容 ============ -->
        <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          <!-- —— AI 模型 —— -->
          <template v-if="active === 'roles'">
            <p class="mb-4 text-xs leading-relaxed text-muted-foreground">
              单一 AI 负责旁白与世界响应并扮演所有 NPC；结对用于启发设定建议。
            </p>

            <section class="mb-5">
              <h3 class="mb-1.5 text-[11px] font-bold tracking-[0.14em] text-muted-foreground uppercase">运行后端</h3>
              <div class="divide-y divide-border/60 rounded-xl border border-border bg-card/40 px-3.5">
                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">AI 后端</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">rig = 用框架调真实模型；scripted = 确定性脚本（离线 / 测试）</div>
                  </div>
                  <div class="w-52 shrink-0">
                    <Select :model-value="ensureAi().provider" @update:model-value="(v) => { if (typeof v === 'string') ensureAi().provider = v }">
                      <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem v-for="m in PROVIDER_MODES" :key="m.value" :value="m.value">{{ m.label }}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              </div>
            </section>

            <section v-for="r in ROLES" :key="r.key" class="mb-5 last:mb-0">
              <h3 class="mb-1.5 text-[11px] font-bold tracking-[0.14em] text-muted-foreground uppercase">{{ r.label }}</h3>
              <div class="divide-y divide-border/60 rounded-xl border border-border bg-card/40 px-3.5">
                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">供应商</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">{{ r.desc }}</div>
                  </div>
                  <div class="w-52 shrink-0">
                    <Select :model-value="ensureRole(r.key).provider_id" @update:model-value="(v) => { if (typeof v === 'string') onRoleProvider(r.key, v) }">
                      <SelectTrigger class="w-full"><SelectValue placeholder="选择供应商" /></SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem v-for="p in config.providers" :key="p.id" :value="p.id">{{ p.label }}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                </div>

                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">模型</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">该角色调用的具体模型</div>
                  </div>
                  <div class="w-52 shrink-0">
                    <Select :model-value="ensureRole(r.key).model" @update:model-value="(v) => { if (typeof v === 'string') ensureRole(r.key).model = v }">
                      <SelectTrigger class="w-full"><SelectValue placeholder="选择模型" /></SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem v-for="m in modelsOf(ensureRole(r.key).provider_id)" :key="m.id" :value="m.id">{{ m.name || m.id }}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                </div>

                <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">温度</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">越高越发散，0–2</div>
                    </div>
                    <div class="w-28 shrink-0">
                      <Input v-model.number="ensureRole(r.key).temperature" type="number" step="0.1" min="0" max="2" class="h-9 text-right" />
                    </div>
                  </div>
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">输出预算</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">max_tokens：思考与正文共享这份预算，越长的回答需要越大；留空按模型上限自动取（不低于 16384）</div>
                    </div>
                    <div class="w-28 shrink-0">
                      <Input v-model.number="ensureRole(r.key).max_tokens" type="number" step="1024" min="1024" max="65536" class="h-9 text-right" placeholder="自动" />
                    </div>
                  </div>
                  <div v-if="roleSupportsReasoning(r.key)" class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">思考强度</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">reasoning_effort：缺省用供应商默认</div>
                    </div>
                    <div class="w-52 shrink-0">
                      <Select :model-value="roleLevel(r.key)" @update:model-value="(v) => { if (typeof v === 'string') onEffort(r.key, v) }">
                        <SelectTrigger class="w-full"><SelectValue placeholder="默认" /></SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem v-for="lv in roleLevels(r.key)" :key="lv" :value="lv">{{ LEVEL_LABEL[lv] ?? lv }}</SelectItem>
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">采样预设</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">一键套用一档参数；再手改即为自定义</div>
                    </div>
                    <div class="w-52 shrink-0">
                      <Select :model-value="ensureRole(r.key).preset ?? 'custom'" @update:model-value="(v) => { if (typeof v === 'string') onPreset(r.key, v) }">
                        <SelectTrigger class="w-full"><SelectValue placeholder="选择预设" /></SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem v-for="p in AI_PRESETS" :key="p.name" :value="p.name">{{ p.name }}</SelectItem>
                            <SelectItem value="custom">自定义</SelectItem>
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">Top-P</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">核取样阈值，推荐 0.9</div>
                    </div>
                    <div class="w-28 shrink-0">
                      <Input v-model.number="ensureRole(r.key).top_p" type="number" step="0.05" min="0" max="1" class="h-9 text-right" placeholder="0.9" />
                    </div>
                  </div>
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">存在 / 频率惩罚</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">抑制复读，推荐 0–0.3</div>
                    </div>
                    <div class="flex w-52 shrink-0 items-center gap-1.5">
                      <Input v-model.number="ensureRole(r.key).presence_penalty" type="number" step="0.1" min="-2" max="2" class="h-9 text-right" placeholder="0" />
                      <Input v-model.number="ensureRole(r.key).frequency_penalty" type="number" step="0.1" min="-2" max="2" class="h-9 text-right" placeholder="0" />
                    </div>
                  </div>
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">停止序列</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">逗号分隔，命中即停</div>
                    </div>
                    <div class="w-52 shrink-0">
                      <Input
                        :model-value="(ensureRole(r.key).stop ?? []).join(', ')"
                        placeholder="如 </story>, ---"
                        @update:model-value="(v) => { ensureRole(r.key).stop = String(v).split(',').map(s => s.trim()).filter(Boolean) }"
                      />
                    </div>
                  </div>
              </div>
            </section>
          </template>

          <!-- —— 供应商 —— -->
          <template v-else-if="active === 'providers'">
            <div class="mb-3 flex items-start justify-between gap-3">
              <p class="max-w-md text-xs leading-relaxed text-muted-foreground">
                API Key 保存于本地配置文件（0600），日志脱敏；支持环境变量覆盖。
              </p>
              <Button size="sm" class="shrink-0" :disabled="adding" @click="startAdd">
                <IconPlus data-icon="inline-start" />添加供应商
              </Button>
            </div>

            <!-- 新增表单 -->
            <div v-if="adding" class="mb-3 rounded-xl border border-primary/40 bg-primary/5 p-3.5">
              <div class="mb-2.5 text-[12px] font-bold text-primary">新增供应商</div>
              <div class="mb-3 flex flex-col gap-1.5">
                <span class="text-[11px] font-semibold text-muted-foreground">从目录选择（可选，自动填充名称 / 端点 / 模型）</span>
                <div class="relative">
                  <Input
                    v-model="catalogQuery"
                    placeholder="搜索供应商，如 deepseek / anthropic / openrouter…"
                    class="h-9"
                    @focus="catalogOpen = true"
                    @blur="closeCatalogSoon"
                  />
                  <div v-if="catalogOpen" class="absolute inset-x-0 top-full z-50 mt-1 max-h-64 overflow-y-auto rounded-lg border border-border bg-popover p-1 shadow-lg shadow-black/40" @mousedown.prevent>
                    <button
                      v-for="cp in catalogFiltered"
                      :key="cp.id"
                      type="button"
                      class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-accent"
                      @mousedown.prevent="selectCatalog(cp.id)"
                    >
                      <span class="truncate">{{ cp.label }}</span>
                      <span class="ml-auto shrink-0 text-[11px] text-muted-foreground">{{ cp.models.length }} 个模型</span>
                    </button>
                    <div v-if="!catalogFiltered.length" class="px-2.5 py-3 text-center text-[11.5px] text-muted-foreground">没有匹配的供应商</div>
                  </div>
                </div>
              </div>
              <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <label class="flex flex-col gap-1.5">
                  <span class="text-[11px] font-semibold text-muted-foreground">名称</span>
                  <Input v-model="draft.label" placeholder="例如 DeepSeek / 本地 Ollama" class="h-9" />
                </label>
                <label class="flex flex-col gap-1.5">
                  <span class="text-[11px] font-semibold text-muted-foreground">类型</span>
                  <Select :model-value="draft.kind" @update:model-value="(v) => { if (typeof v === 'string') applyKind(draft, v as ProviderKind) }">
                    <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem v-for="k in KINDS" :key="k.value" :value="k.value">{{ k.label }}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  <span class="text-[10.5px] text-muted-foreground/70">类型 = 协议方言；厂商（DeepSeek / Moonshot 等）选「OpenAI 兼容端点」</span>
                </label>
                <label class="flex flex-col gap-1.5 sm:col-span-2">
                  <span class="text-[11px] font-semibold text-muted-foreground">Base URL</span>
                  <Input v-model="draft.base_url" placeholder="https://…" class="h-9" />
                </label>
                <label class="flex flex-col gap-1.5 sm:col-span-2">
                  <span class="text-[11px] font-semibold text-muted-foreground">API Key</span>
                  <Input v-model="draft.api_key" type="password" :placeholder="draft.kind === 'ollama' ? '（本地无需）' : 'sk-…'" class="h-9" />
                </label>
                <div class="flex flex-col gap-1.5 sm:col-span-2">
                  <span class="text-[11px] font-semibold text-muted-foreground">模型清单</span>
                  <ModelPicker
                    :model-value="draft.models"
                    :provider="draft"
                    @update:model-value="draft.models = $event"
                    @pick="(m: string) => autoFillFromModel(m, draft)"
                  />
                </div>
              </div>
              <div class="mt-3 flex justify-end gap-2">
                <Button size="sm" variant="ghost" @click="cancelAdd">取消</Button>
                <Button size="sm" :disabled="!draft.label.trim()" @click="saveAdd">添加</Button>
              </div>
            </div>

            <!-- 供应商列表 -->
            <div v-for="p in config.providers" :key="p.id" class="mb-2.5 overflow-hidden rounded-xl border border-border bg-card/40">
              <div class="flex items-center gap-2 px-3.5 py-2.5">
                <button type="button" class="flex min-w-0 flex-1 items-center gap-2 text-left" @click="toggle(p.id)">
                  <IconChevronRight aria-hidden="true" class="size-3.5 shrink-0 text-muted-foreground transition-transform" :class="expandedId === p.id ? 'rotate-90' : ''" />
                  <span class="shrink-0 text-[13px] font-semibold">{{ p.label }}</span>
                  <Badge variant="outline" class="shrink-0 font-mono text-[10px]">{{ p.kind }}</Badge>
                  <span class="truncate text-xs text-muted-foreground">{{ p.base_url }}</span>
                </button>
                <Button size="xs" variant="outline" :disabled="store.tests[p.id]?.testing" @click="store.test(p)">
                  <IconLoader2 v-if="store.tests[p.id]?.testing" data-icon="inline-start" class="animate-spin" />
                  <IconPlugConnected v-else data-icon="inline-start" />
                  测试
                </Button>
                <Button size="icon-xs" variant="ghost" class="text-muted-foreground hover:text-destructive" title="删除供应商" @click="removeProvider(p)">
                  <IconTrash />
                </Button>
              </div>

              <div v-if="store.tests[p.id] && !store.tests[p.id].testing" class="flex items-center gap-1.5 border-t border-border/60 px-3.5 py-1.5 text-xs" :class="store.tests[p.id].ok ? 'text-success' : 'text-destructive'">
                <IconCircleCheck v-if="store.tests[p.id].ok" class="size-3.5" />
                <IconAlertTriangle v-else class="size-3.5" />
                {{ store.tests[p.id].message }}<template v-if="store.tests[p.id].latency_ms"> · {{ store.tests[p.id].latency_ms }}ms</template>
              </div>

              <div v-if="expandedId === p.id" class="divide-y divide-border/60 border-t border-border/60 px-3.5">
                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">类型</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">协议方言；DeepSeek 等厂商选「OpenAI 兼容端点」</div>
                  </div>
                  <div class="w-72 shrink-0">
                    <Select :model-value="p.kind" @update:model-value="(v) => { if (typeof v === 'string') applyKind(p, v as ProviderKind) }">
                      <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem v-for="k in KINDS" :key="k.value" :value="k.value">{{ k.label }}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                </div>
                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">Base URL</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">兼容端点地址</div>
                  </div>
                  <div class="w-72 shrink-0">
                    <Input v-model="p.base_url" placeholder="https://…" class="h-9" />
                  </div>
                </div>
                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">API Key</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">{{ p.kind === 'ollama' ? '本地服务通常无需' : '仅存本地，不入日志' }}</div>
                  </div>
                  <div class="w-72 shrink-0">
                    <Input v-model="p.api_key" type="password" :placeholder="p.kind === 'ollama' ? '（本地无需）' : 'sk-…'" class="h-9" />
                  </div>
                </div>
                <div class="flex flex-col gap-1.5 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">模型清单</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">供角色分工下拉选择</div>
                  </div>
                  <ModelPicker
                    :model-value="p.models"
                    :provider="p"
                    @update:model-value="p.models = $event"
                    @pick="(m: string) => autoFillFromModel(m, p)"
                  />
                </div>
              </div>
            </div>

            <div v-if="!config.providers.length" class="rounded-xl border border-dashed border-border px-4 py-8 text-center text-xs text-muted-foreground">
              还没有供应商，点右上角「添加供应商」。
            </div>
          </template>

          <!-- —— 成本护栏 —— -->
          <template v-else-if="active === 'budget'">
            <p class="mb-4 text-xs leading-relaxed text-muted-foreground">
              超限时引擎发 warning 事件并收束回合；默认取 #05 的预算，可按 provider 调整。
            </p>
            <div class="divide-y divide-border/60 rounded-xl border border-border bg-card/40 px-3.5">
              <div class="flex items-center justify-between gap-6 py-2.5">
                <div class="min-w-0">
                  <div class="text-[13px] font-semibold">每回合 token 上限</div>
                  <div class="mt-0.5 text-xs text-muted-foreground">0 = 不限</div>
                </div>
                <div class="w-28 shrink-0">
                  <Input v-model.number="config.turn_token_budget" type="number" min="0" step="1000" class="h-9 text-right" />
                </div>
              </div>
            </div>
          </template>

          <!-- —— 提示词 —— -->
          <template v-else-if="active === 'prompts'">
            <PromptSettings />
          </template>

          <!-- —— 外观与主题 —— -->
          <template v-else-if="active === 'appearance'">
            <p class="mb-4 text-xs leading-relaxed text-muted-foreground">
              Octopus 界面风格与明暗外观设置。支持白昼明亮模式、沉浸暗色模式或跟随系统。
            </p>

            <div class="grid grid-cols-3 gap-3">
              <button
                type="button"
                class="group relative flex flex-col items-center gap-3 rounded-xl border p-4 text-center transition-all cursor-pointer hover:border-primary/50 hover:bg-card/90"
                :class="themeMode === 'light' ? 'border-primary bg-primary/10 shadow-sm ring-1 ring-primary' : 'border-border bg-card/40 text-muted-foreground'"
                @click="setTheme('light')"
              >
                <div class="flex size-10 items-center justify-center rounded-full bg-amber-500/15 text-amber-600 dark:text-amber-400">
                  <IconSun class="size-5" />
                </div>
                <div>
                  <div class="text-sm font-semibold text-foreground">明亮模式</div>
                  <div class="mt-1 text-[11px] leading-snug text-muted-foreground">Material 3 浅色基调</div>
                </div>
                <div v-if="themeMode === 'light'" class="absolute top-2.5 right-2.5">
                  <IconCircleCheck class="size-4 text-primary" />
                </div>
              </button>

              <button
                type="button"
                class="group relative flex flex-col items-center gap-3 rounded-xl border p-4 text-center transition-all cursor-pointer hover:border-primary/50 hover:bg-card/90"
                :class="themeMode === 'dark' ? 'border-primary bg-primary/10 shadow-sm ring-1 ring-primary' : 'border-border bg-card/40 text-muted-foreground'"
                @click="setTheme('dark')"
              >
                <div class="flex size-10 items-center justify-center rounded-full bg-primary/15 text-primary">
                  <IconMoon class="size-5" />
                </div>
                <div>
                  <div class="text-sm font-semibold text-foreground">暗色模式</div>
                  <div class="mt-1 text-[11px] leading-snug text-muted-foreground">Material 3 深色基调</div>
                </div>
                <div v-if="themeMode === 'dark'" class="absolute top-2.5 right-2.5">
                  <IconCircleCheck class="size-4 text-primary" />
                </div>
              </button>

              <button
                type="button"
                class="group relative flex flex-col items-center gap-3 rounded-xl border p-4 text-center transition-all cursor-pointer hover:border-primary/50 hover:bg-card/90"
                :class="themeMode === 'auto' ? 'border-primary bg-primary/10 shadow-sm ring-1 ring-primary' : 'border-border bg-card/40 text-muted-foreground'"
                @click="setTheme('auto')"
              >
                <div class="flex size-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
                  <IconDeviceDesktop class="size-5" />
                </div>
                <div>
                  <div class="text-sm font-semibold text-foreground">跟随系统</div>
                  <div class="mt-1 text-[11px] leading-snug text-muted-foreground">跟随操作系统变化</div>
                </div>
                <div v-if="themeMode === 'auto'" class="absolute top-2.5 right-2.5">
                  <IconCircleCheck class="size-4 text-primary" />
                </div>
              </button>
            </div>

            <!-- —— 界面字体设置（Local Font Access API） —— -->
            <div class="mt-5 rounded-xl border border-border bg-card/40 p-4">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <h4 class="flex items-center gap-1.5 text-xs font-bold text-foreground">
                    <IconTypography class="size-4 text-primary" />
                    界面字体
                  </h4>
                  <p class="mt-0.5 text-[11px] text-muted-foreground">
                    调用浏览器本地字体查询（Local Font Access API）读取系统字体，选择后保存至 LocalStorage。
                  </p>
                </div>
                <div class="flex items-center gap-2">
                  <Button
                    size="xs"
                    variant="outline"
                    :disabled="isQueryingFonts"
                    class="cursor-pointer"
                    @click="onQueryBrowserFonts"
                  >
                    <IconLoader2 v-if="isQueryingFonts" class="size-3.5 animate-spin" />
                    <IconSearch v-else class="size-3.5" />
                    {{ availableFonts.length ? '重新查询系统字体' : '查询系统字体' }}
                  </Button>
                  <Button
                    v-if="currentFont"
                    size="xs"
                    variant="ghost"
                    class="text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                    @click="selectFont('')"
                  >
                    恢复默认
                  </Button>
                </div>
              </div>

              <!-- 状态提示信息 -->
              <div v-if="fontQueryFeedback" class="mt-2.5 flex items-center gap-1.5 text-xs" :class="fontQueryFeedback.type === 'ok' ? 'text-success' : 'text-destructive'">
                <IconCircleCheck v-if="fontQueryFeedback.type === 'ok'" class="size-3.5 shrink-0" />
                <IconAlertTriangle v-else class="size-3.5 shrink-0" />
                <span>{{ fontQueryFeedback.message }}</span>
              </div>
              <div v-else-if="!isFontApiSupported" class="mt-2.5 flex items-center gap-1.5 text-[11px] text-muted-foreground/80">
                <IconInfoCircle class="size-3.5 shrink-0" />
                <span>当前浏览器不支持 Local Font Access API（需 Chromium 103+），可直接从预设中选择或输入字体名称。</span>
              </div>

              <!-- 字体搜索与选取 -->
              <div class="relative mt-3">
                <div class="flex gap-2">
                  <div class="relative flex-1">
                    <Input
                      v-model="fontFilter"
                      placeholder="搜索或输入字体名称（如 PingFang SC、霞鹜文楷、Consolas）…"
                      class="h-9 pr-8"
                      @focus="fontDropdownOpen = true"
                      @blur="closeFontDropdownSoon"
                      @keydown.enter.prevent="onCustomFontSubmit"
                    />
                    <button
                      v-if="fontFilter"
                      type="button"
                      class="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground/60 hover:text-foreground cursor-pointer text-xs"
                      @mousedown.prevent="fontFilter = ''"
                    >
                      ✕
                    </button>
                  </div>
                  <Button
                    size="sm"
                    variant="secondary"
                    :disabled="!fontFilter.trim() || fontFilter.trim() === currentFont"
                    class="shrink-0 cursor-pointer"
                    @click="onCustomFontSubmit"
                  >
                    应用
                  </Button>
                </div>

                <!-- 字体下拉候选列表 -->
                <div
                  v-if="fontDropdownOpen && (filteredFonts.length > 0 || availableFonts.length === 0)"
                  class="absolute inset-x-0 top-full z-50 mt-1 max-h-56 overflow-y-auto rounded-lg border border-border bg-popover p-1 shadow-lg shadow-black/30"
                  @mousedown.prevent
                >
                  <div v-if="availableFonts.length" class="px-2 py-1 text-[10px] font-semibold text-muted-foreground uppercase tracking-wider">
                    已检测到的系统字体（{{ availableFonts.length }}）
                  </div>
                  <div v-else class="px-2.5 py-2 text-center text-xs text-muted-foreground">
                    尚未扫描系统字体，可点击右上角「查询系统字体」或选择下方常用预设
                  </div>
                  <button
                    v-for="item in filteredFonts"
                    :key="item.family"
                    type="button"
                    class="flex w-full items-center justify-between rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-accent cursor-pointer transition-colors"
                    :class="currentFont === item.family ? 'bg-primary/10 text-primary font-medium' : 'text-foreground'"
                    @mousedown.prevent="selectFont(item.family)"
                  >
                    <span class="truncate" :style="{ fontFamily: `&quot;${item.family}&quot;, sans-serif` }">{{ item.family }}</span>
                    <span v-if="currentFont === item.family" class="flex items-center gap-1 text-[11px] text-primary">
                      <IconCircleCheck class="size-3.5" />
                      当前
                    </span>
                  </button>
                </div>
              </div>

              <!-- 快捷预设标签 -->
              <div class="mt-3 flex flex-wrap items-center gap-1.5">
                <span class="text-[11px] font-medium text-muted-foreground">常用预设：</span>
                <button
                  v-for="p in PRESET_FONTS"
                  :key="p.value"
                  type="button"
                  class="rounded-md border px-2 py-0.5 text-[11px] transition-all cursor-pointer"
                  :class="currentFont === p.value ? 'border-primary bg-primary/15 text-primary font-semibold' : 'border-border bg-card/60 text-muted-foreground hover:border-border-strong hover:text-foreground'"
                  @click="selectFont(p.value)"
                >
                  {{ p.label.split(' ')[0] }}
                </button>
              </div>

              <!-- 实时排版效果预览 -->
              <div class="mt-3.5 rounded-lg border border-border bg-muted/20 p-3">
                <div class="mb-1.5 flex items-center justify-between text-[11px]">
                  <span class="font-medium text-foreground">
                    实时效果预览（{{ currentFont || '系统默认' }}）
                  </span>
                  <span class="font-mono text-[10px] text-muted-foreground">
                    {{ currentFont ? '已持久化至 LocalStorage' : '默认字体栈' }}
                  </span>
                </div>
                <div class="text-[13.5px] font-medium leading-relaxed text-foreground" :style="{ fontFamily: currentFont ? `&quot;${currentFont}&quot;, sans-serif` : undefined }">
                  风雪落满青石栈道，远山的钟声在群峰间久久回荡。
                </div>
                <div class="mt-1 text-xs text-muted-foreground leading-normal" :style="{ fontFamily: currentFont ? `&quot;${currentFont}&quot;, sans-serif` : undefined }">
                  The quick brown fox jumps over the lazy dog. 0123456789
                </div>
              </div>
            </div>

            <!-- —— 偏好：新建故事书引导 —— -->
            <div class="mt-5 flex items-center justify-between rounded-xl border border-border bg-card/40 px-4 py-3">
              <div>
                <div class="text-[13px] font-semibold text-foreground">新建故事书创作引导</div>
                <div class="mt-0.5 text-xs text-muted-foreground">创建新故事书草稿时，在编辑器顶部弹出三范式指引</div>
              </div>
              <label class="relative inline-flex items-center cursor-pointer">
                <input
                  type="checkbox"
                  :checked="editorIntroEnabled"
                  class="size-4 rounded border-border accent-primary cursor-pointer"
                  @change="(e) => toggleEditorIntro((e.target as HTMLInputElement).checked)"
                />
              </label>
            </div>

            <div class="mt-5 rounded-xl border border-border bg-card/40 p-4">
              <h4 class="text-xs font-semibold text-foreground">Material Design 3 风格体系</h4>
              <p class="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                当前风格遵循 Google Material Design 3 规范设计，采用经典 Material 靛蓝（Royal Indigo）基准色体系。卡片与交互组件具备统一的圆角比例（12px）与双态平滑色彩过渡动效。
              </p>
            </div>
          </template>

          <!-- —— 关于 —— -->
          <template v-else>
            <div class="divide-y divide-border/60 rounded-xl border border-border bg-card/40 px-3.5">
              <div class="flex items-center justify-between gap-6 py-2.5">
                <div class="text-[13px] font-semibold">Octopus</div>
                <span class="text-xs text-muted-foreground">通用 AI RPG · v1 原型</span>
              </div>
              <div class="flex items-center justify-between gap-6 py-2.5">
                <div class="text-[13px] font-semibold">配置文件</div>
                <span class="font-mono text-xs text-muted-foreground">应用数据目录 / config.toml（0600）</span>
              </div>
              <div class="flex items-center justify-between gap-6 py-2.5">
                <div class="text-[13px] font-semibold">密钥存储</div>
                <span class="text-xs text-muted-foreground">本地配置文件，日志脱敏</span>
              </div>
              <div class="flex items-center justify-between gap-6 py-2.5">
                <div class="text-[13px] font-semibold">AI 模型</div>
                <span class="text-xs text-muted-foreground">AI / 结对各自可配</span>
              </div>
            </div>
          </template>
        </div>
      </div>

      <DialogFooter class="flex-row items-center justify-end gap-2 border-t border-border px-5 py-3">
        <Button variant="ghost" :disabled="store.saving" @click="close">取消</Button>
        <Button :disabled="store.saving || !config" @click="save">{{ store.saving ? '保存中…' : '保存配置' }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
