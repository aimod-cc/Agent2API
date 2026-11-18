//! 持久 CLI actor：每个 HTTP 响应是一段，工具调用挂起在原 CLI/MCP 中。
//! 下一请求的结果只解除对应 pending；取消优先，任何错误都杀整棵子进程树。
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

use super::{
    accounts::{self, Link},
    errors,
    events::{Boundary, Decoder, Reply},
    mcp::CallbackServer,
    process,
    sessions::{self, Command, Handle, Output},
    tools::{self, IncomingCall, ToolResult, ToolSet},
    usage,
};
use crate::server::core::upstream::{cancellation, usage::RequestTelemetry};
use crate::server::core::{account_store::AccountStore, proxies::ResolvedProxy};
use crate::server::errors::GatewayError;

const RUN_TIMEOUT: Duration = Duration::from_secs(600);
const CALLBACK_GRACE: Duration = Duration::from_secs(20);

struct Pending {
    name: String,
    args: Value,
    reply: oneshot::Sender<Result<ToolResult, GatewayError>>,
}
struct EarlyResult {
    call: tools::Call,
    result: ToolResult,
}
struct Active {
    command: Command,
    deadline: Instant,
    tool_deadline: Option<Instant>,
}

pub(super) fn launch(
    handle: Arc<Handle>,
    commands: mpsc::Receiver<Command>,
    store: AccountStore,
    account: String,
    link: Link,
    proxy: Option<ResolvedProxy>,
) {
    crate::spawn_task(async move {
        actor(handle.clone(), commands, store, account, link, proxy).await;
        sessions::remove(&handle.id);
    });
}

async fn actor(
    handle: Arc<Handle>,
    mut commands: mpsc::Receiver<Command>,
    store: AccountStore,
    account: String,
    link: Link,
    proxy: Option<ResolvedProxy>,
) {
    let first =
        tokio::select! { biased; _ = handle.cancel.cancelled() => return, command = commands.recv() => command };
    let Some(first) = first else {
        return;
    };
    let first_output = first.output.clone();
    let startup = start(&handle, &link, proxy.as_ref(), &first);
    let prepared = tokio::select! { biased;
        _ = handle.cancel.cancelled() => return,
        _ = wait_cancel(&first.telemetry) => { let _ = first_output.send(Output::Failed(cancellation::cancelled_error())).await; return; },
        result = tokio::time::timeout(Duration::from_secs(60), startup) => result
            .map_err(|_| errors::local("claude_start_timeout", "Claude Code 启动超时，已回收子进程"))
            .and_then(|r| r),
    };
    let (mut run, mut callback, _temporary, _tools_file, _mcp_file) = match prepared {
        Ok(result) => result,
        Err(error) => {
            let _ = first_output.send(Output::Failed(error)).await;
            return;
        }
    };
    drop(first_output);
    let identity = first.request.identity.clone();
    let mut tools = first.request.tools.clone();
    let mut decoder = Decoder::new(tools.clone());
    let mut active = Some(Active { command: first, deadline: Instant::now() + RUN_TIMEOUT, tool_deadline: None });
    let mut pending: HashMap<String, Pending> = HashMap::new();
    // Magpie early：CLI 可能等第一个结果回来才发第二个 MCP 调用，不能等齐回调再下发。
    let mut early: HashMap<String, EarlyResult> = HashMap::new();
    let mut parked_reply: Option<Reply> = None;
    let mut last_result = false;
    let mut idle_since = Instant::now();
    let mut last_line_at = Instant::now();
    let mut last_account_check = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let failure = loop {
        let next = tokio::select! { biased;
            _ = handle.cancel.cancelled() => break Some(errors::session("Claude 请求已取消，子进程已终止")),
            _ = async { if let Some(a) = &active { wait_cancel(&a.command.telemetry).await; } else { std::future::pending::<()>().await; } } => break Some(cancellation::cancelled_error()),
            _ = async { if let Some(a) = &active { a.command.output.closed().await; } else { std::future::pending::<()>().await; } } => break Some(errors::session("客户端已断开，Claude 子进程已终止")),
            _ = tick.tick() => ActorInput::Tick,
            command = commands.recv(), if active.is_none() => ActorInput::Command(command),
            call = callback.calls.recv() => ActorInput::Call(call),
            line = run.output.next() => ActorInput::Line(line),
        };
        let outcome: Result<(), GatewayError> = match next {
            ActorInput::Tick => {
                if last_account_check.elapsed() >= Duration::from_secs(5) {
                    if let Err(error) = link.check_profile() {
                        break Some(error);
                    }
                    if store
                        .route_account(&account)
                        .is_none_or(|row| row.get("enabled").and_then(Value::as_bool) == Some(false))
                    {
                        break Some(errors::session("Claude 账号已删除或停用，已结束该会话"));
                    }
                    last_account_check = Instant::now();
                }
                if active.is_some() && last_line_at.elapsed() >= Duration::from_millis(300) {
                    let update = match decoder.flush_whole() {
                        Ok(update) => update,
                        Err(error) => break Some(error),
                    };
                    if let Some(a) = &active {
                        a.command.telemetry.report_usage(&decoder.reply.usage);
                        for delta in update.deltas {
                            if send(&handle, &a.command, Output::Delta(delta)).await.is_err() {
                                handle.cancel.cancel();
                                break;
                            }
                        }
                    }
                    if matches!(update.boundary, Some(Boundary::Tools)) {
                        if !early.is_empty() {
                            break Some(errors::protocol("Claude 尚未取完上轮工具结果，未继续下一轮"));
                        }
                        if let Some(a) = &mut active {
                            a.tool_deadline = Some(Instant::now() + CALLBACK_GRACE);
                        }
                    }
                }
                if let Some(a) = &active {
                    if Instant::now() >= a.deadline {
                        break Some(errors::local("claude_request_timeout", "Claude 请求超时，已终止子进程"));
                    }
                    if a.tool_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        break Some(errors::protocol("Claude MCP 未接到有效工具调用；未返回无法回灌的工具结果"));
                    }
                } else if idle_since.elapsed()
                    >= if parked_reply.is_some() { sessions::TOOL_TTL } else { sessions::IDLE_TTL }
                {
                    break None;
                }
                if active.as_ref().is_some_and(|a| a.tool_deadline.is_some())
                    && callbacks_ready(&decoder.reply, &pending)
                {
                    if let Err(error) = run.check_alive() {
                        break Some(error);
                    }
                    finish(&handle, &mut active, &decoder.reply).await.map(|_| {
                        parked_reply = Some(decoder.reply.clone());
                        idle_since = Instant::now();
                    })
                } else {
                    Ok(())
                }
            }
            ActorInput::Command(None) => break None,
            ActorInput::Command(Some(command)) => {
                let initial = tokio::select! { biased;
                    _ = handle.cancel.cancelled() => Err(errors::session("Claude 请求已取消")),
                    _ = wait_cancel(&command.telemetry) => Err(cancellation::cancelled_error()),
                    result = async {
                    link.check_profile()?;
                    if command.request.identity != identity { return Err(errors::session("Claude 会话运行参数已变化，未复用旧进程")); }
                    tools = command.request.tools.clone();
                    if let Some(reply) = parked_reply.take() {
                        let deliver = command.results.iter().map(|(id, result)| (id.clone(), result.clone())).collect::<HashMap<_, _>>();
                        if deliver.len() != reply.calls.len() || reply.calls.iter().any(|call| !deliver.contains_key(&call.id)) {
                            return Err(errors::session("Claude 工具结果与挂起轮次不一致"));
                        }
                        if !command.alongside.is_empty() {
                            run.send(&json!({"type":"user", "message":{"role":"user","content":command.alongside}})).await?;
                        }
                        if !early.is_empty() { return Err(errors::session("Claude 上一轮尚有未取走的工具结果")); }
                        // 先校验整批，再解除等待；尚未发出的 MCP 调用保留结果，后到即答。
                        let deliveries = pending_delivery_keys(&reply, &pending)
                            .ok_or_else(|| errors::session("Claude 工具回调已失效或存在歧义"))?;
                        decoder.begin(tools.clone());
                        for call in reply.calls {
                            let result = deliver.get(&call.id).cloned().ok_or_else(|| errors::session("Claude 工具结果缺失"))?;
                            if let Some(key) = deliveries.get(&call.id) {
                                let wait = pending.remove(key).ok_or_else(|| errors::session("Claude 工具回调已失效"))?;
                                wait.reply.send(Ok(result)).map_err(|_| errors::session("Claude 已停止等待工具结果"))?;
                            } else {
                                early.insert(call.id.clone(), EarlyResult { call, result });
                            }
                        }
                    } else {
                        if !last_result { return Err(errors::session("Claude 上一轮尚未完整结束")); }
                        decoder.begin(tools.clone());
                        run.send(&json!({"type":"user","message":{"role":"user","content":command.prompt}})).await?;
                    }
                    last_result = false;
                    Ok(())
                    } => result,
                };
                active = Some(Active { command, deadline: Instant::now() + RUN_TIMEOUT, tool_deadline: None });
                initial
            }
            ActorInput::Call(None) => Err(errors::protocol("Claude 工具回调服务已结束")),
            ActorInput::Call(Some(call)) => {
                accept_call(call, &tools, &mut pending, &mut early, decoder.expected_calls())
            }
            ActorInput::Line(Err(error)) => Err(error),
            ActorInput::Line(Ok(None)) => Err(errors::protocol("Claude Code 在会话结束前退出，回复可能被截断")),
            ActorInput::Line(Ok(Some(line))) => {
                last_line_at = Instant::now();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let value: Value = match serde_json::from_slice(&line) {
                    Ok(value) => value,
                    Err(_) => break Some(errors::protocol("Claude 输出了无效的流式 JSON")),
                };
                if value.get("type").and_then(Value::as_str) == Some("control_request") {
                    let name = value.pointer("/request/tool_name").and_then(Value::as_str).unwrap_or("");
                    let subtype = value.pointer("/request/subtype").and_then(Value::as_str).unwrap_or("");
                    let allowed = subtype == "can_use_tool" && name.starts_with(tools::PREFIX) && tools.can_call(name);
                    let response = if allowed {
                        json!({"behavior":"allow","updatedInput":value.pointer("/request/input").cloned().unwrap_or_else(|| json!({}))})
                    } else {
                        json!({"behavior":"deny","message":"This gateway permits only its declared client MCP tools."})
                    };
                    let response = json!({"type":"control_response","response":{"subtype":"success",
                        "request_id":value.get("request_id"),"response":response}});
                    tokio::select! { biased;
                        _ = handle.cancel.cancelled() => Err(errors::session("Claude 请求已取消")),
                        _ = async { if let Some(a) = &active { wait_cancel(&a.command.telemetry).await; } else { std::future::pending::<()>().await; } } => Err(cancellation::cancelled_error()),
                        result = tokio::time::timeout(Duration::from_secs(10), run.send(&response)) =>
                            result.map_err(|_| errors::protocol("Claude 权限答复写入超时")).and_then(|r| r),
                    }
                } else if value.get("type").and_then(Value::as_str) == Some("system")
                    && value.get("subtype").and_then(Value::as_str) == Some("init")
                {
                    validate_init(&value, &tools)
                } else if active.is_none() && decoder.is_duplicate_whole(&value) {
                    Ok(())
                } else if active.is_none() && value.get("type").and_then(Value::as_str) == Some("result") {
                    Err(errors::protocol("Claude 等待工具结果时意外结束"))
                } else {
                    match decoder.event(&value) {
                        Err(error) => Err(error),
                        Ok(update) => {
                            if let Some(limits) = update.limits {
                                usage::note(&store, &account, &limits);
                            }
                            if let Some(a) = &active {
                                if !decoder.reply.usage.is_null() {
                                    a.command.telemetry.report_usage(&decoder.reply.usage);
                                }
                                if value.get("type").and_then(Value::as_str) == Some("stream_event") {
                                    if let Some(model) = value.pointer("/event/message/model").and_then(Value::as_str) {
                                        a.command.telemetry.note_upstream_model(model);
                                    }
                                }
                                let mut failed = None;
                                for delta in update.deltas {
                                    if let Err(error) = send(&handle, &a.command, Output::Delta(delta)).await {
                                        failed = Some(error);
                                        break;
                                    }
                                }
                                if let Some(error) = failed {
                                    break Some(error);
                                }
                            } else if !update.deltas.is_empty() {
                                break Some(errors::protocol("Claude 在没有客户端请求时继续输出内容"));
                            }
                            match update.boundary {
                                Some(Boundary::Tools) => {
                                    if !early.is_empty() {
                                        break Some(errors::protocol(
                                            "Claude 新一轮调用前仍有未取走的工具结果，未丢弃结果继续",
                                        ));
                                    }
                                    if let Some(a) = &mut active {
                                        a.tool_deadline = Some(Instant::now() + CALLBACK_GRACE);
                                    }
                                    if callbacks_ready(&decoder.reply, &pending) {
                                        if let Err(error) = run.check_alive() {
                                            break Some(error);
                                        }
                                        finish(&handle, &mut active, &decoder.reply).await.map(|_| {
                                            parked_reply = Some(decoder.reply.clone());
                                            idle_since = Instant::now();
                                        })
                                    } else {
                                        Ok(())
                                    }
                                }
                                Some(Boundary::Complete) => {
                                    if !pending.is_empty() || !early.is_empty() {
                                        Err(errors::protocol("Claude 结束回复时仍有未完成的工具结果往返"))
                                    } else {
                                        if let Err(error) = run.check_alive() {
                                            break Some(error);
                                        }
                                        last_result = true;
                                        idle_since = Instant::now();
                                        finish(&handle, &mut active, &decoder.reply).await
                                    }
                                }
                                None => Ok(()),
                            }
                        }
                    }
                }
            }
        };
        if let Err(error) = outcome {
            break Some(error);
        }
    };
    let stopped = run.stop().await;
    let failure = failure.or_else(|| stopped.err());
    if let Some(error) = failure {
        if let Some(active) = active {
            active.command.telemetry.note_error(&error.message);
            let _ =
                tokio::time::timeout(Duration::from_secs(2), active.command.output.send(Output::Failed(error))).await;
        }
    }
    for (_, wait) in pending {
        let _ = wait.reply.send(Err(errors::session("Claude 会话已结束")));
    }
    // Windows 不允许删除仍有打开文件的目录；先收回回调/配置句柄，再删目录。
    drop(callback);
    drop(_tools_file);
    drop(_mcp_file);
    drop(run);
    drop(_temporary);
}

enum ActorInput {
    Tick,
    Command(Option<Command>),
    Call(Option<IncomingCall>),
    Line(Result<Option<Vec<u8>>, GatewayError>),
}

async fn start(
    handle: &Handle,
    link: &Link,
    proxy: Option<&ResolvedProxy>,
    first: &Command,
) -> Result<
    (process::Running, CallbackServer, tempfile::TempDir, tempfile::NamedTempFile, tempfile::NamedTempFile),
    GatewayError,
> {
    accounts::verify(link, proxy).await?;
    let temp = process::private_dir()?;
    let callback = CallbackServer::start(temp.path()).await?;
    let tools_json =
        serde_json::to_vec(&first.request.tools.offered).map_err(|_| errors::protocol("Claude 工具定义无法序列化"))?;
    if tools_json.len() > 4 * 1024 * 1024 {
        return Err(GatewayError::bad_request("Claude 工具声明超过 4MiB 限制"));
    }
    let tools_file = process::private_file(temp.path(), &tools_json)?;
    let exe =
        std::env::current_exe().map_err(|_| errors::local("claude_mcp_binary", "无法定位网关自身的 MCP 助手入口"))?;
    let config = json!({"mcpServers": {(tools::SERVER): {"command":exe.to_string_lossy(),
        "args":[super::MCP_HELPER_ARGUMENT, callback.file.path().to_string_lossy(),tools_file.path().to_string_lossy()]}}});
    let mcp_file = process::private_file(temp.path(), config.to_string().as_bytes())?;
    let mut args = vec![
        "-p".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--allowedTools".into(),
        format!("mcp__{}", tools::SERVER),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];
    args.extend(process::safe_args(Some(mcp_file.path()), true));
    if first.request.model != "default" {
        args.extend(["--model".into(), first.request.model.clone()]);
    }
    if let Some(effort) = &first.request.effort {
        args.extend(["--effort".into(), effort.clone(), "--thinking-display".into(), "summarized".into()]);
    }
    if first.request.thinking_off {
        args.extend(["--thinking".into(), "disabled".into()]);
    }
    let mut run = process::spawn(link, &args, temp.path(), proxy)?;
    let request = process::token()?;
    run.send(&json!({"type":"control_request","request_id":request,"request":{"subtype":"initialize"}})).await?;
    loop {
        let line = tokio::select! { biased;
            _ = handle.cancel.cancelled() => return Err(errors::session("Claude 请求已取消")),
            line = run.output.next() => line?,
        }
        .ok_or_else(|| errors::protocol("Claude Code 初始化前退出"))?;
        let value: Value = serde_json::from_slice(&line).map_err(|_| errors::protocol("Claude 初始化输出无效"))?;
        if value.get("type").and_then(Value::as_str) == Some("system")
            && value.get("subtype").and_then(Value::as_str) == Some("init")
        {
            validate_init(&value, &first.request.tools)?;
        }
        if value.get("type").and_then(Value::as_str) == Some("control_response")
            && value.pointer("/response/request_id").and_then(Value::as_str) == Some(request.as_str())
        {
            if value.pointer("/response/subtype").and_then(Value::as_str) != Some("success") {
                return Err(errors::protocol("Claude Code 拒绝初始化桥接，请更新官方 CLI"));
            }
            break;
        }
        if value.get("type").and_then(Value::as_str) == Some("result") {
            return Err(errors::upstream(&value));
        }
    }
    run.send(&json!({"type":"user","message":{"role":"user","content":first.prompt}})).await?;
    Ok((run, callback, temp, tools_file, mcp_file))
}

fn validate_init(value: &Value, tools: &ToolSet) -> Result<(), GatewayError> {
    if value.get("plugins").and_then(Value::as_array).is_some_and(|plugins| !plugins.is_empty()) {
        return Err(errors::protocol("Claude 载入了额外插件，已终止此桥接"));
    }
    if value.get("tools").and_then(Value::as_array).is_some_and(|rows| {
        rows.iter().any(|tool| {
            tool.as_str().is_none_or(|name| !name.starts_with(tools::PREFIX) || tools.original(name).is_none())
        })
    }) {
        return Err(errors::protocol("Claude 载入了非本轮客户端工具，已终止此桥接"));
    }
    if value.get("mcp_servers").and_then(Value::as_array).is_some_and(|rows| {
        rows.iter().any(|server| {
            server.get("name").and_then(Value::as_str) != Some(tools::SERVER)
                || server.get("status").and_then(Value::as_str) != Some("connected")
        })
    }) {
        return Err(errors::protocol("Claude MCP 未连接或载入了额外服务，已终止此桥接"));
    }
    Ok(())
}

/// 当前已抵达的回调必须逐个对应本轮声明；允许其余调用稍后由 CLI 串行发出。
/// 缺工具 id 的旧 CLI 仍只在名称/参数唯一时配对，绝不猜两次相同调用的归属。
fn pending_delivery_keys(reply: &Reply, pending: &HashMap<String, Pending>) -> Option<HashMap<String, String>> {
    if reply.calls.is_empty() || pending.is_empty() || pending.len() > reply.calls.len() {
        return None;
    }
    let mut deliveries = HashMap::new();
    for (key, wait) in pending {
        if wait.reply.is_closed() {
            return None;
        }
        let exact = reply.calls.iter().find(|call| call.id == *key);
        let call = if let Some(call) = exact {
            if call.name != wait.name || call.args != wait.args {
                return None;
            }
            call
        } else {
            let mut matching = reply.calls.iter().filter(|call| call.name == wait.name && call.args == wait.args);
            let call = matching.next()?;
            if matching.next().is_some() {
                return None;
            }
            call
        };
        if deliveries.insert(call.id.clone(), key.clone()).is_some() {
            return None;
        }
    }
    Some(deliveries)
}

fn callbacks_ready(reply: &Reply, pending: &HashMap<String, Pending>) -> bool {
    pending_delivery_keys(reply, pending).is_some()
}

fn accept_call(
    call: IncomingCall,
    tools: &ToolSet,
    pending: &mut HashMap<String, Pending>,
    early: &mut HashMap<String, EarlyResult>,
    expected: &[tools::Call],
) -> Result<(), GatewayError> {
    tools::validate_call(&call.name, &call.args, tools)?;
    let name = tools.original(&call.name).unwrap_or("").to_string();
    // 工具结果属于先前已允许的那一轮；当前轮的 tool_choice 不得改写其成功/失败结果。
    let early_key = if let Some(saved) = early.get(&call.id) {
        if saved.call.name != name || saved.call.args != call.args {
            return Err(errors::protocol("Claude 延迟工具回调与已保存结果不匹配"));
        }
        Some(call.id.clone())
    } else {
        let mut matching = early.iter().filter(|(_, saved)| saved.call.name == name && saved.call.args == call.args);
        let first = matching.next().map(|(id, _)| id.clone());
        if matching.next().is_some() {
            return Err(errors::protocol("Claude 延迟工具调用缺少可唯一关联的 id"));
        }
        first
    };
    if let Some(id) = early_key {
        let saved = early.remove(&id).ok_or_else(|| errors::session("Claude 延迟工具结果已失效"))?;
        return call.response.send(Ok(saved.result)).map_err(|_| errors::session("Claude 已停止等待工具结果"));
    }
    if !tools.allowed.contains(&name) {
        let _ = call.response.send(Ok(ToolResult::denied()));
        return Ok(());
    }
    if pending.len() >= 256 || pending.contains_key(&call.id) {
        return Err(errors::protocol("Claude MCP 重复调用或挂起数量超限"));
    }
    if expected
        .iter()
        .find(|c| c.id == call.id)
        .is_some_and(|c| c.name != name || c.args.is_object() && c.args != call.args)
    {
        return Err(errors::protocol("Claude MCP 参数与向客户端声明的工具不一致"));
    }
    pending.insert(call.id, Pending { name, args: call.args, reply: call.response });
    Ok(())
}

async fn send(handle: &Handle, command: &Command, output: Output) -> Result<(), GatewayError> {
    tokio::select! { biased;
        _ = handle.cancel.cancelled() => Err(errors::session("Claude 请求已取消")),
        _ = wait_cancel(&command.telemetry) => Err(cancellation::cancelled_error()),
        result = tokio::time::timeout(Duration::from_secs(30), command.output.send(output)) => {
            result.map_err(|_| errors::session("客户端未及时读取 Claude 输出，已终止请求"))?
                .map_err(|_| errors::session("客户端已断开"))
        }
    }
}
async fn finish(handle: &Handle, active: &mut Option<Active>, reply: &Reply) -> Result<(), GatewayError> {
    let Some(current) = active.as_ref() else {
        return Err(errors::protocol("Claude 重复结束回复"));
    };
    sessions::park(handle, &current.command.request, reply)?;
    send(handle, &current.command, Output::Done(reply.clone())).await?;
    *active = None;
    Ok(())
}
async fn wait_cancel(telemetry: &RequestTelemetry) {
    match telemetry.cancel_token() {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}
