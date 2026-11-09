use agent2api_provider_protocol as proto;
use serde_json::Value;

fn normalized(key: &str) -> String { key.to_ascii_lowercase().replace(['_', '-'], "") }

fn secret_key(key: &str) -> bool {
    let key = normalized(key);
    key.contains("token") || key.contains("password") || key.contains("cookie") || key.contains("secret")
        || key.contains("authorization") || key.contains("apikey") || key == "key"
}

fn safe_scalar(key: &str, value: &Value) -> bool {
    match normalized(key).as_str() {
        "hasaccesstoken" | "hasrefreshtoken" | "hasbalancetoken" | "hastoken" | "hasapikey"
        | "hascookie" | "refreshtokenlonglived" => value.is_boolean(),
        "tokenexpiresat" | "tokenexpireat" | "accesstokenexpiresat" | "refreshtokenexpiresat" =>
            value.is_null() || value.as_i64().is_some_and(|value| value >= 0),
        "tokentail" => value.is_null() || value.as_str().is_some_and(|text| text.chars().count() <= 6),
        _ => false,
    }
}

pub fn validate(public: &Value, secret: &Value) -> Result<(), proto::WireError> {
    fn collect(value: &Value, protected: bool, out: &mut Vec<String>, depth: usize) {
        if depth > 32 { return; }
        match value {
            Value::Object(map) => for (key, value) in map { collect(value, protected || secret_key(key), out, depth + 1); },
            Value::Array(items) => for item in items { collect(item, protected, out, depth + 1); },
            Value::String(text) if protected && text.len() >= 8 => out.push(text.clone()),
            _ => {}
        }
    }
    fn check(value: &Value, secrets: &[String], depth: usize) -> bool {
        if depth > 16 { return false; }
        match value {
            Value::Object(map) => map.iter().all(|(key, value)|
                (!secret_key(key) || safe_scalar(key, value)) && check(value, secrets, depth + 1)),
            Value::Array(items) => items.iter().all(|item| check(item, secrets, depth + 1)),
            Value::String(text) => !secrets.iter().any(|secret| text.contains(secret)),
            _ => true,
        }
    }
    if !secret.is_object() || secret.to_string().len() > 256 * 1024
        || !public.is_object() || public.to_string().len() > 64 * 1024 {
        return Err(super::error("插件公开视图或凭据格式无效、超过大小限制"));
    }
    let mut secrets = Vec::new();
    collect(secret, false, &mut secrets, 0);
    if !check(public, &secrets, 0) {
        return Err(super::error("插件公开视图包含受保护内容或字段类型无效"));
    }
    Ok(())
}
