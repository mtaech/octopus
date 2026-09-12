// ============================================================
// 派生值 / 修正来源（#2）
// 小表达式求值器 + 修正叠加 + 派生计算。与 Rust 端 crates/octopus-engine/src/derived.rs 同语法。
// 语法：数值 / 变量（属性维度 key 或前面已声明的派生 key）/ + - * / ( )
//      函数 floor ceil round min max abs
// ============================================================
import type { CharacterDef, Modifier, Storybook } from '@/types'

export type VarMap = Record<string, number>

/** 前缀无关的表达式求值：出错抛异常（调用方决定显示 — 还是报错）。 */
export function evalFormula(src: string, vars: VarMap): number {
  const s = src
  let i = 0
  const skip = (): void => { while (i < s.length && /\s/.test(s[i])) i++ }
  const peek = (): string | undefined => { skip(); return s[i] }

  function parseExpr(): number { return parseAdd() }
  function parseAdd(): number {
    let v = parseMul()
    for (;;) {
      const c = peek()
      if (c === '+') { i++; v += parseMul() }
      else if (c === '-') { i++; v -= parseMul() }
      else return v
    }
  }
  function parseMul(): number {
    let v = parseUnary()
    for (;;) {
      const c = peek()
      if (c === '*') { i++; v *= parseUnary() }
      else if (c === '/') { i++; v /= parseUnary() }
      else return v
    }
  }
  function parseUnary(): number {
    const c = peek()
    if (c === '-') { i++; return -parseUnary() }
    if (c === '+') { i++; return parseUnary() }
    return parsePrimary()
  }
  function parsePrimary(): number {
    const c = peek()
    if (c === '(') {
      i++
      const v = parseExpr()
      if (peek() !== ')') throw new Error('缺少右括号')
      i++
      return v
    }
    if (c !== undefined && /[0-9.]/.test(c)) {
      const start = i
      while (i < s.length && /[0-9.]/.test(s[i])) i++
      return Number(s.slice(start, i))
    }
    if (c !== undefined && /[A-Za-z_]/.test(c)) {
      const start = i
      while (i < s.length && /[A-Za-z0-9_.]/.test(s[i])) i++
      const name = s.slice(start, i)
      if (peek() === '(') {
        i++
        const args: number[] = []
        if (peek() !== ')') {
          args.push(parseExpr())
          while (peek() === ',') { i++; args.push(parseExpr()) }
        }
        if (peek() !== ')') throw new Error('缺少右括号')
        i++
        return applyFn(name, args)
      }
      if (!(name in vars)) throw new Error('未知变量 ' + name)
      return vars[name]
    }
    throw new Error('意外的字符 ' + String(c))
  }

  const out = parseExpr()
  skip()
  if (i < s.length) throw new Error('多余的输入 ' + s.slice(i))
  return out
}

function applyFn(name: string, a: number[]): number {
  switch (name) {
    case 'floor': return Math.floor(a[0])
    case 'ceil': return Math.ceil(a[0])
    case 'round': return Math.round(a[0])
    case 'abs': return Math.abs(a[0])
    case 'min': return Math.min(...a)
    case 'max': return Math.max(...a)
    default: throw new Error('未知函数 ' + name)
  }
}

/** 收集修正来源：挂接定义的 modifiers + 已装备物品的 modifiers。 */
export function collectModifiers(sb: Storybook, c: CharacterDef): Modifier[] {
  const out: Modifier[] = []
  for (const ids of Object.values(c.attachments ?? {})) {
    for (const id of ids ?? []) {
      const def = (sb.definitions ?? []).find(x => x.id === id)
      for (const m of def?.modifiers ?? []) out.push(m)
    }
  }
  const equipped = new Set(c.equipped ?? [])
  for (const entry of c.inventory ?? []) {
    if (!equipped.has(entry.id)) continue
    const item = (sb.items ?? []).find(i => i.id === entry.id)
    for (const m of item?.modifiers ?? []) out.push(m)
  }
  return out
}

export interface ModifierBuckets { add: Record<string, number>; max: Record<string, number>; set: Record<string, number> }

/** 修正来源按 target 归入 add（累加）/ max（取高）/ set（覆盖）。 */
export function bucketModifiers(mods: Modifier[]): ModifierBuckets {
  const add: Record<string, number> = {}
  const max: Record<string, number> = {}
  const set: Record<string, number> = {}
  for (const m of mods) {
    if (!m || typeof m.target !== 'string') continue
    if (m.op === 'set') set[m.target] = m.value
    else if (m.op === 'max') max[m.target] = Math.max(max[m.target] ?? -Infinity, m.value)
    else add[m.target] = (add[m.target] ?? 0) + m.value
  }
  return { add, max, set }
}

function applyBuckets(base: number, key: string, b: ModifierBuckets): number {
  let v = base + (b.add[key] ?? 0)
  if (key in b.max) v = Math.max(v, b.max[key])
  if (key in b.set) v = b.set[key]
  return v
}

/** 基础属性 + 修正叠加后的有效属性。 */
export function effectiveAttrs(sb: Storybook, c: CharacterDef): VarMap {
  const attrs: VarMap = {}
  for (const dim of sb.attribute_dimensions ?? []) {
    if (dim.type !== 'number') continue
    const raw = c.attributes?.[dim.key]
    const n = typeof raw === 'number' ? raw : Number(raw)
    attrs[dim.key] = Number.isFinite(n) ? n : 0
  }
  const b = bucketModifiers(collectModifiers(sb, c))
  for (const k of Object.keys(attrs)) attrs[k] = applyBuckets(attrs[k], k, b)
  return attrs
}

export interface DerivedValue {
  key: string
  label: string
  group?: string
  value: number | null
  signed?: boolean
  error?: string
}

/** 计算某角色的全部派生值（按声明顺序，后者可引用前者）。 */
export function computeDerived(sb: Storybook, c: CharacterDef): { attrs: VarMap; derived: DerivedValue[] } {
  const attrs = effectiveAttrs(sb, c)
  const buckets = bucketModifiers(collectModifiers(sb, c))
  const vars: VarMap = { ...attrs }
  const derived: DerivedValue[] = []
  for (const def of sb.derived ?? []) {
    try {
      const v = applyBuckets(evalFormula(def.formula, vars), def.key, buckets)
      vars[def.key] = v
      derived.push({ key: def.key, label: def.label || def.key, group: def.group, value: v, signed: def.signed })
    } catch (e) {
      derived.push({ key: def.key, label: def.label || def.key, group: def.group, value: null, signed: def.signed, error: (e as Error).message })
    }
  }
  return { attrs, derived }
}
