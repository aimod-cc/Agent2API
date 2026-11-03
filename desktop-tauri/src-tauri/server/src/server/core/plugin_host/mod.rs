//! 提供商插件宿主：子进程监管 + RPC 客户端。
//!
//! ── 这个模块在架构里的位置 ──────────────────────────────────
//! 它是**网关侧**的插件基础设施，替代原本「provider 实现编译进同一个进程」
//! 的那条路。上层（`providers::adapter::adapter_for`）只看见
//! [`crate::server::core::providers::adapter::ProviderAdapter`]；这个模块
//! 提供 [`RpcAdapter`]（那个 trait 的远程实现）与 [`Host`]（进程生命周期）。
//!
//! ── 进程模型：每个插件包一个宿主 ──────────────────────────────
//! 与 magpie 同款：一个包 = 一个进程 = 若干家 provider。理由：
//!   - 21 份 tokio 运行时与连接池的代价在桌面端很明显（每份至少几 MB 常驻）；
//!   - 跨 provider 的共享状态（HTTP 客户端、模型缓存）只存一份；
//!   - 崩溃恢复的粒度足够 —— 一个 provider 的 panic 带走的是同包其它家，
//!     但**不会**带走网关（那是插件架构的主要收益）。
//!
//! ── 崩溃恢复 ────────────────────────────────────────────────
//! 子进程死了 → 所有在途调用立刻失败（不是挂起）→ 网关按「上游不可用」
//! 处理（换账号 / 换家 / 报错）→ 下一次调用时重新 spawn。
//!
//! **不做无限重启**：连续崩溃按指数退避（1s → 2s → 4s … 上限 60s），
//! 且达到 5 次后进入「坏包」状态 —— 界面显示「这个包反复崩溃，已停用」，
//! 用户可以手动重试。无限重启会让一个必崩的包把 CPU 打满。
//!
//! ── 死锁防线（改代码前必读）──────────────────────────────────
//! 读循环绝不在里面 await 业务。插件会在处理请求时回调网关
//! （`store.get` 等），那些回调由本模块派发；若读循环正卡在等某个业务
//! 完成，回调的应答就永远读不到。所以：
//!   - 读循环只做「分类 → 投递到 pending 表 / 起 task」；
//!   - 所有业务 await 都在独立的 task 里；
//!   - 写端只串行化帧写入，不持进程生命周期锁等待 IO；
//!   - 已开始的帧独立写完，避免调用方取消破坏共享管道的 JSON 边界。

mod activity;
mod bootstrap;
mod callbacks;
pub mod catalog;
mod compatibility;
mod errors;
mod gemini_signatures;
mod install;
mod leased_stream;
mod lifecycle;
mod logs;
mod management;
mod model_sync;
pub mod operations;
mod ownership;
mod runtime;
mod packages;
mod pack_view;
mod process;
mod registry;
mod rpc;
pub(crate) mod rpc_cache;
pub mod schedules;
pub mod settings;
mod stream;
mod switching;
pub mod update;

#[cfg(test)]
mod tests;

pub use callbacks::GatewayCallbacks;
pub use bootstrap::{
    reload as reload_host, set_loopback_port, spawn as spawn_bootstrap, start as bootstrap_host,
    StoreCallbacks,
};
pub use install::{
    install_zip, list_packs, resolve_ui_file, set_pack_enabled, set_provider_enabled, uninstall,
    InstalledPack, PackProviderSummary, PackRegistry, PackSummary, PackUiSummary,
};
pub use registry::{install as install_provider, installed_ids, lookup, lookup_ready, remove, clear};
pub use registry::{serves as serves_provider};
pub use rpc::{HostOptions, RpcAdapter};
pub(crate) use rpc_cache::meta as model_catalog_meta;
pub use management::snapshot as management_snapshot;
pub(crate) use packages::prepare as prepare_pack;
pub(crate) use compatibility::provider_error as provider_compatibility_error;

/// 当前平台的目标三元组（界面提示「下载哪个平台的包」用）。
pub fn host_target() -> String {
    rpc::target_triple()
}

/// 某个 provider 的**展示名**（按已安装包的清单查，不看它是否在跑）。
///
/// ── 为什么不走 `lookup(id).label()` ─────────────────────────
/// 那个只认**握手成功**的家。停用的家（如示例提供商 `demo`）不参与握手，
/// 于是「库里还有它的账号」时界面只能显示裸 id —— 用户看到一个 `demo`
/// 分组，不知道那是什么。清单里写着 label，读它即可。
///
/// 代价是每次调用都读一遍注册表与 `pack.json`。这个函数的唯一消费方是
/// provider 摘要（一次 `/api/session` 调一次、只对「有账号但没在跑」的家
/// 问），量级可以忽略；真要优化再加缓存，别在这里提前做。
pub fn provider_label(id: &str) -> Option<String> {
    for pack in install::list_packs() {
        if !pack.enabled {
            continue;
        }
        if let Some(found) = pack.providers.iter().find(|item| item.id == id) {
            return Some(found.label.clone());
        }
    }
    None
}

/// 插件目录的展示路径（界面显示「装到哪儿了」用）。
pub fn plugins_dir_display() -> String {
    install::plugins_dir().display().to_string()
}

/// 清空 provider 注册表（卸载 / 重装前调）。
///
/// 与 [`clear`] 是同一件事，单独起个名字是为了让 api 层的调用点读起来是
/// 「把已装入的 provider 摘掉」，而不是「清一张表」—— 两处语义确实不同：
/// 前者是用户可见的动作（卸载），后者是内部重建步骤。
pub fn clear_providers() {
    clear();
    rpc_cache::clear_all();
}

pub(crate) use leased_stream::hold_adapter;
pub(crate) use stream::RpcStream;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::sync::{oneshot, Mutex};

use crate::server::logging;

pub use ownership::ProviderCategory;
pub use runtime::{current, detach, host_for_pack, host_for_provider, install, provider_category, provider_pack, shutdown_all};
use runtime::provider_info;

/// 当前已加载的 provider 申报了哪些**管理面板**（去重）。
///
/// ── 界面为什么需要它 ────────────────────────────────────────
/// 伪装头编辑器、令牌池、成长任务、每日保活这些面板的实现在插件里，但渲染
/// 在网关界面。界面原先按 provider id 写死，插件没装时照样显示、点进去是
/// 一屏「读取失败」。改为按这里的申报渲染：没装插件 → 空表 → 不画面板。
///
/// 宿主没起（没装包 / 崩溃中）时返回空表，与「一个面板都没有」同义。
pub fn admin_panels() -> Vec<proto::AdminPanel> {
    let providers = runtime::provider_infos();
    let mut panels: Vec<proto::AdminPanel> = Vec::new();
    for info in providers {
        for panel in info.admin_panels {
            // `Unknown` 是「新插件配旧网关」的降级位，界面不认识它，
            // 别让它出现在清单里
            if panel != proto::AdminPanel::Unknown && !panels.contains(&panel) {
                panels.push(panel);
            }
        }
    }
    panels
}

/// 某一家是否申报了某个管理面板（面板要按 provider 定位时用）。
pub fn has_admin_panel(provider: &str, panel: proto::AdminPanel) -> bool {
    provider_info(provider).is_some_and(|info| info.admin_panels.contains(&panel))
}

/// 各家申报的**账号行渲染参数**（`provider id → AccountFeatures`）。
///
/// ── 界面为什么需要它 ────────────────────────────────────────
/// 账号表每一列读哪个字段、要不要给余额入口、要不要拼地区后缀，全是上游
/// 事实。界面原先有一张按 provider id 写死的常量表，加一家插件就得回头改
/// 网关。改为按这里的申报渲染：装了什么插件，界面就按什么渲染。
///
/// 只收**申报过的**家：没申报（旧插件）的 id 不出现在表里，界面走自己的
/// 兜底表 —— 这样升级是渐进的，不必等所有插件同时发版。
pub fn account_features() -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    let providers = runtime::provider_infos();
    for info in providers {
        let Some(features) = info.account_features else {
            continue;
        };
        // 序列化失败只可能是协议结构本身不可序列化（不会发生）；那时宁缺
        // 勿错 —— 界面走兜底表，也不会画出半份参数
        if let Ok(value) = serde_json::to_value(&features) {
            map.insert(info.id.as_str().to_string(), value);
        }
    }
    map
}

/// 某一家申报的账号行参数（没申报 = `None`）。
pub fn account_features_of(provider: &str) -> Option<proto::AccountFeatures> {
    provider_info(provider)?.account_features
}

/// 某一家申报的签到能力（没申报 = `None`）。
pub fn checkin_of(provider: &str) -> Option<proto::CheckinInfo> {
    provider_info(provider)?.checkin
}

/// 全部申报的签到能力（`provider id → 申报`）。
///
/// 只收申报过的家 —— 旧插件不申报时不在表里，调用方走内置兜底
/// （`Capabilities::checkin`），升级因此是渐进的。
pub fn checkin_features() -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    let providers = runtime::provider_infos();
    for info in providers {
        let Some(checkin) = info.checkin else {
            continue;
        };
        if let Ok(value) = serde_json::to_value(&checkin) {
            map.insert(info.id.as_str().to_string(), value);
        }
    }
    map
}

/// 这次写失败是不是「帧太大」（issue #379）。
///
/// ── 为什么按文案判而不是新加错误码 ──────────────────────────
/// 判据由协议 crate 的 `write_frame` 给出（`io::ErrorKind::InvalidData` +
/// 「帧超过 … 字节上限」），而 `InvalidData` 同时也用于「帧无法序列化」。
/// 两者在编排层应当同样处理 —— 都是**本地确定性失败**，重试没有意义 ——
/// 所以这里把两种 `InvalidData` 一并归入。真正的区分在协议层，网关不必
/// 复制那份知识。
fn is_frame_too_large(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::InvalidData
        && error.to_string().contains("帧超过")
}

/// 宿主状态（给界面显示）。
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct HostStatus {
    /// 子进程是否在跑。
    pub running: bool,
    /// 已加载的 provider id 列表。
    pub providers: Vec<String>,
    /// 连续崩溃次数（> 0 说明这个包不稳）。
    pub crashes: u32,
    /// 已停用（崩溃太多次）。
    pub disabled: bool,
    /// 尚未恢复的失败原因；历史诊断保留在日志中。
    pub last_error: Option<String>,
}

/// 一个提供商插件宿主（对应一个已安装的包）。
pub struct Host {
    /// 启动参数。
    options: HostOptions,
    activity: Arc<activity::Activity>,
    /// 当前代进程的写端；退出状态由 ProcessRun 独立持有。
    inner: Mutex<Option<Inner>>,
    /// 状态转换与调用登记共用此锁；不得跨 await 持有。
    lifecycle: StdMutex<()>,
    generation: AtomicU64,
    active: StdMutex<Option<Arc<process::ProcessRun>>>,
    /// 回调应答的等待者（插件 → 网关的请求）。
    ///
    /// ── 为什么用 std Mutex ────────────────────────────────────
    /// 这个表只在「读循环派发」与「发回调」两处短暂访问，临界区里没有
    /// await。用 tokio 的 Mutex 会让读循环多一次 await 点（而读循环的
    /// 原则是「不 await」）。
    pending: StdMutex<HashMap<i64, PendingCall>>,
    /// 下一个回调 id。
    next_id: AtomicI64,
    /// 在途的流式调用（`abort` 与额度归还都按它找）。
    streams: StdMutex<HashMap<i64, StreamState>>,
    /// 启动与关闭串行化；握手期间不能持有 inner 锁。
    starting: Mutex<()>,
    /// 是否已完成握手（`init` 成功）。
    ///
    /// 与 `running` 分开：进程起来但握手失败时，`running` 为真而 `ready`
    /// 为假 —— 此时 `ensure()` 必须重试而不是把空 provider 列表当成结果。
    ready: AtomicBool,
    /// 退避截止时间（连续崩溃后到点之前不再 spawn）。
    retry_after: StdMutex<Option<std::time::Instant>>,
    /// 连续崩溃次数。
    crashes: StdMutex<u32>,
    /// 是否已停用（崩溃太多次）。
    disabled: AtomicBool,
    /// 主动关闭后该 Host 不得复活；重新启用由管理层创建新 Host。
    closing: AtomicBool,
    /// 子进程已启动且未确认通信中断；只有同代读循环可清除此标志。
    running: AtomicBool,
    /// 尚未恢复的失败原因。
    last_error: StdMutex<Option<String>>,
    /// 已加载的 provider（`init` 之后填）。
    providers: StdMutex<Vec<proto::ProviderInfo>>,
    /// 回调处理器（网关侧实现，见 [`GatewayCallbacks`]）。
    callbacks: Arc<dyn GatewayCallbacks>,
}

/// 子进程与它的管道。
struct Inner {
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    run: Arc<process::ProcessRun>,
}

struct PendingCall {
    tx: oneshot::Sender<Result<serde_json::Value, proto::WireError>>,
    _lease: Option<Arc<activity::UsageLease>>,
}

/// 一条在途流的状态。
///
/// ── 为什么用无界通道而不是有界 ──────────────────────────────
/// 读循环**不能** await（见模块头的死锁防线），有界通道满时只能丢帧或阻塞，
/// 两者都不可接受。真正限制内存的是**插件侧的额度窗口**：网关不还
/// `credit`，插件最多再推一个窗口（512 KiB）就停住。所以无界通道里
/// 最多积压一个窗口的字节 —— 背压来自协议，不来自通道。
struct StreamState {
    /// 把 head / chunk / 收尾事件投给消费者。
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    _lease: Arc<activity::UsageLease>,
    closed: bool,
}

/// 读循环投给流消费者的事件。
///
/// ── 为什么 head 与 chunk 共用一条通道 ────────────────────────
/// 曾经把 head 单独放一条 oneshot，结果是：插件在发 head **之前**失败
/// （凭证缺失、上游连不上）时，网关只盯着 head 通道，插件给出的错误落在
/// 另一条通道里没人看 —— 客户端等满「响应头超时」才收到一句「超时」，
/// 而真正的原因（插件已经说清楚了）被丢掉。一条通道天然保证「谁先到就
/// 先看到」，也保证 chunk 严格排在 head 之后。
pub(crate) enum StreamEvent {
    /// 上游响应头（插件只发一次；网关据此决定状态码与协议）。
    Head(proto::StreamHead),
    /// 一段上游字节（已是 OpenAI chat SSE 形态）。
    Chunk(Vec<u8>),
    /// 插件进程没了 / 帧坏了：让消费者以 IO 错误收尾，而不是永远挂着。
    Failed(String),
    /// 插件给出的最终应答（`result: null` 或协议层错误）。
    ///
    /// 与 `Failed` 的区别：这是插件**自己说的**（它知道自己为什么失败），
    /// `Failed` 是网关替它说的（它已经没法说话了）。
    Done(Result<serde_json::Value, proto::WireError>),
}

/// 一次流式调用的句柄（`conversation` 用）。
pub(crate) struct StreamCall {
    /// 本次调用的 RPC id（归还额度、取消时点名用）。
    pub id: i64,
    /// head / chunk / 收尾事件（顺序即协议顺序）。
    pub events: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
}

const CALLBACK_FORBIDDEN: &str = "forbidden";

impl Host {
    /// 建一个宿主（不启动进程）。
    pub fn new(options: HostOptions, callbacks: Arc<dyn GatewayCallbacks>) -> Self {
        Self {
            options,
            activity: Arc::new(activity::Activity::default()),
            inner: Mutex::new(None),
            lifecycle: StdMutex::new(()),
            generation: AtomicU64::new(0),
            active: StdMutex::new(None),
            pending: StdMutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            streams: StdMutex::new(HashMap::new()),
            starting: Mutex::new(()),
            ready: AtomicBool::new(false),
            retry_after: StdMutex::new(None),
            crashes: StdMutex::new(0),
            disabled: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            running: AtomicBool::new(false),
            last_error: StdMutex::new(None),
            providers: StdMutex::new(Vec::new()),
            callbacks,
        }
    }

    pub(crate) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) && self.running.load(Ordering::Acquire)
            && !self.closing.load(Ordering::Acquire)
    }

    /// 当前状态（给界面）。
    pub fn status(&self) -> HostStatus {
        let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
        let providers = self
            .providers
            .lock()
            .map(|list| list.iter().map(|p| p.id.0.clone()).collect())
            .unwrap_or_default();
        HostStatus {
            // 这里用 try_lock 之外的方式判断「在跑」：`inner` 是 tokio Mutex，
            // 同步上下文不能 await。改用一个原子标志 —— 见 `running` 字段。
            running: self.running.load(Ordering::Acquire),
            providers,
            crashes: self.crashes.lock().map(|c| *c).unwrap_or(0),
            disabled: self.disabled.load(Ordering::Acquire),
            last_error: self.last_error.lock().ok().and_then(|e| e.clone()),
        }
    }


    /// 唤醒一个等待中的调用。
    ///
    /// ── 为什么先查流式表 ────────────────────────────────────────
    /// 流式调用的最终应答必须**排在它自己的 chunk 之后**（见 [`StreamEvent`]），
    /// 所以它不进 `pending` 表，而是投进那条流的事件通道。两个表按 id 互斥
    /// （`open_stream` 只往流式表登记、`call` 只往 pending 登记），先查哪个
    /// 都不影响正确性；先查流式表是因为它的条目在流跑完之前一定存在。
    fn settle(&self, frame: proto::Frame) {
        let outcome = match frame.error {
            Some(err) => Err(err),
            None => Ok(frame.result.unwrap_or(serde_json::Value::Null)),
        };
        let streamed = {
            let guard = self.streams.lock();
            match guard {
                Ok(mut guard) => guard.remove(&frame.id),
                Err(_) => None,
            }
        };
        if let Some(stream) = streamed {
            let _ = stream.tx.send(StreamEvent::Done(outcome));
            return;
        }
        let waiter = self.pending.lock().ok().and_then(|mut map| map.remove(&frame.id));
        if let Some(call) = waiter {
            let _ = call.tx.send(outcome);
        }
    }

    /// 处理插件发来的通知 / 流式中间帧。
    ///
    /// ── 为什么流式帧也走这里（而不在 read_loop 里单独写）──────────
    /// 三类事件（`log` / `head` / `chunk`）的处置都是「查表 → 投递」，
    /// 没有 await，放在同一个函数里能一眼看出读循环到底做了多少事。
    fn handle_event(&self, frame: proto::Frame) {
        match frame.event.as_deref() {
            Some("log") => {
                let level = frame.level.unwrap_or_else(|| "info".to_string());
                let message = frame.message.unwrap_or_default();
                self.callbacks.log_context(level, message, frame.params.unwrap_or(serde_json::Value::Null));
            }
            Some("head") => self.deliver_head(frame),
            Some("chunk") => self.deliver_chunk(frame),
            // `end` 等未来事件：旧网关忽略，不报错（向前兼容）
            _ => {}
        }
    }

    /// 投递 `head`（上游响应头）。
    fn deliver_head(&self, frame: proto::Frame) {
        let status = frame.status.unwrap_or(200);
        let headers = frame.headers.unwrap_or_default();
        // 协议名按线上升形态（snake_case）解。解不出就按 Chat —— 那正是
        // 「网关不认识这个协议」的安全落点（与 `UpstreamKind` 的默认值同义）。
        let upstream = frame
            .upstream
            .as_deref()
            .and_then(|name| {
                serde_json::from_value::<proto::UpstreamKind>(serde_json::Value::String(
                    name.to_string(),
                ))
                .ok()
            })
            .unwrap_or_default();
        let guard = match self.streams.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if let Some(state) = guard.get(&frame.id).filter(|state| !state.closed) {
            // 插件重复发 head 是协议违规，消费者侧按「只认第一条」处理
            let _ = state.tx.send(StreamEvent::Head(proto::StreamHead { status, headers, upstream }));
        }
    }

    /// 投递 `chunk`（一段上游字节）。
    fn deliver_chunk(&self, frame: proto::Frame) {
        let Some(data) = frame.data.as_deref() else { return };
        let bytes = match proto::decode_chunk(data) {
            Ok(bytes) => bytes,
            Err(e) => {
                // base64 坏了 = 这个进程不能信任后续字节边界，与坏帧同等处置
                logging::log("Plugin", &format!("流式帧的 chunk 无法解码: {e}"));
                self.fail_stream(frame.id, format!("插件发出的 chunk 无法解码: {e}"));
                return;
            }
        };
        let guard = match self.streams.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if let Some(state) = guard.get(&frame.id).filter(|state| !state.closed) {
            // 发送失败 = 消费者已经不在了（客户端断开）。不在这里 abort：
            // 消费者 drop 时 `RpcStream` 的 Drop 会发 abort，那条路统一。
            let _ = state.tx.send(StreamEvent::Chunk(bytes));
        }
    }

    /// 让一条在途流以失败收尾（插件崩了 / 帧坏了）。
    fn fail_stream(&self, id: i64, message: String) {
        let guard = match self.streams.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if let Some(state) = guard.get(&id) {
            let _ = state.tx.send(StreamEvent::Failed(message));
        }
    }

    /// 发一个调用并等应答。
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        self.call_coded(method, params, timeout).await.map_err(|error| error.message)
    }

    /// 与 [`Self::call`] 同一件事，但**保留插件的错误分类**（`WireError.code`）。
    ///
    /// ── 为什么需要这一版 ────────────────────────────────────────
    /// 有一类错误的**处置**不在文案里而在分类里：「本地前置条件不成立」
    /// （`LOCAL_PRECONDITION`）要求编排层换下一个候选家顺延、选路暂时绕开
    /// 这一家，而其它错误要终止整份请求。只拿 message 的话，网关只能靠文案
    /// 前缀去猜 —— 插件改一个标点就会让顺延静默失效。
    pub async fn call_coded(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, proto::WireError> {
        let lease = if matches!(method, proto::methods::gateway::INIT | proto::methods::gateway::SHUTDOWN | proto::methods::gateway::ABORT) {
            None
        } else {
            Some(self.activity.acquire().ok_or_else(|| proto::WireError::with_code(
                "插件正在切换或已关闭", "plugin_updating",
            ))?)
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let run = {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            let run = self.current_run().filter(|run| self.is_current_run(run)
                && self.running.load(Ordering::Acquire) && !run.failed.load(Ordering::Acquire)
                && (!run.is_expected_exit() || method == proto::methods::gateway::SHUTDOWN))
                .ok_or_else(|| proto::WireError::with_code("插件进程不可用", proto::WireError::INTERNAL))?;
            self.pending.lock().map_err(|_| proto::WireError::with_code("回调表被污染", proto::WireError::INTERNAL))?
                .insert(id, PendingCall { tx, _lease: lease });
            run
        };
        let frame = proto::Frame {
            id,
            method: Some(method.to_string()),
            params: Some(params),
            ..Default::default()
        };
        if let Err(e) = self.write_for_run(frame, run).await {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&id);
            }
            // ── 帧超限归「本地前置失败」（issue #379）──────────────────
            // 「请求体太大、这一帧写不出去」是**本地确定性的前置失败**：换账号、
            // 重试、换家都不会变好，只会白等一轮。仓库里已有 556
            // （`readiness::LOCAL_PRECONDITION_STATUS`）专门表达这一档，编排层
            // 据此跳过重试（见 `provider_loop` 对 556 的处理）。归到 INTERNAL
            // 会让一次注定失败的请求被当成上游抖动反复重试。
            let code = if is_frame_too_large(&e) {
                proto::WireError::LOCAL_PRECONDITION
            } else {
                proto::WireError::INTERNAL
            };
            return Err(proto::WireError::with_code(
                format!("写插件失败: {e}"),
                code,
            ));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err(proto::WireError::with_code(
                "插件没有应答（宿主已退出）",
                proto::WireError::INTERNAL,
            )),
            Err(_) => {
                // 超时只结束网关等待；插件未回终态前保留租约，不把未知执行结果当空闲。
                if let Ok(mut map) = self.pending.lock() {
                    if map.get(&id).is_some_and(|call| call._lease.is_none()) { map.remove(&id); }
                }
                Err(proto::WireError::with_code(
                    format!("插件调用超时（{method}）"),
                    proto::WireError::INTERNAL,
                ))
            }
        }
    }

    /// 写一帧给插件。
    async fn write(&self, frame: proto::Frame) -> std::io::Result<()> {
        let run = self.current_run().ok_or_else(process::disconnected)?;
        self.write_for_run(frame, run).await
    }

    async fn write_for_run(&self, frame: proto::Frame, run: Arc<process::ProcessRun>) -> std::io::Result<()> {
        let stdin = {
            let guard = self.inner.lock().await;
            let inner = guard.as_ref().filter(|inner| Arc::ptr_eq(&inner.run, &run))
                .ok_or_else(process::disconnected)?;
            inner.stdin.clone()
        };
        let mut stdin = stdin.lock_owned().await;
        // write_all 可能已经写出半帧；调用方取消不能打断共享管道的帧边界。
        tokio::spawn(async move {
            if run.retired.load(Ordering::Acquire) || run.failed.load(Ordering::Acquire) {
                return Err(process::disconnected());
            }
            proto::write_frame(&mut *stdin, &frame).await
        }).await.map_err(|error| std::io::Error::other(format!("插件写入任务未完成：{error}")))?
    }


    /// 发起流式调用；head、chunk 和最终应答共用专属通道，保持帧顺序。
    pub(crate) async fn open_stream(
        self: &Arc<Self>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<StreamCall, String> {
        let lease = self.activity.acquire().ok_or_else(|| "插件正在切换或已关闭".to_string())?;
        self.ensure().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

        let run = {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            let run = self.current_run().filter(|run| self.is_current_run(run) && self.ready()
                && !run.failed.load(Ordering::Acquire) && !run.is_expected_exit())
                .ok_or_else(|| "插件进程不可用".to_string())?;
            self.streams.lock().map_err(|_| "流式表被污染".to_string())?
                .insert(id, StreamState { tx: event_tx, _lease: lease, closed: false });
            run
        };

        let (written, write_result) = oneshot::channel();
        let mut opening = activity::OpeningStream {
            host: self.clone(), id: Some(id), written: Some(write_result),
        };
        let frame = proto::Frame {
            id,
            method: Some(method.to_string()),
            params: Some(params),
            ..Default::default()
        };
        let host = self.clone();
        let write = crate::spawn_task(async move {
            let result = host.write_for_run(frame, run).await;
            let _ = written.send(result.is_ok());
            result
        }).await.map_err(|error| std::io::Error::other(format!("插件写入任务中断：{error}")))
            .and_then(|result| result);
        if let Err(e) = write {
            if let Ok(mut guard) = self.streams.lock() {
                guard.remove(&id);
            }
            // 流式入口的返回类型是 `String`（没有错误码通道），所以这里只在
            // 超限时把「这是本地问题、重试没用」写进文案 —— 调用方据此展示，
            // 不再额外引入一个只在这一处生效的错误类型。
            if is_frame_too_large(&e) {
                return Err(format!("写插件失败: {e}（请求体过大，换账号或重试都不会成功）"));
            }
            return Err(format!("写插件失败: {e}"));
        }
        opening.id = None;
        Ok(StreamCall { id, events: event_rx })
    }

    /// 消费端结束不代表插件已完成；登记与租约由终态应答或进程退出释放。
    pub fn close_stream(&self, id: i64) -> bool {
        if let Ok(mut guard) = self.streams.lock() {
            if let Some(stream) = guard.get_mut(&id) {
                stream.closed = true;
                return true;
            }
        }
        false
    }

    /// 取消一条在途的流式调用。
    pub async fn abort(&self, id: i64) {
        let _ = self.call_coded(
            proto::methods::gateway::ABORT,
            serde_json::json!({ "id": id }),
            Duration::from_secs(10),
        ).await;
    }

    /// 归还背压额度。
    pub async fn credit(&self, id: i64, n: usize) {
        let frame = proto::Frame {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            method: Some(proto::methods::gateway::CREDIT.to_string()),
            params: Some(serde_json::json!({ "id": id, "n": n })),
            ..Default::default()
        };
        let _ = self.write(frame).await;
    }
}

impl HostStatus {
    /// 界面上的状态文案（中文源语言，前端做 i18n）。
    pub fn describe(&self) -> String {
        if self.disabled {
            return format!(
                "插件包已停用（连续崩溃 {} 次）：{}",
                self.crashes,
                self.last_error.as_deref().unwrap_or("原因未知")
            );
        }
        if self.running {
            return format!("插件运行中，已加载 {} 家提供商", self.providers.len());
        }
        match &self.last_error {
            Some(error) => format!("插件未运行：{error}"),
            None => "插件未启动".to_string(),
        }
    }
}
