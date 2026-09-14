#!/usr/bin/env bash
# ==============================================================================
# Octopus 服务器启动脚本（部署在 /home/huang/octopus，在服务器上执行）
#
#   ./server-start.sh start       后台启动后端（写 pid，等 /api/health 通过）
#   ./server-start.sh stop        优雅停止（先 TERM，超时再 KILL）
#   ./server-start.sh restart     重启
#   ./server-start.sh status      查看进程 / 健康检查 / 端口
#   ./server-start.sh logs        跟踪日志（Ctrl+C 退出）
#   ./server-start.sh health      只探测 /api/health
#   ./server-start.sh foreground  前台运行（调试用）
#
# 目录约定:
#   bin/octopus-bin   后端二进制      web/        前端 dist（Caddy 直接托管）
#   octopus.db        SQLite 数据库   assets/     图片资产库
#   config.json       LLM 配置(0600)  logs/ run/  日志与 pid
#
# 可覆盖的环境变量: OCTOPUS_ADDR OCTOPUS_DB OCTOPUS_ASSETS OCTOPUS_CONFIG
#                   RUST_LOG OCTOPUS_LOG_FORMAT
# ==============================================================================
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

OCTOPUS_ADDR="${OCTOPUS_ADDR:-127.0.0.1:8787}"
OCTOPUS_DB="${OCTOPUS_DB:-$APP_DIR/octopus.db}"
OCTOPUS_ASSETS="${OCTOPUS_ASSETS:-$APP_DIR/assets}"
OCTOPUS_CONFIG="${OCTOPUS_CONFIG:-$APP_DIR/config.json}"
RUST_LOG="${RUST_LOG:-info,sqlx=warn}"
OCTOPUS_LOG_FORMAT="${OCTOPUS_LOG_FORMAT:-text}"

BIN="$APP_DIR/bin/octopus-bin"
PID_FILE="$APP_DIR/run/octopus.pid"
LOG_FILE="$APP_DIR/logs/octopus.log"
HEALTH_URL="http://${OCTOPUS_ADDR}/api/health"

if [ -t 1 ]; then
    CYAN='\033[0;36m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; BOLD='\033[1m'; NC='\033[0m'
else
    CYAN=''; GREEN=''; YELLOW=''; RED=''; BOLD=''; NC=''
fi

log()  { printf "${CYAN}[octopus]${NC} %s\n" "$*"; }
ok()   { printf "${GREEN}[octopus]${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}[warn]${NC} %s\n" "$*" >&2; }
die()  { printf "${RED}[error]${NC} %s\n" "$*" >&2; exit 1; }

running_pid() {
    [ -f "$PID_FILE" ] || return 1
    local pid
    pid="$(cat "$PID_FILE" 2>/dev/null || true)"
    [ -n "$pid" ] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    echo "$pid"
}

export OCTOPUS_ADDR OCTOPUS_DB OCTOPUS_ASSETS OCTOPUS_CONFIG RUST_LOG OCTOPUS_LOG_FORMAT

do_start() {
    mkdir -p "$APP_DIR/run" "$APP_DIR/logs" "$APP_DIR/assets" "$APP_DIR/bin" "$APP_DIR/web"

    local pid
    if pid="$(running_pid)"; then
        warn "服务已在运行（PID $pid），如需重启请用: $0 restart"
        return 0
    fi

    [ -x "$BIN" ] || die "找不到可执行文件 $BIN，请先在本地执行 scripts/deploy.sh"
    [ -f "$OCTOPUS_CONFIG" ] || warn "$OCTOPUS_CONFIG 不存在，将使用内置默认配置（AI 调用会失败）"

    log "启动后端"
    log "  监听地址 : $OCTOPUS_ADDR"
    log "  数据库   : $OCTOPUS_DB"
    log "  资产库   : $OCTOPUS_ASSETS"
    log "  配置     : $OCTOPUS_CONFIG"
    log "  日志     : $LOG_FILE"

    cd "$APP_DIR"
    # nohup + 重定向 stdin/stdout/stderr：ssh 断开后进程继续跑
    nohup "$BIN" >>"$LOG_FILE" 2>&1 </dev/null &
    local new_pid=$!
    echo "$new_pid" >"$PID_FILE"

    log "等待 ${HEALTH_URL} 就绪 ..."
    for _ in $(seq 1 30); do
        if curl -fsS -m 2 "$HEALTH_URL" >/dev/null 2>&1; then
            ok "启动成功（PID $new_pid），健康检查通过"
            return 0
        fi
        if ! kill -0 "$new_pid" 2>/dev/null; then
            rm -f "$PID_FILE"
            warn "进程已退出，最后 20 行日志："
            tail -n 20 "$LOG_FILE" 2>/dev/null || true
            return 1
        fi
        sleep 1
    done

    warn "30 秒内健康检查未通过，进程仍在运行（PID $new_pid），请查看日志: $0 logs"
    return 1
}

do_stop() {
    local pid
    if ! pid="$(running_pid)"; then
        log "服务未在运行"
        rm -f "$PID_FILE"
        return 0
    fi

    log "停止服务（PID $pid）..."
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do
        if ! kill -0 "$pid" 2>/dev/null; then
            rm -f "$PID_FILE"
            ok "已停止"
            return 0
        fi
        sleep 0.5
    done

    warn "10 秒内未退出，强制结束"
    kill -9 "$pid" 2>/dev/null || true
    rm -f "$PID_FILE"
    ok "已强制停止"
}

do_status() {
    local pid
    if pid="$(running_pid)"; then
        printf "${GREEN}● 后端进程: 运行中${NC} (PID %s)\n" "$pid"
    else
        printf "${YELLOW}○ 后端进程: 未运行${NC}\n"
    fi

    if curl -fsS -m 2 "$HEALTH_URL" >/dev/null 2>&1; then
        printf "${GREEN}● 健康检查: 通过${NC} (%s)\n" "$HEALTH_URL"
    else
        printf "${YELLOW}○ 健康检查: 未通过${NC} (%s)\n" "$HEALTH_URL"
    fi

    local port="${OCTOPUS_ADDR##*:}"
    if command -v ss >/dev/null 2>&1 && ss -tln 2>/dev/null | grep -qE "[:.]${port}\b"; then
        printf "${GREEN}● 端口 %s: 已监听${NC}\n" "$port"
    else
        printf "${YELLOW}○ 端口 %s: 未监听${NC}\n" "$port"
    fi

    if [ -d "$APP_DIR/web" ]; then
        printf "  前端 dist: %s (%s)\n" "$APP_DIR/web" "$(du -sh "$APP_DIR/web" 2>/dev/null | cut -f1)"
    else
        printf "${YELLOW}  前端 dist: 缺失${NC}\n"
    fi
}

case "${1:-start}" in
    start)      do_start ;;
    stop)       do_stop ;;
    restart)    do_stop; do_start ;;
    status)     do_status ;;
    health)     curl -fsS -m 5 "$HEALTH_URL" && echo ;;
    logs)       [ -f "$LOG_FILE" ] || die "日志文件不存在: $LOG_FILE"; tail -n 50 -f "$LOG_FILE" ;;
    foreground) cd "$APP_DIR"; exec "$BIN" ;;
    -h|--help|help)
        sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        ;;
    *) die "未知命令 $1（可用: start stop restart status logs health foreground）" ;;
esac
