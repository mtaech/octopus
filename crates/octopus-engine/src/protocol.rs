//! protocol：AI 输出协议端口（叙事契约 P2）。
//!
//! 故事书可在 \`storybook.narrative.protocol\` 覆盖 AI 的输出协议（格式说明 + 解析），
//! 但必须收敛到固定接口：三种模式的出口都是 \`Vec<Intent>\`，结算层完全不感知协议模式。
//!
//! - \`default\`：引擎内置文本 + \`parse_intents\`（与 P2 之前行为一致）；
//! - \`declarative\`：引擎按 \`intents\` 白名单渲染协议说明，解析仍用 \`parse_intents\`，
//!   但白名单之外的意图被过滤（记警告事件 \`intent_not_allowed\`）；
//! - \`lua\`：\`protocol.preamble(ctx)\` / \`protocol.parse(raw)\` / 可选 \`normalize\`，
//!   复用 \`LuaHost\` 沙箱（\`LuaMount::Protocol\`，只读，独立指令预算）。
//!
//! 确定性：所有模式都是纯函数；Lua 用固定种子 + 确定性沙箱，不引入正则 / 文本变换层。

use std::sync::Mutex;

use octopus_types::{Intent, IntentEnvelope, IssueSeverity, ValidationIssue};
use serde_json::Value;

use crate::error::EngineError;
use crate::lua_host::{LuaHost, LuaHostContext, SandboxLimits};
use crate::ports::TurnContext;

/// 协议模式。缺省 / 显式 \`default\` 都走引擎内置协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtocolMode {
    #[default]
    Default,
    Declarative,
    Lua,
}

impl ProtocolMode {
    /// 解析故事书里的 mode 字符串；未知值返回 None（校验层负责报错，运行期回落 Default）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "default" => Some(ProtocolMode::Default),
            "declarative" => Some(ProtocolMode::Declarative),
            "lua" => Some(ProtocolMode::Lua),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProtocolMode::Default => "default",
            ProtocolMode::Declarative => "declarative",
            ProtocolMode::Lua => "lua",
        }
    }
}

/// 解析后的协议声明（TurnContext 携带；None / Default 都等价于不覆盖）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtocolSpec {
    pub mode: ProtocolMode,
    /// declarative 白名单（须是引擎已知意图集合的子集）。
    pub intents: Vec<String>,
    /// 协议补充说明；计入 turn_token_budget，可被裁剪。
    pub instructions: Option<String>,
    /// lua 协议插件源码。
    pub lua: Option<String>,
}

impl ProtocolSpec {
    /// 从故事书 JSON 解析协议声明；无 protocol 字段返回 None（= 默认协议）。
    pub fn from_storybook(storybook: &Value) -> Option<ProtocolSpec> {
        let p = storybook.pointer("/narrative/protocol")?;
        if p.is_null() {
            return None;
        }
        Some(Self::from_config(p))
    }

    /// 从 \`storybook.narrative.protocol\` 对象解析；非法 mode 回落到 Default（运行期不 panic）。
    pub fn from_config(p: &Value) -> ProtocolSpec {
        let mode = p
            .get("mode")
            .and_then(Value::as_str)
            .and_then(ProtocolMode::parse)
            .unwrap_or_default();
        let intents = p
            .get("intents")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let instructions = p
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        let lua = p
            .get("lua")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        ProtocolSpec { mode, intents, instructions, lua }
    }

    /// 是否为有效的 lua 模式协议（api 发布门据它决定是否跑动态一致性）。
    pub fn is_lua(&self) -> bool {
        self.mode == ProtocolMode::Lua && self.lua.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
}

/// 引擎已知的意图类型集合（declarative 白名单的合法取值域）。
pub const KNOWN_INTENTS: &[&str] = &[
    "think",
    "narrate",
    "speak",
    "emote",
    "check",
    "move",
    "use_skill",
    "use_item",
    "interact",
    "strike",
    "enemy_strike",
    "advance_scene",
    "query_world",
    "query_character",
    "query_relationships",
    "intervene",
    "quest",
    "encounter",
    "adjust",
    "status",
    "summary",
    "end_turn",
    "finish_turn",
];

/// 至少含其中一个，回合才不至于空转（校验规则 §6.1-3）。
pub const NARRATIVE_INTENTS: &[&str] = &["narrate", "speak", "emote"];

pub fn is_known_intent(name: &str) -> bool {
    KNOWN_INTENTS.contains(&name)
}

/// 每个意图的签名与用途：declarative 模板按白名单渲染。
const INTENT_CATALOG: &[(&str, &str, &str)] = &[
    ("think", "think {content}", "思考草稿（引擎折叠展示，不进叙事）"),
    ("narrate", "narrate {content, actor_id?}", "旁白（环境 / 场景 / 玩家角色动作；角色自身的动作请用 emote）"),
    ("speak", "speak {content, actor_id, tone?}", "角色台词"),
    ("emote", "emote {content, actor_id, emotion?}", "神态动作（角色自身的动作与神态，必须带 actor_id）"),
    ("check", "check {attribute, difficulty?}", "判定"),
    ("move", "move {destination_id}", "移动"),
    ("use_skill", "use_skill {skill_id, target_id?}", "使用技能"),
    ("use_item", "use_item {item_id, target_id?}", "使用物品"),
    ("interact", "interact {object_id, action}", "与场景物件交互（#01 objects）"),
    ("strike", "strike {enemy_id, skill_id?}", "攻击遭遇中的敌人（由引擎结算）"),
    (
        "enemy_strike",
        "enemy_strike {enemy_id, target_id?, skill_id?}",
        "遭遇中的敌人攻击某个角色（由引擎结算；target_id 缺省 = 受控角色）",
    ),
    ("advance_scene", "advance_scene {target_scene_id?, abandon?}", "推进场景"),
    ("query_world", "query_world {query}", "查询世界"),
    (
        "query_character",
        "query_character {character_id?}",
        "查询角色资料（角色 AI 只能查自己）",
    ),
    (
        "query_relationships",
        "query_relationships {entity_id?}",
        "查询关系（角色 AI 只得触及自己的边）",
    ),
    ("intervene", "intervene {content}", "介入"),
    ("quest", "quest {text, hidden?, primary?}", "新增任务（导演）"),
    (
        "encounter",
        "encounter {name, enemies:[{name,hp?,ac?,template_id?,count?,skill_id?}], note?}",
        "创建遭遇（导演；template_id 引用图鉴模板，按 count 克隆实例）",
    ),
    ("adjust", "adjust {character_id, resource, amount}", "调整资源（导演）"),
    ("status", "status {character_id, status_id, remove?}", "施加 / 移除状态（导演）"),
    ("summary", "summary {text}", "本回合微摘要（派生记忆，不进叙事）"),
    (
        "end_turn",
        "end_turn {}",
        "结束当前行动者的时序回合（战斗中推进到下一个行动者；不改变叙事）",
    ),
    ("finish_turn", "finish_turn {}", "回合收束"),
];

fn catalog_entry(name: &str) -> Option<(&'static str, &'static str)> {
    INTENT_CATALOG
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, sig, desc)| (*sig, *desc))
}

/// Intent → 类型名（与 serde tag 一致），供白名单过滤与调试。
pub fn intent_kind(intent: &Intent) -> &'static str {
    match intent {
        Intent::Think { .. } => "think",
        Intent::Narrate { .. } => "narrate",
        Intent::Speak { .. } => "speak",
        Intent::Emote { .. } => "emote",
        Intent::Move { .. } => "move",
        Intent::UseSkill { .. } => "use_skill",
        Intent::UseItem { .. } => "use_item",
        Intent::Interact { .. } => "interact",
        Intent::Check { .. } => "check",
        Intent::QueryWorld { .. } => "query_world",
        Intent::QueryCharacter { .. } => "query_character",
        Intent::QueryRelationships { .. } => "query_relationships",
        Intent::AdvanceScene { .. } => "advance_scene",
        Intent::Intervene { .. } => "intervene",
        Intent::Quest { .. } => "quest",
        Intent::Adjust { .. } => "adjust",
        Intent::Strike { .. } => "strike",
        Intent::EnemyStrike { .. } => "enemy_strike",
        Intent::Encounter { .. } => "encounter",
        Intent::Status { .. } => "status",
        Intent::Summary { .. } => "summary",
        Intent::EndTurn => "end_turn",
        Intent::FinishTurn => "finish_turn",
    }
}

/// 引擎默认协议的系统提示词（主线 AI）——即 P2 之前 \`STORY_PREAMBLE\` 原文，保持行为兼容。
pub const SYSTEM_PREAMBLE: &str = "你是 Octopus 游戏的「AI 主持」：负责故事走向、旁白与世界响应，并扮演所有非玩家角色（NPC）的台词、神态与动作，在故事书的结构化骨架内即兴导演。\n\
输出要求（必须严格遵守）：\n\
1. 只输出一个 JSON 数组：全部内容都放进数组元素，不附加说明文字，不使用 Markdown 代码块。\n\
2. 数组每个元素是一个「意图」对象，必须带 type 字段；可用类型：\n\
   narrate {content, actor_id?} 旁白；speak {content, actor_id, tone?} 角色台词；emote {content, actor_id, emotion?} 神态动作；\n\
   check {attribute, difficulty?} 判定（attribute 只能填「可用判定属性」里列出的 key）；move {destination_id, character_id?}；use_skill {skill_id, target_id?}；\n\
   use_item {item_id, target_id?}；interact {object_id, action} 与场景物件交互；strike {enemy_id, skill_id?} 攻击遭遇中的敌人；\n   enemy_strike {enemy_id, target_id?, skill_id?} 让遭遇中的敌人攻击某个角色（target_id 缺省 = 受控角色）；advance_scene {target_scene_id?, abandon?}；query_world {query}；finish_turn {}\n\
3. speak / emote 带上 actor_id：只填「在场角色」名单里括号内的 id（如 char-isa），一次只扮演一个人；玩家受控角色的台词由玩家输入，你专注世界与 NPC 的回应。\n\
   **归属分清：角色的动作、神态与反应一律用 emote（带 actor_id）**；narrate 只写环境、天气、时间流逝、场景切换与玩家角色自己的动作，这类纯旁白可省略 actor_id。\n\
4. 扮演 NPC 时以第一人称口吻，符合其背景、性格与对话示例的语气；引用实体时使用名单里给出的 id。\n\
5. 玩家攻击「当前遭遇」里的敌人时，用 strike {enemy_id} 交给引擎结算；敌人攻击角色时用 enemy_strike {enemy_id, target_id?}；把引擎给出的结果叙述得有画面感。\n\
6. 每次输出 1-3 个意图；确实无事可做时输出 []。\n\
7. 叙事、遭遇与场景推进都围绕【当前场景】与【当前任务】展开，保持一致。\n\
8. 需要先打草稿 / 内心推演时，用 think {content} 意图写思考；引擎会把它折叠展示，不算叙事。\n\
9. 每回合**恰好**输出一条 summary {text}：用一两句话概括本回合发生的事（供记忆检索）。它不算叙事、不改世界状态，属可选但鼓励。\n\
10. 需要世界状态或判定结果时，先输出 query_world / check / interact 意图；引擎会把结果回给你，你据此在同一回合继续输出意图，最后用 finish_turn {} 收束。信息已经足够时一次给全，并用 finish_turn {} 收束。";

/// 工具模式（原生工具调用）下的引擎默认系统提示词：行为规则与 SYSTEM_PREAMBLE 一致，
/// 但输出形态从「输出 JSON 数组」改为「调用意图工具」。
pub const TOOL_PREAMBLE: &str = "你是 Octopus 游戏的「AI 主持」：负责故事走向、旁白与世界响应，并扮演所有非玩家角色（NPC）的台词、神态与动作，在故事书的结构化骨架内即兴导演。\n\
行为规则（必须严格遵守）：\n\
1. 通过调用「意图工具」推进剧情：每个工具对应一种意图（narrate 旁白 / speak 台词 / emote 神态 / check 判定 / move 移动 / use_skill 用技能 / use_item 用物品 / interact 交互 / strike 攻击 / advance_scene 换场 / query_world 查世界 / finish_turn 收束等），一次可并行调用多个。\n\
2. speak / emote 带上 actor_id：只填「在场角色」名单里括号内的 id（如 char-isa），一次只扮演一个人；玩家受控角色的台词由玩家输入，你专注世界与 NPC 的回应。\n\
   **归属分清：角色的动作、神态与反应一律用 emote（带 actor_id）**；narrate 只写环境、天气、时间流逝、场景切换与玩家角色自己的动作，这类纯旁白可省略 actor_id。\n\
3. 扮演 NPC 时以第一人称口吻，符合其背景、性格与对话示例的语气；引用实体时使用名单里给出的 id。\n\
4. 玩家攻击「当前遭遇」里的敌人时，用 strike {enemy_id} 交给引擎结算；敌人攻击角色时用 enemy_strike {enemy_id, target_id?}；把引擎给出的结果叙述得有画面感。\n\
5. 每轮调用 1-3 个意图工具；确实无事可做时直接调用 finish_turn。\n\
6. 叙事、遭遇与场景推进都围绕【当前场景】与【当前任务】展开，保持一致。\n\
7. 需要先打草稿 / 内心推演时，用 think {content} 意图写思考；引擎会把它折叠展示，不算叙事。\n\
8. 每回合**恰好**输出一条 summary {text}：用一两句话概括本回合发生的事（供记忆检索）。它不算叙事、不改世界状态，属可选但鼓励。\n\
9. 需要世界状态或判定结果时，先调用 query_world / check / interact；引擎会把结果回给你，你据此在本轮继续调用工具，最后调用 finish_turn 收束。信息已经足够时一次给全，并用 finish_turn 收束。";



/// 从模型输出里稳健地提取意图数组（容忍围栏代码块与前后废话）。
///
/// P2 从 \`octopus-ai::rig_provider\` 下沉到引擎，供三种协议模式复用；行为保持逐字一致。
/// 返回包络以保留可选的 `intent_id`（#04 ⑨）：老模型 / 旧协议没给 id 时缺省 None，
/// 与「按纯意图列表解析」逐字同义。
pub fn parse_intent_envelopes(raw: &str) -> Result<Vec<IntentEnvelope>, String> {
    let bt = '\u{60}';
    let fence: String = [bt, bt, bt].iter().collect();
    let mut s = raw.trim();
    if let Some(i) = s.find(&fence) {
        let after = &s[i + fence.len()..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(j) = after.find(&fence) {
            s = after[..j].trim();
        }
    }
    if let Ok(v) = serde_json::from_str::<Vec<IntentEnvelope>>(s) {
        return Ok(v);
    }
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(s) {
        if let Some(arr) = obj.get("intents").and_then(|x| x.as_array()) {
            return serde_json::from_value::<Vec<IntentEnvelope>>(serde_json::Value::Array(arr.clone()))
                .map_err(|e| format!("intents 字段解析失败：{e}"));
        }
    }
    if let (Some(a), Some(b)) = (s.find('['), s.rfind(']')) {
        if a < b {
            return serde_json::from_str::<Vec<IntentEnvelope>>(&s[a..=b])
                .map_err(|e| format!("解析意图 JSON 失败：{e}"));
        }
    }
    Err(format!(
        "模型未返回合法意图 JSON：{}",
        raw.chars().take(240).collect::<String>()
    ))
}

/// 提取纯意图列表（不含幂等 id 的投影）：保持既有公开签名，供不需要包络的调用方使用。
pub fn parse_intents(raw: &str) -> Result<Vec<Intent>, String> {
    parse_intent_envelopes(raw).map(|v| v.into_iter().map(|e| e.intent).collect())
}

/// 一个「意图工具」：模型通过**原生工具调用**输出意图（pi 式 agent 的工具形态）。
/// 工具名 = 意图 type；参数 = 意图字段的 JSON Schema（宽松提示，服务端反序列化仍严格校验）。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema 对象（properties + required）。
    pub parameters: Value,
}

/// 由字段表生成 JSON Schema：`(字段名, JSON Schema type, 是否必需)`。
fn tool_schema(props: &[(&str, &str, bool)]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();
    for (name, ty, req) in props {
        properties.insert(name.to_string(), serde_json::json!({ "type": ty }));
        if *req {
            required.push(serde_json::json!(name));
        }
    }
    serde_json::json!({ "type": "object", "properties": properties, "required": required })
}

/// 意图工具清单：`(意图 type, 用途, 参数字段 (名, JSON Schema type, 是否必需))`。
/// 与 `Intent` 变体字段一一对应；`finish_turn` 无参数。
const INTENT_TOOLS: &[(&str, &str, &[(&str, &str, bool)])] = &[
    ("think", "思考草稿（引擎折叠展示，不进叙事）", &[("content", "string", true)]),
    ("narrate", "旁白（环境 / 场景 / 玩家角色动作；角色自身的动作请用 emote）", &[("content", "string", true), ("actor_id", "string", false)]),
    (
        "speak",
        "角色台词",
        &[("content", "string", true), ("actor_id", "string", true), ("tone", "string", false)],
    ),
    (
        "emote",
        "神态动作（角色自身的动作与神态，必须带 actor_id）",
        &[("content", "string", true), ("actor_id", "string", false), ("emotion", "string", false)],
    ),
    (
        "check",
        "判定（attribute 只能填「可用判定属性」里列出的 key）",
        &[("attribute", "string", true), ("difficulty", "integer", false), ("actor_id", "string", false)],
    ),
    (
        "move",
        "移动",
        &[("destination_id", "string", true), ("character_id", "string", false)],
    ),
    ("use_skill", "使用技能", &[("skill_id", "string", true), ("target_id", "string", false)]),
    ("use_item", "使用物品", &[("item_id", "string", true), ("target_id", "string", false)]),
    ("interact", "与场景物件交互", &[("object_id", "string", true), ("action", "string", true)]),
    ("strike", "攻击遭遇中的敌人（由引擎结算）", &[("enemy_id", "string", true), ("skill_id", "string", false)]),
    (
        "enemy_strike",
        "遭遇中的敌人攻击某个角色（由引擎结算；target_id 缺省 = 受控角色）",
        &[
            ("enemy_id", "string", true),
            ("target_id", "string", false),
            ("skill_id", "string", false),
        ],
    ),
    (
        "advance_scene",
        "推进场景",
        &[("target_scene_id", "string", false), ("abandon", "boolean", false)],
    ),
    ("query_world", "查询世界", &[("query", "string", true)]),
    ("query_character", "查询角色资料", &[("character_id", "string", false)]),
    ("query_relationships", "查询关系", &[("entity_id", "string", false)]),
    ("intervene", "介入", &[("content", "string", true)]),
    (
        "quest",
        "新增任务（导演）",
        &[("text", "string", true), ("hidden", "boolean", false), ("primary", "boolean", false)],
    ),
    (
        "encounter",
        "创建遭遇（导演）",
        &[("name", "string", true), ("enemies", "array", true), ("note", "string", false)],
        // enemies 每项 = EnemySpec {name, hp?, ac?, template_id?, count?, skill_id?}：
        // template_id 命中图鉴模板（kind=monster）时按 count 克隆怪物实例。
    ),
    (
        "adjust",
        "调整资源（导演）",
        &[("character_id", "string", true), ("resource", "string", true), ("amount", "integer", true)],
    ),
    (
        "status",
        "施加 / 移除状态（导演）",
        &[("character_id", "string", true), ("status_id", "string", true), ("remove", "boolean", false)],
    ),
    ("summary", "本回合微摘要（派生记忆，不进叙事）", &[("text", "string", true)]),
    (
        "end_turn",
        "结束当前行动者的时序回合（战斗中推进到下一个行动者；不改变叙事）",
        &[],
    ),
    ("finish_turn", "回合收束（本轮不再产生意图）", &[]),
];

/// 全部意图的工具定义（default 协议）。
pub fn all_intent_tools() -> Vec<ToolSpec> {
    INTENT_TOOLS
        .iter()
        .map(|(name, desc, props)| ToolSpec {
            name: name.to_string(),
            description: desc.to_string(),
            parameters: tool_schema(props),
        })
        .collect()
}

/// 白名单意图的工具定义（declarative 协议）；`finish_turn` 恒保留。
pub fn whitelist_intent_tools(names: &[String]) -> Vec<ToolSpec> {
    let mut out: Vec<ToolSpec> = INTENT_TOOLS
        .iter()
        .filter(|(name, _, _)| names.iter().any(|w| w == name))
        .map(|(name, desc, props)| ToolSpec {
            name: name.to_string(),
            description: desc.to_string(),
            parameters: tool_schema(props),
        })
        .collect();
    if !out.iter().any(|t| t.name == "finish_turn") {
        out.push(ToolSpec {
            name: "finish_turn".to_string(),
            description: "回合收束（本轮不再产生意图）".to_string(),
            parameters: tool_schema(&[]),
        });
    }
    out
}

/// 协议适配器端口：三种模式出口统一为 \`Vec<Intent>\`（与 \`AiProvider\` 同构）。
pub trait ProtocolAdapter: Send + Sync {
    /// 该协议支持的原生意图工具清单；None = 走文本协议（模型输出文本由 `parse` 解析）。
    ///
    /// 工具模式下，模型通过「调用意图工具」输出意图，`parse` 退居兜底
    /// （模型没调工具、只输出文本时仍能解析）。
    fn tools(&self) -> Option<Vec<ToolSpec>> {
        None
    }

    /// 工具模式的系统提示词变体：不再要求输出 JSON 数组，改为引导使用工具。
    /// None = 复用 `preamble()`。
    fn tool_preamble(&self, _ctx: &TurnContext) -> Option<String> {
        None
    }
    /// 注入系统层的协议说明（替代硬编码 preamble 里的协议段）。
    fn preamble(&self, ctx: &TurnContext) -> String;

    /// 模型原始输出 → 意图包络；这是引擎唯一入口（包络携带可选 intent_id）。
    fn parse(&self, raw: &str, ctx: &TurnContext) -> Result<Vec<IntentEnvelope>, EngineError>;

    /// 解析后归一化（可选，默认恒等）。
    fn normalize(&self, intents: Vec<IntentEnvelope>, _ctx: &TurnContext) -> Vec<IntentEnvelope> {
        intents
    }

    /// 取走适配器累积的警告（白名单过滤掉意图等），供 Session 发 System 事件。
    fn take_warnings(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 默认协议：引擎内置文本 preamble + 意图 JSON 解析。
pub struct DefaultProtocol;

impl DefaultProtocol {
    pub fn new() -> Self {
        Self
    }
}

impl ProtocolAdapter for DefaultProtocol {
    fn tools(&self) -> Option<Vec<ToolSpec>> {
        // 默认协议：全部意图工具化（模型原生调用工具输出意图；文本输出仍可兜底解析）。
        Some(all_intent_tools())
    }

    fn tool_preamble(&self, _ctx: &TurnContext) -> Option<String> {
        Some(TOOL_PREAMBLE.to_string())
    }

    fn preamble(&self, _ctx: &TurnContext) -> String {
        SYSTEM_PREAMBLE.to_string()
    }

    fn parse(&self, raw: &str, _ctx: &TurnContext) -> Result<Vec<IntentEnvelope>, EngineError> {
        parse_intent_envelopes(raw).map_err(EngineError::Ai)
    }
}

/// 声明式协议：覆盖解释（模板 + instructions），解析仍用 \`parse_intents\` + 白名单过滤。
pub struct DeclarativeProtocol {
    whitelist: Vec<String>,
    instructions: Option<String>,
    warnings: Mutex<Vec<String>>,
}

impl DeclarativeProtocol {
    pub fn new(whitelist: Vec<String>, instructions: Option<String>) -> Self {
        Self { whitelist, instructions, warnings: Mutex::new(Vec::new()) }
    }

    /// 白名单 + 始终保留 finish_turn（回合收束不能被禁用）。
    fn allowed(&self) -> Vec<String> {
        let mut out: Vec<String> = self.whitelist.clone();
        if !out.iter().any(|w| w == "finish_turn") {
            out.push("finish_turn".to_string());
        }
        out
    }
}

impl ProtocolAdapter for DeclarativeProtocol {
    fn tools(&self) -> Option<Vec<ToolSpec>> {
        // 白名单即工具清单：模型只能调用白名单内的意图工具（比「输出后过滤」更强）。
        Some(whitelist_intent_tools(&self.allowed()))
    }

    fn tool_preamble(&self, ctx: &TurnContext) -> Option<String> {
        let mut b = String::new();
        b.push_str("你是 Octopus 游戏的「AI 主持」：负责故事走向、旁白与世界响应，并扮演所有非玩家角色的台词、神态与动作，在故事书的结构化骨架内即兴导演。\n");
        b.push_str("行为规则（必须严格遵守）：\n");
        b.push_str("1. 通过调用「意图工具」推进剧情：每个工具对应一种意图；本故事书允许以下工具：\n");
        for name in self.allowed() {
            if let Some((_, desc)) = catalog_entry(&name) {
                b.push_str(&format!("   {name}：{desc}；\n"));
            }
        }
        b.push_str("2. speak / emote 带上 actor_id：只填「在场角色」名单里括号内的 id（如 char-isa）；纯旁白 narrate 可省略。\n");
        b.push_str("3. 扮演 NPC 时以第一人称口吻，符合其背景、性格与对话示例的语气；引用实体时使用名单里给出的 id。\n");
        b.push_str("4. 每轮调用 1-3 个意图工具；确实无事可做时直接调用 finish_turn。\n");
        if let Some(ins) = self.instructions.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            b.push_str("\n【协议补充说明】\n");
            b.push_str(&trim_instructions(ins, ctx.token_budget));
            b.push('\n');
        }
        Some(b)
    }

    fn preamble(&self, ctx: &TurnContext) -> String {
        let intro = "你是 Octopus 游戏的「AI 主持」：负责故事走向、旁白与世界响应，并扮演所有非玩家角色的台词、神态与动作，在故事书的结构化骨架内即兴导演。";
        let mut b = String::new();
        b.push_str(intro);
        b.push('\n');
        b.push_str("输出要求（必须严格遵守）：\n");
        b.push_str("1. 只输出一个 JSON 数组：全部内容都放进数组元素，不附加说明文字，不使用 Markdown 代码块。\n");
        b.push_str("2. 数组每个元素是一个「意图」对象，必须带 type 字段；本故事书允许以下类型：\n");
        for name in self.allowed() {
            if let Some((sig, desc)) = catalog_entry(&name) {
                b.push_str(&format!("   {sig} {desc}；\n"));
            }
        }
        b.push_str("3. speak / emote 带上 actor_id：只填「在场角色」名单里括号内的 id（如 char-isa）；纯旁白 narrate 可省略。\n");
        b.push_str("4. 扮演 NPC 时以第一人称口吻，符合其背景、性格与对话示例的语气；引用实体时使用名单里给出的 id。\n");
        b.push_str("5. 每次输出 1-3 个意图；确实无事可做时输出 []。\n");
        if let Some(ins) = self.instructions.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            b.push_str("\n【协议补充说明】\n");
            b.push_str(&trim_instructions(ins, ctx.token_budget));
            b.push('\n');
        }
        b
    }

    fn parse(&self, raw: &str, _ctx: &TurnContext) -> Result<Vec<IntentEnvelope>, EngineError> {
        let intents = parse_intent_envelopes(raw).map_err(EngineError::Ai)?;
        let mut kept = Vec::with_capacity(intents.len());
        let mut rejected: Vec<String> = Vec::new();
        for envelope in intents {
            let kind = intent_kind(&envelope.intent);
            if kind == "finish_turn" || self.whitelist.iter().any(|w| w == kind) {
                kept.push(envelope);
            } else {
                rejected.push(kind.to_string());
            }
        }
        if !rejected.is_empty() {
            if let Ok(mut w) = self.warnings.lock() {
                for kind in rejected {
                    w.push(format!("意图 '{kind}' 不在协议白名单内，已被忽略"));
                }
            }
        }
        Ok(kept)
    }

    fn take_warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .map(|mut w| std::mem::take(&mut *w))
            .unwrap_or_default()
    }
}

/// instructions 计入 turn_token_budget（复用 lore 的 2 字符/token 保守估计），超出即按字符截断。
/// 协议模板本身是引擎固定文本，永不裁剪；只有创作者追加的 instructions 可裁。
fn trim_instructions(text: &str, token_budget: usize) -> String {
    if token_budget == 0 {
        return text.to_string();
    }
    let budget = token_budget.saturating_mul(2);
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let head: String = text.chars().take(budget).collect();
    format!("{head}…（已按 token 预算截断）")
}

/// 协议沙箱种子：固定值。协议解析是纯函数，不消费 RNG；固定种子保证确定性。
const PROTOCOL_SEED: u64 = 0;

/// 协议插件的独立沙箱预算（与技能 Lua 分离）。
pub fn protocol_sandbox_limits() -> SandboxLimits {
    SandboxLimits { memory_bytes: 8 * 1024 * 1024, max_instructions: 500_000 }
}

/// Lua 协议：preamble 与 parse 都由插件实现；复用 LuaHost 沙箱（只读挂载点）。
pub struct LuaProtocol {
    source: String,
    warnings: Mutex<Vec<String>>,
}

impl LuaProtocol {
    pub fn new(source: impl Into<String>) -> Self {
        Self { source: source.into(), warnings: Mutex::new(Vec::new()) }
    }

    fn host(&self) -> Result<LuaHost, EngineError> {
        LuaHost::with_limits(PROTOCOL_SEED, protocol_sandbox_limits())
            .map_err(|e| EngineError::Ai(format!("初始化协议沙箱失败：{e}")))
    }

    /// 用只读上下文跑 \`protocol.parse\`；conformance 与运行期共用。
    fn parse_raw(&self, raw: &str, host_ctx: &LuaHostContext) -> Result<Vec<IntentEnvelope>, EngineError> {
        let host = self.host()?;
        let out = host
            .run_protocol_parse(&self.source, raw, host_ctx)
            .map_err(|e| EngineError::Ai(format!("Lua 协议 parse 执行失败：{e}")))?;
        if let Some(err) = out.get("error").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            return Err(EngineError::Ai(format!("Lua 协议解析失败：{err}")));
        }
        let Some(arr) = out.get("intents").and_then(Value::as_array) else {
            return Err(EngineError::Ai(
                "Lua 协议 parse 必须返回 { intents = { ... }, error = nil|string }".to_string(),
            ));
        };
        serde_json::from_value::<Vec<IntentEnvelope>>(Value::Array(arr.clone()))
            .map_err(|e| EngineError::Ai(format!("Lua 协议返回的意图无法反序列化：{e}")))
    }

    fn record(&self, message: String) {
        if let Ok(mut w) = self.warnings.lock() {
            w.push(message);
        }
    }
}

/// TurnContext → Lua 只读上下文（对齐判定器 Lua 的快照）。
fn lua_context(ctx: &TurnContext) -> LuaHostContext {
    LuaHostContext {
        script_id: "protocol".to_string(),
        scene_id: ctx.scene_id.clone(),
        round: ctx.round,
        present: ctx.characters.iter().map(|c| c.id.clone()).collect(),
        controlled: ctx.controlled.clone(),
        ..Default::default()
    }
}

impl ProtocolAdapter for LuaProtocol {
    fn preamble(&self, ctx: &TurnContext) -> String {
        let host_ctx = lua_context(ctx);
        let host = match self.host() {
            Ok(h) => h,
            Err(e) => {
                self.record(e.to_string());
                return String::new();
            }
        };
        match host.run_protocol_preamble(&self.source, &host_ctx) {
            Ok(s) => s,
            Err(e) => {
                self.record(format!("Lua protocol.preamble 执行失败：{e}"));
                String::new()
            }
        }
    }

    fn parse(&self, raw: &str, ctx: &TurnContext) -> Result<Vec<IntentEnvelope>, EngineError> {
        self.parse_raw(raw, &lua_context(ctx))
    }

    fn normalize(&self, intents: Vec<IntentEnvelope>, ctx: &TurnContext) -> Vec<IntentEnvelope> {
        // normalize 可选：任何失败都回落到原意图（宁可少归一化，也不让整回合失败）。
        let host_ctx = lua_context(ctx);
        let host = match self.host() {
            Ok(h) => h,
            Err(e) => {
                self.record(e.to_string());
                return intents;
            }
        };
        let value = match serde_json::to_value(&intents) {
            Ok(v) => v,
            Err(_) => return intents,
        };
        match host.run_protocol_normalize(&self.source, &value, &host_ctx) {
            Ok(Some(v)) => serde_json::from_value::<Vec<IntentEnvelope>>(v).unwrap_or(intents),
            Ok(None) => intents,
            Err(e) => {
                self.record(format!("Lua protocol.normalize 执行失败：{e}"));
                intents
            }
        }
    }

    fn take_warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .map(|mut w| std::mem::take(&mut *w))
            .unwrap_or_default()
    }
}

/// 按解析后的 spec 构造适配器（单一 AI 出口）。
pub fn build_protocol_adapter(spec: &ProtocolSpec) -> Box<dyn ProtocolAdapter> {
    match spec.mode {
        ProtocolMode::Default => Box::new(DefaultProtocol::new()),
        ProtocolMode::Declarative => Box::new(DeclarativeProtocol::new(
            spec.intents.clone(),
            spec.instructions.clone(),
        )),
        ProtocolMode::Lua => Box::new(LuaProtocol::new(spec.lua.clone().unwrap_or_default())),
    }
}

/// 固定样例：合法意图数组。
const SAMPLE_VALID: &str = r#"[{"type":"narrate","content":"夜色沉下来。"}]"#;
/// 固定样例：围栏代码块 + 前后废话。
const SAMPLE_FENCED: &str =
    "好的，这是意图：\n\u{60}\u{60}\u{60}json\n[{\"type\":\"speak\",\"content\":\"稀客。\"}]\n\u{60}\u{60}\u{60}\n就这样。";
/// 固定样例：垃圾输入（既非 JSON 也无法提取数组）。
const SAMPLE_GARBAGE: &str = "我不会响应这种请求。";

/// 用固定样例跑一遍 Lua 协议插件，验证它能稳定产出意图。
///
/// 由 api 层 publish 调用（草稿保存不跑，避免每次自动保存都执行 Lua）：
/// 1. 合法意图数组 → 必须产出非空 intents；
/// 2. 围栏 + 废话 → 必须能提取；
/// 3. 垃圾输入 → 必须返回明确错误（Err），不得 panic / 超时。
pub fn check_protocol_conformance(lua_src: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if lua_src.trim().is_empty() {
        push_conformance(&mut issues, "协议插件源码为空");
        return issues;
    }
    let proto = LuaProtocol::new(lua_src);
    let host_ctx = LuaHostContext::default();

    match proto.parse_raw(SAMPLE_VALID, &host_ctx) {
        Ok(v) if !v.is_empty() => {}
        Ok(_) => push_conformance(&mut issues, "样例「合法意图数组」未产出任何意图"),
        Err(e) => push_conformance(&mut issues, &format!("样例「合法意图数组」解析失败：{e}")),
    }
    match proto.parse_raw(SAMPLE_FENCED, &host_ctx) {
        Ok(v) if !v.is_empty() => {}
        Ok(_) => push_conformance(&mut issues, "样例「围栏 + 废话」未提取出意图"),
        Err(e) => push_conformance(&mut issues, &format!("样例「围栏 + 废话」解析失败：{e}")),
    }
    match proto.parse_raw(SAMPLE_GARBAGE, &host_ctx) {
        Err(_) => {}
        Ok(_) => push_conformance(&mut issues, "样例「垃圾输入」未返回明确错误（parse 必须拒绝或报错）"),
    }
    issues
}

fn push_conformance(issues: &mut Vec<ValidationIssue>, message: &str) {
    issues.push(ValidationIssue {
        severity: IssueSeverity::Error,
        code: "protocol_conformance_failed".to_string(),
        target: Some("narrative.protocol".to_string()),
        message: format!("协议一致性检查未通过：{message}"),
        related_refs: None,
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    use octopus_types::{ActorRef, RoundChannel};

    /// 原生工具调用：default 协议的工具清单必须覆盖全部已知意图（工具名 = 意图 type）。
    #[test]
    fn default_tool_catalog_covers_all_known_intents() {
        let tools = all_intent_tools();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        let mut known: Vec<&str> = KNOWN_INTENTS.to_vec();
        known.sort_unstable();
        assert_eq!(names, known, "default 协议的工具清单应与 KNOWN_INTENTS 一一对应");
        for t in &tools {
            assert!(
                t.parameters.get("type").and_then(Value::as_str) == Some("object"),
                "工具 {} 的参数必须是 object schema",
                t.name
            );
        }
    }

    /// declarative 白名单 = 工具清单；finish_turn 恒保留。
    #[test]
    fn declarative_tools_follow_whitelist_and_keep_finish_turn() {
        let tools = whitelist_intent_tools(&["narrate".to_string(), "speak".to_string()]);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["narrate", "speak", "finish_turn"]);
        // 空白名单也保留 finish_turn。
        let empty = whitelist_intent_tools(&[]);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].name, "finish_turn");
    }

    /// 工具模式的系统提示词引导「调用工具」，不再要求输出 JSON 数组。
    #[test]
    fn tool_preamble_guides_tool_calls_not_json() {
        assert!(TOOL_PREAMBLE.contains("意图工具"), "{TOOL_PREAMBLE}");
        assert!(!TOOL_PREAMBLE.contains("JSON 数组"), "工具模式不应再要求 JSON 数组");
        assert!(TOOL_PREAMBLE.contains("finish_turn"));
        // 文本模式的 SYSTEM_PREAMBLE 仍要求 JSON 数组（互不干扰）。
        assert!(SYSTEM_PREAMBLE.contains("JSON 数组"));
    }

    /// 工具参数 schema：必需字段进 required，可选字段只在 properties。
    #[test]
    fn tool_schema_marks_required_fields() {
        let tools = all_intent_tools();
        let narrate = tools.iter().find(|t| t.name == "narrate").expect("narrate 工具");
        let required: Vec<&str> = narrate
            .parameters
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(required, vec!["content"], "narrate 只需 content");
        let finish = tools.iter().find(|t| t.name == "finish_turn").expect("finish_turn 工具");
        assert_eq!(
            finish.parameters.get("required").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
    }

    fn turn_ctx(protocol: Option<ProtocolSpec>) -> TurnContext {
        TurnContext {
            save_id: "sv-1".into(),
            round: 1,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "你好".into(),
            channel: RoundChannel::Character,
            characters: vec![ActorRef { id: "char-isa".into(), name: "伊莎".into() }],
            personas: vec![],
            premise: None,
            narrative: vec![],
            lore: vec![],
            token_budget: 0,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            attributes: vec![],
            model: None,
            memories: vec![],
            turn_feedback: vec![],
            protocol,
        }
    }

    /// 测试用「好插件」：自带最小 JSON 解码器，能从数组 / 围栏 + 废话里提取意图，
    /// 垃圾输入返回 error 字段（引擎据此报错）。刻意不放宽引擎接口，验证插件可自足。
    const GOOD_LUA_PLUGIN: &str = r#"
local function ws(s, i)
  while i <= #s do
    local c = s:sub(i, i)
    if c == " " or c == "\n" or c == "\r" or c == "\t" then i = i + 1 else break end
  end
  return i
end
local function parse_string(s, i)
  local buf = {}
  i = i + 1
  while true do
    local c = s:sub(i, i)
    if c == "" then error("unterminated string") end
    if c == '"' then return table.concat(buf), i + 1 end
    if c == "\\" then
      local n = s:sub(i + 1, i + 1)
      if n == "n" then table.insert(buf, "\n")
      elseif n == "t" then table.insert(buf, "\t")
      else table.insert(buf, n) end
      i = i + 2
    else
      table.insert(buf, c)
      i = i + 1
    end
  end
end
local parse_value
parse_value = function(s, i)
  i = ws(s, i)
  local c = s:sub(i, i)
  if c == '"' then return parse_string(s, i)
  elseif c == "[" then
    local arr = {}
    i = ws(s, i + 1)
    if s:sub(i, i) == "]" then return arr, i + 1 end
    while true do
      local v
      v, i = parse_value(s, i)
      table.insert(arr, v)
      i = ws(s, i)
      local d = s:sub(i, i)
      if d == "," then i = i + 1
      elseif d == "]" then return arr, i + 1
      else error("bad array") end
    end
  elseif c == "{" then
    local obj = {}
    i = ws(s, i + 1)
    if s:sub(i, i) == "}" then return obj, i + 1 end
    while true do
      local k
      k, i = parse_string(s, i)
      i = ws(s, i)
      if s:sub(i, i) ~= ":" then error("bad object") end
      local v
      v, i = parse_value(s, i + 1)
      obj[k] = v
      i = ws(s, i)
      local d = s:sub(i, i)
      if d == "," then i = i + 1
      elseif d == "}" then return obj, i + 1
      else error("bad object") end
    end
  elseif c == "t" then return true, i + 4
  elseif c == "f" then return false, i + 5
  elseif c == "n" then return nil, i + 4
  else
    local num = s:sub(i):match("^-?%d+%.?%d*")
    if not num then error("bad value") end
    return tonumber(num), i + #num
  end
end
protocol = protocol or {}
function protocol.preamble(ctx)
  return "输出 JSON 意图数组。"
end
function protocol.parse(raw)
  local open = raw:find("[", 1, true)
  if not open then return { error = "未找到意图数组" } end
  local ok, value = pcall(function() return (parse_value(raw, open)) end)
  if not ok or type(value) ~= "table" then return { error = "解析失败" } end
  return { intents = value }
end
"#;

    /// 测试用「坏插件」：parse 无条件抛错，样例一即失败。
    const BAD_LUA_PLUGIN: &str = "function protocol.preamble(ctx) return '' end\nfunction protocol.parse(raw) error('nope') end";

    fn declarative(intents: &[&str], instructions: Option<&str>) -> ProtocolSpec {
        ProtocolSpec {
            mode: ProtocolMode::Declarative,
            intents: intents.iter().map(|s| s.to_string()).collect(),
            instructions: instructions.map(str::to_string),
            lua: None,
        }
    }

    #[test]
    fn default_protocol_matches_engine_preamble_and_parse() {
        let ctx = turn_ctx(None);
        let ai = build_protocol_adapter(&ProtocolSpec::default());
        assert_eq!(ai.preamble(&ctx), SYSTEM_PREAMBLE, "默认协议 preamble 必须逐字不变");

        let raw = r#"[{"type":"narrate","content":"夜色沉下来。"}]"#;
        let a = ai.parse(raw, &ctx).unwrap();
        let b = parse_intents(raw).unwrap();
        assert_eq!(a.len(), b.len());
        assert_eq!(intent_kind(&a[0].intent), intent_kind(&b[0]));
        assert!(ai.take_warnings().is_empty());
    }

    /// #04 ⑨：解析保留可选 intent_id（缺省 None），interact 进已知意图。
    #[test]
    fn parse_keeps_intent_id_and_interact() {
        let raw = r#"[{"intent_id":"i-1","type":"interact","object_id":"o1","action":"open"}]"#;
        let v = parse_intent_envelopes(raw).unwrap();
        assert_eq!(v[0].intent_id.as_deref(), Some("i-1"));
        assert!(matches!(v[0].intent, Intent::Interact { .. }));
        // 旧协议无 intent_id：缺省 None，parse_intents 行为不变。
        let old = parse_intents(r#"[{"type":"narrate","content":"夜。"}]"#).unwrap();
        assert!(matches!(old[0], Intent::Narrate { .. }));
        assert!(is_known_intent("interact"));
    }

    #[test]
    fn declarative_filters_out_of_whitelist_and_records_warning() {
        let spec = declarative(&["narrate", "speak"], None);
        let adapter = build_protocol_adapter(&spec);
        let ctx = turn_ctx(Some(spec));
        let raw = r#"[{"type":"narrate","content":"夜。"},{"type":"strike","enemy_id":"e1"},{"type":"speak","content":"喂。","actor_id":"char-isa"},{"type":"finish_turn"}]"#;
        let intents = adapter.parse(raw, &ctx).unwrap();
        let kinds: Vec<&str> = intents.iter().map(|e| intent_kind(&e.intent)).collect();
        assert_eq!(kinds, vec!["narrate", "speak", "finish_turn"], "白名单外被过滤，finish_turn 永远保留");
        let warnings = adapter.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("strike"), "{warnings:?}");
        assert!(adapter.take_warnings().is_empty(), "警告取走后应清空");
    }

    /// #05 §3.2：summary 是引擎已知意图——默认协议直接解析，declarative 白名单含它即保留。
    #[test]
    fn summary_intent_is_known_and_allowed_by_whitelist() {
        assert!(is_known_intent("summary"));
        let spec = declarative(&["narrate", "summary"], None);
        let adapter = build_protocol_adapter(&spec);
        let ctx = turn_ctx(Some(spec));
        let raw = r#"[{"type":"narrate","content":"夜。"},{"type":"summary","text":"本回合入夜。"}]"#;
        let intents = adapter.parse(raw, &ctx).unwrap();
        let kinds: Vec<&str> = intents.iter().map(|e| intent_kind(&e.intent)).collect();
        assert_eq!(kinds, vec!["narrate", "summary"]);
        assert!(adapter.take_warnings().is_empty(), "白名单含 summary 不应产生警告");

        // 默认协议：无白名单也能解析 summary。
        let default = build_protocol_adapter(&ProtocolSpec::default());
        let intents = default
            .parse(r#"[{"type":"summary","text":"本回合入夜。"}]"#, &turn_ctx(None))
            .unwrap();
        assert_eq!(intent_kind(&intents[0].intent), "summary");
    }

    #[test]
    fn declarative_preamble_lists_whitelist_and_instructions() {
        let spec = declarative(&["narrate"], Some("用第二人称。"));
        let adapter = build_protocol_adapter(&spec);
        let p = adapter.preamble(&turn_ctx(Some(spec)));
        assert!(p.contains("narrate"));
        assert!(p.contains("finish_turn"), "需声明回合收束");
        assert!(!p.contains("strike"), "白名单外不应出现在协议说明里");
        assert!(p.contains("用第二人称。"));
    }

    #[test]
    fn declarative_instructions_are_trimmed_by_token_budget() {
        let mut spec = declarative(&["narrate"], Some(&"字".repeat(50)));
        spec.intents = vec!["narrate".to_string()];
        let adapter = build_protocol_adapter(&spec);
        let mut ctx = turn_ctx(Some(spec));
        ctx.token_budget = 5; // 5 token → 10 字符预算
        let p = adapter.preamble(&ctx);
        assert!(p.contains("已按 token 预算截断"), "{p}");
    }

    #[test]
    fn lua_protocol_parses_valid_fenced_and_rejects_garbage() {
        let spec = ProtocolSpec { mode: ProtocolMode::Lua, lua: Some(GOOD_LUA_PLUGIN.into()), ..Default::default() };
        let adapter = build_protocol_adapter(&spec);
        let ctx = turn_ctx(Some(spec));
        let valid = adapter.parse(SAMPLE_VALID, &ctx).unwrap();
        assert_eq!(valid.len(), 1);
        assert!(matches!(valid[0].intent, Intent::Narrate { .. }));
        let fenced = adapter.parse(SAMPLE_FENCED, &ctx).unwrap();
        assert_eq!(fenced.len(), 1);
        assert!(matches!(fenced[0].intent, Intent::Speak { .. }));
        assert!(adapter.parse(SAMPLE_GARBAGE, &ctx).is_err(), "垃圾输入必须明确报错");
    }

    #[test]
    fn lua_protocol_preamble_runs_with_context() {
        let src = r#"
protocol = protocol or {}
function protocol.preamble(ctx)
  return "在场=" .. tostring(#ctx.present) .. ";受控=" .. tostring(ctx.controlled) .. ";host=" .. tostring(host.present[1])
end
function protocol.parse(raw) return { intents = {} } end
"#;
        let spec = ProtocolSpec { mode: ProtocolMode::Lua, lua: Some(src.into()), ..Default::default() };
        let adapter = build_protocol_adapter(&spec);
        let p = adapter.preamble(&turn_ctx(Some(spec.clone())));
        assert_eq!(p, "在场=1;受控=米拉(char-mira);host=char-isa");
        assert!(adapter.take_warnings().is_empty());
    }

    #[test]
    fn lua_protocol_infinite_loop_is_rejected() {
        let src = "function protocol.preamble(ctx) return '' end\nfunction protocol.parse(raw) while true do end end";
        let spec = ProtocolSpec { mode: ProtocolMode::Lua, lua: Some(src.into()), ..Default::default() };
        let adapter = build_protocol_adapter(&spec);
        assert!(adapter.parse(SAMPLE_VALID, &turn_ctx(Some(spec))).is_err());
    }

    #[test]
    fn conformance_accepts_good_plugin_and_rejects_bad() {
        assert!(check_protocol_conformance(GOOD_LUA_PLUGIN).is_empty(), "好插件应零问题");
        let issues = check_protocol_conformance(BAD_LUA_PLUGIN);
        assert!(!issues.is_empty());
        assert!(issues.iter().all(|i| i.severity == IssueSeverity::Error));
        assert!(issues.iter().any(|i| i.code == "protocol_conformance_failed"));
        assert!(issues.iter().all(|i| i.target.as_deref() == Some("narrative.protocol")));
    }

    #[test]
    fn spec_from_storybook_parses_and_falls_back() {
        let sb = serde_json::json!({
            "narrative": { "protocol": { "mode": "declarative", "intents": ["narrate", " narrate "], "instructions": " 说明 " } }
        });
        let spec = ProtocolSpec::from_storybook(&sb).expect("protocol");
        assert_eq!(spec.mode, ProtocolMode::Declarative);
        assert_eq!(spec.intents, vec!["narrate", "narrate"]);
        assert_eq!(spec.instructions.as_deref(), Some(" 说明 "));
        // 未知 mode 解析期回落 Default（校验层负责报错）。
        let bad = serde_json::json!({ "narrative": { "protocol": { "mode": "bogus" } } });
        assert_eq!(ProtocolSpec::from_storybook(&bad).unwrap().mode, ProtocolMode::Default);
        assert!(ProtocolSpec::from_storybook(&serde_json::json!({})).is_none());
    }
}

