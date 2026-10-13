//! WorkBuddy 国际版的每日活跃保活请求构造。
//!
//! ── 这是什么 ──────────────────────────────────────────────
//! 国际版有一个「每日活跃奖励」：活动开放时领一份奖励，并且要求账号当天有
//! 一次模型调用（保持活跃）。参考客户端把「活动探测 → 条件领取 → 免费模型
//! 最小对话」接到既有签到调度器上；本仓的对应编排在
//! `billing::activity` 的 `workbuddy_daily_activity`，本文件只负责**把一发
//! 保活对话的请求素材拼出来**（URL / 头 / 体 / 代理）。
//!
//! ── 为什么保活要和真对话共用一套构造 ──────────────────────
//! 保活的本质是"让上游把这次调用记成一次正常使用"，所以它必须长得和
//! 真对话一模一样：头集合与 URL 直接复用 `adapter::chat_headers` /
//! `adapter::chat_completions_url`（两套实现必然漂移，漂移的后果是被
//! 客户端画像识别拦下）。这里只固定参考客户端使用的**最小流式 body**，
//! 并补齐国际版浏览器画像的会话头。
//!
//! ── 模型链 ────────────────────────────────────────────────
//! [`DEFAULT_FREE_MODELS`] 是参考客户端实测不消耗账号积分的免费模型，按
//! 可用性优先级排列；运行时真正使用的模型链可在签到中心自定义
//! （`billing::keepalive` 读配置），列表为空或全失败时逐个回退到下一个。

use serde_json::{Value, json};

use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::request::TransportRequest;

use super::Region;
use super::adapter::{chat_completions_url, chat_headers};

/// 国际版每日活跃任务默认使用的免费模型，按可用性优先级尝试。
///
/// 这些模型在参考客户端中实测为 x0.00，不会消耗账号积分；它是**缺省值**
/// （配置里没写 `checkinKeepalive.models` 时用它），用户自定义后的链路
/// 存在 config.json，读写见 `billing::keepalive`。列表保持集中，
/// 便于上游模型调整时只改一处。
pub const DEFAULT_FREE_MODELS: &[&str] =
    &["deepseek-v4.1-flash", "hy3", "hy4-preview", "hy4-preview-f"];

/// 构造国际版每日活跃保活请求。
///
/// 活跃任务必须沿用 WorkBuddy 对话链路的端点、鉴权头和账号代理；这里只固定
/// 参考客户端使用的最小流式 body（16 个 token 上限的一次 "hi"）。调用方
/// （`billing::activity::poke_daily_activity`）负责通过传输层发送并消费响应体。
pub fn build_daily_activity_request(
    session: &Value,
    model: &str,
) -> Result<TransportRequest, String> {
    let body = json!({
        "model": model,
        "stream": true,
        "max_tokens": 16,
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "hi"},
        ],
    });
    let payload =
        serde_json::to_string(&body).map_err(|error| format!("活跃请求序列化失败: {error}"))?;
    let request_id = crate::server::core::upstream::request::new_request_id();
    let mut headers = chat_headers(session, Region::Intl, &request_id, Some("text/event-stream"));
    // 国际版参考客户端使用 conversation/purpose 头族；保留既有 WorkBuddy
    // 头集合并补齐这些浏览器/会话字段，国内版请求形态不受影响（国内版不进
    // 这条链，`workbuddy_daily_activity` 只服务国际版会话）。
    headers.retain(|(key, _)| key != "X-Agent-Intent");
    headers.push(("X-Agent-Purpose".to_string(), "conversation".to_string()));
    headers.push(("X-Conversation-Message-ID".to_string(), request_id.clone()));
    headers.push(("X-Root-Request-ID".to_string(), request_id.clone()));
    headers.push(("Origin".to_string(), "https://www.workbuddy.ai".to_string()));
    headers.push(("Referer".to_string(), "https://www.workbuddy.ai/".to_string()));
    headers.push(("X-Requested-With".to_string(), "XMLHttpRequest".to_string()));
    let proxy = match ResolvedProxy::from_json(session.get("proxy").unwrap_or(&Value::Null)) {
        Ok(proxy) => proxy,
        Err(reason) => {
            crate::server::logging::log(
                "[Upstream]",
                &format!("⚠️ 账号代理不可用（{reason}），活跃请求回退直连"),
            );
            None
        }
    };
    Ok(TransportRequest {
        url: chat_completions_url(session, Region::Intl),
        headers,
        payload,
        proxy,
        // 保活是转发链路的旁支，口径跟转发一致（workbuddy 不覆写能力位 = 直连）
        system_proxy_when_unset: false,
    })
}
