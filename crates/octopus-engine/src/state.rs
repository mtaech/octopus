//! 运行时世界状态（#06 ① 扁平分面 + id 互引），可物化为投影。

use std::collections::BTreeMap;

use octopus_types::{
    CharacterInstance, EncounterView, ProjectionMeta, QuestView, Seq, SkeletonProgress,
    StatusInstance, WorldProjection,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 运行时世界状态。
///
/// 序列化用于「新原点 / 升级基座」的全量检查点（#14）：把当前状态写进命令日志
/// 的一条 delta，重放时原样恢复。日志被归档或故事书换版后，检查点保证重放有确定基线。
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// 已消耗的 RNG 取值次数（#06 ②）：由命令日志的 rng_consume 事件重放累加。
    ///
    /// 随世界状态一起物化，快照 / 新原点检查点据此把 RNG 拨回同一位置，保证跨重启
    /// 与「快照 + 其后命令」两条路径的骰序一致。旧快照 / 检查点缺省为 0。
    #[serde(default)]
    pub rng_position: u64,
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
