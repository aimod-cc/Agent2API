use crate::server::core::upstream::usage;
use crate::server::logging;
use crate::server::request_stats::{AttemptDetail, LimiterEvent, NewRequestEntry, RetryEvent, SensitiveHit};

use super::pipeline::{truncate_chars, RecordContext, ERROR_SUMMARY_CHARS};

pub(super) fn stored_attempt_details(list: &[usage::AttemptDetail]) -> Vec<AttemptDetail> {
    list.iter()
        .map(|item| AttemptDetail {
            provider: item.provider.clone(),
            account: item.account.clone(),
            account_id: item.account_id.clone(),
            candidate_id: item.candidate_id.clone(),
            member_id: item.member_id.clone(),
            model: item.model.clone(),
            reasoning: item.reasoning.clone(),
            reason: item.reason.clone(),
            sticky_hit: item.sticky_hit,
            started_at: item.started_at,
            response_at: item.response_at,
            finished_at: item.finished_at,
            status: item.status,
            error: item.error.clone(),
            retries: item.retries.iter().map(|retry| RetryEvent {
                reason: retry.reason.clone(),
                status: retry.status,
                delay_ms: retry.delay_ms,
            }).collect(),
            notice: item.notice.clone(),
            body_bytes: item.body_bytes,
        })
        .collect()
}

pub(super) fn stored_sensitive_hits(list: &[usage::SensitiveHit]) -> Vec<SensitiveHit> {
    list.iter().map(|hit| SensitiveHit {
        word: hit.word.clone(),
        count: hit.count,
    }).collect()
}

pub fn record_entry(context: &RecordContext, fallback_error: Option<String>) {
    crate::server::core::blocking::in_place(|| persist(context, fallback_error));
}

fn persist(context: &RecordContext, fallback_error: Option<String>) {
    super::vision_compat_record::persist(context);
    let mut snapshot = context.telemetry.snapshot();
    let finished_at = logging::now_ms();
    let duration_ms = finished_at - context.started_at;
    let first_response_ms = snapshot.first_response_at.map(|at| (at - context.started_at).max(0));
    let error = snapshot.error.clone().or(fallback_error).map(|text| {
        if context.raw_request.is_some() || context.raw_response.is_some()
            || context.telemetry.capture().is_some() || crate::server::core::debug_traffic::enabled()
        {
            text
        } else {
            truncate_chars(&text, ERROR_SUMMARY_CHARS)
        }
    });
    snapshot.finish_trace(finished_at, error.as_deref());
    let attempts = snapshot.attempts.max(1);
    let mut entry = NewRequestEntry::new(context.model.clone(), context.status);
    entry.ts = Some(context.started_at);
    entry.id = snapshot.id.clone();
    entry.duration_ms = duration_ms;
    entry.first_response_ms = first_response_ms;
    entry.attempts = attempts;
    entry.account_id = snapshot.account_id;
    entry.account_name = snapshot.account_name;
    entry.provider = snapshot.provider;
    entry.client_model = context.client_model.clone();
    entry.upstream_model = snapshot.upstream_model;
    entry.client_reasoning = context.client_reasoning.clone();
    entry.upstream_reasoning = snapshot.upstream_reasoning;
    entry.attempt_details = stored_attempt_details(&snapshot.attempts_detail);
    entry.sensitive_hits = stored_sensitive_hits(&snapshot.sensitive_hits);
    entry.route = snapshot.route.unwrap_or(serde_json::Value::Null);
    entry.is_test = context.is_test;
    entry.source = context.source.clone();
    entry.error = error;
    entry.prompt_tokens = snapshot.prompt_tokens;
    entry.completion_tokens = snapshot.completion_tokens;
    entry.total_tokens = snapshot.total_tokens;
    entry.cache_read_tokens = snapshot.cache_read_tokens;
    entry.credit = snapshot.credit;

    // 限额沿用上游实际消耗，不能改用失败清零后的报表读数。
    let account = entry.account_id.clone();
    let request = entry.id.clone();
    let model = if entry.upstream_model.is_empty() { entry.model.clone() } else { entry.upstream_model.clone() };
    let limiter_event = (snapshot.attempts > 0).then_some(LimiterEvent {
        account: &account,
        request: &request,
        model: &model,
        channel: &snapshot.quota_channel,
        tokens: entry.total_tokens,
        started_at: context.started_at,
        finished_at,
    });
    context.stats.record_with_limiter(entry, limiter_event);
    // 正文独立提交，排障数据写入失败不能撤销明细与额度。
    context.stats.store_raw(
        &snapshot.id,
        context.started_at,
        context.raw_request.as_deref(),
        context.raw_response.as_deref(),
    );
    logging::verbose(
        "[Stats]",
        &format!(
            "记一条请求: model={} status={} {duration_ms}ms 首响={} attempts={attempts} tokens={}+{}（缓存 {}）",
            if context.model.is_empty() { "(未指定)" } else { &context.model },
            context.status,
            first_response_ms.map(|value| format!("{value}ms")).unwrap_or_else(|| "-".to_string()),
            snapshot.prompt_tokens,
            snapshot.completion_tokens,
            snapshot.cache_read_tokens,
        ),
    );
}
