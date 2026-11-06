use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::billing::BillingService;
use crate::server::core::providers::{adapter::adapter_by_id, DEFAULT_PROVIDER_ID};
use crate::server::logging;

use super::claim_result;

pub(super) async fn checkin(
    store: &AccountStore,
    billing: &BillingService,
    id: String,
    name: Value,
    display: &str,
) -> Value {
    let Some(entry) = store.get_session_by_id(&id) else {
        return json!({
            "id": id,
            "name": name,
            "claim": Value::Null,
            "error": "没有可用凭证",
        });
    };
    let claim = billing
        .claim_daily_checkin(Some(&entry.session))
        .await
        .map_err(|error| error.message);
    let mut row = claim_result(id.clone(), name, display, false, claim);
    attach_growth(store, &id, &entry.session, display, &mut row).await;

    // 一次性运营奖励只保留在旧计费路径，插件签到不在这里重复领取。
    if !is_enterprise_session(&entry.session) {
        let mut rewards: Vec<Value> = Vec::new();
        for reward in [
            billing.claim_gift(&entry.session).await,
            billing.claim_compensation(&entry.session).await,
        ] {
            match reward {
                Ok(reward) => {
                    let credited = reward.get("success").and_then(Value::as_bool) == Some(true)
                        && reward.get("credit").and_then(Value::as_i64).unwrap_or(0) > 0;
                    if credited {
                        let message = reward.get("msg").and_then(Value::as_str).unwrap_or("");
                        logging::log("[Accounts]", &format!("账号 {display}: {message}"));
                    }
                    rewards.push(reward);
                }
                Err(error) => {
                    logging::verbose(
                        "[Accounts]",
                        &format!("账号 {display} 运营奖励未领取: {}", error.message),
                    );
                }
            }
        }
        if !rewards.is_empty() {
            if let Some(object) = row.as_object_mut() {
                object.insert("rewards".to_string(), Value::Array(rewards));
            }
        }
    }
    row
}

/// 企业版没有个人成长 / 补偿体系，空 enterpriseId 仍按个人版处理。
fn is_enterprise_session(session: &Value) -> bool {
    session
        .get("account")
        .and_then(|account| account.get("enterpriseId"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

/// 成长中心连登与签到活动连签是两套读数，缺失时不能互相补齐。
pub(super) async fn attach_growth(
    store: &AccountStore,
    id: &str,
    session: &Value,
    display: &str,
    row: &mut Value,
) {
    if is_enterprise_session(session) {
        return;
    }
    let growth = match adapter_by_id(DEFAULT_PROVIDER_ID) {
        Some(adapter) => match adapter
            .admin_action(store, id, "growth.daily", &Value::Null)
            .await
        {
            Ok(growth) => growth,
            Err(error) => json!({
                "reported": false,
                "streakDays": Value::Null,
                "error": error.message,
            }),
        },
        None => json!({
            "reported": false,
            "streakDays": Value::Null,
            "error": "没有可用的 WorkBuddy 适配器（未安装插件？）",
        }),
    };
    if let Some(error) = growth
        .get("error")
        .and_then(Value::as_str)
        .filter(|error| !error.trim().is_empty())
    {
        logging::verbose(
            "[Accounts]",
            &format!("账号 {display} 每日活跃上报失败: {error}"),
        );
    } else {
        // 刚上报后的 0 可能是尚未结算的旧读数，不能覆盖已有连登。
        let days = growth
            .get("streakDays")
            .and_then(Value::as_i64)
            .filter(|days| *days > 0);
        if let Some(days) = days {
            if !store.mark_streak_days(id, days) {
                logging::verbose("[Accounts]", &format!("账号 {id} 的连登天数未能落盘"));
            }
        }
        if growth.get("reported").and_then(Value::as_bool) == Some(true) {
            logging::verbose(
                "[Accounts]",
                &format!(
                    "账号 {display}: 每日活跃已上报{}",
                    days.map(|days| format!("（连登 {days} 天）"))
                        .unwrap_or_default()
                ),
            );
        }
    }
    // `activity` 会被当作签到完成；国内版成长结果必须单独保存在 `growth`。
    if let Some(object) = row.as_object_mut() {
        object.insert("growth".to_string(), growth);
    }
}
