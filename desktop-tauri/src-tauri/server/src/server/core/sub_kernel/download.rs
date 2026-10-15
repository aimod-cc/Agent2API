//! 内核下载：GitHub Release 资产 → 流式落盘（进度进快照）→ sha256 校验 →
//! 解压安装到内核目录。
//!
//! ── 出口候选与软件更新同一套 ────────────────────────────────
//! 下载源是 GitHub（国内直连常常不通），出口顺序：**下载弹窗里选定的线路**
//! （`kernelProxy` 配置键，选了就只用它）优先；没选时走软件更新那套兜底顺序
//! （直连 → Clash 混合端口兜底），判据一致：只有网络层失败才换出口，HTTP
//! 状态码直接报。
//!
//! ── 存放目录可配（kernelDir）────────────────────────────────
//! 二进制、`config.yaml`、端口注册表、provider 缓存全部落在 `kernel_dir()`，
//! 而它是「配置里写了绝对路径就用它，否则默认 `{config_dir}/proxy-kernel`」。
//! 下载弹窗在开始前把目录落配置，这里只读 —— 于是「换目录」对本模块与
//! runtime 是同一件事（都读同一个函数），没有第二处要同步。
//!
//! ── 中途停止 ────────────────────────────────────────────────
//! 用户按停止 → `request_cancel()` 置原子标志 → 下载循环在**每个分片**上查一次
//! （并且等分片时带 500ms 超时，链路卡住也照样在 0.5 秒内响应），命中就中止流、
//! 删掉半截压缩包、返回 `DownloadOutcome::Canceled`。取消**不算失败**：
//! 不写 `last_error`、不试下一条线路（用户叫停就是叫停）。
//!
//! ── 校验失败的整体拒绝 ──────────────────────────────────────
//! sha256 与 pin 死的值（mod.rs 的资产表）对不上就删掉坏文件并报错 ——
//! 代理内核是要接管出网流量的二进制，装一个被篡改的包比装不上严重得多，
//! 不做「校验失败但继续」的降级。
//!
//! ── 解压 ────────────────────────────────────────────────────
//! Windows 资产是 zip（官方只出 zip），解压用系统 PowerShell 的
//! `Expand-Archive`（不引第三方 zip 依赖树；PowerShell 5.1 起 Windows 自带）；
//! macOS / Linux 资产是 .gz，用已在依赖树里的 flate2。解出的二进制统一改名
//! 到 `kernel_binary_name()`（config / 状态文件引用固定名，不随版本漂移）。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;

use crate::server::config;
use crate::server::core::egress;
use crate::server::core::proxies::{resolve_account_proxy, ProxyResolution, CLASH_MIXED_UID};
use crate::server::core::sub_kernel::{kernel_binary_name, kernel_dir, log_line, KERNEL_VERSION};

/// 下载请求的 UA（GitHub 不校验 UA，给个非空的便于排障）
const USER_AGENT: &str = "agent2api";

/// 下载中的进度回报间隔（字节）
const PROGRESS_STEP: u64 = 256 * 1024;

/// 「停止下载」的请求标志（见模块头的「中途停止」）。
///
/// 与内核操作的命令队列无关 —— 那个队列此刻正被这次下载占着，投进去的停止
/// 命令要等下载结束才轮得到（理由见 `sub_kernel::request_cancel_download`）。
static CANCEL: AtomicBool = AtomicBool::new(false);

/// 置位停止请求（用户点「停止下载」）
pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 清掉停止请求（`request_download` 在新一轮开始时调用）—— 否则上一轮的停止
/// 会把下一轮刚开的下载当场掐死
pub fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

fn cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

/// 一次下载的结局。取消**不是错误**，所以不能只靠 `Err` 表达 —— worker 据此
/// 决定快照进 `canceled` 还是 `error`（前者不该写 `last_error`）。
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// 下载 + 校验 + 安装都完成
    Done,
    /// 用户中途停止：半截文件已删，什么都没装
    Canceled,
}

/// 下载过程中的失败（与「用户停止」分开，见 `DownloadOutcome`）
enum DownloadError {
    Canceled,
    Failed(String),
}

/// 执行下载（worker 调用；已安装时是 no-op ——「重复点下载」不重复拉）。
pub async fn run_download() -> Result<DownloadOutcome, String> {
    if crate::server::core::sub_kernel::installed() {
        return Ok(DownloadOutcome::Done);
    }
    let Some((asset, sha256)) = crate::server::core::sub_kernel::asset_for_current_platform() else {
        return Err("当前平台暂不支持代理内核（官方没有这一平台的预编译产物）".to_string());
    };
    let dir = kernel_dir();
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建内核目录失败: {error}"))?;
    let archive_path = dir.join(asset);
    let url = format!("https://github.com/MetaCubeX/mihomo/releases/download/{KERNEL_VERSION}/{asset}");
    log_line(&format!("⬇️ 开始下载代理内核 {KERNEL_VERSION}（{asset}）"));

    // 出口候选在开跑前定下：选定线路不可用时**直接报错**，不乱试别的线路
    // （理由见 egress_candidates 的说明）
    let candidates = egress_candidates()?;
    let candidate_count = candidates.len();

    // 依次试各出口（网络层失败才换；HTTP 状态码直接报）
    let mut last_error = String::new();
    for (index, proxy) in candidates.into_iter().enumerate() {
        match download_to_file(&url, &archive_path, proxy.as_ref()).await {
            Ok(()) => {
                last_error.clear();
                break;
            }
            // 用户叫停：整次下载到此为止（不换线路、不往校验 / 解压走）
            Err(DownloadError::Canceled) => {
                let _ = std::fs::remove_file(&archive_path);
                return Ok(DownloadOutcome::Canceled);
            }
            Err(DownloadError::Failed(error)) => {
                last_error = error;
                if index + 1 < candidate_count {
                    log_line(&format!("⚠️ {last_error}，换下一条线路重试"));
                }
            }
        }
    }
    if !last_error.is_empty() {
        let _ = std::fs::remove_file(&archive_path);
        return Err(last_error);
    }

    // sha256 校验（对不上整体拒绝，坏文件删掉）
    let actual = file_sha256(&archive_path)?;
    if !actual.eq_ignore_ascii_case(sha256) {
        let _ = std::fs::remove_file(&archive_path);
        return Err("内核下载校验失败（sha256 与官方发布值不匹配），已删除坏文件".to_string());
    }

    // 解压安装（统一改名到固定名）
    install_binary(&archive_path, &dir)?;
    let _ = std::fs::remove_file(&archive_path);
    Ok(DownloadOutcome::Done)
}

/// 出口候选：**下载弹窗里选定的线路**优先且独占；没选时走默认链（直连 →
/// Clash 混合端口兜底，与 `update::client` 的 `resolveEgressCandidates`
/// 同一条顺序）。
///
/// ── 选定线路**不回落**（与 updateProxy 的取向刻意不同）────────────
/// 软件更新的口径是「指定出口解析不出来就退回默认链」—— 那条链路后台静默跑，
/// 宁可降级也别失败。内核下载是用户**刚在弹窗里点过一条线路**的前台动作：
/// 这时悄悄改走直连，失败信息会变成「无法连接 GitHub」，用户会去怀疑网络、
/// 怀疑订阅，而真正的问题是那条线路已经不在了。所以这里报错，并把该去哪儿改
/// 说清楚。
fn egress_candidates() -> Result<Vec<Option<crate::server::core::proxies::ResolvedProxy>>, String> {
    if let Some(configured) = config::kernel_proxy() {
        return match resolve_account_proxy(Some(&configured)) {
            Some(ProxyResolution::Resolved(proxy)) if proxy.port.is_some() => Ok(vec![Some(proxy)]),
            Some(ProxyResolution::Resolved(_)) => Err(
                "选定的下载出口没有可用的监听端口（内核还没为它分配端口），换一条线路再试".to_string(),
            ),
            Some(ProxyResolution::Failed(reason)) => Err(format!(
                "选定的下载出口不可用（{reason}），重新选一条线路再试"
            )),
            None => Err("选定的下载出口已不存在，重新选一条线路再试".to_string()),
        };
    }
    let mut candidates = vec![None];
    if let Some(ProxyResolution::Resolved(proxy)) = resolve_account_proxy(Some(&json!({
        "source": "clash",
        "listenerUid": CLASH_MIXED_UID,
    }))) {
        if proxy.port.is_some() {
            candidates.push(Some(proxy));
        }
    }
    Ok(candidates)
}

/// 流式下载到文件，进度进快照（界面轮询渲染）。
///
/// 不给总超时（几十 MB 的资产不可能在固定时限内下完）；卡死场景由
/// egress 客户端的 read_timeout 兜底（与软件更新的下载同语义）。
async fn download_to_file(
    url: &str,
    path: &std::path::Path,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
) -> Result<(), DownloadError> {
    let client = egress::client_for(proxy);
    let response = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|error| {
            DownloadError::Failed(format!(
                "无法连接 GitHub: {}",
                egress::describe_error_detail(&error)
            ))
        })?;
    if !response.status().is_success() {
        return Err(DownloadError::Failed(format!(
            "GitHub 返回 {}",
            response.status().as_u16()
        )));
    }
    let total = response.content_length().unwrap_or(0);
    crate::server::core::sub_kernel::record_download_progress(0, total);
    let mut file = std::fs::File::create(path)
        .map_err(|error| DownloadError::Failed(format!("创建下载文件失败: {error}")))?;
    let mut stream = response.bytes_stream();
    use futures::StreamExt;
    let mut received: u64 = 0;
    let mut next_report = PROGRESS_STEP;
    loop {
        // 先看停止请求再写盘：停止是**合作式**的（不杀进程，只是不再往下读）。
        // 分片几十 KB → 正常情况下下一拍就响应
        if cancel_requested() {
            return Err(DownloadError::Canceled);
        }
        // 等分片**带超时**：链路卡住（连接建了但一个字节都不来）时，若干等
        // stream.next()，停止请求要等 read_timeout 兜底才被看见 —— 用户看到的是
        // 「停止中…」挂着不动。500ms 回来看一眼标志，停止的响应时间就与链路
        // 是否卡住无关了（丢掉一个没等到的 next() 是安全的：缓冲在 stream 里）
        let next = match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(next) => next,
            Err(_) => continue,
        };
        let Some(chunk) = next else { break };
        let chunk =
            chunk.map_err(|error| DownloadError::Failed(format!("下载中断: {error}")))?;
        file.write_all(&chunk)
            .map_err(|error| DownloadError::Failed(format!("写入下载文件失败: {error}")))?;
        received += chunk.len() as u64;
        if received >= next_report {
            next_report = received + PROGRESS_STEP;
            crate::server::core::sub_kernel::record_download_progress(received, total);
        }
    }
    file.flush()
        .map_err(|error| DownloadError::Failed(format!("写入下载文件失败: {error}")))?;
    crate::server::core::sub_kernel::record_download_progress(received, total);
    Ok(())
}

/// 文件的 sha256（流式读，不动内存里的整份文件）
fn file_sha256(path: &std::path::Path) -> Result<String, String> {
    use sha2::Digest;
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("打开下载文件失败: {error}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取下载文件失败: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// 解压安装：Windows zip 走 PowerShell Expand-Archive，unix .gz 走 flate2；
/// 解出的二进制统一改名到 `kernel_binary_name()`。
fn install_binary(archive_path: &std::path::Path, dir: &std::path::Path) -> Result<(), String> {
    let binary_path = dir.join(kernel_binary_name());
    // 旧二进制先删（Windows 上 rename 到已存在的目标会失败）
    let _ = std::fs::remove_file(&binary_path);
    #[cfg(windows)]
    {
        let extract_dir = dir.join("extract");
        let _ = std::fs::remove_dir_all(&extract_dir);
        std::fs::create_dir_all(&extract_dir)
            .map_err(|error| format!("创建解压目录失败: {error}"))?;
        let archive = archive_path.to_string_lossy().to_string();
        let destination = extract_dir.to_string_lossy().to_string();
        // powershell 也是控制台程序：桌面端不加 CREATE_NO_WINDOW 就会在解压那几秒
        // 闪一个黑框（与内核进程本身同一个成因，见 runtime::start_kernel 的说明）
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Expand-Archive -LiteralPath '{archive}' -DestinationPath '{destination}' -Force"),
            ])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .map_err(|error| format!("调用 PowerShell 解压失败: {error}"))?;
        if !output.status.success() {
            let _ = std::fs::remove_dir_all(&extract_dir);
            return Err(format!(
                "解压内核失败（{}）",
                String::from_utf8_lossy(&output.stderr).trim().chars().take(200).collect::<String>()
            ));
        }
        // 解出的 exe 名是 `mihomo-windows-amd64-<版本>.exe`，改名到固定名
        let extracted = std::fs::read_dir(&extract_dir)
            .map_err(|error| format!("读取解压目录失败: {error}"))?
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                name.starts_with("mihomo-") && name.ends_with(".exe")
            })
            .ok_or_else(|| "解压结果里没有内核二进制".to_string())?;
        std::fs::rename(extracted.path(), &binary_path)
            .map_err(|error| format!("安装内核失败: {error}"))?;
        let _ = std::fs::remove_dir_all(&extract_dir);
    }
    #[cfg(not(windows))]
    {
        let archive = std::fs::File::open(archive_path)
            .map_err(|error| format!("打开下载文件失败: {error}"))?;
        let mut decoder = flate2::read::GzDecoder::new(archive);
        let mut binary = std::fs::File::create(&binary_path)
            .map_err(|error| format!("安装内核失败: {error}"))?;
        std::io::copy(&mut decoder, &mut binary)
            .map_err(|error| format!("解压内核失败: {error}"))?;
        // 可执行位（unix 上从网络下载的二进制默认没有执行权限）
        use std::os::unix::fs::PermissionsExt;
        let _ = binary.set_permissions(std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// 卸载内核：删掉整个内核目录（二进制 / `config.yaml` / `kernel-state.json` /
/// `providers` 缓存 / 下载残留）。
///
/// ── 调用方必须先停内核进程 ───────────────────────────────────
/// worker 的 Remove 分支先 `stop_kernel()`（等子进程退出）再进来：Windows 上
/// 正在运行的可执行文件删不掉（共享冲突），而且「先停再删」本来也是更直白的语义。
///
/// 目录不存在（重复点删除 / 从没装过）**按成功处理** —— 这个动作的目标状态是
/// 「内核不在磁盘上」，它已经成立了。真删不掉才报错（被杀软或别的进程占着），
/// 让界面能把原因说出来，而不是静默留一半文件。
///
/// 只删内核自身：订阅（`core::subscriptions`）与账号引用都不在内核目录里，
/// 不受影响 —— 订阅节点的端口回收见 `proxy_pool::unprovision_subscription_nodes`。
pub fn uninstall() -> Result<(), String> {
    let dir = kernel_dir();
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|error| format!("删除内核目录失败（{}）：{error}", dir.display()))
}

/// 等待一段时间的辅助（下载重试之间用）
#[allow(dead_code)]
fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
