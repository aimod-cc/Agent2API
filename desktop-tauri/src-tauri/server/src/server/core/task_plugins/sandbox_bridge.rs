//! 定时任务沙箱的受控宿主能力：白名单 HTTP、加密小仓库、系统通知与常用加解密辅助。

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use md5::Md5;
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::server::core::{egress, notifications, proxy_pool};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_STORAGE_BYTES: usize = 256 * 1024;
const MAX_STORAGE_KEYS: usize = 256;
const MAX_OUTPUT_CHARS: usize = 100_000;
const MAX_REDIRECTS: usize = 5;

#[derive(Clone)]
pub struct SandboxHost {
    pub plugin_id: String,
    pub plugin_name: String,
    pub domains: Vec<String>,
    pub declares_notify: bool,
    pub notify_allowed: bool,
    pub proxy_id: String,
    pub timeout_ms: u64,
    pub preview_mode: bool,
    pub abort: Arc<AtomicBool>,
    pub abort_notify: Arc<Notify>,
    pub logs: Arc<Mutex<Vec<String>>>,
    pub logs_len: Arc<Mutex<usize>>,
    pub storage: Arc<Mutex<StorageState>>,
}

#[derive(Default)]
pub struct StorageState {
    pub map: BTreeMap<String, String>,
    pub dirty: bool,
}

impl SandboxHost {
    pub fn append_log(&self, level: &str, message: &str) {
        let Ok(mut total) = self.logs_len.lock() else { return };
        if *total >= MAX_OUTPUT_CHARS {
            return;
        }
        let line = format!("[{level}] {message}");
        let remaining = MAX_OUTPUT_CHARS - *total;
        let clipped: String = line.chars().take(remaining).collect();
        *total += clipped.chars().count() + 1;
        if let Ok(mut list) = self.logs.lock() {
            list.push(clipped);
        }
    }

    pub fn take_logs(&self) -> String {
        self.logs
            .lock()
            .map(|list| list.join("\n"))
            .unwrap_or_default()
    }

    pub fn is_aborted(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }

    pub fn storage_get(&self, key: &str) -> Result<Option<String>, String> {
        let guard = self.storage.lock().map_err(|_| "任务存储锁不可用".to_string())?;
        Ok(guard.map.get(key).cloned())
    }

    pub fn storage_set(&self, key: &str, value: &str) -> Result<(), String> {
        if key.is_empty() || key.len() > 128 {
            return Err("存储键名长度须在 1–128 字符之间".to_string());
        }
        let mut guard = self.storage.lock().map_err(|_| "任务存储锁不可用".to_string())?;
        if !guard.map.contains_key(key) && guard.map.len() >= MAX_STORAGE_KEYS {
            return Err(format!("单个任务最多存储 {MAX_STORAGE_KEYS} 个键"));
        }
        let prev_size: usize = guard
            .map
            .iter()
            .filter(|(k, _)| k.as_str() != key)
            .map(|(k, v)| k.len() + v.len())
            .sum();
        if prev_size + key.len() + value.len() > MAX_STORAGE_BYTES {
            return Err("任务专属存储已达 256 KB 上限".to_string());
        }
        guard.map.insert(key.to_string(), value.to_string());
        guard.dirty = true;
        Ok(())
    }

    pub fn storage_delete(&self, key: &str) -> Result<(), String> {
        let mut guard = self.storage.lock().map_err(|_| "任务存储锁不可用".to_string())?;
        if guard.map.remove(key).is_some() {
            guard.dirty = true;
        }
        Ok(())
    }

    pub fn storage_keys(&self) -> Result<Vec<String>, String> {
        let guard = self.storage.lock().map_err(|_| "任务存储锁不可用".to_string())?;
        Ok(guard.map.keys().cloned().collect())
    }

    pub async fn sleep_ms(&self, ms: u64) -> Result<(), String> {
        let clamped = ms.min(30_000);
        if self.is_aborted() {
            return Err("任务已停止".to_string());
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(clamped)) => Ok(()),
            _ = self.abort_notify.notified() => Err("任务已停止".to_string()),
        }
    }

    pub async fn notify_send(
        &self,
        title: &str,
        body: &str,
        data: Value,
    ) -> Result<Value, String> {
        if !self.declares_notify {
            return Err("当前插件未声明系统通知权限，无法发送通知".to_string());
        }
        if !self.notify_allowed {
            self.append_log("warn", "系统通知权限已被用户关闭，跳过发送");
            return Ok(json!({ "sent": false, "reason": "disabled_by_user" }));
        }
        let title: String = title.trim().chars().take(120).collect();
        let body: String = body.trim().chars().take(2000).collect();
        if title.is_empty() && body.is_empty() {
            return Err("通知标题和正文不能同时为空".to_string());
        }
        if self.preview_mode {
            self.append_log("info", &format!("试跑模拟系统通知：【{title}】{body}"));
            return Ok(json!({ "sent": true, "preview": true }));
        }
        let settings = notifications::NotificationSettings::current();
        let has_channel =
            settings.webhook.enabled || settings.telegram.enabled || settings.bark.enabled;
        if !settings.enabled || !has_channel {
            self.append_log("warn", "全局系统通知未开启或未启用任何通知渠道，跳过发送");
            return Ok(json!({ "sent": false, "reason": "no_channel_enabled" }));
        }
        let payload = json!({
            "pluginId": self.plugin_id,
            "pluginName": self.plugin_name,
            "extra": data,
        });
        let results = tokio::select! {
            res = notifications::channels::dispatch_all(
                &settings,
                "task_plugin",
                if title.is_empty() { &self.plugin_name } else { &title },
                &body,
                &payload,
            ) => res,
            _ = self.abort_notify.notified() => return Err("任务已停止".to_string()),
        };
        let mut ok_channels = Vec::new();
        let mut errors = Vec::new();
        for (channel, outcome) in results {
            match outcome {
                Ok(()) => {
                    self.append_log("info", &format!("已通过 {channel} 发送系统通知"));
                    ok_channels.push(channel);
                }
                Err(err) => {
                    self.append_log("warn", &format!("{channel} 通知发送失败：{err}"));
                    errors.push(format!("{channel}: {err}"));
                }
            }
        }
        Ok(json!({
            "sent": !ok_channels.is_empty(),
            "channels": ok_channels,
            "errors": errors,
        }))
    }

    pub async fn http_request(&self, payload: Value) -> Result<Value, String> {
        let raw_url = payload["url"]
            .as_str()
            .ok_or_else(|| "请求缺少 url".to_string())?;
        let mut url = validate_allowed_url(raw_url, &self.domains)?;
        let mut method_str = payload["method"]
            .as_str()
            .unwrap_or("GET")
            .trim()
            .to_ascii_uppercase();
        if !matches!(
            method_str.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
        ) {
            return Err(format!("不支持的 HTTP 方法：{method_str}"));
        }

        let mut req_headers = Vec::new();
        if let Some(obj) = payload.get("headers").and_then(Value::as_object) {
            for (k, v) in obj {
                let lower = k.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "host" | "connection" | "content-length" | "proxy-authorization"
                ) {
                    continue;
                }
                if let Some(val) = v.as_str() {
                    req_headers.push((k.clone(), val.to_string()));
                } else if !v.is_null() {
                    req_headers.push((k.clone(), v.to_string()));
                }
            }
        }

        let mut body_bytes: Option<Vec<u8>> = match payload.get("body") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.as_bytes().to_vec()),
            Some(other) => Some(other.to_string().into_bytes()),
        };

        let client = self.build_http_client()?;
        let per_req_timeout = Duration::from_millis(self.timeout_ms.min(60_000).max(1_000));

        for hop in 0..=MAX_REDIRECTS {
            if self.is_aborted() {
                return Err("任务已停止".to_string());
            }
            if self.proxy_id.is_empty() {
                verify_public_dns(&url).await?;
            }
            let method = reqwest::Method::from_bytes(method_str.as_bytes())
                .map_err(|_| format!("无效 HTTP 方法：{method_str}"))?;
            let mut builder = client.request(method, url.clone()).timeout(per_req_timeout);
            for (k, v) in &req_headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            if !matches!(method_str.as_str(), "GET" | "HEAD") {
                if let Some(ref body) = body_bytes {
                    builder = builder.body(body.clone());
                }
            }

            let mut response = tokio::select! {
                res = builder.send() => res.map_err(|err| format!("网络请求失败：{}", egress::describe_error_detail(&err)))?,
                _ = self.abort_notify.notified() => return Err("任务已停止".to_string()),
            };

            let status = response.status();
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                if let Some(loc) = response.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()) {
                    if hop == MAX_REDIRECTS {
                        return Err("HTTP 重定向次数超过上限".to_string());
                    }
                    let next_url = url
                        .join(loc)
                        .map_err(|_| format!("重定向地址无效：{loc}"))?;
                    url = validate_allowed_url(next_url.as_str(), &self.domains)?;
                    if status.as_u16() == 303 {
                        method_str = "GET".to_string();
                        body_bytes = None;
                    }
                    continue;
                }
            }

            let mut headers_map = serde_json::Map::new();
            let mut set_cookies = Vec::new();
            for (name, value) in response.headers() {
                if let Ok(text) = value.to_str() {
                    let key = name.as_str().to_ascii_lowercase();
                    if key == "set-cookie" {
                        set_cookies.push(Value::String(text.to_string()));
                    }
                    if let Some(existing) = headers_map.get(&key).and_then(Value::as_str).map(str::to_string) {
                        let combined = format!("{existing}, {text}");
                        headers_map.insert(key, Value::String(combined));
                    } else {
                        headers_map.insert(key, Value::String(text.to_string()));
                    }
                }
            }

            let mut buf = Vec::new();
            while let Some(chunk) = tokio::select! {
                next = response.chunk() => next.map_err(|err| format!("读取响应体失败：{err}"))?,
                _ = self.abort_notify.notified() => return Err("任务已停止".to_string()),
            } {
                if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err("HTTP 响应体超过 2 MB 上限".to_string());
                }
                buf.extend_from_slice(&chunk);
            }
            let body_text = String::from_utf8_lossy(&buf).into_owned();
            return Ok(json!({
                "ok": status.is_success(),
                "status": status.as_u16(),
                "statusText": status.canonical_reason().unwrap_or(""),
                "url": response.url().as_str(),
                "headers": headers_map,
                "setCookies": set_cookies,
                "body": body_text,
            }));
        }

        Err("未能完成 HTTP 请求".to_string())
    }

    fn build_http_client(&self) -> Result<reqwest::Client, String> {
        let mut builder = reqwest::Client::builder()
            .user_agent(egress::DEFAULT_USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .no_proxy();
        if !self.proxy_id.is_empty() {
            let resolved = proxy_pool::resolve_reference(&self.proxy_id)?;
            let port = resolved
                .port
                .ok_or_else(|| "所选代理缺少端口号".to_string())?;
            let scheme = if resolved.protocol.eq_ignore_ascii_case("socks5") {
                "socks5h"
            } else {
                "http"
            };
            let host = if resolved.host.contains(':') && !resolved.host.starts_with('[') {
                format!("[{}]", resolved.host)
            } else {
                resolved.host.clone()
            };
            let proxy_url = format!("{scheme}://{host}:{port}");
            let mut proxy = reqwest::Proxy::all(&proxy_url)
                .map_err(|error| format!("构造代理配置失败：{error}"))?;
            if !resolved.username.is_empty() || !resolved.password.is_empty() {
                proxy = proxy.basic_auth(&resolved.username, &resolved.password);
            }
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .map_err(|error| format!("初始化网络客户端失败：{error}"))
    }
}

pub fn validate_allowed_url(raw: &str, domains: &[String]) -> Result<url::Url, String> {
    let url = url::Url::parse(raw.trim()).map_err(|_| format!("无效请求地址：{raw}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("仅允许 http / https 请求地址".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("请求地址不允许内嵌用户名或密码".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "请求地址缺少主机名".to_string())?
        .to_ascii_lowercase();
    if is_blocked_host(&host) {
        return Err(format!("禁止访问本机或内网地址：{host}"));
    }
    if domains.is_empty() {
        return Err("该插件未声明任何允许访问的域名白名单".to_string());
    }
    let matched = domains.iter().any(|pattern| {
        if pattern == "*" {
            true
        } else if let Some(suffix) = pattern.strip_prefix("*.") {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host == *pattern
        }
    });
    if !matched {
        return Err(format!("域名「{host}」不在插件声明的白名单内"));
    }
    Ok(url)
}

fn is_blocked_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    let trimmed = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return is_private_ip(ip);
    }
    false
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => is_private_v6(v6),
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (o[0] == 100 && (64..=127).contains(&o[1]))
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_v4(v4);
    }
    let seg0 = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg0 & 0xfe00) == 0xfc00
        || (seg0 & 0xffc0) == 0xfe80
}

async fn verify_public_dns(url: &url::Url) -> Result<(), String> {
    let Some(host) = url.host_str() else {
        return Err("请求地址缺少主机名".to_string());
    };
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("域名解析失败（{host}）：{error}"))?;
    for addr in addrs {
        if is_private_ip(addr.ip()) {
            return Err(format!("域名「{host}」解析到内网或本机地址，已拦截"));
        }
    }
    Ok(())
}

pub fn crypto_helper(op: &str, arg1: &str, arg2: &str) -> Result<String, String> {
    match op {
        "md5" => {
            let mut h = Md5::new();
            h.update(arg1.as_bytes());
            Ok(hex_encode(&h.finalize()))
        }
        "sha1" => {
            let mut h = Sha1::new();
            h.update(arg1.as_bytes());
            Ok(hex_encode(&h.finalize()))
        }
        "sha256" => {
            let mut h = Sha256::new();
            h.update(arg1.as_bytes());
            Ok(hex_encode(&h.finalize()))
        }
        "hmacSha256" => {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(arg1.as_bytes())
                .map_err(|_| "HMAC 密钥无效".to_string())?;
            mac.update(arg2.as_bytes());
            Ok(hex_encode(&mac.finalize().into_bytes()))
        }
        "base64Encode" => Ok(base64::engine::general_purpose::STANDARD.encode(arg1.as_bytes())),
        "base64Decode" => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(arg1.trim())
                .map_err(|_| "Base64 内容无效".to_string())?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
        "randomHex" => {
            let n = arg1.trim().parse::<usize>().unwrap_or(16).clamp(1, 64);
            let mut buf = vec![0u8; n];
            getrandom::getrandom(&mut buf).map_err(|_| "随机数生成失败".to_string())?;
            Ok(hex_encode(&buf))
        }
        _ => Err(format!("不支持的加密辅助操作：{op}")),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}
