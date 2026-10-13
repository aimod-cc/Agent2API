//! KukuAI 凭证：解析 / 本机登录态读取 / 从账号记录取快照。
//!
//! ── 凭证形态（zhengwuji-workbuddy 实测，见 Acankao/）──────────
//! 服务端只需要三个 Cookie 就能认证：`BDUSS`、`STOKEN`、`gfprotpl=genflowpro`。
//! 其中 `gfprotpl` 是恒定的（值就是 `genflowpro`），不在账号里存，拼 Cookie 头
//! 时固定附加。`BDUSS` 必填；`STOKEN` 可空（部分接口只用 BDUSS，实测三件套中
//! 缺 STOKEN 仍能过多数接口）。
//!
//! ── 本机登录态（`importDesktop: true`）────────────────────────
//! 客户端是标准 Electron：userData 目录 `%APPDATA%\baidugenflowpro\`（注意不是
//! kuku，靠名字猜不到），登录态在 `Network\Cookies`（SQLite）。该文件**不是**
//! Chromium 加密存储：`encrypted_value` 长度为 0，凭据全部落在明文 `value` 列，
//! 因此无需 DPAPI —— 与 CatPaw 的 `auth.json` 同性质（实时读，不落 token）。
//! 客户端运行时 Chromium 会**独占锁**该文件：SQLite 读不到时给出「请先关闭
//! 客户端」的可读文案（对应参考实现的两段式确认，占用冲突处理见账号层）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Map, Value};
use std::path::PathBuf;

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::{USER_AGENT};

/// BDUSS 最小长度（百度通行证登录态，实测远大于此；只拦明显残缺的值）
const MIN_BDUSS_LENGTH: usize = 8;

/// 一条 KukuAI 登录态（转信用核心三件套 + 同会话的其它 Cookie + 展示字段）。
#[derive(Clone, Debug)]
pub struct KukuCredentials {
    pub bduss: String,
    pub stoken: String,
    /// 同一登录会话里的其它 Cookie（`BAIDUID` / `PTOKEN` / `PANPSC` 等）。
    ///
    /// ── 为什么要保留（2026-10-07/08 实测两次修正）────────────
    /// 第一版以为「只带 BDUSS/STOKEN/gfprotpl 会被判未登录，补 BAIDUID
    /// 就好」——那只对客户端导入的凭证成立。10-08 从官方客户端 asar
    /// 逆向 + 换发实测定位到真正机制：业务接口（userreport 等）认的是
    /// **按产品签发的 STOKEN**（`engine.rs`），网页登录 Cookie 里那份
    /// 通行证级 STOKEN 无论带多少 Cookie 都过不了；而 extras 里的
    /// **PTOKEN 正是换发它的必要材料**（`ptoken_of` / `engine.rs`）。
    /// 余额这类通用接口只看 BDUSS —— 所以症状是「余额正常、模型拉不下」。
    pub extras: Vec<(String, String)>,
    /// userreport 返回的 `uk`（账号数字标识，兼作账号 id 来源）
    pub uid: String,
    /// 展示名（best-effort：profile 接口取不到就留空，由账号层兜底）
    pub nickname: String,
}

impl KukuCredentials {
    /// 拼出请求用 Cookie 头（核心三件套 + extras + 恒附加 gfprotpl）。
    pub fn cookie_header(&self) -> String {
        let mut parts = vec![format!("BDUSS={}", self.bduss)];
        if !self.stoken.is_empty() {
            parts.push(format!("STOKEN={}", self.stoken));
        }
        for (name, value) in &self.extras {
            // 主字段与恒附加项不重复（extras 里若混入也跳掉）
            if name.eq_ignore_ascii_case("BDUSS")
                || name.eq_ignore_ascii_case("STOKEN")
                || name.eq_ignore_ascii_case("gfprotpl")
            {
                continue;
            }
            parts.push(format!("{name}={value}"));
        }
        parts.push("gfprotpl=genflowpro".to_string());
        parts.join("; ")
    }

    /// 脱敏尾段（展示用；与各家 `tokenTail` 同一口径）
    pub fn token_tail(&self) -> String {
        if self.bduss.len() <= 8 {
            return String::new();
        }
        self.bduss.chars().skip(self.bduss.len() - 8).collect()
    }
}

/// 从字符串里解析一对 `name=value`（取首个 `=` 后的全部，容忍 `;` 外的空白）。
fn cookie_pair(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let (name, value) = text.split_once('=')?;
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() || value.is_empty() {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

/// 从 Cookie 头串提取核心三件套 + 其余 Cookie（保序、按名去重）。
///
/// `pub(crate)`：登录收尾（`login::complete_login`）要复用同一套解析，
/// 两处共用一份不会出现「登录解析和账号解析认的字段不一样」的分叉。
pub(crate) fn parse_cookie_parts(text: &str) -> (String, String, Vec<(String, String)>) {
    let mut bduss = String::new();
    let mut stoken = String::new();
    let mut extras: Vec<(String, String)> = Vec::new();
    for segment in text.split(';') {
        if let Some((name, value)) = cookie_pair(segment) {
            if name.eq_ignore_ascii_case("BDUSS") {
                bduss = value;
            } else if name.eq_ignore_ascii_case("STOKEN") {
                stoken = value;
            } else if !name.eq_ignore_ascii_case("gfprotpl") {
                if !extras
                    .iter()
                    .any(|(existing, _)| existing.eq_ignore_ascii_case(&name))
                {
                    extras.push((name, value));
                }
            }
        }
    }
    (bduss, stoken, extras)
}

/// 从对象里按大小写不敏感取核心字段 + 其余 Cookie。
fn parts_from_object(object: &Map<String, Value>) -> (String, String, Vec<(String, String)>) {
    let mut bduss = String::new();
    let mut stoken = String::new();
    let mut extras: Vec<(String, String)> = Vec::new();
    for (key, value) in object {
        let Some(text) = value.as_str() else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("BDUSS") {
            bduss = text.to_string();
        } else if key.eq_ignore_ascii_case("STOKEN") {
            stoken = text.to_string();
        } else if !key.eq_ignore_ascii_case("gfprotpl") {
            extras.push((key.clone(), text.to_string()));
        }
    }
    (bduss, stoken, extras)
}

/// 从 `[{name, value}, …]` 数组形态取核心字段 + 其余 Cookie。
fn parts_from_array(items: &[Value]) -> (String, String, Vec<(String, String)>) {
    let mut bduss = String::new();
    let mut stoken = String::new();
    let mut extras: Vec<(String, String)> = Vec::new();
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else { continue };
        let Some(value) = item.get("value").and_then(Value::as_str) else { continue };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if name.eq_ignore_ascii_case("BDUSS") {
            bduss = value.to_string();
        } else if name.eq_ignore_ascii_case("STOKEN") {
            stoken = value.to_string();
        } else if !name.eq_ignore_ascii_case("gfprotpl") {
            extras.push((name.to_string(), value.to_string()));
        }
    }
    (bduss, stoken, extras)
}

/// 从 payload 解析登录态，支持三种形态（与 zhengwuji-workbuddy 面板同口径）：
///   1. **字符串**：整段 Cookie 头（`BDUSS=…; STOKEN=…; …`）；
///   2. **数组**：Cookie 编辑器导出的 `[{name, value}, …]`（kuku2api 的
///      `kuku_cookies.json` 同形态）；
///   3. **对象**：平铺 `{BDUSS, STOKEN}`、嵌套 `{auth: {…}}`，或
///      `{cookie | cookies: "<Cookie 头>"}` 再递归。
pub fn credentials_from_payload(payload: &Value) -> Result<KukuCredentials, String> {
    let (bduss, stoken, extras) = match payload {
        Value::String(text) => parse_cookie_parts(text),
        Value::Array(items) => parts_from_array(items),
        Value::Object(object) => {
            let direct = parts_from_object(object);
            if direct.0.is_empty() && direct.1.is_empty() {
                // 嵌套形态：auth / cookie / cookies 字段再取
                let mut nested = (String::new(), String::new(), Vec::new());
                for key in ["auth", "cookie", "cookies", "credentials"] {
                    if let Some(inner) = object.get(key) {
                        nested = match inner {
                            Value::String(text) => parse_cookie_parts(text),
                            Value::Object(inner_map) => parts_from_object(inner_map),
                            Value::Array(items) => parts_from_array(items),
                            _ => continue,
                        };
                        if !nested.0.is_empty() || !nested.1.is_empty() {
                            break;
                        }
                    }
                }
                nested
            } else {
                direct
            }
        }
        _ => (String::new(), String::new(), Vec::new()),
    };
    let bduss = bduss.trim().to_string();
    let stoken = stoken.trim().to_string();
    if bduss.len() < MIN_BDUSS_LENGTH {
        return Err("缺少有效的 BDUSS（粘贴 Cookie 头、Cookie 编辑器 JSON 或 {BDUSS, STOKEN} 对象）".to_string());
    }
    Ok(KukuCredentials {
        bduss,
        stoken,
        extras,
        uid: String::new(),
        nickname: String::new(),
    })
}

// ─── 本机登录态（importDesktop）────────────────────────────────

/// 候选的 userData 目录：`%APPDATA%\*\baidugenflowpro` 与 `%LOCALAPPDATA%\*\baidugenflowpro`
/// 的通配，外加 `KUKU_USERDATA` 环境变量直指（与参考实现同口径）。
fn user_data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(value) = std::env::var_os("KUKU_USERDATA") {
        let path = PathBuf::from(value);
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    for base in ["APPDATA", "LOCALAPPDATA"] {
        let Some(value) = std::env::var_os(base) else { continue };
        let base = PathBuf::from(value);
        // 固定形态（Electron 默认）：%APPDATA%\baidugenflowpro
        let direct = base.join("baidugenflowpro");
        if !dirs.contains(&direct) {
            dirs.push(direct);
        }
        // 通配形态：%APPDATA%\*\baidugenflowpro（厂商目录名不同时兜底）
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let candidate = entry.path().join("baidugenflowpro");
                if candidate.is_dir() && !dirs.contains(&candidate) {
                    dirs.push(candidate);
                }
            }
        }
    }
    dirs.into_iter().filter(|path| path.is_dir()).collect()
}

/// 定位本机客户端的 Cookies 文件（找不到返回 None）。
pub fn cookies_path() -> Option<PathBuf> {
    for dir in user_data_dirs() {
        let path = dir.join("Network").join("Cookies");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// 读本机客户端登录态（`importDesktop: true` 用）。
///
/// 直读 `Network\Cookies` 的 `value` 列（明文，无需解密）。客户端运行时该文件
/// 被 Chromium 独占锁 —— 读失败时把「可能正在运行」写进文案（占用冲突的
/// 两段式处置在账号层 / 前端，这里只负责如实报错）。
pub fn read_desktop_credentials() -> Result<KukuCredentials, String> {
    let path = cookies_path().ok_or_else(|| {
        "没有找到本机 KukuAI（GenFlowPro）客户端的登录态（%APPDATA%\\baidugenflowpro\\Network\\Cookies）；请先安装并登录客户端，或用「粘贴 Cookie」添加账号".to_string()
    })?;
    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!(
            "读取客户端登录态失败（{}，Cookie 文件可能正被 KukuAI 客户端占用，请先关闭客户端重试）：{error}",
            path.display()
        )
    })?;
    let mut bduss = String::new();
    let mut stoken = String::new();
    let mut extras: Vec<(String, String)> = Vec::new();
    // 读**全部百度域 cookie**（不只 BDUSS/STOKEN）：userreport 校验完整会话，
    // 缺 BAIDUID / BAIDUID_BFESS 会被判「未登录」（2026-10-07 实测，见
    // KukuCredentials::extras 的说明）。统计类（Hm_* / HMACCOUNT）不带 ——
    // 对认证无用，只会把 Cookie 头撑长。
    let mut stmt = conn
        .prepare(
            "SELECT name, value FROM cookies \
             WHERE host_key LIKE '%baidu%' AND value != '' \
               AND name NOT LIKE 'Hm\\_%' ESCAPE '\\' AND name != 'HMACCOUNT'",
        )
        .map_err(|error| format!("读取客户端登录态失败（{error}）"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("读取客户端登录态失败（{error}）"))?;
    for row in rows.flatten() {
        let (name, value) = row;
        if name.eq_ignore_ascii_case("BDUSS") && bduss.is_empty() {
            bduss = value;
        } else if name.eq_ignore_ascii_case("STOKEN") && stoken.is_empty() {
            stoken = value;
        } else if !name.eq_ignore_ascii_case("BDUSS")
            && !name.eq_ignore_ascii_case("STOKEN")
            && !name.eq_ignore_ascii_case("gfprotpl")
            && !extras
                .iter()
                .any(|(existing, _)| existing.eq_ignore_ascii_case(&name))
        {
            extras.push((name, value));
        }
    }
    if bduss.len() < MIN_BDUSS_LENGTH {
        return Err("本机客户端未登录（Cookies 里没有有效的 BDUSS）；请先在 KukuAI 客户端登录".to_string());
    }
    Ok(KukuCredentials {
        bduss,
        stoken,
        extras,
        uid: String::new(),
        nickname: String::new(),
    })
}

/// 本机登录态的展示摘要（前端导入按钮悬停 / 预检用）。
pub fn desktop_summary() -> Result<Value, String> {
    let credentials = read_desktop_credentials()?;
    Ok(serde_json::json!({
        "uid": credentials.uid,
        "nickname": credentials.nickname,
        "tokenTail": credentials.token_tail(),
    }))
}

/// 从 extras 里取 PTOKEN（`PTOKEN` 优先，影子 `PTOKEN_BFESS` 兜底）。
///
/// PTOKEN 是换发「genflowpro 作用域 STOKEN」的必要材料（见 `engine.rs`），
/// 网页登录 / 手动粘贴完整 Cookie / 本机客户端登录态三条路径都带它。
pub(crate) fn ptoken_of(extras: &[(String, String)]) -> Option<String> {
    let pick = |want: &str| {
        extras
            .iter()
            .find(|(name, value)| name.eq_ignore_ascii_case(want) && !value.trim().is_empty())
            .map(|(_, value)| value.trim().to_string())
    };
    pick("PTOKEN").or_else(|| pick("PTOKEN_BFESS"))
}

/// 尽力把凭证的 STOKEN 换成「genflowpro 作用域」的新令牌（见 `engine.rs`）。
///
/// 有 PTOKEN 且换发成功才替换，否则原样克隆返回（失败只记日志，不阻断）。
/// 签到这类**直接打业务接口**、不经过 `session::ensure_tokens` -6 自愈路径
/// 的场景用它保障会话可用；每次换发就是一次 passport POST，签到一天一次
/// 的频率没有风控顾虑。
pub async fn with_business_stoken(credentials: &KukuCredentials) -> KukuCredentials {
    let mut prepared = credentials.clone();
    if let Some(ptoken) = ptoken_of(&prepared.extras) {
        match super::engine::exchange_genflowpro_stoken(&prepared.bduss, &ptoken).await {
            Ok(stoken) => {
                prepared.stoken = stoken;
            }
            Err(error) => {
                crate::server::logging::log(
                    "[KukuAI]",
                    &format!("⚠️ 换发业务会话令牌失败（{}），按原凭证继续", error.message),
                );
            }
        }
    }
    prepared
}

/// 把「本机或手动解析出来的凭证」补上账号身份（uid / nickname），供账号层落盘。
///
/// ── 换发业务会话令牌（2026-10-08 实测后加，见 `engine.rs` 模块头）────
/// KukuAI 业务接口只认**按产品签发**的 STOKEN，网页登录 Cookie 里那份
/// 通行证级 STOKEN 过不了校验。所以验证前先拿 `BDUSS + PTOKEN` 向百度
/// 通行证换发一次；换发失败（没装客户端 / 缺 PTOKEN）不阻断 —— 按原
/// 凭证继续走，行为与旧版一致。
///
/// `userreport` 换会话三件套的同时带回 `uk`；`settings/profile` 带昵称（best-effort）。
/// **验证失败是错误**（返回 `Err`）：添加账号时调用方据此拒绝无效凭证
/// （与 Go 版保存时验证同一口径）；身份字段读不到（`uk` 为空）**不是失败**，
/// 账号照样落（uid 由账号层用 cookie 尾兜底）。`force` 表示要不要跳过会话槽
/// 缓存（添加账号时用 `force = true`，要的是「现在真的能用」的即时验证）。
pub async fn enrich_identity(
    credentials: &KukuCredentials,
    proxy: Option<&crate::server::core::proxies::ResolvedProxy>,
    force: bool,
) -> Result<KukuCredentials, GatewayError> {
    let prepared = with_business_stoken(credentials).await;
    let triple = super::session::ensure_tokens(&prepared, proxy, force).await?;
    let mut enriched = prepared;
    if !triple.uk.is_empty() {
        enriched.uid = triple.uk;
    }
    Ok(enriched)
}

/// 出站通用请求头（Cookie + UA + 同源 Referer/Origin）。
pub fn request_headers(credentials: &KukuCredentials) -> Vec<(String, String)> {
    vec![
        ("Cookie".to_string(), credentials.cookie_header()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("Referer".to_string(), format!("{}/genflowpro", super::BASE_URL)),
        ("Origin".to_string(), super::BASE_URL.to_string()),
        ("Accept-Language".to_string(), "zh-CN,zh;q=0.9".to_string()),
    ]
}

/// 从账号记录取转发凭证（`account_id` 为空 → 本家组内优先级最小的启用账号）。
///
/// 记录里的 `accessToken` 存的是 Cookie 头串（`BDUSS=…; STOKEN=…`，见账号层）。
pub fn snapshot_for(store: &AccountStore, account_id: &str) -> Result<KukuCredentials, GatewayError> {
    let record = store.kuku_account_record(account_id).ok_or_else(|| {
        GatewayError::with_status(
            401,
            "KukuAI 没有可用的登录凭证：请在账号页添加账号（粘贴 Cookie 或导入本机客户端登录态）",
        )
    })?;
    let cookie = record
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (bduss, stoken, extras) = parse_cookie_parts(&cookie);
    if bduss.len() < MIN_BDUSS_LENGTH {
        return Err(GatewayError::with_status(
            401,
            "KukuAI 账号缺少有效的 BDUSS：请重新添加或重新导入该账号",
        ));
    }
    let uid = record.get("uid").and_then(Value::as_str).unwrap_or("").to_string();
    let nickname = record.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    Ok(KukuCredentials {
        bduss,
        stoken,
        extras,
        uid,
        nickname,
    })
}
