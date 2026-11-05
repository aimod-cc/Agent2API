use serde_json::Value;

use super::state::StoredAccount;
use crate::server::core::beijing;

pub(super) const LEDGER_KIND: &str = "dailyCheckin";
const LEDGER_KEY: &str = "pluginLedger.dailyCheckin";

pub(super) fn record_status(record: &StoredAccount) -> Option<&Value> {
    record.get(LEDGER_KEY).filter(|value| {
        value.get("at").and_then(Value::as_i64)
            .and_then(beijing::date_key_of_ms)
            .is_some_and(|date| date == beijing::today_key())
            && value.get("completed").and_then(Value::as_bool).is_some()
    })
}

pub(super) fn completed(record: &StoredAccount) -> bool {
    record_status(record)
        .and_then(|value| value.get("completed"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            record.checkin_at() > 0 && beijing::date_key_of_ms(record.checkin_at())
                .is_some_and(|date| date == beijing::today_key())
        })
}

pub(super) fn apply(record: &mut StoredAccount, value: &Value) -> Result<(), String> {
    if value.is_null() { return Ok(()); }
    let at = value.get("at").and_then(Value::as_i64)
        .filter(|at| *at > 0 && beijing::date_key_of_ms(*at).is_some())
        .ok_or_else(|| "每日签到状态缺少有效时间戳".to_string())?;
    let completed = value.get("completed").and_then(Value::as_bool)
        .ok_or_else(|| "每日签到状态缺少完成标记".to_string())?;
    let count = value.get("completedCount").and_then(Value::as_u64);
    let total = value.get("totalCount").and_then(Value::as_u64);
    if count.zip(total).is_some_and(|(count, total)| count > total)
        || completed && (!count.zip(total).is_some_and(|(count, total)| total > 0 && count == total)
            || value.get("sourcesComplete").and_then(Value::as_bool) != Some(true))
    {
        return Err("每日签到状态的领取数量或来源状态不一致".to_string());
    }
    if let Some(status) = value.get("status") {
        let status = status.as_str().filter(|status| matches!(*status,
            "claimed" | "done" | "confirming" | "inactive" | "ineligible" | "captcha" | "failed"))
            .ok_or_else(|| "每日签到状态类型无效".to_string())?;
        if completed != matches!(status, "claimed" | "done") {
            return Err("每日签到状态与完成标记不一致".to_string());
        }
    }
    if value.get("message").is_some_and(|message| !message.is_string()
        || message.as_str().is_some_and(|message| message.chars().count() > 2000))
        || value.get("queryOnly").is_some_and(|query| !query.is_boolean())
        || value.get("retryAt").is_some_and(|retry| !retry.is_null()
            && !retry.as_i64().is_some_and(|retry| retry > 0 && beijing::date_key_of_ms(retry).is_some()))
    {
        return Err("每日签到状态的说明或查询时间无效".to_string());
    }
    if completed && at >= record.checkin_at() {
        record.set_checkin_at(at);
    }
    Ok(())
}

pub(crate) fn checked_in_today(account: &Value) -> bool {
    account.get("checkedInToday").and_then(Value::as_bool).unwrap_or_else(|| {
        account.get("checkinAt").and_then(Value::as_i64)
            .filter(|at| *at > 0)
            .and_then(beijing::date_key_of_ms)
            .is_some_and(|date| date == beijing::today_key())
    })
}
