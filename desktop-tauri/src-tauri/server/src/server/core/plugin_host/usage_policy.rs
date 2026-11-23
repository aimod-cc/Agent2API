//! 解析并缓存插件在握手阶段申报的余额查询并发与间隔策略。
//!
//! 只消费受信任的插件握手 JSON，并对并发上限与最小请求间隔做安全裁剪。

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

const MAX_CONCURRENCY: usize = 64;
const MAX_MIN_INTERVAL_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UsageQueryPolicy {
    pub concurrency: Option<usize>,
    pub min_interval_ms: u64,
}

#[derive(Deserialize)]
struct RawPolicy {
    #[serde(default)]
    concurrency: Option<usize>,
    #[serde(default, alias = "minIntervalMs")]
    min_interval_ms: Option<u64>,
}

fn parse_provider(item: &Value) -> Option<UsageQueryPolicy> {
    let raw = item.get("usage_query").or_else(|| item.get("usageQuery"))?;
    let parsed: RawPolicy = serde_json::from_value(raw.clone()).ok()?;
    let concurrency = parsed.concurrency.map(|value| value.clamp(1, MAX_CONCURRENCY));
    let min_interval_ms = parsed.min_interval_ms.unwrap_or(0).min(MAX_MIN_INTERVAL_MS);
    (concurrency.is_some() || min_interval_ms > 0).then_some(UsageQueryPolicy {
        concurrency,
        min_interval_ms,
    })
}

pub(super) fn parse(value: &Value) -> BTreeMap<String, UsageQueryPolicy> {
    let mut map = BTreeMap::new();
    let Some(items) = value.as_array() else { return map };
    for item in items {
        let Some(id) = item["id"].as_str().map(str::trim).filter(|id| !id.is_empty() && id.len() <= 128) else {
            continue;
        };
        if let Some(policy) = parse_provider(item) {
            map.insert(id.to_string(), policy);
        }
    }
    map
}

pub(crate) fn for_provider(provider: &str) -> UsageQueryPolicy {
    let Some(host) = super::host_for_provider(provider) else {
        return UsageQueryPolicy::default();
    };
    host.usage_policies
        .lock()
        .ok()
        .and_then(|map| map.get(provider).copied())
        .unwrap_or_default()
}
