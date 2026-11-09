use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::{client::CodexClient, state::LiveThread, string};

fn terminal(status: &str) -> bool { matches!(status, "completed" | "interrupted" | "failed") }

fn text_parts(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n\n"),
        _ => String::new(),
    }
}

fn related_text(left: &str, right: &str) -> bool {
    left == right || !left.is_empty() && !right.is_empty() && (left.starts_with(right) || right.starts_with(left))
}

fn same_item(saved: &Value, live: &Value) -> bool {
    if string(saved, "type") != string(live, "type") { return false; }
    if !string(saved, "id").is_empty() && string(saved, "id") == string(live, "id") { return true; }
    match string(saved, "type") {
        "userMessage" => {
            let left = saved.get("clientId").and_then(Value::as_str);
            let right = live.get("clientId").and_then(Value::as_str);
            if left.is_some() && right.is_some() { left == right } else { saved.get("content") == live.get("content") }
        }
        "agentMessage" | "plan" => {
            let same_phase = saved.get("phase").is_none_or(Value::is_null) || live.get("phase").is_none_or(Value::is_null)
                || saved.get("phase") == live.get("phase");
            same_phase && related_text(string(saved, "text"), string(live, "text"))
        }
        "reasoning" => {
            let left = text_parts(saved.get("summary"));
            let right = text_parts(live.get("summary"));
            if !left.is_empty() || !right.is_empty() { related_text(&left, &right) }
            else { related_text(&text_parts(saved.get("content")), &text_parts(live.get("content"))) }
        }
        "commandExecution" => !string(saved, "command").is_empty() && saved.get("command") == live.get("command"),
        "fileChange" => saved.get("changes") == live.get("changes"),
        "mcpToolCall" | "dynamicToolCall" => saved.get("tool") == live.get("tool") && saved.get("server") == live.get("server") && saved.get("arguments") == live.get("arguments"),
        "webSearch" => saved.get("action") == live.get("action") && saved.get("query") == live.get("query"),
        "contextCompaction" => true,
        "error" => saved.get("message") == live.get("message"),
        _ => false,
    }
}

fn merged_item(saved: &Value, live: &Value, prefer_live: bool) -> Value {
    let mut item = if prefer_live { live.clone() } else { saved.clone() };
    let other = if prefer_live { saved } else { live };
    if let Some(fields) = other.as_object() {
        for (key, value) in fields {
            if item.get(key).is_none_or(Value::is_null) { item[key] = value.clone(); }
        }
    }
    for key in ["text", "aggregatedOutput"] {
        let own = string(&item, key);
        let other_text = string(other, key);
        if other_text.len() > own.len() && other_text.starts_with(own) { item[key] = json!(other_text); }
    }
    if string(&item, "type") == "reasoning" {
        for key in ["summary", "content"] {
            let Some(other_parts) = other.get(key).and_then(Value::as_array) else { continue; };
            if !item[key].is_array() { item[key] = json!([]); }
            let parts = item[key].as_array_mut().unwrap();
            for (index, part) in other_parts.iter().enumerate() {
                if index >= parts.len() { parts.push(part.clone()); }
                else if let (Some(own), Some(other)) = (parts[index].as_str(), part.as_str()) {
                    if other.len() > own.len() && other.starts_with(own) { parts[index] = part.clone(); }
                }
            }
        }
    }
    item
}

fn merge_turn(saved: &[Value], live_items: &[(&str, &Value)], live: &LiveThread, revision: u64, status: &str) -> Vec<Value> {
    // Legacy history rebuilds item IDs. Reconcile ordered occurrences inside one turn, never across turns.
    let authoritative = terminal(status) && !saved.is_empty()
        && live_items.iter().all(|(key, _)| live.item_versions.get(*key).is_none_or(|v| *v <= revision));
    let mut result = Vec::new();
    let mut cursor = 0;
    let mut pending = Vec::new();
    for (key, item) in live_items {
        let position = saved.iter().enumerate().skip(cursor).find(|(_, row)| same_item(row, item)).map(|(index, _)| index);
        if let Some(index) = position {
            result.extend(saved[cursor..index].iter().cloned());
            result.append(&mut pending);
            let newer = live.item_versions.get(*key).is_some_and(|v| *v > revision);
            result.push(merged_item(&saved[index], item, !authoritative && (newer || status == "inProgress")));
            cursor = index + 1;
        } else if !authoritative || matches!(string(item, "type"), "planUpdate" | "error") {
            pending.push((*item).clone());
        }
    }
    result.extend(saved[cursor..].iter().cloned());
    result.append(&mut pending);
    result
}

pub fn snapshot(client: &CodexClient, thread: &Value, before_revision: u64) -> Value {
    let mut saved = HashMap::<String, Vec<Value>>::new();
    let mut statuses = HashMap::<String, String>::new();
    let mut order = Vec::new();
    let mut forkable = HashSet::new();
    if let Some(turns) = thread.get("turns").and_then(Value::as_array) {
        for turn in turns {
            let id = string(turn, "id").to_string();
            if id.is_empty() { continue; }
            if !saved.contains_key(&id) { order.push(id.clone()); }
            let status = string(turn, "status");
            statuses.insert(id.clone(), status.to_string());
            if terminal(status) { forkable.insert(id.clone()); }
            let mut rows = turn.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(error) = turn.get("error").filter(|v| !v.is_null()) {
                rows.push(json!({ "id": format!("error:{id}"), "type": "error", "message": error.get("message") }));
            }
            saved.insert(id, rows);
        }
    }
    let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    let live = state.threads.get(string(thread, "id"));
    let mut live_turns = HashMap::<String, Vec<(&str, &Value)>>::new();
    if let Some(live) = live {
        for key in &live.item_order {
            let Some(item) = live.items.get(key) else { continue; };
            let turn = string(item, "turnId").to_string();
            if turn.is_empty() { continue; }
            if !saved.contains_key(&turn) && !live_turns.contains_key(&turn) { order.push(turn.clone()); }
            statuses.entry(turn.clone()).or_insert_with(|| if live.completed_turns.contains(&turn) { "completed" } else { "inProgress" }.into());
            live_turns.entry(turn).or_default().push((key, item));
        }
    }
    let mut items = Vec::new();
    let mut turns = Vec::new();
    for turn in order {
        let empty = Vec::new();
        let rows = saved.get(&turn).unwrap_or(&empty);
        let saved_status = statuses.get(&turn).map(String::as_str).unwrap_or("inProgress");
        let status = if live.is_some_and(|s| s.completed_turns.contains(&turn)) && saved_status == "inProgress" {
            "completed"
        } else { saved_status };
        let rows = match (live, live_turns.get(&turn)) {
            (Some(live), Some(live_items)) => merge_turn(rows, live_items, live, before_revision, saved_status),
            _ => rows.clone(),
        };
        let mut occurrences = HashMap::<String, usize>::new();
        for mut item in rows {
            let kind = string(&item, "type").to_string();
            let index = occurrences.entry(kind.clone()).or_default();
            item["nativeId"] = json!(string(&item, "id"));
            item["id"] = json!(format!("{turn}:{kind}:{index}"));
            item["turnId"] = json!(turn);
            item["turnStatus"] = json!(status);
            if terminal(status) { item["streaming"] = json!(false); }
            *index += 1;
            items.push(item);
        }
        turns.push(json!({ "id": turn, "status": status, "canFork": forkable.contains(&turn) }));
    }
    json!({ "items": items, "turns": turns })
}
