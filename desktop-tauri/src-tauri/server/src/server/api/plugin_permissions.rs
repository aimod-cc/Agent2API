use std::collections::HashMap;

use axum::{body::Bytes, extract::Query, response::Response};

use crate::server::{
    core::plugin_host::wasm::{
        account_origin,
        managed_task::{declaration, permissions as managed_tasks},
    },
    errors::management_error,
    http::{ok_json, parse_body},
};
use serde_json::Value;

async fn snapshot(pack: &str) -> Result<Value, String> {
    // 两类权限各自获取变更锁；外层不能持有同一把锁再调用它们。
    let mut value = account_origin::snapshot(pack).await?;
    value["managedTasks"] = managed_tasks::snapshot(pack).await?["managedTasks"].clone();
    Ok(value)
}

pub async fn list(Query(query): Query<HashMap<String, String>>) -> Response {
    let Some(pack) = query
        .get("pack")
        .filter(|pack| !pack.is_empty() && pack.len() <= 128)
    else {
        return management_error(400, "缺少有效插件标识");
    };
    match snapshot(pack).await {
        Ok(value) => ok_json(value),
        Err(message) => management_error(400, message),
    }
}

pub async fn configure(body: Bytes) -> Response {
    if body.len() > 16 * 1024 {
        return management_error(413, "插件权限授权输入过大");
    }
    let value = match parse_body(&body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "插件权限授权请求不是有效 JSON"),
    };
    if value
        .get("kind")
        .is_some_and(|kind| kind.as_str() != Some("managedTask"))
        && declaration::parse_object::<Value>(&body, 16 * 1024).is_err()
    {
        return management_error(400, "插件权限授权请求字段无效或存在重复字段");
    }
    match value.get("kind") {
        None => configure_account_origin(value).await,
        Some(Value::String(kind)) if kind == "managedTask" => {
            // 从原始字节严格解析，不能让 Value 覆盖重复 kind、摘要或 authorized。
            let change = match managed_tasks::Change::parse(&body) {
                Ok(change) => change,
                Err(message) => return management_error(400, message),
            };
            let pack = change.pack().to_string();
            // 先读取另一类权限，避免写入后再遇读取失败而隐藏 saved:false。
            let origins = match account_origin::snapshot(&pack).await {
                Ok(value) => value,
                Err(message) => return management_error(400, message),
            };
            match managed_tasks::configure(change).await {
                Ok(mut value) => {
                    value["accountOrigins"] = origins["accountOrigins"].clone();
                    ok_json(value)
                }
                Err(message) => management_error(400, message),
            }
        }
        Some(_) => management_error(400, "插件权限授权类型无效"),
    }
}

async fn configure_account_origin(value: Value) -> Response {
    let change = match serde_json::from_value::<account_origin::Change>(value) {
        Ok(change) => change,
        Err(_) => return management_error(400, "账号来源授权请求字段无效"),
    };
    // 无 kind 的旧请求仍使用原校验、保存和授权语义。
    let tasks = managed_tasks::snapshot(&change.pack).await.ok();
    match account_origin::configure(change).await {
        Ok(mut value) => {
            if let Some(tasks) = tasks {
                value["managedTasks"] = tasks["managedTasks"].clone();
            }
            ok_json(value)
        }
        Err(message) => management_error(400, message),
    }
}
