//! Magpie codex_models.go：按账号拉取 /models?client_version=…，保留套餐权限。
//! 与 Magpie catalog.Codex 一样不编造固定模型清单；兜底使用上次成功目录。
//! 原始模型（含 base_instructions）只存在本家缓存，公开目录只输出白名单能力字段。

use std::collections::{BTreeMap, HashSet};
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::{adapter::ModelRefreshOutcome, catalog_cache};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{account, credentials, http, ID};

pub const CACHE_SCOPE: &str = "codexSubscription";
pub(super) const IMAGE_MODEL: &str = "gpt-image-2";
const TTL_MS: i64 = 10 * 60 * 1000;

pub(super) fn is_image_model(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case(IMAGE_MODEL)
}

fn image_model_view() -> Value {
    json!({
        "id": IMAGE_MODEL,
        "name": "GPT Image 2",
        "provider": ID,
        "providerName": "OpenAI",
        "owned_by": ID,
        "kind": "image",
        "output_modalities": ["image"],
        "supportsImages": true,
        "supports_image_generation": true,
        "supports_image_editing": true,
        "supportsToolCall": false,
        "supportsReasoning": false,
        "isDefault": false,
        "description": "Codex 内置图片生成与参考图编辑模型（实际可用性与额度由账号套餐决定）",
    })
}

fn mark_image_capabilities(model: &mut Value) {
    model["kind"] = json!("image");
    model["output_modalities"] = json!(["image"]);
    model["supportsImages"] = json!(true);
    model["supports_image_generation"] = json!(true);
    model["supports_image_editing"] = json!(true);
    model["supportsToolCall"] = json!(false);
    model["supportsReasoning"] = json!(false);
    model["isDefault"] = json!(false);
}

#[derive(Clone)]
struct AccountCatalog {
    models: Vec<Value>,
    fetched_at: i64,
}

#[derive(Default)]
struct Catalog {
    accounts: BTreeMap<String, AccountCatalog>,
}

static CACHE: OnceLock<RwLock<Catalog>> = OnceLock::new();

fn cache() -> &'static RwLock<Catalog> {
    CACHE.get_or_init(|| {
        let mut state = Catalog::default();
        if let Some(saved) = catalog_cache::load(CACHE_SCOPE) {
            for entry in saved.models {
                let Some(identity) = entry
                    .get("identity")
                    .and_then(Value::as_str)
                    .filter(|identity| !identity.is_empty())
                else {
                    continue;
                };
                let Some(models) = entry
                    .get("models")
                    .and_then(Value::as_array)
                    .filter(|models| !models.is_empty())
                else {
                    continue;
                };
                state.accounts.insert(
                    identity.to_string(),
                    AccountCatalog {
                        models: models.clone(),
                        fetched_at: entry
                            .get("fetchedAt")
                            .and_then(Value::as_i64)
                            .unwrap_or(saved.fetched_at),
                    },
                );
            }
        }
        RwLock::new(state)
    })
}

pub fn refresh_meta() -> (bool, i64) {
    let state = cache().read().unwrap_or_else(|error| error.into_inner());
    (
        !state.accounts.is_empty(),
        state
            .accounts
            .values()
            .map(|entry| entry.fetched_at)
            .max()
            .unwrap_or(0),
    )
}

pub(super) fn list() -> Vec<Value> {
    list_for(None)
}

fn list_for(identities: Option<&HashSet<String>>) -> Vec<Value> {
    let state = cache().read().unwrap_or_else(|error| error.into_inner());
    let mut models: BTreeMap<String, (i64, Value)> = BTreeMap::new();
    for (identity, entry) in &state.accounts {
        if identities.is_some_and(|identities| !identities.contains(identity)) {
            continue;
        }
        for raw in &entry.models {
            let Some(model) = model_view(raw) else {
                continue;
            };
            let id = account::text(&model, &["id"]).to_string();
            match models.get_mut(&id) {
                Some((_, existing)) => {
                    let mut levels = existing
                        .get("reasoningLevels")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for level in model
                        .get("reasoningLevels")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if !levels.contains(level) {
                            levels.push(level.clone());
                        }
                    }
                    if !levels.is_empty() {
                        existing["reasoningLevels"] = Value::Array(levels);
                    }
                    for key in ["maxInputTokens", "maxContextWindow"] {
                        if let Some(next) = model.get(key).and_then(Value::as_u64) {
                            if existing.get(key).and_then(Value::as_u64).unwrap_or(0) < next {
                                existing[key] = json!(next);
                            }
                        }
                    }
                    if model.get("supportsImages").and_then(Value::as_bool) == Some(true) {
                        existing["supportsImages"] = json!(true);
                    }
                }
                None => {
                    models.insert(
                        id,
                        (
                            raw.get("priority").and_then(Value::as_i64).unwrap_or(0),
                            model,
                        ),
                    );
                }
            }
        }
    }
    if identities.is_none_or(|ids| !ids.is_empty())
        && !models.keys().any(|id| is_image_model(id))
    {
        models.insert(IMAGE_MODEL.to_string(), (i64::MAX, image_model_view()));
    }
    let mut ordered: Vec<_> = models.into_values().collect();
    ordered.sort_by_key(|(priority, _)| *priority);
    ordered.into_iter().map(|(_, model)| model).collect()
}

pub(super) fn advertise(store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
    let identities: HashSet<_> = store
        .accounts_for_provider(ID)
        .iter()
        .filter_map(|account| {
            account
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| store.plugin_account(id))
                .and_then(|value| account::Tokens::read(&value).ok())
                .map(|tokens| tokens.identity())
        })
        .collect();
    let allowed: HashSet<String> = list_for(Some(&identities))
        .into_iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    // 只做收窄，不重造条目而丢弃上层已应用的手工能力覆盖。
    manifest
        .into_iter()
        .filter(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| allowed.contains(id))
        })
        .collect()
}

fn model_view(raw: &Value) -> Option<Value> {
    let id = account::text(raw, &["slug"]);
    if id.is_empty() || id.len() > 256 || account::text(raw, &["visibility"]) == "hide" {
        return None;
    }
    let name = account::text(raw, &["display_name"]);
    let mut model = json!({ "id": id, "name": if name.is_empty() { id } else { name },
        "provider": ID, "providerName": "OpenAI", "owned_by": ID, "supportsToolCall": true,
        "protocol": "responses" });
    for (source, target) in [
        ("context_window", "maxInputTokens"),
        ("max_context_window", "maxContextWindow"),
    ] {
        if let Some(limit) = raw
            .get(source)
            .and_then(Value::as_u64)
            .filter(|limit| *limit > 0)
        {
            model[target] = json!(limit);
        }
    }
    if let Some(modalities) = raw.get("input_modalities").and_then(Value::as_array) {
        model["supportsImages"] = json!(modalities
            .iter()
            .any(|value| value.as_str() == Some("image")));
    }
    if let Some(levels) = raw
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
    {
        let levels: Vec<_> = levels
            .iter()
            .filter_map(|entry| entry.get("effort").and_then(Value::as_str))
            .collect();
        model["supportsReasoning"] = json!(!levels.is_empty());
        model["reasoningLevels"] = json!(levels);
    }
    if let Some(default) = raw.get("default_reasoning_level").and_then(Value::as_str) {
        model["reasoningDefaultLevel"] = json!(default);
    }
    if let Some(tiers) = raw.get("service_tiers").and_then(Value::as_array) {
        model["serviceTiers"] = Value::Array(
            tiers
                .iter()
                .filter_map(|tier| tier.get("id").cloned())
                .collect(),
        );
    }
    if let Some(description) = raw.get("description").and_then(Value::as_str) {
        model["description"] = json!(description);
    }
    if is_image_model(id) {
        mark_image_capabilities(&mut model);
    }
    Some(model)
}

pub(super) fn entry(identity: &str, model: &str) -> Option<Value> {
    if is_image_model(model) {
        return None;
    }
    let state = cache().read().unwrap_or_else(|error| error.into_inner());
    state
        .accounts
        .get(identity)?
        .models
        .iter()
        .find(|raw| {
            account::text(raw, &["slug"]) == model && account::text(raw, &["visibility"]) != "hide"
        })
        .cloned()
}

pub(super) fn known(identity: &str) -> bool {
    cache()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .accounts
        .contains_key(identity)
}

pub(super) fn cooldown_models(
    store: &AccountStore,
    account_id: &str,
    wire_model: &str,
) -> Vec<String> {
    // 图片生成与对话各自有独立配额桶（image_gen / imagegen_premium），互不连坐冷却。
    if is_image_model(wire_model) {
        return vec![wire_model.to_string()];
    }
    let identity = store
        .plugin_account(account_id)
        .and_then(|value| account::Tokens::read(&value).ok())
        .map(|tokens| tokens.identity());
    let state = cache().read().unwrap_or_else(|error| error.into_inner());
    let mut models: Vec<String> = identity
        .and_then(|identity| state.accounts.get(&identity))
        .map(|catalog| {
            catalog
                .models
                .iter()
                .filter_map(|raw| raw.get("slug").and_then(Value::as_str))
                .filter(|slug| !is_image_model(slug))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !models.iter().any(|model| model == wire_model) {
        models.push(wire_model.to_string());
    }
    models
}

pub(super) async fn refresh(
    store: &AccountStore,
    account_id: &str,
    force: bool,
) -> ModelRefreshOutcome {
    match refresh_inner(store, account_id, force).await {
        Ok(Some(count)) => ModelRefreshOutcome::refreshed(count),
        Ok(None) => ModelRefreshOutcome::unchanged(),
        Err(error) => ModelRefreshOutcome::failed(error.message),
    }
}

async fn refresh_inner(
    store: &AccountStore,
    account_id: &str,
    force: bool,
) -> Result<Option<usize>, GatewayError> {
    let candidates = store.maintenance_accounts_for_provider(ID);
    let selected = if account_id.is_empty() {
        candidates.first().and_then(|account| account.get("id").and_then(Value::as_str))
            .ok_or_else(|| GatewayError::bad_request("请先添加有可用凭据的 Codex 账号"))?
    } else {
        account_id
    };
    let initial = account::snapshot(store, selected)?;
    if !candidates
        .iter()
        .any(|account| account.get("id").and_then(Value::as_str) == Some(initial.id.as_str()))
    {
        return Err(GatewayError::bad_request(
            "指定 Codex 账号没有可用于获取模型的凭据，未改用其他账号",
        ));
    }
    let mut ids = vec![initial.id];
    if account_id.is_empty() {
        for account in candidates {
            if let Some(id) = account.get("id").and_then(Value::as_str) {
                if !ids.iter().any(|existing| existing == id) {
                    ids.push(id.to_string());
                }
            }
        }
    }
    let mut refreshed = false;
    let mut failure = None;
    for id in ids {
        match refresh_one(store, &id, force).await {
            Ok(outcome) => refreshed |= outcome.is_some(),
            Err(error) if !account_id.is_empty() => return Err(error),
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(refreshed.then(|| list().len()))
}

async fn refresh_one(
    store: &AccountStore,
    account_id: &str,
    force: bool,
) -> Result<Option<usize>, GatewayError> {
    let snapshot = account::snapshot(store, account_id)?;
    let identity = snapshot.tokens.identity();
    if !force {
        let fresh = cache()
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .accounts
            .get(&identity)
            .is_some_and(|entry| logging::now_ms().saturating_sub(entry.fetched_at) < TTL_MS);
        if fresh {
            return Ok(None);
        }
    }
    credentials::ensure(store, &snapshot.id, false).await?;
    let mut refreshed = false;
    let raw = loop {
        let snapshot = account::snapshot(store, &snapshot.id)?;
        let headers = http::account_headers(&snapshot.tokens.access, &snapshot.tokens.account_id);
        let request = http::with_headers(
            http::client(snapshot.proxy.as_ref())?
                .get(format!("{}/models", http::BASE))
                .query(&[("client_version", http::version())]),
            &headers,
        );
        let reply = http::request(request, 16 * 1024 * 1024, "获取模型").await?;
        if reply.status == 401 && !refreshed {
            credentials::ensure(store, &snapshot.id, true).await?;
            refreshed = true;
            continue;
        }
        if !(200..300).contains(&reply.status) {
            return Err(http::response_error("获取模型", &reply));
        }
        break reply.body;
    };
    let models = raw
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .ok_or_else(|| {
            GatewayError::with_status(502, "Codex 没有返回模型目录，已保留上次成功的清单")
        })?;
    if models.len() > 4096 {
        return Err(GatewayError::with_status(502, "Codex 模型目录条目过多"));
    }
    let count = models.iter().filter_map(model_view).count();
    if count == 0 {
        return Err(GatewayError::with_status(
            502,
            "Codex 没有返回可用模型，已保留上次成功的清单",
        ));
    }
    let current = account::snapshot(store, &snapshot.id)?;
    if current.tokens.identity() != identity {
        return Err(GatewayError::with_status(
            409,
            "获取模型时账号身份已变化，请重新获取",
        ));
    }
    let now = logging::now_ms();
    // 缓存与持久化在同一把本家写锁中更新，避免慢写覆盖另一账号刚刷好的清单。
    let mut state = cache().write().unwrap_or_else(|error| error.into_inner());
    state.accounts.insert(
        identity,
        AccountCatalog {
            models: models.clone(),
            fetched_at: now,
        },
    );
    let saved: Vec<_> = state
        .accounts
        .iter()
        .map(|(identity, entry)| {
            json!({
                "identity": identity, "models": entry.models, "fetchedAt": entry.fetched_at,
            })
        })
        .collect();
    catalog_cache::save(CACHE_SCOPE, &saved, now);
    drop(state);
    super::super::catalog::invalidate();
    Ok(Some(count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_builtin_image_model_and_isolates_cooldown() {
        let items = list();
        let image = items
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(IMAGE_MODEL))
            .expect("Codex 模型清单应包含内置图像模型");
        assert_eq!(image["kind"], json!("image"));
        assert_eq!(image["output_modalities"], json!(["image"]));
        assert_eq!(image["supports_image_generation"], json!(true));
        assert_eq!(image["supports_image_editing"], json!(true));
        assert_eq!(image["supportsImages"], json!(true));
        assert_eq!(image["supportsToolCall"], json!(false));
        assert_eq!(image["supportsReasoning"], json!(false));
        assert!(entry("any-identity", IMAGE_MODEL).is_none());

        let store = AccountStore::with_db(None);
        assert_eq!(
            cooldown_models(&store, "acc-1", IMAGE_MODEL),
            vec![IMAGE_MODEL.to_string()]
        );
    }
}
