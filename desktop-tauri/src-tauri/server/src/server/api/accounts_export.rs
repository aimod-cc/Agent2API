use axum::body::Bytes;
use axum::http::header::CACHE_CONTROL;
use axum::http::HeaderValue;
use axum::response::Response;
use serde_json::{json, Map, Value};

use super::accounts::{parse_json_body, store_error};
use crate::server::core::account_transfer::{
    export_account_files, resolve_provider, ExportSelection, ACCOUNT_FILE_FORMAT,
};
use crate::server::errors::management_error;
use crate::server::http::ok_json;
use crate::server::ServerState;

pub async fn account_files(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
    };
    let selection = match (text("id"), text("provider")) {
        (id, "") if !id.is_empty() => ExportSelection::Account(id.to_string()),
        ("", provider) if !provider.is_empty() => ExportSelection::Provider(provider.to_string()),
        _ => return management_error(400, "请指定一个账号 id 或一个提供商分组"),
    };
    let result = match export_account_files(state.store(), selection) {
        Ok(result) => result,
        Err(error) => return store_error(error),
    };
    match serde_json::to_value(result) {
        Ok(value) => {
            let mut response = ok_json(value);
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => management_error(500, "账号导出内容序列化失败"),
    }
}

enum ExportDocument {
    Single(Map<String, Value>),
    Batch {
        accounts: Vec<Value>,
        custom_providers: Option<Value>,
    },
}

fn is_software_account_record(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    if map.get("format").and_then(Value::as_str) == Some(ACCOUNT_FILE_FORMAT) {
        return true;
    }
    let has_id = map
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty());
    if !has_id {
        return false;
    }
    map.get("provider")
        .and_then(Value::as_str)
        .is_some_and(|provider| !provider.trim().is_empty())
        || map.get("pluginSecret").is_some_and(Value::is_object)
        || map.get("accessToken").and_then(Value::as_str).is_some()
        || map.get("refreshToken").and_then(Value::as_str).is_some()
        || map.get("apiKey").and_then(Value::as_str).is_some()
}

fn extract_export_document(doc: &Value) -> Option<ExportDocument> {
    if let Some(map) = doc.as_object() {
        if map.get("format").and_then(Value::as_str) == Some(ACCOUNT_FILE_FORMAT) {
            return Some(ExportDocument::Single(map.clone()));
        }
        let root = if !map.contains_key("accounts") && !map.contains_key("provider") {
            match map.get("data").and_then(Value::as_object) {
                Some(data) if data.get("accounts").is_some_and(Value::is_array) => data,
                _ => map,
            }
        } else {
            map
        };
        if let Some(accounts) = root.get("accounts").and_then(Value::as_array) {
            return Some(ExportDocument::Batch {
                accounts: accounts.clone(),
                custom_providers: root.get("customProviders").cloned(),
            });
        }
        return None;
    }
    if let Some(items) = doc.as_array() {
        if !items.is_empty() && items.iter().all(is_software_account_record) {
            return Some(ExportDocument::Batch {
                accounts: items.clone(),
                custom_providers: None,
            });
        }
    }
    None
}

fn belongs_to_other_provider(map: &Map<String, Value>, target: &str) -> bool {
    match resolve_provider(map) {
        Ok(resolved) => resolved != target,
        Err(_) => map
            .get("provider")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|provider| !provider.is_empty() && provider != target),
    }
}

pub async fn import_account_file(
    state: &ServerState,
    provider: &str,
    outer_payload: &Value,
    name: Option<&str>,
) -> Option<Response> {
    let selected_proxy = outer_payload.get("proxy");
    let file = outer_payload
        .get("authFileContent")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let apply_outer_settings = file.is_some();
    let doc = file.as_ref().unwrap_or(outer_payload);
    let export = extract_export_document(doc)?;
    if crate::server::core::plugin_host::script::known_provider(provider) {
        return None;
    }
    if crate::server::core::providers::availability::is_suspended(provider) {
        return Some(management_error(
            503,
            "该提供商已暂停接入，暂不支持添加账号；已有数据保留",
        ));
    }
    match export {
        ExportDocument::Single(single) => {
            Some(
                import_single_account_file(
                    state,
                    provider,
                    outer_payload,
                    apply_outer_settings,
                    single,
                    selected_proxy,
                    name,
                )
                .await,
            )
        }
        ExportDocument::Batch {
            accounts,
            custom_providers,
        } => {
            Some(
                import_batch_account_file(
                    state,
                    provider,
                    outer_payload,
                    apply_outer_settings,
                    accounts,
                    custom_providers,
                    selected_proxy,
                    name,
                )
                .await,
            )
        }
    }
}

async fn import_single_account_file(
    state: &ServerState,
    provider: &str,
    outer_payload: &Value,
    apply_outer_settings: bool,
    mut account: Map<String, Value>,
    selected_proxy: Option<&Value>,
    name: Option<&str>,
) -> Response {
    if account.get("provider").and_then(Value::as_str) != Some(provider) {
        return management_error(
            400,
            "账号文件的提供商与当前选择不一致，请选择对应的提供商后再导入",
        );
    }
    let id = account
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return management_error(400, "账号导出文件缺少 id");
    }
    if let Some(proxy) = selected_proxy {
        account.insert("proxy".to_string(), proxy.clone());
    }
    if let Some(name) = name.filter(|name| !name.trim().is_empty()) {
        account.insert("name".to_string(), Value::String(name.trim().to_string()));
        account.insert("nameCustom".to_string(), Value::Bool(true));
    }
    let custom_providers = account.get("customProviders").cloned();
    let mut body = json!({ "accounts": [Value::Object(account)] });
    if let Some(definitions) = custom_providers {
        body["customProviders"] = definitions;
    }
    let result = match crate::server::core::account_transfer::import_accounts(state.store(), &body)
    {
        Ok(result) => result,
        Err(error) => return store_error(error),
    };
    if result.get("failed").and_then(Value::as_u64).unwrap_or(0) > 0
        || result.get("skipped").and_then(Value::as_u64).unwrap_or(0) > 0
    {
        let message = result
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|errors| errors.first())
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("账号文件导入失败");
        return management_error(400, message);
    }
    let imported_id = result
        .get("importedIds")
        .and_then(Value::as_array)
        .and_then(|ids| ids.first())
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    if apply_outer_settings {
        if let Err(error) = state
            .store()
            .apply_initial_account_settings(&imported_id, outer_payload, true)
        {
            return store_error(error);
        }
    }
    let mut response = json!({
        "created": result.get("added").and_then(Value::as_u64).unwrap_or(0) > 0,
        "proxySaved": true,
    });
    super::account_import_setup::complete(state, &[imported_id.clone()], &mut response).await;
    crate::server::core::providers::subscriptions::refresh_after_account_saved(
        state.store().clone(),
        provider,
        &imported_id,
    );
    response["account"] = state
        .store()
        .plugin_account(&imported_id)
        .and_then(|record| record.get("public").cloned())
        .unwrap_or(Value::Null);
    response["list"] = state.store().list_visible_accounts();
    ok_json(response)
}

async fn import_batch_account_file(
    state: &ServerState,
    provider: &str,
    outer_payload: &Value,
    apply_outer_settings: bool,
    accounts: Vec<Value>,
    custom_providers: Option<Value>,
    selected_proxy: Option<&Value>,
    name: Option<&str>,
) -> Response {
    if accounts.is_empty() {
        return management_error(400, "导出文件中没有可导入的账号");
    }
    let mut matched = Vec::new();
    let mut other_provider_skipped: usize = 0;
    for item in accounts {
        match item {
            Value::Object(mut map) => {
                if belongs_to_other_provider(&map, provider) {
                    other_provider_skipped += 1;
                    continue;
                }
                if let Some(proxy) = selected_proxy {
                    map.insert("proxy".to_string(), proxy.clone());
                }
                matched.push(Value::Object(map));
            }
            other => matched.push(other),
        }
    }
    if matched.is_empty() {
        if other_provider_skipped > 0 {
            return management_error(
                400,
                format!(
                    "导出文件中没有属于当前提供商的账号（已忽略 {other_provider_skipped} 个其他提供商账号）；如需导入全部提供商请到「设置 → 账号导入」"
                ),
            );
        }
        return management_error(400, "导出文件中没有可导入的账号");
    }
    if matched.len() == 1 && other_provider_skipped == 0 {
        if let Some(remark) = name.filter(|remark| !remark.trim().is_empty()) {
            if let Some(map) = matched[0].as_object_mut() {
                map.insert("name".to_string(), Value::String(remark.trim().to_string()));
                map.insert("nameCustom".to_string(), Value::Bool(true));
            }
        }
    }
    let mut body = json!({ "accounts": matched });
    if let Some(definitions) = custom_providers.filter(Value::is_array) {
        body["customProviders"] = definitions;
    }
    let result = match crate::server::core::account_transfer::import_accounts(state.store(), &body)
    {
        Ok(result) => result,
        Err(error) => return store_error(error),
    };
    let imported_ids = super::account_import_setup::imported_ids(&result);
    for imported_id in &imported_ids {
        if apply_outer_settings {
            if let Err(error) = state
                .store()
                .apply_initial_account_settings(imported_id, outer_payload, true)
            {
                return store_error(error);
            }
        }
        crate::server::core::providers::subscriptions::refresh_after_account_saved(
            state.store().clone(),
            provider,
            imported_id,
        );
    }
    let added = result.get("added").and_then(Value::as_u64).unwrap_or(0);
    let updated = result.get("updated").and_then(Value::as_u64).unwrap_or(0);
    let skipped = result.get("skipped").and_then(Value::as_u64).unwrap_or(0);
    let failed = result.get("failed").and_then(Value::as_u64).unwrap_or(0);
    let total = result.get("total").and_then(Value::as_u64).unwrap_or(0);
    let errors = result
        .get("errors")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let mut response = json!({
        "batch": true,
        "total": total,
        "added": added,
        "updated": updated,
        "skipped": skipped,
        "otherProviderSkipped": other_provider_skipped,
        "failed": failed,
        "errors": errors,
        "importedIds": imported_ids,
        "created": added > 0,
        "proxySaved": true,
        "account": Value::Null,
    });
    super::account_import_setup::complete(state, &imported_ids, &mut response).await;
    if let Some(first_id) = imported_ids.first() {
        response["account"] = state
            .store()
            .plugin_account(first_id)
            .and_then(|record| record.get("public").cloned())
            .unwrap_or(Value::Null);
    }
    response["list"] = state.store().list_visible_accounts();
    ok_json(response)
}
