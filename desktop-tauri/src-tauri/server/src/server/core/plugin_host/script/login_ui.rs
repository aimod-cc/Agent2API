use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginUi {
    browser_mode: String,
    manual_callback: bool,
    callback_input: String,
    cookie_capture: Option<String>,
    #[serde(default)]
    capture_callback: Option<bool>,
    #[serde(default)]
    loopback_targets: Vec<String>,
}

struct Entry {
    provider: String,
    at: i64,
    ui: LoginUi,
}

fn pending() -> &'static Mutex<HashMap<String, Entry>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn remember(provider: &str, flow: &str, value: &Value) -> Result<(), String> {
    let mut ui: LoginUi = serde_json::from_value(value.clone()).map_err(|_| "脚本登录窗口信息无效")?;
    if !matches!(ui.browser_mode.as_str(), "choice" | "external" | "plugin" | "none")
        || !matches!(ui.callback_input.as_str(), "url" | "code")
        || ui.callback_input == "code" && !ui.manual_callback
        || ui.cookie_capture.as_deref().is_some_and(|id| provider != "wps-lingxi" || id != provider)
        || ui.loopback_targets.len() > 8
    {
        return Err("脚本登录窗口契约不兼容".into());
    }
    for target in &mut ui.loopback_targets {
        let mut url = url::Url::parse(target).map_err(|_| "脚本登录本机回调地址无效")?;
        if url.scheme() != "http" || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            || url.port().is_none() || !url.username().is_empty() || url.password().is_some()
        {
            return Err("脚本登录回调不属于本机接收端".into());
        }
        url.set_query(None);
        url.set_fragment(None);
        *target = url.to_string();
    }
    let now = crate::server::logging::now_ms();
    let mut entries = pending().lock().map_err(|_| "脚本登录窗口信息不可用")?;
    entries.retain(|_, entry| now - entry.at < 60_000);
    entries.insert(flow.into(), Entry { provider: provider.into(), at: now, ui });
    Ok(())
}

pub(crate) fn decorate(provider: &str, flow: &str, response: &mut Value) {
    let entry = pending().lock().ok().and_then(|mut entries| entries.remove(flow));
    let Some(entry) = entry.filter(|entry| entry.provider == provider) else { return };
    response["browserMode"] = json!(entry.ui.browser_mode);
    response["manualCallback"] = json!(entry.ui.manual_callback);
    response["callbackInput"] = json!(entry.ui.callback_input);
    response["cookieCapture"] = json!(entry.ui.cookie_capture);
    if let Some(capture) = entry.ui.capture_callback { response["captureCallback"] = json!(capture); }
    response["loopbackTargets"] = json!(entry.ui.loopback_targets);
}
