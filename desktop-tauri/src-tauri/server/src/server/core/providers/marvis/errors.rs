//! Marvis 上游错误分类。
//!
//! ── 实测到的码表（2026-10-20，逐条打真实上游）────────────────
//! ```text
//!   401 {"code":4100403,"msg":"客户端通用错误:登录态过期/未登录"}
//!       ← token 缺失/失效
//!   403 {"code":4100404,"msg":"客户端通用错误:请求被风控拦截"}
//!       ← uskey 缺失 / 请求节奏过密
//! ```
//!
//! ── 四档怎么归（这是本家最容易判错的一档）────────────────────
//!   * `4100403`（登录态过期）→ **`TokenExpired`**：换令牌后同账号重试。
//!     注意本家 `supports_refresh = false`（上游没有刷新端点），所以这一档
//!     实际表现为「告诉用户重新提取 token」，而不是自动续期。**不能归
//!     `Fatal`** —— 那样用户看到的是原始 401，而本仓的 `TokenExpired`
//!     路径会给出「凭证过期」的可读提示。
//!   * `4100404`（风控）→ **`QuotaLimited`**：这是本家最反直觉的一条。
//!     它**不是**内容审核（不换提示词），也**不是**永久失败 —— 参考实现的
//!     FINDINGS 明确写「账号级自适应风控；密集突发 → 403，会冷却」。
//!     归 `QuotaLimited` 能让编排层冷却该账号并换下一个号，正是要的行为。
//!   * 429 / `rate limit` / `too many requests` → `QuotaLimited`（同上）。
//!   * 内容审核（`content_filter` / 敏感词类）→ `ContentBlocked`：
//!     不罚账号，换中性提示词重试一次。判据用上游文案里的关键词，
//!     本家实测没触发过，取保守集合（宁可漏判成 Fatal，也不误罚账号）。
//!   * 其余 → `Fatal`（原样透传）。
//!
//! ── 为什么 401/403 不能只看 HTTP 码 ─────────────────────────
//! 上游把两种完全不同的情况都塞进 401/403：登录态过期 vs 风控。
//! 只看状态码会把「换号就能好」的风控当永久失败，也会把「必须重新提取
//! token」的过期当临时问题反复重试。所以判据是**正文里的 `code` 字段**
//! （`4100403` / `4100404`），不是 HTTP 状态码。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::providers::adapter::UpstreamErrorClass;

/// 登录态过期（token 失效）。
const CODE_TOKEN_EXPIRED: &str = "4100403";

/// 风控拦截（uskey 缺失 / 请求过密）。
const CODE_RISK_BLOCKED: &str = "4100404";

/// 内容审核类关键词（保守集合：宁可漏判成 Fatal，也不误罚账号）。
const CONTENT_MARKERS: [&str; 4] = [
    "content_filter",
    "content filter",
    "内容审核",
    "违反",
];

/// 限流类关键词（文案兜底：有些网关只在 msg 里写）。
const RATE_MARKERS: [&str; 4] = [
    "rate limit",
    "too many requests",
    "请求过于频繁",
    "限流",
];

/// 从正文里取业务码（`{"code":4100403}` 里的整数，或字符串形态）。
fn upstream_code(body: &Value) -> Option<i64> {
    body.get("code").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

/// 客户端可见的完整文案：`上游返回 {status}: {上游原文}`（与别家同格式）。
fn message_of(status: u16, body: &Value) -> String {
    format!("上游返回 {status}: {body}")
}

/// 按上游响应归类。
pub fn classify(status: u16, body: &Value) -> UpstreamErrorClass {
    let text = body.to_string();
    let lower = text.to_ascii_lowercase();
    let code = upstream_code(body);

    // ── 先按业务码判（比 HTTP 状态码精确，见模块头）──────────────
    if lower.contains(CODE_TOKEN_EXPIRED) {
        return UpstreamErrorClass::TokenExpired {
            message: message_of(status, body),
        };
    }
    if lower.contains(CODE_RISK_BLOCKED) {
        // 风控是账号级、会冷却 —— 换号能好，归 QuotaLimited。
        // `reset_at: None` 让调用方走兜底冷却时长（上游不给恢复时间）。
        return UpstreamErrorClass::QuotaLimited {
            reset_at: None,
            message: message_of(status, body),
            upstream_code: code,
            status,
        };
    }

    // ── 内容审核（不罚账号）──────────────────────────────────
    if CONTENT_MARKERS
        .iter()
        .any(|marker| lower.contains(&marker.to_ascii_lowercase()))
    {
        return UpstreamErrorClass::ContentBlocked {
            status,
            message: message_of(status, body),
            upstream_code: code,
        };
    }

    // ── 限流 / 配额（冷却账号）───────────────────────────────
    if status == 429
        || RATE_MARKERS
            .iter()
            .any(|marker| lower.contains(&marker.to_ascii_lowercase()))
    {
        return UpstreamErrorClass::QuotaLimited {
            reset_at: None,
            message: message_of(status, body),
            upstream_code: code,
            status,
        };
    }

    UpstreamErrorClass::Fatal {
        status,
        message: message_of(status, body),
        upstream_code: code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn login_expired_is_token_expired() {
        let body = json!({"code": 4100403, "msg": "客户端通用错误:登录态过期/未登录"});
        assert!(matches!(
            classify(401, &body),
            UpstreamErrorClass::TokenExpired { .. }
        ));
    }

    #[test]
    fn risk_blocked_is_quota_limited() {
        // 风控 ≠ 内容审核：它会冷却、换号能好（见模块头）
        let body = json!({"code": 4100404, "msg": "客户端通用错误:请求被风控拦截"});
        match classify(403, &body) {
            UpstreamErrorClass::QuotaLimited {
                upstream_code,
                status,
                ..
            } => {
                assert_eq!(upstream_code, Some(4100404));
                assert_eq!(status, 403);
            }
            other => panic!("风控应归 QuotaLimited，实际 {other:?}"),
        }
    }

    #[test]
    fn rate_limit_is_quota_limited() {
        let body = json!({"error": {"message": "Rate limit exceeded"}});
        assert!(matches!(
            classify(429, &body),
            UpstreamErrorClass::QuotaLimited { .. }
        ));
    }

    #[test]
    fn content_filter_does_not_punish_account() {
        let body = json!({"error": {"code": "content_filter"}});
        assert!(matches!(
            classify(400, &body),
            UpstreamErrorClass::ContentBlocked { .. }
        ));
    }

    #[test]
    fn unknown_becomes_fatal() {
        let body = json!({"error": "boom"});
        assert!(matches!(
            classify(500, &body),
            UpstreamErrorClass::Fatal { .. }
        ));
    }

    #[test]
    fn token_expired_wins_over_generic_401() {
        // 401 本身不足以判过期：正文码才是判据
        let generic = json!({"error": "unauthorized"});
        assert!(matches!(
            classify(401, &generic),
            UpstreamErrorClass::Fatal { .. }
        ));
    }
}
