use std::sync::Arc;

use crate::server::core::limiter;
use crate::server::request_stats::{NewRequestEntry, RequestStats};

pub(super) struct FinishedRecord<'a> {
    pub stats: Arc<RequestStats>,
    pub entry: NewRequestEntry,
    pub sent: bool,
    pub channel: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub raw_request: Option<&'a str>,
    pub raw_response: Option<&'a str>,
}

pub(super) fn persist(record: FinishedRecord<'_>) {
    let FinishedRecord { stats, entry, sent, channel, started_at, finished_at, raw_request, raw_response } = record;
    let account = entry.account_id.clone();
    let tokens = entry.total_tokens;
    let model = if entry.upstream_model.is_empty() { entry.model.clone() } else { entry.upstream_model.clone() };
    let id = entry.id.clone();
    stats.record(entry);
    if sent { limiter::counter::note(&account, &id, &model, &channel, tokens, started_at); }
    if !account.is_empty() { limiter::note_request_tokens(&account, tokens, finished_at); }
    stats.store_raw(&id, started_at, raw_request, raw_response);
}
