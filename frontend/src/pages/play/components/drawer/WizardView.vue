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
    <div class="flex items-center gap-2 border-b border-border/80 bg-card/40 px-3 py-2.5">
      <Button size="xs" variant="ghost" class="h-7 text-xs font-semibold" @click="drawer.resetWizard()">
        <IconArrowLeft class="size-3.5 mr-1" />
        <span>存读档</span>
      </Button>
      <span class="text-[13px] font-extrabold text-foreground">版次平滑升级</span>
    </div>

    <!-- 步骤指示：① ② ③ -->
    <div class="flex items-center justify-between border-b border-border/60 bg-muted/20 px-4 py-2.5 text-[11px]">
      <span
        v-for="(l, i) in stepLabels"
        :key="l"
        class="inline-flex items-center gap-1.5 whitespace-nowrap transition-colors"
        :class="w.step === i + 1 ? 'font-extrabold text-primary' : w.step > i + 1 ? 'text-success font-medium' : 'text-muted-foreground/70'"
      >
        <span
          class="inline-flex size-4.5 items-center justify-center rounded-full border text-[10px] font-extrabold transition-all"
          :class="w.step > i + 1
            ? 'border-success bg-success/20 text-success'
            : w.step === i + 1
              ? 'border-primary bg-primary/20 text-primary shadow-xs'
              : 'border-border bg-muted/50 text-muted-foreground'"
        >
          <IconCheck v-if="w.step > i + 1" class="size-2.5" />
          <template v-else>{{ i + 1 }}</template>
        </span>
        <span>{{ l }}</span>
      </span>
    </div>

    <!-- step1 迁移报告 -->
    <template v-if="w.step === 1">
      <div class="px-4 pt-3 pb-2">
        <div class="text-[15px] font-extrabold text-foreground">对比与迁移报告</div>
        <div v-if="w.report" class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
          <span class="font-medium text-foreground/80">{{ store.detail?.storybook_title ?? '故事书' }}</span>
          <span>·</span>
          <span class="font-mono">rev {{ w.report.from_revision }} → rev {{ w.report.to_revision }}</span>
        </div>
      </div>

      <div v-if="w.reportLoading" class="flex flex-col items-center gap-3 px-4 py-12 text-center text-[13px] text-muted-foreground">
        <IconRotateClockwise class="size-6 animate-spin text-primary" />
        <span>正在对比新旧世界版次差异…</span>
      </div>
      <div v-else-if="w.error" class="flex flex-col items-center gap-2.5 px-4 py-8 text-center text-[13px] text-destructive">
        <span>{{ w.error }}</span>
        <Button size="sm" variant="outline" @click="drawer.runDryRun()">重试</Button>
      </div>
      <template v-else-if="w.report">
        <div class="flex-1 space-y-3 overflow-y-auto px-3 py-1">
          <!-- 引用变更组（折叠，自动处理） -->
          <details class="overflow-hidden rounded-xl border border-border/80 bg-card/80 transition-all" open>
            <summary class="flex cursor-pointer items-center gap-2 px-3.5 py-2.5 text-[12.5px] font-extrabold select-none hover:bg-muted/40 transition-colors">
              <span class="text-muted-foreground">▸</span>
              <span>引用变更</span>
              <Badge variant="secondary" class="font-mono text-[10px]">{{ w.report.groups.changes.length }}</Badge>
              <span class="ml-auto text-[10px] font-bold tracking-widest text-success uppercase">自动无损迁移</span>
            </summary>
            <div class="px-3.5 pb-3">
              <div
                v-for="(c, i) in w.report.groups.changes"
                :key="i"
                class="flex items-baseline gap-2 border-b border-dashed border-border/50 py-1.5 text-[12px] last:border-b-0"
              >
                <span class="font-mono rounded border border-border/80 bg-muted/60 px-1 py-px text-[9.5px] uppercase text-muted-foreground">{{ c.kind }}</span>
                <span class="font-bold text-foreground">{{ c.label }}</span>
                <span class="ml-auto text-right text-[11.5px] text-muted-foreground/80">{{ c.action }}</span>
              </div>
              <div v-if="!w.report.groups.changes.length" class="py-1 text-xs text-muted-foreground/70">无引用变更</div>
            </div>
          </details>

          <!-- 人物消失组：逐项内联裁决 -->
          <details class="overflow-hidden rounded-xl border border-warning/40 bg-warning/5 transition-all" open>
            <summary class="flex cursor-pointer items-center gap-2 px-3.5 py-2.5 text-[12.5px] font-extrabold text-warning select-none hover:bg-warning/10 transition-colors">
              <span>▸</span>
              <span>人物变动裁决</span>
              <Badge variant="outline" class="border-warning/60 bg-warning/15 font-mono text-[10px] text-warning">{{ w.report.groups.gone_characters.length }}</Badge>
            </summary>
            <div class="px-3.5 pb-3">
              <div
                v-for="gc in w.report.groups.gone_characters"
                :key="gc.character_id"
                class="mb-2.5 rounded-xl border border-border/80 bg-card/90 p-3 shadow-2xs"
              >
                <div class="flex items-baseline gap-2">
                  <span class="text-[13.5px] font-extrabold text-foreground">{{ gc.name }}</span>
                  <span class="text-[11.5px] text-muted-foreground">{{ gc.reason ?? '新版故事书中不再预设' }}</span>
                </div>
                <RadioGroup
                  class="mt-2.5 grid grid-cols-2 gap-2"
                  :model-value="pickVal(gc.character_id)"
                  @update:model-value="(v: unknown) => onPick(gc.character_id, String(v))"
                >
                  <div
                    class="flex items-start gap-2 rounded-lg border p-2.5 text-[11.5px] transition-all cursor-pointer"
                    :class="pickOf(gc.character_id) === 'freeze' ? 'border-primary bg-primary/10 ring-1 ring-primary/30' : 'border-border/70 hover:border-border'"
                  >
                    <RadioGroupItem id="gone-freeze" :value="'freeze'" class="mt-0.5 size-4" />
                    <Label for="gone-freeze" class="flex flex-col items-start gap-0.5 font-normal cursor-pointer">
                      <span class="font-extrabold text-foreground">遗留冻结</span>
                      <span class="text-[10px] text-muted-foreground">角色随存档封存，不再出场</span>
                    </Label>
                  </div>
                  <div
                    class="flex items-start gap-2 rounded-lg border p-2.5 text-[11.5px] transition-all cursor-pointer"
                    :class="pickOf(gc.character_id) === 'departure' ? 'border-primary bg-primary/10 ring-1 ring-primary/30' : 'border-border/70 hover:border-border'"
                  >
                    <RadioGroupItem id="gone-depart" :value="'departure'" class="mt-0.5 size-4" />
                    <Label for="gone-depart" class="flex flex-col items-start gap-0.5 font-normal cursor-pointer">
                      <span class="font-extrabold text-foreground">叙事离场</span>
                      <span class="text-[10px] text-muted-foreground">在后续叙事中安排合理离开</span>
                    </Label>
                  </div>
                </RadioGroup>
              </div>
              <div v-if="!w.report.groups.gone_characters.length" class="text-xs text-muted-foreground/70">无人物变动需要裁决</div>
            </div>
          </details>
        </div>

        <div class="flex gap-2 border-t border-border/60 bg-card/40 px-4 py-3">
          <Button
            class="ml-auto font-bold shadow-xs transition-all"
            :disabled="!drawer.allAdjudicated"
            @click="drawer.goConfirm()"
          >
            {{ drawer.allAdjudicated ? '下一步：确认执行 →' : '请先裁决所有人物' }}
          </Button>
        </div>
      </template>
    </template>

    <!-- step2 确认执行 -->
    <template v-else-if="w.step === 2">
      <div class="px-4 pt-3 pb-2"><div class="text-[15px] font-extrabold text-foreground">确认执行升级</div></div>
      <div class="flex-1 px-4 space-y-3">
        <Card size="sm" class="rounded-xl border-border/80 bg-card/90 py-0 shadow-2xs">
          <CardContent class="p-3.5 space-y-2">
            <div class="flex justify-between border-b border-dashed border-border/60 pb-2 text-[13px]">
              <span class="text-muted-foreground">目标故事书</span>
              <span class="font-bold text-foreground">{{ store.detail?.storybook_title ?? '' }}</span>
            </div>
            <div v-if="w.report" class="flex justify-between border-b border-dashed border-border/60 pb-2 text-[13px]">
              <span class="text-muted-foreground">目标版次</span>
              <span class="font-mono font-bold text-primary">rev {{ w.report.from_revision }} → rev {{ w.report.to_revision }}</span>
            </div>
            <div class="flex justify-between pt-1 text-[13px]">
              <span class="text-muted-foreground">人物裁决</span>
              <span class="flex flex-wrap gap-1 font-medium">
                <template v-if="w.picks.length">
                  <span
                    v-for="p in w.picks"
                    :key="p.character_id"
                    class="rounded-md border border-border/80 bg-muted/70 px-1.5 py-0.5 text-[11px]"
                  >
                    {{ p.name }} · {{ p.disposition === 'freeze' ? '遗留冻结' : '叙事离场' }}
                  </span>
                </template>
                <span v-else class="text-muted-foreground/60">无特别处置</span>
              </span>
            </div>
          </CardContent>
        </Card>

        <div class="flex items-center gap-2.5 rounded-xl border border-dashed border-primary/40 bg-primary/5 p-3 text-xs text-muted-foreground">
          <IconAlertTriangle class="size-4 shrink-0 text-warning" />
          <span>升级执行前将自动生成独立备份快照（自动轮转保留 5 份历史），可随时还原。</span>
        </div>

        <div v-if="w.error" class="mt-2 rounded-lg bg-destructive/10 p-2.5 text-center text-[12.5px] text-destructive">{{ w.error }}</div>
      </div>

      <div class="flex gap-2 border-t border-border/60 bg-card/40 px-4 py-3">
        <Button variant="outline" class="border-border/80" @click="drawer.backToReport()">
          <IconArrowLeft class="size-3.5 mr-1" />
          <span>返回修改</span>
        </Button>
        <Button
          class="ml-auto font-bold shadow-xs"
          :disabled="w.executing || !drawer.allAdjudicated"
          @click="drawer.doExecute()"
        >
          {{ w.executing ? '升级执行中…' : '开始无损升级' }}
        </Button>
      </div>
    </template>

    <!-- step3 执行结果 -->
    <template v-else-if="w.step === 3">
      <div class="flex flex-col items-center gap-3 px-5 pt-12 pb-8 text-center">
        <span class="flex size-12 items-center justify-center rounded-full border border-success/60 bg-success/15 text-success shadow-sm">
          <IconCircleCheck class="size-7" />
        </span>
        <div class="text-[17px] font-extrabold text-foreground">版次升级顺利完成</div>
        <div v-if="w.report" class="text-xs text-muted-foreground">
          已成功由 rev {{ w.report.from_revision }} 平滑过渡至 rev {{ w.report.to_revision }}
        </div>
        <div class="mt-2 w-full rounded-xl border border-dashed border-primary/40 bg-primary/5 p-3 text-left text-xs text-muted-foreground space-y-1">
          <div class="font-bold text-foreground">自动备份快照</div>
          <div class="font-mono text-[11px] break-all text-primary">{{ w.backupName }}</div>
        </div>
        <div class="mt-1 max-w-[280px] text-xs text-muted-foreground/80 leading-relaxed">
          内嵌冻结模板已替换为最新版次，维护审计历史已成功落账。
        </div>
        <Button class="mt-4 px-6 font-bold shadow-sm" @click="drawer.resetWizard()">完成并返回</Button>
      </div>
    </template>
  </div>
</template>