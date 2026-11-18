use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::server::core::{debug_traffic::TrafficCapture, egress::managed::FrozenHttpClient};
use super::{cancellation, error, frozen::WorkerState, pending::Pending};
use crate::server::core::plugin_host::wasm::conversation::WorkBuddyExecution;

/// 仅由可信 Rust 调用链传递，绝不从脚本或 WASM JSON 读取绝对时间。
#[derive(Clone, Copy, Default)]
pub(in crate::server::core::plugin_host) struct HttpDeadline(Option<Instant>);

impl HttpDeadline {
    pub fn at(deadline: Instant) -> Self { Self(Some(deadline)) }

    // 宿主专用入口只能缩短到原调用期限，不能由JSON或重新起算延长。
    pub(in crate::server::core::plugin_host) fn cap_at(self, deadline: Instant) -> Self {
        Self(Some(self.0.map_or(deadline, |original| original.min(deadline))))
    }

    pub(in crate::server::core::plugin_host::script) fn instant(self) -> Option<Instant> { self.0 }

    pub fn error() -> proto::WireError {
        proto::WireError::with_code("上游单请求总期限已到", "timeout")
    }

    pub fn check(self) -> Result<(), proto::WireError> {
        if self.0.is_some_and(|deadline| Instant::now() >= deadline) { Err(Self::error()) }
        else { Ok(()) }
    }

    async fn elapsed(self) {
        match self.0 {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending::<()>().await,
        }
    }

    pub async fn run<T>(self, work: impl Future<Output = Result<T, proto::WireError>>) -> Result<T, proto::WireError> {
        let result = self.run_until(work).await;
        // 同步准备/序列化可能在一次 poll 内跨过期限，不能只依赖计时器唤醒。
        self.check()?;
        result
    }

    pub async fn run_until<T>(self, work: impl Future<Output = Result<T, proto::WireError>>) -> Result<T, proto::WireError> {
        self.check()?;
        tokio::select! {
            biased;
            _ = self.elapsed() => Err(Self::error()),
            result = work => result,
        }
    }
}

const ACTIVE: u8 = 0;
const EXPIRED: u8 = 1;
const CLOSED: u8 = 2;

pub(in crate::server::core::plugin_host::script) struct Download {
    pub(in crate::server::core::plugin_host::script) response: tokio::sync::Mutex<Option<reqwest::Response>>,
    pub(super) pending: tokio::sync::Mutex<Pending>,
    pub(super) chunk_limit: usize,
    pub(super) touched: tokio::sync::Mutex<std::time::Instant>,
    pub(super) sensitive: bool,
    pub(super) usage_trace: Option<crate::server::core::usage_query::diagnostics::RequestTrace>,
    capture: tokio::sync::Mutex<Option<Arc<TrafficCapture>>>,
    deadline: HttpDeadline,
    generation: u64,
    phase: AtomicU8,
    cancel: CancellationToken,
    // Join和finished各自保留真实事实；空Join槽不能说明worker已退出。
    pub(in crate::server::core::plugin_host::script) worker: Option<Arc<WorkerState>>,
    // 原发送/读者均持同一Arc<Download>；Response与worker退出后才释放执行占用，再释放出口。
    // 普通请求均为None，不增添执行租约或改变其取消行为。
    execution: Option<Arc<WorkBuddyExecution>>,
    frozen_owner: Option<FrozenHttpClient>,
    network: Mutex<Option<crate::server::core::runtime_capacity::Ticket>>,
    _buffer: crate::server::core::runtime_capacity::Allocation,
}

impl Download {
    pub(super) fn holds_frozen(&self) -> bool { self.frozen_owner.is_some() }

    pub(in crate::server::core::plugin_host::script) fn frozen_current(&self) -> Result<(), proto::WireError> {
        if let Some(owner) = &self.frozen_owner {
            owner.client().map_err(|_| error("原冻结出口已关闭，未重新选择或回退普通客户端"))?;
        }
        Ok(())
    }

    pub(in crate::server::core::plugin_host::script) fn check(&self) -> Result<(), proto::WireError> {
        if self.phase.load(Ordering::Acquire) == ACTIVE && self.deadline.check().is_err() { self.expire(); }
        match self.phase.load(Ordering::Acquire) {
            EXPIRED => Err(HttpDeadline::error()),
            CLOSED => Err(cancellation()),
            _ => Ok(()),
        }
    }

    pub(super) fn finish(&self) -> Result<(), proto::WireError> {
        self.check()?;
        if self.phase.compare_exchange(ACTIVE, CLOSED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err(self.stopped());
        }
        self.cancel.cancel();
        Ok(())
    }

    fn stopped(&self) -> proto::WireError { self.check().err().unwrap_or_else(cancellation) }

    pub(in crate::server::core::plugin_host::script) async fn wait<T>(&self, work: impl Future<Output = T>) -> Result<T, proto::WireError> {
        self.check()?;
        let result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(self.stopped()),
            _ = self.deadline.elapsed() => { self.expire(); return Err(self.stopped()); }
            result = work => result,
        };
        self.check()?;
        Ok(result)
    }

    pub(super) async fn capture(&self) -> Result<Option<Arc<TrafficCapture>>, proto::WireError> {
        let capture = self.wait(self.capture.lock()).await?;
        Ok(capture.clone())
    }

    fn expire(&self) {
        if self.phase.compare_exchange(ACTIVE, EXPIRED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.cancel.cancel();
            self.release_ready();
        }
    }

    pub(in crate::server::core::plugin_host::script) fn close(&self) {
        // 已超时的对象保留原因；正在退出的 read 仍应得到 timeout 而非取消。
        let _ = self.phase.compare_exchange(ACTIVE, CLOSED, Ordering::AcqRel, Ordering::Acquire);
        self.cancel.cancel();
        self.release_ready();
    }

    fn release_ready(&self) {
        if let Ok(mut response) = self.response.try_lock() { response.take(); }
        if let Ok(mut pending) = self.pending.try_lock() { pending.clear(); }
        let capture = self.capture.try_lock().ok().and_then(|mut capture| capture.take());
        drop(capture);
    }

    async fn release(&self) {
        // read/send 已被 cancel 唤醒。即使当时持锁，也会在退出后主动释放，毋须下一次 read。
        self.response.lock().await.take();
        self.pending.lock().await.clear();
        self.network.lock().unwrap_or_else(|error| error.into_inner()).take();
        let capture = self.capture.lock().await.take();
        drop(capture);
    }

    fn watch(self: &Arc<Self>, id: String) {
        // 无 timeout 的旧请求只监听原有取消，不增加计时器或更短的生命周期。
        let deadline = self.deadline;
        let weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        let execution_cancel = self.execution.as_ref().and_then(|execution| execution.cancellation_tokens().ok());
        crate::spawn_task(async move {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {}
                _ = async {
                    match execution_cancel {
                        Some([call, ended, runtime]) => tokio::select! {
                            biased;
                            _ = call.cancelled() => {},
                            _ = ended.cancelled() => {},
                            _ = runtime.cancelled() => {},
                        },
                        None => std::future::pending::<()>().await,
                    }
                } => { if let Some(download) = weak.upgrade() { download.close(); } }
                _ = deadline.elapsed() => {
                    if let Some(download) = weak.upgrade() { download.expire(); }
                }
            }
            if let Some(download) = weak.upgrade() {
                if download.holds_frozen() { close_matching(&id, &download); }
                download.release().await;
            }
        });
    }
}

fn downloads() -> &'static Mutex<HashMap<String, Arc<Download>>> {
    static ITEMS: OnceLock<Mutex<HashMap<String, Arc<Download>>>> = OnceLock::new();
    ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Context 持有具体对象而非可重用的名称；同名替换后的旧析构不能关闭新连接。
pub(in crate::server::core::plugin_host) struct HttpHandle {
    id: String,
    pub(in crate::server::core::plugin_host::script) download: Arc<Download>,
    close_on_drop: bool,
}

impl HttpHandle {
    pub fn id(&self) -> &str { &self.id }

    pub fn deadline(&self) -> HttpDeadline { self.download.deadline }

    pub async fn read(&self) -> Result<Value, proto::WireError> {
        super::read_download(&self.id, &self.download).await
    }

    pub fn close(&self) { close_matching(&self.id, &self.download); }

    // 旧脚本继续通过全局表与显式 close 管理；只解除本地 opening 保护，不泄漏 Arc。
    pub(super) fn detach(mut self) { self.close_on_drop = false; }
}

impl Drop for HttpHandle {
    fn drop(&mut self) {
        if self.close_on_drop { self.close(); }
    }
}

pub(super) fn register(params: &Value, deadline: HttpDeadline, capture: Option<Arc<TrafficCapture>>,
    sensitive: bool, system_proxy_when_unset: bool) -> Result<HttpHandle, proto::WireError> {
    register_inner(params, deadline, capture, sensitive, system_proxy_when_unset, None, None)
}

pub(in crate::server::core::plugin_host::script) fn register_frozen(params: &Value, deadline: HttpDeadline,
    capture: Option<Arc<TrafficCapture>>, owner: FrozenHttpClient, execution: Arc<WorkBuddyExecution>,
) -> Result<HttpHandle, proto::WireError> {
    register_inner(params, deadline, capture, true, false, Some(owner), Some(execution))
}

fn register_inner(params: &Value, deadline: HttpDeadline, capture: Option<Arc<TrafficCapture>>,
    sensitive: bool, system_proxy_when_unset: bool, frozen_owner: Option<FrozenHttpClient>, execution: Option<Arc<WorkBuddyExecution>>) -> Result<HttpHandle, proto::WireError> {
    deadline.check()?;
    let id = params["id"].as_str().filter(|id| id.len() <= 80 && !id.is_empty())
        .ok_or_else(|| error("缺少脚本请求标识"))?.to_string();
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    let worker = frozen_owner.as_ref().map(|_| WorkerState::new());
    let mut registration = worker.as_ref().map(|worker| worker.registration_guard());
    let capacity = crate::server::core::runtime_capacity::limits();
    let network = crate::server::core::runtime_capacity::try_acquire(
        crate::server::core::runtime_capacity::Class::Network, params["provider"].as_str().unwrap_or(""))?;
    let buffer = crate::server::core::runtime_capacity::Allocation::new(256 * 1024)?;
    let usage_trace = if sensitive { None } else {
        crate::server::core::usage_query::diagnostics::current().and_then(|trace| {
            let raw = params["url"].as_str().unwrap_or("");
            let url = url::Url::parse(raw).ok()?;
            let proxy = super::proxy_for(&url, params).ok()?;
            trace.request(params["method"].as_str().unwrap_or("GET"), raw, proxy.as_ref(),
                system_proxy_when_unset && !super::is_loopback(&url))
        })
    };
    let download = Arc::new(Download {
        response: tokio::sync::Mutex::new(None), pending: tokio::sync::Mutex::new(Pending::default()),
        chunk_limit: params["max_chunk"].as_u64().map(|size| size.clamp(1024, 256 * 1024) as usize).unwrap_or(usize::MAX),
        touched: tokio::sync::Mutex::new(std::time::Instant::now()), sensitive, usage_trace, capture: tokio::sync::Mutex::new(capture), deadline,
        generation: GENERATION.fetch_add(1, Ordering::Relaxed), phase: AtomicU8::new(ACTIVE),
        cancel: CancellationToken::new(), worker, execution, frozen_owner,
        network: Mutex::new(Some(network)), _buffer: buffer,
    });
    let mut stale = Vec::new();
    let inserted = {
        // 全局表只做有界元数据操作；释放响应/调试采集不能占住这把锁。
        let mut items = downloads().lock().map_err(|_| error("脚本网络会话不可用"))?;
        deadline.check()?;
        items.retain(|_, entry| {
            // 终态保留到原 read/close/Context Drop 回收，不另建墓碑表。
            let keep = entry.deadline.0.is_some() || entry.touched.try_lock()
                .map_or(true, |time| time.elapsed() < Duration::from_secs(1800));
            if !keep { stale.push(entry.clone()); }
            keep
        });
        if items.len() >= capacity.connections || items.contains_key(&id) { false }
        else { items.insert(id.clone(), download.clone()); true }
    };
    let handle = HttpHandle { id, download, close_on_drop: true };
    if inserted { handle.download.watch(handle.id.clone()); }
    for entry in stale { entry.close(); }
    if !inserted { return Err(error("脚本上游连接数已达上限或请求标识重复")); }
    handle.download.check()?;
    if let Some(registration) = &mut registration { registration.accept(); }
    Ok(handle)
}

pub(super) fn find(id: &str) -> Result<Arc<Download>, proto::WireError> {
    downloads().lock().map_err(|_| error("脚本网络会话不可用"))?.get(id).cloned()
        .ok_or_else(|| error("脚本网络会话已结束"))
}

pub(super) fn close_matching(id: &str, download: &Arc<Download>) {
    {
        let mut items = downloads().lock().unwrap_or_else(|error| error.into_inner());
        if items.get(id).is_some_and(|current| current.generation == download.generation && Arc::ptr_eq(current, download)) {
            items.remove(id);
        }
    }
    download.close();
}

pub(super) fn close_id(id: &str) {
    let download = {
        let mut items = downloads().lock().unwrap_or_else(|error| error.into_inner());
        // 冻结响应不能凭插件给的名字关闭；原HttpHandle::close仍按对象身份收尾。
        if items.get(id).is_some_and(|download| download.holds_frozen()) { return; }
        items.remove(id)
    };
    if let Some(download) = download { download.close(); }
}

pub(super) fn clear_scripts() {
    let mut closing = Vec::new();
    {
        let mut items = downloads().lock().unwrap_or_else(|error| error.into_inner());
        items.retain(|id, download| {
            if id.starts_with("wasm-") { return true; }
            closing.push(download.clone());
            false
        });
    }
    for download in closing { download.close(); }
}
