<script setup lang="ts">
// ============================================================
// 右上角账户菜单：当前身份 + 修改密码 + 退出登录
//
// 三个页面（列表 / 编辑器 / 游玩）共用，所以放在 components/ 顶层。
// 「仍在使用初始口令」时触发器上挂一个警示点——这是唯一不会自动消失的提醒。
// ============================================================
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { auth, changePassword, logout } from '@/lib/auth'
import { toast } from '@/api'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { Alert, AlertDescription } from '@/components/ui/alert'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import {
  IconAlertTriangleFilled, IconChevronDown, IconKey, IconLoader2, IconLogout, IconShieldCheck,
  IconShieldLock,
} from '@tabler/icons-vue'

const router = useRouter()

const user = computed(() => auth.user)
const initial = computed(() => (user.value?.display_name || user.value?.username || '?').trim().charAt(0).toUpperCase())

// ---- 修改密码 ----
const pwdOpen = ref(false)
const current = ref('')
const next = ref('')
const confirm = ref('')
const busy = ref(false)
const error = ref('')

function openPasswordDialog() {
  current.value = ''
  next.value = ''
  confirm.value = ''
  error.value = ''
  pwdOpen.value = true
}

async function submitPassword() {
  if (busy.value) return
  if (next.value.length < 4) { error.value = '新密码至少 4 个字符'; return }
  if (next.value !== confirm.value) { error.value = '两次输入的新密码不一致'; return }
  if (next.value === current.value) { error.value = '新密码不能与当前密码相同'; return }
  busy.value = true
  error.value = ''
  try {
    await changePassword(current.value, next.value)
    pwdOpen.value = false
    toast('ok', '密码已修改，其它设备上的登录已退出')
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    busy.value = false
  }
}

function goAdmin() {
  void router.push({ name: 'admin' })
}

async function onLogout() {
  await logout()
  toast('info', '已退出登录')
  await router.push({ name: 'login' })
}
</script>

<template>
  <DropdownMenu v-if="user">
    <DropdownMenuTrigger as-child>
      <button
        type="button"
        class="relative flex h-8 items-center gap-2 rounded-full border border-border/80 bg-card/60 pl-1 pr-2.5 text-xs font-medium text-foreground transition-colors hover:border-primary/40 hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-primary/25 focus-visible:outline-none"
        title="账户 · 修改密码 / 退出登录"
      >
        <span class="flex size-6 items-center justify-center rounded-full bg-primary/15 font-serif text-[11px] font-bold text-primary">
          {{ initial }}
        </span>
        <span class="hidden max-w-[7rem] truncate sm:inline">{{ user.display_name }}</span>
        <IconChevronDown aria-hidden="true" class="size-3.5 text-muted-foreground" />
        <span
          v-if="user.must_change_password"
          class="absolute -right-0.5 -top-0.5 size-2.5 rounded-full bg-warning ring-2 ring-background"
          :title="'仍在使用初始口令'"
        />
      </button>
    </DropdownMenuTrigger>

    <DropdownMenuContent align="end" class="w-60">
      <DropdownMenuLabel class="flex flex-col gap-1 py-2">
        <span class="flex items-center gap-1.5 text-sm font-semibold">
          {{ user.display_name }}
          <Badge v-if="user.is_admin" variant="secondary" class="px-1.5 py-0 text-[10px]">管理员</Badge>
        </span>
        <span class="text-[11px] font-normal text-muted-foreground">@{{ user.username }}</span>
      </DropdownMenuLabel>
      <DropdownMenuSeparator />
      <DropdownMenuItem v-if="user.must_change_password" disabled class="text-warning focus:text-warning">
        <IconAlertTriangleFilled aria-hidden="true" />
        仍在使用初始口令
      </DropdownMenuItem>
      <DropdownMenuItem v-if="user.is_admin" @select="goAdmin">
        <IconShieldLock aria-hidden="true" />
        管理后台
      </DropdownMenuItem>
      <DropdownMenuItem @select="openPasswordDialog">
        <IconKey aria-hidden="true" />
        修改密码
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem @select="onLogout">
        <IconLogout aria-hidden="true" />
        退出登录
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>

  <Dialog v-model:open="pwdOpen">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-lg">
          <IconShieldCheck aria-hidden="true" class="size-4.5 text-primary" />
          修改密码
        </DialogTitle>
        <DialogDescription class="text-xs">
          改完只保留当前设备的登录态，其它设备会被退出。
        </DialogDescription>
      </DialogHeader>

      <form class="space-y-4" @submit.prevent="submitPassword">
        <div class="space-y-1.5">
          <Label for="pwd-current" class="text-xs font-semibold text-muted-foreground">当前密码</Label>
          <Input id="pwd-current" v-model="current" type="password" autocomplete="current-password" :disabled="busy" />
        </div>
        <div class="space-y-1.5">
          <Label for="pwd-new" class="text-xs font-semibold text-muted-foreground">新密码</Label>
          <Input id="pwd-new" v-model="next" type="password" autocomplete="new-password" placeholder="至少 4 个字符" :disabled="busy" />
        </div>
        <div class="space-y-1.5">
          <Label for="pwd-confirm" class="text-xs font-semibold text-muted-foreground">确认新密码</Label>
          <Input id="pwd-confirm" v-model="confirm" type="password" autocomplete="new-password" :disabled="busy" />
        </div>
        <Alert v-if="error" variant="destructive" class="py-2.5">
          <IconAlertTriangleFilled aria-hidden="true" />
          <AlertDescription class="text-xs">{{ error }}</AlertDescription>
        </Alert>
        <DialogFooter class="gap-2">
          <Button type="button" variant="ghost" :disabled="busy" @click="pwdOpen = false">取消</Button>
          <Button type="submit" :disabled="busy">
            <IconLoader2 v-if="busy" data-icon="inline-start" class="animate-spin" />
            确认修改
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
