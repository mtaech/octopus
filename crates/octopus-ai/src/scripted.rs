//! 脚本化 AiProvider：确定性、零依赖，供冒烟与确定性重放测试（#20 ④）。

use async_trait::async_trait;
use octopus_engine::{AiOutput, AiProvider, EngineError, TurnContext};
use octopus_types::Intent;

pub struct ScriptedProvider;

#[async_trait]
impl AiProvider for ScriptedProvider {
    async fn story_intents(&self, ctx: &TurnContext) -> Result<AiOutput, EngineError> {
        let t = &ctx.player_text;
        let mut out = Vec::new();
        if t.contains("观察") || t.contains("调查") || t.contains("查看") {
            out.push(Intent::Check {
                attribute: "wit".into(),
                difficulty: Some(12),
                actor_id: None,
                // 判定 C3：脚本化 provider 不发起对抗判定（None = 旧行为逐字不变）。
                opponent_id: None,
            });
        } else if t.contains("梦") || t.contains("传闻") {
            out.push(Intent::Narrate {
                content: "伊莎压低声音，油灯的火苗晃了一下。".into(),
                actor_id: None,
            });
        } else {
            out.push(Intent::Narrate {
                content: format!("酒馆里的喧闹照旧，「{t}」像投进湖面的石子，短暂地起了涟漪。"),
                actor_id: None,
            });
        }
        Ok(AiOutput {
            intents: out.into_iter().map(Into::into).collect(),
            reasoning: None,
            intent_warnings: vec![],
            trace: None,
            compaction: None,
        })
    }

    
}
