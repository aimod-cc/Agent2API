//! 官方详情仅生成固定任务候选；不从展示列表、名称、status缺值或用户参数猜领奖资格。

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{super::checkin_call::doubao_daily::credentials::Credentials, unavailable};

pub(super) const V3_GROUP: &str = "pc_incentive_task_mvp_v3";
pub(super) const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const MAX_TASKS: usize = 512;

pub(super) struct Task {
    pub(super) group: String,
    pub(super) item: String,
    pub(super) task: String,
    pub(super) body: Option<Value>,
    pub(super) status: Option<u64>,
    pub(super) title: String,
    pub(super) amount: Option<f64>,
}
impl Task {
    pub(super) fn completed(&self) -> bool { self.status == Some(3) }
    pub(super) fn campaign_id(&self) -> String { campaign_id(&self.group, &self.item, &self.task) }
}

pub(super) fn campaign_id(group: &str, item: &str, task: &str) -> String {
    format!("doubao-task-{:x}", Sha256::digest(json!([group, item, task]).to_string().as_bytes()))
}

pub(super) fn launch(value: &Value, credentials: &Credentials) -> Result<Option<String>, proto::WireError> {
    success(value, false)?;
    let data = value.get("data").filter(|value| value.is_object()).ok_or_else(unavailable)?;
    let info = data.get("ug_campaign_info").ok_or_else(unavailable)?;
    if info.is_null() { return Ok(None); }
    Ok(Some(identifier(&info["campaign_group_code"], credentials)?))
}

pub(super) fn tasks(value: &Value, root: &str, credentials: &Credentials) -> Result<Vec<Task>, proto::WireError> {
    success(value, true)?;
    let data = value.get("data").filter(|value| value.is_object()).ok_or_else(unavailable)?;
    if root == V3_GROUP && data.get("campaign_code").is_some_and(|value| !value.is_null() && value.as_str() != Some("") && value.as_str() != Some(root)) {
        return Err(unavailable());
    }
    let mut scan = Scan { credentials, count: 0, recognized: false, keys: std::collections::BTreeSet::new(), result: Vec::new() };
    scan.walk(data, root, 0)?;
    if !scan.recognized { return Err(unavailable()); }
    Ok(scan.result)
}
struct Scan<'a> {
    credentials: &'a Credentials,
    count: usize,
    recognized: bool,
    keys: std::collections::BTreeSet<(String, String, String)>,
    result: Vec<Task>,
}
impl Scan<'_> {
    fn walk(&mut self, value: &Value, inherited: &str, depth: usize) -> Result<(), proto::WireError> {
        if depth > 8 || !value.is_object() { return Err(unavailable()); }
        let group = match value.get("campaign_group_code").or_else(|| value.get("campaign_code")).filter(|value| !value.is_null() && value.as_str() != Some("")) {
            Some(value) => identifier(value, self.credentials)?, None => inherited.to_string(),
        };
        for key in ["tasks", "campaigns", "milestones"] {
            let Some(rows) = value.get(key) else { continue; };
            self.recognized = true;
            let rows = rows.as_array().filter(|rows| rows.len() <= MAX_TASKS).ok_or_else(unavailable)?;
            for row in rows {
                self.count += 1;
                if self.count > MAX_TASKS || !row.is_object() { return Err(unavailable()); }
                super::super::checkin_call::doubao_daily::parse::successful(row, true, false).map_err(|_| unavailable())?;
                let item = identifier(&row["campaign_code"], self.credentials)?;
                let group = match row.get("campaign_group_code").filter(|value| !value.is_null()) {
                    Some(value) => identifier(value, self.credentials)?, None => group.clone(),
                };
                let task = row.get("task_id").filter(|value| !value.is_null() && value.as_str() != Some(""))
                    .map(|value| identifier(value, self.credentials)).transpose()?.unwrap_or_default();
                if !self.keys.insert((group.clone(), item.clone(), task.clone())) { return Err(unavailable()); }
                if row.get("daily_checkin").is_some_and(|value| !value.is_null()) || row["task_kind"] == "daily_signin" { continue; }
                let status = row.get("task_status").filter(|value| !value.is_null()).map(unsigned).transpose()?;
                let mode = row.get("reward_mode").filter(|value| !value.is_null()).map(unsigned).transpose()?;
                let body = if status == Some(2) {
                    if mode == Some(2) || key == "milestones" {
                        Some(json!({"task_id": "", "campaign_group_code": group, "campaign_item_code": item}))
                    } else if !task.is_empty() { Some(json!({"task_id": task})) } else { None }
                } else { None };
                let title = row.get("title").or_else(|| row.get("name")).and_then(Value::as_str)
                    .filter(|text| text.len() <= 640 && !text.chars().any(char::is_control) && !self.credentials.protected(text))
                    .unwrap_or(&item).chars().take(160).collect();
                let amount = row.pointer("/reward_summaries/0/reward_amount")
                    .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
                    .filter(|amount: &f64| amount.is_finite() && *amount >= 0.0);
                self.result.push(Task { group, item, task, body, status, title, amount });
            }
        }
        if let Some(child) = value.get("campaign_group") { self.walk(child, &group, depth + 1)?; }
        if let Some(children) = value.get("campaign_groups") {
            let children = children.as_array().filter(|items| items.len() <= MAX_TASKS).ok_or_else(unavailable)?;
            self.recognized |= children.is_empty();
            for child in children { self.walk(child, &group, depth + 1)?; }
        }
        Ok(())
    }
}

pub(super) fn confirmed(value: &Value, root: &str, selected: &Task, credentials: &Credentials) -> Result<bool, proto::WireError> {
    Ok(tasks(value, root, credentials)?.into_iter()
        .find(|task| task.group == selected.group && task.item == selected.item && task.task == selected.task)
        .ok_or_else(unavailable)?.status == Some(3))
}

pub(super) fn success(value: &Value, ug: bool) -> Result<(), proto::WireError> {
    super::super::checkin_call::doubao_daily::parse::successful(value, ug, true).map_err(|_| unavailable())?;
    if let Some(data) = value.get("data") {
        super::super::checkin_call::doubao_daily::parse::successful(data, ug, false).map_err(|_| unavailable())?;
    }
    Ok(())
}
fn unsigned(value: &Value) -> Result<u64, proto::WireError> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok()).ok_or_else(unavailable)
}
fn identifier(value: &Value, credentials: &Credentials) -> Result<String, proto::WireError> {
    value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|value| value.to_string()))
        .filter(|text| !text.is_empty() && text.len() <= 256 && text.trim() == text
            && !text.chars().any(|ch| ch.is_control() || ch.is_whitespace()) && !text.contains("://") && !credentials.protected(text))
        .ok_or_else(unavailable)
}
