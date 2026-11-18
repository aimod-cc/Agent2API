//! 原Runtime返回后的固定字段投影。输入只借用，不clone任意Value/错误文本。
//! 原生checkin_state将confirming编码为inactive；不可因此断言“无活动/未发送”。
//! rewards的task_status=3也不能替代每日checked_in_today；此处不从它补造完成状态。

use agent2api_provider_protocol as proto;
use serde_json::{Map, Value};

use super::RecordedReply;

const MAX_FIELDS: usize = 32;

pub(super) fn read(raw: &Value) -> RecordedReply {
    parse(raw).unwrap_or(RecordedReply::InvalidReply)
}

fn parse(raw: &Value) -> Option<RecordedReply> {
    let envelope = object(raw)?;
    if envelope.len() != 1 { return None; }
    if let Some(error) = envelope.get("error") {
        let error = object(error)?;
        // 只核对格式后丢弃message/code等文本，不把上游或插件文字放进托管槽。
        if error.keys().any(|name| !matches!(name.as_str(),
            "message" | "code" | "status" | "upstream_code" | "reset_at" | "credential_scope")) { return None; }
        let message = error.get("message")?.as_str()?;
        if message.len() > 16 * 1024 || !optional_text(error, "code", 4096) { return None; }
        let status = optional_u32(error, "status")?;
        if status.is_some_and(|status| !(400..600).contains(&status)) { return None; }
        optional_i64(error, "upstream_code")?;
        if optional_i64(error, "reset_at")?.is_some_and(|at| at < 0 || at > (1_i64 << 53) - 1
            || chrono::DateTime::from_timestamp_millis(at).is_none()) { return None; }
        if let Some(scope) = present(error, "credential_scope") {
            if !crate::server::core::plugin_host::errors::credential_scope_allowed(scope.as_str()?) { return None; }
        }
        return Some(RecordedReply::PluginError { status: status.map(|status| status as u16) });
    }
    let report = object(envelope.get("result")?)?;
    let outcome = match report.get("outcome")?.as_str()? {
        "claimed" => proto::CheckinOutcome::Claimed,
        "done" => proto::CheckinOutcome::Done,
        "ineligible" => proto::CheckinOutcome::Ineligible,
        "inactive" => proto::CheckinOutcome::Inactive,
        "captcha" => proto::CheckinOutcome::Captcha,
        "failed" => proto::CheckinOutcome::Failed,
        _ => return None,
    };
    if !optional_text(report, "credit", 4096) || !optional_text(report, "message", 16 * 1024) { return None; }
    let streak = optional_u32(report, "streak")?;
    let daily = match present(report, "details") {
        Some(details) => match present(object(details)?, "dailyCheckin") {
            Some(daily) => Some(object(daily)?),
            None => None,
        },
        None => None,
    };
    let (completed, sources_complete, query_only, confirming) = match daily {
        Some(daily) => {
            let completed = optional_bool(daily, "completed")?;
            let sources_complete = optional_bool(daily, "sourcesComplete")?;
            let query_only = optional_bool(daily, "queryOnly")?;
            let confirming = match present(daily, "status") {
                None => None,
                Some(status) => {
                    let status = status.as_str()?;
                    if status == "confirming" {
                        if !matches!(outcome, proto::CheckinOutcome::Inactive | proto::CheckinOutcome::Failed)
                            || completed == Some(true) { return None; }
                        Some(true)
                    } else {
                        if Some(status) != report.get("outcome").and_then(Value::as_str) { return None; }
                        Some(false)
                    }
                }
            };
            // completed=false在旧confirming输出中仅是展示值；保留报告但不据此判定远端未发生。
            let reported_done = matches!(outcome, proto::CheckinOutcome::Claimed | proto::CheckinOutcome::Done);
            if completed.is_some_and(|completed| completed != reported_done)
                || reported_done && sources_complete == Some(false)
                || outcome == proto::CheckinOutcome::Claimed && query_only == Some(true) { return None; }
            (completed, sources_complete, query_only, confirming)
        }
        None => (None, None, None, None),
    };
    Some(RecordedReply::Report { outcome, completed, sources_complete, query_only, confirming, streak })
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object().filter(|fields| fields.len() <= MAX_FIELDS)
}

fn present<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    fields.get(name).filter(|value| !value.is_null())
}

fn optional_bool(fields: &Map<String, Value>, name: &str) -> Option<Option<bool>> {
    match present(fields, name) {
        Some(value) => value.as_bool().map(Some),
        None => Some(None),
    }
}

fn optional_u32(fields: &Map<String, Value>, name: &str) -> Option<Option<u32>> {
    match present(fields, name) {
        Some(value) => u32::try_from(value.as_u64()?).ok().map(Some),
        None => Some(None),
    }
}

fn optional_i64(fields: &Map<String, Value>, name: &str) -> Option<Option<i64>> {
    match present(fields, name) {
        Some(value) => value.as_i64().map(Some),
        None => Some(None),
    }
}

fn optional_text(fields: &Map<String, Value>, name: &str, max: usize) -> bool {
    present(fields, name).is_none_or(|value| value.as_str().is_some_and(|text| text.len() <= max))
}
