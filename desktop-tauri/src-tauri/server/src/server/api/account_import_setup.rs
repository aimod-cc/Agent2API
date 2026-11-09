use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::server::core::providers::adapter::adapter_by_id;
use crate::server::{logging, ServerState};

const PROVIDER: &str = "officeace";
const IMPORT_BUDGET: Duration = Duration::from_secs(45);

pub(super) async fn complete(state: &ServerState, ids: &[String], result: &mut Value) {
    let deadline = tokio::time::Instant::now() + IMPORT_BUDGET;
    let mut seen = HashSet::new();
    let mut reports = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) || state.store().account_provider_of(id).as_deref() != Some(PROVIDER) {
            continue;
        }
        let name = state.store().route_account(id)
            .and_then(|account| account.get("name").and_then(Value::as_str).map(str::to_string))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        let report = match adapter_by_id(PROVIDER) {
            None => json!({ "status": "skipped", "message": "提供商插件未加载，未检查或开通免费套餐；安装插件后可到「签到中心 → 新手福利」开通" }),
            Some(_) if tokio::time::Instant::now() >= deadline => json!({
                "status": "skipped",
                "message": "本轮导入的套餐检查已达到等待上限，该账号未执行开通；可稍后到「签到中心 → 新手福利」开通",
            }),
            Some(adapter) => {
                let payload = json!({});
                match tokio::time::timeout_at(
                    deadline,
                    adapter.admin_action(state.store(), id, "subscription.preview", &payload),
                ).await {
                    Ok(Ok(value)) => public_report(&value),
                    Ok(Err(error)) if error.status_code == 501 => json!({
                        "status": "skipped",
                        "message": "当前提供商插件不支持开通免费套餐；更新插件后可到「签到中心 → 新手福利」开通",
                    }),
                    Ok(Err(error)) => json!({ "status": "failed", "message": error.message }),
                    Err(_) => json!({
                        "status": "pending",
                        "message": "套餐检查仍在处理，暂不能确认开通结果；账号已保留，请稍后查看套餐状态，勿连续重试",
                    }),
                }
            }
        };
        let mut report = report;
        report["id"] = Value::String(id.clone());
        report["name"] = Value::String(name.clone());
        report["provider"] = Value::String(PROVIDER.to_string());
        logging::log_with_level(
            "[Accounts]",
            &format!("账号 {name} 导入后套餐检查：{}", report["message"].as_str().unwrap_or("未返回结果")),
            if succeeded(&report) { "info" } else { "warn" },
        );
        reports.push(report);
    }
    if reports.is_empty() { return; }
    let notices: Vec<String> = reports.iter().map(|report| format!(
        "{}：{}", report["name"].as_str().unwrap_or("果办账号"), report["message"].as_str().unwrap_or("未返回结果"),
    )).collect();
    let warnings: Vec<Value> = reports.iter().filter(|report| !succeeded(report)).cloned().collect();
    result["subscriptionChecks"] = Value::Array(reports);
    result["importNotice"] = Value::String(notices.join("；"));
    if !warnings.is_empty() {
        result["warning"] = json!(warnings.iter().filter_map(|item| {
            Some(format!("{}：{}", item.get("name")?.as_str()?, item.get("message")?.as_str()?))
        }).collect::<Vec<_>>().join("；"));
        result["warnings"] = Value::Array(warnings);
    }
}

pub(super) fn imported_ids(result: &Value) -> Vec<String> {
    result.get("importedIds").and_then(Value::as_array).into_iter().flatten()
        .filter_map(Value::as_str).map(str::to_string).collect()
}

fn succeeded(report: &Value) -> bool {
    matches!(report.get("status").and_then(Value::as_str), Some("activated" | "already_active"))
}

fn public_report(value: &Value) -> Value {
    match (value.get("status").and_then(Value::as_str), value.get("message").and_then(Value::as_str)) {
        (Some("active"), _) => json!({ "status": "already_active", "message": "套餐已开通，无需重复操作" }),
        (Some("inactive"), _) => json!({
            "status": "skipped",
            "message": "尚未开通体验套餐，请到签到中心确认服务协议后开通；导入账号不会自动下单",
        }),
        (Some("pending"), Some(message)) => json!({ "status": "pending", "message": message }),
        (Some("expired" | "frozen" | "unknown" | "credentials_missing" | "credentials_expired" | "permission_denied" | "unsupported"), Some(message)) => {
            json!({ "status": "skipped", "message": message })
        }
        _ => json!({ "status": "failed", "message": "提供商插件未返回有效的套餐检查结果，暂不能确认开通状态" }),
    }
}
