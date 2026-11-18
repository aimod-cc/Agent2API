use axum::{body::Bytes, response::Response};
use serde_json::Value;

use crate::server::{core::accounts_list::{self, AccountListQuery}, errors::management_error, http::{ok_json, query_param}, ServerState};

pub fn list(state: &ServerState, query: &str) -> Response {
    if query_param(query, "view").as_deref() == Some("index") {
        return crate::server::core::blocking::in_place(|| {
            let mut snapshot = accounts_list::summary(state.store());
            snapshot["accountFeatures"] = super::session::account_features();
            snapshot["providerIcons"] = crate::server::core::plugin_host::provider_icons();
            snapshot["desktopLaunch"] = crate::server::core::plugin_host::wasm::desktop_launch::summaries();
            ok_json(snapshot)
        });
    }
    let Some(size) = query_param(query, "limit") else {
        return ok_json(state.store().list_visible_accounts());
    };
    let Ok(size) = size.parse::<usize>() else { return management_error(400, "每页账号数量无效"); };
    if !(1..=200).contains(&size) { return management_error(400, "每页账号数量必须是 1~200"); }
    let offset = match query_param(query, "offset") {
        Some(value) => match value.parse::<usize>() { Ok(value) => value, Err(_) => return management_error(400, "账号分页位置无效") },
        None => 0,
    };
    let filter = AccountListQuery { page_size: size, offset,
        q: query_param(query, "q").unwrap_or_default(),
        providers: query_param(query, "provider").filter(|value| value != "all").into_iter().collect(),
        enabled: query_param(query, "enabled").unwrap_or_default(),
        limit: query_param(query, "limitState").unwrap_or_default(),
        ..Default::default()
    };
    crate::server::core::blocking::in_place(|| ok_json(accounts_list::page(state.store(), &filter, false)))
}

pub fn query(state: &ServerState, body: &Bytes, ids_only: bool) -> Response {
    let filter = match serde_json::from_slice::<AccountListQuery>(body) {
        Ok(filter) => filter,
        Err(_) => return management_error(400, "账号筛选参数无效"),
    };
    if !(1..=200).contains(&filter.page_size) || filter.q.len() > 4096
        || filter.ids.iter().chain(&filter.providers).chain(&filter.collapsed).any(|id| id.len() > 512) {
        return management_error(400, "账号分页或筛选参数超出范围");
    }
    crate::server::core::blocking::in_place(|| ok_json(accounts_list::page(state.store(), &filter, ids_only)))
}

pub fn detail(state: &ServerState, id: &str) -> Response {
    match state.store().public_account_by_id(id) {
        Some(mut account) => {
            account["detailsLoaded"] = Value::Bool(true);
            ok_json(account)
        }
        None => management_error(404, "账号不存在，请刷新后重试"),
    }
}
