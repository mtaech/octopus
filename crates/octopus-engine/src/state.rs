//! 运行时世界状态（#06 ① 扁平分面 + id 互引），可物化为投影。

use std::collections::BTreeMap;

use octopus_types::{
    BudgetView, CharacterInstance, EncounterView, ProjectionMeta, QuestView, Seq, SkeletonProgress,
    StatusInstance, TurnView, WorldProjection,
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
    /// 技能冷却起点（#01）：实例键 → 技能 id → 上次使用所在回合号。
    ///
    /// 只由 \`apply_delta\`（Character 域的 \`cooldown.<skill_id>\` 字段）写入，随命令日志重放，
    /// 所以重开进程 / 重放后冷却不会凭空消失。旧存档缺省为空表。
    #[serde(default)]
    pub cooldowns: BTreeMap<String, BTreeMap<String, u32>>,
    pub rng_seed: u64,
    /// 已消耗的 RNG 取值次数（#06 ②）：由命令日志的 rng_consume 事件重放累加。
    ///
    /// 随世界状态一起物化，快照 / 新原点检查点据此把 RNG 拨回同一位置，保证跨重启
    /// 与「快照 + 其后命令」两条路径的骰序一致。旧快照 / 检查点缺省为 0。
    #[serde(default)]
    pub rng_position: u64,
    /// 时序运行时状态（#GAP-I）；`order` 为空即「不在时序中」。随命令日志重放。
    #[serde(default)]
    pub turn: TurnState,
}

/// 时序运行时状态（#GAP-I）：顺序 + 指针 + 每角色剩余预算 + 跳过次数。
///
/// **只由 \`DeltaDomain::Turn\` 的 delta 变更**（\`apply_delta\` 是唯一变更路径），随命令日志重放；
/// 旧存档 / 旧快照缺省为空。**「是否在时序中」是派生态**——「故事书声明了 world.turn」
/// 且「当前场景有未结束的遭遇」，不额外存一个可能与遭遇不同步的 active 字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// 时序轮（1 起）：与「玩家输入次数」的全局 round 不是一回事。
    pub round: u32,
    /// 先攻顺序（角色实例键，按行动先后）。
    pub order: Vec<String>,
    /// 当前行动者在 \`order\` 中的下标。
    pub index: usize,
    /// 实例键 → 预算 id → 剩余。
    pub budgets: BTreeMap<String, BTreeMap<String, i64>>,
    /// 实例键 → 还要跳过的时序回合数（「失去回合」的载体）。
    pub skip: BTreeMap<String, u32>,
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
                    // 导演新增的任务不属于任何场景 → 没有可继承的地点（地图 P5）。
                    location_id: None,
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
            turn: self.turn_view(),
            locations: self.locations.clone(),
            meta: self.meta.clone(),
        }
    }

    /// 时序投影（#GAP-I）：没有顺序（不在时序中）时 None。
    ///
    /// 显示名优先取实例的 name；实例已不在表里（旧日志 / 实例被清理）时回落实例键本身，
    /// 绝不因为一个查不到的键丢掉整段时序信息。
    pub fn turn_view(&self) -> Option<TurnView> {
        if self.turn.order.is_empty() {
            return None;
        }
        let name_of = |k: &String| {
            self.characters
                .get(k)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| k.clone())
        };
        let current_key = self.turn.order.get(self.turn.index);
        let budgets = current_key
            .and_then(|k| self.turn.budgets.get(k))
            .map(|m| {
                m.iter()
                    .map(|(id, left)| BudgetView { id: id.clone(), left: *left })
                    .collect()
            })
            .unwrap_or_default();
        Some(TurnView {
            round: self.turn.round,
            order: self.turn.order.iter().map(name_of).collect(),
            current: current_key.map(name_of),
            budgets,
        })
    }

    /// 插入 / 替换一个**完整角色实例**（图鉴 M2）：怪物克隆走 delta 的唯一变更路径。
    ///
    /// 与 `push_status` 之类的「改已存在项」不同，实例表原本没有任何插入入口——
    /// 图鉴模板不在开档时实例化（`build_state` 跳过 kind=monster），遭遇创建时才克隆。
    pub fn upsert_instance(&mut self, key: &str, instance: CharacterInstance) {
        self.characters.insert(key.to_string(), instance);
    }

    pub fn push_status(&mut self, instance_id: &str, status: StatusInstance) {
        if let Some(c) = self.characters.get_mut(instance_id) {
            c.statuses.push(status);
        }
    }
}
