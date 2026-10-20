//! 网关错误类型与 OpenAI 风格错误 payload。
//!
//! 对照 Node 版 server.mjs 313-332 行的 `errorPayload()`：
//! 上游/业务错误统一带 `statusCode`（可选 `upstreamCode`），
//! 由本模块翻译成给客户端看的 `{ error: { message, type, ... } }`。
//!
//! 分类规则（照抄，不做「顺手修复」）：
//!   - 429 或上游码 6004            → rate_limit_exceeded（并附 reset_at 恢复时间）
//!   - 401                          → authentication_error
//!   - statusCode < 500             → invalid_request_error
//!   - 其余（含 5xx 与非法 status）  → proxy_error
//!
//! 默认状态码 500：Node 版用 `Number.isInteger(statusCode) ? statusCode : 500`，
//! 这里用 `Option<i32>` 表达同一语义。
//!
//! 注意：项目 release profile 是 `panic = "abort"`，handler 里任何 panic
//! 都会直接带走整个桌面应用，所以错误一律走 Result + 本模块的类型，
//! 不用 unwrap/expect。

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::TimeZone;
use serde_json::{json, Map, Value};

/// 账号限额的上游业务码（Node 版 QUOTA_LIMIT_CODE）
pub const QUOTA_LIMIT_CODE: i64 = 6004;

/// 统一的网关错误。
///
/// `status_code` 对应 Node 版错误对象上的 `statusCode`，
/// `upstream_code` 对应各模块的 `upstreamCode`（带上就会出现在响应里，
/// 客户端据此识别腾讯侧的业务码），
/// `code` 对应 Node 版手写 body 里的 `code` 字段（目前只有模型不存在用：
/// `code: 'model_not_found'`）。
#[derive(Clone, Debug)]
pub struct GatewayError {
    pub message: String,
    pub status_code: i32,
    pub upstream_code: Option<i64>,
    pub code: Option<String>,
    /// `Retry-After`（秒）；`None` = 不带这个头。
    ///
    /// 只有**算得出恢复时刻**的限额类错误才该有值（见
    /// [`GatewayError::with_retry_after_seconds`]）：给不出时刻就不给头，
    /// 让客户端按自己的退避来 —— 编一个等待时长比不给更坏。
    pub retry_after_secs: Option<u64>,
}

impl GatewayError {
    /// 通用构造：默认 500（对应 Node 版「非整数状态码回落 500」）
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status_code: 500,
            upstream_code: None,
            code: None,
            retry_after_secs: None,
        }
    }

    /// 指定状态码
    pub fn with_status(status: i32, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status_code: status,
            upstream_code: None,
            code: None,
            retry_after_secs: None,
        }
    }

    /// 400 参数错误（后续切片的入参校验用）
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::with_status(400, message)
    }

    /// 挂上错误码字符串（Node 版手写 body 里的 `code` 字段）
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// 挂上上游业务码（如 6004 限额）
    pub fn upstream_code(mut self, code: i64) -> Self {
        self.upstream_code = Some(code);
        self
    }

    /// 挂上游业务码（Option 形态；None 时保持原值不变）——
    /// 转发链路里上游码本来就是 Option，用这个可以省掉一次分支。
    pub fn with_optional_code(mut self, code: Option<i64>) -> Self {
        if let Some(code) = code {
            self.upstream_code = Some(code);
        }
        self
    }

    /// 是否为账号限额错误（HTTP 429 或上游 code 6004）
    pub fn is_quota_limit(&self) -> bool {
        self.status_code == 429 || self.upstream_code == Some(QUOTA_LIMIT_CODE)
    }

    /// 挂上 `Retry-After`（秒）。
    ///
    /// 用在**已知恢复时刻**的限额类错误上（上游给了窗口重置文案、我们算得出
    /// 还差多久）。⚠️ 算不出就别挂 —— 客户端会照这个值退避，编一个时长比不给更坏。
    /// 0 秒会被响应侧忽略（见 `response`）。
    pub fn with_retry_after_seconds(mut self, seconds: u64) -> Self {
        self.retry_after_secs = Some(seconds);
        self
    }

    /// 归一后的状态码：非 100..600 的取值一律当 500（防止把非法码塞进 HTTP 响应）
    fn safe_status(&self) -> i32 {
        if (100..600).contains(&self.status_code) {
            self.status_code
        } else {
            500
        }
    }

    /// 错误类型串，照抄 Node 版 errorPayload 的判定顺序
    fn error_type(&self) -> &'static str {
        if self.is_quota_limit() {
            "rate_limit_exceeded"
        } else if self.status_code == 401 {
            "authentication_error"
        } else if self.status_code < 500 {
            "invalid_request_error"
        } else {
            "proxy_error"
        }
    }

    /// 构造 OpenAI 风格错误 body（对应 Node 版 errorPayload().body）。
    ///
    /// 字段顺序与 Node 版一致：message → type → upstream_code（可选）→
    /// `code`（可选，手写 body 用）→ reset_at（可选）。
    /// `reset_at` / `reset_at_text` 只在成功解析出恢复时间时出现（Node 版同理）。
    pub fn payload(&self) -> Value {
        let mut error = Map::new();
        error.insert("message".to_string(), Value::String(self.message.clone()));
        error.insert("type".to_string(), Value::String(self.error_type().to_string()));
        if let Some(code) = &self.code {
            error.insert("code".to_string(), Value::String(code.clone()));
        }
        if let Some(code) = self.upstream_code {
            error.insert("upstream_code".to_string(), Value::from(code));
        }
        if self.is_quota_limit() {
            let reset_at = parse_quota_reset_at(&self.message);
            if reset_at > 0 {
                error.insert("reset_at".to_string(), Value::from(reset_at));
                error.insert(
                    "reset_at_text".to_string(),
                    Value::String(format_reset_at_text(reset_at)),
                );
            }
        }
        json!({ "error": Value::Object(error) })
    }

    /// 直接转成 HTTP 响应（状态码 + OpenAI 风格 body）。
    ///
    /// 与 `IntoResponse` 等价，区别只是调用点写起来更明确 ——
    /// 转发链路里 `Err(error) => error.payload_response()` 比
    /// `IntoResponse::into_response(error)` 可读。
    pub fn payload_response(self) -> Response {
        self.response()
    }

    /// 唯一的响应构造点：状态码 + body（+ 有恢复时刻时挂 `Retry-After`）。
    ///
    /// `IntoResponse` 与 `payload_response` 都走这里，免得两条路只改了一条
    /// （之前它们各写一遍 `(status, Json(payload))`）。
    fn response(self) -> Response {
        let status = self.http_status();
        // 0 秒不当 `Retry-After` 发：上游说「马上就好」时让客户端自己退避，
        // 而不是收到一个 `Retry-After: 0` 立刻再打一发。
        let retry_after = self.retry_after_secs.filter(|seconds| *seconds > 0);
        let mut response = (status, Json(self.payload())).into_response();
        if let Some(seconds) = retry_after {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }

    /// HTTP 状态码
    pub fn http_status(&self) -> StatusCode {
        StatusCode::from_u16(self.safe_status() as u16).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for GatewayError {}

/// 让 handler 可以直接 `Result<T, GatewayError>` 返回，错误自动变成
/// 状态码 + OpenAI 风格 body（无需每个 handler 手写转换）。
impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        self.response()
    }
}

/// 从 `std::io::Error` / `serde_json::Error` 这类常见错误快速转成 500。
impl From<std::io::Error> for GatewayError {
    fn from(error: std::io::Error) -> Self {
        Self::new(format!("文件操作失败: {error}"))
    }
}

impl From<serde_json::Error> for GatewayError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(format!("JSON 解析失败: {error}"))
    }
}

/// 401 未授权响应。
///
/// 文案与 type 都照抄 Node 版 unauthorized()：
/// `{error:{message:"Unauthorized: invalid or missing API key", type:"invalid_api_key"}}`
///
/// 注意 type 是 `invalid_api_key` 而**不是** errorPayload() 给 401 用的
/// `authentication_error` —— 因为 Node 的 unauthorized() 是自己拼的 body，
/// 没走 errorPayload()。两者不要混用。
pub fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "Unauthorized: invalid or missing API key",
                "type": "invalid_api_key",
            }
        })),
    )
        .into_response()
}

/// 401：面板登录缺失（web_shim 据此弹「账号密码」框而不是 Key 框）。
///
/// `type` 用 `panel_login_required` 与普通 Key 鉴权失败（`invalid_api_key`）
/// 区分 —— 两者都是 401，但浏览器面板对它们的反应不同。
///
/// 需要 `headers` 是为了判断「refresh 还活不活」——见下面的清除条件。
pub fn panel_login_required_response(headers: &axum::http::HeaderMap) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "请先登录管理面板",
                "type": "panel_login_required",
            }
        })),
    )
        .into_response();
    // 只在**整条会话链确实没了**时才清那枚会话 cookie —— 判据见 `access::has_live_refresh`。
    //
    // refresh 还活着时**必须留着它**：access 的生命周期只有 2 小时（`ACCESS_TTL`），
    // 而会话 cookie 里同时装着 30 天的 refresh（一枚装两段，见 `access::ACCESS_COOKIE`）。
    // 401 无差别清除等于把 30 天的静默续期砍成每 2 小时强制重登一次 —— 浏览器删 cookie
    // 要名字与 Path 同时命中，清掉之后客户端紧接着那次 `/api/panel/refresh` 就没有
    // refresh 候选了（直连 cookie 模式下 localStorage 里没有令牌，头回退也走不通）。
    //
    // 而链已经没了的时候，不清才是"反复"的成因：罐里那枚死凭据每次开页都被带回来、
    // 服务端每次重新判一次无效（一手读数：同一把对不上任何登录的指纹两天里出现 51 次，
    // 横跨四个时段；后来 dev 上的一条清除头把它从共享罐子里删掉，两个实例此后都是 0 次）。
    //
    // 清的时候只发一条 `Set-Cookie`：合并型中转会把多条拼成一条（见
    // `access::session_valid` 的事故记录），两条清除头等于一条都到不了。
    if !crate::server::access::has_live_refresh(headers) {
        if let Ok(value) = axum::http::HeaderValue::from_str(&crate::server::access::clear_cookie())
        {
            response
                .headers_mut()
                .append(axum::http::header::SET_COOKIE, value);
        }
    }
    response
}

/// 503：headless 公网形态未配置任何 API Key 时，`/v1/*` 的 fail-closed 响应。
///
/// 与 401 不同：这不是「Key 错了」，而是「还没有任何 Key 可校验」——
/// 引导用户去面板创建第一把（面板本身经入口认证，不需要 Key）。
pub fn v1_fail_closed_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": {
                "message": "网关尚未配置任何 API Key：请登录管理面板，在「网关 Key」页创建一把",
                "type": "api_key_not_configured",
            }
        })),
    )
        .into_response()
}

/// 404 兜底响应。
///
/// 注意这里**不带 type 字段** —— Node 版 404 分支发的是
/// `{ error: { message: "Not found: <METHOD> <path>" } }`，
/// 没有经过 errorPayload()，因此不会有 type/upstream_code。
/// 单独出一个构造函数而不是复用 GatewayError，就是为了不「顺手」补上 type。
pub fn not_found_response(method: &str, path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": { "message": format!("Not found: {method} {path}") } })),
    )
        .into_response()
}

/// 管理 API 的错误信封 `{ success: false, error: "..." }`。
/// 与 OpenAI 风格 payload 不同 —— 管理接口（/api/*）走这个形状，
/// 对应 Node 版各 route 模块里的 `sendJson(res, status, { success:false, error })`。
pub fn management_error(status: i32, message: impl Into<String>) -> Response {
    let status = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::BAD_REQUEST);
    (status, Json(json!({ "success": false, "error": message.into() }))).into_response()
}

/// 解析上游限额恢复时间：`YYYY-MM-DD HH:MM:SS UTC+8` → 毫秒时间戳。
///
/// 对应 Node 版 parseQuotaResetAt（workbuddy-upstream-client.mjs 82-87 行）：
/// ```js
/// /(\d{4})-(\d{2})-(\d{2})\s+(\d{2}):(\d{2}):(\d{2})\s*UTC\+8/
/// ```
/// 解析不出时返回 0（Node 版同样返回 0，调用方据此决定要不要带 reset_at 字段），
/// 而不是报错 —— 这条路径在处理上游错误，不能再制造新错误。
///
/// ── 为什么改写成「按字节扫描 + 数值解析」（切片 4 发现并修复）────
/// 上一版是「滑动窗口 + `&text[start..start+19]` 切片」，在**中文错误文案**上
/// 会直接 panic（release 是 panic=abort，会带走整个桌面应用）：
/// 上游 429 的 msg 是「您的使用量已超出频率限制，将在 2026-09-11 19:43:46
/// UTC+8 重置…」，窗口起点一旦落在汉字中间（如「您」的第 2、3 字节），
/// 切片就在非字符边界上，`is_char_boundary` 的检查写在切片**之后**已经来不及。
/// 实测这条路径在真实 429 响应上 100% 触发。
///
/// 现在全程按 `&[u8]` 判定：只有当某个位置起确实是 `dddd-dd-dd` 形态
/// （全是 ASCII 数字与 `-`）才继续，而 ASCII 字节不可能是多字节字符的
/// 续字节，所以后续任何切片都天然落在字符边界上。顺带把 Node 正则里
/// 的 `\s+`（日期与时间之间至少一个空白）与 `\s*UTC+8` 后缀也补齐了 ——
/// 上一版只认「一个空格 + 19 字符定长窗口」且不校验 UTC+8。
pub fn parse_quota_reset_at(text: &str) -> i64 {
    parse_quota_reset_at_at(text, crate::server::logging::now_ms())
}

/// [`parse_quota_reset_at`] 的**可注入「现在」**版本。
///
/// 只有「当日 / 今日 HH:MM」那一形需要知道今天是哪天（见 `scan_same_day_window`），
/// 所以把 now 提出来当入参 —— 线上入口仍是不带 now 的那个，测试钉死日期用这个。
pub fn parse_quota_reset_at_at(text: &str, now_ms: i64) -> i64 {
    let bytes = text.as_bytes();
    // 空白判定：`\s` 在 Node 正则里含空格/制表/换行（字节层面判 ASCII 空白即可）
    let space = |index: usize| -> bool {
        bytes
            .get(index)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
    };
    /// 取 `width` 位十进制数字（不足或非数字返回 None）
    fn number(bytes: &[u8], start: usize, width: usize) -> Option<u32> {
        let mut value = 0u32;
        for offset in 0..width {
            let byte = bytes.get(start + offset)?;
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + (byte - b'0') as u32;
        }
        Some(value)
    }

    let mut index = 0usize;
    while index < bytes.len() {
        // ① dddd-dd-dd
        let date = (
            number(bytes, index, 4),
            bytes.get(index + 4),
            number(bytes, index + 5, 2),
            bytes.get(index + 7),
            number(bytes, index + 8, 2),
        );
        if let (Some(year), Some(b'-'), Some(month), Some(b'-'), Some(day)) = date {
            // ② \s+ 分隔
            let mut cursor = index + 10;
            let space_start = cursor;
            while space(cursor) {
                cursor += 1;
            }
            if cursor > space_start {
                // ③ dd:dd:dd
                let time = (
                    number(bytes, cursor, 2),
                    bytes.get(cursor + 2),
                    number(bytes, cursor + 3, 2),
                    bytes.get(cursor + 5),
                    number(bytes, cursor + 6, 2),
                );
                if let (Some(hour), Some(b':'), Some(minute), Some(b':'), Some(second)) = time {
                    // ④ \s*UTC+8 —— 必须命中，否则不是限额恢复时间（与 Node 正则一致）
                    let mut suffix = cursor + 8;
                    while space(suffix) {
                        suffix += 1;
                    }
                    if bytes[suffix..].starts_with(b"UTC+8") {
                        if let Some(millis) = to_utc_plus_8_millis(year, month, day, hour, minute, second) {
                            return millis;
                        }
                    }
                }
            }
        }
        index += 1;
    }
    // ② 「当日 / 今日 HH:MM」—— 另一种常见写法：没有日期、没有秒、也没有
    //    `UTC+8` 后缀，所以上面的扫描认不出来。AtomCode 的配额文案就是这个形态：
    //    「…当前滚动窗口约在当日 18:18 结束并重置额度，请稍后重试。」
    //    （2026-10-18 在 dev 上逐字抓过）。⚠️ 改这一形之前，这类文案解析出来是 0，
    //    于是冷却落到「10 分钟兜底」而不是真正的窗口重置点。
    scan_same_day_window(text, now_ms).unwrap_or(0)
}

/// 「当日 / 今日 HH:MM」→ 北京时间**当天**那个时刻的毫秒时间戳。
///
/// 判据（比完整时刻形宽松，因为它本来就没给日期与秒）：
///   · 关键词 `当日` / `今日` 之后的 **16 个字符**内出现 `H[H]:MM`；
///   · 越界（`25:00` 之类）与「分钟后面还接数字」都不认；
///   · 算出来**已经过去** ⇒ `None`（这个窗口早就重置了。返回一个过去的时刻
///     等于让调用方写一条立刻过期的冷却，比不解析更坏）。
fn scan_same_day_window(text: &str, now_ms: i64) -> Option<i64> {
    let date = crate::server::core::beijing::date_time_of_ms(now_ms)?.date_naive();
    for keyword in ["当日", "今日"] {
        let Some(at) = text.find(keyword) else {
            continue;
        };
        // 按**字符**取窗口：`&rest[..16]` 落在汉字中间会 panic
        // （release profile 是 panic=abort，这类切片错误会带走整个进程）
        let rest: String = text[at + keyword.len()..].chars().take(16).collect();
        let Some((hour, minute)) = first_clock(&rest) else {
            continue;
        };
        let Some(naive) = date.and_hms_opt(hour, minute, 0) else {
            continue;
        };
        let instant = crate::server::core::beijing::offset()
            .from_local_datetime(&naive)
            .single()?;
        let millis = instant.timestamp_millis();
        if millis > now_ms {
            return Some(millis);
        }
    }
    None
}

/// 文本里第一处 `H[H]:MM`（小时 1–2 位、分钟必须 2 位，且分钟之后不再接数字）。
fn first_clock(text: &str) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let mut cursor = index;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        let hour_digits = cursor - index;
        // 全是 ASCII 数字 ⇒ 按字节切片天然落在字符边界上
        let hour = text[index..cursor].parse::<u32>().ok();
        let minute = (bytes.get(cursor) == Some(&b':')
            && bytes.get(cursor + 1).is_some_and(u8::is_ascii_digit)
            && bytes.get(cursor + 2).is_some_and(u8::is_ascii_digit))
        .then(|| u32::from(bytes[cursor + 1] - b'0') * 10 + u32::from(bytes[cursor + 2] - b'0'));
        let trailing_digit = bytes.get(cursor + 3).is_some_and(u8::is_ascii_digit);
        if let (Some(hour), Some(minute)) = (hour, minute) {
            if (1..=2).contains(&hour_digits) && !trailing_digit && hour < 24 && minute < 60 {
                return Some((hour, minute));
            }
        }
        index = cursor;
    }
    None
}

/// `YYYY-MM-DD HH:MM:SS`（UTC+8）→ 毫秒时间戳；字段非法时 None。
///
/// 不引入 regex：手写扫描比正则更容易控制「失败即 0」的语义。
fn to_utc_plus_8_millis(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<i64> {
    // UTC+8 固定偏移：上游文案里就是这么标的，不做时区探测
    let offset = chrono::FixedOffset::east_opt(8 * 3600)?;
    let naive = chrono::NaiveDate::from_ymd_opt(year as i32, month, day)?
        .and_hms_opt(hour, minute, second)?;
    Some(offset.from_local_datetime(&naive).single()?.timestamp_millis())
}

/// 恢复时间的本地化展示（对应 Node 版 `toLocaleString('zh-CN', { hour12:false })`）。
/// 统一按 UTC+8 渲染：上游恢复时间本身就是 UTC+8 标定的，
/// 用本机时区反而会让用户对不上上游给的原文。
pub(crate) fn format_reset_at_text(reset_at: i64) -> String {
    // 固定偏移 +8 一定能构造成功；万一失败就给空串（这只是展示文案，
    // 绝不能因为一个格式化失败把请求带崩 —— release 是 panic=abort）
    let Some(offset) = chrono::FixedOffset::east_opt(8 * 3600) else {
        return String::new();
    };
    match chrono::DateTime::from_timestamp_millis(reset_at) {
        Some(utc) => utc
            .with_timezone(&offset)
            .format("%Y/%m/%d %H:%M:%S")
            .to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod panel_401_cookie_tests {
    //! 面板 401 的 cookie 清除**只在会话链确实没了时才发**：
    //!
    //! - 链没了还留着那枚 cookie 就是「反复」的成因 —— 罐里那枚同名 cookie 每次开页
    //!   都原样带回来，服务端每次重新判一次无效（生产读数：同一把对不上任何登录的
    //!   指纹两天里出现 51 次）。
    //! - 链还活着却清掉，是另一个方向的错，而且更贵：access 只有 2 小时寿命、会话
    //!   cookie 里同时装着 30 天的 refresh，一过期就清等于把静默续期砍成每 2 小时
    //!   强制重登一次（客户端拿到 401 的下一步就是拿这枚 cookie 去换新令牌）。
    //!
    //! 两个方向各有一条用例钉住。清除头只发一条：合并型中转会把多条拼成一条
    //! （见 `access::session_valid` 的事故记录），两条清除头等于一条都到不了。

    use super::*;
    use crate::server::access;
    use axum::http::{HeaderMap, HeaderValue};

    /// 只带 `Cookie` 头的请求。
    fn with_cookie(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    fn set_cookies(response: &Response) -> Vec<String> {
        response
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn the_panel_401_clears_the_stale_access_cookie() {
        // 没有任何 refresh 候选（匿名访问 / 罐里只剩死值）：该清
        let response = panel_login_required_response(&HeaderMap::new());
        let cookies = set_cookies(&response);
        assert_eq!(cookies.len(), 1, "401 只该发一条清除头（多条会被中转拼坏）");
        assert!(cookies[0].starts_with(&format!("{}=;", access::ACCESS_COOKIE)));
        assert!(cookies[0].contains("Path=/"), "清除要落在 access 的签发 Path 上");
        assert!(cookies[0].contains("Max-Age=0"));
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "错误语义不能因为加了一条头就变"
        );
    }

    /// **反对照（这条是回归的钉子）**：access 过期但 refresh 还活着时，401 不许清
    /// cookie —— 一枚都不许发。
    ///
    /// 清掉的后果是可观测的：浏览器删 cookie 要名字与 Path 同时命中，这枚 cookie
    /// 里同时装着 refresh，删掉之后客户端紧接着的 `/api/panel/refresh` 就没有任何
    /// refresh 候选了（直连 cookie 模式下 localStorage 是空的，头回退也走不通），
    /// 于是每 2 小时（`ACCESS_TTL`）被强制重登一次。
    #[test]
    fn the_panel_401_leaves_a_live_refresh_alone() {
        let session = access::IssuedSession::new_session();
        let headers = with_cookie(&format!(
            "{}=not-a-live-access.{}",
            access::ACCESS_COOKIE, session.refresh_token
        ));
        // 前提：这把 refresh 确实活着（否则下面那条断言就在真空里跑）
        assert!(
            access::has_live_refresh(&headers),
            "签发的 refresh 应当被判为活的"
        );
        let response = panel_login_required_response(&headers);
        assert!(
            set_cookies(&response).is_empty(),
            "refresh 还活着时 401 不许清 cookie —— 清了就把静默续期砍掉"
        );
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[cfg(test)]
mod quota_reset_text_tests {
    //! 上游配额文案里「什么时候恢复」的**两种写法**。
    //!
    //! 一种是完整时刻（`… 2026-09-11 19:43:46 UTC+8 重置`，Node 版正则那一形），
    //! 另一种只有当日时刻（`… 当前滚动窗口约在当日 18:18 结束并重置额度 …`，
    //! AtomCode 的实测原话）。第二种在解析器认出它之前会被读成 0，于是账号冷却
    //! 落到「10 分钟兜底」而不是真正的窗口重置点。

    use super::*;

    /// 北京时间某个整分的毫秒时间戳（钉死日期用；秒固定 0）。
    fn beijing(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        crate::server::core::beijing::offset()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("合法时刻")
            .timestamp_millis()
    }

    #[test]
    fn the_same_day_window_form_is_readable() {
        // 2026-10-18 在 dev 上逐字抓到的原话
        let text = "模型「glm5.3-flash」在当前 CodingPlan 用量窗口内已达上限。\
                    当前滚动窗口约在当日 18:18 结束并重置额度，请稍后重试。";
        let now = beijing(2026, 10, 18, 12, 0);
        assert_eq!(
            beijing(2026, 10, 18, 18, 18),
            parse_quota_reset_at_at(text, now),
            "「当日 18:18」应读成北京时间当天 18:18"
        );
        // 「今日」是同一件事的另一种说法
        assert_eq!(
            beijing(2026, 10, 18, 18, 18),
            parse_quota_reset_at_at("约在今日 18:18 结束并重置额度", now)
        );
    }

    #[test]
    fn a_same_day_window_already_in_the_past_is_not_invented() {
        // 文案是旧的（窗口早就过了）：返回 0 让调用方用自己的兜底，
        // 而不是拿一个过去的时刻去写冷却 —— 那种冷却等于没冷却。
        let now = beijing(2026, 10, 18, 12, 0);
        assert_eq!(0, parse_quota_reset_at_at("约在今日 09:05 结束并重置额度", now));
    }

    #[test]
    fn the_full_utc_plus_8_stamp_still_parses() {
        // 回归：新增第二种形态不能把原来那一形弄坏
        let text = "您的使用量已超出频率限制，将在 2026-09-11 19:43:46 UTC+8 重置";
        assert_eq!(
            beijing(2026, 9, 11, 19, 43) + 46_000,
            parse_quota_reset_at(text)
        );
        assert_eq!(0, parse_quota_reset_at("正文里没有任何时刻"));
    }

    #[test]
    fn malformed_clocks_are_not_read() {
        let now = beijing(2026, 10, 18, 12, 0);
        for text in [
            "当日 25:00 结束",   // 小时越界
            "当日 18:6 结束",    // 分钟只有一位
            "当日 1818 结束",    // 没有冒号
            "当日 18:180 结束",  // 分钟后面还接着数字
        ] {
            assert_eq!(0, parse_quota_reset_at_at(text, now), "不该认得：{text}");
        }
    }
}
