//! 系统事件与调试日志共用 SQLite 表，按级别预留容量，合计不超过配置上限。
//! 查询可按重要事件、全部事件、调试详情筛选，并在分页前折叠重复记录。
//! 原始记录不因折叠删除；日志 id 高水位只在显式全清时重置。

mod presentation;
mod sql;

pub(crate) use presentation::prepare as prepare_entry;
pub use presentation::{category_for_tag, LogGroup, LogView};

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{Duration as ChronoDuration, Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::server::config;
use crate::server::db::Db;

pub const MAX_ENTRIES: usize = 500;
pub const MAX_EVENT_ENTRIES: usize = 5_000;
pub const MAX_WARN_ENTRIES: usize = 1_000;
pub const MAX_ERROR_ENTRIES: usize = 1_000;
pub const MAX_ALERT_ENTRIES: usize = MAX_WARN_ENTRIES + MAX_ERROR_ENTRIES;
pub const MAX_DEBUG_ENTRIES: usize = 1_000;
const LEVEL_CAPACITY_WEIGHT: usize = MAX_EVENT_ENTRIES + MAX_ALERT_ENTRIES + MAX_DEBUG_ENTRIES;

pub(crate) fn retention_capacity() -> usize {
    config::retention_settings().log_count
        .clamp(config::RETENTION_MIN_COUNT, config::RETENTION_MAX_COUNT) as usize
}

pub(crate) fn level_capacity(total: usize, level: &str) -> usize {
    let weight = match level {
        "debug" => MAX_DEBUG_ENTRIES,
        "warn" => MAX_WARN_ENTRIES,
        "error" => MAX_ERROR_ENTRIES,
        _ => MAX_EVENT_ENTRIES,
    };
    ((total as u64 * weight as u64 / LEVEL_CAPACITY_WEIGHT as u64) as usize).max(1)
}
pub const DEBUG_RETENTION_DAYS: i64 = 3;
pub const LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];
pub const CATEGORIES: [(&str, &str); 11] = [
    ("server", "服务"),
    ("auth", "登录"),
    ("account", "账号"),
    ("model", "模型"),
    ("upstream", "上游"),
    ("desensitize", "脱敏"),
    ("config", "配置"),
    ("checkin", "自动签到"),
    ("maintenance", "凭证自动维护"),
    ("update", "软件版本检查"),
    ("plugin", "插件"),
];

const MAX_MESSAGE_LENGTH: usize = 1000;
const MAX_DATA_KEYS: usize = 24;

/// 写入多少条才做一次保留期 / 容量裁剪。
///
/// 原先每写一条日志都要做两次 `COUNT(*)` + 两次 DELETE 来判断是否超限；
/// 日志页空闲时也会持续写心跳类日志，这几次全表扫描没有意义。按批裁剪把
/// 写入路径压成「一条 INSERT」，最多让容量超出 63 条（下一次裁剪抹平）。
const MAINTENANCE_EVERY: u64 = 64;

/// 日志内容版本：任何会改变查询/统计结果的写入（追加、裁剪、清空）都 +1。
///
/// 缓存只服务「同一次读取反复发生」的场景（日志页 1 秒轮询、统计与列表
/// 同一轮里各读一次）；版本变了就必须重新读库。用进程级原子而不是每个
/// `LogStore` 字段，是为了覆盖迁移代码绕过 store 直接写表的那条路径
/// （`sql_trim_capacity` 在模块内，能顺带 bump）。
static CACHE_EPOCH: AtomicU64 = AtomicU64::new(0);
/// 距离上次容量裁剪已经写入了多少条（进程级，单例 store 无冲突）。
static APPENDS_SINCE_TRIM: AtomicU64 = AtomicU64::new(0);

fn bump_cache_epoch() {
    CACHE_EPOCH.fetch_add(1, Ordering::Relaxed);
}

fn cache_epoch() -> u64 {
    CACHE_EPOCH.load(Ordering::Relaxed)
}

fn should_trim_now() -> bool {
    let count = APPENDS_SINCE_TRIM.fetch_add(1, Ordering::Relaxed) + 1;
    if count >= MAINTENANCE_EVERY {
        APPENDS_SINCE_TRIM.store(0, Ordering::Relaxed);
        true
    } else {
        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub ts: i64,
    pub level: String,
    pub category: String,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub entries: Vec<LogEntry>,
    pub groups: Vec<LogGroup>,
    pub total: usize,
    pub matched: usize,
    pub grouped: usize,
    pub offset: usize,
    pub limit: usize,
    pub by_level: Map<String, Value>,
    pub last_id: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: usize,
    pub max: usize,
    pub by_level: Map<String, Value>,
    pub by_category: Map<String, Value>,
    pub last_id: u64,
    pub file: String,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub limit: Option<usize>,
    pub offset: usize,
    pub aggregate: bool,
    pub view: LogView,
    /// 固定读取水位，新增条目不会移动正在阅读的历史分页。
    pub until_id: Option<u64>,
    pub level: Option<String>,
    pub category: Option<String>,
    pub keyword: Option<String>,
    pub since_id: Option<u64>,
    pub start: Option<i64>,
    /// 时间区间为 [start, end)，单位毫秒。
    pub end: Option<i64>,
}

pub(crate) fn normalize_level(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    if matches!(lower.as_str(), "verbose" | "trace") {
        return "debug".to_string();
    }
    if LEVELS.contains(&lower.as_str()) {
        lower
    } else {
        "info".to_string()
    }
}

pub(crate) fn level_rank(level: &str) -> usize {
    LEVELS.iter().position(|item| *item == level).unwrap_or(1)
}

pub(crate) fn normalize_category(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    if CATEGORIES.iter().any(|(key, _)| *key == lower) {
        lower
    } else {
        "server".to_string()
    }
}

pub(crate) fn normalize_data(value: Option<&Value>) -> Option<Value> {
    let Value::Object(map) = value? else {
        return None;
    };
    let mut out = Map::new();
    for (key, item) in map.iter().take(MAX_DATA_KEYS) {
        let trimmed = match item {
            Value::String(text) if text.chars().count() > MAX_MESSAGE_LENGTH => {
                Value::String(format!(
                    "{}…",
                    text.chars().take(MAX_MESSAGE_LENGTH).collect::<String>()
                ))
            }
            other => other.clone(),
        };
        out.insert(key.clone(), trimmed);
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

pub(crate) fn squeeze_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn clamp_message(text: &str) -> String {
    text.chars().take(MAX_MESSAGE_LENGTH).collect()
}

pub(crate) fn sql_raise_next_id(conn: &rusqlite::Connection, value: u64) -> rusqlite::Result<()> {
    sql::raise_next_id(conn, value)
}

pub(crate) fn sql_trim_capacity(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    let removed = sql::trim_to_capacity(conn, retention_capacity())?;
    // 迁移路径会直接改表、不经过 LogStore；调用点就在导入之后，无论本次裁剪
    // 有没有删行都要让可能已存在的列表 / 统计缓存失效（导入的正是新内容）。
    bump_cache_epoch();
    Ok(removed)
}

pub struct LogStore {
    db: Option<Db>,
    file_path: PathBuf,
    get_retention_days: Arc<dyn Fn() -> i64 + Send + Sync>,
    inner: Mutex<()>,
    /// 最近几次列表查询的结果。日志页会「主查询 + 增量探测」交替发，
    /// 只缓存一条会让两者互相挤掉；两条足够覆盖这个节奏。
    query_cache: Mutex<VecDeque<CachedQuery>>,
    /// 全局统计（级别 / 分类条数）。日志页与导航角标都读它；没有新日志时
    /// 直接复用，避免每秒重扫整表。
    stats_cache: Mutex<Option<(u64, Stats)>>,
}

struct CachedQuery {
    epoch: u64,
    key: String,
    result: QueryResult,
}

impl LogStore {
    pub fn with_db(
        db: Option<Db>,
        get_retention_days: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        let file_path = db
            .as_ref()
            .map(|db| db.file().to_path_buf())
            .unwrap_or_else(|| config::config_dir().join(crate::server::db::FILE_NAME));
        Self {
            db,
            file_path,
            get_retention_days: Arc::new(get_retention_days),
            inner: Mutex::new(()),
            query_cache: Mutex::new(VecDeque::new()),
            stats_cache: Mutex::new(None),
        }
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn file(&self) -> PathBuf {
        self.file_path.clone()
    }

    fn with_conn<T>(
        &self,
        _guard: &MutexGuard<'_, ()>,
        action: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        match self.db.as_ref()?.with(action) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                eprintln!("[Logs] 日志数据库操作失败: {error}");
                None
            }
            None => {
                eprintln!("[Logs] 日志数据库不可用（连接已标记中毒）");
                None
            }
        }
    }

    fn with_conn_mut<T>(
        &self,
        _guard: &MutexGuard<'_, ()>,
        action: impl FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        match self.db.as_ref()?.with_mut(action) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                eprintln!("[Logs] 日志数据库操作失败: {error}");
                None
            }
            None => {
                eprintln!("[Logs] 日志数据库不可用（连接已标记中毒）");
                None
            }
        }
    }

    pub(crate) fn retention_cutoff_ms(&self) -> i64 {
        let days = (self.get_retention_days)()
            .clamp(config::RETENTION_MIN_DAYS, config::RETENTION_MAX_DAYS);
        cutoff_ms(days)
    }

    fn debug_cutoff_ms(&self) -> i64 {
        self.retention_cutoff_ms()
            .max(cutoff_ms(DEBUG_RETENTION_DAYS))
    }

    pub fn prune(&self) -> usize {
        let guard = self.guard();
        let cutoff = self.retention_cutoff_ms();
        let debug_cutoff = self.debug_cutoff_ms();
        let removed = self
            .with_conn_mut(&guard, |conn| {
                let tx = conn.transaction()?;
                let removed = sql::delete_expired(&tx, cutoff, None)?
                    + sql::delete_expired_debug(&tx, debug_cutoff, None)?
                    + sql::trim_to_capacity(&tx, retention_capacity())?;
                tx.commit()?;
                Ok(removed)
            })
            .unwrap_or(0);
        // 显式裁剪（启动 / 改保留期 / 手动清空）之后重置批次计数：下一次写入
        // 不必再从头攒 64 条才检查一次。即便这次没有删行，保留期上限本身可能
        // 已经变了（`Stats.max` 读的是配置），缓存也要失效。
        APPENDS_SINCE_TRIM.store(0, Ordering::Relaxed);
        bump_cache_epoch();
        removed
    }

    pub fn append(&self, entry: NewEntry<'_>) -> Option<LogEntry> {
        let message = clamp_message(&squeeze_whitespace(entry.message));
        if message.is_empty() {
            return None;
        }
        let guard = self.guard();
        let cutoff = self.retention_cutoff_ms();
        let debug_cutoff = self.debug_cutoff_ms();
        let ts = entry.ts.unwrap_or_else(crate::server::logging::now_ms);
        let level = normalize_level(entry.level);
        let category = normalize_category(entry.category);
        let data = normalize_data(entry.data);
        // 保留期 / 容量裁剪按批做，见 `MAINTENANCE_EVERY` 的说明。
        let trim_now = should_trim_now();
        let result = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            let id = sql::allocate_next_id(&tx)?;
            let record = presentation::prepare(LogEntry {
                id,
                ts,
                level,
                category,
                message,
                data,
            });
            sql::insert(&tx, &record)?;
            if trim_now {
                sql::delete_expired(&tx, cutoff, Some(id))?;
                sql::delete_expired_debug(&tx, debug_cutoff, Some(id))?;
                let capacity = retention_capacity();
                sql::trim_level_capacity(&tx, &record.level, capacity)?;
                sql::trim_total_capacity(&tx, capacity)?;
            }
            tx.commit()?;
            Ok(record)
        });
        if result.is_some() {
            bump_cache_epoch();
        }
        result
    }

    pub fn query(&self, query: &Query) -> QueryResult {
        let guard = self.guard();
        let key = format!("{query:?}");
        if let Some(cached) = self.cached_query(&guard, &key) {
            return cached;
        }
        let plan = sql::FilterPlan::of(query);
        let limit = query.limit.unwrap_or(200).clamp(1, MAX_ENTRIES);
        let fallback = QueryResult {
            entries: Vec::new(),
            groups: Vec::new(),
            total: 0,
            matched: 0,
            grouped: 0,
            offset: query.offset,
            limit,
            by_level: empty_level_map(),
            last_id: 0,
        };
        let computed = self.with_conn(&guard, |conn| -> rusqlite::Result<QueryResult> {
            let total = sql::count_all(conn)?;
            let last_id = sql::max_id(conn)?;
            let filtered = sql::select_matching_desc(conn, &plan)?;
            let matched = filtered.len();
            let mut counts: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();
            for entry in &filtered {
                *counts.entry(entry.level.clone()).or_insert(0) += 1;
            }
            let by_level = level_map(LEVELS.iter().map(|level| {
                (
                    (*level).to_string(),
                    counts.get(*level).copied().unwrap_or(0),
                )
            }));
            let (grouped, groups) =
                presentation::groups_paged(filtered, query.aggregate, query.offset, limit);
            let entries = groups.iter().map(|group| group.entry.clone()).collect();
            Ok(QueryResult {
                entries,
                groups,
                total,
                matched,
                grouped,
                offset: query.offset,
                limit,
                by_level,
                last_id,
            })
        });
        let Some(result) = computed else {
            return fallback;
        };
        self.store_query_cache(&guard, key, &result);
        result
    }

    fn cached_query(&self, _guard: &MutexGuard<'_, ()>, key: &str) -> Option<QueryResult> {
        let epoch = cache_epoch();
        let cache = self
            .query_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache
            .iter()
            .find(|entry| entry.epoch == epoch && entry.key == key)
            .map(|entry| entry.result.clone())
    }

    fn store_query_cache(&self, _guard: &MutexGuard<'_, ()>, key: String, result: &QueryResult) {
        let epoch = cache_epoch();
        let mut cache = self
            .query_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 旧版本（epoch 不同）的结果一律丢掉：它们可能对应已被裁剪 / 清空的表。
        cache.retain(|entry| entry.epoch == epoch);
        if let Some(index) = cache.iter().position(|entry| entry.key == key) {
            cache.remove(index);
        }
        cache.push_front(CachedQuery {
            epoch,
            key,
            result: result.clone(),
        });
        cache.truncate(2);
    }

    pub fn clear_where(&self, query: &Query) -> (usize, Stats) {
        let removed = {
            let guard = self.guard();
            let plan = sql::FilterPlan::of(query);
            self.with_conn_mut(&guard, |conn| {
                let tx = conn.transaction()?;
                // 分类 / 级别在 SQL 里只做了「超集」过滤（旧插件日志读取时会
                // 改判），所以删除仍要经过 Rust 侧 matches 精确筛一遍。
                let ids: Vec<u64> = sql::select_matching_desc(&tx, &plan)?
                    .iter()
                    .map(|entry| entry.id)
                    .collect();
                let removed = sql::delete_by_ids(&tx, &ids)?;
                tx.commit()?;
                Ok(removed)
            })
            .unwrap_or(0)
        };
        if removed > 0 {
            bump_cache_epoch();
        }
        (removed, self.stats())
    }

    pub fn stats(&self) -> Stats {
        let guard = self.guard();
        let epoch = cache_epoch();
        {
            let cache = self
                .stats_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((cached_epoch, stats)) = cache.as_ref() {
                if *cached_epoch == epoch {
                    return stats.clone();
                }
            }
        }
        let loaded = self.with_conn(&guard, sql::stats_snapshot);
        let (total, last_id, buckets) = match loaded {
            Some(value) => value,
            _ => {
                return Stats {
                    total: 0,
                    max: retention_capacity(),
                    by_level: empty_level_map(),
                    by_category: empty_category_map(),
                    last_id: 0,
                    file: self.file_path.display().to_string(),
                }
            }
        };
        let mut by_level = empty_level_map();
        let mut by_category = empty_category_map();
        for (level, category, count) in buckets {
            let level_slot = by_level.entry(level).or_insert(Value::from(0));
            let level_total = level_slot.as_i64().unwrap_or(0) + count;
            *level_slot = Value::from(level_total);
            let category_slot = by_category.entry(category).or_insert(Value::from(0));
            let category_total = category_slot.as_i64().unwrap_or(0) + count;
            *category_slot = Value::from(category_total);
        }
        let stats = Stats {
            total,
            max: retention_capacity(),
            by_level,
            by_category,
            last_id,
            file: self.file_path.display().to_string(),
        };
        *self
            .stats_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((epoch, stats.clone()));
        stats
    }

    pub fn clear(&self) -> Stats {
        {
            let guard = self.guard();
            let changed = self
                .with_conn_mut(&guard, |conn| {
                    let tx = conn.transaction()?;
                    sql::delete_all(&tx)?;
                    sql::reset_next_id(&tx)?;
                    tx.commit()
                })
                .is_some();
            if changed {
                APPENDS_SINCE_TRIM.store(0, Ordering::Relaxed);
                bump_cache_epoch();
            }
        }
        self.stats()
    }

    pub fn to_jsonl(&self) -> String {
        let guard = self.guard();
        let entries = self
            .with_conn(&guard, sql::select_all_asc)
            .unwrap_or_default();
        render_jsonl(&entries)
    }

    pub fn to_jsonl_where(&self, query: &Query) -> String {
        let guard = self.guard();
        let plan = sql::FilterPlan::of(query);
        let mut entries = self
            .with_conn(&guard, |conn| sql::select_matching_desc(conn, &plan))
            .unwrap_or_default();
        entries.reverse();
        render_jsonl(&entries)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        let guard = self.guard();
        self.with_conn(&guard, sql::count_all).unwrap_or(0)
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn empty_level_map() -> Map<String, Value> {
    LEVELS
        .iter()
        .map(|level| (level.to_string(), Value::from(0)))
        .collect()
}

fn empty_category_map() -> Map<String, Value> {
    CATEGORIES
        .iter()
        .map(|(key, _)| (key.to_string(), Value::from(0)))
        .collect()
}

fn level_map(pairs: impl IntoIterator<Item = (String, i64)>) -> Map<String, Value> {
    let mut map = empty_level_map();
    for (level, count) in pairs {
        map.insert(level, Value::from(count.max(0)));
    }
    map
}

fn cutoff_ms(days: i64) -> i64 {
    let day = Local::now().date_naive() - ChronoDuration::days(days - 1);
    day.and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
        .map(|value| value.timestamp_millis())
        .unwrap_or(0)
}

fn render_jsonl(entries: &[LogEntry]) -> String {
    let mut text = String::new();
    for entry in entries {
        if let Ok(line) = serde_json::to_string(entry) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    text
}

pub struct NewEntry<'a> {
    pub level: &'a str,
    pub category: &'a str,
    pub message: &'a str,
    pub data: Option<&'a Value>,
    pub ts: Option<i64>,
}
