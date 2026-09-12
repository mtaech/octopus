<script setup lang="ts">
// LuaTryRun —— Lua 脚本试跑（#23）：调后端沙箱执行，展示结果 / 错误 / 写请求。
import { ref } from 'vue'
import { runLua } from '@/api'
import { Button } from '@/components/ui/button'
import { IconPlayerPlay } from '@tabler/icons-vue'

const props = withDefaults(defineProps<{
  script: string
  mode?: 'check' | 'condition' | 'hook'
  skill?: unknown
}>(), { mode: 'check' })

const running = ref(false)
const result = ref('')

async function run(): Promise<void> {
  if (!props.script?.trim()) return
  running.value = true
  result.value = ''
  try {
    const res = await runLua({ script: props.script, mode: props.mode, skill: props.skill })
    if (!res.ok) {
      result.value = res.error ?? '执行失败'
    } else {
      const head = JSON.stringify(res.result ?? {}, null, 2)
      const reqs = res.requests?.length ? '\n写请求：' + JSON.stringify(res.requests, null, 2) : ''
      result.value = head + reqs
    }
  } catch (e) {
    result.value = e instanceof Error ? e.message : String(e)
  } finally {
    running.value = false
  }
}
</script>

<template>
  <div class="space-y-1.5">
    <div class="flex flex-wrap items-center gap-2">
      <Button
        variant="ghost"
        size="sm"
        class="h-7 gap-1 text-xs text-muted-foreground"
        :disabled="running || !script?.trim()"
        @click="run"
      >
        <IconPlayerPlay class="size-3.5" />
        试跑
      </Button>
      <span class="text-[11px] text-muted-foreground/60">{{ running ? '执行中…' : '静态预检 + 沙箱执行' }}</span>
    </div>
    <pre v-if="result" class="max-h-44 overflow-auto rounded-lg border border-border/60 bg-muted/40 p-2.5 font-mono text-[11px] text-foreground">{{ result }}</pre>
  </div>
</template>
