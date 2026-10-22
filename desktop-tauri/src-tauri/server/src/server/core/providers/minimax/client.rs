//! MiniMax Code 的出站请求：**Anthropic Messages 协议** + Bearer 鉴权。
//!
//! ── 为什么不是 OpenAI 兼容 ──────────────────────────────────
//! 客户端的 provider 声明是 `npm: '@ai-sdk/anthropic'`，端点
//! `POST /mavis/api/v1/llm/v1/messages` 就是标准 Anthropic Messages。
//! 因此本家的请求体要**从 chat 形态转成 Anthropic 形态**，响应帧要**折回 chat SSE**
//! —— 两侧都走既有的翻译层（`protocol::anthropic_outbound`），不自己写一套：
//!   * 请求：`anthropic_request_from_chat`（system 提到顶层、tool 消息变
//!     `tool_result` 块、`tool_calls` 变 `tool_use` 块）；
//!   * 响应：`UpstreamResponse::Anthropic`（编排层据此选 `AnthropicToChatStream`）。
//! 走翻译层的理由与 ZCode 活动套餐通道逐字相同：账号轮换、限额冷却、退避重试、
//! usage 记账与取消处理全部留在编排层，只有字节形态在翻译器里变。
//!
//! ── 两个网关门禁（实测 2026-10-21，`_recon/minimax-code-contract-*.md` §2）──
//! ```text
//!   无 Authorization            → 401（空体）
//!   Bearer <任意假令牌>          → 401 {"error":"invalid access token"}
//!   带查询串（设备参数）         → 同上（**签名头与查询串都不影响判定**）
//! ```
//! ⇒ 推理链只需要 `Authorization: Bearer`，不需要 `endpoints::sign`。
//!
//! ── 强制推理（会咬人的一处）────────────────────────────────
//! 本家的模型**都要求思考**：发 `thinking:{"type":"disabled"}` 会被上游
//! `400 … requires adaptive thinking` 拒掉（`dsh-connect-minimaxcode` README §五
//! 第 3 条，实测）。所以出站**不注入** `disabled`：客户端没要点思考时就让
//! `anthropic_request_from_chat` 的默认行为走（不写 `thinking` 键），
//! 由上游按模型自身的 `thinking_config`（M3 = switchable / M3.1-Flash = forced_on）决定。
//! 「客户端想要更多思考」正常透传（`reasoning_effort` → `thinking.budget_tokens`
//! 由转换层负责），「客户端明确关闭」这一档在这里被吞掉 —— 那是「上游做不到」，
//! 不是「我们没实现」。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::protocol::anthropic_outbound;
use crate::server::errors::GatewayError;

use super::credentials::MinimaxCredential;
use super::endpoints;
use super::region::Region;

/// Anthropic 协议版本头（与 ZCode 通道同值）。
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 一次转发请求的成品。
#[derive(Clone, Debug)]
pub struct ChatPlan {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

/// 组装一次转发请求。
///
/// `body` 是客户端发来的 chat 请求体；本函数负责把它翻成 Anthropic Messages。
/// 模型名缺省时不猜（上游没有「默认模型」概念，猜一个会让客户端的意图被悄悄改写）——
/// 留给 `api::pipeline` 的模型解析去兜。
///
/// `region` 决定打哪个网关（国内 `agent.minimax.cn` / 国际 `agent.minimaxi.com`）：
/// 两个地区的**请求形状与鉴权完全相同**，差的只有域名 —— 所以这里只是把基址
/// 换成 `region.gateway_url()`，别处（body 转换、头、常量）一律不分叉。
pub fn build_chat_plan(
    region: Region,
    credential: &MinimaxCredential,
    body: &Value,
) -> Result<ChatPlan, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if model.is_empty() {
        return Err(GatewayError::with_status(
            400i32,
            "MiniMax Code 请求缺少模型名",
        ));
    }

    let mut payload = anthropic_outbound::anthropic_request_from_chat(body, model)
        .map_err(|message| GatewayError::with_status(400i32, format!("请求体转换失败：{message}")))?;
    // 强制推理：删掉客户端可能的「关闭思考」表达（见模块头）。
    // 只删 `disabled`，不删 `enabled` / `adaptive` 与预算。
    if let Some(object) = payload.as_object_mut() {
        let disabled = object
            .get("thinking")
            .and_then(|thinking| thinking.get("type"))
            .and_then(Value::as_str)
            .map(|kind| kind.eq_ignore_ascii_case("disabled"))
            .unwrap_or(false);
        if disabled {
            object.remove("thinking");
        }
    }

    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        (
            "Authorization".to_string(),
            format!("Bearer {}", credential.access_token),
        ),
        ("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string()),
    ];

    Ok(ChatPlan {
        url: endpoints::gateway_url(region, endpoints::MESSAGES_PATH),
        headers,
        body: payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn credential() -> MinimaxCredential {
        MinimaxCredential {
            id: "minimax-1".into(),
            access_token: "mmoat_test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn builds_an_anthropic_request_with_bearer() {
        let body = json!({
            "model": "MiniMax-M3.1-Flash",
            "max_tokens": 64,
            "messages": [
                {"role": "system", "content": "你是助手"},
                {"role": "user", "content": "hi"}
            ]
        });
        let plan = build_chat_plan(Region::Cn, &credential(), &body).expect("应能构造");
        assert_eq!(
            plan.url,
            "https://agent.minimax.cn/mavis/api/v1/llm/v1/messages"
        );
        // 鉴权：只认 Bearer（x-api-key 会被 401 拒，见模块头）
        let auth = plan
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        assert_eq!(auth, "Bearer mmoat_test");
        assert!(
            !plan.headers.iter().any(|(key, _)| key.eq_ignore_ascii_case("x-api-key")),
            "不能带 x-api-key"
        );
        // 系统消息提到顶层（Anthropic 协议要求）
        assert_eq!(plan.body["system"], json!("你是助手"));
        assert_eq!(plan.body["model"], json!("MiniMax-M3.1-Flash"));
        assert_eq!(plan.body["messages"][0]["role"], json!("user"));
    }

    #[test]
    fn thinking_disabled_is_dropped() {
        // 上游对 disabled 回 400 requires adaptive thinking（见模块头）
        let body = json!({
            "model": "MiniMax-M3",
            "max_tokens": 32,
            "thinking": {"type": "disabled"},
            "messages": [{"role": "user", "content": "hi"}]
        });
        let plan = build_chat_plan(Region::Cn, &credential(), &body).expect("应能构造");
        assert!(
            plan.body.get("thinking").is_none(),
            "关闭思考的表达必须被删掉，否则上游 400"
        );
    }

    /// **地区必须落在 URL 上**：这是拆家后唯一真正分叉的地方 —— 请求体、头、
    /// 转换层两地逐字相同。国际版打到 `agent.minimax.io`（**不是** `.minimaxi.com`
    /// —— 后者只认国内令牌，见 `region.rs` 的 `gateway_url` 实测表）。
    #[test]
    fn the_region_decides_the_gateway() {
        let body = json!({
            "model": "MiniMax-M3.1-Flash",
            "max_tokens": 8,
            "messages": [{"role": "user", "content": "hi"}]
        });
        let intl = build_chat_plan(Region::Intl, &credential(), &body).expect("应能构造");
        assert_eq!(intl.url, "https://agent.minimax.io/mavis/api/v1/llm/v1/messages");
        assert!(
            !intl.url.contains("minimax.cn"),
            "国际版绝不能打到国内网关：{}",
            intl.url
        );
        // 除 URL 之外两地必须一字不差 —— 有别的差异就说明分叉错了
        let cn = build_chat_plan(Region::Cn, &credential(), &body).expect("应能构造");
        assert_eq!(cn.headers, intl.headers, "鉴权头两地一致（同一个 client_id）");
        assert_eq!(cn.body, intl.body, "转换后的请求体两地一致");
    }

    #[test]
    fn missing_model_is_rejected_locally() {
        let body = json!({"messages": [{"role": "user", "content": "hi"}]});
        let error = build_chat_plan(Region::Cn, &credential(), &body).expect_err("应被拒");
        assert!(error.to_string().contains("模型名"), "实际：{error}");
    }
}
