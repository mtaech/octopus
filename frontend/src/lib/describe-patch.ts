// ============================================================
// 把「工具改动 patch」翻译成人话：字段 → 中文标签 + 可读值。
// 结对「待审查改动」卡片专用——**绝不向用户展示原始 JSON**。
// 纯函数，无 Vue 依赖，便于单独验证。
// ============================================================
import { refKindLabel } from './entity-refs'

export interface PatchDetail {
  label: string
  value: string
}

/** patch 字段 → 中文标签 */
const FIELD_LABEL: Record<string, string> = {
  name: '名称', title: '标题', label: '显示名', key: '标识', description: '描述', summary: '说明',
  background: '背景', personality: '性格', appearance: '外观', kind: '类型',
  premise: '世界前提', opening: '故事开头', check: '判定方式',
  baseline: '基线', options: '可选值', min: '最小值', max: '最大值', default_max: '上限',
  category: '分类', target: '目标类型', cost: '消耗', cooldown: '冷却', effect: '效果',
  status: '状态', duration: '时长', unit: '单位', stack: '叠加',
  attributes: '属性', resources: '资源', skills: '关联技能', quantity: '数量', type: '类型',
  location_id: '地点', parent_id: '父级', default_attitude: '默认态度', goals: '目标', scenes: '场景',
  text: '文本', primary: '主目标', hidden: '隐藏', repeatable: '可重复', condition: '触发条件', hint: '提示',
  from_kind: '主体类型', from: '主体', to_kind: '客体类型', to: '客体', value: '强度',
  from_id: '主体（旧）', to_id: '客体（旧）',
  present_char_ids: '在场人物', actions: '动作', level: '档位', tone: '语气', emotion: '情绪',
  audience: '听众', gesture: '动作',
  fields: '字段', group: '分组', singleton: '单例', mechanics: '结算机制', ref_kind: '引用种类',
  content: '内容', keys: '触发词', priority: '优先级', constant: '常驻', recursive: '递归扫描', enabled: '启用',
}

/** 不该出现在卡片上的噪音字段：id/key 已体现在标题里；lua 由卡片里的只读代码编辑器单独呈现，避免重复 */
const SKIP = new Set(['id', 'key', 'schema_version', 'parent_id', 'lua'])

function shortValue(v: unknown): string {
  if (v === null || v === undefined) return '—'
  if (Array.isArray(v)) return v.length + ' 项'
  if (typeof v === 'object') return '已配置'
  if (typeof v === 'boolean') return v ? '是' : '否'
  const s = String(v)
  return s.length > 40 ? s.slice(0, 40) + '…' : s
}

/** 数组元素的展示名（取 name/title/label/text/key/id 之一） */
function itemName(x: unknown): string {
  if (typeof x === 'string' || typeof x === 'number') return String(x)
  if (x && typeof x === 'object') {
    const r = x as Record<string, unknown>
    return String(r.name ?? r.title ?? r.label ?? r.text ?? r.key ?? r.id ?? '')
  }
  return ''
}

/** 可读值：数组取前几个名字、对象摊成 k v、长文本截断——任何情况下都不吐 JSON */
export function formatPatchValue(key: string, v: unknown): string {
  if (v === null || v === undefined) return '—'
  if (key === 'cost' && Array.isArray(v)) {
    const parts = v
      .map(c => {
        const r = (c ?? {}) as Record<string, unknown>
        const res = String(r.resource ?? '')
        const amt = r.amount == null ? '' : String(r.amount)
        return res ? res + (amt ? '×' + amt : '') : ''
      })
      .filter(Boolean)
    if (parts.length) return parts.join('、')
  }
  if (Array.isArray(v)) {
    const names = v.map(itemName).filter(Boolean)
    if (!names.length) return v.length + ' 项'
    const head = names.slice(0, 6).join('、')
    return names.length > 6 ? head + ' 等 ' + names.length + ' 项' : head
  }
  if (typeof v === 'object') {
    const entries = Object.entries(v as Record<string, unknown>).filter(([, val]) => val !== undefined)
    if (!entries.length) return '空'
    const head = entries
      .slice(0, 4)
      .map(([k, val]) => (FIELD_LABEL[k] ?? k) + ' ' + shortValue(val))
      .join('、')
    return entries.length > 4 ? head + ' 等 ' + entries.length + ' 项' : head
  }
  if (typeof v === 'boolean') return v ? '是' : '否'
  if (key === 'kind' && (v === 'pc' || v === 'npc')) return v === 'pc' ? '玩家角色' : '非玩家角色'
  if (key === 'cooldown' && typeof v === 'object' && !Array.isArray(v)) {
    const t = (v as Record<string, unknown>).turns
    if (t != null) return t + ' 回合'
  }
  if (key === 'type') {
    const T: Record<string, string> = { number: '数值', enum: '枚举', text: '文本', numerical: '数值', binary: '有 / 无' }
    if (T[String(v)]) return T[String(v)]
  }
  const s = String(v)
  return s.length > 80 ? s.slice(0, 80) + '…' : s
}

/** patch → 卡片上的字段列表（中文标签 + 可读值） */
export function describePatch(patch: Record<string, unknown>): PatchDetail[] {
  const out: PatchDetail[] = []
  for (const [k, v] of Object.entries(patch)) {
    if (SKIP.has(k)) continue
    if (v === undefined || v === null || v === '') continue
    const value = formatPatchValue(k, v)
    if (value === '—' || value === '空') continue
    out.push({ label: FIELD_LABEL[k] ?? k, value })
  }
  return out
}

/** 卡片标题：动作 + 中文 kind + 友好名（拿不到名字时由调用方回退） */
export function describeActionTitle(kind: string, action: string, patch: Record<string, unknown>): string {
  const ACTION: Record<string, string> = { create: '新增', update: '更新', delete: '删除' }
  const name = String(patch.label ?? patch.name ?? patch.title ?? patch.text ?? '').trim()
  const head = (ACTION[action] ?? action) + refKindLabel(kind)
  return name ? head + '「' + name + '」' : head
}
