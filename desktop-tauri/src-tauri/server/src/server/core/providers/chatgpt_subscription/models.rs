//! Magpie siwcFetchModels / parseSIWCModels：GET /v1/models 的账号目录。
//! 只缓存这条 SIWC 路线实际返回的 slug；不拼入 Codex 缓存或编造模型兜底。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::ModelRefreshOutcome;
use crate::server::core::providers::{catalog, catalog_cache};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{account, credentials, error, http, API_BASE};

pub const CACHE_SCOPE: &str = "chatgptSubscription";
const CACHE_TTL_MS: i64 = 5 * 60 * 1000;

struct AccountCatalog {
    models: Vec<Value>,
    fetched_at: i64,
}

#[derive(Default)]
struct Catalog {
    accounts: BTreeMap<String, AccountCatalog>,
}

fn cache() -> &'static RwLock<Catalog> {
    static CACHE: OnceLock<RwLock<Catalog>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut state = Catalog::default();
        if let Some(saved) = catalog_cache::load(CACHE_SCOPE) {
            for entry in saved.models {
                let Some(identity) = entry.get("identity").and_then(Value::as_str) else {
                    continue;
                };
                let Some(models) = entry
                    .get("models")
                    .and_then(Value::as_array)
                    .filter(|rows| !rows.is_empty())
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

pub(super) fn list() -> Vec<Value> {
    let cached = cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut merged = BTreeMap::<String, (i64, Value)>::new();
    for catalog in cached.accounts.values() {
        for model in &catalog.models {
            let Some(id) = model.get("id").and_then(Value::as_str) else {
                continue;
            };
            if merged
                .get(id)
                .is_none_or(|(at, _)| catalog.fetched_at >= *at)
            {
                merged.insert(id.to_string(), (catalog.fetched_at, model.clone()));
            }
        }
    }
    merged.into_values().map(|(_, model)| model).collect()
}

pub(super) fn advertise(store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
    let accounts = store.accounts_for_provider(super::ID);
    let identities: BTreeSet<&str> = accounts
        .iter()
        .filter_map(|account| account.get("uid").and_then(Value::as_str))
        .collect();
    let cached = cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let allowed: BTreeSet<&str> = cached
        .accounts
        .iter()
        .filter(|(identity, _)| identities.contains(identity.as_str()))
        .flat_map(|(_, catalog)| {
            catalog
                .models
                .iter()
                .filter_map(|row| row.get("id").and_then(Value::as_str))
        })
        .collect();
    manifest
        .into_iter()
        .filter(|model| {
            model
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| allowed.contains(id))
        })
        .collect()
}

pub(super) fn check_model(identity: &str, model: &str) -> Result<(), GatewayError> {
    let cached = cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let catalog = cached.accounts.get(identity).ok_or_else(|| {
        GatewayError::bad_request("尚未取得这个 ChatGPT 账号的模型权限，请先获取模型目录")
            .with_code("account_model_permissions_unknown")
    })?;
    if catalog
        .models
        .iter()
        .any(|row| row.get("id").and_then(Value::as_str) == Some(model))
    {
        return Ok(());
    }
    Err(GatewayError::bad_request(
        "这个 ChatGPT 账号的目录未包含所选模型，请换用有权限的账号或刷新目录",
    )
    .with_code("account_model_unavailable"))
}

pub fn refresh_meta() -> (bool, i64) {
    let cached = cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (
        !cached.accounts.is_empty(),
        cached
            .accounts
            .values()
            .map(|catalog| catalog.fetched_at)
            .max()
            .unwrap_or(0),
    )
}

pub(super) async fn refresh(
    store: &AccountStore,
    requested: &str,
    force: bool,
) -> ModelRefreshOutcome {
    let selected = if requested.is_empty() {
        store.maintenance_accounts_for_provider(super::ID).into_iter()
            .find_map(|account| account.get("id").and_then(Value::as_str).map(str::to_string))
    } else {
        Some(requested.to_string())
    };
    let Some(selected) = selected else {
        return ModelRefreshOutcome::failed("请先添加有可用凭据的 ChatGPT 账号");
    };
    let id = match account::select_id(store, &selected) {
        Ok(id) => id,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    let identity = match account::load(store, &id) {
        Ok((_, account)) => account.identity(),
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    {
        let cached = cache()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !force
            && cached.accounts.get(&identity).is_some_and(|catalog| {
                logging::now_ms().saturating_sub(catalog.fetched_at) < CACHE_TTL_MS
            })
        {
            return ModelRefreshOutcome::unchanged();
        }
    }
    let models = match fetch(store, &id).await {
        Ok(models) => models,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    if models.is_empty() {
        return ModelRefreshOutcome::failed(
            "OpenAI 没有为当前 ChatGPT 账号列出可用模型，保留之前的目录",
        );
    }
    if !account::load(store, &id).is_ok_and(|(_, account)| account.identity() == identity) {
        return ModelRefreshOutcome::failed("获取 ChatGPT 模型时账号身份已变化，请重新获取");
    }
    let fetched_at = logging::now_ms();
    let count = models.len();
    {
        let mut cached = cache()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cached
            .accounts
            .insert(identity, AccountCatalog { models, fetched_at });
        let saved: Vec<_> = cached.accounts.iter().map(|(identity, catalog)| {
            json!({ "identity": identity, "models": catalog.models, "fetchedAt": catalog.fetched_at })
        }).collect();
        catalog_cache::save(CACHE_SCOPE, &saved, fetched_at);
    }
    catalog::invalidate();
    ModelRefreshOutcome::refreshed(count)
}

async fn fetch(store: &AccountStore, id: &str) -> Result<Vec<Value>, GatewayError> {
    let mut credentials = credentials::fresh(store, id, false).await?;
    let proxy = account::proxy(store, id)?;
    let client = http::client(proxy.as_ref())?;
    for attempt in 0..2 {
        let response = http::send(
            client
                .get(format!("{API_BASE}/models"))
                .bearer_auth(&credentials.access_token)
                .header("Accept", "application/json"),
        )
        .await?;
        let status = response.status().as_u16();
        if status == 401 && attempt == 0 {
            drop(response);
            credentials = credentials::fresh(store, id, true).await?;
            continue;
        }
        let body = match http::json(response, 4 * 1024 * 1024).await {
            Ok(body) => body,
            Err(_) if status != 200 => return Err(error::upstream(status, &Value::Null)),
            Err(error) => return Err(error),
        };
        if status != 200 {
            return Err(error::upstream(status, &body));
        }
        return parse(&body);
    }
    Err(GatewayError::with_status(
        401,
        "ChatGPT 凭证未被接受，请重新登录",
    ))
}

fn parse(body: &Value) -> Result<Vec<Value>, GatewayError> {
    let items = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| GatewayError::with_status(502, "OpenAI 的 ChatGPT 模型目录格式无法识别"))?;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for item in items {
        let Some(slug) = item.get("slug").and_then(Value::as_str).filter(|slug| {
            !slug.is_empty() && slug.len() <= 256 && !slug.chars().any(char::is_control)
        }) else {
            continue;
        };
        let visibility = item.get("visibility").and_then(Value::as_str).unwrap_or("");
        if !visibility.is_empty() && visibility != "list" || !seen.insert(slug.to_string()) {
            continue;
        }
        let name = item
            .get("display_name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(slug);
        let levels: Vec<Value> = item
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|level| level.get("effort").and_then(Value::as_str))
            .filter(|effort| !effort.is_empty() && effort.len() <= 32)
            .map(|effort| Value::String(effort.to_string()))
            .collect();
        let mut model = json!({
            "id": slug, "name": account::safe_text(name), "kind": "chat",
            "supportsToolCall": true, "supportsReasoning": !levels.is_empty(),
            "reasoningLevels": levels, "reasoningBindingSupported": !levels.is_empty(),
            "reasoningSource": "provider", "isDefault": result.is_empty(),
        });
        if let Some(input) = item.get("input_modalities").and_then(Value::as_array) {
            model["supportsImages"] =
                json!(input.iter().any(|value| value.as_str() == Some("image")));
        }
        if let Some(context) = item
            .get("context_window")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
        {
            model["maxInputTokens"] = json!(context);
        }
        if let Some(max) = item
            .get("max_context_window")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
        {
            model["maxContextWindow"] = json!(max);
        }
        result.push(model);
    }
    Ok(result)
}
