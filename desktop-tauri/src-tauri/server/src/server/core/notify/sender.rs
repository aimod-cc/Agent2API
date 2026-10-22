//! 投递层：把一条事件按各渠道的**消息体形状**发出去。
//!
//! ── 三家形状的差异（这是本文件存在的全部理由）───────────────
//!   · 飞书：`{"msgtype":"text","content":{"text":"…"}}`
//!   · 企业微信：`{"msgtype":"text","content":{"content":"…"}}`
//!     —— 注意键名是 `content` 而不是 `text`，写错会被对方以 400 拒绝
//!   · 钉钉：`{"msgtype":"text","text":{"content":"…"}}`
//!     —— 顶层键是 `text` 而不是 `content`（与飞书**正好错位**）
//!   · 通用：直接 POST 纯文本（自建接收端 / 别家的机器人）
//!
//! 三个键名两两不同、且都是「另一个的」常见写法 —— 这是把差异集中到
//! [`payload_for`] 一处的原因，也是它值得一个独立测试的原因。
//!
//! ── 超时与失败 ──────────────────────────────────────────────
//! 10 秒硬超时：通知不该拖着一个后台任务不放（虽然走 spawn，但长期挂着的
//! 连接会积压）。失败只记日志、**不重试**（理由见模块头）。

use std::time::Duration;

use serde_json::{json, Value};

use super::config::{NotifyChannelConfig, NotifyConfig};
use super::event::{is_subscribed, NotifyEvent};

/// 单次投递超时
const TIMEOUT: Duration = Duration::from_secs(10);

/// 把一条事件发给全部已配置渠道（去重与开关判断已在上层做过）。
pub async fn deliver(config: &NotifyConfig, event: &NotifyEvent) {
    if !is_subscribed(&config.events, &event.kind) {
        return;
    }
    for channel in &config.channels {
        if let Err(error) = deliver_one(channel, event).await {
            super::log(&format!("投递失败（{}）：{}", label(channel), error));
        }
    }
}

/// 发给单个渠道。返回可读的失败原因（界面「发送测试」直接显示它）。
pub async fn deliver_one(channel: &NotifyChannelConfig, event: &NotifyEvent) -> Result<(), String> {
    let url = channel.url.trim();
    if url.is_empty() {
        return Err("Webhook 地址为空".to_string());
    }
    let payload = payload_for(&channel.channel_type, event);
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败：{error}"))?;
    let request = match &payload {
        // 通用渠道发纯文本（不是 JSON）
        Payload::Text(text) => client.post(url).body(text.clone()),
        Payload::Json(value) => client
            .post(url)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(value).map_err(|e| format!("消息体序列化失败：{e}"))?),
    };
    let response = request.send().await.map_err(|error| format!("请求失败：{error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // 把对方回的正文带出来（钉钉 / 飞书失败时会说明原因，如「关键词不匹配」
        // —— 那是最常见的失败原因，不显示会让人完全猜不到）
        let snippet = body.trim();
        let detail = if snippet.is_empty() { String::new() } else { format!("：{}", truncate(snippet, 300)) };
        return Err(format!("HTTP {}{}", status.as_u16(), detail));
    }
    // 飞书 / 钉钉在 HTTP 200 时也可能返回错误码（body 里有 code != 0）
    if let Some(message) = embedded_error(&body) {
        return Err(message);
    }
    Ok(())
}

/// 消息体的两种形态（通用渠道发纯文本，其余发 JSON）
enum Payload {
    Text(String),
    Json(Value),
}

/// 按渠道类型组装消息体（键名差异见模块头）。
fn payload_for(channel_type: &str, event: &NotifyEvent) -> Payload {
    let text = event.plain_text();
    match channel_type {
        // 飞书：content.text
        "feishu" => Payload::Json(json!({
            "msgtype": "text",
            "content": { "text": text },
        })),
        // 企业微信：content.content（键名与飞书不同）
        "wecom" => Payload::Json(json!({
            "msgtype": "text",
            "content": { "content": text },
        })),
        // 钉钉：顶层 text.content（与飞书正好错位）
        "dingtalk" => Payload::Json(json!({
            "msgtype": "text",
            "text": { "content": text },
        })),
        // 其余（含 generic 与任何未知值）：纯文本。**不猜** —— 猜错形状对方直接
        // 拒收，而纯文本是自建接收端最容易处理的一种
        _ => Payload::Text(text),
    }
}

/// 从响应体里找「HTTP 200 但业务失败」的错误码。
///
/// 飞书：`{"code":19024,"msg":"Key Words Not Found"}`；
/// 钉钉：`{"errcode":310000,"errmsg":"keywords not in content"}`。
/// 两家的键名完全不同（code/msg 与 errcode/errmsg），都要认。
fn embedded_error(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let code = value
        .get("code")
        .or_else(|| value.get("errcode"))
        .and_then(Value::as_i64);
    if code.unwrap_or(0) == 0 {
        return None;
    }
    let message = value
        .get("msg")
        .or_else(|| value.get("errmsg"))
        .and_then(Value::as_str)
        .unwrap_or("对方返回了错误码");
    Some(format!("渠道返回错误 {}：{}", code.unwrap_or(-1), message))
}

/// 截断到 n 个**字符**（不是字节 —— 中文按字节切会切出半个字）
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut result: String = text.chars().take(limit).collect();
    result.push('…');
    result
}

fn label(channel: &NotifyChannelConfig) -> String {
    let name = channel.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    if channel.channel_type.trim().is_empty() {
        "未命名渠道".to_string()
    } else {
        channel.channel_type.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NotifyEvent {
        NotifyEvent::test()
    }

    /// 三家形状的键名两两不同 —— 写错就会被对方以 400 拒收，
    /// 所以把这三条断言钉在这里
    #[test]
    fn feishu_shape_is_content_text() {
        match payload_for("feishu", &sample()) {
            Payload::Json(value) => {
                assert_eq!(value["msgtype"], "text");
                assert!(value["content"]["text"].is_string(), "飞书必须是 content.text");
                assert!(value["content"]["content"].is_null());
            }
            _ => panic!("飞书应发 JSON"),
        }
    }

    #[test]
    fn wecom_shape_is_content_content() {
        match payload_for("wecom", &sample()) {
            Payload::Json(value) => {
                assert_eq!(value["msgtype"], "text");
                assert!(value["content"]["content"].is_string(), "企微必须是 content.content");
                assert!(value["content"]["text"].is_null());
            }
            _ => panic!("企微应发 JSON"),
        }
    }

    #[test]
    fn dingtalk_shape_is_top_level_text() {
        match payload_for("dingtalk", &sample()) {
            Payload::Json(value) => {
                assert!(value["text"]["content"].is_string(), "钉钉必须是顶层 text.content");
                assert!(value["content"].is_null(), "钉钉顶层不该有 content");
            }
            _ => panic!("钉钉应发 JSON"),
        }
    }

    #[test]
    fn unknown_type_falls_back_to_plain_text() {
        assert!(matches!(payload_for("generic", &sample()), Payload::Text(_)));
        assert!(matches!(payload_for("", &sample()), Payload::Text(_)));
    }

    #[test]
    fn embedded_error_reads_both_shapes() {
        assert_eq!(embedded_error("{\"code\":19024,\"msg\":\"x\"}").is_some(), true);
        assert_eq!(embedded_error("{\"errcode\":310000,\"errmsg\":\"y\"}").is_some(), true);
        assert_eq!(embedded_error("{\"code\":0}").is_none(), true);
        assert_eq!(embedded_error("not json").is_none(), true);
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // 中文按字节切会切出半个字；这里必须按字符数
        let text = "中文测试内容";
        assert_eq!(truncate(text, 3), "中文测…");
        assert_eq!(truncate("abc", 5), "abc");
    }
}
