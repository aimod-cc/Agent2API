use serde_json::{Map, Value};

use super::bindings::{
    active_conflict, enable_exclusive, normalize_name, put_binding, set_default, set_membership,
    Catalog,
};
use super::{CapabilityOverride, CustomModel, ModelRules, KEY_MODEL_RULES};
use crate::server::config;
use crate::server::errors::GatewayError;

type Result<T> = std::result::Result<T, GatewayError>;

fn conflict(message: impl Into<String>) -> GatewayError {
    GatewayError::with_status(409, message)
}

pub(super) fn mutate<T>(run: impl FnOnce(&mut ModelRules) -> Result<T>) -> Result<T> {
    config::try_update_raw_field(
        KEY_MODEL_RULES,
        |value| {
            let mut raw = Map::new();
            raw.insert(KEY_MODEL_RULES.to_string(), value.clone());
            let mut rules = ModelRules::from_raw(&raw);
            let result = run(&mut rules)?;
            *value = rules.to_value();
            Ok(result)
        },
        || GatewayError::new("模型配置保存失败，本次修改未生效；请检查数据库与磁盘空间"),
    )
}

pub fn set_state(provider: Option<&str>, id: &str, enabled: Option<bool>) -> Result<ModelRules> {
    let catalog = Catalog::current();
    mutate(|rules| {
        let owners = provider
            .map(|provider| vec![provider.to_string()])
            .unwrap_or_else(|| catalog.owners(rules, id));
        for owner in &owners {
            if catalog.target(rules, owner, id).is_none() {
                return Err(GatewayError::with_status(
                    404,
                    format!("该提供商的清单里没有模型「{id}」"),
                ));
            }
        }
        if let Some(enabled) = enabled {
            if provider.is_none() && enabled {
                rules
                    .disabled
                    .retain(|entry| !entry.id.eq_ignore_ascii_case(id));
            }
            for owner in owners {
                let target = catalog
                    .target(rules, &owner, id)
                    .unwrap_or_else(|| id.to_string());
                normalize_name(rules, &catalog, &owner, &target);
                if enabled {
                    enable_exclusive(rules, &catalog, &owner, &target, &target);
                } else {
                    set_default(rules, &catalog, &owner, &target, false);
                    normalize_name(rules, &catalog, &owner, &target);
                }
            }
            if provider.is_none() && !enabled {
                set_membership(&mut rules.disabled, None, id, true);
            }
        }
        Ok(rules.clone())
    })
}

pub fn add_mapping(
    alias: &str,
    target: &str,
    provider: Option<&str>,
    reasoning: Option<Option<&str>>,
    enabled: Option<bool>,
    create: Option<bool>,
) -> Result<ModelRules> {
    let catalog = Catalog::current();
    mutate(|rules| {
        let target = provider
            .map(|owner| catalog.stored_target(rules, owner, alias, target))
            .unwrap_or_else(|| target.to_string());
        let target = target.as_str();
        let owners = provider
            .map(|provider| vec![provider.to_string()])
            .unwrap_or_else(|| catalog.owners(rules, target));
        let existing = provider
            .and_then(|owner| rules.binding(owner, alias, target).cloned())
            .or_else(|| {
                rules
                    .mappings
                    .iter()
                    .find(|mapping| {
                        provider.is_none()
                            && mapping.provider.is_none()
                            && mapping.alias.eq_ignore_ascii_case(alias)
                            && mapping.target.eq_ignore_ascii_case(target)
                    })
                    .cloned()
            });
        if create == Some(true) && (alias.eq_ignore_ascii_case(target) || existing.is_some()) {
            return Err(conflict(format!(
                "模型 ID「{alias}」已绑定到此上游模型，请直接使用已有绑定"
            )));
        }
        if create == Some(false) && existing.is_none() && !alias.eq_ignore_ascii_case(target) {
            return Err(GatewayError::with_status(
                404,
                format!("映射不存在: {alias} → {target}"),
            ));
        }
        if create != Some(true)
            && existing.is_none()
            && enabled == Some(false)
            && !alias.eq_ignore_ascii_case(target)
        {
            return Err(GatewayError::with_status(
                404,
                format!("映射不存在: {alias} → {target}"),
            ));
        }
        let target = existing
            .as_ref()
            .map(|mapping| mapping.target.as_str())
            .unwrap_or(target);
        let effective =
            enabled.unwrap_or_else(|| existing.as_ref().map_or(true, |mapping| mapping.enabled));
        let creating = !alias.eq_ignore_ascii_case(target) && existing.is_none();
        for owner in &owners {
            if (creating || enabled == Some(true))
                && catalog.target(rules, owner, target).is_none()
                && (creating
                    || crate::server::core::providers::catalog::entry_id_in_manifest(
                        &catalog.manifest(rules, owner),
                        target,
                    )
                    .is_none())
            {
                return Err(GatewayError::with_status(
                    404,
                    format!("该提供商的清单里没有模型「{target}」"),
                ));
            }
            if creating && active_conflict(rules, &catalog, owner, alias, target) {
                return Err(conflict(format!(
                    "模型 ID「{alias}」在该提供商已有启用绑定，请先关闭它再添加"
                )));
            }
        }
        if creating && owners.is_empty() {
            return Err(GatewayError::with_status(
                404,
                format!("没有提供商承载上游模型「{target}」"),
            ));
        }
        if create == Some(true) || enabled.is_some() {
            for owner in &owners {
                normalize_name(rules, &catalog, owner, alias);
            }
        }
        put_binding(rules, provider, alias, target, reasoning, enabled);
        for owner in owners {
            if creating || enabled == Some(true) {
                if effective {
                    enable_exclusive(rules, &catalog, &owner, alias, target);
                }
            } else if enabled == Some(false) && alias.eq_ignore_ascii_case(target) {
                set_default(rules, &catalog, &owner, target, false);
            }
            normalize_name(rules, &catalog, &owner, alias);
        }
        Ok(rules.clone())
    })
}

pub fn add_custom(provider: &str, id: &str) -> Result<ModelRules> {
    let catalog = Catalog::current();
    mutate(|rules| {
        if catalog.native_exists(provider, id)
            || rules.custom.iter().any(|model| model.matches(provider, id))
        {
            return Err(conflict(format!("模型 ID「{id}」已存在，请使用已有模型")));
        }
        if active_conflict(rules, &catalog, provider, id, id) {
            return Err(conflict(format!(
                "模型 ID「{id}」在该提供商已有启用绑定，请先关闭它再添加"
            )));
        }
        rules.custom.push(CustomModel {
            provider: provider.to_string(),
            id: id.to_string(),
        });
        enable_exclusive(rules, &catalog, provider, id, id);
        Ok(rules.clone())
    })
}

pub fn remove_custom(provider: &str, id: &str) -> Result<(ModelRules, bool)> {
    let catalog = Catalog::current();
    mutate(|rules| {
        let before = rules.custom.len();
        let native = catalog.native_exists(provider, id);
        if !native && rules.custom.iter().any(|model| model.matches(provider, id)) {
            normalize_name(rules, &catalog, provider, id);
        }
        rules.custom.retain(|model| !model.matches(provider, id));
        let removed = rules.custom.len() != before;
        if removed && !native {
            rules.disabled.retain(|entry| {
                !(entry.id.eq_ignore_ascii_case(id)
                    && entry
                        .provider
                        .as_deref()
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(provider)))
            });
            rules.mappings.retain(|mapping| {
                !(mapping.target.eq_ignore_ascii_case(id)
                    && mapping
                        .provider
                        .as_deref()
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(provider)))
            });
            rules
                .capabilities
                .retain(|entry| !entry.matches(provider, id));
        }
        Ok((rules.clone(), removed))
    })
}

pub fn remove_mapping(
    alias: &str,
    target: &str,
    provider: Option<&str>,
) -> Result<(ModelRules, bool)> {
    let catalog = Catalog::current();
    mutate(|rules| {
        let target = provider
            .map(|owner| catalog.stored_target(rules, owner, alias, target))
            .unwrap_or_else(|| target.to_string());
        let target = target.as_str();
        let globals = rules
            .mappings
            .iter()
            .filter(|mapping| {
                mapping.provider.is_none()
                    && mapping.alias.eq_ignore_ascii_case(alias)
                    && mapping.target.eq_ignore_ascii_case(target)
            })
            .cloned()
            .collect::<Vec<_>>();
        let owners = catalog.owners(rules, target);
        let removed = rules.mappings.iter().any(|mapping| {
            mapping.alias.eq_ignore_ascii_case(alias)
                && mapping.target.eq_ignore_ascii_case(target)
                && provider.map_or(true, |provider| {
                    mapping
                        .provider
                        .as_deref()
                        .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
                })
        });
        let normalizing = provider
            .map(|provider| vec![provider.to_string()])
            .unwrap_or_else(|| owners.clone());
        for owner in &normalizing {
            normalize_name(rules, &catalog, owner, alias);
        }
        rules.mappings.retain(|mapping| {
            !(mapping.alias.eq_ignore_ascii_case(alias)
                && mapping.target.eq_ignore_ascii_case(target)
                && provider.map_or(true, |provider| {
                    mapping
                        .provider
                        .as_deref()
                        .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
                }))
        });
        if let (Some(provider), Some(global)) = (provider, globals.first()) {
            for owner in owners
                .iter()
                .filter(|owner| !owner.eq_ignore_ascii_case(provider))
            {
                if rules.binding(owner, alias, target).is_none() {
                    rules.mappings.push(super::Mapping {
                        provider: Some(owner.clone()),
                        ..global.clone()
                    });
                }
            }
        }
        Ok((rules.clone(), removed))
    })
}

pub fn set_capabilities(
    provider: &str,
    id: &str,
    patch: &Map<String, Value>,
) -> Result<ModelRules> {
    mutate(|rules| {
        let index = rules
            .capabilities
            .iter()
            .position(|entry| entry.matches(provider, id));
        let mut values = index
            .map(|index| rules.capabilities[index].values.clone())
            .unwrap_or_default();
        for (key, value) in patch {
            if value.is_null() {
                values.remove(key);
            } else if let Some(value) = crate::server::core::capability::normalize_value(key, value)
            {
                values.insert(key.clone(), value);
            }
        }
        match index {
            Some(index) if values.is_empty() => {
                rules.capabilities.remove(index);
            }
            Some(index) => rules.capabilities[index].values = values,
            None if values.is_empty() => {}
            None => rules.capabilities.push(CapabilityOverride {
                provider: provider.to_string(),
                id: id.to_string(),
                values,
            }),
        }
        Ok(rules.clone())
    })
}
