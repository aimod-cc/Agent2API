use serde_json::{json, Value};

use crate::server::config::error_policy::{CooldownMode, CooldownScope, ErrorKind, ErrorRule};
use crate::server::logging;

use super::{sql, state::StoredAccount, store::AccountStore, store_util::truncate_chars};

const PROVIDER_STATE: &str = "routingProviderCooldowns";
const DEFAULT_RETAIN_MS: i64 = 1_800_000;
const BALANCE_RETAIN_MS: i64 = 21_600_000;

fn retain_window_ms(entry: &Value) -> i64 {
    if entry.get("kind").and_then(Value::as_str) == Some("balance") {
        BALANCE_RETAIN_MS
    } else {
        DEFAULT_RETAIN_MS
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BalanceCooldownReconcile {
    Unchanged,
    BaselineUpdated(f64),
    Cleared {
        from: Option<f64>,
        to: Option<f64>,
        unlimited: bool,
    },
}

/// 余额查询成功后，核对账号级「余额不足暂停」标记：
/// - 余额转为无上限，或读数较基线明显回升（且大于 0）→ 自动解除暂停；
/// - 读数低于基线（旧缓存追平为真实低余额 / 0）或此前无基线 → 更新基线水位，保持暂停，
///   以便后续充值 / 签到回升时立即识别并自愈；
/// - 读数与基线持平（例如上游接口持续返回偏高旧数）→ 保持暂停，避免每分钟反复解封撞 402。
pub(crate) fn reconcile_balance_cooldown(
    record: &mut StoredAccount,
    remaining: Option<f64>,
    unlimited: bool,
    now: i64,
) -> BalanceCooldownReconcile {
    let Some(mut limits) = record
        .get("routingCooldowns")
        .filter(|value| value.is_object())
        .cloned()
    else {
        return BalanceCooldownReconcile::Unchanged;
    };
    let Some(entry) = limits.get("@account") else {
        return BalanceCooldownReconcile::Unchanged;
    };
    if entry.get("kind").and_then(Value::as_str) != Some("balance") {
        return BalanceCooldownReconcile::Unchanged;
    }
    let reset_at = entry
        .get("resetAt")
        .and_then(|value| value.as_i64().or_else(|| value.as_f64().filter(|v| v.is_finite()).map(|v| v as i64)))
        .unwrap_or(0);
    if reset_at <= now {
        return BalanceCooldownReconcile::Unchanged;
    }
    let baseline = entry
        .get("balanceBaseline")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite());

    const EPS: f64 = 1e-6;
    if unlimited {
        if let Some(object) = limits.as_object_mut() {
            object.remove("@account");
            if object.is_empty() {
                record.remove("routingCooldowns");
            } else {
                record.set("routingCooldowns", limits);
            }
        }
        record.set_updated_at(now);
        return BalanceCooldownReconcile::Cleared {
            from: baseline,
            to: None,
            unlimited: true,
        };
    }

    let Some(value) = remaining.filter(|value| value.is_finite()) else {
        return BalanceCooldownReconcile::Unchanged;
    };
    match baseline {
        Some(base) if value > base + EPS && value > 0.0 => {
            if let Some(object) = limits.as_object_mut() {
                object.remove("@account");
                if object.is_empty() {
                    record.remove("routingCooldowns");
                } else {
                    record.set("routingCooldowns", limits);
                }
            }
            record.set_updated_at(now);
            BalanceCooldownReconcile::Cleared {
                from: Some(base),
                to: Some(value),
                unlimited: false,
            }
        }
        Some(base) if value < base - EPS => {
            if let Some(target) = limits.get_mut("@account").and_then(Value::as_object_mut) {
                target.insert("balanceBaseline".to_string(), json!(value));
            }
            record.set("routingCooldowns", limits);
            record.set_updated_at(now);
            BalanceCooldownReconcile::BaselineUpdated(value)
        }
        None => {
            if let Some(target) = limits.get_mut("@account").and_then(Value::as_object_mut) {
                target.insert("balanceBaseline".to_string(), json!(value));
            }
            record.set("routingCooldowns", limits);
            record.set_updated_at(now);
            BalanceCooldownReconcile::BaselineUpdated(value)
        }
        _ => BalanceCooldownReconcile::Unchanged,
    }
}

/// 一次加载整份提供商共享冷却映射，供批量列表装饰和限流判定复用，避免按账号逐条查库。
pub(crate) struct PolicyCooldownState {
    providers: Value,
}

impl PolicyCooldownState {
    pub(crate) fn cooldown_for(&self, provider: &str, model: &str, now: i64) -> Option<Value> {
        pick_policy_cooldown(&self.providers, provider, model, now)
    }
}

fn pick_policy_cooldown(state: &Value, provider: &str, model: &str, now: i64) -> Option<Value> {
    let limits = state.get(provider)?;
    [limits.get(model), limits.get("@provider")]
        .into_iter()
        .flatten()
        .filter(|entry| entry["resetAt"].as_i64().is_some_and(|at| at > now))
        .max_by_key(|entry| entry["resetAt"].as_i64().unwrap_or(0))
        .cloned()
}

impl AccountStore {
    pub(crate) fn mark_policy_cooldown(&self, account_id: &str, model: &str, scope: CooldownScope,
        status: i32, code: Option<i64>, reset_at: Option<i64>, message: &str, rule: &ErrorRule, floor_ms: u64) -> Option<Value> {
        self.mark_policy_cooldown_with_baseline(account_id, model, scope, status, code, reset_at, message, rule, floor_ms, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mark_policy_cooldown_with_baseline(&self, account_id: &str, model: &str, scope: CooldownScope,
        status: i32, code: Option<i64>, reset_at: Option<i64>, message: &str, rule: &ErrorRule, floor_ms: u64,
        balance_baseline: Option<f64>) -> Option<Value> {
        let guard = self.guard();
        let mut record = self.record_by_id(&guard, account_id)?;
        let now = logging::now_ms();
        let provider = record.provider().to_string();
        let (key, previous) = match scope {
            CooldownScope::Provider => ("@provider".to_string(), self.with_conn(&guard, |conn| load_providers(conn)).ok()
                .and_then(|state| state.get(&provider).and_then(|models| models.get("@provider")).cloned())),
            CooldownScope::Account => ("@account".into(), record.get("routingCooldowns").and_then(|limits| limits.get("@account")).cloned()),
            _ => (model.to_string(), record.get("rateLimits").and_then(|limits| limits.get(model)).cloned()),
        };
        let mut entry = cooldown_entry(previous.as_ref(), status, code, reset_at, message, rule, scope, floor_ms, now);
        if rule.kind == ErrorKind::Balance && scope == CooldownScope::Account {
            let prev_baseline = previous
                .as_ref()
                .filter(|item| item.get("kind").and_then(Value::as_str) == Some("balance"))
                .and_then(|item| item.get("balanceBaseline"))
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite());
            let baseline = match (prev_baseline, balance_baseline.filter(|value| value.is_finite())) {
                (Some(prev), Some(curr)) => Some(prev.min(curr)),
                (None, Some(curr)) => Some(curr),
                (Some(prev), None) => Some(prev),
                (None, None) => None,
            };
            if let (Some(value), Some(object)) = (baseline, entry.as_object_mut()) {
                object.insert("balanceBaseline".to_string(), json!(value));
            }
        }
        let saved = match scope {
            CooldownScope::Provider => self.with_conn(&guard, |conn| {
                let mut state = load_providers(conn)?;
                if let Some(providers) = state.as_object_mut() {
                    providers.retain(|_, limits| {
                        if let Some(models) = limits.as_object_mut() {
                            models.retain(|_, entry| entry["resetAt"].as_i64().is_some_and(|at| at.saturating_add(retain_window_ms(entry)) > now));
                            !models.is_empty()
                        } else { false }
                    });
                }
                if !state.is_object() { state = json!({}); }
                if !state[&provider].is_object() { state[&provider] = json!({}); }
                state[&provider][&key] = entry.clone();
                save_providers(conn, &state)
            }).is_ok(),
            _ => {
                let field = if scope == CooldownScope::Account { "routingCooldowns" } else { "rateLimits" };
                let mut limits = record.get(field).filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
                if let Some(object) = limits.as_object_mut() {
                    object.retain(|_, entry| entry["resetAt"].as_i64().or_else(|| entry["resetAt"].as_f64().filter(|value| value.is_finite()).map(|value| value as i64))
                        .is_some_and(|at| at.saturating_add(retain_window_ms(entry)) > now));
                    if object.len() >= 1024 && !object.contains_key(&key) { return None; }
                }
                limits[&key] = entry.clone(); record.set(field, limits); record.set_updated_at(now);
                self.with_conn(&guard, |conn| sql::update_in_place(conn, &record)).is_ok()
            }
        };
        saved.then_some(entry)
    }

    pub(crate) fn policy_cooldown_snapshot(&self) -> PolicyCooldownState {
        let guard = self.guard();
        let providers = self
            .with_conn(&guard, |conn| load_providers(conn))
            .ok()
            .unwrap_or_else(|| json!({}));
        PolicyCooldownState { providers }
    }

    pub(crate) fn policy_cooldown_for(&self, provider: &str, model: &str) -> Option<Value> {
        let guard = self.guard();
        self.with_conn(&guard, |conn| load_providers(conn))
            .ok()
            .and_then(|state| pick_policy_cooldown(&state, provider, model, logging::now_ms()))
    }

    pub(crate) fn clear_provider_policy_cooldowns(&self, account_id: &str) -> Result<usize, String> {
        let guard = self.guard();
        let record = self.record_by_id(&guard, account_id).ok_or_else(|| "账号不存在".to_string())?;
        let provider = record.provider().to_string();
        self.with_conn(&guard, |conn| {
            let mut state = load_providers(conn)?;
            let removed = state.as_object_mut().and_then(|object| object.remove(&provider));
            let count = removed.as_ref().and_then(Value::as_object).map_or(0, |entries| entries.len());
            if removed.is_some() { save_providers(conn, &state)?; }
            Ok(count)
        }).map_err(|_| "提供商共享冷却清除失败".to_string())
    }

    pub(crate) fn clear_policy_cooldown(&self, account_id: &str, model: &str) -> bool {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else { return false };
        let now = logging::now_ms();
        let mut cleared_account = false;
        if let Some(mut limits) = record.get("routingCooldowns").filter(|value| value.is_object()).cloned() {
            if let Some(object) = limits.as_object_mut() {
                if object.remove("@account").is_some() {
                    cleared_account = true;
                }
                object.retain(|_, entry| entry["resetAt"].as_i64().is_some_and(|at| at > now));
            }
            if record.get("routingCooldowns") != Some(&limits) {
                if limits.as_object().is_some_and(|object| object.is_empty()) {
                    record.remove("routingCooldowns");
                } else {
                    record.set("routingCooldowns", limits);
                }
                record.set_updated_at(now);
                let _ = self.with_conn(&guard, |conn| sql::update_in_place(conn, &record));
            }
        }
        let provider = record.provider().to_string();
        let _ = self.with_conn(&guard, |conn| {
            let mut state = load_providers(conn)?;
            let before = state.clone();
            if let Some(object) = state.get_mut(&provider).and_then(Value::as_object_mut) {
                if object.get(model).is_some_and(|entry| entry["resetAt"].as_i64().unwrap_or(0) <= now) { object.remove(model); }
                if object.get("@provider").is_some_and(|entry| entry["resetAt"].as_i64().unwrap_or(0) <= now) { object.remove("@provider"); }
                if object.is_empty() { state.as_object_mut().map(|object| object.remove(&provider)); }
            }
            if state != before { save_providers(conn, &state)?; }
            Ok(())
        });
        cleared_account
    }
}

pub(crate) fn cooldown_entry(previous: Option<&Value>, status: i32, code: Option<i64>, reset_at: Option<i64>,
    message: &str, rule: &ErrorRule, scope: CooldownScope, floor_ms: u64, now: i64) -> Value {
    let previous_at = previous.and_then(|entry| entry["resetAt"].as_i64()
        .or_else(|| entry["resetAt"].as_f64().filter(|value| value.is_finite()).map(|value| value as i64))).unwrap_or(0);
    let guessed_before = previous.and_then(|entry| entry["guessed"].as_bool()).unwrap_or(false);
    let escalation_window = if rule.kind == ErrorKind::Balance { BALANCE_RETAIN_MS } else { DEFAULT_RETAIN_MS };
    let strikes = if now.saturating_sub(previous_at) <= escalation_window {
        previous.and_then(|entry| entry["strikes"].as_u64()).unwrap_or(0).saturating_add(1)
    } else { 1 };
    let given = reset_at.filter(|at| *at > now && *at <= (1_i64 << 53) - 1 && chrono::DateTime::from_timestamp_millis(*at).is_some());
    let delay = if rule.exponential { rule.fallback_seconds.saturating_mul(1u64 << strikes.saturating_sub(1).min(30)).min(rule.fallback_cap_seconds) }
        else { rule.fallback_seconds };
    let floor = now.saturating_add(floor_ms.min(i64::MAX as u64) as i64);
    let fallback = now.saturating_add(delay.saturating_mul(1000).min(i64::MAX as u64) as i64).max(floor);
    let fresh = match given {
        Some(at) if rule.cooldown == CooldownMode::Fixed => at.max(fallback),
        Some(at) => at.max(floor),
        None => fallback,
    };
    let until = if given.is_some() && guessed_before { fresh } else { fresh.max(previous_at) };
    let previous_kept = !(given.is_some() && guessed_before) && previous_at > fresh;
    let guessed = if previous_kept { guessed_before } else { given.is_none() || given.is_some_and(|at| fresh > at) };
    json!({ "status": status, "code": code, "resetAt": until, "message": truncate_chars(message, 300),
        "at": now, "strikes": strikes, "guessed": guessed, "kind": rule.kind, "ruleId": rule.id,
        "scope": scope, "timeSource": if guessed { "fallback" } else { "upstream" } })
}

fn load_providers(conn: &rusqlite::Connection) -> rusqlite::Result<Value> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = conn.query_row("SELECT value FROM kv WHERE key = ?1", [PROVIDER_STATE], |row| row.get(0)).optional()?;
    Ok(value.and_then(|value| serde_json::from_str(&value).ok()).filter(Value::is_object).unwrap_or_else(|| json!({})))
}

fn save_providers(conn: &rusqlite::Connection, value: &Value) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![PROVIDER_STATE, value.to_string()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_with_balance_pause(baseline: Option<f64>, reset_at: i64) -> StoredAccount {
        let mut entry = json!({
            "status": 402,
            "resetAt": reset_at,
            "kind": "balance",
            "scope": "account",
            "message": "积分余额不足"
        });
        if let Some(value) = baseline {
            entry["balanceBaseline"] = json!(value);
        }
        StoredAccount::from_value(json!({
            "id": "acc-1",
            "provider": "loomy",
            "enabled": true,
            "routingCooldowns": { "@account": entry }
        })).expect("stored account")
    }

    #[test]
    fn watermark_keeps_pause_when_inflated_reading_is_unchanged_and_clears_on_increase() {
        let now = 1_000_000;
        let mut record = account_with_balance_pause(Some(120.0), now + 600_000);
        // 上游持续返回偏高的 120：不能误解封
        assert_eq!(reconcile_balance_cooldown(&mut record, Some(120.0), false, now), BalanceCooldownReconcile::Unchanged);
        assert!(record.get("routingCooldowns").and_then(|v| v.get("@account")).is_some());

        // 追平为真实 0：下调基线到 0，继续暂停
        assert_eq!(reconcile_balance_cooldown(&mut record, Some(0.0), false, now + 1000), BalanceCooldownReconcile::BaselineUpdated(0.0));
        assert_eq!(record.get("routingCooldowns").and_then(|v| v.pointer("/@account/balanceBaseline")).and_then(Value::as_f64), Some(0.0));

        // 充值 / 签到回升到 25：立刻自动解除暂停
        assert_eq!(
            reconcile_balance_cooldown(&mut record, Some(25.0), false, now + 2000),
            BalanceCooldownReconcile::Cleared { from: Some(0.0), to: Some(25.0), unlimited: false }
        );
        assert!(record.get("routingCooldowns").is_none());
    }

    #[test]
    fn watermark_records_first_reading_when_baseline_was_missing() {
        let now = 1_000_000;
        let mut record = account_with_balance_pause(None, now + 600_000);
        assert_eq!(reconcile_balance_cooldown(&mut record, Some(80.0), false, now), BalanceCooldownReconcile::BaselineUpdated(80.0));
        assert_eq!(reconcile_balance_cooldown(&mut record, Some(80.0), false, now + 1000), BalanceCooldownReconcile::Unchanged);
        assert_eq!(
            reconcile_balance_cooldown(&mut record, Some(180.0), false, now + 2000),
            BalanceCooldownReconcile::Cleared { from: Some(80.0), to: Some(180.0), unlimited: false }
        );
    }
}
