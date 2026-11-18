//! 私有真实原读取回执的严格daily选择；不接受自由任务参数、旧台账或status=3替代每日false。
//! 原生根group与task.scene可不同；分别保留，回查只认原scene/item，无缺失换组补查。

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::{credentials::Credentials, network::Route, DailyError};

const MAX_TASKS: usize = 512;

pub(super) struct Daily { pub(super) scene: String, pub(super) item: String, pub(super) checked: bool }

pub(super) fn launch_group(value: &Value, credentials: &Credentials) -> Result<String, DailyError> {
    successful(value, false, true)?;
    let data = value.get("data").and_then(Value::as_object).filter(|data| !data.is_empty())
        .ok_or(DailyError::ResponseInvalid)?;
    let info = data.get("ug_campaign_info").ok_or(DailyError::ResponseInvalid)?;
    if info.is_null() { return Err(DailyError::NoActivity); }
    let info = info.as_object().ok_or(DailyError::ResponseInvalid)?;
    identifier(info.get("campaign_group_code").ok_or(DailyError::ResponseInvalid)?, credentials)
}

pub(super) fn unique_daily(value: &Value, root: &str, endpoint: Route, credentials: &Credentials) -> Result<Daily, DailyError> {
    successful(value, true, true)?;
    let data = value.get("data").filter(|data| data.is_object()).ok_or(DailyError::ResponseInvalid)?;
    if endpoint == Route::V3 {
        if let Some(code) = data.get("campaign_code").filter(|value| !value.is_null() && value.as_str() != Some("")) {
            if identifier(code, credentials)? != root { return Err(DailyError::ResponseInvalid); }
        }
    }
    let mut scan = Scan { tasks: 0, daily: None, keys: BTreeSet::new(), recognized: false, credentials };
    scan.walk(data, root, 0)?;
    if !scan.recognized { return Err(DailyError::ResponseInvalid); }
    scan.daily.ok_or(DailyError::NoDaily)
}

struct Scan<'a> {
    tasks: usize,
    daily: Option<Daily>,
    keys: BTreeSet<(String, String)>,
    recognized: bool,
    credentials: &'a Credentials,
}

impl Scan<'_> {
    fn walk(&mut self, value: &Value, inherited: &str, depth: usize) -> Result<(), DailyError> {
        if depth > 8 { return Err(DailyError::ResponseInvalid); }
        let fields = value.as_object().ok_or(DailyError::ResponseInvalid)?;
        successful(value, true, false)?;
        let group = group(fields, inherited, self.credentials)?;
        for key in ["tasks", "campaigns", "milestones"] {
            let Some(rows) = fields.get(key) else { continue; };
            self.recognized = true;
            let rows = rows.as_array().filter(|rows| rows.len() <= MAX_TASKS).ok_or(DailyError::ResponseInvalid)?;
            for row in rows {
                self.tasks += 1;
                if self.tasks > MAX_TASKS { return Err(DailyError::ResponseInvalid); }
                let row_fields = row.as_object().ok_or(DailyError::ResponseInvalid)?;
                successful(row, true, false)?;
                let item = identifier(row_fields.get("campaign_code").ok_or(DailyError::ResponseInvalid)?, self.credentials)?;
                let scene = match row_fields.get("campaign_group_code") {
                    Some(value) if !value.is_null() => identifier(value, self.credentials)?,
                    _ => group.clone(),
                };
                // 同tuple重复即拒绝，不能先去重挑一份true/false或发布残缺选择。
                if !self.keys.insert((scene.clone(), item.clone())) { return Err(DailyError::AmbiguousDaily); }
                if let Some(status) = row_fields.get("task_status").filter(|value| !value.is_null()) {
                    unsigned(status)?; // 仅校验字段格式，绝不从它推每日完成状态。
                }
                let Some(daily) = row_fields.get("daily_checkin").filter(|value| !value.is_null()) else {
                    if row["task_kind"] == "daily_signin" { return Err(DailyError::ResponseInvalid); }
                    continue;
                };
                successful(daily, true, false)?;
                let daily = daily.as_object().ok_or(DailyError::ResponseInvalid)?;
                let checked = daily.get("checked_in_today").and_then(Value::as_bool).ok_or(DailyError::ResponseInvalid)?;
                if self.daily.is_some() { return Err(DailyError::AmbiguousDaily); }
                self.daily = Some(Daily { scene, item, checked });
            }
        }
        if let Some(child) = fields.get("campaign_group") { self.walk(child, &group, depth + 1)?; }
        if let Some(children) = fields.get("campaign_groups") {
            let children = children.as_array().filter(|children| children.len() <= MAX_TASKS).ok_or(DailyError::ResponseInvalid)?;
            if children.is_empty() { self.recognized = true; }
            for child in children { self.walk(child, &group, depth + 1)?; }
        }
        Ok(())
    }
}

fn group(fields: &Map<String, Value>, inherited: &str, credentials: &Credentials) -> Result<String, DailyError> {
    let selected = fields.get("campaign_group_code").or_else(|| fields.get("campaign_code"));
    match selected.filter(|value| !value.is_null() && value.as_str() != Some("")) {
        Some(value) => identifier(value, credentials),
        None => Ok(inherited.to_string()),
    }
}

fn identifier(value: &Value, credentials: &Credentials) -> Result<String, DailyError> {
    let text = value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|value| value.to_string()))
        .filter(|text| !text.is_empty() && text.len() <= 256 && text.trim() == text
            && !text.chars().any(|character| character.is_control() || character.is_whitespace())
            && !text.contains("://") && !credentials.protected(text)).ok_or(DailyError::ResponseInvalid)?;
    Ok(text)
}

fn unsigned(value: &Value) -> Result<u64, DailyError> {
    value.as_u64().or_else(|| value.as_str()?.trim().parse().ok()).ok_or(DailyError::ResponseInvalid)
}

/// 根launch/detail必须有明确已知成功码：launch仅0；detail的code可0/200、error_code仅0。
/// 多码按各自语义逐项检查，任一失败不能被另一成功码覆盖；success=true或HTTP2xx不能替代。
/// 嵌套task/daily可不带独立码，但若显式存在则仍严格检查；每日完成只由真实布尔给出。
pub(in crate::server::core::plugin_host::wasm) fn successful(value: &Value, ug: bool, require_code: bool) -> Result<(), DailyError> {
    let fields = value.as_object().ok_or(DailyError::ResponseInvalid)?;
    let mut confirmed_code = false;
    for key in ["code", "error_code"] {
        if let Some(raw) = fields.get(key) {
            let code = unsigned(raw)?;
            if code != 0 && !(ug && key == "code" && code == 200) { return Err(DailyError::ResponseRejected); }
            confirmed_code = true;
        }
    }
    if require_code && !confirmed_code { return Err(DailyError::ResponseInvalid); }
    for key in ["success", "failed"] {
        if let Some(flag) = fields.get(key).filter(|value| !value.is_null()) {
            let flag = flag.as_bool().ok_or(DailyError::ResponseInvalid)?;
            if key == "success" && !flag || key == "failed" && flag { return Err(DailyError::ResponseRejected); }
        }
    }
    if fields.get("error").is_some_and(|value| !value.is_null() && value != &Value::Bool(false)) {
        return Err(DailyError::ResponseRejected);
    }
    for key in ["biz_error", "error_detail"] {
        if let Some(error) = fields.get(key).filter(|value| !value.is_null()) {
            let error = error.as_object().ok_or(DailyError::ResponseInvalid)?;
            let mut has_code = false;
            for key in ["code", "error_code"] {
                if let Some(code) = error.get(key) {
                    if unsigned(code)? != 0 { return Err(DailyError::ResponseRejected); }
                    has_code = true;
                }
            }
            if !has_code { return Err(DailyError::ResponseInvalid); }
        }
    }
    let decision = fields.get("extra").and_then(|value| value.get("decision")).or_else(|| fields.get("decision"));
    if decision.is_some_and(|value| !value.is_null() && value != &Value::Bool(false)) {
        // 不保留/解析挑战来兜造完成；此固定状态查询出现任何验证决策只返回未知。
        return Err(DailyError::ResponseRejected);
    }
    Ok(())
}
