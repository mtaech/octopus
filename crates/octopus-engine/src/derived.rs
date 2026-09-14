//! 派生值求值（#2）：小型表达式求值器。
//! 与前端 frontend/src/lib/derived.ts 同语法：数值 / 变量 / + - * / ( ) / floor ceil round min max abs。

use std::collections::HashMap;

/// 求值：vars 为「属性维度 + 已计算派生值」的取值表。
pub fn eval_formula(src: &str, vars: &HashMap<String, f64>) -> Result<f64, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = Parser { c: &chars, i: 0, vars, collect: None };
    let v = p.parse_expr()?;
    p.skip_ws();
    if p.i < p.c.len() {
        let rest: String = p.c[p.i..].iter().collect();
        return Err(format!("多余的输入 {rest}"));
    }
    Ok(v)
}

/// 只做语法检查并收集变量名（校验用；不要求变量存在）。
pub fn check_formula(src: &str) -> Result<Vec<String>, String> {
    let vars: HashMap<String, f64> = HashMap::new();
    let chars: Vec<char> = src.chars().collect();
    let mut p = Parser { c: &chars, i: 0, vars: &vars, collect: Some(Vec::new()) };
    p.parse_expr()?;
    p.skip_ws();
    if p.i < p.c.len() {
        let rest: String = p.c[p.i..].iter().collect();
        return Err(format!("多余的输入 {rest}"));
    }
    Ok(p.collect.unwrap_or_default())
}

/// 按声明顺序求值一整套派生值（#2 / 图鉴 M2 §4.4）：变量 = 属性 + 之前已算出的派生值。
///
/// - `attrs` = 实例属性（调用方已并入挂接定义 / 已装备物品的修正）；
/// - `defs` = (key, formula)，顺序即故事书 derived[] 的声明顺序（后者可引用前者）；
/// - `modifiers` = 同名修正（add 累加 → max 取高 → set 覆盖），与前端 computeDerived 同口径。
///
/// 求值失败的派生值**不写入**（后续引用它的公式同样失败）：宁可少一个派生值，
/// 也不要写入一个由错误算出来的数。
pub fn compute_derived(
    attrs: &HashMap<String, f64>,
    defs: &[(String, String)],
    modifiers: &HashMap<String, crate::modifiers::AttrModifier>,
) -> HashMap<String, f64> {
    let mut vars = attrs.clone();
    for (key, value) in vars.iter_mut() {
        if let Some(m) = modifiers.get(key) {
            *value = m.apply(*value);
        }
    }
    for (key, formula) in defs {
        let Ok(v) = eval_formula(formula, &vars) else { continue };
        let v = modifiers.get(key).map(|m| m.apply(v)).unwrap_or(v);
        vars.insert(key.clone(), v);
    }
    vars
}

struct Parser<'a> {
    c: &'a [char],
    i: usize,
    vars: &'a HashMap<String, f64>,
    collect: Option<Vec<String>>,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.c.len() && self.c[self.i].is_whitespace() { self.i += 1; }
    }
    fn peek(&mut self) -> Option<char> { self.skip_ws(); self.c.get(self.i).copied() }
    fn parse_expr(&mut self) -> Result<f64, String> { self.parse_add() }
    fn parse_add(&mut self) -> Result<f64, String> {
        let mut v = self.parse_mul()?;
        loop {
            match self.peek() {
                Some('+') => { self.i += 1; v += self.parse_mul()?; }
                Some('-') => { self.i += 1; v -= self.parse_mul()?; }
                _ => return Ok(v),
            }
        }
    }
    fn parse_mul(&mut self) -> Result<f64, String> {
        let mut v = self.parse_unary()?;
        loop {
            match self.peek() {
                Some('*') => { self.i += 1; v *= self.parse_unary()?; }
                Some('/') => { self.i += 1; v /= self.parse_unary()?; }
                _ => return Ok(v),
            }
        }
    }
    fn parse_unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some('-') => { self.i += 1; Ok(-self.parse_unary()?) }
            Some('+') => { self.i += 1; self.parse_unary() }
            _ => self.parse_primary(),
        }
    }
    fn parse_primary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some('(') => {
                self.i += 1;
                let v = self.parse_expr()?;
                if self.peek() != Some(')') { return Err("缺少右括号".into()); }
                self.i += 1;
                Ok(v)
            }
            Some(ch) if ch.is_ascii_digit() || ch == '.' => {
                let start = self.i;
                while self.i < self.c.len() && (self.c[self.i].is_ascii_digit() || self.c[self.i] == '.') { self.i += 1; }
                let s: String = self.c[start..self.i].iter().collect();
                s.parse::<f64>().map_err(|_| format!("非法数字 {s}"))
            }
            Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {
                let start = self.i;
                while self.i < self.c.len() && (self.c[self.i].is_ascii_alphanumeric() || self.c[self.i] == '_' || self.c[self.i] == '.') { self.i += 1; }
                let name: String = self.c[start..self.i].iter().collect();
                if self.peek() == Some('(') {
                    self.i += 1;
                    let mut args: Vec<f64> = Vec::new();
                    if self.peek() != Some(')') {
                        args.push(self.parse_expr()?);
                        while self.peek() == Some(',') { self.i += 1; args.push(self.parse_expr()?); }
                    }
                    if self.peek() != Some(')') { return Err("缺少右括号".into()); }
                    self.i += 1;
                    apply_fn(&name, &args)
                } else {
                    match self.vars.get(&name).copied() {
                        Some(v) => Ok(v),
                        None => match self.collect.as_mut() {
                            Some(col) => { col.push(name); Ok(0.0) }
                            None => Err(format!("未知变量 {name}")),
                        },
                    }
                }
            }
            other => Err(format!("意外的字符 {}", other.map(String::from).unwrap_or_default())),
        }
    }
}

fn apply_fn(name: &str, a: &[f64]) -> Result<f64, String> {
    let first = *a.first().unwrap_or(&0.0);
    match name {
        "floor" => Ok(first.floor()),
        "ceil" => Ok(first.ceil()),
        "round" => Ok(first.round()),
        "abs" => Ok(first.abs()),
        "min" => Ok(a.iter().cloned().fold(f64::INFINITY, f64::min)),
        "max" => Ok(a.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
        _ => Err(format!("未知函数 {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_basic() {
        let mut vars = HashMap::new();
        vars.insert("int".to_string(), 16.0);
        assert_eq!(eval_formula("floor((int - 10) / 2)", &vars).unwrap(), 3.0);
        assert_eq!(eval_formula("8 + 2 + 3", &vars).unwrap(), 13.0);
        assert_eq!(eval_formula("max(1, 5, 3)", &vars).unwrap(), 5.0);
    }

    #[test]
    fn eval_unknown_var_errors() {
        let vars = HashMap::new();
        assert!(eval_formula("foo + 1", &vars).is_err());
    }

    /// 图鉴 M2 §4.4：派生值按声明顺序求值，变量 = 属性 + 之前已算的派生值，
    /// 并叠加同名修正（挂接定义 / 已装备物品）——与前端口径一致。
    #[test]
    fn compute_derived_follows_declaration_order_and_modifiers() {
        let mut attrs = HashMap::new();
        attrs.insert("str".to_string(), 8.0);
        attrs.insert("dex".to_string(), 14.0);
        let defs = vec![
            ("dex_mod".to_string(), "floor((dex - 10) / 2)".to_string()),
            ("ac".to_string(), "10 + dex_mod".to_string()),
        ];
        // 挂接定义 monster-armor 给出 ac +3（LMoP 地精：10 + 2 + 3 = 15）。
        let mut mods = HashMap::new();
        mods.insert(
            "ac".to_string(),
            crate::modifiers::AttrModifier { add: 3, max: None, set: None },
        );
        let out = compute_derived(&attrs, &defs, &mods);
        assert_eq!(out.get("dex_mod"), Some(&2.0));
        assert_eq!(out.get("ac"), Some(&15.0));
        // 缺失变量的派生值不写入（宁缺毋滥）。
        let out2 = compute_derived(&attrs, &[("ac".to_string(), "10 + wis_mod".to_string())], &HashMap::new());
        assert_eq!(out2.get("ac"), None);
    }

    #[test]
    fn check_collects_vars() {
        let mut v = check_formula("floor((int - 10) / 2)").unwrap();
        v.sort();
        assert_eq!(v, vec!["int".to_string()]);
        assert!(check_formula("(1 + )").is_err());
    }
}
