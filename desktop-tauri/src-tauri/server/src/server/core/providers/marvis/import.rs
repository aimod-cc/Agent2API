//! Marvis 本机凭证导入：**从已登录的客户端进程内存里直接取**，用户不用粘贴。
//!
//! ── 为什么要派生一个提权子进程（这是本家的硬约束）──────────────
//! 客户端的凭证**不落明文**：`db/login.db` 的 userinfo 是密文、
//! `marvis_seckv.dll` 的 `mskv_get` 会 `WinVerifyTrust` 校验调用方签名
//! （只认腾讯签名进程）。令牌只在 `Marvis.exe` / `MarvisHost.exe` 的内存里。
//!
//! 而客户端进程**自设了受限 DACL**（反调试）：同用户下
//! `OpenProcess(PROCESS_QUERY_INFORMATION|PROCESS_VM_READ)` 返回 `err=5`，
//! 连 `taskkill` 都被拒。**只有提权（SeDebugPrivilege）能读**。
//!
//! 网关进程本身**不是**管理员（正常安装形态），所以它读不了 —— 这也是本模块
//! 存在的全部理由：由网关派生一个**提权**的提取器进程（弹一次 UAC），
//! 提取器把结果写成 JSON，网关再读回来落账号。用户的操作只剩「点『是』」。
//!
//! ── 与「网页登录」的关系（为什么没有走那条）────────────────────
//! 马维斯**没有网页版**（官网只有下载入口，`marvis.qq.com/web/` 308 跳走），
//! 登录只发生在客户端自己的 CEF 里。而 `marvis_oauth` 需要客户端侧已注册的
//! 登录会话（纯 HTTP 复现恒 `-109 RC_PARAMS_INVALID`，已系统性排除 7 种原因，
//! 见 `_recon/agent2api-marvis-port-plan.md` §1.7）。⇒ 没有可复现的网页登录链。
//!
//! ── 提权子进程跑的是什么 ────────────────────────────────────
//! `_recon/marvis-cred-extract.py`（随仓库分发）。它与本模块是**同一份契约**：
//! 读 `%APPDATA%\Tencent\Marvis\*` 下客户端进程内存 → 写
//! `<repo>/_recon/marvis-cred.json`。契约变更时两边必须一起改。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::server::errors::GatewayError;

/// 提取器脚本相对仓库根的位置。
const EXTRACTOR_REL: &str = "_recon/marvis-cred-extract.py";

/// 提取器写出的结果文件（与脚本里的 `OUT` 同一路径，契约见模块头）。
const RESULT_REL: &str = "_recon/marvis-cred.json";

/// 等提权进程写完结果的最长时间。用户要点 UAC、脚本要扫 1GB 级内存，
/// 30 秒偏紧，给 90 秒（超时不算失败，只是「没等到」）。
const WAIT_TIMEOUT: Duration = Duration::from_secs(90);

/// 轮询间隔。
const POLL: Duration = Duration::from_millis(400);

/// 仓库根：从当前可执行文件的位置向上找 `_recon/` 目录。
///
/// ── 为什么不用 `CARGO_MANIFEST_DIR` ───────────────────────────
/// 那是**编译期**路径，release 包在用户机器上根本不存在。运行时能定位到的
/// 只有「可执行文件在哪」，而仓库布局在开发机与打包产物里不同：
///   · 开发：`target/debug/agent2api-server.exe` → 上溯若干级到仓库根；
///   · 打包：安装目录旁边可能没有 `_recon/`（此时**明确报错**让用户手动跑，
///     而不是悄悄失败）。
fn repo_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..6 {
        if dir.join(EXTRACTOR_REL).is_file() {
            return Some(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    None
}

/// 结果文件比这次请求更新（防「读到上一次的旧结果」）。
fn is_fresh(path: &Path, started: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(|mtime| mtime >= started)
        .unwrap_or(false)
}

/// 读结果 JSON 并校验形状。
fn read_result(path: &Path) -> Result<Value, GatewayError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        GatewayError::with_status(500i32, format!("读取 Marvis 凭证文件失败：{error}"))
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        GatewayError::with_status(
            500i32,
            format!("Marvis 凭证文件不是合法 JSON（{error}）：请删除 {RESULT_REL} 后重试"),
        )
    })?;
    // 脚本输出 `access_token`（蛇形）；本仓落账号用 `accessToken`（驼峰）。
    // 在**这里**统一改名，避免「同一份数据两种命名」扩散到存储层。
    let mut record = serde_json::Map::new();
    let token = value
        .get("access_token")
        .or_else(|| value.get("accessToken"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let openid = value.get("openid").and_then(Value::as_str).unwrap_or("");
    let guid = value.get("guid").and_then(Value::as_str).unwrap_or("");
    if token.is_empty() || openid.is_empty() || guid.is_empty() {
        return Err(GatewayError::with_status(
            500i32,
            "Marvis 凭证文件缺字段：需要 access_token / openid / guid 三项",
        ));
    }
    record.insert("accessToken".into(), Value::String(token.to_string()));
    record.insert("openid".into(), Value::String(openid.to_string()));
    record.insert("guid".into(), Value::String(guid.to_string()));
    Ok(Value::Object(record))
}

/// 派生提权提取器并等它写完结果。
///
/// ── 为什么用 `ShellExecuteW(…, "runas", …)` ─────────────────
/// 先试过 `powershell Start-Process -Verb RunAs`，**运行期被安全策略拦掉**
/// （症状是「导入一直等到超时」，日志里没有任何提权进程痕迹）。`ShellExecuteW`
/// 是这条动作的原生入口，不经过任何 shell，也不受该策略影响。
///
/// ── 返回码怎么读 ───────────────────────────────────────────
/// `ShellExecuteW` 的返回值 > 32 才算成功；`5`（SE_ERR_ACCESSDENIED）与
/// `1223`（ERROR_CANCELLED，用户点了「否」）是两种最常见的失败，要分开报 ——
/// 「你取消了」和「系统不让提权」对用户是两件不同的事。
#[cfg(windows)]
fn spawn_extractor(script: &Path) -> Result<(), GatewayError> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let python = std::env::var("WORKBUDDY_MARVIS_PYTHON")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "python".to_string());

    let wide = |text: &str| -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    };
    let verb = wide("runas");
    let file = wide(&python);
    // 参数整体加引号：脚本路径可能含空格（本仓库在 E:\Agent2API，稳妥起见）
    let params = wide(&format!("\"{}\"", script.display()));
    let dir = script
        .parent()
        .map(|path| wide(&path.display().to_string()))
        .unwrap_or_else(|| wide(""));

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR(dir.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code > 32 {
        return Ok(());
    }
    let hint = match code {
        5 => "（系统拒绝提权，可能是 UAC 策略限制）",
        1223 => "（你取消了管理员授权弹窗）",
        2 => "（找不到 python，可用环境变量 WORKBUDDY_MARVIS_PYTHON 指定路径）",
        _ => "",
    };
    Err(GatewayError::with_status(
        400i32,
        format!("未能启动 Marvis 凭证提取器（ShellExecute 返回 {code}{hint}）"),
    ))
}

/// 本机导入：派生提权提取器 → 等结果 → 返回可直接落账号的记录。
///
/// 返回的 `Value` 形状与「手工粘贴」那条路**完全一致**
/// （`{accessToken, openid, guid}`），所以 `add_account` 的两条入口共用同一段
/// 落库代码，不需要为「导入」再写一份。
#[cfg(windows)]
pub async fn import_from_local() -> Result<Value, GatewayError> {
    let root = repo_root().ok_or_else(|| {
        GatewayError::with_status(
            400i32,
            format!(
                "没有找到 Marvis 凭证提取器（{EXTRACTOR_REL}）：打包版不含该脚本。\
                 请以管理员身份手动运行它，然后把生成的 JSON 粘贴到「手工粘贴」里。"
            ),
        )
    })?;
    let script = root.join(EXTRACTOR_REL);
    let result = root.join(RESULT_REL);
    let started = SystemTime::now();

    spawn_extractor(&script)?;

    // 轮询：用户点 UAC 需要时间，脚本扫内存也要时间
    let deadline = std::time::Instant::now() + WAIT_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if result.is_file() && is_fresh(&result, started) {
            return read_result(&result);
        }
        tokio::time::sleep(POLL).await;
    }
    Err(GatewayError::with_status(
        400i32,
        "等待 Marvis 凭证超时（90 秒）。常见原因：① 提权弹窗被取消；\
         ② 马维斯客户端没有运行或未登录 —— 令牌只在其运行时存在于内存；\
         ③ 客户端刚启动还没发过请求 —— 随便问它一句话再试。\
         也可以改用「手工粘贴」：以管理员身份运行 _recon/marvis-cred-extract.py。"
            .to_string(),
    ))
}

/// 非 Windows：客户端只有 Windows / macOS 版，本机导入只在 Windows 上有意义。
#[cfg(not(windows))]
pub async fn import_from_local() -> Result<Value, GatewayError> {
    Err(GatewayError::with_status(
        400i32,
        "Marvis 本机导入仅支持 Windows（设备指纹与凭证提取都依赖 Windows 客户端）",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn result_shape_uses_camel_case_for_storage() {
        // 脚本输出蛇形，落库用驼峰 —— 改名只发生在 read_result 一处
        let dir = std::env::temp_dir().join("mv-cred-shape-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("c.json");
        std::fs::write(
            &path,
            json!({"access_token": "mv_abc", "openid": "o1", "guid": "g1"}).to_string(),
        )
        .expect("写临时文件");
        let value = read_result(&path).expect("应能解析");
        assert_eq!(value["accessToken"], "mv_abc");
        assert_eq!(value["openid"], "o1");
        assert!(value.get("access_token").is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_field_is_rejected() {
        let dir = std::env::temp_dir().join("mv-cred-shape-test2");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("c.json");
        std::fs::write(&path, json!({"access_token": "mv_abc"}).to_string()).expect("写");
        assert!(read_result(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_result_is_not_fresh() {
        // 时间戳早于「开始等待」时刻 ⇒ 不采纳（防读到上一次的旧结果）
        let path = std::env::temp_dir().join("mv-cred-fresh-test.json");
        std::fs::write(&path, "{}").expect("写");
        let future = SystemTime::now() + Duration::from_secs(60);
        assert!(!is_fresh(&path, future));
        let _ = std::fs::remove_file(&path);
    }
}
