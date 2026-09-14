#!/usr/bin/env bash
# scripts/lmop-engine-check.sh —— 用**真实引擎**校验 LMoP 故事书（T15 缺口闭合验收）。
#
# 做什么：
#   1. 构建独立校验器 crate（scripts/lmop-engine-check/，只 path 依赖 crates/，不修改 crates/）
#   2. 跑发布门（validate_storybook + lint_storybook）+ 运行时规则断言（真实 LuaHost / 骨架求值器）
#
# 用法：
#   ./scripts/lmop-engine-check.sh                      # 校验 story_example/lmop-storybook.json
#   ./scripts/lmop-engine-check.sh <storybook.json>
#   ./scripts/lmop-engine-check.sh --build-only
#
# 构建产物落在 .scratch/（已 gitignore），不污染仓库。
set -uo pipefail
cd "$(dirname "$0")/.."

MANIFEST=scripts/lmop-engine-check/Cargo.toml
TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/.scratch/lmop-engine-check-target}"
BIN="$TARGET_DIR/debug/lmop-engine-check"
SB="story_example/lmop-storybook.json"

if [ "${1:-}" = "--build-only" ]; then
  CARGO_TARGET_DIR="$TARGET_DIR" cargo build --manifest-path "$MANIFEST" || exit 1
  echo "[OK] 校验器已构建：$BIN"
  exit 0
fi
if [ -n "${1:-}" ]; then SB="$1"; fi

echo "== LMoP 真实引擎校验 =="
echo "storybook = $SB"
CARGO_TARGET_DIR="$TARGET_DIR" cargo build --manifest-path "$MANIFEST" 2>&1 | tail -2 || exit 1
"$BIN" "$SB"
code=$?
echo "退出码 = $code（0 = 发布门 0 error / lua_lint 0 issue 且全部规则断言通过）"
exit $code
