use std::cmp::Ordering;
use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::server::config::BalanceSortDirection;
use super::usage_records;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSortBalance {
    pub remaining: Option<f64>,
    pub unit: String,
    pub unlimited: bool,
}

impl AccountSortBalance {
    pub fn from_usage(usage: &Value) -> Self {
        let (remaining, unlimited) = usage_records::extract_remaining(usage);
        let account_unit = usage.get("quotaSources").and_then(Value::as_array)
            .into_iter().flatten().find(|source| source.get("id").and_then(Value::as_str) == Some("account"))
            .and_then(|source| source.get("unit")).and_then(Value::as_str);
        let unit = usage.get("unit").and_then(Value::as_str).map(str::trim).filter(|unit| !unit.is_empty())
            .or_else(|| account_unit.map(str::trim).filter(|unit| !unit.is_empty()))
            .unwrap_or_else(|| if usage.get("totalLeft").is_some() { "积分" } else { "" });
        let unit = if unit.eq_ignore_ascii_case("credit") || unit.eq_ignore_ascii_case("credits") { "积分" } else { unit };
        Self { remaining, unlimited, unit: unit.to_string() }
    }

    pub fn comparable(&self) -> Option<f64> {
        if self.unlimited || self.unit.is_empty() || self.unit == "%" { return None; }
        self.remaining.filter(|value| value.is_finite())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BalanceSortState { Ready, NoData, MixedUnits, Unsupported }

pub fn state<'a>(supported: bool, balances: impl Iterator<Item = &'a AccountSortBalance>) -> BalanceSortState {
    if !supported { return BalanceSortState::Unsupported; }
    let units: BTreeSet<&str> = balances.filter(|balance| balance.comparable().is_some())
        .map(|balance| balance.unit.as_str()).collect();
    match units.len() {
        0 => BalanceSortState::NoData,
        1 => BalanceSortState::Ready,
        _ => BalanceSortState::MixedUnits,
    }
}

pub fn compare(left: Option<f64>, right: Option<f64>, direction: BalanceSortDirection) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            let order = left.partial_cmp(&right).unwrap_or(Ordering::Equal);
            match direction { BalanceSortDirection::High => order.reverse(), BalanceSortDirection::Low => order }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
