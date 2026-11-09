use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

use super::{clients, model_config, native, validate, ClientDef, Profile};

pub struct Configuration {
    pub profile: Profile,
    pub live_id: Option<String>,
    pub saved_ids: Vec<String>,
    pub status: &'static str,
    pub editable: bool,
    pub deletable: bool,
    pub validation_error: Option<String>,
}

impl Configuration {
    pub fn view(&self) -> Value {
        let mut view = self.profile.view();
        view["liveId"] = json!(self.live_id);
        view["savedIds"] = json!(self.saved_ids);
        view["status"] = json!(self.status);
        view["editable"] = json!(self.editable);
        view["deletable"] = json!(self.deletable);
        view["validationError"] = json!(self.validation_error);
        view
    }
}

fn same_target(def: &ClientDef, saved: &Profile, live: &Profile) -> bool {
    if matches!(def.id, "claude-code" | "gemini-cli" | "cline" | "workbuddy") { return false; }
    if def.id == "zed" && saved.protocol != live.protocol { return false; }
    if saved.native.source_id.is_empty() && (saved.protocol != live.protocol
        || saved.base_url.trim_end_matches('/') != live.base_url.trim_end_matches('/')) { return false; }
    clients::native_id(saved) == live.native.source_id
}

fn same_content(def: &ClientDef, path: &Path, saved: &Profile, live: &Profile) -> bool {
    if saved.protocol != live.protocol || saved.models.len() != live.models.len() { return false; }
    if def.default_model && live.enabled && saved.model != live.model { return false; }
    if !saved.native.source_id.is_empty() && saved.native.source_id != live.native.source_id
        && def.id != "workbuddy" { return false; }
    if !matches!(def.id, "claude-code" | "gemini-cli" | "cline" | "workbuddy")
        && !same_target(def, saved, live) { return false; }
    if clients::equivalent(def, path, saved, live) { return true; }
    if def.id != "workbuddy" { return false; }
    let base = |url: &str| url.trim_end_matches('/').trim_end_matches("/chat/completions").to_string();
    if base(&saved.base_url) != base(&live.base_url) || saved.models.len() != live.models.len() { return false; }
    let mut comparable = live.clone();
    comparable.native.source_id = saved.native.source_id.clone();
    clients::equivalent(def, path, saved, &comparable)
}

pub fn list(def: &ClientDef, path: &Path, saved: &[Profile], live: &[Profile], writable: bool) -> Vec<Configuration> {
    let mut result = Vec::new();
    let mut used = HashSet::new();
    let mut ids = saved.iter().map(|profile| profile.id.clone()).collect::<HashSet<_>>();
    let mut normalized = live.to_vec();
    for profile in &mut normalized { model_config::normalize(def, profile); }
    let mut ordered = normalized.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|profile| !profile.enabled);
    for current in ordered {
        let preferred = saved.iter().filter(|profile| !used.contains(&profile.id) && same_content(def, path, profile, current))
            .max_by_key(|profile| profile.updated_at)
            .or_else(|| saved.iter().filter(|profile| !used.contains(&profile.id) && same_target(def, profile, current))
                .max_by_key(|profile| profile.updated_at));
        let mut profile = current.clone();
        if let Some(saved) = preferred {
            profile.id = saved.id.clone();
            profile.name = saved.name.clone();
            profile.note = saved.note.clone();
            if !current.enabled && def.default_model {
                if def.id == "codex" && profile.models.is_empty() { profile.models = saved.models.clone(); }
                if !saved.model.is_empty() && profile.models.iter().any(|model| model.id == saved.model) {
                    profile.model = saved.model.clone();
                    profile.effort = saved.effort.clone();
                }
            }
            if clients::equivalent(def, path, saved, &profile) {
                profile.auth_mode = saved.auth_mode.clone();
                profile.gateway_key_id = saved.gateway_key_id.clone();
            }
        }
        if preferred.is_none() {
            let base = profile.id.clone();
            let mut suffix = 2;
            while ids.contains(&profile.id) {
                profile.id = format!("{base}-{suffix}");
                suffix += 1;
            }
            ids.insert(profile.id.clone());
        }
        let saved_ids = preferred.iter().map(|profile| profile.id.clone()).collect::<Vec<_>>();
        used.extend(saved_ids.iter().cloned());
        let editable = writable && native::editable(def, current);
        let validation_error = if editable && !current.enabled { validate::profile(def, &profile).err() } else { None };
        result.push(Configuration {
            editable,
            deletable: writable,
            status: if def.toggle { if current.enabled { "enabled" } else { "disabled" } }
                else if def.default_model && current.enabled && current.default { "active" } else { "available" },
            profile,
            live_id: Some(current.id.clone()),
            saved_ids,
            validation_error,
        });
    }
    for saved in saved.iter().filter(|profile| !used.contains(&profile.id)) {
        let mut profile = saved.clone();
        model_config::normalize(def, &mut profile);
        let editable = writable && (profile.native.source_id.is_empty() || native::editable(def, &profile));
        result.push(Configuration {
            validation_error: validate::profile(def, &profile).err(),
            profile,
            live_id: None,
            saved_ids: vec![saved.id.clone()],
            status: "inactive",
            editable,
            deletable: true,
        });
    }
    result
}

pub fn remember(def: &ClientDef, path: &Path, saved: &mut Vec<Profile>, live: &[Profile], skip: Option<&str>, reserved: &str) {
    for entry in list(def, path, saved, live, true) {
        let Some(live_id) = entry.live_id.as_deref() else { continue; };
        if skip == Some(live_id) || !entry.saved_ids.is_empty() { continue; }
        let mut profile = entry.profile;
        let base = profile.id.clone();
        let mut suffix = 2;
        while profile.id == reserved || saved.iter().any(|item| item.id == profile.id) {
            profile.id = format!("{base}-{suffix}");
            suffix += 1;
        }
        profile.updated_at = crate::server::logging::now_ms();
        saved.push(profile);
    }
}
