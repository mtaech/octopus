//! rig 驱动的 AiProvider（#10/#20）：用 rig 框架调用 OpenAI 兼容端点生成「意图」。
//!
//! 取代手写 reqwest：provider 客户端、CompletionModel、Agent 与提示编排都由 rig 提供。
//! rig 的 OpenAI 客户端默认走 Responses API，这里显式用 CompletionsClient（Chat Completions），
//! 以兼容 DeepSeek / Moonshot / Groq / 本地 ollama-openai 等 OpenAI 兼容端点。

use async_trait::async_trait;
use octopus_engine::{
    AiOutput, AiProvider, CompactionReport, EngineError, ModelRef, ProtocolMode, TurnContext,
    build_protocol_adapter,
};

use crate::prompt::{StoryPrompts, render, render_block};
use octopus_types::{
    AiCallMessage, AiCallPayload, AiCallStatus, AiCallUsage, IntentEnvelope, RoundChannel,
};
use rig::completion::message::{AssistantContent, ReasoningContent, UserContent};
use rig::completion::{CompletionRequest, Message, ToolDefinition};
use rig::prelude::*;
use rig::providers::openai;

/// 单个供应商的连接参数。
#[derive(Debug, Clone)]
pub struct RigProviderParams {
    pub id: String,
    pub base_url: String,
    pub api_key: String,
}

/// 单一 AI 的默认模型与采样参数。
#[derive(Debug, Clone)]
pub struct RigRoleParams {
    pub provider_id: String,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u64,
    /// 供应商特定的采样参数（top_p / penalty / stop…）；空对象表示不发送。
    pub sampling: serde_json::Value,
}

/// 自动上下文压缩参数（DSH `compaction-basic` 的同一套语义）。
#[derive(Debug, Clone)]
pub struct CompactionParams {
    /// false = 关掉压力压缩，只保留「供应商报上下文超限」的兜底。
    pub enabled: bool,
    /// 越过 `ctx × threshold_ratio` 触发。
    pub threshold_ratio: f64,
    /// 逐字保留的尾巴 = `ctx × retain_ratio`。
    pub retain_ratio: f64,
    /// 摘要请求的输出上限。
    pub max_tokens: u64,
}

impl Default for CompactionParams {
    fn default() -> Self {
        Self { enabled: true, threshold_ratio: 0.8, retain_ratio: 0.16, max_tokens: 8192 }
    }
}

/// 构造 rig provider 所需的全部参数（由 config.json 的 providers / roles 映射而来）。
#[derive(Debug, Clone)]
pub struct RigParams {
    pub providers: Vec<RigProviderParams>,
    /// 单一 AI 的默认模型。
    pub story: RigRoleParams,
    /// 便宜角色（pair）：只用于场景摘要压缩等派生记忆。
    pub pair: RigRoleParams,
    /// 可覆盖的提示词（已解析的最终文本；Default = 引擎内置默认）。
    pub prompts: StoryPrompts,
    /// 每个模型声明的上下文窗口，key = `"{provider_id}/{model}"`：自动压缩的阈值基数。
    /// 缺了就不做压力压缩（只留溢出兜底）。
    pub model_ctx: std::collections::HashMap<String, u64>,
    /// 自动上下文压缩参数。
    pub compaction: CompactionParams,
}

pub struct RigProvider {
    /// 全部可用供应商的客户端：存档可在其中任选模型。
    clients: std::collections::HashMap<String, openai::CompletionsClient>,
    story_provider: String,
    story_model: String,
    story_temperature: f64,
    story_max_tokens: u64,
    story_sampling: serde_json::Value,
    pair_provider: String,
    pair_model: String,
    pair_temperature: f64,
    pair_max_tokens: u64,
    pair_sampling: serde_json::Value,
    /// 玩法 / 聊天 AI 的提示词（含用户覆盖）。
    story_prompts: StoryPrompts,
    /// 每存档一条追加式会话：只追加、不重写历史，前缀逐字节稳定，
    /// 供应商据此复用 prompt / KV 缓存（#31 缓存会话）。
    conversations: std::sync::Mutex<std::collections::HashMap<String, Vec<ConvMessage>>>,
    /// 会话持久化端口：None = 只存内存（离线 / 测试默认）。
    conv_store: std::sync::Mutex<Option<std::sync::Arc<dyn octopus_engine::ConversationStore>>>,
    /// 已从持久层载入过会话的存档（懒加载去重）。
    conv_loaded: std::sync::Mutex<std::collections::HashSet<String>>,
    /// 每个模型声明的上下文窗口（key = `"{provider_id}/{model}"`）。
    model_ctx: std::collections::HashMap<String, u64>,
    /// 自动上下文压缩参数。
    compaction: CompactionParams,
    /// 每存档最近一次调用**供应商回传的真实 input token**：压力触发用它，不靠估算。
    last_input: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// 每存档学到的「字符 / token」系数：把 token 预算换算成消息量时用它。
    chars_per_token: std::sync::Mutex<std::collections::HashMap<String, f64>>,
    /// 每存档上一轮请求的逐条消息指纹：缓存前缀守门用（意外失配打 WARN）。
    last_prefix: std::sync::Mutex<std::collections::HashMap<String, Vec<u64>>>,
    /// 每存档的「停止」信号：与在途补全赛跑，唤醒即丢弃该请求。
    cancel_signals:
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Notify>>>,
}

/// 持久化记录 ↔ 内存消息互转。
fn conv_message_from(r: octopus_engine::ConvRecord) -> ConvMessage {
    ConvMessage {
        round: r.round,
        role: if r.role == "assistant" { ConvRole::Assistant } else { ConvRole::User },
        content: r.content,
    }
}

fn conv_record_of(m: &ConvMessage) -> octopus_engine::ConvRecord {
    octopus_engine::ConvRecord {
        round: m.round,
        role: match m.role {
            ConvRole::User => "user".to_string(),
            ConvRole::Assistant => "assistant".to_string(),
        },
        content: m.content.clone(),
    }
}

/// 追加式会话里的一条消息（带回合号，回合重跑时据此截断）。
#[derive(Debug, Clone)]
struct ConvMessage {
    round: u32,
    role: ConvRole,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConvRole {
    User,
    Assistant,
}

/// 意图解析失败后的自动重试次数（不含首次）：重试 = 把失败原因回喂模型重新输出。
/// 上限 2 次 = 单轮最多 3 次调用；再失败才把回合判失败（pi 式 agent 循环的 retry 上限）。
const INTENT_PARSE_RETRIES: u32 = 2;

fn build_client(
    id: &str,
    base_url: &str,
    api_key: &str,
) -> Result<openai::CompletionsClient, String> {
    openai::CompletionsClient::builder()
        .api_key(api_key.to_string())
        .base_url(base_url.to_string())
        .build()
        .map_err(|e| format!("构建 rig 客户端失败（{id} / {base_url}）：{e}"))
}

impl RigProvider {
    /// 为每个可用供应商各建一个 OpenAI 兼容 Chat Completions 客户端。
    pub fn new(p: RigParams) -> Result<Self, String> {
        if p.providers.is_empty() {
            return Err("没有可用供应商（缺 Base URL / API Key）".to_string());
        }
        let mut clients = std::collections::HashMap::new();
        for e in &p.providers {
            clients.insert(e.id.clone(), build_client(&e.id, &e.base_url, &e.api_key)?);
        }
        Ok(Self {
            clients,
            story_provider: p.story.provider_id,
            story_model: p.story.model,
            story_temperature: p.story.temperature,
            story_max_tokens: p.story.max_tokens,
            story_sampling: p.story.sampling,
            pair_provider: p.pair.provider_id,
            pair_model: p.pair.model,
            pair_temperature: p.pair.temperature,
            pair_max_tokens: p.pair.max_tokens,
            pair_sampling: p.pair.sampling,
            story_prompts: p.prompts,
            conversations: std::sync::Mutex::new(std::collections::HashMap::new()),
            conv_store: std::sync::Mutex::new(None),
            conv_loaded: std::sync::Mutex::new(std::collections::HashSet::new()),
            model_ctx: p.model_ctx,
            compaction: p.compaction,
            last_input: std::sync::Mutex::new(std::collections::HashMap::new()),
            chars_per_token: std::sync::Mutex::new(std::collections::HashMap::new()),
            last_prefix: std::sync::Mutex::new(std::collections::HashMap::new()),
            cancel_signals: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// 本回合用哪个客户端 + 模型：存档指定优先（跨供应商），否则回落角色默认。
    fn pick<'a>(
        &'a self,
        preferred: Option<&ModelRef>,
        default_provider: &str,
        default_model: &'a str,
    ) -> Result<(&'a openai::CompletionsClient, String), EngineError> {
        if let Some(m) = preferred {
            if let Some(c) = self.clients.get(&m.provider_id) {
                return Ok((c, m.model.clone()));
            }
        }
        let client = self
            .clients
            .get(default_provider)
            .or_else(|| self.clients.values().next())
            .ok_or_else(|| EngineError::Ai(format!("未找到供应商 {default_provider}")))?;
        Ok((client, default_model.to_string()))
    }
}

// 提示词分层（借鉴 SillyTavern 提示词工程的五层架构；上层覆盖下层，层间不写矛盾指令）：
//   ① 系统提示词（本 preamble，权重最高）
//   ② 采样参数（temperature / top_p / penalty…，见 RoleConfig）
//   ③ 人物设定（personas：背景 / 性格 / 对话示例）
//   ④ 世界词条（lore：关键词触发注入；constant 条目始终在）
//   ⑤ 玩家输入（turn_prompt 末尾，最直接）
// 心法：把提示词写成「给一个聪明但不了解背景的人看的指令」，指令尽量正向。
/// 采样参数 + 本存档覆盖的思考强度；结果非空才返回（空对象不发送，保留供应商默认）。
fn sampling_with_effort(
    base: &serde_json::Value,
    model: Option<&ModelRef>,
) -> Option<serde_json::Value> {
    let mut obj = base.as_object().cloned().unwrap_or_default();
    if let Some(effort) = model
        .and_then(|m| m.reasoning_effort.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        obj.insert(
            "reasoning_effort".into(),
            serde_json::Value::String(effort.to_string()),
        );
    }
    if obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(obj))
    }
}

/// 组装一轮提示词（单一 AI，引擎默认提示词）：注入相关往事与回合工具结果。
///
/// 保留这个无参入口供默认路径与测试使用；运行期走 turn_prompt_with（提示词可被配置覆盖）。
#[allow(dead_code)]
fn turn_prompt(ctx: &TurnContext) -> String {
    turn_prompt_with(ctx, &StoryPrompts::default())
}

/// 用给定（已解析）提示词组装一轮提示词。
fn turn_prompt_with(ctx: &TurnContext, prompts: &StoryPrompts) -> String {
    turn_prompt_inner(ctx, true, prompts)
}

/// 把检索到的相关往事渲染成相关往事块；为空则不注入。
///
/// 明确告诉模型这是「可能过时的历史片段」，避免把旧事当当前事实照抄。
fn memories_block(ctx: &TurnContext, template: &str) -> String {
    if ctx.memories.is_empty() {
        return String::new();
    }
    let mut items = String::new();
    for m in &ctx.memories {
        let text = m.text.trim();
        if text.is_empty() {
            continue;
        }
        items.push_str(&format!("- [第{}回合/{}] {text}\n", m.round, m.kind));
    }
    if items.is_empty() {
        return String::new();
    }
    render_block(template, &items)
}

fn turn_prompt_inner(ctx: &TurnContext, include_memories: bool, prompts: &StoryPrompts) -> String {
    let blocks = &prompts.blocks;
    // 相关往事按预算注入（#05 §3.4）。
    let memories = if include_memories {
        memories_block(ctx, &blocks.memories)
    } else {
        String::new()
    };
    let channel = match ctx.channel {
        RoundChannel::Character => "角色输入",
        RoundChannel::Meta => "元指令",
        RoundChannel::Gm => "导演指令（人代替 GM 推进剧情）",
    };
    // 导演已裁定的事实：最高优先级，AI 不得推翻
    let canon = if ctx.canon.is_empty() {
        String::new()
    } else {
        let mut items = String::new();
        for c in &ctx.canon {
            items.push_str(&format!("- {c}\n"));
        }
        render_block(&blocks.canon, &items)
    };
    // #04 ⑦ 回合内续轮：只回喂模型自己刚触发的 query_world / check / interact 结果，
    // 不重发世界全量。首轮该字段为空 → 提示词与单轮路径逐字一致。
    let turn_feedback = if include_memories && !ctx.turn_feedback.is_empty() {
        let mut items = String::new();
        for f in &ctx.turn_feedback {
            items.push_str(&format!("- {f}\n"));
        }
        render_block(&blocks.turn_feedback, &items)
    } else {
        String::new()
    };
    // 当前任务（含骨架目标与导演新增）
    let shown: Vec<&octopus_types::QuestView> =
        ctx.quests.iter().filter(|q| !q.hidden).take(20).collect();
    let quests = if shown.is_empty() {
        String::new()
    } else {
        let mut items = String::new();
        for q in shown {
            items.push_str(&format!(
                "- [{}] {}{}\n",
                if q.done { "x" } else { " " },
                q.text,
                if q.primary { "（主线）" } else { "" }
            ));
        }
        render_block(&blocks.quests, &items)
    };
    // 可推进的场景清单：不给合法 id，AI 用 advance_scene 只能瞎猜目标。
    let scenes = if ctx.scenes.is_empty() {
        String::new()
    } else {
        let mut items = String::new();
        for s in &ctx.scenes {
            items.push_str(&format!(
                "- {}{}（{}）{}\n",
                if s.chapter.is_empty() {
                    String::new()
                } else {
                    format!("{} · ", s.chapter)
                },
                s.title,
                s.id,
                if s.id == ctx.scene_id { " ← 当前" } else { "" }
            ));
        }
        render_block(&blocks.scenes, &items)
    };
    // 当前遭遇（结构化敌人）
    let encounters = if ctx.encounters.iter().any(|e| e.active) {
        let mut items = String::new();
        for e in ctx.encounters.iter().filter(|e| e.active).take(3) {
            items.push_str(&format!(
                "- {}{}\n",
                e.name,
                e.note
                    .as_ref()
                    .map(|n| format!("（{n}）"))
                    .unwrap_or_default()
            ));
            for en in &e.enemies {
                // 数据卡摘要（图鉴 M2 §4.6）：名字 / HP / AC / 可用攻击（技能名 + 伤害骰）。
                // AI 不必再瞎编怪物能干什么；临时敌人没有图鉴攻击 → 这一段不出现。
                let attacks = if en.attacks.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> = en
                        .attacks
                        .iter()
                        .map(|a| {
                            let damage = a.damage.trim();
                            if damage.is_empty() {
                                a.name.clone()
                            } else {
                                format!("{} {}", a.name, damage)
                            }
                        })
                        .collect();
                    format!("（可用攻击：{}）", list.join("、"))
                };
                items.push_str(&format!(
                    "  · {} {} HP {}/{} AC {}{}\n",
                    en.id, en.name, en.hp, en.max, en.ac, attacks
                ));
            }
        }
        render_block(&blocks.encounters, &items)
    } else {
        String::new()
    };
    // 导演模式专属说明
    let gm = if ctx.channel == RoundChannel::Gm {
        blocks.gm.clone()
    } else {
        String::new()
    };
    let chars = if ctx.characters.is_empty() {
        "（无）".to_string()
    } else {
        ctx.characters
            .iter()
            .map(|c| format!("{}({})", c.name, c.id))
            .collect::<Vec<_>>()
            .join("、")
    };
    let controlled = if ctx.controlled.is_empty() {
        "（未指定）"
    } else {
        ctx.controlled.as_str()
    };
    // 在场人物的人格档案：单一 AI 据此扮演并保持人物一致。
    // 对话示例是 few-shot 风格样板——模仿语气句式，不照抄台词。
    let visible_personas: Vec<&octopus_engine::PersonaView> = ctx.personas.iter().collect();
    let personas = if visible_personas.is_empty() {
        String::new()
    } else {
        let mut items = String::new();
        for p in visible_personas {
            items.push_str(&format!("▸ {}（{}）\n", p.name, p.id));
            for (label, val) in [
                ("背景", &p.background),
                ("性格", &p.personality),
                ("外观", &p.appearance),
            ] {
                let v = val.trim();
                if !v.is_empty() {
                    items.push_str(&format!("  {label}：{v}\n"));
                }
            }
            let ex = p.example_dialogues.trim();
            if !ex.is_empty() {
                items.push_str(&format!("  对话示例（模仿语气，勿照抄）：\n{ex}\n"));
            }
        }
        render_block(&blocks.personas, &items)
    };
    // 世界词条：关键词命中的背景设定（引擎已按预算裁剪）。
    let lore = if ctx.lore.is_empty() {
        String::new()
    } else {
        let mut items = String::new();
        for l in &ctx.lore {
            let title = l.title.trim();
            let content = l.content.trim();
            if title.is_empty() {
                items.push_str(&format!("- {content}\n"));
            } else {
                items.push_str(&format!("- {title}：{content}\n"));
            }
        }
        render_block(&blocks.lore, &items)
    };
    let mut scene = if ctx.scene_title.is_empty() {
        "（未命名场景）".to_string()
    } else {
        ctx.scene_title.clone()
    };
    // 当前地点（地图 P2 / 设计 §6.6）：紧跟场景标题，形如
    // 「场景：碎星酒馆的夜晚（地点：碎星酒馆）」；场景未声明地点时这一段整体不出现
    // （老故事书的提示词逐字不变）。
    //
    // 位置是关键：地点逐回合会变，只能进**回合用户提示词**（这个 turn 骨架），
    // 绝不进系统 preamble——否则会击穿前缀缓存（AGENTS.md 上下文缓存不变量）。
    if let Some(loc) = ctx.location.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        scene.push_str(&format!("（地点：{loc}）"));
    }
    if let Some(desc) = ctx
        .scene_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        scene.push_str("\n场景描述：");
        scene.push_str(desc);
    }
    let focus = if ctx.focus.is_empty() {
        String::new()
    } else {
        let mut items = String::new();
        for f in &ctx.focus {
            items.push_str(&format!(
                "- {}「{}」({})\n```json\n{}\n```\n",
                f.kind,
                f.name,
                f.id.as_deref().unwrap_or("-"),
                serde_json::to_string_pretty(&f.entity).unwrap_or_else(|_| "{}".to_string())
            ));
        }
        render_block(&blocks.focus, &items)
    };
    // 单一 AI：叙述段不再按角色筛 scope，全部注入（story / character / both / character:<id> 一视同仁）。
    let sections = |slot: &str| -> String {
        let mut b = String::new();
        for s in ctx.narrative.iter().filter(|s| s.slot == slot) {
            b.push_str(&s.text);
            b.push('\n');
        }
        b
    };
    let mut world = String::new();
    if let Some(p) = ctx
        .premise
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        world.push_str(&render(&blocks.world_premise, &[("premise", p)]));
    }
    let world_sections = sections("world");
    if !world_sections.is_empty() {
        world.push_str(&render_block(&blocks.world_sections, &world_sections));
    }
    let mut directives = String::new();
    let directive_items: String = ctx
        .narrative
        .iter()
        .filter(|s| s.slot == "style" || s.slot == "behavior")
        .map(|s| format!("{}\n", s.text))
        .collect();
    if !directive_items.is_empty() {
        directives.push_str(&render_block(&blocks.directives, &directive_items));
    }
    let closing_sections = sections("closing");
    let closing = if closing_sections.is_empty() {
        String::new()
    } else {
        render_block(&blocks.closing, &closing_sections)
    };
    // C：把故事书声明的判定属性 key 明给模型，避免它拿英文别名瞎猜（如 dexterity）。
    let attributes = if ctx.attributes.is_empty() {
        String::new()
    } else {
        render_block(&blocks.attributes, &ctx.attributes.join("、"))
    };
    render(
        &prompts.turn_template,
        &[
            ("round", &ctx.round.to_string()),
            ("world", &world),
            ("scene", &scene),
            ("memories", &memories),
            ("controlled", controlled),
            ("chars", &chars),
            ("attributes", &attributes),
            ("personas", &personas),
            ("lore", &lore),
            ("channel", channel),
            ("canon", &canon),
            ("quests", &quests),
            ("scenes", &scenes),
            ("encounters", &encounters),
            ("directives", &directives),
            ("turn_feedback", &turn_feedback),
            ("text", &ctx.player_text),
            ("closing", &closing),
            ("focus", &focus),
            ("gm", &gm),
        ],
    )
}

/// 一次原生工具调用（意图工具）：name = 意图 type，arguments = 意图字段。
struct ToolCallExtract {
    name: String,
    arguments: serde_json::Value,
}

/// 从 rig 响应里抽出正文、思考链文本与工具调用。
fn split_response(choice: &[AssistantContent]) -> (String, String, Vec<ToolCallExtract>) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tools = Vec::new();
    for c in choice {
        match c {
            AssistantContent::Text(t) => text.push_str(&t.text),
            AssistantContent::Reasoning(r) => {
                for block in &r.content {
                    match block {
                        ReasoningContent::Text { text, .. } => reasoning.push_str(text),
                        ReasoningContent::Summary(s) => reasoning.push_str(s),
                        _ => {}
                    }
                }
            }
            AssistantContent::ToolCall(tc) => {
                tools.push(ToolCallExtract {
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                });
            }
            _ => {}
        }
    }
    (text, reasoning, tools)
}

/// 把一次原生工具调用反序列化为意图包络：工具名 = 意图 type，参数 = 意图字段。
/// 与文本协议的 `parse_intent_envelopes` 同构（包络携带可选 intent_id）。
fn intent_from_tool_call(name: &str, arguments: &serde_json::Value) -> Result<IntentEnvelope, String> {
    let mut value = arguments.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.insert("type".to_string(), serde_json::Value::String(name.to_string()));
    }
    serde_json::from_value::<IntentEnvelope>(value)
        .map_err(|e| format!("意图工具 {name} 参数无法解析为合法意图：{e}"))
}

/// 把工具调用渲染成可读文本（**仅供日志 / 轨迹展示**，绝不写进模型会话）。
fn tool_call_text(name: &str, arguments: &serde_json::Value) -> String {
    format!("[工具调用] {name} {arguments}")
}

/// 原生工具调用 → 会话里记录的文本：**规范的意图 JSON 数组**。
///
/// 为什么不能记展示格式（`[工具调用] narrate {…}`）：模型把自己过去的输出当范本模仿，
/// 下一轮就直接**用文字写出** `[工具调用] …`——那不是合法 JSON，协议解析在第二个字符就崩
/// （`expected value at line 1 column 2`），于是回喂纠正消息重试，来回烧上下文（实测
/// save sv-23ae… 的 round 82/83 就栽在这里：模型 reasoning 里明说「instead of [工具调用]」）。
/// 记成规范意图 JSON 还顺带兜底：模型真要「用文字给意图」，那份文本本身就能被解析出意图。
fn intent_json_text(tool_calls: &[ToolCallExtract]) -> String {
    let items: Vec<serde_json::Value> = tool_calls
        .iter()
        .map(|tc| {
            let mut v = tc.arguments.clone();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("type".to_string(), serde_json::Value::String(tc.name.clone()));
            }
            v
        })
        .collect();
    serde_json::to_string(&items).unwrap_or_default()
}

/// 老版本把原生工具调用记成了展示格式：读回会话时就地修成规范意图 JSON（一次性自愈）。
/// 只有**整段都是**该格式时才改写，其它文本一律原样返回 None（不误伤）。
fn heal_legacy_tool_calls(content: &str) -> Option<String> {
    let mut items = Vec::new();
    for line in content.lines() {
        let rest = line.trim().strip_prefix("[工具调用] ")?;
        let (name, args) = rest.split_once(' ')?;
        let mut v: serde_json::Value = serde_json::from_str(args).ok()?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("type".to_string(), serde_json::Value::String(name.to_string()));
        }
        items.push(v);
    }
    if items.is_empty() {
        return None;
    }
    serde_json::to_string(&items).ok()
}

/// 把 rig Message 折叠成日志可读的 (role, content)：纯文本直出，工具调用 / 附件
/// 折叠成一行摘要。游玩主线不用 tools，历史里基本只有 Text；兜底保证不 panic。
fn message_to_trace(m: &Message) -> AiCallMessage {
    fn user_part(c: &UserContent) -> String {
        match c {
            rig::completion::message::UserContent::Text(t) => t.text.clone(),
            _ => "[非文本用户内容]".to_string(),
        }
    }
    fn assistant_part(c: &AssistantContent) -> String {
        match c {
            AssistantContent::Text(t) => t.text.clone(),
            AssistantContent::Reasoning(_) => String::new(),
            AssistantContent::ToolCall(tc) => {
                tool_call_text(&tc.function.name, &tc.function.arguments)
            }
            _ => "[非文本内容]".to_string(),
        }
    }
    let (role, content) = match m {
        Message::System { content } => ("system".to_string(), content.clone()),
        Message::User { content } => (
            "user".to_string(),
            content.iter().map(user_part).collect::<Vec<_>>().join("
"),
        ),
        Message::Assistant { content, .. } => (
            "assistant".to_string(),
            content.iter().map(assistant_part).collect::<Vec<_>>().join("
"),
        ),
    };
    AiCallMessage { role, content }
}

impl RigProvider {
    /// 懒加载：首次看到该存档时从持久层读回会话（重启后仍能维持缓存前缀）。
    async fn ensure_loaded(&self, save_id: &str) {
        let store = self.conv_store.lock().ok().and_then(|g| g.clone());
        let Some(store) = store else { return };
        let need = self
            .conv_loaded
            .lock()
            .map(|s| !s.contains(save_id))
            .unwrap_or(false);
        if !need {
            return;
        }
        match store.load(save_id).await {
            Ok(recs) => {
                if let Ok(mut conv) = self.conversations.lock() {
                    let entry = conv.entry(save_id.to_string()).or_default();
                    if entry.is_empty() && !recs.is_empty() {
                        *entry = recs
                            .into_iter()
                            .map(conv_message_from)
                            // 老版本记的展示格式会教坏模型（见 heal_legacy_tool_calls）。
                            .map(|mut m| {
                                if m.role == ConvRole::Assistant {
                                    if let Some(fixed) = heal_legacy_tool_calls(&m.content) {
                                        m.content = fixed;
                                    }
                                }
                                m
                            })
                            .collect();
                    }
                }
                if let Ok(mut s) = self.conv_loaded.lock() {
                    s.insert(save_id.to_string());
                }
            }
            Err(e) => tracing::warn!(save_id = %save_id, error = %e, "读取模型会话失败，本次从空会话继续"),
        }
    }

    /// 把当前内存会话整段写回持久层（派生数据；失败只 warn）。
    async fn persist_conversation(&self, save_id: &str) {
        let store = self.conv_store.lock().ok().and_then(|g| g.clone());
        let Some(store) = store else { return };
        let records: Vec<octopus_engine::ConvRecord> = self
            .conversations
            .lock()
            .map(|conv| {
                conv.get(save_id)
                    .map(|v| v.iter().map(conv_record_of).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        if let Err(e) = store.save(save_id, &records).await {
            tracing::warn!(save_id = %save_id, error = %e, "写入模型会话失败（派生数据，忽略）");
        }
    }

    /// 该 (provider, model) 声明的上下文窗口；没声明 → 不做压力压缩（只留溢出兜底）。
    fn ctx_window_of(&self, provider_id: &str, model: &str) -> Option<u64> {
        self.model_ctx
            .get(&format!("{provider_id}/{model}"))
            .copied()
            .filter(|c| *c > 0)
    }

    /// 该存档的取消信号（按需创建）：`AiProvider::cancel` 用它唤醒在途请求。
    fn cancel_signal(&self, save_id: &str) -> std::sync::Arc<tokio::sync::Notify> {
        let mut map = self.cancel_signals.lock().expect("cancel signals poisoned");
        map.entry(save_id.to_string()).or_default().clone()
    }

    /// 该存档的「字符 / token」系数（缺省见 `DEFAULT_CHARS_PER_TOKEN`）。
    fn chars_per_token_of(&self, save_id: &str) -> f64 {
        self.chars_per_token
            .lock()
            .ok()
            .and_then(|m| m.get(save_id).copied())
            .unwrap_or(DEFAULT_CHARS_PER_TOKEN)
    }

    /// 压力触发：供应商回传的真实 input token（+ 本轮新输入的估算）越过
    /// `ctx × threshold_ratio` 就压一次。没有声明 ctx 的模型不做这件事。
    #[allow(clippy::too_many_arguments)]
    async fn compact_for_pressure(
        &self,
        client: &openai::CompletionsClient,
        provider_id: &str,
        model: &str,
        ctx: &TurnContext,
        preamble: &str,
        tool_defs: &[ToolDefinition],
        prompt: &str,
    ) -> Option<CompactionReport> {
        let window = self.ctx_window_of(provider_id, model)?;
        let threshold = (window as f64 * self.compaction.threshold_ratio) as u64;
        let observed = self
            .last_input
            .lock()
            .ok()
            .and_then(|m| m.get(&ctx.save_id).copied())
            .unwrap_or(0);
        let projected = observed
            + estimate_tokens(
                prompt.chars().count(),
                self.chars_per_token_of(&ctx.save_id),
            );
        if projected < threshold {
            return None;
        }
        let retain = (window as f64 * self.compaction.retain_ratio) as u64;
        tracing::info!(
            save_id = %ctx.save_id,
            round = ctx.round,
            model = %model,
            window,
            threshold,
            projected,
            retain,
            "上下文压力到阈值：压缩模型会话（只重写派生 surface）"
        );
        match self
            .compact_region(client, model, ctx, preamble, tool_defs, "pressure", retain)
            .await
        {
            Ok(report) => report,
            Err(e) => {
                tracing::warn!(save_id = %ctx.save_id, error = %e, "压力压缩失败（本轮照常继续）");
                None
            }
        }
    }

    /// 自动上下文压缩：把最旧的一段原文换成一条摘要，**只重写派生 surface**。
    ///
    /// 摘要请求逐字重放被遮蔽的原文 + 末尾追加压缩指令 = 上一次请求的真前缀，
    /// 供应商的热缓存直接复用，只有指令与输出未命中（DSH summarizer 的同款做法）。
    /// 返回 None = 没得压 / 摘要没通过收缩校验（调用方照常走原路径）。
    #[allow(clippy::too_many_arguments)]
    async fn compact_region(
        &self,
        client: &openai::CompletionsClient,
        model: &str,
        ctx: &TurnContext,
        preamble: &str,
        tool_defs: &[ToolDefinition],
        trigger: &str,
        retain_tokens: u64,
    ) -> Result<Option<CompactionReport>, EngineError> {
        let cpt = self.chars_per_token_of(&ctx.save_id);
        // 溢出兜底（retain_tokens = 0）也要留最近两个回合，别把接续的上下文压没。
        let min_rounds = if retain_tokens == 0 { 2 } else { 1 };
        // 1) 只读选区：够不够压、压哪一段。
        let (shadowed, cut, chars_before, rounds) = {
            let conv = self.conversations.lock().expect("conversations poisoned");
            let entry = conv.get(&ctx.save_id).cloned().unwrap_or_default();
            let Some(cut) = select_compaction_range(&entry, cpt, retain_tokens, min_rounds) else {
                return Ok(None);
            };
            let shadowed: Vec<ConvMessage> = entry[..cut].to_vec();
            if shadowed.is_empty() {
                return Ok(None);
            }
            let chars_before: usize = shadowed.iter().map(|m| m.content.chars().count()).sum();
            let rounds = {
                let mut seen: Vec<u32> = Vec::new();
                for m in &shadowed {
                    if !seen.contains(&m.round) {
                        seen.push(m.round);
                    }
                }
                seen.len() as u32
            };
            (shadowed, cut, chars_before, rounds)
        };
        // 2) 摘要请求：重放被遮蔽的原文 + 末尾压缩指令（热前缀复用）。
        let mut history: Vec<Message> = shadowed.iter().map(conv_to_rig_message).collect();
        history.push(Message::user(self.story_prompts.compaction.clone()));
        let request = CompletionRequest {
            model: None,
            preamble: Some(preamble.to_string()),
            chat_history: history,
            documents: Vec::new(),
            tools: tool_defs.to_vec(),
            temperature: Some(0.3),
            max_tokens: Some(self.compaction.max_tokens),
            tool_choice: None,
            additional_params: None,
            output_schema: None,
            record_telemetry_content: false,
        };
        let started = std::time::Instant::now();
        let response = client
            .completion_model(model.to_string())
            .completion(request)
            .await
            .map_err(|e| EngineError::Ai(e.to_string()))?;
        let (text, _, _) = split_response(&response.choice);
        let summary = text.trim().to_string();
        // 3) 收缩校验：摘要必须比原文短，否则当失败（DSH 的同款 gate）。
        let after = summary.chars().count();
        if after == 0 || after >= chars_before {
            tracing::warn!(
                save_id = %ctx.save_id,
                before = chars_before,
                after,
                "摘要没有比原文更短，放弃本次压缩"
            );
            return Ok(None);
        }
        // 4) 就地替换：一条摘要 + 原样保留的尾巴。
        let last_shadowed_round = shadowed.last().map(|m| m.round).unwrap_or(0);
        let framed = frame_summary(&summary);
        {
            let mut conv = self.conversations.lock().expect("conversations poisoned");
            if let Some(entry) = conv.get_mut(&ctx.save_id) {
                if cut <= entry.len() {
                    let mut rebuilt: Vec<ConvMessage> = Vec::with_capacity(entry.len() - cut + 1);
                    rebuilt.push(ConvMessage {
                        round: last_shadowed_round,
                        role: ConvRole::User,
                        content: framed,
                    });
                    rebuilt.extend(entry[cut..].iter().cloned());
                    *entry = rebuilt;
                }
            }
        }
        self.persist_conversation(&ctx.save_id).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        tracing::info!(
            save_id = %ctx.save_id,
            round = ctx.round,
            model = %model,
            trigger,
            shadowed_rounds = rounds,
            chars_before,
            chars_after = after,
            input = response.usage.input_tokens,
            cached = response.usage.cached_input_tokens,
            latency_ms,
            "上下文压缩完成（只重写派生 surface，权威日志不动）"
        );
        Ok(Some(CompactionReport {
            trigger: trigger.to_string(),
            shadowed_rounds: rounds,
            chars_before: chars_before as u64,
            chars_after: after as u64,
            usage: octopus_types::AiCallUsage {
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                total_tokens: response.usage.total_tokens,
                cached_input_tokens: response.usage.cached_input_tokens,
                cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
                ..Default::default()
            },
            latency_ms,
        }))
    }
    /// 单次补全：拿意图，并把供应商返回的思考链一并带出（供前端「思考」折叠块）。
    ///
    /// preamble 与 parse 都由故事书声明的协议适配器决定（叙事契约 P2）：
    /// 缺省协议 = 引擎内置文本 + `parse_intents`，与旧行为逐字一致。
    ///
    /// 同时采集本次调用的完整轨迹（pi 式 span）：发给模型的完整上下文 + 思考链 +
    /// 用量 + 延迟 + 状态，随 AiOutput.trace 交给 Session 落 ai_call 事件，供日志复盘。
    ///
    /// **自动重试（pi 式 agent 循环的 retry）**：协议解析失败时，把失败原因作为一条
    /// 纠正消息回喂给模型重新输出（失败轮成对保留在会话里，模型能看到自己的上轮输出
    /// 为何不被接受），最多 `INTENT_PARSE_RETRIES` 次；尝试次数记入 `AiCallPayload.attempts`。
    async fn complete(
        &self,
        client: &openai::CompletionsClient,
        model: String,
        ctx: &TurnContext,
        temperature: f64,
        max_tokens: u64,
        sampling: Option<serde_json::Value>,
        prompt: String,
    ) -> Result<AiOutput, EngineError> {
        let spec = ctx.protocol.clone().unwrap_or_default();
        let adapter = build_protocol_adapter(&spec);
        // 首次看到该存档时从持久层读回会话（重启后仍能维持缓存前缀）。
        self.ensure_loaded(&ctx.save_id).await;
        // 该协议是否启用「原生工具调用」（意图 = 工具）：default / declarative 走工具，
        // lua 协议保持文本。工具模式下模型通过调用意图工具输出意图，parse 退居兜底。
        let tool_specs = adapter.tools();
        let tool_mode = tool_specs.is_some();
        // 整轮开始 / 重跑时截断到本回合之前；续轮 / 重试都保留本回合已产生的消息。
        let fresh_round = ctx.turn_feedback.is_empty();
        // provider id：存档覆盖的模型优先（与 pick 同一口径），否则角色默认供应商。
        let provider_id = ctx
            .model
            .as_ref()
            .filter(|m| self.clients.contains_key(&m.provider_id))
            .map(|m| m.provider_id.clone())
            .unwrap_or_else(|| self.story_provider.clone());
        let rig_model = client.completion_model(model.clone());

        // 解析失败重试循环（pi：failToolCallsFromTruncatedMessage 的「错误回喂重发」思路）。
        // 首次尝试按整轮语义截断；重试时失败轮已 record_round 进会话，fresh=false 保留它，
        // 模型在下一次请求里能看到自己上一轮的输出与纠正指令。
        let mut attempts = 0u32;
        let mut current_prompt = prompt;
        // 本次调用前做过一次自动压缩时的记账（随 AiOutput 交给引擎落日志）。
        let mut compaction_report: Option<CompactionReport> = None;
        // 上下文超限只兜底一次，避免「压了还超 → 再压」的死循环。
        let mut overflow_recovered = false;
        // 本轮前缀被改写的可归因原因（回合重跑；压缩另有 compaction_report 标记）。
        let mut prefix_rewrite_reason: Option<&str> = None;
        // 原生工具调用是否已降级为文本协议（供应商不支持 tools 字段时自动回退）。
        let mut degraded = false;
        loop {
            attempts += 1;
            // 工具模式（未降级）：把「意图工具」交给模型原生调用；文本协议则要求输出 JSON。
            let tool_active = tool_mode && !degraded;
            let tool_defs: Option<Vec<ToolDefinition>> = if tool_active {
                tool_specs.as_ref().map(|specs|
                    specs
                        .iter()
                        .map(|s| ToolDefinition {
                            name: s.name.clone(),
                            description: s.description.clone(),
                            parameters: s.parameters.clone(),
                        })
                        .collect(),
                )
            } else {
                None
            };
            // 系统提示词优先级：故事书显式声明了协议（declarative / lua）时以故事书为准；
            // 默认协议下用配置里的覆盖文本（未覆盖 = 引擎内置默认）。
            let preamble = if spec.mode == ProtocolMode::Default {
                if tool_active {
                    self.story_prompts.protocol_tool.clone()
                } else {
                    self.story_prompts.protocol_text.clone()
                }
            } else if tool_active {
                adapter.tool_preamble(ctx).unwrap_or_else(|| adapter.preamble(ctx))
            } else {
                adapter.preamble(ctx)
            };
            // 自动上下文压缩（DSH 的 pressure 触发）：在**请求派生之前**看一眼用量。
            // 只在整轮首次尝试时做一次；压缩只重写派生 surface，权威日志不动。
            if self.compaction.enabled
                && fresh_round
                && attempts == 1
                && compaction_report.is_none()
            {
                compaction_report = self
                    .compact_for_pressure(
                        client,
                        &provider_id,
                        &model,
                        ctx,
                        &preamble,
                        tool_defs.as_deref().unwrap_or(&[]),
                        &current_prompt,
                    )
                    .await;
            }
            // 追加式会话：取出本存档历史（首次尝试按整轮语义截断），再把这次的
            // user 提示词追加为最后一条——前缀稳定，缓存才命中。
            let chat_history = {
                let mut conv = self.conversations.lock().expect("conversations poisoned");
                let entry = conv.entry(ctx.save_id.clone()).or_default();
                prefix_rewrite_reason = None;
                // 回合重跑会丢掉本回合已记录的消息 → 前缀必然重建（可归因，不是回归）。
                if fresh_round && attempts == 1 && entry.iter().any(|m| m.round == ctx.round) {
                    prefix_rewrite_reason = Some("turn_rerun");
                }
                append_round_history(
                    entry,
                    ctx.round,
                    fresh_round && attempts == 1,
                    &current_prompt,
                )
            };
            let request = CompletionRequest {
                model: None,
                preamble: Some(preamble.clone()),
                chat_history,
                documents: Vec::new(),
                // clone：溢出兜底压缩要再用一次同一份工具定义（保持前缀对齐）。
                tools: tool_defs.clone().unwrap_or_default(),
                temperature: Some(temperature),
                max_tokens: Some(max_tokens),
                // None = 供应商默认（OpenAI 兼容端点为 auto）：允许但引导模型优先调用工具。
                tool_choice: None,
                additional_params: sampling.clone(),
                output_schema: None,
                record_telemetry_content: false,
            };
            // 轨迹用的请求上下文：system 提示词 + 会话历史（含本次输入）。必须在 request move 前取。
            let mut trace_messages: Vec<AiCallMessage> =
                Vec::with_capacity(request.chat_history.len() + 1);
            trace_messages.push(AiCallMessage {
                role: "system".into(),
                content: preamble.clone(),
            });
            for m in &request.chat_history {
                trace_messages.push(message_to_trace(m));
            }

            // ── 缓存前缀守门 ──
            // 供应商的上下文缓存是**前缀缓存**：只有「这一轮的请求序列是上一轮的前缀」才可能整体
            // 命中。这里逐条比对，把「命中率为什么低」变成可归因的日志：
            // 压缩 / 回合重跑导致的重建 = INFO（预期），其它任何改写 = WARN（回归）。
            let digests: Vec<u64> = trace_messages
                .iter()
                .map(|m| message_digest(&m.role, &m.content))
                .collect();
            let prev_prefix = self
                .last_prefix
                .lock()
                .ok()
                .and_then(|m| m.get(&ctx.save_id).cloned());
            if let Some(prev) = prev_prefix {
                let common = common_prefix_len(&prev, &digests);
                if common < prev.len() {
                    let reason = if compaction_report.is_some() {
                        Some("compaction")
                    } else {
                        prefix_rewrite_reason
                    };
                    match reason {
                        Some(r) => tracing::info!(
                            save_id = %ctx.save_id,
                            round = ctx.round,
                            reason = r,
                            common,
                            prev_len = prev.len(),
                            "上下文前缀重建（预期）：供应商缓存需要重新预热"
                        ),
                        None => tracing::warn!(
                            save_id = %ctx.save_id,
                            round = ctx.round,
                            common,
                            prev_len = prev.len(),
                            first_changed = %trace_messages
                                .get(common)
                                .map(|m| m.role.as_str())
                                .unwrap_or("?"),
                            "缓存前缀意外失配：这一轮的请求不是上一轮的前缀（整段历史将按原价重算）"
                        ),
                    }
                }
            }
            if let Ok(mut m) = self.last_prefix.lock() {
                m.insert(ctx.save_id.clone(), digests);
            }
            let started = std::time::Instant::now();
            // 玩家可以随时按停止：与在途请求赛跑，取消赢了就直接丢弃这个请求
            //（reqwest 的 future 被 drop 即中断连接，不会继续烧 token）。
            let cancel = self.cancel_signal(&ctx.save_id);
            let cancelled = cancel.notified();
            tokio::pin!(cancelled);
            let response = match tokio::select! {
                biased;
                _ = &mut cancelled => {
                    tracing::info!(save_id = %ctx.save_id, round = ctx.round, "在途 AI 调用被玩家取消");
                    return Err(EngineError::Cancelled);
                }
                r = rig_model.completion(request) => r,
            } {
                Ok(r) => r,
                Err(e) => {
                    let latency_ms = started.elapsed().as_millis() as u64;
                    tracing::warn!(
                        save_id = %ctx.save_id,
                        round = ctx.round,
                        provider = %provider_id,
                        model = %model,
                        latency_ms,
                        error = %e,
                        "AI 调用失败"
                    );
                    // 原生工具调用被供应商拒绝（如不支持 tools 字段）→ 降级为文本协议重试一次。
                    if tool_mode && !degraded {
                        tracing::warn!(
                            save_id = %ctx.save_id,
                            round = ctx.round,
                            provider = %provider_id,
                            model = %model,
                            "原生工具调用不可用，降级为文本协议"
                        );
                        degraded = true;
                        continue;
                    }
                    // 供应商确认上下文超限：更激进地压一次再重试该请求
                    //（DSH 的 context-overflow 恢复；权威日志不动）。
                    if self.compaction.enabled
                        && !overflow_recovered
                        && is_context_overflow(&e.to_string())
                    {
                        overflow_recovered = true;
                        match self
                            .compact_region(
                                client,
                                &model,
                                ctx,
                                &preamble,
                                tool_defs.as_deref().unwrap_or(&[]),
                                "context_overflow",
                                0,
                            )
                            .await
                        {
                            Ok(Some(report)) => {
                                tracing::info!(
                                    save_id = %ctx.save_id,
                                    round = ctx.round,
                                    model = %model,
                                    shadowed_rounds = report.shadowed_rounds,
                                    "上下文超限：压缩后重试本轮"
                                );
                                compaction_report = Some(report);
                                continue;
                            }
                            Ok(None) => tracing::warn!(
                                save_id = %ctx.save_id,
                                "上下文超限，但没有可安全压缩的范围（历史本身就是不可分单元）"
                            ),
                            Err(ce) => tracing::warn!(
                                save_id = %ctx.save_id,
                                error = %ce,
                                "上下文超限后的兜底压缩失败"
                            ),
                        }
                    }
                    return Err(EngineError::Ai(e.to_string()));
                }
            };
            let latency_ms = started.elapsed().as_millis() as u64;
            let (text, reasoning, tool_calls) = split_response(&response.choice);
            // 用量遥测：cached 是检验「缓存是否吃满」的关键指标（供应商不回传时为 0）。
            tracing::info!(
                save_id = %ctx.save_id,
                round = ctx.round,
                provider = %provider_id,
                model = %model,
                attempt = attempts,
                tool_active,
                tool_calls = tool_calls.len(),
                input = response.usage.input_tokens,
                output = response.usage.output_tokens,
                cached = response.usage.cached_input_tokens,
                cache_write = response.usage.cache_creation_input_tokens,
                latency_ms,
                "AI 调用用量"
            );
            // 记账：真实 input token（压力触发的依据）+ 该存档的「字符 / token」系数
            //（把 token 预算换算成消息量时用它；首次用保守缺省）。
            if response.usage.input_tokens > 0 {
                if let Ok(mut m) = self.last_input.lock() {
                    m.insert(ctx.save_id.clone(), response.usage.input_tokens);
                }
                let chars: usize = trace_messages
                    .iter()
                    .map(|m| m.content.chars().count())
                    .sum();
                let cpt = chars as f64 / response.usage.input_tokens as f64;
                if cpt.is_finite() && (0.5..=8.0).contains(&cpt) {
                    if let Ok(mut m) = self.chars_per_token.lock() {
                        m.insert(ctx.save_id.clone(), cpt);
                    }
                }
            }
            // 把这次的 user 提示词与模型输出追加进会话（失败轮也成对保留——pi transcript
            // 思路：模型下一轮能看到自己上轮的输出与纠正指令）。
            // 工具调用记成**规范意图 JSON**，不记展示格式（见 intent_json_text 的说明）。
            let assistant_text = if tool_active && !tool_calls.is_empty() {
                intent_json_text(&tool_calls)
            } else {
                text.clone()
            };
            if let Ok(mut conv) = self.conversations.lock() {
                let entry = conv.entry(ctx.save_id.clone()).or_default();
                record_round(entry, ctx.round, current_prompt.clone(), assistant_text);
            }
            // 产出意图：工具模式优先取原生工具调用（每个工具调用 = 一个意图）；
            // 模型没调工具只输出文本时，走协议 parse 兜底。任一失败 → 回喂纠正重试。
            let parsed: Result<Vec<IntentEnvelope>, String> =
                if tool_active && !tool_calls.is_empty() {
                    let mut out = Vec::with_capacity(tool_calls.len());
                    let mut first_err: Option<String> = None;
                    for tc in &tool_calls {
                        match intent_from_tool_call(&tc.name, &tc.arguments) {
                            Ok(envelope) => out.push(envelope),
                            Err(e) => {
                                first_err = Some(e);
                                break;
                            },
                        }
                    }
                    match first_err {
                        Some(e) => Err(e),
                        None => Ok(out),
                    }
                } else {
                    adapter.parse(&text, ctx).map_err(|e| e.to_string())
                };
            match parsed {
                Ok(raw_intents) => {
                    let intents = adapter.normalize(raw_intents, ctx);
                    let intent_warnings = adapter.take_warnings();
                    self.persist_conversation(&ctx.save_id).await;
                    // 轨迹（pi 式 span 的结束属性）：意图名摘要 + 用量 + 延迟 + 状态 + 尝试次数。
                    let intent_names: Vec<String> = intents
                        .iter()
                        .map(|en| octopus_engine::intent_kind(&en.intent).to_string())
                        .collect();
                    let trace = AiCallPayload {
                        stage: "story_thinking".to_string(),
                        provider: provider_id,
                        model,
                        temperature,
                        max_tokens,
                        messages: trace_messages,
                        reasoning: (!reasoning.trim().is_empty()).then(|| reasoning.clone()),
                        usage: AiCallUsage {
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            total_tokens: response.usage.total_tokens,
                            cached_input_tokens: response.usage.cached_input_tokens,
                            cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
                        },
                        latency_ms,
                        attempts,
                        status: AiCallStatus::Ok,
                        error: None,
                        intents: intent_names,
                        warnings: intent_warnings.clone(),
                    };
                    return Ok(AiOutput {
                        intents,
                        reasoning: (!reasoning.trim().is_empty()).then_some(reasoning),
                        intent_warnings,
                        trace: Some(trace),
                        compaction: compaction_report.take(),
                    });
                }
                Err(e) => {
                    if attempts > INTENT_PARSE_RETRIES {
                        tracing::warn!(
                            save_id = %ctx.save_id,
                            round = ctx.round,
                            provider = %provider_id,
                            model = %model,
                            attempt = attempts,
                            error = %e,
                            "意图产出失败已达重试上限"
                        );
                        self.persist_conversation(&ctx.save_id).await;
                        return Err(EngineError::Ai(e));
                    }
                    tracing::warn!(
                        save_id = %ctx.save_id,
                        round = ctx.round,
                        provider = %provider_id,
                        model = %model,
                        attempt = attempts,
                        error = %e,
                        "意图产出失败，回喂纠正消息重试"
                    );
                    // 纠正消息：明确失败原因与要求，让模型忽略上一条重新输出（pi retry 思路）。
                    let err = e.to_string();
                    current_prompt = if tool_active {
                        render(&self.story_prompts.retry_tool, &[("error", &err)])
                    } else {
                        render(&self.story_prompts.retry_text, &[("error", &err)])
                    };
                }
            }
        }
    }
}

/// 组装本次请求的会话历史：整轮开始 / 重跑时截断到本回合之前，再追加本次 user 提示词。
///
/// 只追加、不重写既有消息，因此「system, u1, a1, ..., uN」的前缀逐字节稳定，
/// 供应商可整段复用缓存；round 用于回合重跑（重跑同一回合会先丢弃该回合的旧消息）。
fn append_round_history(
    entry: &mut Vec<ConvMessage>,
    round: u32,
    fresh_round: bool,
    prompt: &str,
) -> Vec<Message> {
    if fresh_round {
        entry.retain(|m| m.round < round);
    }
    let mut history: Vec<Message> = entry
        .iter()
        .map(|m| match m.role {
            ConvRole::User => Message::user(m.content.clone()),
            ConvRole::Assistant => Message::assistant(m.content.clone()),
        })
        .collect();
    history.push(Message::user(prompt.to_string()));
    history
}

/// 记录一轮模型往返（user 提示词 + assistant 原文）。**只追加、永不裁剪**。
///
/// 两个理由：
/// 1. **缓存**：供应商的上下文缓存是**前缀缓存**——按请求序列的公共前缀匹配。
///    从队首删消息会让第一条之后整段失配（octopus.db 的 ai_call 事件实测：每回合
///    砍头时命中率 0.4%，纯追加时 ~97%），等于把整段历史按原价重算一遍。
/// 2. **上下文**：模型窗口是百万级，早期剧情不该被静默丢掉。真超了窗口，供应商会
///    明确报错（该回合失败并留痕），好过悄悄遗忘。
///
/// 唯一允许的历史改写是「回合重跑」：`append_round_history` 丢掉**本回合**的旧消息，
/// 那只是重写队尾，不影响此前的前缀。
fn record_round(entry: &mut Vec<ConvMessage>, round: u32, prompt: String, assistant: String) {
    entry.push(ConvMessage { round, role: ConvRole::User, content: prompt });
    entry.push(ConvMessage { round, role: ConvRole::Assistant, content: assistant });
}

// ============================================================
// 自动上下文压缩（对齐 DSH compaction-basic 的语义）
//
// 只重写**派生 surface**（模型会话 `ai_conversations` / 内存 entry），权威命令日志永不改写：
// 玩家看到的叙事、回放与存档都不受影响。触发两种：
//   ① pressure：供应商回传的真实 input token 越过 `ctx × threshold_ratio`；
//   ② context_overflow：供应商报上下文超限 → 更激进地压一次 + 重试该请求。
// 摘要请求复用热前缀（原系统提示词 + 被遮蔽的原文 + 末尾指令），只有指令与输出未命中缓存。
// ============================================================

/// 缺省的「字符 / token」系数：中文叙事实测约 1.8 字符/token
///（octopus.db：83 万字符 ↔ 45 万 token）。首次调用后会被真实用量纠正。
const DEFAULT_CHARS_PER_TOKEN: f64 = 1.8;

/// 文本长度 → 估算 token。只用于把 token 预算换算成消息量；触发阈值用供应商回传的真实用量。
fn estimate_tokens(chars: usize, chars_per_token: f64) -> u64 {
    (chars as f64 / chars_per_token.max(0.2)).ceil() as u64
}

/// i 是不是「回合开头」（该回合第一条 user 消息）：压缩切点只能落在这里，
/// 免得把一回合内的续轮往返劈成两半。
fn is_round_start(entry: &[ConvMessage], i: usize) -> bool {
    match entry.get(i) {
        None => false,
        Some(m) => m.role == ConvRole::User && (i == 0 || entry[i - 1].round < m.round),
    }
}

/// 从 i 往前找最近的回合开头。
fn round_start_at_or_before(entry: &[ConvMessage], i: usize) -> Option<usize> {
    if entry.is_empty() {
        return None;
    }
    let mut j = i.min(entry.len() - 1);
    loop {
        if is_round_start(entry, j) {
            return Some(j);
        }
        if j == 0 {
            return None;
        }
        j -= 1;
    }
}

/// `entry[from..]` 里覆盖了几个回合。
fn rounds_in(entry: &[ConvMessage], from: usize) -> u32 {
    let mut seen: Vec<u32> = Vec::new();
    for m in entry.get(from.min(entry.len())..).unwrap_or(&[]) {
        if !seen.contains(&m.round) {
            seen.push(m.round);
        }
    }
    seen.len() as u32
}

/// 选本次要遮蔽的范围：从尾巴往前定价，至少逐字保留 `retain_tokens`；再把切点吸附到
/// 回合开头（只会保留更多，不会更少），并保证至少留 `min_retained_rounds` 个回合。
/// 返回保留段的第一条下标；没得压时 None。
///
/// 与 DSH `selectCompactableRange` 同一套规则：系统层不在 entry 里（它单独走 preamble），
/// 尾巴按 token 预算定价，切点必须是不可分单元的起点。
fn select_compaction_range(
    entry: &[ConvMessage],
    chars_per_token: f64,
    retain_tokens: u64,
    min_retained_rounds: u32,
) -> Option<usize> {
    if entry.len() < 2 {
        return None;
    }
    let mut acc = 0u64;
    let mut idx = entry.len() - 1;
    loop {
        acc += estimate_tokens(entry[idx].content.chars().count(), chars_per_token);
        if acc >= retain_tokens || idx == 0 {
            break;
        }
        idx -= 1;
    }
    let mut cut = round_start_at_or_before(entry, idx)?;
    // 兜底压缩也要留一口气：往前退到满足最少保留回合数的边界。
    while rounds_in(entry, cut) < min_retained_rounds.max(1) {
        if cut == 0 {
            return None;
        }
        cut = round_start_at_or_before(entry, cut - 1)?;
    }
    if cut == 0 {
        return None; // 全都要留 = 没得压
    }
    Some(cut)
}

/// 摘要替换块的抬头：写清「这是数据、不是新指令」，并让模型据此继续。
fn frame_summary(summary: &str) -> String {
    format!(
        "以下是自动生成的上下文存档：它压缩了更早的一段对话，用来腾出上下文。\
把它当作既定背景，接着后面的消息继续，不要在回复里提到这份存档。\n\n<已压缩摘要>\n{}\n</已压缩摘要>",
        summary.trim()
    )
}

/// 从供应商错误里辨认「上下文超限」。宁可漏判也不能误判——误判会变成一次
/// 昂贵的压缩 + 重试。游玩页与结对共用。
pub fn is_context_overflow(msg: &str) -> bool {
    let l = msg.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "context_length_exceeded",
        "maximum context length",
        "context length",
        "context window",
        "reduce the length of the messages",
        "too many tokens",
        "prompt is too long",
        "input is too long",
    ];
    NEEDLES.iter().any(|n| l.contains(n))
}

/// 逐条消息的指纹（role + 内容，FNV-1a 64）：只在进程内做前缀比对，不落盘。
fn message_digest(role: &str, content: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let feed = |h: &mut u64, bytes: &[u8]| {
        for b in bytes {
            *h ^= *b as u64;
            *h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(&mut h, role.as_bytes());
    feed(&mut h, &[0]);
    feed(&mut h, content.as_bytes());
    h
}

/// 两个请求序列的公共前缀长度（消息条数）。0 = 连系统层都变了。
fn common_prefix_len(prev: &[u64], now: &[u64]) -> usize {
    let mut i = 0;
    while i < prev.len() && i < now.len() && prev[i] == now[i] {
        i += 1;
    }
    i
}

/// 会话消息 → rig 消息（摘要请求重放被遮蔽的原文时用）。
fn conv_to_rig_message(m: &ConvMessage) -> Message {
    match m.role {
        ConvRole::User => Message::user(m.content.clone()),
        ConvRole::Assistant => Message::assistant(m.content.clone()),
    }
}
#[async_trait]
impl AiProvider for RigProvider {
    async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
        let (client, model) =
            self.pick(ctx.model.as_ref(), &self.story_provider, &self.story_model)?;
        self.complete(
            client,
            model,
            ctx,
            self.story_temperature,
            self.story_max_tokens,
            sampling_with_effort(&self.story_sampling, ctx.model.as_ref()),
            turn_prompt_with(ctx, &self.story_prompts),
        )
        .await
    }

    fn set_conversation_store(&self, store: std::sync::Arc<dyn octopus_engine::ConversationStore>) {
        if let Ok(mut slot) = self.conv_store.lock() {
            *slot = Some(store);
        }
    }

    /// 玩家按「停止」：唤醒该存档的在途补全（没有在途调用时是空操作）。
    fn cancel(&self, save_id: &str) {
        let signal = self
            .cancel_signals
            .lock()
            .ok()
            .and_then(|m| m.get(save_id).cloned());
        if let Some(signal) = signal {
            signal.notify_waiters();
        }
    }

    /// 丢弃某存档的会话（内存 + 持久层）；新原点 / 升级 / 导入 / 删除时由 api 层调用。
    async fn clear_conversation(&self, save_id: &str) {
        if let Ok(mut conv) = self.conversations.lock() {
            conv.remove(save_id);
        }
        if let Ok(mut s) = self.conv_loaded.lock() {
            s.remove(save_id);
        }
        if let Ok(mut m) = self.last_prefix.lock() {
            m.remove(save_id);
        }
        let store = self.conv_store.lock().ok().and_then(|g| g.clone());
        if let Some(store) = store {
            if let Err(e) = store.clear(save_id).await {
                tracing::warn!(save_id = %save_id, error = %e, "清空模型会话失败（派生数据，忽略）");
            }
        }
    }

    /// 场景摘要压缩走 **pair 角色**（最便宜的既有角色），一次纯文本补全。
    ///
    /// 失败 / 空输出都返回 Err / None，由 Session 退化为确定性拼接——摘要是派生数据，
    /// 绝不能让压缩失败影响权威回合。
    async fn summarize(&self, text: &str) -> Result<Option<String>, EngineError> {
        if text.trim().is_empty() {
            return Ok(None);
        }
        let client = self
            .clients
            .get(&self.pair_provider)
            .or_else(|| self.clients.values().next())
            .ok_or_else(|| EngineError::Ai(format!("未找到供应商 {}", self.pair_provider)))?;
        let request = CompletionRequest {
            model: None,
            preamble: Some(self.story_prompts.memory_summary.clone()),
            chat_history: vec![Message::user(text.to_string())],
            documents: Vec::new(),
            tools: Vec::new(),
            temperature: Some(self.pair_temperature),
            max_tokens: Some(self.pair_max_tokens),
            tool_choice: None,
            additional_params: sampling_with_effort(&self.pair_sampling, None),
            output_schema: None,
            record_telemetry_content: false,
        };
        let model = client.completion_model(self.pair_model.clone());
        let response = model
            .completion(request)
            .await
            .map_err(|e| EngineError::Ai(e.to_string()))?;
        let (out, _, _) = split_response(&response.choice);
        let trimmed = out.trim();
        // 空输出视同没有压缩结果，交给调用方回退拼接。
        Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use octopus_engine::parse_intents;
    use octopus_types::Intent;

    /// 原生工具调用 → 意图：工具名即意图 type，参数即字段。
    #[test]
    fn intent_from_tool_call_parses_narrate_speak_and_finish() {
        let narrate = super::intent_from_tool_call(
            "narrate",
            &serde_json::json!({ "content": "夜色沉下来。" }),
        )
        .expect("narrate");
        assert!(matches!(
            narrate.intent,
            Intent::Narrate { ref content, .. } if content == "夜色沉下来。"
        ));
        let speak = super::intent_from_tool_call(
            "speak",
            &serde_json::json!({ "content": "别走那条路。", "actor_id": "char-linas", "tone": "warn" }),
        )
        .expect("speak");
        assert!(matches!(
            speak.intent,
            Intent::Speak { ref actor_id, .. } if actor_id.as_deref() == Some("char-linas")
        ));
        let finish = super::intent_from_tool_call("finish_turn", &serde_json::json!({}))
            .expect("finish_turn");
        assert!(matches!(finish.intent, Intent::FinishTurn));
    }

    /// 工具参数非法（缺必需字段 / 未知工具）必须明确报错，交由重试循环回喂。
    #[test]
    fn intent_from_tool_call_rejects_bad_args() {
        // narrate 缺 content。
        assert!(super::intent_from_tool_call("narrate", &serde_json::json!({})).is_err());
        // 未知工具名。
        assert!(super::intent_from_tool_call("hack", &serde_json::json!({})).is_err());
        // 非对象参数。
        assert!(super::intent_from_tool_call("narrate", &serde_json::json!(42)).is_err());
    }

    /// 原生工具调用记进会话时必须落成**可解析的规范意图 JSON**，不能落展示格式——
    /// 展示格式会被模型当范本模仿成无法解析的文本（实测 round 82/83 的坑）。
    #[test]
    fn tool_calls_are_recorded_as_canonical_intent_json() {
        let calls = vec![
            super::ToolCallExtract {
                name: "narrate".into(),
                arguments: serde_json::json!({ "content": "夜色沉下来。" }),
            },
            super::ToolCallExtract {
                name: "speak".into(),
                arguments: serde_json::json!({ "content": "别走。", "actor_id": "char-isa" }),
            },
        ];
        let text = super::intent_json_text(&calls);
        assert!(
            !text.contains("工具调用"),
            "记录文本里不能出现展示格式：{text}"
        );
        let parsed = octopus_engine::parse_intents(&text).expect("记录文本必须能解析回意图");
        assert_eq!(parsed.len(), 2);
    }

    /// 老存档里已经写进去的展示格式：读回会话时自愈成规范意图 JSON，其它文本不误伤。
    #[test]
    fn legacy_tool_call_text_is_healed_into_intent_json() {
        let legacy = "[工具调用] narrate {\"content\":\"夜色沉下来。\"}\n[工具调用] speak {\"content\":\"别走。\",\"actor_id\":\"char-isa\"}";
        let healed = super::heal_legacy_tool_calls(legacy).expect("整段都是老格式时应能修复");
        let parsed = octopus_engine::parse_intents(&healed).expect("修复后必须可解析");
        assert_eq!(parsed.len(), 2);
        // 正常文本 / 混合内容一律不动。
        assert!(super::heal_legacy_tool_calls("夜色沉下来。").is_none());
        assert!(super::heal_legacy_tool_calls("先说一句\n[工具调用] narrate {}").is_none());
    }

    /// 工具调用渲染成可读文本（日志展示用）。
    #[test]
    fn tool_call_text_is_readable() {
        let t = super::tool_call_text("check", &serde_json::json!({ "attribute": "wit" }));
        assert!(t.starts_with("[工具调用] check"), "{t}");
        assert!(t.contains("wit"), "{t}");
    }

    /// 追加式会话：前缀逐字节稳定、重跑按回合截断。
    #[test]
    fn conversation_appends_and_truncates_by_round() {
        let mut entry: Vec<super::ConvMessage> = Vec::new();

        // 第 1 回合：历史里只有本次 user 提示词。
        let h = super::append_round_history(&mut entry, 1, true, "u1");
        assert_eq!(h.len(), 1);
        super::record_round(&mut entry, 1, "u1".into(), "a1".into());
        assert_eq!(entry.len(), 2);

        // 第 2 回合：前缀仍是 [u1, a1]，只追加 u2。
        let h = super::append_round_history(&mut entry, 2, true, "u2");
        assert_eq!(h.len(), 3, "第二轮请求要带上第一轮的 user + assistant");
        super::record_round(&mut entry, 2, "u2".into(), "a2".into());

        // 同一回合的续轮（turn_feedback 非空）：不截断，继续追加。
        let h = super::append_round_history(&mut entry, 2, false, "u2b");
        assert_eq!(h.len(), 5, "续轮保留本回合已产生的消息");

        // 整轮重跑（fresh_round = true，同一回合号）：丢弃该回合旧消息，从上一回合续起。
        let h = super::append_round_history(&mut entry, 2, true, "u2r");
        assert_eq!(h.len(), 3, "重跑丢弃本回合旧消息，只保留 round < 2 的前缀");
    }

    /// clear_conversation 只丢弃目标存档的会话，不影响其它存档。
    #[tokio::test]
    async fn clear_conversation_drops_only_target_save() {
        let p = super::RigProvider::new(super::RigParams {
            providers: vec![super::RigProviderParams {
                id: "p1".into(),
                base_url: "http://127.0.0.1:1".into(),
                api_key: "k".into(),
            }],
            story: test_role("p1", "m1"),
            pair: test_role("p1", "m1"),
            prompts: Default::default(),
            model_ctx: Default::default(),
            compaction: Default::default(),
        })
        .expect("构造 provider");
        let seed = || vec![super::ConvMessage { round: 1, role: super::ConvRole::User, content: "u1".into() }];
        {
            let mut conv = p.conversations.lock().unwrap();
            conv.insert("sv-a".into(), seed());
            conv.insert("sv-b".into(), seed());
        }
        octopus_engine::AiProvider::clear_conversation(&p, "sv-a").await;
        let conv = p.conversations.lock().unwrap();
        assert!(!conv.contains_key("sv-a"), "目标存档会话应被丢弃");
        assert!(conv.contains_key("sv-b"), "其它存档的会话不受影响");
    }

    /// 可注入的假持久层：验证懒加载 / 写回 / 清空三条路径。
    #[derive(Default)]
    struct FakeConvStore {
        rows: std::sync::Mutex<std::collections::HashMap<String, Vec<octopus_engine::ConvRecord>>>,
        clears: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl octopus_engine::ConversationStore for FakeConvStore {
        async fn load(
            &self,
            save_id: &str,
        ) -> Result<Vec<octopus_engine::ConvRecord>, octopus_engine::EngineError> {
            Ok(self.rows.lock().unwrap().get(save_id).cloned().unwrap_or_default())
        }
        async fn save(
            &self,
            save_id: &str,
            records: &[octopus_engine::ConvRecord],
        ) -> Result<(), octopus_engine::EngineError> {
            self.rows.lock().unwrap().insert(save_id.to_string(), records.to_vec());
            Ok(())
        }
        async fn clear(&self, save_id: &str) -> Result<(), octopus_engine::EngineError> {
            self.rows.lock().unwrap().remove(save_id);
            self.clears.lock().unwrap().push(save_id.to_string());
            Ok(())
        }
    }

    /// 会话懒加载 → 追加后写回 → 清空，三段都要落到持久层。
    #[tokio::test]
    async fn conversation_loads_persists_and_clears() {
        let p = super::RigProvider::new(super::RigParams {
            providers: vec![super::RigProviderParams {
                id: "p1".into(),
                base_url: "http://127.0.0.1:1".into(),
                api_key: "k".into(),
            }],
            story: test_role("p1", "m1"),
            pair: test_role("p1", "m1"),
            prompts: Default::default(),
            model_ctx: Default::default(),
            compaction: Default::default(),
        })
        .expect("构造 provider");
        let fake = std::sync::Arc::new(FakeConvStore::default());
        fake.rows.lock().unwrap().insert(
            "sv-a".into(),
            vec![
                octopus_engine::ConvRecord { round: 1, role: "user".into(), content: "u1".into() },
                octopus_engine::ConvRecord { round: 1, role: "assistant".into(), content: "a1".into() },
            ],
        );
        octopus_engine::AiProvider::set_conversation_store(&p, fake.clone());

        // 懒加载：首次 ensure_loaded 把持久层的历史装进内存。
        p.ensure_loaded("sv-a").await;
        {
            let conv = p.conversations.lock().unwrap();
            let entry = conv.get("sv-a").expect("会话已载入");
            assert_eq!(entry.len(), 2);
            assert_eq!(entry[1].content, "a1");
        }
        // 再调一次不会重复载入。
        p.ensure_loaded("sv-a").await;
        assert_eq!(p.conversations.lock().unwrap().get("sv-a").unwrap().len(), 2);

        // 追加后写回：持久层应变成 3 条。
        p.conversations
            .lock()
            .unwrap()
            .entry("sv-a".into())
            .or_default()
            .push(super::ConvMessage { round: 2, role: super::ConvRole::User, content: "u2".into() });
        p.persist_conversation("sv-a").await;
        assert_eq!(fake.rows.lock().unwrap().get("sv-a").unwrap().len(), 3);

        // 清空：内存与持久层都不再保留。
        octopus_engine::AiProvider::clear_conversation(&p, "sv-a").await;
        assert!(!p.conversations.lock().unwrap().contains_key("sv-a"));
        assert!(fake.rows.lock().unwrap().get("sv-a").is_none());
        assert_eq!(fake.clears.lock().unwrap().as_slice(), ["sv-a".to_string()]);
    }

    /// 会话历史只追加、**永不裁剪**：模型窗口是百万级，早期剧情不该被静默丢掉。
    ///
    /// 供应商的上下文缓存是**前缀缓存**，队首一变后面整段按原价重算
    ///（octopus.db 实测：每回合砍头时命中率 0.4%，纯追加时 ~97%）。
    #[test]
    fn conversation_history_is_never_truncated() {
        let mut entry: Vec<super::ConvMessage> = Vec::new();
        for r in 0..300u32 {
            super::record_round(&mut entry, r, format!("u{r}"), format!("a{r}"));
        }
        assert_eq!(entry.len(), 600, "只追加，不裁剪");
        assert_eq!(entry[0].content, "u0", "最早期的那一轮必须还在");
        assert_eq!(entry[599].content, "a299");
    }

    /// 第 N 轮发出去的历史必须是第 N-1 轮的**前缀 + 追加**。
    ///
    /// 这是缓存命中的充要条件：只要每轮都是「上一轮 + 新往返」，供应商就能把
    /// 整段历史按缓存价复用；一旦某轮从中间改了序列，从改动点起全部失效。
    #[test]
    fn conversation_history_is_always_append_only() {
        let mut entry: Vec<super::ConvMessage> = Vec::new();
        let mut prev: Vec<String> = Vec::new();
        for round in 1..200u32 {
            let sent = super::append_round_history(&mut entry, round, true, &format!("u{round}"));
            let sent: Vec<String> = sent
                .iter()
                .map(|m| super::message_to_trace(m).content)
                .collect();
            assert!(
                sent.starts_with(&prev[..]),
                "第 {round} 轮的历史不再是上一轮的前缀（前缀缓存会整段失效）"
            );
            assert_eq!(sent.len(), 2 * round as usize - 1, "历史必须完整保留");
            prev = sent;
            super::record_round(&mut entry, round, format!("u{round}"), format!("a{round}"));
        }
        assert_eq!(entry.len(), 398, "199 轮往返全部留在会话里");
    }

    #[test]
    fn turn_prompt_carries_canon_quests_and_gm_mode() {
        use octopus_engine::TurnContext;
        use octopus_types::{QuestView, RoundChannel};
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 6,
            scene_id: "sc-2".into(),
            scene_title: "三野猪小径".into(),
            scene_description: Some("秋雨与霜雾笼罩的商道，车队残骸散落。".into()),
            location: None,
            controlled: "莱纳斯·晨星(char-linas)".into(),
            player_text: "新增一个主线任务「查明狼群巢穴的位置」".into(),
            channel: RoundChannel::Gm,
            characters: vec![],
            personas: vec![],
            lore: vec![],
            premise: None,
            narrative: vec![],
            token_budget: 0,
            focus: vec![],
            canon: vec!["灌木后是幻影，底下还有一道法术".into()],
            quests: vec![QuestView {
                id: "quest-1".into(),
                text: "查明狼群巢穴的位置".into(),
                done: false,
                source: "gm".into(),
                hidden: false,
                primary: true,
                // 地图 P5 的加性字段（QuestView 的地点继承所属场景）：测试字面量用默认值。
                ..Default::default()
            }],
            encounters: vec![octopus_types::EncounterView {
                id: "enc-1".into(),
                name: "狼群合围".into(),
                enemies: vec![octopus_types::EnemyView {
                    id: "e1".into(),
                    name: "灰狼".into(),
                    hp: 7,
                    max: 11,
                    ac: 12,
                    ..Default::default()
                }],
                note: None,
                active: true,
                // 地图 P5 的叙事锚（创建时快照）：测试字面量用默认值。
                ..Default::default()
            }],
            scenes: vec![
                octopus_engine::SceneBrief {
                    id: "sc-1".into(),
                    title: "三野猪小径的伏击".into(),
                    chapter: "第一章".into(),
                },
                octopus_engine::SceneBrief {
                    id: "sc-2".into(),
                    title: "克雷格莫巢穴".into(),
                    chapter: "第一章".into(),
                },
            ],
            attributes: vec![],
            model: None,
            memories: vec![],
            turn_feedback: vec![],
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("已裁定的事实"), "必须带上导演已裁定的事实");
        assert!(p.contains("灌木后是幻影"));
        assert!(p.contains("场景描述：秋雨与霜雾"), "必须带上当前场景描述");
        assert!(p.contains("当前任务"));
        assert!(p.contains("查明狼群巢穴的位置"));
        assert!(p.contains("导演模式"), "导演回合要有专属说明");
        assert!(p.contains("导演指令"), "渠道要标明是导演指令");
        assert!(p.contains("当前遭遇"), "有遭遇时要带上遭遇段");
        assert!(
            p.contains("e1 灰狼 HP 7/11"),
            "遭遇里要带敌人 id，AI 才能 strike"
        );
        assert!(p.contains("优先级"), "要显式声明提示词分层的优先级");
        assert!(
            !p.contains("本回合分工"),
            "导演回合由主线 AI 自由演绎，不写分工"
        );
        assert!(
            p.contains("可推进的场景"),
            "要给出可 advance_scene 的场景清单"
        );
        assert!(
            p.contains("sc-2") && p.contains("← 当前"),
            "当前场景要被标记出来"
        );
    }

    /// 图鉴 M2 §4.6：遭遇块升级为数据卡摘要——名字 / HP / AC / 可用攻击（技能名 + 伤害骰）。
    /// 临时敌人（无 attacks）时这一段不出现，提示词与改动前一致。
    #[test]
    fn turn_prompt_renders_bestiary_data_card_attacks() {
        use octopus_engine::TurnContext;
        use octopus_types::{EnemyAttack, EnemyView, RoundChannel};
        let enemy = |attacks: Vec<EnemyAttack>| EnemyView {
            id: "e1".into(),
            name: "地精".into(),
            hp: 7,
            max: 7,
            ac: 15,
            attacks,
            ..Default::default()
        };
        let ctx_with = |attacks: Vec<EnemyAttack>| TurnContext {
            save_id: "sv-1".into(),
            round: 1,
            scene_id: "sc-1".into(),
            scene_title: "洞穴".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-a)".into(),
            player_text: "上".into(),
            channel: RoundChannel::Character,
            characters: vec![],
            personas: vec![],
            lore: vec![],
            premise: None,
            narrative: vec![],
            token_budget: 0,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![octopus_types::EncounterView {
                id: "enc-1".into(),
                name: "洞穴伏击".into(),
                enemies: vec![enemy(attacks)],
                note: None,
                active: true,
                // 地图 P5 的叙事锚（创建时快照）：测试字面量用默认值。
                ..Default::default()
            }],
            scenes: vec![],
            attributes: vec![],
            model: None,
            memories: vec![],
            turn_feedback: vec![],
            protocol: None,
        };
        let p = super::turn_prompt(&ctx_with(vec![
            EnemyAttack {
                skill_id: "sk-scimitar".into(),
                name: "弯刀".into(),
                damage: "1d6+2".into(),
            },
            EnemyAttack {
                skill_id: "sk-bow".into(),
                name: "短弓".into(),
                damage: "1d6+2".into(),
            },
        ]));
        assert!(p.contains("e1 地精 HP 7/7 AC 15"), "名字 / HP / AC 仍在：{p}");
        assert!(p.contains("可用攻击：弯刀 1d6+2、短弓 1d6+2"), "技能名 + 伤害骰：{p}");
        // 临时敌人（AI 现编）没有图鉴攻击 → 与改动前的提示词逐字一致。
        let plain = super::turn_prompt(&ctx_with(vec![]));
        assert!(plain.contains("e1 地精 HP 7/7 AC 15"));
        assert!(!plain.contains("（可用攻击："), "无攻击摘要时不出现这一段");
    }

    /// 地点进回合提示词（设计 §6.6）：形如「场景：碎星酒馆的夜晚（地点：碎星酒馆）」，
    /// 且只落在**用户消息**（turn 骨架）里——系统 preamble 必须逐回合稳定（AGENTS.md 硬约束）。
    #[test]
    fn turn_prompt_shows_current_location_but_preamble_stays_clean() {
        use octopus_engine::{ProtocolSpec, TurnContext};
        use octopus_types::RoundChannel;
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 3,
            scene_id: "sc-tavern".into(),
            scene_title: "碎星酒馆的夜晚".into(),
            scene_description: Some("窗外的雨敲打着木板。".into()),
            location: Some("碎星酒馆".into()),
            controlled: "米拉(char-mira)".into(),
            player_text: "环顾四周".into(),
            channel: RoundChannel::Character,
            characters: vec![],
            personas: vec![],
            lore: vec![],
            premise: None,
            narrative: vec![],
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
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("场景：碎星酒馆的夜晚（地点：碎星酒馆）"), "地点要跟在场景标题后面：{p}");
        assert!(p.contains("场景描述：窗外的雨敲打着木板。"), "描述仍在地点之后");
        // 系统层（默认协议 preamble）逐回合稳定：地点绝不进去，否则前缀缓存整段失效。
        let adapter = octopus_engine::build_protocol_adapter(&ProtocolSpec::default());
        let preamble = adapter.tool_preamble(&ctx).unwrap_or_else(|| adapter.preamble(&ctx));
        assert!(!preamble.contains("碎星酒馆"), "地点不得进系统 preamble（缓存前缀必须稳定）");
        // 旧故事书（场景未声明地点）：提示词与加入地点之前逐字一致。
        let mut no_loc = ctx.clone();
        no_loc.location = None;
        let p2 = super::turn_prompt(&no_loc);
        assert!(!p2.contains("（地点"), "未声明地点时不该出现地点段");
        assert!(p2.contains("场景：碎星酒馆的夜晚\n场景描述：窗外的雨敲打着木板。"));
    }

    #[test]
    fn parses_actor_id_on_speak_and_emote() {
        // 说话人归属随意图下发；旁白可省略。
        let raw = r#"[{"type":"speak","content":"别走那条路。","actor_id":"char-linas","tone":"warn"},{"type":"emote","content":"她抬手示意别动。","actor_id":"char-quelin"},{"type":"narrate","content":"夜色沉下来。"}]"#;
        let v = parse_intents(raw).expect("intents with actor_id");
        match &v[0] {
            Intent::Speak { actor_id, .. } => assert_eq!(actor_id.as_deref(), Some("char-linas")),
            other => panic!("expected speak, got {other:?}"),
        }
        match &v[1] {
            Intent::Emote { actor_id, .. } => assert_eq!(actor_id.as_deref(), Some("char-quelin")),
            other => panic!("expected emote, got {other:?}"),
        }
        match &v[2] {
            Intent::Narrate { actor_id, .. } => assert_eq!(actor_id.as_deref(), None),
            other => panic!("expected narrate, got {other:?}"),
        }
    }

    #[test]
    fn turn_prompt_includes_focus_definitions() {
        use octopus_engine::TurnContext;
        use octopus_types::{FocusEntity, RoundChannel};
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 1,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            location: None,
            controlled: "char-a".into(),
            player_text: "把短剑给雨果".into(),
            channel: RoundChannel::Character,
            characters: vec![],
            personas: vec![],
            lore: vec![],
            premise: None,
            narrative: vec![],
            token_budget: 0,
            focus: vec![FocusEntity {
                kind: "item".into(),
                id: Some("it-sword".into()),
                name: "生锈短剑".into(),
                entity: serde_json::json!({ "id": "it-sword", "name": "生锈短剑", "type": "weapon" }),
            }],
            canon: vec!["地精在灌木后设了伏击".into()],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            attributes: vec![],
            model: None,
            memories: vec![],
            turn_feedback: vec![],
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("本次玩家明确引用"), "应包含引用段落");
        assert!(p.contains("生锈短剑"));
        assert!(p.contains("it-sword"));
    }

    #[test]
    fn turn_prompt_injects_personas_and_example_dialogues() {
        use octopus_engine::{PersonaView, TurnContext};
        use octopus_types::RoundChannel;
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 2,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "「伊莎，来杯麦酒。」".into(),
            channel: RoundChannel::Character,
            characters: vec![octopus_types::ActorRef {
                id: "char-isa".into(),
                name: "伊莎".into(),
            }],
            personas: vec![PersonaView {
                id: "char-isa".into(),
                name: "伊莎".into(),
                kind: "npc".into(),
                background: "碎星酒馆老板娘。".into(),
                personality: "热情圆滑，爱打听消息。".into(),
                appearance: String::new(),
                example_dialogues: "「哟，稀客。」\n「这杯算我的。」".into(),
            }],
            lore: vec![],
            premise: None,
            narrative: vec![],
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
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("人物设定"), "应包含人物设定段");
        assert!(p.contains("伊莎"));
        assert!(p.contains("热情圆滑"), "性格要注入");
        assert!(p.contains("这杯算我的"), "对话示例要注入，供模型模仿语气");
    }

    #[test]
    fn turn_prompt_injects_matched_lore() {
        use octopus_engine::{LoreView, TurnContext};
        use octopus_types::RoundChannel;
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 3,
            scene_id: "sc-1".into(),
            scene_title: "暗影森林入口".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "我们走进暗影森林。".into(),
            channel: RoundChannel::Character,
            characters: vec![],
            personas: vec![],
            lore: vec![LoreView {
                id: "lore-shadow".into(),
                title: "暗影森林".into(),
                content: "终年迷雾，深处有精灵遗迹。".into(),
                priority: 10,
            }],
            premise: None,
            narrative: vec![],
            token_budget: 2000,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            attributes: vec![],
            model: None,
            memories: vec![],
            turn_feedback: vec![],
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("世界设定"), "应包含世界设定段");
        assert!(p.contains("终年迷雾"), "命中词条的内容要注入");
    }

    /// C：合法判定属性要列进提示词（否则模型会拿 dexterity 这类英文别名瞎猜）。
    #[test]
    fn turn_prompt_lists_check_attributes() {
        use octopus_engine::TurnContext;
        use octopus_types::RoundChannel;
        let mut ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 1,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "我试试".into(),
            channel: RoundChannel::Character,
            characters: vec![],
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
            attributes: vec!["agi".into(), "str".into()],
            model: None,
            memories: vec![],
            turn_feedback: vec![],
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("可用判定属性"), "{p}");
        assert!(p.contains("agi") && p.contains("str"), "{p}");
        // 未声明属性时整段不注入。
        ctx.attributes.clear();
        assert!(!super::turn_prompt(&ctx).contains("可用判定属性"));
    }

    #[test]
    fn preamble_is_positive_first() {
        // 正向指令为主（借鉴提示词工程的 8:2）：系统提示词避免「不要…」式表述。
        assert!(
            !octopus_engine::SYSTEM_PREAMBLE.contains("不要"),
            "系统 preamble 应正向表述"
        );
        assert!(
            octopus_engine::SYSTEM_PREAMBLE.contains("strike"),
            "攻击交给引擎结算要写明"
        );
        assert!(
            octopus_engine::SYSTEM_PREAMBLE.contains("对话示例"),
            "扮演口吻要参考对话示例"
        );
    }

    #[test]
    fn turn_prompt_renders_narrative_sections_by_slot_and_scope() {
        use octopus_engine::{NarrativeView, TurnContext};
        use octopus_types::{ActorRef, RoundChannel};
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 4,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "你好".into(),
            channel: RoundChannel::Character,
            characters: vec![ActorRef {
                id: "char-isa".into(),
                name: "伊莎".into(),
            }],
            personas: vec![],
            premise: Some("坠星谷的边境小镇。".into()),
            narrative: vec![
                NarrativeView {
                    id: "w1".into(),
                    slot: "world".into(),
                    scope: "both".into(),
                    text: "补充世界设定。".into(),
                },
                NarrativeView {
                    id: "s1".into(),
                    slot: "style".into(),
                    scope: "both".into(),
                    text: "冷硬派文风。".into(),
                },
                NarrativeView {
                    id: "b1".into(),
                    slot: "behavior".into(),
                    scope: "story".into(),
                    text: "只有主线看得到。".into(),
                },
                NarrativeView {
                    id: "c1".into(),
                    slot: "closing".into(),
                    scope: "both".into(),
                    text: "结尾收束一句。".into(),
                },
                NarrativeView {
                    id: "x1".into(),
                    slot: "style".into(),
                    scope: "character:char-other".into(),
                    text: "别人专属。".into(),
                },
            ],
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
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("【世界前提】") && p.contains("坠星谷的边境小镇。"));
        assert!(p.contains("【世界设定补充】") && p.contains("补充世界设定。"));
        assert!(p.contains("【叙事要求】") && p.contains("冷硬派文风。"));
        assert!(p.contains("【收尾要求】") && p.contains("结尾收束一句。"));
        assert!(p.contains("只有主线看得到。"), "单一 AI 统一应用 story scope");
        assert!(p.contains("别人专属。"), "单一 AI 统一应用 character:<id> scope");
    }

    #[test]
    fn parses_bare_array() {
        let raw = r#"[{"type":"narrate","content":"夜色沉下来。"}]"#;
        let v = parse_intents(raw).expect("bare array");
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn parses_fenced_array_with_prose() {
        let bt = '\u{60}';
        let fence: String = [bt, bt, bt].iter().collect();
        let raw = format!(
            "好的，这是意图：\n{fence}json\n[{{\"type\":\"speak\",\"content\":\"稀客。\"}}]\n{fence}\n就这样。"
        );
        let v = parse_intents(&raw).expect("fenced array");
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn parses_intents_wrapper() {
        let raw = r#"{"intents":[{"type":"finish_turn"}]}"#;
        let v = parse_intents(raw).expect("wrapper object");
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_intents("我不会响应这种请求。").is_err());
    }

    fn test_role(provider_id: &str, model: &str) -> super::RigRoleParams {
        super::RigRoleParams {
            provider_id: provider_id.into(),
            model: model.into(),
            temperature: 0.8,
            max_tokens: 1024,
            sampling: serde_json::json!({}),
        }
    }

    /// 按存档取单一模型：存档指定优先（可跨供应商），否则回落全局默认。
    #[test]
    fn pick_prefers_save_model_then_falls_back_to_global_default() {
        use octopus_engine::ModelRef;
        let p = super::RigProvider::new(super::RigParams {
            providers: vec![
                super::RigProviderParams {
                    id: "p1".into(),
                    base_url: "http://127.0.0.1:1".into(),
                    api_key: "k".into(),
                },
                super::RigProviderParams {
                    id: "p2".into(),
                    base_url: "http://127.0.0.1:2".into(),
                    api_key: "k".into(),
                },
            ],
            story: test_role("p1", "m1"),
            pair: test_role("p1", "m1"),
            prompts: Default::default(),
            model_ctx: Default::default(),
            compaction: Default::default(),
        })
        .expect("构造 provider");

        // 存档指定了模型：用存档的模型（可跨供应商）。
        let preferred = ModelRef {
            provider_id: "p2".into(),
            model: "custom".into(),
            reasoning_effort: None,
        };
        let (_, model) = p
            .pick(Some(&preferred), &p.story_provider, &p.story_model)
            .unwrap();
        assert_eq!(model, "custom");

        // 指定了未知供应商：回落该角色的全局默认。
        let unknown = ModelRef {
            provider_id: "nope".into(),
            model: "x".into(),
            reasoning_effort: None,
        };
        let (_, model) = p
            .pick(Some(&unknown), &p.story_provider, &p.story_model)
            .unwrap();
        assert_eq!(model, "m1");

        // 没有存档覆盖：取全局默认。
        let (_, story_model) = p.pick(None, &p.story_provider, &p.story_model).unwrap();
        assert_eq!(story_model, "m1");
    }

    /// #05 §3.4：相关往事注入单一 AI 提示词。
    #[test]
    fn memories_are_injected_into_single_ai_prompt() {
        use octopus_engine::{MemoryHit, TurnContext};
        use octopus_types::RoundChannel;
        let mut ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 2,
            scene_id: "sc-1".into(),
            scene_title: "酒馆".into(),
            scene_description: None,
            location: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "你还记得那座古堡吗？".into(),
            channel: RoundChannel::Character,
            characters: vec![],
            personas: vec![],
            lore: vec![],
            premise: None,
            narrative: vec![],
            memories: vec![MemoryHit {
                seq: 7,
                round: 1,
                kind: "narrate".into(),
                text: "月光下的古堡矗立在悬崖边。".into(),
                score: 0.9,
            }],
            turn_feedback: vec![],
            token_budget: 0,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            attributes: vec![],
            model: None,
            protocol: None,
        };
        let prompt = super::turn_prompt(&ctx);
        assert!(prompt.contains("【相关往事】"), "单一 AI 提示词要带相关往事段");
        assert!(prompt.contains("月光下的古堡矗立在悬崖边。"));
        assert!(prompt.contains("第1回合"));

        // 无命中时不注入空段。
        ctx.memories.clear();
        assert!(!super::turn_prompt(&ctx).contains("【相关往事】"));
    }

    /// 缓存前缀守门：公共前缀长度算准（0 = 连系统层都变了）。
    #[test]
    fn prefix_guard_measures_the_common_prefix() {
        let d = |r: &str, c: &str| super::message_digest(r, c);
        let a = [d("system", "S"), d("user", "u1"), d("assistant", "a1")];
        assert_eq!(super::common_prefix_len(&a, &a), 3);
        // 追加：整段前缀都在（健康回合）。
        let b = [
            d("system", "S"),
            d("user", "u1"),
            d("assistant", "a1"),
            d("user", "u2"),
        ];
        assert_eq!(super::common_prefix_len(&a, &b), 3);
        // 系统层变了：0——逐回合会变的内容进 preamble 就是这个下场。
        let c = [d("system", "S2"), d("user", "u1"), d("assistant", "a1")];
        assert_eq!(super::common_prefix_len(&a, &c), 0);
        // 队首被丢：从第 1 条起失配——正是「每回合砍头」的病。
        let e = [d("system", "S"), d("user", "u2"), d("assistant", "a2")];
        assert_eq!(super::common_prefix_len(&a, &e), 1);
        // 角色也参与指纹。
        assert_ne!(d("user", "u1"), d("assistant", "u1"));
    }

    /// token 估算：用学到的「字符 / token」系数，且有下限保护（不会除零）。
    #[test]
    fn token_estimate_uses_the_learned_factor() {
        assert_eq!(super::estimate_tokens(0, 1.8), 0);
        assert_eq!(super::estimate_tokens(180, 1.8), 100);
        assert_eq!(super::estimate_tokens(1, 0.0), 5);
    }

    /// 压缩范围：切点只落在回合开头（不劈开一回合内的续轮往返），
    /// 且至少逐字保留 token 预算那么多的尾巴；全都要留时返回 None。
    #[test]
    fn compaction_range_keeps_the_priced_tail_and_cuts_at_round_starts() {
        let mut entry: Vec<super::ConvMessage> = Vec::new();
        for r in 1..=10u32 {
            super::record_round(&mut entry, r, "u".repeat(1000), "a".repeat(1000));
        }
        // 预算 0：只留最后一轮（2 条）。
        let cut = super::select_compaction_range(&entry, 1.8, 0, 1).expect("有得压");
        assert_eq!(entry.len() - cut, 2, "只保留最后一轮的往返");
        assert!(super::is_round_start(&entry, cut));
        // 预算 4000 token（≈7200 字符，每条约 556）：至少保留 2 轮。
        let cut = super::select_compaction_range(&entry, 1.8, 4000, 1).expect("有得压");
        assert!(super::rounds_in(&entry, cut) >= 2, "尾巴按预算保留");
        assert!(super::is_round_start(&entry, cut), "切点必须在回合开头");
        assert!(
            entry[..cut].iter().all(|m| m.round < entry[cut].round),
            "被遮蔽的必须是更早的回合"
        );
        // 预算大过全部历史：没得压（不能把整段都遮蔽掉）。
        assert!(super::select_compaction_range(&entry, 1.8, 1_000_000, 1).is_none());
    }

    /// 溢出兜底（预算 0）也要至少留两个回合，别把接续的上下文压没。
    #[test]
    fn compaction_range_keeps_two_rounds_on_overflow() {
        let mut entry: Vec<super::ConvMessage> = Vec::new();
        for r in 1..=10u32 {
            super::record_round(&mut entry, r, "u".repeat(1000), "a".repeat(1000));
        }
        let cut = super::select_compaction_range(&entry, 1.8, 0, 2).expect("有得压");
        assert_eq!(super::rounds_in(&entry, cut), 2);
    }

    /// 超限错误的辨认要保守（误判 = 一次昂贵的压缩 + 重试），摘要要有可识别的框。
    #[test]
    fn overflow_classifier_and_summary_framing() {
        assert!(super::is_context_overflow(
            "Error: This model's maximum context length is 131072 tokens"
        ));
        assert!(super::is_context_overflow("context_length_exceeded"));
        assert!(!super::is_context_overflow("401 invalid api key"));
        assert!(!super::is_context_overflow("rate limit exceeded"));
        let framed = super::frame_summary("  旧事重提  ");
        assert!(framed.contains("<已压缩摘要>") && framed.contains("</已压缩摘要>"));
        assert!(framed.contains("旧事重提"));
        assert!(!framed.contains("  旧事重提  "), "摘要要去掉首尾空白");
    }
}
