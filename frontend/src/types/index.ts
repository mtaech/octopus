// ============================================================
// Octopus 前端契约类型（蓝图 #01/#12/#13/#04/#17/#23/#24 的 TS 镜像）
// 唯一数据形状来源：列表 / 编辑器 / 游玩页三页只 import 这里与 api。
// ============================================================

// ---------- 故事书静态模板（#01，编辑/校验/存档内嵌冻结） ----------

export type DimensionType = 'number' | 'enum' | 'text'

/** 属性维度：故事书全局定义，人物填值（#01 ④ / #07 ④） */
export interface AttributeDimension {
  key: string
  label: string
  type: DimensionType
  /** type=number 的可用范围 */
  min?: number
  max?: number
  /** type=enum 的候选标签 */
  options?: string[]
  /** type=number 数值型默认 50 为基线（#12 中心偏移公式） */
  baseline?: number
}

export interface LocationDef {
  id: string
  name: string
  description?: string
  parent_id?: string
}

export interface ResourceDef {
  id: string
  name: string
  type: 'numerical' | 'binary'
  default_max?: number
  /** #12 natural_recovery: amount + trigger */
  natural_recovery?: { amount: number; trigger: 'per_turn' | 'per_scene' | 'per_rest' }
}

export type CheckMode = 'gte' | 'lte' | 'opposed'

/** 判定器声明（#12）：参数化配置，引擎不预设骰系 */
export interface CheckerDef {
  /** 骰子表达式 "1d20" / "1d100" / "3d6"，null = 无骰（AI 依属性叙事裁决） */
  dice?: string | null
  mode?: CheckMode
  /** 属性修正映射：键为维度 key */
  attribute_modifier?: Record<string, number>
  /** 中心偏移公式默认 (v-50)/5 向下取整 */
  modifier_formula?: string
  /** 成功度分档阈值（按差值），默认 [+10, 0, -10] → 大成功/成功/勉强/失败 */
  degree_thresholds?: number[]
}

export type CondExpr =
  | { op: 'all_of'; children: CondExpr[] }
  | { op: 'any_of'; children: CondExpr[] }
  | { op: 'not'; child: CondExpr }
  | { op: 'beat_fired'; beat_id: string }
  | { op: 'flag_set'; flag: string }
  | { op: 'at_location'; location_id: string }
  | { op: 'attribute_ge'; attribute: string; value: number }
  | { op: 'relationship_ge'; from: string; to: string; type: string; value: number }
  | { op: 'lua'; script: string }

/** 目标 goal（#13 ①）：场景/章节完成判据 */
export interface GoalDef {
  id: string
  text: string
  primary?: boolean
  hidden?: boolean
  condition?: CondExpr | null
}

/** 节拍 beat（#13 ②）：关键事件节点，不改世界状态 */
export interface BeatDef {
  id: string
  title: string
  description?: string
  condition?: CondExpr | null
  /** 触发后给主线 AI 的提示 */
  hint: string
  repeatable?: boolean
}

export interface SceneDef {
  id: string
  title: string
  description?: string
  location_id?: string
  /** 进入场景的在场人物 template id 集合 */
  present_char_ids?: string[]
  goals: GoalDef[]
  beats: BeatDef[]
}

export interface ChapterDef {
  id: string
  title: string
  description?: string
  scenes: SceneDef[]
}

/** 人物模板（#01 characters）—— 运行时实例化后成为角色实例 */
export interface CharacterDef {
  id: string
  name: string
  kind?: 'pc' | 'npc'
  background: string
  personality: string
  /** 属性值：key = 全局属性维度 key */
  attributes: Record<string, number | string>
  /** 初始资源 */
  resources?: Record<string, number>
  appearance?: string
}

export type ImmediateEffect =
  | { kind: 'damage'; amount: string }
  | { kind: 'heal'; amount: string }
  | { kind: 'modify_resource'; resource: string; amount: string }
  | { kind: 'set_flag'; flag: string }

export interface EffectDef {
  immediate?: ImmediateEffect[]
  /** 持续效果：duration 单位 turns|scenes，stack = replace|add|max */
  status?: { id: string; name: string; duration: number; unit: 'turns' | 'scenes'; stack?: 'replace' | 'add' | 'max'; effect?: ImmediateEffect[] }[]
  /** 条件触发：event + condition + effects */
  triggers?: { id: string; event: string; condition?: CondExpr | null; effects: ImmediateEffect[] }[]
  /** 静态属性修正：同名取 max */
  modifiers?: { attribute: string; value: number }[]
}

export interface SkillDef {
  id: string
  name: string
  description: string
  category?: string
  cost?: { resource: string; amount: number }[]
  cooldown?: { turns: number } | null
  target?: string
  /** 技能级判定器：引用全局或用技能自声明（#12 双作用域） */
  check?: string | CheckerDef
  effect?: EffectDef
  /** Lua 钩子源码（#02：声明式核心 + Lua 兜底） */
  lua?: string
}

export interface ItemDef {
  id: string
  name: string
  description: string
  type?: string
  quantity?: number
  /** 引用技能 id → 物品技能（#01 技能单一实体） */
  skills?: string[]
  properties?: Record<string, string>
}

/** 物件（#01 修订）：场景内可交互物件，可挂技能与条件 */
export interface ObjectDef {
  id: string
  name: string
  description?: string
  location_id?: string
  /** 可交互动作：key 为引擎动作标识，label 为展示名 */
  actions?: { key: string; label: string }[]
  /** 可挂技能 id */
  skills?: string[]
  condition?: CondExpr | null
}

/** 声明区条目（#01 修订）：flag / 事件 / 关系类型 / 目标类型，供引用校验 */
export interface DeclarationDef { key: string; label?: string }

export interface FactionDef {
  id: string
  name: string
  description: string
  goals?: string[]
  default_attitude?: number
}

/** 关系 = 有向边（#01）：可连 人物/势力 两两之间 */
export interface RelationshipDef {
  id: string
  from_kind: 'character' | 'faction'
  from_id: string
  to_kind: 'character' | 'faction'
  to_id: string
  type: string
  value: number
}

export interface StorybookWorld {
  premise: string
  locations: LocationDef[]
  resources: ResourceDef[]
  /** 全局判定器（#12 world.rules.check 落在世界区） */
  check?: CheckerDef | null
}

/** 故事书 = 模板唯一事实来源（#01） */
export interface Storybook {
  schema_version: number
  meta: {
    id: string
    title: string
    author?: string
    language?: string
    description?: string
    /** 创建/更新时间戳 */
    created_at?: string
    updated_at?: string
  }
  world: StorybookWorld
  /** 属性维度全局定义（人物只消费，不内联） */
  attribute_dimensions: AttributeDimension[]
  skeleton: ChapterDef[]
  characters: CharacterDef[]
  skills: SkillDef[]
  items: ItemDef[]
  objects: ObjectDef[]
  factions: FactionDef[]
  relationships: RelationshipDef[]
  /** 声明区（#01 修订）：引用校验用，Lua 兜底可绕过 */
  flags: DeclarationDef[]
  events: DeclarationDef[]
  relationship_types: DeclarationDef[]
  target_types: DeclarationDef[]
}

// ---------- 编辑器（#23 单资源两态 + #01/#13 实体） ----------

export interface StorybookListItem {
  id: string
  title: string
  revision: number
  draft_version: number
  updated_at: string
  released_at?: string | null
  /** released 非空才算已发布（游玩侧可用） */
  published: boolean
  /** 故事书简介（#01 meta.description），列表卡可展示摘要 */
  description?: string
}

/** GET /api/storybooks/:id —— 草稿 + 发布态元信息 */
export interface StorybookDocument {
  id: string
  revision: number
  draft_version: number
  updated_at: string
  released_at?: string | null
  published: boolean
  /** 编辑中工作副本（唯一主人：editor store，#22） */
  draft: Storybook
  /** 当前已发布版次（冻结副本），未发布为 null */
  released: Storybook | null
}

export type IssueSeverity = 'error' | 'warning'

export interface ValidationIssue {
  severity: IssueSeverity
  code: string
  /** 实体 kind + id 或 JSON 路径 */
  target?: string
  message: string
  /** 被引用清单（引用图权威侧兜底，#23 ②） */
  related_refs?: { kind: string; id: string; label: string }[]
}

export interface ValidateResult {
  issues: ValidationIssue[]
}

export interface ApiErrorBody {
  code: string
  message: string
  detail?: Record<string, unknown>
}

/** 结对建议（#23 ④ / #22 pair store）：采纳 → editor store action 落稿 */
export interface PairSuggestion {
  id: string
  action: 'create' | 'update' | 'delete'
  /** 实体 kind：character / skill / item / faction / location / goal / beat / chapter / dimension / relationship */
  target: { kind: string; id?: string }
  /** 与实体形状同构的 patch */
  patch: Record<string, unknown>
  label: string
  summary: string
}

export type PairStreamEvent =
  | { type: 'text_delta'; content: string }
  | { type: 'suggestion'; suggestion: PairSuggestion }
  | { type: 'done' }
  | { type: 'error'; message: string }

// ---------- 存档（#24 / #21 / #14） ----------

export interface SaveListItem {
  id: string
  title: string
  storybook_id: string
  storybook_title: string
  embedded_revision: number
  latest_revision: number
  needs_upgrade: boolean
  /** 导入存档标记（#21 ④） */
  imported?: boolean
  created_at: string
  updated_at: string
  last_played_at: string
}

/** 存档 = 单文件自包含：内嵌冻结故事书 + 命令日志 + 快照（#06/#14） */
export interface SaveDetail extends SaveListItem {
  /** 内嵌冻结模板 */
  storybook: Storybook
}

export type Disposition = 'freeze' | 'departure'

export interface UpgradeCharacterItem {
  character_id: string
  name: string
  disposition: Disposition | null
}

export interface UpgradeReportGroup {
  /** 引用变更（技能/物品/势力/地点消失等，自动处理） */
  changes: { kind: string; id: string; label: string; action: string }[]
  /** 人物消失（需逐项裁决） */
  gone_characters: { character_id: string; name: string; reason?: string }[]
}

export interface UpgradeReport {
  from_revision: number
  to_revision: number
  groups: UpgradeReportGroup
}

// ---------- 运行时世界状态（#06 投影 / #17 GET /state 水合） ----------

export type PhaseStage = 'story_thinking' | 'character_thinking' | 'resolving' | 'waiting_confirm' | 'idle'

/** 运行时角色实例（#06 ①：模板 + 当前覆盖） */
export interface CharacterInstance {
  instance_id: string
  template_id: string
  name: string
  kind: 'pc' | 'npc'
  attributes: Record<string, number | string>
  resources: Record<string, number>
  location_id?: string
  present: boolean
  statuses: { id: string; name: string; turns_left?: number; scenes_left?: number }[]
}

/** 骨架进度：goal 达成 / beat 触发（#13 引擎回合末求值产物） */
export interface SkeletonProgress {
  goals: Record<string, boolean>
  beats: Record<string, boolean>
  /** 未完成即离开的 scene */
  abandoned_scenes: string[]
}

export interface WorldProjection {
  seq: number
  scene_id: string
  scene_title: string
  characters: Record<string, CharacterInstance>
  /** 控制中的角色实例 */
  controlled: string[]
  flags: Record<string, boolean>
  progress: SkeletonProgress
  locations: LocationDef[]
  meta: {
    save_id: string
    save_title: string
    storybook_title: string
    revision: number
    needs_upgrade: boolean
    auto_confirm: boolean
  }
}

// ---------- 演出流事件（#17 包络 + 类型表，零呈现字段） ----------

export interface EventEnvelopeBase {
  id: string
  /** 单调递增，重连水合丢 seq ≤ watermark */
  seq: number
  round: number
  ts: string
  /** 叙事事件语义归属（渲染器据此署名） */
  actor?: { id: string; name: string } | null
  /** 机制事件关联回动作协议意图 */
  intent_id?: string | null
}

export interface PendingPayload {
  action_id: string
  intent_id?: string
  actor: { id: string; name: string }
  description: string
  impact?: string
  timeout_ms: number
}

export type SuccessLevel = 'great' | 'success' | 'barely' | 'fail'

export interface CheckResultPayload {
  intent_id?: string
  actor: { id: string; name: string }
  attribute: string
  expr?: string
  rolls?: number[]
  mod: number
  total: number
  target: number
  /** total − target */
  margin: number
  result: boolean
  level: SuccessLevel
  /** mode=opposed 时的对手 */
  opponent?: { id: string; name: string } | null
}

export interface StateDelta {
  domain: 'character' | 'goal' | 'beat' | 'location' | 'relationship' | 'resource' | 'flag'
  entity_id: string
  field: string
  op: 'set' | 'add' | 'remove'
  value: unknown
}

export interface ResolutionPayload {
  intent_id?: string
  status: 'ok' | 'rejected'
  rejection_code?: string
  narrative?: string
  outcome?: string
  triggered_events?: string[]
  state_changes: StateDelta[]
}

export type PlayEvent =
  | (EventEnvelopeBase & { type: 'scene'; payload: { scene_id: string; title: string; description?: string; present: string[] } })
  | (EventEnvelopeBase & { type: 'narrate'; payload: { content: string; scene_ref?: string } })
  | (EventEnvelopeBase & { type: 'dialogue'; payload: { content: string; audience?: string[] } })
  | (EventEnvelopeBase & { type: 'emote'; payload: { content: string; emotion?: string; gesture?: string } })
  | (EventEnvelopeBase & { type: 'pending'; payload: PendingPayload })
  | (EventEnvelopeBase & { type: 'check_result'; payload: CheckResultPayload })
  | (EventEnvelopeBase & { type: 'resolution'; payload: ResolutionPayload })
  | (EventEnvelopeBase & { type: 'state_update'; payload: { changes: StateDelta[] } })
  | (EventEnvelopeBase & { type: 'phase'; payload: { stage: Exclude<PhaseStage, 'idle'>; detail?: string } })
  | (EventEnvelopeBase & { type: 'round_start'; payload: { input: { channel: 'character' | 'meta'; text: string } } })
  | (EventEnvelopeBase & { type: 'round_end'; payload: { round: number } })
  | (EventEnvelopeBase & { type: 'system'; payload: { level: 'info' | 'warn' | 'error'; code?: string; text: string } })

export type StreamStatus = 'connecting' | 'open' | 'closed' | 'error'

/** 叙事历史分页（#17/#24 修订）：直接复用演出流事件形状 */
export interface HistoryPage {
  /** 升序，最旧在前 */
  events: PlayEvent[]
  hasMore: boolean
}

/** 存档级设置（#24 修订：免确认） */
export interface SaveSettings { auto_confirm: boolean }

/** 维护历史（#21 / #14 修订） */
export interface MaintenanceRow { at: string; op: string; summary: string }

// ---------- 通用工具类型 ----------

export interface Listener<T> {
  onEvent: (evt: T) => void
  onStatus?: (status: StreamStatus) => void
}

export function uid(prefix = 'id'): string {
  return prefix + '-' + Math.random().toString(36).slice(2, 8) + Date.now().toString(36).slice(-4)
}
