//! 系统通知配置与事件分发（默认关闭；支持登录成功、登录失败、自动签到失败）。
//!
//! 敏感凭据（Webhook 自定义请求头、Telegram Bot Token、Bark Device Key）统一使用
//! `SecretScope::PanelCaptcha` 加密后落入配置项 `notifications`，对前端只回传
//! `*Set: bool` 与请求头名称列表，不回传明文。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use crate::server::config;
use crate::server::core::beijing;
use crate::server::core::crypto::{self, SecretScope};
use crate::server::logging;

pub mod channels;

pub const KEY_NOTIFICATIONS: &str = "notifications";
const SCOPE: SecretScope = SecretScope::PanelCaptcha;

pub const DEFAULT_TELEGRAM_API_BASE: &str = "https://api.telegram.org";
pub const DEFAULT_BARK_SERVER_URL: &str = "https://api.day.app";
pub const DEFAULT_BARK_GROUP: &str = "Agent2API";

#[derive(Clone, Debug, Default)]
pub struct EventSwitches {
    pub login_success: bool,
    pub login_failure: bool,
    pub checkin_failure: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WebhookChannelConfig {
    pub enabled: bool,
    pub url: String,
    pub(crate) headers_enc: String,
    pub header_names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TelegramChannelConfig {
    pub enabled: bool,
    pub(crate) bot_token_enc: String,
    pub chat_id: String,
    pub api_base: String,
    pub proxy_url: String,
}

impl Default for TelegramChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token_enc: String::new(),
            chat_id: String::new(),
            api_base: DEFAULT_TELEGRAM_API_BASE.to_string(),
            proxy_url: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BarkChannelConfig {
    pub enabled: bool,
    pub(crate) device_key_enc: String,
    pub server_url: String,
    pub group: String,
    pub sound: String,
}

impl Default for BarkChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device_key_enc: String::new(),
            server_url: DEFAULT_BARK_SERVER_URL.to_string(),
            group: DEFAULT_BARK_GROUP.to_string(),
            sound: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub events: EventSwitches,
    pub webhook: WebhookChannelConfig,
    pub telegram: TelegramChannelConfig,
    pub bark: BarkChannelConfig,
}

impl NotificationSettings {
    pub fn current() -> Self {
        config::with_raw(Self::from_raw).unwrap_or_default()
    }

    pub fn from_raw(raw: &Map<String, Value>) -> Self {
        let Some(root) = raw.get(KEY_NOTIFICATIONS).and_then(Value::as_object) else {
            return Self::default();
        };
        let enabled = root.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        let events_obj = root.get("events").and_then(Value::as_object);
        let events = EventSwitches {
            login_success: events_obj
                .and_then(|m| m.get("loginSuccess"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            login_failure: events_obj
                .and_then(|m| m.get("loginFailure"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            checkin_failure: events_obj
                .and_then(|m| m.get("checkinFailure"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };

        let channels_obj = root.get("channels").and_then(Value::as_object);
        let wh_obj = channels_obj
            .and_then(|m| m.get("webhook"))
            .and_then(Value::as_object);
        let webhook = WebhookChannelConfig {
            enabled: wh_obj
                .and_then(|m| m.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            url: wh_obj
                .and_then(|m| m.get("url"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            headers_enc: wh_obj
                .and_then(|m| m.get("headersEnc"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            header_names: wh_obj
                .and_then(|m| m.get("headerNames"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        };

        let tg_obj = channels_obj
            .and_then(|m| m.get("telegram"))
            .and_then(Value::as_object);
        let tg_base = tg_obj
            .and_then(|m| m.get("apiBase"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_TELEGRAM_API_BASE)
            .to_string();
        let telegram = TelegramChannelConfig {
            enabled: tg_obj
                .and_then(|m| m.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            bot_token_enc: tg_obj
                .and_then(|m| m.get("botTokenEnc"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            chat_id: tg_obj
                .and_then(|m| m.get("chatId"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            api_base: tg_base,
            proxy_url: tg_obj
                .and_then(|m| m.get("proxyUrl"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
        };

        let bark_obj = channels_obj
            .and_then(|m| m.get("bark"))
            .and_then(Value::as_object);
        let bark_server = bark_obj
            .and_then(|m| m.get("serverUrl"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_BARK_SERVER_URL)
            .to_string();
        let bark_group = bark_obj
            .and_then(|m| m.get("group"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_BARK_GROUP)
            .to_string();
        let bark = BarkChannelConfig {
            enabled: bark_obj
                .and_then(|m| m.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            device_key_enc: bark_obj
                .and_then(|m| m.get("deviceKeyEnc"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            server_url: bark_server,
            group: bark_group,
            sound: bark_obj
                .and_then(|m| m.get("sound"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
        };

        Self {
            enabled,
            events,
            webhook,
            telegram,
            bark,
        }
    }

    pub fn to_stored_value(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "events": {
                "loginSuccess": self.events.login_success,
                "loginFailure": self.events.login_failure,
                "checkinFailure": self.events.checkin_failure,
            },
            "channels": {
                "webhook": {
                    "enabled": self.webhook.enabled,
                    "url": self.webhook.url,
                    "headersEnc": self.webhook.headers_enc,
                    "headerNames": self.webhook.header_names,
                },
                "telegram": {
                    "enabled": self.telegram.enabled,
                    "botTokenEnc": self.telegram.bot_token_enc,
                    "chatId": self.telegram.chat_id,
                    "apiBase": self.telegram.api_base,
                    "proxyUrl": self.telegram.proxy_url,
                },
                "bark": {
                    "enabled": self.bark.enabled,
                    "deviceKeyEnc": self.bark.device_key_enc,
                    "serverUrl": self.bark.server_url,
                    "group": self.bark.group,
                    "sound": self.bark.sound,
                }
            }
        })
    }

    pub fn management_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "events": {
                "loginSuccess": self.events.login_success,
                "loginFailure": self.events.login_failure,
                "checkinFailure": self.events.checkin_failure,
            },
            "channels": {
                "webhook": {
                    "enabled": self.webhook.enabled,
                    "url": self.webhook.url,
                    "headersSet": !self.webhook.headers_enc.is_empty(),
                    "headerNames": self.webhook.header_names,
                },
                "telegram": {
                    "enabled": self.telegram.enabled,
                    "botTokenSet": !self.telegram.bot_token_enc.is_empty(),
                    "chatId": self.telegram.chat_id,
                    "apiBase": self.telegram.api_base,
                    "proxyUrl": self.telegram.proxy_url,
                },
                "bark": {
                    "enabled": self.bark.enabled,
                    "deviceKeySet": !self.bark.device_key_enc.is_empty(),
                    "serverUrl": self.bark.server_url,
                    "group": self.bark.group,
                    "sound": self.bark.sound,
                }
            }
        })
    }
}

pub(crate) fn decrypt_secret(enc: &str, label: &str) -> Result<Zeroizing<String>, String> {
    if enc.trim().is_empty() {
        return Err(format!("请先填写{label}"));
    }
    if !crypto::is_envelope(enc) {
        return Ok(Zeroizing::new(enc.to_string()));
    }
    let bytes = Zeroizing::new(
        crypto::open(enc, SCOPE)
            .map_err(|_| format!("{label}无法解密，请在通知设置中重新保存"))?,
    );
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| format!("{label}格式无效"))
}

pub(crate) fn decrypt_webhook_headers(enc: &str) -> Result<BTreeMap<String, String>, String> {
    if enc.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let plain = decrypt_secret(enc, "Webhook 自定义请求头")?;
    serde_json::from_str::<BTreeMap<String, String>>(&plain)
        .map_err(|_| "Webhook 自定义请求头格式无效，请重新保存".to_string())
}

pub(crate) fn validate_http_url(raw: &str, label: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("请填写{label}"));
    }
    let parsed = url::Url::parse(trimmed).map_err(|_| format!("{label}格式无效"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!("{label}必须以 http:// 或 https:// 开头"));
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

pub(crate) fn parse_telegram_proxy(raw: &str) -> Result<Option<crate::server::core::proxies::ResolvedProxy>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| "Telegram 代理地址格式无效，请使用 http:// 或 socks5:// 开头".to_string())?;
    let protocol = match parsed.scheme().to_ascii_lowercase().as_str() {
        "http" | "https" => "http",
        "socks5" | "socks5h" => "socks5",
        _ => return Err("Telegram 代理仅支持 http / https / socks5 协议".to_string()),
    };
    let host = parsed
        .host_str()
        .map(|s| s.trim_matches(|c| c == '[' || c == ']').to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Telegram 代理地址缺少主机名".to_string())?;
    let port = parsed
        .port()
        .filter(|&p| p > 0)
        .ok_or_else(|| "Telegram 代理地址必须包含有效端口（1-65535）".to_string())?;
    Ok(Some(crate::server::core::proxies::ResolvedProxy {
        source: "custom".to_string(),
        protocol: protocol.to_string(),
        host,
        port: Some(port),
        username: parsed.username().to_string(),
        password: parsed.password().unwrap_or("").to_string(),
        label: trimmed.to_string(),
        non_direct: false,
    }))
}

pub fn apply_patch(
    settings: &mut NotificationSettings,
    object: &Map<String, Value>,
) -> Result<(), String> {
    if let Some(enabled) = object.get("enabled") {
        settings.enabled = enabled
            .as_bool()
            .ok_or_else(|| "enabled 必须是布尔值".to_string())?;
    }
    if let Some(events) = object.get("events").and_then(Value::as_object) {
        for (key, slot) in [
            ("loginSuccess", &mut settings.events.login_success),
            ("loginFailure", &mut settings.events.login_failure),
            ("checkinFailure", &mut settings.events.checkin_failure),
        ] {
            if let Some(val) = events.get(key) {
                *slot = val
                    .as_bool()
                    .ok_or_else(|| format!("events.{key} 必须是布尔值"))?;
            }
        }
    }
    if let Some(channels) = object.get("channels").and_then(Value::as_object) {
        if let Some(wh) = channels.get("webhook").and_then(Value::as_object) {
            if let Some(enabled) = wh.get("enabled") {
                settings.webhook.enabled = enabled
                    .as_bool()
                    .ok_or_else(|| "webhook.enabled 必须是布尔值".to_string())?;
            }
            if let Some(url) = wh.get("url").and_then(Value::as_str) {
                let trimmed = url.trim();
                if trimmed.is_empty() {
                    settings.webhook.url.clear();
                } else {
                    let parsed = url::Url::parse(trimmed)
                        .map_err(|_| "Webhook 地址格式无效".to_string())?;
                    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                        return Err("Webhook 地址必须以 http:// 或 https:// 开头".to_string());
                    }
                    settings.webhook.url = trimmed.to_string();
                }
            }
            if wh.get("clearHeaders").and_then(Value::as_bool) == Some(true)
                || wh.get("headers").is_some_and(Value::is_null)
            {
                settings.webhook.headers_enc.clear();
                settings.webhook.header_names.clear();
            } else if let Some(headers_val) = wh.get("headers") {
                let map = parse_headers_input(headers_val)?;
                if map.is_empty() {
                    if headers_val.is_object() {
                        settings.webhook.headers_enc.clear();
                        settings.webhook.header_names.clear();
                    }
                } else {
                    let names: Vec<String> = map.keys().cloned().collect();
                    let serialized = serde_json::to_string(&map)
                        .map_err(|_| "序列化自定义请求头失败".to_string())?;
                    settings.webhook.headers_enc = crypto::seal(serialized.as_bytes(), SCOPE)?;
                    settings.webhook.header_names = names;
                }
            }
        }

        if let Some(tg) = channels.get("telegram").and_then(Value::as_object) {
            if let Some(enabled) = tg.get("enabled") {
                settings.telegram.enabled = enabled
                    .as_bool()
                    .ok_or_else(|| "telegram.enabled 必须是布尔值".to_string())?;
            }
            if let Some(chat_id) = tg.get("chatId").and_then(Value::as_str) {
                settings.telegram.chat_id = chat_id.trim().to_string();
            }
            if let Some(api_base) = tg.get("apiBase").and_then(Value::as_str) {
                let trimmed = api_base.trim();
                settings.telegram.api_base = if trimmed.is_empty() {
                    DEFAULT_TELEGRAM_API_BASE.to_string()
                } else {
                    validate_http_url(trimmed, "Telegram 接口地址")?
                };
            }
            if let Some(proxy_url) = tg.get("proxyUrl").and_then(Value::as_str) {
                let trimmed = proxy_url.trim();
                if trimmed.is_empty() {
                    settings.telegram.proxy_url.clear();
                } else {
                    parse_telegram_proxy(trimmed)?;
                    settings.telegram.proxy_url = trimmed.to_string();
                }
            }
            if tg.get("clearBotToken").and_then(Value::as_bool) == Some(true)
                || tg.get("botToken").is_some_and(Value::is_null)
            {
                settings.telegram.bot_token_enc.clear();
            } else if let Some(token) = tg.get("botToken").and_then(Value::as_str) {
                let trimmed = token.trim();
                if !trimmed.is_empty() {
                    settings.telegram.bot_token_enc = crypto::seal(trimmed.as_bytes(), SCOPE)?;
                }
            }
        }

        if let Some(bk) = channels.get("bark").and_then(Value::as_object) {
            if let Some(enabled) = bk.get("enabled") {
                settings.bark.enabled = enabled
                    .as_bool()
                    .ok_or_else(|| "bark.enabled 必须是布尔值".to_string())?;
            }
            if let Some(server_url) = bk.get("serverUrl").and_then(Value::as_str) {
                let trimmed = server_url.trim();
                settings.bark.server_url = if trimmed.is_empty() {
                    DEFAULT_BARK_SERVER_URL.to_string()
                } else {
                    validate_http_url(trimmed, "Bark 服务地址")?
                };
            }
            if let Some(group) = bk.get("group").and_then(Value::as_str) {
                let trimmed = group.trim();
                settings.bark.group = if trimmed.is_empty() {
                    DEFAULT_BARK_GROUP.to_string()
                } else {
                    trimmed.to_string()
                };
            }
            if let Some(sound) = bk.get("sound").and_then(Value::as_str) {
                settings.bark.sound = sound.trim().to_string();
            }
            if bk.get("clearDeviceKey").and_then(Value::as_bool) == Some(true)
                || bk.get("deviceKey").is_some_and(Value::is_null)
            {
                settings.bark.device_key_enc.clear();
            } else if let Some(key) = bk.get("deviceKey").and_then(Value::as_str) {
                let trimmed = key.trim();
                if !trimmed.is_empty() {
                    settings.bark.device_key_enc = crypto::seal(trimmed.as_bytes(), SCOPE)?;
                }
            }
        }
    }

    validate_enabled_channels(settings)
}

fn validate_enabled_channels(settings: &NotificationSettings) -> Result<(), String> {
    if settings.webhook.enabled {
        validate_http_url(&settings.webhook.url, "Webhook 地址")?;
    }
    if settings.telegram.enabled {
        if settings.telegram.bot_token_enc.is_empty() {
            return Err("启用 Telegram 通知前请填写 Bot Token".to_string());
        }
        if settings.telegram.chat_id.is_empty() {
            return Err("启用 Telegram 通知前请填写 Chat ID".to_string());
        }
        validate_http_url(&settings.telegram.api_base, "Telegram 接口地址")?;
    }
    if settings.bark.enabled {
        if settings.bark.device_key_enc.is_empty() {
            return Err("启用 Bark 通知前请填写 Device Key".to_string());
        }
        validate_http_url(&settings.bark.server_url, "Bark 服务地址")?;
    }
    Ok(())
}

pub(crate) fn parse_headers_input(value: &Value) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    match value {
        Value::Object(obj) => {
            for (k, v) in obj {
                let key = k.trim();
                if key.is_empty() {
                    continue;
                }
                let val = v
                    .as_str()
                    .ok_or_else(|| format!("请求头 {key} 的值必须是字符串"))?
                    .trim();
                if !val.is_empty() {
                    map.insert(key.to_string(), val.to_string());
                }
            }
        }
        Value::String(text) => {
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Some((k, v)) = trimmed.split_once(':') else {
                    return Err(format!("请求头格式错误（应为 Header: Value）：{trimmed}"));
                };
                let key = k.trim();
                let val = v.trim();
                if key.is_empty() || val.is_empty() {
                    return Err(format!("请求头名称或值不能为空：{trimmed}"));
                }
                map.insert(key.to_string(), val.to_string());
            }
        }
        _ => return Err("自定义请求头格式无效".to_string()),
    }
    if map.len() > 32 {
        return Err("自定义请求头最多支持 32 项".to_string());
    }
    Ok(map)
}

pub fn save(object: &Map<String, Value>) -> Result<NotificationSettings, (i32, String)> {
    config::try_update_raw_field(
        KEY_NOTIFICATIONS,
        |slot| {
            let mut raw_map = Map::new();
            if !slot.is_null() {
                raw_map.insert(KEY_NOTIFICATIONS.to_string(), slot.clone());
            }
            let mut settings = NotificationSettings::from_raw(&raw_map);
            apply_patch(&mut settings, object).map_err(|msg| (400, msg))?;
            *slot = settings.to_stored_value();
            Ok(settings)
        },
        || (500, "写入通知设置失败，请重试".to_string()),
    )
}

fn now_beijing_label() -> String {
    beijing::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 面板登录成功通知（异步后台分发，不阻塞登录响应）。
pub fn notify_login_success(method: &str, identity: &str, ip: &str) {
    let settings = NotificationSettings::current();
    if !settings.enabled || !settings.events.login_success {
        return;
    }
    let method = method.to_string();
    let identity = if identity.trim().is_empty() {
        "管理员".to_string()
    } else {
        identity.trim().to_string()
    };
    let ip = ip.to_string();
    let time_str = now_beijing_label();
    let title = "面板登录成功".to_string();
    let body = format!(
        "登录方式：{method}\n登录账号：{identity}\n来源地址：{ip}\n时间：{time_str}"
    );
    let data = json!({
        "method": method,
        "identity": identity,
        "ip": ip,
        "time": time_str,
    });
    spawn_dispatch(settings, "login_success", title, body, data);
}

/// 面板登录失败 / 锁定告警（异步后台分发）。
pub fn notify_login_failure(method: &str, identity: &str, ip: &str, reason: &str) {
    let settings = NotificationSettings::current();
    if !settings.enabled || !settings.events.login_failure {
        return;
    }
    let method = method.to_string();
    let identity = if identity.trim().is_empty() {
        "未知".to_string()
    } else {
        identity.trim().to_string()
    };
    let ip = ip.to_string();
    let reason = reason.to_string();
    let time_str = now_beijing_label();
    let title = "面板登录失败告警".to_string();
    let body = format!(
        "登录方式：{method}\n尝试账号：{identity}\n来源地址：{ip}\n原因：{reason}\n时间：{time_str}"
    );
    let data = json!({
        "method": method,
        "identity": identity,
        "ip": ip,
        "reason": reason,
        "time": time_str,
    });
    spawn_dispatch(settings, "login_failure", title, body, data);
}

/// 自动签到失败通知（仅在 `source` 为 `schedule` / `makeup` / `startup` 时触发；手动签到绝不触发）。
pub fn notify_checkin_failure(source: &str, reason: &str, failures: &[String]) {
    if !matches!(source, "schedule" | "makeup" | "startup") || failures.is_empty() {
        return;
    }
    let settings = NotificationSettings::current();
    if !settings.enabled || !settings.events.checkin_failure {
        return;
    }
    let source = source.to_string();
    let reason = reason.to_string();
    let count = failures.len();
    let preview: Vec<String> = failures.iter().take(10).cloned().collect();
    let mut lines = preview
        .iter()
        .map(|item| format!("• {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    if count > preview.len() {
        lines.push_str(&format!("\n…等共 {count} 个账号"));
    }
    let time_str = now_beijing_label();
    let title = "自动签到失败提醒".to_string();
    let body = format!(
        "触发轮次：{reason}\n失败数量：{count} 个账号\n失败明细：\n{lines}\n时间：{time_str}"
    );
    let data = json!({
        "source": source,
        "reason": reason,
        "failedCount": count,
        "failures": failures,
        "time": time_str,
    });
    spawn_dispatch(settings, "checkin_failure", title, body, data);
}

fn spawn_dispatch(
    settings: NotificationSettings,
    event: &'static str,
    title: String,
    body: String,
    data: Value,
) {
    if !settings.webhook.enabled && !settings.telegram.enabled && !settings.bark.enabled {
        return;
    }
    crate::spawn_task(async move {
        let outcomes = channels::dispatch_all(&settings, event, &title, &body, &data).await;
        for (channel, result) in outcomes {
            if let Err(error) = result {
                logging::log(
                    "[Notify]",
                    &format!("⚠️ 通知发送失败（{channel} / {event}）：{error}"),
                );
            }
        }
    });
}
