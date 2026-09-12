// ============================================================
// Token 粗估与格式化：CJK 按 1、其余按 1/4。
// 不引分词器依赖——只用于界面计量与预算提示，不需要精确。
// ============================================================

export function estimateTokens(text: string): number {
  if (!text) return 0
  let cjk = 0
  let other = 0
  for (const ch of text) {
    const c = ch.codePointAt(0) ?? 0
    // CJK 统一表意 + 中文标点 + 全角符号：按 1 token 粗估
    if ((c >= 0x4e00 && c <= 0x9fff) || (c >= 0x3000 && c <= 0x303f) || (c >= 0xff00 && c <= 0xffef)) cjk++
    else other++
  }
  return Math.ceil(cjk + other / 4)
}

/** 任意 JSON 值的 token 粗估 */
export function estimateTokensOf(value: unknown): number {
  try {
    return estimateTokens(JSON.stringify(value) ?? '')
  } catch {
    return 0
  }
}

/** 1234 → 1.2k；1234567 → 1.2M */
export function fmtTokens(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M'
  if (n >= 1000) return (n / 1000).toFixed(n >= 10_000 ? 0 : 1) + 'k'
  return String(n)
}
