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
    pub retry_at: i64,
    pub retry_source: String,
    pub last_state: Option<String>,
    #[serde(skip)]
    context: String,
    #[serde(skip)]
    request_url: String,
    #[serde(rename = "validUntil")]
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
        retry_at: 0,
        retry_source: String::new(),
        last_state: None,
        context: candidate.context.clone(),
        request_url: candidate.original.clone(),
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
    let records: Vec<_> = match memory().lock() {
        Ok(memory) => memory.observations.values()
            .filter(|value| value.context == context && now - value.checked_at < 60 * 60_000)
            .cloned().collect(),
        Err(_) => return vec![],
    };
    let states = crate::server::core::task_state::read_all().unwrap_or_default();
    let headers = super::client::github_headers();
    let mut latest = HashMap::new();
    for mut value in records {
        let candidate = super::source::configured_candidates(&value.request_url, true)
            .into_iter().find(|candidate| candidate.id == value.source);
        let limited = candidate.as_ref().and_then(|candidate| candidate.stored_limit(&headers, &states));
        if let Some(limit) = limited.filter(|limit| limit.retry_at > now) {
            value.state = "rateLimited".to_string();
            value.retry_at = limit.retry_at;
            value.retry_source = limit.retry_source;
            value.selected = false;
        } else if value.until <= now || value.state == "rateLimited" {
            value.last_state = Some(value.state.clone());
            value.state = "stale".to_string();
            value.selected = false;
        }
        let key = (value.source.clone(), value.resource.clone());
        let entry = latest.entry(key).or_insert_with(|| value.clone());
        if value.checked_at > entry.checked_at { *entry = value; }
    }
    let urls = [
        "https://api.github.com/repos/Agent2plugin/Agent2plugin/releases",
        "https://raw.githubusercontent.com/Agent2plugin/Agent2plugin/main/index.json",
        "https://github.com/Agent2plugin/Agent2plugin/releases/download/current/package.zip",
    ];
    for url in urls {
        for candidate in super::source::configured_candidates(url, true) {
            let Some(limit) = candidate.stored_limit(&headers, &states).filter(|limit| limit.retry_at > now) else { continue; };
            let key = (candidate.id.clone(), resource(url).to_string());
            latest.entry(key).or_insert_with(|| Observation {
                source: candidate.id.clone(), label: candidate.label(), resource: resource(url).to_string(),
                state: "rateLimited".to_string(), elapsed_ms: None, checked_at: limit.observed_at,
                selected: false, retry_at: limit.retry_at, retry_source: limit.retry_source, last_state: None,
                context: context.clone(), request_url: url.to_string(), until: limit.retry_at,
            });
        }
    }
    let mut result: Vec<_> = latest.into_values().collect();
    result.sort_by(|left, right| right.checked_at.cmp(&left.checked_at));
    result
}
