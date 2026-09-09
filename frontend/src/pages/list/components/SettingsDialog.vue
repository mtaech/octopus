<script setup lang="ts">
// 设置 · AI Provider（#26）：左分区导航 + 右行式配置。
// 供应商支持「列表 / 展开编辑 / 新增 / 删除」；密钥仅存本地配置文件（0600）。
import { computed, ref, watch } from 'vue'
import { useSettingsStore } from '../stores/settings'
import type { ProviderConfig, ProviderKind } from '@/types'
import { MODEL_CATALOG, PROVIDER_PRESETS } from '@/api/model-catalog'
import { catalogProvider, findModelProviders } from '@/api/model-catalog-utils'
import ModelPicker from './ModelPicker.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import {
  IconSettings, IconRobot, IconPlugConnected, IconCoin, IconInfoCircle,
  IconCircleCheck, IconAlertTriangle, IconLoader2, IconPlus, IconTrash, IconChevronRight,
} from '@tabler/icons-vue'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>()
const store = useSettingsStore()

type RoleKey = 'story' | 'character' | 'embedding'
const ROLES: { key: RoleKey; label: string; desc: string }[] = [
  { key: 'story', label: '主线 AI', desc: '故事走向、旁白与世界响应' },
  { key: 'character', label: '角色 AI', desc: '扮演人物言行（可用更便宜的模型）' },
  { key: 'embedding', label: 'Embedding', desc: '事件检索向量化（默认本地 bge-small-zh）' },
]

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: 'openai', label: 'OpenAI' },
  { value: 'anthropic', label: 'Anthropic' },
  { value: 'deepseek', label: 'DeepSeek' },
  { value: 'ollama', label: 'Ollama（本地）' },
  { value: 'openai-compatible', label: 'OpenAI 兼容端点' },
]

type SectionKey = 'roles' | 'providers' | 'budget' | 'about'
const NAV: { key: SectionKey; label: string; icon: unknown }[] = [
  { key: 'roles', label: '模型分工', icon: IconRobot },
  { key: 'providers', label: '供应商', icon: IconPlugConnected },
  { key: 'budget', label: '成本护栏', icon: IconCoin },
  { key: 'about', label: '关于', icon: IconInfoCircle },
]
const active = ref<SectionKey>('roles')

// 供应商：展开态 / 新增态
const expandedId = ref<string | null>(null)
const adding = ref(false)
const draft = ref<ProviderConfig>({ id: '', label: '', kind: 'openai', base_url: '', api_key: '', models: [] })
const catalogPick = ref('')

watch(() => props.open, (v) => {
  if (v) {
    active.value = 'roles'
    adding.value = false
    expandedId.value = null
    void store.load()
  }
})

const config = computed(() => store.config)
function modelsOf(providerId: string): string[] {
  return config.value?.providers.find(p => p.id === providerId)?.models ?? []
}
function onRoleProvider(key: RoleKey, providerId: string) {
  const c = config.value
  if (!c) return
  c.roles[key].provider_id = providerId
  c.roles[key].model = modelsOf(providerId)[0] ?? ''
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
  draft.value.models = p.models.map(m => m.id)
}

function startAdd() {
  draft.value = { id: '', label: '', kind: 'openai', base_url: '', api_key: '', models: [] }
  catalogPick.value = ''
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
function removeProvider(p: ProviderConfig) {
  const c = config.value
  if (!c) return
  if (!window.confirm(`删除供应商「${p.label}」？引用了它的模型分工将回退到第一个供应商。`)) return
  c.providers = c.providers.filter(x => x.id !== p.id)
  const fallback = c.providers[0]?.id ?? ''
  ;(['story', 'character', 'embedding'] as RoleKey[]).forEach(k => {
    if (c.roles[k].provider_id === p.id) {
      c.roles[k].provider_id = fallback
      c.roles[k].model = modelsOf(fallback)[0] ?? ''
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
            class="flex shrink-0 items-center gap-2 rounded-lg px-3 py-2 text-left text-[13px] font-medium transition-colors sm:w-full"
            :class="active === n.key ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'"
            @click="active = n.key"
          >
            <component :is="n.icon" aria-hidden="true" class="size-4" :class="active === n.key ? 'text-primary' : ''" />
            {{ n.label }}
          </button>
        </nav>

        <!-- ============ 右：内容 ============ -->
        <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          <!-- —— 模型分工 —— -->
          <template v-if="active === 'roles'">
            <p class="mb-4 text-xs leading-relaxed text-muted-foreground">
              三类模型各自可配。主线用强模型、角色用更便宜的模型、Embedding 固定本地，符合 #05/#15 的预算与选型。
            </p>
            <section v-for="r in ROLES" :key="r.key" class="mb-5 last:mb-0">
              <h3 class="mb-1.5 text-[11px] font-bold tracking-[0.14em] text-muted-foreground uppercase">{{ r.label }}</h3>
              <div class="divide-y divide-border/60 rounded-xl border border-border bg-card/40 px-3.5">
                <div class="flex items-center justify-between gap-6 py-2.5">
                  <div class="min-w-0">
                    <div class="text-[13px] font-semibold">供应商</div>
                    <div class="mt-0.5 text-xs text-muted-foreground">{{ r.desc }}</div>
                  </div>
                  <div class="w-52 shrink-0">
                    <Select :model-value="config.roles[r.key].provider_id" @update:model-value="(v) => { if (typeof v === 'string') onRoleProvider(r.key, v) }">
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
                    <Select :model-value="config.roles[r.key].model" @update:model-value="(v) => { if (typeof v === 'string') config!.roles[r.key].model = v }">
                      <SelectTrigger class="w-full"><SelectValue placeholder="选择模型" /></SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem v-for="m in modelsOf(config.roles[r.key].provider_id)" :key="m" :value="m">{{ m }}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                </div>

                <template v-if="r.key !== 'embedding'">
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">温度</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">越高越发散，0–2</div>
                    </div>
                    <div class="w-28 shrink-0">
                      <Input v-model.number="config.roles[r.key].temperature" type="number" step="0.1" min="0" max="2" class="h-9 text-right" />
                    </div>
                  </div>
                  <div class="flex items-center justify-between gap-6 py-2.5">
                    <div class="min-w-0">
                      <div class="text-[13px] font-semibold">最大输出 tokens</div>
                      <div class="mt-0.5 text-xs text-muted-foreground">单次回复的上限</div>
                    </div>
                    <div class="w-28 shrink-0">
                      <Input v-model.number="config.roles[r.key].max_tokens" type="number" min="256" step="256" class="h-9 text-right" />
                    </div>
                  </div>
                </template>
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
                <Select :model-value="catalogPick" @update:model-value="(v) => { if (typeof v === 'string') { catalogPick = v; applyCatalog(v) } }">
                  <SelectTrigger class="w-full"><SelectValue placeholder="选择一个已知供应商…" /></SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem v-for="cp in MODEL_CATALOG" :key="cp.id" :value="cp.id">{{ cp.label }} · {{ cp.models.length }} 个模型</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </div>
              <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <label class="flex flex-col gap-1.5">
                  <span class="text-[11px] font-semibold text-muted-foreground">名称</span>
                  <Input v-model="draft.label" placeholder="例如 DeepSeek / 本地 Ollama" class="h-9" />
                </label>
                <label class="flex flex-col gap-1.5">
                  <span class="text-[11px] font-semibold text-muted-foreground">类型</span>
                  <Select :model-value="draft.kind" @update:model-value="(v) => { if (typeof v === 'string') draft.kind = v as ProviderKind }">
                    <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem v-for="k in KINDS" :key="k.value" :value="k.value">{{ k.label }}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
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
                    :provider-url="draft.base_url"
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
                    :provider-url="p.base_url"
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
                <div class="text-[13px] font-semibold">模型分工</div>
                <span class="text-xs text-muted-foreground">主线 / 角色 / Embedding 三类各自可配</span>
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
