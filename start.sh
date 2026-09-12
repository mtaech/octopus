#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# Octopus 项目启动管理脚本
# 支持一键启动前端与后端、单独启动、状态检测与优雅停止。
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# 终端彩色输出支持
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    BLUE='\033[0;34m'
    YELLOW='\033[1;33m'
    CYAN='\033[0;36m'
    RED='\033[0;31m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    GREEN=''
    BLUE=''
    YELLOW=''
    CYAN=''
    RED=''
    BOLD=''
    NC=''
fi

# 加载 .env 环境变量文件（如果存在）
if [ -f "$SCRIPT_DIR/.env" ]; then
    echo -e "${CYAN}[env] 加载配置文件: .env${NC}"
    set -a
    # shellcheck disable=SC1091
    source "$SCRIPT_DIR/.env"
    set +a
fi

# 默认配置
TARGET="all"
RELEASE_FLAG=""
OCTOPUS_ADDR="${OCTOPUS_ADDR:-127.0.0.1:8787}"
OCTOPUS_DB="${OCTOPUS_DB:-$SCRIPT_DIR/octopus.db}"
RUST_LOG="${RUST_LOG:-info,sqlx=warn}"

show_help() {
    cat <<EOF
Octopus 启动管理脚本

用法:
  ./start.sh [选项] [命令/目标]

命令 / 目标 (可选，默认 all):
  all         同时启动后端与前端服务 (前台运行)
  backend     仅启动后端服务 (Rust / axum)
  frontend    仅启动前端服务 (Vue 3 / Vite)
  status      查看当前后端与前端服务运行状态
  stop        停止正在运行的 Octopus 后端与前端进程

选项:
  -b, --backend         仅启动后端 (等同于 backend)
  -f, --frontend        仅启动前端 (等同于 frontend)
  -r, --release         以 release 编译模式运行后端 (默认 debug)
  -p, --port <port>     设置后端监听端口 (默认: 8787)
  --db <path>           设置 SQLite 数据库路径 (默认: ./octopus.db)
  -h, --help            显示此帮助信息

环境变量支持:
  OCTOPUS_ADDR          后端监听地址 (例如 127.0.0.1:8787)
  OCTOPUS_DB            SQLite 数据库路径 (例如 ./octopus.db)
  RUST_LOG              日志过滤级别 (默认: info,sqlx=warn)
  OCTOPUS_LOG_FORMAT    日志格式 (默认文本; 设 json 输出结构化 JSON)
EOF
}

# 检测 Node.js 包管理器
detect_pkg_manager() {
    if command -v pnpm &>/dev/null; then
        echo "pnpm"
    elif command -v npm &>/dev/null; then
        echo "npm"
    elif command -v bun &>/dev/null; then
        echo "bun"
    elif command -v yarn &>/dev/null; then
        echo "yarn"
    else
        echo ""
    fi
}

is_port_in_use() {
    local port="$1"
    if command -v ss &>/dev/null; then
        ss -tln | grep -qE "[:.]${port}\b"
    elif command -v lsof &>/dev/null; then
        lsof -i :"$port" &>/dev/null
    else
        (exec 3<>/dev/tcp/127.0.0.1/"$port") 2>/dev/null && exec 3>&-
    fi
}

# 检查当前服务状态
do_status() {
    echo -e "${BOLD}${CYAN}--- 服务状态检查 ---${NC}"
    local backend_pids
    backend_pids=$(pgrep -f "octopus-bin" | tr '\n' ' ' || true)
    if [ -n "${backend_pids// /}" ]; then
        echo -e "${GREEN}● 后端 (octopus-bin): 正在运行 (PID: ${backend_pids})${NC}"
    else
        echo -e "${YELLOW}○ 后端 (octopus-bin): 未运行${NC}"
    fi

    local frontend_pids
    frontend_pids=$(pgrep -f "vite" | tr '\n' ' ' || true)
    if [ -n "${frontend_pids// /}" ]; then
        echo -e "${GREEN}● 前端 (vite):        正在运行 (PID: ${frontend_pids})${NC}"
    else
        echo -e "${YELLOW}○ 前端 (vite):        未运行${NC}"
    fi

    local backend_port="${OCTOPUS_ADDR##*:}"
    if is_port_in_use "$backend_port"; then
        echo -e "  - 端口 ${backend_port} (后端): 已被占用"
    else
        echo -e "  - 端口 ${backend_port} (后端): 空闲"
    fi

    if is_port_in_use "5173"; then
        echo -e "  - 端口 5173 (前端): 已被占用"
    else
        echo -e "  - 端口 5173 (前端): 空闲"
    fi
}

# 停止可能遗留的后台进程
do_stop() {
    echo -e "${YELLOW}[stop] 正在查找并停止已有进程...${NC}"
    local backend_pids
    backend_pids=$(pgrep -f "octopus-bin" || true)
    if [ -n "$backend_pids" ]; then
        echo -e "  停止 octopus-bin (PID: ${backend_pids})..."
        kill $backend_pids 2>/dev/null || true
    fi

    local frontend_pids
    frontend_pids=$(pgrep -f "vite" || true)
    if [ -n "$frontend_pids" ]; then
        echo -e "  停止 vite (PID: ${frontend_pids})..."
        kill $frontend_pids 2>/dev/null || true
    fi

    sleep 0.5
    echo -e "${GREEN}[stop] 停止操作完成。${NC}"
}

# 解析命令行参数
while [[ $# -gt 0 ]]; do
    case "$1" in
        all|backend|frontend|status|stop)
            TARGET="$1"
            shift
            ;;
        -b|--backend)
            TARGET="backend"
            shift
            ;;
        -f|--frontend)
            TARGET="frontend"
            shift
            ;;
        -r|--release)
            RELEASE_FLAG="--release"
            shift
            ;;
        -p|--port)
            if [[ -n "${2:-}" ]]; then
                OCTOPUS_ADDR="127.0.0.1:$2"
                shift 2
            else
                echo -e "${RED}错误: --port 需要指定端口号${NC}" >&2
                exit 1
            fi
            ;;
        --db)
            if [[ -n "${2:-}" ]]; then
                OCTOPUS_DB="$2"
                shift 2
            else
                echo -e "${RED}错误: --db 需要指定文件路径${NC}" >&2
                exit 1
            fi
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo -e "${RED}错误: 未知参数 $1${NC}" >&2
            show_help
            exit 1
            ;;
    esac
done

export OCTOPUS_ADDR
export OCTOPUS_DB
export RUST_LOG

# 针对 status 与 stop 单独分支处理
if [ "$TARGET" = "status" ]; then
    do_status
    exit 0
fi

if [ "$TARGET" = "stop" ]; then
    do_stop
    exit 0
fi

BACKEND_PID=""
FRONTEND_PID=""

kill_tree() {
    local pid="$1"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        pkill -P "$pid" 2>/dev/null || true
        kill "$pid" 2>/dev/null || true
    fi
}

cleanup() {
    trap - SIGINT SIGTERM EXIT
    echo -e "\n${YELLOW}[stop] 收到退出信号，正在关闭子服务...${NC}"
    kill_tree "$FRONTEND_PID"
    kill_tree "$BACKEND_PID"
    wait 2>/dev/null || true
    echo -e "${GREEN}[stop] 服务已全部安全退出。${NC}"
}

start_backend() {
    if ! command -v cargo &>/dev/null; then
        echo -e "${RED}[error] 未找到 cargo，请先安装 Rust 工具链并将其加入 PATH。${NC}" >&2
        exit 1
    fi

    local port="${OCTOPUS_ADDR##*:}"
    if is_port_in_use "$port"; then
        echo -e "${YELLOW}[warn] 端口 ${port} 当前已被占用！若已有实例运行，可执行 ./start.sh stop 先行清理。${NC}"
    fi

    echo -e "${BLUE}[backend] 启动后端服务 (cargo run -p octopus-bin ${RELEASE_FLAG:-debug})...${NC}"
    echo -e "${BLUE}[backend] API 地址: http://${OCTOPUS_ADDR}${NC}"
    echo -e "${BLUE}[backend] 数据库:   ${OCTOPUS_DB}${NC}"
    echo -e "${BLUE}[backend] 日志过滤: ${RUST_LOG}${NC}"

    cargo run -p octopus-bin ${RELEASE_FLAG} &
    BACKEND_PID=$!
}

start_frontend() {
    local PKG_MGR
    PKG_MGR=$(detect_pkg_manager)
    if [ -z "$PKG_MGR" ]; then
        echo -e "${RED}[error] 未找到 Node.js 包管理器 (pnpm / npm / bun / yarn)，无法启动前端。${NC}" >&2
        exit 1
    fi

    if [ ! -d "$SCRIPT_DIR/frontend/node_modules" ]; then
        echo -e "${YELLOW}[frontend] 未检测到 node_modules，正在执行 ${PKG_MGR} install...${NC}"
        (cd "$SCRIPT_DIR/frontend" && "$PKG_MGR" install)
    fi

    if is_port_in_use "5173"; then
        echo -e "${YELLOW}[warn] 端口 5173 当前已被占用，Vite 将自动尝试下一个空闲端口（例如 5174）。${NC}"
    fi

    echo -e "${GREEN}[frontend] 启动前端服务 (${PKG_MGR} run dev)...${NC}"
    echo -e "${GREEN}[frontend] 默认访问地址: http://127.0.0.1:5173${NC}"

    (cd "$SCRIPT_DIR/frontend" && "$PKG_MGR" run dev) &
    FRONTEND_PID=$!
}

echo -e "${BOLD}${CYAN}====================================================${NC}"
echo -e "${BOLD}${CYAN}             Octopus 项目启动管理脚本               ${NC}"
echo -e "${BOLD}${CYAN}====================================================${NC}"

trap cleanup SIGINT SIGTERM EXIT

case "$TARGET" in
    all)
        start_backend
        sleep 1
        start_frontend
        echo -e "\n${BOLD}${GREEN}✔ 后端与前端已启动！按 Ctrl+C 可停止所有服务。${NC}\n"
        wait
        ;;
    backend)
        start_backend
        echo -e "\n${BOLD}${GREEN}✔ 后端已启动！按 Ctrl+C 可停止服务。${NC}\n"
        wait "$BACKEND_PID"
        ;;
    frontend)
        start_frontend
        echo -e "\n${BOLD}${GREEN}✔ 前端已启动！按 Ctrl+C 可停止服务。${NC}\n"
        wait "$FRONTEND_PID"
        ;;
esac
