//! 仅从原RowSource完整授权投影提取本次只读请求需要的Cookie/设备/UA，不读客户端/文件/环境。
//! 宽输入别名只限原投影，多份Cookie包装/冲突Cookie/UA拒绝；不续Cookie、保存Set-Cookie或补设备。

use std::collections::BTreeMap;

use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use zeroize::Zeroizing;

use super::{json_text, DailyError, ORIGIN};
use crate::server::core::account_store::credential_version::{RowProvider, RowSource};

const MAX_COOKIES: usize = 512;
const MAX_COOKIE_BYTES: usize = 64 * 1024;
const MAX_SECRET: usize = 256 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
const CONTROL_VERSION: &str = "2.36.11";
const META_KEYS: &[&str] = &["name", "userId", "enterpriseType", "email", "avatar", "deviceId", "iid", "webId", "fingerprint",
    "clientVersion", "channel", "userAgent", "chromiumVersion", "teaUuid", "webTabId", "source", "desktop", "edition",
    "baseUrl", "provider", "id", "schema", "identity", "pluginIdentity", "tokenTail", "hasCredentials", "validation", "validatedAt"];

pub(in crate::server::core::plugin_host::wasm) struct Credentials {
    cookie: Zeroizing<String>,
    pairs: BTreeMap<String, Zeroizing<String>>,
    metadata: BTreeMap<String, Zeroizing<String>>,
    user_agent: Zeroizing<String>,
}

pub(in crate::server::core::plugin_host::wasm) fn from_original(row: &RowSource, now: i64) -> Result<Credentials, DailyError> {
    if row.provider() != RowProvider::DoubaoWork || row.identity().is_empty() { return Err(DailyError::CredentialsInvalid); }
    let secret = row.secret();
    target(secret)?;
    let fields = secret.as_object().ok_or(DailyError::CredentialsInvalid)?;
    if fields.len() > 128 { return Err(DailyError::CredentialsInvalid); }
    let cookie_fields = ["cookies", "cookieHeader", "cookie", "token"];
    let mut roots = cookie_fields.iter()
        .filter(|key| fields.get(**key).is_some_and(|value| !value.is_null()));
    let chosen = roots.next().copied();
    if roots.next().is_some() { return Err(DailyError::CredentialsInvalid); }
    if chosen.is_none() { return Err(DailyError::CredentialsInvalid); }
    let mut pairs = BTreeMap::new();
    collect(secret, now, 0, &mut pairs)?;
    if !["sessionid", "sessionid_ss"].iter().any(|key| pairs.get(*key).is_some_and(|value| !value.is_empty())) {
        return Err(DailyError::CredentialsInvalid);
    }
    let mut metadata = BTreeMap::new();
    for (field, cookies) in [("deviceId", &["device_id"][..]), ("iid", &["iid"][..]), ("webId", &["web_id"][..]), ("fingerprint", &["s_v_web_id", "fp"][..])] {
        if let Some(value) = cookies.iter().find_map(|key| pairs.get(*key)) { metadata.insert(field.to_string(), value.clone()); }
    }
    for key in ["deviceId", "iid", "webId", "fingerprint", "clientVersion", "channel", "userAgent", "chromiumVersion", "teaUuid", "webTabId"] {
        if let Some(value) = secret.get(key).filter(|value| !value.is_null()) {
            let raw = value.as_str().ok_or(DailyError::CredentialsInvalid)?;
            if raw.len() > 1024 || raw.chars().any(char::is_control) { return Err(DailyError::CredentialsInvalid); }
            if !raw.trim().is_empty() { metadata.insert(key.to_string(), Zeroizing::new(raw.trim().to_string())); }
        }
    }
    let saved = crate::server::core::user_agent::override_value(
        "doubaowork", row.user_agent().unwrap_or(""),
    );
    if !saved.is_empty() {
        // 账号网络设置是宿主选择，不要求用户手改已保存的旧浏览器UA副本。
        metadata.insert("userAgent".into(), Zeroizing::new(saved.to_string()));
    }
    let version = metadata.get("clientVersion").map(|value| value.as_str()).unwrap_or(CONTROL_VERSION);
    let ua = metadata.get("userAgent").map(|value| value.as_str()).unwrap_or(USER_AGENT);
    let user_agent = if ua == USER_AGENT { format!("{USER_AGENT} SamanthaDoubaoWork/{version}") } else { ua.to_string() };
    let mut cookie = Zeroizing::new(String::new());
    for (name, value) in &pairs {
        if !cookie.is_empty() { cookie.push_str("; "); }
        cookie.push_str(name); cookie.push('='); cookie.push_str(value.as_str());
        if cookie.len() > MAX_COOKIE_BYTES { return Err(DailyError::CredentialsInvalid); }
    }
    Ok(Credentials { cookie, pairs, metadata, user_agent: Zeroizing::new(user_agent) })
}

pub(in crate::server::core::plugin_host::wasm) fn is_numeric_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

impl Credentials {
    /// 只核对实际发送材料是否来自本次原行；该方法不授予对话或发送许可。
    pub(in crate::server::core::plugin_host::wasm) fn matches_turn_request(
        &self, url: &url::Url, headers: &Value,
    ) -> Result<String, DailyError> {
        let headers = headers.as_object().filter(|fields| fields.len() <= 128)
            .ok_or(DailyError::CredentialsInvalid)?;
        let mut cookie = headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case("cookie"));
        let raw = cookie.next().and_then(|(_, value)| value.as_str())
            .ok_or(DailyError::CredentialsInvalid)?;
        if cookie.next().is_some() { return Err(DailyError::CredentialsInvalid); }
        let mut pairs = BTreeMap::new();
        collect(&Value::String(raw.to_string()), crate::server::logging::now_ms(), 0, &mut pairs)?;
        if pairs != self.pairs { return Err(DailyError::SourceChanged); }
        let query: Vec<_> = url.query_pairs().collect();
        let saved_device = self.metadata.get("deviceId")
            .map(|value| value.as_str())
            .filter(|value| is_numeric_id(value))
            .unwrap_or("");
        let mut device_values = query.iter().filter(|(key, _)| key == "device_id");
        let actual_device = device_values.next().map(|(_, value)| value.as_ref());
        if device_values.next().is_some() { return Err(DailyError::CredentialsInvalid); }
        let effective_device = match actual_device {
            Some(actual) if !saved_device.is_empty() => {
                if actual != saved_device { return Err(DailyError::SourceChanged); }
                actual
            }
            Some(actual) if is_numeric_id(actual) => actual,
            _ => return Err(DailyError::CredentialsInvalid),
        };
        for (wire, field) in [("iid", "iid"), ("channel", "channel"),
            ("chromium_version", "chromiumVersion"), ("web_tab_id", "webTabId")] {
            let mut values = query.iter().filter(|(key, _)| key == wire);
            let actual = values.next().map(|(_, value)| value.as_ref());
            if values.next().is_some() { return Err(DailyError::CredentialsInvalid); }
            let expected = self.metadata.get(field).map(|value| value.as_str()).unwrap_or("");
            if actual.is_some_and(|actual| actual != expected) {
                return Err(DailyError::SourceChanged);
            }
        }
        for (wire, field) in [("web_id", "webId"), ("tea_uuid", "teaUuid")] {
            let mut values = query.iter().filter(|(key, _)| key == wire);
            let actual = values.next().map(|(_, value)| value.as_ref());
            if values.next().is_some() { return Err(DailyError::CredentialsInvalid); }
            let expected = self.metadata.get(field).map(|value| value.as_str()).unwrap_or("");
            if !expected.is_empty() {
                if actual.is_some_and(|actual| actual != expected)
                    || (wire == "web_id" && actual != Some(expected))
                {
                    return Err(DailyError::SourceChanged);
                }
            } else if actual.is_some_and(|actual| actual != effective_device) {
                return Err(DailyError::SourceChanged);
            }
        }
        let mut fingerprint = query.iter().filter(|(key, _)| key == "fp");
        let fp = fingerprint.next().map(|(_, value)| value.as_ref());
        if fingerprint.next().is_some() { return Err(DailyError::CredentialsInvalid); }
        let saved_fp = self.metadata.get("fingerprint").map(|value| value.as_str()).unwrap_or("");
        let derived = format!("verify_{effective_device}");
        let fp_ok = fp == Some(derived.as_str())
            || (!saved_fp.is_empty() && fp == Some(saved_fp));
        if !fp_ok {
            return Err(DailyError::SourceChanged);
        }
        Ok(effective_device.to_string())
    }

    pub(in crate::server::core::plugin_host::wasm) fn protected(&self, text: &str) -> bool {
        text.contains(self.cookie.as_str()) || self.pairs.iter().any(|(name, value)|
            !value.is_empty() && (matches!(name.as_str(), "sessionid" | "sessionid_ss") || value.len() >= 8)
                && text.contains(value.as_str()))
            || ["deviceId", "iid", "webId", "fingerprint", "teaUuid", "webTabId"].iter().any(|key|
                self.metadata.get(*key).is_some_and(|value| !value.is_empty() && text.contains(value.as_str())))
    }

    pub(in crate::server::core::plugin_host::wasm) fn url(&self, path: &'static str) -> String {
        self.url_with_device(path, None)
    }

    pub(in crate::server::core::plugin_host::wasm) fn url_with_device(
        &self, path: &'static str, fallback_device: Option<&str>,
    ) -> String {
        let field = |key: &str| self.metadata.get(key).map(|value| value.as_str()).unwrap_or("");
        let version = self.metadata.get("clientVersion").map(|value| value.as_str()).unwrap_or(CONTROL_VERSION);
        let mut query = vec![("aid", "1044603".to_string()), ("real_aid", "1044603".into()),
            ("device_platform", "web".into()), ("version_code", "20800".into()), ("samantha_web", "1".into()),
            ("web_platform", "desktop".into()), ("doubao_device_platform", "desktop".into()), ("client_platform", "pc_client".into()),
            ("pkg_type", "release_version".into()), ("runtime", "web".into()), ("runtime_version", "3.45.3".into()),
            ("pc_version", version.into()), ("doubao_pc_version", version.into()), ("use-olympus-account", "1".into()),
            ("language", "zh".into()), ("region", "CN".into()), ("sys_region", "CN".into())];
        let device = Some(field("deviceId")).filter(|value| is_numeric_id(value))
            .or_else(|| fallback_device.filter(|value| is_numeric_id(value)))
            .unwrap_or("");
        if !device.is_empty() { query.push(("device_id", device.to_string())); }
        let web_id = if !field("webId").is_empty() { field("webId") } else { device };
        if !web_id.is_empty() { query.push(("web_id", web_id.to_string())); }
        for (wire, key) in [("iid", "iid"), ("channel", "channel"),
            ("chromium_version", "chromiumVersion"), ("tea_uuid", "teaUuid"), ("web_tab_id", "webTabId")] {
            if !field(key).is_empty() { query.push((wire, field(key).to_string())); }
        }
        if !field("fingerprint").is_empty() { query.push(("fp", field("fingerprint").to_string())); }
        else if !device.is_empty() { query.push(("fp", format!("verify_{device}"))); }
        let encoded = query.into_iter().map(|(key, value)| format!("{key}={}", encode(&value))).collect::<Vec<_>>().join("&");
        format!("{ORIGIN}{path}?{encoded}")
    }

    pub(in crate::server::core::plugin_host::wasm) fn headers(&self, ug: bool) -> Result<HeaderMap, DailyError> {
        let value = |text: &str| HeaderValue::from_str(text).map_err(|_| DailyError::CredentialsInvalid);
        let mut headers = HeaderMap::new();
        let mut cookie = value(self.cookie.as_str())?; cookie.set_sensitive(true);
        headers.insert("cookie", cookie);
        headers.insert("origin", HeaderValue::from_static(ORIGIN));
        headers.insert("referer", value(&format!("{ORIGIN}{}", if ug { "/pc/agent-explore" } else { "/chat/" }))?);
        headers.insert("user-agent", value(self.user_agent.as_str())?);
        headers.insert("agw-js-conv", HeaderValue::from_static("str"));
        headers.insert("accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        Ok(headers)
    }
}

fn target(value: &Value) -> Result<(), DailyError> {
    for key in ["provider", "edition"] {
        if value.get(key).is_some_and(|value| !value.is_null() && value.as_str() != Some("doubaowork")) { return Err(DailyError::CredentialsInvalid); }
    }
    if value.get("baseUrl").is_some_and(|value| !value.is_null() && value.as_str().is_none_or(|text| text.trim_end_matches('/') != ORIGIN)) {
        return Err(DailyError::CredentialsInvalid);
    }
    Ok(())
}

fn collect(value: &Value, now: i64, depth: usize, pairs: &mut BTreeMap<String, Zeroizing<String>>) -> Result<(), DailyError> {
    if depth > 8 { return Err(DailyError::CredentialsInvalid); }
    target(value)?;
    if let Value::Object(fields) = value {
        if fields.len() > MAX_COOKIES { return Err(DailyError::CredentialsInvalid); }
        let count = ["cookies", "cookieHeader", "cookie", "token"].iter()
            .filter(|key| fields.get(**key).is_some_and(|value| !value.is_null())).count();
        if count > 1 { return Err(DailyError::CredentialsInvalid); }
    }
    let source = ["cookies", "cookieHeader", "cookie", "token"].iter()
        .find_map(|key| value.get(*key).filter(|value| !value.is_null())).unwrap_or(value);
    if source.is_object() && ["cookies", "cookieHeader", "cookie", "token"].iter().any(|key| source.get(*key).is_some_and(|value| !value.is_null())) {
        return collect(source, now, depth + 1, pairs);
    }
    match source {
        Value::String(raw) => {
            if raw.len() > MAX_SECRET || raw.chars().any(char::is_control) { return Err(DailyError::CredentialsInvalid); }
            let raw = raw.trim();
            if raw.starts_with('{') || raw.starts_with('[') {
                let parsed = json_text::parse(raw.as_bytes(), MAX_SECRET).map_err(|_| DailyError::CredentialsInvalid)?;
                return collect(&parsed, now, depth + 1, pairs);
            }
            let raw = raw.strip_prefix("Cookie:").unwrap_or(raw).trim();
            if raw.len() > MAX_COOKIE_BYTES || raw.chars().any(char::is_control) { return Err(DailyError::CredentialsInvalid); }
            for item in raw.split(';').map(str::trim).filter(|item| !item.is_empty()) {
                let (name, value) = item.split_once('=').ok_or(DailyError::CredentialsInvalid)?;
                put(pairs, name.trim(), value.trim())?;
            }
        }
        Value::Array(rows) => {
            if rows.len() > MAX_COOKIES { return Err(DailyError::CredentialsInvalid); }
            for row in rows {
                let row = row.as_object().ok_or(DailyError::CredentialsInvalid)?;
                if let Some(domain) = row.get("domain").filter(|value| !value.is_null()) {
                    if !matches!(domain.as_str().ok_or(DailyError::CredentialsInvalid)?.trim_start_matches('.'), "doubao.com" | "www.doubao.com") { continue; }
                }
                if let Some(path) = row.get("path").filter(|value| !value.is_null()) {
                    if path.as_str().ok_or(DailyError::CredentialsInvalid)? != "/" { continue; }
                }
                if let Some(expires) = row.get("expires").or_else(|| row.get("expirationDate")).filter(|value| !value.is_null()) {
                    let expires = expires.as_f64().filter(|value| value.is_finite()).ok_or(DailyError::CredentialsInvalid)?;
                    if expires > 0.0 && expires <= (now / 1000) as f64 { continue; }
                }
                put(pairs, row.get("name").and_then(Value::as_str).ok_or(DailyError::CredentialsInvalid)?,
                    row.get("value").and_then(Value::as_str).ok_or(DailyError::CredentialsInvalid)?)?;
            }
        }
        Value::Object(fields) => {
            if let (Some(name), Some(value)) = (fields.get("name").and_then(Value::as_str), fields.get("value").and_then(Value::as_str)) {
                return put(pairs, name, value);
            }
            for (name, value) in fields {
                if META_KEYS.contains(&name.as_str()) { continue; }
                put(pairs, name, value.as_str().ok_or(DailyError::CredentialsInvalid)?)?;
            }
        }
        _ => return Err(DailyError::CredentialsInvalid),
    }
    Ok(())
}

fn put(pairs: &mut BTreeMap<String, Zeroizing<String>>, name: &str, value: &str) -> Result<(), DailyError> {
    if name.is_empty() || name.len() > 256 || value.len() > 16 * 1024
        || !name.bytes().all(|byte| byte.is_ascii_graphic() && !b"=;,\"\\".contains(&byte))
        || value.chars().any(|ch| ch.is_control() || ch == ';') { return Err(DailyError::CredentialsInvalid); }
    if pairs.get(name).is_some_and(|previous| previous.as_str() != value) { return Err(DailyError::CredentialsInvalid); }
    if pairs.len() >= MAX_COOKIES && !pairs.contains_key(name) { return Err(DailyError::CredentialsInvalid); }
    let previous = pairs.get(name).map(|value| value.len() + name.len() + 3).unwrap_or(0);
    let total: usize = pairs.iter().map(|(name, value)| name.len() + value.len() + 3).sum();
    if total.saturating_sub(previous).saturating_add(name.len() + value.len() + 3) > MAX_COOKIE_BYTES {
        return Err(DailyError::CredentialsInvalid);
    }
    pairs.insert(name.to_string(), Zeroizing::new(value.to_string()));
    Ok(())
}

fn encode(text: &str) -> String {
    let mut output = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { output.push(char::from(byte)); }
        else { output.push_str(&format!("%{byte:02X}")); }
    }
    output
}
