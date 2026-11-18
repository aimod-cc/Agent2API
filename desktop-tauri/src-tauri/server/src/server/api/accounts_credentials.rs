use axum::{body::Bytes, response::Response};
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroize;

use crate::server::{core::account_store::credential_version::FreeEnrollmentInput,
    errors::management_error, http::ok_json, ServerState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Replacement {
    credential_json: String,
    confirmed: bool,
    expected_updated_at: i64,
    #[serde(default)]
    authorize_refresh: bool,
}

impl Drop for Replacement {
    fn drop(&mut self) { self.credential_json.zeroize(); }
}

pub(super) async fn replace_cline_free(state: &ServerState, id: &str, body: &Bytes) -> Response {
    if body.len() > 1024 * 1024 {
        return management_error(400, "凭证输入过大，请只粘贴当前账号的完整凭证 JSON");
    }
    let replacement: Replacement = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "凭证更新参数无效，未保存"),
    };
    if !replacement.confirmed || replacement.expected_updated_at < 0 {
        return management_error(400, "请先确认更新当前账号的登录凭证");
    }
    let input = match FreeEnrollmentInput::parse(replacement.credential_json.as_bytes()) {
        Ok(input) => input,
        Err(error) => return management_error(400, error.message()),
    };
    let store = state.store().clone();
    let id = id.to_string();
    let expected_updated_at = replacement.expected_updated_at;
    let authorize_refresh = replacement.authorize_refresh;
    let saved = crate::server::core::blocking::run(move ||
        store.replace_cline_free_credentials(&id, expected_updated_at, input, authorize_refresh)).await;
    match saved {
        Ok(Ok(account)) => {
            crate::server::core::usage_query::refresh_configuration(state.store());
            ok_json(json!({ "updated": true, "account": account }))
        }
        Ok(Err(error)) => management_error(error.status_code, error.message),
        Err(_) => management_error(503, "凭证保存结果未确认，请刷新后核对；不会自动重试"),
    }
}
