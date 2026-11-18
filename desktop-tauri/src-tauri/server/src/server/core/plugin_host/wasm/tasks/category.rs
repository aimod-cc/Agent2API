use std::collections::HashMap;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Definition, Host};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Checkin,
    Growth,
    Rewards,
    Maintenance,
    #[default]
    #[serde(other)]
    Other,
}

impl Category {
    pub(crate) fn belongs_to_checkin(self) -> bool {
        matches!(self, Self::Checkin | Self::Growth | Self::Rewards)
    }
}

pub(super) async fn load(host: &Host, provider: &str, definitions: &[Definition]) -> HashMap<String, Category> {
    let mut categories: HashMap<_, _> = definitions.iter().map(|task| {
        (task.id.clone(), legacy(provider, task))
    }).collect();
    if definitions.is_empty() { return categories; }
    let result = host.call_coded("tasks.categories", json!({ "provider": provider }), Duration::from_secs(10)).await;
    match result {
        Ok(value) => match parse(value, definitions) {
            Ok(declared) => categories.extend(declared),
            Err(message) => crate::server::logging::log_with_level("[PluginTask]", message, "warn"),
        },
        Err(error) if matches!(error.code.as_deref(), Some(proto::WireError::NO_METHOD) | Some(proto::WireError::UNSUPPORTED)) => {},
        Err(_) => crate::server::logging::log_with_level("[PluginTask]", "任务分类暂不可用，保留兼容分类与原有排程", "warn"),
    }
    categories
}

fn parse(value: Value, definitions: &[Definition]) -> Result<HashMap<String, Category>, &'static str> {
    let fields = value.as_object().filter(|fields| fields.len() <= definitions.len())
        .ok_or("任务分类必须是已声明任务的有限映射")?;
    fields.iter().map(|(id, value)| {
        if !definitions.iter().any(|task| &task.id == id) || !value.is_string() {
            return Err("任务分类引用了未声明的任务或无效类别");
        }
        let category = serde_json::from_value(value.clone()).map_err(|_| "任务分类格式无效")?;
        Ok((id.clone(), category))
    }).collect()
}

fn legacy(provider: &str, task: &Definition) -> Category {
    if task.checkin { return Category::Checkin; }
    // 只兼容已发布的旧任务身份，不按名称猜测业务或授予能力。
    match (provider, task.id.as_str()) {
        ("workbuddy", "travel" | "growth-daily") => Category::Growth,
        ("workbuddy", "growth-streak") => Category::Rewards,
        ("officeace", "model-probe") => Category::Maintenance,
        _ => Category::Other,
    }
}
