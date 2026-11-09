use serde_json::{json, Value};

use super::{client::CodexClient, now, CodingError};

pub async fn account(client: &CodexClient) -> Result<Value, CodingError> {
    client.check_current().await?;
    let mut cache = client.usage_cache.lock().await;
    if let Some((at, data)) = cache.as_ref().filter(|(at, _)| now().saturating_sub(*at) < 60_000) {
        return Ok(json!({ "available": true, "data": data, "updatedAt": at, "stale": false }));
    }
    let account = client.rpc("account/read", json!({ "refreshToken": false })).await?;
    let kind = account.pointer("/account/type").and_then(Value::as_str).unwrap_or("");
    if !matches!(kind, "chatgpt" | "chatgptAuthTokens") {
        *cache = None;
        return Ok(json!({ "available": false, "reason": "account_type", "updatedAt": null }));
    }
    match client.rpc("account/rateLimits/read", json!({})).await {
        Ok(data) => {
            let at = now();
            *cache = Some((at, data.clone()));
            Ok(json!({ "available": true, "data": data, "updatedAt": at, "stale": false }))
        }
        Err(error) => {
            if let Some((at, data)) = cache.as_ref() {
                Ok(json!({ "available": true, "data": data, "updatedAt": at, "stale": true, "error": error.message }))
            } else {
                Ok(json!({ "available": false, "reason": "unavailable", "error": error.message, "updatedAt": null }))
            }
        }
    }
}
