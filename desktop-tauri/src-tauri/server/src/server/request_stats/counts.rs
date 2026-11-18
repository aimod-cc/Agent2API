use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::sql::{self, FilterPlan};

const MAX_QUERIES: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Version {
    pub requests: u64,
    pub database: u64,
    pub external: i64,
}

#[derive(Default)]
struct Cached {
    version: Option<Version>,
    checked_at: Option<Instant>,
    total: Option<usize>,
    queries: VecDeque<(FilterPlan, (usize, usize))>,
}

#[derive(Default)]
pub(super) struct CountsCache(Mutex<Cached>);

impl CountsCache {
    pub fn read(&self, conn: &Connection, version: Version, plan: &FilterPlan) -> rusqlite::Result<(usize, usize, usize)> {
        let mut cache = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if cache.version != Some(version)
            || cache.checked_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(30)) {
            *cache = Cached { version: Some(version), checked_at: Some(Instant::now()), ..Default::default() };
        }
        let total = match cache.total {
            Some(total) => total,
            None => {
                let total = sql::count_all(conn)?;
                cache.total = Some(total);
                total
            }
        };
        if let Some((_, (matched, running))) = cache.queries.iter().find(|(query, _)| query == plan) {
            return Ok((total, *matched, *running));
        }
        let (matched, running) = sql::filtered_counts(conn, plan, total)?;
        if cache.queries.len() >= MAX_QUERIES { cache.queries.pop_front(); }
        cache.queries.push_back((plan.clone(), (matched, running)));
        Ok((total, matched, running))
    }
}
