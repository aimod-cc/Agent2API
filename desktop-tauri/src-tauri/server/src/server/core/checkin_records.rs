//! 签到记录的**每账号明细台账**（`checkin_records` 表，schema v11）。
//!
//! ── 解决什么问题 ────────────────────────────────────────────
//! 改造前「签到到底领到了什么」只有两个落点：config.json 的 `lastResult`
//! （一条聚合快照，下一轮就覆盖）与运行日志（文本、翻页即逝）。签到中心的
//! 「签到记录」页要的是**可翻阅的明细**：哪个账号、哪天、什么来源触发、
//! 领成功还是没领、失败错误是什么、领了多少（收益文案）—— 这张表就是那个落点。
//!
//! ── 来源（source）的字面集合 ─────────────────────────────────
//! `schedule` 定时条目轮次 / `startup` 启动补签 / `manual` 手动批量与条目
//! 立即执行 / `single` 单账号签到 / `activity` 国际版活动的手动领取与保活。
//! 前四种都从 `run_checkin` 的唯一记账点进来（`source` 参数原样透传），
//! `activity` 从 `api::billing::run_activity` 进来（那条路径不走 run_checkin，
//! 但行的形状同构，见 `record_activity`）。
//!
//! ── 结果判据只此一份 ────────────────────────────────────────
//! outcome 的判定复用签到执行体的同一对函数
//! （`billing::checkin::{checkin_completed_today, claim_pending}`）——
//! 「今天已完成」「等窗口重试」的口径在台账与调度（`auto_checkin::tick`）
//! 之间必须一致，各写一份迟早分叉。
//!
//! ── 写失败不报错 ────────────────────────────────────────────
//! 记账点在签到响应路径上，上游积分已经领到 —— 台账缺一条不该让那次签到
//! 报错（与 `run_checkin` 里 `mark_checkin` 的取舍同构）。失败打一行
//! **verbose** 日志就完。
//!
//! ── 硬约束：持锁（连接锁）期间不打日志 ──────────────────────
//! 与 `core::usage_records` 同一条：`Db::with*` 拿的是全局唯一的连接锁，
//! `logging::log` 要往同一个库写 `logs` 表 —— 闭包里打日志会当场死锁。

use std::sync::OnceLock;

use rusqlite::params;
use serde_json::{json, Value};

use crate::server::core::beijing;
use crate::server::core::billing::checkin::{checkin_completed_today, claim_pending};
use crate::server::db::Db;
use crate::server::logging;

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 结果档位（`outcome` 列的字面集合）
pub const OUTCOME_SUCCESS: &str = "success";
pub const OUTCOME_ALREADY: &str = "already";
pub const OUTCOME_PENDING: &str = "pending";
pub const OUTCOME_FAILED: &str = "failed";

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Option<&'static Db> {
    DB.get().and_then(Option::as_ref)
}

// ─── 记账 ────────────────────────────────────────────────────

/// 行的 `activity` 段是否携带「活跃上报成功」读数（`report.succeeded`）。
///
/// 国际版每日活跃的三件事里，**只有活跃上报点亮连登**（`/v2/report` 实测，
/// 见 `billing::activity::report_chat_activity`）；领取依赖上游活动开放、
/// 保活对话不参与活跃计数。「活动未开启」轮次的成败就认这一格 —— 与
/// `claim_pending` 只认中性文案是同一条克制原则：上报失败、网络错误都照旧
/// 落 failed，不会把真故障美化成成功。
fn activity_report_ok(row: &Value) -> bool {
    row.get("activity")
        .and_then(|activity| activity.get("report"))
        .and_then(|report| report.get("succeeded"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 签到结果行的记账（`run_checkin` 的每账号行，`{id, name, claim, activity?,
/// error}` 形状）。`provider` 由调用方从账号记录带上（结果行里没有这个字段，
/// 账号页的过滤列要用它）。
///
/// outcome 判定（只此一份，见模块头）：
///   - 行的 `error` 非空 → **failed**（执行出错：网络 / 凭证 / 未接入）；
///   - claim 判「今日已完成」→ success（真领到）或 already（上游说已领过）；
///   - claim 判「中性未领取」→ **pending**（此刻没有可领的活动，等窗口）；
///   - 国际版每日活跃「活动未开启但活跃上报成功」→ **already** —— 活动没开
///     是上游运营态（谁也领不到），而活跃上报已把连登点亮，这一天的「日活」
///     实际完成了；记 failed 会把每天的正常轮次刷成一片红（改造前的实况）。
///   - 其余 → **failed**，错误原文取行的 `error`，缺了取 claim 的 `msg`
///     （claim.success=false 且带文案的失败，如「登录态已过期」）。
pub fn record_row(row: &Value, provider: &str, source: &str, reason: &str) {
    let claim = row.get("claim").cloned().unwrap_or(Value::Null);
    let row_error = row
        .get("error")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string);
    let (outcome, error) = if let Some(text) = row_error {
        (OUTCOME_FAILED, Some(text))
    } else if checkin_completed_today(&claim) {
        let outcome = if claim.get("success").and_then(Value::as_bool) == Some(true) {
            OUTCOME_SUCCESS
        } else {
            OUTCOME_ALREADY
        };
        (outcome, None)
    } else if claim_pending(&claim) {
        (OUTCOME_PENDING, None)
    } else if activity_report_ok(row) {
        (OUTCOME_ALREADY, None)
    } else {
        // 失败且行上没有 error：失败文案在 claim.msg 里（claim_result 的
        // Err 分支已经把 message 放进行 error，这里兜的是直接给出
        // success=false claim 的实现，如 workbuddy-intl）
        let message = claim
            .get("msg")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string);
        (OUTCOME_FAILED, message)
    };
    let kept_active = row
        .get("activity")
        .and_then(|activity| activity.get("pokeSucceeded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    insert(Entry {
        account_id: text_or(row.get("id"), ""),
        account_name: text_or(row.get("name"), &text_or(row.get("id"), "")),
        provider: provider.to_string(),
        source,
        reason,
        outcome,
        ok: outcome == OUTCOME_SUCCESS || outcome == OUTCOME_ALREADY,
        message: claim
            .get("msg")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        error,
        kept_active,
    });
}

/// 国际版活动的手动领取 / 保活（`api::billing::run_activity` 的返回行，
/// `{id, name, mode, claim, activity}`）。
///
/// 判定按 mode 分两条：`keepalive` 看「上报成功或保活成功」任一 —— 它的
/// claim 是 `{success:false, msg:"活跃保活完成"}` 的形态，走 claim 判据会被
/// 误判成 failed，而这一颗按钮的本意就是「维持账号活跃」，活跃上报（点亮
/// 连登的那条通道）成功同样算数；`full` / `claim` 与签到同判据（claim），
/// 「活动未开启但活跃上报成功」按 `record_row` 同款落到 already。
/// reason 用人话（界面的「来源/说明」列直接展示）。
pub fn record_activity(row: &Value, provider: &str, mode: &str) {
    let reason = match mode {
        "keepalive" => "手动保活",
        "claim" => "手动领取活动",
        _ => "手动活动",
    };
    let claim = row.get("claim").cloned().unwrap_or(Value::Null);
    let kept_active = row
        .get("activity")
        .and_then(|activity| activity.get("pokeSucceeded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let report_ok = activity_report_ok(row);
    let (outcome, error, ok) = if mode == "keepalive" {
        match kept_active || report_ok {
            true => (OUTCOME_SUCCESS, None, true),
            false => (
                OUTCOME_FAILED,
                Some(
                    claim
                        .get("msg")
                        .and_then(Value::as_str)
                        .unwrap_or("活跃保活失败")
                        .to_string(),
                ),
                false,
            ),
        }
    } else if checkin_completed_today(&claim) {
        let outcome = if claim.get("success").and_then(Value::as_bool) == Some(true) {
            OUTCOME_SUCCESS
        } else {
            OUTCOME_ALREADY
        };
        (outcome, None, true)
    } else if claim_pending(&claim) {
        (OUTCOME_PENDING, None, false)
    } else if report_ok {
        // full / claim 模式「活动未开启但活跃上报成功」：与 record_row 同款，
        // 这一天的国际版日活实际完成了，不落 failed。
        (OUTCOME_ALREADY, None, true)
    } else {
        (
            OUTCOME_FAILED,
            claim
                .get("msg")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_string),
            false,
        )
    };
    insert(Entry {
        account_id: text_or(row.get("id"), ""),
        account_name: text_or(row.get("name"), &text_or(row.get("id"), "")),
        provider: provider.to_string(),
        source: "activity",
        reason,
        outcome,
        ok,
        message: claim
            .get("msg")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        error,
        kept_active,
    });
}

/// 一行明细的落库形态（`insert` 的入参；字段与表列一一对应）
struct Entry<'a> {
    account_id: String,
    account_name: String,
    provider: String,
    source: &'a str,
    reason: &'a str,
    outcome: &'a str,
    ok: bool,
    message: Option<String>,
    error: Option<String>,
    kept_active: bool,
}

/// 落一行台账（一次事务）。失败打 verbose，不向上报
/// （见模块头「写失败不报错」）。
///
/// 台账**全量保留**：不按天数裁、也没有条数上限（与请求日志同一取向）——
/// 要清历史用签到记录页自己的「清空 / 按筛选删除」（`clear_all` /
/// `clear_where`）。这里原本会顺带按「请求日志保留天数」裁超期行，
/// 那套保留期已随请求日志一起移除。
fn insert(entry: Entry<'_>) {
    let Some(db) = database() else { return };
    let ts = logging::now_ms();
    let date = beijing::today_key();
    let failed = db
        .with_mut(|conn| -> Result<(), rusqlite::Error> {
            conn.execute(
                "INSERT INTO checkin_records
                   (ts, date, source, reason, account_id, account_name, provider,
                    outcome, ok, message, error, kept_active)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    ts,
                    date,
                    entry.source,
                    entry.reason,
                    entry.account_id,
                    entry.account_name,
                    entry.provider,
                    entry.outcome,
                    entry.ok as i64,
                    entry.message,
                    entry.error,
                    entry.kept_active as i64,
                ],
            )?;
            Ok(())
        })
        .map(|result| result.is_err())
        .unwrap_or(true);
    if failed {
        logging::verbose("[Checkin]", "签到记录未能落库（库不可用或写入失败）");
    }
}

/// 结果行的字符串字段读取（非字符串给缺省）
fn text_or(value: Option<&Value>, fallback: &str) -> String {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

// ─── 查询与删除 ──────────────────────────────────────────────

/// 查询条件（GET 的查询串与 DELETE 的护栏共用一套字段）
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub outcome: Option<String>,
    pub provider: Option<String>,
    pub keyword: Option<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl Query {
    /// 是否带了任一筛选条件（DELETE 的护栏判据：一个都没带 = 全清）
    pub fn has_filters(&self) -> bool {
        self.outcome.as_deref().is_some_and(|text| !text.is_empty())
            || self.provider.as_deref().is_some_and(|text| !text.is_empty())
            || self.keyword.as_deref().is_some_and(|text| !text.is_empty())
            || self.start.is_some()
            || self.end.is_some()
    }

    /// 拼 WHERE 子句与参数（查询与删除共用同一份过滤链 —— 「筛出什么就删什么」）
    fn clause(&self) -> (String, Vec<Value>) {
        let mut conditions: Vec<String> = Vec::new();
        let mut params_json: Vec<Value> = Vec::new();
        if let Some(outcome) = self.outcome.as_deref().filter(|text| !text.is_empty()) {
            conditions.push("outcome = ?".to_string());
            params_json.push(json!(outcome));
        }
        if let Some(provider) = self.provider.as_deref().filter(|text| !text.is_empty()) {
            conditions.push("provider = ?".to_string());
            params_json.push(json!(provider));
        }
        if let Some(keyword) = self.keyword.as_deref().filter(|text| !text.is_empty()) {
            // 关键词扫账号名 / 收益文案 / 错误原文 / 触发原因四个可读字段
            let pattern = format!("%{}%", keyword.replace('%', "\\%").replace('_', "\\_"));
            conditions.push(
                "(account_name LIKE ? ESCAPE '\\'
                  OR message LIKE ? ESCAPE '\\'
                  OR error LIKE ? ESCAPE '\\'
                  OR reason LIKE ? ESCAPE '\\')"
                    .to_string(),
            );
            for _ in 0..4 {
                params_json.push(json!(pattern));
            }
        }
        if let Some(start) = self.start {
            conditions.push("ts >= ?".to_string());
            params_json.push(json!(start));
        }
        if let Some(end) = self.end {
            conditions.push("ts <= ?".to_string());
            params_json.push(json!(end));
        }
        if conditions.is_empty() {
            (String::new(), params_json)
        } else {
            (format!(" WHERE {}", conditions.join(" AND ")), params_json)
        }
    }
}

/// 查询一页明细（`offset` 真分页，`limit` 缺省 200、上限 1000）。
/// 响应形状 `{records: [...], total}`：total 是**同条件**的总数（分页器用），
/// records 按时间倒序（新的在前）。库不可用给空表（界面显示空态，不报错）。
pub fn query(query: &Query, limit: usize, offset: usize) -> Value {
    let limit = limit.clamp(1, 1000);
    let Some(db) = database() else {
        return json!({ "records": [], "total": 0 });
    };
    let (where_clause, filter_params) = query.clause();
    let sql_rows = format!(
        "SELECT row_id, ts, date, source, reason, account_id, account_name,
                provider, outcome, ok, message, error, kept_active
         FROM checkin_records{where_clause}
         ORDER BY ts DESC, row_id DESC
         LIMIT {limit} OFFSET {offset}"
    );
    let sql_count = format!("SELECT COUNT(*) FROM checkin_records{where_clause}");
    let loaded = db
        .with(|conn| -> Result<Value, rusqlite::Error> {
            let params_from_json = |values: &[Value]| -> Vec<String> {
                values.iter().map(value_to_text).collect()
            };
            let total: i64 = conn.query_row(
                &sql_count,
                rusqlite::params_from_iter(params_from_json(&filter_params)),
                |row| row.get(0),
            )?;
            let mut statement = conn.prepare(&sql_rows)?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(params_from_json(&filter_params)),
                |row| {
                    Ok(json!({
                        "rowId": row.get::<_, i64>("row_id")?,
                        "ts": row.get::<_, i64>("ts")?,
                        "date": row.get::<_, String>("date")?,
                        "source": row.get::<_, String>("source")?,
                        "reason": row.get::<_, String>("reason")?,
                        "accountId": row.get::<_, String>("account_id")?,
                        "accountName": row.get::<_, String>("account_name")?,
                        "provider": row.get::<_, String>("provider")?,
                        "outcome": row.get::<_, String>("outcome")?,
                        "ok": row.get::<_, i64>("ok").unwrap_or(0) != 0,
                        "message": row.get::<_, Option<String>>("message")?,
                        "error": row.get::<_, Option<String>>("error")?,
                        "keptActive": row.get::<_, i64>("kept_active").unwrap_or(0) != 0,
                    }))
                },
            )?;
            let records: Vec<Value> = rows.filter_map(|item| item.ok()).collect();
            Ok(json!({ "records": records, "total": total }))
        })
        .and_then(|result| result.ok())
        .unwrap_or_else(|| json!({ "records": [], "total": 0 }));
    loaded
}

/// 按筛选条件删除（「清空筛选结果」）；无筛选条件的调用**必须**由 API 层
/// 先过显式 `all=1` 护栏（照抄 logs 的规则，理由见 api::checkin_records）。
/// 返回删除条数；库不可用给 None（API 层给 500 信封）。
pub fn clear_where(query: &Query) -> Option<usize> {
    let db = database()?;
    let (where_clause, filter_params) = query.clause();
    let sql = format!("DELETE FROM checkin_records{where_clause}");
    let params_from_json = |values: &[Value]| -> Vec<String> {
        values.iter().map(value_to_text).collect()
    };
    db.with_mut(|conn| {
        conn.execute(&sql, rusqlite::params_from_iter(params_from_json(&filter_params)))
            .ok()
    })
    .flatten()
}

/// 清空全部（API 层护栏已过）。返回删除条数。
pub fn clear_all() -> Option<usize> {
    let db = database()?;
    db.with_mut(|conn| conn.execute("DELETE FROM checkin_records", []).ok()).flatten()
}

/// rusqlite 的参数都是 ToSql：这里统一转文本（数字 / 字符串都安全）
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
