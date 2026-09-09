<script setup lang="ts">
// 设置 · AI Provider（#26）：三类模型分工 + 供应商凭据 + 成本护栏
// 密钥仅存本地配置文件（0600），日志脱敏；此处为原型交互。
import { computed, watch } from 'vue'
import { useSettingsStore } from '../stores/settings'
import type { ProviderConfig } from '@/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { IconCircleCheck, IconAlertTriangle, IconLoader2, IconSettings, IconPlugConnected } from '@tabler/icons-vue'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ (e: 'update:open', v: boolean): void }>()
const store = useSettingsStore()

type RoleKey = 'story' | 'character' | 'embedding'
const ROLES: { key: RoleKey; label: string; desc: string }[] = [
  { key: 'story', label: '主线 AI', desc: '故事走向、旁白与世界响应' },
  { key: 'character', label: '角色 AI', desc: '扮演人物言行（可用更便宜的模型）' },
  { key: 'embedding', label: 'Embedding', desc: '事件检索向量化（默认本地 bge-small-zh）' },
]

watch(() => props.open, (v) => { if (v) void store.load() })

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
function setModels(p: ProviderConfig, v: string) {
  p.models = v.split(',').map(s => s.trim()).filter(Boolean)
}
function modelsText(p: ProviderConfig): string { return p.models.join(', ') }

function close() { if (!store.saving) emit('update:open', false) }
async function save() { if (await store.save()) close() }
</script>

<template>
  <Dialog :open="open" @update:open="() => close()">
    <DialogContent class="max-h-[86vh] overflow-y-auto sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-xl">
          <IconSettings aria-hidden="true" class="size-5 text-primary" />
          设置 · AI Provider
        </DialogTitle>
        <DialogDescription>
          主线 AI / 角色 AI / Embedding 三类模型可分别配置。API Key 保存于本地配置文件（0600），日志与错误信息中一律脱敏。
        </DialogDescription>
      </DialogHeader>

      <div v-if="!config" class="py-8 text-center text-sm text-muted-foreground">
        <IconLoader2 aria-hidden="true" class="mx-auto size-5 animate-spin text-primary/70" />
        <div class="mt-2">正在读取配置…</div>
      </div>

      <template v-else>
        <!-- 角色分工 -->
        <section class="mt-2">
          <h3 class="text-[11px] font-bold tracking-[0.18em] text-muted-foreground uppercase">模型分工</h3>
          <div class="mt-2 flex flex-col gap-3">
            <div v-for="r in ROLES" :key="r.key" class="rounded-xl border border-border bg-card/60 p-3">
              <div class="mb-2 flex items-baseline gap-2">
                <span class="text-sm font-bold">{{ r.label }}</span>
                <span class="text-xs text-muted-foreground">{{ r.desc }}</span>
              </div>
              <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <div class="flex flex-col gap-1.5">
                  <Label class="text-xs text-muted-foreground">供应商</Label>
                  <Select :model-value="config.roles[r.key].provider_id" @update:model-value="(v) => { if (typeof v === 'string') onRoleProvider(r.key, v) }">
                    <SelectTrigger class="w-full"><SelectValue placeholder="选择供应商" /></SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem v-for="p in config.providers" :key="p.id" :value="p.id">{{ p.label }}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </div>
                <div class="flex flex-col gap-1.5">
                  <Label class="text-xs text-muted-foreground">模型</Label>
                  <Select :model-value="config.roles[r.key].model" @update:model-value="(v) => { if (typeof v === 'string') config!.roles[r.key].model = v }">
                    <SelectTrigger class="w-full"><SelectValue placeholder="选择模型" /></SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem v-for="m in modelsOf(config.roles[r.key].provider_id)" :key="m" :value="m">{{ m }}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </div>
                <template v-if="r.key !== 'embedding'">
                  <div class="flex flex-col gap-1.5">
                    <Label class="text-xs text-muted-foreground">温度</Label>
                    <Input v-model.number="config.roles[r.key].temperature" type="number" step="0.1" min="0" max="2" />
                  </div>
                  <div class="flex flex-col gap-1.5">
                    <Label class="text-xs text-muted-foreground">最大输出 tokens</Label>
                    <Input v-model.number="config.roles[r.key].max_tokens" type="number" min="256" step="256" />
                  </div>
                </template>
              </div>
            </div>
          </div>
        </section>

        <!-- 供应商 -->
        <section class="mt-5">
          <h3 class="text-[11px] font-bold tracking-[0.18em] text-muted-foreground uppercase">供应商</h3>
          <div class="mt-2 flex flex-col gap-3">
            <div v-for="p in config.providers" :key="p.id" class="rounded-xl border border-border bg-card/60 p-3">
              <div class="mb-2 flex items-center gap-2">
                <span class="text-sm font-bold">{{ p.label }}</span>
                <Badge variant="outline" class="font-mono text-[10px]">{{ p.kind }}</Badge>
                <Button size="xs" variant="outline" class="ml-auto" :disabled="store.tests[p.id]?.testing" @click="store.test(p)">
                  <IconLoader2 v-if="store.tests[p.id]?.testing" data-icon="inline-start" class="animate-spin" />
                  <IconPlugConnected v-else data-icon="inline-start" />
                  测试
                </Button>
              </div>
              <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <div class="flex flex-col gap-1.5">
                  <Label class="text-xs text-muted-foreground">Base URL</Label>
                  <Input v-model="p.base_url" placeholder="https://…" />
                </div>
                <div class="flex flex-col gap-1.5">
                  <Label class="text-xs text-muted-foreground">API Key</Label>
                  <Input v-model="p.api_key" type="password" :placeholder="p.kind === 'ollama' ? '（本地无需）' : 'sk-…'" />
                </div>
                <div class="flex flex-col gap-1.5 sm:col-span-2">
                  <Label class="text-xs text-muted-foreground">模型清单（逗号分隔）</Label>
                  <Input :model-value="modelsText(p)" placeholder="gpt-4o, gpt-4o-mini" @update:model-value="setModels(p, String($event))" />
                </div>
              </div>
              <div v-if="store.tests[p.id] && !store.tests[p.id].testing" class="mt-2 flex items-center gap-1.5 text-xs" :class="store.tests[p.id].ok ? 'text-success' : 'text-destructive'">
                <IconCircleCheck v-if="store.tests[p.id].ok" class="size-3.5" />
                <IconAlertTriangle v-else class="size-3.5" />
                {{ store.tests[p.id].message }}<template v-if="store.tests[p.id].latency_ms"> · {{ store.tests[p.id].latency_ms }}ms</template>
              </div>
            </div>
          </div>
        </section>

        <!-- 成本护栏 -->
        <section class="mt-5">
          <h3 class="text-[11px] font-bold tracking-[0.18em] text-muted-foreground uppercase">成本护栏</h3>
          <div class="mt-2 flex items-center gap-3 rounded-xl border border-border bg-card/60 p-3">
            <Label class="text-xs text-muted-foreground">每回合 token 上限</Label>
            <Input v-model.number="config.turn_token_budget" type="number" min="0" step="1000" class="max-w-32" />
            <span class="text-xs text-muted-foreground">0 = 不限；超限发 warning 事件并收束回合</span>
          </div>
        </section>
      </template>

      <DialogFooter class="mt-4 gap-2 sm:justify-end">
        <Button variant="ghost" :disabled="store.saving" @click="close">取消</Button>
        <Button :disabled="store.saving || !config" @click="save">{{ store.saving ? '保存中…' : '保存配置' }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
