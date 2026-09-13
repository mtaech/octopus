//! resolve：结算与判定分档（#12 / #20 被调层）。
//!
//! 只被 command 调（#20 ②）。判定器有两种来源，都收敛到这里：
//! - 声明式配置：骰子表达式 + 比较模式 + 属性修正 + 成功度阈值。
//! - Lua 判定脚本：归一化输出 total / margin，档位仍由引擎按阈值分。
//!
//! RNG 一律走引擎的 DeterministicRng，保证可重放（#12 ④）。

use octopus_types::{CheckKind, CheckMode, CheckerDef, SuccessLevel};

use crate::error::EngineError;
use crate::lua_host::{LuaHost, LuaHostContext};
use crate::rng::DeterministicRng;

/// 默认成功度阈值：>=+10 大成功 / >=0 成功 / >=-10 勉强 / 其余失败。
pub const DEFAULT_DEGREE_THRESHOLDS: [i64; 3] = [10, 0, -10];
/// 属性中心偏移公式的基线。
pub const DEFAULT_BASELINE: f64 = 50.0;

/// 一次判定的完整结果（对应 CheckResultPayload，但不含 actor）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCheck {
    pub attribute: String,
    /// 骰子表达式（无骰 / Lua 判定为 None）。
    pub expr: Option<String>,
    pub rolls: Vec<i64>,
    /// 属性 / 固定修正。
    pub r#mod: i64,
    pub total: i64,
    pub target: i64,
    pub margin: i64,
    pub result: bool,
    pub level: SuccessLevel,
    /// 是否真的掷了骰：false 表示无骰判定，应交由 AI 叙事裁决。
    pub rolled: bool,
    /// 判定种类（#3）。
    pub kind: CheckKind,
}

/// 骰子表达式求值结果。
#[derive(Debug, Clone, PartialEq)]
pub struct DiceRoll {
    pub expr: String,
    pub rolls: Vec<i64>,
    /// 非骰子的常数项之和（含符号）。
    pub flat: i64,
    pub total: i64,
}

/// 求值骰子表达式：支持 NdM / dM / 常数，以及 +/- 串联（如 1d20+2d6-1）。
pub fn roll_dice(expr: &str, rng: &mut DeterministicRng) -> Result<DiceRoll, EngineError> {
    let normalized = expr.trim().to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    let mut i = 0usize;
    let mut sign = 1i64;
    let mut rolls: Vec<i64> = Vec::new();
    let mut flat = 0i64;
    let mut dice_sum = 0i64;
    let mut saw_term = false;

    while i < bytes.len() {
        match bytes[i] {
            b' ' => i += 1,
            b'+' => {
                sign = 1;
                i += 1;
            }
            b'-' => {
                sign = -1;
                i += 1;
            }
            _ => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let leading: Option<i64> = if i > start {
                    Some(normalized[start..i].parse::<i64>().map_err(|_| {
                        EngineError::Internal(format!("无法解析骰子表达式: {expr}"))
                    })?)
                } else {
                    None
                };

                if i < bytes.len() && bytes[i] == b'd' {
                    i += 1;
                    let sides_start = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == sides_start {
                        return Err(EngineError::Internal(format!("骰子表达式缺少面数: {expr}")));
                    }
                    let sides: i64 = normalized[sides_start..i].parse().map_err(|_| {
                        EngineError::Internal(format!("无法解析骰子面数: {expr}"))
                    })?;
                    let count = leading.unwrap_or(1);
                    if !(1..=1000).contains(&count) || !(1..=1_000_000).contains(&sides) {
                        return Err(EngineError::Internal(format!("骰子表达式超出范围: {expr}")));
                    }
                    for _ in 0..count {
                        let roll = rng.range_inclusive(1, sides);
                        rolls.push(roll);
                        dice_sum += sign * roll;
                    }
                    saw_term = true;
                    sign = 1;
                } else {
                    let value = leading.ok_or_else(|| {
                        EngineError::Internal(format!("无法解析骰子表达式: {expr}"))
                    })?;
                    flat += sign * value;
                    saw_term = true;
                    sign = 1;
                }
            }
        }
    }

    if !saw_term {
        return Err(EngineError::Internal(format!("空的骰子表达式: {expr}")));
    }

    Ok(DiceRoll {
        expr: expr.trim().to_string(),
        rolls,
        flat,
        total: flat + dice_sum,
    })
}

/// 修正配置：由属性维度声明决定默认中心偏移（基线 / 范围 / 步长）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModifierProfile {
    pub baseline: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// 修正步长：modifier = floor((clamp(v) - baseline) / step)。默认 5。
    pub step: f64,
}

impl Default for ModifierProfile {
    fn default() -> Self {
        Self { baseline: DEFAULT_BASELINE, min: None, max: None, step: 5.0 }
    }
}

impl ModifierProfile {
    /// 从故事书的 attribute_dimensions 取某个维度的修正配置；缺失则回落默认。
    pub fn for_storybook(storybook: &serde_json::Value, attribute: &str) -> Self {
        let Some(dims) = storybook.get("attribute_dimensions").and_then(|v| v.as_array()) else {
            return Self::default();
        };
        let Some(d) = dims
            .iter()
            .find(|d| d.get("key").and_then(|v| v.as_str()) == Some(attribute))
        else {
            return Self::default();
        };
        let num = |k: &str| d.get(k).and_then(|v| v.as_f64());
        let step = num("modifier_step").filter(|s| *s > 0.0).unwrap_or(5.0);
        Self {
            baseline: num("baseline").unwrap_or(DEFAULT_BASELINE),
            min: num("min"),
            max: num("max"),
            step,
        }
    }

    /// 该维度允许的修正范围（由 min / max / baseline / step 推出，用于夹取）。
    pub fn modifier_bounds(&self) -> (i64, i64) {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) => {
                let a = ((lo - self.baseline) / self.step).floor() as i64;
                let b = ((hi - self.baseline) / self.step).floor() as i64;
                (a.min(b), a.max(b))
            }
            _ => (-10, 10),
        }
    }
}

/// 属性 → 修正：优先 attribute_modifier 固定映射；否则按维度声明的
/// 基线 / 范围 / 步长算 floor((clamp(v) - baseline) / step)，再夹到该维度的修正范围。
pub fn modifier_for(checker: &CheckerDef, attribute: &str, value: f64, profile: ModifierProfile) -> i64 {
    if let Some(map) = &checker.attribute_modifier {
        if let Some(fixed) = map.get(attribute) {
            return *fixed;
        }
    }
    let mut v = value;
    if let Some(lo) = profile.min {
        v = v.max(lo);
    }
    if let Some(hi) = profile.max {
        v = v.min(hi);
    }
    let (lo, hi) = profile.modifier_bounds();
    (((v - profile.baseline) / profile.step).floor() as i64).clamp(lo, hi)
}

/// 判定器生效的成功度阈值（不足 3 档回落默认）。
pub fn degree_thresholds(checker: &CheckerDef) -> &[i64] {
    match &checker.degree_thresholds {
        Some(t) if t.len() >= 3 => t.as_slice(),
        _ => &DEFAULT_DEGREE_THRESHOLDS,
    }
}

/// 按差值分档（与骰面无关）。
pub fn level_for_margin(margin: i64, thresholds: &[i64]) -> SuccessLevel {
    let t = if thresholds.len() >= 3 { thresholds } else { &DEFAULT_DEGREE_THRESHOLDS };
    if margin >= t[0] {
        SuccessLevel::Great
    } else if margin >= t[1] {
        SuccessLevel::Success
    } else if margin >= t[2] {
        SuccessLevel::Barely
    } else {
        SuccessLevel::Fail
    }
}

/// 给已解析的判定追加静态修正（同名 modifier 取 max 后由调用方传入），并重算 total / margin / 结果 / 档位。
pub fn apply_check_bonus(
    check: &mut ResolvedCheck,
    bonus: i64,
    mode: CheckMode,
    thresholds: &[i64],
) {
    if bonus == 0 {
        return;
    }
    check.r#mod += bonus;
    check.total += bonus;
    check.margin = check.total - check.target;
    check.result = match mode {
        CheckMode::Lte => check.total <= check.target,
        CheckMode::Gte | CheckMode::Opposed => check.total >= check.target,
    };
    check.level = level_for_margin(check.margin, thresholds);
}

/// 声明式判定：掷骰（可选）+ 属性修正 → total / margin / 档位。
/// 判定骰式：显式 dice 优先；否则把 type（如 "d20"）规范成骰式。
///
/// 返回 None = 无骰（被动判定 / 由 AI 依属性叙事裁决）。
pub fn checker_dice(checker: &CheckerDef) -> Option<String> {
    if let Some(d) = checker.dice.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(d.to_string());
    }
    let t = checker.r#type.as_deref()?.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    // "d20" / "d100"：补默认骰数 1。
    if let Some(n) = t.strip_prefix('d') {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("1d{n}"));
        }
    }
    // "3d6" / "1d100"：已是骰式，原样。
    if let Some((a, b)) = t.split_once('d') {
        if !a.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && !b.is_empty()
            && b.chars().all(|c| c.is_ascii_digit())
        {
            return Some(t);
        }
    }
    None
}

pub fn resolve_declarative_check(
    checker: &CheckerDef,
    attribute: &str,
    attribute_value: f64,
    difficulty: i64,
    profile: ModifierProfile,
    rng: &mut DeterministicRng,
) -> Result<ResolvedCheck, EngineError> {
    let mode = checker.mode.unwrap_or(CheckMode::Gte);
    let kind = checker.kind.unwrap_or(CheckKind::Attribute);
    let r#mod = modifier_for(checker, attribute, attribute_value, profile);

    // 被动值：不掷骰，直接 passive_base（缺省 10）+ 修正。
    let (expr, rolls, dice_total, rolled) = if kind == CheckKind::Passive {
        (None, Vec::new(), 0, false)
    } else {
        match checker_dice(checker) {
            Some(dice) => {
                let roll = roll_dice(&dice, rng)?;
                (Some(roll.expr), roll.rolls, roll.total, true)
            }
            None => (None, Vec::new(), 0, false),
        }
    };
    let passive_base = if kind == CheckKind::Passive {
        checker.passive_base.unwrap_or(10)
    } else {
        0
    };

    let total = dice_total + r#mod + passive_base;
    let target = difficulty;
    let margin = total - target;
    let result = match mode {
        CheckMode::Lte => total <= target,
        // gte / opposed 由调用方给出对抗方数值后同样比较 >=
        CheckMode::Gte | CheckMode::Opposed => total >= target,
    };
    let level = level_for_margin(margin, degree_thresholds(checker));

    Ok(ResolvedCheck {
        attribute: attribute.to_string(),
        expr,
        rolls,
        r#mod,
        total,
        target,
        margin,
        result,
        level,
        rolled,
        kind,
    })
}

/// Lua 判定：脚本返回归一化 { total, margin }，target 由 total - margin 反推，档位仍由引擎分。
pub fn resolve_lua_check(
    host: &LuaHost,
    script: &str,
    ctx: &LuaHostContext,
    mode: CheckMode,
    thresholds: &[i64],
    kind: CheckKind,
) -> Result<ResolvedCheck, EngineError> {
    let outcome = host.run_check(script, ctx)?;
    let target = outcome.total - outcome.margin;
    let result = match mode {
        CheckMode::Lte => outcome.total <= target,
        CheckMode::Gte | CheckMode::Opposed => outcome.total >= target,
    };
    Ok(ResolvedCheck {
        attribute: String::new(),
        expr: None,
        rolls: Vec::new(),
        r#mod: 0,
        total: outcome.total,
        target,
        margin: outcome.margin,
        result,
        level: level_for_margin(outcome.margin, thresholds),
        rolled: true,
        kind,
    })
}

/// 判定器分派：声明了 lua 脚本就走 Lua，否则走声明式配置。
pub fn resolve_checker(
    checker: &CheckerDef,
    attribute: &str,
    attribute_value: f64,
    difficulty: i64,
    profile: ModifierProfile,
    rng: &mut DeterministicRng,
    lua: Option<(&LuaHost, &LuaHostContext)>,
) -> Result<ResolvedCheck, EngineError> {
    if let Some(script) = checker.lua.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let (host, ctx) = lua
            .ok_or_else(|| EngineError::Lua("判定器声明了 lua 脚本，但未提供 LuaHost".to_string()))?;
        let mut out = resolve_lua_check(
            host,
            script,
            ctx,
            checker.mode.unwrap_or(CheckMode::Gte),
            degree_thresholds(checker),
            checker.kind.unwrap_or(CheckKind::Attribute),
        )?;
        out.attribute = attribute.to_string();
        return Ok(out);
    }
    resolve_declarative_check(checker, attribute, attribute_value, difficulty, profile, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(script_id: &str) -> LuaHostContext {
        LuaHostContext { script_id: script_id.into(), actor: json!({}), ..Default::default() }
    }

    #[test]
    fn dice_expression_parses_and_rolls() {
        let mut rng = DeterministicRng::new(1);
        let roll = roll_dice("2d6+3", &mut rng).unwrap();
        assert_eq!(roll.expr, "2d6+3");
        assert_eq!(roll.rolls.len(), 2);
        assert!(roll.rolls.iter().all(|r| (1..=6).contains(r)));
        assert_eq!(roll.total, roll.rolls.iter().sum::<i64>() + 3);
        assert_eq!(roll.flat, 3);
    }

    #[test]
    fn dice_expression_supports_compound_and_bare_d() {
        let mut rng = DeterministicRng::new(9);
        let roll = roll_dice("1d20+2d6-1", &mut rng).unwrap();
        assert_eq!(roll.rolls.len(), 3);
        assert_eq!(roll.flat, -1);
        assert_eq!(roll.total, roll.rolls.iter().sum::<i64>() - 1);
        let mut rng = DeterministicRng::new(9);
        let bare = roll_dice("d20", &mut rng).unwrap();
        assert_eq!(bare.rolls.len(), 1);
        assert!(roll_dice("", &mut rng).is_err());
        assert!(roll_dice("2d", &mut rng).is_err());
    }

    #[test]
    fn modifier_uses_map_then_center_formula() {
        let mut checker = CheckerDef { dice: Some("1d20".into()), ..Default::default() };
        assert_eq!(modifier_for(&checker, "str", 70.0, ModifierProfile::default()), 4);
        assert_eq!(modifier_for(&checker, "str", 30.0, ModifierProfile::default()), -4);
        assert_eq!(modifier_for(&checker, "str", 0.0, ModifierProfile::default()), -10);
        checker.attribute_modifier = Some([("str".to_string(), 7)].into_iter().collect());
        assert_eq!(modifier_for(&checker, "str", 30.0, ModifierProfile::default()), 7);
    }

    /// B：故事书写 type: "d20" 也要掷骰（不再静默无骰 → 0 分必失败）。
    #[test]
    fn checker_dice_accepts_type_alias() {
        assert_eq!(checker_dice(&CheckerDef::default()), None);
        assert_eq!(
            checker_dice(&CheckerDef { r#type: Some("d20".into()), ..Default::default() }),
            Some("1d20".to_string())
        );
        assert_eq!(
            checker_dice(&CheckerDef { r#type: Some("3d6".into()), ..Default::default() }),
            Some("3d6".to_string())
        );
        assert_eq!(
            checker_dice(&CheckerDef { r#type: Some("attribute".into()), ..Default::default() }),
            None,
            "非骰式类型名不当作骰式"
        );
        assert_eq!(
            checker_dice(&CheckerDef {
                dice: Some("1d100".into()),
                r#type: Some("d20".into()),
                ..Default::default()
            }),
            Some("1d100".to_string()),
            "显式 dice 优先于 type 别名"
        );

        let checker = CheckerDef { r#type: Some("d20".into()), ..Default::default() };
        let mut rng = DeterministicRng::new(7);
        let r = resolve_declarative_check(&checker, "agi", 14.0, 12, ModifierProfile::default(), &mut rng)
            .unwrap();
        assert_eq!(r.expr.as_deref(), Some("1d20"));
        assert_eq!(r.rolls.len(), 1);
        assert!(r.rolled);
    }

    #[test]
    fn success_levels_bucket_by_margin() {
        assert_eq!(level_for_margin(15, &DEFAULT_DEGREE_THRESHOLDS), SuccessLevel::Great);
        assert_eq!(level_for_margin(5, &DEFAULT_DEGREE_THRESHOLDS), SuccessLevel::Success);
        assert_eq!(level_for_margin(-5, &DEFAULT_DEGREE_THRESHOLDS), SuccessLevel::Barely);
        assert_eq!(level_for_margin(-20, &DEFAULT_DEGREE_THRESHOLDS), SuccessLevel::Fail);
    }

    #[test]
    fn declarative_check_is_deterministic_and_consistent() {
        let checker = CheckerDef {
            dice: Some("1d20".into()),
            mode: Some(CheckMode::Gte),
            ..Default::default()
        };
        let mut a = DeterministicRng::new(2024);
        let mut b = DeterministicRng::new(2024);
        let ra = resolve_declarative_check(&checker, "str", 70.0, 12, ModifierProfile::default(), &mut a).unwrap();
        let rb = resolve_declarative_check(&checker, "str", 70.0, 12, ModifierProfile::default(), &mut b).unwrap();
        assert_eq!(ra, rb);
        assert_eq!(ra.r#mod, 4);
        assert_eq!(ra.target, 12);
        assert_eq!(ra.margin, ra.total - 12);
        assert_eq!(ra.result, ra.total >= 12);
        assert_eq!(ra.level, level_for_margin(ra.margin, &DEFAULT_DEGREE_THRESHOLDS));
        assert!(ra.rolled);
    }

    #[test]
    fn lua_check_normalizes_and_buckets() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx("check-1");
        let out = resolve_lua_check(
            &host,
            "return { total = 18, margin = 6 }",
            &c,
            CheckMode::Gte,
            &DEFAULT_DEGREE_THRESHOLDS,
            CheckKind::Attribute,
        )
        .unwrap();
        assert_eq!(out.total, 18);
        assert_eq!(out.target, 12);
        assert_eq!(out.margin, 6);
        assert!(out.result);
        assert_eq!(out.level, SuccessLevel::Success);
    }

    #[test]
    fn dispatcher_prefers_lua_then_falls_back() {
        let host = LuaHost::new(1).unwrap();
        let c = ctx("check-2");
        let lua_checker = CheckerDef {
            dice: Some("1d20".into()),
            lua: Some("return { total = 3, margin = -9 }".into()),
            ..Default::default()
        };
        let mut rng = DeterministicRng::new(1);
        let out = resolve_checker(&lua_checker, "str", 70.0, 12, ModifierProfile::default(), &mut rng, Some((&host, &c))).unwrap();
        assert_eq!(out.attribute, "str");
        assert_eq!(out.total, 3);
        assert_eq!(out.level, SuccessLevel::Barely);

        let decl = CheckerDef { dice: Some("1d20".into()), ..Default::default() };
        let mut rng = DeterministicRng::new(1);
        let out = resolve_checker(&decl, "str", 70.0, 12, ModifierProfile::default(), &mut rng, None).unwrap();
        assert!(out.rolled);
        assert_eq!(out.attribute, "str");
    }

    #[test]
    fn lua_checker_without_host_errors() {
        let checker =
            CheckerDef { lua: Some("return { total = 1, margin = 0 }".into()), ..Default::default() };
        let mut rng = DeterministicRng::new(1);
        assert!(resolve_checker(&checker, "str", 50.0, 10, ModifierProfile::default(), &mut rng, None).is_err());
    }

    #[test]
    fn passive_check_is_base_plus_bonus_without_roll() {
        let mut modifier = std::collections::BTreeMap::new();
        modifier.insert("wis".to_string(), 1);
        let checker = CheckerDef {
            kind: Some(CheckKind::Passive),
            passive_base: Some(10),
            attribute_modifier: Some(modifier),
            ..Default::default()
        };
        let mut rng = DeterministicRng::new(1);
        let consumed_before = rng.consumed.len();
        let out = resolve_declarative_check(&checker, "wis", 13.0, 15, ModifierProfile::default(), &mut rng).unwrap();
        assert_eq!(out.total, 11);
        assert!(!out.rolled);
        assert_eq!(out.kind, CheckKind::Passive);
        assert!(!out.result);
        assert_eq!(rng.consumed.len(), consumed_before, "被动判定不应消耗随机数");
    }

    #[test]
    fn save_check_uses_save_bonus() {
        let mut modifier = std::collections::BTreeMap::new();
        modifier.insert("con".to_string(), 2);
        let checker = CheckerDef {
            kind: Some(CheckKind::Save),
            dice: Some("1d20".into()),
            attribute_modifier: Some(modifier),
            ..Default::default()
        };
        let mut rng = DeterministicRng::new(3);
        let out = resolve_declarative_check(&checker, "con", 15.0, 13, ModifierProfile::default(), &mut rng).unwrap();
        assert!(out.rolled);
        assert_eq!(out.kind, CheckKind::Save);
        assert_eq!(out.r#mod, 2);
        assert_eq!(out.total, out.rolls[0] + 2);
    }

    #[test]
    fn modifier_uses_dimension_profile() {
        // D&D：1..30，基线 10，步长 2 → 调整值 -5..+10
        let profile = ModifierProfile { baseline: 10.0, min: Some(1.0), max: Some(30.0), step: 2.0 };
        let checker = CheckerDef::default();
        assert_eq!(modifier_for(&checker, "dex", 14.0, profile), 2);
        assert_eq!(modifier_for(&checker, "int", 16.0, profile), 3);
        assert_eq!(modifier_for(&checker, "cha", 9.0, profile), -1);
        assert_eq!(modifier_for(&checker, "str", 1.0, profile), -5);
        assert_eq!(modifier_for(&checker, "con", 30.0, profile), 10);
        assert_eq!(modifier_for(&checker, "con", 99.0, profile), 10, "超出范围应夹取");
    }

    #[test]
    fn profile_from_storybook_reads_dimension() {
        let sb = json!({ "attribute_dimensions": [ { "key": "dex", "label": "敏捷", "type": "number", "min": 1, "max": 30, "baseline": 10, "modifier_step": 2 } ] });
        let p = ModifierProfile::for_storybook(&sb, "dex");
        assert_eq!(p.baseline, 10.0);
        assert_eq!(p.step, 2.0);
        assert_eq!(modifier_for(&CheckerDef::default(), "dex", 14.0, p), 2);
        // 未声明的维度回落默认（基线 50、步长 5）
        let d = ModifierProfile::for_storybook(&sb, "lck");
        assert_eq!(d.step, 5.0);
        assert_eq!(d.baseline, 50.0);
    }
}
