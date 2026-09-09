<script setup lang="ts">
// 升级向导（#21 ② C 形态，抽屉内聚焦步进，非新路由）：
// step1 迁移报告（dry-run 加载：引用变更折叠 + 人物消失逐项内联单选）
// step2 确认执行（自动备份提示）→ step3 执行结果（已升级 + 备份名 + 维护历史落账）
// 迁移：人物消失裁决 → <RadioGroup>；步骤指示 ①②③ + text-muted；报告组 <details> 折叠。
import { computed } from 'vue'
import type { Disposition } from '@/types'
import { usePlayStore } from '../../stores/play'
import { useDrawerStore } from '../../stores/drawer'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent } from '@/components/ui/card'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Label } from '@/components/ui/label'
import { IconArrowLeft, IconCheck, IconCircleCheck, IconAlertTriangle, IconRotateClockwise } from '@tabler/icons-vue'

const store = usePlayStore()
const drawer = useDrawerStore()
const w = computed(() => drawer.wizard)

const stepLabels = ['迁移报告', '确认执行', '执行结果']
/** pickOf 返回受控值（'' 表示未裁决） */
const pickOf = (cid: string): Disposition | null => w.value.picks.find(p => p.character_id === cid)?.disposition ?? null
const pickVal = (cid: string): string => pickOf(cid) ?? ''
function onPick(cid: string, v: string) {
  if (v === 'freeze' || v === 'departure') drawer.pick(cid, v)
}
</script>

<template>
  <div class="flex h-full flex-col">
    <!-- 面包屑返回 -->
    <div class="flex items-center gap-2 border-b border-border px-3 py-2">
      <Button size="xs" variant="ghost" @click="drawer.resetWizard()"><IconArrowLeft data-icon="inline-start" />存读档</Button>
      <span class="text-[13px] font-extrabold">版次升级</span>
    </div>

    <!-- 步骤指示：① ② ③ -->
    <div class="flex items-center gap-2.5 px-3.5 pt-2.5 pb-1.5 text-[11px]">
      <span v-for="(l, i) in stepLabels" :key="l" class="inline-flex items-center gap-1 whitespace-nowrap" :class="w.step === i + 1 ? 'font-extrabold text-foreground' : w.step > i + 1 ? 'text-success' : 'text-muted-foreground'">
        <span class="inline-flex size-4 items-center justify-center rounded-full border text-[10px] font-extrabold"
          :class="w.step > i + 1 ? 'border-success bg-success/15 text-success' : w.step === i + 1 ? 'border-primary bg-primary/15 text-primary' : 'border-border'">
          <IconCheck v-if="w.step > i + 1" class="size-2.5" />
          <template v-else>{{ i + 1 }}</template>
        </span>{{ l }}
      </span>
    </div>

    <!-- step1 迁移报告 -->
    <template v-if="w.step === 1">
      <div class="px-4 pt-2 pb-2.5">
        <div class="text-[15px] font-extrabold">迁移报告</div>
        <div v-if="w.report" class="mt-0.5 text-xs text-muted-foreground">{{ store.detail?.storybook_title ?? '故事书' }} · rev {{ w.report.from_revision }} → rev {{ w.report.to_revision }}</div>
      </div>

      <div v-if="w.reportLoading" class="flex flex-col items-center gap-2.5 px-4 py-8 text-center text-[13px] text-muted-foreground">
        <IconRotateClockwise class="size-5 animate-spin text-primary" />
        <span>正在对比新旧版次…</span>
      </div>
      <div v-else-if="w.error" class="flex flex-col items-center gap-2.5 px-4 py-8 text-center text-[13px] text-destructive">
        <span>{{ w.error }}</span>
        <Button size="sm" variant="outline" @click="drawer.runDryRun()">重试</Button>
      </div>
      <template v-else-if="w.report">
        <!-- 引用变更组（折叠，自动处理） -->
        <details class="mx-3 mb-3 overflow-hidden rounded-xl border border-border bg-card" open>
          <summary class="flex cursor-pointer items-center gap-2 px-3 py-2.5 text-[12.5px] font-extrabold select-none">
            <span class="text-muted-foreground">▸</span>引用变更
            <Badge variant="secondary" class="text-[10px]">{{ w.report.groups.changes.length }}</Badge>
            <span class="ml-auto text-[10px] font-bold tracking-widest text-success">自动处理</span>
          </summary>
          <div class="px-3 pb-3">
            <div v-for="(c, i) in w.report.groups.changes" :key="i" class="flex items-baseline gap-2 border-b border-dashed border-border/50 py-1 text-[12px]">
              <span class="font-mono rounded border border-border px-1 text-[10px] uppercase text-muted-foreground">{{ c.kind }}</span>
              <span class="font-bold">{{ c.label }}</span>
              <span class="ml-auto text-right text-[11.5px] text-muted-foreground">{{ c.action }}</span>
            </div>
            <div v-if="!w.report.groups.changes.length" class="py-1 text-xs text-muted-foreground/70">无引用变更</div>
          </div>
        </details>

        <!-- 人物消失组：逐项内联裁决 -->
        <details class="mx-3 mb-3 overflow-hidden rounded-xl border border-warning/40 bg-warning/5" open>
          <summary class="flex cursor-pointer items-center gap-2 px-3 py-2.5 text-[12.5px] font-extrabold text-warning select-none">
            <span>▸</span>人物消失
            <Badge variant="outline" class="border-warning/60 text-warning text-[10px]">{{ w.report.groups.gone_characters.length }}</Badge>
          </summary>
          <div class="px-3 pb-3">
            <div v-for="gc in w.report.groups.gone_characters" :key="gc.character_id" class="mb-2 rounded-lg border border-border bg-card p-2.5">
              <div class="flex items-baseline gap-2">
                <span class="text-[13px] font-extrabold">{{ gc.name }}</span>
                <span class="text-[11.5px] text-muted-foreground">{{ gc.reason ?? '新版故事书中不再出现' }}</span>
              </div>
              <RadioGroup class="mt-2 grid grid-cols-2 gap-1.5" :model-value="pickVal(gc.character_id)" @update:model-value="(v: unknown) => onPick(gc.character_id, String(v))">
                <div class="flex items-start gap-2 rounded-lg border p-2 text-[11.5px]" :class="pickOf(gc.character_id) === 'freeze' ? 'border-warning bg-warning/10' : 'border-border'">
                  <RadioGroupItem id="gone-freeze" :value="'freeze'" class="mt-0.5 size-4" />
                  <Label for="gone-freeze" class="flex flex-col items-start gap-0.5 font-normal">
                    <span class="font-extrabold">遗留冻结</span>
                    <span class="text-[10.5px] text-muted-foreground">角色随存档封存，不再出场</span>
                  </Label>
                </div>
                <div class="flex items-start gap-2 rounded-lg border p-2 text-[11.5px]" :class="pickOf(gc.character_id) === 'departure' ? 'border-warning bg-warning/10' : 'border-border'">
                  <RadioGroupItem id="gone-depart" :value="'departure'" class="mt-0.5 size-4" />
                  <Label for="gone-depart" class="flex flex-col items-start gap-0.5 font-normal">
                    <span class="font-extrabold">叙事离场</span>
                    <span class="text-[10.5px] text-muted-foreground">在叙事中安排角色离开</span>
                  </Label>
                </div>
              </RadioGroup>
            </div>
            <div v-if="!w.report.groups.gone_characters.length" class="text-xs text-muted-foreground/70">无人物消失</div>
          </div>
        </details>

        <div class="flex gap-2 px-4 pb-4">
          <Button class="ml-auto" :disabled="!drawer.allAdjudicated" @click="drawer.goConfirm()">
            {{ drawer.allAdjudicated ? '继续 →' : '请先裁决所有人物' }}
          </Button>
        </div>
      </template>
    </template>

    <!-- step2 确认执行 -->
    <template v-else-if="w.step === 2">
      <div class="px-4 pt-2 pb-3"><div class="text-[15px] font-extrabold">确认执行</div></div>
      <div class="px-4">
        <Card size="sm" class="py-0">
          <CardContent class="px-3.5 py-2.5">
            <div class="flex gap-2.5 border-b border-dashed border-border/60 py-1.5 text-[13px]">
              <span class="w-16 shrink-0 text-muted-foreground">目标</span><span class="font-semibold">{{ store.detail?.storybook_title ?? '' }}</span>
            </div>
            <div v-if="w.report" class="flex gap-2.5 border-b border-dashed border-border/60 py-1.5 text-[13px]">
              <span class="w-16 shrink-0 text-muted-foreground">版次</span><span class="font-semibold">rev {{ w.report.from_revision }} → rev {{ w.report.to_revision }}</span>
            </div>
            <div class="flex gap-2.5 py-1.5 text-[13px]">
              <span class="w-16 shrink-0 text-muted-foreground">人物裁决</span>
              <span class="flex flex-wrap gap-1.5 font-semibold">
                <template v-if="w.picks.length">
                  <span v-for="p in w.picks" :key="p.character_id" class="rounded-md border border-border bg-muted px-1.5 text-[11.5px]">{{ p.name }} · {{ p.disposition === 'freeze' ? '遗留冻结' : '叙事离场' }}</span>
                </template>
                <span v-else class="text-muted-foreground/60">无人物处置</span>
              </span>
            </div>
          </CardContent>
        </Card>
        <div class="mt-3 flex items-center gap-2 rounded-lg border border-dashed border-primary/40 bg-primary/5 px-3 py-2 text-xs text-muted-foreground">
          <IconAlertTriangle class="size-4 shrink-0 text-warning" />
          <span>执行前将自动创建备份存档（保留 5 份）</span>
        </div>
        <div v-if="w.error" class="mt-2 text-center text-[13px] text-destructive">{{ w.error }}</div>
        <div class="mt-3 flex gap-2 pb-4">
          <Button variant="outline" @click="drawer.backToReport()"><IconArrowLeft data-icon="inline-start" />返回报告</Button>
          <Button class="ml-auto" :disabled="w.executing || !drawer.allAdjudicated" @click="drawer.doExecute()">
            {{ w.executing ? '执行中…' : '执行升级' }}
          </Button>
        </div>
      </div>
    </template>

    <!-- step3 执行结果 -->
    <template v-else-if="w.step === 3">
      <div class="flex flex-col items-center gap-2 px-5 pt-10 pb-6 text-center">
        <span class="flex size-11 items-center justify-center rounded-full border border-success bg-success/15 text-success"><IconCircleCheck class="size-6" /></span>
        <div class="text-[15px] font-extrabold">升级完成</div>
        <div v-if="w.report" class="text-xs text-muted-foreground">已从 rev {{ w.report.from_revision }} 升级到 rev {{ w.report.to_revision }}</div>
        <div class="mt-1 w-full rounded-lg border border-dashed border-primary/40 bg-primary/5 px-3 py-2 text-left text-xs text-muted-foreground">
          <span>自动备份：</span><span class="font-mono text-[11px] break-all text-primary">{{ w.backupName }}</span>
        </div>
        <div class="max-w-[280px] text-xs text-muted-foreground">内嵌冻结模板已替换为新版次，维护历史已落账。</div>
        <Button class="mt-3" @click="drawer.resetWizard()">完成</Button>
      </div>
    </template>
  </div>
</template>