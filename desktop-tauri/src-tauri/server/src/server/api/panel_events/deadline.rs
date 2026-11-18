use crate::server::core::{account_store::AccountStore, limiter};

pub(super) fn next_change(store: &AccountStore) -> Option<i64> {
    let now = crate::server::logging::now_ms();
    let mut next = None;
    let mut include = |at: i64| {
        if at > now { next = Some(next.map_or(at, |current: i64| current.min(at))); }
    };
    for account in &store.routing_snapshot().public_accounts {
        for key in ["expiresAt", "tokenExpiresAt", "tokenExpireAt", "accessTokenExpiresAt", "refreshTokenExpiresAt",
            "upstreamCooldownUntil"] {
            if let Some(value) = account.get(key).and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok())) {
                if value.is_finite() && value > 0.0 {
                    include(if value < 1e11 { (value * 1000.0) as i64 } else { value as i64 });
                }
            }
        }
        for limit in account.get("rateLimits").and_then(serde_json::Value::as_object).into_iter().flat_map(|limits| limits.values()) {
            if let Some(at) = limit.get("resetAt").and_then(serde_json::Value::as_i64) { include(at); }
        }
        for group in limiter::groups::effective_groups(account) {
            for condition in group.conditions {
                if let Some(window) = limiter::counter::window_for(&condition) {
                    include(window.starts_at);
                    include(window.ends_at);
                    if !matches!(window.reset, limiter::types::Reset::Rolling | limiter::types::Reset::Total) {
                        include(limiter::counter::window_bounds(&window, now).1);
                    }
                }
            }
        }
    }
    for facts in limiter::sources::facts().values() {
        for source in &facts.sources {
            include(source.starts_at);
            include(source.ends_at);
            include(source.resets_at);
        }
    }
    if let Some(at) = limiter::counter::next_change(now) { include(at); }
    for record in crate::server::core::usage_records::cached_snapshot().values() {
        if let Some(credits) = record.usage.as_ref().and_then(|usage| usage.get("credits")).and_then(serde_json::Value::as_array) {
            for credit in credits {
                if let Some(at) = credit.get("expireAt").and_then(serde_json::Value::as_i64) { include(at); }
            }
        }
    }
    next
}
