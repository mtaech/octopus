//! 导出物的文件名（`Content-Disposition`）。
//!
//! 约定：**文件用标题命名，不用 id**——下载目录里 `凡戴尔的失落矿坑.octopus-book.zip`
//! 一眼知道是什么，`sb-9f3c….octopus-book.zip` 只有机器看得懂。
//!
//! 但标题是用户输入：可能带 `/` `\` `:` 这类路径字符、换行、超长，甚至是 Windows 保留名。
//! 所以先规整成**安全词干**，再按 RFC 6266 / 5987 组装头部：
//!
//! ```text
//! Content-Disposition: attachment; filename="<ASCII 兜底>"; filename*=UTF-8''<百分号编码>
//! ```
//!
//! 中文标题**不能**直接写进 `filename=`：HTTP 头值只允许 ASCII，非 ASCII 会被拒收。
//! 老客户端读 `filename=`（ASCII 兜底 = id），现代浏览器读 `filename*`（真实标题）。

/// Windows 保留名：`CON.octopus-book.zip` 这类名字在 Windows 上仍然非法（保留名不看扩展名）。
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 词干长度上限（按字符数，不是字节）。标题可以很长，但文件名没有可读性收益，
/// 且中文按字节算会成倍膨胀，截断能保证头部长度可控。
const MAX_STEM_CHARS: usize = 60;

/// 把标题规整成文件名安全词干；全被过滤掉时回落到 `fallback`（调用方传 id）。
///
/// 规则：路径分隔符 / 通配符 → `_`；控制字符 → `_`；空白折叠为单个空格；
/// 去掉首尾的空格与点；截到 [`MAX_STEM_CHARS`]；Windows 保留名加 `_` 前缀。
pub fn safe_file_stem(title: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in title.chars() {
        if ch.is_whitespace() {
            // 折叠连续空白，并丢掉前导空白（末尾空白最后统一去）
            pending_space = !out.is_empty();
            continue;
        }
        let c = match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        };
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }

    // 首尾的空格与点都要去：Windows 会静默丢掉尾部，前导点会让文件变成隐藏文件
    let trimmed = out.trim_matches(|c: char| c == ' ' || c == '.');
    let mut stem: String = trimmed.chars().take(MAX_STEM_CHARS).collect();
    stem.truncate(stem.trim_end_matches(|c: char| c == ' ' || c == '.').len());

    if stem.is_empty() {
        return fallback.to_string();
    }
    if WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&stem))
    {
        return format!("_{stem}");
    }
    stem
}

/// 组装 `Content-Disposition` 值（保证输出纯 ASCII，可直接进 header）。
/// `ext` 是完整扩展名（如 `octopus-book.zip`），`ascii_fallback` 用于非 ASCII 标题。
pub fn content_disposition(stem: &str, ascii_fallback: &str, ext: &str) -> String {
    let real = format!("{stem}.{ext}");
    let ascii = if real.is_ascii() {
        real.clone()
    } else {
        format!("{ascii_fallback}.{ext}")
    };
    format!(
        r#"attachment; filename="{}"; filename*=UTF-8''{}"#,
        ascii.replace('"', "_"),
        percent_encode_utf8(&real)
    )
}

/// RFC 5987 的百分号编码（UTF-8 字节）：attr-char 集合之外一律编码。
/// 只保留 `A-Za-z0-9-._~`——比标准更严格，但两者都认，且输出稳定可测。
fn percent_encode_utf8(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_readable_titles() {
        assert_eq!(safe_file_stem("凡戴尔的失落矿坑", "sb-1"), "凡戴尔的失落矿坑");
        assert_eq!(safe_file_stem("Lost Mine of Phandelver", "sb-1"), "Lost Mine of Phandelver");
    }

    #[test]
    fn strips_path_chars_and_collapses_space() {
        // 路径分隔符不能活下来（前导点也一并去掉，避免隐藏文件）
        assert_eq!(safe_file_stem("../../etc/passwd", "sb-1"), "_.._etc_passwd");
        assert_eq!(safe_file_stem(r#"a/b\c:d*e?f"g<h>i|j"#, "sb-1"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(safe_file_stem("  多个   空格   ", "sb-1"), "多个 空格");
        assert_eq!(safe_file_stem("换行\n与\t制表", "sb-1"), "换行 与 制表");
    }

    #[test]
    fn trims_dots_caps_length_and_falls_back() {
        assert_eq!(safe_file_stem("...隐藏...", "sb-1"), "隐藏");
        assert_eq!(safe_file_stem("   ", "sb-1"), "sb-1");
        assert_eq!(safe_file_stem("", "sb-1"), "sb-1");
        assert_eq!(safe_file_stem("CON", "sb-1"), "_CON");
        assert_eq!(safe_file_stem("con", "sb-1"), "_con");

        let long = "字".repeat(200);
        assert_eq!(safe_file_stem(&long, "sb-1").chars().count(), MAX_STEM_CHARS);
    }

    #[test]
    fn disposition_is_ascii_and_carries_utf8_name() {
        let value = content_disposition("凡戴尔的失落矿坑", "sb-1", "octopus-book.zip");
        assert!(value.is_ascii(), "header 值必须纯 ASCII：{value}");
        assert!(value.contains(r#"filename="sb-1.octopus-book.zip""#));
        // 「凡戴尔的失落矿坑」的 UTF-8 百分号编码
        assert!(value.contains("filename*=UTF-8''%E5%87%A1%E6%88%B4%E5%B0%94%E7%9A%84%E5%A4%B1%E8%90%BD%E7%9F%BF%E5%9D%91.octopus-book.zip"), "实际：{value}");
    }

    #[test]
    fn disposition_uses_ascii_title_when_possible() {
        let value = content_disposition("Lost Mine", "sb-1", "octopus-book.zip");
        assert!(value.contains(r#"filename="Lost Mine.octopus-book.zip""#));
        assert!(value.contains("filename*=UTF-8''Lost%20Mine.octopus-book.zip"));
    }
}
