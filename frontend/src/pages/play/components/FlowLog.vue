<script setup lang="ts">
// 流程日志视图：按 seq 顺序展示本会话的原始事件。
// 与「对话」视图的区别：这里保留**全部**事件——包括不渲染进对话的引擎内部事件
// （被驳回的意图 / 阶段 / 思考 / 状态变更 / 系统消息），用于对演出流程做整体复盘。
//
// 每条事件可点击展开查看**完整 payload**（pi 式 span 属性）：
// - ai_call：结构化视图（请求上下文全文 / 思考链 / 用量 / 延迟 / 状态）；
// - 其它事件：完整 payload JSON + 一键复制。
import { ref } from 'vue'
import { IconChevronRight, IconCopy } from '@tabler/icons-vue'
import type { FlowLogLine } from '../stores/play'
import type { AiCallPayload } from '@/types'

defineProps<{ lines: FlowLogLine[] }>()

const TONE_CLASS: Record<FlowLogLine['tone'], string> = {
  info: 'text-foreground/85',
  warn: 'text-warning',
  error: 'text-destructive',
  muted: 'text-muted-foreground/60',
}
const TYPE_CLASS: Record<string, string> = {
  resolution: 'text-warning',
  check_result: 'text-primary',
  system: 'text-muted-foreground',
  ai_call: 'text-primary',
}
function hhmmss(ts?: string): string {
  if (!ts) return ''
  const d = new Date(ts)
  return isNaN(d.getTime()) ? '' : d.toLocaleTimeString('zh-CN', { hour12: false })
}

/** 展开状态：seq → 是否展开（只有带 detail 的行可展开） */
const expanded = ref<Set<number>>(new Set())
function toggle(seq: number): void {
  const next = new Set(expanded.value)
  if (next.has(seq)) next.delete(seq)
  else next.add(seq)
  expanded.value = next
}

/** 原始事件完整 JSON（复制用；带 detail 的行才有值）。 */
function detailJson(l: FlowLogLine): string {
  try { return JSON.stringify(l.detail, null, 2) } catch { return String(l.detail) }
}
function copyDetail(l: FlowLogLine): void {
  const text = detailJson(l)
  if (navigator.clipboard?.writeText) {
    void navigator.clipboard.writeText(text)
    return
  }
  const ta = document.createElement('textarea')
  ta.value = text
  document.body.appendChild(ta)
  ta.select()
  try { document.execCommand('copy') } catch { /* noop */ }
  document.body.removeChild(ta)
}

/** 判别：ai_call 事件且 detail 是结构化 payload。 */
function isAiCall(l: FlowLogLine): l is FlowLogLine & { detail: AiCallPayload } {
  return l.type === 'ai_call' && !!l.detail && typeof l.detail === 'object' && 'model' in l.detail
}
function roleClass(role: string): string {
  if (role === 'system') return 'bg-muted text-muted-foreground'
  if (role === 'user') return 'bg-primary/15 text-primary'
  return 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
}
</script>

<template>
  <div class="mx-auto w-[96%] max-w-[980px] py-3 font-mono text-[11.5px] leading-relaxed">
    <div v-if="!lines.length" class="py-10 text-center text-[12px] text-muted-foreground">
      本会话还没有事件。
    </div>

    <div
      v-for="l in lines"
      :key="l.seq"
      class="border-b border-border/40 hover:bg-muted/30"
    >
      <!-- 摘要行：点击展开 / 收起（带 detail 的行可见可点击的展开提示） -->
      <button
        type="button"
        class="flex w-full items-start gap-2 px-1 py-[3px] text-left transition-colors"
        :class="l.detail !== undefined ? 'cursor-pointer hover:bg-muted/40' : 'cursor-default'"
        :title="l.detail !== undefined ? (expanded.has(l.seq) ? '收起细节' : '展开细节') : undefined"
        @click="l.detail !== undefined && toggle(l.seq)"
      >
        <span class="w-9 shrink-0 text-right tabular-nums text-muted-foreground/45">{{ l.seq }}</span>
        <span class="w-10 shrink-0 tabular-nums text-muted-foreground/45">r{{ l.round }}</span>
        <span
          class="w-[104px] shrink-0 truncate"
          :class="TYPE_CLASS[l.type] ?? 'text-primary/70'"
          :title="l.type"
        >{{ l.type }}</span>
        <span class="w-[62px] shrink-0 tabular-nums text-muted-foreground/45">{{ hhmmss(l.ts) }}</span>
        <span class="min-w-0 flex-1 break-words whitespace-pre-wrap" :class="TONE_CLASS[l.tone]">{{ l.text }}</span>
        <IconChevronRight
          v-if="l.detail !== undefined"
          class="mt-0.5 size-3.5 shrink-0 text-muted-foreground/50 transition-transform"
          :class="expanded.has(l.seq) ? 'rotate-90 text-primary' : ''"
        />
        <span v-else class="w-3.5 shrink-0" />
      </button>

      <!-- 展开区：结构化细节 -->
      <div v-if="expanded.has(l.seq)" class="mb-1.5 ml-[21px] border-l-2 border-border/60 pl-3 pr-2">
        <!-- ai_call：结构化视图（请求属性 → 事件 → 状态） -->
        <template v-if="isAiCall(l)">
          <div class="flex flex-wrap items-center gap-x-4 gap-y-1 py-1.5 text-[11px]">
            <span
              class="rounded-full px-2 py-px text-[10px] font-bold"
              :class="l.detail.status === 'ok' ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400' : 'bg-destructive/15 text-destructive'"
            >{{ l.detail.status === 'ok' ? '成功' : '失败' }}</span>
            <span class="text-muted-foreground">供应商 <b class="text-foreground/85">{{ l.detail.provider }}</b></span>
            <span class="text-muted-foreground">模型 <b class="text-foreground/85">{{ l.detail.model }}</b></span>
            <span class="text-muted-foreground">温度 <b class="text-foreground/85">{{ l.detail.temperature }}</b></span>
            <span class="text-muted-foreground">max_tokens <b class="text-foreground/85">{{ l.detail.max_tokens }}</b></span>
            <span class="text-muted-foreground">延迟 <b class="text-foreground/85">{{ l.detail.latency_ms }}ms</b></span>
            <span
              v-if="(l.detail.attempts ?? 1) > 1"
              class="rounded-full bg-warning/15 px-2 py-px text-[10px] font-bold text-warning"
              :title="'意图解析失败后自动重试 ' + ((l.detail.attempts ?? 1) - 1) + ' 次'">
              自动重试 {{ (l.detail.attempts ?? 1) - 1 }} 次
            </span>
            <span class="text-muted-foreground">
              tokens <b class="text-foreground/85">{{ l.detail.usage.input_tokens }}→{{ l.detail.usage.output_tokens }}</b>
              <span class="text-muted-foreground/60">（缓存 {{ l.detail.usage.cached_input_tokens }} · 写缓存 {{ l.detail.usage.cache_creation_input_tokens }}）</span>
            </span>
          </div>

          <div v-if="l.detail.intents.length" class="flex flex-wrap items-center gap-1 py-1">
            <span class="text-muted-foreground/70">意图</span>
            <span
              v-for="i in l.detail.intents"
              :key="i"
              class="rounded bg-primary/10 px-1.5 py-px text-[10px] text-primary"
            >{{ i }}</span>
          </div>

          <div v-if="l.detail.warnings.length" class="py-1 text-[11px] text-warning">
            <div v-for="(w, i) in l.detail.warnings" :key="i">⚠ {{ w }}</div>
          </div>

          <div
            v-if="l.detail.error"
            class="rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-[11px] text-destructive"
          >{{ l.detail.error }}</div>

          <!-- 完整上下文（发给模型的全部消息） -->
          <details class="group mt-1.5">
            <summary class="cursor-pointer list-none text-[11px] text-muted-foreground transition-colors hover:text-foreground">
              <span class="inline-flex items-center gap-1">
                <IconChevronRight class="size-3 transition-transform group-open:rotate-90" />
                完整上下文（{{ l.detail.messages.length }} 条消息）
              </span>
            </summary>
            <div class="mt-1 space-y-1.5">
              <div
                v-for="(m, i) in l.detail.messages"
                :key="i"
                class="rounded-md border border-border/60 bg-card/60"
              >
                <div class="flex items-center gap-2 border-b border-border/40 px-2 py-0.5 text-[10px]">
                  <span class="rounded px-1.5 py-px font-bold" :class="roleClass(m.role)">#{{ i }} {{ m.role }}</span>
                  <span class="truncate text-muted-foreground/50">{{ m.content.length }} 字符</span>
                </div>
                <pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words px-2 py-1.5 text-[11px] text-foreground/85">{{ m.content }}</pre>
              </div>
            </div>
          </details>

          <!-- 思考链全文 -->
          <details v-if="l.detail.reasoning" class="group mt-1.5">
            <summary class="cursor-pointer list-none text-[11px] text-muted-foreground transition-colors hover:text-foreground">
              <span class="inline-flex items-center gap-1">
                <IconChevronRight class="size-3 transition-transform group-open:rotate-90" />
                思考链（{{ l.detail.reasoning.length }} 字符）
              </span>
            </summary>
            <pre class="mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border/60 bg-card/60 px-2 py-1.5 text-[11px] text-foreground/85">{{ l.detail.reasoning }}</pre>
          </details>
        </template>

        <!-- 其它事件：完整 payload JSON -->
        <template v-else>
          <div class="flex items-center justify-between py-1">
            <span class="text-[10px] text-muted-foreground/60">完整 payload</span>
            <button
              type="button"
              class="inline-flex cursor-pointer items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              @click="copyDetail(l)"
            >
              <IconCopy class="size-3" />复制
            </button>
          </div>
          <pre class="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border/60 bg-card/60 px-2 py-1.5 text-[11px] text-foreground/85">{{ detailJson(l) }}</pre>
        </template>

        <!-- ai_call 也提供原始 JSON 复制 -->
        <button
          v-if="isAiCall(l)"
          type="button"
          class="mt-1.5 inline-flex cursor-pointer items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          @click="copyDetail(l)"
        >
          <IconCopy class="size-3" />复制原始 JSON
        </button>
      </div>
    </div>
  </div>
</template>
