#!/usr/bin/env bash
# 上下文缓存对账：直接读权威命令日志 octopus.db 里的 ai_call 事件（无需后端在跑）。
#
# 用法：
#   ./scripts/cache-audit.sh                # 各存档的汇总命中率
#   ./scripts/cache-audit.sh <save_id>      # 该存档逐次调用的明细
#
# 看什么：供应商的上下文缓存是**前缀缓存**——按请求序列的公共前缀匹配，
# 队首一变后面整段失效。所以「cache 命中率」之外还要看 **与上一次请求的公共前缀**：
# 老回合整段命中、序列只追加，才是健康状态。
#
# 数据库路径：$OCTOPUS_DB，缺省为仓库根目录的 octopus.db（与 start.sh / session-log.sh 一致）。
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


def calls(save_id=None):
    """按 seq 升序取出带完整上下文的 ai_call 事件。"""
    sql = ("SELECT save_id, seq, round, payload_json FROM commands "
           "WHERE kind = 'ai_call'")
    params = []
    if save_id:
        sql += " AND save_id = ?"
        params.append(save_id)
    sql += " ORDER BY seq"
    out = []
    for sid, seq, rnd, pj in cur.execute(sql, params):
        p = (json.loads(pj) or {}).get("payload") or {}
        if not p.get("messages"):
            continue  # 失败留痕的事件没有上下文
        u = p.get("usage") or {}
        out.append({
            "save_id": sid,
            "seq": seq,
            "round": rnd,
            "input": u.get("input_tokens") or 0,
            "cached": u.get("cached_input_tokens") or 0,
            "msgs": p["messages"],
        })
    return out


def hit_pct(c):
    return (100.0 * c["cached"] / c["input"]) if c["input"] else 0.0


def common_prefix(a, b):
    i = 0
    while i < min(len(a), len(b)) and a[i] == b[i]:
        i += 1
    return i


if not args:
    print("存档汇总（input 合计 / 命中合计 / 命中率）")
    agg = {}
    for c in calls():
        a = agg.setdefault(c["save_id"], [0, 0, 0])
        a[0] += 1
        a[1] += c["input"]
        a[2] += c["cached"]
    for sid, (n, ti, tc) in agg.items():
        pct = (100.0 * tc / ti) if ti else 0.0
        print(f"  {sid}  调用 {n:>3} 次  input {ti:>10,}  cached {tc:>10,}  命中 {pct:5.1f}%")
    print("\n明细：./scripts/cache-audit.sh <save_id>")
    sys.exit(0)

sid = args[0]
rows = calls(sid)
if not rows:
    print(f"（{sid} 没有带上下文的 ai_call 记录）")
    sys.exit(0)

ti = sum(c["input"] for c in rows)
tc = sum(c["cached"] for c in rows)
print(f"{sid}：调用 {len(rows)} 次  input {ti:,}  cached {tc:,}  命中 {100.0 * tc / ti if ti else 0:.1f}%")
print()
print(f"{'轮':>4} {'input':>9} {'cached':>9} {'命中':>7} {'消息':>5}  前缀")
prev = None
for c in rows:
    note = ""
    if prev is not None:
        i = common_prefix(prev["msgs"], c["msgs"])
        if i == 0:
            note = "system 就变了（前缀全废）"
        elif i == len(prev["msgs"]) and len(prev["msgs"]) <= len(c["msgs"]):
            note = f"整段命中（+{len(c['msgs']) - len(prev['msgs'])} 条追加）"
        else:
            note = f"从第 {i} 条起失配（历史被改写 / 压缩）"
    print(f"{c['round']:>4} {c['input']:>9,} {c['cached']:>9,} {hit_pct(c):>6.1f}% {len(c['msgs']):>5}  {note}")
    prev = c
PY
