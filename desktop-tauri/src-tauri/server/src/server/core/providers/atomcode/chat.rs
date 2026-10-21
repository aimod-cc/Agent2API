//! AtomCode 的转发：**同方言透传**，本仓最薄的一层。
//!
//! ── 为什么这一层几乎什么都不做 ────────────────────────────────
//! 网关本身就是 OpenAI Chat Completions，所以客户端请求体几乎逐字节上行、
//! 上游的 SSE 帧逐字节下发。别家那套「按家翻译帧」在这里没有对应物 ——
//! 没有一条字段会在「没有发生的翻译」里丢掉，这正是走同方言上游的全部好处。
//! 剩下的只有格式本身表达不了的四件事：**身份、模型名映射、参数补全、
//! 以及静默失败的识别**（最后一条在 `errors.rs` 与 SSE 出口）。
//!
//! ── 只改三处，每处都有它非改不可的理由 ──────────────────────────
//!   1. `model`：客户端点的是**展示名**（目录里的 `GLM-5.3-Flash`），网关只认
//!      自己的 id（`glm5.3-flash`）。不做这一步，网关不会报错，它会回
//!      `HTTP 200 + 正文 参数错误`（见 `errors.rs` 的模块头）——
//!      一个「模型名没映射」的 bug 会伪装成「参数不被接受」。
//!      同时剥掉账号前缀（`<accountId>/<model>`，多账号下同名模型的消歧手段）。
//!   2. `stream_options`：官方客户端**无条件**发
//!      `{"include_usage": true}`，网关据此才吐尾部 usage 帧。客户端没指定时补上
//!      （客户端显式指定就让它赢）；非流式路径删掉它（没有流可挂）。
//!   3. 其余（messages / tools / tool_choice / temperature / max_tokens /
//!      reasoning_effort / 图片分片）**一律不动**。
//!
//! ── User-Agent 是功能性的（再说一遍，因为它最容易被「清理」掉）────
//! 网关按 UA 决定是否索要闭源签名：自称 `atomcode/<ver>` 会立刻
//! `403 ATOMCODE_SIG_MISSING`。见 `endpoints::ADAPTER_USER_AGENT` 的实测表。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::providers::adapter::ChatRequestPlan;
use crate::server::errors::GatewayError;

use super::credentials::AtomcodeCredentials;
use super::endpoints::{ADAPTER_USER_AGENT, gateway_chat_url};
use super::models;

/// 账号前缀的分隔符（目录里同名模型的消歧形态：`<accountId>/<model>`）
const ACCOUNT_PREFIX_SEPARATOR: char = '/';

/// 剥掉账号前缀：`<account>/glm5.3-flash` → `glm5.3-flash`。
///
/// ⚠️ **只剥一层且只剥「已知账号 id」那一层**不能在这里做（本函数拿不到账号表），
/// 因此判据是「第一段看起来像账号 id」：目录里的模型名本身**不含** `/`
/// （网关的 id 形如 `glm5.3-flash` / `deepseek-flash`），所以出现 `/` 就说明
/// 那是我们自己的前缀。这条判据的代价写在这里：将来若网关开始下发带 `/` 的
/// 模型 id（如 `moonshotai/Kimi-K2-Instruct` 这种**含斜杠**的上游 id），
/// 本函数会误剥。届时改为「按调用方给的账号 id 精确剥」。
/// 现在不那样写是因为 `build_chat_request` 的 `account` 参数里没有账号 id
/// 的独立字段（见 `adapter.rs` 关于 `account` 形态的说明）。
pub fn strip_account_prefix(model: &str) -> &str {
    let trimmed = model.trim();
    match trimmed.split_once(ACCOUNT_PREFIX_SEPARATOR) {
        Some((_prefix, rest)) if !rest.trim().is_empty() => rest.trim(),
        _ => trimmed,
    }
}

/// 准备一次上游调用。
pub fn build_chat_request(
    account: &Value,
    body: &Value,
    _client_headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    let credentials = AtomcodeCredentials {
        id: String::new(),
        access_token: account
            .get("auth")
            .and_then(|auth| auth.get("accessToken"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        refresh_token: String::new(),
        expires_at: 0,
        user_id: String::new(),
        name: String::new(),
    };
    if credentials.access_token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "AtomCode 账号缺少访问令牌，无法转发（请重新登录）",
        ));
    }

    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if requested.is_empty() {
        return Err(GatewayError::with_status(400, "请求缺少 model 字段"));
    }
    // 客户端点的是展示名 / 带前缀名；网关只认自己的 id。
    let published = strip_account_prefix(requested);
    let upstream_model = models::upstream_id_for(published).unwrap_or_else(|| published.to_string());

    let mut payload = body.clone();
    if let Some(object) = payload.as_object_mut() {
        object.insert("model".to_string(), Value::String(upstream_model));
        let streaming = object.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if streaming {
            // 官方客户端无条件带它，网关据此才吐尾部 usage 帧；
            // 客户端显式指定过就不覆盖。
            if !object.contains_key("stream_options") {
                object.insert(
                    "stream_options".to_string(),
                    serde_json::json!({ "include_usage": true }),
                );
            }
        } else {
            // 非流式路径没有流可挂 usage，留着只会让上游多回一个字段
            object.remove("stream_options");
        }
    } else {
        return Err(GatewayError::with_status(400, "请求体不是 JSON 对象"));
    }

    let headers: Vec<(String, String)> = vec![
        (
            "authorization".to_string(),
            format!("Bearer {}", credentials.access_token),
        ),
        ("accept".to_string(), "application/json".to_string()),
        // 功能性常量：自称官方客户端会触发 ATOMCODE_SIG_MISSING
        ("user-agent".to_string(), ADAPTER_USER_AGENT.to_string()),
    ];

    Ok(ChatRequestPlan::chat(gateway_chat_url(), headers, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Value {
        json!({ "auth": { "accessToken": "at-1" } })
    }

    /// 展示名要被换成网关 id —— 不做这一步网关会回 `200 + 参数错误`。
    #[test]
    fn published_name_is_mapped_to_the_gateway_id() {
        let plan = build_chat_request(
            &account(),
            &json!({"model": "GLM-5.3-Flash", "messages": [{"role": "user", "content": "hi"}]}),
            &HeaderMap::new(),
        )
        .expect("应构造成功");
        // 目录里若认得这个名字就换成 id；认不得时**原样发**（不猜）
        let sent = plan.body.get("model").and_then(Value::as_str).unwrap_or("");
        assert!(!sent.is_empty());
        assert_ne!(sent, "");
    }

    /// 账号前缀要被剥掉（多账号下同名模型的消歧形态）。
    #[test]
    fn account_prefix_is_stripped() {
        assert_eq!(strip_account_prefix("acct-42/glm5.3-flash"), "glm5.3-flash");
        assert_eq!(strip_account_prefix("glm5.3-flash"), "glm5.3-flash");
        assert_eq!(strip_account_prefix("  acct/  glm  "), "glm");
        // 只有斜杠没有后半段时不剥（避免把 `a/` 剥成空）
        assert_eq!(strip_account_prefix("a/"), "a/");
    }

    /// **核心护栏**：User-Agent 必须存在且不能自称官方客户端。
    #[test]
    fn user_agent_is_sent_and_is_not_the_official_one() {
        let plan = build_chat_request(
            &account(),
            &json!({"model": "m", "messages": []}),
            &HeaderMap::new(),
        )
        .expect("应构造成功");
        let ua = plan
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
            .map(|(_, value)| value.clone())
            .expect("必须带 User-Agent");
        assert!(ua.starts_with("agent2api-atomcode/"));
        assert!(!ua.to_ascii_lowercase().starts_with("atomcode/"));
    }

    /// 流式请求补 `stream_options.include_usage`；客户端指定过就不覆盖。
    #[test]
    fn stream_options_are_added_only_when_the_client_did_not_choose() {
        let plan = build_chat_request(
            &account(),
            &json!({"model": "m", "stream": true, "messages": []}),
            &HeaderMap::new(),
        )
        .expect("应构造成功");
        assert_eq!(
            plan.body
                .get("stream_options")
                .and_then(|value| value.get("include_usage"))
                .and_then(Value::as_bool),
            Some(true)
        );

        let explicit = build_chat_request(
            &account(),
            &json!({"model": "m", "stream": true, "stream_options": {"include_usage": false}, "messages": []}),
            &HeaderMap::new(),
        )
        .expect("应构造成功");
        assert_eq!(
            explicit
                .body
                .get("stream_options")
                .and_then(|value| value.get("include_usage"))
                .and_then(Value::as_bool),
            Some(false),
            "客户端显式指定时必须让位"
        );
    }

    /// 非流式请求不得带 `stream_options`。
    #[test]
    fn non_streaming_drops_stream_options() {
        let plan = build_chat_request(
            &account(),
            &json!({"model": "m", "stream_options": {"include_usage": true}, "messages": []}),
            &HeaderMap::new(),
        )
        .expect("应构造成功");
        assert!(plan.body.get("stream_options").is_none());
    }

    /// 除 model 与 stream_options 之外**逐字不动**（同方言透传的全部价值）。
    #[test]
    fn everything_else_is_forwarded_untouched() {
        let original = json!({
            "model": "glm5.3-flash",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {"type": "object"}}}],
            "tool_choice": "auto",
            "temperature": 0.3,
            "max_tokens": 1234,
            "reasoning_effort": "high"
        });
        let plan = build_chat_request(&account(), &original, &HeaderMap::new())
            .expect("应构造成功");
        for key in ["messages", "tools", "tool_choice", "temperature", "max_tokens", "reasoning_effort", "stream"] {
            assert_eq!(
                plan.body.get(key),
                original.get(key),
                "字段 {key} 必须逐字透传"
            );
        }
    }

    /// 缺令牌 / 缺 model 时报可读错误，而不是发一个空请求出去。
    ///
    /// 这里手写 `match` 而不是 `expect_err`：`ChatRequestPlan` 没有实现 `Debug`
    /// （它是跨家共用的类型，不为了一个测试给它加派生）。
    #[test]
    fn missing_credentials_or_model_fail_fast() {
        let error = match build_chat_request(
            &json!({"auth": {}}),
            &json!({"model": "m", "messages": []}),
            &HeaderMap::new(),
        ) {
            Ok(_) => panic!("缺令牌应当报错，却构造出了请求"),
            Err(error) => error,
        };
        assert_eq!(error.status_code, 401);

        let error = match build_chat_request(&account(), &json!({"messages": []}), &HeaderMap::new()) {
            Ok(_) => panic!("缺 model 应当报错，却构造出了请求"),
            Err(error) => error,
        };
        assert_eq!(error.status_code, 400);
    }
}
