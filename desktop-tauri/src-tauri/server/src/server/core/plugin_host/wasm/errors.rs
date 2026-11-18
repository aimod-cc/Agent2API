use agent2api_provider_protocol as proto;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Failure {
    message: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    upstream_code: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
    #[serde(default)]
    credential_scope: Option<String>,
}

pub(super) fn decode(mut value: Value) -> Result<Value, proto::WireError> {
    if let Some(error) = value.get("error") {
        let error: Failure = serde_json::from_value(error.clone()).map_err(|_| protocol())?;
        if error.message.len() > 16 * 1024 || error.code.as_ref().is_some_and(|code| code.len() > 4096)
            || error.reset_at.is_some_and(|reset| reset < 0 || reset > (1_i64 << 53) - 1
                || chrono::DateTime::from_timestamp_millis(reset).is_none())
            || error.credential_scope.as_deref().is_some_and(|scope| !super::super::errors::credential_scope_allowed(scope)) {
            return Err(protocol());
        }
        if let Some(status) = error.status {
            if !(400..600).contains(&status) { return Err(protocol()); }
            return Err(proto::WireError::with_code(error.message, format!("provider_http:{}", serde_json::json!({
                "status": status, "code": error.code, "upstreamCode": error.upstream_code,
                "resetAt": error.reset_at, "credentialScope": error.credential_scope,
            }))));
        }
        return Err(proto::WireError { message: error.message, code: error.code });
    }
    value.as_object_mut().and_then(|object| object.remove("result")).ok_or_else(protocol)
}

pub(super) fn protocol() -> proto::WireError {
    proto::WireError::with_code("Wasm 插件返回了无效的协议消息", "plugin_protocol_error")
}

pub(super) fn interrupted(error: &wasmtime::Error) -> proto::WireError {
    let (code, message) = match error.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) =>
            ("plugin_resource_limit", "Wasm 插件计算额度耗尽"),
        Some(wasmtime::Trap::StackOverflow) =>
            ("plugin_resource_limit", "Wasm 插件执行栈溢出"),
        Some(_) => ("plugin_trap", "Wasm 插件执行中断"),
        None => ("plugin_protocol_error", "Wasm 插件超过资源限制或返回了无效消息"),
    };
    proto::WireError::with_code(message, code)
}
