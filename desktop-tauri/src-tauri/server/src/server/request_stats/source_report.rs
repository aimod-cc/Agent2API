use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::record::{DailyEntry, ModelAccum, RequestEntry};
use super::source::{ApiKeyAccum, ApiKeyIdentity, ClientAccum, UsageAccum};
use super::report::{model_breakdown_json, model_stat_key, push_model_accum, safe_rate};

pub(super) fn request_usage(item: &RequestEntry) -> UsageAccum {
    UsageAccum {
        requests: 1,
        successful: i64::from(item.is_success()),
        tokens: item.total_tokens,
        prompt_tokens: item.prompt_tokens,
        completion_tokens: item.completion_tokens,
        cache_read_tokens: item.cache_read_tokens,
        token_details_requests: 1,
        last_used_at: Some(item.ts),
    }
}

pub(super) fn merge_client(list: &mut Vec<ClientAccum>, client: &str, label: &str, usage: &UsageAccum) {
    if let Some(item) = list.iter_mut().find(|item| item.client == client) {
        if !label.is_empty() && usage.last_used_at >= item.usage.last_used_at { item.label = label.into(); }
        item.usage.merge(usage);
    } else {
        list.push(ClientAccum { client: client.to_string(), label: label.into(), usage: usage.clone() });
    }
}

fn merge_key(list: &mut Vec<ApiKeyAccum>, incoming: &ApiKeyAccum) {
    if let Some(item) = list.iter_mut().find(|item| {
        item.identity.id == incoming.identity.id && item.identity.source == incoming.identity.source
    }) {
        if incoming.usage.last_used_at >= item.usage.last_used_at && !incoming.identity.name.is_empty() {
            item.identity.name = incoming.identity.name.clone();
        }
        item.usage.merge(&incoming.usage);
        for client in &incoming.clients {
            merge_client(&mut item.clients, &client.client, &client.label, &client.usage);
        }
        for model in &incoming.models {
            push_model_accum(&mut item.models, &model.model, model.tokens, model.requests);
        }
    } else {
        list.push(incoming.clone());
    }
}

pub(super) fn fold_sources(day: &mut DailyEntry, item: &RequestEntry) {
    let usage = request_usage(item);
    let label = item.source.recorded_client_name();
    merge_client(&mut day.client_stats, &item.source.client, label, &usage);
    merge_key(&mut day.api_key_stats, &ApiKeyAccum {
        identity: item.source.api_key.clone(),
        usage: usage.clone(),
        clients: vec![ClientAccum { client: item.source.client.clone(), label: label.into(), usage }],
        models: vec![ModelAccum {
            model: model_stat_key(&item.model, &item.upstream_model).to_string(),
            requests: 1, tokens: item.total_tokens,
        }],
    });
}

pub(super) fn client_label(client: &str) -> &str {
    match client {
        "" => "未记录客户端",
        "other" => "其它",
        "model_test" => "模型测试",
        "assistant" => "AI 助手",
        "assistant_images" => "助手图片工具",
        "assistant_title" => "助手标题总结",
        "checkin_keepalive" => "日活保活",
        _ => client,
    }
}

pub(super) fn key_label(identity: &ApiKeyIdentity) -> String {
    match identity.source.as_str() {
        "none" => "免鉴权调用".to_string(),
        "env" => "环境变量 Key".to_string(),
        "key" if !identity.name.is_empty() => identity.name.clone(),
        "key" => identity.id.clone(),
        _ => "历史未记录 Key".to_string(),
    }
}

fn usage_json(usage: &UsageAccum) -> Value {
    let details = usage.token_details_requests >= usage.requests
        && (usage.tokens == 0 || usage.token_details_requests > 0);
    json!({
        "requests": usage.requests,
        "success": usage.successful,
        "failures": (usage.requests - usage.successful).max(0),
        "successRate": if usage.requests > 0 { Some(usage.successful as f64 / usage.requests as f64) } else { None },
        "totalTokens": usage.tokens,
        "promptTokens": if details { Some(usage.prompt_tokens) } else { None },
        "completionTokens": if details { Some(usage.completion_tokens) } else { None },
        "cacheReadTokens": if details { Some(usage.cache_read_tokens) } else { None },
        "cacheRate": if details && usage.prompt_tokens > 0 { Some(safe_rate(usage.cache_read_tokens, usage.prompt_tokens)) } else { None },
        "tokenDetailsComplete": details,
        "lastUsedAt": usage.last_used_at,
    })
}

pub(super) fn build_clients(list: &[ClientAccum]) -> Vec<Value> {
    let mut rows: Vec<_> = list.iter().filter(|item| item.usage.requests > 0 || item.usage.tokens > 0).collect();
    rows.sort_by(|left, right| right.usage.tokens.cmp(&left.usage.tokens)
        .then(right.usage.requests.cmp(&left.usage.requests)).then(left.client.cmp(&right.client)));
    rows.into_iter().map(|item| {
        let mut row = usage_json(&item.usage);
        if let Some(object) = row.as_object_mut() {
            object.insert("id".to_string(), json!(item.client));
            object.insert("label".to_string(), json!(crate::server::core::client::display_name(
                &item.client, if item.label.is_empty() { client_label(&item.client) } else { &item.label },
            )));
        }
        row
    }).collect()
}

fn build_keys(list: &[ApiKeyAccum]) -> Vec<Value> {
    let mut rows: Vec<_> = list.iter().filter(|item| item.usage.requests > 0 || item.usage.tokens > 0).collect();
    rows.sort_by(|left, right| right.usage.tokens.cmp(&left.usage.tokens)
        .then(right.usage.requests.cmp(&left.usage.requests))
        .then(left.identity.filter_value().cmp(&right.identity.filter_value())));
    rows.into_iter().map(|item| {
        let mut row = usage_json(&item.usage);
        if let Some(object) = row.as_object_mut() {
            object.insert("id".to_string(), json!(item.identity.id));
            object.insert("label".to_string(), json!(key_label(&item.identity)));
            object.insert("source".to_string(), json!(item.identity.source));
            object.insert("filterValue".to_string(), json!(item.identity.filter_value()));
        }
        row
    }).collect()
}

fn remainder(day: &DailyEntry, usages: impl Iterator<Item = UsageAccum>) -> UsageAccum {
    let mut covered = UsageAccum::default();
    for usage in usages { covered.merge(&usage); }
    UsageAccum {
        requests: (day.requests - covered.requests).max(0),
        successful: (day.successful - covered.successful).max(0),
        tokens: (day.tokens - covered.tokens).max(0),
        ..Default::default()
    }
}

pub(super) fn day_sources(day: &DailyEntry) -> (Vec<ClientAccum>, Vec<ApiKeyAccum>) {
    let mut clients = day.client_stats.clone();
    let missing = remainder(day, clients.iter().map(|item| item.usage.clone()));
    merge_client(&mut clients, "", "", &missing);
    let mut keys = day.api_key_stats.clone();
    let missing = remainder(day, keys.iter().map(|item| item.usage.clone()));
    merge_key(&mut keys, &ApiKeyAccum { usage: missing, ..Default::default() });
    (clients, keys)
}

pub(super) fn source_totals(
    daily: &BTreeMap<String, DailyEntry>, start: &str, end: &str,
) -> (Vec<ClientAccum>, Vec<ApiKeyAccum>) {
    let mut clients = Vec::new();
    let mut keys = Vec::new();
    for (_, day) in daily.range(start.to_string()..=end.to_string()) {
        let (day_clients, day_keys) = day_sources(day);
        for client in day_clients { merge_client(&mut clients, &client.client, &client.label, &client.usage); }
        for key in day_keys { merge_key(&mut keys, &key); }
    }
    (clients, keys)
}

pub(super) fn source_summary(
    daily: &BTreeMap<String, DailyEntry>, start: &str, end: &str, key: Option<&str>,
) -> Value {
    let (clients, keys) = source_totals(daily, start, end);
    let detail = key.and_then(|filter| keys.iter().find(|item| item.identity.filter_value() == filter))
        .map(|item| json!({
            "filterValue": item.identity.filter_value(),
            "label": key_label(&item.identity),
            "requests": item.usage.requests,
            "totalTokens": item.usage.tokens,
            "detailsComplete": item.clients.iter().map(|client| client.usage.requests).sum::<i64>() == item.usage.requests,
            "clients": build_clients(&item.clients),
            "models": model_rows(&item.models),
        }))
        .or_else(|| key.map(|filter| json!({ "filterValue": filter, "requests": 0, "totalTokens": 0, "detailsComplete": true, "clients": [], "models": [] })));
    json!({ "clients": build_clients(&clients), "apiKeys": build_keys(&keys), "apiKeyDetail": detail })
}

fn model_rows(models: &[ModelAccum]) -> Vec<Value> {
    let mut rows: Vec<_> = models.iter().filter(|item| item.requests > 0 || item.tokens > 0).collect();
    rows.sort_by(|left, right| right.tokens.cmp(&left.tokens).then(right.requests.cmp(&left.requests)).then(left.model.cmp(&right.model)));
    rows.into_iter().map(|item| json!({
        "model": item.model, "label": if item.model.is_empty() { "未知模型" } else { &item.model },
        "requests": item.requests, "totalTokens": item.tokens,
    })).collect()
}

pub(super) fn source_breakdown(day: &DailyEntry) -> (Vec<Value>, Vec<Value>) {
    let (clients, keys) = day_sources(day);
    let clients = build_clients(&clients).into_iter().map(|row| json!({
        "id": row["id"], "label": row["label"], "tokens": row["totalTokens"], "requests": row["requests"],
    })).collect();
    let keys = build_keys(&keys).into_iter().map(|row| json!({
        "id": row["filterValue"], "label": row["label"], "tokens": row["totalTokens"], "requests": row["requests"],
    })).collect();
    (clients, keys)
}

pub(super) fn all_model_rows(models: &[ModelAccum]) -> Vec<Value> {
    let mut rows = model_breakdown_json(models);
    for item in models.iter().filter(|item| item.requests > 0 && item.tokens == 0) {
        rows.push(json!({ "model": item.model, "label": if item.model.is_empty() { "未知模型" } else { &item.model }, "tokens": 0, "requests": item.requests }));
    }
    rows
}
