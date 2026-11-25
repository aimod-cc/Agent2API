use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    And,
    #[default]
    Or,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    #[default]
    Skip,
    Disable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    #[default]
    Models,
    Account,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Remaining,
    RemainingPercent,
    Used,
    UsedPercent,
    Tokens,
    Requests,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Lt,
    Lte,
    Gte,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reset {
    #[default]
    Fixed,
    Rolling,
    Daily,
    Weekly,
    Monthly,
    Total,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    #[serde(default)]
    pub reset: Reset,
    #[serde(default)]
    pub seconds: i64,
    #[serde(default)]
    pub starts_at: i64,
    #[serde(default)]
    pub ends_at: i64,
    #[serde(default)]
    pub utc_offset_minutes: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    #[serde(default)]
    pub join: Join,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub source: Option<QuotaSource>,
    #[serde(default)]
    pub models: Vec<String>,
    pub metric: Metric,
    pub comparison: Comparison,
    pub threshold: f64,
    #[serde(default)]
    pub window: Option<Window>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub action: Action,
    #[serde(default)]
    pub target: Target,
    pub conditions: Vec<Condition>,
}

fn yes() -> bool { true }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSource {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub plan_name: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub remaining: Option<f64>,
    #[serde(default)]
    pub total: Option<f64>,
    #[serde(default)]
    pub used: Option<f64>,
    #[serde(default)]
    pub remaining_percent: Option<f64>,
    #[serde(default)]
    pub starts_at: i64,
    #[serde(default)]
    pub ends_at: i64,
    #[serde(default)]
    pub resets_at: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_amount: Option<f64>,
    #[serde(default)]
    pub window: Option<Window>,
    #[serde(default)]
    pub unlimited: bool,
    #[serde(default)]
    pub local_estimate: bool,
    #[serde(default)]
    pub local_only: bool,
}

fn is_zero_i64(value: &i64) -> bool { *value == 0 }

impl QuotaSource {
    pub fn identity(&self) -> Self {
        let mut source = self.clone();
        source.remaining = None;
        source.total = None;
        source.used = None;
        source.remaining_percent = None;
        source.resets_at = 0;
        source.expires_at = 0;
        source.expires_amount = None;
        source
    }

    pub fn live_at(&self, now: i64) -> bool {
        (self.starts_at <= 0 || now >= self.starts_at)
            && (self.ends_at <= 0 || now < self.ends_at)
            && (self.resets_at <= 0 || now < self.resets_at)
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub known: bool,
    pub value: Option<f64>,
    pub hit: bool,
    pub local: bool,
    pub models: Vec<String>,
    pub model_values: std::collections::BTreeMap<String, f64>,
    pub window_start: i64,
    pub resets_at: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupState {
    pub hit: bool,
    pub account: bool,
    pub models: Vec<String>,
    pub conditions: Vec<Reading>,
}

pub fn json_of<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!([]))
}
