use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, Duration, Local, TimeZone};
use rusqlite::params;

use crate::server::{db::Db, logging};

use super::types::*;

#[cfg(test)]
#[path = "counter_tests.rs"]
mod tests;

static DB: OnceLock<Option<Db>> = OnceLock::new();

#[derive(Clone, Debug, Default)]
pub struct ModelCount {
    pub tokens: i64,
    pub requests: i64,
}

#[derive(Clone, Debug)]
struct Event {
    ts: i64,
    model: String,
    tokens: i64,
    requests: i64,
}

type Release = (i64, i64, i64);

#[derive(Debug, Default)]
pub struct Count {
    pub tokens: i64,
    pub requests: i64,
    pub start: i64,
    pub end: i64,
    pub watermark: i64,
    pub window: Window,
    pub models: Vec<String>,
    pub channel: String,
    pub by_model: HashMap<String, ModelCount>,
    /// 滚动窗口专用：`(过期时刻, tokens, requests)`，按过期时刻升序。
    /// 只记「这条事件什么时候滑出窗口」，让 `evaluate` 能在不改动事件队列的前提下
    /// 反推出「计数回落到阈值以下」的时刻。非滚动窗口恒为空。
    pub releases: Vec<Release>,
    /// 同一张表按模型小写名分组（条件限定了模型时才用得上）。
    pub model_releases: HashMap<String, Vec<Release>>,
    events: VecDeque<Event>,
}

impl Clone for Count {
    fn clone(&self) -> Self {
        Self {
            tokens: self.tokens,
            requests: self.requests,
            start: self.start,
            end: self.end,
            watermark: self.watermark,
            window: self.window.clone(),
            models: self.models.clone(),
            channel: self.channel.clone(),
            by_model: self.by_model.clone(),
            // 读数侧（`evaluate`）要拿这两张表算滚动窗口的恢复时刻，所以必须随克隆带过去。
            // `events` 仍留在这里：它只服务于写侧的淘汰，克隆出去没有用途。
            releases: self.releases.clone(),
            model_releases: self.model_releases.clone(),
            events: VecDeque::new(),
        }
    }
}

impl Count {
    fn add(&mut self, event: Event) {
        self.tokens = self.tokens.saturating_add(event.tokens);
        self.requests = self.requests.saturating_add(event.requests);
        let model = self.by_model.entry(event.model.to_ascii_lowercase()).or_default();
        model.tokens = model.tokens.saturating_add(event.tokens);
        model.requests = model.requests.saturating_add(event.requests);
        if self.window.reset == Reset::Rolling {
            let index = self.events.iter().position(|existing| existing.ts > event.ts)
                .unwrap_or(self.events.len());
            let release = (event.ts.saturating_add(self.window.seconds.saturating_mul(1000)),
                event.tokens, event.requests);
            push_release(&mut self.releases, release);
            let key = event.model.to_ascii_lowercase();
            let bucket = self.model_releases.entry(key).or_default();
            push_release(bucket, release);
            self.events.insert(index, event);
        }
    }

    fn advance(&mut self, now: i64) {
        let (start, end) = window_bounds(&self.window, now);
        if self.window.reset == Reset::Rolling {
            while self.events.front().is_some_and(|event| event.ts < start) {
                let Some(event) = self.events.pop_front() else { break };
                self.tokens = self.tokens.saturating_sub(event.tokens).max(0);
                self.requests = self.requests.saturating_sub(event.requests).max(0);
                let key = event.model.to_ascii_lowercase();
                if let Some(model) = self.by_model.get_mut(&key) {
                    model.tokens = model.tokens.saturating_sub(event.tokens).max(0);
                    model.requests = model.requests.saturating_sub(event.requests).max(0);
                }
            }
            // 与 `events` 同步淘汰：过期时刻 <= now 的条目已经滑出窗口，留着只会让
            // 恢复时刻算得偏晚。
            while self.releases.first().is_some_and(|release| release.0 <= now) {
                self.releases.remove(0);
            }
            for bucket in self.model_releases.values_mut() {
                while bucket.first().is_some_and(|release| release.0 <= now) {
                    bucket.remove(0);
                }
            }
        } else if self.start != start {
            self.tokens = 0;
            self.requests = 0;
            self.by_model.clear();
            self.releases.clear();
            self.model_releases.clear();
        }
        self.start = start;
        self.end = end;
    }
}

/// 按过期时刻升序插入（事件本身来自 `ORDER BY ts`，绝大多数情况就是尾部追加）。
fn push_release(bucket: &mut Vec<Release>, release: Release) {
    let index = bucket.iter().position(|existing| existing.0 > release.0).unwrap_or(bucket.len());
    bucket.insert(index, release);
}

/// 滚动窗口下「计数回落到解除条件」的时刻（毫秒时间戳）。
///
/// `total` 是当前窗口内的累计读数；从最旧的事件开始逐条扣减，
/// 第一次满足「不再命中」时返回该事件的过期时刻。全部扣完仍命中
/// （阈值比窗口内所有事件之和还小）时返回最后一条的过期时刻 —— 那之后
/// 窗口一定是空的。拿不到任何事件（含非滚动窗口）返回 0，前端据此不显示倒计时。
pub fn rolling_release(releases: &[Release], total: i64, tokens: bool, mut still_hit: impl FnMut(f64) -> bool) -> i64 {
    let Some(last) = releases.last() else { return 0 };
    if !still_hit(total as f64) { return 0; }
    let mut remaining = total;
    for release in releases {
        remaining = remaining.saturating_sub(if tokens { release.1 } else { release.2 }).max(0);
        if !still_hit(remaining as f64) { return release.0; }
    }
    last.0
}

pub type Facts = HashMap<(String, String), Count>;

fn slot() -> &'static Mutex<Facts> {
    static FACTS: OnceLock<Mutex<Facts>> = OnceLock::new();
    FACTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn install(db: Option<Db>) { let _ = DB.set(db); }

pub fn facts() -> Facts {
    let Ok(mut facts) = slot().lock() else { return Facts::new() };
    let now = logging::now_ms();
    for count in facts.values_mut() { count.advance(now); }
    facts.clone()
}

pub fn invalidate() { if let Ok(mut facts) = slot().lock() { facts.clear(); } }

pub fn channel_of(account: &serde_json::Value) -> String {
    account.get("quotaChannel").or_else(|| account.get("zcodePlan"))
        .and_then(serde_json::Value::as_str).unwrap_or("").to_string()
}

pub fn key(condition: &Condition) -> String {
    let channel = condition.source.as_ref().map(|source| source.channel.as_str()).unwrap_or("");
    serde_json::to_string(&(condition.source_id.as_str(), channel, models_for(condition), window_for(condition))).unwrap_or_default()
}

pub fn window_bounds(window: &Window, now: i64) -> (i64, i64) {
    match window.reset {
        Reset::Total => (window.starts_at, window.ends_at),
        Reset::Rolling => (now.saturating_sub(window.seconds.saturating_mul(1000)), now.saturating_add(1)),
        Reset::Fixed => {
            let width = window.seconds.max(1).saturating_mul(1000);
            let start = now.saturating_sub(now.saturating_sub(window.starts_at).rem_euclid(width));
            (start, start.saturating_add(width))
        }
        _ => {
            let offset = match window.utc_offset_minutes {
                Some(offset) => i64::from(offset) * 60_000,
                None => Local.timestamp_millis_opt(now).single()
                    .map(|time| i64::from(time.offset().local_minus_utc()) * 1000).unwrap_or(0),
            };
            let Some(local) = chrono::Utc.timestamp_millis_opt(now.saturating_add(offset)).single() else {
                return (0, 0);
            };
            let date = local.date_naive();
            let start_date = match window.reset {
                Reset::Weekly => date - Duration::days(i64::from(date.weekday().num_days_from_monday())),
                Reset::Monthly => date.with_day(1).unwrap_or(date),
                _ => date,
            };
            let end_date = match window.reset {
                Reset::Weekly => start_date + Duration::days(7),
                Reset::Monthly => (start_date + Duration::days(32)).with_day(1).unwrap_or(start_date),
                _ => start_date + Duration::days(1),
            };
            let stamp = |date: chrono::NaiveDate| {
                date.and_hms_opt(0, 0, 0).map(|time| {
                    if window.utc_offset_minutes.is_none() {
                        Local.from_local_datetime(&time).earliest().map(|time| time.timestamp_millis())
                            .unwrap_or_else(|| time.and_utc().timestamp_millis() - offset)
                    } else {
                        time.and_utc().timestamp_millis() - offset
                    }
                }).unwrap_or(0)
            };
            (stamp(start_date), stamp(end_date))
        }
    }
}

pub fn models_for(condition: &Condition) -> &[String] {
    if !condition.models.is_empty() { &condition.models }
    else { condition.source.as_ref().map(|source| source.models.as_slice()).unwrap_or_default() }
}

pub fn window_for(condition: &Condition) -> Option<Window> {
    condition.window.clone().or_else(|| condition.source.as_ref()?.window.clone())
}

pub fn refresh(accounts: &[serde_json::Value]) { refresh_inner(accounts, true); }

pub fn refresh_account(account: &serde_json::Value) {
    refresh_inner(std::slice::from_ref(account), false);
}

fn refresh_inner(accounts: &[serde_json::Value], replace: bool) {
    let Some(db) = DB.get().and_then(Option::as_ref) else { return };
    let now = logging::now_ms();
    let mut queries = HashMap::new();
    let ids: Vec<&str> = accounts.iter().filter_map(|account| account.get("id").and_then(serde_json::Value::as_str)).collect();
    for account in accounts {
        let Some(id) = account.get("id").and_then(serde_json::Value::as_str) else { continue };
        for group in super::groups::effective_groups(account) {
            for condition in group.conditions {
                if matches!(condition.metric, Metric::Tokens | Metric::Requests) {
                    queries.insert((id.to_string(), key(&condition)), condition);
                }
            }
        }
    }
    let mut slot = match slot().lock() { Ok(slot) => slot, Err(_) => return };
    let fresh = db.with(|conn| -> rusqlite::Result<Facts> {
        let watermark: i64 = conn.query_row("SELECT COALESCE(MAX(row_id), 0) FROM account_limiter_events", [], |row| row.get(0))?;
        let mut facts = Facts::new();
        let mut statement = conn.prepare(
            "SELECT ts, model, tokens, requests FROM account_limiter_events \
             WHERE account_id = ?1 AND ts >= ?2 AND (?3 = 0 OR ts < ?3) \
             AND (?4 = '' OR channel = ?4) ORDER BY ts")?;
        for ((id, key), condition) in queries {
            let Some(window) = window_for(&condition) else { continue };
            let (start, end) = window_bounds(&window, now);
            let channel = condition.source.as_ref().map(|source| source.channel.as_str()).unwrap_or("");
            let rows = statement.query_map(params![id, start, end, channel], |row| {
                Ok(Event { ts: row.get(0)?, model: row.get(1)?, tokens: row.get(2)?, requests: row.get(3)? })
            })?;
            let mut count = Count { start, end, watermark, window,
                models: models_for(&condition).to_vec(), channel: channel.to_string(), ..Count::default() };
            for row in rows {
                let event = row?;
                if count.models.is_empty() || count.models.iter().any(|wanted| wanted.eq_ignore_ascii_case(&event.model)) {
                    count.add(event);
                }
            }
            facts.insert((id, key), count);
        }
        Ok(facts)
    }).and_then(Result::ok);
    if let Some(fresh) = fresh {
        if replace { *slot = fresh; }
        else {
            slot.retain(|(id, _), _| !ids.contains(&id.as_str()));
            slot.extend(fresh);
        }
    }
}

pub fn note(id: &str, request_id: &str, model: &str, channel: &str, tokens: i64, ts: i64) -> bool {
    if id.is_empty() { return false; }
    let Some(db) = DB.get().and_then(Option::as_ref) else { return false };
    let inserted = db.with_mut(|conn| {
        insert_event(conn, id, request_id, model, channel, tokens, ts)
    }).and_then(Result::ok).flatten();
    let Some(row_id) = inserted else { return false };
    update_facts(id, model, channel, tokens, ts, row_id);
    true
}

pub(crate) fn insert_event(
    conn: &rusqlite::Connection,
    id: &str,
    request_id: &str,
    model: &str,
    channel: &str,
    tokens: i64,
    ts: i64,
) -> rusqlite::Result<Option<i64>> {
    if id.is_empty() { return Ok(None); }
    // 额度台账独立于请求日志；关联 id 用于收尾幂等，清理日志不回退消耗。
    let changed = conn.execute(
        "INSERT OR IGNORE INTO account_limiter_events \
         (id, account_id, model, channel, ts, tokens, requests) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
        params![if request_id.is_empty() { format!("{id}:{ts}:{model}") } else { request_id.to_string() },
            id, model, channel, ts, tokens.max(0)]
    )?;
    Ok((changed == 1).then(|| conn.last_insert_rowid()))
}

/// 只能在事务提交且数据库连接锁释放后更新，刷新路径按事实锁、数据库锁的顺序取锁。
pub(crate) fn update_facts(
    id: &str,
    model: &str,
    channel: &str,
    tokens: i64,
    ts: i64,
    row_id: i64,
) {
    if let Ok(mut facts) = slot().lock() {
        let now = logging::now_ms();
        for ((account, _), count) in facts.iter_mut() {
            if account != id || row_id <= count.watermark { continue; }
            count.advance(now);
            if ts < count.start || (count.end > 0 && ts >= count.end) { continue; }
            if !count.channel.is_empty() && count.channel != channel { continue; }
            if !count.models.is_empty() && !count.models.iter().any(|wanted| wanted.eq_ignore_ascii_case(model)) { continue; }
            count.add(Event { ts, model: model.to_string(), tokens: tokens.max(0), requests: 1 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rolling_release;

    /// `(过期时刻, 请求数)` → `rolling_release` 吃的三元组（tokens 固定 0）
    fn requests(at: &[(i64, i64)]) -> Vec<(i64, i64, i64)> {
        at.iter().map(|(expire, count)| (*expire, 0, *count)).collect()
    }

    #[test]
    fn a_rolling_window_recovers_when_enough_events_slide_out() {
        let table = requests(&[(1_000, 5), (2_000, 5), (3_000, 5)]);
        let hit = |value: f64| value >= 10.0;
        // 15 次、阈值 10：一条滑出后剩 10（仍命中），第二条滑出后剩 5 才放行
        assert_eq!(2_000, rolling_release(&table, 15, false, hit));
        // 阈值 15：第一条滑出就放行
        assert_eq!(1_000, rolling_release(&table, 15, false, |value| value >= 15.0));
        // 阈值 1：要等窗口整段滑空
        assert_eq!(3_000, rolling_release(&table, 15, false, |value| value >= 1.0));
    }

    #[test]
    fn an_unhit_or_empty_window_has_no_recovery_moment() {
        assert_eq!(0, rolling_release(&[], 0, false, |_| true));
        // 当前读数本就不命中：没有「恢复时刻」可言，返回 0 让前端不画倒计时
        assert_eq!(0, rolling_release(&requests(&[(1_000, 1)]), 1, false, |value| value >= 5.0));
    }

    #[test]
    fn tokens_use_their_own_column() {
        let table = vec![(1_000i64, 400i64, 3i64), (2_000, 400, 3)];
        // 阈值 5：按 tokens 扣是 800 → 400 → 0，要等两条都滑出；
        // 若错用 requests 列（6 → 3）则第一条滑出就低于 5、返回 1000，
        // 所以 2000 这个预期同时钉住了「读的是 tokens 列」。
        assert_eq!(2_000, rolling_release(&table, 800, true, |value| value >= 5.0));
    }
}
