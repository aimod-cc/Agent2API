//! Coze 凭证：Cookie 整包解析 + PAT 铸造缓存 + 账号记录快照。
//!
//! ── 凭证形态（客户端 `main.js` `CREDENTIAL_COOKIE_NAMES` 同款）──────
//! 服务端认证 = `.coze.cn` 域一整包 Cookie。客户端自己列的凭证名：
//! `sessionid` / `sessionid_ss` / `sid_guard` / `sid_tt` / `uid_tt` /
//! `uid_tt_ss` / `passport_csrf_token` / `passport_csrf_token_default` /
//! `ttwid` / `odin_tt`。**整包存、整包带** —— 只挑几个会被判「未登录」
//! （与 KukuAI 的 extras 同教训：接口校验完整会话）。
//! 判定「已登录」的充分证件是 `sessionid_ss`（或 `sessionid`）。
//!
//! ── PAT（短期派生凭证）────────────────────────────────────
//! `POST /api/permission_api/coze_web_app/impersonate_coze_user`
//! body `{duration_seconds: 600}` → `data.access_token`（`cztei_` 前缀）。
//! ⚠️ 响应里的 `expires_in` 是**绝对 epoch 秒**（实测 1792587101 =
//! 2026-10-21 20:51:41，不是剩余秒数）—— `pat_remaining_seconds` 双判。
//! PAT 按账号缓存（进程级 `Mutex<HashMap>`，持锁不跨 await），临期
//! 60 秒内才重铸（桌面客户端 9 分钟一轮，同量级）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::http;
use super::{PAT_DURATION_SECONDS, PAT_REFRESH_MARGIN_SECONDS, USER_AGENT, WEB_BASE};

/// `sessionid_ss` 最小长度（只拦明显残缺的值）
const MIN_SESSION_LENGTH: usize = 8;

/// 一条 Coze 登录态（整包 Cookie + 展示字段）。
#[derive(Clone, Debug)]
pub struct CozeCredentials {
    /// 整包 Cookie（保序、按名去重；含 `sessionid_ss` 等全部凭证）
    pub cookies: Vec<(String, String)>,
    /// `passport/account/info/v2/` 的 `user_id_str`（账号数字标识，id 首选来源）
    pub user_id: String,
    /// 展示名（`screen_name` / `name`，best-effort）
    pub nickname: String,
}

impl CozeCredentials {
    /// 拼出请求用 Cookie 头。
    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 脱敏尾段（展示用；与各家 `tokenTail` 同一口径，取 sessionid_ss 尾 8 位）。
    pub fn token_tail(&self) -> String {
        let session = self
            .cookies
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("sessionid_ss"))
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        if session.len() <= 8 {
            return String::new();
        }
        session.chars().skip(session.len() - 8).collect()
    }

    /// 是否带有效登录态（有 sessionid_ss / sessionid 即认为带）。
    pub fn has_session(&self) -> bool {
        self.cookies.iter().any(|(name, value)| {
            (name.eq_ignore_ascii_case("sessionid_ss") || name.eq_ignore_ascii_case("sessionid"))
                && value.len() >= MIN_SESSION_LENGTH
        })
    }
}

/// 从字符串里解析一对 `name=value`（取首个 `=` 后的全部，容忍空白）。
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

/// 把一段 Cookie 头 / 数组 / 对象统一解析成整包 Cookie（保序、按名去重）。
///
/// 三种形态与 KukuAI 同口径：Cookie 头串、Cookie 编辑器 `[{name,value}]`、
/// 平铺 / 嵌套对象。登录收尾（`login::complete_login`）与账号解析共用这一份。
pub(crate) fn parse_cookie_parts(payload: &Value) -> Vec<(String, String)> {
    let mut cookies: Vec<(String, String)> = Vec::new();
    let mut push = |name: String, value: String| {
        if cookies
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(&name))
        {
            return;
        }
        cookies.push((name, value));
    };
    match payload {
        Value::String(text) => {
            for segment in text.split(';') {
                if let Some((name, value)) = cookie_pair(segment) {
                    push(name, value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                let Some(name) = item.get("name").and_then(Value::as_str) else { continue };
                let Some(value) = item.get("value").and_then(Value::as_str) else { continue };
                let name = name.trim();
                let value = value.trim();
                if !name.is_empty() && !value.is_empty() {
                    push(name.to_string(), value.to_string());
                }
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                let Some(text) = value.as_str() else { continue };
                let text = text.trim();
                if !key.trim().is_empty() && !text.is_empty() {
                    push(key.trim().to_string(), text.to_string());
                }
            }
        }
        _ => {}
    }
    cookies
}

/// 从 payload 解析登录态（Cookie 头 / 数组 / 对象；嵌套 `cookie`/`cookies` 字段递归）。
pub fn credentials_from_payload(payload: &Value) -> Result<CozeCredentials, String> {
    let mut cookies = parse_cookie_parts(payload);
    if cookies.is_empty() {
        if let Value::Object(object) = payload {
            for key in ["cookie", "cookies", "auth", "credentials"] {
                if let Some(inner) = object.get(key) {
                    cookies = parse_cookie_parts(inner);
                    if !cookies.is_empty() {
                        break;
                    }
                }
            }
        }
    }
    let credentials = CozeCredentials {
        cookies,
        user_id: String::new(),
        nickname: String::new(),
    };
    if !credentials.has_session() {
        return Err(
            "缺少有效的 Coze 登录态（Cookie 里没有 sessionid_ss / sessionid；请粘贴 www.coze.cn 的完整 Cookie）"
                .to_string(),
        );
    }
    Ok(credentials)
}

// ─── PAT 铸造与缓存 ─────────────────────────────────────────

/// 一枚 PAT 缓存槽。
struct PatSlot {
    access_token: String,
    /// 绝对过期时刻（epoch 秒）；双判前的原值可能是剩余秒数，已在写入前归一
    expires_at_secs: i64,
}

/// 进程级 PAT 缓存（key = 账号 id；持锁只做哈希表读写，不跨 await）
static PAT_SLOTS: Mutex<Option<HashMap<String, PatSlot>>> = Mutex::new(None);

fn pat_slots() -> std::sync::MutexGuard<'static, Option<HashMap<String, PatSlot>>> {
    match PAT_SLOTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 把上游 `expires_in` 归一成「绝对 epoch 秒」。
///
/// ⚠️ Coze 这个字段回的是**绝对时间戳**（实测 1792587101 = 2026-10-21
/// 20:51:41），不是剩余秒数 —— 桌面客户端 `fetchPatToken` 直接 `Number()`
/// 存下来也印证这点。但为防上游改语义，这里双判：大于 1e9 当绝对时间，
/// 否则当「自此刻起的秒数」。
/// 判据：这个数像不像「绝对 epoch 秒」（> 1e9 = 2001-09 之后的时间戳；
/// 这个量级的「剩余秒数」等于 31 年，不可能是有效期）。
pub(crate) fn epoch_like(raw: i64) -> bool {
    raw > 1_000_000_000
}

fn normalize_expires_at(raw: i64) -> i64 {
    let now = crate::server::logging::now_ms() / 1000;
    if epoch_like(raw) {
        raw
    } else {
        now + raw.max(0)
    }
}

/// 取（必要时重铸）某账号的 PAT。
///
/// `force = true` 跳过缓存直接重铸（401 自愈 / 添加账号验证时用）。
/// 缓存命中条件：距过期还超过 [`PAT_REFRESH_MARGIN_SECONDS`]。
///
/// ── 为什么要跟着账号的出口走（2026-10-22 补）────────────────────
/// 铸造是一次真实的上游请求。原先这里固定传 `None`（= 显式直连），于是
/// **同一台账号出现了两种出口**：对话走它配置的代理、余额 / 签到 / 目录却走直连。
/// 用户配代理的目的（公司网络只放行代理、或本机直连被限）在报告页与签到上
/// 就全失效了，而且症状是「转发正常、余额一直失败」这种最难归因的形态。
/// 现在按 `session.proxy` 取出口，与 `chat.rs` 的转发链同一来源
/// （`session_from_record` 已经把账号的代理解析进会话）。
/// 没配代理时仍然是 `None` —— 那是「直连」的唯一含义，不跟随系统代理
/// （见 `egress::build_client` 的说明）。
pub async fn ensure_pat(
    store: &AccountStore,
    account_id: &str,
    force: bool,
) -> Result<String, GatewayError> {
    let now = crate::server::logging::now_ms() / 1000;
    if !force {
        let guard = pat_slots();
        if let Some(slot) = guard.as_ref().and_then(|map| map.get(account_id)) {
            if slot.expires_at_secs - now > PAT_REFRESH_MARGIN_SECONDS {
                return Ok(slot.access_token.clone());
            }
        }
    }
    let credentials = snapshot_for(store, account_id)?;
    let minted = mint_pat(&credentials, account_proxy(store, account_id).as_ref()).await?;
    {
        let mut guard = pat_slots();
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(
            account_id.to_string(),
            PatSlot {
                access_token: minted.access_token.clone(),
                expires_at_secs: minted.expires_at_secs,
            },
        );
    }
    Ok(minted.access_token)
}

/// 这枚账号配的出网出口（`session.proxy`）；没配 = `None`（直连）。
///
/// 与 `providers::minimax::balance::account_proxy` 同一取法与同一理由：账号记录
/// 上的代理就是用户为**这个账号**挑的出口，任何发往上游的请求都该用它
/// （转发链已经在用，余额 / 签到 / 目录不该另走一条）。
pub fn account_proxy(store: &AccountStore, account_id: &str) -> Option<ResolvedProxy> {
    store
        .get_session_by_id(account_id)
        .and_then(|entry| crate::server::core::proxies::session_proxy(&entry.session))
}

/// 一次铸造的结果（令牌 + 归一后的绝对过期时刻）。
pub struct MintedPat {
    pub access_token: String,
    pub expires_at_secs: i64,
}

/// 打一次 `impersonate_coze_user` 铸一枚新 PAT（带 Cookie + 桌面端形态头）。
///
/// 桌面端同款请求：`{duration_seconds: 600}` + `.coze.cn` 整包 Cookie +
/// `x-space-client: pc` / `x-coze-version: 3.5`（`main.js` `getDesktopHeaders`，
/// 逐字照抄 —— 客户端就是这么打这个接口的）。
pub async fn mint_pat(
    credentials: &CozeCredentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<MintedPat, GatewayError> {
    let url = format!("{WEB_BASE}/api/permission_api/coze_web_app/impersonate_coze_user");
    let mut headers = cookie_headers(credentials);
    headers.push(("x-space-client".to_string(), "pc".to_string()));
    headers.push(("x-coze-version".to_string(), "3.5".to_string()));
    headers.push((
        "x-coze-version-name".to_string(),
        "1.1.46".to_string(),
    ));
    let body = serde_json::json!({ "duration_seconds": PAT_DURATION_SECONDS });
    let value = http::post_json_value(&url, &headers, &body, proxy, Some(30_000)).await?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        return Err(GatewayError::with_status(
            401,
            format!("Coze 铸造访问令牌失败（code={code}：{message}）——请重新登录该账号"),
        ));
    }
    let access_token = value
        .get("data")
        .and_then(|data| data.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if access_token.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "Coze impersonate_coze_user 返回缺少 access_token（上游响应异常）",
        ));
    }
    // 上游给的过期时刻优先（实测是绝对 epoch 秒），缺失时回落约定的 10 分钟
    let expires_at_secs = value
        .get("data")
        .and_then(|data| data.get("expires_in"))
        .and_then(Value::as_i64)
        .map(normalize_expires_at)
        .unwrap_or_else(|| crate::server::logging::now_ms() / 1000 + PAT_DURATION_SECONDS);
    Ok(MintedPat {
        access_token,
        expires_at_secs,
    })
}

/// PAT 归一后的剩余秒数（测试与诊断用）。
pub fn pat_remaining_seconds(raw_expires_in: i64) -> i64 {
    normalize_expires_at(raw_expires_in) - crate::server::logging::now_ms() / 1000
}

// ─── 出站头 ─────────────────────────────────────────────────

/// Cookie 形态出站头（任务中心 / 机器人列表 / 身份 / 活动奖励这条链）。
pub fn cookie_headers(credentials: &CozeCredentials) -> Vec<(String, String)> {
    vec![
        ("Cookie".to_string(), credentials.cookie_header()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("Referer".to_string(), format!("{WEB_BASE}/")),
        ("Origin".to_string(), WEB_BASE.to_string()),
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        (
            "Accept-Language".to_string(),
            "zh-CN,zh;q=0.9".to_string(),
        ),
    ]
}

/// PAT 形态出站头（trade 系列 / 领取 / 推理这条链）。
pub fn pat_headers(pat: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".to_string(), format!("Bearer {pat}")),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("Referer".to_string(), format!("{WEB_BASE}/")),
        ("Origin".to_string(), WEB_BASE.to_string()),
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        (
            "Accept-Language".to_string(),
            "zh-CN,zh;q=0.9".to_string(),
        ),
    ]
}

// ─── 账号记录快照 ───────────────────────────────────────────

/// 从账号记录取转发凭证（`account_id` 为空 → 本家组内优先级最小的启用账号）。
///
/// 记录里的 `accessToken` 存的是整包 Cookie 头串（见账号层 `coze_accounts`）。
pub fn snapshot_for(store: &AccountStore, account_id: &str) -> Result<CozeCredentials, GatewayError> {
    let record = store.coze_account_record(account_id).ok_or_else(|| {
        GatewayError::with_status(
            401,
            "Coze 没有可用的登录凭证：请在账号页添加账号（网页登录或粘贴 Cookie）",
        )
    })?;
    let cookie = record
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let cookies = parse_cookie_parts(&Value::String(cookie));
    let credentials = CozeCredentials {
        cookies,
        user_id: record.get("uid").and_then(Value::as_str).unwrap_or("").to_string(),
        nickname: record.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
    };
    if !credentials.has_session() {
        return Err(GatewayError::with_status(
            401,
            "Coze 账号缺少有效的登录态（sessionid_ss）：请重新添加或重新登录该账号",
        ));
    }
    Ok(credentials)
}

/// 尽力补全账号身份（`user_id` / `nickname`），供账号层落盘。
///
/// `GET /passport/account/info/v2/`（**只能 Cookie 打**，PAT 会返「会话过期」）
/// → `data.user_id_str` / `data.screen_name`。读不到不失败（uid 由账号层用
/// cookie 尾兜底），验证不了登录态才是失败。
pub async fn enrich_identity(
    credentials: &CozeCredentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<CozeCredentials, GatewayError> {
    let url = format!("{WEB_BASE}/passport/account/info/v2/");
    let headers = cookie_headers(credentials);
    let value = http::get_json_value(&url, &headers, proxy, Some(15_000)).await?;
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if message != "success" {
        let description = data
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("账号信息回读失败")
            .to_string();
        return Err(GatewayError::with_status(
            401,
            format!("Coze 登录态校验未通过（{description}）——请重新登录该账号"),
        ));
    }
    let user_id = data
        .get("user_id_str")
        .or_else(|| data.get("user_id"))
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .unwrap_or_default()
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    let nickname = data
        .get("screen_name")
        .or_else(|| data.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(CozeCredentials {
        cookies: credentials.cookies.clone(),
        user_id,
        nickname,
    })
}

/// 账号 id 生成（账号层用）：`user_id` 优先，cookie 尾兜底。
pub fn account_id_of(credentials: &CozeCredentials) -> String {
    if !credentials.user_id.trim().is_empty() {
        credentials.user_id.trim().to_string()
    } else {
        format!("coze-{}", credentials.token_tail())
    }
}

/// 记录里的公开摘要（预检 / 悬停用）。
pub fn record_summary(record: &Value) -> Value {
    let mut map = Map::new();
    map.insert(
        "uid".to_string(),
        Value::String(record.get("uid").and_then(Value::as_str).unwrap_or("").to_string()),
    );
    map.insert(
        "nickname".to_string(),
        Value::String(record.get("loginName").and_then(Value::as_str).unwrap_or("").to_string()),
    );
    map.insert(
        "tokenTail".to_string(),
        Value::String(record.get("tokenTail").and_then(Value::as_str).unwrap_or("").to_string()),
    );
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{epoch_like, normalize_expires_at, parse_cookie_parts, CozeCredentials};

    /// 整包 Cookie 头：保序、按名去重（同名的第一份胜出）。
    #[test]
    fn a_cookie_header_keeps_order_and_drops_duplicates() {
        let parts = parse_cookie_parts(&json!(
            "sessionid_ss=abc123456789; ttwid=w1; sessionid_ss=later; odin_tt=o1"
        ));
        let names: Vec<&str> = parts.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["sessionid_ss", "ttwid", "odin_tt"]);
        // 第一份胜出，不是后到的覆盖
        assert_eq!(parts[0].1, "abc123456789");
    }

    /// Cookie 编辑器导出的 `[{name, value}]` 与 `{name: value}` 对象都能进。
    #[test]
    fn both_cookie_editor_shapes_are_accepted() {
        let array = parse_cookie_parts(&json!([
            { "name": "sessionid_ss", "value": "abc123456789" },
            { "name": "sid_guard", "value": "g1" }
        ]));
        assert_eq!(array.len(), 2);
        let object = parse_cookie_parts(&json!({
            "sessionid_ss": "abc123456789",
            "ttwid": "w1"
        }));
        assert_eq!(object.len(), 2);
        assert_eq!(object[0].0, "sessionid_ss");
    }

    /// 只有 `sessionid` / `sessionid_ss` 才算登录态；太短的值不算。
    #[test]
    fn only_a_real_session_cookie_counts_as_logged_in() {
        let short = CozeCredentials {
            cookies: vec![("sessionid_ss".to_string(), "abc".to_string())],
            user_id: String::new(),
            nickname: String::new(),
        };
        assert!(!short.has_session());
        let full = CozeCredentials {
            cookies: vec![("sessionid".to_string(), "abcdefghij".to_string())],
            user_id: String::new(),
            nickname: String::new(),
        };
        assert!(full.has_session());
    }

    /// `tokenTail` 取 `sessionid_ss` 尾 8 位（与各家同一口径）。
    #[test]
    fn the_tail_comes_from_the_session_cookie() {
        let credentials = CozeCredentials {
            cookies: vec![("sessionid_ss".to_string(), "0123456789abcdef".to_string())],
            user_id: String::new(),
            nickname: String::new(),
        };
        assert_eq!(credentials.token_tail(), "89abcdef");
    }

    /// ⚠️ 这条是本家最容易踩的坑：`expires_in` 回的是**绝对 epoch 秒**
    /// （实测 1792587101 = 2026-10-21 20:51:41），不是剩余秒数。
    /// 双判必须把两种语义都吃下 —— 只认一种会让 PAT 要么永不过期、
    /// 要么每次请求都重铸。
    #[test]
    fn an_absolute_timestamp_is_not_read_as_a_remaining_span() {
        let absolute = 1_792_587_101i64;
        assert_eq!(normalize_expires_at(absolute), absolute);
        // 明显是「还剩多少秒」的形态：必须换算成「此刻 + n」
        let now = crate::server::logging::now_ms() / 1000;
        let relative = normalize_expires_at(600);
        assert!(
            (relative - now - 600).abs() <= 2,
            "剩余秒数应换算成 now+600，实际 {relative}，now={now}"
        );
    }

    /// 双判的判据是 1e9（2021-09 之后的时间戳都大于它，之前的「剩余秒数」
    /// 不可能达到）—— 这里把它钉住，改阈值会当场失败。
    #[test]
    fn the_divider_sits_at_one_billion() {
        assert!(epoch_like(1_000_000_001));
        assert!(!epoch_like(1_000_000_000));
        assert!(!epoch_like(600));
    }
}
