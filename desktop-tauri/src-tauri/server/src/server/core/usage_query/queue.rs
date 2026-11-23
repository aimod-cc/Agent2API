use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::Notify;

use super::jobs::Job;
use crate::server::core::account_store::AccountStore;

struct Entry { store: AccountStore, job: Arc<Job>, provider: String }
#[derive(Default)]
struct Queue { entries: VecDeque<Entry>, started: bool }

fn queue() -> &'static Mutex<Queue> {
    static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
    QUEUE.get_or_init(Mutex::default)
}

fn wake_notify() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

pub(super) fn submit(store: AccountStore, job: Arc<Job>) {
    let start = {
        let mut queue = queue().lock().unwrap_or_else(|error| error.into_inner());
        queue.entries.retain(|entry| !entry.job.cancel.is_cancelled());
        let provider = crate::server::core::routing::provider_of(&job.account).to_string();
        queue.entries.push_back(Entry { store, job, provider });
        if queue.started { false } else { queue.started = true; true }
    };
    if start {
        crate::spawn_task(worker());
    } else {
        wake_notify().notify_one();
    }
}

async fn worker() {
    let mut active = futures::stream::FuturesUnordered::new();
    let mut providers: HashMap<String, usize> = HashMap::new();
    let mut next_ready: HashMap<String, tokio::time::Instant> = HashMap::new();
    loop {
        let now = tokio::time::Instant::now();
        let limits = crate::server::core::runtime_capacity::limits();
        let mut next_wake: Option<tokio::time::Instant> = None;
        let entries = {
            let mut queue = queue().lock().unwrap_or_else(|error| error.into_inner());
            queue.entries.retain(|entry| !entry.job.cancel.is_cancelled());
            let mut entries = Vec::new();
            while active.len() + entries.len() < limits.background {
                let mut chosen = None;
                for (index, entry) in queue.entries.iter().enumerate() {
                    let policy = crate::server::core::plugin_host::usage_policy::for_provider(&entry.provider);
                    let provider_cap = policy
                        .concurrency
                        .map_or(limits.background_per_provider, |cap| limits.background_per_provider.min(cap));
                    if providers.get(&entry.provider).copied().unwrap_or(0) >= provider_cap {
                        continue;
                    }
                    if let Some(ready_at) = next_ready.get(&entry.provider) {
                        if *ready_at > now {
                            next_wake = Some(match next_wake {
                                Some(current) => current.min(*ready_at),
                                None => *ready_at,
                            });
                            continue;
                        }
                    }
                    chosen = Some((index, policy.min_interval_ms));
                    break;
                }
                let Some((index, min_interval_ms)) = chosen else { break; };
                let Some(entry) = queue.entries.remove(index) else { break; };
                *providers.entry(entry.provider.clone()).or_default() += 1;
                if min_interval_ms > 0 {
                    next_ready.insert(entry.provider.clone(), now + Duration::from_millis(min_interval_ms));
                } else {
                    next_ready.remove(&entry.provider);
                }
                entries.push(entry);
            }
            if queue.entries.is_empty() && active.is_empty() && entries.is_empty() {
                queue.started = false;
                return;
            }
            entries
        };
        for entry in entries {
            active.push(async move {
                super::run::execute(entry.store, entry.job).await;
                entry.provider
            });
        }
        let sleep_for = next_wake
            .map(|wake| wake.saturating_duration_since(tokio::time::Instant::now()).clamp(Duration::from_millis(10), Duration::from_millis(100)))
            .unwrap_or(Duration::from_millis(100));
        tokio::select! {
            provider = active.next(), if !active.is_empty() => {
                if let Some(provider) = provider {
                    if let Some(count) = providers.get_mut(&provider) {
                        *count = count.saturating_sub(1);
                        if *count == 0 { providers.remove(&provider); }
                    }
                    let interval = crate::server::core::plugin_host::usage_policy::for_provider(&provider).min_interval_ms;
                    if interval > 0 {
                        next_ready.insert(provider, tokio::time::Instant::now() + Duration::from_millis(interval));
                    } else {
                        next_ready.remove(&provider);
                    }
                }
            },
            _ = wake_notify().notified() => {},
            _ = tokio::time::sleep(sleep_for) => {},
        }
    }
}
