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

use octopus_types::{Intent, IssueSeverity, ValidationIssue};
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

/// AI 角色：同一协议在两处出口的默认文本不同（主线 / 角色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRole {
    Story,
    Character,
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
    "strike",
    "advance_scene",
    "query_world",
    "intervene",
    "quest",
    "encounter",
    "adjust",
    "status",
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
    ("narrate", "narrate {content, actor_id?}", "旁白"),
    ("speak", "speak {content, actor_id, tone?}", "角色台词"),
    ("emote", "emote {content, actor_id, emotion?}", "神态动作"),
    ("check", "check {attribute, difficulty?}", "判定"),
    ("move", "move {destination_id}", "移动"),
    ("use_skill", "use_skill {skill_id, target_id?}", "使用技能"),
    ("use_item", "use_item {item_id, target_id?}", "使用物品"),
    ("strike", "strike {enemy_id, skill_id?}", "攻击遭遇中的敌人（由引擎结算）"),
    ("advance_scene", "advance_scene {target_scene_id?, abandon?}", "推进场景"),
    ("query_world", "query_world {query}", "查询世界"),
    ("intervene", "intervene {content}", "介入"),
    ("quest", "quest {text, hidden?, primary?}", "新增任务（导演）"),
    ("encounter", "encounter {name, enemies:[{name,hp?,ac?}], note?}", "创建遭遇（导演）"),
    ("adjust", "adjust {character_id, resource, amount}", "调整资源（导演）"),
    ("status", "status {character_id, status_id, remove?}", "施加 / 移除状态（导演）"),
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
        Intent::Check { .. } => "check",
        Intent::QueryWorld { .. } => "query_world",
        Intent::AdvanceScene { .. } => "advance_scene",
        Intent::Intervene { .. } => "intervene",
        Intent::Quest { .. } => "quest",
        Intent::Adjust { .. } => "adjust",
        Intent::Strike { .. } => "strike",
        Intent::Encounter { .. } => "encounter",
        Intent::Status { .. } => "status",
        Intent::FinishTurn => "finish_turn",
    }
}

/// 引擎默认协议的系统提示词（主线 AI）——即 P2 之前 \`STORY_PREAMBLE\` 原文，保持行为兼容。
pub const STORY_PREAMBLE: &str = "你是 Octopus 游戏的「主线 AI」：负责故事走向、旁白与世界响应，在故事书的结构化骨架内即兴导演。\n\
输出要求（必须严格遵守）：\n\
1. 只输出一个 JSON 数组：全部内容都放进数组元素，不附加说明文字，不使用 Markdown 代码块。\n\
2. 数组每个元素是一个「意图」对象，必须带 type 字段；可用类型：\n\
   narrate {content, actor_id?} 旁白；speak {content, actor_id, tone?} 角色台词；emote {content, actor_id, emotion?} 神态动作；\n\
   check {attribute, difficulty?} 判定；move {destination_id}；use_skill {skill_id, target_id?}；\n\
   use_item {item_id, target_id?}；strike {enemy_id, skill_id?} 攻击遭遇中的敌人；advance_scene {target_scene_id?, abandon?}；query_world {query}；finish_turn {}\n\
3. speak / emote 带上 actor_id：填说话人或动作主体在「在场角色」名单里的 id（形如 名字(id)）。纯旁白 narrate 可省略。\n\
4. 用中文推进剧情，语气沉浸；引用实体时使用名单里给出的 id。\n\
5. 玩家攻击「当前遭遇」里的敌人时，用 strike {enemy_id} 交给引擎结算；把引擎给出的结果叙述得有画面感。\n\
6. 每次输出 1-3 个意图；确实无事可做时输出 []。\n\
7. 叙事、遭遇与场景推进都围绕【当前场景】与【当前任务】展开，保持一致。\n\
8. 需要先打草稿 / 内心推演时，用 think {content} 意图写思考；引擎会把它折叠展示，不算叙事。";

/// 引擎默认协议的系统提示词（角色 AI）。
pub const CHARACTER_PREAMBLE: &str = "你是 Octopus 游戏里的「角色 AI」：按某个人物的人设扮演其言行——台词、神态与动作。玩家输入是玩家（受控角色）的言行，你让在场 NPC 以第一人称作出符合人设的回应。场景、天气与全局旁白交给主线 AI；你聚焦这个人此刻说了什么、神态如何。玩家的话由玩家自己行动，你直接以角色口吻回应，而不是转述玩家说了什么。\n\
输出要求（必须严格遵守）：\n\
1. 只输出一个 JSON 数组：全部内容都放进数组元素，不附加说明文字，不使用 Markdown 代码块。\n\
2. 元素是「意图」对象，必须带 type 字段；角色最常用：\n\
   speak {content, actor_id, tone?} 台词；emote {content, actor_id, emotion?} 神态动作；narrate 只用于补足该角色自己的动作细节；必要时可用 check / use_skill / use_item。\n\
   actor_id 填说话人在「在场角色」名单里的 id（形如 名字(id)），一次只扮演一个人。\n\
3. 始终以角色第一人称口吻，符合其背景、性格与对话示例的语气；引用实体时使用名单里给出的 id。\n\
4. 只扮演「在场角色」名单里的人。\n\
5. 每次输出 1-3 个意图。\n\
6. 需要先打草稿 / 内心推演时，用 think {content} 意图写思考；引擎会把它折叠展示，不算台词或旁白。";

/// 从模型输出里稳健地提取意图数组（容忍围栏代码块与前后废话）。
///
/// P2 从 \`octopus-ai::rig_provider\` 下沉到引擎，供三种协议模式复用；行为保持逐字一致。
pub fn parse_intents(raw: &str) -> Result<Vec<Intent>, String> {
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
    if let Ok(v) = serde_json::from_str::<Vec<Intent>>(s) {
        return Ok(v);
    }
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(s) {
        if let Some(arr) = obj.get("intents").and_then(|x| x.as_array()) {
            return serde_json::from_value::<Vec<Intent>>(serde_json::Value::Array(arr.clone()))
                .map_err(|e| format!("intents 字段解析失败：{e}"));
        }
    }
    if let (Some(a), Some(b)) = (s.find('['), s.rfind(']')) {
        if a < b {
            return serde_json::from_str::<Vec<Intent>>(&s[a..=b])
                .map_err(|e| format!("解析意图 JSON 失败：{e}"));
        }
    }
    Err(format!(
        "模型未返回合法意图 JSON：{}",
        raw.chars().take(240).collect::<String>()
    ))
}

/// 协议适配器端口：三种模式出口统一为 \`Vec<Intent>\`（与 \`AiProvider\` 同构）。
pub trait ProtocolAdapter: Send + Sync {
    /// 注入系统层的协议说明（替代硬编码 preamble 里的协议段）。
    fn preamble(&self, ctx: &TurnContext) -> String;

    /// 模型原始输出 → 意图；这是引擎唯一入口。
    fn parse(&self, raw: &str, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError>;

    /// 解析后归一化（可选，默认恒等）。
    fn normalize(&self, intents: Vec<Intent>, _ctx: &TurnContext) -> Vec<Intent> {
        intents
    }

    /// 取走适配器累积的警告（白名单过滤掉意图等），供 Session 发 System 事件。
    fn take_warnings(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 默认协议：与 P2 之前完全一致的行为。
pub struct DefaultProtocol {
    role: ProtocolRole,
}

impl DefaultProtocol {
    pub fn new(role: ProtocolRole) -> Self {
        Self { role }
    }
}

impl ProtocolAdapter for DefaultProtocol {
    fn preamble(&self, _ctx: &TurnContext) -> String {
        match self.role {
            ProtocolRole::Story => STORY_PREAMBLE.to_string(),
            ProtocolRole::Character => CHARACTER_PREAMBLE.to_string(),
        }
    }

    fn parse(&self, raw: &str, _ctx: &TurnContext) -> Result<Vec<Intent>, EngineError> {
        parse_intents(raw).map_err(EngineError::Ai)
    }
}

/// 声明式协议：覆盖解释（模板 + instructions），解析仍用 \`parse_intents\` + 白名单过滤。
pub struct DeclarativeProtocol {
    whitelist: Vec<String>,
    instructions: Option<String>,
    role: ProtocolRole,
    warnings: Mutex<Vec<String>>,
}

impl DeclarativeProtocol {
    pub fn new(whitelist: Vec<String>, instructions: Option<String>, role: ProtocolRole) -> Self {
        Self { whitelist, instructions, role, warnings: Mutex::new(Vec::new()) }
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
    fn preamble(&self, ctx: &TurnContext) -> String {
        let intro = match self.role {
            ProtocolRole::Story => "你是 Octopus 游戏的「主线 AI」：负责故事走向、旁白与世界响应，在故事书的结构化骨架内即兴导演。",
            ProtocolRole::Character => "你是 Octopus 游戏里的「角色 AI」：按某个人物的人设扮演其言行——台词、神态与动作。",
        };
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
        b.push_str("3. speak / emote 带上 actor_id：填说话人或动作主体在「在场角色」名单里的 id（形如 名字(id)）；纯旁白 narrate 可省略。\n");
        b.push_str("4. 用中文推进剧情，语气沉浸；引用实体时使用名单里给出的 id。\n");
        b.push_str("5. 每次输出 1-3 个意图；确实无事可做时输出 []。\n");
        if let Some(ins) = self.instructions.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            b.push_str("\n【协议补充说明】\n");
            b.push_str(&trim_instructions(ins, ctx.token_budget));
            b.push('\n');
        }
        b
    }

    fn parse(&self, raw: &str, _ctx: &TurnContext) -> Result<Vec<Intent>, EngineError> {
        let intents = parse_intents(raw).map_err(EngineError::Ai)?;
        let mut kept = Vec::with_capacity(intents.len());
        let mut rejected: Vec<String> = Vec::new();
        for intent in intents {
            let kind = intent_kind(&intent);
            if kind == "finish_turn" || self.whitelist.iter().any(|w| w == kind) {
                kept.push(intent);
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
    fn parse_raw(&self, raw: &str, host_ctx: &LuaHostContext) -> Result<Vec<Intent>, EngineError> {
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
        serde_json::from_value::<Vec<Intent>>(Value::Array(arr.clone()))
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

    fn parse(&self, raw: &str, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError> {
        self.parse_raw(raw, &lua_context(ctx))
    }

    fn normalize(&self, intents: Vec<Intent>, ctx: &TurnContext) -> Vec<Intent> {
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
            Ok(Some(v)) => serde_json::from_value::<Vec<Intent>>(v).unwrap_or(intents),
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

/// 按解析后的 spec 构造适配器；role 区分主线 / 角色出口。
pub fn build_protocol_adapter(spec: &ProtocolSpec, role: ProtocolRole) -> Box<dyn ProtocolAdapter> {
    match spec.mode {
        ProtocolMode::Default => Box::new(DefaultProtocol::new(role)),
        ProtocolMode::Declarative => Box::new(DeclarativeProtocol::new(
            spec.intents.clone(),
            spec.instructions.clone(),
            role,
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

    fn turn_ctx(protocol: Option<ProtocolSpec>) -> TurnContext {
        TurnContext {
            save_id: "sv-1".into(),
            round: 1,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "你好".into(),
            channel: RoundChannel::Character,
            characters: vec![ActorRef { id: "char-isa".into(), name: "伊莎".into() }],
            personas: vec![],
            premise: None,
            narrative: vec![],
            lore: vec![],
            token_budget: 0,
            story_narration: None,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            model: None,
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
        let story = build_protocol_adapter(&ProtocolSpec::default(), ProtocolRole::Story);
        assert_eq!(story.preamble(&ctx), STORY_PREAMBLE, "默认协议主线 preamble 必须逐字不变");
        let character = build_protocol_adapter(&ProtocolSpec::default(), ProtocolRole::Character);
        assert_eq!(character.preamble(&ctx), CHARACTER_PREAMBLE, "默认协议角色 preamble 必须逐字不变");

        let raw = r#"[{"type":"narrate","content":"夜色沉下来。"}]"#;
        let a = story.parse(raw, &ctx).unwrap();
        let b = parse_intents(raw).unwrap();
        assert_eq!(a.len(), b.len());
        assert_eq!(intent_kind(&a[0]), intent_kind(&b[0]));
        assert!(story.take_warnings().is_empty());
    }

    #[test]
    fn declarative_filters_out_of_whitelist_and_records_warning() {
        let spec = declarative(&["narrate", "speak"], None);
        let adapter = build_protocol_adapter(&spec, ProtocolRole::Story);
        let ctx = turn_ctx(Some(spec));
        let raw = r#"[{"type":"narrate","content":"夜。"},{"type":"strike","enemy_id":"e1"},{"type":"speak","content":"喂。","actor_id":"char-isa"},{"type":"finish_turn"}]"#;
        let intents = adapter.parse(raw, &ctx).unwrap();
        let kinds: Vec<&str> = intents.iter().map(intent_kind).collect();
        assert_eq!(kinds, vec!["narrate", "speak", "finish_turn"], "白名单外被过滤，finish_turn 永远保留");
        let warnings = adapter.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("strike"), "{warnings:?}");
        assert!(adapter.take_warnings().is_empty(), "警告取走后应清空");
    }

    #[test]
    fn declarative_preamble_lists_whitelist_and_instructions() {
        let spec = declarative(&["narrate"], Some("用第二人称。"));
        let adapter = build_protocol_adapter(&spec, ProtocolRole::Story);
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
        let adapter = build_protocol_adapter(&spec, ProtocolRole::Story);
        let mut ctx = turn_ctx(Some(spec));
        ctx.token_budget = 5; // 5 token → 10 字符预算
        let p = adapter.preamble(&ctx);
        assert!(p.contains("已按 token 预算截断"), "{p}");
    }

    #[test]
    fn lua_protocol_parses_valid_fenced_and_rejects_garbage() {
        let spec = ProtocolSpec { mode: ProtocolMode::Lua, lua: Some(GOOD_LUA_PLUGIN.into()), ..Default::default() };
        let adapter = build_protocol_adapter(&spec, ProtocolRole::Story);
        let ctx = turn_ctx(Some(spec));
        let valid = adapter.parse(SAMPLE_VALID, &ctx).unwrap();
        assert_eq!(valid.len(), 1);
        assert!(matches!(valid[0], Intent::Narrate { .. }));
        let fenced = adapter.parse(SAMPLE_FENCED, &ctx).unwrap();
        assert_eq!(fenced.len(), 1);
        assert!(matches!(fenced[0], Intent::Speak { .. }));
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
        let adapter = build_protocol_adapter(&spec, ProtocolRole::Story);
        let p = adapter.preamble(&turn_ctx(Some(spec.clone())));
        assert_eq!(p, "在场=1;受控=米拉(char-mira);host=char-isa");
        assert!(adapter.take_warnings().is_empty());
    }

    #[test]
    fn lua_protocol_infinite_loop_is_rejected() {
        let src = "function protocol.preamble(ctx) return '' end\nfunction protocol.parse(raw) while true do end end";
        let spec = ProtocolSpec { mode: ProtocolMode::Lua, lua: Some(src.into()), ..Default::default() };
        let adapter = build_protocol_adapter(&spec, ProtocolRole::Story);
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

