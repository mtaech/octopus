// ============================================================
// Octopus 前端契约类型（蓝图 #01/#12/#13/#04/#17/#23/#24 的 TS 镜像）
// 唯一数据形状来源：列表 / 编辑器 / 游玩页三页只 import 这里与 api。
// ============================================================
import type { EntityRef } from './generated/EntityRef'

// ---------- 资产库（#28） ----------

/**
 * 图片资产引用：图片按内容寻址存于文件系统（文件名 = <sha256>.<ext>），
 * JSON 里只留名字 + 尺寸；发布/存档冻结无需拷贝，导出时才打进 zip。
 */
export interface AssetRef {
  /** 资产库文件名：<sha256>.<ext>，内容即身份（不可变、天然去重） */
  asset: string
  /** 像素宽高：供前端预留版面，避免加载抖动 */
  w: number
  h: number
  /** 无障碍替代文本 */
  alt?: string
}

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
  /** 修正步长（#2）：modifier = floor((clamp(值) - 基线) / 步长)，默认 5；D&D 用 2 */
  modifier_step?: number
}

export interface LocationDef {
  id: string
  name: string
  description?: string
  parent_id?: string
  /** 地点插图（#28） */
  image?: AssetRef
}

export interface ResourceDef {
  id: string
  name: string
  type: 'numerical' | 'binary'
  default_max?: number
  /** 层阶（#4）：法术位按环分组展示；普通资源可不填 */
  tier?: number
  /** #4 恢复时机：回合 / 场景 / 短休 / 长休（per_rest 视为长休别名） */
  natural_recovery?: { amount: number; trigger: 'per_turn' | 'per_scene' | 'per_short_rest' | 'per_long_rest' | 'per_rest' }
}

export type CheckMode = 'gte' | 'lte' | 'opposed'

/** 判定种类（#3）：谁掷骰 / 是否掷骰 / 比较对象。 */
export type CheckKind = 'attribute' | 'attack' | 'save' | 'passive'

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
  /** Lua 判定脚本（归一化输出 total / margin，由引擎分档；#12） */
  lua?: string
  /** 判定种类（#3）：缺省 attribute */
  kind?: CheckKind
  /** 被动判定基数（缺省 10；D&D 被动察觉 = 10 + 加值） */
  passive_base?: number
}

export type CondExpr =
  | { op: 'all_of'; children: CondExpr[] }
  | { op: 'any_of'; children: CondExpr[] }
  | { op: 'not'; child: CondExpr }
  | { op: 'trigger_fired'; trigger_id: string }
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

/** 剧情触发点 trigger（#13 ②）：条件触发的剧情节点，触发后提示 AI 演绎，不改世界状态 */
export interface TriggerDef {
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
  triggers: TriggerDef[]
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
  /** 掌握技能 id → 角色技能（#01 技能单一实体） */
  skills?: string[]
  /** 初始物品栏（#01 物品栏）：item id + 数量，缺省 1 */
  inventory?: { id: string; quantity?: number }[]
  /** 已装备物品 id（#5：必须在本人物品栏内；按 slot 唯一） */
  equipped?: string[]
  appearance?: string
  /** 对话示例：3-5 轮示范该角色口吻的对话——AI 模仿语气与句式最有效的手段（few-shot） */
  example_dialogues?: string
  /** 作者注释：给创作者的备注（灵感、使用建议、已知问题），永远不会发给 AI */
  notes?: string
  /** 人物立绘（#28）：游玩页角色卡与对话气泡使用 */
  portrait?: AssetRef
  /** 每日准备（#4）：从已掌握技能里挑选的当日法术（D&D prepared） */
  prepared?: string[]
  /** 开放种类挂接：kind key → Definition id 列表（如 { race: ['race-human'], feature: [...] }）。
   *  这是把自定义种类接到人物上的唯一通道——没有它，开放定义就只能待在扩展里。 */
  attachments?: Record<string, string[]>
}

export type ImmediateEffect =
  | { kind: 'damage'; amount: string; resource?: string }
  | { kind: 'heal'; amount: string; resource?: string }
  | { kind: 'modify_resource'; resource: string; amount: string }
  | { kind: 'set_flag'; flag: string }

/** 持续状态（#12 ③）：故事书顶层声明，技能与 Lua 按 id 引用 */
export interface StatusDef {
  id: string
  name: string
  description?: string
  /** 持续时长（单位见 unit） */
  duration: number
  unit: 'turns' | 'scenes'
  /** 同名叠加策略：replace 替换 / add 层数相加 / max 取较大 */
  stack?: 'replace' | 'add' | 'max'
  /** 每 tick 施加的即时效果 */
  effect?: ImmediateEffect[]
}

export interface EffectDef {
  immediate?: ImmediateEffect[]
  /** 施加的持续状态：引用故事书顶层 statuses 的 id */
  status?: string[]
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
  /** 装备位（#5）：非空表示可装备；同一角色同一位只允许一件 */
  slot?: string
  /** 装备后贡献的修正值（#5）：走与挂接定义同一套 Modifier 叠加 */
  modifiers?: Modifier[]
  /** 价格（故事书基准货币，可选） */
  price?: number
  /** 物品图标（#28） */
  icon?: AssetRef
  /** 武器伤害骰（如 1d6）：玩家点名这件武器但没挂技能时，引擎按它结算 */
  damage?: string
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
  /** 主体 id（规范字段，决策 #11 对齐 #01 的 from/to） */
  from: string
  to_kind: 'character' | 'faction'
  /** 客体 id（规范字段） */
  to: string
  type: string
  value: number
}

/**
 * 读取侧兼容（决策 #11）：旧故事书的关系边写作 from_id / to_id，规范字段是
 * from / to。编辑器载入草稿时归一为规范字段，之后只写规范字段；旧数据原位读取
 * 兜底，绝不静默丢引用。原地改动 draft（与其它编辑操作一致）。
 */
export function normalizeRelationshipFields(d: Storybook): void {
  for (const rel of d.relationships ?? []) {
    const legacy = rel as RelationshipDef & { from_id?: string; to_id?: string }
    if (legacy.from == null && legacy.from_id != null) legacy.from = legacy.from_id
    if (legacy.to == null && legacy.to_id != null) legacy.to = legacy.to_id
    delete legacy.from_id
    delete legacy.to_id
  }
}

/** 读取关系边端点：规范字段 from/to，兼容旧 from_id/to_id（决策 #11），无值返回空串。 */
export function relationshipEnd(rel: RelationshipDef, end: 'from' | 'to'): string {
  const legacy = rel as RelationshipDef & { from_id?: string; to_id?: string }
  const value = end === 'from' ? rel.from ?? legacy.from_id : rel.to ?? legacy.to_id
  return value ?? ''
}

// ---------- 开放内容元模型（元模型探针） ----------
//
// 主张：故事内容是开放词汇（任一题材的概念都可能出现），引擎机制是封闭语义
// （只有条件 / 效果 / 判定 / 资源 / 标记这些原语）。于是「新概念」应该只是数据，
// 而不是新类型：新增一个 KindDef + 若干 Definition，不应改动编辑器 / 校验 / AI 上下文。
//
// 词汇开放、语义封闭：KindDef 只能声明「字段」（词汇），不能声明新的求值规则（语义）；
// 真正的新机制走既有原语或 Lua 兜底。

/** 开放种类字段的类型（词汇层，引擎不认识具体 kind，只认识这些字段类型）。 */
export type FieldType = 'text' | 'textarea' | 'number' | 'boolean' | 'enum' | 'ref' | 'ref_list' | 'asset'

/** 通用字段 schema：编辑器按它渲染表单，校验按它做存在性 / 引用检查。 */
export interface FieldDef {
  key: string
  label: string
  type: FieldType
  required?: boolean
  /** type=enum 的候选值 */
  options?: string[]
  /** type=ref / ref_list 的目标 kind（缺省 = 不限） */
  ref_kind?: string
  placeholder?: string
  hint?: string
}

/** 开放内容种类声明：由故事书数据自带，代码不枚举。 */
export interface KindDef {
  key: string
  label: string
  group?: string
  /** 单例种类（如 meta / world）：同一故事书只允许一条 */
  singleton?: boolean
  /** 可挂接到哪些实体（如 ['character']）：声明后，该实体的编辑面板会出现挂接区 */
  applies_to?: string[]
  fields: FieldDef[]
}

/** 修正值（来源：挂接定义 / 装备 / 状态）。target = 属性维度 key 或派生值 key。 */
export interface Modifier {
  target: string
  value: number
  /** add 累加（默认）/ max 取高 / set 覆盖 */
  op?: 'add' | 'max' | 'set'
}

/** 派生值声明：由公式从「属性维度 + earlier 派生值」算出（如调整值 / 豁免 / AC / 法术DC）。 */
export interface DerivedDef {
  key: string
  label: string
  /** 公式：数值 / 变量（属性维度 key 或前面已声明的派生 key）/ + - * / ( ) / floor ceil round min max abs */
  formula: string
  group?: string
  /** 展示时带正号（调整值类） */
  signed?: boolean
}

/** 卡面分区（#6）：按声明顺序组织角色卡内容；未声明时面板走内置布局。 */
export interface SheetSection {
  title: string
  /** 挂接种类 key：展示该种类下已挂接的定义（名称 + 字段） */
  kinds?: string[]
  /** 派生值 key：展示标签 + 数值 */
  derived?: string[]
  /** 资源 id：展示 当前 / 上限 */
  resources?: string[]
  /** 叙事字段：background / personality / appearance */
  narrative?: string[]
}

/** 引擎结算用的机制（与 kind 无关，duck-typed：填了就按它结算）。 */
export interface DefinitionMechanics {
  check?: string | CheckerDef | null
  cost?: { resource: string; amount: number }[]
  cooldown?: { turns: number } | null
  target?: string
  effect?: EffectDef
}

/** 开放内容实例：kind 指向 kinds[].key；fields 是字段值袋（由 KindDef.fields 解释）。 */
export interface Definition {
  id: string
  kind: string
  name: string
  description?: string
  fields?: Record<string, unknown>
  mechanics?: DefinitionMechanics
  /** 挂接后对宿主贡献的修正值（种族 +1、装备 +2 AC 等） */
  modifiers?: Modifier[]
}

export interface StorybookWorld {
  premise: string
  /** 故事开头（#29）：开档后玩家读到的第一段叙事，先于世界前提呈现 */
  opening?: string
  locations: LocationDef[]
  resources: ResourceDef[]
  /** 全局判定器（#12 world.rules.check 落在世界区） */
  check?: CheckerDef | null
}

/** 故事书 = 模板唯一事实来源（#01） */
/** 世界词条（关键词触发注入）：命中 keys 才把 content 交给 AI，省上下文。
 *  与「剧情触发点」不同——它只做条件性上下文注入，不改世界状态。 */
export interface LoreDef {
  id: string
  title: string
  /** 注入给 AI 的核心事实：3-5 句为宜（呼应「少即是多」） */
  content: string
  /** 触发关键词（中/英/简称/昵称等多种变体）；constant 为 true 时可留空 */
  keys?: string[]
  /** 越大越靠前注入（核心世界观靠前） */
  priority?: number
  /** 常驻：无需关键词，每回合都注入 */
  constant?: boolean
  /** 递归扫描：命中后其内容里再出现其它词条的 key 时继续命中 */
  recursive?: boolean
  /** 停用（默认启用） */
  enabled?: boolean
}

/** 叙述段的变体选项（P1）：互斥文本，引擎按「存档选择 → defaultVariant」解析出唯一文本 */
export interface NarrativeVariant {
  key: string
  label: string
  text: string
}

/** 叙述段（叙事契约）：故事书声明的「怎么讲」——按槽位注入、按 scope 筛选 */
export interface NarrativeSection {
  id: string
  title: string
  /** 槽位：world | style | behavior | closing（引擎固定顺序） */
  slot: 'world' | 'style' | 'behavior' | 'closing'
  /** 作用范围：主线 / 角色 / 两者 / 指定人物 */
  scope: 'story' | 'character' | 'both' | { characterId: string }
  /** 常驻文本；与 variants 互斥（P1） */
  text?: string
  /** 变体组（P1）：互斥选项；声明后必须给 defaultVariant，且与 text 互斥 */
  variants?: NarrativeVariant[]
  /** 默认变体 key（P1）：玩家未选择时使用；必须命中某个变体 key */
  defaultVariant?: string
  /** 注入条件（P1）：世界状态满足时才注入该段；缺省 = 每回合都注入 */
  when?: CondExpr | null
  /** 玩家可否在游玩页开关 / 选择变体（P1）；缺省 false = 只有作者能改 */
  playerEditable?: boolean
  /** 是否启用（缺省 true） */
  enabled?: boolean
}

/** 输出协议模式（叙事契约 P2）：default = 引擎内置；declarative = 白名单渲染；lua = 协议插件 */
export type ProtocolMode = 'default' | 'declarative' | 'lua'

/** 输出协议（P2）：故事书覆盖 AI 的输出格式说明与解析，出口仍是引擎意图数组。 */
export interface ProtocolConfig {
  mode: ProtocolMode
  /** declarative：声明使用哪些意图（须是引擎已知意图集合的子集） */
  intents?: string[]
  /** 协议补充说明；计入每回合 token 预算，可被裁剪 */
  instructions?: string
  /** lua：协议插件源码（protocol.preamble / protocol.parse / 可选 normalize） */
  lua?: string
}

/** 叙事契约：故事书里声明的提示词段（P0 段 + P1 条件/变体/玩家偏好 + P2 协议 + P3 思考展示策略） */
export interface NarrativeContract {
  sections: NarrativeSection[]
  /** 输出协议（P2）；缺省 = 引擎默认协议 */
  protocol?: ProtocolConfig
  /** 思考草稿展示策略（P3）：控制模型用 think 意图写在正文里的思考怎么显示 */
  display?: NarrativeDisplay
}

/** 思考草稿展示策略（P3）：folded = 折叠展示（默认）；hidden = 不渲染 */
export interface NarrativeDisplay {
  draft?: 'folded' | 'hidden'
}

export interface Storybook {
  schema_version: number
  meta: {
    id: string
    title: string
    author?: string
    language?: string
    description?: string
    /** 故事书封面（#28）：缺省时列表页/编辑器回落为 CSS 生成封面 */
    cover?: AssetRef
    /** 内容评级（P3）：创作者自报，仅列表徽标用；不做过滤、不做年龄验证，引擎不读 */
    rating?: 'sfw' | 'nsfw'
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
  /** 持续状态全局声明（#12 ③）：技能 effect.status 与 Lua apply_status 引用其 id */
  statuses: StatusDef[]
  /** 世界词条（关键词触发注入）：命中才把内容交给 AI，控制上下文膨胀 */
  lore?: LoreDef[]
  /** 叙事契约（怎么讲）：按槽位注入的叙述段 */
  narrative?: NarrativeContract
  /** 开放内容种类注册表（元模型探针）：新概念 = 一条数据，不是新类型 */
  kinds?: KindDef[]
  /** 开放内容实例：kind 指向 kinds[].key；fields 为字段值袋 */
  definitions?: Definition[]
  /** 派生值声明（#2）：从属性维度与修正来源算出的二级数值 */
  derived?: DerivedDef[]
  /** 卡面分区（#6）：角色卡的展示结构（分区 + 每区内容） */
  sheet?: SheetSection[]
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
  /** 故事书封面（#28）：列表接口从已发布版次（否则草稿）的 meta 里取；缺省为 null */
  cover?: AssetRef | null
  /** 内容评级（P3，meta.rating）：列表卡徽标用；缺省 sfw */
  rating?: 'sfw' | 'nsfw' | null
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
  /**
   * 实体 kind：meta / world / character / location / resource / dimension /
   * skill / item / object / faction / chapter / scene / goal / trigger / relationship /
   * flag / event / relationship_type / target_type
   * - id：update / delete 的目标（dimension 与声明类用 key）
   * - parent_id：嵌套实体新建必填（scene = 章节 id；goal / trigger = 场景 id）
   */
  target: { kind: string; id?: string; parent_id?: string }
  /** 与实体形状同构的 patch */
  patch: Record<string, unknown>
  label: string
  summary: string
  /** 卡片上的可读字段列表（工具改动生成；**永不展示原始 JSON**） */
  details?: { label: string; value: string }[]
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
  /** 沙箱试玩标记（草稿态试玩） */
  is_sandbox?: boolean
  created_at: string
  updated_at: string
  last_played_at: string
}

/** 存档 = 单文件自包含：内嵌冻结故事书 + 命令日志 + 快照（#06/#14） */
export interface SaveDetail extends SaveListItem {
  /** 内嵌冻结模板 */
  storybook: Storybook
  /** 存档遗留区（#14）：版次升级时被删除、旧定义仍可解释的实体；无遗留时不返回 */
  legacy?: SaveLegacy[]
}

/** 遗留区条目（#14）：旧故事书里已删除的实体定义 */
export interface SaveLegacy {
  kind: string
  id: string
  name: string
  definition: unknown
  frozen_at_revision: number
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
  /** 物品栏（#01）：item id → 数量 */
  inventory: Record<string, number>
  location_id?: string
  present: boolean
  statuses: { id: string; name: string; turns_left?: number; scenes_left?: number }[]
}

/** 骨架进度：goal 达成 / trigger 触发（#13 引擎回合末求值产物） */
export interface SkeletonProgress {
  goals: Record<string, boolean>
  triggers: Record<string, boolean>
  /** 未完成即离开的 scene */
  abandoned_scenes: string[]
}

export interface EnemyView {
  /** 遭遇内寻址 id（e1、e2…），攻击时用它精确指定目标 */
  id: string
  name: string
  hp: number
  max: number
  /** 防御值：攻击判定的难度就是它 */
  ac: number
}

export interface EncounterView {
  id: string
  name: string
  enemies: EnemyView[]
  note?: string | null
  active: boolean
}

export interface QuestView {
  id: string
  text: string
  done: boolean
  /** skeleton = 故事书骨架；gm = 导演运行时新增 */
  source: 'skeleton' | 'gm' | string
  hidden: boolean
  primary: boolean
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
  /** 运行时任务（骨架目标 + 导演新增），来源见 source */
  quests?: QuestView[]
  /** 结构化遭遇（导演创建） */
  encounters?: EncounterView[]
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
  /** 判定种类（#3）：被动 / 豁免 / 攻击 / 属性 */
  kind?: CheckKind
}

export interface StateDelta {
  domain: 'character' | 'goal' | 'trigger' | 'location' | 'relationship' | 'resource' | 'flag' | 'origin'
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
  | (EventEnvelopeBase & { type: 'round_start'; payload: { input: { channel: 'character' | 'meta' | 'gm'; text: string; refs?: EntityRef[] } } })
  | (EventEnvelopeBase & { type: 'round_end'; payload: { round: number } })
  | (EventEnvelopeBase & { type: 'system'; payload: { level: 'info' | 'warn' | 'error'; code?: string; text: string } })
  | (EventEnvelopeBase & { type: 'reasoning'; payload: { stage: string; text: string; /** P3：provider = 供应商 reasoning_content；model = think 意图。旧事件缺省为 provider */ source?: 'provider' | 'model' } })

export type StreamStatus = 'connecting' | 'open' | 'closed' | 'error'

/** 叙事历史分页（#17/#24 修订）：直接复用演出流事件形状 */
export interface HistoryPage {
  /** 升序，最旧在前 */
  events: PlayEvent[]
  hasMore: boolean
}

/** 存档级设置（#24 修订：免确认） */
/** 单存档模型选择（单一 AI）：供应商 + 模型 + 思考强度（undefined = 供应商默认）。 */
export interface SaveModelChoice {
  provider_id: string
  model: string
  reasoning_effort?: string
}

export interface SaveSettings {
  auto_confirm: boolean
  /** 本存档使用的模型；缺省 = 用全局角色默认 */
  model_provider_id?: string
  model?: string
  /** 本存档的思考强度（reasoning_effort）；缺省 = 用角色 / 供应商默认 */
  reasoning_effort?: string
  /** 叙述段玩家偏好：section id → 开关(bool) 或变体 key(string)；只影响之后的回合，不回写历史 */
  narrative?: Record<string, boolean | string>
}

/** 维护历史（#21 / #14 修订） */
export interface MaintenanceRow { at: string; op: string; summary: string }

// ---------- 应用配置 / AI Provider（#26） ----------

// 类型 = 协议方言（不是厂商）；DeepSeek/Moonshot/Groq 等走 openai-compatible
export type ProviderKind = 'openai' | 'anthropic' | 'ollama' | 'openai-compatible' | 'local-embedding'

export interface ModelEntry {
  id: string
  name?: string
  /** 用户自定义的模型元数据（小中转站等目录未收录的模型）：覆盖目录快照 */
  ctx?: number
  maxOut?: number
  reasoning?: boolean
  /** 思考等级 → 实际下发值（null = 不发送该参数） */
  tl?: Record<string, string | null>
}

export interface ProviderConfig {
  id: string
  label: string
  kind: ProviderKind
  base_url?: string
  /** 真实后端存本地配置文件（0600），日志脱敏；此处仅原型展示 */
  api_key?: string
  /** 可选模型清单（id + 展示名） */
  models: ModelEntry[]
}

export interface RoleConfig {
  provider_id: string
  model: string
  temperature?: number
  max_tokens?: number
  /** 核取样：只保留累计概率前 P 的候选词（top_p） */
  top_p?: number
  /** 存在惩罚：抑制已出现过的词（presence_penalty） */
  presence_penalty?: number
  /** 频率惩罚：按出现次数抑制高频词（frequency_penalty） */
  frequency_penalty?: number
  /** 重复惩罚：部分供应商支持（repetition_penalty），默认不发 */
  repetition_penalty?: number
  /** 停止序列：命中即停 */
  stop?: string[]
  /** 命名预设档标签（创意写作 / 日常 RP / 严肃对话 / NPC 模式） */
  preset?: string
  /** 思考强度（reasoning_effort）：low / medium / high 等；缺省 = 供应商默认 */
  reasoning_effort?: string
}

/** 全局模型配置（#26 ③）：单一 AI（story）+ 结对（pair）+ 向量化（embedding） */
export interface AppConfig {
  providers: ProviderConfig[]
  roles: {
    story: RoleConfig
    pair?: RoleConfig
    embedding: RoleConfig
  }
  /** 成本护栏：每回合 token 上限，0 = 不限（#26 ⑤） */
  turn_token_budget?: number
  /** AI 后端开关：provider = auto|rig|scripted；embedding = auto|rig|stub */
  ai?: { provider: string; embedding: string }
}

export interface ProviderTestResult {
  ok: boolean
  message: string
  latency_ms?: number
}

/** 模型探测结果（GET {base_url}/models） */
export interface ProbeResult { models: ModelEntry[]; source: string }

/** 模型目录（来源 https://pi.dev/models 的快照，含上下文 / 思考能力元数据） */
export interface CatalogModel {
  id: string
  name: string
  /** 上下文窗口（tokens） */
  ctx?: number
  /** 最大输出（tokens）；缺省同 ctx */
  maxOut?: number
  /** 是否支持思考（reasoning） */
  reasoning?: boolean
  /** 思考等级 → 供应商实际下发值（null = 关闭，即不发送该参数） */
  tl?: Record<string, string | null>
}
export interface CatalogProvider { id: string; label: string; models: CatalogModel[] }

// ---------- 通用工具类型 ----------

export interface Listener<T> {
  onEvent: (evt: T) => void
  onStatus?: (status: StreamStatus) => void
}

export function uid(prefix = 'id'): string {
  return prefix + '-' + Math.random().toString(36).slice(2, 8) + Date.now().toString(36).slice(-4)
}

export type { SavePackage } from './generated/SavePackage'
export type { PlaytestRequest } from './generated/PlaytestRequest'
export type { EntityRef } from './generated/EntityRef'
export type { FocusEntity } from './generated/FocusEntity'

