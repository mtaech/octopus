<script setup lang="ts">
// 提示词设置：把各处发往模型的固定文本集中暴露给创作者自定义（GET /api/prompts）。
//
// 约定：默认值由后端注册表提供；本组件只写「覆盖」，清空文本框 = 恢复默认。
// 改动落在 store.config.prompts，随「保存」一起 PUT /api/config，后端即改即生效。
import { computed, onMounted, ref } from 'vue'
import { useSettingsStore } from '../stores/settings'
import { getPromptCatalog, toast } from '@/api'
import type { PromptDef } from '@/types'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Textarea } from '@/components/ui/textarea'
import { IconChevronRight, IconLoader2, IconRefresh, IconSparkles } from '@tabler/icons-vue'

const store = useSettingsStore()
const defs = ref<PromptDef[]>([])
const loading = ref(false)
const loadError = ref('')
const openKey = ref<string | null>(null)
const showDefault = ref<Record<string, boolean>>({})

const groups = computed(() => {
  const map = new Map<string, PromptDef[]>()
  for (const d of defs.value) {
    const list = map.get(d.group)
    if (list) list.push(d)
    else map.set(d.group, [d])
  }
  return [...map.entries()].map(([group, items]) => ({ group, items }))
})

/** 覆盖表（直接挂在全局配置上，随设置保存一起提交）。 */
const overrides = computed<Record<string, string>>(() => {
  const cfg = store.config
  if (!cfg) return {}
  if (!cfg.prompts) cfg.prompts = {}
  return cfg.prompts
})

const customCount = computed(() => defs.value.filter(d => isCustom(d)).length)

function textOf(d: PromptDef): string {
  return overrides.value[d.key] ?? ''
}
function setText(d: PromptDef, v: string) {
  const o = overrides.value
  if (v.trim() === '') delete o[d.key]
  else o[d.key] = v
}
function reset(d: PromptDef) {
  delete overrides.value[d.key]
}
function isCustom(d: PromptDef): boolean {
  return (overrides.value[d.key] ?? '').trim() !== ''
}
function toggle(key: string) {
  openKey.value = openKey.value === key ? null : key
}
function toggleDefault(key: string) {
  showDefault.value[key] = !showDefault.value[key]
}
/** 模板占位符文本（在脚本里拼，避免模板插值里的花括号把解析器带偏）。 */
function varText(name: string): string {
  return '{{' + name + '}}'
}

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    defs.value = await getPromptCatalog()
    if (!defs.value.length) {
      loadError.value = '未取到提示词目录：当前处于演示（mock）模式，或后端未启动。连接后端后这里会列出全部可自定义提示词。'
    }
  } catch (e) {
    loadError.value = '读取提示词目录失败：' + ((e as Error)?.message ?? String(e))
  } finally {
    loading.value = false
  }
}

function resetAll() {
  if (!store.config) return
  store.config.prompts = {}
  toast('ok', '已恢复全部默认提示词（点「保存」后生效）')
}

onMounted(load)
</script>

<template>
  <div>
    <p class="mb-4 text-xs leading-relaxed text-muted-foreground">
      这里集中管理所有发往模型的固定文本：<span class="font-semibold text-foreground">游玩（聊天）AI</span> 与
      <span class="font-semibold text-foreground">结对 AI</span>。默认值由引擎提供，填写即覆盖该条；清空文本框即恢复默认，改动点「保存」后生效。
    </p>

    <div v-if="loading" class="flex h-32 items-center justify-center gap-2 text-xs text-muted-foreground">
      <IconLoader2 aria-hidden="true" class="size-4 animate-spin text-primary/70" />
      正在读取提示词目录…
    </div>

    <div v-else-if="loadError" class="rounded-xl border border-dashed border-border px-4 py-8 text-center text-xs leading-relaxed text-muted-foreground">
      {{ loadError }}
    </div>

    <template v-else>
      <div class="mb-4 flex items-center justify-between gap-4">
        <div class="text-xs text-muted-foreground">
          共 {{ defs.length }} 条，已自定义
          <span class="font-semibold text-primary">{{ customCount }}</span> 条
        </div>
        <Button variant="outline" size="sm" :disabled="!customCount" @click="resetAll">
          <IconRefresh aria-hidden="true" class="size-3.5" />
          全部恢复默认
        </Button>
      </div>

      <section v-for="g in groups" :key="g.group" class="mb-6 last:mb-0">
        <h3 class="mb-2 flex items-center gap-1.5 text-[11px] font-bold tracking-[0.14em] text-muted-foreground uppercase">
          <IconSparkles aria-hidden="true" class="size-3.5 text-primary/70" />
          {{ g.group }}
        </h3>
        <div class="overflow-hidden rounded-xl border border-border bg-card/40">
          <div v-for="d in g.items" :key="d.key" class="border-b border-border/60 last:border-b-0">
            <button
              type="button"
              class="flex w-full cursor-pointer items-center gap-3 px-3.5 py-2.5 text-left transition-colors hover:bg-accent/50"
              :aria-expanded="openKey === d.key"
              @click="toggle(d.key)"
            >
              <IconChevronRight
                aria-hidden="true"
                class="size-4 shrink-0 text-muted-foreground transition-transform"
                :class="openKey === d.key ? 'rotate-90' : ''"
              />
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-2">
                  <span class="truncate text-[13px] font-semibold">{{ d.label }}</span>
                  <Badge v-if="isCustom(d)" variant="secondary" class="shrink-0">已自定义</Badge>
                </span>
                <span class="mt-0.5 block truncate font-mono text-[11px] text-muted-foreground/70">{{ d.key }}</span>
              </span>
            </button>

            <div v-if="openKey === d.key" class="border-t border-border/60 bg-background/40 px-3.5 py-3">
              <p class="mb-2.5 text-xs leading-relaxed text-muted-foreground">{{ d.description }}</p>

              <div v-if="d.variables.length" class="mb-2.5">
                <div class="mb-1.5 text-[11px] font-semibold text-muted-foreground">
                  可用变量（写进模板即被替换成对应内容）
                </div>
                <div class="flex flex-wrap gap-1.5">
                  <span
                    v-for="v in d.variables"
                    :key="v.name"
                    class="inline-flex items-center gap-1.5 rounded-md border border-border bg-muted/40 px-1.5 py-0.5"
                    :title="v.description"
                  >
                    <code class="font-mono text-[11px] text-primary">{{ varText(v.name) }}</code>
                    <span class="text-[11px] text-muted-foreground">{{ v.description }}</span>
                  </span>
                </div>
              </div>

              <Textarea
                :model-value="textOf(d)"
                placeholder="留空 = 使用内置默认"
                class="min-h-40 font-mono text-[12px] leading-relaxed"
                @update:model-value="(v) => setText(d, String(v))"
              />

              <div class="mt-2.5 flex flex-wrap items-center gap-2">
                <Button variant="outline" size="sm" :disabled="!isCustom(d)" @click="reset(d)">
                  <IconRefresh aria-hidden="true" class="size-3.5" />
                  恢复默认
                </Button>
                <Button variant="ghost" size="sm" @click="toggleDefault(d.key)">
                  {{ showDefault[d.key] ? '隐藏内置默认' : '查看内置默认' }}
                </Button>
                <span class="text-[11px] text-muted-foreground">覆盖文本随「保存」写入本地配置文件</span>
              </div>

              <pre
                v-if="showDefault[d.key]"
                class="mt-2.5 max-h-64 overflow-auto rounded-lg border border-border/70 bg-muted/30 p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap text-foreground/80"
              >{{ d.default }}</pre>
            </div>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>
