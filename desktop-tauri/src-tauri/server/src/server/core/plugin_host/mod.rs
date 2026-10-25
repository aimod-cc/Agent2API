//! 提供商插件宿主：子进程监管 + RPC 客户端。
//!
//! ── 这个模块在架构里的位置 ──────────────────────────────────
//! 它是**网关侧**的插件基础设施，替代原本「provider 实现编译进同一个进程」
//! 的那条路。上层（`providers::adapter::adapter_for`）只看见
//! [`crate::server::core::providers::adapter::ProviderAdapter`]；这个模块
//! 提供 [`RpcAdapter`]（那个 trait 的远程实现）与 [`Host`]（进程生命周期）。
//!
//! ── 进程模型：一个宿主跑全部 provider ────────────────────────
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
//!   - 写端是一把 Mutex（不是 channel）：channel 会在写满时阻塞发送方，
//!     而发送方可能就是读循环 —— 那又是一条死锁。

mod bootstrap;
pub mod catalog;
mod errors;
mod install;
mod management;
pub mod operations;
mod packages;
mod pack_view;
mod registry;
mod rpc;
mod rpc_cache;
pub mod schedules;
pub mod settings;
mod stream;
pub mod update;

#[cfg(test)]
mod tests;

pub use bootstrap::{
    reload as reload_host, set_loopback_port, spawn as spawn_bootstrap, start as bootstrap_host,
    StoreCallbacks,
};
pub use install::{
    install_zip, list_packs, resolve_ui_file, set_pack_enabled, set_provider_enabled, uninstall,
    InstalledPack, PackProviderSummary, PackRegistry, PackSummary, PackUiSummary,
};
pub use registry::{install as install_provider, installed_ids, lookup, remove, clear};
pub use registry::{serves as serves_provider};
pub use rpc::{HostOptions, RpcAdapter};
pub(crate) use rpc_cache::meta as model_catalog_meta;
pub use management::snapshot as management_snapshot;
pub(crate) use packages::prepare as prepare_pack;

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

pub(crate) use stream::RpcStream;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::sync::{oneshot, Mutex};

use crate::server::logging;

/// 进程级宿主句柄。
///
/// ── 为什么不是 `OnceLock<Arc<Host>>` ─────────────────────────
/// 插件可以在运行期安装 / 升级 / 卸载（`api::plugins`），宿主必须能**换一个**。
/// `OnceLock` 只允许装一次，换宿主就得泄漏旧值 —— 而旧宿主还占着一个子进程。
/// 改成「一把 std Mutex 包住 `Option`」：读多写极少，临界区里没有 await。
static HOST: OnceLock<StdMutex<Option<Arc<Host>>>> = OnceLock::new();

fn host_slot() -> &'static StdMutex<Option<Arc<Host>>> {
    HOST.get_or_init(|| StdMutex::new(None))
}

/// 装入宿主（覆盖旧的 —— 重装 / 升级走这条）。
pub fn install(host: Arc<Host>) {
    if let Ok(mut slot) = host_slot().lock() {
        *slot = Some(host);
    }
}

/// 摘掉宿主句柄（关闭后调，避免后续调用拿到一个已死的宿主）。
pub fn detach() {
    if let Ok(mut slot) = host_slot().lock() {
        *slot = None;
    }
}

/// 取当前宿主（没装过 = 插件架构未启用）。
pub fn current() -> Option<Arc<Host>> {
    host_slot().lock().ok().and_then(|slot| slot.clone())
}

/// 当前已加载的 provider 申报了哪些**管理面板**（去重）。
///
/// ── 界面为什么需要它 ────────────────────────────────────────
/// 伪装头编辑器、令牌池、成长任务、每日保活这些面板的实现在插件里，但渲染
/// 在网关界面。界面原先按 provider id 写死，插件没装时照样显示、点进去是
/// 一屏「读取失败」。改为按这里的申报渲染：没装插件 → 空表 → 不画面板。
///
/// 宿主没起（没装包 / 崩溃中）时返回空表，与「一个面板都没有」同义。
pub fn admin_panels() -> Vec<proto::AdminPanel> {
    let Some(host) = current() else {
        return Vec::new();
    };
    let providers = host
        .providers
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default();
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
    let Some(host) = current() else {
        return false;
    };
    host.providers
        .lock()
        .map(|list| {
            list.iter().any(|info| {
                info.id.as_str() == provider && info.admin_panels.contains(&panel)
            })
        })
        .unwrap_or(false)
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
    let Some(host) = current() else {
        return map;
    };
    let providers = host
        .providers
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default();
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
    let host = current()?;
    let guard = host.providers.lock().ok()?;
    let found = guard
        .iter()
        .find(|info| info.id.as_str() == provider)
        .and_then(|info| info.account_features.clone());
    drop(guard);
    found
}

/// 某一家申报的「有没有续期能力」（没装 / 没这一家 = `false`）。
///
/// ── 为什么要有这个 store 层的读数入口 ──────────────────────────
/// 「手动刷新 Token」这个入口由界面按公开视图的 `hasRefreshToken` 决定是否
/// 渲染，而公开视图在 `account_store` 层拼装 —— 那里拿不到适配器（插件家
/// 的 id 也不在 `ProviderKind` 枚举里）。`plugin_update_secret` 那边的
/// `hasRefreshToken` 原先恒写 `false`，于是**插件家一律没有刷新按钮**，
/// 哪怕插件明明白白申报了 `refresh: true` 并且实现了 `token.refresh`。
///
/// 与 [`account_features_of`] 同一形态：读握手带回的 `ProviderInfo`。
pub fn refresh_capable(provider: &str) -> bool {
    let Some(host) = current() else {
        return false;
    };
    let Ok(guard) = host.providers.lock() else {
        return false;
    };
    guard
        .iter()
        .find(|info| info.id.as_str() == provider)
        .map(|info| info.capabilities.refresh)
        .unwrap_or(false)
}

/// 某一家申报的签到能力（没申报 = `None`）。
pub fn checkin_of(provider: &str) -> Option<proto::CheckinInfo> {
    let host = current()?;
    let guard = host.providers.lock().ok()?;
    let found = guard
        .iter()
        .find(|info| info.id.as_str() == provider)
        .and_then(|info| info.checkin.clone());
    drop(guard);
    found
}

/// 全部申报的签到能力（`provider id → 申报`）。
///
/// 只收申报过的家 —— 旧插件不申报时不在表里，调用方走内置兜底
/// （`Capabilities::checkin`），升级因此是渐进的。
pub fn checkin_features() -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    let Some(host) = current() else {
        return map;
    };
    let providers = host
        .providers
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default();
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
    /// 最近一次失败原因。
    pub last_error: Option<String>,
}

/// 一个提供商插件宿主（对应一个已安装的包）。
pub struct Host {
    /// 启动参数。
    options: HostOptions,
    /// 子进程与管道（`None` = 没在跑）。
    inner: Mutex<Option<Inner>>,
    /// 回调应答的等待者（插件 → 网关的请求）。
    ///
    /// ── 为什么用 std Mutex ────────────────────────────────────
    /// 这个表只在「读循环派发」与「发回调」两处短暂访问，临界区里没有
    /// await。用 tokio 的 Mutex 会让读循环多一次 await 点（而读循环的
    /// 原则是「不 await」）。
    pending: StdMutex<HashMap<i64, oneshot::Sender<Result<serde_json::Value, proto::WireError>>>>,
    /// 下一个回调 id。
    next_id: AtomicI64,
    /// 在途的流式调用（`abort` 与额度归还都按它找）。
    streams: StdMutex<HashMap<i64, StreamState>>,
    /// 启动/重启的串行化锁。
    ///
    /// ── 为什么不能只靠 `inner` ──────────────────────────────────
    /// 握手是一次 `call()`，它要写 stdin（拿 `inner`）。若启动路径仍持着
    /// `inner`，两者就锁死了自己。这把锁把「启动」串行化，`inner` 只在
    /// 真正 spawn 与写帧时短暂持有。
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
    /// 是否正在主动关闭。
    ///
    /// ── 为什么需要它 ──────────────────────────────────────────
    /// `shutdown()` 会让插件进程退出，读循环随之看到 EOF。若不区分「我们
    /// 让它退的」与「它自己崩了」，每次正常关停都会给崩溃计数 +1 ——
    /// 重启五次之后这个包就被永久停用了（用户完全不知道为什么）。
    closing: AtomicBool,
    /// 子进程是否在跑。
    ///
    /// ── 为什么单独一个原子而不是看 `inner` ──────────────────────
    /// `inner` 是 tokio Mutex，同步的 `status()` 不能 await 它；而界面
    /// 每秒都在轮询状态。用一个独立的原子标志，读循环与启动路径各自维护
    /// 它 —— 两者的一致性由「只在 `start` 成功之后置真、只在读循环退出时
    /// 置假」这条规则保证。
    running: AtomicBool,
    /// 最近一次失败原因。
    last_error: StdMutex<Option<String>>,
    /// 已加载的 provider（`init` 之后填）。
    providers: StdMutex<Vec<proto::ProviderInfo>>,
    /// 回调处理器（网关侧实现，见 [`GatewayCallbacks`]）。
    callbacks: Arc<dyn GatewayCallbacks>,
}

/// 子进程与它的管道。
struct Inner {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
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

/// 网关侧要实现的回调（插件的 `store.*` / `settings.*` / `log` / `notify`）。
///
/// ── 为什么是 trait 而不是直接调 account_store ─────────────────
/// 这个模块属于「插件基础设施」，不该认识账号库的具体形状；而账号库在
/// `core::account_store`，两者互相依赖会形成循环。用 trait 隔开之后，
/// 接线在 `bootstrap` 里做一次，测试时也能塞假实现。
pub trait GatewayCallbacks: Send + Sync + 'static {
    /// `store.get`
    fn store_get(
        &self,
        provider: String,
        account: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    /// `store.list`
    fn store_list(
        &self,
        provider: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    /// `store.update`（比较再写）
    fn store_update(
        &self,
        account: String,
        expect: serde_json::Value,
        secret: serde_json::Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    /// `store.mark_limited`
    fn store_mark_limited(
        &self,
        account: String,
        model: String,
        reset_at: Option<i64>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>,
    >;

    /// `store.ledger_get`
    fn ledger_get(
        &self,
        account: String,
        kind: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    /// `store.ledger_put`
    fn ledger_put(
        &self,
        account: String,
        kind: String,
        value: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>,
    >;

    /// `settings.get`
    fn settings_get(
        &self,
        provider: String,
        key: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    /// `settings.set`
    fn settings_set(
        &self,
        provider: String,
        key: String,
        value: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>,
    >;

    /// 插件日志（进运行日志页）。
    fn log(&self, level: String, message: String);

    /// 插件通知界面（账号状态变化、进度）。
    fn notify(&self, kind: String, payload: serde_json::Value);
}

impl Host {
    /// 建一个宿主（不启动进程）。
    pub fn new(options: HostOptions, callbacks: Arc<dyn GatewayCallbacks>) -> Self {
        Self {
            options,
            inner: Mutex::new(None),
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

    /// 当前状态（给界面）。
    pub fn status(&self) -> HostStatus {
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

    /// 确保子进程在跑且已完成握手，返回可用的 provider 列表。
    ///
    /// ── 幂等 ──────────────────────────────────────────────────
    /// 已握手就直接返回缓存的 provider 列表。崩溃之后调用它会重新 spawn
    /// （受退避与次数限制）。
    pub async fn ensure(self: &Arc<Self>) -> Result<Vec<proto::ProviderInfo>, String> {
        if self.disabled.load(Ordering::Acquire) {
            return Err(self
                .last_error
                .lock()
                .ok()
                .and_then(|e| e.clone())
                .unwrap_or_else(|| "插件包反复崩溃，已停用".to_string()));
        }
        if self.ready.load(Ordering::Acquire) {
            return Ok(self.providers.lock().map(|p| p.clone()).unwrap_or_default());
        }
        self.start().await
    }

    /// 优雅关闭（网关退出时调）。
    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        let mut guard = self.inner.lock().await;
        if let Some(inner) = guard.as_mut() {
            let frame = proto::Frame {
                id: 0,
                method: Some(proto::methods::gateway::SHUTDOWN.to_string()),
                ..Default::default()
            };
            let _ = proto::write_frame(&mut inner.stdin, &frame).await;
            // 给它 2 秒体面退出，然后杀
            let _ = tokio::time::timeout(Duration::from_secs(2), inner.child.wait()).await;
        }
        *guard = None;
        self.running.store(false, Ordering::Release);
        self.ready.store(false, Ordering::Release);
    }

    /// 启动子进程并握手。
    ///
    /// ── 锁的顺序（改之前务必读完）────────────────────────────────
    /// `starting` 串行化整条启动路径；`inner` 只在「spawn 落库」与写帧时
    /// 短暂持有。**绝不能持着 `inner` 去 `call()`** —— `call()` 内部要写
    /// stdin，也就是再拿一次 `inner`；tokio 的 Mutex 不可重入，那会直接
    /// 死锁（曾经的 bug，7 个集成测试一起挂死就是它）。
    async fn start(self: &Arc<Self>) -> Result<Vec<proto::ProviderInfo>, String> {
        let _serial = self.starting.lock().await;
        // 排队等锁期间可能已经被别人拉起来了
        if self.ready.load(Ordering::Acquire) {
            return Ok(self.providers.lock().map(|p| p.clone()).unwrap_or_default());
        }
        if self.disabled.load(Ordering::Acquire) {
            return Err(self
                .last_error
                .lock()
                .ok()
                .and_then(|e| e.clone())
                .unwrap_or_else(|| "插件包反复崩溃，已停用".to_string()));
        }
        // 退避窗口内不 spawn（避免一个必崩的包把 CPU 打满）
        if let Ok(guard) = self.retry_after.lock() {
            if let Some(deadline) = *guard {
                if std::time::Instant::now() < deadline {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    return Err(format!("插件包刚崩溃过，{:.1} 秒后重试", left.as_secs_f32()));
                }
            }
        }

        // 上一个进程可能还在（握手失败后没清）：先收掉，免得泄漏句柄
        {
            let mut guard = self.inner.lock().await;
            if let Some(mut old) = guard.take() {
                let _ = old.child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(2), old.child.wait()).await;
            }
        }
        self.running.store(false, Ordering::Release);

        let mut command = tokio::process::Command::new(&self.options.exe);
        command
            .args(&self.options.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // ── Windows 上不要弹控制台窗口 ────────────────────────
        // 桌面端是 GUI 子系统程序，spawn 一个控制台子进程会闪一个黑框。
        //
        // 必须是**编译期**的 `#[cfg(windows)]`，不能写成
        // `if cfg!(windows) { … }`：`creation_flags` 这个方法是 tokio 只为
        // Windows 目标提供的，在 macOS / Linux 上它根本不存在 —— 运行期判断
        // 两边都要参与编译，于是非 Windows 平台直接编译失败
        // （error[E0599]: method not found in `&mut tokio::process::Command`）。
        #[cfg(windows)]
        {
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        if let Some(dir) = &self.options.cwd {
            command.current_dir(dir);
        }

        let mut child = command.spawn().map_err(|e| {
            let message = format!("启动插件进程失败（{}）: {e}", self.options.exe.display());
            self.note_failure(&message);
            message
        })?;
        let stdin = child.stdin.take().ok_or_else(|| "插件进程没有 stdin".to_string())?;
        let stdout = child.stdout.take().ok_or_else(|| "插件进程没有 stdout".to_string())?;
        let stderr = child.stderr.take();

        // stderr 单独一个 task：那是「插件崩了」的兜底诊断通道，
        // 混进业务日志会淹没它。
        if let Some(stderr) = stderr {
            let exe = self.options.exe.display().to_string();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = proto::frame::buffered(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logging::console_line("plugin", &format!("[{exe}] {line}"));
                }
            });
        }

        self.running.store(true, Ordering::Release);
        // 锁只在这里短暂持有，随后立刻释放再握手
        *self.inner.lock().await = Some(Inner { child, stdin });

        // 读循环：独立 task，持有 Arc<Host>
        let host = self.clone();
        let reader = proto::frame::buffered(stdout);
        tokio::spawn(async move {
            host.read_loop(reader).await;
        });

        // 握手
        let env = proto::HostEnv {
            data_dir: self.options.data_dir.clone(),
            host_version: self.options.host_version.clone(),
            target: self.options.target.clone(),
            proxy: self.options.proxy.clone(),
            debug_traffic: self.options.debug_traffic,
            loopback_port: self.options.loopback_port,
        };
        let params = serde_json::json!({ "env": env });
        match self
            .call(proto::methods::gateway::INIT, params, self.options.init_timeout)
            .await
        {
            Ok(value) => match serde_json::from_value::<Vec<proto::ProviderInfo>>(value) {
                Ok(list) => {
                    if let Ok(mut guard) = self.providers.lock() {
                        *guard = list.clone();
                    }
                    // 握手成功：崩溃计数与退避都归零
                    if let Ok(mut crashes) = self.crashes.lock() {
                        *crashes = 0;
                    }
                    if let Ok(mut retry) = self.retry_after.lock() {
                        *retry = None;
                    }
                    self.ready.store(true, Ordering::Release);
                    Ok(list)
                }
                Err(e) => {
                    let message = format!("插件返回的 provider 列表无法解析: {e}");
                    self.note_failure(&message);
                    Err(message)
                }
            },
            Err(e) => {
                let message = format!("插件握手失败: {e}");
                self.note_failure(&message);
                Err(message)
            }
        }
    }

    /// 读循环：只派发，不 await 业务。
    async fn read_loop(
        self: Arc<Self>,
        mut reader: tokio::io::BufReader<tokio::process::ChildStdout>,
    ) {
        loop {
            match proto::read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    match frame.kind() {
                        proto::FrameKind::Reply | proto::FrameKind::Error => {
                            // 这是插件对**我们发起的调用**的应答
                            self.settle(frame);
                        }
                        proto::FrameKind::Request => {
                            // 插件回调网关（store.* 等）—— 起 task 处理
                            let host = self.clone();
                            tokio::spawn(async move {
                                host.serve_callback(frame).await;
                            });
                        }
                        proto::FrameKind::Event => {
                            // 插件的主动通知（log / notify）
                            self.handle_event(frame);
                        }
                        proto::FrameKind::Unknown => {}
                    }
                }
                Ok(None) => {
                    self.note_exit("插件进程关闭了 stdout");
                    return;
                }
                Err(e) => {
                    self.note_exit(&format!("插件帧格式错误: {e}"));
                    return;
                }
            }
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
                Ok(guard) => guard.get(&frame.id).map(|state| state.tx.clone()),
                Err(_) => None,
            }
        };
        if let Some(tx) = streamed {
            let _ = tx.send(StreamEvent::Done(outcome));
            return;
        }
        let waiter = self.pending.lock().ok().and_then(|mut map| map.remove(&frame.id));
        if let Some(tx) = waiter {
            let _ = tx.send(outcome);
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
                self.callbacks.log(level, message);
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
        if let Some(state) = guard.get(&frame.id) {
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
        if let Some(state) = guard.get(&frame.id) {
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

    /// 处理插件的回调请求。
    async fn serve_callback(&self, frame: proto::Frame) {
        let Some(method) = frame.method.clone() else { return };
        let id = frame.id;
        let params = frame.params.unwrap_or(serde_json::Value::Null);
        let text = |key: &str| {
            params.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string()
        };
        let value = |key: &str| params.get(key).cloned().unwrap_or(serde_json::Value::Null);

        let outcome: Result<serde_json::Value, proto::WireError> = match method.as_str() {
            proto::methods::plugin::STORE_GET => {
                self.callbacks.store_get(text("provider"), text("account")).await
            }
            proto::methods::plugin::STORE_LIST => self.callbacks.store_list(text("provider")).await,
            proto::methods::plugin::STORE_UPDATE => {
                self.callbacks
                    .store_update(text("account"), value("expect"), value("secret"))
                    .await
            }
            proto::methods::plugin::STORE_MARK_LIMITED => {
                let reset_at = params.get("reset_at").and_then(serde_json::Value::as_i64);
                self.callbacks
                    .store_mark_limited(text("account"), text("model"), reset_at)
                    .await
                    .map(|()| serde_json::Value::Null)
            }
            proto::methods::plugin::STORE_LEDGER_GET => {
                self.callbacks.ledger_get(text("account"), text("kind")).await
            }
            proto::methods::plugin::STORE_LEDGER_PUT => {
                self.callbacks
                    .ledger_put(text("account"), text("kind"), value("value"))
                    .await
                    .map(|()| serde_json::Value::Null)
            }
            proto::methods::plugin::SETTINGS_GET => {
                self.callbacks.settings_get(text("provider"), text("key")).await
            }
            proto::methods::plugin::SETTINGS_SET => {
                self.callbacks
                    .settings_set(text("provider"), text("key"), value("value"))
                    .await
                    .map(|()| serde_json::Value::Null)
            }
            proto::methods::plugin::LOG => {
                self.callbacks.log(text("level"), text("message"));
                Ok(serde_json::Value::Null)
            }
            proto::methods::plugin::NOTIFY => {
                self.callbacks.notify(text("kind"), value("payload"));
                Ok(serde_json::Value::Null)
            }
            other => Err(proto::WireError::with_code(
                format!("不认识的回调 {other}"),
                proto::WireError::NO_METHOD,
            )),
        };

        let frame = match outcome {
            Ok(result) => proto::Frame { id, result: Some(result), ..Default::default() },
            Err(error) => proto::Frame { id, error: Some(error), ..Default::default() },
        };
        let _ = self.write(frame).await;
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
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending.lock() {
            map.insert(id, tx);
        } else {
            return Err(proto::WireError::with_code(
                "回调表被污染",
                proto::WireError::INTERNAL,
            ));
        }
        let frame = proto::Frame {
            id,
            method: Some(method.to_string()),
            params: Some(params),
            ..Default::default()
        };
        if let Err(e) = self.write(frame).await {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&id);
            }
            return Err(proto::WireError::with_code(
                format!("写插件失败: {e}"),
                proto::WireError::INTERNAL,
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
                if let Ok(mut map) = self.pending.lock() {
                    map.remove(&id);
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
        let mut guard = self.inner.lock().await;
        match guard.as_mut() {
            Some(inner) => proto::write_frame(&mut inner.stdin, &frame).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "插件进程没有在跑",
            )),
        }
    }

    /// 记一次失败（崩溃计数 + 退避判定）。
    fn note_failure(&self, message: &str) {
        if let Ok(mut last) = self.last_error.lock() {
            *last = Some(message.to_string());
        }
        let crashes = self.crashes.lock().map(|mut c| {
            *c += 1;
            *c
        }).unwrap_or(1);
        logging::log("Plugin", &format!("宿主失败（第 {crashes} 次）: {message}"));
        // 指数退避：1s → 2s → 4s → 8s → 16s，上限 60s。
        // 第 1 次也给退避，否则一个「起来就崩」的包会被 ensure() 的调用方
        // 按请求频率反复 spawn（每个请求一次进程启动）。
        let secs = 1u64 << (crashes.saturating_sub(1)).min(6);
        if let Ok(mut retry) = self.retry_after.lock() {
            *retry = Some(std::time::Instant::now() + Duration::from_secs(secs.min(60)));
        }
        if crashes >= 5 {
            self.disabled.store(true, Ordering::Release);
            logging::log("Plugin", "插件包连续崩溃 5 次，已停用；请检查版本或重新安装");
        }
    }

    /// 子进程退出（读循环结束时调）。
    fn note_exit(&self, message: &str) {
        let was_running = self.running.swap(false, Ordering::AcqRel);
        self.ready.store(false, Ordering::Release);
        if let Ok(mut guard) = self.streams.lock() {
            // ── 先让在途流以错误收尾，再清表 ──────────────────────
            // 只清表的话，消费者的通道发送端随条目一起被丢弃，接收端读到
            // `None` —— 那与「上游正常结束」不可区分，客户端会拿到一条
            // 截断但没有错误的流（看起来像模型少说了半句）。
            for (_, state) in guard.iter() {
                let _ = state.tx.send(StreamEvent::Failed(message.to_string()));
            }
            guard.clear();
        }
        // 主动关闭不算失败：否则每次正常关停都给崩溃计数 +1，五次之后
        // 这个包就被永久停用了（见 `closing` 字段的说明）。
        if self.closing.swap(false, Ordering::AcqRel) {
            logging::verbose("Plugin", &format!("插件宿主已关闭（{message}）"));
            return;
        }
        if was_running {
            self.note_failure(message);
        }
    }

    /// 发起一次**流式**调用（`conversation`）。
    ///
    /// ── 与 `call()` 的分工 ──────────────────────────────────────
    /// `call()` 是「一问一答」：投进 pending 表，等一条应答。流式调用有三段
    /// （head → chunk* → 最终应答），中间的帧没有 pending 表可投 —— 它们要
    /// 落到一个**专属通道**里，由消费者边读边还额度。所以这里单独一条路：
    ///
    ///   - `head` 走 oneshot（消费者要先知道状态码才能决定返回什么）；
    ///   - `chunk` 走无界通道（读循环不 await，见 [`StreamState`] 的说明）；
    ///   - 最终应答仍走 `pending`（`result: null` 或 error）。
    ///
    /// ── 为什么 `ensure()` 在这里而不是调用方 ─────────────────────
    /// 与 [`Self::call`] 一致：崩溃之后第一次调用自动拉起子进程。放在这里
    /// 就不会有「某条新加的流式调用忘了 ensure」这种漏。
    pub(crate) async fn open_stream(
        self: &Arc<Self>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<StreamCall, String> {
        self.ensure().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

        {
            let mut guard = self.streams.lock().map_err(|_| "流式表被污染".to_string())?;
            guard.insert(id, StreamState { tx: event_tx });
        }

        let frame = proto::Frame {
            id,
            method: Some(method.to_string()),
            params: Some(params),
            ..Default::default()
        };
        if let Err(e) = self.write(frame).await {
            if let Ok(mut guard) = self.streams.lock() {
                guard.remove(&id);
            }
            return Err(format!("写插件失败: {e}"));
        }
        Ok(StreamCall { id, events: event_rx })
    }

    /// 收掉一条流式调用的登记（消费者结束时调）。
    ///
    /// 只清登记、不发 abort：正常收尾时插件已经在收尾了，多发一个 abort 会
    /// 让「刚好在最后一帧之后到达」的取消被当成客户端断开。
    ///
    /// 不碰 `pending`：流式调用从不往那里登记（见 [`Self::settle`]）。
    pub fn close_stream(&self, id: i64) {
        if let Ok(mut guard) = self.streams.lock() {
            guard.remove(&id);
        }
    }

    /// 取消一条在途的流式调用。
    pub async fn abort(&self, id: i64) {
        let frame = proto::Frame {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            method: Some(proto::methods::gateway::ABORT.to_string()),
            params: Some(serde_json::json!({ "id": id })),
            ..Default::default()
        };
        let _ = self.write(frame).await;
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
