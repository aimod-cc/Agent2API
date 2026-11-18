//! 旧 JSON 字符串凭据只在本次调用中投影成对象；写回仍比较最初读取的完整原文。
use serde_json::Value;

use super::context::Context;

pub(super) struct Snapshot {
    account: String,
    original: Value,
    parsed: Value,
}

pub(super) fn decode(value: &Value) -> Value {
    value.as_str().filter(|raw| raw.len() <= 256 * 1024)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok()).filter(Value::is_object)
        .unwrap_or_else(|| value.clone())
}

pub(super) fn project(record: &mut Value) {
    record["secret"] = decode(&record["secret"]);
}

pub(super) fn expose(context: &mut Context, record: &mut Value) {
    let Some(raw) = record["secret"].as_str().filter(|raw| raw.len() <= 256 * 1024) else { return; };
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else { return; };
    if !parsed.is_object() { return; }
    if context.legacy_secret.is_none() {
        context.legacy_secret = Some(Snapshot {
            account: context.account.clone(), original: record["secret"].clone(), parsed: parsed.clone(),
        });
    }
    record["secret"] = parsed;
}

pub(super) fn original(context: &Context, account: &str, expect: &Value) -> Value {
    match context.legacy_secret.as_ref() {
        Some(snapshot) if snapshot.account == account && snapshot.parsed == *expect => snapshot.original.clone(),
        _ => expect.clone(),
    }
}
