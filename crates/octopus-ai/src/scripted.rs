//! 脚本化 AiProvider：确定性、零依赖，供冒烟与确定性重放测试（#20 ④）。

use async_trait::async_trait;
use octopus_engine::{AiProvider, EngineError, TurnContext};
use octopus_types::Intent;

pub struct ScriptedProvider;

#[async_trait]
impl AiProvider for ScriptedProvider {
    async fn story_intents(&self, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError> {
        let t = &ctx.player_text;
        let mut out = Vec::new();
        if t.contains("观察") || t.contains("调查") || t.contains("查看") {
            out.push(Intent::Check { attribute: "wit".into(), difficulty: Some(12) });
        } else if t.contains("梦") || t.contains("传闻") {
            out.push(Intent::Narrate {
                content: "伊莎压低声音，油灯的火苗晃了一下。".into(),
            });
        } else {
            out.push(Intent::Narrate {
                content: format!("酒馆里的喧闹照旧，「{t}」像投进湖面的石子，短暂地起了涟漪。"),
            });
        }
        Ok(out)
    }

    async fn character_intents(&self, ctx: &TurnContext) -> Result<Vec<Intent>, EngineError> {
        let t = &ctx.player_text;
        let mut out = Vec::new();
        if t.contains("梦") || t.contains("传闻") {
            out.push(Intent::Speak {
                content: "你也听说了？这三日镇上没几个睡踏实的。".into(),
                tone: None,
            });
            out.push(Intent::Emote {
                content: "伊莎说着，下意识摸了摸手腕上一道旧疤。".into(),
                emotion: Some("concern".into()),
            });
        } else if t.contains("你好") || t.contains("嗨") {
            out.push(Intent::Speak {
                content: "稀客呀。坐下来喝一杯，第一杯算我的。".into(),
                tone: None,
            });
        } else {
            out.push(Intent::Speak {
                content: "说来话长，不过你算是问对人了。".into(),
                tone: None,
            });
        }
        Ok(out)
    }
}
