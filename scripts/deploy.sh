#!/usr/bin/env bash
# ==============================================================================
# Octopus 远程部署脚本（本地执行）
#
# 服务器（ali-dev）上没有 Rust / Node 工具链，所以全部产物都在本地构建好再上传：
#   后端   cargo build --release -p octopus-bin   ->  bin/octopus-bin
#   前端   pnpm -C frontend build                ->  web/
#
# 用法:
#   ./scripts/deploy.sh [选项]
#
# 选项:
#   --skip-build     跳过构建，直接上传现有产物
#   --backend-only   只构建并上传后端
#   --frontend-only  只构建并上传前端
#   --push-config    连本地 config.json 一起上传（覆盖服务器上的同名文件）
#   --restart        上传完成后在服务器执行 ./server-start.sh restart
#   --dry-run        只打印将要执行的命令，不实际改动
#   -h, --help       显示帮助
#
# 环境变量:
#   OCTOPUS_REMOTE       ssh 主机别名（默认 ali-dev）
#   OCTOPUS_REMOTE_DIR   服务器部署目录（默认 /home/huang/octopus）
# ==============================================================================
set -euo pipefail

LOCAL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="${OCTOPUS_REMOTE:-ali-dev}"
REMOTE_DIR="${OCTOPUS_REMOTE_DIR:-/home/huang/octopus}"

DO_BACKEND=1
DO_FRONTEND=1
SKIP_BUILD=0
PUSH_CONFIG=0
DO_RESTART=0
DRY_RUN=0

if [ -t 1 ]; then
    CYAN='\033[0;36m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; BOLD='\033[1m'; NC='\033[0m'
else
    CYAN=''; GREEN=''; YELLOW=''; RED=''; BOLD=''; NC=''
fi

log()  { printf "${CYAN}[deploy]${NC} %s\n" "$*"; }
ok()   { printf "${GREEN}[deploy]${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}[warn]${NC} %s\n" "$*" >&2; }
die()  { printf "${RED}[error]${NC} %s\n" "$*" >&2; exit 1; }

show_help() {
    sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

remote_run() {
    if [ "$DRY_RUN" = 1 ]; then
        printf '  [dry-run] ssh %s %s\n' "$REMOTE" "$1"
    else
        ssh -o BatchMode=yes "$REMOTE" "$1"
    fi
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)    SKIP_BUILD=1; shift ;;
        --backend-only)  DO_FRONTEND=0; shift ;;
        --frontend-only) DO_BACKEND=0; shift ;;
        --push-config)   PUSH_CONFIG=1; shift ;;
        --restart)       DO_RESTART=1; shift ;;
        --dry-run)       DRY_RUN=1; shift ;;
        -h|--help)       show_help; exit 0 ;;
        *) die "未知参数 $1（用 --help 查看用法）" ;;
    esac
done

printf "${BOLD}${CYAN}=== Octopus 部署 -> %s:%s ===${NC}\n" "$REMOTE" "$REMOTE_DIR"

# ------------------------------------------------------------------------------
# 0. 连通性检查
# ------------------------------------------------------------------------------
log "检查 ssh 连通性 ..."
if [ "$DRY_RUN" = 0 ]; then
    ssh -o BatchMode=yes -o ConnectTimeout=15 "$REMOTE" true \
        || die "无法免密登录 $REMOTE，请先配置 ssh key"
    ok "ssh 正常"
fi

# ------------------------------------------------------------------------------
# 1. 构建
# ------------------------------------------------------------------------------
if [ "$SKIP_BUILD" = 0 ]; then
    if [ "$DO_BACKEND" = 1 ]; then
        command -v cargo >/dev/null || die "未找到 cargo"
        log "构建后端 release 二进制（cargo build --release -p octopus-bin）..."
        ( cd "$LOCAL_DIR" && cargo build --release -p octopus-bin )
        ok "后端构建完成"
    fi
    if [ "$DO_FRONTEND" = 1 ]; then
        command -v pnpm >/dev/null || die "未找到 pnpm"
        log "构建前端产物（pnpm -C frontend build）..."
        ( cd "$LOCAL_DIR" && pnpm -C frontend build )
        ok "前端构建完成"
    fi
else
    warn "跳过构建（--skip-build）"
fi

# ------------------------------------------------------------------------------
# 2. 准备服务器目录
# ------------------------------------------------------------------------------
log "准备服务器目录 ..."
remote_run "mkdir -p '$REMOTE_DIR/bin' '$REMOTE_DIR/web' '$REMOTE_DIR/assets' '$REMOTE_DIR/logs' '$REMOTE_DIR/run'"

# ------------------------------------------------------------------------------
# 3. 上传后端二进制（先传 .new 再原子替换：直接覆盖正在运行的二进制会报 Text file busy）
# ------------------------------------------------------------------------------
if [ "$DO_BACKEND" = 1 ]; then
    # target 目录可能被 ~/.cargo/config.toml 改到别处（本机就改到了 /home/huang/cargo-target），问 cargo 要
    TARGET_DIR="$( cd "$LOCAL_DIR" && cargo metadata --no-deps --format-version 1 2>/dev/null \
        | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' )"
    [ -n "$TARGET_DIR" ] || TARGET_DIR="$LOCAL_DIR/target"
    BIN="$TARGET_DIR/release/octopus-bin"
    [ -f "$BIN" ] || die "找不到 $BIN，请先构建"
    log "上传后端二进制（$(du -h "$BIN" | cut -f1)）..."
    if [ "$DRY_RUN" = 1 ]; then
        printf '  [dry-run] scp %s %s:%s/bin/octopus-bin.new\n' "$BIN" "$REMOTE" "$REMOTE_DIR"
    else
        scp -q "$BIN" "$REMOTE:$REMOTE_DIR/bin/octopus-bin.new"
        ssh -o BatchMode=yes "$REMOTE" \
            "chmod +x '$REMOTE_DIR/bin/octopus-bin.new' && mv -f '$REMOTE_DIR/bin/octopus-bin.new' '$REMOTE_DIR/bin/octopus-bin'"
    fi
    ok "后端已上传"
fi

# ------------------------------------------------------------------------------
# 4. 上传前端 dist（tar 流式传输 + 整目录替换，避免残留旧 hash 资源）
# ------------------------------------------------------------------------------
if [ "$DO_FRONTEND" = 1 ]; then
    DIST="$LOCAL_DIR/frontend/dist"
    [ -f "$DIST/index.html" ] || die "找不到 $DIST/index.html，请先构建前端"
    log "上传前端 dist（$(du -sh "$DIST" | cut -f1)）..."
    if [ "$DRY_RUN" = 1 ]; then
        printf '  [dry-run] tar -C %s -czf - . | ssh %s tar -xzf - -C %s/web.next\n' "$DIST" "$REMOTE" "$REMOTE_DIR"
    else
        ssh -o BatchMode=yes "$REMOTE" "rm -rf '$REMOTE_DIR/web.next' && mkdir -p '$REMOTE_DIR/web.next'"
        tar -C "$DIST" -czf - . | ssh -o BatchMode=yes "$REMOTE" "tar -xzf - -C '$REMOTE_DIR/web.next'"
        # web -> web.prev，web.next -> web：切目录是原子的，不会出现半截站点
        ssh -o BatchMode=yes "$REMOTE" \
            "rm -rf '$REMOTE_DIR/web.prev'; if [ -d '$REMOTE_DIR/web' ]; then mv '$REMOTE_DIR/web' '$REMOTE_DIR/web.prev'; fi; mv '$REMOTE_DIR/web.next' '$REMOTE_DIR/web'"
    fi
    ok "前端已上传"
fi

# ------------------------------------------------------------------------------
# 5. 上传服务器启动脚本与 Caddy 配置块
# ------------------------------------------------------------------------------
log "上传 server-start.sh / caddy-octopus.conf ..."
if [ "$DRY_RUN" = 1 ]; then
    printf '  [dry-run] scp scripts/{server-start.sh,caddy-octopus.conf} %s:%s/\n' "$REMOTE" "$REMOTE_DIR"
else
    scp -q "$LOCAL_DIR/scripts/server-start.sh" "$REMOTE:$REMOTE_DIR/server-start.sh"
    scp -q "$LOCAL_DIR/scripts/caddy-octopus.conf" "$REMOTE:$REMOTE_DIR/caddy-octopus.conf"
    ssh -o BatchMode=yes "$REMOTE" "chmod +x '$REMOTE_DIR/server-start.sh'"
fi
ok "脚本已上传"

# ------------------------------------------------------------------------------
# 6. config.json（含 LLM 密钥，0600）
#    默认只在服务器上没有时才推送，避免覆盖服务器上的修改；--push-config 强制覆盖。
# ------------------------------------------------------------------------------
CFG="$LOCAL_DIR/config.json"
if [ -f "$CFG" ]; then
    if [ "$PUSH_CONFIG" = 1 ]; then
        log "推送 config.json（--push-config，强制覆盖服务器上的同名文件）..."
        if [ "$DRY_RUN" = 0 ]; then
            scp -q "$CFG" "$REMOTE:$REMOTE_DIR/config.json"
            ssh -o BatchMode=yes "$REMOTE" "chmod 600 '$REMOTE_DIR/config.json'"
        fi
        ok "config.json 已推送"
    elif [ "$DRY_RUN" = 0 ] && ! ssh -o BatchMode=yes "$REMOTE" "test -f '$REMOTE_DIR/config.json'"; then
        warn "服务器上没有 config.json，先推一份过去（之后如需覆盖请用 --push-config）"
        scp -q "$CFG" "$REMOTE:$REMOTE_DIR/config.json"
        ssh -o BatchMode=yes "$REMOTE" "chmod 600 '$REMOTE_DIR/config.json'"
    fi
else
    warn "本地没有 config.json，服务器将使用内置默认配置（AI 调用会失败）"
fi

# ------------------------------------------------------------------------------
# 7. 可选：重启服务
# ------------------------------------------------------------------------------
if [ "$DO_RESTART" = 1 ]; then
    log "重启服务器进程 ..."
    remote_run "'$REMOTE_DIR/server-start.sh' restart"
    ok "已重启"
fi

echo
ok "部署完成。"
echo "  远端目录 : $REMOTE_DIR"
echo "  启动服务 : ssh $REMOTE '$REMOTE_DIR/server-start.sh start'"
echo "  查看状态 : ssh $REMOTE '$REMOTE_DIR/server-start.sh status'"
echo "  查看日志 : ssh $REMOTE '$REMOTE_DIR/server-start.sh logs'"
