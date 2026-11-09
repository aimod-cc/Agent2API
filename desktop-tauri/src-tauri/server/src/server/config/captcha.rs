use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use super::{
    RuntimeConfig, CONFIG, CONFIG_UPDATE_LOCK, KEY_CAPTCHA_DESKTOP, KEY_CAPTCHA_ENABLED,
    KEY_CAPTCHA_WEB,
};
use crate::server::core::crypto::{self, SecretScope};

pub(crate) const KEY_PANEL_CAPTCHA: &str = "panelCaptcha";
const SCOPE: SecretScope = SecretScope::PanelCaptcha;

#[derive(Clone)]
pub(crate) struct CaptchaSettings {
    pub provider: String,
    pub site_key: String,
    pub hostnames: Vec<String>,
    secret_key: String,
}

impl CaptchaSettings {
    pub fn from_config(config: &RuntimeConfig) -> Self {
        let raw = config.raw().get(KEY_PANEL_CAPTCHA);
        let text = |key: &str| {
            raw.and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        Self {
            provider: raw
                .and_then(|value| value.get("provider"))
                .and_then(Value::as_str)
                .unwrap_or("altcha")
                .to_string(),
            site_key: text("siteKey"),
            secret_key: text("secretKey"),
            hostnames: raw
                .and_then(|value| value.get("hostnames"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn secret(&self) -> Result<Zeroizing<String>, String> {
        if self.secret_key.is_empty() {
            return Err("请填写 Turnstile 服务端密钥".into());
        }
        let key = crypto::existing_key_bytes(SCOPE)
            .ok_or("Turnstile 密钥无法解密，请在机器人校验设置中重新保存服务端密钥")?;
        let bytes = Zeroizing::new(
            crypto::open_with_key(&self.secret_key, &key)
                .map_err(|_| "Turnstile 密钥无法解密，请在机器人校验设置中重新保存服务端密钥")?,
        );
        String::from_utf8(bytes.to_vec())
            .map(Zeroizing::new)
            .map_err(|_| "Turnstile 服务端密钥格式无效".into())
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.provider.as_str() {
            "altcha" => Ok(()),
            "turnstile" => {
                validate_key(&self.site_key, "Turnstile 站点密钥")?;
                if self.hostnames.is_empty() {
                    return Err("请填写 Turnstile 允许的域名".into());
                }
                for hostname in &self.hostnames {
                    normalize_hostname(hostname)?;
                }
                validate_key(&self.secret()?, "Turnstile 服务端密钥")
            }
            _ => Err("机器人校验方式无效，请选择 ALTCHA 或 Cloudflare Turnstile".into()),
        }
    }

    pub fn public_json(&self, enabled: bool) -> Value {
        json!({
            "captchaEnabled": enabled,
            "captchaProvider": self.provider,
            "turnstileSiteKey": if enabled && self.provider == "turnstile" { Some(&self.site_key) } else { None },
            "captchaError": if enabled { self.validate().err().map(|_| "机器人校验配置不完整，请联系管理员") } else { None },
        })
    }

    pub fn management_json(&self, config: &RuntimeConfig, local_panel: bool) -> Value {
        let (desktop, web) = config.captcha_settings();
        json!({
            "captchaDesktop": desktop, "captchaWeb": web, "localPanel": local_panel,
            "captchaProvider": self.provider, "turnstileSiteKey": self.site_key,
            "turnstileSecretSet": !self.secret_key.is_empty(), "turnstileHostnames": self.hostnames,
            "error": self.validate().err(),
        })
    }
}

fn validate_key(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("请填写{label}"));
    }
    if value.len() > 512
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("{label}格式无效"));
    }
    Ok(())
}

pub(crate) fn normalize_hostname(input: &str) -> Result<String, String> {
    let value = input.trim().trim_end_matches('.');
    if value.is_empty() || value.len() > 253 || value.contains(['/', ':', '*', '?', '#', '@']) {
        return Err("允许的域名不能包含协议、端口、路径或通配符".into());
    }
    let host = url::Host::parse(value)
        .map_err(|_| "允许的域名格式无效")?
        .to_string()
        .to_ascii_lowercase();
    if !host.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && !part.starts_with('-')
            && !part.ends_with('-')
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err("允许的域名格式无效".into());
    }
    Ok(host)
}

fn bool_field(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(format!("{key} 必须是布尔值")),
    }
}

pub(crate) fn save(object: &Map<String, Value>) -> Result<(), (i32, String)> {
    let invalid = |message: String| (400, message);
    let legacy = bool_field(object, KEY_CAPTCHA_ENABLED).map_err(invalid)?;
    let desktop = bool_field(object, KEY_CAPTCHA_DESKTOP)
        .map_err(invalid)?
        .or(legacy);
    let web = bool_field(object, KEY_CAPTCHA_WEB)
        .map_err(invalid)?
        .or(legacy);
    let fields = [
        "captchaProvider",
        "turnstileSiteKey",
        "turnstileSecretKey",
        "turnstileHostnames",
    ];
    if desktop.is_none() && web.is_none() && !fields.iter().any(|key| object.contains_key(*key)) {
        return Err((400, "未提供机器人校验设置".into()));
    }
    let _guard = CONFIG_UPDATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut next = super::current();
    let before = super::synced_fingerprint(&next.raw);
    let mut settings = CaptchaSettings::from_config(&next);
    for (name, target) in [
        ("captchaProvider", &mut settings.provider),
        ("turnstileSiteKey", &mut settings.site_key),
    ] {
        if let Some(value) = object.get(name) {
            *target = value
                .as_str()
                .ok_or_else(|| (400, format!("{name} 必须是字符串")))?
                .trim()
                .to_string();
        }
    }
    if let Some(value) = object.get("turnstileHostnames") {
        let values = value
            .as_array()
            .filter(|items| items.len() <= 32)
            .ok_or_else(|| (400, "允许的域名必须是数组，最多 32 项".into()))?;
        settings.hostnames.clear();
        for value in values {
            let hostname = normalize_hostname(
                value
                    .as_str()
                    .ok_or_else(|| (400, "允许的域名必须是字符串".into()))?,
            )
            .map_err(invalid)?;
            if !settings.hostnames.contains(&hostname) {
                settings.hostnames.push(hostname);
            }
        }
    }
    if let Some(value) = object.get("turnstileSecretKey") {
        match value {
            Value::Null => settings.secret_key.clear(),
            Value::String(value) if value.trim().is_empty() => {}
            Value::String(value) => {
                let secret = Zeroizing::new(value.trim().to_string());
                validate_key(&secret, "Turnstile 服务端密钥").map_err(invalid)?;
                settings.secret_key =
                    crypto::seal(secret.as_bytes(), SCOPE).map_err(|error| (500, error))?;
            }
            _ => return Err((400, "Turnstile 服务端密钥必须是字符串或 null".into())),
        }
    }
    let changing_provider = fields.iter().any(|key| object.contains_key(*key));
    if changing_provider || desktop == Some(true) || web == Some(true) {
        settings.validate().map_err(invalid)?;
    }
    if let Some(value) = desktop {
        next.captcha_desktop = value;
        next.raw.insert(KEY_CAPTCHA_DESKTOP.into(), json!(value));
    }
    if let Some(value) = web {
        next.captcha_web = value;
        next.raw.insert(KEY_CAPTCHA_WEB.into(), json!(value));
    }
    if changing_provider {
        next.raw.insert(
            KEY_PANEL_CAPTCHA.into(),
            json!({
                "provider": settings.provider, "siteKey": settings.site_key,
                "secretKey": settings.secret_key, "hostnames": settings.hostnames,
            }),
        );
    }
    // 登录门槛只有在持久化成功后切换，避免保存失败却已把用户锁在新配置外。
    if !super::save_raw(&next.raw) {
        return Err((500, "写入配置失败，请重试".into()));
    }
    let changed = before != super::synced_fingerprint(&next.raw);
    *CONFIG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(next);
    if changed {
        crate::server::core::cloud_sync::auto_sync::notify_change();
    }
    Ok(())
}
