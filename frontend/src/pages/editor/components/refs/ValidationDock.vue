<script setup lang="ts">
// ValidationDock —— 右侧可折叠共享校验 dock（跨范式常驻，#07 ⑥ / #22 ③）
import { computed, ref, watch } from 'vue'
import { useEditorStore } from '../../stores/editor'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconChevronsLeft, IconChevronsRight, IconCircleCheck, IconAlertTriangle, IconAlertCircle, IconRefresh } from '@tabler/icons-vue'

const editor = useEditorStore()
const open = ref(true)

watch(open, (v) => { if (v) editor.runValidate() })

const errIssues = computed(() => editor.issues.filter(i => i.severity === 'error'))
const warnIssues = computed(() => editor.issues.filter(i => i.severity === 'warning'))

function displayTarget(t?: string): string {
  if (!t) return ''
  const [kind, id] = t.split(':')
  return kind && id ? kind + ' · ' + id : t
}
</script>

<template>
  <aside
    class="flex shrink-0 flex-col border-l border-border bg-background/70 transition-[width] duration-200"
    :class="open ? 'w-80' : 'w-11'"
  >
    <!-- 折叠头（整条可点） -->
    <header
      class="flex cursor-pointer items-center gap-2 border-b border-border px-2 py-2 select-none"
      :class="open ? 'justify-start' : 'flex-col justify-center py-3'"
      @click="open = !open"
    >
      <template v-if="open">
        <span class="text-xs font-semibold tracking-wider text-muted-foreground">校验</span>
        <Badge v-if="errIssues.length" variant="destructive" class="px-1.5 text-[11px]">{{ errIssues.length }} 错误</Badge>
        <Badge v-else-if="warnIssues.length" variant="outline" class="border-warning/40 px-1.5 text-[11px] text-warning">{{ warnIssues.length }} 警告</Badge>
        <Badge v-else variant="outline" class="border-success/40 px-1.5 text-[11px] text-success">
          <IconCircleCheck />
          无问题
        </Badge>
        <div class="ml-auto flex items-center gap-1" @click.stop>
          <Button variant="ghost" size="icon-xs" title="立即重新校验" @click="editor.runValidate()">
            <IconRefresh class="size-3.5 text-muted-foreground/70 hover:text-foreground" />
          </Button>
          <Button variant="ghost" size="icon-xs" title="收起面板" @click="open = false">
            <IconChevronsRight class="size-3.5 text-muted-foreground/50" />
          </Button>
        </div>
      </template>
      <template v-else>
        <IconChevronsLeft class="text-muted-foreground/70" />
        <Badge v-if="errIssues.length" variant="destructive" class="mt-1 px-1 text-[10px]">{{ errIssues.length }}</Badge>
      </template>
    </header>

    <div v-show="open" class="min-h-0 flex-1 overflow-y-auto p-3">
      <div v-if="!errIssues.length && !warnIssues.length" class="text-xs leading-5 text-success">
        <p>校验通过，草稿结构完整，可安全发布或开启沙箱试玩。</p>
        <p class="mt-1 text-muted-foreground/70">错误（error）会阻挡发布与试玩；警告（warning）仅供参考。</p>
      </div>

      <div v-if="errIssues.length" class="mb-3">
        <div class="mb-1.5 flex items-center justify-between text-[11px] font-bold tracking-wider text-muted-foreground">
          <span>错误 · 阻挡发布与试玩</span>
          <span class="text-[10px] font-normal text-muted-foreground/70">点击可定位</span>
        </div>
        <div
          v-for="(iss, i) in errIssues"
          :key="'e' + i"
          class="mb-1.5 cursor-pointer rounded-md border border-destructive/30 bg-destructive/5 px-2.5 py-2 text-xs leading-5 text-destructive transition-all duration-150 hover:border-destructive/60 hover:bg-destructive/10 hover:shadow-xs"
          title="点击定位到对应编辑项"
          @click="editor.navigateToIssue(iss)"
        >
          <div class="flex gap-2">
            <IconAlertCircle class="mt-0.5 size-3.5 shrink-0" />
            <div class="min-w-0">
              <div class="break-words font-medium">{{ iss.message }}</div>
              <div v-if="iss.target" class="mt-0.5 font-mono text-[10px] opacity-80 underline underline-offset-2">
                @ {{ displayTarget(iss.target) }}
              </div>
              <div v-if="iss.related_refs?.length" class="mt-1 flex flex-wrap gap-1">
                <span v-for="(r, j) in iss.related_refs" :key="j" class="rounded-full border border-border bg-muted/40 px-2 py-px text-[10px] text-muted-foreground">
                  {{ r.label ?? r.id }}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div v-if="warnIssues.length">
        <div class="mb-1.5 flex items-center justify-between text-[11px] font-bold tracking-wider text-muted-foreground">
          <span>警告 · 仅提示</span>
          <span class="text-[10px] font-normal text-muted-foreground/70">点击可定位</span>
        </div>
        <div
          v-for="(iss, i) in warnIssues"
          :key="'w' + i"
          class="mb-1.5 cursor-pointer rounded-md border border-warning/25 bg-warning/5 px-2.5 py-2 text-xs leading-5 text-warning transition-all duration-150 hover:border-warning/50 hover:bg-warning/10 hover:shadow-xs"
          title="点击定位到对应编辑项"
          @click="editor.navigateToIssue(iss)"
        >
          <div class="flex gap-2">
            <IconAlertTriangle class="mt-0.5 size-3.5 shrink-0" />
            <div class="min-w-0">
              <div class="break-words">{{ iss.message }}</div>
              <div v-if="iss.target" class="mt-0.5 font-mono text-[10px] opacity-75 underline underline-offset-2">
                @ {{ displayTarget(iss.target) }}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </aside>
</template>
