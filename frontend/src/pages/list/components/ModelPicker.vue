<script setup lang="ts">
// 模型清单编辑器：逐行编辑（id + 显示名 + 删除），支持「恢复默认模型」从目录拉取。
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconPlus, IconTrash } from '@tabler/icons-vue'
import { catalogEntries, catalogProvider, detectProviderByUrl } from '@/api/model-catalog-utils'
import type { ModelEntry } from '@/types'

const props = defineProps<{ modelValue: ModelEntry[]; providerUrl?: string }>()
const emit = defineEmits<{
  (e: 'update:modelValue', v: ModelEntry[]): void
  (e: 'pick', modelId: string): void
  (e: 'detected', providerId: string): void
}>()

const tried = ref(false)
const detectedId = computed(() => detectProviderByUrl(props.providerUrl ?? ''))
const detectedName = computed(() => (detectedId.value ? catalogProvider(detectedId.value)?.label ?? detectedId.value : ''))
const detectedCount = computed(() => (detectedId.value ? catalogEntries(detectedId.value).length : 0))
const models = computed(() => props.modelValue ?? [])

function addRow() { emit('update:modelValue', [...models.value, { id: '', name: '' }]) }
function removeAt(i: number) { emit('update:modelValue', models.value.filter((_, idx) => idx !== i)) }
function restoreDefaults() {
  tried.value = true
  const pid = detectedId.value
  if (!pid) return
  emit('detected', pid)
  emit('update:modelValue', catalogEntries(pid).map(m => ({ ...m })))
}
function onIdChange(id: string) { if (id.trim()) emit('pick', id.trim()) }
</script>

<template>
  <div class="w-full">
    <div class="mb-1.5 flex items-center gap-2">
      <span v-if="detectedId" class="text-[11px] text-success">已识别：{{ detectedName }} · {{ detectedCount }} 个模型</span>
      <span v-else-if="tried" class="text-[11px] text-warning">未能从 Base URL 识别供应商，可手填模型 id</span>
      <span v-else class="text-[11px] text-muted-foreground">已配置 {{ models.length }} 个模型</span>
      <div class="ml-auto flex shrink-0 items-center gap-2">
        <button type="button" class="text-[11px] text-muted-foreground transition-colors hover:text-primary" @click="restoreDefaults">恢复默认模型</button>
        <Button size="xs" variant="outline" @click="addRow"><IconPlus data-icon="inline-start" />添加模型</Button>
      </div>
    </div>

    <div v-if="models.length" class="flex flex-col gap-1.5">
      <div v-for="(m, i) in models" :key="i" class="flex items-center gap-1.5">
        <Input v-model="m.id" placeholder="模型 id（如 deepseek-chat）" class="h-9 flex-1 font-mono text-[12.5px]" @change="onIdChange(m.id)" />
        <Input v-model="m.name" placeholder="显示名（可选）" class="h-9 flex-1" />
        <Button size="icon-sm" variant="ghost" class="shrink-0 text-muted-foreground hover:text-destructive" title="删除模型" @click="removeAt(i)">
          <IconTrash />
        </Button>
      </div>
    </div>
    <div v-else class="rounded-lg border border-dashed border-border px-3 py-4 text-center text-[11.5px] text-muted-foreground">
      还没有模型。点「添加模型」手填，或「恢复默认模型」从目录拉取。
    </div>
  </div>
</template>
