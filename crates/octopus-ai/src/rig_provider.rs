//! rig 驱动的 AiProvider（#10/#20）：用 rig 框架调用 OpenAI 兼容端点生成「意图」。
//!
//! 取代手写 reqwest：provider 客户端、CompletionModel、Agent 与提示编排都由 rig 提供。
//! rig 的 OpenAI 客户端默认走 Responses API，这里显式用 CompletionsClient（Chat Completions），
//! 以兼容 DeepSeek / Moonshot / Groq / 本地 ollama-openai 等 OpenAI 兼容端点。

use async_trait::async_trait;
use octopus_engine::{
    build_protocol_adapter, AiOutput, AiProvider, EngineError, ModelRef, ProtocolRole, TurnContext,
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

/// 单个角色（story / character）的默认模型与采样参数。
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
    pub story: RigRoleParams,
    pub character: RigRoleParams,
}

pub struct RigProvider {
    /// 全部可用供应商的客户端：存档可在其中任选模型。
    clients: std::collections::HashMap<String, openai::CompletionsClient>,
    story_provider: String,
    character_provider: String,
    story_model: String,
    character_model: String,
    story_temperature: f64,
    character_temperature: f64,
    story_max_tokens: u64,
    character_max_tokens: u64,
    story_sampling: serde_json::Value,
    character_sampling: serde_json::Value,
}

fn build_client(id: &str, base_url: &str, api_key: &str) -> Result<openai::CompletionsClient, String> {
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
            character_provider: p.character.provider_id,
            story_model: p.story.model,
            character_model: p.character.model,
            story_temperature: p.story.temperature,
            character_temperature: p.character.temperature,
            story_max_tokens: p.story.max_tokens,
            character_max_tokens: p.character.max_tokens,
            story_sampling: p.story.sampling,
            character_sampling: p.character.sampling,
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
        obj.insert("reasoning_effort".into(), serde_json::Value::String(effort.to_string()));
    }
    if obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(obj))
    }
}

fn turn_prompt(ctx: &TurnContext) -> String {
    let channel = match ctx.channel {
        RoundChannel::Character => "角色输入",
        RoundChannel::Meta => "元指令",
        RoundChannel::Gm => "导演指令（人代替 GM 推进剧情）",
    };
    // 导演已裁定的事实：最高优先级，AI 不得推翻
    let canon = if ctx.canon.is_empty() {
        String::new()
    } else {
        let mut b = String::from("
【已裁定的事实（导演给出，最高优先级：必须遵守，不得推翻）】
");
        for c in &ctx.canon {
            b.push_str(&format!("- {c}
"));
        }
        b
    };
    // 当前任务（含骨架目标与导演新增）
    let shown: Vec<&octopus_types::QuestView> =
        ctx.quests.iter().filter(|q| !q.hidden).take(20).collect();
    let quests = if shown.is_empty() {
        String::new()
    } else {
        let mut b = String::from("
【当前任务】
");
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
        let mut b = String::from("\n【可推进的场景】需要换场时用 advance_scene {target_scene_id}：\n");
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
                if s.id == ctx.scene_id { " ← 当前" } else { "" }
            ));
        }
        b
    };
    // 当前遭遇（结构化敌人）
    let encounters = if ctx.encounters.iter().any(|e| e.active) {
        let mut b = String::from("
【当前遭遇】
");
        for e in ctx.encounters.iter().filter(|e| e.active).take(3) {
            b.push_str(&format!(
                "- {}{}
",
                e.name,
                e.note.as_ref().map(|n| format!("（{n}）")).unwrap_or_default()
            ));
            for en in &e.enemies {
                b.push_str(&format!("  · {} {} HP {}/{} AC {}
", en.id, en.name, en.hp, en.max, en.ac));
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
    let controlled = if ctx.controlled.is_empty() { "（未指定）" } else { ctx.controlled.as_str() };
    // 在场人物的人格档案：角色 AI 据此扮演，主线 AI 据此保持人物一致。
    // 对话示例是 few-shot 风格样板——模仿语气句式，不照抄台词。
    let personas = if ctx.personas.is_empty() {
        String::new()
    } else {
        let mut b = String::from("【人物设定】按下列档案扮演这些人物；对话示例用于模仿语气与句式，不要照抄台词。\n");
        for p in &ctx.personas {
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
    // 主线 AI 本回合已叙述的内容：角色 AI 应接着演，而不是重描同一段场景。
    let story_so_far = match ctx
        .story_narration
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(t) => format!("\n【主线 AI 本回合已叙述（不要重复这段描写，据此作出角色的回应）】\n{t}\n"),
        None => String::new(),
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
    // 分工（非导演回合）：台词由角色 AI 演绎，主线 AI 聚焦旁白与世界响应。
    // 否则两个 AI 会各给同一个人写一段台词，出现「Lucy 一口气说了两遍」。
    let division = if ctx.channel == RoundChannel::Gm {
        String::new()
    } else {
        "【本回合分工】你负责旁白、环境与世界响应；在场角色的**台词**由角色 AI 演绎——你可以描写他们的动作与神态，但不必替他们说话。\n"
            .to_string()
    };
    // 叙述段（故事书声明）：按渠道筛 scope、按槽位渲染。
    let present_ids: Vec<&str> = ctx.characters.iter().map(|c| c.id.as_str()).collect();
    let scope_ok = |scope: &str| -> bool {
        match ctx.channel {
            RoundChannel::Character => {
                scope == "character"
                    || scope == "both"
                    || scope
                        .strip_prefix("character:")
                        .map(|id| present_ids.iter().any(|p| *p == id))
                        .unwrap_or(false)
            }
            _ => scope == "story" || scope == "both",
        }
    };
    let sections = |slot: &str| -> String {
        let mut b = String::new();
        for s in ctx.narrative.iter().filter(|s| s.slot == slot && scope_ok(&s.scope)) {
            b.push_str(&s.text);
            b.push('\n');
        }
        b
    };
    let mut world = String::new();
    if let Some(p) = ctx.premise.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
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
        .filter(|s| (s.slot == "style" || s.slot == "behavior") && scope_ok(&s.scope))
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
    format!(
        "【回合 {round}】\n（各段冲突时的优先级：已裁定的事实 > 人物设定 / 世界设定 > 场景与任务 > 玩家输入）\n{division}{world}场景：{scene}\n受控角色：{controlled}\n在场角色：{chars}\n{personas}{lore}输入渠道：{channel}\n{canon}{quests}{scenes}{encounters}{directives}{story_so_far}\n输入：{text}\n{closing}{focus}{gm}\n请输出意图 JSON 数组。",
        round = ctx.round,
        scene = scene,
        controlled = controlled,
        chars = chars,
        personas = personas,
        lore = lore,
        story_so_far = story_so_far,
        division = division,
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
    /// 单次补全：拿意图，并把供应商返回的思考链一并带出（供前端「思考」折叠块）。
    ///
    /// preamble 与 parse 都由故事书声明的协议适配器决定（叙事契约 P2）：
    /// 缺省协议 = 引擎内置文本 + `parse_intents`，与旧行为逐字一致。
    async fn complete(
        &self,
        client: &openai::CompletionsClient,
        model: String,
        role: ProtocolRole,
        ctx: &TurnContext,
        temperature: f64,
        max_tokens: u64,
        sampling: Option<serde_json::Value>,
        prompt: String,
    ) -> Result<AiOutput, EngineError> {
        let spec = ctx.protocol.clone().unwrap_or_default();
        let adapter = build_protocol_adapter(&spec, role);
        let request = CompletionRequest {
            model: None,
            preamble: Some(adapter.preamble(ctx)),
            chat_history: vec![Message::user(prompt)],
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

#[async_trait]
impl AiProvider for RigProvider {
    async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
        let (client, model) = self.pick(ctx.model.as_ref(), &self.story_provider, &self.story_model)?;
        self.complete(
            client,
            model,
            ProtocolRole::Story,
            ctx,
            self.story_temperature,
            self.story_max_tokens,
            sampling_with_effort(&self.story_sampling, ctx.model.as_ref()),
            turn_prompt(ctx),
        )
        .await
    }

    async fn character_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
        let (client, model) = self.pick(ctx.model.as_ref(), &self.character_provider, &self.character_model)?;
        self.complete(
            client,
            model,
            ProtocolRole::Character,
            ctx,
            self.character_temperature,
            self.character_max_tokens,
            sampling_with_effort(&self.character_sampling, ctx.model.as_ref()),
            turn_prompt(ctx),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use octopus_engine::parse_intents;
    use octopus_types::Intent;

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
            story_narration: None,
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
                enemies: vec![octopus_types::EnemyView { id: "e1".into(), name: "灰狼".into(), hp: 7, max: 11, ac: 12 }],
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
            model: None,
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
        assert!(p.contains("e1 灰狼 HP 7/11"), "遭遇里要带敌人 id，AI 才能 strike");
        assert!(p.contains("优先级"), "要显式声明提示词分层的优先级");
        assert!(!p.contains("本回合分工"), "导演回合由主线 AI 自由演绎，不写分工");
        assert!(p.contains("可推进的场景"), "要给出可 advance_scene 的场景清单");
        assert!(p.contains("sc-2") && p.contains("← 当前"), "当前场景要被标记出来");
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
            story_narration: None,
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
            model: None,
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
            characters: vec![octopus_types::ActorRef { id: "char-isa".into(), name: "伊莎".into() }],
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
            story_narration: None,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            model: None,
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
            story_narration: None,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            model: None,
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("世界设定"), "应包含世界设定段");
        assert!(p.contains("终年迷雾"), "命中词条的内容要注入");
    }

    #[test]
    fn preambles_are_positive_first() {
        // 正向指令为主（借鉴提示词工程的 8:2）：系统提示词避免「不要…」式表述。
        assert!(!octopus_engine::STORY_PREAMBLE.contains("不要"), "主线 preamble 应正向表述");
        assert!(!octopus_engine::CHARACTER_PREAMBLE.contains("不要"), "角色 preamble 应正向表述");
        assert!(octopus_engine::STORY_PREAMBLE.contains("strike"), "攻击交给引擎结算要写明");
        assert!(octopus_engine::CHARACTER_PREAMBLE.contains("对话示例"), "角色口吻要参考对话示例");
    }

    #[test]
    fn turn_prompt_gives_character_ai_the_story_narration() {
        use octopus_engine::TurnContext;
        use octopus_types::RoundChannel;
        let ctx = TurnContext {
            save_id: "sv-1".into(),
            round: 2,
            scene_id: "sc-1".into(),
            scene_title: "教室里".into(),
            scene_description: None,
            controlled: "米拉(char-mira)".into(),
            player_text: "我没带伞".into(),
            channel: RoundChannel::Character,
            characters: vec![],
            personas: vec![],
            lore: vec![],
            premise: None,
            narrative: vec![],
            token_budget: 0,
            story_narration: Some("雨云压得极低，空气里泛着潮湿的凉意。".into()),
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            model: None,
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("已叙述"), "角色 AI 要看到主线 AI 本回合已叙述的内容");
        assert!(p.contains("本回合分工"), "非导演回合要写明台词归角色 AI，避免同一角色说两遍");
        assert!(p.contains("雨云压得极低"));
        // 角色 preamble 明确不抢旁白，避免与主线 AI 重描同一场景
        assert!(octopus_engine::CHARACTER_PREAMBLE.contains("主线 AI"));
        assert!(octopus_engine::CHARACTER_PREAMBLE.contains("场景"));
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
            characters: vec![ActorRef { id: "char-isa".into(), name: "伊莎".into() }],
            personas: vec![],
            premise: Some("坠星谷的边境小镇。".into()),
            narrative: vec![
                NarrativeView { id: "w1".into(), slot: "world".into(), scope: "both".into(), text: "补充世界设定。".into() },
                NarrativeView { id: "s1".into(), slot: "style".into(), scope: "both".into(), text: "冷硬派文风。".into() },
                NarrativeView { id: "b1".into(), slot: "behavior".into(), scope: "story".into(), text: "只有主线看得到。".into() },
                NarrativeView { id: "c1".into(), slot: "closing".into(), scope: "both".into(), text: "结尾收束一句。".into() },
                NarrativeView { id: "x1".into(), slot: "style".into(), scope: "character:char-other".into(), text: "别人专属。".into() },
            ],
            lore: vec![],
            token_budget: 0,
            story_narration: None,
            focus: vec![],
            canon: vec![],
            quests: vec![],
            encounters: vec![],
            scenes: vec![],
            model: None,
            protocol: None,
        };
        let p = super::turn_prompt(&ctx);
        assert!(p.contains("【世界前提】") && p.contains("坠星谷的边境小镇。"));
        assert!(p.contains("【世界设定补充】") && p.contains("补充世界设定。"));
        assert!(p.contains("【叙事要求】") && p.contains("冷硬派文风。"));
        assert!(p.contains("【收尾要求】") && p.contains("结尾收束一句。"));
        assert!(!p.contains("只有主线看得到。"), "story scope 不该进角色回合");
        assert!(!p.contains("别人专属。"), "非在场角色的专属段不该注入");
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
        let raw = format!("好的，这是意图：\n{fence}json\n[{{\"type\":\"speak\",\"content\":\"稀客。\"}}]\n{fence}\n就这样。");
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
}
