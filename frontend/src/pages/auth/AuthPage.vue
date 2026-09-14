<script setup lang="ts">
// ============================================================
// 登录 / 注册页（/login · /register）——多账户的唯一入口
//
// 两种模式共用一个组件（路由名区分）：字段差异只有「显示名 / 确认密码」，
// 拆两个组件会立刻产生两份要同步维护的校验与错误文案。
//
// 跳转：登录成功后回 `?redirect=` 指定的页面（默认列表页）；已登录时访问本页直接放行。
// ============================================================
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { login, register } from '@/lib/auth'
import { toast } from '@/api'
import ThemeToggle from '@/components/ThemeToggle.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Alert, AlertDescription } from '@/components/ui/alert'
import {
  IconAlertCircleFilled,
  IconDiamondFilled,
  IconEye,
  IconEyeOff,
  IconLoader2,
  IconLock,
  IconUserPlus,
  IconLogin2,
  IconShieldLock,
} from '@tabler/icons-vue'

const route = useRoute()
const router = useRouter()

const mode = computed<'login' | 'register'>(() => (route.name === 'register' ? 'register' : 'login'))
const isRegister = computed(() => mode.value === 'register')

const username = ref('')
const displayName = ref('')
const password = ref('')
const confirm = ref('')
const remember = ref(true)
const showPassword = ref(false)
const busy = ref(false)
const error = ref('')

// 切换模式时清掉上一模式的错误与密码（避免把登录密码带进注册表单）
watch(mode, () => {
  error.value = ''
  password.value = ''
  confirm.value = ''
})

/** 前端先做一遍与后端同源的校验：明显的错误不必往返一次网络。 */
function validate(): string {
  const name = username.value.trim()
  if (name.length < 2) return '用户名至少 2 个字符'
  if (name.length > 24) return '用户名最多 24 个字符'
  if (/\s/.test(name)) return '用户名不能包含空白字符'
  if (password.value.length < 4) return '密码至少 4 个字符'
  if (isRegister.value && password.value !== confirm.value) return '两次输入的密码不一致'
  return ''
}

async function submit() {
  if (busy.value) return
  error.value = validate()
  if (error.value) return
  busy.value = true
  try {
    if (isRegister.value) {
      const account = await register(username.value, password.value, displayName.value, remember.value)
      toast('ok', `账户 ${account.display_name} 已创建`)
    } else {
      const account = await login(username.value, password.value, remember.value)
      if (account.must_change_password) {
        toast('warn', '仍在使用初始口令，请到右上角账户菜单修改密码')
      } else {
        toast('ok', `欢迎回来，${account.display_name}`)
      }
    }
    const redirect = typeof route.query.redirect === 'string' ? route.query.redirect : '/'
    await router.replace(redirect)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    busy.value = false
  }
}

function switchMode() {
  const target = isRegister.value ? 'login' : 'register'
  void router.replace({ name: target, query: route.query })
}
</script>

<template>
  <div class="relative flex min-h-dvh flex-col overflow-hidden bg-background text-foreground">
    <!-- 环境光晕：与列表页同款，进门第一眼就是同一个产品的观感 -->
    <div aria-hidden="true" class="pointer-events-none absolute inset-x-0 top-0 h-[420px] overflow-hidden">
      <div class="absolute top-[-220px] left-1/2 h-[460px] w-[820px] max-w-[130vw] -translate-x-1/2 rounded-full bg-primary/12 blur-3xl dark:bg-primary/18" />
    </div>

    <header class="relative z-10 mx-auto flex h-14 w-full max-w-[1120px] items-center gap-3 px-4 sm:px-6">
      <div class="inline-flex select-none items-center gap-2.5">
        <div class="flex size-7 items-center justify-center rounded-lg border border-primary/30 bg-primary/12 text-primary shadow-sm shadow-primary/20">
          <IconDiamondFilled aria-hidden="true" class="size-3.5" />
        </div>
        <span class="font-serif text-lg font-bold tracking-[0.04em]">Octopus</span>
      </div>
      <span class="hidden text-xs text-muted-foreground sm:inline">通用 AI RPG · 你自己的故事书</span>
      <div class="flex-1"></div>
      <ThemeToggle />
    </header>

    <main class="relative z-10 flex flex-1 items-center justify-center px-4 pb-16">
      <form
        class="w-full max-w-[26rem] rounded-2xl border border-border/80 bg-card/70 p-6 shadow-xl shadow-black/5 backdrop-blur-md sm:p-7"
        @submit.prevent="submit"
      >
        <div class="mb-6 flex items-center gap-3">
          <div class="flex size-9 shrink-0 items-center justify-center rounded-xl border border-primary/25 bg-primary/12 text-primary">
            <IconUserPlus v-if="isRegister" aria-hidden="true" class="size-4.5" />
            <IconLogin2 v-else aria-hidden="true" class="size-4.5" />
          </div>
          <div class="min-w-0">
            <h1 class="font-serif text-xl font-bold tracking-[0.02em]">
              {{ isRegister ? '创建账户' : '登录' }}
            </h1>
            <p class="mt-0.5 text-xs text-muted-foreground">
              {{ isRegister ? '每个账户有各自的存档与故事书' : '登录后继续你的故事' }}
            </p>
          </div>
        </div>

        <div class="space-y-4">
          <div class="space-y-1.5">
            <Label for="auth-username" class="text-xs font-semibold text-muted-foreground">用户名</Label>
            <Input
              id="auth-username"
              v-model="username"
              autocomplete="username"
              placeholder="2–24 个字符，登录时不分大小写"
              :disabled="busy"
            />
          </div>

          <div v-if="isRegister" class="space-y-1.5">
            <Label for="auth-display" class="text-xs font-semibold text-muted-foreground">
              显示名 <span class="font-normal text-muted-foreground/70">（可留空，默认用用户名）</span>
            </Label>
            <Input
              id="auth-display"
              v-model="displayName"
              autocomplete="nickname"
              placeholder="界面上显示的名字"
              :disabled="busy"
            />
          </div>

          <div class="space-y-1.5">
            <Label for="auth-password" class="text-xs font-semibold text-muted-foreground">密码</Label>
            <div class="relative">
              <Input
                id="auth-password"
                v-model="password"
                :type="showPassword ? 'text' : 'password'"
                :autocomplete="isRegister ? 'new-password' : 'current-password'"
                :placeholder="isRegister ? '至少 4 个字符' : '账户密码'"
                class="pr-10"
                :disabled="busy"
              />
              <button
                type="button"
                class="absolute inset-y-0 right-0 flex w-10 items-center justify-center rounded-r-md text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-primary/30 focus-visible:outline-none"
                :title="showPassword ? '隐藏密码' : '显示密码'"
                :aria-label="showPassword ? '隐藏密码' : '显示密码'"
                @click="showPassword = !showPassword"
              >
                <IconEyeOff v-if="showPassword" aria-hidden="true" class="size-4" />
                <IconEye v-else aria-hidden="true" class="size-4" />
              </button>
            </div>
          </div>

          <div v-if="isRegister" class="space-y-1.5">
            <Label for="auth-confirm" class="text-xs font-semibold text-muted-foreground">确认密码</Label>
            <Input
              id="auth-confirm"
              v-model="confirm"
              :type="showPassword ? 'text' : 'password'"
              autocomplete="new-password"
              placeholder="再输一次"
              :disabled="busy"
            />
          </div>

          <label class="flex cursor-pointer items-center gap-2 text-xs text-muted-foreground select-none">
            <input
              v-model="remember"
              type="checkbox"
              class="size-3.5 accent-[var(--primary)]"
              :disabled="busy"
            >
            记住我（关闭标签页后仍保持登录；不勾则仅在当前标签页有效）
          </label>

          <Alert v-if="error" variant="destructive" class="py-2.5">
            <IconAlertCircleFilled aria-hidden="true" />
            <AlertDescription class="text-xs leading-relaxed">{{ error }}</AlertDescription>
          </Alert>

          <Button type="submit" class="w-full shadow-md shadow-primary/20" size="lg" :disabled="busy">
            <IconLoader2 v-if="busy" data-icon="inline-start" class="animate-spin" />
            <IconShieldLock v-else-if="isRegister" data-icon="inline-start" />
            {{ busy ? '请稍候…' : isRegister ? '创建账户并登录' : '登录' }}
          </Button>

          <p class="text-center text-xs text-muted-foreground">
            {{ isRegister ? '已经有账户了？' : '还没有账户？' }}
            <button
              type="button"
              class="font-medium text-primary underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-primary/30 focus-visible:outline-none"
              @click="switchMode"
            >
              {{ isRegister ? '去登录' : '创建一个' }}
            </button>
          </p>

          <p class="flex items-start gap-1.5 border-t border-border/60 pt-3 text-[11px] leading-relaxed text-muted-foreground/80">
            <IconLock aria-hidden="true" class="mt-0.5 size-3.5 shrink-0" />
            口令以 argon2id 加盐哈希存储；登录令牌只存摘要，服务端不保存原文。
          </p>
        </div>
      </form>
    </main>
  </div>
</template>
