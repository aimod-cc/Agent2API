use serde_json::{Map, Value};

use super::state::StoredAccount;

const EXPIRY_KEYS: &[&str] = &[
    "accessTokenExpiresAt", "expiresAt", "tokenExpiresAt", "expiresAtMs", "tokenExpireAt", "expires_at", "expiration",
];
const REFRESH_EXPIRY_KEYS: &[&str] = &[
    "refreshTokenExpiresAt", "refreshExpiresAt", "refresh_expires_at", "refresh_token_expires_at",
];

pub(super) fn credential_fields(record: &StoredAccount, public: &mut Map<String, Value>) {
    let secret = record.get("pluginSecret").and_then(Value::as_object);
    // Copilot 的 auth.refresh 保存 GitHub Token，不是 OAuth Refresh Token。
    let refresh_keys: &[&str] = if record.provider() == "copilot-subscription" {
        &["refreshToken", "refresh_token"]
    } else {
        &["refreshToken", "refresh_token", "refresh"]
    };
    let has_refresh = token_present(record, public, refresh_keys, "hasRefreshToken");
    let can_refresh = public.get("canRefresh").and_then(Value::as_bool).unwrap_or(false) || has_refresh;
    public.insert("hasRefreshToken".to_string(), Value::Bool(has_refresh));
    public.insert("canRefresh".to_string(), Value::Bool(can_refresh));
    let has_access = token_present(record, public, &["accessToken", "access_token", "access", "token"], "hasAccessToken")
        || record.provider() == "codearts" && token_present(record, public, &["access_key_id"], "hasAccessToken");
    public.insert("hasAccessToken".to_string(), Value::Bool(has_access));
    if record.provider() == "codearts" {
        for key in ["needsRelogin", "needsReloginReason"] {
            if let Some(value) = secret.and_then(|fields| fields.get(key)) {
                public.insert(key.to_string(), value.clone());
            }
        }
    }
    if record.provider() == super::CATPAW_PROVIDER_ID {
        let token = match secret {
            Some(fields) => fields.get("balanceToken"),
            None => record.get("balanceToken"),
        };
        public.insert("hasBalanceToken".to_string(), Value::Bool(token.and_then(Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())));
    }

    // 只派生白名单标量，凭据对象不进入公开视图；陈旧快照不能压住新的到期时刻。
    let expiry = EXPIRY_KEYS.iter().flat_map(|key| {
        [public.get(*key), secret.and_then(|fields| fields.get(*key)), record.get(key)]
    }).flatten().filter_map(expiry_millis).max();
    if let Some(expiry) = expiry {
        for key in ["accessTokenExpiresAt", "expiresAt", "tokenExpiresAt", "expiresAtMs", "tokenExpireAt"] {
            public.insert(key.to_string(), Value::from(expiry));
        }
    }
    let refresh_expiry = REFRESH_EXPIRY_KEYS.iter().flat_map(|key| {
        [secret.and_then(|fields| fields.get(*key)), record.get(key)]
    }).flatten().find_map(expiry_millis).or_else(|| {
        REFRESH_EXPIRY_KEYS.iter().filter_map(|key| public.get(*key)).find_map(expiry_millis)
    });
    if let Some(expiry) = refresh_expiry {
        public.insert("refreshTokenExpiresAt".to_string(), Value::from(expiry));
    }
}

fn token_present(record: &StoredAccount, public: &Map<String, Value>, keys: &[&str], flag: &str) -> bool {
    let secret = record.get("pluginSecret").and_then(Value::as_object);
    let fields = secret.unwrap_or(record.fields());
    let mut values = keys.iter().filter_map(|key| {
        fields.get("auth").and_then(|auth| auth.get(*key)).or_else(|| fields.get(*key))
    }).peekable();
    if values.peek().is_some() {
        return values.any(|value| value.as_str().is_some_and(|token| !token.trim().is_empty()));
    }
    // 持有本机引用或不透明凭证时，公开视图仍可声明存在标记；直接凭证缺字段则不能沿用旧快照。
    let indirect = record.is_desktop() || secret.is_some_and(|fields| {
        fields.get("auth").is_none_or(|auth| !auth.is_object())
            && !fields.keys().any(|key| matches!(key.as_str(), "accessToken" | "access_token" | "refreshToken"
                | "refresh_token" | "access" | "refresh" | "token" | "access_key_id" | "accessKeyId"
                | "secretAccessKey" | "securityToken" | "modelAppKey" | "model_app_key" | "appKey"
                | "apiKey" | "api_key" | "key" | "cookie" | "cookies" | "session" | "sessionToken"))
    });
    indirect && public.get(flag).and_then(Value::as_bool) == Some(true)
}

pub(super) fn is_private_field(key: &str) -> bool {
    matches!(key,
        "pluginSecret" | "pluginPublic" | "pluginIdentity" | "accessToken" | "refreshToken"
        | "access_token" | "refresh_token" | "apiKey" | "api_key" | "password"
        | "balanceToken" | "workbuddyWebSession" | "access_key_id" | "secret_access_key" | "security_token" | "oauth_context"
    )
}

fn expiry_millis(value: &Value) -> Option<i64> {
    let raw = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
                return (time.timestamp_millis() > 0).then(|| time.timestamp_millis());
            }
            text.parse::<f64>().ok()
        }
        _ => None,
    }?;
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let millis = if raw < 1e11 { raw * 1000.0 } else { raw };
    if millis >= i64::MAX as f64 {
        return None;
    }
    (millis >= 1.0).then_some(millis as i64)
}
