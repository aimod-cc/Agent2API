//! Magpie claudebridge/mcp.go 的 stdio MCP 辅助入口，编译在同一个主程序内。
//! 只实现 initialize/tools/list/tools/call；后者经一次性回环回调等待客户端结果。
//! 不执行工具、不启动 shell、不经模型代理回调、不把 token 放进命令行。
use axum::{
    extract::{DefaultBodyLimit, State},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::NamedTempFile;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;

use super::{
    errors, process,
    tools::{IncomingCall, Tool, ToolResult},
};
use crate::server::errors::GatewayError;

#[derive(Clone)]
struct CallbackState {
    tx: mpsc::Sender<IncomingCall>,
    calls: Arc<Semaphore>,
    cancel: CancellationToken,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackInput {
    tool_call_id: String,
    name: String,
    arguments: Value,
}

pub(super) struct CallbackServer {
    pub file: NamedTempFile,
    pub calls: mpsc::Receiver<IncomingCall>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl CallbackServer {
    pub async fn start(dir: &std::path::Path) -> Result<Self, GatewayError> {
        let token = process::token()?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| errors::local("claude_mcp_listen", "无法创建 Claude 本机工具回调端口"))?;
        let address = listener
            .local_addr()
            .map_err(|_| errors::local("claude_mcp_listen", "无法读取 Claude 本机工具回调端口"))?;
        let path = format!("/claude-mcp/{token}");
        let file = process::private_file(dir, format!("http://127.0.0.1:{}{path}", address.port()).as_bytes())?;
        let (tx, calls) = mpsc::channel(256);
        let cancel = CancellationToken::new();
        let router = Router::new()
            .route(&path, post(callback))
            .with_state(CallbackState { tx, calls: Arc::new(Semaphore::new(256)), cancel: cancel.clone() })
            .layer(DefaultBodyLimit::max(process::MAX_LINE));
        let shutdown = cancel.clone();
        let task = crate::spawn_task(async move {
            let _ =
                axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await;
        });
        Ok(Self { file, calls, cancel, task })
    }
}
impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

async fn callback(
    State(state): State<CallbackState>,
    headers: axum::http::HeaderMap,
    Json(input): Json<CallbackInput>,
) -> Response {
    // 浏览器不能跨站借用回调；不存在任何通用网关鉴权豁免，监听仅限回环。
    if headers.contains_key("origin")
        || input.tool_call_id.is_empty()
        || input.tool_call_id.len() > 256
        || input.name.len() > 512
        || !input.arguments.is_object()
    {
        return GatewayError::bad_request("无效的 Claude 工具回调").into_response();
    }
    let _permit = match state.calls.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return GatewayError::with_status(503, "Claude 工具回调数量超过上限").into_response(),
    };
    let (tx, rx) = oneshot::channel();
    let call = IncomingCall { id: input.tool_call_id, name: input.name, args: input.arguments, response: tx };
    let send = tokio::select! { biased;
        _ = state.cancel.cancelled() => return GatewayError::with_status(410, "Claude 会话已结束").into_response(),
        result = state.tx.send(call) => result,
    };
    if send.is_err() {
        return GatewayError::with_status(410, "Claude 会话已结束").into_response();
    }
    tokio::select! { biased;
        _ = state.cancel.cancelled() => GatewayError::with_status(410, "Claude 会话已结束").into_response(),
        answer = tokio::time::timeout(Duration::from_secs(310), rx) => match answer {
            Ok(Ok(Ok(result))) => Json(result).into_response(),
            Ok(Ok(Err(error))) => error.into_response(),
            _ => GatewayError::with_status(410, "Claude 工具结果等待已结束").into_response(),
        }
    }
}

fn helper_write(output: &Mutex<std::io::Stdout>, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| "MCP response encoding failed".to_string())?;
    bytes.push(b'\n');
    let mut writer = output.lock().map_err(|_| "MCP output unavailable".to_string())?;
    writer.write_all(&bytes).and_then(|_| writer.flush()).map_err(|_| "MCP output closed".to_string())
}

fn read_helper_file(path: &str, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let path = std::path::Path::new(path);
    if !path.is_absolute() {
        return Err("MCP configuration path must be absolute".into());
    }
    let mut file = std::fs::File::open(path).map_err(|_| "MCP configuration unavailable")?;
    if !file.metadata().is_ok_and(|m| m.is_file() && m.len() <= limit) {
        return Err("MCP configuration is invalid".into());
    }
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes).map_err(|_| "MCP configuration unreadable")?;
    if bytes.len() > limit as usize {
        return Err("MCP configuration too large".into());
    }
    Ok(bytes)
}

pub(super) async fn run_helper(args: Vec<String>) -> Result<(), String> {
    if args.len() != 2 {
        return Err("Claude MCP helper expects callback and tools file paths".into());
    }
    let callback = String::from_utf8(read_helper_file(&args[0], 2048)?).map_err(|_| "MCP callback is invalid")?;
    let url = url::Url::parse(callback.trim()).map_err(|_| "MCP callback is invalid")?;
    let token = url.path().strip_prefix("/claude-mcp/").unwrap_or("");
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || token.len() != 48
        || !token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("MCP callback must be the gateway's one-time loopback address".into());
    }
    let tools: Vec<Tool> = serde_json::from_slice(&read_helper_file(&args[1], 4 * 1024 * 1024)?)
        .map_err(|_| "MCP tool definitions are invalid")?;
    let tools = Arc::new(tools);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(315))
        .build()
        .map_err(|_| "MCP callback client unavailable")?;
    let output = Arc::new(Mutex::new(std::io::stdout()));
    let (tx, mut rx) = mpsc::channel(256);
    let reader = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        loop {
            let mut line = Vec::new();
            loop {
                let buffer = input.fill_buf().map_err(|_| "MCP input failed")?;
                if buffer.is_empty() {
                    return if line.is_empty() { Ok(()) } else { Err("MCP input truncated".into()) };
                }
                let end = buffer.iter().position(|b| *b == b'\n');
                let size = end.map_or(buffer.len(), |i| i + 1);
                if line.len().saturating_add(size) > process::MAX_LINE {
                    return Err("MCP input exceeds limit".into());
                }
                line.extend_from_slice(&buffer[..size]);
                input.consume(size);
                if end.is_some() {
                    break;
                }
            }
            if tx.blocking_send(line).is_err() {
                return Ok(());
            }
        }
    });
    let cancel = CancellationToken::new();
    let limit = Arc::new(Semaphore::new(256));
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = rx.recv().await {
        let value: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                helper_write(
                    &output,
                    &json!({"jsonrpc":"2.0","id":Value::Null,"error":{"code":-32700,"message":"Parse error"}}),
                )?;
                continue;
            }
        };
        let Some(id) = value.get("id").filter(|id| !id.is_null()).cloned() else {
            continue;
        };
        if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            continue;
        }
        let permit = match limit.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                helper_write(
                    &output,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Too many pending tools"}}),
                )?;
                continue;
            }
        };
        let (output, client, url, tools, cancel) =
            (output.clone(), client.clone(), url.clone(), tools.clone(), cancel.clone());
        tasks.spawn(async move {
            let _permit = permit;
            let method = value.get("method").and_then(Value::as_str).unwrap_or("");
            let result: Result<Value, String> = match method {
                "initialize" => {
                    let requested = value.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18");
                    let version = if matches!(requested, "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25") { requested } else { "2025-06-18" };
                    Ok(json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"agent2api-claude","version":"1"}}))
                },
                "ping" => Ok(json!({})),
                "tools/list" => Ok(json!({"tools":tools.as_ref()})),
                "tools/call" => {
                    let name = value.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
                    let args = value.pointer("/params/arguments").cloned().unwrap_or_else(|| json!({}));
                    if !args.is_object() || !tools.iter().any(|tool| tool.name == name) { Err("Unknown tool or invalid arguments".into()) }
                    else {
                        let call_id = value.get("params").and_then(|p| p.get("_meta")).and_then(|meta| meta.get("claudecode/toolUseId"))
                            .and_then(Value::as_str).filter(|id| !id.is_empty())
                            .map(str::to_string).or_else(|| process::token().ok().map(|token| format!("call_{token}")));
                        if let Some(call_id) = call_id {
                            let call = async {
                                let mut response = client.post(url).json(&json!({"tool_call_id":call_id,"name":name,"arguments":args})).send().await.map_err(|_| "Tool callback failed")?;
                                if !response.status().is_success() { return Err("Tool callback expired or was refused"); }
                                let mut bytes = Vec::new();
                                while let Some(chunk) = response.chunk().await.map_err(|_| "Tool callback interrupted")? {
                                    if bytes.len().saturating_add(chunk.len()) > process::MAX_LINE { return Err("Tool result exceeds limit"); }
                                    bytes.extend_from_slice(&chunk);
                                }
                                let result: ToolResult = serde_json::from_slice(&bytes).map_err(|_| "Tool callback response invalid")?;
                                Ok(json!({"content":result.content,"isError":result.is_error}))
                            };
                            tokio::select! { biased;
                                _ = cancel.cancelled() => Err("Claude run ended".into()),
                                answer = call => answer.map_err(str::to_string),
                            }
                        } else { Err("Cannot allocate tool call id".into()) }
                    }
                },
                _ => Err("Method not found".into()),
            };
            let response = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(message) => json!({"jsonrpc":"2.0","id":id,"error":{"code":if method == "tools/call" {-32000} else {-32601},"message":message}}),
            };
            let _ = helper_write(&output, &response);
        });
        while tasks.try_join_next().is_some() {}
    }
    cancel.cancel();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    reader.await.map_err(|_| "MCP input task failed".to_string())?
}
