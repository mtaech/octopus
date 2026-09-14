#!/usr/bin/env python3
"""清理 octopus.db 中的端到端验证残留数据（账户 / 故事书 / 存档及其子表）。

背景
----
多轮端到端验证会在真实库 octopus.db 里留下「验证账户 + 验证故事书 + 验证存档」。
该库**没有声明任何外键**（PRAGMA foreign_key_list 全为空），因此必须手工清理子表，
否则会留下孤儿行。

行为
----
* 默认 dry-run：只计算并打印将要删除的内容（逐表行数 + 受影响 id），绝不写库。
* --apply：真正执行；执行前用 sqlite3 backup API 备份到 .scratch/，打印备份路径与字节数。
* 幂等：没有残留时打印「无残留可清理」，退出码 0，且不产生备份。
* 保护对象校验：若候选集合会命中受保护的账户/故事书/存档，拒绝执行并 exit 2。

退出码
------
0  成功（dry-run 有/无残留、apply 成功、幂等空跑）
2  拒绝执行（受保护对象将被误删）
1  其他错误（库不存在、表结构不符等）

用法
----
    python3 scripts/cleanup-verification-data.py            # dry-run
    python3 scripts/cleanup-verification-data.py --apply    # 备份后执行
"""

from __future__ import annotations

import argparse
import datetime as _dt
import os
import re
import sqlite3
import sys

# --------------------------------------------------------------------------
# 删除规则（显式且保守）
# --------------------------------------------------------------------------

# 账户：username 以 verify + 数字开头（verify2*、verify3* …）
VERIFY_USERNAME_RE = re.compile(r"^verify\d")

# 故事书 / 存档：标题包含「验证」
VERIFY_TITLE_MARKER = "验证"

# --------------------------------------------------------------------------
# 绝不许删的保护对象
# --------------------------------------------------------------------------

PROTECTED_USER_IDS = {"user-octopus"}
PROTECTED_USERNAMES = {"octopus"}
PROTECTED_STORYBOOK_IDS = {"sb-ffa1d260083048a68f1e63a2e9a219c3"}
PROTECTED_SAVE_IDS = {"sv-61111592c52c4437ac8282147e60f45a"}

# 子表清单：表名 -> 关联键列（用于按父 id 删除）
SAVE_CHILD_TABLES = [
    ("commands", "save_id"),
    ("archived_commands", "save_id"),
    ("maintenance", "save_id"),
    ("round_summaries", "save_id"),
    ("scene_summaries", "save_id"),
    ("snapshots", "save_id"),
    ("ai_conversations", "save_id"),
    ("events_fts", "save_id"),
]
SESSION_CHILD_TABLES = [("sessions", "user_id")]
PAIR_THREAD_CHILD_TABLES = [("pair_messages", "thread_id")]

# 备份目录
DEFAULT_BACKUP_DIR = ".scratch"

# dry-run 明细里最多列出的 id 个数
MAX_IDS_SHOWN = 10


class RefusedError(RuntimeError):
    """受保护对象将被误删，拒绝执行。"""


# --------------------------------------------------------------------------
# 工具函数
# --------------------------------------------------------------------------


def q(conn: sqlite3.Connection, sql: str, params: tuple = ()) -> list:
    return conn.execute(sql, params).fetchall()


def placeholders(n: int) -> str:
    return ",".join("?" * n)


def like_escape_marker(marker: str) -> str:
    """把标题标记转成安全的 LIKE 模式（对 % _ \\ 转义）。"""
    escaped = marker.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
    return f"%{escaped}%"


def table_exists(conn: sqlite3.Connection, name: str) -> bool:
    row = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name=?",
        (name,),
    ).fetchone()
    return row is not None


def table_count(conn: sqlite3.Connection, name: str) -> int:
    if not table_exists(conn, name):
        return 0
    return conn.execute(f'SELECT count(*) FROM "{name}"').fetchone()[0]


def fmt_ids(ids) -> str:
    ids = sorted(ids)
    if not ids:
        return "（无）"
    if len(ids) <= MAX_IDS_SHOWN:
        return ", ".join(ids)
    head = ", ".join(ids[:MAX_IDS_SHOWN])
    return f"{head} … 共 {len(ids)} 个"


# --------------------------------------------------------------------------
# 计划计算
# --------------------------------------------------------------------------


def build_plan(conn: sqlite3.Connection) -> dict:
    """计算删除计划。只读，不修改数据库。"""
    # 1) 验证账户
    users = q(conn, "SELECT id, username FROM users ORDER BY username")
    verify_users = [
        {"id": uid, "username": uname}
        for uid, uname in users
        if VERIFY_USERNAME_RE.match(uname or "")
    ]
    user_ids = [u["id"] for u in verify_users]

    # 2) 故事书：标题含「验证」或由验证账户拥有
    storybooks = q(conn, "SELECT id, title, owner_id FROM storybooks ORDER BY id")
    sb_title = like_escape_marker(VERIFY_TITLE_MARKER)
    verify_storybooks = []
    for sid, title, owner_id in storybooks:
        by_title = bool(title) and (
            conn.execute(
                "SELECT 1 WHERE ? LIKE ? ESCAPE '\\'", (title, sb_title)
            ).fetchone()
            is not None
        )
        by_owner = owner_id in user_ids and owner_id is not None
        if by_title or by_owner:
            verify_storybooks.append(
                {
                    "id": sid,
                    "title": title,
                    "owner_id": owner_id,
                    "reason": "标题含验证" if by_title else "验证账户拥有",
                }
            )
    storybook_ids = [s["id"] for s in verify_storybooks]

    # 3) 存档：storybook_id 属于被删故事书，或标题含「验证」
    saves = q(conn, "SELECT id, title, storybook_id, owner_id FROM saves ORDER BY id")
    verify_saves = []
    for sid, title, storybook_id, owner_id in saves:
        by_sb = storybook_id in storybook_ids
        by_title = bool(title) and (
            conn.execute(
                "SELECT 1 WHERE ? LIKE ? ESCAPE '\\'", (title, sb_title)
            ).fetchone()
            is not None
        )
        if by_sb or by_title:
            verify_saves.append(
                {
                    "id": sid,
                    "title": title,
                    "storybook_id": storybook_id,
                    "owner_id": owner_id,
                    "reason": "属于验证故事书" if by_sb else "标题含验证",
                }
            )
    save_ids = [s["id"] for s in verify_saves]

    # 4) pair_threads：属于被删故事书
    threads = []
    if table_exists(conn, "pair_threads") and storybook_ids:
        threads = [
            {"id": tid, "storybook_id": sb, "title": t}
            for tid, sb, t in q(
                conn,
                f"SELECT id, storybook_id, title FROM pair_threads "
                f"WHERE storybook_id IN ({placeholders(len(storybook_ids))})",
                tuple(storybook_ids),
            )
        ]
    thread_ids = [t["id"] for t in threads]

    # 5) 逐表删除计数与谓词
    children = []  # (表, 列, 谓词 SQL, 参数, 说明)

    for table, col in SESSION_CHILD_TABLES:
        if user_ids and table_exists(conn, table):
            children.append(
                (
                    table,
                    col,
                    f'SELECT count(*), {col} FROM "{table}" WHERE {col} IN ({placeholders(len(user_ids))}) GROUP BY {col}',
                    tuple(user_ids),
                )
            )

    for table, col in SAVE_CHILD_TABLES:
        if save_ids and table_exists(conn, table):
            children.append(
                (
                    table,
                    col,
                    f'SELECT count(*), {col} FROM "{table}" WHERE {col} IN ({placeholders(len(save_ids))}) GROUP BY {col}',
                    tuple(save_ids),
                )
            )

    for table, col in PAIR_THREAD_CHILD_TABLES:
        if thread_ids and table_exists(conn, table):
            children.append(
                (
                    table,
                    col,
                    f'SELECT count(*), {col} FROM "{table}" WHERE {col} IN ({placeholders(len(thread_ids))}) GROUP BY {col}',
                    tuple(thread_ids),
                )
            )

    # pair_messages 另有 storybook_id 列：按故事书再兜一遍，避免孤儿
    pair_msg_by_sb = None
    if (
        storybook_ids
        and table_exists(conn, "pair_messages")
        and "storybook_id" in {c[1] for c in q(conn, "PRAGMA table_info(pair_messages)")}
    ):
        pair_msg_by_sb = (
            "pair_messages",
            "storybook_id",
            f'SELECT count(*), storybook_id FROM pair_messages '
            f'WHERE storybook_id IN ({placeholders(len(storybook_ids))}) GROUP BY storybook_id',
            tuple(storybook_ids),
        )

    # 逐表统计
    child_stats = []
    for table, col, sql, params in children:
        rows = q(conn, sql, params)
        total = sum(r[0] for r in rows)
        child_stats.append(
            {
                "table": table,
                "column": col,
                "count": total,
                "ids": [r[1] for r in rows],
                "note": "",
            }
        )
    if pair_msg_by_sb:
        table, col, sql, params = pair_msg_by_sb
        rows = q(conn, sql, params)
        if rows:
            child_stats.append(
                {
                    "table": table,
                    "column": col,
                    "count": sum(r[0] for r in rows),
                    "ids": [r[1] for r in rows],
                    "note": "按 storybook_id 兜底",
                }
            )

    # 父表计数
    # 注意：sessions 属于 user 的子表，已在 children 里统计，这里不重复计数
    parent_stats = [
        {"table": "pair_threads", "column": "storybook_id", "count": len(threads), "ids": thread_ids, "note": "父表: storybooks"},
        {"table": "saves", "column": "id", "count": len(save_ids), "ids": save_ids, "note": "父表: storybooks"},
        {"table": "storybooks", "column": "id", "count": len(storybook_ids), "ids": storybook_ids, "note": "父表: users"},
        {"table": "users", "column": "id", "count": len(user_ids), "ids": user_ids, "note": "根"},
    ]

    return {
        "users": verify_users,
        "user_ids": user_ids,
        "storybooks": verify_storybooks,
        "storybook_ids": storybook_ids,
        "saves": verify_saves,
        "save_ids": save_ids,
        "threads": threads,
        "thread_ids": thread_ids,
        "children": child_stats,
        "parents": parent_stats,
    }


# --------------------------------------------------------------------------
# 保护对象校验
# --------------------------------------------------------------------------


def check_protected(conn: sqlite3.Connection, plan: dict) -> list:
    """返回违规列表；非空表示拒绝执行。"""
    violations = []

    hit_users = set(plan["user_ids"]) & PROTECTED_USER_IDS
    for uid in hit_users:
        violations.append(f"受保护账户将被删除: {uid}")

    usernames = {u["username"] for u in plan["users"]}
    hit_names = usernames & PROTECTED_USERNAMES
    for name in hit_names:
        violations.append(f"受保护用户名将被删除: {name}")

    hit_sb = set(plan["storybook_ids"]) & PROTECTED_STORYBOOK_IDS
    for sid in hit_sb:
        violations.append(f"受保护故事书将被删除: {sid}")

    hit_sv = set(plan["save_ids"]) & PROTECTED_SAVE_IDS
    for sid in hit_sv:
        violations.append(f"受保护存档将被删除: {sid}")

    # 额外保守护栏：任何属于受保护账户（octopus）的故事书/存档都不得因标题匹配被删
    for s in plan["storybooks"]:
        if s["owner_id"] in PROTECTED_USER_IDS:
            violations.append(
                f"故事书 {s['id']} 属于受保护账户 {s['owner_id']}，拒绝删除（原因: {s['reason']}）"
            )
    for s in plan["saves"]:
        if s["owner_id"] in PROTECTED_USER_IDS:
            violations.append(
                f"存档 {s['id']} 属于受保护账户 {s['owner_id']}，拒绝删除（原因: {s['reason']}）"
            )

    return violations


def check_protected_sessions(conn: sqlite3.Connection, plan: dict) -> list:
    if not plan["user_ids"] or not table_exists(conn, "sessions"):
        return []
    rows = q(
        conn,
        f"SELECT token_hash, user_id FROM sessions WHERE user_id IN ({placeholders(len(plan['user_ids']))})",
        tuple(plan["user_ids"]),
    )
    bad = [r for r in rows if r[1] in PROTECTED_USER_IDS]
    return [f"受保护账户的会话将被删除: {r[1]} ({r[0][:12]}…)" for r in bad]


# --------------------------------------------------------------------------
# 报告
# --------------------------------------------------------------------------


def print_header(title: str) -> None:
    print()
    print("=" * 74)
    print(title)
    print("=" * 74)


def print_stats(conn: sqlite3.Connection, label: str) -> dict:
    tables = [
        "users",
        "storybooks",
        "saves",
        "commands",
        "archived_commands",
        "maintenance",
        "round_summaries",
        "scene_summaries",
        "snapshots",
        "ai_conversations",
        "events_fts",
        "sessions",
        "pair_threads",
        "pair_messages",
    ]
    stats = {t: table_count(conn, t) for t in tables}
    print(f"\n[{label}]")
    for t in tables:
        print(f"  {t:<20} {stats[t]}")
    return stats


def print_plan(plan: dict, apply_mode: bool) -> None:
    print_header("删除计划" + ("（将被执行）" if apply_mode else "（dry-run：不写库）"))

    print("\n账户（users）— 规则: username 匹配 ^verify\\d")
    if plan["users"]:
        for u in plan["users"]:
            print(f"  - {u['id']}  username={u['username']}")
    else:
        print("  （无）")

    print("\n故事书（storybooks）— 规则: 标题含「验证」或由验证账户拥有")
    if plan["storybooks"]:
        for s in plan["storybooks"]:
            print(f"  - {s['id']}  「{s['title']}」  ({s['reason']})")
    else:
        print("  （无）")

    print("\n存档（saves）— 规则: storybook_id 属于被删故事书，或标题含「验证」")
    if plan["saves"]:
        for s in plan["saves"]:
            print(f"  - {s['id']}  「{s['title']}」  ({s['reason']})")
    else:
        print("  （无）")

    print("\n逐表删除行数：")
    print(f"  {'表':<20} {'键列':<16} {'行数':>6}  受影响 id")
    total = 0
    for c in plan["parents"]:
        total += c["count"]
        note = f"  [{c['note']}]" if c["note"] else ""
        print(
            f"  {c['table']:<20} {c['column']:<16} {c['count']:>6}  {fmt_ids(c['ids'])}{note}"
        )
    for c in plan["children"]:
        total += c["count"]
        note = f"  [{c['note']}]" if c["note"] else ""
        print(
            f"  {c['table']:<20} {c['column']:<16} {c['count']:>6}  {fmt_ids(c['ids'])}{note}"
        )
    print(f"  {'-' * 20}")
    print(f"  {'合计':<20} {'':<16} {total:>6}")


# --------------------------------------------------------------------------
# 执行
# --------------------------------------------------------------------------


def make_backup(conn: sqlite3.Connection, db_path: str, backup_dir: str) -> tuple:
    os.makedirs(backup_dir, exist_ok=True)
    stamp = _dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    base = os.path.basename(db_path)
    dest = os.path.join(backup_dir, f"{base}.verify-cleanup-{stamp}.bak")
    bck = sqlite3.connect(dest)
    try:
        with bck:
            conn.backup(bck)
    finally:
        bck.close()
    return dest, os.path.getsize(dest)


def apply_plan(conn: sqlite3.Connection, plan: dict) -> None:
    """在单事务内按子表 -> 父表顺序删除。"""
    with conn:  # 事务
        # 1) 会话（users 的子表）
        if plan["user_ids"] and table_exists(conn, "sessions"):
            conn.execute(
                f"DELETE FROM sessions WHERE user_id IN ({placeholders(len(plan['user_ids']))})",
                tuple(plan["user_ids"]),
            )

        # 2) pair_messages（thread_id 或 storybook_id）
        if plan["thread_ids"] and table_exists(conn, "pair_messages"):
            conn.execute(
                f"DELETE FROM pair_messages WHERE thread_id IN ({placeholders(len(plan['thread_ids']))})",
                tuple(plan["thread_ids"]),
            )
        if plan["storybook_ids"] and table_exists(conn, "pair_messages"):
            cols = {c[1] for c in q(conn, "PRAGMA table_info(pair_messages)")}
            if "storybook_id" in cols:
                conn.execute(
                    f"DELETE FROM pair_messages WHERE storybook_id IN ({placeholders(len(plan['storybook_ids']))})",
                    tuple(plan["storybook_ids"]),
                )

        # 3) 存档子表
        for table, col in SAVE_CHILD_TABLES:
            if plan["save_ids"] and table_exists(conn, table):
                conn.execute(
                    f'DELETE FROM "{table}" WHERE {col} IN ({placeholders(len(plan["save_ids"]))})',
                    tuple(plan["save_ids"]),
                )

        # 4) pair_threads
        if plan["storybook_ids"] and table_exists(conn, "pair_threads"):
            conn.execute(
                f"DELETE FROM pair_threads WHERE storybook_id IN ({placeholders(len(plan['storybook_ids']))})",
                tuple(plan["storybook_ids"]),
            )

        # 5) saves -> storybooks -> users
        if plan["save_ids"]:
            conn.execute(
                f"DELETE FROM saves WHERE id IN ({placeholders(len(plan['save_ids']))})",
                tuple(plan["save_ids"]),
            )
        if plan["storybook_ids"]:
            conn.execute(
                f"DELETE FROM storybooks WHERE id IN ({placeholders(len(plan['storybook_ids']))})",
                tuple(plan["storybook_ids"]),
            )
        if plan["user_ids"]:
            conn.execute(
                f"DELETE FROM users WHERE id IN ({placeholders(len(plan['user_ids']))})",
                tuple(plan["user_ids"]),
            )


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="清理 octopus.db 中的端到端验证残留（默认 dry-run）",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="真正执行删除（执行前自动备份到 .scratch/）",
    )
    parser.add_argument(
        "--db",
        default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "octopus.db"),
        help="数据库路径（默认 仓库根/octopus.db）",
    )
    parser.add_argument(
        "--backup-dir",
        default=None,
        help="备份目录（默认 仓库根/.scratch）",
    )
    args = parser.parse_args(argv)

    db_path = os.path.abspath(args.db)
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    backup_dir = os.path.abspath(args.backup_dir) if args.backup_dir else os.path.join(repo_root, DEFAULT_BACKUP_DIR)

    if not os.path.exists(db_path):
        print(f"错误: 数据库不存在: {db_path}", file=sys.stderr)
        return 1

    print(f"数据库: {db_path}")
    print(f"模式:   {'APPLY（会写库）' if args.apply else 'DRY-RUN（只读）'}")

    conn = sqlite3.connect(db_path)
    try:
        required = ["users", "storybooks", "saves", "commands", "sessions"]
        missing = [t for t in required if not table_exists(conn, t)]
        if missing:
            print(f"错误: 表结构不符，缺少表: {missing}", file=sys.stderr)
            return 1

        before = print_stats(conn, "清理前统计")

        plan = build_plan(conn)
        print_plan(plan, args.apply)

        violations = check_protected(conn, plan) + check_protected_sessions(conn, plan)
        if violations:
            print_header("拒绝执行：受保护对象将被误删")
            for v in violations:
                print(f"  !! {v}")
            print("\n未做任何修改。")
            return 2

        has_work = bool(
            plan["user_ids"] or plan["storybook_ids"] or plan["save_ids"] or plan["thread_ids"]
            or any(c["count"] for c in plan["children"])
        )

        if not has_work:
            print_header("无残留可清理")
            print("\n没有匹配「验证残留」模式的数据，未做任何修改。")
            print("\n结论: 幂等检查通过（exit 0）")
            return 0

        if not args.apply:
            print_header("DRY-RUN 结束")
            print("\n未写库。加 --apply 才会真正执行（执行前会自动备份）。")
            return 0

        # ---- 真跑 ----
        print_header("备份")
        dest, size = make_backup(conn, db_path, backup_dir)
        print(f"  备份路径: {dest}")
        print(f"  字节数:   {size}")

        apply_plan(conn, plan)

        # 校验
        after_plan = build_plan(conn)
        residual_left = bool(
            after_plan["user_ids"] or after_plan["storybook_ids"] or after_plan["save_ids"]
            or after_plan["thread_ids"] or any(c["count"] for c in after_plan["children"])
        )

        protected_ok = True
        for uid in PROTECTED_USER_IDS:
            if not conn.execute("SELECT 1 FROM users WHERE id=?", (uid,)).fetchone():
                protected_ok = False
        for sid in PROTECTED_STORYBOOK_IDS:
            if not conn.execute("SELECT 1 FROM storybooks WHERE id=?", (sid,)).fetchone():
                protected_ok = False
        for sid in PROTECTED_SAVE_IDS:
            if not conn.execute("SELECT 1 FROM saves WHERE id=?", (sid,)).fetchone():
                protected_ok = False

        after = print_stats(conn, "清理后统计")

        print_header("前后对比")
        for t in before:
            d = after[t] - before[t]
            print(f"  {t:<20} {before[t]:>7} -> {after[t]:>7}  ({d:+d})")

        print_header("保护对象校验")
        print(f"  账户 {sorted(PROTECTED_USER_IDS)} 仍存在: {protected_ok}")
        print(f"  故事书 {sorted(PROTECTED_STORYBOOK_IDS)} 仍存在: "
              f"{all(conn.execute('SELECT 1 FROM storybooks WHERE id=?', (s,)).fetchone() for s in PROTECTED_STORYBOOK_IDS)}")
        print(f"  存档 {sorted(PROTECTED_SAVE_IDS)} 仍存在: "
              f"{all(conn.execute('SELECT 1 FROM saves WHERE id=?', (s,)).fetchone() for s in PROTECTED_SAVE_IDS)}")

        if residual_left or not protected_ok:
            print("\n错误: 清理后仍检测到残留或保护对象缺失。", file=sys.stderr)
            return 1

        print("\n完成: 验证残留已清理，保护对象完好。")
        return 0
    finally:
        conn.close()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RefusedError as e:
        print(f"拒绝执行: {e}", file=sys.stderr)
        sys.exit(2)
