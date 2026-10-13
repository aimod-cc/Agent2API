//! Command Code 的**设备指纹**（确定性派生）与两条预请求（握手）。
//!
//! ── 上游为什么要这两条预请求（逆向结论，见规格 §4）──────────────
//! `device_fingerprints` 表按 `(userId, thumbmark)` 建唯一索引。官方 CLI 每次
//! 启动上报一次「我在哪台机器上」，反代必须复刻同一件事：
//! ```text
//!   POST /alpha/fingerprint/record   {thumbmark, components{15 项}}
//!   POST /alpha/lifecycle-events     {eventType:"cli_session_exists", metadata{…}}
//! ```
//! 两条**并行发出**，每个 key 首次请求前 + 每 8h（+0~2h 抖动）一次；
//! **失败不阻塞生成**（只告警，下次请求再试）。
//!
//! ── 为什么指纹必须由 apiKey **确定性派生**（而不是随机 / 取模）───
//!   1. **随机**：进程重启、内存回收、多实例都会无故换机器 —— 设备身份漂移
//!      本身就是可疑信号；
//!   2. **取模分桶**（`hash(key) % 候选数`）：桶数就是熵上限，key 数一超过桶数
//!      必然出现多个 key 共用指纹，而上游那张表上「共用指纹 = 多账号同机」的
//!      直接证据；
//!   3. **本实现**：`sha256(salt\0apiKey\0字段)` 逐字段派生，每 key 都是独立设备
//!      （碰撞概率 2^-256），且**同一 key 每次得到同一个指纹** —— 不落盘也能
//!      重启一致（这正是「指纹不持久化」的前提）。
//! 从候选池里挑一项用「打分取最大」而不是取模：往池里加候选只影响「新候选恰好
//! 胜出」的那部分 key，不会像取模那样因池长度变化让所有 key 一起换设备。
//!
//! ── 明文与哈希的边界（出网只有哈希）──────────────────────────
//! 哈希字段（`machineIdHash` / `macHashes` / `osUserHash` / `hostnameHash` /
//! `gitEmailHash`）的明文（伪造的 GUID / MAC / 用户名 / 主机名 / 邮箱）只存在
//! 于本函数的栈上；出网的是 `sha256(FP_SALT\0小写明文)`。明文字段
//! （cpuModel / cpuCount / memGiB / timezone …）上游会交叉核对，
//! 因此 `cpuCount` 取**逻辑处理器数**（对齐 CLI 的 `os.cpus().length`），
//! 不取物理核数。
//!
//! ── 本机真实信息一个字都不出网 ─────────────────────────────────
//! 平台 / 架构 / 系统版本 / 项目目录全部用**设备档案**里的伪造常量
//! （见 [`device_project_dir`] 与 [`DEVICE_PLATFORM`]）—— 同一个档案也供
//! `plan.rs` 的 `config` 块与 `x-project-slug` 使用，避免出现「指纹说 win32、
//! 环境说 linux」这类自相矛盾。
//!
//! ── TTL 状态放在哪（为什么内存里按 key 记时间戳）────────────────
//! 规格 §4.5 明确「**不持久化到磁盘**」：刷新周期是客户端侧约定（服务端不下发
//! TTL），而指纹本身是确定性派生的，重启后重算即同值。因此本模块只用一张
//! **进程内存**表（`key 摘要 → 上次上报时刻`），重启丢失的后果仅仅是
//! 「重启后第一轮请求重新上报一次指纹/lifecycle」—— 那正是参考实现的行为。
//! 表里存的是 **key 的 sha256 摘要**而不是明文 key：内存里少一份凭证副本。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::core::auth_http::send_raw;
use crate::server::logging;

use super::endpoints;

/// CLI 的根盐（`buildMachineFingerprint` 的常量 `sb`；参与最终哈希，不可改）
pub const FP_SALT: &str = "command-code:device-fingerprint:v1";

/// 伪造的设备档案：平台 / 架构 / 系统版本（**不读宿主真实平台**）
pub const DEVICE_PLATFORM: &str = "win32";
/// 见 [`DEVICE_PLATFORM`]
pub const DEVICE_ARCH: &str = "x64";
/// 见 [`DEVICE_PLATFORM`]（Windows 11 23H2 的版本串，与参考实现一致）
pub const DEVICE_OS_RELEASE: &str = "10.0.22631";

/// 伪造的默认项目目录（参考实现的 `deviceProjectDir`）。
///
/// 为什么发伪造值而不是宿主真实 cwd：真实路径会泄露用户名与目录结构，
/// 参考实现从头到尾就不发宿主真值（见规格 §4.3 的设备档案）。
pub const DEFAULT_DEVICE_PROJECT_DIR: &str = "C:\\Users\\dev\\projects\\app";

/// CPU 候选表（型号 + 物理核数 + 逻辑处理器数）。
///
/// `threads` 才是出网值（见模块头）；`cores` 只参与候选标签的构造 —— 标签必须
/// 与参考实现逐字一致（`model|cores`），否则同一个 key 会挑出不同的设备。
const CPUS: [(&str, i64, i64); 15] = [
    ("12th Gen Intel(R) Core(TM) i7-12650H", 10, 16),
    ("12th Gen Intel(R) Core(TM) i5-12400F", 6, 12),
    ("12th Gen Intel(R) Core(TM) i9-12900K", 16, 24),
    ("13th Gen Intel(R) Core(TM) i7-13700K", 16, 24),
    ("13th Gen Intel(R) Core(TM) i5-13600K", 14, 20),
    ("13th Gen Intel(R) Core(TM) i9-13900K", 24, 32),
    ("Intel(R) Core(TM) Ultra 7 155H", 16, 22),
    ("Intel(R) Core(TM) Ultra 9 285H", 16, 16),
    ("Intel(R) Core(TM) i9-14900K", 24, 32),
    ("Intel(R) Core(TM) i7-14700K", 20, 28),
    ("AMD Ryzen 7 7800X3D", 8, 16),
    ("AMD Ryzen 9 7950X", 16, 32),
    ("AMD Ryzen 5 7600", 6, 12),
    ("AMD Ryzen 9 7900X", 12, 24),
    ("AMD Ryzen 7 5800X3D", 8, 16),
];

/// 内存（GiB）候选
const MEMS: [i64; 6] = [8, 16, 24, 32, 48, 64];

/// 时区候选（本机 OS 时区语义，**不与出口 IP 绑定**）
const TIMEZONES: [&str; 15] = [
    "America/New_York",
    "America/Chicago",
    "America/Los_Angeles",
    "America/Toronto",
    "Europe/London",
    "Europe/Berlin",
    "Europe/Paris",
    "Europe/Moscow",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Asia/Singapore",
    "Asia/Seoul",
    "Asia/Hong_Kong",
    "Australia/Sydney",
    "Pacific/Auckland",
];

/// MAC 地址条数候选（2~5）
const MAC_COUNTS: [i64; 4] = [2, 3, 4, 5];

/// 系统用户名候选
const OS_USERS: [&str; 6] = ["dev", "user", "admin", "coder", "engineer", "work"];

/// git 邮箱域名候选
const MAIL_DOMAINS: [&str; 4] = ["gmail.com", "outlook.com", "qq.com", "163.com"];

/// 指纹刷新周期（8 小时）
const INIT_REFRESH_MS: i64 = 8 * 60 * 60 * 1000;
/// 刷新抖动上限（0~2 小时随机）
const INIT_JITTER_MS: i64 = 2 * 60 * 60 * 1000;
/// 预请求超时（比目录查询略宽：两条请求要跑两次 TLS 握手）
const PREFLIGHT_TIMEOUT_MS: u64 = 20_000;

/// 一次派生出的设备指纹
#[derive(Clone, Debug)]
pub struct DeviceFingerprint {
    /// 设备指纹（64 位小写 hex；上游 `(userId, thumbmark)` 的唯一键）
    pub thumbmark: String,
    /// 15 项组件（出网的完整形状）
    pub components: Value,
}

impl DeviceFingerprint {
    /// `POST /alpha/fingerprint/record` 的请求体（参考实现直接序列化整个对象）
    pub fn record_payload(&self) -> Value {
        json!({
            "thumbmark": self.thumbmark,
            "components": self.components,
        })
    }
}

/// 读环境变量覆盖（空值视为未设置）
fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 派生盐：`COMMANDCODE_FINGERPRINT_SALT` / `CC_FINGERPRINT_SALT` 可覆盖。
///
/// 它**只影响「伪造出哪台机器」**，不参与最终哈希（最终哈希固定用 [`FP_SALT`]）。
/// 留空时派生是 apiKey 的纯函数（可被反推），因此参考实现建议设盐；
/// 多实例部署必须设同一个值，否则同一 key 在不同实例上会看到两台设备。
pub fn fingerprint_salt() -> String {
    env_override("COMMANDCODE_FINGERPRINT_SALT")
        .or_else(|| env_override("CC_FINGERPRINT_SALT"))
        .unwrap_or_default()
}

/// 伪造的项目目录：`COMMANDCODE_DEVICE_PROJECT_DIR` / `CC_DEVICE_PROJECT_DIR` 可覆盖
pub fn device_project_dir() -> String {
    env_override("COMMANDCODE_DEVICE_PROJECT_DIR")
        .or_else(|| env_override("CC_DEVICE_PROJECT_DIR"))
        .unwrap_or_else(|| DEFAULT_DEVICE_PROJECT_DIR.to_string())
}

/// CLI 的 slug 规则：整串小写、非字母数字折成 `-`、去首尾 `-`，空则 `root`。
///
/// 与 `x-project-slug`（`plan.rs` 生成）同源：真机里 slug 就是
/// `slugify(workingDir)`，两处不一致是可观测的矛盾。
pub fn slugify_project_path(path: &str) -> String {
    let lowered = path.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "root".to_string()
    } else {
        out
    }
}

/// `sha256("<salt>\0<apiKey>\0<field>")`（返回原始 32 字节）
fn fp_digest(api_key: &str, field: &str) -> [u8; 32] {
    let salt = fingerprint_salt();
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update([0u8]);
    hasher.update(api_key.as_bytes());
    hasher.update([0u8]);
    hasher.update(field.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..]);
    out
}

/// 从候选池里确定性地挑一项：`argmax_i digest(apiKey, "{field}\0{label_i}")`。
///
/// 比较是**词典序字节比较**（对齐参考实现的 `Buffer.compare`），不是取模。
fn fp_pick_index(api_key: &str, field: &str, labels: &[String]) -> usize {
    let mut best_index = 0usize;
    let mut best_score: Option<[u8; 32]> = None;
    for (index, label) in labels.iter().enumerate() {
        let score = fp_digest(api_key, &format!("{field}\0{label}"));
        if best_score.map(|best| score > best).unwrap_or(true) {
            best_score = Some(score);
            best_index = index;
        }
    }
    best_index
}

/// 小写 hex
fn hex_of(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// CLI 的 `hashSignal`：`sha256(FP_SALT\0小写明文)` 的 hex。
///
/// 空值在参考实现里被丢弃（返回 undefined）。本模块的调用点传入的都是
/// 派生出来的非空值，因此这里对空输入给空串、并在组件里**不插入**该键
/// （与参考的 `JSON.stringify` 丢 undefined 同语义）。
fn fingerprint_hash(value: &str) -> Option<String> {
    let lowered = value.trim().to_lowercase();
    if lowered.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(FP_SALT.as_bytes());
    hasher.update([0u8]);
    hasher.update(lowered.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

/// 由 apiKey 确定性地派生一整套设备指纹（规格 §4.3 的逐字段实现）。
pub fn generate(api_key: &str) -> DeviceFingerprint {
    // 候选标签必须与参考实现逐字一致（标签是派生输入的一部分）
    let cpu_labels: Vec<String> = CPUS
        .iter()
        .map(|(model, cores, _)| format!("{model}|{cores}"))
        .collect();
    let cpu = CPUS[fp_pick_index(api_key, "cpu", &cpu_labels)];
    let mem_labels: Vec<String> = MEMS.iter().map(|value| value.to_string()).collect();
    let mem_gib = MEMS[fp_pick_index(api_key, "mem", &mem_labels)];
    let tz_labels: Vec<String> = TIMEZONES.iter().map(|value| (*value).to_string()).collect();
    let timezone = TIMEZONES[fp_pick_index(api_key, "timezone", &tz_labels)];
    let mac_labels: Vec<String> = MAC_COUNTS.iter().map(|value| value.to_string()).collect();
    let mac_count = MAC_COUNTS[fp_pick_index(api_key, "macCount", &mac_labels)];
    let user_labels: Vec<String> = OS_USERS.iter().map(|value| (*value).to_string()).collect();
    let os_user = OS_USERS[fp_pick_index(api_key, "osUser", &user_labels)];
    let domain_labels: Vec<String> = MAIL_DOMAINS
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let mail_domain = MAIL_DOMAINS[fp_pick_index(api_key, "mailDomain", &domain_labels)];

    // 派生字段：hex(前 N 字节)（见规格 §4.3 的表）
    let hex_field = |field: &str, bytes: usize| -> String {
        let digest = fp_digest(api_key, field);
        hex_of(&digest[..bytes.min(digest.len())])
    };
    // Windows MachineGuid 形状：8-4-4-4-12
    let mid = hex_field("machineId", 16);
    let machine_id = if mid.len() == 32 {
        format!(
            "{}-{}-{}-{}-{}",
            &mid[0..8],
            &mid[8..12],
            &mid[12..16],
            &mid[16..20],
            &mid[20..32]
        )
    } else {
        mid.clone()
    };
    let mut macs: Vec<String> = Vec::new();
    for index in 0..mac_count.max(0) {
        let digest = fp_digest(api_key, &format!("mac{index}"));
        let parts: Vec<String> = digest[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        macs.push(parts.join(":"));
    }
    // CLI 对 MAC 去重后排序
    macs.sort();
    macs.dedup();
    let hostname = format!("DESKTOP-{}", hex_field("hostname", 4).to_uppercase());
    let git_email = format!("{os_user}.{}@{mail_domain}", hex_field("gitEmail", 3));

    // thumbmark：`sha256(FP_SALT + "\0machine\0" + join('|'))`；machineId 非空时
    // 不再拼 hostname / cpuModel（参考实现里 machineId 恒非空，走的就是这一支）
    let thumb_seed: Vec<String> = vec![
        machine_id.trim().to_string(),
        macs.join(","),
        if machine_id.trim().is_empty() {
            hostname.clone()
        } else {
            String::new()
        },
        if machine_id.trim().is_empty() {
            cpu.0.to_string()
        } else {
            String::new()
        },
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect();
    let seed = if thumb_seed.is_empty() {
        "unknown".to_string()
    } else {
        thumb_seed.join("|")
    };
    let thumbmark = {
        let mut hasher = Sha256::new();
        hasher.update(FP_SALT.as_bytes());
        hasher.update(b"\0machine\0");
        hasher.update(seed.as_bytes());
        format!("{:x}", hasher.finalize())
    };

    // 15 项组件：哈希字段走 fingerprint_hash（本实现里都非空），明文字段直接给
    let mut components = serde_json::Map::new();
    for (key, value) in [
        ("machineIdHash", machine_id.as_str()),
        ("osUserHash", os_user),
        ("hostnameHash", hostname.as_str()),
        ("gitEmailHash", git_email.as_str()),
    ] {
        if let Some(hashed) = fingerprint_hash(value) {
            components.insert(key.to_string(), Value::String(hashed));
        }
    }
    let mac_hashes: Vec<Value> = macs
        .iter()
        .filter_map(|mac| fingerprint_hash(mac).map(Value::String))
        .collect();
    if !mac_hashes.is_empty() {
        components.insert("macHashes".to_string(), Value::Array(mac_hashes));
    }
    components.insert(
        "platform".to_string(),
        Value::String(DEVICE_PLATFORM.to_string()),
    );
    components.insert("arch".to_string(), Value::String(DEVICE_ARCH.to_string()));
    components.insert(
        "osRelease".to_string(),
        Value::String(DEVICE_OS_RELEASE.to_string()),
    );
    components.insert("cpuModel".to_string(), Value::String(cpu.0.to_string()));
    // cpuCount 取 **threads**（逻辑处理器数）：CLI 的 gatherRawSignals 取
    // `os.cpus().length`，明文对会被服务端交叉核对（见模块头）
    components.insert("cpuCount".to_string(), Value::from(cpu.2));
    components.insert("memGiB".to_string(), Value::from(mem_gib));
    components.insert("isContainer".to_string(), Value::Bool(false));
    components.insert("timezone".to_string(), Value::String(timezone.to_string()));
    components.insert("runtime".to_string(), Value::String("cli".to_string()));
    components.insert("collectorVersion".to_string(), Value::from(1));

    DeviceFingerprint {
        thumbmark,
        components: Value::Object(components),
    }
}

/// 生成一个 W3C traceparent（`00-<32hex trace>-<16hex parent>-01`）
pub fn traceparent() -> String {
    format!("00-{}-{}-01", random_hex(16), random_hex(8))
}

/// `N` 字节的随机 hex（随机源不可用时回落时间戳 —— 它只是追踪串，不是凭证）
fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    if getrandom::getrandom(&mut buffer).is_err() {
        return format!("{:0>width$x}", logging::now_ms() as u64, width = bytes * 2);
    }
    hex_of(&buffer)
}

/// lifecycle 事件的请求体（`cli_session_exists`）。
///
/// 形状**恰为** `{eventType, metadata{sessionId, cliVersion, mode, os}}` ——
/// 15 项组件之外一个字段都不能多（上游按形状核对）。
fn lifecycle_payload() -> Value {
    json!({
        "eventType": "cli_session_exists",
        "metadata": {
            // 每次上报一个随机 id（参考实现同款；它不是聊天会话 id）
            "sessionId": format!("sess_{}", random_hex(8)),
            "cliVersion": endpoints::protocol_version(),
            "mode": "interactive",
            "os": format!("{DEVICE_PLATFORM}-{DEVICE_ARCH}"),
        },
    })
}

/// 每 key 的初始化节流状态
#[derive(Default)]
struct InitState {
    /// 有预请求在途（并发请求不重复上报 —— 参考实现的语义是「同 key 只上报一次」）
    in_flight: bool,
    /// 下一次可以上报的时刻（0 = 从没成功过，下次请求就试）
    next_init_at: i64,
}

/// 进程内存的 key → 节流状态（key 用 sha256 摘要标识，内存里不放第二份明文）
static INIT_STATES: OnceLock<Mutex<HashMap<String, InitState>>> = OnceLock::new();

/// key 的内存标识（摘要前 32 个 hex 字符足够区分，且不可反推）
fn key_id(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    format!("{:x}", hasher.finalize())[..32].to_string()
}

/// 取节流表（毒锁按「继续用」处理：这张表丢了只会多上报一次指纹）
fn state_table() -> &'static Mutex<HashMap<String, InitState>> {
    INIT_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 确保「指纹已上报」（每 key 首次 + 每 8h+2h 抖动）—— **best-effort**。
///
/// 契约（与参考实现的 `ensureInitialized` 逐条对齐）：
///   - 未到期直接返回（零开销的快速路径）；
///   - 有在途上报时直接返回（不重复打上游）；
///   - 两条预请求**并行**发出，**任何失败都不影响生成**（只告警）；
///   - `next_init_at` 只在**两条都拿到 HTTP 响应**时推进 —— 传输层失败
///     （连不上 / 超时）不推进，于是「下次请求再试」这一条成立。
///
/// 调用点：`adapter::ensure_access_token`（每次转发前的凭证准备）。放在那里
/// 而不是 `build_chat_request` 里：后者是同步的（不能 await 网络），而
/// `ensure_access_token` 本来就是异步的凭证准备钩子 —— 语义与「发正式请求前
/// 确保伪装已就位」正好对上。
pub async fn ensure_initialized(api_key: &str) {
    let key = api_key.trim();
    if key.is_empty() {
        return;
    }
    let id = key_id(key);
    let now = logging::now_ms();
    {
        let mut guard = match state_table().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let state = guard.entry(id.clone()).or_default();
        if state.in_flight || now < state.next_init_at {
            return;
        }
        state.in_flight = true;
    }

    let fingerprint = generate(key);
    let headers = {
        let mut headers = endpoints::cli_headers();
        headers.extend(endpoints::auth_headers(key));
        headers
    };
    // 临时值先落成局部量：`futures::join!` 的 future 会跨 await 持有这些引用
    let fingerprint_url = endpoints::url(endpoints::FINGERPRINT_PATH);
    let lifecycle_url = endpoints::url(endpoints::LIFECYCLE_PATH);
    let fingerprint_body = fingerprint.record_payload();
    let lifecycle_body = lifecycle_payload();
    let (fingerprint_result, lifecycle_result) = futures::join!(
        post_preflight(
            &fingerprint_url,
            &headers,
            &fingerprint_body,
            "设备指纹上报",
        ),
        post_preflight(
            &lifecycle_url,
            &headers,
            &lifecycle_body,
            "生命周期事件上报",
        ),
    );
    // 「两条都拿到了 HTTP 响应」才算这一轮走完（非 2xx 也算 —— 参考实现在
    // 那种情况下同样推进节流，只在日志里告警）
    let settled = fingerprint_result.is_ok() && lifecycle_result.is_ok();

    {
        let mut guard = match state_table().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(state) = guard.get_mut(&id) {
            state.in_flight = false;
            if settled {
                state.next_init_at = logging::now_ms() + INIT_REFRESH_MS + random_jitter();
            }
        }
    }
    if !settled {
        // 失败不阻塞生成：这一行是 verbose（每个 key 每次刷新周期最多两条）
        logging::verbose(
            "[CommandCode]",
            "设备指纹 / 生命周期预请求失败（不影响生成，下次请求再试）",
        );
    }
}

/// 0~2 小时的随机抖动（随机源不可用时给 0）
fn random_jitter() -> i64 {
    let mut buffer = [0u8; 4];
    if getrandom::getrandom(&mut buffer).is_err() {
        return 0;
    }
    let value = u32::from_le_bytes(buffer) as i64;
    value % (INIT_JITTER_MS + 1)
}

/// 发一条预请求；返回 `Err(())` 仅表示**传输层**没走完（拿到任何 HTTP 状态都算 Ok）
async fn post_preflight(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    what: &str,
) -> Result<(), ()> {
    match send_raw(
        "POST",
        url,
        Some(body),
        headers,
        None,
        Some(PREFLIGHT_TIMEOUT_MS),
    )
    .await
    {
        Ok(response) => {
            if !response.ok {
                logging::verbose(
                    "[CommandCode]",
                    &format!("{what}返回 HTTP {}（不阻塞生成）", response.status),
                );
            }
            Ok(())
        }
        Err(error) => {
            logging::verbose(
                "[CommandCode]",
                &format!("{what}失败：{error}（不阻塞生成）"),
            );
            Err(())
        }
    }
}
