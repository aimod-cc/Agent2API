//! Marvis 余额读数：**从本机客户端日志里读「每天 1000 万 Token」的钱包**。
//!
//! ── 一句话 ──────────────────────────────────────────────────
//! 上游钱包接口（`/v3/marvis_get_wallet`）要 CloudAssetService 的 AK/SK，
//! 网关拿不到（五条路全堵，见文末排除记录）。但**客户端自己已经把那笔读数
//! 写进了日志** —— 用户装了客户端、登了录，那个数就在磁盘上。本模块读它，
//! 归一成 `query_usage` 的契约形状（`available` + `wallets[]`）。
//!
//! ── 读数长什么样（`Marvis.log` 原文，实测 2026-10-21）───────────
//! ```text
//! [AGUIConnection] agent.action 响应: {"action":"token.wallet","data":{
//!   "avail_tokens":"9981078","frozen_tokens":"0","openid":"oAN1…",
//!   "remaining_tokens":0,"saved_tokens":0,"status":1,
//!   "total_cost":0,"total_tokens":"10000000"}}
//! ```
//! `total_tokens` 是**日额度**（官方口径 1000 万），`avail_tokens` 是当日剩余，
//! `frozen_tokens` 是冻结量（`avail + frozen` 才是当天总量；本机实测冻结恒 0）。
//! 四次采样递减（10000000 → 9989044 → 9988730 → 9981078），确认它就是
//! 「当天还能用多少」。
//!
//! ── 为什么读日志，而不是抄客户端那条链 ──────────────────────
//! 客户端的链是 `UI(CEF) --IPC token.wallet--> MarvisAgent --CloudAssetService--> 上游`。
//! 两段都过不去：
//!   · IPC：`\\.\pipe\tmp\marvisagent_*` 存在，但客户端给管道设了受限 DACL，
//!     同用户 `CreateFileW` 直接 `ERROR_ACCESS_DENIED(5)`（Python 侧
//!     `PermissionError [Errno 13]`）；
//!   · 上游：见文末排除记录（AK/SK 拿不到）。
//! 而日志是**普通文件、当前用户可读**，客户端每次刷新钱包都会追加一行。
//! 读它等于「用用户自己的客户端当采样器」—— 不引入新逆向链，不需要 UAC。
//!
//! ── 四条必须守住的纪律 ──────────────────────────────────────
//!   1. **只认本账号的读数**：日志里可能同时有别的 openid（同一台机器登过两个号）。
//!      判定靠 `data.openid` 精确匹配；读数的 `.data` 里没有 openid 时**跳过**
//!      （不按行内文本搜，也不拿别人的读数冒充自己的）。
//!   2. **跨天作废**：日额度每天重置，昨天的读数拿今天的日期解释就是错的。
//!      不是同一个自然日 → 直接不用（`freshness` 里返回原因，调用方报错）。
//!   3. **陈旧如实说**：同一天但超过 [`MAX_AGE_MS`] 的读数仍然返回数值，
//!      另挂 `warning` 说明它有多旧 —— 整次判失败会让用户以为没读到。
//!   4. **不猜总量、不编到期**：上游没给 `total_tokens` 就不写 `total`；
//!      日额度没有「到期时刻」，**绝不写 `credits[]`** —— 那会把账号页
//!      「最近到期 / 最近到期积分」两列填成一个假的到期日。
//!
//! ── 排除记录：为什么没有直连上游（2026-10-21，逐路实测）──────────
//! ```text
//!   POST /v3/marvis_get_wallet            bid=marvis_client → 403
//!                                         bid=pc_yyb 等 8 个 → 400（响应体为空）
//!   POST /marvis_client/marvis_create_aksk  文档串标 auth_mode=UAL_ONLY（只要登录头）
//!                                         3 businessid × 8 body = 24 组 → 全 403/400
//!   MarvisAgent 的 data.db → aksks 表     表在，0 行（AK/SK 只在内存）
//!   MarvisHost 10123 本机服务             路由已全量提取，没有任何 wallet 路由
//!   cloud_asset.accesskey                 在 _internal/CONFIG 里是整文件 Fernet 密文
//! ```
//! 唯一剩下的直连路是「提权扫 MarvisAgent 内存取应用级密钥」—— 那是把腾讯的
//! 应用凭据带进网关，性价比极低。**读日志是这条之外的等价替代**。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic（`unwrap_or` 不算）。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::credentials;

/// 用环境变量指定马维斯的数据根目录（`%APPDATA%\Tencent\Marvis` 的替身）。
///
/// 测试**必须**用它 —— 单测不能依赖本机真的装了客户端（见 MEMORY 里那条
/// 「测试绝不依赖本机环境」的教训）。
pub const HOME_ENV: &str = "WORKBUDDY_MARVIS_HOME";

/// 单个日志文件最多读尾部这么多字节。
///
/// `Marvis.log` 实测增长到 8MB 级且一直追加，整文件读进来纯属浪费 —— 钱包读数
/// 总在最后一次刷新里，读尾部就够。4MB 足以覆盖「今天开机后第一次刷新」的位置。
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// 读数最大年龄（同一天内）：超过它就不当作「现在」，但仍返回数值 + 一句说明。
///
/// 客户端只要开着、UI 停在会话页，就会周期性刷新钱包（实测一天十来次，用户每次
/// 交互前后都会刷）。一小时是宽松但有效的界。
const MAX_AGE_MS: i64 = 60 * 60 * 1000;

/// 钱包行的定位串：只认 `token.wallet` 这个**动作名**本身。
///
/// ⚠️ 不要写成 `"action":"token.wallet"` —— 日志里整段 JSON 被**转义了一层**，
/// 实际字节是 `\"action\":\"token.wallet\"`（每个引号前面都有反斜杠），带引号的
/// 定位串永远匹配不上，症状是「明明有读数却报读不到」（2026-10-21 踩过）。
/// 动作名本身不含引号，是转义前后都原样存在的唯一片段。
const WALLET_MARK: &str = "token.wallet";

/// 一次钱包读数的原始字段。
#[derive(Clone, Debug, Default)]
struct Wallet {
    /// 当日剩余（`avail_tokens`）
    avail: Option<f64>,
    /// 当日总量（`total_tokens`）
    total: Option<f64>,
    /// 冻结量（`frozen_tokens`）
    frozen: Option<f64>,
    /// 累计消耗（`total_cost`）
    cost: Option<f64>,
    /// 这条读数属于谁（`data.openid`；空 = 日志没给，不能归属任何人）
    openid: String,
    /// 日志行的时间戳（毫秒；解析不出为 0）
    at: i64,
}

/// 数值字段：上游给的是**字符串**（`"avail_tokens":"9981078"`），也兼容数字。
fn num(value: Option<&Value>) -> Option<f64> {
    let parsed = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    parsed.filter(|value| value.is_finite())
}

/// 从一行日志里取出钱包 JSON。
///
/// ── 谁也没想到的坑：日志会把 JSON **截断**（2026-10-21 实测）──────
/// `Marvis.log` 对长行有长度上限，实测钱包回包被切成这样：
/// ```text
/// …\","total_tokens\":\"10000000...(truncated, total: 359)
/// ```
/// 也就是说 `serde_json` **永远解析失败** —— 症状是「行命中了、读数却为 0 条」
/// （诊断简报里 `命中 token.wallet 4 行，其中属于本账号 0 行`）。
/// 好在被截掉的是**末尾**，`avail_tokens` / `openid` / `total_tokens` 都在截断点
/// 之前，所以走两条路：
///   1. 完整 JSON（日志行短、没被截时）：正常 `serde_json::from_str`；
///   2. 被截断时：**逐字段**用 `"key":"value"` / `"key":number` 的形态抠值。
/// 第 2 条是主路（实测日志行 375 字符恒被截），第 1 条留着兜「未来上限变宽」。
///
/// 两个顺序都不能反：
///   · 先按**未转义**的 `{` 回退找起点 —— 此时日志里唯一的裸 `{` 是 `{\"action\"`；
///   · 解一层 `\"` → `"` 之后再 parse / 抠值。
fn wallet_of_line(line: &str) -> Option<Wallet> {
    let mark = line.find(WALLET_MARK)?;
    // 回到这条 JSON 的开头（日志里唯一的裸 `{`，见上面的顺序说明）
    let start = line[..mark].rfind('{')?;
    // 尾巴整段交给下面两种解析（截断时没有配对的 `}`，所以要保留到行尾）
    let raw = &line[start..];
    // 转义层：先试解转义（实测形态），再试原样（万一哪天客户端不转义了）
    let unescaped = raw.replace("\\\"", "\"");
    let text = if unescaped.contains("\"action\"") { unescaped } else { raw.to_string() };

    // ① 完整 JSON：只认「真的解析出 data 对象」的结果
    if let Some(wallet) = parse_whole(&text) {
        return Some(wallet);
    }
    // ② 被截断：逐字段抠
    parse_truncated(&text)
}

/// 完整 JSON 解析（`data` 必须是对象、且里面**真有钱包数值字段**才算数）。
///
/// 后半句不是洁癖：日志被截断时 `serde_json` 偶尔能「成功」解析出前半段
/// （截断点正好落在 `}`/`,` 边界上），此时 `data` 是对象但没有 `avail_tokens`，
/// 返回一个全 `None` 的 [`Wallet`] 会把 [`parse_truncated`] 这条主路整个挡掉 ——
/// 症状是「明明抠得出 `10000000`，`total` 却是 `None`」（2026-10-21 实测踩过）。
fn parse_whole(text: &str) -> Option<Wallet> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    let data = value.get("data")?;
    if !data.is_object() {
        return None;
    }
    // 一个钱包数值字段都没有 ⇒ 这不是「真·完整回包」，交给逐字段抠
    if num(data.get("avail_tokens")).is_none() && num(data.get("total_tokens")).is_none() {
        return None;
    }
    Some(Wallet {
        avail: num(data.get("avail_tokens")),
        total: num(data.get("total_tokens")),
        frozen: num(data.get("frozen_tokens")),
        cost: num(data.get("total_cost")),
        openid: data
            .get("openid")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        at: 0,
    })
}

/// 从被截断的 JSON 片段里逐字段抠值（日志行长上限导致，见 [`wallet_of_line`]）。
///
/// 只认 `"key":` 后面紧跟的两种形态：带引号的字符串（`"9981078"` / `"oAN1…"`）
/// 与裸数字（`0`）。抠不到就是 `None` —— 与「上游真没给这个字段」同一处置，
/// 绝不默认成 0 之外的东西。
fn parse_truncated(text: &str) -> Option<Wallet> {
    /// 取 `"key":` 之后的原始值文本（不含引号）；找不到返回 None
    ///
    /// 值的形态有**三种**，必须按顺序判：
    ///   1. 裸数字：`"total_cost":0` → 取数字前缀；
    ///   2. 已闭合字符串：`"avail_tokens":"9973660"` → 引号内全是数字，也走数字前缀；
    ///   3. **截断字符串**：`"total_tokens":"10000000...(truncated, total: 359)"`
    ///      → 数字前缀能取到 `10000000` 就收，**不要**再要求后面跟着收尾引号。
    ///
    /// ⚠️ 别写成「数字前缀后面必须紧跟 `"`」—— 截断值后面跟的是 `...`，
    /// 这一条会把明明看得见的 `10000000` 判成没读到（2026-10-21 实测踩过）。
    /// 反过来，字符串值（`openid`）的前缀不含数字，自然落到第 4 条分支。
    fn field(text: &str, key: &str) -> Option<String> {
        let needle = format!("\"{key}\":");
        let start = text.find(&needle)? + needle.len();
        let rest = text[start..].trim_start();

        // 剥掉开引号（截断时这个引号可能永远不闭合）
        let value = rest.strip_prefix('"').unwrap_or(rest);
        // 数字前缀：可选的负号 + 数字 + 可选小数
        let end = value
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '-' || ch == '.'))
            .unwrap_or(value.len());
        let number = trim_number(&value[..end]);
        if !number.is_empty() {
            return Some(number);
        }
        // 非数字值（`openid` 这类）：取到收尾引号；没有收尾引号就不给
        let end = value.find('"')?;
        Some(value[..end].trim().to_string())
    }

    /// 把「数字前缀」修成能被 `f64::from_str` 吃下的形状。
    ///
    /// 截断点实测长这样：`"total_tokens":"10000000...(truncated` —— 按
    /// 「数字/-/.」取前缀会得到 `10000000...`（**尾部那两个点是省略号，不是小数点**），
    /// 直接 `parse::<f64>()` 失败，症状是「明明抠到了 10000000，字段却是 None」。
    /// 所以：先去尾部的 `.`，再去掉只有一个点时的悬挂点。
    fn trim_number(raw: &str) -> String {
        let trimmed = raw.trim_end_matches('.');
        // 只剩 `-` / 空 / 多个点时视为没有数字
        if !trimmed.chars().any(|ch| ch.is_ascii_digit()) {
            return String::new();
        }
        trimmed.to_string()
    }

    fn number(text: &str, key: &str) -> Option<f64> {
        field(text, key)?
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    }

    // 一个数字字段都抠不到 ⇒ 这不是钱包行，交给调用方跳过
    if field(text, "avail_tokens").is_none() && field(text, "total_tokens").is_none() {
        return None;
    }
    Some(Wallet {
        avail: number(text, "avail_tokens"),
        total: number(text, "total_tokens"),
        frozen: number(text, "frozen_tokens"),
        cost: number(text, "total_cost"),
        openid: field(text, "openid").unwrap_or_default(),
        at: 0,
    })
}

/// 解析行首的 `MM-DD HH:MM:SS.mmm` → 毫秒。
///
/// ── 时区（2026-10-21 真机自检抓到）──────────────────────────
/// 日志时间戳是**本机时区**的，而毫秒是 UTC 口径 —— 中间必须过一次
/// [`local_time_ms`]（`TzSpecificLocalTimeToSystemTime`）。少了它，东八区的
/// 读数会比「现在」晚 8 小时，被 [`freshness`] 判成「来自未来」而**整条作废**。
///
/// ── 定长 18，且切分只出 6 段（这里踩过两次，别再凭感觉算）──────
/// `10-21 07:21:13.530` 逐段数：`10`(2) `-` `21`(2) ` ` `07`(2) `:` `21`(2)
/// `:` `13`(2) `.` `530`(3) = **18** 字节，紧跟其后的第 19 个字节是空格。
/// 按 `['-', ' ', ':', '.']` 切出来是 6 段（月/日/时/分/秒/毫秒），**不是 7 段**。
/// 不能按第一个空格切 —— 日期与时间之间还有一个空格，切出来只有 `10-21`；
/// 也不能不管长度整行 split（日志尾巴里的 `[` `.` `:` 会被一起切进来）。
///
/// 日志不含年份。跨年那天（12-31 / 01-01）要特判：算出来的时刻若比「现在」
/// 晚一天以上，说明那条是去年的 —— 减一年。少了这步，元旦当天的读数年龄会
/// 算成 +365 天然后被当成陈旧丢掉。
fn line_time_ms(line: &str, now_ms: i64) -> i64 {
    /// `MM-DD HH:MM:SS.mmm` 的字节长度（见上面的逐段算式）
    const STAMP_LEN: usize = 18;

    let bytes = line.as_bytes();
    // 时间戳之后必须是空格（日志格式保证）；长度不够就是别的形态
    if bytes.len() <= STAMP_LEN || bytes[STAMP_LEN] != b' ' {
        return 0;
    }
    if !line.is_char_boundary(STAMP_LEN) {
        return 0;
    }
    let head = &line[..STAMP_LEN];
    let parts: Vec<&str> = head.split(['-', ' ', ':', '.']).collect();
    if parts.len() != 6 {
        return 0;
    }
    let mut nums = [0i64; 6];
    for (index, slot) in nums.iter_mut().enumerate() {
        let Ok(value) = parts[index].trim().parse::<i64>() else {
            return 0;
        };
        *slot = value;
    }
    let [month, day, hour, minute, second, millis] = nums;
    let year = local_year();
    let mut at = local_time_ms(year, month, day, hour, minute, second, millis);
    if at > now_ms + 24 * 3600 * 1000 {
        at = local_time_ms(year - 1, month, day, hour, minute, second, millis);
    }
    at
}

/// 本机时区的当年。
#[cfg(windows)]
fn local_year() -> i64 {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let time = unsafe { GetLocalTime() };
    i64::from(time.wYear)
}

/// 非 Windows：本家客户端只有 Windows / macOS 版，日志路径本来也不存在。
///
/// 给 1970 会让读数恒被判成「陈旧 / 跨天」而作废 —— 这正是期望行为：
/// 不拿编出来的时间冒充现在。
#[cfg(not(windows))]
fn local_year() -> i64 {
    1970
}

/// 公历 → 毫秒（UTC 口径，Howard Hinnant 的 `days_from_civil`）。
///
/// 不引入 chrono：只用它算「这条读数多久以前」，与时区无关的差值即可，
/// 闰年也不会算错（有单测锁）。
fn civil_to_ms(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, millis: i64) -> i64 {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return 0;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis
}

/// 数据根目录：`%APPDATA%\Tencent\Marvis`（客户端写日志的地方）。
///
/// 顺序：`WORKBUDDY_MARVIS_HOME` → `APPDATA` → `USERPROFILE\AppData\Roaming`。
fn home_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(HOME_ENV) {
        let path = PathBuf::from(dir.trim());
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let base = appdata.trim();
        if !base.is_empty() {
            return Some(PathBuf::from(base).join("Tencent").join("Marvis"));
        }
    }
    std::env::var("USERPROFILE")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(|home| {
            PathBuf::from(home.trim())
                .join("AppData")
                .join("Roaming")
                .join("Tencent")
                .join("Marvis")
        })
}

/// 收集日志文件：`<home>/logs/*.log` 与 `…\Marvis\Application\<版本>\logs\*.log`。
///
/// ── 日志到底在哪（2026-10-21 实测，别按直觉放）───────────────
/// 钱包读数在 **安装目录**里，不在 `%APPDATA%`：
/// ```text
///   D:\Program Files\Tencent\Marvis\Application\1.60.3000.228\logs\Marvis.log
///   %APPDATA%\Tencent\Marvis\        ← 这里**没有** logs/（只有 User/ db/ cef/ …）
/// ```
/// 所以两个位置都扫：`<home>/logs` 是留给「客户端哪天改到用户目录」的兜底，
/// 安装目录才是现在真正命中的那一条。
///
/// 版本号目录随客户端升级变化，因此**扫 `Application/*` 而不是写死版本**；
/// 定位口径与 `fingerprint.rs` 的 `locate_install_dir` 一致（env →
/// 默认路径 → 扫同级），只是那边要 `beacon_wrapper.dll` 存在，这边只要能读到
/// 日志就行 —— 两者判断依据不同，不要合并。
fn log_files(home: &Path) -> Vec<PathBuf> {
    fn collect_logs(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_log = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("log"))
                .unwrap_or(false);
            if is_log && path.is_file() {
                out.push(path);
            }
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    collect_logs(&home.join("logs"), &mut out);

    // HOME_ENV 生效 = 调用方给的是**假目录**（单测），安装目录一律不扫。
    // 否则装了真客户端的开发机上，单测会读到真日志（实测被覆盖成 `9973660`，
    // 而用例里写的是 `9981078`）—— 测试就成了环境的函数，CI 与本机两种结果。
    if std::env::var(HOME_ENV).map(|dir| !dir.trim().is_empty()).unwrap_or(false) {
        return out;
    }

    // 安装目录：`…\Tencent\Marvis`（它下面才是 `Application\<版本>`，再下面
    // 才是 `logs`）。`WORKBUDDY_MARVIS_DIR` 给的是**版本目录**（与
    // fingerprint.rs 同一约定），所以要往上退两级才回到 `Marvis`。
    const DEFAULT_ROOT: &str = r"D:\Program Files\Tencent\Marvis";
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("WORKBUDDY_MARVIS_DIR") {
        let version_dir = PathBuf::from(dir.trim());
        if let Some(marvis_root) = version_dir.parent().and_then(Path::parent) {
            roots.push(marvis_root.to_path_buf());
        }
    }
    roots.push(PathBuf::from(DEFAULT_ROOT));

    for root in roots {
        let app = root.join("Application");
        // Application 本身也可能挂着一层 logs（见过的安装形态），先收
        collect_logs(&app.join("logs"), &mut out);
        // 再按版本子目录收（现在的实际形态）
        let Ok(entries) = std::fs::read_dir(&app) else {
            continue;
        };
        for entry in entries.flatten() {
            collect_logs(&entry.path().join("logs"), &mut out);
        }
    }
    out
}

/// 读文件尾部（最多 [`TAIL_BYTES`]），按行返回。
fn tail_lines(path: &Path) -> VecDeque<String> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return VecDeque::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if len == 0 {
        return VecDeque::new();
    }
    let start = len.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return VecDeque::new();
    }
    let mut buffer = Vec::with_capacity((len - start).min(8 * 1024 * 1024) as usize);
    if file.read_to_end(&mut buffer).is_err() {
        return VecDeque::new();
    }
    let text = String::from_utf8_lossy(&buffer);
    // 从中间切开时首行可能是半行（它前面的 `{` 不在缓冲区里）—— 丢掉
    let body = if start == 0 {
        text.as_ref()
    } else {
        text.find('\n').map(|index| &text[index + 1..]).unwrap_or("")
    };
    body.lines().map(str::to_string).collect()
}

/// 在全部候选日志里找**属于这个账号**的最新一条钱包读数。
///
/// 归属判据只有一条：`data.openid` 与账号的 openid 相等。日志里没带 openid 的
/// 行一律跳过 —— 宁可报「读不到」，也不把别人的余额画成你的。
fn latest_wallet(openid: &str, now_ms: i64) -> Option<Wallet> {
    if openid.trim().is_empty() {
        return None;
    }
    let home = home_dir()?;
    let mut best: Option<Wallet> = None;
    for path in log_files(&home) {
        for line in tail_lines(&path) {
            // 预筛只看标记；**归属只认解析出来的 `data.openid`**（见模块头纪律 1）。
            // 别在这里提前拿 `line.contains(openid)` 过滤：日志行会被客户端截断，
            // 半行里 openid 可能露不全，那样会把唯一一条真读数筛掉。
            if !line.contains(WALLET_MARK) {
                continue;
            }
            let Some(mut wallet) = wallet_of_line(&line) else {
                continue;
            };
            if wallet.openid != openid {
                continue;
            }
            wallet.at = line_time_ms(&line, now_ms);
            if best.as_ref().map(|prev| wallet.at >= prev.at).unwrap_or(true) {
                best = Some(wallet);
            }
        }
    }
    best
}

/// 新鲜度判定：`None` = 够新，可以直接当现在；`Some(原因)` = 不能用。
fn freshness(wallet: &Wallet, now_ms: i64) -> Option<String> {
    if wallet.at <= 0 {
        return Some("日志行的时间戳解析不出来，无法判断这条读数有多旧".to_string());
    }
    if wallet.at > now_ms {
        return Some("日志时间戳晚于本机时间（系统时钟可能被调整过）".to_string());
    }
    // 跨天先判：日额度每天重置，昨天的数拿今天解释就是错的
    if !same_local_day(wallet.at, now_ms) {
        return Some(
            "这条读数是前一天的（每日额度已按自然日重置）：请在马维斯客户端里打开一次会话页，\
             让它刷新钱包后再查询"
                .to_string(),
        );
    }
    let age = now_ms - wallet.at;
    if age > MAX_AGE_MS {
        return Some(format!(
            "这条读数是 {} 分钟前的（客户端近期没刷新钱包；打开一次会话页即可刷新）",
            age / 60_000
        ));
    }
    None
}

/// 两个毫秒时刻是否同一自然日（本机时区）。
#[cfg(windows)]
fn same_local_day(a_ms: i64, b_ms: i64) -> bool {
    let (a, b) = (local_day_key(a_ms), local_day_key(b_ms));
    a != (0, 0, 0) && a == b
}

#[cfg(not(windows))]
fn same_local_day(_a_ms: i64, _b_ms: i64) -> bool {
    false
}

/// **本地**日历时刻 → 真实 UTC 毫秒。
///
/// ── 为什么非要这一步（2026-10-21 真机自检抓到）───────────────
/// [`civil_to_ms`] 算出来的是「把这份年月日时分秒当成 **UTC**」的时刻。日志里的
/// 时间戳却是**本机时区**的（`Marvis.log` 就是按本地时间写的）。少了这一层换算，
/// 东八区的时间戳会比真实时刻早 8 小时 —— 症状有两个，一个比一个难查：
///   · 读数「来自未来」：`freshness` 直接判「日志时间戳晚于本机时间」，
///     余额一列永远是空的（真机自检实测就是这句）；
///   · 跨零点后误判跨天：早上 8 点前的读数会被算成「昨天」，整次查询失败。
///
/// 用 `TzSpecificLocalTimeToSystemTime` 而不是拿 `GetTimeZoneInformation` 手算：
/// 后者要自己处理 `Bias` 的符号与夏令时，前者由系统给答案。
#[cfg(windows)]
fn local_time_ms(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, millis: i64) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};

    let clamp = |value: i64, max: i64| -> u16 {
        value.clamp(0, max) as u16
    };
    let local = SYSTEMTIME {
        wYear: clamp(year, 9999),
        wMonth: clamp(month, 12),
        wDay: clamp(day, 31),
        wHour: clamp(hour, 23),
        wMinute: clamp(minute, 59),
        wSecond: clamp(second, 59),
        wMilliseconds: clamp(millis, 999),
        ..Default::default()
    };
    let mut utc = SYSTEMTIME::default();
    if unsafe { TzSpecificLocalTimeToSystemTime(None, &local, &mut utc) }.is_err() {
        // 换算不了（系统时区数据异常）就退回「当 UTC 算」—— 宁可差几小时，
        // 也别让整条读数作废；`freshness` 的陈旧档会把它如实标出来
        return civil_to_ms(year, month, day, hour, minute, second, millis);
    }
    let mut file_time = FILETIME::default();
    if unsafe { SystemTimeToFileTime(&utc, &mut file_time) }.is_err() {
        return civil_to_ms(year, month, day, hour, minute, second, millis);
    }
    let ticks = (u64::from(file_time.dwHighDateTime) << 32) | u64::from(file_time.dwLowDateTime);
    ticks as i64 / 10_000 - 11_644_473_600_000
}

/// 非 Windows：没有客户端日志可读，退化成「当 UTC 算」即可（见 [`local_year`]）。
#[cfg(not(windows))]
fn local_time_ms(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, millis: i64) -> i64 {
    civil_to_ms(year, month, day, hour, minute, second, millis)
}

/// 毫秒时刻 → 本机时区的 `(年, 月, 日)`。
///
/// ── 两个换算坑（都在 2026-10-21 真机自检里现形）──────────────
/// ① **FILETIME 的单位是 100ns，不是毫秒**：`(毫秒 + 纪元偏移)` 必须再 ×10_000。
///    少了这一步，`16xx 年`级的时刻会被当成 `2000 年代`，`FileTimeToSystemTime`
///    照样返回成功，解出来却是 `1601-01-16` —— 于是所有读数都被判「跨天」而作废。
/// ② 不能用 `FileTimeToLocalFileTime` 转本地日历：它返回的是「UTC 文件时间 + 时区差」
///    的**文件时间**，不是合法 UTC 文件时间，再喂 `FileTimeToSystemTime` 解出的年月日
///    是错的。正确链路是三段：
/// ```text
/// Unix 毫秒 → ×10_000 → FILETIME(UTC) → FileTimeToSystemTime → SystemTimeToTzSpecificLocalTime
/// ```
///
/// ⚠️ 在 windows crate 0.61 里这几个函数**不住在同一个模块**：
/// `GetLocalTime` 在 `Win32::System::SystemInformation`，
/// `SystemTimeToTzSpecificLocalTime` / `FileTimeToSystemTime` 在 `Win32::System::Time`。
#[cfg(windows)]
fn local_day_key(at_ms: i64) -> (u16, u16, u16) {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

    let Some(utc) = unix_ms_to_file_time(at_ms) else {
        return (0, 0, 0);
    };
    let mut utc_time = SYSTEMTIME::default();
    if unsafe { FileTimeToSystemTime(&utc, &mut utc_time) }.is_err() {
        return (0, 0, 0);
    }
    let mut local = SYSTEMTIME::default();
    if unsafe { SystemTimeToTzSpecificLocalTime(None, &utc_time, &mut local) }.is_err() {
        return (0, 0, 0);
    }
    (local.wYear, local.wMonth, local.wDay)
}

/// Unix 毫秒 → FILETIME（1601 纪元，**单位 100ns**）。
///
/// 单独抽出来是因为这个 ×10_000 被漏过一次（见 [`local_day_key`] 的坑 ①）：
/// 两处换算写两遍就会两处都漏，写一遍至少只有一处可能错。
#[cfg(windows)]
fn unix_ms_to_file_time(at_ms: i64) -> Option<windows::Win32::Foundation::FILETIME> {
    use windows::Win32::Foundation::FILETIME;

    // 1601-01-01 到 1970-01-01 的毫秒数；负数（1601 之前）无意义，直接拒
    const EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;
    const TICKS_PER_MS: i64 = 10_000;

    let ticks = at_ms.checked_add(EPOCH_OFFSET_MS)?.checked_mul(TICKS_PER_MS)?;
    if ticks < 0 {
        return None;
    }
    let ticks = ticks as u64;
    Some(FILETIME {
        dwLowDateTime: (ticks % 4_294_967_296) as u32,
        dwHighDateTime: (ticks / 4_294_967_296) as u32,
    })
}

/// 把钱包归一成 `query_usage` 的契约形状（**照 CodeArts 的 token 口径**）。
///
/// ── 前端到底读哪几个键（`usageDetailOf`，accounts-panels.tsx:1290）────
/// 钱包行的分支判据是「有 `creditRemaining` → 积分类」否则
/// 「有 `usedPercent` 或 `allowanceTokens` → **token 类**」。两者都没有就落到
/// 「既无积分也无 token 量」的兜底，只显示一句灰字 —— 这正是第一版把字段写成
/// `used` / `remainingPercent` 时看到的症状。所以 token 类必须写成：
/// ```text
///   usedTokens       已用（= total - avail）
///   allowanceTokens  当日总量（total_tokens）
///   usedPercent      已用百分比
/// ```
/// 余额列主读数（`usageSummary`）则走 `availableView`（优先）+ `balanceView`（悬停明细）。
///
/// ⚠️ **不写 `credits[]`**：那是「逐份积分包 + 到期时刻」的承诺，账号页
/// 「最近到期 / 最近到期积分」两列与「积分明细」入口都读它。日额度每天重置、
/// 没有作废时刻，写进去只会让那两列显示一个假的到期日 —— CodeArts 同理
/// （它登记 `usage` + `usageDetail`，也不产 `credits`）。
fn normalize(wallet: &Wallet, note: Option<String>) -> Value {
    let avail = wallet.avail.unwrap_or(0.0);
    let frozen = wallet.frozen.unwrap_or(0.0);
    let total = wallet.total;
    // 已用量与百分比只在**有真总量**时给：没有 total 就算不出「已用多少」，
    // 编一个 0 会被明细读成「一点没用」，与事实可能相反
    let used = total.map(|total| (total - avail).max(0.0));
    let used_percent = match total {
        Some(total) if total > 1.0 => Some((used.unwrap_or(0.0) / total * 100.0).clamp(0.0, 100.0)),
        _ => None,
    };

    let mut row = Map::new();
    row.insert("type".to_string(), json!("daily_tokens"));
    row.insert("displayName".to_string(), json!("每日免费 Token"));
    row.insert("balance".to_string(), json!(avail));
    row.insert("unit".to_string(), json!("Token"));
    // `daily: true` 是给前端 `creditExpiryOf` / `creditRowsOf` 的**语义标记**：
    // 每日回满，不是到期作废
    row.insert("daily".to_string(), json!(true));
    match total {
        Some(total) => {
            row.insert("total".to_string(), json!(total));
            row.insert("used".to_string(), json!(used.unwrap_or(0.0)));
            row.insert("remainingPercent".to_string(), json!((avail / total * 100.0).clamp(0.0, 100.0)));
            // ↓ 这三个才是 usageDetailOf 认 token 类的判据（见函数头）
            row.insert("usedTokens".to_string(), json!(used.unwrap_or(0.0)));
            row.insert("allowanceTokens".to_string(), json!(total));
            row.insert(
                "usedPercent".to_string(),
                used_percent.map(Value::from).unwrap_or(Value::Null),
            );
            // 展示串照 CodeArts 的 token 款：「已用 x / y token（z%）」——
            // 余额列只有 148px，这一串主要出现在悬停明细里
            row.insert(
                "balanceView".to_string(),
                json!(format!(
                    "已用 {} / {} token（{}%）",
                    compact(used.unwrap_or(0.0)),
                    compact(total),
                    compact(used_percent.unwrap_or(0.0).round()),
                )),
            );
        }
        None => {
            // 没给总量就不写 total / used / 比例：画一条满格的条与一条空条
            // 读起来是相反的意思，而它们都可能是错的
            row.insert("balanceView".to_string(), json!(format!("剩 {} Token", compact(avail))));
        }
    }

    // 余额列的主读数：它回答的是「现在还剩多少」，与明细里的「已用 / 总量」
    // 是两个问题 —— 照 CodeArts 的做法，主读数给剩余、明细给已用（`availableView`
    // 优于 `balanceView`，见 usageSummary 第 811 行）
    let view = match total {
        Some(total) => format!("剩 {} / {} Token", compact(avail), compact(total)),
        None => format!("剩 {} Token", compact(avail)),
    };

    let mut document = json!({
        "available": avail,
        "availableView": view,
        "unit": "Token",
        "wallets": [Value::Object(row)],
        "subscription": {
            "planName": "每日免费额度（1000 万 Token / 天）",
            "status": "daily",
            // 日额度没有到期时刻：写 0 而不是编一个
            "expireAt": 0,
            "resetDate": "每日 0 点重置",
            "remainQuota": avail,
            "totalQuota": total,
        },
        "marvis": {
            "source": "desktop-log",
            "collectedAt": wallet.at,
            "frozen": frozen,
            "totalCost": wallet.cost,
            "expiresAt": 0,
            "dailyReset": true,
            "expiresNote": "额度按自然日重置，没有到期时刻 —— 「最近到期 / 最近到期积分」两列对它恒为空",
            "stale": note,
        },
        "raw": {
            "avail_tokens": wallet.avail,
            "total_tokens": wallet.total,
            "frozen_tokens": wallet.frozen,
            "total_cost": wallet.cost,
            "collected_at": wallet.at,
        },
    });
    // 陈旧提示挂到顶层 `warning`：`usageSummary` 只读 available/wallets，明细弹层
    // 读 `missing[]` —— 顶层键是两边都能看见的那一处（悬停提示里会出现）
    if let Some(text) = note {
        if let Some(object) = document.as_object_mut() {
            object.insert("warning".to_string(), json!(text));
        }
    }
    document
}

/// 数字 → 紧凑串（`998.1万` / `1000万` / `9,988`）：余额列只有 148px 宽。
///
/// 一万以下千分位、万以上「万」、亿以上「亿」，一位小数且整数不带点。
fn compact(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let abs = value.abs();
    if abs >= 100_000_000.0 {
        return scaled(value / 100_000_000.0, "亿");
    }
    if abs >= 10_000.0 {
        return scaled(value / 10_000.0, "万");
    }
    let text = format!("{:.0}", value.round());
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest.to_string()),
        None => ("", text),
    };
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{sign}{out}")
}

/// `123.4万`：一位小数，整数不带小数点。
fn scaled(value: f64, unit: &str) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < f64::EPSILON {
        format!("{:.0}{unit}", rounded)
    } else {
        format!("{rounded:.1}{unit}")
    }
}

/// 读失败时附在文案末尾的**定位简报**（找日志这一步断了还是解析断了）。
///
/// ── 为什么要把它放进用户可见的文案 ──────────────────────────
/// 「读不到」有三种完全不同的原因，用户看到的却是同一句话：
///   ① 日志文件一个都没找到（路径 / 权限）—— 用户在错误的目录里翻半天；
///   ② 找到了文件但没有 `token.wallet` 行 —— 客户端没刷过钱包；
///   ③ 有这行、但 `data.openid` 对不上 —— 登的是另一个号。
/// 把「扫了几个文件 / 命中几行 / 有几个 openid」直接写出来，用户（和排障的人）
/// 一眼就知道该往哪走，不必再猜。带的是**计数**，不漏任何 openid 原文。
fn diagnostic(openid: &str) -> String {
    let Some(home) = home_dir() else {
        return "日志目录：<未找到 APPDATA>".to_string();
    };
    let files = log_files(&home);
    let mut hits = 0usize;
    let mut mine = 0usize;
    // 日志里出现过的 openid（脱敏）——「对不上」时这一项是唯一能自救的信息：
    // 用户一眼能看出「日志里是另一个号」，而不用去猜是不是路径错了
    let mut seen: Vec<String> = Vec::new();
    for path in &files {
        for line in tail_lines(path) {
            if !line.contains(WALLET_MARK) {
                continue;
            }
            // 两种行都进日志：「发送 agent.action」（无 data）与「响应」（有 data）
            hits += 1;
            if let Some(wallet) = wallet_of_line(&line) {
                if wallet.openid == openid {
                    mine += 1;
                } else if !wallet.openid.is_empty() {
                    let masked = credentials::masked(&wallet.openid);
                    if !seen.contains(&masked) {
                        seen.push(masked);
                    }
                }
            }
        }
    }
    // 目录去重：同一个目录会因为「Application 直下 logs」与「版本子目录 logs」
    // 两条路径被收两遍，拼出来一长串完全相同的路径，反而看不清
    let mut dirs: Vec<String> = files
        .iter()
        .filter_map(|path| path.parent().map(|dir| dir.display().to_string()))
        .collect();
    dirs.dedup();
    let dirs = if dirs.is_empty() {
        format!("未找到日志文件（已扫 {}）", home.display())
    } else {
        dirs.join("；")
    };
    let mut text = format!(
        "日志目录：{dirs}（共 {} 个 .log 文件，命中 token.wallet {hits} 行，其中属于本账号 {mine} 行）",
        files.len()
    );
    if mine == 0 && !seen.is_empty() {
        text.push_str(&format!(
            "；日志里出现的 openid 是 {}，与本账号（{}）不同 —— 客户端当前登录的是另一个号",
            seen.join(" / "),
            credentials::masked(openid),
        ));
    }
    text
}

/// 读一次余额（`ProviderAdapter::query_usage` 的实现）。
///
/// 失败给**可执行的下一步**，不笼统报「查询失败」：
///   · 账号缺 openid → 无法归属读数，要求重新导入登录态
///   · 日志里没有本账号的读数 → 「打开客户端会话页让它刷新一次」
///   · 只有昨天的读数 → 跨天作废，同样要求刷新
pub async fn query_usage(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    let record = store
        .marvis_account_record(account_id)
        .ok_or_else(|| GatewayError::with_status(404, "Marvis 账号不存在或不可用"))?;
    let credential = credentials::from_record(Some(&record)).map_err(|_| {
        GatewayError::with_status(
            400i32,
            "Marvis 账号缺少 openid（无法确定钱包读数属于哪个账号）：请重新「导入本机登录态」",
        )
    })?;

    let now_ms = crate::server::logging::now_ms();
    let home = home_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| "<未找到 APPDATA>".to_string());
    let Some(wallet) = latest_wallet(&credential.openid, now_ms) else {
        return Err(GatewayError::with_status(
            400i32,
            format!(
                "没有读到马维斯的钱包读数：客户端日志里找不到这个账号（{}）的 token.wallet 记录。\
                 请确认马维斯客户端在运行、已登录该账号，并在客户端里打开一次会话页（会随即刷新钱包）\
                 后再查询。{}",
                credentials::masked(&credential.openid),
                diagnostic(&credential.openid),
            ),
        ));
    };

    match freshness(&wallet, now_ms) {
        // 陈旧 / 无时间戳：读数是真的，照给数值，另挂 warning 说明有多旧
        Some(note) if wallet.at > 0 && same_local_day(wallet.at, now_ms) => {
            Ok(normalize(&wallet, Some(note)))
        }
        // 跨天 / 时钟异常：这个数已经不能代表「今天还剩多少」，整次判失败
        Some(note) => Err(GatewayError::with_status(400i32, format!("{note}。日志目录：{home}"))),
        None => Ok(normalize(&wallet, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 改环境变量的单测必须串行 —— `std::env` 是**进程级**的，两个测试并发跑会
    /// 互相看到对方设的 HOME，症状是偶发失败（本地过了、CI 挂）。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 取锁；被 poison（别的用例 panic 过）也照用 —— 测的是环境变量隔离，
    /// 不是锁的健壮性，因为一次 panic 让整组永久失败没有意义。
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 客户端日志原样一行（含时间戳前缀与转义层）—— 测试的唯一真实样本来源。
    const LINE: &str = "10-21 07:21:13.530 [22708.20704](info)[js] [] (:0) [Msg]: \"[i]ai_starter|Chat|[AGUIConnection] agent.action 响应: {\"action\":\"token.wallet\",\"data\":{\"avail_tokens\":\"9981078\",\"frozen_tokens\":\"0\",\"openid\":\"oAN1i2RKFSGN7j81LNLueIhnSTes\",\"remaining_tokens\":0,\"saved_tokens\":0,\"status\":1,\"total_cost\":0,\"total_tokens\":\"10000000\"}}\"";

    #[test]
    fn parses_the_real_log_line() {
        let wallet = wallet_of_line(LINE).expect("真实日志行应能解析");
        assert_eq!(wallet.avail, Some(9_981_078.0));
        assert_eq!(wallet.total, Some(10_000_000.0));
        assert_eq!(wallet.frozen, Some(0.0));
        assert_eq!(wallet.openid, "oAN1i2RKFSGN7j81LNLueIhnSTes");
    }

    /// 真实日志会对长行**截断**（`Marvis.log` 实测 375 字符处切掉，尾巴是
    /// `...(truncated, total: 359)"`）—— 完整 JSON 必然解析失败，必须能逐字段抠。
    ///
    /// 这条用例用的是**实测原文的尾部**，不是构造出来的漂亮 JSON。
    /// 截断点上 `total_tokens` 的值是 `10000000...(truncated` —— 引号没闭、
    /// 数字后面跟的是 `...`，抠值只能按「数字前缀」收。
    #[test]
    fn parses_a_truncated_wallet_line() {
        let line = "10-21 09:09:36.955 [22708.20704](info)[js] [] (:0) [Msg]: \"[i]ai_starter|Chat|[AGUIConnection] agent.action 响应: {\\\"action\\\":\\\"token.wallet\\\",\\\"data\\\":{\\\"avail_tokens\\\":\\\"9973660\\\",\\\"frozen_tokens\\\":\\\"0\\\",\\\"openid\\\":\\\"oAN1i2RKFSGN7j81LNLueIhnSTes\\\",\\\"remaining_tokens\\\":0,\\\"saved_tokens\\\":0,\\\"status\\\":1,\\\"total_cost\\\":0,\\\"total_tokens\\\":\\\"10000000...(truncated, total: 359)\"\r";
        let wallet = wallet_of_line(line).expect("截断行也要能读出字段");
        assert_eq!(wallet.avail, Some(9_973_660.0));
        assert_eq!(
            wallet.total,
            Some(10_000_000.0),
            "截断值 `\"10000000...(truncated` 要按数字前缀收，实际抠到 {:?}",
            wallet.total
        );
        assert_eq!(wallet.frozen, Some(0.0));
        assert_eq!(
            wallet.openid, "oAN1i2RKFSGN7j81LNLueIhnSTes",
            "openid 在截断点之前，必须抠得出来"
        );
    }

    /// 截断点若落在一个**能侥幸 parse 成功**的位置（前半段自成合法 JSON、
    /// 但 `data` 里没有数值字段），不能因此把逐字段抠的主路挡掉。
    #[test]
    fn a_half_parsable_fragment_still_falls_back_to_field_picking() {
        // 前半段正好闭合：`{"action":"token.wallet","data":{"openid":"oX"}}`
        let whole = r#"{"action":"token.wallet","data":{"openid":"oX"}}"#;
        assert!(
            parse_whole(whole).is_none(),
            "没有 avail_tokens / total_tokens 就不是完整回包"
        );
        // 逐字段抠也抠不到数值 ⇒ 整体判失败（不能编一个 0 出来）
        assert!(parse_truncated(whole).is_none());
    }
    ///
    /// 真实日志里整段 JSON 被转义一层（`\"action\":\"token.wallet\"`），所以拿
    /// `"action":"token.wallet"` 去 `contains` 永远为假 —— 症状是「日志里明明有
    /// 读数、接口却报读不到」（2026-10-21 实际踩过，找了半天路径问题）。
    #[test]
    fn the_wallet_mark_survives_escaping() {
        // 实测字节形态：每个引号前都有反斜杠
        let escaped = "… 响应: {\\\"action\\\":\\\"token.wallet\\\",\\\"data\\\":{}}";
        assert!(
            escaped.contains(WALLET_MARK),
            "WALLET_MARK 必须能命中**转义后**的日志行；当前值是 {WALLET_MARK:?}"
        );
        // 没转义的形态也得命中（客户端哪天改了写法）
        assert!("{\"action\":\"token.wallet\"}".contains(WALLET_MARK));
    }

    #[test]
    fn ignores_the_request_line_which_has_no_data() {
        // 「发送 agent.action」那行只有 action、没有 data —— 不能当读数
        let line = "10-21 07:21:13.118 [22708.20704](info)[js] [] (:0) [Msg]: \"[i]ai_starter|Chat|[AGUIConnection] 发送 agent.action: {\\\"action\\\":\\\"token.wallet\\\"}\"";
        assert!(wallet_of_line(line).is_none());
    }

    #[test]
    fn normalize_writes_no_credits_and_no_expire() {
        let wallet = Wallet {
            avail: Some(9_981_078.0),
            total: Some(10_000_000.0),
            frozen: Some(0.0),
            cost: Some(0.0),
            openid: "oX".into(),
            at: 1_700_000_000_000,
        };
        let out = normalize(&wallet, None);
        assert_eq!(out["available"], json!(9_981_078.0));
        assert_eq!(out["unit"], json!("Token"));
        assert!(out.get("credits").is_none(), "日额度没有逐份到期，绝不写 credits");
        assert_eq!(out["subscription"]["expireAt"], json!(0), "不编到期时刻");
        assert_eq!(out["wallets"][0]["daily"], json!(true));
        assert_eq!(out["marvis"]["dailyReset"], json!(true));
        // 余额列主读数：剩 x / 共 y
        assert_eq!(out["availableView"], json!("剩 998.1万 / 1000万 Token"));
        // 明细读数照 CodeArts 的 token 款：已用 x / y token（z%）
        assert_eq!(
            out["wallets"][0]["balanceView"],
            json!("已用 1.9万 / 1000万 token（0%）")
        );
    }

    /// 明细要能被 `usageDetailOf` 认成 **token 类**（而不是「既无积分也无 token 量」
    /// 的兜底分支）—— 判据就是这三个键，缺一个都会退化成一句灰字。
    #[test]
    fn wallet_carries_the_token_keys_usage_detail_reads() {
        let wallet = Wallet {
            avail: Some(7_500_000.0),
            total: Some(10_000_000.0),
            ..Default::default()
        };
        let row = &normalize(&wallet, None)["wallets"][0];
        assert_eq!(row["usedTokens"], json!(2_500_000.0));
        assert_eq!(row["allowanceTokens"], json!(10_000_000.0));
        assert_eq!(row["usedPercent"], json!(25.0));
    }

    #[test]
    fn normalize_without_total_does_not_invent_one() {
        let wallet = Wallet { avail: Some(123.0), ..Default::default() };
        let out = normalize(&wallet, None);
        assert_eq!(out["available"], json!(123.0));
        assert!(out["wallets"][0].get("total").is_none(), "没给总量就不写");
        assert!(out["wallets"][0].get("remainingPercent").is_none(), "画不出比例");
        assert_eq!(out["subscription"]["totalQuota"], Value::Null);
    }

    #[test]
    fn compact_uses_wan_and_yi() {
        assert_eq!(compact(9_981_078.0), "998.1万");
        assert_eq!(compact(10_000_000.0), "1000万");
        assert_eq!(compact(9_988.0), "9,988");
        assert_eq!(compact(123_400_000.0), "1.2亿");
        assert_eq!(compact(f64::INFINITY), "0", "非有限值不出去");
    }

    #[test]
    fn civil_math_survives_leap_day() {
        let leap = civil_to_ms(2024, 2, 29, 0, 0, 0, 0);
        let next = civil_to_ms(2024, 3, 1, 0, 0, 0, 0);
        assert_eq!(next - leap, 86_400_000, "闰年 2-29 必须正好一天");
        assert_eq!(civil_to_ms(2026, 13, 1, 0, 0, 0, 0), 0, "非法月份返回 0");
    }

    #[test]
    fn line_time_parses_the_prefix() {
        // 前缀没有年份：只验证月/日/时/分/秒/毫秒被解出且与本行自洽
        let parsed = line_time_ms(LINE, 1_700_000_000_000);
        assert!(
            parsed > 0,
            "前缀应能解析；line 前 30 字节 = {:?}",
            &LINE[..LINE.len().min(30)]
        );
        // 07:21:13.530 的毫秒部分必须保留
        assert_eq!(parsed % 1000, 530);
    }

    /// **回归护栏**：日志时间戳是本机时区，解出来必须是真实 UTC 毫秒。
    ///
    /// 2026-10-21 真机自检抓到：直接把「本地年月日时分秒」当 UTC 算，东八区
    /// 差 8 小时，`freshness` 判「日志时间戳晚于本机时间」⇒ 余额永远读不出来。
    /// 断言方式与时区无关：把解析结果再换回本机日历，必须与行首字面一致。
    #[test]
    fn the_log_stamp_is_local_time_not_utc() {
        let parsed = line_time_ms(LINE, crate::server::logging::now_ms());
        let (year, month, day) = local_day_key(parsed);
        assert_ne!((year, month, day), (0, 0, 0), "应能换回本机日历");
        assert_eq!(
            (month, day),
            (10, 21),
            "行首是 10-21（本机时区）；解析出 {parsed}，换回 ({year}, {month}, {day})，local_year()={}",
            local_year()
        );
        // 时刻：把解析结果按本机时区换回去，时分秒要能对上（毫秒已在上面锁过）
        #[cfg(windows)]
        {
            let (month, day, hour, minute, second) = local_parts(parsed);
            assert_eq!(
                (month, day, hour, minute, second),
                (10, 21, 7, 21, 13),
                "本机时区的月日时分秒应与行首 `10-21 07:21:13` 一致"
            );
        }
    }

    #[test]
    fn stale_reading_keeps_numbers_but_warns() {
        let wallet = Wallet {
            avail: Some(100.0),
            total: Some(200.0),
            at: 1_700_000_000_000,
            ..Default::default()
        };
        // 用日志里的真实一天做基准：读数 2 小时前
        let now = civil_to_ms(2026, 10, 21, 9, 21, 13, 530);
        let wallet = Wallet { at: now - 2 * 3600 * 1000, ..wallet };
        let note = freshness(&wallet, now).expect("两小时前的读数应被判陈旧");
        // 跨天与时区无关的那档：至少有一句说明
        assert!(!note.is_empty());
        let out = normalize(&wallet, Some(note));
        assert_eq!(out["available"], json!(100.0), "读数本身照给");
        assert!(out.get("warning").is_some(), "陈旧要挂在 warning 上");
    }

    #[test]
    fn wallet_without_openid_cannot_be_attributed() {
        // 归属只认 data.openid：日志里没有它，这个读数就不能算到任何账号头上
        let line = "10-21 07:21:13.530 [22708.20704](info)[js] [] (:0) [Msg]: \"[i]x|[AGUIConnection] agent.action 响应: {\"action\":\"token.wallet\",\"data\":{\"avail_tokens\":\"1\",\"total_tokens\":\"2\"}}\"";
        let wallet = wallet_of_line(line).expect("行本身能解析");
        assert!(wallet.openid.is_empty(), "没有 openid 时字段为空");
        // 空 openid 的账号不该去匹配（latest_wallet 直接返回 None）
        assert!(latest_wallet("", 1_700_000_000_000).is_none());
    }

    #[test]
    fn home_env_is_honored() {
        assert_eq!(HOME_ENV, "WORKBUDDY_MARVIS_HOME");
    }

    /// 假 HOME 目录（`%APPDATA%\Tencent\Marvis` 的替身），跑完删掉。
    fn fake_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent2api-marvis-balance-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("logs")).expect("建临时目录");
        dir
    }

    /// 端到端：日志在 `<home>/logs` 下时，能被找到并归一成读数。
    ///
    /// 用 [`HOME_ENV`] 指向临时目录 —— 单测**绝不能**依赖本机真的装了客户端。
    ///
    /// ⚠️ 行首格式必须与 `Marvis.log` **逐字同款**（`MM-DD HH:MM:SS.mmm ` 之后再
    /// 紧跟内容）。`line_time_ms` 固定切 `[..23]`（23 = 时间戳本身的长度），前缀
    /// 长了就会多切进内容、split 出 8 段而解析失败 —— 第一版这条用例就是拼成
    /// `… .000 [1.2]` 才挂的（切到了 `[1.`），不是解析器的问题。
    #[test]
    fn reads_the_wallet_from_a_home_override() {
        let _guard = env_guard();
        let home = fake_home("read");
        let log = home.join("logs").join("Marvis.log");
        // 时间戳用「刚才」，让新鲜度判定走正常档
        let now = crate::server::logging::now_ms();
        let (month, day, hour, minute, second) = local_parts(now);
        let line = format!(
            "{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.000 [3356.20704](info)[js] [] (:0) \
             [Msg]: \"[i]x|[AGUIConnection] agent.action 响应: {{\"action\":\"token.wallet\",\
             \"data\":{{\"avail_tokens\":\"9981078\",\"frozen_tokens\":\"0\",\
             \"openid\":\"oAN1i2RKFSGN7j81LNLueIhnSTes\",\"total_tokens\":\"10000000\"}}}}\""
        );
        std::fs::write(&log, format!("{line}\n")).expect("写临时日志");
        std::env::set_var(HOME_ENV, &home);

        let wallet = latest_wallet("oAN1i2RKFSGN7j81LNLueIhnSTes", now).expect("应读到读数");
        assert_eq!(wallet.avail, Some(9_981_078.0));
        assert_eq!(wallet.total, Some(10_000_000.0));
        assert!(wallet.at > 0, "时间戳前缀要能解析");

        // 别人的 openid 一律读不到（纪律 1）
        assert!(latest_wallet("oSomeoneElse", now).is_none());

        std::env::remove_var(HOME_ENV);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `WORKBUDDY_MARVIS_DIR` 给的是**版本目录**，日志在
    /// `<root>/Application/<版本>/logs` —— 收集时必须往上退两级回到 `Marvis`。
    ///
    /// 这条单测锁的是一个真踩过的坑：按「上一级就是 Application」写过，结果
    /// `parent().parent()` 才是 `Marvis`，改错了会**静默**读不到任何日志。
    #[test]
    fn install_dir_env_points_at_a_version_directory() {
        let _guard = env_guard();
        let root = std::env::temp_dir().join(format!(
            "agent2api-marvis-install-{}",
            std::process::id()
        ));
        let version_dir = root.join("Application").join("1.2.3.4");
        std::fs::create_dir_all(version_dir.join("logs")).expect("建临时安装目录");
        let log = version_dir.join("logs").join("Marvis.log");
        std::fs::write(&log, "x\n").expect("写临时日志");

        // 传版本目录（与 fingerprint.rs 的口径一致）
        std::env::set_var("WORKBUDDY_MARVIS_DIR", &version_dir);
        let empty_home = std::env::temp_dir().join("agent2api-marvis-no-home");
        let found = log_files(&empty_home);
        std::env::remove_var("WORKBUDDY_MARVIS_DIR");

        assert!(
            found.iter().any(|path| path == &log),
            "版本目录 → 上退两级到 Marvis → 扫 Application/*/logs 必须命中"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真机自检：直接读**本机真实安装目录**的日志，不设任何环境变量。
    ///
    /// 默认 `#[ignore]` —— 它依赖「装了马维斯客户端且登录过」的机器，CI 上必然
    /// 没有。用法（开发机手动跑）：
    /// ```bash
    /// cargo test -p agent2api-server --lib -j 1 \
    ///   marvis::balance::tests::reads_the_real_machine -- --ignored --nocapture
    /// ```
    /// 它回答的是单测回答不了的那个问题：**这条取数链路在本机真的通吗**
    /// （路径、截断、转义层、openid 归属四件事同时成立才算通）。
    #[test]
    #[ignore = "需要本机装有并登录过马维斯客户端"]
    fn reads_the_real_machine() {
        let now = crate::server::logging::now_ms();
        let Some(home) = home_dir() else {
            panic!("找不到 APPDATA");
        };
        let files = log_files(&home);
        println!("日志文件（{} 个）：", files.len());
        for path in &files {
            println!("  {}", path.display());
        }
        // 从日志里反查 openid —— 不依赖账号库里那份，先证明「能读出数」
        let mut found: Vec<(String, f64, Option<f64>, String)> = Vec::new();
        for path in &files {
            for line in tail_lines(path) {
                if !line.contains(WALLET_MARK) {
                    continue;
                }
                let Some(wallet) = wallet_of_line(&line) else {
                    continue;
                };
                if wallet.openid.is_empty() {
                    continue;
                }
                found.push((
                    credentials::masked(&wallet.openid),
                    wallet.avail.unwrap_or(-1.0),
                    wallet.total,
                    line.chars().take(18).collect(),
                ));
            }
        }
        println!("可归属的钱包读数（{} 条）：", found.len());
        for row in &found {
            println!("  openid={} avail={} total={:?} at={}", row.0, row.1, row.2, row.3);
        }
        assert!(!found.is_empty(), "本机日志里至少要能读出一条带 openid 的钱包读数");

        // 用日志里的真实 openid 走一遍完整链路（含新鲜度判定）
        let real_openid = {
            let mut id = String::new();
            for path in &files {
                for line in tail_lines(path) {
                    if !line.contains(WALLET_MARK) {
                        continue;
                    }
                    if let Some(wallet) = wallet_of_line(&line) {
                        if !wallet.openid.is_empty() {
                            id = wallet.openid;
                        }
                    }
                }
            }
            id
        };
        let wallet = latest_wallet(&real_openid, now).expect("按真实 openid 应能取到最新读数");
        println!(
            "最新读数：avail={:?} total={:?} openid={} 新鲜度={:?}",
            wallet.avail,
            wallet.total,
            credentials::masked(&wallet.openid),
            freshness(&wallet, now)
        );
        let document = normalize(&wallet, freshness(&wallet, now));
        println!(
            "余额列 = {}；明细 = {}",
            document["availableView"], document["wallets"][0]["balanceView"]
        );
    }

    /// `WORKBUDDY_MARVIS_DIR` 给的是**版本目录**，日志在
    /// 毫秒时刻 → 本机时区的 (月, 日, 时, 分, 秒)，给上面的测试拼日志前缀。
    fn local_parts(at_ms: i64) -> (u16, u16, u16, u16, u16) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::SYSTEMTIME;
            use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
            // 与 `local_day_key` 同一条链路（×10_000 与 SystemTimeToTzSpecificLocalTime，
            // 两处都踩过，见它的函数注释）
            let Some(utc) = unix_ms_to_file_time(at_ms) else {
                return (1, 1, 0, 0, 0);
            };
            let mut utc_time = SYSTEMTIME::default();
            let mut local = SYSTEMTIME::default();
            let ok = unsafe { FileTimeToSystemTime(&utc, &mut utc_time) }.is_ok()
                && unsafe { SystemTimeToTzSpecificLocalTime(None, &utc_time, &mut local) }.is_ok();
            if ok {
                return (
                    local.wMonth,
                    local.wDay,
                    local.wHour,
                    local.wMinute,
                    local.wSecond,
                );
            }
        }
        (1, 1, 0, 0, 0)
    }
}
