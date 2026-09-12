//! 演示种子故事书（真实编辑器落盘前，供开档与冒烟用）。

use serde_json::{json, Value};

pub struct SeedStorybook {
    pub id: String,
    pub title: String,
    pub revision: u32,
    pub json: Value,
}

pub fn seed_storybooks() -> Vec<SeedStorybook> {
    vec![SeedStorybook {
        id: "sb-fallingstar".into(),
        title: "坠星谷 · 酒馆之夜".into(),
        revision: 1,
        json: json!({
            "schema_version": 3,
            "meta": { "id": "sb-fallingstar", "title": "坠星谷 · 酒馆之夜", "author": "星尘旅人", "language": "zh-CN",
                      "description": "边境小镇坠星谷，一颗流星坠落后，夜晚变得不再平静。" },
            "world": {
                "premise": "坠星谷是群山环抱的边境小镇，三日前一颗流星坠落在镇外废矿坑，镇民开始做同一个怪梦。你是路过的旅人，被酒馆老板娘伊莎收留过夜。",
                "opening": "夜里的雨敲打着碎星酒馆的窗。你推开木门，油灯光一晃，老板娘伊莎抬眼打量你：「稀客。先坐下来，喝一杯暖暖身子。」你抖落斗篷上的水珠，在她对面坐下——坠星谷的怪梦，刚刚开始。",
                "locations": [
                    { "id": "loc-tavern", "name": "碎星酒馆", "description": "镇中心的老酒馆。" },
                    { "id": "loc-mine", "name": "废矿坑", "description": "流星坠落处，被镇公所围起。" }
                ],
                "resources": [ { "id": "res-gold", "name": "金币", "type": "numerical", "default_max": 999 } ]
            },
            "attribute_dimensions": [
                { "key": "str", "label": "力量", "type": "number", "min": 0, "max": 100, "baseline": 50 },
                { "key": "wit", "label": "机敏", "type": "number", "min": 0, "max": 100, "baseline": 50 },
                { "key": "cha", "label": "魅力", "type": "number", "min": 0, "max": 100, "baseline": 50 }
            ],
            "skeleton": [ { "id": "ch-1", "title": "第一章 · 流星之夜", "scenes": [
                { "id": "sc-tavern-night", "title": "碎星酒馆的夜晚", "location_id": "loc-tavern",
                  "present_char_ids": ["char-mira", "char-isa", "char-oden", "char-kael"],
                  "goals": [ { "id": "g1", "text": "在酒馆打听到怪梦的传闻", "primary": true, "condition": { "op": "flag_set", "flag": "heard_dreams" } } ],
                  "triggers": [ { "id": "b1", "title": "梦的怪象", "hint": "镇民说着同一个怪梦。", "condition": { "op": "flag_set", "flag": "heard_dreams" }, "repeatable": true } ] }
            ] } ],
            "characters": [
                { "id": "char-mira", "name": "米拉", "kind": "pc", "background": "流浪的赏金猎人。", "personality": "冷静寡言。", "attributes": { "str": 55, "wit": 75, "cha": 45 }, "resources": { "res-gold": 32 } },
                { "id": "char-isa", "name": "伊莎", "kind": "npc", "background": "碎星酒馆老板娘。", "personality": "热情圆滑。", "attributes": { "str": 40, "wit": 80, "cha": 85 } },
                { "id": "char-oden", "name": "奥登", "kind": "npc", "background": "星辰教堂执事。", "personality": "温和谨慎。", "attributes": { "str": 45, "wit": 70, "cha": 65 } },
                { "id": "char-kael", "name": "凯尔", "kind": "npc", "background": "镇上的年轻铁匠。", "personality": "冲动直率。", "attributes": { "str": 78, "wit": 40, "cha": 50 } }
            ],
            "skills": [], "items": [], "objects": [], "factions": [], "relationships": [],
            "statuses": [],
            "flags": [ { "key": "heard_dreams", "label": "听闻怪梦" }, { "key": "met_isa", "label": "结识伊莎" } ],
            "events": [ { "key": "scene_change", "label": "场景切换" } ],
            "relationship_types": [ { "key": "好感", "label": "好感" } ],
            "target_types": [ { "key": "single", "label": "单体" } ]
        }),
    }]
}
