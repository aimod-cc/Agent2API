//! Magpie subscriptionBridge 的 runs/calls/idle：活跃 CLI 按账号、调用方和历史绑定。
//! 工具结果只交给原运行实例，不换号重放。内存会话失效明确报错，不改写官方存档。
use super::{errors, events::Reply, messages::Request, process, tools::ToolResult};
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::errors::GatewayError;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) const IDLE_TTL: Duration = Duration::from_secs(3600);
pub(super) const TOOL_TTL: Duration = Duration::from_secs(300);
const MAX_RUNS: usize = 24;
const MAX_IDLE: usize = 6;

pub(super) enum Output {
    Delta(Value),
    Done(Reply),
    Failed(GatewayError),
}

pub(super) struct Command {
    pub request: Request,
    pub prompt: Vec<Value>,
    pub results: Vec<(String, ToolResult)>,
    pub alongside: Vec<Value>,
    pub output: mpsc::Sender<Output>,
    pub telemetry: Arc<RequestTelemetry>,
}

pub(super) struct Handle {
    pub id: String,
    pub cancel: CancellationToken,
    pub commands: mpsc::Sender<Command>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(super) struct Lease {
    pub handle: Arc<Handle>,
    armed: bool,
}
impl Lease {
    pub fn new(handle: Arc<Handle>) -> Self {
        Self { handle, armed: true }
    }
    pub fn complete(&mut self) {
        self.armed = false;
    }
    pub fn cancel(&self) {
        self.handle.cancel.cancel();
        remove(&self.handle.id);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.armed {
            self.cancel();
        }
    }
}

struct Cursor {
    hash: String,
    waiting: HashSet<String>,
    at: Instant,
}
struct Entry {
    handle: Arc<Handle>,
    owner: String,
    account: String,
    /// 同一进程的模型/工具/出口指纹。
    identity: String,
    cursor: Option<Cursor>,
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) struct Selected {
    pub handle: Arc<Handle>,
    pub new_receiver: Option<mpsc::Receiver<Command>>,
    pub start: usize,
    pub waiting_for_tools: bool,
}

pub(super) fn select(owner: &str, account: &str, request: &Request) -> Result<Selected, GatewayError> {
    let mut entries =
        registry().lock().map_err(|_| errors::local("claude_sessions_unavailable", "Claude 会话表不可用"))?;
    entries.retain(|_, entry| {
        let expired = entry.handle.cancel.is_cancelled()
            || entry.cursor.as_ref().is_some_and(|cursor| {
                cursor.at.elapsed() >= if cursor.waiting.is_empty() { IDLE_TTL } else { TOOL_TTL }
            });
        if expired {
            entry.handle.cancel.cancel();
        }
        !expired
    });
    let result_ids: HashSet<String> = request.latest_results.iter().map(|(id, _, _)| id.clone()).collect();
    let mut found = None;
    for (id, entry) in entries.iter() {
        if entry.owner != owner {
            continue;
        }
        let Some(cursor) = entry.cursor.as_ref() else {
            if entry.account == account && entry.identity == request.identity {
                return Err(GatewayError::with_status(409, "相同 Claude 对话已有请求正在执行，请等待或取消上一轮")
                    .with_code("claude_session_busy"));
            }
            continue;
        };
        let prefix = request.prefix_hashes.iter().position(|hash| hash == &cursor.hash);
        let matches = if result_ids.is_empty() {
            cursor.waiting.is_empty() && prefix.is_some()
        } else if cursor.waiting.is_empty() {
            prefix.is_some()
        } else {
            !cursor.waiting.is_disjoint(&result_ids)
        };
        if !matches {
            continue;
        }
        if entry.account != account {
            return Err(errors::session("Claude 工具或会话属于另一个账号，不能换号回灌"));
        }
        if entry.identity != request.identity {
            return Err(errors::session("Claude 会话的模型、工具或系统说明已变化，不能把结果交给旧会话"));
        }
        if prefix.is_none() {
            return Err(errors::session("Claude 工具结果与原会话历史不匹配，未重放到新进程"));
        }
        if !result_ids.is_empty() && !cursor.waiting.is_empty() && cursor.waiting != result_ids {
            return Err(errors::session("请一次回传这轮全部 Claude 工具结果；缺失、重复或陌生调用未被接受"));
        }
        if found.is_some() {
            return Err(errors::session("存在多个相同 Claude 会话，无法确定工具归属；请为每个对话提供独立会话标识"));
        }
        found = Some((id.clone(), prefix.unwrap_or(0) + 1, !cursor.waiting.is_empty()));
    }
    if let Some((id, start, waiting_for_tools)) = found {
        let entry = entries.get_mut(&id).ok_or_else(|| errors::session("Claude 会话已失效"))?;
        if !waiting_for_tools && start >= request.messages.len() {
            return Err(errors::session("Claude 会话没有新消息，未重复发送已完成的请求"));
        }
        entry.cursor = None;
        return Ok(Selected { handle: entry.handle.clone(), new_receiver: None, start, waiting_for_tools });
    }
    if !result_ids.is_empty() {
        return Err(errors::session("原 Claude 工具会话已结束或不属于当前调用方，未在新进程伪造工具结果"));
    }
    if entries.len() >= MAX_RUNS {
        let oldest = entries
            .iter()
            .filter_map(|(id, entry)| {
                entry.cursor.as_ref().filter(|c| c.waiting.is_empty()).map(|c| (id.clone(), c.at))
            })
            .min_by_key(|(_, at)| *at)
            .map(|(id, _)| id);
        if let Some(id) = oldest {
            if let Some(entry) = entries.remove(&id) {
                entry.handle.cancel.cancel();
            }
        }
    }
    if entries.len() >= MAX_RUNS {
        return Err(errors::local("claude_capacity", "Claude 活跃会话已达上限，请完成或取消已有对话后重试"));
    }
    let id = process::token()?;
    let (commands, receiver) = mpsc::channel(1);
    let handle = Arc::new(Handle { id: id.clone(), commands, cancel: CancellationToken::new() });
    entries.insert(
        id,
        Entry {
            handle: handle.clone(),
            owner: owner.into(),
            account: account.into(),
            identity: request.identity.clone(),
            cursor: None,
        },
    );
    Ok(Selected { handle, new_receiver: Some(receiver), start: 0, waiting_for_tools: false })
}

pub(super) fn park(handle: &Handle, request: &Request, reply: &Reply) -> Result<(), GatewayError> {
    let waiting: HashSet<_> = reply.calls.iter().map(|call| call.id.clone()).collect();
    let hash = if waiting.is_empty() {
        let mut messages = request.messages.clone();
        messages.push(reply.message());
        super::messages::hashes(&messages).last().cloned().unwrap_or_default()
    } else {
        request.prefix_hashes.last().cloned().unwrap_or_default()
    };
    let mut entries = registry().lock().map_err(|_| errors::session("Claude 会话表不可用"))?;
    let entry = entries.get_mut(&handle.id).ok_or_else(|| errors::session("Claude 会话已结束"))?;
    if handle.cancel.is_cancelled() {
        return Err(errors::session("Claude 会话已取消"));
    }
    entry.cursor = Some(Cursor { hash, waiting, at: Instant::now() });
    let mut idle: Vec<_> = entries
        .iter()
        .filter_map(|(id, e)| e.cursor.as_ref().filter(|c| c.waiting.is_empty()).map(|c| (id.clone(), c.at)))
        .collect();
    idle.sort_by_key(|(_, at)| *at);
    for (id, _) in idle.iter().take(idle.len().saturating_sub(MAX_IDLE)) {
        if let Some(entry) = entries.remove(id) {
            entry.handle.cancel.cancel();
        }
    }
    Ok(())
}

pub(super) fn remove(id: &str) {
    if let Ok(mut entries) = registry().lock() {
        entries.remove(id);
    }
}

pub(super) fn shutdown() {
    if let Ok(mut entries) = registry().lock() {
        for (_, entry) in entries.drain() {
            entry.handle.cancel.cancel();
        }
    }
}
