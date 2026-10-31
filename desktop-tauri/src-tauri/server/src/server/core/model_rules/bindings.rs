use serde_json::{json, Value};

use super::{Mapping, ModelRules, RuleEntry};
use crate::server::core::models::model_id;
use crate::server::core::providers::catalog;

pub(super) struct Catalog {
    upstreams: Vec<(String, Vec<Value>)>,
}

impl Catalog {
    pub fn current() -> Self {
        Self {
            upstreams: catalog::upstream_manifests(),
        }
    }

    pub fn native_exists(&self, provider: &str, id: &str) -> bool {
        self.upstreams
            .iter()
            .find(|(owner, _)| owner.eq_ignore_ascii_case(provider))
            .is_some_and(|(_, items)| {
                items
                    .iter()
                    .any(|item| model_id(item).eq_ignore_ascii_case(id))
            })
    }

    pub fn manifest(&self, rules: &ModelRules, provider: &str) -> Vec<Value> {
        let mut items = self
            .upstreams
            .iter()
            .find(|(owner, _)| owner.eq_ignore_ascii_case(provider))
            .map(|(_, items)| items.clone())
            .unwrap_or_default();
        for model in rules
            .custom
            .iter()
            .filter(|model| model.provider.eq_ignore_ascii_case(provider))
        {
            if !items
                .iter()
                .any(|item| model_id(item).eq_ignore_ascii_case(&model.id))
            {
                items.push(json!({ "id": model.id, "name": model.id }));
            }
        }
        items
    }

    pub fn owners(&self, rules: &ModelRules, target: &str) -> Vec<String> {
        let mut owners = self
            .upstreams
            .iter()
            .map(|(owner, _)| owner.clone())
            .collect::<Vec<_>>();
        for model in &rules.custom {
            if !owners
                .iter()
                .any(|owner| owner.eq_ignore_ascii_case(&model.provider))
            {
                owners.push(model.provider.clone());
            }
        }
        owners.retain(|owner| {
            catalog::entry_id_in_manifest(&self.manifest(rules, owner), target).is_some()
        });
        owners
    }

    pub fn stored_target(
        &self,
        rules: &ModelRules,
        provider: &str,
        alias: &str,
        target: &str,
    ) -> String {
        let manifest = self.manifest(rules, provider);
        let candidates = rules.mappings.iter().filter(|mapping| {
            mapping.alias.eq_ignore_ascii_case(alias)
                && mapping
                    .provider
                    .as_deref()
                    .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
        });
        candidates
            .clone()
            .find(|mapping| mapping.target.eq_ignore_ascii_case(target))
            .or_else(|| {
                candidates.clone().find(|mapping| {
                    catalog::entry_id_in_manifest(&manifest, &mapping.target)
                        .is_some_and(|id| id.eq_ignore_ascii_case(target))
                })
            })
            .map(|mapping| mapping.target.clone())
            .unwrap_or_else(|| target.to_string())
    }

    pub fn target(&self, rules: &ModelRules, provider: &str, target: &str) -> Option<String> {
        let manifest = self.manifest(rules, provider);
        manifest
            .iter()
            .find(|item| model_id(item).eq_ignore_ascii_case(target))
            .map(model_id)
    }
}

impl ModelRules {
    pub(crate) fn selected_mapping<'a>(
        &'a self,
        provider: &str,
        alias: &str,
        manifest: &[Value],
    ) -> Option<&'a Mapping> {
        if manifest
            .iter()
            .any(|item| model_id(item).eq_ignore_ascii_case(alias))
            && self.default_enabled(provider, alias)
        {
            return None;
        }
        let prefix =
            crate::server::core::providers::cline::models::Pool::from_provider_id(provider)
                .map(crate::server::core::providers::cline::models::Pool::target_prefix);
        self.mappings
            .iter()
            .filter(|mapping| {
                mapping.alias.eq_ignore_ascii_case(alias)
                    && !mapping.alias.eq_ignore_ascii_case(&mapping.target)
                    && mapping
                        .provider
                        .as_deref()
                        .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
            })
            .filter_map(|mapping| self.binding(provider, &mapping.alias, &mapping.target))
            .filter(|mapping| mapping.enabled)
            .min_by_key(|mapping| {
                let target = catalog::entry_id_in_manifest(manifest, &mapping.target);
                (
                    target.is_none(),
                    mapping.provider.is_none(),
                    !target.as_deref().is_some_and(|target| {
                        prefix.is_some_and(|prefix| target.starts_with(prefix))
                    }),
                )
            })
    }

    pub(crate) fn binding_enabled(
        &self,
        provider: &str,
        alias: &str,
        target: &str,
        manifest: &[Value],
    ) -> bool {
        if alias.eq_ignore_ascii_case(target) {
            return self.default_enabled(provider, target);
        }
        self.selected_mapping(provider, alias, manifest)
            .is_some_and(|mapping| {
                let selected = catalog::entry_id_in_manifest(manifest, &mapping.target)
                    .unwrap_or_else(|| mapping.target.clone());
                let target = catalog::entry_id_in_manifest(manifest, target)
                    .unwrap_or_else(|| target.to_string());
                selected.eq_ignore_ascii_case(&target)
            })
    }
}

pub(super) fn set_membership(
    list: &mut Vec<RuleEntry>,
    provider: Option<&str>,
    id: &str,
    present: bool,
) {
    list.retain(|entry| {
        !(entry
            .provider
            .as_deref()
            .map_or(provider.is_none(), |owner| {
                provider.is_some_and(|provider| owner.eq_ignore_ascii_case(provider))
            })
            && entry.id.eq_ignore_ascii_case(id))
    });
    if present {
        list.push(RuleEntry::new(provider, id));
    }
}

fn enable_on(rules: &mut ModelRules, catalog: &Catalog, provider: &str, id: &str) {
    let owners = catalog.owners(rules, id);
    let global = rules
        .disabled
        .iter()
        .any(|entry| entry.provider.is_none() && entry.id.eq_ignore_ascii_case(id));
    rules.disabled.retain(|entry| !entry.matches(provider, id));
    if global {
        for owner in owners
            .iter()
            .filter(|owner| !owner.eq_ignore_ascii_case(provider))
        {
            if !ModelRules::hit(&rules.disabled, owner, id) {
                rules.disabled.push(RuleEntry::new(Some(owner), id));
            }
        }
    }
}

pub(super) fn put_binding(
    rules: &mut ModelRules,
    provider: Option<&str>,
    alias: &str,
    target: &str,
    reasoning: Option<Option<&str>>,
    enabled: Option<bool>,
) {
    let inherited = provider.and_then(|owner| rules.binding(owner, alias, target).cloned());
    let index = rules.mappings.iter().position(|mapping| {
        mapping.alias.eq_ignore_ascii_case(alias)
            && mapping.target.eq_ignore_ascii_case(target)
            && mapping
                .provider
                .as_deref()
                .map_or(provider.is_none(), |owner| {
                    provider.is_some_and(|provider| owner.eq_ignore_ascii_case(provider))
                })
    });
    let mapping = if let Some(index) = index {
        &mut rules.mappings[index]
    } else {
        rules.mappings.push(Mapping {
            alias: alias.to_string(),
            target: target.to_string(),
            provider: provider.map(str::to_string),
            reasoning: inherited
                .as_ref()
                .and_then(|mapping| mapping.reasoning.clone()),
            enabled: inherited.as_ref().map_or(true, |mapping| mapping.enabled),
        });
        let Some(mapping) = rules.mappings.last_mut() else {
            return;
        };
        mapping
    };
    if let Some(reasoning) = reasoning {
        mapping.reasoning = reasoning.and_then(super::normalize_reasoning);
    }
    if let Some(enabled) = enabled {
        mapping.enabled = enabled;
    }
}

pub(super) fn set_default(
    rules: &mut ModelRules,
    catalog: &Catalog,
    provider: &str,
    id: &str,
    enabled: bool,
) {
    if enabled {
        enable_on(rules, catalog, provider, id);
    } else {
        set_membership(&mut rules.disabled, Some(provider), id, true);
    }
    put_binding(rules, Some(provider), id, id, None, Some(enabled));
}

pub(super) fn enable_exclusive(
    rules: &mut ModelRules,
    catalog: &Catalog,
    provider: &str,
    alias: &str,
    target: &str,
) {
    let manifest = catalog.manifest(rules, provider);
    if !alias.eq_ignore_ascii_case(target)
        && manifest
            .iter()
            .any(|item| model_id(item).eq_ignore_ascii_case(alias))
    {
        set_default(rules, catalog, provider, alias, false);
    }
    let rivals = rules
        .mappings
        .iter()
        .filter(|mapping| {
            mapping.alias.eq_ignore_ascii_case(alias)
                && !mapping.target.eq_ignore_ascii_case(target)
                && mapping
                    .provider
                    .as_deref()
                    .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
        })
        .map(|mapping| mapping.target.clone())
        .collect::<Vec<_>>();
    for rival in rivals {
        put_binding(rules, Some(provider), alias, &rival, None, Some(false));
    }
    if alias.eq_ignore_ascii_case(target) {
        set_default(rules, catalog, provider, target, true);
    } else {
        put_binding(rules, Some(provider), alias, target, None, Some(true));
    }
}

pub(super) fn normalize_name(
    rules: &mut ModelRules,
    catalog: &Catalog,
    provider: &str,
    alias: &str,
) {
    let manifest = catalog.manifest(rules, provider);
    let default = manifest
        .iter()
        .any(|item| model_id(item).eq_ignore_ascii_case(alias))
        && rules.default_enabled(provider, alias);
    let selected = if default {
        Some(alias.to_string())
    } else {
        rules
            .selected_mapping(provider, alias, &manifest)
            .map(|mapping| mapping.target.clone())
    };
    let rivals = rules
        .mappings
        .iter()
        .filter(|mapping| {
            mapping.alias.eq_ignore_ascii_case(alias)
                && mapping.enabled
                && !selected
                    .as_deref()
                    .is_some_and(|target| mapping.target.eq_ignore_ascii_case(target))
                && mapping
                    .provider
                    .as_deref()
                    .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
        })
        .map(|mapping| mapping.target.clone())
        .collect::<Vec<_>>();
    for target in rivals {
        put_binding(rules, Some(provider), alias, &target, None, Some(false));
    }
}

pub(super) fn active_conflict(
    rules: &ModelRules,
    catalog: &Catalog,
    provider: &str,
    alias: &str,
    target: &str,
) -> bool {
    let manifest = catalog.manifest(rules, provider);
    (manifest
        .iter()
        .any(|item| model_id(item).eq_ignore_ascii_case(alias))
        && !alias.eq_ignore_ascii_case(target)
        && rules.default_enabled(provider, alias))
        || rules
            .selected_mapping(provider, alias, &manifest)
            .is_some_and(|mapping| !mapping.target.eq_ignore_ascii_case(target))
}
