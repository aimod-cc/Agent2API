//! 每个账号的目录来自官方 CLI initialize.models（Hapi claudeModels.ts），
//! 再合并 Magpie claude_code_models.go 所读的官方 profile 权益缓存。
//! 不发送模型提示词、不拿订阅令牌请求 HTTP；失败保留旧清单。
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use super::{
    accounts::{self, Link},
    errors, process, ID,
};
use crate::server::core::{
    account_store::AccountStore,
    providers::{adapter::ModelRefreshOutcome, catalog_cache},
    proxies::ResolvedProxy,
};
use crate::server::{errors::GatewayError, logging};

const SCOPE: &str = "claude-subscription";
const TTL_MS: i64 = 300_000;
#[derive(Default)]
struct Catalog {
    accounts: HashMap<String, (Vec<Value>, i64)>,
}

fn cache() -> &'static Mutex<Catalog> {
    static CACHE: OnceLock<Mutex<Catalog>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut state = Catalog::default();
        if let Some(saved) = catalog_cache::load(SCOPE) {
            for entry in saved.models {
                if let (Some(account), Some(models)) =
                    (entry.get("account").and_then(Value::as_str), entry.get("models").and_then(Value::as_array))
                {
                    if models.is_empty() {
                        continue;
                    }
                    state.accounts.insert(
                        account.into(),
                        (models.clone(), entry.get("fetchedAt").and_then(Value::as_i64).unwrap_or(saved.fetched_at)),
                    );
                }
            }
        }
        Mutex::new(state)
    })
}

fn row(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": if name.is_empty() { id } else { name }, "supportsToolCall": true,
        "supportsImages": true, "supportsVideo": false,
        "supportsJsonSchema": false, "provider": ID })
}

pub(super) fn list() -> Vec<Value> {
    let mut models = BTreeMap::new();
    if let Ok(state) = cache().lock() {
        for (rows, _) in state.accounts.values() {
            for item in rows {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    models.insert(id.to_string(), item.clone());
                }
            }
        }
    }
    // 官方 CLI 的模型别名；不是猜测当前账号拥有某个 gated 版本。
    for id in ["default", "sonnet", "opus", "haiku"] {
        models.entry(id.into()).or_insert_with(|| {
            let mut item = row(id, id);
            item["catalogSource"] = json!("cli-alias-fallback");
            item
        });
    }
    models.into_values().collect()
}

pub(super) fn advertise(store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
    let accounts = store.accounts_for_provider(ID);
    let keys: std::collections::HashSet<_> =
        accounts.iter().filter_map(|account| account.get("uid").and_then(Value::as_str)).collect();
    if keys.is_empty() {
        return Vec::new();
    }
    let Ok(state) = cache().lock() else {
        return Vec::new();
    };
    let known: std::collections::HashSet<_> = state
        .accounts
        .iter()
        .filter(|(key, _)| keys.contains(key.as_str()))
        .flat_map(|(_, (rows, _))| rows.iter().filter_map(|row| row.get("id").and_then(Value::as_str)))
        .collect();
    if known.is_empty() {
        return manifest
            .into_iter()
            .filter(|item| {
                matches!(item.get("id").and_then(Value::as_str), Some("default" | "sonnet" | "opus" | "haiku"))
            })
            .collect();
    }
    manifest
        .into_iter()
        .filter(|row| row.get("id").and_then(Value::as_str).is_some_and(|id| known.contains(id)))
        .collect()
}

pub(super) fn refresh_meta() -> (bool, i64) {
    cache()
        .lock()
        .map(|state| (!state.accounts.is_empty(), state.accounts.values().map(|(_, at)| *at).max().unwrap_or(0)))
        .unwrap_or((false, 0))
}

pub(super) async fn refresh(store: &AccountStore, selected: &str, force: bool) -> ModelRefreshOutcome {
    let result = async {
        let account = if selected.is_empty() {
            store
                .maintenance_accounts_for_provider(ID)
                .into_iter()
                .find_map(|row| row.get("id").and_then(Value::as_str).map(str::to_string))
                .ok_or_else(|| errors::local("claude_account_required", "请先关联一个官方 Claude Code 账号"))?
        } else {
            selected.to_string()
        };
        let link = accounts::load(store, &account)?;
        link.check_profile()?;
        if !force
            && cache()
                .lock()
                .ok()
                .and_then(|state| state.accounts.get(&link.account_key).map(|(_, at)| *at))
                .is_some_and(|at| logging::now_ms().saturating_sub(at) < TTL_MS)
        {
            return Ok(ModelRefreshOutcome::unchanged());
        }
        let proxy = accounts::account_proxy(store, &account)?;
        accounts::verify(&link, proxy.as_ref()).await?;
        let raw = read_cli_catalog(&link, proxy.as_ref()).await?;
        link.check_profile()?;
        let has_snapshot = raw.iter().any(|row| row.get("value").and_then(Value::as_str).is_some_and(valid_id));
        if !has_snapshot {
            return Err(errors::protocol("官方 Claude Code 未返回有效目录快照，未把旧 profile 缓存冒充在线刷新"));
        }
        let profile = link.profile()?;
        let rows = normalize(raw, &profile);
        if rows.is_empty() {
            return Err(errors::protocol("官方 Claude Code 未返回可用模型清单，已保留旧目录"));
        }
        let count = rows.len();
        let now = logging::now_ms();
        let persisted = {
            let mut state =
                cache().lock().map_err(|_| errors::local("claude_catalog_unavailable", "Claude 模型缓存暂不可用"))?;
            if state.accounts.len() >= 256 && !state.accounts.contains_key(&link.account_key) {
                return Err(errors::local(
                    "claude_catalog_capacity",
                    "Claude 目录缓存账号数量已达上限，未覆盖其他账号",
                ));
            }
            state.accounts.insert(link.account_key.clone(), (rows, now));
            state
                .accounts
                .iter()
                .map(|(account, (models, at))| json!({"account": account, "models": models, "fetchedAt": at}))
                .collect::<Vec<_>>()
        };
        catalog_cache::save(SCOPE, &persisted, now);
        super::super::catalog::invalidate();
        Ok(ModelRefreshOutcome::refreshed(count))
    }
    .await;
    result.unwrap_or_else(|error: GatewayError| ModelRefreshOutcome::failed(error.message))
}

async fn read_cli_catalog(link: &Link, proxy: Option<&ResolvedProxy>) -> Result<Vec<Value>, GatewayError> {
    let temp = process::private_dir()?;
    let mut args = vec![
        "-p".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    args.extend(process::safe_args(None, true));
    let mut run = process::spawn(link, &args, temp.path(), proxy)?;
    let request = process::token()?;
    let reading = async {
        run.send(&json!({"type":"control_request", "request_id": request, "request":{"subtype":"initialize"}})).await?;
        while let Some(line) = run.output.next().await? {
            let value: Value =
                serde_json::from_slice(&line).map_err(|_| errors::protocol("Claude 模型初始化响应不是 JSON"))?;
            if value.get("type").and_then(Value::as_str) != Some("control_response") {
                continue;
            }
            let response = value.get("response").unwrap_or(&Value::Null);
            if response.get("request_id").and_then(Value::as_str) != Some(request.as_str()) {
                continue;
            }
            if response.get("subtype").and_then(Value::as_str) != Some("success") {
                return Err(errors::protocol("Claude Code 拒绝模型目录初始化，请更新官方原生 CLI"));
            }
            return response
                .pointer("/response/models")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| errors::protocol("此 Claude Code 版本没有返回模型目录，已保留旧清单"));
        }
        Err(errors::protocol("Claude Code 在返回模型目录前退出"))
    };
    let result = tokio::time::timeout(Duration::from_secs(20), reading)
        .await
        .map_err(|_| errors::local("claude_catalog_timeout", "Claude 模型目录初始化超时，已终止读取"));
    run.stop().await?;
    result?
}

fn normalize(raw: Vec<Value>, profile: &accounts::Profile) -> Vec<Value> {
    let effort_known = raw.iter().any(|row| row.get("supportsEffort").is_some());
    let mut rows = BTreeMap::new();
    for item in raw {
        let Some(id) = item.get("value").and_then(Value::as_str).filter(|s| valid_id(s)) else {
            continue;
        };
        if item.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let name = item.get("displayName").and_then(Value::as_str).unwrap_or(id);
        let mut entry = row(id, name);
        entry["catalogSource"] = json!("official-cli-initialize");
        if let Some(resolved) = item.get("resolvedModel").and_then(Value::as_str).filter(|s| valid_id(s)) {
            entry["resolvedModel"] = json!(resolved);
        }
        if item.get("supportsEffort").and_then(Value::as_bool) == Some(true) {
            entry["supportsReasoning"] = json!(true);
        }
        if effort_known && item.get("supportsEffort").and_then(Value::as_bool) != Some(true) {
            entry["reasoningLevels"] = json!([]);
            entry["reasoningBindingSupported"] = json!(false);
        } else if let Some(levels) = item.get("supportedEffortLevels").and_then(Value::as_array) {
            entry["reasoningLevels"] = json!(levels
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| matches!(*s, "low" | "medium" | "high" | "xhigh" | "max"))
                .collect::<Vec<_>>());
        }
        rows.insert(id.to_string(), entry);
    }
    for option in &profile.additional_model_options_cache {
        if option.disabled {
            continue;
        }
        if let Some(id) = option.value.as_deref().filter(|s| valid_id(s)) {
            rows.entry(id.into()).or_insert_with(|| row(id, &option.label));
        }
    }
    for access in &profile.model_access_cache {
        if !valid_id(&access.api_name) {
            continue;
        }
        if access.entitled == Some(true) {
            rows.entry(access.api_name.clone()).or_insert_with(|| row(&access.api_name, ""));
        }
        if access.entitled == Some(false) {
            rows.retain(|id, row| {
                id != &access.api_name
                    && row.get("resolvedModel").and_then(Value::as_str) != Some(access.api_name.as_str())
            });
        }
    }
    for entry in rows.values_mut() {
        if let Some(at) = profile.additional_model_options_answered_at {
            entry["upstreamAnsweredAt"] = json!(at);
        }
    }
    rows.into_values().collect()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 180 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.[]".contains(&b))
}

pub(super) fn check_model(link: &Link, model: &str) -> Result<(), GatewayError> {
    if !valid_id(model) {
        return Err(GatewayError::bad_request("Claude 模型名称无效"));
    }
    let profile = link.profile()?;
    let resolved = cache().lock().ok().and_then(|state| {
        state
            .accounts
            .get(&link.account_key)
            .and_then(|(rows, _)| rows.iter().find(|row| row.get("id").and_then(Value::as_str) == Some(model)))
            .and_then(|row| row.get("resolvedModel").and_then(Value::as_str))
            .map(str::to_string)
    });
    let api = resolved.as_deref().unwrap_or(model).split('[').next().unwrap_or(model);
    if profile.model_access_cache.iter().any(|entry| entry.api_name == api && entry.entitled == Some(false)) {
        return Err(
            GatewayError::bad_request("官方 Claude Code 账号资料明确拒绝此模型").with_code("account_model_unavailable")
        );
    }
    Ok(())
}
