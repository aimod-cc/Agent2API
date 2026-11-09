use serde_json::Value;

use crate::server::core::login::LoginProxySelection;
use crate::server::errors::GatewayError;

pub(super) fn action_payload(
    request: &Value,
    mut payload: Value,
) -> Result<(Value, Option<LoginProxySelection>), GatewayError> {
    let selection = super::checked_login_proxy(request.get("proxy"))?;
    if let Some(proxy) = &selection {
        if !payload.is_object() {
            payload = serde_json::json!({});
        }
        payload["proxy"] = proxy.resolved.clone();
    }
    Ok((payload, selection))
}
