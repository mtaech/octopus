// 浏览器下载助手：a[download] + object URL（用后回收）。
// 列表页导出故事书与游玩页导出存档共用这一份，避免两处各写一遍。

/**
 * 从 `Content-Disposition` 里取文件名。
 *
 * 后端按 RFC 6266/5987 出头部：`filename="<ASCII 兜底>"` 给老客户端、
 * `filename*=UTF-8''<百分号编码>`（真实标题，中文在这）。**先读 filename***，
 * 否则中文标题会被 ASCII 兜底（id）顶掉。
 */
export function filenameFromContentDisposition(header: string | null, fallback: string): string {
  if (!header) return fallback
  const star = header.match(/filename\*\s*=\s*(?:UTF-8|utf-8)''([^;]+)/)
  if (star?.[1]) {
    try {
      return decodeURIComponent(star[1].trim())
    } catch {
      /* 编码坏了就退回 filename= */
    }
  }
  const plain = header.match(/filename\s*=\s*"([^"]+)"/) ?? header.match(/filename\s*=\s*([^;]+)/)
  return plain?.[1]?.trim() || fallback
}

/**
 * 文件名安全词干（Mock 模式自己造下载名用）。
 * 服务端那套完整规整在 `crates/octopus-api/src/filename.rs`，这里只保证不出路径字符。
 */
export function safeFileStem(title: string, fallback: string): string {
  const stem = title
    .replace(/[/\\:*?"<>|]/g, '_')
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/^[.\s]+|[.\s]+$/g, '')
  return stem || fallback
}

/** 触发一次文件下载。 */
export function downloadBlob(filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 4000)
}
