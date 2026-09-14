<script setup lang="ts">
// 新建 / 编辑账户弹窗。两种模式共用一个表单：字段差异只有「用户名」与「口令」
// （编辑时都不改——改口令走单独的重置入口，语义不同：重置会吊销该账户全部登录态）。
import { computed, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Switch } from '@/components/ui/switch'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import { IconAlertCircleFilled, IconLoader2, IconShieldLock, IconUserPlus } from '@tabler/icons-vue'
import type { AdminUserRow } from '@/types'

const props = defineProps<{
  open: boolean
  /** 传入 = 编辑；不传 = 新建。 */
  target?: AdminUserRow | null
  busy?: boolean
}>()
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'submit', payload: { username: string; password: string; displayName: string; isAdmin: boolean }): void
}>()

const isEdit = computed(() => !!props.target)
const username = ref('')
const password = ref('')
const displayName = ref('')
const isAdmin = ref(false)
const error = ref('')

watch(() => props.open, (open) => {
  if (!open) return
  error.value = ''
  username.value = props.target?.username ?? ''
  displayName.value = props.target?.display_name ?? ''
  isAdmin.value = props.target?.is_admin ?? false
  password.value = ''
})

function submit() {
  error.value = ''
  const name = username.value.trim()
  if (!isEdit.value) {
    if (name.length < 2 || name.length > 24) { error.value = '用户名需 2–24 个字符'; return }
    if (/\s/.test(name)) { error.value = '用户名不能包含空白字符'; return }
    if (password.value.length < 4) { error.value = '初始口令至少 4 个字符'; return }
  }
  emit('submit', {
    username: name,
    password: password.value,
    displayName: displayName.value.trim(),
    isAdmin: isAdmin.value,
  })
}
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-lg">
          <IconUserPlus v-if="!isEdit" aria-hidden="true" class="size-4.5 text-primary" />
          <IconShieldLock v-else aria-hidden="true" class="size-4.5 text-primary" />
          {{ isEdit ? '编辑账户' : '新建账户' }}
        </DialogTitle>
        <DialogDescription class="text-xs">
          {{ isEdit
            ? '用户名不可更改；口令请走「重置口令」入口（会同时退出该账户的所有设备）。'
            : '由管理员直接设定用户名与初始口令；本机实例没有邀请邮件这条链路。' }}
        </DialogDescription>
      </DialogHeader>

      <form class="space-y-4" @submit.prevent="submit">
        <div class="space-y-1.5">
          <Label for="au-username" class="text-xs font-semibold text-muted-foreground">用户名</Label>
          <Input
            id="au-username"
            v-model="username"
            :disabled="isEdit || busy"
            placeholder="2–24 个字符，登录时不分大小写"
            autocomplete="off"
          />
        </div>

        <div v-if="!isEdit" class="space-y-1.5">
          <Label for="au-password" class="text-xs font-semibold text-muted-foreground">初始口令</Label>
          <Input
            id="au-password"
            v-model="password"
            type="password"
            placeholder="至少 4 个字符"
            autocomplete="new-password"
            :disabled="busy"
          />
        </div>

        <div class="space-y-1.5">
          <Label for="au-display" class="text-xs font-semibold text-muted-foreground">
            显示名 <span class="font-normal text-muted-foreground/70">（可留空，默认用用户名）</span>
          </Label>
          <Input id="au-display" v-model="displayName" placeholder="界面上显示的名字" :disabled="busy" />
        </div>

        <label class="flex cursor-pointer items-center justify-between gap-3 rounded-lg border border-border/70 bg-muted/30 px-3 py-2.5">
          <span class="min-w-0">
            <span class="block text-xs font-semibold text-foreground">管理员</span>
            <span class="block text-[11px] leading-relaxed text-muted-foreground">
              可进入管理后台、读写全局 AI 配置。日常游玩不需要这个权限。
            </span>
          </span>
          <Switch :model-value="isAdmin" :disabled="busy" @update:model-value="(v: boolean) => (isAdmin = v)" />
        </label>

        <Alert v-if="error" variant="destructive" class="py-2.5">
          <IconAlertCircleFilled aria-hidden="true" />
          <AlertDescription class="text-xs">{{ error }}</AlertDescription>
        </Alert>

        <DialogFooter class="gap-2">
          <Button type="button" variant="ghost" :disabled="busy" @click="emit('update:open', false)">取消</Button>
          <Button type="submit" :disabled="busy">
            <IconLoader2 v-if="busy" data-icon="inline-start" class="animate-spin" />
            {{ isEdit ? '保存' : '创建账户' }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
