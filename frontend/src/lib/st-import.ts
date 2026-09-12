// ============================================================
// SillyTavern 兼容导入：解析角色卡（PNG 内嵌 JSON / V2 JSON）与世界书 JSON，
// 转换为本项目的「人物」与「世界词条」。纯前端、纯函数，便于离线复用。
// 参考：character-card-spec-v2 与 SillyTavern world info 结构。
// ============================================================
import type { CharacterDef, LoreDef } from '@/types'
import { uid } from '@/types'

export interface StCharacterImport {
  character: CharacterDef
  lore: LoreDef[]
  /** 原卡 first_mes：可作为新故事书的「故事开头」 */
  opening?: string
}

/** base64 → UTF-8 JSON（失败返回 null）。 */
function decodeBase64Json(b64: string): unknown {
  try {
    const bin = atob(b64.trim())
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    const text = new TextDecoder('utf-8').decode(bytes)
    return JSON.parse(text)
  } catch {
    return null
  }
}

/** 读取 PNG tEXt / iTXt 分块里的 chara（base64 JSON）；zTXt 压缩块不支持。 */
export function extractCharaFromPng(bytes: Uint8Array): unknown | null {
  const sig = [137, 80, 78, 71, 13, 10, 26, 10]
  if (bytes.length < 8 || sig.some((b, i) => bytes[i] !== b)) return null
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  let off = 8
  while (off + 12 <= bytes.length) {
    const len = dv.getUint32(off)
    const type = String.fromCharCode(bytes[off + 4], bytes[off + 5], bytes[off + 6], bytes[off + 7])
    const start = off + 8
    const end = start + len
    if (end + 4 > bytes.length) break
    if (type === 'tEXt' || type === 'iTXt') {
      const nul = bytes.indexOf(0, start)
      if (nul > start && nul < end) {
        const keyword = latin1(bytes, start, nul)
        if (keyword === 'chara') {
          if (type === 'tEXt') return decodeBase64Json(latin1(bytes, nul + 1, end))
          // iTXt: keyword \0 compFlag(1) compMethod(1) lang \0 translated \0 text
          const compFlag = bytes[nul + 1]
          if (compFlag === 1) return null
          const langEnd = bytes.indexOf(0, nul + 3)
          if (langEnd < 0) return null
          const transEnd = bytes.indexOf(0, langEnd + 1)
          if (transEnd < 0) return null
          const text = new TextDecoder('utf-8').decode(bytes.subarray(transEnd + 1, end))
          return decodeBase64Json(text)
        }
      }
    }
    if (type === 'IEND') break
    off = end + 4
  }
  return null
}

function latin1(bytes: Uint8Array, start: number, end: number): string {
  let s = ''
  for (let i = start; i < end; i++) s += String.fromCharCode(bytes[i])
  return s
}

function str(v: unknown): string { return typeof v === 'string' ? v.trim() : '' }
function isObj(v: unknown): v is Record<string, unknown> { return !!v && typeof v === 'object' && !Array.isArray(v) }

/** SillyTavern 宏替换：{{char}} / <BOT> → 人物名；{{user}} / <USER> → 玩家。未识别的宏保持原样。 */
function substituteMacros(text: string, charName: string): string {
  if (!text) return text
  const who = charName || '该角色'
  return text
    .replace(/\{\{\s*char\s*\}\}/gi, who)
    .replace(/\{\{\s*user\s*\}\}/gi, '你')
    .replace(/<BOT>/gi, who)
    .replace(/<USER>/gi, '你')
}

/** 宏替换用的短名：ST 卡名常带描述（"Lucy - your math teacher"），取分隔符前的部分。 */
function shortName(name: string): string {
  const head = name.split(/\s+[-–—|:：,，]\s+|\s*[（(]/)[0]?.trim()
  return head || name
}

/** 词条标题：优先用 comment/name；否则取首个触发词，过长则截断。 */
function shortTitle(s: string, fallback: string): string {
  const t = s.trim() || fallback
  return t.length > 40 ? t.slice(0, 40) + '…' : t
}

/** 角色卡 JSON（V1 平铺 / V2 {spec,data}）→ 人物 + 内嵌角色书词条。 */
export function parseCharacterCard(card: unknown): StCharacterImport | null {
  if (!isObj(card)) return null
  const data = isObj(card.data) ? card.data : card
  const name = str(data.name)
  if (!name) return null
  const macroName = shortName(name)
  const background = substituteMacros(str(data.description), macroName)
  const personality = substituteMacros(str(data.personality), macroName)
  const scenario = substituteMacros(str(data.scenario), macroName)
  const firstMes = substituteMacros(str(data.first_mes), macroName)
  const mesExample = substituteMacros(str(data.mes_example), macroName)
  const creatorNotes = substituteMacros(str(data.creator_notes), macroName)
  const systemPrompt = substituteMacros(str(data.system_prompt), macroName)
  const postHistory = substituteMacros(str(data.post_history_instructions), macroName)
  const alternates = Array.isArray(data.alternate_greetings) ? data.alternate_greetings : []
  const notes = [
    '（从 SillyTavern 角色卡导入）',
    scenario ? '场景：' + scenario : '',
    firstMes ? '开场白：' + firstMes : '',
    creatorNotes ? '创作者注释：' + creatorNotes : '',
    systemPrompt ? '原卡系统提示词（未注入，供参考）：' + systemPrompt : '',
    postHistory ? '原卡末位指令（未注入，供参考）：' + postHistory : '',
    alternates.length ? `原卡另有 ${alternates.length} 条备选开场白（未导入）` : '',
  ].filter(Boolean).join(String.fromCharCode(10))
  const character: CharacterDef = {
    id: uid('char'),
    name,
    kind: 'npc',
    background,
    personality,
    attributes: {},
    resources: {},
    skills: [],
    example_dialogues: mesExample || undefined,
    notes: notes || undefined,
  }
  const book = isObj(data.character_book) ? data.character_book : undefined
  const lore = book ? worldInfoEntriesToLore(book.entries, macroName) : []
  return { character, lore, opening: firstMes || undefined }
}

/** 世界书 JSON（{entries:{...}} / {entries:[...]} / 直接 entries）→ 世界词条。 */
export function parseWorldInfo(input: unknown, charName = ''): LoreDef[] {
  if (!isObj(input)) return []
  return worldInfoEntriesToLore(input.entries ?? input, charName)
}

function worldInfoEntriesToLore(raw: unknown, charName = ''): LoreDef[] {
  if (!raw) return []
  const list: unknown[] = Array.isArray(raw)
    ? raw
    : isObj(raw) ? Object.values(raw) : []
  const out: LoreDef[] = []
  for (const e of list) {
    if (!isObj(e)) continue
    const content = substituteMacros(str(e.content), charName)
    if (!content) continue
    const keys = [e.key, e.keys, e.keysecondary]
      .filter(Array.isArray)
      .flatMap(a => (a as unknown[]).map(k => substituteMacros(String(k).trim(), charName)))
      .filter(Boolean)
    const title = shortTitle(str(e.comment) || str(e.name) || keys[0] || '', '词条')
    const rawOrder = e.insertion_order ?? e.order ?? e.priority ?? 0
    const priority = Number.isFinite(Number(rawOrder)) ? Number(rawOrder) : 0
    const constant = e.constant === true || e.constant === 'true'
    const recursive = e.recursive === true || e.recursive === 'true' || e.scan_depth != null
    const enabled = e.enabled === undefined ? e.disable !== true : e.enabled !== false
    out.push({
      id: uid('lore'),
      title,
      content,
      keys,
      priority,
      constant,
      recursive,
      enabled,
    })
  }
  return out
}

/** 读文件 → 解析。返回导入的人物与词条；无法识别时抛错。 */
export async function importStFile(file: File): Promise<StCharacterImport> {
  const buf = new Uint8Array(await file.arrayBuffer())
  const isPng = file.name.toLowerCase().endsWith('.png') || (buf.length > 8 && buf[0] === 137 && buf[1] === 80)
  if (isPng) {
    const card = extractCharaFromPng(buf)
    if (!card) throw new Error('这张 PNG 里没有找到 SillyTavern 角色卡数据（chara 分块）')
    const parsed = parseCharacterCard(card)
    if (!parsed) throw new Error('角色卡数据缺少 name 字段，无法导入')
    return parsed
  }
  const text = new TextDecoder('utf-8').decode(buf)
  let json: unknown
  try { json = JSON.parse(text) } catch { throw new Error('不是合法的 JSON 文件') }
  const parsed = parseCharacterCard(json)
  if (parsed) return parsed
  const lore = parseWorldInfo(json)
  if (lore.length) {
    return { character: { id: uid('char'), name: '（仅世界书）', kind: 'npc', background: '', personality: '', attributes: {}, resources: {}, skills: [] }, lore }
  }
  throw new Error('无法识别：既不是角色卡，也没有可用的世界书词条')
}
