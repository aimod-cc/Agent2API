//! 把共享网关的请求/取消/计量接入 Claude actor；返回现有 Chat SSE 或完整 JSON。
//! Stream Drop 与非流请求被丢弃都终止该段所属 CLI，只有消费成功终帧才保留会话。
use axum::http::HeaderMap;
use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::mpsc;

use super::{
    accounts, errors,
    events::Reply,
    messages::Request,
    models, process, runtime,
    sessions::{self, Command, Lease, Output},
};
use crate::server::core::upstream::{cancellation, usage::RequestTelemetry, ForwardOutcome};
use crate::server::core::{account_store::AccountStore, protocol, proxies::ResolvedProxy};
use crate::server::{errors::GatewayError, logging};

pub(super) async fn forward(
    store: &AccountStore,
    account: &str,
    body: &Value,
    headers: &HeaderMap,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<ForwardOutcome, GatewayError> {
    telemetry.note_upstream_protocol("claude_cli");
    let link = accounts::load(store, account)?;
    link.check_profile()?;
    let mut request = Request::parse(body)?;
    models::check_model(&link, &request.model)?;
    // 出口也是进程的固定属性，代理更换后不得悄悄复用旧出口。
    let mut fingerprint = request.identity.clone();
    if let Some(proxy) = &proxy {
        fingerprint.push_str(&format!(
            "\0{}\0{}\0{:?}\0{}\0{}\0{}",
            proxy.protocol, proxy.host, proxy.port, proxy.username, proxy.password, proxy.non_direct
        ));
    }
    fingerprint.push_str(&link.executable.to_string_lossy());
    fingerprint.push_str(&link.profile_path.to_string_lossy());
    request.identity = process::digest(fingerprint.as_bytes());
    let owner = owner(headers, body);
    let selected = sessions::select(&owner, account, &request)?;
    let mut lease = Lease::new(selected.handle.clone());
    let preparation = async {
        let prompt = if !selected.waiting_for_tools {
            request.prompt(selected.start, proxy.as_ref()).await?
        } else {
            Vec::new()
        };
        let results = if selected.waiting_for_tools { request.results(proxy.as_ref()).await? } else { Vec::new() };
        let alongside = if results.is_empty() { Vec::new() } else { request.alongside_results(proxy.as_ref()).await? };
        Ok::<_, GatewayError>((prompt, results, alongside))
    };
    let prepared = tokio::select! { biased;
        _ = wait_cancel(telemetry) => return Err(cancellation::cancelled_error()),
        result = tokio::time::timeout(Duration::from_secs(120), preparation) => result
            .map_err(|_| errors::local("claude_input_timeout", "Claude 消息准备超时，未发送不完整输入"))??,
    };
    let (output, mut incoming) = mpsc::channel(128);
    let model = request.model.clone();
    let command = Command {
        request,
        prompt: prepared.0,
        results: prepared.1,
        alongside: prepared.2,
        output,
        telemetry: telemetry.clone(),
    };
    if let Some(receiver) = selected.new_receiver {
        runtime::launch(selected.handle.clone(), receiver, store.clone(), account.into(), link, proxy);
    }
    selected
        .handle
        .commands
        .try_send(command)
        .map_err(|_| errors::session("Claude 会话进程已结束或已有未接收的请求"))?;
    let first = tokio::select! { biased;
        _ = wait_cancel(telemetry) => return Err(cancellation::cancelled_error()),
        result = incoming.recv() => result.ok_or_else(|| errors::protocol("Claude 没有返回任何完整结果"))?,
    };
    let first = match first {
        Output::Failed(error) => return Err(error),
        other => other,
    };
    telemetry.note_first_frame();
    let id = format!("chatcmpl-{}", process::token()?);
    if stream {
        Ok(ForwardOutcome::Stream {
            status: 200,
            stream: Box::new(ClaudeStream {
                input: incoming,
                first: Some(first),
                lease,
                telemetry: telemetry.clone(),
                id,
                model,
                queued: VecDeque::new(),
                terminal: false,
                finished: false,
                success: false,
                role_sent: false,
            }),
        })
    } else {
        let reply = match first {
            Output::Done(reply) => reply,
            Output::Delta(_) => loop {
                let event = tokio::select! { biased;
                    _ = wait_cancel(telemetry) => return Err(cancellation::cancelled_error()),
                    result = incoming.recv() => result.ok_or_else(|| errors::protocol("Claude 回复在结束前中断"))?,
                };
                match event {
                    Output::Done(reply) => break reply,
                    Output::Failed(error) => return Err(error),
                    Output::Delta(_) => {}
                }
            },
            Output::Failed(error) => return Err(error),
        };
        telemetry.report_usage(&reply.usage);
        lease.complete();
        Ok(ForwardOutcome::Completion { body: completion(&id, &model, &reply) })
    }
}

fn completion(id: &str, model: &str, reply: &Reply) -> Value {
    json!({"id":id,"object":"chat.completion","created":logging::now_ms()/1000,"model":model,
        "choices":[{"index":0,"message":reply.message(),"finish_reason":reply.finish}],"usage":reply.usage})
}

fn owner(headers: &HeaderMap, body: &Value) -> String {
    let session = crate::server::core::upstream::plugin_session_id(headers, body);
    let mut identity = Vec::new();
    for name in ["authorization", "x-api-key", "x-goog-api-key"] {
        if let Some(value) = headers.get(name) {
            identity.extend_from_slice(value.as_bytes());
        }
        identity.push(0);
    }
    if !session.is_empty() {
        identity.extend_from_slice(session.as_bytes());
    } else if let Some(first) = body.get("messages").and_then(Value::as_array).and_then(|messages| {
        messages.iter().find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    }) {
        identity.extend_from_slice(
            super::messages::hashes(std::slice::from_ref(first)).first().map(String::as_bytes).unwrap_or(&[]),
        );
    }
    process::digest(&identity)
}

struct ClaudeStream {
    input: mpsc::Receiver<Output>,
    first: Option<Output>,
    lease: Lease,
    telemetry: Arc<RequestTelemetry>,
    id: String,
    model: String,
    queued: VecDeque<Bytes>,
    terminal: bool,
    finished: bool,
    success: bool,
    role_sent: bool,
}

impl ClaudeStream {
    fn frame(&self, delta: Value, finish: Option<&str>, usage: Option<&Value>) -> Bytes {
        let mut value = json!({"id":self.id,"object":"chat.completion.chunk","created":logging::now_ms()/1000,
            "model":self.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        if let Some(usage) = usage {
            value["usage"] = usage.clone();
        }
        protocol::chat_frame(&value)
    }
}

impl Stream for ClaudeStream {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        if self.telemetry.is_cancelled() {
            self.lease.cancel();
            self.finished = true;
            self.telemetry.note_error(cancellation::MANUAL_TERMINATED);
            return Poll::Ready(Some(Err(std::io::Error::other(cancellation::MANUAL_TERMINATED))));
        }
        if !self.role_sent {
            self.role_sent = true;
            return Poll::Ready(Some(Ok(self.frame(json!({"role":"assistant"}), None, None))));
        }
        if let Some(frame) = self.queued.pop_front() {
            if self.terminal && self.success && self.queued.is_empty() {
                self.lease.complete();
            }
            return Poll::Ready(Some(Ok(frame)));
        }
        if self.terminal {
            if self.success {
                self.lease.complete();
            }
            self.finished = true;
            return Poll::Ready(None);
        }
        let event =
            if let Some(first) = self.first.take() { Poll::Ready(Some(first)) } else { self.input.poll_recv(cx) };
        match event {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                self.finished = true;
                self.lease.cancel();
                self.telemetry.note_error("Claude 回复在完整终帧前中断");
                Poll::Ready(Some(Err(std::io::Error::other("Claude 回复在完整终帧前中断"))))
            }
            Poll::Ready(Some(Output::Delta(delta))) => {
                self.telemetry.note_first_frame();
                Poll::Ready(Some(Ok(self.frame(delta, None, None))))
            }
            Poll::Ready(Some(Output::Done(reply))) => {
                self.telemetry.report_usage(&reply.usage);
                self.terminal = true;
                self.success = true;
                self.queued.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                Poll::Ready(Some(Ok(self.frame(json!({}), Some(&reply.finish), Some(&reply.usage)))))
            }
            Poll::Ready(Some(Output::Failed(error))) => {
                self.telemetry.note_error(&error.message);
                self.terminal = true;
                self.success = false;
                self.lease.cancel();
                // 发错误而非 finish_reason=stop 或 [DONE]，避免截断被客户端当成成功。
                Poll::Ready(Some(Ok(protocol::chat_frame(&error.payload()))))
            }
        }
    }
}

async fn wait_cancel(telemetry: &RequestTelemetry) {
    match telemetry.cancel_token() {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}
