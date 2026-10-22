//! Marvis 设备指纹 `uskey`：**离线生成**，无需启动客户端。
//!
//! ── 这是什么、为什么必须有（2026-10-20 实测）──────────────────
//! 上游 `POST /v1/chat/completions` 的鉴权分三层（实测逐层验证）：
//!
//! ```text
//!   ① access_token  —— 硬凭据。缺失/失效 → 401 {"code":4100403}
//!   ② uskey         —— 设备指纹。缺失 → 403 {"code":4100404}（风控拦截）
//!   ③ Ual-Access-Signature —— **实测不校验**（空签名同样 200，见下）
//! ```
//!
//! 所以本家唯一需要现场算的动态件就是 `uskey`，放在请求头
//! `Ual-Access-MarvisExt`（JSON）的 `uskey` 字段里。
//!
//! ── `Ual-Access-Signature` 为什么不用复现（重要，别白干）────────
//! Windows 版的 `MarvisHost.exe` 确实会拼 `Ual-Access-Signature` /
//! `-Timestamp` / `-Nonce`（`backend_client\mod.rs`），但**服务端不验**：
//! 实测带完整签名与带空签名各发一次，两者都是 HTTP 200 + 正常回答。因此本模块
//! **不实现签名**——那是纯装饰，复现它只会引入一个会过期、会算错的脆弱依赖。
//! 这条结论有反向证据：只去掉 uskey 保留签名 → 403；只去掉签名保留 uskey → 200。
//!
//! ── uskey 怎么来（Windows 与 macOS 不同，别照抄参考实现）────────
//! macOS 参考实现（Zhengyuuuui/marvis2api）用 `libbeacon_wrapper.dylib` 的
//! `UKInstanceGetUSkey`，输出 **992B / 前缀 `CiDVDCER`**。
//! Windows 是 `beacon_wrapper.dll`，导出的是 C 风格三件套，输出
//! **544B / 前缀 `CsDVDCER`** —— 长度和前缀都不同，签名也不同，照抄必错：
//!
//! ```text
//!   int beacon_init(const char* cfg)                            // NULL 可，返回 0
//!   int beacon_get_qimei(char* out, int out_len)                // → 36（q36）
//!   int beacon_get_uskey(const char* q36, const char* businfo,  // businfo 传 "{}"
//!                        char* out, int out_len, int reserved)  // → 544
//! ```
//!
//! 调用顺序：`beacon_init(NULL)` → `beacon_get_qimei` → `beacon_get_uskey`。
//!
//! ── 两个必须踩准的细节 ──────────────────────────────────────
//!   1. **加载前要设 DLL 搜索目录**：`qimei.dll` 在同一个安装目录下，
//!      只 `LoadLibraryW` 绝对路径不够（它按名字找依赖），必须
//!      `SetDllDirectoryW(安装目录)`，否则 `beacon_init` 返回非 0。
//!   2. **`qimei36` 是稳定值**：多次独立进程调用返回同一串
//!      （本机 `d70929e1…`），所以它可以入库，不必每次重算。
//!
//! ── 为什么 uskey 每次都要新取 ───────────────────────────────
//! 实测连调三次长度恒 544、前缀恒定、**尾部每次不同** ⇒ 每请求唯一是它的
//! 设计意图；参考实现的 FINDINGS 也确认「服务端只校验新鲜度，不校验内容」。
//! 复用同一个 uskey 目前也能过，但那是在赌服务端不收紧，所以本模块按请求取新。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::server::errors::GatewayError;

/// 客户端默认安装目录（安装器写死；环境变量可覆盖）。
const DEFAULT_INSTALL_DIR: &str = r"D:\Program Files\Tencent\Marvis\Application\1.60.3000.228";

/// 允许用环境变量指定安装目录（客户端升级后版本号会变，避免改代码）。
const INSTALL_DIR_ENV: &str = "WORKBUDDY_MARVIS_DIR";

/// 设备指纹库文件名。
const BEACON_DLL: &str = "beacon_wrapper.dll";

/// 定位安装目录：环境变量 → 默认路径 → 注册表卸载项。
///
/// 找不到就返回 `None`，由调用方给出可读错误（不 panic）。
fn locate_install_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(INSTALL_DIR_ENV) {
        let path = PathBuf::from(dir.trim());
        if path.join(BEACON_DLL).is_file() {
            return Some(path);
        }
    }

    let default = PathBuf::from(DEFAULT_INSTALL_DIR);
    if default.join(BEACON_DLL).is_file() {
        return Some(default);
    }

    // 客户端升级会换版本号目录，扫一遍同级 Application/*。
    let root = PathBuf::from(r"D:\Program Files\Tencent\Marvis\Application");
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.join(BEACON_DLL).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 已加载的模块句柄（进程内只加载一次）。
#[cfg(windows)]
static BEACON_LIB: OnceLock<Option<isize>> = OnceLock::new();

/// 一次指纹调用的结果。
#[derive(Clone, Debug, Default)]
pub struct DeviceFingerprint {
    /// 36 位设备号（`qimei36`）
    pub qimei36: String,
    /// 每请求唯一的 uskey（Windows 版 544B，前缀 `CsDVDCER`）
    pub uskey: String,
}

#[cfg(windows)]
fn load_beacon() -> Result<windows::Win32::Foundation::HMODULE, GatewayError> {
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let cached = BEACON_LIB.get().cloned();
    if let Some(Some(raw)) = cached {
        return Ok(windows::Win32::Foundation::HMODULE(
            raw as *mut core::ffi::c_void,
        ));
    }
    if let Some(None) = cached {
        return Err(GatewayError::with_status(
            502,
            "Marvis 设备指纹库加载失败（已缓存失败结果）",
        ));
    }

    let Some(dir) = locate_install_dir() else {
        let _ = BEACON_LIB.set(None);
        return Err(GatewayError::with_status(
            502,
            format!(
                "没有找到 Marvis 客户端设备指纹库（{BEACON_DLL}）：请确认已安装马维斯客户端，\
                 或用环境变量 {INSTALL_DIR_ENV} 指定其安装目录"
            ),
        ));
    };

    // qimei.dll 是 beacon_wrapper 的同目录依赖，按名字查找 —— 必须设搜索目录
    let dir_w: Vec<u16> = dir
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        windows::Win32::System::LibraryLoader::SetDllDirectoryW(PCWSTR(dir_w.as_ptr())).ok();
    }

    let dll = dir.join(BEACON_DLL);
    let dll_w: Vec<u16> = dll
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let loaded = unsafe { LoadLibraryW(PCWSTR(dll_w.as_ptr())) }
        .map(|module| module.0 as isize)
        .map_err(|error| {
            GatewayError::with_status(
                502,
                format!("加载 Marvis 设备指纹库失败（{}）：{error}", dll.display()),
            )
        });

    match loaded {
        Ok(raw) => {
            let _ = BEACON_LIB.set(Some(raw));
            // 触发一次惰性初始化（失败不算致命，由取数时的返回值判定）
            unsafe {
                let module = windows::Win32::Foundation::HMODULE(raw as *mut core::ffi::c_void);
                if let Some(addr) =
                    GetProcAddress(module, windows::core::s!("beacon_init"))
                {
                    type InitFn = unsafe extern "C" fn(*const std::ffi::c_char) -> std::ffi::c_int;
                    let init: InitFn = std::mem::transmute(addr);
                    let _ = init(std::ptr::null());
                }
            }
            Ok(windows::Win32::Foundation::HMODULE(
                raw as *mut core::ffi::c_void,
            ))
        }
        Err(error) => {
            let _ = BEACON_LIB.set(None);
            Err(error)
        }
    }
}

/// 取一次设备指纹（`qimei36` + 新鲜的 `uskey`）。
///
/// 每次调用都会重新生成 uskey（服务端按新鲜度风控）；`qimei36` 是稳定值。
#[cfg(windows)]
pub fn generate() -> Result<DeviceFingerprint, GatewayError> {
    use windows::Win32::System::LibraryLoader::GetProcAddress;

    let module = load_beacon()?;

    unsafe {
        let qimei_addr = GetProcAddress(module, windows::core::s!("beacon_get_qimei"))
            .ok_or_else(|| GatewayError::with_status(502, "设备指纹库缺少 beacon_get_qimei"))?;
        let uskey_addr = GetProcAddress(module, windows::core::s!("beacon_get_uskey"))
            .ok_or_else(|| GatewayError::with_status(502, "设备指纹库缺少 beacon_get_uskey"))?;

        type QimeiFn = unsafe extern "C" fn(*mut std::ffi::c_char, std::ffi::c_int) -> std::ffi::c_int;
        type UskeyFn = unsafe extern "C" fn(
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            *mut std::ffi::c_char,
            std::ffi::c_int,
            std::ffi::c_int,
        ) -> std::ffi::c_int;

        let get_qimei: QimeiFn = std::mem::transmute(qimei_addr);
        let get_uskey: UskeyFn = std::mem::transmute(uskey_addr);

        // 缓冲区 8192 足够（实测 uskey 544B、q36 36B）
        let mut qimei_buf = vec![0u8; 8192];
        let qimei_len = get_qimei(qimei_buf.as_mut_ptr().cast(), qimei_buf.len() as i32);
        if qimei_len <= 0 {
            return Err(GatewayError::with_status(
                502,
                format!("Marvis 设备指纹库未能生成 qimei36（返回 {qimei_len}）"),
            ));
        }
        let qimei36 = String::from_utf8_lossy(&qimei_buf[..qimei_len as usize])
            .trim()
            .to_string();
        if qimei36.is_empty() {
            return Err(GatewayError::with_status(502, "Marvis qimei36 为空"));
        }

        let qimei_c = std::ffi::CString::new(qimei36.clone())
            .map_err(|_| GatewayError::with_status(502, "qimei36 含非法字符"))?;
        // businfo 必须是 JSON 字符串（实测传 "{}" 与真实客户端一致）
        let businfo_c = std::ffi::CString::new("{}")
            .map_err(|_| GatewayError::with_status(502, "businfo 含非法字符"))?;

        let mut uskey_buf = vec![0u8; 8192];
        let uskey_len = get_uskey(
            qimei_c.as_ptr(),
            businfo_c.as_ptr(),
            uskey_buf.as_mut_ptr().cast(),
            uskey_buf.len() as i32,
            0,
        );
        if uskey_len <= 0 {
            return Err(GatewayError::with_status(
                502,
                format!("Marvis 设备指纹库未能生成 uskey（返回 {uskey_len}）"),
            ));
        }
        let uskey = String::from_utf8_lossy(&uskey_buf[..uskey_len as usize])
            .trim()
            .to_string();
        if uskey.is_empty() {
            return Err(GatewayError::with_status(502, "Marvis uskey 为空"));
        }

        Ok(DeviceFingerprint { qimei36, uskey })
    }
}

/// 非 Windows：本家客户端只有 Windows / macOS 版，服务端跑在非 Windows 时
/// 无法加载 `.dll`，给出可读错误而不是编译失败。
#[cfg(not(windows))]
pub fn generate() -> Result<DeviceFingerprint, GatewayError> {
    Err(GatewayError::with_status(
        502,
        "Marvis 设备指纹需要 Windows 客户端（beacon_wrapper.dll）",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_env_is_honored() {
        // 只验证环境变量名被引用，不实际加载（测试不依赖本机环境）
        assert_eq!(INSTALL_DIR_ENV, "WORKBUDDY_MARVIS_DIR");
    }

    #[test]
    fn fingerprint_default_is_empty() {
        let fp = DeviceFingerprint::default();
        assert!(fp.uskey.is_empty());
        assert!(fp.qimei36.is_empty());
    }
}
