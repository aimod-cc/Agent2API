use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{Host, PackRegistry};
use crate::server::logging;

mod events;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase { Queued, Runtime, Starting, Loading, Registering, Ready, Failed, Timeout, Blocked, Disabled, Unknown }

impl Phase {
    pub fn pending(self) -> bool {
        matches!(self, Self::Queued | Self::Runtime | Self::Starting | Self::Loading | Self::Registering)
    }
    pub fn failed(self) -> bool { matches!(self, Self::Failed | Self::Timeout) }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadStep { pub phase: Phase, pub duration_ms: i64 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadStatus {
    pub phase: Phase,
    pub attempt: u64,
    pub started_at: Option<i64>,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    pub deadline_at: Option<i64>,
    pub error: Option<String>,
    pub previous_error: Option<String>,
    pub steps: Vec<LoadStep>,
}

impl LoadStatus {
    fn new(phase: Phase) -> Self {
        Self { phase, attempt: 0, started_at: None, updated_at: logging::now_ms(),
            finished_at: None, deadline_at: None, error: None, previous_error: None, steps: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    pub id: String,
    pub label: String,
    pub pack_id: String,
    pub pack_enabled: bool,
    pub enabled: bool,
    pub available: bool,
    pub startup: LoadStatus,
}

#[derive(Clone)]
struct Provider { id: String, label: String, enabled: bool }

#[derive(Clone)]
struct Entry {
    id: String,
    name: String,
    package_name: String,
    script: bool,
    enabled: bool,
    providers: Vec<Provider>,
    startup: LoadStatus,
    registered: bool,
    host: Option<Weak<Host>>,
}

struct State {
    boot_id: String,
    initialized: bool,
    booting: bool,
    next_attempt: u64,
    entries: BTreeMap<String, Entry>,
    error: Option<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State {
        boot_id: format!("{}-{}", std::process::id(), logging::now_ms()), initialized: false, booting: false,
        next_attempt: 0, entries: BTreeMap::new(), error: None,
    }))
}

fn configure(entries: Vec<Entry>, script: bool) {
    let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
    let ids: BTreeSet<_> = entries.iter().map(|entry| entry.id.clone()).collect();
    state.entries.retain(|id, entry| entry.script != script || ids.contains(id));
    for mut entry in entries {
        if let Some(previous) = state.entries.get(&entry.id) {
            if entry.enabled == previous.enabled {
                entry.startup = previous.startup.clone();
                entry.registered = previous.registered;
                entry.host = previous.host.clone();
            }
        }
        state.entries.insert(entry.id.clone(), entry);
    }
}

pub(super) fn configure_packs(registry: &PackRegistry) {
    let owners = super::ownership::owners(registry);
    configure(registry.packs.iter().map(|pack| Entry {
        id: pack.id.clone(), name: super::install::read_manifest(&super::install::plugins_dir().join(&pack.id).join(&pack.version))
            .map(|manifest| manifest.name).unwrap_or_else(|_| pack.id.clone()),
        package_name: pack.id.clone(), script: false, enabled: pack.enabled,
        providers: pack.manifest_providers.iter().filter(|provider|
            owners.get(provider.id.as_str()).is_none_or(|owner| owner == &pack.id))
            .map(|provider| Provider { id: provider.id.0.clone(), label: provider.label.clone(),
                enabled: pack.providers.get(provider.id.as_str()).copied().unwrap_or(true) }).collect(),
        startup: LoadStatus::new(if pack.enabled { Phase::Queued } else { Phase::Disabled }), registered: false, host: None,
    }).collect(), false);
}

pub(super) fn configure_scripts(registry: &super::script::storage::Registry) {
    let names = super::script::runtime::package_names(registry);
    configure(registry.packages.iter().map(|pack| Entry {
        id: pack.id.clone(), name: names.get(&pack.id).cloned().unwrap_or_else(|| pack.name.clone()),
        package_name: pack.name.clone(), script: true, enabled: pack.enabled,
        providers: pack.providers.iter().map(|id| Provider { id: id.clone(),
            label: super::script::runtime::provider_label(id), enabled: !pack.disabled_providers.contains(id) }).collect(),
        startup: LoadStatus { previous_error: pack.error.as_ref().map(|error| super::process::redact(error)),
            ..LoadStatus::new(if pack.enabled { Phase::Queued } else { Phase::Disabled }) }, registered: false, host: None,
    }).collect(), true);
    state().lock().unwrap_or_else(|error| error.into_inner()).error = None;
}

pub(super) fn initialize() {
    configure_packs(&PackRegistry::load());
    let scripts = super::script::storage::read();
    if let Ok(registry) = &scripts { configure_scripts(registry); }
    let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
    state.boot_id = format!("{}-{}", std::process::id(), logging::now_ms());
    for entry in state.entries.values_mut() {
        let previous = entry.startup.error.clone().or_else(|| entry.startup.previous_error.clone());
        entry.startup = LoadStatus::new(if entry.enabled { Phase::Queued } else { Phase::Disabled });
        entry.startup.previous_error = previous;
        entry.registered = false;
        entry.host = None;
    }
    state.error = scripts.err();
    state.initialized = true;
    state.booting = true;
}

pub(super) fn finish_boot() {
    let event = {
        let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
        if !state.booting { return; }
        state.booting = false;
        let entries = state.entries.values().filter(|entry| entry.enabled).collect::<Vec<_>>();
        (!entries.is_empty() && entries.iter().all(|entry| !entry.startup.phase.pending()))
            .then(|| events::summary_event(&entries, &state.boot_id, 0))
    };
    if let Some(event) = event { event.emit(); }
}

pub(crate) fn log_names() -> BTreeMap<String, String> {
    state().lock().unwrap_or_else(|error| error.into_inner()).entries.values()
        .map(|entry| (entry.id.clone(), events::name(entry))).collect()
}

#[derive(Clone, Debug)]
pub struct Attempt { ids: Vec<String>, generation: u64, last_summary: Arc<Mutex<Option<String>>> }

pub(super) struct LoadGuard { pub attempt: Attempt, completed: bool }

pub(super) fn begin(ids: Vec<String>, phase: Phase) -> LoadGuard {
    let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
    state.next_attempt += 1;
    let generation = state.next_attempt;
    for id in &ids {
        if let Some(entry) = state.entries.get_mut(id) {
            let previous = entry.startup.error.clone().or_else(|| entry.startup.previous_error.clone());
            entry.startup = LoadStatus::new(phase);
            entry.startup.attempt = generation;
            entry.startup.started_at = Some(entry.startup.updated_at);
            entry.startup.previous_error = previous;
            entry.registered = false;
            entry.host = None;
        }
    }
    LoadGuard { attempt: Attempt { ids, generation, last_summary: Arc::new(Mutex::new(None)) }, completed: false }
}

impl Attempt {
    pub(super) fn pending(&self) -> bool {
        state().lock().unwrap_or_else(|error| error.into_inner()).entries.values()
            .any(|entry| entry.startup.attempt == self.generation && entry.startup.phase.pending())
    }

    fn update(&self, id: Option<&str>, phase: Phase, error: Option<&str>, deadline: Option<i64>) {
        let now = logging::now_ms();
        let message = error.map(|error| super::process::redact(error).chars().take(2000).collect::<String>());
        let mut records = Vec::new();
        let summary;
        {
            let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
            let boot_id = state.boot_id.clone();
            for key in &self.ids {
                if id.is_some_and(|id| id != key) { continue; }
                let Some(entry) = state.entries.get_mut(key).filter(|entry|
                    entry.enabled && entry.startup.attempt == self.generation) else { continue; };
                if id.is_none() && (entry.startup.phase.failed() || entry.startup.phase == Phase::Blocked)
                    && (phase != Phase::Starting || !entry.registered) { continue; }
                if id.is_some() && phase.pending() && !entry.startup.phase.pending() { continue; }
                let status = &mut entry.startup;
                if status.phase == phase && status.error == message && status.deadline_at == deadline { continue; }
                if status.phase.pending() && status.phase != phase && status.steps.len() < 16 {
                    status.steps.push(LoadStep { phase: status.phase, duration_ms: now.saturating_sub(status.updated_at) });
                }
                if phase == Phase::Starting && !status.phase.pending() {
                    status.steps.clear();
                    status.previous_error = status.error.take().or_else(|| status.previous_error.clone());
                    status.started_at = Some(now);
                }
                status.phase = phase;
                status.updated_at = now;
                status.finished_at = (!phase.pending()).then_some(now);
                status.deadline_at = deadline;
                status.error = message.clone();
                if phase == Phase::Ready { entry.registered = true; }
                if phase == Phase::Ready || phase.failed() || phase == Phase::Blocked {
                    records.push(events::entry_event(entry, &boot_id));
                }
            }
            let finished = self.ids.iter().filter_map(|id| state.entries.get(id))
                .filter(|entry| entry.enabled && entry.startup.attempt == self.generation).collect::<Vec<_>>();
            let complete = !finished.is_empty() && finished.iter().all(|entry| !entry.startup.phase.pending());
            let mut last_summary = self.last_summary.lock().unwrap_or_else(|error| error.into_inner());
            // 失败收尾可能重复送达；只有本轮各插件的结果变化时才再次汇总。
            if complete && !records.is_empty() {
                let revision = hash(&json!(finished.iter().map(|entry|
                    (&entry.id, entry.startup.phase, entry.startup.started_at, &entry.startup.error)).collect::<Vec<_>>()));
                let changed = last_summary.as_ref() != Some(&revision);
                *last_summary = Some(revision);
                summary = (!state.booting && changed)
                    .then(|| events::summary_event(&finished, &state.boot_id, self.generation));
            } else {
                if !complete { *last_summary = None; }
                summary = None;
            }
        }
        for event in records { event.emit(); }
        if let Some(event) = summary { event.emit(); }
    }

    pub(super) fn phase(&self, phase: Phase) { self.update(None, phase, None, None); }
    pub(super) fn runtime_ready(&self) {
        let registered = state().lock().unwrap_or_else(|error| error.into_inner()).entries.values()
            .any(|entry| entry.startup.attempt == self.generation && entry.registered);
        self.phase(if registered { Phase::Ready } else { Phase::Registering });
    }
    pub(super) fn waiting(&self, phase: Phase, seconds: u64) {
        self.update(None, phase, None, Some(logging::now_ms() + seconds.min(600) as i64 * 1000));
    }
    pub(super) fn fail(&self, message: &str, timeout: bool) {
        let script = state().lock().unwrap_or_else(|error| error.into_inner()).entries.values()
            .filter(|entry| entry.startup.attempt == self.generation && entry.script)
            .map(|entry| (entry.id.clone(), entry.startup.phase)).collect::<Vec<_>>();
        if script.is_empty() {
            self.update(None, if timeout { Phase::Timeout } else { Phase::Failed }, Some(message), None);
        } else {
            for (id, phase) in script {
                if phase.failed() || phase == Phase::Blocked { continue; }
                let blocked = matches!(phase, Phase::Queued | Phase::Registering | Phase::Ready | Phase::Runtime | Phase::Starting);
                self.update(Some(&id), if blocked { Phase::Blocked } else if timeout { Phase::Timeout } else { Phase::Failed }, Some(message), None);
            }
        }
    }
    pub(super) fn attach(&self, host: &Arc<Host>) {
        let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
        for id in &self.ids {
            if let Some(entry) = state.entries.get_mut(id).filter(|entry| entry.startup.attempt == self.generation) {
                entry.host = Some(Arc::downgrade(host));
            }
        }
    }
    pub(super) fn progress(&self, params: &Value) {
        let Some(id) = params["package"].as_str().filter(|id| self.ids.iter().any(|known| known == id)) else { return; };
        let phase = match params["phase"].as_str() {
            Some("queued") => Phase::Queued, Some("loading") => Phase::Loading,
            Some("registering") => Phase::Registering, Some("failed") => Phase::Failed,
            _ => return,
        };
        let current = state().lock().unwrap_or_else(|error| error.into_inner()).entries.get(id)
            .map(|entry| entry.startup.clone());
        if !current.as_ref().is_some_and(|status| status.attempt == self.generation && status.phase.pending()) { return; }
        let message = (phase == Phase::Failed).then(|| params["error"].as_str().unwrap_or("脚本插件初始化失败"));
        let deadline = if phase.pending() { current.and_then(|status| status.deadline_at) } else { None };
        self.update(Some(id), phase, message, deadline);
    }
    pub(super) fn package_failed(&self, id: &str, message: &str) {
        self.update(Some(id), Phase::Failed, Some(message), None);
    }
}

impl LoadGuard {
    pub fn finish(mut self) { self.attempt.phase(Phase::Ready); self.completed = true; }
    pub fn fail(mut self, message: &str) { self.attempt.fail(message, false); self.completed = true; }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        if !self.completed { self.attempt.fail("插件加载任务未完成或被取消，请查看诊断后重试", false); }
    }
}

fn effective(entry: &Entry) -> LoadStatus {
    let mut status = entry.startup.clone();
    if !entry.enabled { status.phase = Phase::Disabled; status.error = None; return status; }
    if status.phase == Phase::Ready {
        match entry.host.as_ref().and_then(Weak::upgrade) {
            Some(host) if host.ready() => {
                if entry.providers.iter().any(|provider| super::registry::is_switching(&provider.id)) {
                    status.phase = Phase::Registering;
                }
            }
            Some(host) => {
                status.error = host.status().last_error;
                status.phase = if status.error.is_some() { Phase::Failed } else { Phase::Unknown };
            }
            None => status.phase = Phase::Unknown,
        }
    }
    status
}

pub fn status(id: &str) -> Option<LoadStatus> {
    let entry = state().lock().unwrap_or_else(|error| error.into_inner()).entries.get(id).cloned()?;
    Some(effective(&entry))
}

pub fn provider(provider: &str) -> Option<ProviderState> {
    let entries: Vec<_> = state().lock().unwrap_or_else(|error| error.into_inner()).entries.values().cloned().collect();
    let entry = entries.iter().filter(|entry| entry.providers.iter().any(|item| item.id == provider))
        .max_by_key(|entry| (entry.enabled, entry.script))?;
    let item = entry.providers.iter().find(|item| item.id == provider)?;
    Some(provider_state(entry, item, &effective(entry)))
}

fn provider_state(entry: &Entry, item: &Provider, status: &LoadStatus) -> ProviderState {
    let enabled = entry.enabled && item.enabled;
    let mut startup = status.clone();
    if !enabled { startup.phase = Phase::Disabled; startup.error = None; }
    let available = enabled && startup.phase == Phase::Ready && super::serves_provider(&item.id);
    if startup.phase == Phase::Ready && !available { startup.phase = Phase::Unknown; }
    ProviderState { id: item.id.clone(), label: item.label.clone(), pack_id: entry.id.clone(),
        pack_enabled: entry.enabled, enabled, available, startup }
}

pub fn snapshot() -> Value {
    let (boot_id, initialized, error, entries) = {
        let state = state().lock().unwrap_or_else(|error| error.into_inner());
        (state.boot_id.clone(), state.initialized, state.error.clone(), state.entries.values().cloned().collect::<Vec<_>>())
    };
    let mut providers = BTreeMap::new();
    let mut packs = Vec::new();
    let mut pending = !initialized;
    for entry in entries {
        let status = effective(&entry);
        pending |= entry.enabled && status.phase.pending();
        for item in &entry.providers {
            let next = provider_state(&entry, item, &status);
            if providers.get(&item.id).is_none_or(|previous: &ProviderState|
                (entry.enabled, entry.script) > (previous.pack_enabled, previous.pack_id.starts_with("script-"))) {
                providers.insert(item.id.clone(), next);
            }
        }
        packs.push(json!({ "id": entry.id, "name": entry.name, "script": entry.script, "startup": status }));
    }
    let revision = hash(&json!({ "packs": packs, "providers": providers, "error": error, "initialized": initialized }));
    let availability_revision = hash(&json!(providers.iter().map(|(id, state)|
        (id, state.available, state.available.then_some((state.startup.attempt, state.startup.started_at)))).collect::<Vec<_>>()));
    json!({ "bootId": boot_id, "revision": revision, "availabilityRevision": availability_revision,
        "pending": pending, "packs": packs, "providers": providers, "error": error,
        "scriptOperation": super::script::operation(), "now": logging::now_ms() })
}

fn hash(value: &Value) -> String { format!("{:x}", Sha256::digest(value.to_string().as_bytes())) }
