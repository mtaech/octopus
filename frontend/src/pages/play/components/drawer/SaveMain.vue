<script setup lang="ts">
// 存读档主视图：当前存档卡（展开）+ 其他存档卡列表 + 维护历史小节
// 设计复审 2026-09-09：重命名 / 删除 / 新原点（玩家侧 v1）/ 维护历史读端点 / 导出抽单存档包。
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { usePlayStore } from '../../stores/play'
import { useDrawerStore } from '../../stores/drawer'
import { relTime, fmtTime } from '../../utils'
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
  if (!window.confirm('确定删除该存档？删除后不可恢复（已导出的备份包不受影响）。')) return
  const ok = await drawer.doDelete(props.playSave.id)
  if (ok) { drawer.closeDrawer(); void router.push('/') }
}

async function doOrigin() {
  if (!window.confirm('压缩为新原点：以当前状态为新起点，之前的命令日志归档为只读。继续？')) return
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
    <div class="flex items-baseline justify-between px-4 pt-3 pb-2">
      <span class="text-[15px] font-extrabold">存读档</span>
      <span class="text-xs text-muted-foreground">当前存档</span>
    </div>

    <!-- 升级提示（needs_upgrade 时抽屉顶部） -->
    <div v-if="props.playSave?.needsUpgrade" class="mx-3 mb-2.5 flex items-center gap-2 rounded-xl border border-warning/40 bg-warning/10 px-3 py-2 text-[12.5px] text-warning">
      <span>故事书已发布新版次 {{ props.playSave.latestRevision }}（当前 rev {{ props.playSave.revision }}）</span>
      <Button size="sm" class="ml-auto" @click="drawer.enterUpgrade()">升级</Button>
    </div>

    <!-- 当前存档卡（展开态） -->
    <Card v-if="props.playSave" size="sm" class="mx-3 mb-2.5 rounded-xl border-primary/50 bg-card py-0 ring-1 ring-primary/20">
      <CardContent class="px-3.5 py-2.5">
        <div v-if="!renaming" class="flex flex-wrap items-center gap-1.5">
          <span class="text-[14px] font-extrabold">{{ props.playSave.title }}</span>
          <Button size="icon-xs" variant="ghost" title="重命名" @click="startRename"><IconPencil /></Button>
          <Badge v-if="props.playSave.needsUpgrade" variant="outline" class="border-warning/60 text-warning">可升级</Badge>
          <Badge v-else variant="outline" class="border-success/60 text-success">最新版次</Badge>
        </div>
        <div v-else class="flex items-center gap-1.5">
          <Input v-model="renameText" class="h-8 text-[13px]" autofocus @keyup.enter="confirmRename" @keyup.esc="cancelRename" />
          <Button size="sm" @click="confirmRename">保存</Button>
          <Button size="sm" variant="ghost" @click="cancelRename">取消</Button>
        </div>
        <div class="mt-0.5 flex flex-wrap gap-2 text-[11.5px] text-muted-foreground">
          <span>{{ props.playSave.storybookTitle }}</span><span>版次 {{ props.playSave.revision }}</span>
        </div>

        <div class="mt-2.5 border-t border-border pt-2">
          <div class="mt-1">
            <h5 class="mb-1.5 text-[10px] font-extrabold tracking-[1.5px] text-muted-foreground">操作</h5>
            <div class="flex flex-wrap gap-1.5">
              <Button size="sm" variant="outline" :disabled="saving || play.sending" @click="doManual"><IconDeviceFloppy data-icon="inline-start" />手动存档</Button>
              <Button v-if="props.playSave.needsUpgrade" size="sm" variant="outline" class="border-warning/40 text-warning" @click="drawer.enterUpgrade()"><IconArrowUpRight data-icon="inline-start" />升级到 rev {{ props.playSave.latestRevision }}</Button>
              <Button size="sm" variant="outline" @click="drawer.doExport()"><IconDownload data-icon="inline-start" />导出</Button>
              <Button size="sm" variant="outline" class="text-destructive" @click="doDelete"><IconTrash data-icon="inline-start" />删除</Button>
            </div>
            <p class="mt-1.5 text-[11.5px] text-muted-foreground/70">导出为单文件 .sqlite 包（内嵌冻结模板 + 该存档历史），可拷贝备份 / 发送，能打开即通过。</p>
          </div>

          <div class="mt-2.5">
            <h5 class="mb-1.5 text-[10px] font-extrabold tracking-[1.5px] text-muted-foreground">维护历史</h5>
            <div v-if="drawer.history.length" class="flex flex-col">
              <div v-for="(h, i) in drawer.history" :key="i" class="grid grid-cols-[auto_1fr_auto] items-baseline gap-2 border-b border-dashed border-border/50 py-1 text-xs" :class="{ 'rounded-md bg-success/8 px-1': h.isNew }">
                <span class="font-bold whitespace-nowrap">{{ h.op }}</span>
                <span class="text-[11.5px] text-muted-foreground">{{ h.summary }}</span>
                <span class="font-mono text-[10.5px] whitespace-nowrap text-muted-foreground/60">{{ fmt(h.at) }}</span>
              </div>
            </div>
            <div v-else class="mt-1 text-[11.5px] text-muted-foreground/70">暂无维护记录</div>
          </div>

          <div class="mt-2.5">
            <h5 class="mb-1.5 text-[10px] font-extrabold tracking-[1.5px] text-muted-foreground">新原点</h5>
            <div class="rounded-lg border border-dashed border-border px-2.5 py-2">
              <div class="text-[12.5px] font-bold">压缩为新原点</div>
              <div class="mt-0.5 mb-2 text-[11.5px] text-muted-foreground">以当前状态为新起点，之前的命令日志归档为只读（回放 / 回滚以新原点为界）。</div>
              <Button size="sm" variant="outline" @click="doOrigin"><IconArchive data-icon="inline-start" />压缩为新原点</Button>
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
    <div v-else class="mx-3 text-[11.5px] text-muted-foreground/70">正在加载存档…</div>

    <!-- 其他存档列表 -->
    <template v-if="drawer.list.length">
      <div class="px-4 pt-3 pb-1.5 text-[11px] font-extrabold tracking-[1.5px] text-muted-foreground/60">我的存档</div>
      <div v-for="s in drawer.list.filter(x => x.id !== props.playSave?.id)" :key="s.id" class="mx-3 mb-2 cursor-pointer rounded-xl border border-border bg-card px-3 py-2 transition-colors hover:border-primary/50" @click="openSave(s.id)">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-[13px] font-extrabold">{{ s.title }}</span>
          <Badge v-if="s.needs_upgrade" variant="outline" class="border-warning/60 text-warning">可升级</Badge>
          <Badge v-if="s.imported" variant="outline" class="border-info/60 text-info">新导入</Badge>
        </div>
        <div class="mt-0.5 flex flex-wrap gap-2 text-[11px] text-muted-foreground">
          <span>{{ s.storybook_title }}</span><span>版次 {{ s.embedded_revision }}</span><span>{{ relTime(s.last_played_at) }}</span>
        </div>
      </div>
    </template>
  </div>
</template>
