use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::core::account_store::AccountStore;
use crate::server::core::models::model_id;

pub(crate) struct ValidationSnapshot {
    pub available: bool,
    pub models: Vec<String>,
}

pub(crate) struct ValidationCache {
    providers: std::collections::HashSet<String>,
    generation: u64,
    built_at: Instant,
    snapshot: Arc<ValidationSnapshot>,
}

pub(crate) fn snapshot(store: &AccountStore, cache: &Mutex<Option<ValidationCache>>) -> Arc<ValidationSnapshot> {
    let accounts = store.routing_snapshot();
    let generation = super::generation();
    if let Ok(cached) = cache.lock() {
        if let Some(cached) = cached.as_ref().filter(|cached| {
            cached.generation == generation && cached.providers == accounts.available_providers
                && cached.built_at.elapsed() < Duration::from_secs(30)
        }) {
            return cached.snapshot.clone();
        }
    }
    let active = super::super::active_manifests(store);
    let available = !active.is_empty() || crate::server::core::providers::executable_provider_ids()
        .iter().any(|id| accounts.available_providers.contains(id));
    let body = super::super::view::models_response_with_active(store, None, active);
    let models = body.get("data").and_then(serde_json::Value::as_array)
        .map(|models| models.iter().map(model_id).collect()).unwrap_or_default();
    let snapshot = Arc::new(ValidationSnapshot { available, models });
    if super::generation() == generation && Arc::ptr_eq(&accounts, &store.routing_snapshot()) {
        if let Ok(mut cached) = cache.lock() {
            *cached = Some(ValidationCache { providers: accounts.available_providers.clone(), generation,
                built_at: Instant::now(), snapshot: snapshot.clone() });
        }
    }
    snapshot
}
