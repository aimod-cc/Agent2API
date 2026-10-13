//! 每账号的**余额查询记录**（`account_usage_records` 表）与选路用的**内存事实表**。
//!
//! ── 这个模块解决什么问题 ─────────────────────────────────────
//! 全局「定时查询积分」退役后，余额查询按账号各自到期触发（`core::usage_query`
//! 的心跳循环）。每账号一条记录（一账号一行，主键即账号 id，重复写入天然是
//! UPSERT）取代旧 kv 快照成为「这个账号最近一次查到多少」的权威存放处：
//!   - **逐条更新**：某个账号到点只写它自己那一行，不再整份快照读改写；
//!   - **选路零 IO**：转发选路（余额不足跳过）每条请求都要比一次余额，
//!     内存里那份 `BalanceFact`（账号 id → 最近成功读数的数字）让它变成
//!     纳秒级的查表 —— 从 JSON 现场解析或整份快照读库都撑不住这个频率。
//!
//! ── 失败保留上次成功的数值（OmniProxy 同一取舍）──────────────
//! `remaining` 只在查询**成功**时覆写：失败行保留上次成功的数字，欠费状态
//! 不会因为「这次查询失败」被放行（避免「查询失败 → 放行 → 402」的窗口期）；
//! 恢复需要查询成功且数值回到阈值之上。失败本身的展示信息在 `error` / `code`
//! 两列，与数值互不覆盖。首次失败（此前从未成功过）没有可保留的数值，
//! `remaining` 为 NULL —— 选路判定「判不出」一律放行，不猜。
//!
//! ── 请求发出**之前**就记一笔尝试 ─────────────────────────────
//! 心跳路径在发出查询前先 `mark_attempt`（与 `core::task_state` 的 claim 同一
//! 硬约束）：「请求途中被杀」不会留下「没跑过」的状态，下一轮仍按间隔来，
//! 不会因崩溃循环把上游打成高频。代价是「失败的那一次也占用了本轮排期」，
//! 而这正是要的（失败不退避、照常按间隔重试的用户决策）。
//!
//! ── 双实例共用同一个库 ──────────────────────────────────────
//! 桌面壳允许开发版与正式版并存。本模块没有跨进程占位（余额查询是只读的，
//! 两个进程偶尔各查一次的代价可以忽略），但 `mark_attempt` 先行的写法把
//! 窗口压到「两次读库之间」，实际几乎不会撞上。
//!
//! ── 硬约束：持锁（连接锁）期间不打日志 ──────────────────────
//! 与 `core::task_state` 同一条：`Db::with*` 拿的是全局唯一的连接锁，而
//! `logging::log` 要往同一个库写 `logs` 表 —— 闭包里打日志会当场死锁。
//! 日志一律在闭包之外打。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use crate::server::core::account_store::store_util::js_truthy;
use crate::server::db::Db;
use crate::server::logging;

/// 旧 kv 快照键（全局「定时查询积分」时代的存放处）。
/// 保留在 `db::schema::RESERVED_KV_KEYS` 里：迁移中断（导入失败不删键）时，
/// 配置写入的「删除已不存在的键」不能把它误清 —— 那是唯一还能重试的来源。
const LEGACY_SNAPSHOT_KEY: &str = "usageQuerySnapshot";

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 余额事实：选路跳过判定读的**内存副本**（账号 id → 最近一次成功读数）。
#[derive(Clone, Copy, Debug)]
pub struct BalanceFact {
    /// 最近一次成功查询的余额数字（与余额列同一口径）。`None` = 判不出
    /// （unlimited、无读数、形状不认）—— 跳过判定一律放行。
    pub remaining: Option<f64>,
}

fn facts_slot() -> &'static Mutex<HashMap<String, BalanceFact>> {
    static FACTS: OnceLock<Mutex<HashMap<String, BalanceFact>>> = OnceLock::new();
    FACTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 一条余额查询记录（读侧形状；行不存在时各字段取缺省值）。
#[derive(Clone, Debug)]
pub struct UsageRecord {
    /// 最近一次**成功**查询的归一化余额 JSON（余额列的渲染原料）。
    pub usage: Option<Value>,
    /// 最近一次失败的原因（成功行清空）。
    pub error: Option<String>,
    /// 失败的机器可识别标记（如「未配置查询凭证」）。
    pub code: Option<String>,
    /// 数值投影（成功时从 usage 提出；失败保留上次成功值；判不出为 NULL）。
    pub remaining: Option<f64>,
    pub unlimited: bool,
    /// 最近一次成功 / 尝试的时刻（毫秒；0 = 从未）。
    pub last_success_at: i64,
    pub last_attempt_at: i64,
}

/// 一次查询的**结果**（成功带 usage，失败带 error；两者都不给 = 行为未定义，
/// 调用方保证二选一）。
pub struct UsageOutcome {
    pub usage: Option<Value>,
    pub error: Option<String>,
    pub code: Option<String>,
}

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
    migrate_legacy_snapshot();
    reload_facts();
}

fn database() -> Option<&'static Db> {
    DB.get().and_then(Option::as_ref)
}

// ─── 读 ──────────────────────────────────────────────────────

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    let usage_text: Option<String> = row.get("usage")?;
    Ok(UsageRecord {
        usage: usage_text.and_then(|text| serde_json::from_str(&text).ok()),
        error: row.get("error")?,
        code: row.get("code")?,
        remaining: row.get::<_, Option<f64>>("remaining")?,
        unlimited: row.get::<_, i64>("unlimited").unwrap_or(0) != 0,
        last_success_at: row.get("last_success_at").unwrap_or(0),
        last_attempt_at: row.get("last_attempt_at").unwrap_or(0),
    })
}

const RECORD_COLUMNS: &str =
    "usage, error, code, remaining, unlimited, last_success_at, last_attempt_at";

/// 某账号的最近一条记录（没有 = 全缺省：从未查过）。
pub fn load(account_id: &str) -> UsageRecord {
    let fallback = UsageRecord {
        usage: None,
        error: None,
        code: None,
        remaining: None,
        unlimited: false,
        last_success_at: 0,
        last_attempt_at: 0,
    };
    let Some(db) = database() else { return fallback };
    db.with(|conn| {
        conn.query_row(
            &format!(
                "SELECT {RECORD_COLUMNS} FROM account_usage_records WHERE account_id = ?1"
            ),
            [account_id],
            row_to_record,
        )
        .optional()
        .unwrap_or_default()
    })
    .flatten()
    .unwrap_or(fallback)
}

/// 全部记录（快照接口的组装原料）。库不可用时给空表 —— 上层按「没查过」降级。
pub fn load_all() -> HashMap<String, UsageRecord> {
    let Some(db) = database() else { return HashMap::new() };
    db.with(|conn| {
        let Ok(mut statement) = conn.prepare(&format!(
            "SELECT account_id, {RECORD_COLUMNS} FROM account_usage_records"
        )) else {
            return HashMap::new();
        };
        let rows = statement.query_map([], |row| {
            let account_id: String = row.get("account_id")?;
            Ok((account_id, row_to_record(row)?))
        });
        let Ok(rows) = rows else { return HashMap::new() };
        rows.filter_map(|item| item.ok()).collect::<HashMap<_, _>>()
    })
    .unwrap_or_default()
}

/// 选路用的余额事实快照（内存查表，调用方克隆一份小 Map）。
pub fn balance_facts() -> HashMap<String, BalanceFact> {
    facts_slot()
        .lock()
        .map(|facts| facts.clone())
        .unwrap_or_default()
}

// ─── 写 ──────────────────────────────────────────────────────

/// 记一笔「尝试开始」（心跳路径在**发请求之前**调用，见模块头）。
/// 只推进时间戳，不动上一次的结果 —— 查询还没回来，旧读数仍是最新事实。
pub fn mark_attempt(account_id: &str) {
    let now = logging::now_ms();
    let Some(db) = database() else { return };
    let failed = db.with_mut(|conn| {
        conn.execute(
            "INSERT INTO account_usage_records (account_id, last_attempt_at, updated_at)
             VALUES (?1, ?2, ?2)
             ON CONFLICT(account_id) DO UPDATE SET
               last_attempt_at = excluded.last_attempt_at,
               updated_at = excluded.updated_at",
            params![account_id, now],
        )
        .is_err()
    });
    if failed != Some(false) {
        logging::verbose("[Usage]", &format!("账号 {account_id} 余额查询占位失败（库里没记上本次尝试）"));
    }
}

/// 写入一次查询的结果（成功覆写数值，失败只写原因 —— 见模块头的取舍）。
/// 内存事实表同步更新：成功且有数字 → 刷新；成功但判不出（unlimited / 形状不认）
/// → 摘除（不能让一个过期的旧数字把账号永久挡在选路之外）。
pub fn write_result(account_id: &str, outcome: &UsageOutcome) {
    let now = logging::now_ms();
    let Some(db) = database() else { return };
    // 闭包返回 `(是否成功, 成功时提出的数字)`：失败分支只写原因、保留旧数值
    // （SQL 里就不碰 usage / remaining / last_success 三列），事实表按它更新。
    // `with_mut` 本身再包一层 Option（锁中毒 = None），match 前先 flatten。
    let written = db.with_mut(|conn| -> Option<(bool, Option<f64>)> {
        match outcome.usage.as_ref() {
            Some(usage) => {
                let (remaining, unlimited) = extract_remaining(usage);
                let usage_text = serde_json::to_string(usage).ok()?;
                conn.execute(
                    "INSERT INTO account_usage_records
                       (account_id, usage, error, code, remaining, unlimited,
                        last_success_at, last_attempt_at, updated_at)
                     VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?5, ?5)
                     ON CONFLICT(account_id) DO UPDATE SET
                       usage = excluded.usage,
                       error = NULL,
                       code = NULL,
                       remaining = excluded.remaining,
                       unlimited = excluded.unlimited,
                       last_success_at = excluded.last_success_at,
                       last_attempt_at = excluded.last_attempt_at,
                       updated_at = excluded.updated_at",
                    params![account_id, usage_text, remaining, unlimited as i64, now],
                )
                .ok()?;
                Some((true, remaining))
            }
            None => {
                let error = outcome.error.clone().unwrap_or_else(|| "查询失败".to_string());
                conn.execute(
                    "INSERT INTO account_usage_records
                       (account_id, error, code, last_attempt_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)
                     ON CONFLICT(account_id) DO UPDATE SET
                       error = excluded.error,
                       code = excluded.code,
                       last_attempt_at = excluded.last_attempt_at,
                       updated_at = excluded.updated_at",
                    params![account_id, error, outcome.code, now],
                )
                .ok()?;
                Some((false, None))
            }
        }
    })
    .flatten();
    match written {
        // 成功且判出了数字 → 刷新事实
        Some((true, Some(remaining))) => {
            if let Ok(mut facts) = facts_slot().lock() {
                facts.insert(account_id.to_string(), BalanceFact { remaining: Some(remaining) });
            }
        }
        // 成功但判不出 → 摘除旧事实（unlimited / 形状变化后不再被旧数字挡路）
        Some((true, None)) => {
            if let Ok(mut facts) = facts_slot().lock() {
                facts.remove(account_id);
            }
        }
        // 失败（或库不可用）→ 事实保持不动（上次成功的数字继续生效，见模块头）
        _ => {}
    }
}

/// 清掉「账号已被删除」的孤儿行（心跳循环每轮顺手一次，一条 SQL）。
pub fn prune_orphans() {
    let Some(db) = database() else { return };
    db.with_mut(|conn| {
        conn.execute(
            "DELETE FROM account_usage_records
             WHERE account_id NOT IN (SELECT id FROM accounts)",
            [],
        )
        .ok()
    });
}

// ─── 余额数字的提取口径（与余额列同源）─────────────────────

/// 归一化余额 JSON → `(剩余数字, unlimited)`。
///
/// 口径与前端余额列逐字同源（`accounts-panels.tsx` 的 `usageSummary`）：
///   - workbuddy 既有形状认 `totalLeft` 键，`unlimited` 真值 = ∞（判不出数字）；
///   - 归一化形状认 `available`；
///   - 两者都判不出（字段缺失 / 非数字）→ `None`，选路按「无法判定」放行。
/// 「按字段形状探测而不是按 provider 分派」的理由与前端同一句：provider 只
/// 决定谁去查，不决定查回来长什么样。
pub fn extract_remaining(usage: &Value) -> (Option<f64>, bool) {
    let Some(fields) = usage.as_object() else { return (None, false) };
    let unlimited = fields.get("unlimited").map(js_truthy).unwrap_or(false);
    if unlimited {
        return (None, true);
    }
    if let Some(total_left) = fields.get("totalLeft") {
        return (total_left.as_f64().filter(|value| value.is_finite()), false);
    }
    if let Some(available) = fields.get("available") {
        return (available.as_f64().filter(|value| value.is_finite()), false);
    }
    (None, false)
}

// ─── 账号上的查询配置读取（调度与选路共用的判定口径）─────────

/// 每账号自动查询的间隔上下限（秒）：30 秒 ~ 1 天。写入侧
/// （`account_store::apply_patch`）与读取侧共用这一对常量 —— 两处各写一份
/// 「迟早会漂」。
pub const MIN_QUERY_INTERVAL_SECONDS: i64 = 30;
pub const MAX_QUERY_INTERVAL_SECONDS: i64 = 86_400;

/// 账号记录上**没有** `usageQuery` / `lowBalance` 配置时的缺省口径。
///
/// 「缺省」必须与全局任务时代的行为对齐：那时「定时查询积分」默认开启
/// （每 10 分钟），账号什么都不配也在查；因此每账号化的缺省同样是**开启**
/// （间隔取 1 分钟 —— 用户升级反馈指定的值），而不是「未配置 = 不查」——
/// 否则升级后所有人的余额列都会静默停更。
///
/// 余额不足的缺省处理按 provider 区分（见 [`default_low_balance_mode`]）：
/// 大多数家沿用跳过（阈值 1）—— 全局任务时代没有这个概念，它是最温和的
/// 兜底；唯独 Cline 免费池缺省**不处理**，原因见那边的文档。
pub const DEFAULT_QUERY_INTERVAL_SECONDS: i64 = 60;
pub const DEFAULT_LOW_BALANCE_THRESHOLD: f64 = 1.0;

/// Cline 免费池的 provider id（从注册表推导，与 `account_store::CLINE_FREE_PROVIDER_ID`
/// 同源 —— 注册表改了 id 这里跟着变）。
const CLINE_FREE_PROVIDER: &str = crate::server::core::providers::kind_id(
    crate::server::core::providers::ProviderKind::ClineFree,
);

/// CodeArts 的 provider id（缺省「不处理」，真正的缺省限制是 Token 规则，见上）。
const CODEARTS_PROVIDER: &str = crate::server::core::providers::kind_id(
    crate::server::core::providers::ProviderKind::CodeArts,
);

/// 各 provider 缺省的「余额不足处理」档（记录上没有 `lowBalance` 配置时用）。
///
/// 这个函数只描述**旧 `lowBalance` 字段**的缺省视图；限制器上线后真正的缺省
/// 规则（含 CodeArts 的 Token 自然日规则）在 `limiter::default_rules`。
///
/// - **Cline 免费池（`cline-free`）→ `off`（不处理）**：免费池的 credit 长期
///   贴着 0 走、用超了还是负数（欠费是常态），跳过档的缺省阈值 1 会把几乎
///   整个池从选路里剔掉。免费池能不能用交给上游裁决（真没钱时 402 自会按
///   错误处置轮换），不靠余额读数预判。
/// - **CodeArts（`codearts`）→ `off`**：它的有效缺省限制是一条 Token 自然日
///   规则（见 `limiter::default_rules`，余额读数常判不出、余额规则形同虚设）；
///   旧字段表达不了 Token 规则，显示 off 比显示一个不会生效的「跳过」诚实。
/// - **其余 provider → `skip`（跳过）**：没钱让路、有钱照常、余额回升自动
///   恢复，不需要任何人善后（历史缺省，不变）。
pub fn default_low_balance_mode(provider: &str) -> &'static str {
    if provider == CLINE_FREE_PROVIDER || provider == CODEARTS_PROVIDER {
        "off"
    } else {
        "skip"
    }
}

/// 缺省档的完整形状（mode + threshold）：公开形态（store_view）与写入侧
/// 归一化（store_crud）共用，保证三处「缺省」永远是同一个对象。
/// off 档阈值归零存放（与 `apply_patch` 的 off 形态一致）。
pub fn default_low_balance(provider: &str) -> Value {
    if default_low_balance_mode(provider) == "off" {
        serde_json::json!({ "mode": "off", "threshold": 0.0 })
    } else {
        serde_json::json!({ "mode": "skip", "threshold": DEFAULT_LOW_BALANCE_THRESHOLD })
    }
}

/// 账号配置的自动查询间隔（秒）。
///
/// 未配置 → 缺省开启（1 分钟）；显式 `{enabled:false}` → `None`（用户关的
/// 必须尊重）；开着但间隔缺失 / 越界（DB 里的手工脏值）→ 按缺省间隔跑，
/// 不因为一个坏数字把整个账号的自动查询停掉。
pub fn query_interval_of(account: &Value) -> Option<i64> {
    let Some(config) = account.get("usageQuery") else {
        return Some(DEFAULT_QUERY_INTERVAL_SECONDS);
    };
    if config.get("enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    match config.get("interval").and_then(Value::as_i64) {
        Some(interval)
            if (MIN_QUERY_INTERVAL_SECONDS..=MAX_QUERY_INTERVAL_SECONDS).contains(&interval) =>
        {
            Some(interval)
        }
        _ => Some(DEFAULT_QUERY_INTERVAL_SECONDS),
    }
}

/// 「余额不足自动禁用」档的阈值。判定读**限制器的有效规则**（`core::limiter`）：
/// 记录上有显式 `limiters` 时按其中的余额 disable 规则取最大阈值；没有时按旧
/// `lowBalance` / provider 缺省推导 —— 与升级前逐字一致。**缺省（推导结果为空）
/// 不启用禁用**：自动禁用是不自动恢复的硬动作，缺省必须是用户显式选过才会发生。
/// 阈值非法（非正 / 非有限数）在规则解析层已拦，这里不再重复判。
pub fn low_balance_disable_threshold(account: &Value) -> Option<f64> {
    crate::server::core::limiter::balance_disable_threshold(account)
}

/// 账号是否应因「余额不足」在选路时被跳过（软跳过档）。
///
/// 判定读**限制器的有效规则**（`core::limiter::balance_skip_blocked`）：任一
/// 启用的余额 skip 规则满足「最近读数**严格小于**阈值（等于阈值仍可用，与
/// OmniProxy 同口径）」即拦。缺省（无 `limiters` 键）按旧配置推导：Cline 免费池
/// 不处理（free 池余额贴 0 是常态，跳过会把整个池剔掉），其余 skip、阈值 1。
/// 判不出的情况 —— 无读数、unlimited、账号已删 —— 一律放行：
/// 跳过是对「这个账号此刻没钱」的断言，断言拿不出证据就不能拦请求。
pub fn balance_blocked(account: &Value, facts: &HashMap<String, BalanceFact>) -> bool {
    crate::server::core::limiter::balance_skip_blocked(account, facts)
}

// ─── 旧 kv 快照的一次性迁移 ─────────────────────────────────

/// 全局任务时代的快照（kv `usageQuerySnapshot`）导入本表后删除该键。
///
/// 为什么在启动时做：升级后余额列不能变空白 —— 旧快照里的每行结论（含失败行）
/// 仍是「那次查询的事实」，导入后界面照常按「上次读数」展示，直到各自的
/// 下一次到期查询把它们逐个刷新。
///
/// 原子性：读键、逐行导入、删键在一个事务里完成，中断则整体回滚、下次启动
/// 重试。失败只记 verbose（旧键还在，数据没丢），不阻断启动。
fn migrate_legacy_snapshot() {
    let Some(db) = database() else { return };
    let imported = db.with_mut(|conn| -> Result<usize, String> {
        let text: Option<String> = conn
            .query_row(
                "SELECT value FROM kv WHERE key = ?1",
                [LEGACY_SNAPSHOT_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(text) = text else { return Ok(0) };
        let snapshot: Value = serde_json::from_str(&text)
            .map_err(|error| format!("旧余额快照格式错误: {error}"))?;
        let at = snapshot.get("at").and_then(Value::as_i64).unwrap_or(0);
        let rows = snapshot
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut imported = 0usize;
        for row in &rows {
            let Some(id) = row.get("id").and_then(Value::as_str) else { continue };
            if id.is_empty() {
                continue;
            }
            let usage = row.get("usage").filter(|value| !value.is_null()).cloned();
            match usage {
                Some(usage) => {
                    let (remaining, unlimited) = extract_remaining(&usage);
                    let usage_text = match serde_json::to_string(&usage) {
                        Ok(text) => text,
                        Err(_) => continue,
                    };
                    conn.execute(
                        "INSERT INTO account_usage_records
                           (account_id, usage, error, code, remaining, unlimited,
                            last_success_at, last_attempt_at, updated_at)
                         VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?5, ?5)
                         ON CONFLICT(account_id) DO UPDATE SET
                           usage = excluded.usage,
                           error = NULL,
                           code = NULL,
                           remaining = excluded.remaining,
                           unlimited = excluded.unlimited,
                           last_success_at = excluded.last_success_at,
                           last_attempt_at = excluded.last_attempt_at,
                           updated_at = excluded.updated_at",
                        params![id, usage_text, remaining, unlimited as i64, at],
                    )
                    .map_err(|error| error.to_string())?;
                }
                None => {
                    let error = row
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("查询失败")
                        .to_string();
                    let code = row.get("code").and_then(Value::as_str);
                    conn.execute(
                        "INSERT INTO account_usage_records
                           (account_id, error, code, last_attempt_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?4)
                         ON CONFLICT(account_id) DO UPDATE SET
                           error = excluded.error,
                           code = excluded.code,
                           last_attempt_at = excluded.last_attempt_at,
                           updated_at = excluded.updated_at",
                        params![id, error, code, at],
                    )
                    .map_err(|error| error.to_string())?;
                }
            }
            imported += 1;
        }
        conn.execute("DELETE FROM kv WHERE key = ?1", [LEGACY_SNAPSHOT_KEY])
            .map_err(|error| error.to_string())?;
        Ok(imported)
    });
    match imported {
        Some(Ok(0)) => { /* 没有旧快照（全新安装或已迁移）：无事可做 */ }
        Some(Ok(count)) => {
            logging::log("[Usage]", &format!("已把旧版余额快照的 {count} 条结果迁入新记录表"));
        }
        other => {
            logging::verbose(
                "[Usage]",
                &format!("旧余额快照迁移未完成（下次启动重试）：{other:?}"),
            );
        }
    }
}

/// 启动时把表里的成功读数装进内存事实表（选路的初始视野）。
fn reload_facts() {
    let facts: HashMap<String, BalanceFact> = load_all()
        .into_iter()
        .filter(|(_, record)| record.usage.is_some())
        .filter_map(|(id, record)| {
            record
                .remaining
                .map(|remaining| (id, BalanceFact { remaining: Some(remaining) }))
        })
        .collect();
    if let Ok(mut slot) = facts_slot().lock() {
        *slot = facts;
    }
}

