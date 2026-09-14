<script setup lang="ts">
// 删除账户弹窗。这是整个后台唯一不可逆的动作，所以：
// - 有内容时必须**显式勾选**「一并删除」才放行（对应后端 `?purge=true`）；
// - 内容条数直接列出来，让人知道自己要删掉多少东西；
// - 「删除自己」在页面层就禁掉（后端也会 409 兜底）。
import { computed, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import { IconAlertTriangleFilled, IconLoader2, IconTrash } from '@tabler/icons-vue'
import type { AdminUserRow } from '@/types'

const props = defineProps<{ open: boolean; target?: AdminUserRow | null; busy?: boolean }>()
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void
  (e: 'submit', purge: boolean): void
}>()

const ack = ref(false)
const hasData = computed(() => !!props.target && (props.target.saves > 0 || props.target.storybooks > 0))

watch(() => props.open, (open) => { if (open) ack.value = false })

function submit() {
  if (hasData.value && !ack.value) return
  emit('submit', hasData.value)
}
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2 font-serif text-lg">
          <IconTrash aria-hidden="true" class="size-4.5 text-destructive" />
          删除账户
        </DialogTitle>
        <DialogDescription class="text-xs">
          将删除 <span class="font-medium text-foreground">{{ target?.display_name }}</span>
          （@{{ target?.username }}）及其全部登录态。此操作不可撤销。
        </DialogDescription>
      </DialogHeader>

      <div class="space-y-4">
        <Alert v-if="hasData" variant="destructive" class="py-2.5">
          <IconAlertTriangleFilled aria-hidden="true" />
          <AlertTitle class="text-xs font-semibold">该账户还有内容</AlertTitle>
          <AlertDescription class="text-xs leading-relaxed">
            {{ target?.saves }} 个存档、{{ target?.storybooks }} 本故事书归它所有。
            不勾选下面的确认，服务端会拒绝这次删除（存档与故事书不会被 quietly 丢掉）。
          </AlertDescription>
        </Alert>

        <label v-if="hasData" class="flex cursor-pointer items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2.5 text-xs leading-relaxed">
          <input v-model="ack" type="checkbox" class="mt-0.5 size-3.5 accent-[var(--destructive)]" :disabled="busy">
          <span>
            我确认<strong>一并删除</strong>它的 {{ target?.storybooks }} 本故事书与
            {{ target?.saves }} 个存档（含命令日志、快照与结对会话）。
          </span>
        </label>

        <DialogFooter class="gap-2">
          <Button type="button" variant="ghost" :disabled="busy" @click="emit('update:open', false)">取消</Button>
          <Button
            type="button"
            variant="destructive"
            :disabled="busy || (hasData && !ack)"
            @click="submit"
          >
            <IconLoader2 v-if="busy" data-icon="inline-start" class="animate-spin" />
            永久删除
          </Button>
        </DialogFooter>
      </div>
    </DialogContent>
  </Dialog>
</template>
