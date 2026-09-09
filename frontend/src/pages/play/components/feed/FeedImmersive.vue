<script setup lang="ts">
// 演出模板 C · 沉浸式（#08 ①）：深色夜空、场景标题大；文本逐行演出：
// 揭示完一条后点击（或空格）前进到下一行；判定/确认门并入浮层卡。
// 迁移：夜空氛围用作用域语义 token 换肤（--background/--primary/...），无 hex 硬写。
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import type { FeedEntry } from '../../stores/play'
import { usePlayStore } from '../../stores/play'
import StreamItem from '../StreamItem.vue'
import { nameTintClass } from '../../utils'
import { IconSettings } from '@tabler/icons-vue'

const props = defineProps<{ feed: FeedEntry[] }>()
const store = usePlayStore()

/** C 只演"叙事行"：content 文本类 + scene 头；机制卡（判定/待确认/系统）实时并入底栏下沿 */
const narrativeLines = computed(() => props.feed.filter(e => e.kind === 'scene' || e.kind === 'content'))
const mechLines = computed(() => props.feed.filter(e => !(e.kind === 'scene' || e.kind === 'content')))

const idx = ref(-1)
const cur = computed(() => narrativeLines.value[idx.value] ?? null)
const curContent = computed(() => (cur.value?.kind === 'content' ? cur.value : null))
/** 无叙事行待演时：展示最近机制条（判定卡 / 待确认 / 系统 / 进度） */
const mechTail = computed(() => {
  const mc = mechLines.value
  if (mc.length === 0) return []
  const live = mc.find(m => m.kind === 'pending' && m.state === 'pending')
  const tail = mc.slice(-2)
  return live ? [...tail.filter(m => m.key !== live.key), live].slice(-3) : tail
})
const speakerTint = computed(() => {
  const c = curContent.value
  return c?.actorName ? nameTintClass(c.actorName) : 'bg-secondary text-muted-foreground'
})

function advance() {
  const c = curContent.value
  if (c && !c.done && c.reveal < c.text.length) { store.skipEntry(c.key); return }
  if (idx.value < narrativeLines.value.length - 1) { idx.value++ }
  else { idx.value = -1 }
}
function keyHandler(e: KeyboardEvent) {
  if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); advance() }
}
watch(() => narrativeLines.value.length, (n, o) => {
  if (n === 0) { idx.value = -1; return }
  if (o === undefined || o === 0 || idx.value < 0 || idx.value >= o - 1) idx.value = n - 1
}, { immediate: true })
onMounted(() => window.addEventListener('keydown', keyHandler))
onUnmounted(() => window.removeEventListener('keydown', keyHandler))
</script>

<template>
  <div class="feed-c" @click="advance">
    <!-- 夜空点缀：月亮 / 星光（用语义色柔光） -->
    <div class="pointer-events-none absolute inset-0">
      <div class="c-moon"></div>
      <div class="c-stars"></div>
    </div>

    <div class="c-stage">
      <transition name="cline" mode="out-in">
        <div v-if="cur && cur.kind === 'scene'" key="scene" class="text-center">
          <div class="csh-bar"></div>
          <h2 class="csh-title">{{ cur.title }}</h2>
          <div v-if="cur.description" class="csh-sub">{{ cur.description }}</div>
          <div class="csh-bar"></div>
        </div>
        <div v-else-if="cur && cur.kind === 'content'" :key="cur.key" class="c-line-box">
          <div v-if="cur.type === 'dialogue'" class="c-speaker" :class="speakerTint">{{ cur.actorName }}</div>
          <div class="c-dialog" :class="{ 'c-dialog--narr': cur.type === 'narrate', 'c-dialog--emote': cur.type === 'emote' }">
            <span v-if="cur.type === 'narrate'" class="text-primary mr-2">—</span>{{ cur.text.slice(0, cur.reveal) }}<span v-if="!cur.done" class="c-caret">▎</span>
          </div>
          <div v-if="mechTail.length" class="c-mech" @click.stop><StreamItem v-for="m in mechTail.slice(-2)" :key="m.key" :entry="m" /></div>
          <div class="c-hint">点击 / 空格 继续</div>
        </div>
        <div v-else-if="mechTail.length" key="mech" class="c-mech-standalone">
          <div class="c-mech-tip flex items-center gap-1.5"><IconSettings class="size-3" />结算消息</div>
          <div class="c-mech" @click.stop><StreamItem v-for="m in mechTail" :key="m.key" :entry="m" /></div>
        </div>
        <div v-else key="wait" class="c-wait">
          <span class="c-wait-pulse"></span>
          <span>静静等待故事展开…</span>
        </div>
      </transition>
    </div>
  </div>
</template>

<style scoped>
/* 夜空氛围：作用域语义 token 换肤（禁 hex 直写，仅变量/语义色） */
.feed-c {
  --background: #0d0f1c;
  --foreground: #f2ecdd;
  --card: #141024;
  --card-foreground: #f2ecdd;
  --secondary: #1c1a30;
  --secondary-foreground: #f2ecdd;
  --muted: #191732;
  --muted-foreground: #a49bc0;
  --border: #38324f;
  --input: #38324f;
  --primary: #e8c37a;
  --primary-foreground: #1a130a;
  --accent: #23204a;
  --accent-foreground: #f2ecdd;
  --popover: #1c1a30;
  --popover-foreground: #f2ecdd;
  --success: #7fb069;
  --warning: #e8c37a;
  --destructive: #d9856b;
  position: relative;
  height: 100%;
  overflow: hidden;
  cursor: pointer;
  background:
    radial-gradient(900px 420px at 72% -6%, rgba(120, 96, 190, .45), transparent 62%),
    radial-gradient(700px 300px at 18% 8%, rgba(64, 120, 170, .22), transparent 60%),
    var(--background);
  color: var(--foreground);
  font-family: 'Noto Serif SC', 'Songti SC', serif;
}
.c-moon {
  position: absolute;
  top: 9%; right: 16%;
  width: 96px; height: 96px; border-radius: 50%;
  background: radial-gradient(circle at 38% 36%, color-mix(in oklab, var(--primary) 90%, #fff), var(--primary) 62%);
  box-shadow: 0 0 60px 18px color-mix(in oklab, var(--primary) 22%, transparent);
  opacity: .9;
}
.c-stars {
  position: absolute; inset: 0; opacity: .7;
  background-image:
    radial-gradient(1.5px 1.5px at 20% 18%, color-mix(in oklab, var(--foreground) 90%, transparent), transparent 60%),
    radial-gradient(1px 1px at 40% 8%, color-mix(in oklab, var(--foreground) 70%, transparent), transparent 60%),
    radial-gradient(1.6px 1.6px at 62% 22%, color-mix(in oklab, var(--foreground) 80%, transparent), transparent 60%),
    radial-gradient(1px 1px at 80% 10%, color-mix(in oklab, var(--foreground) 60%, transparent), transparent 60%),
    radial-gradient(1.2px 1.2px at 88% 30%, color-mix(in oklab, var(--foreground) 75%, transparent), transparent 60%),
    radial-gradient(1px 1px at 12% 32%, color-mix(in oklab, var(--foreground) 55%, transparent), transparent 60%),
    radial-gradient(1.4px 1.4px at 52% 38%, color-mix(in oklab, var(--foreground) 70%, transparent), transparent 60%);
}
.c-stage {
  position: absolute; inset: 0;
  display: flex; flex-direction: column; justify-content: center; align-items: center;
  padding: 48px 28px 80px;
}
.c-scene-head { text-align: center; color: var(--primary); }
.csh-bar { width: 60px; height: 2px; background: color-mix(in oklab, var(--primary) 40%, transparent); margin: 12px auto; }
.csh-title { font-size: 26px; letter-spacing: 6px; font-weight: 800; margin: 0; }
.csh-sub { color: color-mix(in oklab, var(--foreground) 65%, transparent); font-size: 13px; margin-top: 10px; letter-spacing: 1px; }
.c-line-box {
  width: min(660px, 94%);
  background: color-mix(in oklab, var(--background) 72%, transparent);
  border: 1px solid color-mix(in oklab, var(--foreground) 16%, transparent);
  border-radius: var(--radius-xl);
  padding: 20px 24px 18px;
  backdrop-filter: blur(6px);
  box-shadow: 0 12px 40px rgba(0, 0, 0, .5);
  min-height: 150px;
  display: flex; flex-direction: column; justify-content: flex-end;
}
.c-speaker { font-size: 12px; letter-spacing: 2px; font-weight: 800; margin-bottom: 8px; border: 1px solid color-mix(in oklab, var(--foreground) 20%, transparent); border-radius: 999px; padding: 1px 10px; align-self: flex-start; }
.c-dialog { font-size: 17px; line-height: 1.85; color: var(--foreground); }
.c-dialog--narr { color: color-mix(in oklab, var(--foreground) 82%, transparent); font-style: italic; }
.c-dialog--emote { color: var(--primary); font-style: italic; }
.c-caret { animation: c-blink 1s step-start infinite; color: var(--primary); }
@keyframes c-blink { 50% { opacity: 0; } }
.c-hint { margin-top: 12px; font-size: 11px; color: color-mix(in oklab, var(--foreground) 40%, transparent); letter-spacing: 2px; text-align: right; }
.c-mech { margin-top: 10px; border-top: 1px dashed color-mix(in oklab, var(--foreground) 12%, transparent); padding-top: 8px; }
.c-mech-standalone { width: min(660px, 94%); display: flex; flex-direction: column; gap: 4px; }
.c-mech-tip { font-size: 10px; letter-spacing: 2px; color: color-mix(in oklab, var(--foreground) 40%, transparent); }
.c-wait { text-align: center; color: color-mix(in oklab, var(--foreground) 55%, transparent); display: flex; align-items: center; gap: 10px; }
.c-wait-pulse { width: 8px; height: 8px; border-radius: 50%; background: var(--primary); animation: c-blink 1.2s infinite; }
.cline-enter-active, .cline-leave-active { transition: opacity .25s ease, transform .25s ease; }
.cline-enter-from { opacity: 0; transform: translateY(8px); }
.cline-leave-to { opacity: 0; transform: translateY(-4px); }
</style>