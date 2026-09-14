// 管理后台私有小工具。
//
// 时间刻意用**绝对时间**（而不是列表页那种「N 小时前」）：管理员看的是审计信息，
// 「2026年9月13日 17:05」比「2 小时前」更有用。

/** ISO → `2026年9月13日 17:05`；空值 / 非法值 → `—`。 */
export function formatWhen(iso: string | null | undefined): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '—'
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日 ${p(d.getHours())}:${p(d.getMinutes())}`
}

/** 字节数 → 人类可读（二进制单位，与文件管理器一致）。 */
export function formatBytes(n: number): string {
  if (!n) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i += 1
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}
