use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::server::request_stats::LocalDirectRef;

const MAX_TRANSCRIPT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct PreviewError {
    pub status: u16,
    pub message: String,
}

impl PreviewError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: msg.into(),
        }
    }
}

fn resolve_transcript_path(reference: &LocalDirectRef) -> Option<PathBuf> {
    if !reference.transcript_path.trim().is_empty() {
        let p = PathBuf::from(&reference.transcript_path);
        if p.is_file() {
            return Some(p);
        }
    }
    if !reference.db_path.trim().is_empty() && !reference.conversation_id.trim().is_empty() {
        let db = Path::new(&reference.db_path);
        if let Some(root) = db.parent().and_then(Path::parent) {
            let logs = root
                .join("brain")
                .join(&reference.conversation_id)
                .join(".system_generated")
                .join("logs");
            for name in ["transcript_full.jsonl", "transcript.jsonl"] {
                let candidate = logs.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn extract_text(val: Option<&Value>) -> String {
    let Some(v) = val else {
        return String::new();
    };
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(arr) = v.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if let Some(text) = item.as_str() {
                parts.push(text.to_string());
            } else if let Some(text) = item.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            } else if let Some(text) = item.get("content").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        return parts.join("\n");
    }
    String::new()
}

fn normalize_tool_calls(raw: Option<&Value>) -> Option<Vec<Value>> {
    let arr = raw?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let id = item
            .get("id")
            .or_else(|| item.get("call_id"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("call_{idx}"));

        let (name, args_val) = if let Some(func) = item.get("function") {
            let name = func
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let args = func.get("arguments");
            (name, args)
        } else {
            let name = item
                .get("name")
                .or_else(|| item.get("tool_name"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let args = item
                .get("arguments")
                .or_else(|| item.get("args_json"))
                .or_else(|| item.get("input"));
            (name, args)
        };

        let arguments = match args_val {
            Some(Value::String(s)) => s.clone(),
            Some(other) if !other.is_null() => other.to_string(),
            _ => "{}".to_string(),
        };

        out.push(json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": arguments,
            }
        }));
    }
    (!out.is_empty()).then_some(out)
}

fn build_assistant_message(row: &Value) -> Value {
    let content = extract_text(row.get("content"));
    let reasoning = extract_text(row.get("thinking").or_else(|| row.get("reasoning")));
    let tool_calls = normalize_tool_calls(row.get("tool_calls"));
    let mut msg = json!({
        "role": "assistant",
        "content": content,
    });
    if let Some(obj) = msg.as_object_mut() {
        if !reasoning.trim().is_empty() {
            obj.insert("reasoning_content".to_string(), json!(reasoning));
        }
        if let Some(calls) = tool_calls {
            obj.insert("tool_calls".to_string(), Value::Array(calls));
        }
    }
    msg
}

/// 按需读取本机直连对话文件并转换为预览面板所需的 OpenAI 对话格式（不落库）。
pub fn build_live_preview(reference: &LocalDirectRef) -> Result<Value, PreviewError> {
    let path = resolve_transcript_path(reference).ok_or_else(|| {
        PreviewError::not_found("未找到本机对话原文文件（可能已被清理、移动或该版本未生成明文记录）")
    })?;
    let meta = std::fs::metadata(&path)
        .map_err(|_| PreviewError::not_found("无法访问本机对话原文文件"))?;
    if meta.len() > MAX_TRANSCRIPT_BYTES {
        return Err(PreviewError::not_found("本机对话原文超过 16 MB 预览保护上限"));
    }

    let file = File::open(&path).map_err(|_| PreviewError::not_found("读取本机对话原文失败"))?;
    let reader = BufReader::new(file);

    struct ParsedRow {
        step_index: Option<i64>,
        role_kind: &'static str,
        raw: Value,
    }

    let mut rows: Vec<ParsedRow> = Vec::new();
    for line in reader.lines() {
        let Ok(text) = line else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let step_index = raw.get("step_index").and_then(Value::as_i64);
        let kind = raw.get("type").and_then(Value::as_str).unwrap_or("");
        let source = raw.get("source").and_then(Value::as_str).unwrap_or("");
        let role_kind = if kind.contains("USER_INPUT") || source == "user" {
            "user"
        } else if kind.contains("PLANNER_RESPONSE") || source == "assistant" {
            "assistant"
        } else if kind.contains("TOOL_RESULT") || source == "tool" {
            "tool"
        } else {
            continue;
        };
        rows.push(ParsedRow {
            step_index,
            role_kind,
            raw,
        });
    }

    if rows.is_empty() {
        return Err(PreviewError::not_found("该对话文件中暂无可展示的问答记录"));
    }

    let target_step = reference.step_index;
    let has_step_numbers = rows.iter().any(|r| r.step_index.is_some());
    let mut selected_rows: Vec<&ParsedRow> = if has_step_numbers && target_step >= 0 {
        rows.iter()
            .filter(|r| r.step_index.unwrap_or(i64::MAX) <= target_step)
            .collect()
    } else {
        rows.iter().collect()
    };
    if selected_rows.is_empty() {
        selected_rows = rows.iter().collect();
    }

    // 找到目标步骤对应或最近的助手回复作为响应侧展示，其前面的轮次放入请求消息列表。
    let response_pos = selected_rows
        .iter()
        .rposition(|r| r.role_kind == "assistant");

    let mut req_messages: Vec<Value> = Vec::new();
    let mut response_msg: Option<Value> = None;

    for (idx, item) in selected_rows.iter().enumerate() {
        if Some(idx) == response_pos {
            response_msg = Some(build_assistant_message(&item.raw));
            continue;
        }
        match item.role_kind {
            "user" => {
                let text = extract_text(item.raw.get("content"));
                if !text.trim().is_empty() {
                    req_messages.push(json!({ "role": "user", "content": text }));
                }
            }
            "assistant" => {
                req_messages.push(build_assistant_message(&item.raw));
            }
            "tool" => {
                let text = extract_text(item.raw.get("content"));
                let tool_call_id = item
                    .raw
                    .get("tool_call_id")
                    .or_else(|| item.raw.get("call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mut msg = json!({ "role": "tool", "content": text });
                if !tool_call_id.is_empty() {
                    if let Some(obj) = msg.as_object_mut() {
                        obj.insert("tool_call_id".to_string(), json!(tool_call_id));
                    }
                }
                req_messages.push(msg);
            }
            _ => {}
        }
    }

    let req_json = json!({
        "messages": req_messages,
    });
    let resp_json = response_msg
        .map(|msg| json!({ "choices": [{ "index": 0, "message": msg }] }))
        .unwrap_or_else(|| json!({ "choices": [] }));

    Ok(json!({
        "id": reference.request_id,
        "requestBody": req_json.to_string(),
        "responseBody": resp_json.to_string(),
        "truncated": false,
        "liveLocal": true,
        "sourceFile": path.to_string_lossy().to_string(),
        "conversationId": reference.conversation_id,
        "conversationTitle": reference.conversation_title,
        "workspaceUri": reference.workspace_uri,
        "stepIndex": reference.step_index,
    }))
}
