use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::{config, jobs};
use crate::server::core::account_store::AccountStore;

fn blocked() -> &'static Mutex<HashMap<String, String>> {
    static BLOCKED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    BLOCKED.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn requires_action(code: Option<&str>) -> bool {
    matches!(code, Some("account_origin_authorization_required" | "account_origin_proxy_unsupported"
        | "cline_free_refresh_registration_required" | "cline_free_refresh_requires_host"
        | "cline_free_refresh_unconfirmed" | "codearts_refresh_registration_required"
        | "codearts_refresh_unconfirmed"))
}

pub(super) fn settled(job: &jobs::Job, code: Option<&str>) {
    let Ok(mut entries) = blocked().lock() else { return; };
    if requires_action(code) {
        entries.insert(job.id.clone(), job.config.fingerprint.clone());
    } else {
        entries.remove(&job.id);
    }
}

pub(super) fn waiting_for_change(store: &AccountStore, id: &str) -> bool {
    let previous = blocked().lock().ok().and_then(|entries| entries.get(id).cloned());
    let Some(previous) = previous else { return false; };
    let guard = store.guard();
    let current = store.record_by_id(&guard, id).map(|record| config::fingerprint(&record));
    drop(guard);
    if current.as_deref() == Some(previous.as_str()) { return true; }
    if let Ok(mut entries) = blocked().lock() {
        if entries.get(id) == Some(&previous) { entries.remove(id); }
    }
    false
}
