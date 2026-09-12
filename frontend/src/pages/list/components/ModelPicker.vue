<script setup lang="ts">
// 模型清单编辑器：探测（GET {base_url}/models）/ 逐行编辑（id + 显示名）/ 恢复目录默认。
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconPlus, IconTrash, IconWand, IconLoader2, IconAdjustments } from '@tabler/icons-vue'
import { toast } from '@/api'
import { probeProviderModels } from '@/api'
import { catalogEntries, catalogProvider, detectProviderByUrl, fmtContext } from '@/api/model-catalog-utils'
import type { ModelEntry, ProviderConfig } from '@/types'

const props = defineProps<{
  modelValue: ModelEntry[]
  provider: Pick<ProviderConfig, 'base_url' | 'api_key' | 'kind'>
}>()
const emit = defineEmits<{
  (e: 'update:modelValue', v: ModelEntry[]): void
  (e: 'pick', modelId: string): void
}>()

const tried = ref(false)
const probing = ref(false)
const detectedId = computed(() => detectProviderByUrl(props.provider.base_url ?? ''))
const detectedName = computed(() => (detectedId.value ? catalogProvider(detectedId.value)?.label ?? detectedId.value : ''))
const detectedCount = computed(() => (detectedId.value ? catalogEntries(detectedId.value).length : 0))
const models = computed(() => props.modelValue ?? [])

function addRow() { emit('update:modelValue', [...models.value, { id: '', name: '' }]) }
function removeAt(i: number) { emit('update:modelValue', models.value.filter((_, idx) => idx !== i)) }
function restoreDefaults() {
  tried.value = true
  const pid = detectedId.value
  if (!pid) return
  emit('update:modelValue', catalogEntries(pid).map(m => ({ ...m })))
}
function onIdChange(id: string) { if (id.trim()) emit('pick', id.trim()) }

// ---- 自定义模型元数据（小中转站 / 目录未收录的模型） ----
const expanded = ref<number | null>(null)
function toggleMeta(i: number) { expanded.value = expanded.value === i ? null : i }
function metaSummary(m: ModelEntry): string {
  const bits: string[] = []
  if (m.ctx) bits.push(fmtContext(m.ctx))
  if (m.reasoning === true) bits.push('思考')
  if (m.reasoning === false) bits.push('非思考')
  if (m.tl) bits.push(Object.keys(m.tl).join('/'))
  return bits.join(' · ')
}
function levelsText(m: ModelEntry): string {
  if (!m.tl) return ''
  return Object.entries(m.tl).map(([k, v]) => (v == null || v === k ? k : `${k}=${v}`)).join(', ')
}
/** 解析「等级 或 等级=下发值」；留空 = 不发送该参数。 */
function setLevels(m: ModelEntry, text: string) {
  const tl: Record<string, string | null> = {}
  for (const part of text.split(/[,，;；\n]/).map(s => s.trim()).filter(Boolean)) {
    const [k, v] = part.split('=').map(s => s.trim())
    if (k) tl[k] = v && v.length ? v : null
  }
  m.tl = Object.keys(tl).length ? tl : undefined
}
function setNum(m: ModelEntry, key: 'ctx' | 'maxOut', v: string) {
  const n = Number(v)
  m[key] = v.trim() && Number.isFinite(n) && n > 0 ? Math.round(n) : undefined
}

/** 探测：调用 models 端点拉取可用模型 */
async function probe() {
  if (probing.value) return
  probing.value = true
  try {
    const res = await probeProviderModels(props.provider)
    emit('update:modelValue', res.models.map(m => ({ ...m })))
    toast('ok', `探测到 ${res.models.length} 个模型（${res.source}）`)
  } catch (e) {
    tried.value = true
    toast('error', (e as Error)?.message ?? '探测失败')
  } finally {
    probing.value = false
  }
}
</script>

<template>
  <div class="w-full">
    <div class="mb-1.5 flex items-center gap-2">
      <span v-if="detectedId" class="text-[11px] text-success">已识别：{{ detectedName }} · {{ detectedCount }} 个模型</span>
      <span v-else class="text-[11px] text-muted-foreground">已配置 {{ models.length }} 个模型</span>
      <div class="ml-auto flex shrink-0 items-center gap-2">
        <button type="button" class="text-[11px] text-muted-foreground transition-colors hover:text-primary" @click="restoreDefaults">恢复默认模型</button>
        <Button size="xs" variant="outline" :disabled="probing || !provider.base_url" @click="probe">
          <IconLoader2 v-if="probing" data-icon="inline-start" class="animate-spin" />
          <IconWand v-else data-icon="inline-start" />
          {{ probing ? '探测中…' : '探测模型' }}
        </Button>
        <Button size="xs" variant="outline" @click="addRow"><IconPlus data-icon="inline-start" />添加模型</Button>
      </div>
    </div>

    <div v-if="models.length" class="flex flex-col gap-1.5">
      <div v-for="(m, i) in models" :key="i" class="flex flex-col gap-1.5 rounded-lg">
        <div class="flex items-center gap-1.5">
          <Input v-model="m.id" placeholder="模型 id（如 deepseek-chat）" class="h-9 flex-1 font-mono text-[12.5px]" @change="onIdChange(m.id)" />
          <Input v-model="m.name" placeholder="显示名（可选）" class="h-9 flex-1" />
          <span v-if="metaSummary(m)" class="shrink-0 rounded-full bg-muted/70 px-1.5 font-mono text-[9.5px] text-muted-foreground/80">{{ metaSummary(m) }}</span>
          <Button size="icon-sm" variant="ghost" class="shrink-0" :class="expanded === i ? 'text-primary' : 'text-muted-foreground'" title="自定义上下文 / 思考等级（小中转站）" @click="toggleMeta(i)">
            <IconAdjustments />
          </Button>
          <Button size="icon-sm" variant="ghost" class="shrink-0 text-muted-foreground hover:text-destructive" title="删除模型" @click="removeAt(i)">
            <IconTrash />
          </Button>
        </div>

        <!-- 自定义元数据：留空 = 用目录默认 -->
        <div v-if="expanded === i" class="grid grid-cols-1 gap-2 rounded-lg border border-border/70 bg-muted/20 p-2.5 sm:grid-cols-2">
          <label class="flex flex-col gap-1">
            <span class="text-[10.5px] font-semibold text-muted-foreground">上下文窗口（tokens）</span>
            <Input :model-value="m.ctx ?? ''" placeholder="如 128000；留空用目录" class="h-8 font-mono text-[12px]" @update:model-value="(v) => setNum(m, 'ctx', String(v))" />
          </label>
          <label class="flex flex-col gap-1">
            <span class="text-[10.5px] font-semibold text-muted-foreground">最大输出（tokens）</span>
            <Input :model-value="m.maxOut ?? ''" placeholder="如 8192；留空同上下文" class="h-8 font-mono text-[12px]" @update:model-value="(v) => setNum(m, 'maxOut', String(v))" />
          </label>
          <div class="flex flex-col gap-1">
            <span class="text-[10.5px] font-semibold text-muted-foreground">是否支持思考</span>
            <div class="flex items-center gap-1">
              <button type="button" class="rounded-full border px-2 py-0.5 text-[10.5px] transition-colors" :class="m.reasoning === undefined ? 'border-primary/50 bg-primary/10 text-primary' : 'border-border/80 text-muted-foreground hover:bg-muted/60'" @click="m.reasoning = undefined">跟随目录</button>
              <button type="button" class="rounded-full border px-2 py-0.5 text-[10.5px] transition-colors" :class="m.reasoning === true ? 'border-primary/50 bg-primary/10 text-primary' : 'border-border/80 text-muted-foreground hover:bg-muted/60'" @click="m.reasoning = true">支持</button>
              <button type="button" class="rounded-full border px-2 py-0.5 text-[10.5px] transition-colors" :class="m.reasoning === false ? 'border-primary/50 bg-primary/10 text-primary' : 'border-border/80 text-muted-foreground hover:bg-muted/60'" @click="m.reasoning = false">不支持</button>
            </div>
          </div>
          <label class="flex flex-col gap-1">
            <span class="text-[10.5px] font-semibold text-muted-foreground">思考等级（逗号分隔，可写 等级=下发值）</span>
            <Input :model-value="levelsText(m)" placeholder="如 low, medium, high=max" class="h-8 font-mono text-[12px]" @update:model-value="(v) => setLevels(m, String(v))" />
          </label>
          <p class="col-span-full text-[10.5px] leading-relaxed text-muted-foreground/70">小中转站 / 目录未收录的模型：在这里自定义上下文与思考等级；留空则沿用目录默认。只写等级名表示下发同名字段，写「等级=下发值」可映射到不同值。</p>
        </div>
      </div>
    </div>
    <div v-else class="rounded-lg border border-dashed border-border px-3 py-4 text-center text-[11.5px] text-muted-foreground">
      还没有模型。点「探测模型」从端点拉取，或「添加模型」手填。
    </div>
  </div>
</template>
