use std::sync::atomic::Ordering;

use serde_json::{json, Value};
use super::{client::CodexClient, now, storage, string};

pub fn remember(client: &CodexClient, thread: &Value, response: Option<&Value>, attached: Option<bool>) {
    let id = string(thread, "id").to_string();
    if id.is_empty() { return; }
    let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    state.revision += 1;
    let revision = state.revision;
    let live = state.threads.entry(id.clone()).or_default();
    let expected = response.and_then(|r| r.get("_agent2apiRevision")).and_then(Value::as_u64);
    let newer = expected.is_some_and(|r| live.activity_revision > r);
    let newer_settings = expected.is_some_and(|r| live.settings_revision > r);
    let previous_status = live.thread.get("status").cloned();
    live.thread = thread.clone();
    if newer { if let Some(status) = previous_status { live.thread["status"] = status; } }
    if let Some(attached) = attached {
        if !newer { live.attached = attached; live.ending = false; }
    }
    if newer_settings {
        if let Some(cwd) = live.settings.get("cwd") { live.thread["cwd"] = cwd.clone(); }
    } else if let Some(response) = response { apply_settings(live, response); live.settings_revision = revision; }
    if let Some(turns) = thread.get("turns").and_then(Value::as_array) {
        for turn in turns {
            if string(turn, "status") != "inProgress" { live.completed_turns.insert(string(turn, "id").into()); }
            if live.completed_turns.len() > 2000 { live.completed_turns.clear(); }
        }
        if live.attached && !newer {
            live.turn = turns.iter().rev().find(|turn| string(turn, "status") == "inProgress")
                .map(|turn| string(turn, "id").to_string());
        }
    }
    if live.thread.pointer("/status/type").and_then(Value::as_str) == Some("idle") && !newer { live.turn = None; }
    if response.is_none() { live.activity_revision = revision; }
    drop(state);
    client.state.changed(&id);
}

pub(super) fn apply_settings(live: &mut super::state::LiveThread, value: &Value) {
    if let Some(model) = value.get("model").and_then(Value::as_str) { live.model = Some(model.to_string()); }
    if live.settings.is_null() { live.settings = json!({}); }
    for key in ["model", "modelProvider", "cwd", "approvalPolicy", "approvalsReviewer", "sandboxPolicy", "activePermissionProfile", "effort", "reasoningEffort", "collaborationMode", "serviceTier", "summary", "personality"] {
        if let Some(value) = value.get(key) { live.settings[key] = value.clone(); }
    }
    if let Some(effort) = value.get("effort").or_else(|| value.get("reasoningEffort")) { live.settings["effort"] = effort.clone(); }
    if let Some(sandbox) = value.get("sandboxPolicy").or_else(|| value.get("sandbox")) {
        live.sandbox = sandbox.clone(); live.settings["sandboxPolicy"] = sandbox.clone();
        live.permission_mode = match string(sandbox, "type") {
            "readOnly" => "read-only", "workspaceWrite" => "default", "dangerFullAccess" => "full-access", _ => "unsupported",
        }.into();
    }
    if let Some(cwd) = value.get("cwd").and_then(Value::as_str) { live.thread["cwd"] = json!(cwd); }
}

pub fn notification(client: &CodexClient, message: Value) {
    let method = string(&message, "method");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    if method == "account/updated" || method == "account/rateLimits/updated" {
        if let Ok(mut cache) = client.usage_cache.try_lock() { *cache = None; }
        if method == "account/updated" {
            if let Ok(mut cache) = client.catalog_cache.try_lock() { *cache = None; }
        }
        return;
    }
    if method == "thread/started" {
        if let Some(thread) = params.get("thread") { remember(client, thread, None, Some(true)); }
        return;
    }
    let id = string(&params, "threadId").to_string();
    if id.is_empty() { return; }
    let mut consumed = None;
    {
        let mut state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.revision += 1;
        let revision = state.revision;
        if method == "serverRequest/resolved" {
            state.permissions.retain(|_, p| p.thread_id != id || Some(&p.native_id) != params.get("requestId"));
            drop(state);
            client.state.changed(&id);
            return;
        }
        if method == "turn/completed" || matches!(method, "thread/closed" | "thread/archived" | "thread/deleted") {
            let turn_id = params.pointer("/turn/id").and_then(Value::as_str);
            state.permissions.retain(|_, p| p.thread_id != id || (turn_id.is_some() && p.arguments.get("turnId").and_then(Value::as_str) != turn_id));
        }
        if method == "item/completed" {
            let item_id = params.pointer("/item/id").and_then(Value::as_str);
            if item_id.is_some() { state.permissions.retain(|_, p| p.thread_id != id || p.arguments.get("itemId").and_then(Value::as_str) != item_id); }
        }
        let live = state.threads.entry(id.clone()).or_default();
        if live.thread.is_null() { live.thread = json!({ "id": id, "cwd": "", "preview": "", "updatedAt": now() / 1000 }); }
        let turn_id = params.get("turnId").and_then(Value::as_str)
            .or_else(|| params.pointer("/turn/id").and_then(Value::as_str))
            .or(live.turn.as_deref()).unwrap_or("").to_string();
        match method {
            "thread/status/changed" => {
                let status = params.get("status").cloned().unwrap_or(Value::Null);
                if string(&status, "type") == "notLoaded" { live.attached = false; live.turn = None; }
                if string(&status, "type") == "idle" { live.turn = None; }
                live.thread["status"] = status;
            }
            "thread/closed" | "thread/archived" | "thread/deleted" => {
                live.attached = false; live.turn = None; live.thread["status"] = json!({ "type": "notLoaded" });
                if method != "thread/closed" { live.ending = true; live.items.clear(); live.item_order.clear(); live.item_versions.clear(); }
            }
            "thread/tokenUsage/updated" => {
                if let Some(usage) = params.get("tokenUsage").filter(|v| v.is_object()) {
                    live.token_usage = usage.clone(); live.usage_revision = revision;
                    live.usage_updated_at = Some(now()); live.usage_source = Some("live".into());
                }
            }
            "thread/settings/updated" => {
                if let Some(settings) = params.get("threadSettings") { apply_settings(live, settings); live.settings_revision = revision; }
            }
            "thread/name/updated" => live.thread["name"] = params.get("threadName").or_else(|| params.get("name")).cloned().unwrap_or(Value::Null),
            "turn/started" => {
                live.turn = params.pointer("/turn/id").and_then(Value::as_str).map(str::to_string);
                live.thread["status"] = json!({ "type": "active", "activeFlags": [] });
                live.attached = true;
            }
            "turn/completed" => {
                let turn = params.get("turn").unwrap_or(&Value::Null);
                let turn_id = string(turn, "id");
                live.completed_turns.insert(turn_id.to_string());
                if live.completed_turns.len() > 2000 { live.completed_turns.retain(|id| id == turn_id); }
                if live.turn.as_deref() == turn.get("id").and_then(Value::as_str) {
                    live.turn = None;
                    live.thread["status"] = json!({ "type": "idle" });
                }
                if let Some(error) = turn.get("error").filter(|e| !e.is_null()) {
                    put_item(live, turn_id, &format!("error:{}", string(turn, "id")), json!({ "type": "error", "id": format!("error:{}", string(turn, "id")), "message": error.get("message") }), revision);
                }
            }
            "item/started" | "item/completed" => {
                if let Some(item) = params.get("item") {
                    let item_id = string(item, "id");
                    if !item_id.is_empty() {
                        let mut projected = item.clone();
                        if string(item, "type") == "commandExecution" && item.get("aggregatedOutput").is_none_or(Value::is_null) {
                            if let Some(previous) = live.items.get(&item_key(&turn_id, item_id)) {
                                if let Some(output) = previous.get("aggregatedOutput") { projected["aggregatedOutput"] = output.clone(); }
                                if let Some(truncated) = previous.get("outputTruncated") { projected["outputTruncated"] = truncated.clone(); }
                            }
                        }
                        projected["streaming"] = json!(method == "item/started");
                        put_item(live, &turn_id, item_id, projected, revision);
                        if string(item, "type") == "userMessage" {
                            if let Some(local_id) = item.get("clientId").and_then(Value::as_str) { consumed = Some((local_id.to_string(), string(&params, "turnId").to_string())); }
                        }
                    }
                }
            }
            "item/agentMessage/delta" | "item/plan/delta" | "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" | "item/commandExecution/outputDelta" => {
                let item_id = string(&params, "itemId");
                let (kind, field) = match method {
                    "item/agentMessage/delta" => ("agentMessage", "text"),
                    "item/plan/delta" => ("plan", "text"),
                    "item/commandExecution/outputDelta" => ("commandExecution", "aggregatedOutput"),
                    "item/reasoning/summaryTextDelta" => ("reasoning", "summary"),
                    _ => ("reasoning", "content"),
                };
                if !item_id.is_empty() {
                    let mut item = live.items.get(&item_key(&turn_id, item_id)).cloned().unwrap_or_else(|| json!({ "id": item_id, "type": kind }));
                    item["streaming"] = json!(true);
                    let delta = string(&params, "delta");
                    if field == "summary" || field == "content" {
                        let index = params.get(if field == "summary" { "summaryIndex" } else { "contentIndex" }).and_then(Value::as_u64).unwrap_or(0).min(100) as usize;
                        if !item[field].is_array() { item[field] = json!([]); }
                        let parts = item[field].as_array_mut().unwrap();
                        while parts.len() <= index { parts.push(json!("")); }
                        parts[index] = json!(bounded_append(parts[index].as_str().unwrap_or(""), delta));
                    } else {
                        let before = string(&item, field);
                        if before.len().saturating_add(delta.len()) > 512 * 1024 { item["outputTruncated"] = json!(true); }
                        item[field] = json!(bounded_append(string(&item, field), delta));
                    }
                    put_item(live, &turn_id, item_id, item, revision);
                }
            }
            "item/fileChange/patchUpdated" | "item/mcpToolCall/progress" => {
                let item_id = string(&params, "itemId");
                if !item_id.is_empty() {
                    let mut item = live.items.get(&item_key(&turn_id, item_id)).cloned().unwrap_or_else(|| json!({
                        "id": item_id, "type": if method == "item/fileChange/patchUpdated" { "fileChange" } else { "mcpToolCall" },
                    }));
                    if method == "item/fileChange/patchUpdated" { item["changes"] = params.get("changes").cloned().unwrap_or(json!([])); }
                    else { item["progressMessage"] = params.get("message").cloned().unwrap_or(Value::Null); }
                    item["streaming"] = json!(true);
                    put_item(live, &turn_id, item_id, item, revision);
                }
            }
            "turn/plan/updated" => {
                let item_id = format!("plan:{}", string(&params, "turnId"));
                put_item(live, &turn_id, &item_id, json!({ "id": item_id, "type": "planUpdate", "plan": params.get("plan"), "explanation": params.get("explanation") }), revision);
            }
            "error" => {
                if params.get("willRetry").and_then(Value::as_bool) != Some(true) {
                    let item_id = format!("error:{}", string(&params, "turnId"));
                    put_item(live, &turn_id, &item_id, json!({ "id": item_id, "type": "error", "message": params.pointer("/error/message") }), revision);
                }
            }
            _ => return,
        }
        live.activity_revision = revision;
        live.thread["updatedAt"] = json!(now() / 1000);
    }
    if method == "thread/settings/updated" {
        let sandbox = client.state.inner.lock().unwrap_or_else(|e| e.into_inner()).threads.get(&id).map(|s| s.sandbox.clone());
        if let Some(sandbox) = sandbox.filter(|s| matches!(string(s, "type"), "readOnly" | "workspaceWrite")) {
            let _ = storage::save_sandbox(&client.db, &client.scope, &id, &sandbox);
        }
    }
    if let Some((local_id, turn)) = consumed {
        let _ = storage::settle(&client.db, &client.scope, &id, &local_id, "consumed", Some(&turn));
    }
    if client.running.load(Ordering::Acquire) { client.state.changed(&id); }
}

pub fn item_key(turn: &str, id: &str) -> String { format!("{turn}:{id}") }

fn put_item(live: &mut super::state::LiveThread, turn: &str, id: &str, mut item: Value, revision: u64) {
    if turn.is_empty() { return; }
    let key = item_key(turn, id);
    item["turnId"] = json!(turn);
    if !live.items.contains_key(&key) { live.item_order.push(key.clone()); }
    live.items.insert(key.clone(), item);
    live.item_versions.insert(key, revision);
    while live.item_order.len() > 2000 {
        let old = live.item_order.remove(0); live.items.remove(&old); live.item_versions.remove(&old);
    }
}

fn bounded_append(before: &str, delta: &str) -> String {
    const MAX: usize = 512 * 1024;
    let mut text = before.to_string();
    if text.len() < MAX {
        let mut end = delta.len().min(MAX - text.len());
        while !delta.is_char_boundary(end) { end -= 1; }
        text.push_str(&delta[..end]);
    }
    text
}

pub fn history(client: &CodexClient, thread: &Value, before_revision: u64) -> Value {
    super::history::snapshot(client, thread, before_revision)
}
