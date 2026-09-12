//! 运行时世界状态（#06 ① 扁平分面 + id 互引），可物化为投影。

use std::collections::BTreeMap;

use octopus_types::{
    CharacterInstance, EncounterView, ProjectionMeta, QuestView, Seq, SkeletonProgress,
    StatusInstance, WorldProjection,
};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct WorldState {
    pub seq: Seq,
    pub scene_id: String,
    pub scene_title: String,
    pub scene_description: Option<String>,
    pub controlled: Vec<String>,
    pub characters: BTreeMap<String, CharacterInstance>,
    pub flags: BTreeMap<String, Value>,
    /// 结构化遭遇（导演创建）；value 是 EncounterView 形态的 JSON。
    pub encounters: BTreeMap<String, Value>,
    pub progress: SkeletonProgress,
    pub locations: Vec<Value>,
    pub meta: ProjectionMeta,
    pub rng_seed: u64,
}

impl WorldState {
    pub fn projection(&self) -> WorldProjection {
        let characters = self
            .characters
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap_or(Value::Null)))
            .collect();
        let flags = self.flags.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        // 任务 = 带 text 的 goal 条目（骨架目标在 storybook 里，导演新增的带完整对象）
        let quests: Vec<QuestView> = self
            .progress
            .goals
            .iter()
            .filter_map(|(id, v)| {
                let text = v.get("text")?.as_str()?.to_string();
                Some(QuestView {
                    id: id.clone(),
                    text,
                    done: v.get("done").and_then(Value::as_bool).unwrap_or(false),
                    source: v
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or("gm")
                        .to_string(),
                    hidden: v.get("hidden").and_then(Value::as_bool).unwrap_or(false),
                    primary: v.get("primary").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect();
        let encounters: Vec<EncounterView> = self
            .encounters
            .values()
            .filter_map(|v| serde_json::from_value::<EncounterView>(v.clone()).ok())
            .collect();
        WorldProjection {
            seq: self.seq,
            scene_id: self.scene_id.clone(),
            scene_title: self.scene_title.clone(),
            characters,
            controlled: self.controlled.clone(),
            flags,
            progress: self.progress.clone(),
            quests,
            encounters,
            locations: self.locations.clone(),
            meta: self.meta.clone(),
        }
    }

    pub fn push_status(&mut self, instance_id: &str, status: StatusInstance) {
        if let Some(c) = self.characters.get_mut(instance_id) {
            c.statuses.push(status);
        }
    }
}
