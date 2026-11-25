use chrono::Duration as ChronoDuration;
use rusqlite::{params, Connection};

use crate::server::config::{
    DEFAULT_DAILY_RETENTION_DAYS, DEFAULT_REQUEST_RETENTION_COUNT, DEFAULT_REQUEST_RETENTION_DAYS,
    RETENTION_MAX_COUNT, RETENTION_MAX_DAYS, RETENTION_MIN_COUNT, RETENTION_MIN_DAYS,
};

use super::clock::{date_key, local_midnight_ms, today};
use super::sql;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    pub request_days: i64,
    pub request_count: i64,
    pub daily_days: i64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            request_days: DEFAULT_REQUEST_RETENTION_DAYS,
            request_count: DEFAULT_REQUEST_RETENTION_COUNT,
            daily_days: DEFAULT_DAILY_RETENTION_DAYS,
        }
    }
}

impl Retention {
    pub(super) fn normalized(self) -> Self {
        Self {
            request_days: self.request_days.clamp(RETENTION_MIN_DAYS, RETENTION_MAX_DAYS),
            request_count: self.request_count.clamp(RETENTION_MIN_COUNT, RETENTION_MAX_COUNT),
            daily_days: self.daily_days.clamp(RETENTION_MIN_DAYS, RETENTION_MAX_DAYS),
        }
    }
}

pub(super) struct RetentionBounds {
    pub(super) requests_ms: i64,
    pub(super) request_count: i64,
    pub(super) daily_key: String,
}

impl RetentionBounds {
    pub(super) fn of(retention: Retention) -> Self {
        let now_day = today();
        Self {
            requests_ms: local_midnight_ms(now_day - ChronoDuration::days(retention.request_days - 1)),
            request_count: retention.request_count,
            daily_key: date_key(now_day - ChronoDuration::days(retention.daily_days - 1)),
        }
    }
}

// 调用方持有事务；正文先于明细删除，按天聚合不受数量裁剪影响。
pub(super) fn trim_requests(conn: &Connection, max: i64) -> rusqlite::Result<usize> {
    let overflow = (sql::count_all(conn)? as i64 - max).max(0);
    if overflow == 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM request_raw WHERE id IN \
         (SELECT id FROM requests ORDER BY ts ASC, row_id ASC LIMIT ?1)",
        params![overflow],
    )?;
    conn.execute(
        "DELETE FROM local_direct_refs WHERE request_id IN \
         (SELECT id FROM requests ORDER BY ts ASC, row_id ASC LIMIT ?1)",
        params![overflow],
    )?;
    conn.execute(
        "DELETE FROM requests WHERE row_id IN \
         (SELECT row_id FROM requests ORDER BY ts ASC, row_id ASC LIMIT ?1)",
        params![overflow],
    )
}
