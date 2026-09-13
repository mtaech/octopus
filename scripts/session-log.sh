#!/usr/bin/env bash
# 按存档查看会话日志（直接读权威命令日志 octopus.db，无需后端在跑）。
#
# 用法：
#   ./scripts/session-log.sh                      # 列出存档（拿 save_id）
#   ./scripts/session-log.sh <save_id>            # 打印该存档最近 60 条事件
#   ./scripts/session-log.sh <save_id> rejected   # 只看被驳回的意图
#
# 数据库路径：$OCTOPUS_DB，缺省为仓库根目录的 octopus.db（与 start.sh 一致）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DB="${OCTOPUS_DB:-$(cd "$SCRIPT_DIR/.." && pwd)/octopus.db}"

if [ ! -f "$DB" ]; then
    echo "找不到数据库：$DB（可用 OCTOPUS_DB 指定）" >&2
    exit 1
fi

python3 - "$DB" "$@" <<'PY'
import json, sqlite3, sys

db, args = sys.argv[1], sys.argv[2:]
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
cur = con.cursor()

if not args:
    cur.execute("SELECT id, title, last_played_at FROM saves ORDER BY last_played_at DESC")
    print("存档列表（用法：session-log.sh <save_id> [rejected]）")
    for sid, title, ts in cur.fetchall():
        print(f"  {sid}  |  {title}  |  {ts}")
    sys.exit(0)

sid = args[0]
where, params = "save_id = ?", [sid]
if len(args) > 1 and args[1] == "rejected":
    where += " AND kind = 'resolution' AND payload_json LIKE '%\"status\":\"rejected\"%'"

cur.execute(
    f"SELECT seq, round, kind, payload_json FROM commands WHERE {where} ORDER BY seq DESC LIMIT 60",
    params,
)
rows = cur.fetchall()
if not rows:
    print("（无记录。确认 save_id 是否正确）")
    sys.exit(0)

for seq, rnd, kind, payload in reversed(rows):
    p = json.loads(payload)
    pl = p.get("payload", {})
    actor = p.get("actor") or {}
    if kind == "resolution":
        extra = f"{pl.get('status')} code={pl.get('rejection_code')} | {pl.get('narrative')}"
    elif kind in ("dialogue", "narrate", "emote"):
        extra = f"{actor.get('name', '?')}: {str(pl.get('content', ''))[:60]}"
    elif kind == "phase":
        extra = pl.get("stage", "")
    elif kind == "reasoning":
        extra = pl.get("stage", "")
    elif kind == "round_start":
        extra = str(pl.get("input", {}).get("text", ""))[:50]
    elif kind == "system":
        extra = f"[{pl.get('code')}] {str(pl.get('text', ''))[:50]}"
    elif kind == "ai_call":
        u = pl.get("usage") or {}
        extra = f"{pl.get('status')} {pl.get('provider')}/{pl.get('model')} in={u.get('input_tokens')} out={u.get('output_tokens')} cached={u.get('cached_input_tokens')} {pl.get('latency_ms')}ms" + (f" | {pl.get('error')}" if pl.get('error') else "")
    else:
        extra = ""
    print(seq, f"r{rnd}", kind, extra)
PY
