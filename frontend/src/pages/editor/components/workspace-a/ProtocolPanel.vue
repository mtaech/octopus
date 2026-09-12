<script setup lang="ts">
// ProtocolPanel —— A「协议」tab：故事书覆盖 AI 输出格式（叙事契约 P2）。
// 三种模式：default（引擎内置）、declarative（意图白名单 + 补充说明）、lua（插件源码）。
// 硬约束：出口永远是引擎意图数组；Lua 走既有沙箱，与技能脚本同一套白名单。
import { computed } from 'vue'
import { useEditorStore } from '../../stores/editor'
import type { ProtocolConfig, ProtocolMode } from '@/types'
import { buildLuaContext, lintLuaSource } from '@/lib/lua-context'
import FieldGrid from '../fields/FieldGrid.vue'
import FieldSelect from '../fields/FieldSelect.vue'
import FieldArea from '../fields/FieldArea.vue'
import CodeEditor from '@/components/CodeEditor.vue'
import { IconCode, IconAlertTriangle, IconShieldCheck } from '@tabler/icons-vue'

const editor = useEditorStore()
const d = computed(() => editor.draft)
const luaContext = computed(() => buildLuaContext(d.value))

const protocol = computed<ProtocolConfig | null>(() => d.value?.narrative?.protocol ?? null)
const mode = computed<ProtocolMode>(() => protocol.value?.mode ?? 'default')

const MODE_OPTIONS = [
  { value: 'default', label: '默认（引擎内置协议）' },
  { value: 'declarative', label: '声明式（意图白名单 + 说明）' },
  { value: 'lua', label: 'Lua 插件（完全自定义格式）' },
]

/** 引擎已知意图集合（与 octopus-engine::protocol::KNOWN_INTENTS 保持一致）。 */
const KNOWN_INTENTS: { value: string; label: string }[] = [
  { value: 'narrate', label: '旁白' },
  { value: 'speak', label: '台词' },
  { value: 'emote', label: '神态动作' },
  { value: 'check', label: '判定' },
  { value: 'move', label: '移动' },
  { value: 'use_skill', label: '使用技能' },
  { value: 'use_item', label: '使用物品' },
  { value: 'strike', label: '攻击（引擎结算）' },
  { value: 'advance_scene', label: '推进场景' },
  { value: 'query_world', label: '查询世界' },
  { value: 'intervene', label: '介入' },
  { value: 'quest', label: '新增任务（导演）' },
  { value: 'encounter', label: '创建遭遇（导演）' },
  { value: 'adjust', label: '调整资源（导演）' },
  { value: 'status', label: '施加 / 移除状态（导演）' },
]
const NARRATIVE_INTENTS = ['narrate', 'speak', 'emote']

/** 编辑即落地：首次修改时才创建 protocol 对象，缺省保持「无字段 = 引擎默认协议」。 */
function ensure(): ProtocolConfig {
  const sb = d.value!
  if (!sb.narrative) sb.narrative = { sections: [] }
  if (!sb.narrative.protocol) sb.narrative.protocol = { mode: 'default', intents: [], instructions: '', lua: '' }
  return sb.narrative.protocol
}
function setMode(m: string): void {
  ensure().mode = m as ProtocolMode
}
function toggleIntent(v: string): void {
  const p = ensure()
  const list = p.intents ?? (p.intents = [])
  const i = list.indexOf(v)
  if (i >= 0) list.splice(i, 1)
  else list.push(v)
}
function setInstructions(v: string): void {
  ensure().instructions = v
}
function setLua(v: string): void {
  ensure().lua = v
}

const intents = computed<string[]>(() => protocol.value?.intents ?? [])
const hasNarrativeIntent = computed(() => intents.value.some(i => NARRATIVE_INTENTS.includes(i)))
const lintViolations = computed(() => lintLuaSource(protocol.value?.lua))
const luaMissing = computed(() => {
  const src = protocol.value?.lua ?? ''
  const missing: string[] = []
  if (!src.includes('protocol.preamble')) missing.push('protocol.preamble')
  if (!src.includes('protocol.parse')) missing.push('protocol.parse')
  return missing
})
</script>

<template>
  <div class="h-full overflow-y-auto">
    <div class="mx-auto w-full max-w-4xl px-5 py-5">
      <header class="mb-4">
        <div class="flex items-center gap-2">
          <IconCode class="size-4 text-primary" />
          <h2 class="text-sm font-semibold text-foreground">输出协议</h2>
        </div>
        <p class="mt-1 text-xs leading-5 text-muted-foreground">
          故事书可覆盖 AI 的输出格式说明与解析；无论怎么自定义，出口都是引擎意图数组，结算层不感知协议模式。
        </p>
      </header>

      <FieldGrid>
        <FieldSelect
          label="协议模式"
          :model-value="mode"
          :options="MODE_OPTIONS"
          :allow-empty="false"
          hint="默认 = 引擎内置；声明式 = 引擎模板 + 白名单；Lua = 插件自定义格式"
          @update:model-value="setMode"
        />
      </FieldGrid>

      <!-- 默认模式：只读说明 -->
      <div v-if="mode === 'default'" class="mt-5 rounded-lg border border-border/70 bg-muted/20 px-3.5 py-3 text-xs leading-5 text-muted-foreground">
        使用引擎内置协议：模型输出意图 JSON 数组，由引擎解析与校验。适合绝大多数故事书。
      </div>

      <!-- 声明式：意图白名单 + 补充说明 -->
      <section v-else-if="mode === 'declarative'" class="mt-5 space-y-4">
        <div>
          <div class="mb-1.5 flex items-center justify-between gap-2">
            <span class="text-[11px] font-medium text-muted-foreground">允许的意图</span>
            <span v-if="!hasNarrativeIntent" class="flex items-center gap-1 text-[10.5px] text-warning">
              <IconAlertTriangle class="size-3.5" />至少选一个叙事意图（旁白 / 台词 / 神态动作）
            </span>
          </div>
          <div class="flex flex-wrap gap-1.5">
            <button
              v-for="it in KNOWN_INTENTS"
              :key="it.value"
              type="button"
              class="cursor-pointer rounded-md border px-2 py-1 font-mono text-[11.5px] transition-colors"
              :class="intents.includes(it.value)
                ? 'border-primary/50 bg-primary/12 text-primary'
                : 'border-border/70 bg-card/40 text-muted-foreground hover:border-primary/40 hover:text-foreground'"
              :title="'点击' + (intents.includes(it.value) ? '移除' : '加入') + '：' + it.value"
              @click="toggleIntent(it.value)"
            >
              {{ it.value }}<span class="ml-1 font-sans text-[10.5px] opacity-70">{{ it.label }}</span>
            </button>
            <span class="cursor-default rounded-md border border-dashed border-border/70 bg-muted/20 px-2 py-1 font-mono text-[11.5px] text-muted-foreground/70" title="回合收束由引擎始终保留，不能禁用">
              finish_turn<span class="ml-1 font-sans text-[10.5px]">始终保留</span>
            </span>
          </div>
          <p class="mt-1.5 text-[10.5px] leading-4 text-muted-foreground/65">
            白名单之外的意图会被过滤，并在游玩时记一条 <code class="font-mono">intent_not_allowed</code> 系统警告。
          </p>
        </div>

        <FieldArea
          label="补充说明（可选）"
          :rows="6"
          md
          :model-value="protocol?.instructions ?? ''"
          placeholder="如：台词必须带 actor_id；每次最多两个意图……"
          hint="追加到协议说明末尾；计入每回合 token 预算，超预算时会被截断"
          @update:model-value="setInstructions"
        />
      </section>

      <!-- Lua 插件 -->
      <section v-else class="mt-5 space-y-3">
        <div
          v-if="lintViolations.length === 0"
          class="flex items-center gap-1.5 rounded-lg border border-emerald-500/25 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-600 dark:text-emerald-400"
        >
          <IconShieldCheck class="size-4" />
          <span>沙箱合规：未使用白名单外的 Lua API。</span>
        </div>
        <div v-else class="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-xs text-destructive">
          <div class="flex items-center gap-1.5 font-semibold">
            <IconAlertTriangle class="size-4 shrink-0" />
            <span>检测到白名单外或违规 Lua API（无法通过引擎预检）：</span>
          </div>
          <div v-for="v in lintViolations" :key="v.term" class="mt-1 pl-5 font-mono">
            • <span class="font-bold underline">{{ v.term }}</span>：{{ v.tip }}
          </div>
        </div>
        <div v-if="luaMissing.length" class="rounded-lg border border-warning/25 bg-warning/5 px-3 py-2 text-xs text-warning">
          插件需定义 {{ luaMissing.join(' 与 ') }}；发布时会用固定样例做精确一致性检查。
        </div>

        <div class="rounded-xl border border-border/80 bg-card/60 overflow-hidden">
          <div class="border-b border-border/70 bg-muted/30 px-3.5 py-2 text-[11px] font-semibold text-muted-foreground">
            协议插件源码（Lua）
          </div>
          <div class="p-2">
            <CodeEditor
              :model-value="protocol?.lua ?? ''"
              language="lua"
              min-height="14rem"
              max-height="30rem"
              :context="luaContext"
              show-reference
              placeholder="function protocol.preamble(ctx) return '...' end&#10;function protocol.parse(raw) ... end&#10;function protocol.normalize(intents, ctx) ... end"
              @update:model-value="setLua"
            />
          </div>
        </div>
        <p class="text-[10.5px] leading-4 text-muted-foreground/65">
          只读沙箱：无 io / os / package / debug，RNG 走 host.engine_rng；协议挂载点不提供任何世界写入 API。
          必选 <code class="font-mono">protocol.preamble(ctx)</code>、<code class="font-mono">protocol.parse(raw)</code>，可选 <code class="font-mono">protocol.normalize(intents, ctx)</code>。
        </p>
      </section>
    </div>
  </div>
</template>
