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
    } else if let Some(response) = response { apply_settings(live, response); }
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

fn apply_settings(live: &mut super::state::LiveThread, value: &Value) {
    if let Some(model) = value.get("model").and_then(Value::as_str) { live.model = Some(model.to_string()); }
    if live.settings.is_null() { live.settings = json!({}); }
    for key in ["model", "modelProvider", "cwd", "approvalPolicy", "approvalsReviewer", "sandboxPolicy", "activePermissionProfile", "effort", "reasoningEffort", "collaborationMode", "serviceTier", "summary", "personality"] {
        if let Some(value) = value.get(key) { live.settings[key] = value.clone(); }
    }
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
                    put_item(live, &format!("error:{}", string(turn, "id")), json!({ "type": "error", "id": format!("error:{}", string(turn, "id")), "message": error.get("message") }), revision);
                }
            }
            "item/started" | "item/completed" => {
                if let Some(item) = params.get("item") {
                    let item_id = string(item, "id");
                    if !item_id.is_empty() {
                        put_item(live, item_id, item.clone(), revision);
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
                    let mut item = live.items.get(item_id).cloned().unwrap_or_else(|| json!({ "id": item_id, "type": kind }));
                    let delta = string(&params, "delta");
                    if field == "summary" || field == "content" {
                        let index = params.get(if field == "summary" { "summaryIndex" } else { "contentIndex" }).and_then(Value::as_u64).unwrap_or(0).min(100) as usize;
                        if !item[field].is_array() { item[field] = json!([]); }
                        let parts = item[field].as_array_mut().unwrap();
                        while parts.len() <= index { parts.push(json!("")); }
                        parts[index] = json!(bounded_append(parts[index].as_str().unwrap_or(""), delta));
                    } else { item[field] = json!(bounded_append(string(&item, field), delta)); }
                    put_item(live, item_id, item, revision);
                }
            }
            "turn/plan/updated" => {
                let item_id = format!("plan:{}", string(&params, "turnId"));
                put_item(live, &item_id, json!({ "id": item_id, "type": "planUpdate", "plan": params.get("plan"), "explanation": params.get("explanation") }), revision);
            }
            "error" => {
                if params.get("willRetry").and_then(Value::as_bool) != Some(true) {
                    let item_id = format!("error:{}", string(&params, "turnId"));
                    put_item(live, &item_id, json!({ "id": item_id, "type": "error", "message": params.pointer("/error/message") }), revision);
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

fn put_item(live: &mut super::state::LiveThread, id: &str, item: Value, revision: u64) {
    if !live.items.contains_key(id) { live.item_order.push(id.to_string()); }
    live.items.insert(id.to_string(), item);
    live.item_versions.insert(id.to_string(), revision);
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
    let id = string(thread, "id");
    let mut items = Vec::new();
    if let Some(turns) = thread.get("turns").and_then(Value::as_array) {
        for turn in turns {
            if let Some(rows) = turn.get("items").and_then(Value::as_array) { items.extend(rows.iter().cloned()); }
            if let Some(error) = turn.get("error").filter(|v| !v.is_null()) {
                items.push(json!({ "id": format!("error:{}", string(turn, "id")), "type": "error", "message": error.get("message") }));
            }
        }
    }
    let mut indices = std::collections::HashMap::new();
    for (index, item) in items.iter().enumerate() { indices.insert(string(item, "id").to_string(), index); }
    let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(live) = state.threads.get(id) {
        for item_id in &live.item_order {
            let Some(item) = live.items.get(item_id) else { continue; };
            if let Some(index) = indices.get(item_id) {
                if live.item_versions.get(item_id).is_some_and(|v| *v > before_revision)
                    || live.turn.is_some() || item.get("clientId").is_some_and(|v| !v.is_null()) {
                    items[*index] = item.clone();
                }
            } else { indices.insert(item_id.clone(), items.len()); items.push(item.clone()); }
        }
    }
    json!({ "items": items })
}
