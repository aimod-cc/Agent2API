use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{patch_entry, Schedule};
use crate::server::core::{auto_checkin::times, beijing};
use crate::server::logging;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountPlan {
    pub date: String,
    pub accounts: BTreeMap<String, PlannedAccount>,
    pub windows: BTreeMap<String, (i64, i64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlannedAccount {
    pub provider: String,
    pub at: i64,
    pub attempted_at: Option<i64>,
}

impl AccountPlan {
    pub(super) fn from_value(value: Option<&Value>) -> Self {
        value
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }

    pub fn due(&self, now: i64) -> Vec<String> {
        if self.date != beijing::today_key() {
            return Vec::new();
        }
        let mut due: Vec<_> = self
            .accounts
            .iter()
            .filter(|(_, account)| {
                account.at <= now
                    && account.attempted_at.is_none()
                    && crate::server::core::plugin_host::serves_provider(&account.provider)
            })
            .map(|(id, account)| (account.at, id.clone()))
            .collect();
        due.sort();
        due.into_iter().map(|(_, id)| id).collect()
    }

    pub fn pending_for(&self, provider: &str) -> bool {
        self.accounts
            .values()
            .any(|account| account.provider == provider && account.attempted_at.is_none())
    }

    pub fn next_at(&self) -> Option<i64> {
        self.accounts
            .values()
            .filter(|account| {
                account.attempted_at.is_none()
                    && crate::server::core::plugin_host::serves_provider(&account.provider)
            })
            .map(|account| account.at)
            .min()
    }
}

pub fn prepare(
    schedule: &Schedule,
    targets: &[Value],
    now: DateTime<FixedOffset>,
) -> Result<AccountPlan, String> {
    let date = beijing::date_key(now);
    let mut plan = if schedule.account_plan.date == date {
        schedule.account_plan.clone()
    } else {
        AccountPlan {
            date: date.clone(),
            ..Default::default()
        }
    };
    let windows: BTreeMap<String, (i64, i64)> = schedule
        .providers
        .iter()
        .filter_map(|provider| {
            times::provider_window(schedule, provider, now).map(|window| (provider.clone(), window))
        })
        .collect();
    let changed: BTreeSet<String> = windows
        .iter()
        .filter(|(provider, window)| {
            plan.windows
                .get(*provider)
                .is_some_and(|previous| previous != *window)
        })
        .map(|(provider, _)| provider.clone())
        .collect();
    for account in targets {
        let Some(id) = account.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(provider) = account.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let Some(&(start, end)) = windows.get(provider) else {
            continue;
        };
        if let Some(until) = account.get("dailyCheckin")
            .and_then(crate::server::core::account_store::checkin_state::rolling_until)
        {
            let existing = plan.accounts.get(id);
            let at = until.max(start);
            let attempted_at = existing.filter(|existing| existing.provider == provider)
                .and_then(|existing| existing.attempted_at).filter(|attempted| *attempted >= until);
            plan.accounts.insert(id.to_string(), PlannedAccount { provider: provider.into(), at, attempted_at });
            continue;
        }
        let completed = (schedule.account_plan.date != date
            && times::main_fired(schedule, provider, &date))
            || crate::server::core::account_store::checkin_state::checked_in_today(account);
        if let Some(existing) = plan.accounts.get_mut(id) {
            if existing.provider == provider {
                if completed && existing.attempted_at.is_none() {
                    existing.attempted_at = Some(now.timestamp_millis());
                }
                if existing.attempted_at.is_some()
                    || (!changed.contains(provider) && (start..=end).contains(&existing.at))
                {
                    continue;
                }
            }
        }
        // 区间内首次启用或新加入的账号，分配到剩余区间，避免启动时集中执行。
        let lower = start.max(now.timestamp_millis().min(end));
        let at = if completed {
            start
        } else {
            random_time(lower, end)?
        };
        plan.accounts.insert(
            id.to_string(),
            PlannedAccount {
                provider: provider.to_string(),
                at,
                attempted_at: completed.then_some(now.timestamp_millis()),
            },
        );
    }
    let ids: BTreeSet<&str> = targets
        .iter()
        .filter_map(|account| account.get("id").and_then(Value::as_str))
        .collect();
    plan.accounts.retain(|id, account| {
        (ids.contains(id.as_str()) && windows.contains_key(&account.provider))
            || account.attempted_at.is_some()
            || windows.contains_key(&account.provider)
                && !crate::server::core::plugin_host::serves_provider(&account.provider)
    });
    plan.windows = windows;
    Ok(plan)
}

fn random_time(start: i64, end: i64) -> Result<i64, String> {
    let slots = (end - start).max(0) as u64 + 1;
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|_| "无法生成随机签到时间".to_string())?;
    Ok(start + (u64::from_le_bytes(bytes) % slots) as i64)
}

pub fn prepare_saved(
    id: &str,
    targets: &[Value],
    now: DateTime<FixedOffset>,
) -> Result<(), String> {
    let mut outcome = Ok(());
    patch_entry(id, |schedule| match prepare(schedule, targets, now) {
        Ok(plan) if plan != schedule.account_plan => schedule.account_plan = plan,
        Ok(_) => {}
        Err(error) => outcome = Err(error),
    });
    outcome
}

pub fn take_due(id: &str, account_id: &str, date: &str) -> Option<String> {
    let mut provider = None;
    patch_entry(id, |schedule| {
        let now = beijing::now();
        if !schedule.enabled
            || schedule.account_plan.date != date
            || beijing::date_key(now) != date
            || !crate::server::core::auto_checkin::master_enabled()
        {
            return;
        }
        let Some(account) = schedule.account_plan.accounts.get(account_id) else {
            return;
        };
        if account.attempted_at.is_some()
            || account.at > now.timestamp_millis()
            || !schedule.providers.contains(&account.provider)
            || !crate::server::core::plugin_host::serves_provider(&account.provider)
            || schedule
                .account_plan
                .windows
                .get(&account.provider)
                .copied()
                != times::provider_window(schedule, &account.provider, now)
        {
            return;
        }
        provider = Some(account.provider.clone());
        if let Some(account) = schedule.account_plan.accounts.get_mut(account_id) {
            account.attempted_at = Some(now.timestamp_millis());
        }
    });
    provider
}

pub fn mark_attempted(id: &str, account_ids: &[String], date: &str) {
    patch_entry(id, |schedule| {
        if schedule.account_plan.date != date || beijing::today_key() != date {
            return;
        }
        for id in account_ids {
            if let Some(account) = schedule.account_plan.accounts.get_mut(id) {
                account.attempted_at = Some(logging::now_ms());
            }
        }
    });
}

pub fn mark_result(id: &str, result: &Value, date: &str) {
    let ids: Vec<String> = result
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    mark_attempted(id, &ids, date);
}

pub fn finish_providers(id: &str, providers: &[String], date: &str) {
    patch_entry(id, |schedule| {
        if schedule.account_plan.date != date {
            return;
        }
        for provider in providers {
            if !schedule.account_plan.pending_for(provider) {
                schedule
                    .last_fired_providers
                    .insert(provider.clone(), date.to_string());
            }
        }
        schedule.last_fired_date = Some(date.to_string());
    });
}

pub fn row_state(schedule: &Schedule, provider: &str, now: i64) -> Value {
    let accounts: Vec<_> = schedule
        .account_plan
        .accounts
        .values()
        .filter(|account| account.provider == provider)
        .collect();
    let pending: Vec<_> = accounts
        .iter()
        .filter(|account| account.attempted_at.is_none())
        .collect();
    serde_json::json!({
        "accountCount": accounts.len(),
        "attemptedCount": accounts.len() - pending.len(),
        "pendingCount": pending.len(),
        "overdue": pending.iter().any(|account| account.at <= now),
        "nextAccountAt": pending.iter().map(|account| account.at).min(),
    })
}
