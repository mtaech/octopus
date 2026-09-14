//! 提示词装配（可自定义）：所有发往模型的固定文本都在这里集中声明，并支持配置覆盖。
//!
//! 设计：
//! - **默认值即常量**：每个可覆盖的提示词都有一个 *_DEFAULT 常量，作为唯一事实来源；
//!   应用配置（config.json 的 prompts）只在用户显式填写时覆盖它。
//! - **模板用 {{变量}}**：渲染是单遍扫描（不会把变量值里的 {{...}} 二次展开）；
//!   未定义的占位符原样保留，方便用户看到自己写错的变量名。
//! - **空覆盖 = 用默认**：StoryPrompts 的每个字段都是「已解析」的最终文本，
//!   Default 用内置常量填满，所以任何构造路径都不会产出空提示词。

/// 单遍把 {{key}} 替换成对应值；未定义的占位符原样保留。
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len() + 64);
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(close_rel) = template[i + 2..].find("}}") {
                let key = &template[i + 2..i + 2 + close_rel];
                if let Some((_, value)) = vars.iter().find(|(k, _)| *k == key) {
                    out.push_str(value);
                    i = i + 2 + close_rel + 2;
                    continue;
                }
            }
        }
        // 非占位符：按 UTF-8 字符整体推进。
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// 渲染一个「引导语 + 条目」的分节块。
pub fn render_block(template: &str, items: &str) -> String {
    render(template, &[("items", items)])
}

// ============================================================
// 游玩 / 聊天 AI（单一 AI「AI 主持」）
// ============================================================

/// 工具模式（原生工具调用）下的系统提示词默认值（= 引擎内置 TOOL_PREAMBLE）。
pub const PROTOCOL_TOOL_DEFAULT: &str = octopus_engine::TOOL_PREAMBLE;

/// 文本模式（输出意图 JSON 数组）下的系统提示词默认值（= 引擎内置 SYSTEM_PREAMBLE）。
pub const PROTOCOL_TEXT_DEFAULT: &str = octopus_engine::SYSTEM_PREAMBLE;

/// 每回合用户提示词的外层骨架。可用变量见 TURN_TEMPLATE_VARS。
pub const TURN_TEMPLATE_DEFAULT: &str = "【回合 {{round}}】\n（各段冲突时的优先级：已裁定的事实 > 人物设定 / 世界设定 > 场景与任务 > 玩家输入）\n{{world}}场景：{{scene}}\n{{memories}}受控角色：{{controlled}}\n在场角色：{{chars}}\n{{attributes}}{{personas}}{{lore}}输入渠道：{{channel}}\n{{canon}}{{quests}}{{scenes}}{{encounters}}{{directives}}{{turn_feedback}}\n输入：{{text}}\n{{closing}}{{focus}}{{gm}}\n请输出意图 JSON 数组。";

/// 回合模板可用变量的说明（key -> 含义），供前端展示与文档。
pub const TURN_TEMPLATE_VARS: &[(&str, &str)] = &[
    ("round", "当前回合序号（从 1 开始）"),
    ("world", "世界前提 + 故事书 world 槽位叙述段"),
    ("scene", "当前场景标题、当前地点与场景描述（地点形如「（地点：碎星酒馆）」，未声明地点时不出这一段）"),
    ("memories", "检索到的相关往事块（相关往事）"),
    ("controlled", "本回合受控角色名（或「未指定」）"),
    ("chars", "在场角色名单（名字(id)，顿号分隔）"),
    ("attributes", "可用判定属性清单（check 的 attribute 取值域）"),
    ("personas", "在场人物的人格档案块（人物设定）"),
    ("lore", "本回合命中的世界词条块（世界设定）"),
    ("channel", "输入渠道（角色输入 / 元指令 / 导演指令）"),
    ("canon", "导演已裁定的事实块（已裁定的事实）"),
    ("quests", "当前任务块（当前任务）"),
    ("scenes", "可推进场景清单块（可推进的场景）"),
    ("encounters", "当前结构化遭遇块（当前遭遇）"),
    ("directives", "故事书 style / behavior 槽位叙述段（叙事要求）"),
    ("turn_feedback", "回合内续轮回喂的工具结果块（本轮工具结果）"),
    ("text", "玩家本回合的输入正文"),
    ("closing", "故事书 closing 槽位叙述段（收尾要求）"),
    ("focus", "玩家显式引用的实体块（完整定义）"),
    ("gm", "导演模式专属说明块（仅导演回合非空）"),
];

/// 相关往事引导语。变量：{{items}}。
pub const BLOCK_MEMORIES_DEFAULT: &str =
    "\n【相关往事】（仅在相关时参考的历史片段，不要照抄；与当前场景冲突时以当前为准）\n{{items}}";

/// 本轮工具结果引导语。变量：{{items}}。
pub const BLOCK_TURN_FEEDBACK_DEFAULT: &str =
    "\n【本轮工具结果】（你刚发起的查询 / 判定结果，请据此继续；信息足够时可输出 finish_turn 收束）\n{{items}}";

/// 已裁定的事实引导语。变量：{{items}}。
pub const BLOCK_CANON_DEFAULT: &str =
    "\n【已裁定的事实（导演给出，最高优先级：必须遵守，不得推翻）】\n{{items}}";

/// 当前任务引导语。变量：{{items}}。
pub const BLOCK_QUESTS_DEFAULT: &str = "\n【当前任务】\n{{items}}";

/// 可推进的场景引导语。变量：{{items}}。
pub const BLOCK_SCENES_DEFAULT: &str =
    "\n【可推进的场景】需要换场时用 advance_scene {target_scene_id}：\n{{items}}";

/// 当前遭遇引导语。变量：{{items}}。
pub const BLOCK_ENCOUNTERS_DEFAULT: &str = "\n【当前遭遇】（敌方数据卡：HP / AC / 可用攻击；攻击结算用 strike / enemy_strike 交给引擎）\n{{items}}";

/// 人物设定引导语（人格档案 = 扮演依据）。变量：{{items}}。
pub const BLOCK_PERSONAS_DEFAULT: &str =
    "【人物设定】按下列档案扮演这些人物；对话示例用于模仿语气与句式，不要照抄台词。\n{{items}}";

/// 世界设定引导语（关键词命中的世界词条）。变量：{{items}}。
pub const BLOCK_LORE_DEFAULT: &str =
    "【世界设定】以下事实在需要时参考，不要整段复述：\n{{items}}";

/// 世界前提引导语。变量：{{premise}}。
pub const BLOCK_WORLD_PREMISE_DEFAULT: &str = "\n【世界前提】{{premise}}\n";

/// 世界设定补充引导语（world 槽位叙述段）。变量：{{items}}。
pub const BLOCK_WORLD_SECTIONS_DEFAULT: &str = "\n【世界设定补充】\n{{items}}";

/// 叙事要求引导语（style / behavior 槽位叙述段）。变量：{{items}}。
pub const BLOCK_DIRECTIVES_DEFAULT: &str = "\n【叙事要求】\n{{items}}";

/// 收尾要求引导语（closing 槽位叙述段）。变量：{{items}}。
pub const BLOCK_CLOSING_DEFAULT: &str = "\n【收尾要求】\n{{items}}";

/// 玩家显式引用实体的引导语。变量：{{items}}。
pub const BLOCK_FOCUS_DEFAULT: &str =
    "\n本次玩家明确引用了以下实体，请在演绎与意图中聚焦它们：\n{{items}}";

/// 可用判定属性清单。变量：{{items}}。
pub const BLOCK_ATTRIBUTES_DEFAULT: &str =
    "可用判定属性（check 的 attribute 只能填这些）：{{items}}\n";

/// 导演模式专属说明（仅导演回合注入，无变量）。
pub const BLOCK_GM_DEFAULT: &str = "\n【导演模式】本回合是「导演」（人）在代替 GM 推进剧情，不是受控角色的言行：\n         - 把导演的意图扩写成叙事（narrate）与必要的对话/神态，保持既有文风；\n         - 不要替受控角色做决定，也不要让受控角色替导演发言；\n         - 导演专属意图：quest {text, hidden?, primary?} 新增任务；encounter {name, enemies:[{name,hp?,ac?,template_id?,count?,skill_id?}], note?} 创建结构化遭遇（ac=防御值，越高越难打中，缺省 12；template_id 引用图鉴模板，按 count 克隆怪物实例，skill_id 覆盖缺省攻击技能）；adjust {character_id, resource, amount} 调整资源；status {character_id, status_id, remove?} 施加/移除状态。\n         - 未署名的叙事归属「故事本身」，不要挂到玩家角色头上。\n";

/// 意图解析失败后的纠正消息（工具模式）。变量：{{error}}。
pub const RETRY_TOOL_DEFAULT: &str = "你的上一次输出未被接受（原因：{{error}}）。请改用**原生工具调用**逐个调用意图工具（一次可并行调用多个，最后调用 finish_turn 收束）；不要用「[工具调用] 工具名 {…}」这类文字描述来代替真正的调用，也不要输出 JSON 数组。";

/// 意图解析失败后的纠正消息（文本模式）。变量：{{error}}。
pub const RETRY_TEXT_DEFAULT: &str = "你的上一次输出无法被解析为合法的意图 JSON（原因：{{error}}）。请忽略上一条输出，严格按照协议重新输出意图 JSON 数组。";

/// 记忆压缩（派生数据）的系统提示词，不走故事书协议适配器。
pub const MEMORY_SUMMARY_DEFAULT: &str = "你是 Octopus 的记忆压缩器：把给定的一串回合摘要合并压缩成一段更精炼的场景回顾。\n只输出压缩后的正文，不要解释、不要 Markdown、不要标题；控制在 1-3 句，保留人物、地点、关键事件与结果。";

/// 上下文压缩的收尾指令：作为摘要请求的**最后一条 user 消息**，追加在被遮蔽的原文之后。
///
/// 关键点：摘要请求复用会话的真前缀（原系统提示词 + 被遮蔽的那段原文），只在末尾追加这条
/// 指令——上一次请求的前缀就是它，供应商的热缓存直接复用，只有指令与输出未命中
///（DSH `compaction-basic/summarizer.ts` 的同款做法）。
pub const COMPACTION_INSTRUCTION_DEFAULT: &str = "你是 Octopus 的上下文压缩器：把上面那段对话压缩成一份结构化存档，供另一个模型接着演下去时不丢关键信息。

严格按下面的 Markdown 结构输出，小节一个不少、顺序不变；每节用短条目，不写散文段落；没有内容就写「（无）」。

## 原始诉求与目标
- [玩家一开始想做什么，以及后来怎么演变；措辞重要时原样引用]

## 世界与规则
- [世界前提、判定属性 / 资源、已生效的规则与文风要求]

## 人物现状
- [出场人物的身份、性格要点、与玩家的关系、当前处境；只写现在仍然成立的事实]

## 已发生的事件
- [按时间顺序的关键事件链条，每条含主体与结果]

## 已裁定的事实
- [导演或引擎确认过、后续不得推翻的既定事实]

## 当前场景
- [地点、在场角色、正在进行的遭遇或冲突]

## 未决线索
- [伏笔、承诺、任务，以及玩家提出但还没有结果的事]

## 最近一次输入与下一步
- [玩家最后说了 / 做了什么，模型接着要回应什么]

规则：
- 保留人物、地点、物品的准确 id（如 char-isa）与数值，不要改写它们。
- 忠于原文：不新增设定、不评价、不推测。
- 不要提到这次压缩，也不要输出任何与存档无关的内容。
- 如果上面已经有一份 <已压缩摘要> 块，那是更早的存档：不要照抄，把仍然成立的事实与更新的信息合并成一份。
- 只输出这份存档，不要调用任何工具。";

/// 游玩 AI 分节提示词（每个字段都是已解析的最终模板）。
#[derive(Debug, Clone)]
pub struct StoryBlockPrompts {
    pub memories: String,
    pub turn_feedback: String,
    pub canon: String,
    pub quests: String,
    pub scenes: String,
    pub encounters: String,
    pub personas: String,
    pub lore: String,
    pub world_premise: String,
    pub world_sections: String,
    pub directives: String,
    pub closing: String,
    pub focus: String,
    pub gm: String,
    pub attributes: String,
}

impl Default for StoryBlockPrompts {
    fn default() -> Self {
        Self {
            memories: BLOCK_MEMORIES_DEFAULT.to_string(),
            turn_feedback: BLOCK_TURN_FEEDBACK_DEFAULT.to_string(),
            canon: BLOCK_CANON_DEFAULT.to_string(),
            quests: BLOCK_QUESTS_DEFAULT.to_string(),
            scenes: BLOCK_SCENES_DEFAULT.to_string(),
            encounters: BLOCK_ENCOUNTERS_DEFAULT.to_string(),
            personas: BLOCK_PERSONAS_DEFAULT.to_string(),
            lore: BLOCK_LORE_DEFAULT.to_string(),
            world_premise: BLOCK_WORLD_PREMISE_DEFAULT.to_string(),
            world_sections: BLOCK_WORLD_SECTIONS_DEFAULT.to_string(),
            directives: BLOCK_DIRECTIVES_DEFAULT.to_string(),
            closing: BLOCK_CLOSING_DEFAULT.to_string(),
            focus: BLOCK_FOCUS_DEFAULT.to_string(),
            gm: BLOCK_GM_DEFAULT.to_string(),
            attributes: BLOCK_ATTRIBUTES_DEFAULT.to_string(),
        }
    }
}

/// 游玩 AI 提示词的稳定 key（config.json 的 prompts 用它作为键）。
pub mod keys {
    pub const STORY_PROTOCOL_TOOL: &str = "story.protocol.tool";
    pub const STORY_PROTOCOL_TEXT: &str = "story.protocol.text";
    pub const STORY_TURN_TEMPLATE: &str = "story.turn.template";
    pub const STORY_MEMORY_SUMMARY: &str = "story.memory.summary";
    pub const STORY_COMPACTION: &str = "story.compaction";
    pub const STORY_RETRY_TOOL: &str = "story.retry.tool";
    pub const STORY_RETRY_TEXT: &str = "story.retry.text";
    pub const STORY_BLOCK_MEMORIES: &str = "story.block.memories";
    pub const STORY_BLOCK_TURN_FEEDBACK: &str = "story.block.turn_feedback";
    pub const STORY_BLOCK_CANON: &str = "story.block.canon";
    pub const STORY_BLOCK_QUESTS: &str = "story.block.quests";
    pub const STORY_BLOCK_SCENES: &str = "story.block.scenes";
    pub const STORY_BLOCK_ENCOUNTERS: &str = "story.block.encounters";
    pub const STORY_BLOCK_PERSONAS: &str = "story.block.personas";
    pub const STORY_BLOCK_LORE: &str = "story.block.lore";
    pub const STORY_BLOCK_WORLD_PREMISE: &str = "story.block.world_premise";
    pub const STORY_BLOCK_WORLD_SECTIONS: &str = "story.block.world_sections";
    pub const STORY_BLOCK_DIRECTIVES: &str = "story.block.directives";
    pub const STORY_BLOCK_CLOSING: &str = "story.block.closing";
    pub const STORY_BLOCK_FOCUS: &str = "story.block.focus";
    pub const STORY_BLOCK_GM: &str = "story.block.gm";
    pub const STORY_BLOCK_ATTRIBUTES: &str = "story.block.attributes";
}

/// 游玩 / 聊天 AI 的全部可覆盖提示词；Default = 引擎内置默认（永远非空）。
#[derive(Debug, Clone)]
pub struct StoryPrompts {
    /// 工具模式系统提示词。
    pub protocol_tool: String,
    /// 文本模式系统提示词。
    pub protocol_text: String,
    /// 每回合用户提示词骨架。
    pub turn_template: String,
    /// 记忆压缩系统提示词。
    pub memory_summary: String,
    /// 上下文压缩（派生 surface 重写）的收尾指令。
    pub compaction: String,
    /// 解析失败纠正消息（工具模式）。
    pub retry_tool: String,
    /// 解析失败纠正消息（文本模式）。
    pub retry_text: String,
    pub blocks: StoryBlockPrompts,
}

impl Default for StoryPrompts {
    fn default() -> Self {
        Self {
            protocol_tool: PROTOCOL_TOOL_DEFAULT.to_string(),
            protocol_text: PROTOCOL_TEXT_DEFAULT.to_string(),
            turn_template: TURN_TEMPLATE_DEFAULT.to_string(),
            memory_summary: MEMORY_SUMMARY_DEFAULT.to_string(),
            compaction: COMPACTION_INSTRUCTION_DEFAULT.to_string(),
            retry_tool: RETRY_TOOL_DEFAULT.to_string(),
            retry_text: RETRY_TEXT_DEFAULT.to_string(),
            blocks: StoryBlockPrompts::default(),
        }
    }
}

/// 取配置里的覆盖文本：缺失 / 空白一律视为未覆盖。
fn override_of(prompts: &std::collections::BTreeMap<String, String>, key: &str) -> Option<String> {
    prompts
        .get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl StoryPrompts {
    /// 用「非空覆盖」覆盖默认值：空串 / 纯空白视为未覆盖。
    pub fn apply_overrides(&mut self, prompts: &std::collections::BTreeMap<String, String>) {
        if let Some(v) = override_of(prompts, keys::STORY_PROTOCOL_TOOL) {
            self.protocol_tool = v;
        }
        if let Some(v) = override_of(prompts, keys::STORY_PROTOCOL_TEXT) {
            self.protocol_text = v;
        }
        if let Some(v) = override_of(prompts, keys::STORY_TURN_TEMPLATE) {
            self.turn_template = v;
        }
        if let Some(v) = override_of(prompts, keys::STORY_MEMORY_SUMMARY) {
            self.memory_summary = v;
        }
        if let Some(v) = override_of(prompts, keys::STORY_COMPACTION) {
            self.compaction = v;
        }
        if let Some(v) = override_of(prompts, keys::STORY_RETRY_TOOL) {
            self.retry_tool = v;
        }
        if let Some(v) = override_of(prompts, keys::STORY_RETRY_TEXT) {
            self.retry_text = v;
        }
        let b = &mut self.blocks;
        macro_rules! apply {
            ($field:ident, $key:expr) => {
                if let Some(v) = override_of(prompts, $key) {
                    b.$field = v;
                }
            };
        }
        apply!(memories, keys::STORY_BLOCK_MEMORIES);
        apply!(turn_feedback, keys::STORY_BLOCK_TURN_FEEDBACK);
        apply!(canon, keys::STORY_BLOCK_CANON);
        apply!(quests, keys::STORY_BLOCK_QUESTS);
        apply!(scenes, keys::STORY_BLOCK_SCENES);
        apply!(encounters, keys::STORY_BLOCK_ENCOUNTERS);
        apply!(personas, keys::STORY_BLOCK_PERSONAS);
        apply!(lore, keys::STORY_BLOCK_LORE);
        apply!(world_premise, keys::STORY_BLOCK_WORLD_PREMISE);
        apply!(world_sections, keys::STORY_BLOCK_WORLD_SECTIONS);
        apply!(directives, keys::STORY_BLOCK_DIRECTIVES);
        apply!(closing, keys::STORY_BLOCK_CLOSING);
        apply!(focus, keys::STORY_BLOCK_FOCUS);
        apply!(gm, keys::STORY_BLOCK_GM);
        apply!(attributes, keys::STORY_BLOCK_ATTRIBUTES);
    }

    /// 某条 key 是否被覆盖（前端回显用）。
    pub fn is_overridden(prompts: &std::collections::BTreeMap<String, String>, key: &str) -> bool {
        override_of(prompts, key).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_known_and_keeps_unknown() {
        let out = render("A {{x}} B {{y}} C", &[("x", "1")]);
        assert_eq!(out, "A 1 B {{y}} C");
    }

    #[test]
    fn render_is_single_pass() {
        // 变量值里的 {{...}} 不再被展开（避免用户内容被当成模板）。
        let out = render("[{{a}}]", &[("a", "{{b}}"), ("b", "BOOM")]);
        assert_eq!(out, "[{{b}}]");
    }

    #[test]
    fn default_turn_template_renders_all_vars() {
        let vars: Vec<(&str, &str)> = TURN_TEMPLATE_VARS.iter().map(|(k, _)| (*k, "X")).collect();
        let out = render(TURN_TEMPLATE_DEFAULT, &vars);
        assert!(out.contains("【回合 X】"));
        assert!(!out.contains("{{"), "默认模板不应残留未替换的变量：{out}");
    }

    #[test]
    fn story_prompts_default_is_never_empty() {
        let p = StoryPrompts::default();
        assert!(p.protocol_tool.contains("AI 主持"));
        assert!(p.protocol_text.contains("JSON"));
        assert!(p.turn_template.contains("{{round}}"));
        assert!(p.memory_summary.contains("记忆压缩器"));
        assert!(p.compaction.contains("上下文压缩器"));
        assert!(p.blocks.personas.contains("【人物设定】"));
        assert!(p.blocks.gm.contains("导演模式"));
    }

    #[test]
    fn apply_overrides_uses_non_empty_only() {
        let mut p = StoryPrompts::default();
        let mut m = std::collections::BTreeMap::new();
        m.insert(keys::STORY_TURN_TEMPLATE.to_string(), "  ".to_string());
        m.insert(keys::STORY_BLOCK_PERSONAS.to_string(), "MY PERSONAS {{items}}".to_string());
        p.apply_overrides(&m);
        assert!(p.turn_template.contains("{{round}}"), "空白覆盖应视为未覆盖");
        assert_eq!(p.blocks.personas, "MY PERSONAS {{items}}");
    }
}
