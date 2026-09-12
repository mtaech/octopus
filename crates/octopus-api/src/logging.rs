//! 日志装配（#30）：统一全局订阅者与 HTTP 请求日志的格式与开关。
//!
//! ## 格式
//! - **文本模式（默认）**：本地时区 + 毫秒精度时间戳 `2025-09-11 17:43:00.123`；
//!   仅当 stderr 是终端时才输出 ANSI 颜色，写文件 / 管道时是纯文本，不留转义码。
//! - **JSON 模式**：设 `OCTOPUS_LOG_FORMAT=json` 时输出结构化 JSON（一行一条），
//!   供日志采集 / 程序化解析；时间戳同样为本地毫秒精度。
//!
//! ## 级别过滤
//! 读 `RUST_LOG`，缺省 `info`。HTTP 请求日志由 `http_trace_layer` 在 info 级
//! 输出单行（方法 / 路径 / 状态 / 耗时），不再依赖 tower-http 默认的 TRACE 级噪音。

use std::fmt;
use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

/// 缺省过滤：业务 info，SQL 语句明细收敛到 warn，避免启动与请求日志被刷屏。
pub const DEFAULT_FILTER: &str = "info,sqlx=warn";

/// 本地时区毫秒精度时间戳；文本与 JSON 模式共用。
#[derive(Clone, Copy, Default)]
pub struct LocalMillis;

impl FormatTime for LocalMillis {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        write!(w, "{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"))
    }
}

/// 初始化全局日志订阅者（进程内只应调用一次）。
///
/// - 级别：`RUST_LOG`，缺省 `info,sqlx=warn`（SQL 明细要看的用 `RUST_LOG=sqlx=debug`）。
/// - 格式：`OCTOPUS_LOG_FORMAT=json` 时结构化 JSON，否则可读文本。
pub fn init() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_FILTER.into());
    tracing::debug!(filter = %filter, "日志过滤级别已设定");
    let json = std::env::var("OCTOPUS_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    if json {
        tracing_subscriber::fmt()
            .json()
            .with_timer(LocalMillis)
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_timer(LocalMillis)
            .with_env_filter(filter)
            .with_ansi(std::io::stderr().is_terminal())
            .init();
    }
}

// ============================================================
// HTTP 请求日志：每个请求一行，替代 tower-http 默认的 TRACE 级噪音。
// ============================================================

fn http_span(request: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    tracing::span!(
        tracing::Level::INFO,
        "http",
        method = %request.method(),
        uri = %request.uri().path(),
    )
}

fn http_response(
    response: &axum::http::Response<axum::body::Body>,
    latency: std::time::Duration,
    span: &tracing::Span,
) {
    tracing::info!(
        parent: span,
        status = %response.status().as_u16(),
        latency_ms = latency.as_millis() as u64,
        "HTTP 请求完成"
    );
}

fn http_failure(
    failure: tower_http::classify::ServerErrorsFailureClass,
    latency: std::time::Duration,
    span: &tracing::Span,
) {
    match failure {
        tower_http::classify::ServerErrorsFailureClass::Error(err) => {
            tracing::warn!(
                parent: span,
                latency_ms = latency.as_millis() as u64,
                error = %err,
                "HTTP 请求失败"
            );
        }
        tower_http::classify::ServerErrorsFailureClass::StatusCode(_) => {
            tracing::warn!(
                parent: span,
                latency_ms = latency.as_millis() as u64,
                "HTTP 请求失败（5xx）"
            );
        }
    }
}

/// 供 axum 路由装配的请求追踪层（见 `octopus_api::router`）。
pub fn http_trace_layer() -> tower_http::trace::TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    fn(&axum::http::Request<axum::body::Body>) -> tracing::Span,
    tower_http::trace::DefaultOnRequest,
    fn(
        &axum::http::Response<axum::body::Body>,
        std::time::Duration,
        &tracing::Span,
    ),
    tower_http::trace::DefaultOnBodyChunk,
    tower_http::trace::DefaultOnEos,
    fn(
        tower_http::classify::ServerErrorsFailureClass,
        std::time::Duration,
        &tracing::Span,
    ),
> {
    tower_http::trace::TraceLayer::new_for_http()
        .make_span_with(http_span as fn(&axum::http::Request<axum::body::Body>) -> tracing::Span)
        .on_response(
            http_response
                as fn(
                    &axum::http::Response<axum::body::Body>,
                    std::time::Duration,
                    &tracing::Span,
                ),
        )
        .on_failure(
            http_failure
                as fn(
                    tower_http::classify::ServerErrorsFailureClass,
                    std::time::Duration,
                    &tracing::Span,
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_millis_has_expected_shape() {
        let mut buf = String::new();
        LocalMillis
            .format_time(&mut Writer::new(&mut buf))
            .expect("format_time 不应失败");
        assert_eq!(buf.len(), "YYYY-MM-DD HH:MM:SS.mmm".len(), "时间戳 {buf:?}");
        assert_eq!(buf.as_bytes()[4], b'-');
        assert_eq!(buf.as_bytes()[7], b'-');
        assert_eq!(buf.as_bytes()[10], b' ');
        assert_eq!(buf.as_bytes()[13], b':');
        assert_eq!(buf.as_bytes()[16], b':');
        assert_eq!(buf.as_bytes()[19], b'.');
    }
}
