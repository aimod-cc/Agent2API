//! KukuAI 会话令牌换发：调官方客户端引擎构造签名参数，向百度通行证换
//! 「genflowpro 作用域」的 STOKEN。
//!
//! ── 为什么需要这一步（2026-10-08 从客户端 app.asar 逆向 + 真实凭证实测）──
//! KukuAI 业务接口（userreport / model/list / sendmsg…）认的 STOKEN 是
//! **按产品签发**的会话令牌：网页登录后 Cookie 里那份是通行证级的，业务
//! 接口一律判「未登录」（errno=-6），而余额这类通用接口只看 BDUSS ——
//! 所以症状总是「余额能查、模型拉不下」（跨天、跨客户端、跨 TLS 指纹、
//! 跨 Cookie 子集全部复现，已逐一排除其它变量）。
//! 官方客户端登录后的做法（app.asar 认证模块逐行对照）：
//!   1. 登录页 `https://kuku.baidu.com/genflowpro/login` 完成通行证登录；
//!   2. 收 Cookie（`BDUSS` / `PTOKEN` / `PASS_STOKEN` / `STOKEN`）；
//!   3. 用引擎构造**签名**参数体（`genflow_engine_get_login_auth_param`：
//!      在 `bduss+ptoken` 之外补 `appid=1&return_type=1&tpl=genflowpro&
//!      tpl_list=genflowpro|netdisk&sig=…`，`sig` 由引擎内置密钥计算，
//!      缺签 / 错签服务端都回 errno=4·110003）；
//!   4. `POST https://passport.baidu.com/v3/login/api/auth` → 响应顶层
//!      `stoken_list.genflowpro` 即业务会话令牌（响应**不带** Set-Cookie，
//!      令牌只存在于响应体，客户端存进引擎侧登录态）；
//!   5. 之后业务接口只用 `BDUSS + 该 STOKEN + gfprotpl=genflowpro` 三件
//!      （实测三件即可，无需其它 Cookie）。
//! 实测（2026-10-08 真实凭证）：换来的 STOKEN 让 userreport 从 errno=-6
//! 变 errno=0；同响应里的 `netdisk` STOKEN 依旧 -6 —— 确实按产品作用域
//! 区分，不是「新令牌就行」。
//!
//! ── 引擎依赖与回退 ─────────────────────────────────────────
//! `genflowengine.dll` 随官方客户端安装。定位顺序：环境变量
//! `WORKBUDDY_GENFLOW_ENGINE_DLL` 显式指定 → 注册表卸载项（名字含
//! 库库 / GenFlow / Kuku 的 `InstallLocation`）→ 找不到则换发失败，
//! 由调用方回退原凭证并给出可读警告（不阻断登录 / 不崩转发）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

use crate::server::core::egress;
use crate::server::errors::GatewayError;

use super::USER_AGENT;

/// passport 换发接口（客户端 `app.asar` 认证模块的 `lo` 常量，逐字对照）
const PASSPORT_AUTH_URL: &str = "https://passport.baidu.com/v3/login/api/auth";

/// 引擎 DLL 的文件名（客户端安装目录下）
const ENGINE_DLL_NAME: &str = "genflowengine.dll";

/// 定位结果缓存（进程内只找一次；找不到也缓存，避免每次刷新都扫注册表）
static LOCATED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 定位引擎 DLL：环境变量显式指定优先，其次扫注册表卸载项。
fn locate_engine_dll() -> Option<PathBuf> {
    if let Some(path) = LOCATED.get() {
        return path.clone();
    }
    let found = locate_engine_dll_uncached();
    let _ = LOCATED.set(found.clone());
    found
}

fn locate_engine_dll_uncached() -> Option<PathBuf> {
    // ① 显式指定（排障 / 客户端装在非常规位置时给用户一个口子）
    if let Ok(path) = std::env::var("WORKBUDDY_GENFLOW_ENGINE_DLL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    // ② 注册表卸载项（HKLM / HKCU × 两个视图）：读每个子键的
    //    `InstallLocation` / `DisplayIcon`，看目录下有没有引擎 DLL。
    //    不按 DisplayName 过滤 —— 中文名在注册表 API 里是 UTF-16 没有问题，
    //    但在 `reg query` 的控制台输出里会被代码页搞乱，直接逐项试更稳。
    let mut result = None;
    for_each_uninstall_location(&mut |dir| {
        if result.is_some() {
            return;
        }
        let candidate = dir.join(ENGINE_DLL_NAME);
        if candidate.is_file() {
            result = Some(candidate);
        }
    });
    result
}

/// 遍历注册表卸载项，对每个「有 InstallLocation / DisplayIcon 的子键」回调其目录。
///
/// 只在 Windows 下有实现；其它平台直接不回调（桌面壳本身也只在 Windows 出货）。
#[cfg(windows)]
fn for_each_uninstall_location(callback: &mut dyn FnMut(&PathBuf)) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_VALUE_TYPE, RegQueryInfoKeyW,
    };

    const HIVES: &[HKEY] = &[HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER];
    const PREFIXES: &[&str] = &[
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 读一个 REG_SZ 值（不存在 / 类型不符返回 None）。
    fn read_sz(hkey: HKEY, name: &str) -> Option<String> {
        let name_w = wide(name);
        let mut kind = REG_VALUE_TYPE(0);
        let mut size = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status.is_err() || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize) / 2 + 1];
        let mut got = size;
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut got),
            )
        };
        if status.is_err() {
            return None;
        }
        let len = (got as usize) / 2;
        let text = String::from_utf16_lossy(&buffer[..len]);
        let text = text.trim_end_matches('\0').trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    for hive in HIVES {
        for prefix in PREFIXES {
            let prefix_w = wide(prefix);
            let mut root = HKEY::default();
            let status = unsafe {
                RegOpenKeyExW(
                    *hive,
                    PCWSTR(prefix_w.as_ptr()),
                    None,
                    KEY_READ,
                    &mut root,
                )
            };
            if status.is_err() {
                continue;
            }
            // 子键数量与名字长度上限（一次查询，随后循环枚举）
            let mut subkeys = 0u32;
            let mut max_name = 0u32;
            let status = unsafe {
                RegQueryInfoKeyW(
                    root,
                    None,
                    None,
                    None,
                    Some(&mut subkeys),
                    Some(&mut max_name),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status.is_err() {
                unsafe {
                    let _ = RegCloseKey(root);
                }
                continue;
            }
            for index in 0..subkeys {
                let mut name_buf = vec![0u16; max_name as usize + 1];
                let mut name_len = name_buf.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(
                        root,
                        index,
                        Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                        &mut name_len,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                if status.is_err() || name_len == 0 {
                    continue;
                }
                let sub_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                let sub_path_w = wide(&format!("{prefix}\\{sub_name}"));
                let mut sub = HKEY::default();
                let status = unsafe {
                    RegOpenKeyExW(*hive, PCWSTR(sub_path_w.as_ptr()), None, KEY_READ, &mut sub)
                };
                if status.is_err() {
                    continue;
                }
                // InstallLocation 优先；没有就用 DisplayIcon 去掉参数与文件名
                let dir = read_sz(sub, "InstallLocation")
                    .map(PathBuf::from)
                    .or_else(|| {
                        read_sz(sub, "DisplayIcon").map(|icon| {
                            let path = icon.split(',').next().unwrap_or("").trim().to_string();
                            PathBuf::from(path)
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_default()
                        })
                    });
                unsafe {
                    let _ = RegCloseKey(sub);
                }
                if let Some(dir) = dir {
                    if dir.exists() {
                        callback(&dir);
                    }
                }
            }
            unsafe {
                let _ = RegCloseKey(root);
            }
        }
    }
}

#[cfg(not(windows))]
fn for_each_uninstall_location(_callback: &mut dyn FnMut(&PathBuf)) {}

/// 已加载的引擎模块句柄（进程内只加载一次，之后复用；存 HMODULE 的原始值）
#[cfg(windows)]
static ENGINE_LIB: OnceLock<Option<isize>> = OnceLock::new();

/// 构造 passport 换发用的**签名参数体**（调引擎导出函数）。
///
/// 引擎导出 `genflow_engine_get_login_auth_param(bduss, ptoken, &len) -> char*`
/// 与配套的 `genflow_engine_free`。实测**无需先 global_init**，单次调用即出
/// 完整参数体（`appid=1&bduss=…&ptoken=…&return_type=1&tpl=genflowpro&
/// tpl_list=…&sig=…`）。
#[cfg(windows)]
fn build_auth_param(bduss: &str, ptoken: &str) -> Result<String, GatewayError> {
    use windows::core::PCWSTR;
    // 句柄进程内常驻（见 ENGINE_LIB 注释），不需要 FreeLibrary
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let Some(path) = locate_engine_dll() else {
        return Err(GatewayError::with_status(
            502,
            "没有找到 KukuAI 客户端引擎（genflowengine.dll）：请确认已安装库库AI 客户端，\
             或用环境变量 WORKBUDDY_GENFLOW_ENGINE_DLL 指定其路径",
        ));
    };

    // 进程内只 LoadLibrary 一次；句柄留到进程退出（引擎 14MB，常驻无碍）
    let handle = match ENGINE_LIB.get() {
        Some(loaded) => loaded.clone(),
        None => {
            let path_w: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let loaded = unsafe { LoadLibraryW(PCWSTR(path_w.as_ptr())) }
                .map(|module| module.0 as isize)
                .map_err(|error| {
                    GatewayError::with_status(
                        502,
                        format!("加载 KukuAI 客户端引擎失败（{}）：{}", path.display(), error),
                    )
                })
                .ok();
            let _ = ENGINE_LIB.set(loaded.clone());
            loaded
        }
    };
    let Some(raw_handle) = handle else {
        return Err(GatewayError::with_status(502, "KukuAI 客户端引擎加载失败"));
    };
    let module = windows::Win32::Foundation::HMODULE(raw_handle as *mut core::ffi::c_void);

    unsafe {
        let proc_name = windows::core::s!("genflow_engine_get_login_auth_param");
        let addr = GetProcAddress(module, proc_name)
            .ok_or_else(|| GatewayError::with_status(502, "KukuAI 客户端引擎缺少登录参数导出"))?;
        let free_name = windows::core::s!("genflow_engine_free");
        let free_addr = GetProcAddress(module, free_name)
            .ok_or_else(|| GatewayError::with_status(502, "KukuAI 客户端引擎缺少内存释放导出"))?;

        type GetParam = unsafe extern "C" fn(
            bduss: *const std::ffi::c_char,
            ptoken: *const std::ffi::c_char,
            out_len: *mut std::ffi::c_int,
        ) -> *mut std::ffi::c_char;
        type FreeFn = unsafe extern "C" fn(ptr: *mut std::ffi::c_char) -> std::ffi::c_int;

        let get_param: GetParam = std::mem::transmute(addr);
        let free_fn: FreeFn = std::mem::transmute(free_addr);

        let bduss_c = std::ffi::CString::new(bduss)
            .map_err(|_| GatewayError::with_status(400, "BDUSS 含非法字符"))?;
        let ptoken_c = std::ffi::CString::new(ptoken)
            .map_err(|_| GatewayError::with_status(400, "PTOKEN 含非法字符"))?;

        let mut out_len: std::ffi::c_int = 0;
        let ptr = get_param(bduss_c.as_ptr(), ptoken_c.as_ptr(), &mut out_len);
        if ptr.is_null() || out_len <= 0 {
            return Err(GatewayError::with_status(
                502,
                "KukuAI 客户端引擎未能构造登录参数（返回为空）",
            ));
        }
        let bytes =
            std::slice::from_raw_parts(ptr.cast::<u8>(), out_len as usize).to_vec();
        let _ = free_fn(ptr);
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        if text.is_empty() {
            return Err(GatewayError::with_status(502, "KukuAI 客户端引擎返回了空参数"));
        }
        Ok(text)
    }
}

#[cfg(not(windows))]
fn build_auth_param(_bduss: &str, _ptoken: &str) -> Result<String, GatewayError> {
    Err(GatewayError::with_status(
        502,
        "KukuAI 会话换发仅在 Windows（依赖官方客户端引擎）可用",
    ))
}

/// 用 BDUSS + PTOKEN 换发「genflowpro 作用域」的 STOKEN。
///
/// 成功返回新 STOKEN；失败（没装客户端 / 换发被拒）返回带可读原因的错误，
/// 由调用方决定回退方式。
pub async fn exchange_genflowpro_stoken(bduss: &str, ptoken: &str) -> Result<String, GatewayError> {
    if bduss.trim().is_empty() || ptoken.trim().is_empty() {
        return Err(GatewayError::with_status(
            400,
            "换发 KukuAI 会话令牌需要 BDUSS 与 PTOKEN（登录 Cookie 里缺 PTOKEN）",
        ));
    }
    let body = build_auth_param(bduss, ptoken)?;
    let client = egress::client_for(None);
    let response = client
        .post(PASSPORT_AUTH_URL)
        .header("Content-Type", "application/x-www-form-urlencoded;charset=UTF-8")
        .header("Referer", format!("{}/genflowpro", super::BASE_URL))
        .header("Origin", super::BASE_URL)
        .header("User-Agent", USER_AGENT)
        .body(body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| {
            GatewayError::with_status(502, format!("换发 KukuAI 会话令牌失败：{error}"))
        })?;
    let status = response.status().as_u16();
    let value: Value = response.json().await.map_err(|error| {
        GatewayError::with_status(502, format!("换发 KukuAI 会话令牌响应异常：{error}"))
    })?;
    let errno = value.get("errno").and_then(Value::as_i64).unwrap_or(-1);
    if status != 200 || errno != 0 {
        let message = value
            .get("errmsg")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(GatewayError::with_status(
            502,
            format!("换发 KukuAI 会话令牌被拒（errno={errno}：{message}）"),
        ));
    }
    let stoken = value
        .pointer("/stoken_list/genflowpro")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if stoken.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "换发 KukuAI 会话令牌成功但响应缺少 genflowpro 令牌",
        ));
    }
    Ok(stoken)
}
