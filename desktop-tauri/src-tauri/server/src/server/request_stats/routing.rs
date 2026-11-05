//! 路由页的轻量指标与历史回放；按请求模型隔离，不把映射目标的其它请求混进来。
//!
//! ── 模型匹配口径 ─────────────────────────────────────────────
//! 指标与回放都按「这个模型」过滤，匹配三个模型名里的任意一个：
//! 客户端点的名（`client_model`，可能带 `(low)` 这类后缀）、网关解析出的名
//! （`model`）与上游真名（`upstream_model`）。
//!
//! 三个都要比的原因：客户端可以点带后缀的别名（`DeepSeek-V4.1-Flash(low)`），
//! 网关把它映射到自己的模型名再映射到上游真名 —— 三个名字两两不同。
//! 只比 `client_model` 的话，路由页按 `model` 作用域查指标就查不到这一行：
//! 在途连线于是不高亮、提供商卡片上的请求数也永远是 0。
//!
//! 口径与「模型请求日志」一致（见 `trace` 模块的同一处匹配），否则同一屏里
//! 两处对「这个模型有没有流量」给出相反的答案。

use rusqlite::params;
use serde_json::{json, Value};

use super::{entry_json, sql, RequestStats};

impl RequestStats {
    pub fn routing_metrics(&self, model: Option<&str>) -> Value {
        let start = chrono::Local::now().date_naive().and_hms_opt(0, 0, 0)
            .and_then(|time| time.and_local_timezone(chrono::Local).earliest())
            .map(|time| time.timestamp_millis()).unwrap_or(0);
        let model = model.unwrap_or("");
        let loaded = {
            let guard = self.guard();
            self.with_conn(&guard, |conn| {
                let (running, requests, successful, duration): (i64, i64, i64, i64) = conn.query_row(
                    "SELECT COALESCE(SUM(status = 0 AND error IS NULL AND ts >= ?3), 0),
                     COALESCE(SUM(ts >= ?1 AND (status <> 0 OR error IS NOT NULL) AND is_test = 0), 0),
                     COALESCE(SUM(ts >= ?1 AND status >= 200 AND status < 300 AND error IS NULL AND is_test = 0), 0),
                     COALESCE(SUM(CASE WHEN ts >= ?1 AND status >= 200 AND status < 300 AND error IS NULL AND is_test = 0 THEN duration_ms ELSE 0 END), 0)
                     FROM requests WHERE (?2 = '' OR model = ?2 COLLATE NOCASE
                       OR client_model = ?2 COLLATE NOCASE OR upstream_model = ?2 COLLATE NOCASE)
                     AND (ts >= ?1 OR (status = 0 AND error IS NULL AND ts >= ?3))",
                    params![start, model, crate::server::logging::process_started_at()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT account_id, COUNT(*) FROM requests WHERE status = 0 AND error IS NULL AND ts >= ?2
                     AND (?1 = '' OR model = ?1 COLLATE NOCASE
                       OR client_model = ?1 COLLATE NOCASE OR upstream_model = ?1 COLLATE NOCASE)
                     AND account_id <> '' GROUP BY account_id",
                )?;
                let mut counts = serde_json::Map::new();
                let mut rows = stmt.query(params![model, crate::server::logging::process_started_at()])?;
                while let Some(row) = rows.next()? {
                    counts.insert(row.get::<_, String>(0)?, Value::from(row.get::<_, i64>(1)?));
                }
                Ok(json!({
                    "running": running,
                    "todayRequests": requests,
                    "todaySuccessRate": if requests > 0 { Some(successful as f64 * 100.0 / requests as f64) } else { None },
                    "averageDurationMs": if successful > 0 { Some(duration / successful) } else { None },
                    "accountRunning": counts,
                }))
            })
        };
        loaded.unwrap_or_else(|| json!({ "available": false }))
    }

    pub fn routing_replays(&self, model: Option<&str>, offset: usize, limit: usize, status: &str) -> Value {
        let model = model.unwrap_or("");
        let limit = limit.clamp(1, 100);
        let status_clause = match status {
            "error" => " AND status <> 0 AND NOT (status >= 200 AND status < 300 AND error IS NULL)",
            "sticky" => " AND json_valid(route) AND json_extract(route, '$.stickyHit') = 1",
            _ => "",
        };
        let loaded = {
            let guard = self.guard();
            self.with_conn(&guard, |conn| {
                let matched: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM requests WHERE route IS NOT NULL
                     AND (?1 = '' OR model = ?1 COLLATE NOCASE){status_clause}"),
                    params![model], |row| row.get(0),
                )?;
                let query = format!(
                    "SELECT {} FROM requests WHERE route IS NOT NULL
                     AND (?1 = '' OR model = ?1 COLLATE NOCASE){status_clause}
                     ORDER BY ts DESC, row_id DESC LIMIT ?2 OFFSET ?3", sql::REQUEST_COLUMNS,
                );
                let mut stmt = conn.prepare(&query)?;
                let mut rows = stmt.query(params![model, limit as i64, offset as i64])?;
                let mut entries = Vec::new();
                while let Some(row) = rows.next()? { entries.push(entry_json(&sql::decode_request(row)?)); }
                Ok(json!({ "entries": entries, "matched": matched, "offset": offset, "limit": limit }))
            })
        };
        loaded.unwrap_or_else(|| json!({ "entries": [], "matched": 0, "available": false }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::db::Db;
    use crate::server::request_stats::{RequestSource, Retention, RunningProgress};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// 临时库的 RAII 守卫：目录只在用例结束时删一次。
    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_stats(tag: &str) -> (Arc<RequestStats>, TempDir) {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!("request-stats-routing-{}-{}-{id}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::open(&dir.join("agent2api.db")).expect("临时库应当能建起来");
        (Arc::new(RequestStats::with_db(Some(db), Retention::default)), TempDir(dir))
    }

    /// 插一条在途行并回写承载它的账号。
    fn insert_running(
        stats: &RequestStats,
        id: &str,
        model: &str,
        client_model: &str,
        account_id: &str,
    ) {
        // 行的时间戳要明确晚于「进程启动时刻」。
        //
        // 陈旧清理按 `ts < 进程启动时刻` 判僵尸行，而那个时刻是首次调用时
        // 锁定的全局值：并行跑用例时，它可能在某个用例取完时间戳之后才被锁定，
        // 那一行的 ts 就早于它，行会被当成上一轮遗留补成 408（账号与上游
        // 模型一并清空）。生产环境里进程启动时刻早于任何请求，不会有这个竞态。
        //
        // 这里先取一次全局值把它锁住，再在此之上留出一秒余量：无论谁先锁定，
        // 插入行的 ts 都晚于它。
        let _ = crate::server::logging::process_started_at();
        let ts = crate::server::logging::process_started_at() + 1000;
        stats.record_started(
            id,
            ts,
            model,
            client_model,
            "",
            &RequestSource::default(),
            false,
        );
        stats.update_running(id, &RunningProgress {
            provider: "trae".to_string(),
            account_id: account_id.to_string(),
            account_name: "mojie126".to_string(),
            upstream_model: model.to_string(),
            upstream_reasoning: String::new(),
            attempts: 1,
            first_response_ms: None,
            phase: "streaming".to_string(),
            phase_started_at: None,
            attempt_details: Vec::new(),
            route: None,
            sensitive_hits: Vec::new(),
        });
    }

    /// 客户端点的是带后缀的别名，网关按自己的模型名建独立配置时，
    /// 在途请求也要算进这个模型的指标 —— 否则路由页的在途连线不会高亮。
    #[test]
    fn a_running_request_is_counted_by_the_gateway_model_name() {
        let (stats, _dir) = temp_stats("gateway-model");
        insert_running(
            &stats,
            "req-1",
            "deepseek-v4.1-flash",
            "DeepSeek-V4.1-Flash(low)",
            "acct-trae",
        );
        let metrics = stats.routing_metrics(Some("deepseek-v4.1-flash"));
        assert_eq!(
            metrics["accountRunning"].get("acct-trae").and_then(Value::as_i64),
            Some(1),
            "在途请求应按网关模型名计入账号在途数：{metrics}"
        );
    }

    /// 反过来，客户端点的别名也要能查到同一批在途请求。
    #[test]
    fn a_running_request_is_counted_by_the_client_model_alias() {
        let (stats, _dir) = temp_stats("client-model");
        insert_running(
            &stats,
            "req-1",
            "deepseek-v4.1-flash",
            "DeepSeek-V4.1-Flash(low)",
            "acct-trae",
        );
        let metrics = stats.routing_metrics(Some("DeepSeek-V4.1-Flash(low)"));
        assert_eq!(
            metrics["accountRunning"].get("acct-trae").and_then(Value::as_i64),
            Some(1),
            "在途请求应按客户端别名计入账号在途数：{metrics}"
        );
    }

    /// 别的模型的请求不能算进来。
    #[test]
    fn a_running_request_of_another_model_is_not_counted() {
        let (stats, _dir) = temp_stats("other-model");
        insert_running(
            &stats,
            "req-1",
            "glm-5.3",
            "glm-5.3",
            "acct-trae",
        );
        let metrics = stats.routing_metrics(Some("deepseek-v4.1-flash"));
        assert_eq!(
            metrics["accountRunning"].get("acct-trae").and_then(Value::as_i64),
            None,
            "别的模型的请求不该计入本模型的账号在途数：{metrics}"
        );
    }
}
