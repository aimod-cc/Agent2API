//! 只服务宿主frozen_once：登记唯一Join和退出guard后，独立观察本次已发Request。
//! Runtime/下游取消只关闭原Download等待，不丢execute；仅原绝对headers截止收束发送。

use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::{sync::{oneshot, Notify}, task::JoinHandle, time::Instant};

use super::{error, lifecycle::{self, Download, HttpDeadline, HttpHandle}};
use crate::server::core::{debug_traffic::TrafficCapture, egress::managed::FrozenHttpClient,
    plugin_host::wasm::conversation::WorkBuddyExecution};

type Permit = Box<dyn FnOnce(&reqwest::Request) -> Result<(), proto::WireError> + Send>;

/// 独立于Join槽的真实退出事实；绝不按空槽或借出Join推断已退出。
pub(in crate::server::core::plugin_host::script) struct WorkerState {
    registered: AtomicBool,
    finished: AtomicBool,
    job: Mutex<Option<JoinHandle<()>>>,
    result: Mutex<Option<Result<Value, proto::WireError>>>,
    changed: Notify,
}

impl WorkerState {
    pub(in crate::server::core::plugin_host::script) fn new() -> Arc<Self> {
        Arc::new(Self { registered: AtomicBool::new(false), finished: AtomicBool::new(false),
            job: Mutex::new(None), result: Mutex::new(None), changed: Notify::new() })
    }

    pub(in crate::server::core::plugin_host::script) fn registration_guard(self: &Arc<Self>) -> RegistrationGuard {
        RegistrationGuard { state: self.clone(), accepted: false }
    }

    pub(in crate::server::core::plugin_host::script) fn finished(&self) -> bool {
        self.registered.load(Ordering::Acquire) && self.finished.load(Ordering::Acquire)
    }

    fn begin(self: &Arc<Self>, download: Arc<Download>) -> Result<ExitGuard, proto::WireError> {
        self.registered.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| error("原冻结HTTP已登记发送worker，未创建第二份"))?;
        // 必须在spawn/首个await之前构造；后续失败、未poll销毁也如实封存并通知。
        Ok(ExitGuard { state: self.clone(), _download: download, completed: false })
    }

    async fn wait_result(&self) -> Result<Value, proto::WireError> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.finished() {
                let result = self.result.lock().map_err(|_| error("原冻结HTTP结果存储未确认"))?.take();
                return result.unwrap_or_else(|| Err(error("原冻结HTTP缺少本次真实结果，未补成功")));
            }
            changed.await;
        }
    }
}

pub(in crate::server::core::plugin_host::script) struct RegistrationGuard {
    state: Arc<WorkerState>,
    accepted: bool,
}

impl RegistrationGuard {
    pub(in crate::server::core::plugin_host::script) fn accept(&mut self) { self.accepted = true; }
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        if !self.accepted {
            let mut result = self.state.result.lock().unwrap_or_else(|error| error.into_inner());
            *result = Some(Err(error("原冻结HTTP登记未完成，未启动发送worker或恢复资格")));
            drop(result);
            self.state.registered.store(true, Ordering::Release);
            self.state.finished.store(true, Ordering::Release);
            self.state.changed.notify_waiters();
        }
    }
}

struct ExitGuard {
    state: Arc<WorkerState>,
    _download: Arc<Download>,
    completed: bool,
}

impl ExitGuard {
    fn complete(&mut self, result: Result<Value, proto::WireError>) {
        if self.completed { return; }
        {
            let mut slot = self.state.result.lock().unwrap_or_else(|error| error.into_inner());
            *slot = Some(result);
        }
        self.completed = true;
        self.state.finished.store(true, Ordering::Release);
        self.state.changed.notify_waiters();
        // 不在这里减Host.active：原Download/Response/读者仍可持强执行占用。
    }
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        if !self.completed {
            self._download.close();
            self.complete(Err(error("原冻结HTTP发送worker退出，结果未确认；未恢复发送资格")));
        }
    }
}

// 字段按输入→出口→Download→退出guard顺序释放；未poll/drop时请求与票先释放才封finished。
struct Worker {
    request: Option<reqwest::Request>,
    permit: Option<Permit>,
    owner: Option<FrozenHttpClient>,
    capture: Option<Arc<TrafficCapture>>,
    download: Arc<Download>,
    id: String,
    headers_deadline: Instant,
    exit: ExitGuard,
}

impl Worker {
    async fn run(mut self, start: oneshot::Receiver<()>) {
        let result = match tokio::time::timeout_at(self.headers_deadline, start).await {
            Ok(Ok(())) => self.observe().await,
            Ok(Err(_)) => Err(error("原冻结HTTP启动未确认，未发送或恢复发送资格")),
            Err(_) => Err(HttpDeadline::error()),
        };
        // 真实execute已经落定/到期，Response已移交原Download或丢弃；先清实际输入再封退出。
        drop(self.request.take());
        drop(self.permit.take());
        drop(self.capture.take());
        drop(self.owner.take());
        let failed = result.is_err();
        self.exit.complete(result);
        // 结果已封存才唤醒关闭等待；等待者即便失去机会，原请求仍不会恢复重发。
        if failed { self.download.close(); }
    }

    async fn observe(&mut self) -> Result<Value, proto::WireError> {
        if Instant::now() >= self.headers_deadline { return Err(HttpDeadline::error()); }
        self.download.check()?;
        self.download.frozen_current()?;
        let request = self.request.take().ok_or_else(|| error("原冻结HTTP请求已消费"))?;
        let permit = self.permit.take().ok_or_else(|| error("原冻结HTTP发送票已消费"))?;
        let client = self.owner.as_ref().ok_or_else(|| error("原冻结出口占用缺失"))?
            .client().map_err(|_| error("原冻结出口已关闭，未重新选择"))?.clone();
        // 原Call门最终核对/消费票。Join已实际登记；所有门退出后仅一次execute。
        permit(&request)?;
        let response = tokio::time::timeout_at(self.headers_deadline, client.execute(request)).await
            .map_err(|_| HttpDeadline::error())?
            .map_err(|_| error("原冻结HTTP连接失败或结果未确认，未重新提交"))?;
        // 已execute的future没有Runtime/Download取消分支；取消后迟到Response只释放，不补回执。
        self.download.check()?;
        if Instant::now() >= self.headers_deadline { return Err(HttpDeadline::error()); }
        if let Some(capture) = &self.capture { capture.attach_response(response.status().as_u16(), response.headers()); }
        let headers: serde_json::Map<String, Value> = response.headers().iter().filter_map(|(name, value)| {
            value.to_str().ok().map(|value| (name.as_str().to_string(), json!(value)))
        }).collect();
        let headers_raw: Vec<(String, String)> = response.headers().iter().filter_map(|(name, value)| {
            value.to_str().ok().map(|value| (name.as_str().to_string(), value.to_string()))
        }).collect();
        let result = json!({ "id": self.id, "status": response.status().as_u16(), "headers": headers,
            "headers_raw": headers_raw, "url": response.url().as_str() });
        let mut slot = tokio::time::timeout_at(self.headers_deadline, self.download.response.lock()).await
            .map_err(|_| HttpDeadline::error())?;
        self.download.check()?;
        if slot.is_some() { return Err(error("原冻结HTTP响应槽已占用，未覆盖或拼接")); }
        *slot = Some(response);
        self.download.check()?;
        drop(slot);
        Ok(result)
    }
}

pub(super) async fn open<F>(params: &Value, body: bytes::Bytes, owner: FrozenHttpClient,
    execution: Arc<WorkBuddyExecution>, deadline: HttpDeadline, headers_deadline: HttpDeadline, permit: F,
) -> Result<(Value, HttpHandle), proto::WireError>
where F: FnOnce(&reqwest::Request) -> Result<(), proto::WireError> + Send + 'static {
    deadline.check()?;
    headers_deadline.check()?;
    if params.get("body").is_some_and(|body| !body.is_null()) || params.get("body_resource").is_some()
        || params.get("multipart").is_some() || body.is_empty() || body.len() > 8 * 1024 * 1024 {
        return Err(error("封闭HTTP只接受已准备的有界原字节，未读取资源或回退普通正文"));
    }
    let headers_deadline = headers_deadline.instant().ok_or_else(|| error("原冻结HTTP缺少绝对发送截止"))?
        .min(deadline.instant().ok_or_else(|| error("原冻结HTTP缺少原调用截止"))?);
    let url = params["url"].as_str().ok_or_else(|| error("原冻结HTTP缺少固定URL"))?;
    let client = owner.client().map_err(|_| error("原冻结出口不可用，未回退普通客户端"))?.clone();
    let mut request = client.request(reqwest::Method::POST, url);
    let fields = params["headers"].as_object().filter(|fields| fields.len() <= 128)
        .ok_or_else(|| error("原冻结HTTP请求头无效"))?;
    let mut sent_headers = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        let value = value.as_str().ok_or_else(|| error("原冻结HTTP请求头不是文本"))?;
        request = request.header(name, value);
        sent_headers.push((name.clone(), value.to_string()));
    }
    let capture = params["trace"].as_str().and_then(super::super::trace::find);
    if let Some(capture) = &capture {
        let sent_body = serde_json::from_slice(&body).map_err(|_| error("原冻结HTTP正文校验未确认"))?;
        capture.reset_request(url, "workbuddy", &sent_headers, &sent_body);
    }
    let request = request.body(body).build().map_err(|_| error("原冻结HTTP构造失败，未退还发送资格"))?;
    let handle = lifecycle::register_frozen(params, deadline, capture.clone(), owner.clone(), execution)?;
    let download = handle.download.clone();
    let state = download.worker.as_ref().ok_or_else(|| error("原冻结HTTP未登记worker槽"))?.clone();
    let exit = state.begin(download.clone())?;
    let worker = Worker { request: Some(request), permit: Some(Box::new(permit)), owner: Some(owner), capture,
        download: download.clone(), id: handle.id().to_string(), headers_deadline, exit };
    let (start, ready) = oneshot::channel();
    {
        let mut slot = state.job.lock().map_err(|_| error("原冻结HTTP的Join登记不可用"))?;
        if slot.is_some() { return Err(error("原冻结HTTP已有真实Join，未再启动")); }
        // ready阻止worker在Join写入之前消费发送票；无await窗口，spawn失败guard仍会退出通知。
        *slot = Some(crate::spawn_task(worker.run(ready)));
    }
    start.send(()).map_err(|_| error("原冻结HTTPworker启动未确认，未重发"))?;
    // 这里只等待元数据；丢掉此等待/HttpHandle会关闭原读者，但不会abort独立worker的execute。
    let result = download.wait(state.wait_result()).await;
    let result = match result {
        Ok(result) => result?,
        Err(wait_error) => {
            // 退出guard已先保存了真实失败时优先交还原失败；未退出仍只能报告等待未确认。
            if state.finished() {
                let saved = state.result.lock().map_err(|_| error("原冻结HTTP结果存储未确认"))?.take();
                match saved {
                    Some(Err(failure)) => return Err(failure),
                    Some(Ok(_)) | None => return Err(wait_error),
                }
            } else { return Err(wait_error); }
        }
    };
    Ok((result, handle))
}
