<script setup lang="ts">
// ============================================================
// 管理后台（/admin）——只有管理员可见、可进
//
// 门禁有三层，缺一不可：
//   ① 路由 meta.admin + 全局守卫（非管理员直接被弹回列表页，地址栏都留不住）；
//   ② 账户菜单里的入口只对管理员渲染；
//   ③ 服务端 /api/admin/* 整棵子树挂在 guard_admin 之后（403）。
// 前端这两层只是「不给入口」，权威判定永远在服务端。
// ============================================================
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { storeToRefs } from 'pinia'
import { toast } from '@/api'
import { confirm } from '@/lib/confirm'
import { auth } from '@/lib/auth'
import { useAdminStore } from './stores/admin'
import UserDialog from './components/UserDialog.vue'
import ResetPasswordDialog from './components/ResetPasswordDialog.vue'
import DeleteUserDialog from './components/DeleteUserDialog.vue'
import { formatBytes, formatWhen } from './utils/format'
import ThemeToggle from '@/components/ThemeToggle.vue'
import AccountMenu from '@/components/AccountMenu.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Skeleton } from '@/components/ui/skeleton'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  IconActivity, IconAlertTriangleFilled, IconArrowLeft, IconBook2, IconDatabase,
  IconDotsVertical, IconKey, IconLogout, IconPlayerPlayFilled, IconRefresh, IconSearch,
  IconShieldLock, IconTrash, IconUserCog, IconUserPlus, IconUsers,
} from '@tabler/icons-vue'
import type { AdminUserRow } from '@/types'

const router = useRouter()
const store = useAdminStore()
const { overview, users, filteredUsers, adminCount, query, loading, loaded, error } = storeToRefs(store)

const me = computed(() => auth.user)

onMounted(() => { void store.load() })

// ---- 弹窗状态 ----
const userDialogOpen = ref(false)
const editing = ref<AdminUserRow | null>(null)
const resetOpen = ref(false)
const deleteOpen = ref(false)
const target = ref<AdminUserRow | null>(null)
const busy = ref(false)

function openCreate() {
  editing.value = null
  userDialogOpen.value = true
}
function openEdit(row: AdminUserRow) {
  editing.value = row
  userDialogOpen.value = true
}
function openReset(row: AdminUserRow) {
  target.value = row
  resetOpen.value = true
}
function openDelete(row: AdminUserRow) {
  target.value = row
  deleteOpen.value = true
}

function fail(e: unknown) {
  toast('error', e instanceof Error ? e.message : String(e))
}

async function submitUser(payload: { username: string; password: string; displayName: string; isAdmin: boolean }) {
  busy.value = true
  try {
    if (editing.value) {
      const row = await store.update(editing.value.id, {
        display_name: payload.displayName,
        is_admin: payload.isAdmin,
      })
      toast('ok', `已保存 ${row.display_name}`)
    } else {
      const row = await store.create({
        username: payload.username,
        password: payload.password,
        display_name: payload.displayName || undefined,
        is_admin: payload.isAdmin,
      })
      toast('ok', `已创建 ${row.is_admin ? '管理员' : '账户'} ${row.display_name}`)
    }
    userDialogOpen.value = false
  } catch (e) {
    fail(e)
  } finally {
    busy.value = false
  }
}

async function submitReset(password: string) {
  if (!target.value) return
  busy.value = true
  try {
    await store.resetPassword(target.value.id, password)
    toast('warn', `${target.value.display_name} 的口令已重置，其所有设备已退出`)
    resetOpen.value = false
  } catch (e) {
    fail(e)
  } finally {
    busy.value = false
  }
}

async function submitDelete(purge: boolean) {
  if (!target.value) return
  busy.value = true
  try {
    const res = await store.remove(target.value.id, purge)
    const removed = purge ? `（同时删除 ${res.storybooks_deleted} 本故事书、${res.saves_deleted} 个存档）` : ''
    toast('ok', `已删除账户 ${target.value.display_name}${removed}`)
    deleteOpen.value = false
  } catch (e) {
    fail(e)
  } finally {
    busy.value = false
  }
}

/** 提权直接生效；降权先确认——它会改变对方能看到的东西。 */
async function toggleAdmin(row: AdminUserRow) {
  if (!row.is_admin && adminCount.value <= 1 && me.value?.is_admin && !row.is_self) {
    // 提权不涉及锁死风险，只有降权需要拦。
  }
  if (row.is_admin) {
    const ok = await confirm({
      title: `取消 ${row.display_name} 的管理员身份？`,
      description: `取消后该账户无法进入管理后台，也不再有全局配置的读写权限；已有的存档与故事书不受影响。${adminCount.value <= 1 ? '（这是最后一个管理员，服务端会拒绝）' : ''}`,
      confirmText: '取消管理员',
      destructive: true,
    })
    if (!ok) return
  }
  try {
    const updated = await store.update(row.id, { is_admin: !row.is_admin })
    toast('ok', `${updated.display_name} ${updated.is_admin ? '已成为管理员' : '已取消管理员'}`)
  } catch (e) {
    fail(e)
  }
}

async function revokeSessions(row: AdminUserRow) {
  const ok = await confirm({
    title: `退出 ${row.display_name} 的全部设备？`,
    description: '该账户当前的所有登录态会被吊销，需要用原口令重新登录。口令不变。',
    confirmText: '退出其设备',
    destructive: true,
  })
  if (!ok) return
  try {
    const n = await store.revokeSessions(row.id)
    toast('ok', `已吊销 ${n} 条登录态`)
  } catch (e) {
    fail(e)
  }
}
</script>

<template>
  <div class="relative min-h-dvh overflow-x-hidden bg-background text-foreground">
    <div aria-hidden="true" class="pointer-events-none absolute inset-x-0 top-0 h-[320px] overflow-hidden">
      <div class="absolute top-[-200px] left-1/2 h-[380px] w-[720px] max-w-[130vw] -translate-x-1/2 rounded-full bg-primary/10 blur-3xl dark:bg-primary/15" />
    </div>

    <!-- 顶栏 -->
    <header class="sticky top-0 z-20 border-b border-border bg-background/85 backdrop-blur-md supports-[backdrop-filter]:bg-background/70">
      <div class="mx-auto flex h-14 w-full max-w-[1120px] items-center gap-3 px-4 sm:px-6">
        <Button
          variant="ghost"
          size="sm"
          class="gap-1.5 text-xs text-muted-foreground hover:text-foreground"
          title="返回应用"
          @click="router.push('/')"
        >
          <IconArrowLeft class="size-4" />
          <span class="hidden sm:inline">返回应用</span>
        </Button>
        <div class="h-4 w-px bg-border" />
        <div class="inline-flex select-none items-center gap-2">
          <div class="flex size-7 items-center justify-center rounded-lg border border-primary/30 bg-primary/12 text-primary shadow-sm shadow-primary/20">
            <IconShieldLock aria-hidden="true" class="size-3.5" />
          </div>
          <span class="font-serif text-base font-bold tracking-[0.02em]">管理后台</span>
        </div>
        <div class="flex-1"></div>
        <Button
          variant="ghost"
          size="sm"
          class="gap-1.5 text-xs text-muted-foreground hover:text-foreground"
          title="刷新"
          :disabled="loading"
          @click="store.load()"
        >
          <IconRefresh class="size-4" :class="loading ? 'animate-spin' : ''" />
        </Button>
        <ThemeToggle />
        <AccountMenu />
      </div>
    </header>

    <main class="relative z-10 mx-auto w-full max-w-[1120px] px-4 pt-8 pb-20 sm:px-6">
      <!-- 概览 -->
      <section>
        <h2 class="mb-3 flex items-center gap-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <IconActivity aria-hidden="true" class="size-3.5" />
          概览
        </h2>
        <div v-if="!overview" class="grid grid-cols-2 gap-3 lg:grid-cols-5">
          <Skeleton v-for="i in 5" :key="i" class="h-[5.5rem] rounded-xl" />
        </div>
        <div v-else class="grid grid-cols-2 gap-3 lg:grid-cols-5">
          <div
            v-for="card in [
              { label: '账户', value: overview.users, icon: IconUsers, hint: `${overview.admins} 个管理员` },
              { label: '存档', value: overview.saves, icon: IconPlayerPlayFilled, hint: '全部账户合计' },
              { label: '故事书', value: overview.storybooks, icon: IconBook2, hint: '含草稿与已发布' },
              { label: '登录设备', value: overview.active_sessions, icon: IconKey, hint: '未过期的登录态' },
              { label: '数据库', value: formatBytes(overview.db_bytes), icon: IconDatabase, hint: overview.db_path },
            ]"
            :key="card.label"
            class="rounded-xl border border-border/80 bg-card/60 px-4 py-3.5 backdrop-blur-sm"
          >
            <div class="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
              <component :is="card.icon" aria-hidden="true" class="size-3.5" />
              {{ card.label }}
            </div>
            <div class="mt-1.5 font-serif text-2xl font-bold tabular-nums text-foreground">{{ card.value }}</div>
            <div class="mt-0.5 truncate text-[11px] text-muted-foreground/80" :title="String(card.hint)">
              {{ card.hint }}
            </div>
          </div>
        </div>
        <p v-if="overview" class="mt-2 text-[11px] text-muted-foreground/70">
          Octopus {{ overview.version }} · 库文件 {{ overview.db_path }}（{{ formatBytes(overview.db_bytes) }}）
        </p>
      </section>

      <!-- 用户管理 -->
      <section class="mt-10">
        <div class="mb-3 flex flex-wrap items-end justify-between gap-3">
          <div>
            <h2 class="flex items-center gap-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              <IconUsers aria-hidden="true" class="size-3.5" />
              用户管理
            </h2>
            <p class="mt-1 text-[11px] text-muted-foreground/80">
              共 {{ users.length }} 个账户 · {{ adminCount }} 个管理员 · 每个账户只能看到自己的存档
            </p>
          </div>
          <div class="flex items-center gap-2">
            <div class="relative">
              <IconSearch aria-hidden="true" class="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input v-model="query" placeholder="搜索用户名 / 显示名" class="h-8 w-56 pl-8 text-xs" />
            </div>
            <Button size="sm" class="gap-1.5 shadow-sm shadow-primary/20" @click="openCreate">
              <IconUserPlus data-icon="inline-start" class="size-3.5" />
              新建账户
            </Button>
          </div>
        </div>

        <Alert v-if="error" variant="destructive" class="mb-3">
          <IconAlertTriangleFilled aria-hidden="true" />
          <AlertDescription class="text-xs">{{ error }}</AlertDescription>
        </Alert>

        <div v-if="!loaded" class="space-y-2.5">
          <Skeleton v-for="i in 3" :key="i" class="h-[4.5rem] rounded-xl" />
        </div>

        <Empty
          v-else-if="!filteredUsers.length"
          class="rounded-2xl border border-dashed border-border/80 bg-card/40 py-12"
        >
          <EmptyMedia variant="icon"><IconUsers aria-hidden="true" /></EmptyMedia>
          <EmptyHeader>
            <EmptyTitle class="font-serif text-lg">{{ query ? '没有匹配的账户' : '还没有其它账户' }}</EmptyTitle>
            <EmptyDescription class="text-xs">
              {{ query ? '换个关键词试试。' : '点「新建账户」为家人或朋友开一个，各自有独立的存档与故事书。' }}
            </EmptyDescription>
          </EmptyHeader>
        </Empty>

        <div v-else class="overflow-hidden rounded-xl border border-border/80 bg-card/50">
          <div class="flex items-center gap-3 border-b border-border/70 bg-muted/30 px-4 py-2 text-[11px] font-semibold text-muted-foreground">
            <span class="min-w-0 flex-1">账户</span>
            <span class="hidden w-40 text-right sm:block">内容</span>
            <span class="hidden w-40 text-right lg:block">最近登录</span>
            <span class="w-8"></span>
          </div>
          <div
            v-for="row in filteredUsers"
            :key="row.id"
            class="flex items-center gap-3 border-b border-border/50 px-4 py-3 last:border-b-0 hover:bg-muted/25"
          >
            <div class="flex min-w-0 flex-1 items-center gap-3">
              <div class="flex size-8 shrink-0 items-center justify-center rounded-full bg-primary/12 font-serif text-xs font-bold text-primary">
                {{ (row.display_name || row.username).trim().charAt(0).toUpperCase() }}
              </div>
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-1.5">
                  <span class="truncate text-sm font-semibold text-foreground">{{ row.display_name }}</span>
                  <Badge v-if="row.is_admin" variant="secondary" class="px-1.5 py-0 text-[10px]">管理员</Badge>
                  <Badge v-if="row.must_change_password" variant="outline" class="border-warning/60 px-1.5 py-0 text-[10px] text-warning">
                    待改初始口令
                  </Badge>
                  <Badge v-if="row.is_self" variant="outline" class="px-1.5 py-0 text-[10px]">你自己</Badge>
                </div>
                <div class="truncate text-[11px] text-muted-foreground">@{{ row.username }}</div>
              </div>
            </div>

            <div class="hidden w-40 text-right text-[11px] text-muted-foreground sm:block">
              <span class="tabular-nums text-foreground">{{ row.saves }}</span> 存档 ·
              <span class="tabular-nums text-foreground">{{ row.storybooks }}</span> 故事书
              <div class="text-muted-foreground/70">
                {{ row.active_sessions }} 台登录中
              </div>
            </div>

            <div class="hidden w-40 text-right text-[11px] text-muted-foreground lg:block">
              {{ formatWhen(row.last_login_at) }}
              <div class="text-muted-foreground/70">创建于 {{ formatWhen(row.created_at) }}</div>
            </div>

            <div class="w-8 shrink-0">
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="size-7 text-muted-foreground hover:text-foreground"
                    :title="`管理 ${row.display_name}`"
                  >
                    <IconDotsVertical class="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="w-48">
                  <DropdownMenuLabel class="text-[11px] text-muted-foreground">@{{ row.username }}</DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem @select="openEdit(row)">
                    <IconUserCog aria-hidden="true" />
                    编辑资料
                  </DropdownMenuItem>
                  <DropdownMenuItem @select="openReset(row)">
                    <IconKey aria-hidden="true" />
                    重置口令
                  </DropdownMenuItem>
                  <DropdownMenuItem @select="toggleAdmin(row)">
                    <IconShieldLock aria-hidden="true" />
                    {{ row.is_admin ? '取消管理员' : '设为管理员' }}
                  </DropdownMenuItem>
                  <DropdownMenuItem @select="revokeSessions(row)">
                    <IconLogout aria-hidden="true" />
                    退出其所有设备
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    :disabled="row.is_self"
                    class="text-destructive focus:text-destructive"
                    @select="openDelete(row)"
                  >
                    <IconTrash aria-hidden="true" />
                    删除账户
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
        </div>
      </section>
    </main>

    <UserDialog v-model:open="userDialogOpen" :target="editing" :busy="busy" @submit="submitUser" />
    <ResetPasswordDialog v-model:open="resetOpen" :target="target" :busy="busy" @submit="submitReset" />
    <DeleteUserDialog v-model:open="deleteOpen" :target="target" :busy="busy" @submit="submitDelete" />
  </div>
</template>
