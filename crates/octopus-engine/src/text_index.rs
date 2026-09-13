//! CJK 友好的 FTS5 分词（#05 M3）：修中文子串关键词检索。
//!
//! 背景：SQLite 的 \`unicode61\` 按空白 / 标点切词，**连续中文会被当成一个整词**——
//! 文档「月光下的古堡」在索引里是一个词项，查「古堡」永远不命中（M2 实测）。本产品
//! 中文优先，所以：
//!
//! - **写入时**把每个 CJK 字符之间插入空格（逐字成词），拉丁 / 数字连续串保持完整；
//! - **查询时**把连续 CJK 串构造成 FTS5 **短语**（相邻才算命中），拉丁词按 AND 组合。
//!
//! 不变量：写入与查询必须共用这里的 tokenizer，否则词项对不上——所以两边都只经由
//! \`index_text\` / \`match_expression\` 处理，调用方不再手拼 FTS 语法。

/// 表意文字（中日韩）判定：只把「字母」当字切，标点（、。「」…）作分隔符丢弃。
///
/// 全角拉丁（ＡＢＣ）走 \`is_alphabetic\` 但不在这些区间内，因此仍按普通词处理。
fn is_cjk(c: char) -> bool {
    if !c.is_alphabetic() {
        return false;
    }
    matches!(
        c as u32,
        0x3040..=0x30FF      // 平假名 / 片假名
            | 0x3100..=0x312F // 注音符号
            | 0x31F0..=0x31FF // 片假名扩展
            | 0x3400..=0x4DBF // CJK 扩展 A
            | 0x4E00..=0x9FFF // CJK 统一表意
            | 0xAC00..=0xD7AF // 谚文音节
            | 0xF900..=0xFAFF // CJK 兼容表意
            | 0x20000..=0x2FA1F // CJK 扩展 B 及以上
    )
}

/// 一个原子 token：CJK 单字，或一段拉丁 / 数字连续串。
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Cjk(char),
    Word(String),
}

/// 按「CJK 逐字、拉丁数字成词、其余分隔」切出 token 序列（确定性）。
fn pieces(text: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if is_cjk(c) {
            if !word.is_empty() {
                out.push(Piece::Word(std::mem::take(&mut word)));
            }
            out.push(Piece::Cjk(c));
        } else if c.is_alphanumeric() {
            word.push(c);
        } else if !word.is_empty() {
            // 标点 / 空白：结束当前词，且不入索引。
            out.push(Piece::Word(std::mem::take(&mut word)));
        }
    }
    if !word.is_empty() {
        out.push(Piece::Word(word));
    }
    out
}

/// 写入 FTS 的文本：CJK 逐字以空格分隔，保证 unicode61 逐字成词。
///
/// 例：\`index_text("月光下的古堡")\` → \`"月 光 下 的 古 堡"\`。
pub fn index_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for p in pieces(text) {
        if !out.is_empty() {
            out.push(' ');
        }
        match p {
            Piece::Cjk(c) => out.push(c),
            Piece::Word(w) => out.push_str(&w),
        }
    }
    out
}

/// 把原始查询文本构造成 FTS5 MATCH 表达式；无可检索 token 时返回 None。
///
/// 连续 CJK 串合成一个短语（\`"古 堡"\`），因此「古堡」能命中「月光下的古堡」，
/// 而「光古」（两字不相邻）不会命中。拉丁词作为独立项，空格即隐式 AND。
pub fn match_expression(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut phrase: Vec<char> = Vec::new();
    // 把累计的 CJK 短语刷成一个 FTS5 短语字面量。
    let flush_phrase = |phrase: &mut Vec<char>, out: &mut String| {
        if phrase.is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('"');
        for (i, c) in phrase.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push(*c);
        }
        out.push('"');
        phrase.clear();
    };
    let push_term = |word: &str, out: &mut String| {
        if word.is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        // 加引号避免任何 FTS5 语法歧义；内容只含字母数字，无需转义。
        out.push('"');
        out.push_str(word);
        out.push('"');
    };
    for p in pieces(text) {
        match p {
            Piece::Cjk(c) => phrase.push(c),
            Piece::Word(w) => {
                flush_phrase(&mut phrase, &mut out);
                push_term(&w, &mut out);
            }
        }
    }
    flush_phrase(&mut phrase, &mut out);
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_is_split_per_character_while_latin_stays_intact() {
        assert_eq!(index_text("月光下的古堡"), "月 光 下 的 古 堡");
        assert_eq!(index_text("HP回复"), "HP 回 复");
        assert_eq!(index_text("the Ancient Castle"), "the Ancient Castle");
        // 标点作分隔符，不产生空词项。
        assert_eq!(index_text("古堡。"), "古 堡");
        assert_eq!(index_text("月光，古堡！"), "月 光 古 堡");
    }

    #[test]
    fn query_builds_cjk_phrase_and_latin_terms() {
        assert_eq!(match_expression("古堡").as_deref(), Some("\"古 堡\""));
        assert_eq!(match_expression("月光下的古堡").as_deref(), Some("\"月 光 下 的 古 堡\""));
        assert_eq!(match_expression("moon castle").as_deref(), Some("\"moon\" \"castle\""));
        assert_eq!(match_expression("古堡 castle").as_deref(), Some("\"古 堡\" \"castle\""));
        // 纯标点 / 空白：没有可检索 token。
        assert_eq!(match_expression("   。！  "), None);
        assert_eq!(match_expression(""), None);
    }
}
