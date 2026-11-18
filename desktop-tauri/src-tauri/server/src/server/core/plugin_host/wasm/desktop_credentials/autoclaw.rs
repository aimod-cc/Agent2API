use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};

use super::{app_data, content, file, home, invalid, object, project};

#[cfg(windows)]
mod crypto;

fn unseal(raw: &str, state: &std::path::Path) -> Result<String, proto::WireError> {
    if !raw.starts_with("enc:") {
        return Ok(raw
            .trim()
            .strip_prefix("Bearer ")
            .unwrap_or(raw.trim())
            .to_string());
    }
    #[cfg(windows)]
    return crypto::decrypt(raw, &file(state, 2 * 1024 * 1024)?);
    #[cfg(not(windows))]
    {
        let _ = state;
        Err(invalid())
    }
}

pub(super) fn seal(token: &str, previous: &str, state: &std::path::Path) -> Result<String, proto::WireError> {
    if !previous.starts_with("enc:") { return Ok(token.to_string()); }
    #[cfg(windows)]
    return crypto::encrypt(token, &file(state, 2 * 1024 * 1024)?);
    #[cfg(not(windows))]
    { let _ = state; Err(invalid()) }
}

fn claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn projected(token: String, refresh: String, original: &Value) -> Value {
    let claims = claims(&token);
    let mut result = project(original, &["deviceId", "email"]);
    result["token"] = json!(token);
    result["refreshToken"] = json!(refresh);
    result["userId"] = claims["user_id"]
        .as_str()
        .map(|value| json!(value))
        .unwrap_or_else(|| {
            json!(claims["user_id"]
                .as_i64()
                .map(|value| value.to_string())
                .unwrap_or_default())
        });
    if result["deviceId"].is_null() {
        result["deviceId"] = claims["device_id"].clone();
    }
    if let Some(seconds) = claims["exp"].as_i64() {
        result["expiresAt"] = json!(seconds.saturating_mul(1000));
    }
    if result["email"].is_null() {
        result["email"] = original["userInfo"]["email"].clone();
    }
    result
}

pub(super) fn read(provider: &str) -> Result<Value, proto::WireError> {
    let root =
        super::absolute_env("AUTOCLAW_USER_DATA_DIR").unwrap_or(app_data()?.join("AutoClaw"));
    let path = root.join("auth.json");
    let mut response = if path.exists() {
        let value = object(&path, 256 * 1024)?;
        let state = root.join("Local State");
        let token = unseal(value["token"].as_str().unwrap_or(""), &state)?;
        let refresh = unseal(value["refreshToken"].as_str().unwrap_or(""), &state)?;
        if token.is_empty() {
            return Err(invalid());
        }
        let mut result = content(&path, projected(token, refresh, &value))?;
        result["source"] = json!("auth-file");
        result
    } else {
        let path = home()?.join(".openclaw-autoclaw/openclaw.json");
        let value = object(&path, 256 * 1024)?;
        let mut tokens = Vec::new();
        for model in value["models"]["providers"]
            .as_object()
            .into_iter()
            .flat_map(|items| items.values())
            .flat_map(|provider| provider["models"].as_array().into_iter().flatten())
        {
            for key in ["X-Authorization", "x-authorization"] {
                if let Some(token) = model["headers"][key]
                    .as_str()
                    .filter(|token| !token.is_empty())
                {
                    tokens.push(unseal(token, &root.join("Local State"))?);
                }
            }
        }
        tokens.sort();
        tokens.dedup();
        if tokens.len() != 1 {
            return Err(invalid());
        }
        let mut result = content(
            &path,
            projected(tokens.remove(0), String::new(), &json!({})),
        )?;
        result["source"] = json!("gateway-config");
        result
    };
    response
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("sourceId");
    if provider == "autoclaw-intl" {
        response["provider"] = json!(provider);
    }
    Ok(response)
}

pub(super) fn decrypt(provider: &str, payload: &Value) -> Result<Value, proto::WireError> {
    if payload.as_object().is_none_or(|fields| {
        fields
            .keys()
            .any(|key| !matches!(key.as_str(), "token" | "refreshToken"))
    }) {
        return Err(invalid());
    }
    let root =
        super::absolute_env("AUTOCLAW_USER_DATA_DIR").unwrap_or(app_data()?.join("AutoClaw"));
    let path = root.join("Local State");
    let token = payload["token"].as_str().unwrap_or("");
    let refresh = payload["refreshToken"].as_str().unwrap_or("");
    if token.len() > 32768 || refresh.len() > 32768 {
        return Err(invalid());
    }
    let mut result = json!({"token":unseal(token,&path)?,"refreshToken":unseal(refresh,&path)?});
    if provider == "autoclaw-intl" {
        result["provider"] = json!(provider);
    }
    Ok(result)
}
