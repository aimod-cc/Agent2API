//! 旧环境凭据只在用户本次显式选择时读取，不向插件开放任意环境变量。
use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::context::Context;

fn text(name: &'static str) -> Result<String, proto::WireError> {
    let value = std::env::var(name).unwrap_or_default();
    let value = value.trim();
    if value.len() > 64 * 1024 || value.chars().any(char::is_control) { return Err(invalid()); }
    Ok(value.to_string())
}
fn invalid() -> proto::WireError {
    proto::WireError::with_code("旧环境凭据未设置、缺少账号标识或格式不支持，请检查网关启动配置", "legacy_credentials_unavailable")
}

pub(super) fn selected(provider: &str, params: &Value) -> Result<bool, proto::WireError> {
    let source = &params["source"];
    if source["kind"] != "manual" || source["payload"]["inputs"].get("legacyEnvironment").is_none() {
        return Ok(false);
    }
    let index = match provider {
        "raccoon" | "autoclaw" | "catpaw" => 4,
        "autoclaw-intl" => 5,
        "workbuddy" | "workbuddy-intl" => 3,
        _ => return Err(invalid()),
    };
    if source["payload"]["methodIndex"] != index
        || source["payload"]["inputs"].as_object().is_none_or(|fields| fields.len() != 1)
        || source["payload"]["inputs"]["legacyEnvironment"] != "import" {
        return Err(invalid());
    }
    Ok(true)
}

pub(super) fn prepare(provider: &str) -> Result<Value, proto::WireError> {
    let result = match provider {
        "raccoon" => json!({ "accessToken": text("RACCOON_TOKEN")? }),
        "autoclaw" => json!({ "token": text("AUTOCLAW_TOKEN")?, "refreshToken": text("AUTOCLAW_REFRESH_TOKEN")?,
            "deviceId": text("AUTOCLAW_DEVICE_ID")? }),
        "autoclaw-intl" => json!({ "token": text("AUTOCLAW_INTL_TOKEN")?, "refreshToken": text("AUTOCLAW_INTL_REFRESH_TOKEN")?,
            "deviceId": text("AUTOCLAW_INTL_DEVICE_ID")?, "provider": provider }),
        "catpaw" => {
            let raw = text("CATPAW_COOKIE")?;
            let mut token = raw.as_str();
            for part in raw.split(';') {
                if let Some((name, value)) = part.trim().split_once('=') {
                    if name.eq_ignore_ascii_case("X-Passport-Token") { token = value.trim(); break; }
                }
            }
            json!({ "token": token, "uid": text("CATPAW_USER_UID")? })
        }
        "workbuddy" | "workbuddy-intl" => {
            let intl = provider == "workbuddy-intl";
            let mut token = text(if intl { "WORKBUDDY_INTL_TOKEN" } else { "WORKBUDDY_TOKEN" })?;
            if token.is_empty() && intl && text("WORKBUDDY_EDITION")?.eq_ignore_ascii_case("intl") {
                token = text("WORKBUDDY_TOKEN")?;
            }
            if !intl && text("WORKBUDDY_EDITION")?.eq_ignore_ascii_case("intl") {
                return Err(invalid());
            }
            json!({ "accessToken": token, "edition": if intl { "intl" } else { "cn" } })
        }
        _ => return Err(invalid()),
    };
    if ["accessToken", "token"].iter().all(|key| result[*key].as_str().is_none_or(str::is_empty)) {
        return Err(invalid());
    }
    Ok(result)
}

pub(super) fn take(context: &mut Context, params: &Value) -> Result<Value, proto::WireError> {
    context.permission("credentials.legacy-import.v1")?;
    if params.as_object().is_none_or(|fields| !fields.is_empty()) { return Err(invalid()); }
    context.supplied_legacy_credentials.take().ok_or_else(invalid)
}
