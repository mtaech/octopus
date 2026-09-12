// ============================================================
// SillyTavern 预设导入：把 ST 预设的 prompts / prompt_order / 采样 / regex_scripts
// 转换为本项目的「叙述段提案」。纯函数、确定性；不做正则执行层，不直接改草稿。
// 设计见 docs/p3-think-and-import.md §B。仿 st-import.ts 的转换器风格。
// ============================================================
import type { NarrativeVariant } from '@/types'

export type StRating = 'sfw' | 'nsfw'
export type StSectionSlot = 'world' | 'style' | 'behavior' | 'closing'

/** 提案里的一条叙述段：字段是 NarrativeSection 的可导入子集 + 来源信息，供提案 UI 展示。 */
export interface ProposedSection {
  id: string
  title: string
  slot: StSectionSlot
  scope: 'both'
  /** 常驻文本；与 variants 互斥 */
  text?: string
  /** 互斥变体组（人称 / 推进速度 / 篇幅） */
  variants?: NarrativeVariant[]
  defaultVariant?: string
  /** ST 源 prompt identifier（uuid / 具名槽如 jailbreak / 自定义 id） */
  sourceIdentifier: string
  /** ST 源名（prompt name） */
  sourceName: string
  /** 启发式没有把握（无关键词命中），UI 标「需要你确认」 */
  needsConfirm?: boolean
}

export type DroppedKind = 'placeholder' | 'regex' | 'empty' | 'header' | 'config' | 'sampling'

export interface DroppedItem {
  identifier: string
  name: string
  kind: DroppedKind
  reason: string
}

export interface StPresetProposal {
  sections: ProposedSection[]
  dropped: DroppedItem[]
  rating: StRating
}

/** ST 预设 JSON 里的单个 prompt（只取导入需要的字段）。 */
interface StPrompt {
  identifier: string
  name: string
  content: string
  enabled: boolean
  marker: boolean
}

function isObj(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}
function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

// ---------- 常量：占位符 / 正则 / NSFW / 分类关键词 ----------

/** ST 里由 Octopus 槽位注入的占位 prompt：内容来自故事书其它字段，不导入。 */
const PLACEHOLDER_IDS = new Set([
  'worldInfoBefore',
  'worldInfoAfter',
  'charDescription',
  'charPersonality',
  'personaDescription',
  'scenario',
  'chatHistory',
])
const PLACEHOLDER_REASON = '由槽位注入（世界前提 / 人物档案 / 场景 / 历史），不需要导入'
const DIALOGUE_EXAMPLES_REASON = '对话示例由人物档案的 example_dialogues 承载'

/** 正则脚本一律不导入：本系统没有文本变换层。 */
const REGEX_REASON = 'Octopus 不做正则层；隐藏思考走 think 意图'
const HEADER_REASON = 'SillyTavern 的分组标题 / 包裹行，不单独导入（其选项已整理为变体组）'
const EMPTY_REASON = '空内容，没有可导入的文本'
const CONFIG_REASON = 'SillyTavern 预设内部配置，不属于叙事契约'
const SAMPLING_REASON = '不写入叙事契约；请用现有「采样预设」（温度 / Top-P / 惩罚 等）'

/** NSFW 关键词表（命中即把提案 rating 标为 nsfw；只做建议，创作者可改）。 */
const NSFW_KEYWORDS = [
  'nsfw', '色情', '情欲', '情色', '瑟瑟', '性爱', '淫', '肉文', '成人内容',
  'explicit', 'erotic', 'porn', '🔞',
]

/** 文风类关键词（命中倾向 slot: style）。 */
const STYLE_KEYWORDS = [
  '文风', '笔触', '修辞', '描写', '句式', '字数', '风格', '文笔', '白描', '比喻',
  '排版', '段落', '叙事基调', '写作', '对话', '感官', '视角', '措辞', '行文',
]

/** 行为约束类关键词（命中倾向 slot: behavior）。 */
const BEHAVIOR_KEYWORDS = [
  '禁止', '不要', '必须', '规则', '一致性', '铁律', '自检', '权限', '边界', '主权',
  '约束', '不准', '不能', '全知', '抢话', '媚', '八股', 'ooc', '人设', '审查',
  '称呼', '自主性', '抗重复', '防',
]

/** 互斥变体组：只识别规格点名的三类（人称 / 推进速度 / 篇幅），避免误吞普通段。 */
const VARIANT_GROUPS: { key: string; label: string; match: (hay: string) => boolean }[] = [
  { key: 'pov', label: '人称视角', match: h => /人称|视角|<pov>/i.test(h) },
  { key: 'pace', label: '推进速度', match: h => /推进速度|快进|<pace>/i.test(h) },
  { key: 'length', label: '篇幅', match: h => /字数|篇幅|response_length/i.test(h) },
]

/** 采样参数键：命中即提示用户用现有采样预设（不写进叙事契约）。 */
const SAMPLING_KEYS = [
  'temperature', 'top_p', 'top_k', 'top_a', 'min_p',
  'frequency_penalty', 'presence_penalty', 'repetition_penalty',
]

// ---------- 启发式 ----------

/** ST 分组标题行 / 包裹行：名字含「N选1」或被破折号包裹，内容是纯开闭标签。 */
function isHeaderPrompt(name: string, content: string): boolean {
  const n = name.trim()
  if (/选\s*[0-9一二三四五六七八九]/.test(n) || /^-+/.test(n) || /-+$/.test(n)) return true
  const c = content.trim()
  return c.length > 0 && c.length <= 24 && /^<\/?[a-zA-Z_]+>\s*$/.test(c)
}

/** 「N选1」标题行 → 已知变体组的 key；未知分组（如 COT 头）返回 null（只当标题丢弃）。 */
function groupHeaderKey(p: StPrompt): string | null {
  if (!/选\s*[0-9一二三四五六七八九]/.test(p.name)) return null
  const g = VARIANT_GROUPS.find(x => x.match(p.name))
  return g ? g.key : null
}

/**
 * 按 ST 的「N选1」标题区块收集变体选项：标题行开区块，同名标题行 / 分隔行 / 占位行收尾。
 * 返回各组选项与「已被变体组消费」的 prompt id（单成员组会在上层从消费集合移除、回落普通段）。
 */
function detectVariantGroups(ordered: StPrompt[]): { groups: Map<string, StPrompt[]>; consumed: Set<string> } {
  const groups = new Map<string, StPrompt[]>()
  const consumed = new Set<string>()
  let cur: { key: string; members: StPrompt[] } | null = null
  const close = (): void => {
    if (cur) {
      groups.set(cur.key, cur.members)
      cur = null
    }
  }
  for (const p of ordered) {
    const hk = groupHeaderKey(p)
    if (hk) {
      if (cur && cur.key === hk) close()
      else {
        close()
        cur = { key: hk, members: [] }
      }
      continue
    }
    if (PLACEHOLDER_IDS.has(p.identifier) || p.marker || p.identifier === 'SPresetSettings' || isHeaderPrompt(p.name, p.content)) {
      close()
      continue
    }
    if (cur && p.content.trim()) {
      cur.members.push(p)
      consumed.add(p.identifier)
    }
  }
  close()
  return { groups, consumed }
}

/** 语义分类：命中关键词多的一侧胜出；都没命中 → style 且标「需要确认」。 */
function classify(name: string, content: string): { slot: 'style' | 'behavior'; confirmed: boolean } {
  const hay = name + '\n' + content
  let style = 0
  let behavior = 0
  for (const k of STYLE_KEYWORDS) if (hay.includes(k)) style++
  for (const k of BEHAVIOR_KEYWORDS) if (hay.includes(k)) behavior++
  if (style === 0 && behavior === 0) return { slot: 'style', confirmed: false }
  return { slot: behavior > style ? 'behavior' : 'style', confirmed: true }
}

/** 从 ST 来源生成确定性 id（重复导入时由采纳侧补后缀）。 */
function sectionId(raw: string): string {
  const slug = raw
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 48)
    .toLowerCase()
  return 'nar-st-' + (slug || 'item')
}

/** prompt_order → 有序 identifier + 激活态；多 block 合并（先出现定序，后出现覆盖 enabled）。 */
function readPromptOrder(raw: unknown): { identifier: string; enabled: boolean }[] {
  if (!Array.isArray(raw)) return []
  const out: { identifier: string; enabled: boolean }[] = []
  const index = new Map<string, number>()
  for (const block of raw) {
    if (!isObj(block) || !Array.isArray(block.order)) continue
    for (const e of block.order) {
      if (!isObj(e)) continue
      const id = str(e.identifier)
      if (!id) continue
      const on = e.enabled !== false
      const at = index.get(id)
      if (at === undefined) {
        index.set(id, out.length)
        out.push({ identifier: id, enabled: on })
      } else {
        out[at].enabled = on
      }
    }
  }
  return out
}

/** 全量扫描 prompt / regex 文本，命中 NSFW 关键词表则判为 nsfw。 */
function inferRating(input: unknown): StRating {
  if (!isObj(input) && !Array.isArray(input)) return 'sfw'
  const parts: string[] = []
  if (isObj(input)) {
    const prompts = Array.isArray(input.prompts) ? input.prompts : []
    for (const p of prompts) {
      if (!isObj(p)) continue
      parts.push(str(p.name))
      if (typeof p.content === 'string') parts.push(p.content)
    }
    const ext = isObj(input.extensions) ? input.extensions : undefined
    const scripts = ext && Array.isArray(ext.regex_scripts) ? ext.regex_scripts : []
    for (const s of scripts) if (isObj(s)) parts.push(regexHaystack(s))
  } else {
    for (const s of input) if (isObj(s)) parts.push(regexHaystack(s))
  }
  const hay = parts.join('\n').toLowerCase()
  return NSFW_KEYWORDS.some(k => hay.includes(k)) ? 'nsfw' : 'sfw'
}

function regexHaystack(s: Record<string, unknown>): string {
  return [str(s.scriptName), str(s.name), str(s.findRegex), str(s.replaceString)].join(' ')
}

function regexDropped(s: Record<string, unknown>): DroppedItem {
  return {
    identifier: str(s.id) || str(s.scriptName) || '(regex)',
    name: str(s.scriptName) || str(s.name) || '未命名正则',
    kind: 'regex',
    reason: REGEX_REASON,
  }
}

// ---------- 转换器 ----------

/**
 * ST 预设 / 正则脚本文件 → 提案。纯函数、不读写草稿；creator 在提案 UI 里逐段采纳。
 * 兼容三种输入：ST 预设对象（prompts + prompt_order + extensions）、
 * 独立正则脚本数组（silly/regex-*.json）、以及只有 regex_scripts 的对象。
 */
export function convertStPreset(input: unknown): StPresetProposal {
  const dropped: DroppedItem[] = []

  // 独立正则脚本文件：只有要丢弃的脚本，没有叙述段。
  if (Array.isArray(input)) {
    for (const s of input) if (isObj(s)) dropped.push(regexDropped(s))
    return { sections: [], dropped, rating: inferRating(input) }
  }
  if (!isObj(input)) return { sections: [], dropped, rating: 'sfw' }

  // 1) 收集 prompts（缺 identifier / 非法项跳过）。
  const rawPrompts = Array.isArray(input.prompts) ? input.prompts : []
  const prompts: StPrompt[] = []
  const byId = new Map<string, StPrompt>()
  for (const p of rawPrompts) {
    if (!isObj(p)) continue
    const identifier = str(p.identifier)
    if (!identifier) continue
    const prompt: StPrompt = {
      identifier,
      name: str(p.name),
      content: typeof p.content === 'string' ? p.content : '',
      enabled: p.enabled !== false,
      marker: p.marker === true,
    }
    prompts.push(prompt)
    byId.set(identifier, prompt)
  }

  // 2) prompt_order 定序，并用它的 enabled 覆盖 prompt 自身（那才是激活配置）。
  const order = readPromptOrder(input.prompt_order)
  const ordered: StPrompt[] = []
  const seen = new Set<string>()
  const enabledOverride = new Map<string, boolean>()
  for (const e of order) {
    const p = byId.get(e.identifier)
    if (!p) continue
    enabledOverride.set(e.identifier, e.enabled)
    if (!seen.has(p.identifier)) {
      seen.add(p.identifier)
      ordered.push(p)
    }
  }
  for (const p of prompts) if (!seen.has(p.identifier)) ordered.push(p)
  const isEnabled = (p: StPrompt) => enabledOverride.get(p.identifier) ?? p.enabled

  // 3) 变体组识别：以 ST 的「N选1」标题行划定区块，区块内为互斥选项。
  //    不按关键词在正文里乱匹配——否则普通文风段只要提到「视角」就会被卷进人称组。
  const { groups, consumed } = detectVariantGroups(ordered)

  // 4) 可用成员 >= 2 才成组；单成员（或全空）回落为普通段。
  const groupSection = new Map<string, ProposedSection>()
  const groupFirstId = new Map<string, string>()
  for (const g of VARIANT_GROUPS) {
    const members = (groups.get(g.key) ?? []).filter(m => m.content.trim())
    if (members.length < 2) {
      for (const m of groups.get(g.key) ?? []) consumed.delete(m.identifier)
      continue
    }
    const variants: NarrativeVariant[] = members.map((m, i) => ({
      key: 'v' + (i + 1),
      label: m.name || m.identifier,
      text: m.content.trim(),
    }))
    const enabledAt = members.findIndex(m => isEnabled(m))
    const defAt = enabledAt >= 0 ? enabledAt : 0
    groupSection.set(g.key, {
      id: sectionId('group-' + g.key),
      title: g.label,
      slot: 'style',
      scope: 'both',
      variants,
      defaultVariant: variants[defAt].key,
      sourceIdentifier: members.map(m => m.identifier).join(','),
      sourceName: g.label,
    })
    groupFirstId.set(g.key, members[0].identifier)
  }

  // 5) 基础过滤 + 普通段收集（被变体组消费的选项不再单独成段）。
  const normal: StPrompt[] = []
  for (const p of ordered) {
    if (p.marker || PLACEHOLDER_IDS.has(p.identifier)) {
      dropped.push({
        identifier: p.identifier,
        name: p.name || p.identifier,
        kind: 'placeholder',
        reason: p.identifier === 'dialogueExamples' ? DIALOGUE_EXAMPLES_REASON : PLACEHOLDER_REASON,
      })
      continue
    }
    if (p.identifier === 'SPresetSettings' || /SPreset/i.test(p.name)) {
      dropped.push({ identifier: p.identifier, name: p.name || p.identifier, kind: 'config', reason: CONFIG_REASON })
      continue
    }
    if (isHeaderPrompt(p.name, p.content)) {
      if (isEnabled(p)) dropped.push({ identifier: p.identifier, name: p.name || p.identifier, kind: 'header', reason: HEADER_REASON })
      continue
    }
    if (consumed.has(p.identifier)) continue
    if (!p.content.trim()) {
      if (isEnabled(p)) dropped.push({ identifier: p.identifier, name: p.name || p.identifier, kind: 'empty', reason: EMPTY_REASON })
      continue
    }
    if (!isEnabled(p)) continue
    normal.push(p)
  }

  // 6) 普通段按源顺序排列；变体组在首个成员出现处插入。
  const normalById = new Map<string, ProposedSection>()
  for (const p of normal) normalById.set(p.identifier, buildSection(p))
  const emittedGroups = new Set<string>()
  const sections: ProposedSection[] = []
  for (const p of ordered) {
    for (const [key, firstId] of groupFirstId) {
      if (firstId === p.identifier && !emittedGroups.has(key)) {
        emittedGroups.add(key)
        sections.push(groupSection.get(key)!)
      }
    }
    const s = normalById.get(p.identifier)
    if (s) sections.push(s)
  }

  // 6) 正则脚本全部丢弃（读而不执行）。
  const ext = isObj(input.extensions) ? input.extensions : undefined
  const scripts = ext && Array.isArray(ext.regex_scripts) ? ext.regex_scripts : []
  for (const s of scripts) if (isObj(s)) dropped.push(regexDropped(s))

  // 7) 采样参数：不写入叙事契约，提示用户用现有采样预设。
  if (SAMPLING_KEYS.some(k => (input as Record<string, unknown>)[k] !== undefined)) {
    dropped.push({ identifier: '(sampling)', name: '采样参数', kind: 'sampling', reason: SAMPLING_REASON })
  }

  return { sections, dropped, rating: inferRating(input) }
}

function buildSection(p: StPrompt): ProposedSection {
  const { slot, confirmed } = classify(p.name, p.content)
  return {
    id: sectionId(p.identifier),
    title: p.name || p.identifier,
    slot,
    scope: 'both',
    text: p.content.trim(),
    sourceIdentifier: p.identifier,
    sourceName: p.name || p.identifier,
    needsConfirm: !confirmed,
  }
}
