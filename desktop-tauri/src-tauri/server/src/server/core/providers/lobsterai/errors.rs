//! LobsterAI 的错误分类，以及**两种「成功状态下的失败」**的识别。
//!
//! ── 判定顺序即优先级（别重排，`errors.go:55-94` 的原话）──────────
//! ```text
//!   1. HTTP 402              -> 余额耗尽（状态码最权威）
//!   2. 正文含余额耗尽关键词  -> 余额耗尽
//!   3. 正文含 40100 / 40101  -> 会话已死（只能重新登录）
//!   4. HTTP 429              -> 软限流
//!   5. HTTP 404              -> 上游偶发
//!   6. HTTP >= 500           -> 上游故障
//!   7. HTTP >= 400           -> 交共用内容拦截分类
//! ```
//! **为什么正文关键词要排在状态码之前（402 除外）**：实测上游用 **HTTP 400 +
//! 中文「积分不足」**表达余额耗尽（`errors.go:64-66`），只按状态码会把这类错误
//! 判成「可重试的客户端错误」，于是反复重试一个**永远不会成功**的账号。
//!
//! ── 映射到本仓的四档（这是本文件与 Go 唯一需要「翻译」的地方）──────
//! Go / TS 各有自己的类别名（hard-credit / soft-rate / session-dead / …），
//! 本仓的编排层只认四档（见 `adapter.rs` 模块头），所以：
//!   · **余额耗尽 → `QuotaLimited`**：它要做的事与 Go 的 `CoolHard` 逐字相同
//!     ——「标记冷却 + 换下一个账号」（另一个账号可能有额度）。归成 `Fatal`
//!     会把一个「该换号」的错误当成终态直接抛给用户。
//!   · **会话已死 → `TokenExpired`**：刷新后用同一账号重试一次；本家刷新被拒
//!     （40100 / 40101）时刷新本身会失败，编排层随即把它降级成可读错误。
//!   · **软限流 → `QuotaLimited`**（Go 的 `CoolSoft`，同款动作）。
//!   · **404 / 5xx → `Fatal`**（本仓没有「偶发 404 短冷却」这一档，如实透传）。
//!
//! ── 两种「HTTP 200 的假成功」────────────────────────────────
//!   1. **流内错误帧**：`HTTP 200` + SSE 正文是 `{"error":{"message":"…"}}`
//!      —— 额度耗尽正是以这个形态下发的（未实测，见下），换号循环若不认它，
//!      用户就会看到「一个账号用完出错、但池里还有账号也不切换」；
//!   2. **信封不变量**：`code:0` 配 `data:null` = 令牌已失效 —— 这一条**已经
//!      接上**（`oauth::unwrap_envelope` 把 `data:null` 判成 401），这里只把
//!      判据留一份可单测的纯函数。
//!
//! ── ⚠️ 第 1 条目前**没有调用点**（与 `atomcode/errors.rs` 同一处公开的缺口）
//! 「流内错误帧」要在 SSE 逐帧出口判，而本仓的逐帧出口
//! （`upstream::ForwardStream`）现在**只按适配器声明的 `finish_reason`** 识别
//! 拒答（`ProviderAdapter::stream_refusal_finishes`），没有「按帧内容嗅探」的
//! 挂点（理由见 `atomcode/errors.rs` 的模块头：那需要在通用层加一个每帧回调，
//! 而这一家能否用 `finish_reason` 表达额度耗尽**尚未实测**）。因此本函数是
//! **纯函数 + 测试**：判据先立住，等通用层给出挂点再接。它现在**不被任何生产
//! 代码调用**，这是已知缺口，不是遗漏。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::providers::adapter::UpstreamErrorClass;
use crate::server::core::providers::content_block;

/// 余额耗尽关键词（中英双通道**必需**）。
///
/// 中英都要，是因为同一后端在不同场景下会返回中文或英文（实测两种都出现过，
/// `errors.go:40-42`）。中英比较策略：整串先转小写再比，中文小写后与自己相等
/// （冗余但无害，与 TS `lobsterai-errors.ts:111-113` 一致）。
///
/// ⚠️ **与 Go 的一处差异（如实记下）**：Go 的 `hardCreditMarkers`
/// （`errors.go:43-48`）**少了**「额度已用完」「升级套餐」两条，而 TS 原版
/// （`lobsterai-errors.ts:54-66`）有，并在注里说明它们是根据**真实报障**补上的
/// （额度耗尽的**实际文案**就是「免费额度已用完，请升级套餐」，早期表里只有
/// 「额度用尽」——「已用完」与「用尽」字面不同，于是这个最主要的失败模式被判成
/// 不换号）。本实现取**并集**：漏判这一个模式的代价（账号用完不切换）比多两条
/// 关键词的代价大得多。英文同类文案（`free quota` 等）一并保留。
pub const HARD_CREDIT_MARKERS: &[&str] = &[
    "insufficient credit",
    "no credit",
    "credit exhausted",
    "out of credit",
    "quota exceeded",
    "quota exhaust",
    "payment required",
    "credit not enough",
    "not enough credit",
    "freecreditsused",
    "free credits used",
    "free quota",
    "quota used up",
    "upgrade your plan",
    "upgrade to continue",
    "积分不足",
    "额度不足",
    "余额不足",
    "积分用完",
    "额度用尽",
    "没有积分",
    "积分耗尽",
    "额度已用完",
    "升级套餐",
];

/// 会话已死标记（`errors.go:51-53`）：刷新被拒的业务码，终态、重试无意义。
pub const SESSION_DEAD_MARKERS: &[&str] = &[
    "40100",
    "40101",
    "token rejected",
    "refresh token was rejected",
];

/// 会话已死的两个业务码（数值形态，归一化层会把它们填进 `code`）
const SESSION_DEAD_CODES: [i64; 2] = [40100, 40101];

/// 正文（大小写不敏感）或原串里是否含余额耗尽关键词
pub fn mentions_hard_credit(text: &str) -> bool {
    markers_hit(text, HARD_CREDIT_MARKERS)
}

/// 正文里是否含会话已死标记（`40100` / `40101` 也是文本，所以按文本判）
pub fn mentions_session_dead(text: &str) -> bool {
    markers_hit(text, SESSION_DEAD_MARKERS)
}

/// 关键词命中（英文走小写比对、中文走原文比对，见 [`HARD_CREDIT_MARKERS`]）
fn markers_hit(text: &str, markers: &[&str]) -> bool {
    if text.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    markers.iter().any(|marker| {
        lower.contains(&marker.to_lowercase()) || text.contains(marker)
    })
}

/// 从归一化后的错误体里取一行可读文案。
///
/// ⚠️ 传到本模块的 `error_body` 是**传输层归一化之后**的形态
/// （`upstream::request::read_upstream_error`）：`code` 只在顶层且是数字时才有，
/// `message` 是 `message` / `msg` / `error.message` 里第一个非空的（原文最多
/// 截断到 500 字符）。所以这里读字符串是对的，别改成只读结构字段。
fn message_of(error_body: &Value) -> String {
    error_body
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("上游错误")
        .to_string()
}

/// 关键词匹配的**全部原料**：归一化文案 + 整个体的原文序列化。
///
/// 两处都要：`message` 是归一化后的可读串（英文关键词多半在这里），而有些形态
/// （如 `code:40100` 与中文文案同时出现）只在原文里看得全。体最大 500 字符量级，
/// 拼接成本可忽略。
fn haystack(error_body: &Value) -> String {
    format!("{}\n{}", message_of(error_body), error_body)
}

/// 从归一化体里取业务码（数字与数字字符串都认）。
///
/// 归一化层只填**数字**码；字符串码那类信封已经被折进 `message` 原文里，所以
/// 两种来源都覆盖（与 `atomcode/errors.rs::code_of` 同一手法）。
fn code_of(error_body: &Value) -> Option<i64> {
    if let Some(number) = error_body.get("code").and_then(Value::as_i64) {
        return Some(number);
    }
    error_body
        .get("code")
        .and_then(Value::as_str)
        .and_then(|text| text.trim().parse::<i64>().ok())
}

/// LobsterAI 上游错误的分类。
///
/// 判定顺序是契约（见模块头），别重排。
pub fn classify_gateway_error(status: u16, error_body: &Value) -> UpstreamErrorClass {
    let raw = message_of(error_body);
    let message = format!("上游返回 {status}: {raw}");
    let upstream_code = code_of(error_body);
    let text = haystack(error_body);

    // 1) 402：余额耗尽最直接的信号
    if status == 402 {
        return UpstreamErrorClass::QuotaLimited {
            reset_at: None,
            message: format!("LobsterAI 积分不足（402）：{raw}"),
            upstream_code,
            status,
        };
    }

    // 2) 正文关键词命中余额耗尽（**排在状态码之前**，见模块头）
    if mentions_hard_credit(&text) {
        return UpstreamErrorClass::QuotaLimited {
            reset_at: None,
            message: format!("LobsterAI 积分不足：{raw}"),
            upstream_code,
            status: 402,
        };
    }

    // 3) 会话已死：40100 / 40101（数值码或文案），只能重新登录
    if upstream_code.is_some_and(|code| SESSION_DEAD_CODES.contains(&code))
        || mentions_session_dead(&text)
    {
        return UpstreamErrorClass::TokenExpired {
            message: format!("LobsterAI 登录态已失效（{raw}），请重新登录"),
        };
    }

    // 4) 429 软限流
    if status == 429 {
        return UpstreamErrorClass::QuotaLimited {
            reset_at: None,
            message,
            upstream_code,
            status,
        };
    }

    // 5) 404 上游偶发（本仓没有「偶发短冷却」这一档，如实透传）
    if status == 404 {
        return UpstreamErrorClass::Fatal {
            status,
            message: format!("LobsterAI 上游返回 404：{raw}"),
            upstream_code,
        };
    }

    // 6) 5xx 上游故障
    if status >= 500 {
        return UpstreamErrorClass::Fatal {
            status,
            message,
            upstream_code,
        };
    }

    // 7) 其余 4xx / 业务错误：交共用内容拦截分类（审核文案 → ContentBlocked）
    content_block::classify_or_fatal(status, error_body, message, upstream_code)
}

/// 一次「流内错误帧」的识别结果（HTTP 200 下发的业务失败）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StreamSilentFailure {
    /// 帧里是余额耗尽
    HardCredit {
        /// 上游给的原文 message
        message: String,
    },
    /// 帧里是会话已死
    SessionDead {
        /// 上游给的原文 message
        message: String,
    },
    /// 帧里是别的业务错误（Go 的 default 分支：**也换号**）
    Other {
        /// 上游给的原文 message
        message: String,
    },
}

/// 识别**流内错误帧**：`HTTP 200` + SSE 正文是 `{"error":{"message":"…"}}`。
///
/// ── 为什么不能复用 [`classify_gateway_error`] ────────────────────
/// 那个函数的优先级里**状态码排在最前**，而流内错误的 HTTP 状态是 **200** ——
/// 那些分支全失效，只剩关键词表可用；传 200 进去未命中关键词时只能得到「成功」，
/// 而「成功」意味着**不换号**（TS `lobsterai-errors.ts:126-161` 专门说明了这点）。
/// 所以本函数只回答「帧里是不是一类业务失败」，默认值给
/// [`StreamSilentFailure::Other`]（Go 的 handler 里 default 分支同样 `continue`
/// 换号，防雪崩）。
///
/// ⚠️ **纯函数，当前没有调用点**（原因见模块头；`finish_reason` 表达额度耗尽
/// 尚未实测，本仓的逐帧出口也还没有「按内容嗅探」的挂点）。
pub fn silent_failure_in_frame(payload: &str) -> Option<StreamSilentFailure> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    // `{"error":{"message":…}}` 才是这个形态；`{"error":"…"}` 也认（宽容读取）
    let message = match parsed.get("error") {
        Some(Value::Object(error)) => error
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        Some(Value::String(text)) => text.trim().to_string(),
        _ => return None,
    };
    if mentions_hard_credit(&message) || mentions_hard_credit(trimmed) {
        return Some(StreamSilentFailure::HardCredit { message });
    }
    if mentions_session_dead(&message) || mentions_session_dead(trimmed) {
        return Some(StreamSilentFailure::SessionDead { message });
    }
    Some(StreamSilentFailure::Other { message })
}

/// 业务信封是否是「`code:0` + `data:null`」= **令牌已失效**的形态。
///
/// ⚠️ 这一条**已经接上**：`oauth::unwrap_envelope` 就把 `data` 为 null / 缺失
/// 判成 401「登录态可能已失效，请重新登录」，本函数只是把同一条判据做成可单测
/// 的纯函数（Go `envelope.go:26-76` 的第三个检查，注释称它是「载荷件那一条」）。
pub fn envelope_reports_dead_session(payload: &Value) -> bool {
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        return false;
    }
    matches!(payload.get("data"), None | Some(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn classify(status: u16, body: Value) -> UpstreamErrorClass {
        classify_gateway_error(status, &body)
    }

    /// **第一条优先级**：402 最权威，哪怕正文什么都没说。
    #[test]
    fn payment_required_wins_over_everything() {
        match classify(402, json!({"message": "whatever"})) {
            UpstreamErrorClass::QuotaLimited { status, message, .. } => {
                assert_eq!(status, 402);
                assert!(message.contains("积分不足"));
            }
            other => panic!("402 应为 QuotaLimited，实际 {other:?}"),
        }
    }

    /// **关键词先于状态码**：400 + 「积分不足」必须判成额度耗尽，而不是可重试的
    /// 客户端错误 —— 否则会反复重试一个永远不会成功的账号（模块头）。
    #[test]
    fn hard_credit_marker_beats_the_status_code() {
        match classify(400, json!({"code": 1, "msg": "积分不足"})) {
            UpstreamErrorClass::QuotaLimited { status, .. } => assert_eq!(status, 402),
            other => panic!("应判额度耗尽，实际 {other:?}"),
        }
        // 中文实际文案（Go 的表少这两条，本实现取并集，见 HARD_CREDIT_MARKERS）
        match classify(400, json!({"message": "免费额度已用完，请升级套餐"})) {
            UpstreamErrorClass::QuotaLimited { .. } => {}
            other => panic!("「额度已用完」应判额度耗尽，实际 {other:?}"),
        }
        // 英文通道
        assert!(mentions_hard_credit("Insufficient Credit"));
        assert!(mentions_hard_credit("QUOTA EXCEEDED"));
        // 正常回答里提到这些词不该命中（这里判的是错误体，不是模型正文）
        assert!(!mentions_hard_credit("参数错误"));
        assert!(!mentions_hard_credit("hello world"));
    }

    /// 会话已死：数值码 `40100` 与文本标记两条路都要中，且排在 429 之前。
    #[test]
    fn session_dead_is_detected_and_outranks_the_rate_limit_branch() {
        // 数值码形态（归一化层填的是数字 code）
        match classify(401, json!({"code": 40100, "message": "refresh rejected"})) {
            UpstreamErrorClass::TokenExpired { message } => {
                assert!(message.contains("重新登录"))
            }
            other => panic!("40100 应为 TokenExpired，实际 {other:?}"),
        }
        // 文本形态 + 与 429 同时出现时仍判会话已死（重试无意义）
        match classify(
            429,
            json!({"message": "40101 refresh token was rejected"}),
        ) {
            UpstreamErrorClass::TokenExpired { .. } => {}
            other => panic!("40101 应优先于 429，实际 {other:?}"),
        }
        assert!(!mentions_session_dead("普通错误"));
    }

    /// 429 → 软限流（带账号冷却语义，无结构化恢复时间）。
    #[test]
    fn rate_limit_maps_to_quota_limited() {
        match classify(429, json!({"message": "too many requests"})) {
            UpstreamErrorClass::QuotaLimited {
                reset_at, status, ..
            } => {
                assert_eq!(status, 429);
                assert!(reset_at.is_none());
            }
            other => panic!("429 应为 QuotaLimited，实际 {other:?}"),
        }
    }

    /// 404 / 5xx → Fatal（原样透传，无换号动作）。
    #[test]
    fn not_found_and_server_errors_are_fatal() {
        match classify(404, json!({"message": "not found"})) {
            UpstreamErrorClass::Fatal { status, .. } => assert_eq!(status, 404),
            other => panic!("404 应为 Fatal，实际 {other:?}"),
        }
        match classify(503, json!({"message": "bad gateway"})) {
            UpstreamErrorClass::Fatal { status, .. } => assert_eq!(status, 503),
            other => panic!("5xx 应为 Fatal，实际 {other:?}"),
        }
    }

    /// 其余 4xx 交共用内容拦截分类（这里只验证「不是我们擅自归的档」）。
    #[test]
    fn other_client_errors_go_to_the_shared_classifier() {
        let class = classify(400, json!({"code": 1001, "message": "bad request"}));
        match class {
            UpstreamErrorClass::Fatal { .. } | UpstreamErrorClass::ContentBlocked { .. } => {}
            UpstreamErrorClass::QuotaLimited { .. } => panic!("400 不该判限额"),
            other => panic!("意外分类 {other:?}"),
        }
    }

    /// **流内错误帧**：`{"error":{"message":…}}` 要能被认出来并分类。
    #[test]
    fn silent_stream_error_frames_are_classified() {
        assert_eq!(
            silent_failure_in_frame(r#"{"error":{"message":"免费额度已用完，请升级套餐"}}"#),
            Some(StreamSilentFailure::HardCredit {
                message: "免费额度已用完，请升级套餐".to_string()
            })
        );
        assert_eq!(
            silent_failure_in_frame(r#"{"error":{"message":"40100 token rejected"}}"#),
            Some(StreamSilentFailure::SessionDead {
                message: "40100 token rejected".to_string()
            })
        );
        // 别的业务错误：默认给 Other（Go 的 default 分支同样换号）
        assert_eq!(
            silent_failure_in_frame(r#"{"error":{"message":"some business failure"}}"#),
            Some(StreamSilentFailure::Other {
                message: "some business failure".to_string()
            })
        );
        // 正常帧 / 非 JSON / 空 → None（不误判）
        assert_eq!(silent_failure_in_frame(r#"{"choices":[{"delta":{"content":"hi"}}]}"#), None);
        assert_eq!(silent_failure_in_frame("data: [DONE]"), None);
        assert_eq!(silent_failure_in_frame("   "), None);
    }

    /// 信封不变量：`code:0` + `data:null` = 令牌失效（已被 `oauth::unwrap_envelope`
    /// 接上，这里只验证判据本身）。
    #[test]
    fn envelope_with_null_data_reports_a_dead_session() {
        assert!(envelope_reports_dead_session(&json!({"code": 0, "data": null})));
        assert!(envelope_reports_dead_session(&json!({"code": 0})));
        assert!(!envelope_reports_dead_session(&json!({"code": 0, "data": {}})));
        assert!(!envelope_reports_dead_session(&json!({"code": 7, "data": null})));
    }
}
