//! 三类通知源的 HTTP 发送实现：Webhook（通用 JSON）、Telegram、Bark。

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::{
    apply_patch, decrypt_secret, decrypt_webhook_headers, validate_http_url, BarkChannelConfig,
    NotificationSettings, TelegramChannelConfig, WebhookChannelConfig,
};
use crate::server::core::egress;
use crate::server::logging;

const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn dispatch_all(
    settings: &NotificationSettings,
    event: &str,
    title: &str,
    body: &str,
    data: &Value,
) -> Vec<(&'static str, Result<(), String>)> {
    let mut results = Vec::new();
    if settings.webhook.enabled {
        results.push((
            "Webhook",
            send_webhook(&settings.webhook, event, title, body, data).await,
        ));
    }
    if settings.telegram.enabled {
        results.push(("Telegram", send_telegram(&settings.telegram, title, body).await));
    }
    if settings.bark.enabled {
        results.push(("Bark", send_bark(&settings.bark, title, body).await));
    }
    results
}

pub async fn send_test(payload: &Map<String, Value>) -> Result<&'static str, (i32, String)> {
    let channel = payload
        .get("channel")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mut settings = NotificationSettings::current();
    // 允许用户在点击「保存」前直接携带当前表单草稿点击「测试发送」
    let mut draft = payload.clone();
    draft.remove("channel");
    if !draft.is_empty() {
        apply_patch(&mut settings, &draft).map_err(|msg| (400, msg))?;
    }
    let title = "Agent2API 通知测试";
    let body = "这是一条来自 Agent2API 的测试通知，当前通道配置正常。";
    let data = json!({
        "test": true,
        "channel": channel,
    });
    match channel.as_str() {
        "webhook" => {
            send_webhook(&settings.webhook, "test", title, body, &data)
                .await
                .map_err(|msg| (502, msg))?;
            Ok("Webhook")
        }
        "telegram" => {
            send_telegram(&settings.telegram, title, body)
                .await
                .map_err(|msg| (502, msg))?;
            Ok("Telegram")
        }
        "bark" => {
            send_bark(&settings.bark, title, body)
                .await
                .map_err(|msg| (502, msg))?;
            Ok("Bark")
        }
        _ => Err((
            400,
            "未知通知渠道，请选择 webhook、telegram 或 bark".to_string(),
        )),
    }
}

pub async fn send_webhook(
    config: &WebhookChannelConfig,
    event: &str,
    title: &str,
    body: &str,
    data: &Value,
) -> Result<(), String> {
    let url = validate_http_url(&config.url, "Webhook 地址")?;
    let headers = decrypt_webhook_headers(&config.headers_enc)?;
    let client = egress::client_for_system_proxy();
    let payload = json!({
        "title": title,
        "body": body,
        "event": event,
        "timestamp": logging::now_ms(),
        "source": "Agent2API",
        "data": data,
    });
    let mut req = client
        .post(&url)
        .timeout(NOTIFY_TIMEOUT)
        .header("Content-Type", "application/json")
        .json(&payload);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let response = req
        .send()
        .await
        .map_err(|err| format!("Webhook 请求失败：{}", egress::describe_error_detail(&err)))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let snippet: String = text.trim().chars().take(160).collect();
        return Err(if snippet.is_empty() {
            format!("Webhook 返回 HTTP {status}")
        } else {
            format!("Webhook 返回 HTTP {status}：{snippet}")
        });
    }
    Ok(())
}

pub async fn send_telegram(
    config: &TelegramChannelConfig,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let token = decrypt_secret(&config.bot_token_enc, "Telegram Bot Token")?;
    let chat_id = config.chat_id.trim();
    if chat_id.is_empty() {
        return Err("请先填写 Telegram Chat ID".to_string());
    }
    let base = validate_http_url(&config.api_base, "Telegram 接口地址")?;
    let endpoint = format!("{base}/bot{}/sendMessage", token.as_str());
    let payload = json!({
        "chat_id": chat_id,
        "text": format!("【{title}】\n{body}"),
        "disable_web_page_preview": true,
    });
    let proxy = super::parse_telegram_proxy(&config.proxy_url)?;
    let client = if let Some(ref resolved) = proxy {
        egress::client_for(Some(resolved))
    } else {
        egress::client_for_system_proxy()
    };
    let response = client
        .post(&endpoint)
        .timeout(NOTIFY_TIMEOUT)
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("Telegram 请求失败：{}", egress::describe_error_detail(&err)))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<Value>(&text).ok();
    if !status.is_success()
        || parsed
            .as_ref()
            .and_then(|v| v.get("ok"))
            .and_then(Value::as_bool)
            == Some(false)
    {
        let desc = parsed
            .as_ref()
            .and_then(|v| v.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| text.trim().chars().take(160).collect());
        return Err(if desc.is_empty() {
            format!("Telegram 返回 HTTP {status}")
        } else {
            format!("Telegram 发送失败：{desc}")
        });
    }
    Ok(())
}

pub async fn send_bark(
    config: &BarkChannelConfig,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let device_key = decrypt_secret(&config.device_key_enc, "Bark Device Key")?;
    let base = validate_http_url(&config.server_url, "Bark 服务地址")?;
    let endpoint = format!("{base}/push");
    let group = if config.group.trim().is_empty() {
        super::DEFAULT_BARK_GROUP
    } else {
        config.group.trim()
    };
    let mut payload = json!({
        "device_key": device_key.as_str(),
        "title": title,
        "body": body,
        "group": group,
    });
    if !config.sound.trim().is_empty() {
        payload["sound"] = Value::String(config.sound.trim().to_string());
    }
    let client = egress::client_for_system_proxy();
    let response = client
        .post(&endpoint)
        .timeout(NOTIFY_TIMEOUT)
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("Bark 请求失败：{}", egress::describe_error_detail(&err)))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let code_err = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 200);
    if !status.is_success() || code_err {
        let msg = parsed
            .as_ref()
            .and_then(|v| v.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| text.trim().chars().take(160).collect());
        return Err(if msg.is_empty() {
            format!("Bark 返回 HTTP {status}")
        } else {
            format!("Bark 发送失败：{msg}")
        });
    }
    Ok(())
}
