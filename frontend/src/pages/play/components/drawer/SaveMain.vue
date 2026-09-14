<script setup lang="ts">
// 存读档主视图：当前存档卡（展开）+ 其他存档卡列表 + 维护历史小节
// 设计复审 2026-09-09：重命名 / 删除 / 新原点（玩家侧 v1）/ 维护历史读端点 / 导出抽单存档包。
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { usePlayStore } from '../../stores/play'
import { useDrawerStore } from '../../stores/drawer'
import { relTime, fmtTime } from '../../utils'
import { confirm } from '@/lib/confirm'
import { Card, CardContent } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { IconDeviceFloppy, IconDownload, IconArrowUpRight, IconPencil, IconTrash, IconArchive } from '@tabler/icons-vue'

const props = defineProps<{
  playSave: {
    id: string; title: string; storybookTitle: string; revision: number
    needsUpgrade: boolean; latestRevision: number
  } | null
}>()

const play = usePlayStore()
const drawer = useDrawerStore()
const router = useRouter()
const saving = ref(false)
const renaming = ref(false)
const renameText = ref('')

onMounted(() => { void drawer.loadList() })

async function doManual() {
  saving.value = true
  await drawer.doManualSave()
  saving.value = false
}

function startRename() {
  if (!props.playSave) return
  renameText.value = props.playSave.title
  renaming.value = true
}
async function confirmRename() {
  if (!props.playSave) return
  const t = renameText.value.trim()
  if (!t || t === props.playSave.title) { renaming.value = false; return }
  const ok = await drawer.doRename(props.playSave.id, t)
  if (ok) renaming.value = false
}
function cancelRename() { renaming.value = false }

async function doDelete() {
  if (!props.playSave) return
  const confirmed = await confirm({
    title: '删除该存档？',
    description: '删除后不可恢复（已导出的备份包不受影响）。',
    confirmText: '删除',
    destructive: true,
  })
  if (!confirmed) return
  const ok = await drawer.doDelete(props.playSave.id)
  if (ok) { drawer.closeDrawer(); void router.push('/') }
}

async function doOrigin() {
  const confirmed = await confirm({
    title: '压缩为新原点？',
    description: '以当前状态为新起点，之前的命令日志归档为只读。',
    confirmText: '执行压缩',
  })
  if (!confirmed) return
  await drawer.doNewOrigin()
}

function openSave(id: string) {
  if (id === props.playSave?.id) return
  drawer.closeDrawer()
  void router.push('/play/' + id)
}

function fmt(iso: string) { return fmtTime(iso) }
</script>

<template>
  <div class="flex flex-col pb-4">
    <div class="flex items-center justify-between px-4 pt-3 pb-2">
      <div class="flex items-center gap-1.5 text-[11px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">
        <IconArchive class="size-3.5 text-primary" />
        <span>当前游玩档案</span>
      </div>
      <span class="rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-[10px] font-bold text-primary">活跃中</span>
    </div>

    <!-- 升级提示（needs_upgrade 时抽屉顶部） -->
    <div v-if="props.playSave?.needsUpgrade" class="mx-3 mb-3 flex items-center gap-2 rounded-xl border border-warning/40 bg-warning/10 px-3.5 py-2.5 text-[12.5px] text-warning shadow-2xs">
      <span class="leading-relaxed">故事书已发布新版次 {{ props.playSave.latestRevision }}（当前 rev {{ props.playSave.revision }}）</span>
      <Button size="sm" class="ml-auto shrink-0 font-bold" @click="drawer.enterUpgrade()">
        <IconArrowUpRight class="size-3.5 mr-1" />
        <span>立即升级</span>
      </Button>
    </div>

    <!-- 当前存档卡（展开态） -->
    <Card v-if="props.playSave" size="sm" class="mx-3 mb-3 overflow-hidden rounded-xl border-primary/50 bg-gradient-to-b from-primary/10 via-card/95 to-card py-0 shadow-md ring-1 ring-primary/25">
      <CardContent class="p-3.5">
        <div v-if="!renaming" class="flex flex-wrap items-center gap-1.5">
          <span class="text-[15px] font-extrabold text-foreground tracking-tight">{{ props.playSave.title }}</span>
          <Button size="icon-xs" variant="ghost" class="text-muted-foreground hover:text-foreground" title="重命名" @click="startRename">
            <IconPencil class="size-3.5" />
          </Button>
          <Badge v-if="props.playSave.needsUpgrade" variant="outline" class="ml-auto border-warning/60 bg-warning/10 text-warning text-[10.5px]">可升级</Badge>
          <Badge v-else variant="outline" class="ml-auto border-success/60 bg-success/10 text-success text-[10.5px]">最新版次</Badge>
        </div>
        <div v-else class="flex items-center gap-1.5">
          <Input v-model="renameText" class="h-8 text-[13px]" autofocus @keyup.enter="confirmRename" @keyup.esc="cancelRename" />
          <Button size="sm" @click="confirmRename">保存</Button>
          <Button size="sm" variant="ghost" @click="cancelRename">取消</Button>
        </div>

        <div class="mt-1 flex flex-wrap items-center gap-2 text-[11.5px] text-muted-foreground">
          <span class="font-medium text-foreground/80">{{ props.playSave.storybookTitle }}</span>
          <span>·</span>
          <span class="font-mono">版次 rev {{ props.playSave.revision }}</span>
        </div>

        <div class="mt-3 border-t border-border/70 pt-3">
          <!-- 档案操作按键组 -->
          <div>
            <h5 class="mb-2 text-[10px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">档案操作</h5>
            <div class="flex flex-wrap gap-1.5">
              <Button size="sm" variant="outline" class="h-8 border-border/80 text-xs font-semibold shadow-2xs hover:border-primary/40" :disabled="saving || play.sending" @click="doManual">
                <IconDeviceFloppy class="size-3.5 mr-1 text-primary" />
                <span>手动存档</span>
              </Button>
              <Button v-if="props.playSave.needsUpgrade" size="sm" variant="outline" class="h-8 border-warning/40 text-warning text-xs font-semibold hover:bg-warning/10" @click="drawer.enterUpgrade()">
                <IconArrowUpRight class="size-3.5 mr-1" />
                <span>升级到 rev {{ props.playSave.latestRevision }}</span>
              </Button>
              <Button size="sm" variant="outline" class="h-8 border-border/80 text-xs font-semibold shadow-2xs hover:border-primary/40" @click="drawer.doExport()">
                <IconDownload class="size-3.5 mr-1 text-muted-foreground" />
                <span>导出</span>
              </Button>
              <Button size="sm" variant="outline" class="h-8 border-border/80 text-destructive text-xs font-semibold hover:bg-destructive/10" @click="doDelete">
                <IconTrash class="size-3.5 mr-1" />
                <span>删除</span>
              </Button>
            </div>
            <p class="mt-2 text-[11px] leading-relaxed text-muted-foreground/70">
              导出为跨数据库自包含存档包（.octopus.zip：save.json + assets/），内嵌冻结故事书与完整命令日志，便于异地备份与迁移。
            </p>
          </div>

          <!-- 维护历史 -->
          <div class="mt-3.5">
            <h5 class="mb-1.5 text-[10px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">维护历史记录</h5>
            <div v-if="drawer.history.length" class="flex flex-col rounded-lg border border-border/60 bg-muted/30 p-1.5">
              <div
                v-for="(h, i) in drawer.history"
                :key="i"
                class="grid grid-cols-[auto_1fr_auto] items-baseline gap-2 border-b border-dashed border-border/50 px-1 py-1.5 text-xs last:border-b-0"
                :class="{ 'rounded-md bg-success/10 px-1.5 font-medium': h.isNew }"
              >
                <span class="font-bold text-foreground whitespace-nowrap">{{ h.op }}</span>
                <span class="text-[11.5px] text-muted-foreground truncate">{{ h.summary }}</span>
                <span class="font-mono text-[10px] whitespace-nowrap text-muted-foreground/60">{{ fmt(h.at) }}</span>
              </div>
            </div>
            <div v-else class="rounded-lg border border-border/50 bg-muted/20 px-3 py-2 text-[11.5px] text-muted-foreground/70">暂无维护记录</div>
          </div>

          <!-- 压缩为新原点（v1 暂不可用：会话恢复依赖完整命令日志重放） -->
          <div class="mt-3.5">
            <h5 class="mb-1.5 text-[10px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">快照截断</h5>
            <div class="rounded-xl border border-dashed border-border/80 bg-muted/20 p-3">
              <div class="flex items-center gap-2">
                <div class="text-[12.5px] font-bold text-muted-foreground">压缩为新原点</div>
                <Badge variant="outline" class="border-border/70 text-muted-foreground text-[10px]">暂不可用</Badge>
              </div>
              <div class="mt-1 mb-2.5 text-[11.5px] leading-relaxed text-muted-foreground">
                v1 依赖完整命令日志重放来恢复会话，因此暂不支持把旧日志压缩归档。此功能待后续版本实现。
              </div>
              <Button size="sm" variant="outline" class="h-7 text-xs font-semibold" disabled title="v1 暂不可用">
                <IconArchive class="size-3 mr-1 text-muted-foreground" />
                <span>执行压缩</span>
              </Button>
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
    <div v-else class="mx-3 rounded-xl border border-border/60 bg-card/60 p-4 text-center text-[12px] text-muted-foreground">
      正在加载存档信息…
    </div>

    <!-- 其他存档列表 -->
    <template v-if="drawer.list.length">
      <div class="flex items-center justify-between px-4 pt-3 pb-1.5 text-[10.5px] font-extrabold tracking-[1.5px] text-muted-foreground uppercase">
        <span>我的其它存档</span>
        <span class="font-mono text-[10px] text-muted-foreground/60">{{ drawer.list.filter(x => x.id !== props.playSave?.id).length }}</span>
      </div>
      <div
        v-for="s in drawer.list.filter(x => x.id !== props.playSave?.id)"
        :key="s.id"
        class="group mx-3 mb-2 cursor-pointer rounded-xl border border-border/70 bg-card/70 p-3 transition-all hover:border-primary/50 hover:bg-card/95 hover:shadow-2xs"
        @click="openSave(s.id)"
      >
        <div class="flex flex-wrap items-center justify-between gap-1.5">
          <span class="text-[13.5px] font-bold text-foreground group-hover:text-primary transition-colors">{{ s.title }}</span>
          <div class="flex items-center gap-1">
            <Badge v-if="s.needs_upgrade" variant="outline" class="border-warning/60 bg-warning/10 text-warning text-[10px]">可升级</Badge>
            <Badge v-if="s.imported" variant="outline" class="border-info/60 bg-info/10 text-info text-[10px]">新导入</Badge>
          </div>
        </div>
        <div class="mt-1 flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
          <span class="font-medium text-foreground/75">{{ s.storybook_title }}</span>
          <span>·</span>
          <span class="font-mono">rev {{ s.embedded_revision }}</span>
          <span>·</span>
          <span>{{ relTime(s.last_played_at) }}</span>
        </div>
      </div>
    </template>
  </div>
</template>
