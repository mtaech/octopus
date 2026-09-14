<script setup lang="ts">
// 重置口令：管理员直接设新口令。副作用要说清楚——该账户所有设备都会被退出。
import { ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Alert, AlertDescription } from '@/components/ui/alert'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import { IconAlertCircleFilled, IconKey, IconLoader2 } from '@tabler/icons-vue'
import type { AdminUserRow } from '@/types'

const props = defineProps<{ open: boolean; target?: AdminUserRow | null; busy?: boolean }>()
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'submit', password: string): void
}>()

const password = ref('')
const confirm = ref('')
const error = ref('')

watch(() => props.open, (open) => {
  if (!open) return
  password.value = ''
  confirm.value = ''
  error.value = ''
})

function submit() {
  error.value = ''
  if (password.value.length < 4) { error.value = '新口令至少 4 个字符'; return }
  if (password.value !== confirm.value) { error.value = '两次输入的口令不一致'; return }
  emit('submit', password.value)
}
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-lg">
          <IconKey aria-hidden="true" class="size-4.5 text-primary" />
          重置口令
        </DialogTitle>
        <DialogDescription class="text-xs">
          为 <span class="font-medium text-foreground">{{ target?.display_name }}</span>
          （@{{ target?.username }}）设定新口令。
        </DialogDescription>
      </DialogHeader>

      <form class="space-y-4" @submit.prevent="submit">
        <Alert class="py-2.5">
          <IconAlertCircleFilled aria-hidden="true" />
          <AlertDescription class="text-xs leading-relaxed">
            保存后该账户的<strong>所有登录设备会被立即退出</strong>，需要用新口令重新登录。
          </AlertDescription>
        </Alert>

        <div class="space-y-1.5">
          <Label for="rp-new" class="text-xs font-semibold text-muted-foreground">新口令</Label>
          <Input id="rp-new" v-model="password" type="password" autocomplete="new-password" placeholder="至少 4 个字符" :disabled="busy" />
        </div>
        <div class="space-y-1.5">
          <Label for="rp-confirm" class="text-xs font-semibold text-muted-foreground">确认新口令</Label>
          <Input id="rp-confirm" v-model="confirm" type="password" autocomplete="new-password" :disabled="busy" />
        </div>

        <Alert v-if="error" variant="destructive" class="py-2.5">
          <IconAlertCircleFilled aria-hidden="true" />
          <AlertDescription class="text-xs">{{ error }}</AlertDescription>
        </Alert>

        <DialogFooter class="gap-2">
          <Button type="button" variant="ghost" :disabled="busy" @click="emit('update:open', false)">取消</Button>
          <Button type="submit" variant="destructive" :disabled="busy">
            <IconLoader2 v-if="busy" data-icon="inline-start" class="animate-spin" />
            重置并退出其设备
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
