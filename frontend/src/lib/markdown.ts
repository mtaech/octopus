// ============================================================
// 极简 Markdown 渲染（#01/#22 决议 12：叙事字段格式 = Markdown）
// 零依赖：先转义 HTML 再渲染，行内（代码 / 链接 / 加粗 / 斜体 / 删除线）+
// 块级（标题 / 列表 / 表格 / 引用 / 围栏代码 / 分隔线）。
// 结构化数据仍走字段，不在此渲染。游玩页 / 文档视图 / 结对会话共用本实现。
// ============================================================

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

/** 只放行安全 URL；其余按纯文本处理，避免 javascript: 注入 */
function safeUrl(raw: string): string | null {
  const u = raw.trim()
  return /^(https?:\/\/|mailto:|\/|#)/i.test(u) ? u : null
}

/** 行内渲染：转义 → 行内代码 → 链接 → 加粗 / 删除线 / 斜体 */
export function renderInline(text: string): string {
  let out = escapeHtml(text)
  const codes: string[] = []
  out = out.replace(/`([^`\n]+)`/g, (_m, code: string) => {
    codes.push(code)
    return '\u0000C' + (codes.length - 1) + '\u0000'
  })
  out = out.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (whole, label: string, url: string) => {
    const u = safeUrl(url)
    return u ? '<a href="' + u + '" target="_blank" rel="noopener noreferrer">' + label + '</a>' : whole
  })
  out = out.replace(/\*\*([^\n]+?)\*\*/g, '<strong>$1</strong>')
  out = out.replace(/~~([^\n]+?)~~/g, '<del>$1</del>')
  out = out.replace(/(^|[^*])\*([^*\n]+?)\*/g, '$1<em>$2</em>')
  out = out.replace(/\u0000C(\d+)\u0000/g, (_m, i: string) => '<code>' + codes[Number(i)] + '</code>')
  return out
}

/** 行内版（不产生块级标签）：用于台词 / 旁白等必须留在行内流的场景（<span> 内）。 */
export function renderMarkdownInline(text: string): string {
  if (!text) return ''
  return renderInline(text.replace(/\r\n?/g, '\n')).replace(/\n/g, '<br>')
}

function splitRow(line: string): string[] {
  let s = line.trim()
  if (s.startsWith('|')) s = s.slice(1)
  if (s.endsWith('|')) s = s.slice(0, -1)
  return s.split('|').map(c => c.trim())
}

function isTableDelim(line: string): boolean {
  return line.includes('-') && /^\s*\|?[\s:|-]*\|[\s:|-]*\|?\s*$/.test(line)
}

function isBlockStart(lines: string[], i: number): boolean {
  const line = lines[i]
  if (!line.trim()) return true
  if (/^```/.test(line)) return true
  if (/^#{1,6}\s+/.test(line)) return true
  if (/^\s*>/.test(line)) return true
  if (/^\s*([-*+]|\d+\.)\s+/.test(line)) return true
  if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) return true
  if (line.includes('|') && i + 1 < lines.length && isTableDelim(lines[i + 1])) return true
  return false
}

/** 连续列表行 → 嵌套 <ul>/<ol>（按缩进层级） */
function renderList(lines: string[], start: number): { html: string; next: number } {
  const out: string[] = []
  const stack: { indent: number; type: 'ul' | 'ol' }[] = []
  let i = start
  while (i < lines.length) {
    const m = lines[i].match(/^(\s*)([-*+]|\d+\.)\s+(.*)$/)
    if (!m) break
    const indent = m[1].replace(/\t/g, '  ').length
    const type: 'ul' | 'ol' = /\d/.test(m[2]) ? 'ol' : 'ul'
    while (stack.length && indent < stack[stack.length - 1].indent) {
      const s = stack.pop() as { type: 'ul' | 'ol' }
      out.push(s.type === 'ul' ? '</ul>' : '</ol>')
    }
    const top = stack[stack.length - 1]
    if (!top || indent > top.indent) {
      stack.push({ indent, type })
      out.push(type === 'ul' ? '<ul>' : '<ol>')
    } else if (top.type !== type) {
      out.push(top.type === 'ul' ? '</ul>' : '</ol>')
      stack.pop()
      stack.push({ indent, type })
      out.push(type === 'ul' ? '<ul>' : '<ol>')
    }
    out.push('<li>' + renderInline(m[3]) + '</li>')
    i++
  }
  while (stack.length) {
    const s = stack.pop() as { type: 'ul' | 'ol' }
    out.push(s.type === 'ul' ? '</ul>' : '</ol>')
  }
  return { html: out.join(''), next: i }
}

/** 文本 → 安全 HTML 片段（可直接 v-html） */
export function renderMarkdown(text: string): string {
  if (!text) return ''
  const lines = text.replace(/\r\n?/g, '\n').split('\n')
  const html: string[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    // 围栏代码
    const fence = line.match(/^```(\S*)\s*$/)
    if (fence) {
      const lang = fence[1]
      const buf: string[] = []
      i++
      while (i < lines.length && !/^```\s*$/.test(lines[i])) { buf.push(lines[i]); i++ }
      i++
      html.push('<pre><code' + (lang ? ' class="language-' + escapeHtml(lang) + '"' : '') + '>' + escapeHtml(buf.join('\n')) + '</code></pre>')
      continue
    }
    if (!line.trim()) { i++; continue }
    // 标题
    const h = line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/)
    if (h) {
      const n = h[1].length
      html.push('<h' + n + '>' + renderInline(h[2]) + '</h' + n + '>')
      i++
      continue
    }
    // 分隔线
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) { html.push('<hr>'); i++; continue }
    // 引用
    if (/^\s*>/.test(line)) {
      const buf: string[] = []
      while (i < lines.length && /^\s*>/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*>\s?/, ''))
        i++
      }
      html.push('<blockquote>' + renderInline(buf.join('\n')).replace(/\n/g, '<br>') + '</blockquote>')
      continue
    }
    // 表格
    if (line.includes('|') && i + 1 < lines.length && isTableDelim(lines[i + 1])) {
      const aligns = splitRow(lines[i + 1]).map(c => /^:-+:$/.test(c) ? 'center' : /-+:$/.test(c) ? 'right' : '')
      const head = splitRow(line)
      const body: string[][] = []
      i += 2
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) {
        body.push(splitRow(lines[i]))
        i++
      }
      const cell = (tag: 'th' | 'td', txt: string, k: number): string =>
        '<' + tag + (aligns[k] ? ' style="text-align:' + aligns[k] + '"' : '') + '>' + renderInline(txt) + '</' + tag + '>'
      html.push(
        '<table><thead><tr>' + head.map((c, k) => cell('th', c, k)).join('') + '</tr></thead><tbody>'
        + body.map(row => '<tr>' + head.map((_c, k) => cell('td', row[k] ?? '', k)).join('') + '</tr>').join('')
        + '</tbody></table>',
      )
      continue
    }
    // 列表
    if (/^\s*([-*+]|\d+\.)\s+/.test(line)) {
      const r = renderList(lines, i)
      html.push(r.html)
      i = r.next
      continue
    }
    // 段落（保留软换行为 <br>）
    const buf: string[] = []
    while (i < lines.length && !isBlockStart(lines, i)) { buf.push(lines[i]); i++ }
    if (!buf.length) { html.push('<p>' + renderInline(lines[i]) + '</p>'); i++; continue }
    html.push('<p>' + renderInline(buf.join('\n')).replace(/\n/g, '<br>') + '</p>')
  }
  return html.join('')
}
