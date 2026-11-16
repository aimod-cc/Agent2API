use serde_json::Value;

use super::{counter, groups, sources, types::*};
use crate::server::core::{custom_providers, providers};

#[cfg(test)]
#[path = "evaluate_tests.rs"]
mod tests;

pub fn compare(value: f64, condition: &Condition) -> bool {
    match condition.comparison {
        Comparison::Lt => value < condition.threshold,
        Comparison::Lte => value <= condition.threshold,
        Comparison::Gte => value >= condition.threshold,
    }
}

pub fn read(account: &Value, condition: &Condition, facts: &sources::Facts, counts: &counter::Facts, now: i64) -> Reading {
    let id = account.get("id").and_then(Value::as_str).unwrap_or("");
    let record = facts.get(id).filter(|facts| sources::current_for(account, facts));
    let source = record.and_then(|record| record.sources.iter().find(|source| source.id == condition.source_id));
    let metadata = source.or(condition.source.as_ref());
    let mut reading = Reading::default();
    if let Some(source) = metadata {
        if !source.channel.is_empty() && source.channel != counter::channel_of(account) {
            reading.reason = "此额度不属于账号当前使用的通道".to_string();
            return reading;
        }
        if source.starts_at > now || (source.ends_at > 0 && now >= source.ends_at) {
            reading.reason = "套餐尚未生效或已经到期".to_string();
            return reading;
        }
        if source.unlimited && !matches!(condition.metric, Metric::Tokens | Metric::Requests) {
            reading.reason = "上游额度无上限".to_string();
            return reading;
        }
    }
    let value = if matches!(condition.metric, Metric::Tokens | Metric::Requests) {
        reading.local = true;
        let Some(window) = counter::window_for(condition) else { return reading };
        if window.starts_at > now || (window.ends_at > 0 && now >= window.ends_at) {
            reading.reason = "统计周期尚未开始或已经结束".to_string();
            return reading;
        }
        let (start, end) = counter::window_bounds(&window, now);
        reading.window_start = start;
        reading.resets_at = if window.reset == Reset::Rolling { 0 } else { end };
        let count = counts.get(&(id.to_string(), counter::key(condition)))
            .filter(|count| window.reset == Reset::Rolling || count.start == start);
        if let Some(count) = count {
            let models = counter::models_for(condition);
            if !models.is_empty() {
                for model in models {
                    let value = count.by_model.get(&model.to_ascii_lowercase())
                        .map(|count| if condition.metric == Metric::Tokens { count.tokens as f64 } else { count.requests as f64 })
                        .unwrap_or(0.0);
                    reading.model_values.insert(model.clone(), value);
                    if compare(value, condition) { reading.models.push(model.clone()); }
                }
                reading.model_values.values().copied().reduce(f64::max)
            } else {
                Some(if condition.metric == Metric::Tokens { count.tokens as f64 } else { count.requests as f64 })
            }
        } else { None }
    } else if condition.source_id.is_empty() || condition.source_id == "account" {
        record.and_then(|record| record.remaining)
    } else {
        source.filter(|source| source.live_at(now)).and_then(|source| {
            reading.resets_at = source.resets_at;
            match condition.metric {
                Metric::Remaining => source.remaining,
                Metric::Used => source.used,
                Metric::RemainingPercent => source.remaining_percent.or_else(|| {
                    Some(source.remaining? / source.total.filter(|total| *total > 0.0)? * 100.0)
                }),
                Metric::UsedPercent => source.remaining_percent.map(|remaining| 100.0 - remaining).or_else(|| {
                    Some(source.used? / source.total.filter(|total| *total > 0.0)? * 100.0)
                }),
                _ => None,
            }
        })
    };
    reading.value = value.filter(|value| value.is_finite());
    reading.known = reading.value.is_some();
    reading.hit = if reading.local && !counter::models_for(condition).is_empty() {
        !reading.models.is_empty()
    } else {
        reading.value.is_some_and(|value| compare(value, condition))
    };
    if !reading.local && reading.hit { reading.models = counter::models_for(condition).to_vec(); }
    if !reading.known { reading.reason = "暂无可判定的额度读数".to_string(); }
    reading
}

pub fn state(account: &Value, group: &Group, facts: &sources::Facts, counts: &counter::Facts, now: i64) -> GroupState {
    let readings: Vec<Reading> = group.conditions.iter().map(|condition| read(account, condition, facts, counts, now)).collect();
    // 与优先于或，只有成立的合取分支才能贡献拦截范围。
    let mut branches: Vec<Vec<usize>> = Vec::new();
    for (index, condition) in group.conditions.iter().enumerate() {
        if index == 0 || condition.join == Join::Or { branches.push(Vec::new()); }
        if let Some(branch) = branches.last_mut() { branch.push(index); }
    }
    let mut hit = false;
    let mut account_hit = false;
    let mut models = Vec::new();
    if group.enabled {
        for branch in branches {
            if !branch.iter().all(|index| readings[*index].hit) { continue; }
            hit = true;
            let mut branch_models = Vec::new();
            for index in branch {
                for model in &readings[index].models {
                    if !branch_models.iter().any(|existing: &String| existing.eq_ignore_ascii_case(model)) {
                        branch_models.push(model.clone());
                    }
                }
            }
            if branch_models.is_empty() { account_hit = true; }
            for model in branch_models {
                if !models.iter().any(|existing: &String| existing.eq_ignore_ascii_case(&model)) {
                    models.push(model);
                }
            }
        }
    }
    GroupState { hit, account: hit && (group.target == Target::Account || account_hit), models, conditions: readings }
}

/// 模型条目里的倍率读数；`None` = 目录没有可判定的计费信息。
///
/// 字段优先级：`billingEffectiveMultiplier` > `billingMultiplier` >
/// `pointsMultiplier` > `multiplier` > `rate` > `credits` 展示串。
/// 0 与负数都按「免费」处理（负数只可能是上游异常读数，按不消耗积分更安全）；
/// 非有限值直接丢弃，避免把 NaN / Infinity 当成免费。
fn multiplier_of(model: &Value) -> Option<f64> {
    for key in [
        "billingEffectiveMultiplier",
        "billingMultiplier",
        "pointsMultiplier",
        "multiplier",
        "rate",
    ] {
        if let Some(value) = model.get(key).and_then(Value::as_f64) {
            if value.is_finite() {
                return Some(value);
            }
        }
    }
    // 展示串形态：`x0.00 credits` / `x0.0` / `x0.5 credits` / `0.7x` / `免费`。
    // `creditMultiplier` 是 Tabbit 的字段名；`credits` 是其余各家的通用字段。
    for key in ["credits", "creditMultiplier"] {
        let Some(value) = model.get(key) else { continue };
        if let Some(number) = value.as_f64() {
            if number.is_finite() {
                return Some(number);
            }
        }
        let Some(text) = value.as_str() else { continue };
        let text = text.trim();
        if text == "免费" {
            return Some(0.0);
        }
        let text = text
            .strip_prefix('x')
            .or_else(|| text.strip_prefix('X'))
            .unwrap_or(text)
            .trim();
        let Some(number) = text.split_whitespace().next() else { continue };
        let number = number
            .strip_suffix('x')
            .or_else(|| number.strip_suffix('X'))
            .unwrap_or(number);
        if let Ok(value) = number.parse::<f64>() {
            if value.is_finite() {
                return Some(value);
            }
        }
    }
    None
}

/// 某家当前生效的模型清单：自定义家走自己的模型表，其余走提供商目录。
fn manifest(provider: &str) -> Vec<Value> {
    if custom_providers::is_custom_provider_id(provider) {
        custom_providers::models_of(provider)
    } else {
        providers::catalog::manifest_for_provider(provider)
    }
}

/// 某家上游模型当前的倍率；目录里找不到该模型时返回 `None`。
fn model_multiplier(provider: &str, model: &str) -> Option<f64> {
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let manifest = manifest(provider);
    let entry = manifest.iter().find(|entry| {
        entry
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.eq_ignore_ascii_case(model))
            || entry
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(model))
    })?;
    multiplier_of(entry)
}

/// 倍率 0 的限时免费模型：余额/积分限制不应生效。
fn is_free_model(provider: &str, model: &str) -> bool {
    model_multiplier(provider, model).is_some_and(|multiplier| multiplier <= 0.0)
}

/// 命中里是否只有余额/额度条件。
///
/// `local = true` 的是 Token / 请求数这类**本地消耗保护**，不是账号积分；
/// 免费模型可以豁免前者，但不该豁免后者。
fn only_balance_conditions(conditions: &[Reading]) -> bool {
    conditions.iter().filter(|reading| reading.hit).all(|reading| !reading.local)
}

pub fn blocked(account: &Value, model: &str, facts: &sources::Facts, counts: &counter::Facts, now: i64) -> bool {
    let provider = crate::server::core::routing::provider_of(account);
    // 目录查询只在「确实有余额条件命中」时发生一次：多数账号不会走到这里。
    let mut free: Option<bool> = None;
    groups::effective_groups(account).iter().any(|group| {
        if group.action != Action::Skip { return false; }
        let state = state(account, group, facts, counts, now);
        if !state.hit { return false; }
        let matched = state.account
            || (!model.is_empty() && state.models.iter().any(|wanted| wanted.eq_ignore_ascii_case(model)));
        if !matched { return false; }
        // 倍率 0 的限时免费模型不消耗账号积分：纯余额/额度条件的命中不生效。
        // Token / 请求数限制仍然生效（那是本地保护，与积分无关）。
        if !model.is_empty() && only_balance_conditions(&state.conditions) {
            let is_free = *free.get_or_insert_with(|| is_free_model(provider, model));
            if is_free { return false; }
        }
        true
    })
}

pub fn disable_hit(account: &Value, facts: &sources::Facts, counts: &counter::Facts, now: i64) -> bool {
    groups::effective_groups(account).iter().any(|group| {
        group.action == Action::Disable && state(account, group, facts, counts, now).hit
    })
}

pub fn states(account: &Value, facts: &sources::Facts, counts: &counter::Facts, now: i64) -> Vec<GroupState> {
    groups::effective_groups(account).iter().map(|group| state(account, group, facts, counts, now)).collect()
}
