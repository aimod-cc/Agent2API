use crate::server::core::limiter;

use super::{daily, lifecycle, now_ms, retention, sql, stale_cutoff, NewRequestEntry, RequestSource, RequestStats, RunningProgress, MAX_RAW_ROWS};

pub(crate) struct LimiterEvent<'a> {
    pub account: &'a str,
    pub request: &'a str,
    pub model: &'a str,
    pub channel: &'a str,
    pub tokens: i64,
    pub started_at: i64,
    pub finished_at: i64,
}

impl LimiterEvent<'_> {
    fn insert(&self, conn: &rusqlite::Connection) -> rusqlite::Result<Option<i64>> {
        limiter::counter::insert_event(conn, self.account, self.request, self.model,
            self.channel, self.tokens, self.started_at)
    }

    fn committed(&self, row_id: i64) {
        limiter::counter::update_facts(self.account, self.model, self.channel,
            self.tokens, self.started_at, row_id);
        limiter::note_request_tokens(self.account, self.tokens, self.finished_at);
    }

    fn persist_without_stats(&self) {
        if limiter::counter::note(self.account, self.request, self.model, self.channel,
            self.tokens, self.started_at)
        {
            limiter::note_request_tokens(self.account, self.tokens, self.finished_at);
        }
    }
}

impl RequestStats {
    pub fn record(&self, entry: NewRequestEntry) {
        self.record_with_limiter(entry, None);
    }

    pub(crate) fn record_with_limiter(&self, entry: NewRequestEntry, limiter_event: Option<LimiterEvent<'_>>) {
        let record = entry.normalize();
        let bounds = self.retention_bounds();
        let maintain = self.should_maintain();
        let outcome = {
            let guard = self.guard();
            self.with_conn_mut(&guard, |conn| {
                let tx = conn.transaction()?;
                let terminal_exists = !record.id.is_empty() && sql::terminal_record_exists(&tx, &record.id)?;
                if !terminal_exists {
                    let updated = sql::update_running_request(&tx, &record)?;
                    if updated == 0 {
                        sql::insert_request(&tx, &record)?;
                    }
                    if maintain {
                        sql::delete_expired_requests(&tx, bounds.requests_ms)?;
                        retention::trim_requests(&tx, bounds.request_count)?;
                    }
                    if !record.is_test {
                        lifecycle::add_to_daily(&tx, &record)?;
                    }
                    daily::delete_daily_before(&tx, &bounds.daily_key)?;
                    // 正文仅用于排障，其裁剪失败不能撤销请求记账。
                    if maintain {
                        if let Ok(raw_rows) = sql::count_raw(&tx) {
                            if raw_rows > MAX_RAW_ROWS {
                                let _ = sql::trim_raw_rows(&tx, raw_rows - MAX_RAW_ROWS);
                            }
                        }
                    }
                }
                // 僵尸收尾可能先于真实用量到达；终态不覆盖，额度仍按请求 ID 幂等补记。
                let row_id = limiter_event.as_ref().map(|event| event.insert(&tx)).transpose()?.flatten();
                tx.commit()?;
                Ok(row_id)
            })
        };
        if let Some(event) = limiter_event {
            match outcome {
                Some(Some(row_id)) => event.committed(row_id),
                // 日报等统计失败时保留独立额度台账的降级路径，不能因此漏计真实消耗。
                None => event.persist_without_stats(),
                Some(None) => {}
            }
        }
    }

    pub fn record_started(
        &self,
        id: &str,
        ts: i64,
        model: &str,
        client_model: &str,
        client_reasoning: &str,
        source: &RequestSource,
        is_test: bool,
    ) {
        if id.is_empty() {
            return;
        }
        let bounds = self.retention_bounds();
        let maintain = self.should_maintain();
        let guard = self.guard();
        let _ = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            sql::insert_started_request(&tx, id, ts, model, client_model, client_reasoning, source, is_test)?;
            lifecycle::finish_stale(&tx, stale_cutoff(), now_ms())?;
            if maintain {
                sql::delete_expired_requests(&tx, bounds.requests_ms)?;
                retention::trim_requests(&tx, bounds.request_count)?;
            }
            tx.commit()
        });
    }

    pub fn sweep_stale_running(&self) {
        let bounds = self.retention_bounds();
        let guard = self.guard();
        let _ = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            lifecycle::finish_stale(&tx, stale_cutoff(), now_ms())?;
            daily::delete_daily_before(&tx, &bounds.daily_key)?;
            tx.commit()
        });
    }

    pub fn update_running(&self, id: &str, progress: &RunningProgress) {
        if id.is_empty() {
            return;
        }
        let guard = self.guard();
        let _ = self.with_conn_mut(&guard, |conn| sql::update_running_progress(conn, id, progress));
    }
}
