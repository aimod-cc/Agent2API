//! EasyClash 配置读取与快照缓存。
//!
//! 为什么单独一个模块：账号代理的解析（proxies.rs）只依赖「快照形状」，
//! 而快照自身的读取涉及多个平台默认数据目录、store.json 解析与一层 TTL 缓存，
//! 属于「外部环境探测」，与归一/解析这类纯逻辑混在一起会让两边都难改。
//!
//! 关键设计：EasyClash 侧的代理项是**严格镜像** —— 端口只在 EasyClash 里改，
//! 这里每次都从 EasyClash 的 store.json 实时读取，账号或代理池只记 listenerUid。
//! EasyClash 里删掉了某个节点端口映射，引用它的账号会解析失败并回退直连
//! （提示由 describe/resolve 给出）。
//!
//! 缓存：解析账号代理要读 store.json，桌面端列表也频繁刷新，因此加一层 3 秒
//! TTL 缓存 —— EasyClash 改端口最多延迟 3 秒生效。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::server::logging;

/// EasyClash 全局混合端口在账号配置里的保留 uid（与真实节点名不会冲突）
pub const CLASH_MIXED_UID: &str = "__mixed__";

/// EasyClash 全局混合端口在代理列表与选项里的默认名称
pub const CLASH_MIXED_NAME: &str = "EasyClash 混合端口";

/// EasyClash 配置未找到时的原因文案
pub const CLASH_UNAVAILABLE: &str = "未找到 EasyClash 配置";

/// EasyClash 的应用目录名（Tauri identifier，各平台 app_data_dir 下的目录名）
const CLASH_APP_ID: &str = "com.easyclash.desktop";

/// 快照缓存 TTL：EasyClash 改端口最多延迟 3 秒生效
const CLASH_CACHE_TTL_MS: i64 = 3000;

/// 代理配置里 listenerUid 的最大字符长度（与 proxies.rs 保持一致）
const MAX_LISTENER_UID_CHARS: usize = 100;

/// EasyClash 端口快照。
#[derive(Clone, Debug)]
pub struct ClashSnapshot {
    pub available: bool,
    pub dir: Option<String>,
    pub mixed_port: Option<u16>,
    #[allow(dead_code)]
    pub socks_port: Option<u16>,
    #[allow(dead_code)]
    pub http_port: Option<u16>,
    pub listeners: Vec<ClashListener>,
    pub current_profile_uid: Option<String>,
    pub error: Option<String>,
}

impl ClashSnapshot {
    /// 「找不到配置」或「配置不可读」快照
    fn unavailable(error: &str, dir: Option<PathBuf>) -> Self {
        Self {
            available: false,
            dir: dir.map(|path| path.to_string_lossy().to_string()),
            mixed_port: None,
            socks_port: None,
            http_port: None,
            listeners: Vec::new(),
            current_profile_uid: None,
            error: Some(error.to_string()),
        }
    }

    /// 判断某个 `listenerUid` 当前是否仍存在于 EasyClash 实时配置中。
    /// - EasyClash 不可用（未安装 / 找不到 store.json / 配置不可读）→ `false`
    /// - `__mixed__` → `self.mixed_port.is_some()`
    /// - 其它 uid → `self.listeners` 中存在同 uid 项（不论该项在 EasyClash 中启停）
    pub fn contains_listener(&self, listener_uid: &str) -> bool {
        if !self.available {
            return false;
        }
        if listener_uid == CLASH_MIXED_UID {
            return self.mixed_port.is_some();
        }
        self.listeners.iter().any(|listener| listener.uid == listener_uid)
    }
}

/// EasyClash 的一个节点独立混合端口映射
#[derive(Clone, Debug)]
pub struct ClashListener {
    pub uid: String,
    pub name: String,
    #[allow(dead_code)]
    pub proxy: String,
    pub port: u16,
    pub profile_uid: Option<String>,
    pub enabled: bool,
}

// ─── 配置目录与 JSON 读取 ───────────────────────────────────

/// 用户主目录。
///
/// Windows 上优先 USERPROFILE；这里按同一顺序取，不额外引入 dirs crate。
fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// EasyClash 默认配置目录（对应 Tauri `app.path().app_data_dir()`）：
///
///   ① `%APPDATA%\com.easyclash.desktop`（Windows 常规位置）
///   ② `$XDG_DATA_HOME/com.easyclash.desktop`（Linux 自定义 XDG 数据目录）
///   ③ `~/AppData/Roaming/com.easyclash.desktop`（APPDATA 缺失时的回落）
///   ④ `~/Library/Application Support/com.easyclash.desktop`（macOS）
///   ⑤ `~/.local/share/com.easyclash.desktop`（Linux 默认位置）
///   ⑥ `~/.config/com.easyclash.desktop`（兜底位置）
///
/// 判定标准是「目录里存在 store.json」—— EasyClash 启动时会把端口与设置写入 store.json。
pub fn clash_config_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        let trimmed = appdata.trim();
        if !trimmed.is_empty() {
            candidates.push(Path::new(trimmed).join(CLASH_APP_ID));
        }
    }
    if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
        let trimmed = xdg_data.trim();
        if !trimmed.is_empty() {
            candidates.push(Path::new(trimmed).join(CLASH_APP_ID));
        }
    }
    if let Some(home) = home_dir() {
        candidates.push(home.join("AppData").join("Roaming").join(CLASH_APP_ID));
        candidates.push(
            home.join("Library")
                .join("Application Support")
                .join(CLASH_APP_ID),
        );
        candidates.push(home.join(".local").join("share").join(CLASH_APP_ID));
        candidates.push(home.join(".config").join(CLASH_APP_ID));
    }
    candidates
        .into_iter()
        .find(|dir| dir.join("store.json").exists())
}

/// 读一个 JSON 文件；缺失/解析失败/顶层不是对象都返回 None。
///
/// 刻意吞掉错误：store.json 可能正被 EasyClash 写入（读到半截），
/// 这时应当视为「本轮读不到」，而不是把异常抛给调用方。
fn read_json(path: &Path) -> Option<Value> {
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    parsed.is_object().then_some(parsed)
}

/// JSON 里的端口值 → 端口号（1..=65535 整数）。
fn json_port(value: Option<&Value>) -> Option<u16> {
    let number = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !number.is_finite() || number.fract() != 0.0 || !(1.0..=65535.0).contains(&number) {
        return None;
    }
    Some(number as u16)
}

/// 取 JSON 字符串并去空白（非字符串一律当空串）
fn json_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        _ => String::new(),
    }
}

/// 为节点端口映射生成稳定的 uid：
/// 直接按节点名生成（而不是按端口号），这样用户在 EasyClash 里修改节点端口时，
/// 代理池和账号引用会自动跟随新端口而不会失效。
fn listener_uid(node_name: &str) -> String {
    if node_name != CLASH_MIXED_UID && node_name.chars().count() <= MAX_LISTENER_UID_CHARS {
        return node_name.to_string();
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in node_name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("node:{hash:016x}")
}

/// 读取 EasyClash 的端口快照（主混合端口 + 节点独立端口映射）。
///
/// EasyClash 未安装或配置不可读时 `available=false`，不抛异常：
///   ① 候选目录里都没有 store.json → dir=null + error=未找到 EasyClash 配置
///   ② store.json 解析失败 → dir 有值 + error=store.json 解析失败
///   ③ 正常读取 → error=null + 主端口与端口映射列表
pub fn read_easyclash_config() -> ClashSnapshot {
    let Some(dir) = clash_config_dir() else {
        return ClashSnapshot::unavailable(CLASH_UNAVAILABLE, None);
    };
    let Some(store) = read_json(&dir.join("store.json")) else {
        return ClashSnapshot::unavailable("store.json 解析失败", Some(dir));
    };

    let settings = store.get("settings").and_then(Value::as_object);
    let mixed_port = json_port(settings.and_then(|map| map.get("mainPort")));
    let controller_port = json_port(settings.and_then(|map| map.get("controllerPort")));

    // 节点独立端口映射：store.json 里的 ports[] 每项形如 { "nodeName": "...", "port": 18001 }。
    // 与 EasyClash 生成 runtime.yaml 保持一致：忽略无效项以及与主端口/控制端口冲突的项。
    let mut listeners: Vec<ClashListener> = Vec::new();
    let mut seen_uids = HashSet::new();
    if let Some(items) = store.get("ports").and_then(Value::as_array) {
        for item in items {
            let Some(mapping) = item.as_object() else {
                continue;
            };
            let node_name = json_text(mapping.get("nodeName"));
            if node_name.is_empty() {
                continue;
            }
            let Some(port) = json_port(mapping.get("port")) else {
                continue;
            };
            if Some(port) == mixed_port || Some(port) == controller_port {
                continue;
            }
            let uid = listener_uid(&node_name);
            if !seen_uids.insert(uid.clone()) {
                continue;
            }
            listeners.push(ClashListener {
                uid,
                name: node_name.clone(),
                proxy: node_name,
                port,
                profile_uid: None,
                enabled: true,
            });
        }
    }

    let current_profile_uid = {
        let current = json_text(store.get("activeSubscriptionId"));
        if current.is_empty() { None } else { Some(current) }
    };

    ClashSnapshot {
        available: true,
        dir: Some(dir.to_string_lossy().to_string()),
        mixed_port,
        socks_port: None,
        http_port: None,
        listeners,
        current_profile_uid,
        error: None,
    }
}

// ─── 快照缓存 ───────────────────────────────────────────────

/// 进程内快照缓存：`(取快照的时刻, 快照)`。
static CLASH_CACHE: Mutex<Option<(i64, ClashSnapshot)>> = Mutex::new(None);

/// 取快照（默认走 3 秒 TTL 缓存）
pub fn clash_snapshot() -> ClashSnapshot {
    clash_snapshot_with(false)
}

/// 取快照，`force=true` 时跳过缓存（前端「重新读取」/「同步」按钮用得上）。
pub fn clash_snapshot_with(force: bool) -> ClashSnapshot {
    let now = logging::now_ms();
    let mut guard = match CLASH_CACHE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !force {
        if let Some((at, snapshot)) = guard.as_ref() {
            if now - *at < CLASH_CACHE_TTL_MS {
                return snapshot.clone();
            }
        }
    }
    let snapshot = read_easyclash_config();
    *guard = Some((now, snapshot.clone()));
    snapshot
}

/// EasyClash 代理可选项（GET /api/proxies 与 /api/session 用）。
pub fn clash_proxy_options() -> Value {
    let snapshot = clash_snapshot();
    if !snapshot.available {
        return json!({
            "available": false,
            "error": snapshot.error,
            "dir": snapshot.dir,
            "options": [],
        });
    }
    let mut options = Vec::new();
    if let Some(port) = snapshot.mixed_port {
        options.push(json!({
            "uid": CLASH_MIXED_UID,
            "name": CLASH_MIXED_NAME,
            "port": port,
            "kind": "mixed",
            "enabled": true,
            "profileActive": true,
        }));
    }
    for listener in &snapshot.listeners {
        let active = listener
            .profile_uid
            .as_ref()
            .map(|uid| Some(uid) == snapshot.current_profile_uid.as_ref())
            .unwrap_or(true);
        options.push(json!({
            "uid": listener.uid,
            "name": listener.name,
            "port": listener.port,
            "kind": "listener",
            "enabled": listener.enabled,
            "profileActive": active,
        }));
    }
    json!({
        "available": true,
        "error": Value::Null,
        "dir": snapshot.dir,
        "options": options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_listener(uid: &str, enabled: bool) -> ClashListener {
        ClashListener {
            uid: uid.to_string(),
            name: uid.to_string(),
            proxy: uid.to_string(),
            port: 18001,
            profile_uid: None,
            enabled,
        }
    }

    #[test]
    fn contains_listener_handles_unavailable_and_present_endpoints() {
        let unavailable = ClashSnapshot::unavailable(CLASH_UNAVAILABLE, None);
        assert!(!unavailable.contains_listener(CLASH_MIXED_UID));
        assert!(!unavailable.contains_listener("node-a"));

        let available_no_mixed = ClashSnapshot {
            available: true,
            dir: Some("/tmp/easyclash".to_string()),
            mixed_port: None,
            socks_port: None,
            http_port: None,
            listeners: vec![sample_listener("node-a", true), sample_listener("node-off", false)],
            current_profile_uid: None,
            error: None,
        };
        assert!(!available_no_mixed.contains_listener(CLASH_MIXED_UID));
        assert!(available_no_mixed.contains_listener("node-a"));
        assert!(available_no_mixed.contains_listener("node-off"));
        assert!(!available_no_mixed.contains_listener("missing"));

        let available_with_mixed = ClashSnapshot {
            mixed_port: Some(7890),
            ..available_no_mixed
        };
        assert!(available_with_mixed.contains_listener(CLASH_MIXED_UID));
    }
}
