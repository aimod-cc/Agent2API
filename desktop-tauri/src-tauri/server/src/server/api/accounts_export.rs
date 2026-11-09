use axum::body::Bytes;
use axum::http::header::CACHE_CONTROL;
use axum::http::HeaderValue;
use axum::response::Response;
use serde_json::{json, Value};

use super::accounts::{parse_json_body, store_error};
use crate::server::core::account_transfer::{
    export_account_files, ExportSelection, ACCOUNT_FILE_FORMAT,
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

pub async fn import_account_file(
    state: &ServerState,
    provider: &str,
    payload: &Value,
    name: Option<&str>,
) -> Option<Response> {
    let selected_proxy = payload.get("proxy");
    let file = payload
        .get("authFileContent")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let payload = file.as_ref().unwrap_or(payload);
    if payload.get("format").and_then(Value::as_str) != Some(ACCOUNT_FILE_FORMAT) {
        return None;
    }
    if payload.get("provider").and_then(Value::as_str) != Some(provider) {
        return Some(management_error(
            400,
            "账号文件的提供商与当前选择不一致，请选择对应的提供商后再导入",
        ));
    }
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if id.is_empty() {
        return Some(management_error(400, "账号导出文件缺少 id"));
    }
    let mut account = payload.clone();
    if let Some(proxy) = selected_proxy {
        account["proxy"] = proxy.clone();
    }
    if let Some(name) = name.filter(|name| !name.trim().is_empty()) {
        account["name"] = Value::String(name.trim().to_string());
        account["nameCustom"] = Value::Bool(true);
    }
    let mut body = json!({ "accounts": [account] });
    if let Some(definitions) = payload.get("customProviders") {
        body["customProviders"] = definitions.clone();
    }
    let result = match crate::server::core::account_transfer::import_accounts(state.store(), &body)
    {
        Ok(result) => result,
        Err(error) => return Some(store_error(error)),
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
        return Some(management_error(400, message));
    }
    let imported_id = result
        .get("importedIds")
        .and_then(Value::as_array)
        .and_then(|ids| ids.first())
        .and_then(Value::as_str)
        .unwrap_or(id);
    let mut response = json!({
        "created": result.get("added").and_then(Value::as_u64).unwrap_or(0) > 0,
    });
    super::account_import_setup::complete(state, &[imported_id.to_string()], &mut response).await;
    response["account"] = state.store().plugin_account(imported_id)
        .and_then(|record| record.get("public").cloned())
        .unwrap_or(Value::Null);
    response["list"] = state.store().list_visible_accounts();
    Some(ok_json(response))
}
