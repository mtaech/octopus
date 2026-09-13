// ============================================================
// Lua 编辑上下文（#02 / #12）：从故事书草稿抽取「当前可用标识」，
// 供 CodeEditor 做补全与「可用 API / 资源」参考面板。
// ============================================================
import type { Storybook } from '@/types'

export interface LuaIdOption {
  id: string
  label?: string
}

/** 当前草稿里可被 Lua 引用的标识集合。 */
export interface LuaCompletionContext {
  attributes: LuaIdOption[]
  resources: LuaIdOption[]
  flags: LuaIdOption[]
  events: LuaIdOption[]
  statuses: LuaIdOption[]
  skills: LuaIdOption[]
  items: LuaIdOption[]
  characters: LuaIdOption[]
  locations: LuaIdOption[]
  relationshipTypes: LuaIdOption[]
}

export function emptyLuaContext(): LuaCompletionContext {
  return {
    attributes: [], resources: [], flags: [], events: [], statuses: [],
    skills: [], items: [], characters: [], locations: [], relationshipTypes: [],
  }
}

export function buildLuaContext(d: Storybook | null | undefined): LuaCompletionContext {
  if (!d) return emptyLuaContext()
  // 状态来自故事书顶层声明（技能 effect.status 只是 id 引用）。
  const statuses: LuaIdOption[] = (d.statuses ?? []).map(s => ({ id: s.id, label: s.name }))
  return {
    attributes: (d.attribute_dimensions ?? []).map(x => ({ id: x.key, label: x.label })),
    resources: (d.world?.resources ?? []).map(r => ({ id: r.id, label: r.name })),
    flags: (d.flags ?? []).map(f => ({ id: f.key, label: f.label })),
    events: (d.events ?? []).map(e => ({ id: e.key, label: e.label })),
    statuses,
    skills: (d.skills ?? []).map(s => ({ id: s.id, label: s.name })),
    items: (d.items ?? []).map(i => ({ id: i.id, label: i.name })),
    characters: (d.characters ?? []).map(c => ({ id: c.id, label: c.name })),
    locations: (d.world?.locations ?? []).map(l => ({ id: l.id, label: l.name })),
    relationshipTypes: (d.relationship_types ?? []).map(r => ({ id: r.key, label: r.label })),
  }
}

/** host.* API 参考：签名 + 说明 + 可插入的默认调用文本（纯文本，无占位语法）。 */
export interface LuaApiEntry {
  name: string
  signature: string
  group: '只读' | '可写' | '环境'
  desc: string
  snippet: string
  /** 首个字符串参数对应的上下文 id 组（用于参数内补全）。 */
  argContext?: keyof LuaCompletionContext
}

export const LUA_API_REFERENCE: LuaApiEntry[] = [
  { name: 'get_attribute', signature: 'host.get_attribute(key)', group: '只读', desc: '当前角色属性值', snippet: "host.get_attribute('str')", argContext: 'attributes' },
  { name: 'get_resource', signature: 'host.get_resource(id)', group: '只读', desc: '当前角色资源余额', snippet: "host.get_resource('res-gold')", argContext: 'resources' },
  { name: 'has_status', signature: 'host.has_status(id)', group: '只读', desc: '是否带有某状态', snippet: "host.has_status('burn')", argContext: 'statuses' },
  { name: 'relationship', signature: 'host.relationship(from, to, type)', group: '只读', desc: '关系强度（有向边）', snippet: "host.relationship('from', 'to', '好感')", argContext: 'characters' },
  { name: 'engine_rng', signature: 'host.engine_rng(min, max)', group: '只读', desc: '确定性随机（引擎序列，可重放）', snippet: 'host.engine_rng(1, 20)' },
  { name: 'request_cost', signature: 'host.request_cost(id, amount)', group: '可写', desc: '声明资源消耗（引擎校验后扣减）', snippet: "host.request_cost('res-gold', 1)", argContext: 'resources' },
  { name: 'apply_status', signature: 'host.apply_status(target, id, duration, unit)', group: '可写', desc: '施加持续状态', snippet: "host.apply_status('target', 'burn', 3, 'turns')", argContext: 'characters' },
  { name: 'remove_status', signature: 'host.remove_status(target, id)', group: '可写', desc: '移除状态', snippet: "host.remove_status('target', 'burn')", argContext: 'characters' },
  { name: 'trigger_event', signature: 'host.trigger_event(event)', group: '可写', desc: '触发引擎事件', snippet: "host.trigger_event('scene')", argContext: 'events' },
  { name: 'query_world', signature: 'host.query_world(query)', group: '可写', desc: '查询世界（返回过滤后的实体）', snippet: "host.query_world('...')" },
  { name: 'scene_id', signature: 'host.scene_id', group: '环境', desc: '当前场景 id', snippet: 'host.scene_id' },
  { name: 'round', signature: 'host.round', group: '环境', desc: '当前回合数', snippet: 'host.round' },
  { name: 'difficulty', signature: 'host.difficulty', group: '环境', desc: '本次判定难度', snippet: 'host.difficulty' },
  { name: 'mount', signature: 'host.mount', group: '环境', desc: '当前挂载点', snippet: 'host.mount' },
  { name: 'actor', signature: 'host.actor', group: '环境', desc: '当前角色（id / name / kind / attributes / resources / statuses）', snippet: 'host.actor' },
  { name: 'target', signature: 'host.target', group: '环境', desc: '当前目标（id / name / kind）', snippet: 'host.target' },
  { name: 'definition', signature: 'host.definition', group: '环境', desc: '当前技能 / 物品定义', snippet: 'host.definition' },
  { name: 'storage', signature: 'host.storage', group: '环境', desc: 'per-script 持久存储（跨回合）', snippet: 'host.storage' },
  { name: 'log', signature: 'host.log(msg)', group: '环境', desc: '调试输出（不产生副作用）', snippet: "host.log('...')" },
]

export interface LuaLintViolation {
  term: string
  tip: string
}

export const LUA_FORBIDDEN_RULES: LuaLintViolation[] = [
  { term: 'math.random', tip: '需替换为确定性随机源 host.engine_rng(min, max)' },
  { term: 'io.', tip: '沙箱禁止系统 IO 操作' },
  { term: 'os.', tip: '沙箱禁止操作系统级调用' },
  { term: 'package.', tip: '沙箱禁止动态模块包管理' },
  { term: 'require', tip: '沙箱禁止动态加载外部库' },
  { term: 'debug.', tip: '沙箱禁止调试反射 API' },
  { term: 'collectgarbage', tip: '沙箱托管垃圾回收' },
  { term: 'coroutine.', tip: '沙箱禁止未受控协程' },
  { term: 'dofile', tip: '沙箱禁止加载外部文件' },
  { term: 'loadfile', tip: '沙箱禁止加载外部文件' },
]

export function lintLuaSource(source: string | null | undefined): LuaLintViolation[] {
  if (!source) return []
  const hits: LuaLintViolation[] = []
  for (const rule of LUA_FORBIDDEN_RULES) {
    if (source.includes(rule.term)) {
      hits.push(rule)
    }
  }
  return hits
}
