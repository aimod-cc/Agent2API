mod schema;

use std::collections::{BTreeMap, HashSet};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

use futures::{stream, StreamExt};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::Host;
use crate::server::{config, errors::GatewayError};

pub(super) use schema::Schema;
pub(super) use schema::parse;

fn key(provider: &str, field: &str) -> String { format!("plugin.{provider}.{field}") }

pub(super) fn storage_provider(provider: &str, field: &str) -> String {
    let Some(host) = super::host_for_provider(provider) else { return provider.into() };
    host.provider_settings.lock().ok().and_then(|schemas| schemas.get(provider)
        .filter(|schema| schema.fields.iter().any(|entry| entry.key == field))
        .map(|schema| schema.owner(provider).to_string())).unwrap_or_else(|| provider.into())
}

pub(super) fn user_owned(host: &Host, provider: &str, field: &str) -> bool {
    host.provider_settings.lock().is_ok_and(|schemas| schemas.iter().any(|(id, schema)|
        (id == provider || schema.owner(id) == provider) && schema.fields.iter().any(|entry| entry.key == field)))
}

fn stored(schema: &Schema, provider: &str) -> Map<String, Value> {
    config::with_raw(|raw| schema.fields.iter().map(|field| {
        let value = raw.get(&key(schema.owner(provider), &field.key)).cloned().unwrap_or(Value::Null);
        (field.key.clone(), value)
    }).collect()).unwrap_or_default()
}

fn revision(host: &Host, provider: &str, schema: &Schema, values: &Map<String, Value>) -> String {
    let input = json!({ "provider": provider, "schema": schema, "values": values,
        "host": host as *const Host as usize, "generation": host.generation.load(Ordering::Acquire) });
    format!("{:x}", Sha256::digest(input.to_string().as_bytes()))
}

fn active(provider: &str) -> Result<Arc<Host>, GatewayError> {
    if !super::serves_provider(provider) || super::registry::is_switching(provider) {
        return Err(GatewayError::with_status(409, "提供商插件未启用或正在切换"));
    }
    super::host_for_provider(provider).filter(|host| host.ready())
        .ok_or_else(|| GatewayError::with_status(503, "提供商插件当前不可用"))
}

fn declared(host: &Host, provider: &str) -> Result<Schema, GatewayError> {
    host.provider_settings.lock().ok().and_then(|schemas| schemas.get(provider).cloned())
        .ok_or_else(|| GatewayError::with_status(404, "插件没有声明可编辑的提供商设置"))
}

pub(crate) fn available(provider: &str) -> bool {
    active(provider).is_ok_and(|host| declared(&host, provider).is_ok())
}

fn group(host: &Host, provider: &str, label: &str, schema: &Schema) -> Value {
    let owner = schema.owner(provider);
    let members: HashSet<String> = host.provider_settings.lock().map(|schemas| schemas.iter()
        .filter(|(id, schema)| schema.owner(id) == owner).map(|(id, _)| id.clone()).collect()).unwrap_or_default();
    let mut providers: Vec<Value> = host.providers.lock().map(|infos| infos.iter()
        .filter(|info| members.contains(info.id.as_str()))
        .map(|info| json!({ "id": info.id, "label": info.label, "enabled": super::serves_provider(info.id.as_str()) }))
        .collect()).unwrap_or_default();
    if providers.is_empty() { providers.push(json!({ "id": provider, "label": label, "enabled": true })); }
    providers.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    let pack = super::provider_pack(provider).unwrap_or_default();
    json!({ "id": format!("{pack}:{owner}"), "label": schema.group.as_ref().map(|group| &group.label),
        "providers": providers })
}

fn render(host: &Host, provider: &str, label: &str, schema: &Schema,
    values: Map<String, Value>, overrides: Map<String, Value>) -> Value {
    let mut effective = Map::new();
    let mut invalid = Map::new();
    let public: Map<String, Value> = values.iter().map(|(key, value)| {
        let scalar = value.is_null() || value.is_boolean() || value.is_number() || value.is_string();
        (key.clone(), if scalar && value.to_string().len() <= 16 * 1024 { value.clone() } else { Value::Null })
    }).collect();
    for field in &schema.fields {
        match field.normalize(values.get(&field.key).unwrap_or(&Value::Null)) {
            Ok(value) => { effective.insert(field.key.clone(), if value.is_null() { field.default.clone() } else { value }); }
            Err(_) => {
                effective.insert(field.key.clone(), Value::Null);
                invalid.insert(field.key.clone(), json!(true));
            }
        }
        if let Some(override_) = overrides.get(&field.key) { effective.insert(field.key.clone(), override_["value"].clone()); }
    }
    json!({ "provider": provider, "label": label, "group": group(host, provider, label, schema),
        "schema": schema, "values": public, "effective": effective, "invalid": invalid,
        "overrides": overrides, "revision": revision(host, provider, schema, &values) })
}

async fn row(host: &Arc<Host>, provider: &str, label: &str, schema: &Schema) -> Result<Value, GatewayError> {
    let _lease = host.activity.acquire().ok_or_else(|| GatewayError::with_status(409, "插件正在切换"))?;
    let generation = host.generation.load(Ordering::Acquire);
    let mut overrides = Map::new();
    if schema.runtime_overrides {
        let reply = host.call_coded("settings.overrides", json!({ "provider": provider }), Duration::from_secs(5)).await
            .map_err(|_| GatewayError::with_status(503, "无法读取插件设置的运行时覆盖，暂不允许编辑"))?;
        let map = reply.as_object().filter(|map| map.len() <= schema.fields.len() && reply.to_string().len() <= 64 * 1024)
            .ok_or_else(|| GatewayError::with_status(502, "插件设置运行时覆盖格式无效"))?;
        for (name, entry) in map {
            let field = schema.fields.iter().find(|field| &field.key == name)
                .ok_or_else(|| GatewayError::with_status(502, "插件返回了未声明的设置覆盖"))?;
            if entry.as_object().is_none_or(|object| object.len() != 2)
                || entry["source"].as_str().is_none_or(|source| source.trim().is_empty() || source.len() > 128 || source.contains('\0')) {
                return Err(GatewayError::with_status(502, "插件设置覆盖来源无效"));
            }
            let value = field.normalize(&entry["value"]).map_err(|_| GatewayError::with_status(502, "插件设置覆盖不符合声明规则"))?;
            if value.is_null() { return Err(GatewayError::with_status(502, "插件设置覆盖不能是空值")); }
            overrides.insert(name.clone(), json!({ "source": entry["source"], "value": value }));
        }
    }
    if !host.ready() || host.generation.load(Ordering::Acquire) != generation {
        return Err(GatewayError::with_status(503, "提供商插件当前不可用或已重新加载"));
    }
    Ok(render(host, provider, label, schema, stored(schema, provider), overrides))
}

pub(crate) async fn snapshot() -> Value {
    let mut seen = HashSet::new();
    let mut providers = Vec::new();
    for info in super::runtime::provider_infos() {
        let provider = info.id.as_str();
        let Ok(host) = active(provider) else { continue };
        let Ok(schema) = declared(&host, provider) else { continue };
        let owner = (super::provider_pack(provider), schema.owner(provider).to_string());
        if !seen.insert(owner) { continue; }
        providers.push((provider.to_string(), info.label, host, schema));
    }
    let mut settings: Vec<_> = stream::iter(providers).map(|(provider, label, host, schema)| async move {
        match row(&host, &provider, &label, &schema).await {
            Ok(value) => value,
            Err(error) => json!({ "provider": provider, "label": label, "schema": schema,
                "group": group(&host, &provider, &label, &schema), "error": error.message }),
        }
    }).buffer_unordered(4).collect().await;
    settings.sort_by(|left, right| left["provider"].as_str().cmp(&right["provider"].as_str()));
    json!({ "settings": settings })
}

pub(crate) async fn configure(provider: &str, expected: &str, patch: &Value) -> Result<Value, GatewayError> {
    let host = active(provider)?;
    let _lease = host.activity.acquire().ok_or_else(|| GatewayError::with_status(409, "插件正在切换"))?;
    let schema = declared(&host, provider)?;
    let patch = patch.as_object().filter(|patch| !patch.is_empty() && patch.len() <= schema.fields.len())
        .ok_or_else(|| GatewayError::bad_request("插件设置必须是非空对象"))?;
    let mut normalized = BTreeMap::new();
    for (name, value) in patch {
        let field = schema.fields.iter().find(|field| &field.key == name)
            .ok_or_else(|| GatewayError::bad_request("不能修改插件未声明的设置"))?;
        normalized.insert(name.clone(), field.normalize(value).map_err(GatewayError::bad_request)?);
    }
    let current = row(&host, provider, provider, &schema).await?;
    if current["revision"].as_str() != Some(expected) {
        return Err(GatewayError::with_status(409, "插件或设置已变化，请刷新后再保存"));
    }
    let mut changed = false;
    let saved = config::update_raw_fields(|raw| {
        let values: Map<String, Value> = schema.fields.iter().map(|field| (field.key.clone(),
            raw.get(&key(schema.owner(provider), &field.key)).cloned().unwrap_or(Value::Null))).collect();
        if !host.ready() || revision(&host, provider, &schema, &values) != expected { return; }
        for (name, value) in normalized {
            let path = key(schema.owner(provider), &name);
            if value.is_null() { raw.remove(&path); } else { raw.insert(path, value); }
        }
        changed = true;
    });
    if !changed { return Err(GatewayError::with_status(409, "插件或设置已变化，请刷新后再保存")); }
    let overrides = current["overrides"].as_object().cloned().unwrap_or_default();
    let mut result = render(&host, provider, provider, &schema, stored(&schema, provider), overrides);
    result["saved"] = json!(saved);
    Ok(result)
}

pub(crate) async fn legacy_provider(candidates: &[&str], field: &str) -> Result<Value, GatewayError> {
    for provider in candidates {
        let Ok(host) = active(provider) else { continue };
        let Ok(schema) = declared(&host, provider) else { continue };
        if schema.fields.iter().any(|entry| entry.key == field) { return row(&host, provider, provider, &schema).await; }
    }
    Err(GatewayError::with_status(501, "请安装并启用支持通用提供商设置的插件"))
}
