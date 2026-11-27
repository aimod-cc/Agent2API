//! 网关 API Key 的用量限制配置、独立台账、窗口统计与请求准入。
//!
//! ── 设计取舍 ──────────────────────────────────────────────────
//! 1. **二选一指标**：每把 Key 可配置「Token 用量」或「请求次数」其中之一
//!    （`UsageLimitMetric`），按需填写每日 / 每周 / 每月（本地时区自然周期自动重置）
//!    与累计总量（不自动重置，可调高或手动清零）四项上限，留空即不限制。
//! 2. **独立台账表 `api_key_usage_events`**：与请求明细 `requests` 脱钩，清理请求日志或
//!    过期裁剪不会回退已用额度；按请求 id 幂等写入，手动清零与删 Key 只动台账表，
//!    不影响历史报表。
//! 3. **锁序安全**：绝不在持有内存缓存锁时调用 `Db::with` / `Db::with_mut`，也不在持锁时
//!    写事件日志，避免与数据库互斥锁发生死锁。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};

use chrono::{Datelike, Duration, Local, TimeZone};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::server::db::Db;
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 与 JS `Number.MAX_SAFE_INTEGER` 对齐的上界，避免前后端往返精度丢失。
pub const MAX_USAGE_LIMIT: i64 = 9_007_199_254_740_991;

/// 用量计量维度（二选一）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageLimitMetric {
    Tokens,
    Requests,
}

impl UsageLimitMetric {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Requests => "requests",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tokens" => Some(Self::Tokens),
            "requests" => Some(Self::Requests),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tokens => "Token 用量",
            Self::Requests => "请求次数",
        }
    }
}

/// 统计周期类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitPeriod {
    Daily,
    Weekly,
    Monthly,
    Total,
}

impl LimitPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Total => "total",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "每日",
            Self::Weekly => "每周",
            Self::Monthly => "每月",
            Self::Total => "累计",
        }
    }
}

/// 单把网关 Key 的用量限制配置；四项上限全空则视为不限制。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyUsageLimit {
    pub metric: UsageLimitMetric,
    pub daily: Option<i64>,
    pub weekly: Option<i64>,
    pub monthly: Option<i64>,
    pub total: Option<i64>,
}

impl KeyUsageLimit {
    pub fn is_active(&self) -> bool {
        self.daily.is_some_and(|v| v > 0)
            || self.weekly.is_some_and(|v| v > 0)
            || self.monthly.is_some_and(|v| v > 0)
            || self.total.is_some_and(|v| v > 0)
    }

    /// 读侧容错解析：旧配置缺失、类型不符或全部上限为空/非正数时返回 `None`，
    /// 遵循 `api_keys`「升级绝不作废已有 Key」的硬不变量。
    pub fn from_value(value: Option<&Value>) -> Option<Self> {
        let map = value?.as_object()?;
        let metric = map.get("metric").and_then(Value::as_str).and_then(UsageLimitMetric::parse)?;
        let cap = |field: &str| {
            map.get(field)
                .and_then(parse_positive_int_tolerant)
                .filter(|v| *v > 0 && *v <= MAX_USAGE_LIMIT)
        };
        let limit = Self {
            metric,
            daily: cap("daily"),
            weekly: cap("weekly"),
            monthly: cap("monthly"),
            total: cap("total"),
        };
        limit.is_active().then_some(limit)
    }

    /// 写侧严格校验：用于管理 API 创建/更新入口。
    /// `null` 或四项全空归一为 `Ok(None)`（清除限制）。
    pub fn parse_strict(value: &Value) -> Result<Option<Self>, String> {
        if value.is_null() {
            return Ok(None);
        }
        let Some(map) = value.as_object() else {
            return Err("usageLimit 必须是对象或 null".to_string());
        };
        let metric = match map.get("metric") {
            Some(Value::String(raw)) => UsageLimitMetric::parse(raw)
                .ok_or_else(|| "用量限制方式只支持 tokens 或 requests".to_string())?,
            Some(Value::Null) | None => {
                // 全空对象视作清除限制；有任意上限却缺 metric 才报错
                let any_cap = ["daily", "weekly", "monthly", "total"]
                    .iter()
                    .any(|field| map.get(*field).is_some_and(|v| !v.is_null()));
                if !any_cap {
                    return Ok(None);
                }
                return Err("请先选择用量限制方式（Token 用量或请求次数）".to_string());
            }
            _ => return Err("usageLimit.metric 必须是字符串".to_string()),
        };
        let daily = parse_cap_strict(map.get("daily"), "每日上限")?;
        let weekly = parse_cap_strict(map.get("weekly"), "每周上限")?;
        let monthly = parse_cap_strict(map.get("monthly"), "每月上限")?;
        let total = parse_cap_strict(map.get("total"), "累计总量上限")?;
        let limit = Self { metric, daily, weekly, monthly, total };
        Ok(limit.is_active().then_some(limit))
    }

    pub fn to_value(&self) -> Value {
        json!({
            "metric": self.metric.as_str(),
            "daily": self.daily,
            "weekly": self.weekly,
            "monthly": self.monthly,
            "total": self.total,
        })
    }

    fn cap_for(&self, period: LimitPeriod) -> Option<i64> {
        match period {
            LimitPeriod::Daily => self.daily,
            LimitPeriod::Weekly => self.weekly,
            LimitPeriod::Monthly => self.monthly,
            LimitPeriod::Total => self.total,
        }
    }
}

fn parse_positive_int_tolerant(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return (n > 0).then_some(n);
    }
    if let Some(n) = value.as_u64() {
        return i64::try_from(n).ok().filter(|v| *v > 0);
    }
    if let Some(n) = value.as_f64() {
        if n.is_finite() && n.fract() == 0.0 && n > 0.0 && n <= MAX_USAGE_LIMIT as f64 {
            return Some(n as i64);
        }
    }
    None
}

fn parse_cap_strict(value: Option<&Value>, label: &str) -> Result<Option<i64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let parsed = if let Some(n) = value.as_i64() {
        Some(n)
    } else if let Some(n) = value.as_u64() {
        i64::try_from(n).ok()
    } else if let Some(n) = value.as_f64() {
        if n.is_finite() && n.fract() == 0.0 && n >= 0.0 && n <= MAX_USAGE_LIMIT as f64 {
            Some(n as i64)
        } else {
            None
        }
    } else {
        None
    };
    match parsed {
        Some(n) if (1..=MAX_USAGE_LIMIT).contains(&n) => Ok(Some(n)),
        _ => Err(format!("{label}必须是大于 0 的整数")),
    }
}

/// 本地时区自然日 / 自然周 / 自然月窗口边界（毫秒时间戳，左闭右开 `[start, end)`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeriodBounds {
    pub day_start: i64,
    pub day_end: i64,
    pub week_start: i64,
    pub week_end: i64,
    pub month_start: i64,
    pub month_end: i64,
}

impl PeriodBounds {
    pub fn for_now(now_ms: i64) -> Self {
        let local_dt = Local
            .timestamp_millis_opt(now_ms)
            .single()
            .unwrap_or_else(Local::now);
        let date = local_dt.date_naive();
        let day_start_date = date;
        let day_end_date = date + Duration::days(1);
        let week_start_date = date - Duration::days(i64::from(date.weekday().num_days_from_monday()));
        let week_end_date = week_start_date + Duration::days(7);
        let month_start_date = date.with_day(1).unwrap_or(date);
        let month_end_date = (month_start_date + Duration::days(32))
            .with_day(1)
            .unwrap_or(day_end_date);

        let midnight = |d: chrono::NaiveDate| -> i64 {
            d.and_hms_opt(0, 0, 0)
                .and_then(|naive| Local.from_local_datetime(&naive).earliest())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0)
        };

        Self {
            day_start: midnight(day_start_date),
            day_end: midnight(day_end_date),
            week_start: midnight(week_start_date),
            week_end: midnight(week_end_date),
            month_start: midnight(month_start_date),
            month_end: midnight(month_end_date),
        }
    }

    pub fn resets_at(&self, period: LimitPeriod) -> Option<i64> {
        match period {
            LimitPeriod::Daily => (self.day_end > 0).then_some(self.day_end),
            LimitPeriod::Weekly => (self.week_end > 0).then_some(self.week_end),
            LimitPeriod::Monthly => (self.month_end > 0).then_some(self.month_end),
            LimitPeriod::Total => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowTotals {
    pub tokens: i64,
    pub requests: i64,
}

impl WindowTotals {
    fn add(&mut self, tokens: i64, requests: i64) {
        self.tokens = self.tokens.saturating_add(tokens.max(0));
        self.requests = self.requests.saturating_add(requests.max(0));
    }

    fn value(&self, metric: UsageLimitMetric) -> i64 {
        match metric {
            UsageLimitMetric::Tokens => self.tokens,
            UsageLimitMetric::Requests => self.requests,
        }
    }
}

/// 单把 Key 当前四个窗口的已用快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyUsageSnapshot {
    pub bounds: PeriodBounds,
    pub daily: WindowTotals,
    pub weekly: WindowTotals,
    pub monthly: WindowTotals,
    pub total: WindowTotals,
}

impl KeyUsageSnapshot {
    pub fn empty(bounds: PeriodBounds) -> Self {
        Self { bounds, ..Default::default() }
    }

    fn apply_event(&mut self, ts: i64, tokens: i64, requests: i64) {
        self.total.add(tokens, requests);
        if ts >= self.bounds.day_start && ts < self.bounds.day_end {
            self.daily.add(tokens, requests);
        }
        if ts >= self.bounds.week_start && ts < self.bounds.week_end {
            self.weekly.add(tokens, requests);
        }
        if ts >= self.bounds.month_start && ts < self.bounds.month_end {
            self.monthly.add(tokens, requests);
        }
    }

    fn window(&self, period: LimitPeriod) -> WindowTotals {
        match period {
            LimitPeriod::Daily => self.daily,
            LimitPeriod::Weekly => self.weekly,
            LimitPeriod::Monthly => self.monthly,
            LimitPeriod::Total => self.total,
        }
    }

    /// 评估当前是否已达任一已配置上限；返回最先命中的超限信息。
    pub fn check(&self, limit: &KeyUsageLimit, in_flight_requests: i64) -> Option<LimitBreach> {
        let extra = match limit.metric {
            UsageLimitMetric::Requests => in_flight_requests.max(0),
            UsageLimitMetric::Tokens => 0,
        };
        for period in [
            LimitPeriod::Daily,
            LimitPeriod::Weekly,
            LimitPeriod::Monthly,
            LimitPeriod::Total,
        ] {
            let Some(cap) = limit.cap_for(period).filter(|v| *v > 0) else {
                continue;
            };
            let used = self.window(period).value(limit.metric).saturating_add(extra);
            if used >= cap {
                return Some(LimitBreach {
                    period,
                    metric: limit.metric,
                    used,
                    limit: cap,
                    resets_at: self.bounds.resets_at(period),
                });
            }
        }
        None
    }

    /// 管理接口 `GET /api/keys` 每把 Key 的 `usage` 字段。
    pub fn to_json(&self, limit: Option<&KeyUsageLimit>) -> Value {
        let breach = limit.and_then(|cfg| self.check(cfg, 0));
        json!({
            "daily": {
                "tokens": self.daily.tokens,
                "requests": self.daily.requests,
                "resetsAt": self.bounds.day_end,
            },
            "weekly": {
                "tokens": self.weekly.tokens,
                "requests": self.weekly.requests,
                "resetsAt": self.bounds.week_end,
            },
            "monthly": {
                "tokens": self.monthly.tokens,
                "requests": self.monthly.requests,
                "resetsAt": self.bounds.month_end,
            },
            "total": {
                "tokens": self.total.tokens,
                "requests": self.total.requests,
            },
            "exceeded": breach.is_some(),
            "exceededPeriod": breach.as_ref().map(|b| b.period.as_str()),
            "exceededReason": breach.as_ref().map(LimitBreach::summary_text),
        })
    }
}

/// 命中用量上限的结构化结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitBreach {
    pub period: LimitPeriod,
    pub metric: UsageLimitMetric,
    pub used: i64,
    pub limit: i64,
    pub resets_at: Option<i64>,
}

impl LimitBreach {
    pub fn summary_text(&self) -> String {
        let used = format_grouped(self.used);
        let limit = format_grouped(self.limit);
        let base = format!(
            "已达{}{}上限（{used} / {limit}）",
            self.period.label(),
            self.metric.label()
        );
        match self.resets_at.and_then(format_local_reset) {
            Some(at) => format!("{base}，将于 {at} 自动重置"),
            None => base,
        }
    }

    pub fn gateway_error(&self, key_name: &str, now_ms: i64) -> GatewayError {
        let prefix = if key_name.trim().is_empty() {
            "当前 API Key ".to_string()
        } else {
            format!("API Key「{}」", key_name.trim())
        };
        let used = format_grouped(self.used);
        let limit = format_grouped(self.limit);
        let message = match self.resets_at.and_then(format_local_reset) {
            Some(at) => format!(
                "{prefix}已达到{}{}上限（{used} / {limit}），将于 {at} 自动重置，也可在「网关 Key」页调高上限或清零用量",
                self.period.label(),
                self.metric.label()
            ),
            None => format!(
                "{prefix}已达到{}{}上限（{used} / {limit}），请在「网关 Key」页调高上限或清零用量",
                self.period.label(),
                self.metric.label()
            ),
        };
        let mut error = GatewayError::with_status(429, message).with_code("key_usage_limit_exceeded");
        if let Some(resets_at) = self.resets_at {
            let secs = resets_at.saturating_sub(now_ms).max(1000) as u64 / 1000;
            error = error.with_retry_after_seconds(secs.max(1));
        }
        error
    }
}

fn format_grouped(value: i64) -> String {
    let raw = value.max(0).to_string();
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 3);
    for (idx, ch) in bytes.iter().enumerate() {
        if idx > 0 && (bytes.len() - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(*ch as char);
    }
    out
}

fn format_local_reset(ms: i64) -> Option<String> {
    if ms <= 0 {
        return None;
    }
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
}

// ─── 存储句柄与内存缓存 ──────────────────────────────────────

static DB: RwLock<Option<Db>> = RwLock::new(None);

#[derive(Default)]
struct CacheState {
    generation: u64,
    snapshots: HashMap<String, KeyUsageSnapshot>,
    in_flight: HashMap<String, i64>,
}

fn cache() -> &'static Mutex<CacheState> {
    static CACHE: OnceLock<Mutex<CacheState>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(CacheState::default()))
}

fn lock_cache() -> std::sync::MutexGuard<'static, CacheState> {
    match cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn current_db() -> Option<Db> {
    match DB.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// 绑定统一 SQLite 句柄；同时清空旧缓存，便于重启或隔离测试。
pub fn install(db: Option<Db>) {
    match DB.write() {
        Ok(mut slot) => *slot = db,
        Err(poisoned) => *poisoned.into_inner() = db,
    }
    invalidate();
}

/// 清空全部内存快照（整库恢复 / 云同步后调用）。
pub fn invalidate() {
    let mut state = lock_cache();
    state.generation = state.generation.wrapping_add(1);
    state.snapshots.clear();
}

/// 在途请求占额句柄：防止高并发下同时放行超出「请求次数」上限的多个请求。
#[derive(Debug)]
pub struct KeyUsageLease {
    key_id: String,
}

impl Drop for KeyUsageLease {
    fn drop(&mut self) {
        let mut state = lock_cache();
        if let Some(count) = state.in_flight.get_mut(&self.key_id) {
            if *count <= 1 {
                state.in_flight.remove(&self.key_id);
            } else {
                *count -= 1;
            }
        }
    }
}

/// 检查 Key 用量上限并在放行时登记一条在途占额。
pub fn check_and_reserve(
    key_id: &str,
    key_name: &str,
    limit: &KeyUsageLimit,
) -> Result<KeyUsageLease, GatewayError> {
    if key_id.is_empty() || !limit.is_active() {
        return Ok(KeyUsageLease { key_id: String::new() });
    }
    let now = logging::now_ms();
    let bounds = PeriodBounds::for_now(now);

    // 1. 快速路径：命中同窗口缓存
    let generation_before = {
        let mut state = lock_cache();
        if let Some(snapshot) = state.snapshots.get(key_id) {
            if snapshot.bounds == bounds {
                let in_flight = *state.in_flight.get(key_id).unwrap_or(&0);
                if let Some(breach) = snapshot.check(limit, in_flight) {
                    return Err(breach.gateway_error(key_name, now));
                }
                *state.in_flight.entry(key_id.to_string()).or_insert(0) += 1;
                return Ok(KeyUsageLease { key_id: key_id.to_string() });
            }
        }
        state.generation
    };

    // 2. 缓存未命中或跨周期：在缓存锁外查库，再带代次校对回填
    let loaded = query_one(key_id, bounds);
    let mut state = lock_cache();
    let snapshot = if state.generation == generation_before {
        state.snapshots.insert(key_id.to_string(), loaded.clone());
        loaded
    } else if let Some(fresh) = state.snapshots.get(key_id).filter(|s| s.bounds == bounds) {
        fresh.clone()
    } else {
        // 期间发生了写/重置，重新查一次以确保精确
        drop(state);
        let reloaded = query_one(key_id, bounds);
        let mut state = lock_cache();
        let in_flight = *state.in_flight.get(key_id).unwrap_or(&0);
        if let Some(breach) = reloaded.check(limit, in_flight) {
            return Err(breach.gateway_error(key_name, now));
        }
        *state.in_flight.entry(key_id.to_string()).or_insert(0) += 1;
        return Ok(KeyUsageLease { key_id: key_id.to_string() });
    };

    let in_flight = *state.in_flight.get(key_id).unwrap_or(&0);
    if let Some(breach) = snapshot.check(limit, in_flight) {
        return Err(breach.gateway_error(key_name, now));
    }
    *state.in_flight.entry(key_id.to_string()).or_insert(0) += 1;
    Ok(KeyUsageLease { key_id: key_id.to_string() })
}

/// 在已有事务内幂等记入一条 Key 用量明细；返回是否真正新增了行。
pub(crate) fn insert_event(
    conn: &Connection,
    request_id: &str,
    key_id: &str,
    ts: i64,
    tokens: i64,
    requests: i64,
) -> rusqlite::Result<bool> {
    if key_id.is_empty() {
        return Ok(false);
    }
    let event_id = if request_id.is_empty() {
        format!("{key_id}:{ts}:{tokens}")
    } else {
        request_id.to_string()
    };
    let changed = conn.execute(
        "INSERT OR IGNORE INTO api_key_usage_events (id, key_id, ts, tokens, requests) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![event_id, key_id, ts, tokens.max(0), requests.max(0)],
    )?;
    Ok(changed == 1)
}

/// 事务提交并释放 DB 锁后增量同步内存缓存。
pub(crate) fn note_committed(key_id: &str, ts: i64, tokens: i64, requests: i64) {
    if key_id.is_empty() {
        return;
    }
    let now = logging::now_ms();
    let bounds = PeriodBounds::for_now(now);
    let mut state = lock_cache();
    state.generation = state.generation.wrapping_add(1);
    if let Some(snapshot) = state.snapshots.get_mut(key_id) {
        if snapshot.bounds == bounds {
            snapshot.apply_event(ts, tokens, requests);
        } else {
            state.snapshots.remove(key_id);
        }
    }
}

/// 降级路径：若请求明细表写入失败，仍把 Key 消耗落进独立台账。
pub(crate) fn persist_without_stats(request_id: &str, key_id: &str, ts: i64, tokens: i64, requests: i64) {
    if key_id.is_empty() {
        return;
    }
    let Some(db) = current_db() else {
        return;
    };
    let inserted = db
        .with_mut(|conn| insert_event(conn, request_id, key_id, ts, tokens, requests))
        .and_then(Result::ok)
        .unwrap_or(false);
    if inserted {
        note_committed(key_id, ts, tokens, requests);
    }
}

/// 清零单把 Key 的全部用量台账（管理页「清零用量」与删除 Key 共用）。
pub fn clear_key(key_id: &str) -> Result<(), String> {
    if key_id.is_empty() {
        return Ok(());
    }
    if let Some(db) = current_db() {
        db.with_mut(|conn| {
            conn.execute("DELETE FROM api_key_usage_events WHERE key_id = ?1", params![key_id])
        })
        .ok_or_else(|| "用量数据库不可用".to_string())?
        .map_err(|error| format!("清零用量失败：{error}"))?;
    }
    let mut state = lock_cache();
    state.generation = state.generation.wrapping_add(1);
    state.snapshots.remove(key_id);
    Ok(())
}

/// 批量读取所有 Key 的当前用量快照（供 `GET /api/keys` 单次聚合查询，零 N+1）。
pub fn snapshots_all() -> (PeriodBounds, HashMap<String, KeyUsageSnapshot>) {
    let bounds = PeriodBounds::for_now(logging::now_ms());
    let Some(db) = current_db() else {
        return (bounds, HashMap::new());
    };
    let rows = db
        .with(|conn| query_all_conn(conn, bounds))
        .and_then(Result::ok)
        .unwrap_or_default();
    (bounds, rows)
}

fn query_one(key_id: &str, bounds: PeriodBounds) -> KeyUsageSnapshot {
    let Some(db) = current_db() else {
        return KeyUsageSnapshot::empty(bounds);
    };
    db.with(|conn| query_one_conn(conn, key_id, bounds))
        .and_then(Result::ok)
        .unwrap_or_else(|| KeyUsageSnapshot::empty(bounds))
}

fn query_one_conn(conn: &Connection, key_id: &str, bounds: PeriodBounds) -> rusqlite::Result<KeyUsageSnapshot> {
    conn.query_row(
        "SELECT \
           COALESCE(SUM(CASE WHEN ts >= ?1 AND ts < ?2 THEN tokens ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?1 AND ts < ?2 THEN requests ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?3 AND ts < ?4 THEN tokens ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?3 AND ts < ?4 THEN requests ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?5 AND ts < ?6 THEN tokens ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?5 AND ts < ?6 THEN requests ELSE 0 END), 0), \
           COALESCE(SUM(tokens), 0), \
           COALESCE(SUM(requests), 0) \
         FROM api_key_usage_events WHERE key_id = ?7",
        params![
            bounds.day_start,
            bounds.day_end,
            bounds.week_start,
            bounds.week_end,
            bounds.month_start,
            bounds.month_end,
            key_id
        ],
        |row| {
            Ok(KeyUsageSnapshot {
                bounds,
                daily: WindowTotals { tokens: row.get(0)?, requests: row.get(1)? },
                weekly: WindowTotals { tokens: row.get(2)?, requests: row.get(3)? },
                monthly: WindowTotals { tokens: row.get(4)?, requests: row.get(5)? },
                total: WindowTotals { tokens: row.get(6)?, requests: row.get(7)? },
            })
        },
    )
}

fn query_all_conn(conn: &Connection, bounds: PeriodBounds) -> rusqlite::Result<HashMap<String, KeyUsageSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT \
           key_id, \
           COALESCE(SUM(CASE WHEN ts >= ?1 AND ts < ?2 THEN tokens ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?1 AND ts < ?2 THEN requests ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?3 AND ts < ?4 THEN tokens ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?3 AND ts < ?4 THEN requests ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?5 AND ts < ?6 THEN tokens ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN ts >= ?5 AND ts < ?6 THEN requests ELSE 0 END), 0), \
           COALESCE(SUM(tokens), 0), \
           COALESCE(SUM(requests), 0) \
         FROM api_key_usage_events GROUP BY key_id",
    )?;
    let mut out = HashMap::new();
    let rows = stmt.query_map(
        params![
            bounds.day_start,
            bounds.day_end,
            bounds.week_start,
            bounds.week_end,
            bounds.month_start,
            bounds.month_end
        ],
        |row| {
            let key_id: String = row.get(0)?;
            let snapshot = KeyUsageSnapshot {
                bounds,
                daily: WindowTotals { tokens: row.get(1)?, requests: row.get(2)? },
                weekly: WindowTotals { tokens: row.get(3)?, requests: row.get(4)? },
                monthly: WindowTotals { tokens: row.get(5)?, requests: row.get(6)? },
                total: WindowTotals { tokens: row.get(7)?, requests: row.get(8)? },
            };
            Ok((key_id, snapshot))
        },
    )?;
    for row in rows {
        let (key_id, snapshot) = row?;
        out.insert(key_id, snapshot);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strict_validates_and_normalizes() {
        assert_eq!(KeyUsageLimit::parse_strict(&Value::Null).unwrap(), None);
        assert_eq!(KeyUsageLimit::parse_strict(&json!({})).unwrap(), None);
        assert_eq!(
            KeyUsageLimit::parse_strict(&json!({ "metric": "tokens", "daily": null })).unwrap(),
            None
        );

        let parsed = KeyUsageLimit::parse_strict(&json!({
            "metric": "tokens",
            "daily": 500_000,
            "weekly": null,
            "monthly": 10_000_000,
            "total": 50_000_000
        }))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.metric, UsageLimitMetric::Tokens);
        assert_eq!(parsed.daily, Some(500_000));
        assert_eq!(parsed.weekly, None);
        assert_eq!(parsed.monthly, Some(10_000_000));
        assert_eq!(parsed.total, Some(50_000_000));

        assert!(KeyUsageLimit::parse_strict(&json!({ "metric": "usd", "daily": 10 })).is_err());
        assert!(KeyUsageLimit::parse_strict(&json!({ "metric": "requests", "daily": 0 })).is_err());
        assert!(KeyUsageLimit::parse_strict(&json!({ "metric": "requests", "daily": -5 })).is_err());
        assert!(KeyUsageLimit::parse_strict(&json!({ "metric": "requests", "daily": 1.5 })).is_err());
        assert!(KeyUsageLimit::parse_strict(&json!({ "daily": 10 })).is_err());
    }

    #[test]
    fn from_value_is_tolerant_for_upgrade_safety() {
        assert_eq!(KeyUsageLimit::from_value(None), None);
        assert_eq!(KeyUsageLimit::from_value(Some(&json!("corrupted"))), None);
        assert_eq!(KeyUsageLimit::from_value(Some(&json!({ "metric": "unknown", "daily": 10 }))), None);
        let parsed = KeyUsageLimit::from_value(Some(&json!({
            "metric": "requests",
            "daily": 100,
            "weekly": -1,
            "monthly": "bad",
            "total": 500
        })))
        .unwrap();
        assert_eq!(parsed.daily, Some(100));
        assert_eq!(parsed.weekly, None);
        assert_eq!(parsed.monthly, None);
        assert_eq!(parsed.total, Some(500));
    }

    #[test]
    fn usage_ledger_enforces_tokens_requests_inflight_and_reset() {
        crate::test_support::isolated(
            concat!(module_path!(), "::usage_ledger_enforces_tokens_requests_inflight_and_reset"),
            || {
            let dir = std::env::temp_dir().join(format!(
                "agent2api-key-usage-test-{}",
                logging::now_ms()
            ));
            let _ = std::fs::create_dir_all(&dir);
            let db = Db::open(&dir.join("agent2api.db")).expect("打开测试库失败");
            install(Some(db.clone()));

            let req_limit = KeyUsageLimit {
                metric: UsageLimitMetric::Requests,
                daily: Some(2),
                weekly: None,
                monthly: None,
                total: None,
            };

            // 并发两次在途占额：前两次放行，第三次被 429 拦下
            let lease1 = check_and_reserve("k1", "测试 Key", &req_limit).expect("第 1 次应放行");
            let lease2 = check_and_reserve("k1", "测试 Key", &req_limit).expect("第 2 次应放行");
            let blocked = check_and_reserve("k1", "测试 Key", &req_limit).expect_err("第 3 次应超限");
            assert_eq!(blocked.status_code, 429);
            assert_eq!(blocked.code.as_deref(), Some("key_usage_limit_exceeded"));
            assert!(blocked.retry_after_secs.is_some());

            // 释放未落账的在途占额后恢复可用
            drop(lease2);
            let lease3 = check_and_reserve("k1", "测试 Key", &req_limit).expect("释放后应可再放行");
            drop(lease1);
            drop(lease3);

            // 落盘记账（幂等去重）
            let now = logging::now_ms();
            persist_without_stats("req-1", "k1", now, 600, 1);
            persist_without_stats("req-1", "k1", now, 600, 1); // 重复 id 不得双计
            persist_without_stats("req-2", "k1", now, 500, 1);

            let (_, all) = snapshots_all();
            let snap = all.get("k1").expect("应查到 k1 快照");
            assert_eq!(snap.daily.requests, 2);
            assert_eq!(snap.daily.tokens, 1100);
            assert_eq!(snap.total.requests, 2);
            assert_eq!(snap.total.tokens, 1100);

            // 请求次数与 Token 上限均应拦截
            assert!(check_and_reserve("k1", "测试 Key", &req_limit).is_err());
            let tok_limit = KeyUsageLimit {
                metric: UsageLimitMetric::Tokens,
                daily: None,
                weekly: None,
                monthly: None,
                total: Some(1000),
            };
            let tok_err = check_and_reserve("k1", "测试 Key", &tok_limit).expect_err("Token 应超限");
            assert_eq!(tok_err.status_code, 429);
            assert!(tok_err.retry_after_secs.is_none());

            // 手动清零后立刻恢复可用
            clear_key("k1").expect("清零不应失败");
            assert!(check_and_reserve("k1", "测试 Key", &req_limit).is_ok());
            assert!(check_and_reserve("k1", "测试 Key", &tok_limit).is_ok());

            install(None);
            drop(db);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
