use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;

use super::jobs::Job;
use crate::server::core::account_store::AccountStore;

struct Entry { store: AccountStore, job: Arc<Job>, provider: String }
#[derive(Default)]
struct Queue { entries: VecDeque<Entry>, started: bool }

fn queue() -> &'static Mutex<Queue> {
    static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
    QUEUE.get_or_init(Mutex::default)
}

pub(super) fn submit(store: AccountStore, job: Arc<Job>) {
    let start = {
        let mut queue = queue().lock().unwrap_or_else(|error| error.into_inner());
        queue.entries.retain(|entry| !entry.job.cancel.is_cancelled());
        let provider = crate::server::core::routing::provider_of(&job.account).to_string();
        queue.entries.push_back(Entry { store, job, provider });
        if queue.started { false } else { queue.started = true; true }
    };
    if start { crate::spawn_task(worker()); }
}

async fn worker() {
    let mut active = futures::stream::FuturesUnordered::new();
    let mut providers: HashMap<String, usize> = HashMap::new();
    loop {
        let limits = crate::server::core::runtime_capacity::limits();
        let entries = {
            let mut queue = queue().lock().unwrap_or_else(|error| error.into_inner());
            queue.entries.retain(|entry| !entry.job.cancel.is_cancelled());
            let mut entries = Vec::new();
            while active.len() + entries.len() < limits.background {
                let Some(index) = queue.entries.iter().position(|entry|
                    providers.get(&entry.provider).copied().unwrap_or(0) < limits.background_per_provider) else { break; };
                let Some(entry) = queue.entries.remove(index) else { break; };
                *providers.entry(entry.provider.clone()).or_default() += 1;
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
        tokio::select! {
            provider = active.next(), if !active.is_empty() => {
                if let Some(provider) = provider {
                    if let Some(count) = providers.get_mut(&provider) {
                        *count = count.saturating_sub(1);
                        if *count == 0 { providers.remove(&provider); }
                    }
                }
            },
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}
