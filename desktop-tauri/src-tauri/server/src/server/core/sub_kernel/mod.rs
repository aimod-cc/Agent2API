//! 订阅内核（mihomo sidecar）：按需下载、配置生成、进程管理、节点同步。
//!
//! ── 为什么单独一个模块（对照 Clash Verge Rev 的 sidecar 形态）──
//! 订阅节点是 ss / vmess / vless / trojan / hysteria2 这类协议，出网层
//! （`core::egress`）只认 http / socks5 —— 内核负责把订阅转换成本地
//! http/socks 监听器（每节点一个端口），出网层照旧连 `127.0.0.1:端口`，
//! 协议代码一行不改。内核本体是 mihomo 官方二进制（GPL-3.0，按需下载、
//! sha256 校验后落 `kernel_dir()` —— 默认 `{config_dir}/proxy-kernel/`，
//! 下载弹窗里能改到别的盘上），与本进程是**两个独立程序**（通过本地 socket
//! 通信，GPL 的「聚合」语义，本项目保持 MIT）。
//!
//! ── 下载按需而不是捆绑 ───────────────────────────────────────
//! 内核 ~20-40MB/平台，捆绑进安装包会让所有用户（多数不用订阅功能）白背
//! 这份体积；按需下载在「订阅管理」第一次点「下载内核」时才拉（GitHub
//! Release 的预编译产物）。出口与目录都由**下载弹窗**在开始前定下并落配置：
//! 出口是选定的那条线路（没选时走直连 → Clash 混合端口兜底的默认链），
//! 目录是 `kernelDir`；sha256 与 pin 死的值对不上就整体拒装。
//!
//! ── 单 worker 串行（为什么不并发 spawn / kill）────────────────
//! 内核操作（下载 / 重建配置 / 重启 / 刷新）互相踩：并发重启会 spawn 出两个
//! 内核抢同一批端口，并发下载与重启会互相看到半截状态。所有命令进一条
//! mpsc 队列、一个 worker 串行消费；API handler 只投命令 + 读快照，界面用
//! 轮询看进度（下载字节 / 运行状态），不引入第二条推送通道。
//!
//! ── 与代理池的关系 ──────────────────────────────────────────
//! 内核拉到订阅后把节点镜像进池（`proxy_pool::sync_subscription_nodes`，
//! `source: "subscription"` 的只读条目，账号按 id 引用）；订阅删了 / 节点
//! 消失了，镜像条目随之摘掉 —— 与 Clash Verge 镜像（`proxy_pool::sync_clash`）
//! 同一套只读语义。端口映射（节点 → 本地端口）持久在内核目录的
//! `kernel-state.json` 里：重启后端口保持稳定，账号引用不会因重启漂移。

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxy_pool;
use crate::server::logging;

pub mod download;
pub mod runtime;

/// 内核版本（pin 死：换版本时改这里并同步 4 份 sha256 —— 不追 latest，
/// 避免上游发版悄悄改行为）
pub const KERNEL_VERSION: &str = "v1.19.32";

/// 内核目录名（`config_dir` 下的子目录，与日志 / updates 同级）
pub const KERNEL_DIR_NAME: &str = "proxy-kernel";

/// 内核的**默认**存放目录：`{config_dir}/proxy-kernel/`
pub fn default_kernel_dir() -> PathBuf {
    crate::paths::config_dir().join(KERNEL_DIR_NAME)
}

/// 内核目录：配置里 `kernelDir` 写了**绝对路径**就用它，否则回落默认目录。
///
/// 「写坏回落而不是报错」与本项目其它目录型配置（`logDir`）同一取向：这里报错
/// 会让内核功能整个不可用，而「手改配置写了个相对路径」最多是配置不规范。
/// 目录本身未必存在 —— 下载时才 `create_dir_all`（改目录后第一次下载就建好）。
pub fn kernel_dir() -> PathBuf {
    crate::server::config::kernel_dir_raw()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(default_kernel_dir)
}

/// 进程级账号存储句柄（bootstrap 时 install 一次）。
///
/// 内核的 lazy listeners（`runtime::allocate_listeners`）要回答「哪些节点该开
/// 监听器」—— 其中**账号引用**那一半的答案在账号存储里，而 worker 是独立任务、
/// 拿不到 `ServerState`。与 `proxy_pool::install` 同一形态：bootstrap 把句柄装
/// 进来，runtime 用它读引用集合。`None`（库不可用）时引用集合按空处理 —— 账号
/// 引用的节点一个都不开，但**测试钉住的节点照常开**（那一半在代理池里，与账号
/// 库无关，见 `runtime::listener_node_keys`），节点仍镜像进池、可恢复。
static STORE: OnceLock<Option<AccountStore>> = OnceLock::new();

/// 接线（`ServerState::bootstrap` 调用）。重复调用无副作用。
pub fn install_store(store: Option<AccountStore>) {
    let _ = STORE.set(store);
}

fn account_store() -> Option<&'static AccountStore> {
    STORE.get().and_then(Option::as_ref)
}

/// 当前平台的发布资产（名字 + sha256 pin）。
///
/// 不支持的平台返回 None（界面提示「暂不支持的平台」；内核是 Go 预编译
/// 二进制，官方 Release 只出这几组）。
pub fn asset_for_current_platform() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some((
            "mihomo-windows-amd64-v1.19.32.zip",
            "1ac84e795b5b915446e20139677fb6cc013a0778e876e8d4c96f994750b4c4a3",
        )),
        ("windows", "aarch64") => Some((
            "mihomo-windows-arm64-v1.19.32.zip",
            "7a4f6e58af9a120920935f2150cf642d945085fdb3e753314505b334234e1d1e",
        )),
        ("macos", "x86_64") => Some((
            "mihomo-darwin-amd64-v1.19.32.gz",
            "b7de9fc46af2ca985ecfb302a6362170e04ff2023dee1e82addc54133e669bc9",
        )),
        ("macos", "aarch64") => Some((
            "mihomo-darwin-arm64-v1.19.32.gz",
            "3312a6780652c622890fd4357c6a853bbf865464fd047ac7b7f52dab8de18652",
        )),
        ("linux", "x86_64") => Some((
            "mihomo-linux-amd64-v1.19.32.gz",
            "8451100836c9eda194331c2babfad490b2faf30cebc1a04b0e76fd8ac2d35d10",
        )),
        ("linux", "aarch64") => Some((
            "mihomo-linux-arm64-v1.19.32.gz",
            "9dd862e28b46ff7d775f169cceebc28deccaa0a9e804237d421cd2571e0caba0",
        )),
        _ => None,
    }
}

/// 内核二进制的期望文件名（下载后统一改名到它，config / 状态文件引用固定名）
pub fn kernel_binary_name() -> &'static str {
    if std::env::consts::OS == "windows" {
        "mihomo.exe"
    } else {
        "mihomo"
    }
}

/// 内核是否已安装（二进制存在即视为已装）
pub fn installed() -> bool {
    kernel_dir().join(kernel_binary_name()).exists()
}

// ─── 一条订阅的输入形态 ─────────────────────────────────────

/// worker 命令携带的订阅清单项（api 层从 `core::subscriptions` 的条目构造）。
///
/// 放在输入里而不是让 worker 回读订阅模块：订阅存储（`core::subscriptions`）
/// 依赖本模块的 `ProviderInput` 类型做「已启用清单」的构造，反向依赖会让
/// 两个模块成环 —— 命令带全量输入，worker 不认识订阅存储。
#[derive(Clone, Debug)]
pub struct ProviderInput {
    /// 订阅 id（同时是 mihomo proxy-provider 的名字前缀：`sub_<id>`）
    pub id: String,
    /// 订阅名（代理池节点条目的命名来源）
    pub name: String,
    /// 订阅链接
    pub url: String,
}

/// 内核状态快照（worker 每步操作后更新；status_json 读它）
pub struct StatusSnapshot {
    pub running: bool,
    pub controller_port: Option<u16>,
    pub listener_count: usize,
    pub last_error: Option<String>,
    /// 各订阅最后一次成功同步：`subId → {name, count, at}`
    pub provider_sync: Map<String, Value>,
    pub download: DownloadProgress,
}

/// 下载进度（状态机：idle → downloading → done / error / canceled）
///
/// `canceled` 是**用户按了「停止下载」**（不是失败）：临时压缩包已删掉，
/// 界面既不报错也不提示重试，回到「可以重新开始」的形态。
pub struct DownloadProgress {
    pub state: &'static str,
    pub received: u64,
    pub total: u64,
    pub error: Option<String>,
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            controller_port: None,
            listener_count: 0,
            last_error: None,
            provider_sync: Map::new(),
            download: DownloadProgress {
                state: "idle",
                received: 0,
                total: 0,
                error: None,
            },
        }
    }
}

static STATUS: Mutex<Option<StatusSnapshot>> = Mutex::new(None);

fn patch_status(f: impl FnOnce(&mut StatusSnapshot)) {
    let mut guard = match STATUS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let snapshot = guard.get_or_insert_with(StatusSnapshot::default);
    f(snapshot);
}

fn with_status<R>(f: impl FnOnce(&StatusSnapshot) -> R) -> R {
    let guard = match STATUS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let default = StatusSnapshot::default();
    f(guard.as_ref().unwrap_or(&default))
}

// ─── worker（单条命令队列，串行消费）────────────────────────

enum Command {
    /// 重建配置 + 启动 / 重启内核（订阅增删改后都走它）
    Restart(Vec<ProviderInput>),
    /// 强制刷新一条订阅的内容（mihomo 的 provider 强制拉取），节点集合
    /// 变了才重建配置
    Refresh { sub_id: String, providers: Vec<ProviderInput> },
    /// 下载内核；带非空 providers 时下完自动拉起
    Download(Vec<ProviderInput>),
    /// 停掉内核并删除已下载的内核（二进制 / 配置 / 端口映射）
    Remove,
    Stop,
}

static SENDER: OnceLock<mpsc::UnboundedSender<Command>> = OnceLock::new();

/// 投递命令；worker 未启动时静默丢弃（bootstrap 顺序保证先 spawn，
/// 这里只防极端时序 —— 丢了命令的代价是「这次操作没生效」，不是崩）。
fn send(command: Command) {
    if let Some(sender) = SENDER.get() {
        let _ = sender.send(command);
    }
}

/// 起 worker（bootstrap 调用一次；重复调用无副作用 —— sender 只 set 一次）。
pub fn spawn_worker() {
    let (tx, mut rx) = mpsc::unbounded_channel::<Command>();
    if SENDER.set(tx).is_err() {
        return;
    }
    crate::spawn_task(async move {
        while let Some(command) = rx.recv().await {
            // 单 worker 串行：每条命令跑完（含内核重启与节点同步）才收下一条
            execute(command).await;
        }
    });
}

async fn execute(command: Command) {
    match command {
        Command::Restart(providers) => {
            patch_status(|snapshot| {
                snapshot.download.error = None;
            });
            if let Err(error) = runtime::run_restart(&providers).await {
                patch_status(|snapshot| {
                    snapshot.last_error = Some(error.clone());
                    snapshot.running = false;
                });
                logging::log("[Subscription]", &format!("❌ 内核启动失败: {error}"));
            }
        }
        Command::Refresh { sub_id, providers } => {
            if let Err(error) = runtime::run_refresh(&sub_id, &providers).await {
                patch_status(|snapshot| {
                    snapshot.last_error = Some(error.clone());
                });
                logging::log("[Subscription]", &format!("❌ 订阅刷新失败: {error}"));
            }
        }
        Command::Download(providers) => {
            match download::run_download().await {
                Ok(download::DownloadOutcome::Done) => {
                    patch_status(|snapshot| {
                        snapshot.download.state = "done";
                        snapshot.download.error = None;
                        snapshot.last_error = None;
                    });
                    logging::log(
                        "[Subscription]",
                        &format!("✅ 代理内核 {KERNEL_VERSION} 下载完成"),
                    );
                    // 下完自动拉起（有启用的订阅才有意义；没有时用户之后点重启）
                    if !providers.is_empty() {
                        if let Err(error) = runtime::run_restart(&providers).await {
                            patch_status(|snapshot| {
                                snapshot.last_error = Some(error.clone());
                                snapshot.running = false;
                            });
                            logging::log("[Subscription]", &format!("❌ 内核启动失败: {error}"));
                        }
                    }
                }
                // 用户按了停止：不是失败，**不写 last_error**（那是给「真出错」留的
                // 位置，写进去会让内核状态条上挂着一条用户自己按出来的红字）
                Ok(download::DownloadOutcome::Canceled) => {
                    patch_status(|snapshot| {
                        snapshot.download.state = "canceled";
                        snapshot.download.received = 0;
                        snapshot.download.total = 0;
                        snapshot.download.error = None;
                    });
                    logging::log("[Subscription]", "⏹️ 已停止下载代理内核（临时文件已删除）");
                }
                Err(error) => {
                    patch_status(|snapshot| {
                        snapshot.download.state = "error";
                        snapshot.download.error = Some(error.clone());
                        snapshot.last_error = Some(error.clone());
                    });
                    logging::log("[Subscription]", &format!("❌ 内核下载失败: {error}"));
                }
            }
        }
        Command::Remove => {
            // 先停进程再动文件：Windows 上运行中的 exe 删不掉（共享冲突）。
            // 两步都做：cleanup_stale_kernel 收拾上次运行遗留的（pid 从状态文件里来，
            // 控制器的 secret 对上才杀），stop_kernel 收当前这个子进程句柄 ——
            // 只做后者时，遗留进程占着二进制会让删除失败。
            runtime::cleanup_stale_kernel().await;
            runtime::stop_kernel().await;
            match download::uninstall() {
                Ok(()) => {
                    // 内核没了 = 订阅节点的监听器全没了：端口必须一起回收，否则
                    // 池里那些地址指向的是**没人监听**的端口，界面上却看着像能用。
                    // 不摘条目（保 id / 保账号引用），只把端口置空 —— 重装内核后
                    // 同步按 providerId + node 原位写回，见该函数的说明。
                    match proxy_pool::unprovision_subscription_nodes() {
                        Ok(changes) if changes > 0 => logging::log(
                            "[Subscription]",
                            &format!("🔄 已回收 {changes} 个订阅节点的监听端口"),
                        ),
                        Ok(_) => {}
                        Err(error) => logging::log(
                            "[Subscription]",
                            &format!("❌ 回收订阅节点端口失败: {error}"),
                        ),
                    }
                    // 整份状态回到初始：上一次运行的端口 / 监听器数 / 各订阅同步
                    // 记录（订阅表里的「N 个节点」「上次同步」由它注入）都不再成立；
                    // 保留 last_error 才是错的（那说的是已经不在磁盘上的内核）
                    patch_status(|snapshot| {
                        *snapshot = StatusSnapshot::default();
                    });
                    logging::log(
                        "[Subscription]",
                        &format!("🗑️ 已删除代理内核 {KERNEL_VERSION}（订阅与账号引用保留）"),
                    );
                }
                Err(error) => {
                    patch_status(|snapshot| {
                        snapshot.last_error = Some(error.clone());
                    });
                    logging::log("[Subscription]", &format!("❌ 删除内核失败: {error}"));
                }
            }
        }
        Command::Stop => {
            runtime::stop_kernel().await;
            // 一条启用订阅都不剩 → 订阅节点镜像一并清空（空目标集合 = 摘掉
            // 全部 subscription 条目，见 `proxy_pool::sync_subscription_nodes`）。
            //
            // **必须在停内核这条路径上清**：只有「还有其它启用订阅」时才会走
            // 重启路径顺带同步，删掉最后一条订阅时若不清，节点镜像就留在池里
            // 成了孤儿 —— 归属订阅已不存在，界面上挂着且删不掉（实测踩过）。
            sync_nodes_to_pool(&[]);
            patch_status(|snapshot| {
                snapshot.running = false;
            });
        }
    }
}

// ─── 公开操作（api 层入口）─────────────────────────────────

/// 请求下载内核；`providers` 非空时下完自动拉起。
///
/// 下载参数（出口线路 / 存放目录）**不在命令里带**：它们是配置项
/// （`kernelProxy` / `kernelDir`），api 层在投命令之前就已落盘 —— 命令里再带
/// 一份就等于同一件事有两个真相，而 worker 执行时刻读配置拿到的必然是最新的
/// 那个（包括「投命令之后又改了一次设置」的极端时序）。
pub fn request_download(providers: Vec<ProviderInput>) {
    // 新一轮下载：上一轮可能残留的「停止」请求作废。要在这里清而不是在
    // download::run_download 里清 —— 「点下载」与「worker 真正开跑」之间隔着
    // 队列（前面可能排着一次内核重启），这期间用户点停止必须算数。
    download::reset_cancel();
    patch_status(|snapshot| {
        snapshot.download.state = "downloading";
        snapshot.download.received = 0;
        snapshot.download.total = 0;
        snapshot.download.error = None;
    });
    send(Command::Download(providers));
}

/// 请求停止正在进行的下载（界面上的「停止下载」）。
///
/// ── 为什么不走命令队列（本模块唯一的例外）────────────────────
/// 队列是串行消费的，而「正在下载」这件事本身就意味着 worker 正**阻塞在**
/// 队列里 —— 这时候投一条 Cancel 只会排在下载后面，等它执行时下载早已结束。
/// 所以这里直接置一个原子标志，下载循环每个分片读一次；返回 false 表示当前
/// 没有在下载（调用方据此报「没有正在进行的下载」而不是假装停成功了）。
pub fn request_cancel_download() -> bool {
    let downloading = with_status(|snapshot| snapshot.download.state == "downloading");
    if !downloading {
        return false;
    }
    download::request_cancel();
    true
}

/// 请求启动 / 重启内核（重建配置：订阅增删改、手动重启都走它）。
pub fn request_restart(providers: Vec<ProviderInput>) {
    send(Command::Restart(providers));
}

/// 请求强制刷新一条订阅的内容。
pub fn request_refresh(sub_id: String, providers: Vec<ProviderInput>) {
    send(Command::Refresh { sub_id, providers });
}

/// 请求停掉内核（一条启用订阅都不剩时调用；空跑的内核只会白占端口）
pub fn request_stop() {
    send(Command::Stop);
}

/// 请求停掉内核并删除已下载的内核（界面上的「删除内核」）。
///
/// 走命令队列而不是直接在 handler 里删文件：删除与重启 / 下载会互相踩
/// （删到一半来个 Restart 会按半截目录启动），串行化是这一模块的唯一约定。
pub fn request_remove() {
    send(Command::Remove);
}

/// 状态 JSON（GET /api/subscriptions 与列表响应共用）。
///
/// `running` 是快照与**实际探活**的合取：worker 只在自己的操作里更新
/// running，内核进程在两次操作之间崩溃时快照还是 true —— 这里补一次
/// 本地端口探活（connect 到控制端口，毫秒级），状态不符时如实报 false。
pub fn status_json() -> Value {
    with_status(|snapshot| {
        let controller_port = snapshot.controller_port;
        let snapshot_running = snapshot.running;
        let listener_count = snapshot.listener_count;
        let last_error = snapshot.last_error.clone();
        let provider_sync = snapshot.provider_sync.clone();
        let download_state = snapshot.download.state;
        let download_received = snapshot.download.received;
        let download_total = snapshot.download.total;
        let download_error = snapshot.download.error.clone();
        let running = snapshot_running
            && controller_port
                .map(|port| controller_alive(port))
                .unwrap_or(false);
        json!({
            "installed": installed(),
            "version": KERNEL_VERSION,
            "asset": asset_for_current_platform().map(|(name, _)| name),
            "running": running,
            "controllerPort": controller_port,
            "listenerCount": listener_count,
            "kernelDir": kernel_dir().to_string_lossy(),
            // 默认目录单列一个读数：下载弹窗要给「恢复默认」一个具体的目标值
            // （界面不该自己去拼 `{config_dir}/proxy-kernel`，那是后端的布局）
            "kernelDirDefault": default_kernel_dir().to_string_lossy(),
            // 已存的下载线路（描述形态，与 /api/update/proxy 同口径）：
            // 弹窗用它把「当前选的是哪条」显示出来 —— null = 默认链
            "downloadProxy": crate::server::core::proxies::describe_account_proxy(
                crate::server::config::kernel_proxy().as_ref(),
            ),
            "lastError": last_error,
            "providerSync": provider_sync,
            "download": {
                "state": download_state,
                "received": download_received,
                "total": download_total,
                "error": download_error,
            },
        })
    })
}

/// 控制端口探活（本地 TCP connect，超时 300ms）
fn controller_alive(port: u16) -> bool {
    let address = format!("127.0.0.1:{port}");
    std::net::ToSocketAddrs::to_socket_addrs(&address)
        .ok()
        .and_then(|mut addresses| addresses.next())
        .map(|address| TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok())
        .unwrap_or(false)
}

/// 停机钩子（backend::shutdown 调）：best-effort 杀掉内核子进程。
///
/// worker 可能正阻塞在下一条命令上（收不到 Stop 也要杀），所以这里直接
/// 碰 runtime 里的子进程句柄，不走命令队列。
pub fn shutdown() {
    runtime::kill_child();
    logging::log("[Subscription]", "🛑 已停止代理内核");
}

// ─── 给 runtime / download 的共享小件 ───────────────────────

/// 把节点集合镜像进代理池（runtime 在内核同步完成后调用）。
///
/// 目标类型定义在 proxy_pool（`SubscriptionNodeTarget`）：本模块 → proxy_pool
/// 单向依赖，proxy_pool 不反过来认识内核（订阅条目的端口存在条目自身里，
/// 解析不需要查内核状态）。
pub fn sync_nodes_to_pool(targets: &[proxy_pool::SubscriptionNodeTarget]) {
    match proxy_pool::sync_subscription_nodes(targets) {
        Ok(changes) if changes > 0 => {
            logging::log(
                "[Subscription]",
                &format!("🔄 已同步订阅节点到代理池（{changes} 项变更）"),
            );
        }
        Ok(_) => {}
        Err(error) => logging::log("[Subscription]", &format!("❌ 节点同步代理池失败: {error}")),
    }
}

/// Arc 包装的控制器端口记录（runtime 写快照用）
pub fn record_running(controller_port: Option<u16>, listener_count: usize, error: Option<String>) {
    patch_status(|snapshot| {
        snapshot.running = error.is_none() && controller_port.is_some();
        snapshot.controller_port = controller_port;
        snapshot.listener_count = listener_count;
        snapshot.last_error = error;
    });
}

/// 记录一次订阅同步成功（runtime 的 provider 轮询后调用）
pub fn record_provider_sync(sub_id: &str, sub_name: &str, count: usize) {
    patch_status(|snapshot| {
        snapshot.provider_sync.insert(
            sub_id.to_string(),
            json!({
                "name": sub_name,
                "count": count,
                "at": logging::now_ms(),
            }),
        );
    });
}

/// 记一条内核运行日志（runtime / download 共用；控制台 + 运行日志一条口径）
pub fn log_line(message: &str) {
    logging::log("[Subscription]", message);
}

/// 记下载进度（download 在流式落盘时调用，界面轮询渲染）。
pub fn record_download_progress(received: u64, total: u64) {
    patch_status(|snapshot| {
        snapshot.download.received = received;
        snapshot.download.total = total;
    });
}
