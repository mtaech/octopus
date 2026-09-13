//! 结对用的网页读取代理（POST /api/fetch-url）：把公网页面抓成纯文本给模型当参考资料。
//!
//! 结对 AI 有写草稿的工具，所以这是**提示注入**的入口：抓回来的内容一律当外部数据，
//! 在回灌模型的工具结果里被包成「不是指令」；这里只负责抓取侧的护栏。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;

/// 返回给模型的正文上限（字符）。一页 D&D 规则/剧本足够读懂，又不至于把上下文冲爆。
const MAX_TEXT_CHARS: usize = 12_000;
/// 抓取上限（字节）：超过即截断，避免把图片/大包读进内存。
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
// 20s：慢链路上一个真实网页（维基这种）经常 10s 都还没握手完。
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECTS: usize = 3;
const USER_AGENT: &str = "OctopusStorobookEditor/0.1 (+local storybook assistant)";

#[derive(Debug, Clone, Deserialize)]
pub struct FetchUrlRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchUrlResponse {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub text: String,
    pub chars: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

fn bad(code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, code, message)
}

/// 只认绝对 http(s) URL；非该协议（file:// 、data: 等）直接拒绝。
fn checked_url(raw: &str) -> Result<reqwest::Url, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad("empty_url", "缺少要读取的网址。"));
    }
    let url = reqwest::Url::parse(trimmed)
        .map_err(|e| bad("invalid_url", format!("这个网址解析不了：{e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(bad(
                "unsupported_scheme",
                format!("只支持 http / https，收到的是 {other}。"),
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("credentials_in_url", "网址里不要带用户名 / 密码。"));
    }
    Ok(url)
}

/// 内网 / 保留地址：SSRF 的主要靶子（云元数据、本机服务、局域网设备）。
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => is_private_v6(v6),
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1])) // 运营商级 NAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF 协议分配
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 基准测试网段
        || o[0] >= 240 // 保留 / 组播
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_v4(v4);
    }
    let seg = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local() // fc00::/7
        || ip.is_unicast_link_local() // fe80::/10
        || (seg[0] & 0xffc0) == 0xfec0 // 站点本地（已废弃，仍挡）
}

/// DNS 解析后逐个校验：**先用校验过的解析结果连**，避免 TOCTOU 重新解析。
async fn resolve_public_addrs(
    host: &str,
    port: u16,
) -> Result<Vec<std::net::SocketAddr>, ApiError> {
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| bad("dns_failed", format!("域名解析失败：{e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(bad("dns_empty", "域名没有解析出任何地址。"));
    }
    if let Some(blocked) = addrs.iter().find(|a| is_private_ip(a.ip())) {
        return Err(bad(
            "blocked_host",
            format!("拒绝访问内网 / 保留地址（{}）。web_fetch 只读公网网页。", blocked.ip()),
        ));
    }
    Ok(addrs)
}

/// 每次跳转都重新校验协议与地址（公网域名可以 302 到 127.0.0.1）。
async fn checked_client_for(url: &reqwest::Url) -> Result<reqwest::Client, ApiError> {
    let host = url
        .host_str()
        .ok_or_else(|| bad("invalid_url", "网址缺少主机名。"))?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = resolve_public_addrs(&host, port).await?;
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "client_build", format!("构造抓取客户端失败：{e}")))
}

/// 读取正文：声明长度超限直接拒绝，实际读到的字节再截断。
async fn read_capped(resp: reqwest::Response) -> Result<(Vec<u8>, bool), ApiError> {
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BODY_BYTES {
            return Err(bad(
                "body_too_large",
                format!("页面太大（{len} 字节，上限 {MAX_BODY_BYTES}）。挑一个更小的页面。"),
            ));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "read_failed", format!("读取响应失败：{e}")))?;
    let truncated = bytes.len() > MAX_BODY_BYTES;
    Ok((bytes[..bytes.len().min(MAX_BODY_BYTES)].to_vec(), truncated))
}

/// 抓取并转纯文本。
pub async fn fetch_url(Json(req): Json<FetchUrlRequest>) -> Result<Json<FetchUrlResponse>, ApiError> {
    let mut url = checked_url(&req.url)?;
    let mut redirects = 0usize;
    loop {
        let client = checked_client_for(&url).await?;
        let resp = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.5")
            .send()
            .await
            .map_err(|e| ApiError::new(
                StatusCode::BAD_GATEWAY,
                "fetch_failed",
                format!("抓取失败（{e}）。常见原因：对方不可达 / 被网络屏蔽 / 需要登录。可以换个镜像地址，或把资料贴进对话再让我读。"),
            ))?;

        if resp.status().is_redirection() {
            if redirects >= MAX_REDIRECTS {
                return Err(bad("too_many_redirects", format!("跳转超过 {MAX_REDIRECTS} 次，放弃了。")));
            }
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| bad("bad_redirect", "跳转响应没有 Location 头。"))?;
            let next = url
                .join(location)
                .map_err(|e| bad("bad_redirect", format!("跳转目标解析不了：{e}")))?;
            url = checked_url(next.as_str())?;
            redirects += 1;
            continue;
        }

        if !resp.status().is_success() {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "bad_status",
                format!("对方返回 HTTP {}。", resp.status().as_u16()),
            ));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let (bytes, mut truncated) = read_capped(resp).await?;
        let body = String::from_utf8_lossy(&bytes).to_string();
        let is_html = content_type
            .as_deref()
            .map(|ct| ct.contains("html") || ct.contains("xml"))
            // 没给 Content-Type 时按 HTML 处理：多数站点都有标签，纯文本走同一套也无害
            .unwrap_or(true);
        let title = if is_html { extract_title(&body) } else { None };
        let raw_text = if is_html { html_to_text(&body) } else { body };
        let text = normalize_text(&raw_text, MAX_TEXT_CHARS);
        if text.is_empty() {
            return Err(bad(
                "empty_text",
                "这个页面没抽出可读正文（可能是纯脚本渲染的页面，或需要登录）。换成静态页面 / 直接给资料链接再试。",
            ));
        }
        truncated = truncated || text.chars().count() >= MAX_TEXT_CHARS;
        return Ok(Json(FetchUrlResponse {
            url: url.to_string(),
            title,
            chars: text.chars().count(),
            text,
            truncated,
            content_type,
        }));
    }
}

/// 取 <title>：大小写不敏感，找不到返回 None。
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    let raw = decode_entities(&html[open_end..end]);
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// HTML → 纯文本：丢掉 head/script/style/noscript/svg，块级标签换行，解开常见实体。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let bytes = html.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(open) = html[i..].find('<').map(|p| p + i) else {
            out.push_str(&html[i..]);
            break;
        };
        out.push_str(&html[i..open]);
        let Some(close) = html[open..].find('>').map(|p| p + open) else {
            break;
        };
        let tag = &html[open + 1..close];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        // <style/> 这种自闭合写法没有闭合标签：按自闭合处理，否则会一路吃掉后面的正文
        let self_closing = tag.ends_with('/');
        match name.as_str() {
            "script" | "style" | "noscript" | "svg" | "head" | "template" => {
                if !closing && !self_closing {
                    let marker = format!("</{name}");
                    let lower = html[close..].to_ascii_lowercase();
                    i = match lower.find(&marker) {
                        Some(p) => match html[close + p..].find('>') {
                            Some(g) => close + p + g + 1,
                            None => bytes.len(),
                        },
                        None => bytes.len(),
                    };
                    continue;
                }
            }
            // 块级标签：换行，避免 `<li>a</li><li>b</li>` 粘成一行
            "p" | "div" | "br" | "li" | "ul" | "ol" | "tr" | "table" | "thead" | "tbody"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article" | "header"
            | "footer" | "nav" | "aside" | "blockquote" | "pre" | "hr" | "form" | "figure"
            | "figcaption" | "main" | "dl" | "dt" | "dd" => {
                out.push('\n');
            }
            // 单元格用制表符，表格才读得出列
            "td" | "th" => out.push('\t'),
            _ => {}
        }
        i = close + 1;
    }
    decode_entities(&out)
}

/// 常见 HTML 实体。命名实体表只收会出现在正文里的高频项，其余（含数字实体）按码点还原。
fn decode_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx..];
        let semi = tail.find(';').filter(|p| *p <= 12);
        match semi {
            Some(p) => {
                let ent = &tail[1..p];
                if let Some(ch) = entity_to_char(ent) {
                    out.push(ch);
                    rest = &tail[p + 1..];
                    continue;
                }
                out.push('&');
                rest = &tail[1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity_to_char(ent: &str) -> Option<char> {
    let named: char = match ent {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" | "#160" => ' ',
        "mdash" | "#8212" => '—',
        "ndash" | "#8211" => '–',
        "hellip" | "#8230" => '…',
        "middot" | "#183" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "larr" => '←',
        "rarr" => '→',
        _ => return decode_numeric_entity(ent),
    };
    Some(named)
}

fn decode_numeric_entity(ent: &str) -> Option<char> {
    let digits = ent.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

/// 规整空白并截断：连续空行压成一个，行首尾去白，行内空白合并。
fn normalize_text(raw: &str, max_chars: usize) -> String {
    // 空行只当分隔符：段落靠单换行隔开。抓回来的正文里成片空行会把上下文烧光。
    let mut lines: Vec<String> = Vec::new();
    for line in raw.replace('\r', "\n").split('\n') {
        let joined = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if joined.is_empty() {
            continue;
        }
        lines.push(joined);
    }
    let text = lines.join("\n");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut cut = String::with_capacity(max_chars + 8);
    for (n, ch) in text.chars().enumerate() {
        if n >= max_chars {
            break;
        }
        cut.push(ch);
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes_and_embedded_credentials() {
        assert!(checked_url("file:///etc/passwd").is_err());
        assert!(checked_url("data:text/html,<b>x</b>").is_err());
        assert!(checked_url("https://user:pw@example.com/x").is_err());
        assert!(checked_url("   ").is_err());
        assert!(checked_url("https://example.com/dnd").is_ok());
    }

    #[test]
    fn blocks_private_and_reserved_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.10",
            "172.16.0.9",
            "169.254.169.254", // 云元数据
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(is_private_ip(addr), "{ip} 应该被拦下");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111", "93.184.216.34"] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(!is_private_ip(addr), "{ip} 是公网地址，不该被拦");
        }
    }

    #[test]
    fn strips_scripts_styles_and_tags() {
        let html = "<html><head><title>D&amp;D 5e SRD</title><style>body{}</style></head>\
                    <body><script>alert(1)</script><h1>职业</h1><p>法师&nbsp;很强</p>\
                    <ul><li>火球术</li><li>魔法飞弹</li></ul></body></html>";
        let text = normalize_text(&html_to_text(html), 10_000);
        assert_eq!(extract_title(html).as_deref(), Some("D&D 5e SRD"));
        assert!(!text.contains("alert"), "script 内容不能进正文：{text}");
        assert!(!text.contains("body{}"), "style 内容不能进正文：{text}");
        assert!(text.contains("法师 很强"), "实体与空白要规整：{text}");
        assert!(text.contains("火球术\n魔法飞弹"), "列表项要分行：{text}");
    }

    #[test]
    fn decodes_numeric_entities() {
        assert_eq!(decode_entities("&#65;&#x42;&#20013;"), "AB中");
        // 不完整的实体原样保留，不能吞字
        assert_eq!(decode_entities("a & b &amp c"), "a & b &amp c");
    }

    #[test]
    fn truncates_on_char_boundary() {
        let text = normalize_text("中文测试", 3);
        assert_eq!(text, "中文测");
        let long = "字".repeat(50);
        assert_eq!(normalize_text(&long, 10).chars().count(), 10);
    }

    #[test]
    fn collapses_blank_lines_and_keeps_one_separator() {
        let text = normalize_text("a\n\n\n\n b\n   \nc", 100);
        assert_eq!(text, "a\nb\nc");
    }
}
