//! rig 驱动的 AiProvider（#10/#20）：用 rig 框架调用 OpenAI 兼容端点生成「意图」。
//!
//! 取代手写 reqwest：provider 客户端、CompletionModel、Agent 与提示编排都由 rig 提供。
//! rig 的 OpenAI 客户端默认走 Responses API，这里显式用 CompletionsClient（Chat Completions），
//! 以兼容 DeepSeek / Moonshot / Groq / 本地 ollama-openai 等 OpenAI 兼容端点。

use async_trait::async_trait;
use octopus_engine::{
    AiOutput, AiProvider, EngineError, ModelRef, TurnContext, build_protocol_adapter,
};
use octopus_types::RoundChannel;
use rig::completion::message::{AssistantContent, ReasoningContent};
use rig::completion::{CompletionRequest, Message};
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

/// 构造 rig provider 所需的全部参数（由 config.json 的 providers / roles 映射而来）。
#[derive(Debug, Clone)]
pub struct RigParams {
    pub providers: Vec<RigProviderParams>,
    /// 单一 AI 的默认模型。
    pub story: RigRoleParams,
    /// 便宜角色（pair）：只用于场景摘要压缩等派生记忆。
    pub pair: RigRoleParams,
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
    /// 每存档一条追加式会话：只追加、不重写历史，前缀逐字节稳定，
    /// 供应商据此复用 prompt / KV 缓存（#31 缓存会话）。
    conversations: std::sync::Mutex<std::collections::HashMap<String, Vec<ConvMessage>>>,
    /// 会话持久化端口：None = 只存内存（离线 / 测试默认）。
    conv_store: std::sync::Mutex<Option<std::sync::Arc<dyn octopus_engine::ConversationStore>>>,
    /// 已从持久层载入过会话的存档（懒加载去重）。
    conv_loaded: std::sync::Mutex<std::collections::HashSet<String>>,
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

/// 单存档会话保留的消息上限：超出后从最旧丢弃（缓存前缀随之重建）。
const MAX_CONV_MESSAGES: usize = 80;

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
            conversations: std::sync::Mutex::new(std::collections::HashMap::new()),
            conv_store: std::sync::Mutex::new(None),
            conv_loaded: std::sync::Mutex::new(std::collections::HashSet::new()),
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

/// 组装一轮提示词（单一 AI）：注入【相关往事】与回合工具结果。
fn turn_prompt(ctx: &TurnContext) -> String {
    turn_prompt_inner(ctx, true)
}

/// 把检索到的相关往事渲染成【相关往事】块；为空则不注入。
///
/// 明确告诉模型这是「可能过时的历史片段」，避免把旧事当当前事实照抄。
fn memories_block(ctx: &TurnContext) -> String {
    if ctx.memories.is_empty() {
        return String::new();
    }
    let mut b = String::from(
        "\n【相关往事】（仅在相关时参考的历史片段，不要照抄；与当前场景冲突时以当前为准）\n",
    );
    for m in &ctx.memories {
        let text = m.text.trim();
        if text.is_empty() {
            continue;
        }
        b.push_str(&format!("- [第{}回合/{}] {text}\n", m.round, m.kind));
    }
    b
}

fn turn_prompt_inner(ctx: &TurnContext, include_memories: bool) -> String {
    // 相关往事按预算注入（#05 §3.4）。
    let memories = if include_memories { memories_block(ctx) } else { String::new() };
    let channel = match ctx.channel {
        RoundChannel::Character => "角色输入",
        RoundChannel::Meta => "元指令",
        RoundChannel::Gm => "导演指令（人代替 GM 推进剧情）",
    };
    // 导演已裁定的事实：最高优先级，AI 不得推翻
    let canon = if ctx.canon.is_empty() {
        String::new()
    } else {
        let mut b = String::from(
            "
【已裁定的事实（导演给出，最高优先级：必须遵守，不得推翻）】
",
        );
        for c in &ctx.canon {
            b.push_str(&format!(
                "- {c}
"
            ));
        }
        b
    };
    // #04 ⑦ 回合内续轮：只回喂模型自己刚触发的 query_world / check / interact 结果，
    // 不重发世界全量。首轮该字段为空 → 提示词与单轮路径逐字一致。
    let turn_feedback = if include_memories && !ctx.turn_feedback.is_empty() {
        let mut b = String::from(
            "\n【本轮工具结果】（你刚发起的查询 / 判定结果，请据此继续；信息足够时可输出 finish_turn 收束）\n",
        );
        for f in &ctx.turn_feedback {
            b.push_str(&format!("- {f}\n"));
        }
        b
    } else {
        String::new()
    };
    // 当前任务（含骨架目标与导演新增）
    let shown: Vec<&octopus_types::QuestView> =
        ctx.quests.iter().filter(|q| !q.hidden).take(20).collect();
    let quests = if shown.is_empty() {
        String::new()
    } else {
        let mut b = String::from(
            "
【当前任务】
",
        );
        for q in shown {
            b.push_str(&format!(
                "- [{}] {}{}
",
                if q.done { "x" } else { " " },
                q.text,
                if q.primary { "（主线）" } else { "" }
            ));
        }
        b
    };
    // 可推进的场景清单：不给合法 id，AI 用 advance_scene 只能瞎猜目标。
    let scenes = if ctx.scenes.is_empty() {
        String::new()
    } else {
        let mut b =
            String::from("\n【可推进的场景】需要换场时用 advance_scene {target_scene_id}：\n");
        for s in &ctx.scenes {
            b.push_str(&format!(
                "- {}{}（{}）{}\n",
                if s.chapter.is_empty() {
                    String::new()
                } else {
                    format!("{} · ", s.chapter)
                },
                s.title,
                s.id,
                if s.id == ctx.scene_id {
                    " ← 当前"
                } else {
                    ""
                }
            ));
        }
        b
    };
    // 当前遭遇（结构化敌人）
    let encounters = if ctx.encounters.iter().any(|e| e.active) {
        let mut b = String::from(
            "
【当前遭遇】
",
        );
        for e in ctx.encounters.iter().filter(|e| e.active).take(3) {
            b.push_str(&format!(
                "- {}{}
",
                e.name,
                e.note
                    .as_ref()
                    .map(|n| format!("（{n}）"))
                    .unwrap_or_default()
            ));
            for en in &e.enemies {
                b.push_str(&format!(
                    "  · {} {} HP {}/{} AC {}
",
                    en.id, en.name, en.hp, en.max, en.ac
                ));
            }
        }
        b
    } else {
        String::new()
    };
    // 导演模式专属说明
    let gm = if ctx.channel == RoundChannel::Gm {
        "
【导演模式】本回合是「导演」（人）在代替 GM 推进剧情，不是受控角色的言行：
         - 把导演的意图扩写成叙事（narrate）与必要的对话/神态，保持既有文风；
         - 不要替受控角色做决定，也不要让受控角色替导演发言；
         - 导演专属意图：quest {text, hidden?, primary?} 新增任务；encounter {name, enemies:[{name,hp?,ac?}], note?} 创建结构化遭遇（ac=防御值，越高越难打中，缺省 12）；adjust {character_id, resource, amount} 调整资源；status {character_id, status_id, remove?} 施加/移除状态。
         - 未署名的叙事归属「故事本身」，不要挂到玩家角色头上。
"
    } else {
        ""
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
        let mut b = String::from(
            "【人物设定】按下列档案扮演这些人物；对话示例用于模仿语气与句式，不要照抄台词。\n",
        );
        for p in visible_personas {
            b.push_str(&format!("▸ {}（{}）\n", p.name, p.id));
            for (label, val) in [
                ("背景", &p.background),
                ("性格", &p.personality),
                ("外观", &p.appearance),
            ] {
                let v = val.trim();
                if !v.is_empty() {
                    b.push_str(&format!("  {label}：{v}\n"));
                }
            }
            let ex = p.example_dialogues.trim();
            if !ex.is_empty() {
                b.push_str(&format!("  对话示例（模仿语气，勿照抄）：\n{ex}\n"));
            }
        }
        b
    };
    // 世界词条：关键词命中的背景设定（引擎已按预算裁剪）。
    let lore = if ctx.lore.is_empty() {
        String::new()
    } else {
        let mut b = String::from("【世界设定】以下事实在需要时参考，不要整段复述：\n");
        for l in &ctx.lore {
            let title = l.title.trim();
            let content = l.content.trim();
            if title.is_empty() {
                b.push_str(&format!("- {content}\n"));
            } else {
                b.push_str(&format!("- {title}：{content}\n"));
            }
        }
        b
    };
    let mut scene = if ctx.scene_title.is_empty() {
        "（未命名场景）".to_string()
    } else {
        ctx.scene_title.clone()
    };
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
        let mut block = String::from("\n本次玩家明确引用了以下实体，请在演绎与意图中聚焦它们：\n");
        for f in &ctx.focus {
            block.push_str(&format!(
                "- {}「{}」({})\n```json\n{}\n```\n",
                f.kind,
                f.name,
                f.id.as_deref().unwrap_or("-"),
                serde_json::to_string_pretty(&f.entity).unwrap_or_else(|_| "{}".to_string())
            ));
        }
        block
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
        world.push_str("\n【世界前提】");
        world.push_str(p);
        world.push('\n');
    }
    let world_sections = sections("world");
    if !world_sections.is_empty() {
        world.push_str("\n【世界设定补充】\n");
        world.push_str(&world_sections);
    }
    let mut directives = String::new();
    for s in ctx
        .narrative
        .iter()
        .filter(|s| s.slot == "style" || s.slot == "behavior")
    {
        if directives.is_empty() {
            directives.push_str("\n【叙事要求】\n");
        }
        directives.push_str(&s.text);
        directives.push('\n');
    }
    let closing_sections = sections("closing");
    let closing = if closing_sections.is_empty() {
        String::new()
    } else {
        format!("\n【收尾要求】\n{closing_sections}")
    };
    // C：把故事书声明的判定属性 key 明给模型，避免它拿英文别名瞎猜（如 dexterity）。
    let attributes = if ctx.attributes.is_empty() {
        String::new()
    } else {
        format!(
            "可用判定属性（check 的 attribute 只能填这些）：{}\n",
            ctx.attributes.join("、")
        )
    };
    format!(
        "【回合 {round}】\n（各段冲突时的优先级：已裁定的事实 > 人物设定 / 世界设定 > 场景与任务 > 玩家输入）\n{world}场景：{scene}\n{memories}受控角色：{controlled}\n在场角色：{chars}\n{attributes}{personas}{lore}输入渠道：{channel}\n{canon}{quests}{scenes}{encounters}{directives}{turn_feedback}\n输入：{text}\n{closing}{focus}{gm}\n请输出意图 JSON 数组。",
        round = ctx.round,
        scene = scene,
        memories = memories,
        controlled = controlled,
        chars = chars,
        attributes = attributes,
        personas = personas,
        lore = lore,
        turn_feedback = turn_feedback,
        world = world,
        directives = directives,
        closing = closing,
        channel = channel,
        canon = canon,
        quests = quests,
        scenes = scenes,
        encounters = encounters,
        text = ctx.player_text,
        focus = focus,
        gm = gm,
    )
}

/// 从 rig 响应里抽出正文与思考链文本。
fn split_response(choice: &[AssistantContent]) -> (String, String) {
    let mut text = String::new();
    let mut reasoning = String::new();
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
            _ => {}
        }
    }
    (text, reasoning)
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
                        *entry = recs.into_iter().map(conv_message_from).collect();
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

    /// 单次补全：拿意图，并把供应商返回的思考链一并带出（供前端「思考」折叠块）。
    ///
    /// preamble 与 parse 都由故事书声明的协议适配器决定（叙事契约 P2）：
    /// 缺省协议 = 引擎内置文本 + `parse_intents`，与旧行为逐字一致。
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
        // 追加式会话：取出本存档历史（整轮开始 / 重跑时截断到本回合之前），
        // 再把这次的 user 提示词追加为最后一条——前缀稳定，缓存才命中。
        let chat_history = {
            let mut conv = self.conversations.lock().expect("conversations poisoned");
            let entry = conv.entry(ctx.save_id.clone()).or_default();
            append_round_history(entry, ctx.round, ctx.turn_feedback.is_empty(), &prompt)
        };
        let request = CompletionRequest {
            model: None,
            preamble: Some(adapter.preamble(ctx)),
            chat_history,
            documents: Vec::new(),
            tools: Vec::new(),
            temperature: Some(temperature),
            max_tokens: Some(max_tokens),
            tool_choice: None,
            additional_params: sampling,
            output_schema: None,
            record_telemetry_content: false,
        };
        let rig_model = client.completion_model(model);
        let response = rig_model
            .completion(request)
            .await
            .map_err(|e| EngineError::Ai(e.to_string()))?;
        let (text, reasoning) = split_response(&response.choice);
        // 用量遥测：cached 是检验「缓存是否吃满」的关键指标（供应商不回传时为 0）。
        tracing::info!(
            save_id = %ctx.save_id,
            round = ctx.round,
            input = response.usage.input_tokens,
            output = response.usage.output_tokens,
            cached = response.usage.cached_input_tokens,
            cache_write = response.usage.cache_creation_input_tokens,
            "AI 调用用量"
        );
        // 把这次的 user 提示词与模型原文追加进会话（下次请求即成为稳定前缀）。
        if let Ok(mut conv) = self.conversations.lock() {
            let entry = conv.entry(ctx.save_id.clone()).or_default();
            record_round(entry, ctx.round, prompt, text.clone());
        }
        self.persist_conversation(&ctx.save_id).await;
        // 协议解析 → 归一化；白名单过滤等警告随 AiOutput 交给 Session 落事件。
        let intents = adapter.parse(&text, ctx)?;
        let intents = adapter.normalize(intents, ctx);
        let intent_warnings = adapter.take_warnings();
        Ok(AiOutput {
            intents,
            reasoning: (!reasoning.trim().is_empty()).then_some(reasoning),
            intent_warnings,
        })
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

/// 记录一轮模型往返（user 提示词 + assistant 原文），并裁剪到单存档上限。
fn record_round(entry: &mut Vec<ConvMessage>, round: u32, prompt: String, assistant: String) {
    entry.push(ConvMessage { round, role: ConvRole::User, content: prompt });
    entry.push(ConvMessage { round, role: ConvRole::Assistant, content: assistant });
    if entry.len() > MAX_CONV_MESSAGES {
        let drop = entry.len() - MAX_CONV_MESSAGES;
        entry.drain(0..drop);
    }
}

/// 场景压缩用的系统提示词（#05 §3.3）：只让模型输出压缩后的短摘要。
///
/// 这是派生记忆，不参与叙事契约：所以不走故事书协议适配器，避免协议 preamble 要求
/// 输出意图 JSON 反而污染摘要正文。
const SUMMARY_PREAMBLE: &str = "你是 Octopus 的记忆压缩器：把给定的一串回合摘要合并压缩成一段更精炼的场景回顾。\n\
只输出压缩后的正文，不要解释、不要 Markdown、不要标题；控制在 1-3 句，保留人物、地点、关键事件与结果。";

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
            turn_prompt(ctx),
        )
        .await
    }

    fn set_conversation_store(&self, store: std::sync::Arc<dyn octopus_engine::ConversationStore>) {
        if let Ok(mut slot) = self.conv_store.lock() {
            *slot = Some(store);
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
            preamble: Some(SUMMARY_PREAMBLE.to_string()),
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
        let (out, _) = split_response(&response.choice);
        let trimmed = out.trim();
        // 空输出视同没有压缩结果，交给调用方回退拼接。
        Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use octopus_engine::parse_intents;
    use octopus_types::Intent;

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

    /// 会话上限：超出后丢最旧，保留最新往返。
    #[test]
    fn conversation_trim_keeps_newest() {
        let mut entry: Vec<super::ConvMessage> = Vec::new();
        for r in 0..80u32 {
            super::record_round(&mut entry, r, format!("u{r}"), format!("a{r}"));
        }
        assert_eq!(entry.len(), super::MAX_CONV_MESSAGES);
        assert_eq!(entry.last().unwrap().content, "a79");
        assert_eq!(entry[0].content, format!("u{}", 80 - super::MAX_CONV_MESSAGES as u32 / 2));
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
                }],
                note: None,
                active: true,
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
}
