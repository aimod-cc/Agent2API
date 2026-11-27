//! 积分余额快照：把余额查询结果里的积分钱包记成时间序列，并按天汇总成积分报表。
//!
//! 为什么要存：上游按积分计费（CodeArts / WorkBuddy），但请求响应只回 token
//! 不回积分（`requests.credit` 只有上游主动上报才有），宿主侧也没有积分单价。
//! 唯一可靠的来源是**余额差分**：相邻两条快照之间某账号 `creditUsed` 的上升量
//! 之和，就是这段时间的积分消耗。要做到这一点，余额就不能只留「最新一行」。
//!
//! ── 两层：明细 + 按天汇总（与请求报表的 `requests` / `request_daily` 同一架构）──
//! * `account_credit_snapshots`：明细。**不是每次余额查询都记**。2026-11-27 线上实测：
//!   148 个账号每分钟各 4 行 = 76.7 万行 / 天，每行连同主键索引 309 字节 = 237 MB / 天，
//!   90 天就是 21 GB。规则是「变了就记，但两条之间至少隔 [`MIN_GAP_MS`]；没变每
//!   [`HEARTBEAT_MS`] 记一条」—— 每次上升迟早都有一条样本记下它，按天的合计不受
//!   影响，行数降到约十分之一。
//! * `account_credit_daily`：过了零点的日子各汇总成「每账号每天一行」，由
//!   [`refresh_daily_rollup`] 从明细算出来、幂等。报表读它 + 「今天」的明细现算，
//!   所以不管选多长的区间，每次读的都不超过一万多行 + 一天的明细。
//!   明细按 [`RETENTION_DAYS`] 裁剪；汇总表不裁（148 个账号一年约 5 万行）。
//!
//! 已知的误差（都是低估，不会多算）：零点前后那一步不归任何一天（最多 5 分钟的
//! 消耗）；一次重置之后 5 分钟内已用量就超过重置前水平的话，那一步会被看成普通的
//! 上升（线上 4 小时内 3 次回落，一次都没有出现这种情况）。
//!
//! 刻意**不走 schema 版本链**（`db/schema.rs` 的 `apply_version`）：版本号是
//! 上游的序列，分支自己占一个号会在合并时与上游撞（库已是 37、上游代码也
//! 是 37，上游的 v37 迁移就被静默跳过；或库比上游新，二进制拒绝启动）。
//! 表用 `CREATE TABLE IF NOT EXISTS` 幂等自建，对上游的 diff 最小。

use serde_json::Value;

/// 明细表名。建表 / 写入 / 裁剪共用。
pub const TABLE: &str = "account_credit_snapshots";

/// 明细保留天数。报表能看的区间不受它限制（过完的日子都在汇总表里）；
/// 明细只用于「今天」的现算和事后核对。
pub const RETENTION_DAYS: i64 = 90;

/// 余额**有变化**时，两条明细之间的最小间隔（毫秒）。变化不会丢：到点后的第一条
/// 样本带着这段时间的全部上升量（差分只看相邻两条，不看中间查了多少次）。
pub const MIN_GAP_MS: i64 = 5 * 60 * 1000;

/// 余额**没有变化**时的记录间隔（毫秒）。它不为算消耗，只为了让「重置」当天就被
/// 看见、让闲置账号每天都有样本 —— 否则一天只有一条变化样本的日子会被标成「不完整」。
pub const HEARTBEAT_MS: i64 = 30 * 60 * 1000;

/// 报表只统计的合计钱包。已对线上数据核实它等于三个子钱包之和
/// （`usageOnDemand / Bonus / BasicPackageCredit`），所以只取它一个就不会重复计。
pub const TOTAL_WALLET: &str = "meter:usageTotalPackageCredit";

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

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;

use chrono::{Duration, Local, NaiveDate, TimeZone};
use rusqlite::params;
use serde_json::json;

use crate::server::db::Db;
use crate::server::logging;

/// 建表语句。幂等：已存在时什么都不做。
///
/// 明细的主键含 `ts`：同一账号同一钱包一个时刻一行。`(account_id, wallet_type, ts)`
/// 的主键索引覆盖「某账号某钱包某段时间按时间取相邻两条」的查询；单独的 `ts` 索引给
/// 裁剪和「最早一条」用。
///
/// `account_credit_seen` 是「有过合计钱包快照的账号」名册：按天取数时逐账号走主键
/// 索引（不排序、不扫全表），名册就是账号清单的来源。`account_credit_daily` 是按天
/// 汇总，`account_credit_daily_days` 记哪些日子已经汇总过，`account_credit_meta` 记
/// 一次性的迁移标记（名册补登记做过没有）。
const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS account_credit_snapshots (
    account_id       TEXT    NOT NULL,
    wallet_type      TEXT    NOT NULL,
    ts               INTEGER NOT NULL,
    credit_used      REAL    NOT NULL,
    credit_remaining REAL    NOT NULL,
    credit_total     REAL    NOT NULL,
    PRIMARY KEY (account_id, wallet_type, ts)
);
CREATE INDEX IF NOT EXISTS idx_account_credit_snapshots_ts ON account_credit_snapshots(ts);
CREATE TABLE IF NOT EXISTS account_credit_seen (
    account_id TEXT    PRIMARY KEY,
    first_ts   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS account_credit_daily (
    account_id  TEXT    NOT NULL,
    day         TEXT    NOT NULL,
    credit_used REAL    NOT NULL,
    resets      INTEGER NOT NULL,
    samples     INTEGER NOT NULL,
    PRIMARY KEY (account_id, day)
);
CREATE TABLE IF NOT EXISTS account_credit_daily_days (
    day TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS account_credit_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);";

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
/// 不是每次都落行：读数没变、离上一条又不到间隔的，见 [`should_write`]。
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

/// 把一批快照写进库（建表 → 判断要不要记 → 逐行写入 → 顺手裁剪、按天汇总）。
/// 拆出来是为了能用临时库直接测。
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
    if !should_write(conn, account_id, snapshots, now_ms)? {
        return Ok(());
    }
    for snapshot in snapshots {
        conn.execute(
            "INSERT OR REPLACE INTO account_credit_snapshots
               (account_id, wallet_type, ts, credit_used, credit_remaining, credit_total)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                account_id,
                snapshot.wallet_type,
                now_ms,
                snapshot.credit_used,
                snapshot.credit_remaining,
                snapshot.credit_total,
            ],
        )?;
    }
    // 名册：有合计钱包就登记，首次时刻取明细里最早的一条（主键索引上的 MIN 是一次定位，
    // 不是扫描）。INSERT OR IGNORE：登记过的账号不动。
    if snapshots.iter().any(|snapshot| snapshot.wallet_type == TOTAL_WALLET) {
        conn.execute(
            "INSERT OR IGNORE INTO account_credit_seen (account_id, first_ts) \
             VALUES (?1, (SELECT MIN(ts) FROM account_credit_snapshots \
                          WHERE account_id = ?1 AND wallet_type = ?2))",
            params![account_id, TOTAL_WALLET],
        )?;
    }
    // 裁剪：只删**已经成功汇总**的完整日期对应的明细。如果汇总没跟上（每轮最多 7 天），
    // 未汇总的明细必须保留到成功汇总后再删——否则历史报表会永久少算。
    prune_rolled(conn, now_ms)?;
    reopen_rolled_days(conn, now_ms)?;
    rollup_if_due(conn, now_ms, &NEXT_ROLLUP_MS);
    Ok(())
}

/// 样本落在了**已经汇总过**的日子里：撤掉那一天及之后的「已汇总」标记，让下一轮
/// [`refresh_daily_rollup`] 按新的明细重算（`INSERT OR REPLACE`，重算是幂等的）。
///
/// 正常不会发生 —— 样本的时间戳就是写入那一刻，永远落在「今天」；只有系统时钟被
/// 往回拨过了零点才会出现。不处理的话那一天的汇总就悄悄少算，事后看不出来。
fn reopen_rolled_days(conn: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM account_credit_daily_days WHERE day >= ?1",
        [date_key(day_of_local(now_ms))],
    )
}

/// 这次余额查询要不要落快照。以合计钱包的已用量为准（报表只认它）：
///
/// * 这个账号还没有合计钱包的明细 → 记（第一次见到它）；
/// * 读数变了 → 离上一条满 [`MIN_GAP_MS`] 才记。中间几次查询的上升量不会丢，
///   到点那条样本带着全部差值；
/// * 读数没变 → 每 [`HEARTBEAT_MS`] 记一条。
///
/// 快照里没有合计钱包的（报表用不上）不判断，照旧每次都记。
fn should_write(
    conn: &rusqlite::Connection,
    account_id: &str,
    snapshots: &[CreditSnapshot],
    now_ms: i64,
) -> rusqlite::Result<bool> {
    let Some(total) = snapshots.iter().find(|snapshot| snapshot.wallet_type == TOTAL_WALLET) else {
        return Ok(true);
    };
    let Some((last_ts, last_used)) = last_total_sample(conn, account_id)? else {
        return Ok(true);
    };
    let since_last = now_ms - last_ts;
    Ok(if last_used == total.credit_used {
        since_last >= HEARTBEAT_MS
    } else {
        since_last >= MIN_GAP_MS
    })
}

const LAST_TOTAL_SQL: &str = "SELECT ts, credit_used FROM account_credit_snapshots \
     WHERE account_id = ?1 AND wallet_type = ?2 ORDER BY ts DESC LIMIT 1";

/// 某账号合计钱包的最后一条明细 `(时间戳, 已用积分)`；没有时为 `None`。
/// 走主键索引 `(account_id, wallet_type, ts)` 倒着取第一条，不排序。
fn last_total_sample(conn: &rusqlite::Connection, account_id: &str) -> rusqlite::Result<Option<(i64, f64)>> {
    conn.query_row(LAST_TOTAL_SQL, params![account_id, TOTAL_WALLET], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .optional()
}

/// 删掉已成功汇总的完整日期对应的明细（保留期 [`RETENTION_DAYS`] 以内的也不删，
/// 因为汇总表只按天存，明细对「今天」的现算和事后核对还有用）。
///
/// 只删「已汇总」标记存在且已过期的日子：如果汇总没跟上（每轮最多 7 天），
/// 未汇总的明细保留到成功汇总后再删。这样历史报表不会永久少算。
fn prune_rolled(conn: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let today = day_of_local(now_ms);
    let cutoff = today - Duration::days(RETENTION_DAYS);
    // 找到所有已汇总且已过期的日子
    let mut stmt = conn.prepare_cached(
        "SELECT day FROM account_credit_daily_days WHERE day < ?1",
    )?;
    let days: Vec<String> = stmt.query_map([date_key(cutoff)], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    if days.is_empty() {
        return Ok(0);
    }
    // 逐日删除（走主键索引，不扫全表）
    let mut total = 0;
    for day in days {
        let Some(date) = parse_day(&day) else { continue };
        let Some(start) = day_start(date) else { continue };
        let end = start + 86_400_000;
        total += conn.execute(
            "DELETE FROM account_credit_snapshots WHERE ts >= ?1 AND ts < ?2",
            params![start, end],
        )?;
    }
    Ok(total)
}

/// 下一次允许做「按天汇总」的时刻（毫秒），进程内有效；真正的进度记在库里的
/// `account_credit_daily_days`，重启不丢。
static NEXT_ROLLUP_MS: AtomicI64 = AtomicI64::new(0);

/// 按天汇总的检查间隔（毫秒）。没有新过完的日子时一次检查只查两三次索引；
/// 失败也要隔这么久再试，不会每次写入都去撞。
const ROLLUP_INTERVAL_MS: i64 = 10 * 60 * 1000;

/// 一轮最多汇总多少天。明细很久没人汇总时（面板长期没打开）分几轮追上，
/// 单次持锁时间有上限；报表请求那一路也调它，所以第一次打开报表就能补齐。
const ROLLUP_BATCH_DAYS: usize = 7;

/// 落库路径上的汇总节拍：到了间隔才做一轮。`next` 由调用方传入（生产是
/// [`NEXT_ROLLUP_MS`]），测试可以传自己的。
fn rollup_if_due(conn: &rusqlite::Connection, now_ms: i64, next: &AtomicI64) {
    if now_ms < next.load(Ordering::Relaxed) {
        return;
    }
    next.store(now_ms + ROLLUP_INTERVAL_MS, Ordering::Relaxed);
    if let Err(error) = refresh_daily_rollup(conn, now_ms) {
        logging::verbose("[Credits]", &format!("积分按天汇总失败：{error}"));
    }
}

// ─── 报表（读侧）──────────────────────────────────────────────

use rusqlite::OptionalExtension as _;

/// 某个账号某一天的积分读数。
#[derive(Debug, Clone, PartialEq)]
pub struct DayUsage {
    /// 这一天消耗的积分：当天相邻两条快照的「已用量」增量之和，只累加上升的部分。
    pub credit_used: f64,
    /// `creditUsed` 在这一天里出现**下降**的次数。正常只增不减；下降说明上游收走了一份
    /// 到期套餐（总额同步减少；线上实测每份 100 积分）或重置了额度 ——
    /// 上一条之后到这一条之间的消耗算不出来，当天的数只能当下限看（不到到期套餐的
    /// 已用量，最多几分钟的消耗）。充值只抬高余额和总额，不会让已用量下降，不影响读数。
    pub resets: u32,
    /// 这一天只有一条快照：没有可相减的相邻两条，`credit_used` 恒为 0，
    /// 但那不代表「没消耗」。
    pub partial: bool,
}

/// 把一个账号按时间升序排好的快照（`(毫秒时间戳, 已用积分)`）按天分桶，
/// 逐日算出积分消耗。`day_of` 由调用方给出（为了测试能钉住时区）。
///
/// 为什么是「相邻两条的增量之和」而不是「末 − 首」：`creditUsed` 会因为套餐到期 /
/// 额度重置而回落，末减首会把回落当成负消耗、甚至算出负数；逐段累加只认上升，
/// 回落单独记成 `resets`，前端据此提示「这天偏低」。
///
/// 为什么按天而不是按小时：上游的结算不是实时的（线上实测：有请求的几分钟
/// 积分不动，随后一次性跳十几个积分），小时以内与当时的请求对不上，
/// **整天的合计才是准的**。
///
/// 跨天的第一条只作为新一天的基准；上一天的尾差（上一天最后一条 → 这一天
/// 第一条）不算到任何一天 —— 它跨越零点，归属说不清。
fn daily_usage(samples: &[(i64, f64)], day_of: impl Fn(i64) -> NaiveDate) -> BTreeMap<NaiveDate, DayUsage> {
    let mut out: BTreeMap<NaiveDate, DayUsage> = BTreeMap::new();
    let mut counts: BTreeMap<NaiveDate, usize> = BTreeMap::new();
    let mut previous: Option<(NaiveDate, f64)> = None;
    for &(ts, used) in samples {
        let day = day_of(ts);
        *counts.entry(day).or_insert(0) += 1;
        let entry = out.entry(day).or_insert_with(|| DayUsage { credit_used: 0.0, resets: 0, partial: false });
        if let Some((previous_day, previous_used)) = previous {
            if previous_day == day {
                let delta = used - previous_used;
                if delta >= 0.0 {
                    entry.credit_used += delta;
                } else {
                    entry.resets += 1;
                }
            }
        }
        previous = Some((day, used));
    }
    for (day, usage) in &mut out {
        usage.partial = counts.get(day).copied().unwrap_or(0) < 2;
    }
    out
}

/// 毫秒时间戳 → 本地时区的日期。
///
/// 与 `request_stats::clock::day_of` 是同一口径（`chrono::Local`，即容器时区）；
/// 那边是 `pub(super)`、这里拿不到，所以换算重写一份 —— 「一天怎么切」必须
/// 跟请求报表一致，改其中一处时另一处要跟着改。
fn day_of_local(ts: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ts)
        .single()
        .or_else(|| Local.timestamp_millis_opt(0).single())
        .unwrap_or_else(Local::now)
        .date_naive()
}

fn date_key(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// 本地日期 → `[零点, 次日零点)` 的毫秒区间（左闭右开，与请求报表同口径）。
/// 日期到头时（公元二十几万年）为 `None`。
fn day_bounds(day: NaiveDate) -> Option<(i64, i64)> {
    Some((day_start(day)?, day_start(day.succ_opt()?)?))
}

fn day_start(day: NaiveDate) -> Option<i64> {
    let midnight = day.and_hms_opt(0, 0, 0)?;
    Local.from_local_datetime(&midnight).earliest().map(|time| time.timestamp_millis())
}

/// 明细里最早一条的时间戳；表还没建或还没有数据时为 `None`。
/// 日报表里最早的一天（用于「all」区间的起点，不受明细保留期限制）。
pub fn first_daily_day(conn: &rusqlite::Connection) -> rusqlite::Result<Option<NaiveDate>> {
    let text: Option<String> = conn
        .query_row("SELECT MIN(day) FROM account_credit_daily", [], |row| row.get(0))
        .optional()?;
    Ok(text.as_deref().and_then(parse_day))
}

pub fn first_snapshot_ts(conn: &rusqlite::Connection) -> rusqlite::Result<Option<i64>> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [TABLE],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Ok(None);
    }
    conn.query_row("SELECT MIN(ts) FROM account_credit_snapshots", [], |row| row.get(0))
}

const DAY_SERIES_SQL: &str = "SELECT ts, credit_used FROM account_credit_snapshots \
     WHERE account_id = ?1 AND wallet_type = ?2 AND ts >= ?3 AND ts < ?4 ORDER BY ts";

/// 某一天里每个账号的合计钱包明细（按时间升序），只列这一天有样本的账号。
///
/// 账号清单取名册，再逐账号走主键索引取一段 —— 不排序、不扫全表：连接没开内存
/// 临时表，大排序在只读根文件系统的容器里可能落不了盘；而全表按时间扫再过滤
/// 钱包类型，读的行是这里的 4 倍。
fn day_series(conn: &rusqlite::Connection, day: NaiveDate) -> rusqlite::Result<BTreeMap<String, Vec<(i64, f64)>>> {
    let Some((start_ts, end_ts)) = day_bounds(day) else {
        return Ok(BTreeMap::new());
    };
    let accounts: Vec<String> = {
        let mut stmt = conn.prepare_cached(
            "SELECT account_id FROM account_credit_seen WHERE first_ts < ?1 ORDER BY account_id",
        )?;
        let rows = stmt.query_map([end_ts], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut out = BTreeMap::new();
    let mut stmt = conn.prepare_cached(DAY_SERIES_SQL)?;
    for account_id in accounts {
        let rows = stmt.query_map(params![account_id, TOTAL_WALLET, start_ts, end_ts], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        let series: Vec<(i64, f64)> = rows.collect::<rusqlite::Result<_>>()?;
        if !series.is_empty() {
            out.insert(account_id, series);
        }
    }
    Ok(out)
}

/// 把一天的明细汇总进 `account_credit_daily`（每账号一行）。`INSERT OR REPLACE`：
/// 同一天重算一遍得到的是同一份结果。
fn rollup_day(conn: &rusqlite::Connection, day: NaiveDate) -> rusqlite::Result<()> {
    let mut put = conn.prepare_cached(
        "INSERT OR REPLACE INTO account_credit_daily (account_id, day, credit_used, resets, samples) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (account_id, series) in day_series(conn, day)? {
        let Some(usage) = daily_usage(&series, day_of_local).remove(&day) else { continue };
        put.execute(params![account_id, date_key(day), usage.credit_used, usage.resets, series.len() as i64])?;
    }
    Ok(())
}

fn rollup_days(conn: &rusqlite::Connection, days: &[NaiveDate]) -> rusqlite::Result<usize> {
    let mut done = conn.prepare_cached("INSERT OR IGNORE INTO account_credit_daily_days (day) VALUES (?1)")?;
    for day in days {
        rollup_day(conn, *day)?;
        done.execute([date_key(*day)])?;
    }
    Ok(days.len())
}

/// 名册是空的、明细里却已经有合计钱包的行（名册之前写入的）：按时间顺序扫一遍明细，
/// 每个账号第一次出现的那条就是它的首次时刻。只在没有名册时做一次。
/// 按时间索引的顺序读，不排序。
const BACKFILL_SQL: &str = "SELECT account_id, ts FROM account_credit_snapshots WHERE wallet_type = ?1 ORDER BY ts";

fn backfill_registry(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    let mut first: BTreeMap<String, i64> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(BACKFILL_SQL)?;
        let rows = stmt.query_map([TOTAL_WALLET], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (account_id, ts) = row?;
            first.entry(account_id).or_insert(ts);
        }
    }
    let mut put = conn.prepare_cached("INSERT OR IGNORE INTO account_credit_seen (account_id, first_ts) VALUES (?1, ?2)")?;
    for (account_id, ts) in &first {
        put.execute(params![account_id, ts])?;
    }
    Ok(first.len())
}

/// 已经汇总到哪一天：`account_credit_daily_days` 里最大的日期。按时间顺序汇总，
/// 所以「已汇总」总是从最早一天起连续的一段，最大的那天就是进度。
fn last_rolled_day(conn: &rusqlite::Connection) -> rusqlite::Result<Option<NaiveDate>> {
    let text: Option<String> = conn.query_row("SELECT MAX(day) FROM account_credit_daily_days", [], |row| row.get(0))?;
    Ok(text.as_deref().and_then(parse_day))
}

/// 区间内哪些日子已经汇总过了（用于判断哪些日子需要从明细现算）。
fn rolled_days_in_range(
    conn: &rusqlite::Connection,
    start: NaiveDate,
    end: NaiveDate,
) -> rusqlite::Result<std::collections::HashSet<NaiveDate>> {
    let mut stmt = conn.prepare_cached(
        "SELECT day FROM account_credit_daily_days WHERE day >= ?1 AND day <= ?2",
    )?;
    let rows = stmt.query_map(params![date_key(start), date_key(end)], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).filter_map(|d| parse_day(&d)).collect())
}

/// 名册只可能缺一种行：名册出现之前（更早的那版二进制）写下的明细。补登记每个库只做一次，
/// 做没做过记在库里的 `account_credit_meta`（进程重启不丢）。
///
/// **不能用「名册为空」来判断**：线上切换后的第一次调用来自落库路径，它已经先登记了自己
/// 那一个账号 —— 名册不为空、补登记却被跳过（2026-11-27 上线后名册只有 41 / 148，
/// 靠各账号之后自己登记才补齐）。补登记和标记在同一个事务里落库。
fn backfill_registry_once(conn: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<()> {
    const KEY: &str = "registry_backfilled";
    let done: bool = conn
        .query_row("SELECT 1 FROM account_credit_meta WHERE key = ?1", [KEY], |_| Ok(()))
        .optional()?
        .is_some();
    if done {
        return Ok(());
    }
    let mark = |conn: &rusqlite::Connection| {
        conn.execute(
            "INSERT OR REPLACE INTO account_credit_meta (key, value) VALUES (?1, ?2)",
            params![KEY, now_ms.to_string()],
        )
    };
    if conn.is_autocommit() {
        let tx = conn.unchecked_transaction()?;
        backfill_registry(&tx)?;
        mark(&tx)?;
        tx.commit()?;
    } else {
        backfill_registry(conn)?;
        mark(conn)?;
    }
    Ok(())
}

/// 把「已经过完的日子」（`now_ms` 所在的本地日期之前）从明细汇总进
/// `account_credit_daily`，返回这一轮汇总了几天。幂等：已经汇总过的日子不再动。
///
/// 明细和「已汇总」标记在同一个事务里落库，中途失败不会留下「标记写了、行没写全」。
fn refresh_daily_rollup(conn: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let Some(first) = first_snapshot_ts(conn)?.map(day_of_local) else {
        return Ok(0);
    };
    backfill_registry_once(conn, now_ms)?;
    let today = day_of_local(now_ms);
    let pending: Vec<NaiveDate> = {
        let mut day = match last_rolled_day(conn)? {
            Some(last) => last.succ_opt(),
            None => Some(first),
        };
        let mut days = Vec::new();
        while let Some(current) = day {
            if current >= today || days.len() == ROLLUP_BATCH_DAYS {
                break;
            }
            days.push(current);
            day = current.succ_opt();
        }
        days
    };
    if pending.is_empty() {
        return Ok(0);
    }
    if conn.is_autocommit() {
        let tx = conn.unchecked_transaction()?;
        let rolled = rollup_days(&tx, &pending)?;
        tx.commit()?;
        Ok(rolled)
    } else {
        rollup_days(conn, &pending)
    }
}

/// 某个账号连续几天的读数，日期升序。
type AccountDays = BTreeMap<NaiveDate, DayUsage>;

/// 读出区间内各账号每天的读数：过完的日子来自汇总表（先补齐汇总），`now_ms` 所在的
/// 「今天」从明细现算。
fn usage_by_day(
    conn: &rusqlite::Connection,
    start_date: NaiveDate,
    end_date: NaiveDate,
    now_ms: i64,
) -> rusqlite::Result<BTreeMap<String, AccountDays>> {
    refresh_daily_rollup(conn, now_ms)?;
    let today = day_of_local(now_ms);
    let mut out: BTreeMap<String, AccountDays> = BTreeMap::new();
    if let Some(last_finished) = today.pred_opt() {
        let last = end_date.min(last_finished);
        if start_date <= last {
            // 汇总可能没跟上（每轮最多 7 天）。对所选范围内尚未汇总的日子，
            // 从明细现算并纳入结果，不遗漏。
            let rolled = rolled_days_in_range(conn, start_date, last)?;
            let mut stmt = conn.prepare_cached(
                "SELECT account_id, day, credit_used, resets, samples FROM account_credit_daily \
                 WHERE day >= ?1 AND day <= ?2",
            )?;
            let rows = stmt.query_map(params![date_key(start_date), date_key(last)], |row| {
                let day: String = row.get(1)?;
                Ok((row.get::<_, String>(0)?, day, row.get::<_, f64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?))
            })?;
            for row in rows {
                let (account_id, day, credit_used, resets, samples) = row?;
                let Some(date) = parse_day(&day) else { continue };
                out.entry(account_id).or_default().insert(
                    date,
                    DayUsage { credit_used, resets: resets as u32, partial: samples < 2 },
                );
            }
            // 对未汇总的日子，从明细现算
            let mut day = start_date;
            while day <= last {
                if !rolled.contains(&day) {
                    for (account_id, series) in day_series(conn, day)? {
                        out.entry(account_id).or_default().extend(daily_usage(&series, day_of_local));
                    }
                }
                day = day.succ_opt().unwrap();
            }
        }
    }
    if start_date <= today && today <= end_date {
        for (account_id, series) in day_series(conn, today)? {
            out.entry(account_id).or_default().extend(daily_usage(&series, day_of_local));
        }
    }
    Ok(out)
}

/// `GET /api/stats/credits` 的响应正文：
///
/// ```text
/// { range, startDate, endDate, currency: "credit",
///   totalCreditUsed,                      // 区间内全部账号的消耗合计
///   firstSnapshotAt,                      // 库里最早一条快照（毫秒）；为 null 表示还没有数据
///   accounts: [{
///     id, label, creditUsed, resets, partial,
///     daily: [{ date, creditUsed, resets, partial }]   // 只列有读数的日子
///   }] }
/// ```
///
/// `accounts` 只列区间内**有消耗或有重置**的账号，按 `creditUsed` 降序。
/// `label` 取自 `names`（请求报表聚合时留下的账号名快照），没有就用 id。
/// `first_snapshot_at` 由调用方给：读库时是全库最早的一条（与区间无关，前端用它区分
/// 「还没开始记」和「这段区间没有消耗」）。
fn build_report(
    per_account: &BTreeMap<String, AccountDays>,
    names: &BTreeMap<String, String>,
    range: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
    first_snapshot_at: Option<i64>,
) -> serde_json::Value {
    let mut accounts: Vec<serde_json::Value> = Vec::new();
    let mut total_credit_used = 0.0;
    for (account_id, days) in per_account {
        let credit_used: f64 = days.values().map(|usage| usage.credit_used).sum();
        let resets: u32 = days.values().map(|usage| usage.resets).sum();
        if credit_used <= 0.0 && resets == 0 {
            continue;
        }
        total_credit_used += credit_used;
        let label = names.get(account_id).filter(|name| !name.is_empty()).unwrap_or(account_id);
        accounts.push(json!({
            "id": account_id,
            "label": label,
            "creditUsed": credit_used,
            "resets": resets,
            "partial": days.values().any(|usage| usage.partial),
            "daily": days.iter().filter(|(_, usage)| usage.credit_used > 0.0 || usage.resets > 0 || usage.partial)
                .map(|(day, usage)| json!({
                    "date": date_key(*day),
                    "creditUsed": usage.credit_used,
                    "resets": usage.resets,
                    "partial": usage.partial,
                })).collect::<Vec<_>>(),
        }));
    }
    accounts.sort_by(|left, right| {
        right["creditUsed"].as_f64().unwrap_or(0.0)
            .total_cmp(&left["creditUsed"].as_f64().unwrap_or(0.0))
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
    });

    json!({
        "range": range,
        "startDate": date_key(start_date),
        "endDate": date_key(end_date),
        "currency": "credit",
        "totalCreditUsed": total_credit_used,
        "firstSnapshotAt": first_snapshot_at,
        "accounts": accounts,
    })
}

/// [`build_report`] 的纯内存入口：样本 `(账号 id, 时间戳, 已用积分)`，只取
/// `[start_ts, end_ts)` 内的，`firstSnapshotAt` 取其中最早的一条。
/// 读库的那条路不走它（见 [`report_from_db`]），留着给算法的单元测试。
#[cfg(test)]
fn report(
    samples: &[(String, i64, f64)],
    names: &BTreeMap<String, String>,
    start_ts: i64,
    end_ts: i64,
    range: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> serde_json::Value {
    let mut series: BTreeMap<String, Vec<(i64, f64)>> = BTreeMap::new();
    let mut first_snapshot_at: Option<i64> = None;
    for (account_id, ts, used) in samples {
        if *ts < start_ts || *ts >= end_ts {
            continue;
        }
        first_snapshot_at = Some(first_snapshot_at.map_or(*ts, |first| first.min(*ts)));
        series.entry(account_id.clone()).or_default().push((*ts, *used));
    }
    let per_account: BTreeMap<String, AccountDays> =
        series.iter().map(|(account_id, samples)| (account_id.clone(), daily_usage(samples, day_of_local))).collect();
    build_report(&per_account, names, range, start_date, end_date, first_snapshot_at)
}

/// 在一条连接上读出积分报表。表还不存在时 [`refresh_daily_rollup`] 会把它们建出来，
/// 返回一份空报表（不是错误）。
pub fn report_from_db(
    conn: &rusqlite::Connection,
    names: &BTreeMap<String, String>,
    range: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
    now_ms: i64,
) -> rusqlite::Result<serde_json::Value> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    let per_account = usage_by_day(conn, start_date, end_date, now_ms)?;
    Ok(build_report(&per_account, names, range, start_date, end_date, first_snapshot_ts(conn)?))
}

/// 读出积分报表。库不可用或读失败时返回 `None`（调用方给 500，不给一份
/// 看着像「没消耗」的空报表）。
pub fn load_report(
    names: &BTreeMap<String, String>,
    range: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Option<serde_json::Value> {
    let db = database()?;
    let now_ms = Local::now().timestamp_millis();
    db.with(|conn| match report_from_db(conn, names, range, start_date, end_date, now_ms) {
        Ok(report) => Some(report),
        Err(error) => {
            logging::verbose("[Credits]", &format!("积分报表读取失败：{error}"));
            None
        }
    })?
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
    fn write_snapshots_prunes_rows_older_than_the_retention_window() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-prune");
        let snapshot = extract_wallets(&json!({"wallets": [wallet("free", 1.0, 1.0, 2.0)]}));
        let now = 10_000 * 86_400_000;
        db.with(|conn| {
            let too_old = now - (RETENTION_DAYS + 1) * 86_400_000;
            let kept = now - (RETENTION_DAYS - 1) * 86_400_000;
            write_snapshots(conn, "acct-1", &snapshot, too_old).unwrap();
            write_snapshots(conn, "acct-1", &snapshot, kept).unwrap();
            // 先汇总（让「已汇总」标记落库），再写一次触发裁剪
            refresh_daily_rollup(conn, now).unwrap();
            write_snapshots(conn, "acct-1", &snapshot, now).unwrap();
            let stamps: Vec<i64> = rows(conn).iter().map(|(_, ts, _, _, _)| *ts).collect();
            assert_eq!(stamps, vec![kept, now], "保留期之外且已汇总的快照应被裁掉");
        })
        .expect("库不可用");
    }

    // ─── 落库：抽样 ──────────────────────────────────────────────────

    fn total(used: f64) -> Vec<CreditSnapshot> {
        extract_wallets(&json!({"wallets": [{"type": TOTAL_WALLET, "creditUsed": used}]}))
    }

    fn write_total(conn: &rusqlite::Connection, account_id: &str, used: f64, ts: i64) {
        write_snapshots(conn, account_id, &total(used), ts).unwrap();
    }

    /// 测试专用：只写明细，不做裁剪 / 名册登记 / 汇总维护。
    /// 避免触发全局 `NEXT_ROLLUP_MS` 导致「单独跑」和「整组跑」结果不同。
    fn write_total_raw(conn: &rusqlite::Connection, account_id: &str, used: f64, ts: i64) {
        // 先建表（write_snapshots 里做的事，这里也要做）
        conn.execute_batch(CREATE_TABLE_SQL).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO account_credit_snapshots (account_id, wallet_type, ts, credit_used, credit_remaining, credit_total) \
             VALUES (?1, ?2, ?3, ?4, 0, 0)",
            params![account_id, TOTAL_WALLET, ts, used],
        ).unwrap();
    }

    /// 某账号合计钱包的全部明细 `(时间戳, 已用积分)`，按时间升序。
    fn raw_series(conn: &rusqlite::Connection, account_id: &str) -> Vec<(i64, f64)> {
        let mut stmt = conn.prepare(DAY_SERIES_SQL).unwrap();
        let start: i64 = conn
            .query_row("SELECT MIN(ts) FROM account_credit_snapshots", [], |row| row.get(0))
            .unwrap();
        stmt.query_map(params![account_id, TOTAL_WALLET, start, i64::MAX], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    #[test]
    fn unchanged_readings_are_recorded_once_per_heartbeat() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-heartbeat");
        let t0 = at("2026-11-26", 10, 0);
        db.with(|conn| {
            write_total(conn, "a", 100.0, t0);
            // 之后 29 分钟里每分钟查一次，读数都没变：一条都不记
            for minute in 1..30 {
                write_total(conn, "a", 100.0, t0 + minute * 60_000);
            }
            assert_eq!(raw_series(conn, "a"), vec![(t0, 100.0)], "读数没变、不满 30 分钟：不记");
            // 满 30 分钟记一条
            write_total(conn, "a", 100.0, t0 + HEARTBEAT_MS);
            assert_eq!(raw_series(conn, "a"), vec![(t0, 100.0), (t0 + HEARTBEAT_MS, 100.0)]);
        })
        .expect("库不可用");
    }

    #[test]
    fn changed_readings_wait_for_the_minimum_gap_and_carry_the_whole_rise() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-gap");
        let t0 = at("2026-11-26", 10, 0);
        db.with(|conn| {
            write_total(conn, "a", 100.0, t0);
            // 5 分钟内连涨 4 次：每次都不到最小间隔，先不记
            for step in 1..5 {
                write_total(conn, "a", 100.0 + f64::from(step), t0 + i64::from(step) * 60_000);
            }
            assert_eq!(raw_series(conn, "a"), vec![(t0, 100.0)], "变了但不满 5 分钟：不记");
            // 满 5 分钟后的那条记下最新读数：这 50 分一分不少地体现在差分里
            write_total(conn, "a", 150.0, t0 + MIN_GAP_MS);
            let series = raw_series(conn, "a");
            assert_eq!(series, vec![(t0, 100.0), (t0 + MIN_GAP_MS, 150.0)]);
            assert_eq!(daily_usage(&series, day_of_local)[&day("2026-11-26")].credit_used, 50.0);
        })
        .expect("库不可用");
    }

    #[test]
    fn repeated_calls_in_the_same_instant_do_not_add_rows_and_other_accounts_are_independent() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-repeat");
        let t0 = at("2026-11-26", 10, 0);
        db.with(|conn| {
            write_total(conn, "acct-1", 100.0, t0);
            // 同一毫秒再来一次（余额查询重试）：读数没变，不追加
            write_total(conn, "acct-1", 100.0, t0);
            // 另一个账号在同一毫秒各有一行，互不影响
            write_total(conn, "acct-2", 100.0, t0);
            assert_eq!(raw_series(conn, "acct-1"), vec![(t0, 100.0)], "同一毫秒的重复调用不追加");
            assert_eq!(raw_series(conn, "acct-2"), vec![(t0, 100.0)]);
            assert_eq!(rows(conn).len(), 2, "acct-1 一行 + acct-2 一行");
        })
        .expect("库不可用");
    }

    #[test]
    fn snapshots_without_a_total_wallet_are_recorded_every_time() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-no-total");
        let snapshot = extract_wallets(&json!({"wallets": [wallet("free", 1.0, 1.0, 2.0)]}));
        db.with(|conn| {
            // 建表每次都执行（CREATE IF NOT EXISTS），重复调用不能报错；
            // 没有合计钱包的快照不参与抽样：每次都记，与抽样之前的行为一致
            for ts in [1_000, 2_000, 3_000] {
                write_snapshots(conn, "acct-1", &snapshot, ts).unwrap();
            }
            assert_eq!(rows(conn).len(), 3);
            let registered: i64 = conn.query_row("SELECT COUNT(*) FROM account_credit_seen", [], |row| row.get(0)).unwrap();
            assert_eq!(registered, 0, "没有合计钱包的账号不进名册（报表不读它）");
        })
        .expect("库不可用");
    }

    /// `EXPLAIN QUERY PLAN` 的说明行。SQL 里用到的参数要原样给全：rusqlite 对位置参数的数量是严格相等的。
    fn plan<P: rusqlite::Params>(conn: &rusqlite::Connection, sql: &str, params: P) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt.query_map(params, |row| row.get::<_, String>(3)).unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    #[test]
    fn hot_path_and_day_queries_use_the_primary_key_index_without_sorting() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-plan");
        db.with(|conn| {
            write_total(conn, "a", 1.0, 1_000);
            let last = plan(conn, LAST_TOTAL_SQL, params!["a", TOTAL_WALLET]);
            assert!(last.iter().any(|line| line.contains("sqlite_autoindex_account_credit_snapshots_1")), "{last:?}");
            assert!(!last.iter().any(|line| line.contains("TEMP B-TREE")), "取最后一条不该排序：{last:?}");
            let day = plan(conn, DAY_SERIES_SQL, params!["a", TOTAL_WALLET, 1_000, 2_000]);
            assert!(day.iter().any(|line| line.contains("sqlite_autoindex_account_credit_snapshots_1")), "{day:?}");
            assert!(!day.iter().any(|line| line.contains("TEMP B-TREE")), "按天取明细不该排序：{day:?}");
            let scan = plan(conn, BACKFILL_SQL, params![TOTAL_WALLET]);
            assert!(scan.iter().any(|line| line.contains("idx_account_credit_snapshots_ts")), "{scan:?}");
            assert!(!scan.iter().any(|line| line.contains("TEMP B-TREE")), "补名册不该排序：{scan:?}");
        })
        .expect("库不可用");
    }

    #[test]
    fn the_registry_remembers_the_first_snapshot_of_each_account() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-registry");
        let t0 = at("2026-11-26", 10, 0);
        db.with(|conn| {
            write_total(conn, "a", 1.0, t0);
            write_total(conn, "a", 2.0, t0 + MIN_GAP_MS);
            let first: i64 = conn
                .query_row("SELECT first_ts FROM account_credit_seen WHERE account_id = 'a'", [], |row| row.get(0))
                .unwrap();
            assert_eq!(first, t0, "登记的是最早一条的时刻，不是登记那一刻");
            write_total(conn, "a", 3.0, t0 + 2 * MIN_GAP_MS);
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM account_credit_seen", [], |row| row.get(0)).unwrap();
            assert_eq!(count, 1, "登记过的账号不重复");
        })
        .expect("库不可用");
    }

    // ─── 报表：按天分桶 ──────────────────────────────────────────────

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    /// 本地时间「某日 某时 某分」的毫秒时间戳。在测试里从日期生成，不手算。
    fn at(text: &str, hour: u32, minute: u32) -> i64 {
        Local.from_local_datetime(&day(text).and_hms_opt(hour, minute, 0).unwrap())
            .earliest().unwrap().timestamp_millis()
    }

    fn usage(credit_used: f64, resets: u32, partial: bool) -> DayUsage {
        DayUsage { credit_used, resets, partial }
    }

    #[test]
    fn daily_usage_sums_the_rises_between_adjacent_snapshots() {
        let samples = [(at("2026-11-26", 10, 0), 100.0), (at("2026-11-26", 10, 1), 103.5), (at("2026-11-26", 10, 2), 104.25)];
        assert_eq!(daily_usage(&samples, day_of_local), BTreeMap::from([(day("2026-11-26"), usage(4.25, 0, false))]));
    }

    #[test]
    fn daily_usage_buckets_by_local_day_and_does_not_bridge_midnight() {
        // 23:59 → 次日 00:01 这一步跨越零点，不算到任何一天
        let samples = [
            (at("2026-11-26", 23, 58), 10.0), (at("2026-11-26", 23, 59), 12.0),
            (at("2026-11-27", 0, 1), 20.0), (at("2026-11-27", 0, 2), 21.0),
        ];
        let out = daily_usage(&samples, day_of_local);
        assert_eq!(out[&day("2026-11-26")], usage(2.0, 0, false));
        assert_eq!(out[&day("2026-11-27")], usage(1.0, 0, false), "跨零点那一步的 8 分不归次日");
    }

    #[test]
    fn daily_usage_counts_a_drop_as_a_reset_instead_of_negative_usage() {
        // 套餐到期 / 额度重置让已用量回落：不计成负数，记一次回落（`resets`）
        let samples = [(at("2026-11-26", 10, 0), 500.0), (at("2026-11-26", 10, 1), 503.0), (at("2026-11-26", 10, 2), 1.0), (at("2026-11-26", 10, 3), 4.0)];
        assert_eq!(daily_usage(&samples, day_of_local)[&day("2026-11-26")], usage(6.0, 1, false));
    }

    #[test]
    fn daily_usage_marks_a_day_with_a_single_snapshot_as_partial() {
        let samples = [(at("2026-11-26", 10, 0), 100.0), (at("2026-11-27", 10, 0), 130.0), (at("2026-11-27", 10, 1), 131.0)];
        let out = daily_usage(&samples, day_of_local);
        assert_eq!(out[&day("2026-11-26")], usage(0.0, 0, true), "只有一条：没有可相减的相邻两条");
        assert_eq!(out[&day("2026-11-27")], usage(1.0, 0, false));
    }

    #[test]
    fn daily_usage_on_an_empty_series_is_empty() {
        assert!(daily_usage(&[], day_of_local).is_empty());
    }

    // ─── 报表：装配 ──────────────────────────────────────────────────

    fn names(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(id, name)| (id.to_string(), name.to_string())).collect()
    }

    #[test]
    fn report_sums_per_account_sorts_by_usage_and_names_the_account() {
        let samples = [
            ("a".to_string(), at("2026-11-26", 10, 0), 0.0), ("a".to_string(), at("2026-11-26", 10, 1), 30.0),
            ("b".to_string(), at("2026-11-26", 10, 0), 0.0), ("b".to_string(), at("2026-11-26", 10, 1), 50.0),
        ];
        let start = at("2026-11-26", 0, 0);
        let end = at("2026-11-27", 0, 0);
        let out = report(&samples, &names(&[("b", "乙号")]), start, end, "custom", day("2026-11-26"), day("2026-11-26"));
        assert_eq!(out["totalCreditUsed"], 80.0);
        assert_eq!(out["firstSnapshotAt"], start + 10 * 3_600_000);
        assert_eq!(out["accounts"].as_array().unwrap().len(), 2);
        let accounts = out["accounts"].as_array().unwrap();
        assert_eq!((accounts[0]["id"].as_str(), accounts[0]["label"].as_str()), (Some("b"), Some("乙号")));
        // 没有名字快照的账号用 id 当展示名
        assert_eq!(accounts[1]["label"].as_str(), Some("a"));
        assert_eq!(accounts[0]["daily"][0]["date"].as_str(), Some("2026-11-26"));
    }

    #[test]
    fn report_leaves_out_accounts_without_any_usage_or_reset() {
        let samples = [("a".to_string(), at("2026-11-26", 10, 0), 0.0), ("a".to_string(), at("2026-11-26", 10, 1), 0.0)];
        let out = report(&samples, &BTreeMap::new(), at("2026-11-26", 0, 0), at("2026-11-27", 0, 0), "today", day("2026-11-26"), day("2026-11-26"));
        assert_eq!(out["accounts"].as_array().unwrap().len(), 0, "两条快照数值没变 = 没消耗，不出行");
        assert_eq!(out["totalCreditUsed"], 0.0);
        assert_eq!(out["firstSnapshotAt"], at("2026-11-26", 10, 0), "没消耗也算有数据，firstSnapshotAt 不为 null");
    }

    #[test]
    fn report_ignores_snapshots_outside_the_half_open_range() {
        let samples = [
            ("a".to_string(), at("2026-11-25", 23, 59), 0.0),
            ("a".to_string(), at("2026-11-26", 0, 0), 10.0),
            ("a".to_string(), at("2026-11-26", 23, 59), 70.0),
            ("a".to_string(), at("2026-11-27", 0, 0), 999.0),
        ];
        let start = at("2026-11-26", 0, 0);
        let end = at("2026-11-27", 0, 0);
        let out = report(&samples, &BTreeMap::new(), start, end, "today", day("2026-11-26"), day("2026-11-26"));
        assert_eq!(out["totalCreditUsed"], 60.0, "区间是左闭右开：0:00 那条算、次日 0:00 那条不算");
        assert_eq!(out["firstSnapshotAt"], start);
    }

    #[test]
    fn report_on_no_data_has_a_null_first_snapshot() {
        let out = report(&[], &BTreeMap::new(), 0, 1_000, "today", day("2026-11-26"), day("2026-11-26"));
        assert_eq!(out["totalCreditUsed"], 0.0);
        assert!(out["firstSnapshotAt"].is_null());
        assert_eq!(out["accounts"].as_array().unwrap().len(), 0);
    }

    // ─── 报表：读库（汇总表 + 今天现算）──────────────────────────────

    /// 汇总表的全部行 `(账号, 日期, 消耗, 重置次数, 样本数)`。
    fn daily_rows(conn: &rusqlite::Connection) -> Vec<(String, String, f64, i64, i64)> {
        let mut stmt = conn
            .prepare("SELECT account_id, day, credit_used, resets, samples FROM account_credit_daily ORDER BY account_id, day")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    fn rolled_days(conn: &rusqlite::Connection) -> Vec<String> {
        let mut stmt = conn.prepare("SELECT day FROM account_credit_daily_days ORDER BY day").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0)).unwrap().map(|row| row.unwrap()).collect()
    }

    fn read(
        conn: &rusqlite::Connection,
        names: &BTreeMap<String, String>,
        start: NaiveDate,
        end: NaiveDate,
        now: i64,
    ) -> serde_json::Value {
        report_from_db(conn, names, "custom", start, end, now).unwrap()
    }

    /// 两天、两个账号的样本：
    /// * `a`：26 日 10:00 → 10:10 涨 30；27 日 09:00 → 09:10 涨 5
    /// * `b`：只有 26 日，11:00 → 11:10 涨 7
    fn seed_two_days(conn: &rusqlite::Connection) {
        write_total(conn, "a", 10.0, at("2026-11-26", 10, 0));
        write_total(conn, "a", 40.0, at("2026-11-26", 10, 10));
        write_total(conn, "a", 40.0, at("2026-11-27", 9, 0));
        write_total(conn, "a", 45.0, at("2026-11-27", 9, 10));
        write_total(conn, "b", 0.0, at("2026-11-26", 11, 0));
        write_total(conn, "b", 7.0, at("2026-11-26", 11, 10));
    }

    #[test]
    fn report_reads_finished_days_from_the_rollup_and_today_from_the_raw_rows() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-report-db");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            seed_two_days(conn);
            // 27 日还没过完：此刻只有 26 日进了汇总表
            let out = read(conn, &names(&[("b", "乙号")]), day("2026-11-25"), day("2026-11-27"), now);
            assert_eq!(out["totalCreditUsed"], 42.0, "30 + 7（26 日，读汇总表）+ 5（27 日，明细现算）");
            let accounts = out["accounts"].as_array().unwrap();
            assert_eq!(accounts.len(), 2);
            assert_eq!((accounts[0]["id"].as_str(), accounts[0]["creditUsed"].as_f64()), (Some("a"), Some(35.0)));
            assert_eq!(accounts[0]["daily"].as_array().unwrap().len(), 2, "a 两天都有读数");
            assert_eq!((accounts[1]["id"].as_str(), accounts[1]["label"].as_str()), (Some("b"), Some("乙号")));
            assert_eq!(daily_rows(conn).len(), 2, "26 日两个账号各一行");
            assert_eq!(rolled_days(conn), vec!["2026-11-26".to_string()]);

            // 只看过完的那天：全部来自汇总表，只统计合计钱包、只统计区间内
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-26"), now);
            assert_eq!(out["totalCreditUsed"], 37.0);
            assert_eq!(out["accounts"].as_array().unwrap().len(), 2);
            // 只看今天：全部来自明细
            let out = read(conn, &BTreeMap::new(), day("2026-11-27"), day("2026-11-27"), now);
            assert_eq!(out["totalCreditUsed"], 5.0);
            assert_eq!(out["accounts"].as_array().unwrap().len(), 1);
            // 区间在两条数据之间、不含任何有样本的日子：空报表，但库里有数据
            let out = read(conn, &BTreeMap::new(), day("2026-11-20"), day("2026-11-20"), now);
            assert_eq!(out["accounts"].as_array().unwrap().len(), 0);
            assert_eq!(out["firstSnapshotAt"], at("2026-11-26", 10, 0), "firstSnapshotAt 是全库最早的一条，与区间无关");
        })
        .expect("库不可用");
    }

    #[test]
    fn non_total_wallets_are_left_out_of_the_report() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-wallets");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            // 两个时刻都另带一个数值很大的非合计钱包（999 / 500）：计入的话合计会远大于 9
            let first = extract_wallets(&json!({"wallets": [{"type": TOTAL_WALLET, "creditUsed": 1.0}, {"type": "free", "creditUsed": 999.0}]}));
            let second = extract_wallets(&json!({"wallets": [{"type": TOTAL_WALLET, "creditUsed": 10.0}, {"type": "other", "creditUsed": 500.0}]}));
            write_snapshots(conn, "a", &first, at("2026-11-26", 10, 0)).unwrap();
            write_snapshots(conn, "a", &second, at("2026-11-26", 10, 10)).unwrap();
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-26"), now);
            assert_eq!(out["totalCreditUsed"], 9.0, "只取合计钱包（不含 999 / 500）");
            assert_eq!(daily_rows(conn), vec![("a".to_string(), "2026-11-26".to_string(), 9.0, 0, 2)]);
        })
        .expect("库不可用");
    }

    #[test]
    fn rolling_up_twice_changes_nothing() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-rollup-twice");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            seed_two_days(conn);
            assert_eq!(refresh_daily_rollup(conn, now).unwrap(), 1, "26 日过完了，汇总一天");
            let after_first = daily_rows(conn);
            assert_eq!(refresh_daily_rollup(conn, now).unwrap(), 0, "没有新过完的日子：这一轮什么都不做");
            assert_eq!(daily_rows(conn), after_first, "汇总表不变");
            assert_eq!(rolled_days(conn), vec!["2026-11-26".to_string()]);
        })
        .expect("库不可用");
    }

    #[test]
    fn rollup_keeps_a_reset_and_flags_a_day_with_a_single_sample() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-rollup-flags");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            // a：500 → 503（涨 3）→ 回落到 1（重置）→ 4（涨 3）：消耗 6、重置 1 次
            write_total(conn, "a", 500.0, at("2026-11-26", 10, 0));
            write_total(conn, "a", 503.0, at("2026-11-26", 10, 10));
            write_total(conn, "a", 1.0, at("2026-11-26", 10, 20));
            write_total(conn, "a", 4.0, at("2026-11-26", 10, 30));
            // b：这一天只有一条样本
            write_total(conn, "b", 8.0, at("2026-11-26", 11, 0));
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-26"), now);
            assert_eq!(out["totalCreditUsed"], 6.0, "回落那一步不计成负数，前后的上升都算");
            let accounts = out["accounts"].as_array().unwrap();
            assert_eq!(accounts.len(), 1, "b 没消耗也没重置，不出行");
            assert_eq!((accounts[0]["resets"].as_u64(), accounts[0]["partial"].as_bool()), (Some(1), Some(false)));
            assert_eq!(
                daily_rows(conn),
                vec![
                    ("a".to_string(), "2026-11-26".to_string(), 6.0, 1, 4),
                    ("b".to_string(), "2026-11-26".to_string(), 0.0, 0, 1),
                ]
            );
        })
        .expect("库不可用");
    }

    #[test]
    fn rollup_catches_up_in_batches_and_matches_a_computation_over_all_raw_rows() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-rollup-batches");
        db.with(|conn| {
            // 11 月 1 日到 10 日，每天 a 涨 10（用 write_total_raw 避免触发全局汇总节拍）
            for index in 0..10 {
                let date = day("2026-11-01") + chrono::Duration::days(index);
                let base = index as f64 * 100.0;
                let text = date.format("%Y-%m-%d").to_string();
                write_total_raw(conn, "a", base, at(&text, 10, 0));
                write_total_raw(conn, "a", base + 10.0, at(&text, 10, 10));
            }
            let now = at("2026-11-11", 12, 0);
            assert_eq!(refresh_daily_rollup(conn, now).unwrap(), ROLLUP_BATCH_DAYS, "一轮最多 7 天");
            assert_eq!(refresh_daily_rollup(conn, now).unwrap(), 3, "剩下的 3 天下一轮补齐");
            assert_eq!(refresh_daily_rollup(conn, now).unwrap(), 0);
            assert_eq!(rolled_days(conn).len(), 10);

            let from_rollup = read(conn, &BTreeMap::new(), day("2026-11-01"), day("2026-11-10"), now);
            assert_eq!(from_rollup["totalCreditUsed"], 100.0);
            assert_eq!(from_rollup["accounts"][0]["daily"].as_array().unwrap().len(), 10);
            // 与「把全部明细读回来逐条分桶」的结果逐字段一致
            let all: Vec<(String, i64, f64)> = raw_series(conn, "a").into_iter().map(|(ts, used)| ("a".to_string(), ts, used)).collect();
            let live = report(&all, &BTreeMap::new(), 0, i64::MAX, "custom", day("2026-11-01"), day("2026-11-10"));
            assert_eq!(from_rollup["accounts"], live["accounts"]);
            assert_eq!(from_rollup["totalCreditUsed"], live["totalCreditUsed"]);
        })
        .expect("库不可用");
    }

    #[test]
    fn a_sample_landing_on_an_already_rolled_day_reopens_that_day() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-reopen");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            // 26 日只有 a；27 日把 26 日汇总掉
            write_total(conn, "a", 0.0, at("2026-11-26", 10, 0));
            write_total(conn, "a", 30.0, at("2026-11-26", 10, 10));
            assert_eq!(refresh_daily_rollup(conn, now).unwrap(), 1);
            // 之后有一条 26 日的样本落进来（时钟被往回拨过了零点）
            write_total(conn, "b", 0.0, at("2026-11-26", 11, 0));
            write_total(conn, "b", 7.0, at("2026-11-26", 11, 10));
            assert!(rolled_days(conn).is_empty(), "26 日的「已汇总」标记被撤掉，等着重算");
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-26"), now);
            assert_eq!(out["totalCreditUsed"], 37.0, "重算后 b 的 7 分回来了");
            assert_eq!(rolled_days(conn), vec!["2026-11-26".to_string()]);
            assert_eq!(daily_rows(conn).len(), 2);
        })
        .expect("库不可用");
    }

    #[test]
    fn registry_is_backfilled_from_rows_written_before_it_existed() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-backfill");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            seed_two_days(conn);
            // 抹成名册 / 汇总表出现之前的状态：只有明细（抽样之前的那版二进制写下的）
            conn.execute_batch("DELETE FROM account_credit_seen; DELETE FROM account_credit_daily; DELETE FROM account_credit_daily_days; DELETE FROM account_credit_meta;").unwrap();
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-27"), now);
            assert_eq!(out["totalCreditUsed"], 42.0, "老明细照样进报表");
            let first: Vec<(String, i64)> = conn
                .prepare("SELECT account_id, first_ts FROM account_credit_seen ORDER BY account_id")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|row| row.unwrap())
                .collect();
            assert_eq!(first, vec![("a".to_string(), at("2026-11-26", 10, 0)), ("b".to_string(), at("2026-11-26", 11, 0))]);
            let marked: bool = conn
                .query_row("SELECT 1 FROM account_credit_meta WHERE key = 'registry_backfilled'", [], |_| Ok(()))
                .is_ok();
            assert!(marked, "补登记做过之后库里留下标记");
        })
        .expect("库不可用");
    }

    #[test]
    fn registry_is_backfilled_even_when_a_write_comes_before_the_first_report() {
        // 复现 2026-11-27 上线时的顺序：旧二进制留下了明细、没有名册；新二进制先有一次落库
        // （它登记了自己），之后才有人读报表。第一版用「名册为空」判断补登记，这里名册
        // 已经有 1 行，b 就永远进不了名册。
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-backfill-after-write");
        let now = at("2026-11-27", 12, 0);
        db.with(|conn| {
            seed_two_days(conn);
            conn.execute_batch("DELETE FROM account_credit_seen; DELETE FROM account_credit_daily; DELETE FROM account_credit_daily_days; DELETE FROM account_credit_meta;").unwrap();
            write_total(conn, "a", 50.0, at("2026-11-27", 12, 0));
            // 这一次落库可能顺带触发一轮汇总（进程级的节拍，别的测试也在用），所以不断言
            // 此刻名册里有几行；要断言的是报表结果和名册最终齐全
            // a：昨天 30 + 今天 5 + 5；b：昨天 7
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-27"), now + 60_000);
            assert_eq!(out["totalCreditUsed"], 47.0, "b 的 7 分在：补登记不受「名册里已经有行」影响");
            let registered: i64 = conn.query_row("SELECT COUNT(*) FROM account_credit_seen", [], |row| row.get(0)).unwrap();
            assert_eq!(registered, 2);
            // 做过一次之后不再重做：把名册清空也不会再补（库里已经有标记）
            conn.execute_batch("DELETE FROM account_credit_seen").unwrap();
            read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-27"), now + 120_000);
            let registered: i64 = conn.query_row("SELECT COUNT(*) FROM account_credit_seen", [], |row| row.get(0)).unwrap();
            assert_eq!(registered, 0, "标记在，补登记不重复");
        })
        .expect("库不可用");
    }

    #[test]
    fn report_on_a_fresh_database_is_empty_not_an_error() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-fresh");
        db.with(|conn| {
            let out = read(conn, &BTreeMap::new(), day("2026-11-26"), day("2026-11-27"), at("2026-11-27", 12, 0));
            assert_eq!(out["accounts"].as_array().unwrap().len(), 0);
            assert!(out["firstSnapshotAt"].is_null());
            assert!(table_exists(conn), "表随第一次读取建出来");
            assert_eq!(refresh_daily_rollup(conn, at("2026-11-27", 12, 0)).unwrap(), 0);
        })
        .expect("库不可用");
    }

    #[test]
    fn rollup_on_the_write_path_runs_only_when_the_interval_has_passed() {
        let (db, _guard) = crate::server::db::test_temp::TempDb::open("credit-snapshots-gate");
        db.with(|conn| {
            seed_two_days(conn);
            let now = at("2026-11-27", 12, 0);
            let next = AtomicI64::new(0);
            // 还没到下一次检查的时刻：不汇总
            next.store(now + 1, Ordering::Relaxed);
            rollup_if_due(conn, now, &next);
            assert!(rolled_days(conn).is_empty());
            assert_eq!(next.load(Ordering::Relaxed), now + 1, "没到点不改节拍");
            // 到点了：汇总，并把下一次检查推到一个间隔之后
            rollup_if_due(conn, now + 1, &next);
            assert_eq!(rolled_days(conn), vec!["2026-11-26".to_string()]);
            assert_eq!(next.load(Ordering::Relaxed), now + 1 + ROLLUP_INTERVAL_MS);
            // 间隔内的写入不再触发：拿「27 日也过完了」的时刻去试，它不该被汇总
            rollup_if_due(conn, now + ROLLUP_INTERVAL_MS, &next);
            assert_eq!(rolled_days(conn).len(), 1);
        })
        .expect("库不可用");
    }
}
