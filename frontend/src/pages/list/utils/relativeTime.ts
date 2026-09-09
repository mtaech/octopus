// 列表页私有小工具：中文相对时间格式（规格点 10）
// 「刚刚 / N 分钟前 / N 小时前 / M月D日」——今天内的活动走相对描述，
// 更早则落到具体日期（跨年附年份，避免歧义）。

const MIN = 60 * 1000
const HOUR = 60 * MIN
const DAY = 24 * HOUR

/** 把 ISO 时间戳格式化为中文相对时间 */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const t = new Date(iso).getTime()
  if (Number.isNaN(t)) return ''
  const diff = now - t
  if (diff < MIN) return '刚刚'
  if (diff < HOUR) return Math.floor(diff / MIN) + ' 分钟前'
  if (diff < DAY) return Math.floor(diff / HOUR) + ' 小时前'
  // 超过 24 小时：落到日期（M月D日，跨年附年份）
  const d = new Date(t)
  const y = d.getFullYear()
  const m = d.getMonth() + 1
  const day = d.getDate()
  const nowY = new Date(now).getFullYear()
  const datePart = y === nowY ? `${m}月${day}日` : `${y}年${m}月${day}日`
  return datePart
}
