// ============================================================
// 极简 Markdown 渲染（#01/#22 决议 12：叙事字段格式 = Markdown）
// 只处理行内加粗/斜体/行内代码 + 换行；先转义 HTML，不引第三方依赖。
// 结构化数据仍走字段，不在此渲染。
// ============================================================

/** 叙事文本 → 安全 HTML 片段（可直接 v-html） */
export function renderMarkdown(text: string): string {
  if (!text) return ''
  // 1) 转义 HTML，杜绝注入
  let out = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
  // 2) 行内代码先抽成占位，避免其中的 ** 被当成加粗
  const codes: string[] = []
  out = out.replace(/`([^`\n]+)`/g, (_m, code: string) => {
    codes.push(code)
    return '\u0000CODE' + (codes.length - 1) + '\u0000'
  })
  // 3) 加粗 / 斜体（非贪婪，不跨行）
  out = out.replace(/\*\*([^\n]+?)\*\*/g, '<strong>$1</strong>')
  out = out.replace(/(^|[^*])\*([^*\n]+?)\*/g, '$1<em>$2</em>')
  // 4) 还原行内代码
  out = out.replace(/\u0000CODE(\d+)\u0000/g, (_m, i: string) => '<code>' + codes[Number(i)] + '</code>')
  // 5) 换行按 Markdown 语义保留
  return out.replace(/\n/g, '<br>')
}
