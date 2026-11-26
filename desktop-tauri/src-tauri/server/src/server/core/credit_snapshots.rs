//! 积分余额快照：把余额查询结果里的每个积分钱包记成一行时间序列。
//!
//! 为什么要存：上游按积分计费（CodeArts / WorkBuddy），但请求响应只回 token
//! 不回积分（`requests.credit` 只有上游主动上报才有），宿主侧也没有积分单价。
//! 唯一可靠的来源是**余额差分**：相邻两次余额查询之间某账号 `creditUsed` 的
//! 增量，除以这期间它处理的 token 数，就是实际积分单价。要做到这一点，
//! 余额就不能只留「最新一行」—— 所以每次查询成功时追加一行快照。
//!
//! 刻意**不走 schema 版本链**（`db/schema.rs` 的 `apply_version`）：版本号是
//! 上游的序列，分支自己占一个号会在合并时与上游撞（库已是 37、上游代码也
//! 是 37，上游的 v37 迁移就被静默跳过；或库比上游新，二进制拒绝启动）。
//! 表用 `CREATE TABLE IF NOT EXISTS` 幂等自建，对上游的 diff 最小。

use serde_json::Value;

/// 表名。建表 / 写入 / 裁剪共用。
pub const TABLE: &str = "account_credit_snapshots";

/// 快照保留天数。149 个账号 × 4 个钱包 × 每分钟一次 ≈ 86 万行/天（约 40 MB）；
/// 保留 90 天足够画出余额趋势和推导单价，再多就只是占盘。
pub const RETENTION_DAYS: i64 = 90;

/// 一条积分钱包快照（数值全是积分单位）。
#[derive(Debug, Clone, PartialEq)]
pub struct CreditSnapshot {
    pub wallet_type: String,
    pub credit_used: f64,
    pub credit_remaining: f64,
    pub credit_total: f64,
}

/// 从余额查询结果里提取每个积分钱包的快照行。
///
/// 认两种形状：CodeArts 的 `wallets[]`（每个钱包自带 `creditUsed / creditRemaining /
/// creditTotal`）；`credits[]` 是按积分包的列表，它多出有效期与面额，这里不取
/// ——同一个账号会重复计（一个包一行，加起来才是账号余额），差分口径会乱。
/// 钱包没有 `creditUsed`（只给了剩余量）的跳过：没有已用量就做不了差分。
/// 数值不是有限数的（NaN / 缺失）整条跳过，不写 0 进去冒充「用完了」。
pub fn extract_wallets(usage: &Value) -> Vec<CreditSnapshot> {
    let mut out = Vec::new();
    let Some(wallets) = usage.get("wallets").and_then(Value::as_array) else {
        return out;
    };
    for wallet in wallets {
        let Some(wallet) = wallet.as_object() else { continue };
        let Some(credit_used) = finite(wallet.get("creditUsed")) else { continue };
        // 已用量在就认这条；剩余 / 总量缺失时记 0.0 而不是跳过 —— 差分只用已用量，
        // 不能因为上游少给一个字段就丢掉整条可用的样本。
        let credit_remaining = finite(wallet.get("creditRemaining")).unwrap_or(0.0);
        let credit_total = finite(wallet.get("creditTotal")).unwrap_or(0.0);
        let wallet_type = wallet
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
            .to_string();
        out.push(CreditSnapshot {
            wallet_type,
            credit_used,
            credit_remaining,
            credit_total,
        });
    }
    out
}

fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|value| value.is_finite())
}

// ─── 落库 ──────────────────────────────────────────────────────

use std::sync::OnceLock;

use crate::server::db::Db;
use crate::server::logging;

/// 建表语句。幂等：已存在时什么都不做。
///
/// 主键含 `ts`：同一账号同一钱包每次查询一行。`(account_id, wallet_type, ts)` 的
/// 主键顺带覆盖「某账号某钱包按时间取相邻两条」的差分查询；单独的 `ts` 索引给
/// 裁剪用（`WHERE ts < ?`）。
const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS account_credit_snapshots (
    account_id      TEXT    NOT NULL,
    wallet_type     TEXT    NOT NULL,
    ts              INTEGER NOT NULL,
    credit_used     REAL    NOT NULL,
    credit_remaining REAL   NOT NULL,
    credit_total    REAL    NOT NULL,
    PRIMARY KEY (account_id, wallet_type, ts)
);
CREATE INDEX IF NOT EXISTS idx_account_credit_snapshots_ts ON account_credit_snapshots(ts);";

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 注入数据库句柄（与 `usage_records::install` 同一模式、同一个库）。
pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Option<&'static Db> {
    DB.get().and_then(Option::as_ref)
}

/// 记一次余额查询成功的快照：每个积分钱包一行。没有 `wallets[]`（或都缺已用量）
/// 时什么都不写——余额查询每分钟对 149 个账号各跑一次，没有钱包的账号每次都会
/// 落在这里，不能让它们去碰库。
///
/// 失败不向上冒泡：快照是统计用途，写不进去只该少一条样本，不该影响余额查询
/// 本身（那关系到选路）。
pub fn record(account_id: &str, usage: &Value, now_ms: i64) {
    let snapshots = extract_wallets(usage);
    if snapshots.is_empty() {
        return;
    }
    let Some(db) = database() else { return };
    let outcome = db.with(|conn| write_snapshots(conn, account_id, &snapshots, now_ms));
    if let Some(Err(error)) = outcome {
        logging::verbose("[Credits]", &format!("账号 {account_id} 积分快照写入失败：{error}"));
    }
}

/// 把一批快照写进库（建表 → 逐行写入 → 顺手裁剪）。拆出来是为了能用临时库直接测。
///
/// 建表每次都执行：`CREATE TABLE IF NOT EXISTS` 对已存在的表只是一次 schema 查找
/// （微秒级），比维护一个「表已就绪」的进程级标志简单，也不会在换库 / 测试里出错。
fn write_snapshots(
    conn: &rusqlite::Connection,
    account_id: &str,
    snapshots: &[CreditSnapshot],
    now_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    for snapshot in snapshots {
        conn.execute(
            "INSERT OR REPLACE INTO account_credit_snapshots
               (account_id, wallet_type, ts, credit_used, credit_remaining, credit_total)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                account_id,
                snapshot.wallet_type,
                now_ms,
                snapshot.credit_used,
                snapshot.credit_remaining,
                snapshot.credit_total,
            ],
        )?;
    }
    // 裁剪失败不算写入失败（只是多留一会儿旧数据），所以丢掉它的结果
    let _ = prune_before(conn, now_ms - RETENTION_DAYS * 86_400_000);
    Ok(())
}

/// 删掉 `cutoff` 之前的快照，返回删掉的行数。
fn prune_before(conn: &rusqlite::Connection, cutoff_ms: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM account_credit_snapshots WHERE ts < ?1", [cutoff_ms])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wallet(kind: &str, used: f64, remaining: f64, total: f64) -> Value {
        json!({"type": kind, "creditUsed": used, "creditRemaining": remaining, "creditTotal": total, "usedPercent": 0.4})
    }

    fn snap(kind: &str, used: f64, remaining: f64, total: f64) -> CreditSnapshot {
        CreditSnapshot {
            wallet_type: kind.to_string(),
            credit_used: used,
            credit_remaining: remaining,
            credit_total: total,
        }
    }

    #[test]
    fn extracts_one_row_per_wallet_in_order() {
        let usage = json!({
            "available": 3451.66,
            "wallets": [wallet("free", 100.5, 400.0, 500.0), wallet("paid", 7.0, 93.0, 100.0)],
        });
        assert_eq!(
            extract_wallets(&usage),
            vec![snap("free", 100.5, 400.0, 500.0), snap("paid", 7.0, 93.0, 100.0)],
        );
    }

    #[test]
    fn returns_empty_when_there_is_no_wallets_array() {
        // WorkBuddy 的形状（credits[] 按积分包）与完全空的结果都不产快照
        assert!(extract_wallets(&json!({"credits": [{"remain": 10.0, "total": 20.0}], "totalLeft": 10})).is_empty());
        assert!(extract_wallets(&json!({})).is_empty());
        assert!(extract_wallets(&json!({"wallets": "not-an-array"})).is_empty());
    }

    #[test]
    fn skips_wallets_without_a_credit_used_number() {
        let usage = json!({"wallets": [
            {"type": "a", "creditRemaining": 10.0},
            {"type": "b", "creditUsed": "12", "creditRemaining": 10.0},
            {"type": "c", "creditUsed": 12.0, "creditRemaining": 10.0},
        ]});
        assert_eq!(extract_wallets(&usage), vec![snap("c", 12.0, 10.0, 0.0)]);
    }

    #[test]
    fn keeps_a_wallet_that_only_reports_credit_used() {
        let usage = json!({"wallets": [{"type": "x", "creditUsed": 3.25}]});
        assert_eq!(extract_wallets(&usage), vec![snap("x", 3.25, 0.0, 0.0)]);
    }

    #[test]
    fn uses_a_default_type_when_the_wallet_type_is_missing_or_blank() {
        let usage = json!({"wallets": [
            {"creditUsed": 1.0},
            {"type": "   ", "creditUsed": 2.0},
            {"type": 42, "creditUsed": 3.0},
        ]});
        assert_eq!(
            extract_wallets(&usage),
            vec![snap("default", 1.0, 0.0, 0.0), snap("default", 2.0, 0.0, 0.0), snap("default", 3.0, 0.0, 0.0)],
        );
    }

    #[test]
    fn accepts_zero_used_but_never_non_finite_values() {
        // 0 已用是合法读数（新账号）；NaN / 缺失不是，不能写成 0 冒充真实数据
        let zero = json!({"wallets": [{"type": "z", "creditUsed": 0, "creditRemaining": 4700.0, "creditTotal": 4700.0}]});
        assert_eq!(extract_wallets(&zero), vec![snap("z", 0.0, 4700.0, 4700.0)]);
        assert!(extract_wallets(&json!({"wallets": [{"type": "n", "creditUsed": null}]})).is_empty());
        assert!(extract_wallets(&json!({"wallets": [{"type": "n", "creditUsed": "1.5"}]})).is_empty());
    }

    // ─── 落库 ────────────────────────────────────────────────────────

    fn rows(conn: &rusqlite::Connection) -> Vec<(String, i64, f64, f64, f64)> {
        let mut stmt = conn
            .prepare("SELECT wallet_type, ts, credit_used, credit_remaining, credit_total FROM account_credit_snapshots ORDER BY wallet_type, ts")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    fn table_exists(conn: &rusqlite::Connection) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [TABLE],
            |_| Ok(()),
        )
        .is_ok()
    }

    #[test]
    fn write_snapshots_creates_the_table_and_writes_one_row_per_wallet() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-write");
        let snapshots = extract_wallets(&json!({"wallets": [
            wallet("free", 100.5, 400.0, 500.0), wallet("paid", 7.0, 93.0, 100.0),
        ]}));
        db.with(|conn| {
            write_snapshots(conn, "acct-1", &snapshots, 1_000).unwrap();
            assert!(table_exists(conn), "第一次写入应当把表建出来");
            assert_eq!(
                rows(conn),
                vec![
                    ("free".to_string(), 1_000, 100.5, 400.0, 500.0),
                    ("paid".to_string(), 1_000, 7.0, 93.0, 100.0),
                ]
            );
        })
        .expect("库不可用");
    }

    #[test]
    fn write_snapshots_replaces_the_same_instant_and_appends_a_new_one() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-replace");
        let first = extract_wallets(&json!({"wallets": [wallet("free", 100.0, 400.0, 500.0)]}));
        let again = extract_wallets(&json!({"wallets": [wallet("free", 111.0, 389.0, 500.0)]}));
        let later = extract_wallets(&json!({"wallets": [wallet("free", 125.0, 375.0, 500.0)]}));
        db.with(|conn| {
            write_snapshots(conn, "acct-1", &first, 1_000).unwrap();
            // 同一毫秒重写是替换（主键含 ts）：余额查询重试不会产生重复行
            write_snapshots(conn, "acct-1", &again, 1_000).unwrap();
            write_snapshots(conn, "acct-1", &later, 61_000).unwrap();
            // 另一个账号互不影响
            write_snapshots(conn, "acct-2", &first, 1_000).unwrap();
            // 只取「已用量」做比较并排序：acct-1 与 acct-2 在同一毫秒各有一行，
            // 排序键（wallet_type, ts）相同，两条的先后顺序SQLite 不保证，不能按位置断言。
            let mut used: Vec<f64> = rows(conn).iter().map(|(_, _, used, _, _)| *used).collect();
            used.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(used, vec![100.0, 111.0, 125.0], "同一毫秒是替换不是追加，不同毫秒各一行");
            assert_eq!(rows(conn).len(), 3, "acct-1 两行 + acct-2 一行");
        })
        .expect("库不可用");
    }

    #[test]
    fn write_snapshots_prunes_rows_older_than_the_retention_window() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-prune");
        let snapshot = extract_wallets(&json!({"wallets": [wallet("free", 1.0, 1.0, 2.0)]}));
        let now = 10_000 * 86_400_000;
        db.with(|conn| {
            let too_old = now - (RETENTION_DAYS + 1) * 86_400_000;
            let kept = now - (RETENTION_DAYS - 1) * 86_400_000;
            write_snapshots(conn, "acct-1", &snapshot, too_old).unwrap();
            write_snapshots(conn, "acct-1", &snapshot, kept).unwrap();
            // 再写一次触发裁剪；只有超过保留期的行该被删
            write_snapshots(conn, "acct-1", &snapshot, now).unwrap();
            let stamps: Vec<i64> = rows(conn).iter().map(|(_, ts, _, _, _)| *ts).collect();
            assert_eq!(stamps, vec![kept, now], "保留期之外的快照应被裁掉");
        })
        .expect("库不可用");
    }

    #[test]
    fn write_snapshots_is_idempotent_across_repeated_calls() {
        // 建表每次都执行：重复调用不能报错（CREATE IF NOT EXISTS）
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-idempotent");
        let snapshot = extract_wallets(&json!({"wallets": [wallet("free", 1.0, 1.0, 2.0)]}));
        db.with(|conn| {
            for ts in [1_000, 2_000, 3_000] {
                write_snapshots(conn, "acct-1", &snapshot, ts).unwrap();
            }
            assert_eq!(rows(conn).len(), 3);
        })
        .expect("库不可用");
    }
}
