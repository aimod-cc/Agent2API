#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Agent2API 请求日志分析（skill: request-log-analysis）的伴随脚本。

只读分析统一库 {config_dir}/agent2api.db（安装版 / 服务器部署同一格式）：

  overview   总览：成功率 / 状态码分布 / 按家 / 按模型 / 重试率 / 客户端分布
  failures   失败聚类：按「家 + 状态码 + 归一化错误文案」分组并自动分桶定责
  detail     单条深挖：明细 + 重试链(attempt_details) + 选路快照(route) +
             邻近应用日志 + 调试报文(debug_traffic / request_raw)
  logs       应用日志（logs 表）

成功口径与后端 `RequestEntry::is_success` 逐字一致：
status BETWEEN 200 AND 299 AND error IS NULL，其余一律算失败（status=0 =
没发出去就失败）。error 里「上游返回 X」的 X 是上游侧状态码，status 列是
回给客户端的状态码，两者可以不同。

零第三方依赖（Python 3.8+ 标准库）。默认先把库快照到临时目录再查
（SQLite backup API，热库也能拿到一致快照），绝不写原库。
"""

import argparse
import json
import os
import re
import shutil
import sqlite3
import sys
import tempfile
from datetime import datetime
from pathlib import Path

DB_NAME = "agent2api.db"

# 本次运行创建的临时快照目录：退出时（含异常 / Ctrl-C）由 main 统一删掉。
# 库是 GB 级的，一份快照就等于整库大小；早期版本把快照留在 %TEMP% 里不清理，
# 反复分析几次就能把系统盘吃满（实测残留过 4.7GB）。因此这里必须「谁创建、
# 谁负责删」，而不是把「完事可删」留给用户记着。
_SNAPSHOT_DIRS = []

# 「客户端断开」类错误的识别片段（status 常为 200，error 非空 —— 不是上游的锅）
CLIENT_ABORT_MARKERS = (
    "客户端在响应完成前断开连接",
    "响应流未完整下发",
    "客户端中断",
    "客户端断开",
)

LOG_LEVELS = {"debug": 10, "info": 20, "warn": 30, "warning": 30, "error": 40}

BUCKET_LABELS = {
    "client_abort": "客户端断开",
    "no_account": "无账号可转发",
    "model_missing": "模型/路由不存在",
    "parse": "解析/翻译层",
    "quota": "限额/轮换",
    "auth": "凭证/权限",
    "bad_request": "请求体形态被上游拒",
    "timeout": "超时",
    "upstream_5xx": "上游故障",
    "other": "其它",
}

# Windows 控制台默认 GBK：强制 UTF-8，坏字节替换而不是报错
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


# ────────────────────────── 基础工具 ──────────────────────────

def fmt_ms(ms):
    """毫秒时间戳 → 本地时间字符串。"""
    if ms is None:
        return "-"
    return datetime.fromtimestamp(ms / 1000).strftime("%m-%d %H:%M:%S")


def dur_str(ms):
    if ms is None:
        return "-"
    if ms >= 60000:
        return f"{ms / 60000:.1f}m"
    if ms >= 1000:
        return f"{ms / 1000:.1f}s"
    return f"{ms}ms"


def parse_local(s, end_of_day=False):
    d = datetime.strptime(s, "%Y-%m-%d %H:%M")
    if end_of_day:
        d = d.replace(minute=59, hour=23)
    return int(d.timestamp() * 1000)


def pct(part, whole):
    if not whole:
        return "0%"
    return f"{part * 100.0 / whole:.1f}%"


def percentile(sorted_values, p):
    if not sorted_values:
        return None
    idx = min(len(sorted_values) - 1, max(0, round(p / 100.0 * (len(sorted_values) - 1))))
    return sorted_values[idx]


_HEX_RE = re.compile(r"[0-9a-fA-F]{8,}")
_TS_RE = re.compile(r"\d{13}")
_DATE_RE = re.compile(r"\d{4}-\d{1,2}-\d{1,2}[ T]\d{1,2}:\d{2}(:\d{2})?")
_NUM_RE = re.compile(r"\d+")
_WS_RE = re.compile(r"\s+")


def normalize_error(text, maxlen=150):
    """错误文案归一化：抹掉 request_id / 时间戳 / 数字等动态值，供聚类。

    原始文案保留在分组样例里；分组键用归一化后的形态，否则
    「usage will reset at 2026-10-22 17:11:47」这类每条都不同的错误
    永远聚不到一起。
    """
    s = text or ""
    s = _TS_RE.sub("<ts>", s)
    s = _DATE_RE.sub("<date>", s)
    s = _HEX_RE.sub("<hex>", s)
    s = _NUM_RE.sub("#", s)
    s = _WS_RE.sub(" ", s).strip()
    return s[:maxlen]


# ────────────────────────── 分桶定责 ──────────────────────────

def bucket_of(status, error):
    e = error or ""
    low = e.lower()
    if any(m in e for m in CLIENT_ABORT_MARKERS):
        return "client_abort"
    if "所有账号均已禁用" in e or "无账号可转发" in e or "无可用账号" in e:
        return "no_account"
    if status == 404 or "模型不存在" in e or "is not allowed" in low or "service info not found" in low:
        return "model_missing"
    if "parse response failed" in low or "parse message failed" in low or "unsupported content type" in low:
        return "parse"
    if status == 429 or "too many requests" in low or "usage exceeds" in low or "频率限制" in e:
        return "quota"
    if status in (401, 402, 403):
        return "auth"
    if status == 400:
        return "bad_request"
    if status in (408, 504) or "超时" in e or "timed out" in low or "timeout" in low:
        return "timeout"
    if status >= 500:
        return "upstream_5xx"
    return "other"


# ────────────────────────── 定位与快照 ──────────────────────────

def resolve_db(explicit):
    """库路径：--db > AGENT2API_PROXY_HOME > WORKBUDDY_PROXY_HOME > ~/.agent2api。

    与服务端 paths::config_dir() 的解析顺序一致（环境变量整体覆盖）。
    """
    if explicit:
        p = Path(explicit).expanduser()
        if p.is_dir():
            p = p / DB_NAME
        return p
    for var in ("AGENT2API_PROXY_HOME", "WORKBUDDY_PROXY_HOME"):
        val = os.environ.get(var, "").strip()
        if val:
            return Path(val) / DB_NAME
    home = os.environ.get("USERPROFILE") or os.environ.get("HOME") or "."
    return Path(home) / ".agent2api" / DB_NAME


def snapshot_db(db_path):
    """把库快照到临时目录，返回 (快照路径, 方式说明)。

    方式一 SQLite backup API：网关在跑（WAL 热库）也能拿到一致快照；
    方式二 三件套整拷（db + -wal + -shm），打开副本时自动恢复 WAL。
    只拷 .db 不拷 -wal 会丢最近几分钟的数据 —— 这是常见坑。
    """
    if not db_path.exists():
        raise SystemExit(
            f"找不到库文件：{db_path}\n"
            f"  可用 --db 显式指定（传目录会自动拼 {DB_NAME}）；\n"
            f"  服务器/Docker 部署把 agent2api.db（连同 -wal/-shm）拷回本机后再指进来。"
        )
    tmp = Path(tempfile.mkdtemp(prefix="a2a-logs-"))
    _SNAPSHOT_DIRS.append(tmp)
    snap = tmp / DB_NAME
    try:
        uri = "file:///" + db_path.resolve().as_posix() + "?mode=ro"
        src = sqlite3.connect(uri, uri=True)
        try:
            dst = sqlite3.connect(str(snap))
            try:
                src.backup(dst)
            finally:
                dst.close()
        finally:
            src.close()
        return snap, "backup-api（热库一致快照）"
    except Exception:
        pass
    copied = []
    for suffix in ("", "-wal", "-shm"):
        src = Path(str(db_path) + suffix)
        if src.exists():
            shutil.copy2(src, tmp / (DB_NAME + suffix))
            copied.append(src.name)
    if not copied:
        raise SystemExit(f"库文件读取失败且无可拷贝的文件：{db_path}")
    return snap, "file-copy（" + ", ".join(copied) + "）"


def open_db(args):
    db_path = resolve_db(args.db)
    if args.no_copy:
        uri = "file:///" + db_path.resolve().as_posix() + "?mode=ro"
        conn = sqlite3.connect(uri, uri=True)
        conn.row_factory = sqlite3.Row
        return conn, db_path, "直连原库（只读，未快照）"
    snap, how = snapshot_db(db_path)
    conn = sqlite3.connect(str(snap))
    conn.row_factory = sqlite3.Row
    return conn, snap, f"快照 {how}: {snap}"


def window_ms(args):
    """时间窗：--since/--until 优先，缺省 = 最近 --hours 小时。"""
    if args.since:
        start = parse_local(args.since)
    else:
        start = None
    if args.until:
        end = parse_local(args.until, end_of_day=True)
    else:
        end = None
    now = int(datetime.now().timestamp() * 1000)
    if start is None and end is None:
        end, start = now, now - int(args.hours * 3600 * 1000)
    elif start is None:
        start = end - int(args.hours * 3600 * 1000)
    elif end is None:
        end = now
    return start, end


def test_clause(args):
    return "" if args.include_test else " AND is_test = 0"


# ────────────────────────── overview ──────────────────────────

def cmd_overview(conn, args, start, end):
    where = "ts >= ? AND ts <= ?" + test_clause(args)
    params = [start, end]
    total = conn.execute(f"SELECT COUNT(*) FROM requests WHERE {where}", params).fetchone()[0]
    ok = conn.execute(
        f"SELECT COUNT(*) FROM requests WHERE {where} AND status BETWEEN 200 AND 299 AND error IS NULL",
        params,
    ).fetchone()[0]
    fail = conn.execute(
        f"SELECT COUNT(*) FROM requests WHERE {where} AND (status < 200 OR status > 299 OR error IS NOT NULL)",
        params,
    ).fetchone()[0]
    running = conn.execute(
        f"SELECT COUNT(*) FROM requests WHERE {where} AND phase <> ''", params
    ).fetchone()[0]
    status_rows = conn.execute(
        f"SELECT status, COUNT(*) c FROM requests WHERE {where} GROUP BY status ORDER BY c DESC",
        params,
    ).fetchall()

    data = {
        "window": [fmt_ms(start), fmt_ms(end)],
        "total": total, "success": ok, "failed": fail, "running": running,
        "successRate": pct(ok, total),
        "status": {str(r["status"]): r["c"] for r in status_rows},
    }
    print(f"窗口 {fmt_ms(start)} ~ {fmt_ms(end)}   总数 {total}   成功 {ok} ({pct(ok, total)})"
          f"   失败 {fail} ({pct(fail, total)})   在途 {running}"
          f"{'   [含测试流量]' if args.include_test else '   [已排除测试流量]'}")
    print("状态码:", "  ".join(f"{r['status']}×{r['c']}" for r in status_rows) or "无")

    print("\n── 按家（provider）──")
    print(f"{'provider':22s} {'total':>7s} {'fail':>6s} {'失败率':>7s} {'p50':>8s} {'p95':>8s}")
    prov_rows = conn.execute(
        f"SELECT provider, COUNT(*) total, "
        f"SUM(CASE WHEN status < 200 OR status > 299 OR error IS NOT NULL THEN 1 ELSE 0 END) fail "
        f"FROM requests WHERE {where} GROUP BY provider ORDER BY total DESC",
        params,
    ).fetchall()
    provider_data = []
    for r in prov_rows:
        d = conn.execute(
            f"SELECT duration_ms FROM requests WHERE {where} AND provider = ? ORDER BY duration_ms",
            params + [r["provider"]],
        ).fetchall()
        durs = sorted(x[0] or 0 for x in d)
        p50, p95 = percentile(durs, 50), percentile(durs, 95)
        name = r["provider"] or "(未选路)"
        print(f"{name:22s} {r['total']:>7d} {r['fail'] or 0:>6d} {pct(r['fail'] or 0, r['total']):>7s}"
              f" {dur_str(p50):>8s} {dur_str(p95):>8s}")
        provider_data.append({"provider": r["provider"], "total": r["total"],
                              "failed": r["fail"] or 0, "p50ms": p50, "p95ms": p95})
    data["providers"] = provider_data

    print("\n── 失败 Top 模型 ──")
    model_rows = conn.execute(
        f"SELECT COALESCE(NULLIF(upstream_model, ''), model) m, COUNT(*) c FROM requests "
        f"WHERE {where} AND (status < 200 OR status > 299 OR error IS NOT NULL) "
        f"GROUP BY m ORDER BY c DESC LIMIT 8",
        params,
    ).fetchall()
    for r in model_rows:
        print(f"  {r['m'] or '(空)'} × {r['c']}")

    print("\n── 重试 ──")
    retry = conn.execute(
        f"SELECT COUNT(*) c, MAX(attempts) mx, AVG(attempts * 1.0) avg FROM requests "
        f"WHERE {where} AND attempts > 1",
        params,
    ).fetchone()
    print(f"attempts>1 共 {retry['c']} 条（{pct(retry['c'], total)}），最多 {retry['mx']} 次，"
          f"平均 {retry['avg'] or 0:.2f} 次")

    print("\n── 按客户端 ──")
    client_rows = conn.execute(
        f"SELECT client, COUNT(*) total, "
        f"SUM(CASE WHEN status < 200 OR status > 299 OR error IS NOT NULL THEN 1 ELSE 0 END) fail "
        f"FROM requests WHERE {where} GROUP BY client ORDER BY total DESC",
        params,
    ).fetchall()
    for r in client_rows:
        name = r["client"] or "(未识别)"
        print(f"  {name:18s} total {r['total']:>6d}   fail {r['fail'] or 0:>6d} ({pct(r['fail'] or 0, r['total'])})")

    if args.json:
        print(json.dumps(data, ensure_ascii=False, indent=2))


# ────────────────────────── failures ──────────────────────────

def cmd_failures(conn, args, start, end):
    where = ("ts >= ? AND ts <= ?" + test_clause(args) +
             " AND (status < 200 OR status > 299 OR error IS NOT NULL)")
    params = [start, end]
    total_all = conn.execute(
        f"SELECT COUNT(*) FROM requests WHERE ts >= ? AND ts <= ?" + test_clause(args),
        params,
    ).fetchone()[0]
    rows = conn.execute(
        f"SELECT ts, model, upstream_model, provider, account_name, account_id, status, attempts, "
        f"error, client FROM requests WHERE {where} ORDER BY ts DESC",
        params,
    ).fetchall()

    print(f"失败 {len(rows)} 条 / 总 {total_all} 条（{pct(len(rows), total_all)}），窗口 {fmt_ms(start)} ~ {fmt_ms(end)}")
    if not rows:
        print("窗口内没有失败请求。")
        return

    groups = {}
    for r in rows:
        key = (r["provider"] or "(未选路)", r["status"], normalize_error(r["error"]))
        g = groups.setdefault(key, {
            "count": 0, "first": r["ts"], "last": r["ts"], "accounts": set(),
            "models": set(), "clients": set(), "sample": r["error"] or "",
            "bucket": bucket_of(r["status"], r["error"]),
        })
        g["count"] += 1
        g["first"] = min(g["first"], r["ts"])
        g["last"] = max(g["last"], r["ts"])
        if r["account_name"]:
            g["accounts"].add(r["account_name"])
        g["models"].add(r["upstream_model"] or r["model"])
        if r["client"]:
            g["clients"].add(r["client"])

    ordered = sorted(groups.items(), key=lambda kv: -kv[1]["count"])
    print(f"\n── 失败聚类 Top {args.limit}（键 = 家 + 状态码 + 归一化文案）──")
    for i, ((provider, status, norm), g) in enumerate(ordered[: args.limit], 1):
        print(f"\n#{i} {g['count']}条（占失败 {pct(g['count'], len(rows))}）"
              f" [{BUCKET_LABELS[g['bucket']]}] provider={provider} status={status}"
              f"  账号{len(g['accounts'])}个  模型{sorted(g['models'])}"
              f"  {fmt_ms(g['first'])} ~ {fmt_ms(g['last'])}")
        if g["clients"]:
            print(f"   客户端: {sorted(g['clients'])}")
        sample = (g["sample"] or "").replace("\n", " ")[:220]
        print(f"   样例: {sample}")
        print(f"   归一键: {norm}")

    buckets = {}
    for _, g in groups.items():
        buckets[g["bucket"]] = buckets.get(g["bucket"], 0) + g["count"]
    print("\n── 分桶汇总（定责见 SKILL.md 第 3 节映射表）──")
    for b, c in sorted(buckets.items(), key=lambda kv: -kv[1]):
        print(f"  {BUCKET_LABELS[b]:12s} {c:>6d} ({pct(c, len(rows))})")

    if args.json:
        data = [{
            "provider": p, "status": s, "normalized": n, "bucket": g["bucket"],
            "count": g["count"], "first": g["first"], "last": g["last"],
            "accounts": sorted(g["accounts"]), "models": sorted(g["models"]),
            "sample": g["sample"],
        } for (p, s, n), g in ordered]
        print(json.dumps(data, ensure_ascii=False, indent=2))


# ────────────────────────── detail ──────────────────────────

def print_attempt_details(entry):
    try:
        details = json.loads(entry["attempt_details"] or "[]")
    except (ValueError, TypeError):
        details = []
    if not details:
        print("  attempt_details: (空)")
        return
    print(f"  重试链（{len(details)} 次尝试）：")
    for i, d in enumerate(details, 1):
        retries = d.get("retries") or []
        print(f"   [{i}] provider={d.get('provider') or '-'}  account={d.get('account') or '-'}"
              f"  status={d.get('status')}")
        if d.get("error"):
            print(f"       error: {str(d['error'])[:200]}")
        for rr in retries:
            print(f"       重试: {rr.get('reason') or ''} status={rr.get('status')}"
                  f" delay={rr.get('delayMs')}ms")
        if d.get("notice"):
            print(f"       notice: {d['notice']}")


def print_route(entry):
    raw = entry["route"]
    if not raw:
        print("  route: (空)")
        return
    try:
        r = json.loads(raw)
    except (ValueError, TypeError):
        print(f"  route(原文): {str(raw)[:300]}")
        return
    head = ", ".join(f"{k}={r[k]}" for k in ("strategy", "sticky", "stickyHit") if k in r)
    if r.get("picked"):
        head += f", picked={r['picked']}"
    print(f"  选路快照: {head or '(无字段)'}")
    for kind in ("ordered", "blocked"):
        for e in r.get(kind) or []:
            print(f"   {kind:8s} {e.get('name') or e.get('id')}  provider={e.get('provider')}"
                  f"  state={e.get('state')}  理由: {e.get('reason') or ''}")


def cmd_detail(conn, args, start, end):
    if args.row_id is not None:
        entries = conn.execute(
            "SELECT * FROM requests WHERE row_id = ?", (args.row_id,)
        ).fetchall()
    elif args.id:
        entries = conn.execute(
            "SELECT * FROM requests WHERE id = ? ORDER BY ts DESC, row_id DESC", (args.id,)
        ).fetchall()
    else:
        raise SystemExit("detail 需要 --id <请求id> 或 --row-id <row_id>")
    if not entries:
        raise SystemExit("没有找到对应明细（同 id 可有多行，本命令按 id 全部列出）。")
    print(f"命中 {len(entries)} 行（同 id 多行 = 重试/去重排队产生的重复记账，看最新一行即可）")
    for entry in entries:
        ts = entry["ts"]
        print(f"\nrow_id={entry['row_id']}  id={entry['id']}")
        print(f"  时间 {fmt_ms(ts)}.{ts % 1000:03d}   model={entry['model']}"
              f"  client={entry['client'] or '-'}")
        print(f"  provider={entry['provider'] or '(未选路)'}  account={entry['account_name'] or '-'}"
              f"  status={entry['status']}  attempts={entry['attempts']}"
              f"  phase={entry['phase'] or '-'}")
        print(f"  耗时 {dur_str(entry['duration_ms'])}"
              f"  首帧 {dur_str(entry['first_response_ms']) if entry['first_response_ms'] is not None else '-'}"
              f"  tokens prompt={entry['prompt_tokens']} completion={entry['completion_tokens']}"
              f" total={entry['total_tokens']} cacheRead={entry['cache_read_tokens']}")
        print(f"  模型: client_model={entry['client_model'] or '-'}"
              f"  upstream_model={entry['upstream_model'] or '-'}"
              f"  reasoning: client={entry['client_reasoning'] or '-'}"
              f" upstream={entry['upstream_reasoning'] or '-'}")
        if entry["error"]:
            print(f"  error: {entry['error']}")
        if entry["sensitive_hits"] and entry["sensitive_hits"] != "[]":
            print(f"  sensitive_hits: {entry['sensitive_hits'][:200]}")
        print_attempt_details(entry)
        print_route(entry)

    rid = entries[0]["id"]
    if rid:
        ts = entries[0]["ts"]
        span = args.log_window * 1000
        logs = conn.execute(
            "SELECT ts, level, category, message, data FROM logs "
            "WHERE ts >= ? AND ts <= ? ORDER BY ts LIMIT 40",
            (ts - span, ts + span),
        ).fetchall()
        if logs:
            print(f"\n── 邻近应用日志（±{args.log_window}s，{len(logs)} 条）──")
            for l in logs:
                msg = (l["message"] or "").replace("\n", " ")[:200]
                print(f"  {fmt_ms(l['ts'])} {l['level']:5s} [{l['category']}] {msg}")
                if args.body and l["data"]:
                    print(f"      data: {str(l['data'])[:300]}")

        dt = conn.execute("SELECT * FROM debug_traffic WHERE id = ?", (rid,)).fetchone()
        if dt:
            print(f"\n── 调试报文 debug_traffic（上游侧，头部已脱敏）──")
            print(f"  url={dt['url']}")
            print(f"  provider={dt['provider']}  status={dt['status']}  truncated={dt['truncated']}"
                  f"  size={dt['size']}B")
            if args.body:
                print(f"  request_headers keys: {list((json.loads(dt['request_headers'] or '{}')).keys())}")
                print(f"  request_body 头 1200 字:\n{(dt['request_body'] or '')[:1200]}")
                if dt["response_body"]:
                    print(f"  response_body 头 1200 字:\n{dt['response_body'][:1200]}")
        else:
            print("\n  debug_traffic 无此 id（调试模式没开 —— 面板设置页打开后复现一次再看）")

        rr = conn.execute("SELECT * FROM request_raw WHERE id = ?", (rid,)).fetchone()
        if rr:
            print(f"  request_raw（下游侧）: req {len(rr['request_body'] or '')}B"
                  f" / resp {len(rr['response_body'] or '')}B")
            if args.body:
                print(f"  request_body 头 800 字:\n{(rr['request_body'] or '')[:800]}")
                print(f"  response_body 尾 800 字:\n{(rr['response_body'] or '')[-800:]}")
        else:
            print("  request_raw 无此 id")


# ────────────────────────── logs ──────────────────────────

def cmd_logs(conn, args, start, end):
    where = "ts >= ? AND ts <= ?"
    params = [start, end]
    if args.level:
        min_lv = LOG_LEVELS.get(args.level.lower(), 20)
        conds = " OR ".join(
            f"level = '{lv}'" for lv, v in LOG_LEVELS.items() if v >= min_lv and lv != "warning"
        )
        where += f" AND ({conds})"
    if args.category:
        where += " AND category = ?"
        params.append(args.category)
    rows = conn.execute(
        f"SELECT ts, level, category, message, data FROM logs WHERE {where} "
        f"ORDER BY ts DESC LIMIT {args.limit}",
        params,
    ).fetchall()
    print(f"日志 {len(rows)} 条（level≥{args.level or 'info'}"
          f"{' category=' + args.category if args.category else ''}），窗口 {fmt_ms(start)} ~ {fmt_ms(end)}")
    for r in rows:
        msg = (r["message"] or "") if args.full else (r["message"] or "").replace("\n", " ")[:200]
        print(f"{fmt_ms(r['ts'])} {r['level']:5s} [{r['category']}] {msg}")
    if args.json:
        print(json.dumps([dict(r) for r in rows], ensure_ascii=False, indent=2))


# ────────────────────────── 入口 ──────────────────────────

def build_parser():
    # 公共选项只挂在子解析器上 —— 调用形如
    #   analyze.py overview --hours 6 --no-copy
    # （选项一律放在子命令后面；挂到主解析器上会被子解析器的默认值覆盖，
    # 出现「前面的 --no-copy 静默失效」这类坑）
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--db", help="库路径（文件或目录）；默认自动定位 %%USERPROFILE%%\\.agent2api")
    common.add_argument("--hours", type=float, default=24, help="时间窗（小时），默认 24")
    common.add_argument("--since", help="起始本地时间 YYYY-MM-DD HH:MM")
    common.add_argument("--until", help="截止本地时间 YYYY-MM-DD HH:MM")
    common.add_argument("--limit", type=int, default=50, help="条数上限，默认 50")
    common.add_argument("--json", action="store_true", help="额外输出 JSON")
    common.add_argument("--include-test", action="store_true", help="包含模型测试流量（is_test=1）")
    common.add_argument("--no-copy", action="store_true", help="直连原库（只读），不快照")

    p = argparse.ArgumentParser(
        prog="analyze.py",
        description="Agent2API 请求日志只读分析（skill: request-log-analysis）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("overview", parents=[common], help="总览")
    sub.add_parser("failures", parents=[common], help="失败聚类与分桶")
    dp = sub.add_parser("detail", parents=[common], help="单条深挖")
    dp.add_argument("--id", help="请求关联 id（requests.id）")
    dp.add_argument("--row-id", type=int, help="requests.row_id")
    dp.add_argument("--body", action="store_true", help="同时打印报文/日志正文（注意含用户内容）")
    dp.add_argument("--log-window", type=int, default=90, help="邻近日志窗口秒数，默认 90")
    lp = sub.add_parser("logs", parents=[common], help="应用日志")
    lp.add_argument("--level", default="info", help="最低级别 debug/info/warn/error")
    lp.add_argument("--category", help="按分类过滤（server/upstream/model…）")
    lp.add_argument("--full", action="store_true", help="不截断消息")
    return p


def main():
    args = build_parser().parse_args()
    try:
        # open_db 也可能在建好快照目录之后失败（拷到一半磁盘满 / 库被独占），
        # 所以它同样要落在 try 里，否则那次失败会留下整库大小的垃圾。
        conn, used_path, how = open_db(args)
        try:
            start, end = window_ms(args)
            print(f"# 库: {how}")
            if args.cmd == "overview":
                cmd_overview(conn, args, start, end)
            elif args.cmd == "failures":
                cmd_failures(conn, args, start, end)
            elif args.cmd == "detail":
                cmd_detail(conn, args, start, end)
            elif args.cmd == "logs":
                cmd_logs(conn, args, start, end)
        finally:
            conn.close()
    finally:
        cleanup_snapshots()


def cleanup_snapshots():
    """删掉本次运行创建的临时快照目录（尽力而为）。

    只删 `_SNAPSHOT_DIRS` 里自己 `mkdtemp` 出来的路径 —— 绝不按通配符扫
    `%TEMP%\\a2a-logs-*`：用户可能正用 `--db` 复用上一次的快照追问，
    那种目录不归本次运行管。删不掉（文件仍被占用）只提示路径，不报错。
    """
    while _SNAPSHOT_DIRS:
        path = _SNAPSHOT_DIRS.pop()
        try:
            shutil.rmtree(path)
        except OSError as error:
            print(f"# 提示：临时快照未能删除（{path}）：{error}", file=sys.stderr)


if __name__ == "__main__":
    main()
