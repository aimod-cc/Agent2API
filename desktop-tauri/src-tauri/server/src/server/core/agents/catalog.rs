use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::{validate, Model, Profile};
use crate::server::{
    config,
    core::{
        account_store::AccountStore, capability, custom_providers, key_scope::KeyScope, model_meta,
        models::model_id, providers,
    },
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    #[serde(flatten)]
    pub model: Model,
    #[serde(skip_serializing_if = "is_false")]
    pub routing_group: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_summary: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn models(store: &AccountStore, gateway_key_id: Option<&str>) -> Result<Vec<CatalogModel>, String> {
    let scope = gateway_key_id
        .map(validate::gateway_credential)
        .transpose()?
        .and_then(|(_, scope)| scope);
    Ok(scoped_models(store, scope.as_ref()))
}

fn group_meta(id: &str) -> (Option<usize>, Option<String>) {
    let profiles = config::routing_profiles();
    let Some(group) = profiles
        .groups
        .values()
        .find(|group| group.model.eq_ignore_ascii_case(id))
    else {
        return (None, None);
    };
    let mut member_models = Vec::new();
    let mut seen = HashSet::new();
    let mut active_count = 0usize;
    for member in group.members.iter().filter(|member| member.included) {
        active_count += 1;
        let label = member.model.trim();
        if !label.is_empty() && seen.insert(label.to_ascii_lowercase()) {
            member_models.push(label.to_string());
        }
    }
    let summary = if member_models.is_empty() {
        None
    } else if member_models.len() <= 3 {
        Some(member_models.join(" / "))
    } else {
        Some(format!("{} / +{}", member_models[..3].join(" / "), member_models.len() - 3))
    };
    (Some(active_count), summary)
}

fn scoped_models(store: &AccountStore, scope: Option<&KeyScope>) -> Vec<CatalogModel> {
    let public = providers::catalog::models_response(store, scope);
    let mut manifests = HashMap::<String, Vec<Value>>::new();
    let mut result = Vec::new();
    for item in public.get("data").and_then(Value::as_array).into_iter().flatten() {
        let id = model_id(item);
        if id.is_empty() || item.get("kind").and_then(Value::as_str) == Some("image") {
            continue;
        }
        let provider = item.get("owned_by").and_then(Value::as_str).unwrap_or("");
        let grouped = item.get("routing_group").and_then(Value::as_bool) == Some(true);
        let raw = if grouped {
            None
        } else {
            let custom = custom_providers::is_custom_provider_id(provider);
            let manifest = manifests.entry(provider.to_string()).or_insert_with(|| {
                if !custom {
                    return providers::catalog::manifest_for_provider(provider);
                }
                custom_providers::models_of(provider).into_iter().map(|mut model| {
                    model_meta::apply(&mut model);
                    let overrides = capability::normalize_object(model.get("capabilities"));
                    capability::apply_overrides(&mut model, &overrides);
                    model
                }).collect()
            });
            let target = if custom {
                custom_providers::wire_model_for(provider, &id).0
            } else {
                providers::catalog::wire_target_for_provider(&id, provider, None).model
            };
            manifest.iter().find(|model| model_id(model).eq_ignore_ascii_case(&target))
        };
        // 公开清单把未声明的布尔能力转成 false；编辑器必须从原始能力保留三态。
        let capabilities = raw.map(capability::effective);
        let field = |name: &str, public_name: &str| {
            if grouped { item.get(public_name) } else { capabilities.as_ref().and_then(|caps| caps.get(name)) }
        };
        let mut levels = raw.and_then(|model| model.get("reasoningLevels"))
            .or_else(|| item.get("reasoning_efforts"))
            .and_then(Value::as_array).into_iter().flatten()
            .filter_map(Value::as_str).filter(|level| !level.trim().is_empty())
            .map(str::to_string).collect::<Vec<_>>();
        let mut seen = HashSet::new();
        levels.retain(|level| seen.insert(level.clone()));
        let (member_count, group_summary) = if grouped { group_meta(&id) } else { (None, None) };
        result.push(CatalogModel {
            model: Model {
                name: item.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or(&id).to_string(),
                id,
                context_window: field("maxInputTokens", "max_input_tokens").and_then(Value::as_u64),
                max_output_tokens: field("maxOutputTokens", "max_output_tokens").and_then(Value::as_u64),
                supports_tools: field("supportsToolCall", "supports_tool_call").and_then(Value::as_bool),
                supports_images: field("supportsImages", "supports_images").and_then(Value::as_bool),
                supports_video: field("supportsVideo", "supports_video").and_then(Value::as_bool),
                supports_reasoning: field("supportsReasoning", "supports_reasoning").and_then(Value::as_bool),
                reasoning_levels: levels,
                effort: String::new(),
            },
            routing_group: grouped,
            member_count,
            group_summary,
        });
    }
    result.sort_by_cached_key(|entry| (!entry.routing_group, entry.model.id.to_lowercase()));
    result
}

pub fn validate_profile(store: &AccountStore, profile: &Profile) -> Result<(), String> {
    if profile.kind != "gateway" || profile.auth_mode != "gateway" {
        return Ok(());
    }
    let (_, scope) = validate::gateway_credential(&profile.gateway_key_id)?;
    let available = providers::catalog::models_response(store, scope.as_ref());
    let ids = available.get("data").and_then(Value::as_array).into_iter().flatten()
        .filter(|item| item.get("kind").and_then(Value::as_str) != Some("image"))
        .map(|item| model_id(item).to_lowercase()).collect::<HashSet<_>>();
    let wanted = profile.models.iter().map(|model| model.id.as_str()).chain([
        profile.model.as_str(), profile.claude_models.opus.as_str(), profile.claude_models.sonnet.as_str(),
        profile.claude_models.haiku.as_str(), profile.claude_models.subagent.as_str(),
    ]);
    let unavailable = wanted.filter(|id| !id.is_empty() && !ids.contains(&id.to_lowercase())).collect::<Vec<_>>();
    if !unavailable.is_empty() {
        return Err(format!("以下模型已不可用或所选网关 Key 无权使用，请重新选择：{}", unavailable.join("、")));
    }
    Ok(())
}
