use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::server::{config, logging};

use super::source::Candidate;
use super::token;

const PREFERENCE_MS: i64 = 10 * 60_000;
const FAILURE_MS: i64 = 60_000;
const MAX_RECORDS: usize = 256;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub source: String,
    pub label: String,
    pub resource: String,
    pub state: String,
    pub elapsed_ms: Option<u64>,
    pub checked_at: i64,
    pub selected: bool,
    #[serde(skip)]
    context: String,
    #[serde(skip)]
    until: i64,
}

#[derive(Default)]
struct Memory {
    observations: HashMap<(String, String), Observation>,
    gates: HashMap<String, Weak<tokio::sync::Mutex<()>>>,
}

fn memory() -> &'static Mutex<Memory> {
    static MEMORY: OnceLock<Mutex<Memory>> = OnceLock::new();
    MEMORY.get_or_init(|| Mutex::new(Memory::default()))
}

pub fn context_key() -> String {
    context_key_for(&config::github_access())
}

pub fn context_key_for(settings: &config::GithubAccessSettings) -> String {
    let bytes = serde_json::to_vec(&(
        settings,
        config::update_proxy(),
        token::effective_token(),
        super::client::egress_identity(),
    )).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

pub fn request_key(value: &str, context: &str) -> String {
    let url = url::Url::parse(value).ok();
    let host = url.as_ref().and_then(|url| url.host_str()).unwrap_or("");
    let parts: Vec<_> = url.as_ref().map(|url| url.path().split('/').filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    let skip = usize::from(host == "api.github.com");
    let repo = parts.iter().skip(skip).take(2).copied().collect::<Vec<_>>().join("/");
    let bytes = serde_json::to_vec(&(context, host, repo, resource(value))).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

pub fn resource(value: &str) -> &'static str {
    if super::source::is_official_api(value) {
        "api"
    } else if url::Url::parse(value).is_ok_and(|url| url.host_str() == Some("raw.githubusercontent.com")) {
        "index"
    } else {
        "download"
    }
}

pub fn clear() {
    if let Ok(mut memory) = memory().lock() {
        memory.observations.clear();
    }
}

pub fn gate(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut memory = memory().lock().unwrap_or_else(|error| error.into_inner());
    memory.gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = memory.gates.get(key).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    memory.gates.insert(key.to_string(), Arc::downgrade(&gate));
    gate
}

pub fn record(candidate: &Candidate, state: &str, elapsed_ms: Option<u64>, selected: bool) {
    let now = logging::now_ms();
    let Ok(mut memory) = memory().lock() else { return; };
    memory.observations.retain(|_, value| now - value.checked_at < 60 * 60_000);
    if selected {
        for ((key, _), value) in &mut memory.observations {
            if key == &candidate.key { value.selected = false; }
        }
    }
    let key = (candidate.key.clone(), candidate.id.clone());
    let previous = memory.observations.get(&key);
    let selected = selected || (state == "ok" && previous.is_some_and(|value| value.selected && value.until > now));
    let elapsed_ms = elapsed_ms.or_else(|| previous.and_then(|value| value.elapsed_ms));
    let until = previous.filter(|value| value.state == "ok" && value.until > now && state == "ok")
        .map(|value| value.until)
        .unwrap_or(now + if state == "ok" { PREFERENCE_MS } else { FAILURE_MS });
    memory.observations.insert(key, Observation {
        source: candidate.id.clone(),
        label: candidate.label(),
        resource: resource(&candidate.original).to_string(),
        state: state.to_string(),
        elapsed_ms,
        checked_at: now,
        selected,
        context: candidate.context.clone(),
        until,
    });
    if memory.observations.len() > MAX_RECORDS {
        if let Some(key) = memory.observations.iter().min_by_key(|(_, value)| value.checked_at).map(|(key, _)| key.clone()) {
            memory.observations.remove(&key);
        }
    }
}

pub fn state(candidate: &Candidate) -> Option<Observation> {
    let memory = memory().lock().ok()?;
    memory.observations.get(&(candidate.key.clone(), candidate.id.clone()))
        .filter(|value| value.until > logging::now_ms()).cloned()
}

pub fn remembered(candidates: &[Candidate]) -> bool {
    candidates.iter().any(|candidate| {
        state(candidate).is_some_and(|value| value.state == "ok" && value.selected)
    })
}

pub fn sort(candidates: &mut [Candidate]) {
    candidates.sort_by_cached_key(|candidate| match state(candidate) {
        Some(value) if value.state == "ok" => (u8::from(!value.selected), value.elapsed_ms.unwrap_or(u64::MAX)),
        None => (2, 0),
        Some(_) => (3, 0),
    });
}

pub fn snapshot() -> Vec<Observation> {
    let context = context_key();
    let now = logging::now_ms();
    let Ok(memory) = memory().lock() else { return vec![]; };
    let mut latest = HashMap::new();
    for value in memory.observations.values().filter(|value| value.context == context && now - value.checked_at < 60 * 60_000) {
        let key = (value.source.clone(), value.resource.clone());
        let entry = latest.entry(key).or_insert_with(|| value.clone());
        if value.checked_at > entry.checked_at { *entry = value.clone(); }
    }
    let mut result: Vec<_> = latest.into_values().collect();
    result.sort_by(|left, right| right.checked_at.cmp(&left.checked_at));
    result
}
