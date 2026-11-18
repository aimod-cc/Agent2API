use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use crate::server::core::{routing, usage_records::UsageRecord};

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok()).filter(|number| number.is_finite())
}

pub(super) fn summary<'a>(members: impl Iterator<Item = &'a Value>, records: &HashMap<String, UsageRecord>, filtered: bool, now: i64, offset: i32) -> Value {
    let mut total = 0;
    let mut counted = 0;
    let mut failed = 0;
    let mut pending = 0;
    let mut uncountable = 0;
    let mut partial = 0;
    let mut unlimited = 0;
    let mut sum = 0.0;
    let mut unit = String::new();
    let mut additive = None;
    let mut ratio: Option<Value> = None;
    let mut packs = Vec::new();
    for account in members {
        total += 1;
        if account["isGuest"] == true { pending += 1; continue; }
        let Some(record) = records.get(routing::account_id(account).unwrap_or("")) else { pending += 1; continue; };
        if record.error.is_some() {
            if matches!(record.code.as_deref(), Some("usage_not_configured" | "usage_unavailable")) { uncountable += 1; }
            else { failed += 1; }
            continue;
        }
        let Some(usage) = record.usage.as_ref() else { pending += 1; continue; };
        if usage["statisticsError"].as_str().is_some_and(|value| !value.is_empty())
            || usage["warnings"].as_array().is_some_and(|values| !values.is_empty()) { partial += 1; }
        if crate::server::core::account_order::supports_credit_expiry(routing::provider_of(account)) {
            packs.extend(expiring(usage, now));
        }
        if usage["unlimited"] == true { unlimited += 1; continue; }
        let value = number(usage.get("totalLeft").unwrap_or(&usage["available"]));
        let candidate = ratio_of(usage, value);
        let next_unit = usage["unit"].as_str().filter(|value| !value.trim().is_empty()).unwrap_or("积分").trim();
        let next_additive = next_unit != "%" && value.is_some();
        if value.is_none() && candidate.is_none() { uncountable += 1; continue; }
        if additive.is_some_and(|previous| previous != next_additive)
            || next_additive && !unit.is_empty() && next_unit != unit { uncountable += 1; continue; }
        additive = Some(next_additive);
        if unit.is_empty() { unit = next_unit.to_string(); }
        counted += 1;
        if next_additive { sum += value.unwrap_or(0.0); }
        else if candidate.as_ref().is_some_and(|candidate| ratio.as_ref().is_none_or(|previous| number(&candidate["percent"]) < number(&previous["percent"]))) {
            ratio = candidate;
        }
    }
    let tomorrow = (now + i64::from(offset) * 60_000).div_euclid(86_400_000) * 86_400_000
        + 86_400_000 - i64::from(offset) * 60_000;
    let today = packs.iter().filter(|pack| pack["expireAt"].as_i64().unwrap_or(0) < tomorrow).cloned().collect::<Vec<_>>();
    json!({ "counted": counted, "total": total, "sum": if counted > 0 && additive != Some(false) { Some((sum * 100.0).round() / 100.0) } else { None },
        "unlimited": unlimited, "unit": unit, "failed": failed, "uncountable": uncountable, "pending": pending, "partial": partial,
        "scope": if filtered { "filtered" } else { "group" }, "additive": additive != Some(false), "ratio": ratio,
        "expiring": { "count": packs.len(), "todayCount": today.len(), "amounts": amounts(&packs), "todayAmounts": amounts(&today) } })
}

fn ratio_of(usage: &Value, value: Option<f64>) -> Option<Value> {
    let rows = usage["windows"].as_array().into_iter().flatten().filter(|row| row["aside"] != true).collect::<Vec<_>>();
    let exhausted = rows.iter().filter(|row| matches!(row["window"].as_str(), Some("weekly" | "monthly"))
        && row["unlimited"] != true && (number(&row["remainingPercent"]).is_some_and(|value| value <= 0.0)
        || number(&row["remaining"]).is_some_and(|value| value <= 0.0)))
        .map(|row| row["group"].as_str().unwrap_or("")).collect::<HashSet<_>>();
    let mut adjusted = false;
    let windows = rows.iter().map(|row| {
        let short = matches!(row["window"].as_str(), Some("daily" | "5h"));
        let blocked = short && exhausted.contains(row["group"].as_str().unwrap_or(""));
        let percent = number(&row["remainingPercent"]).map(|value| value.clamp(0.0, 100.0));
        if blocked && (row["unlimited"] == true || percent != Some(0.0)) { adjusted = true; }
        let label = [row["group"].as_str().unwrap_or(""), row["window"].as_str().or(row["label"].as_str()).unwrap_or("")]
            .into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join(" · ");
        json!({ "label": label,
            "percent": if blocked { Some(0.0) } else { percent }, "unlimited": !blocked && row["unlimited"] == true, "shortWindow": short })
    }).collect::<Vec<_>>();
    let short = windows.iter().any(|row| row["shortWindow"] == true && (row["unlimited"] == true || number(&row["percent"]).is_some()));
    let percent = windows.iter().filter(|row| !short || row["shortWindow"] == true)
        .filter_map(|row| if row["unlimited"] == true { Some(100.0) } else { number(&row["percent"]) }).reduce(f64::min);
    if let Some(percent) = percent { return Some(json!({ "percent": percent, "hasShortWindow": short, "adjusted": adjusted, "windows": windows })); }
    value.map(|value| json!({ "percent": value.clamp(0.0, 100.0), "hasShortWindow": false, "adjusted": false,
        "windows": [{ "label": "", "percent": value.clamp(0.0, 100.0), "unlimited": false }] }))
}

fn expiring(usage: &Value, now: i64) -> Vec<Value> {
    let unit = usage["unit"].as_str().unwrap_or(if usage.get("totalLeft").is_some() { "积分" } else { "" });
    usage["credits"].as_array().into_iter().flatten().filter_map(|pack| {
        if ["snapshot", "record", "daily", "rolling", "unlimited"].iter().any(|key| pack[*key] == true) { return None; }
        let remain = number(&pack["remain"]);
        let expiry = number(&pack["expireAt"])? as i64;
        let total = number(&pack["total"]);
        let used = number(&pack["used"]);
        if remain.is_some_and(|value| value <= 0.0) || total == Some(-1.0)
            || remain.is_none() && (number(&pack["remainingPercent"]).is_some_and(|value| value <= 0.0)
                || total.zip(used).is_some_and(|(total, used)| total > 0.0 && used >= total))
            || number(&pack["startAt"]).is_some_and(|value| value > now as f64)
            || expiry <= now || expiry > now + 7 * 86_400_000 { return None; }
        Some(json!({ "expireAt": expiry, "remain": remain, "unit": pack["unit"].as_str().unwrap_or(unit) }))
    }).collect()
}

fn amounts(packs: &[Value]) -> Vec<Value> {
    let mut grouped = BTreeMap::<String, (usize, usize, Option<f64>)>::new();
    for pack in packs {
        let unit = pack["unit"].as_str().unwrap_or("");
        let unit = if matches!(unit.to_ascii_lowercase().as_str(), "credit" | "credits" | "积分") { "积分" } else { unit };
        let item = grouped.entry(unit.to_string()).or_default();
        item.0 += 1;
        let remain = number(&pack["remain"]);
        if remain.is_none() { item.1 += 1; }
        if !unit.is_empty() && unit != "%" { if let Some(remain) = remain { item.2 = Some(item.2.unwrap_or(0.0) + remain); } }
    }
    grouped.into_iter().map(|(unit, (count, unknown_count, sum))| json!({ "unit": unit, "count": count, "unknownCount": unknown_count, "sum": sum })).collect()
}
