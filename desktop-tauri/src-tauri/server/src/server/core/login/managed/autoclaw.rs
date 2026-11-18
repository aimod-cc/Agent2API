use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{Call, unavailable};
use crate::server::{core::plugin_host::wasm::manifest::Options, errors::GatewayError};

pub(super) async fn authorize(call: &Call, params: &Value, options: &Options) -> Result<Value, GatewayError> {
    call.validate("autoclaw-intl")?;
    if call.method_index() != 0 || !call.is_captcha() || !options.allows("login.captcha.aliyun.v1")
        || params.as_object().is_none_or(|fields| fields.len() != 3
            || fields.keys().any(|key| !matches!(key.as_str(), "vendor" | "leaseId" | "deviceId"))) || params["vendor"] != "zai" {
        return Err(unavailable());
    }
    let lease_id = params["leaseId"].as_str().ok_or_else(unavailable)?;
    let device = params["deviceId"].as_str().filter(|value| value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())).ok_or_else(unavailable)?;
    let callback = {
        let state = call.session.state.lock().map_err(|_| unavailable())?;
        let lease = state.leases.get(lease_id).ok_or_else(unavailable)?;
        if lease.closed() || lease.expected != call.expected() || lease.profile != "autoclaw-intl.zai.v1"
            || lease.authorization_url.is_some() { return Err(unavailable()); }
        lease.uri.clone()
    };
    let input = call.take_input()?;
    let captcha = input["captcha"]["param"].as_str().filter(|value| !value.is_empty() && value.len() <= 32 * 1024)
        .ok_or_else(unavailable)?;
    let url = "https://autoglm-api.autoglm.ai/userapi/overseasv1/zai-oauth-url";
    options.check_url(url, false).map_err(|_| unavailable())?;
    let params = call.params()?;
    let proxy = crate::server::core::proxies::ResolvedProxy::from_json(&params["proxy"]).map_err(|_| unavailable())?;
    let client = crate::server::core::egress::managed::FrozenHttpClient::from_selected(proxy).await.map_err(|_| unavailable())?;
    let now = (crate::server::logging::now_ms() / 1000).to_string();
    use md5::{Digest, Md5};
    let sign = format!("{:x}", Md5::digest(format!("100003&{now}&38d2391985e2369a5fb8227d8e6cd5e5")));
    let work = async {
        let mut response = client.client().map_err(|_| unavailable())?.post(url)
            .header("X-Auth-Appid", "100003").header("X-Auth-TimeStamp", now).header("X-Auth-Sign", sign)
            .header("X-Product", "autoclaw").header("X-Version", "1.18.5").header("X-Client-Type", "pc")
            .header("Accept", "application/json")
            .json(&json!({ "source_id": "autoclaw", "device_id": device, "navigate_uri": callback,
                "ali_captcha_verify_param": captcha })).send().await.map_err(|_| unavailable())?;
        if !response.status().is_success() || response.url().as_str() != url { return Err(unavailable()); }
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len().saturating_add(chunk.len()) > 128 * 1024 { return Err(unavailable()); }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = crate::server::core::plugin_host::wasm::managed_task::declaration::parse_object(&bytes, 128 * 1024)
            .map_err(|_| unavailable())?;
        if value["code"] != 0 { return Err(GatewayError::with_status(502, "国际授权地址未取得，请重新完成官方验证")); }
        let raw = value["data"]["oauth_url"].as_str().filter(|raw| raw.len() <= 16 * 1024).ok_or_else(unavailable)?;
        let parsed = url::Url::parse(raw).map_err(|_| unavailable())?;
        if parsed.scheme() != "https" || parsed.port_or_known_default() != Some(443)
            || !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some()
            || parsed.host_str().is_none_or(|host| host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.')) {
            return Err(unavailable());
        }
        let mut pairs = BTreeMap::new();
        for (key, value) in parsed.query_pairs() {
            if pairs.insert(key.into_owned(), value.into_owned()).is_some() { return Err(unavailable()); }
        }
        let state = pairs.get("state").filter(|value| !value.is_empty() && value.len() <= 4096
            && !value.chars().any(|ch| ch.is_control() || ch.is_whitespace())).ok_or_else(unavailable)?;
        if pairs.get("redirect_uri") != Some(&callback) || pairs.get("response_type").is_some_and(|value| value != "code")
            || value["data"].get("state").is_some_and(|value| value.as_str() != Some(state.as_str())) {
            return Err(unavailable());
        }
        Ok((raw.to_string(), state.clone()))
    };
    let (url, state) = tokio::time::timeout_at(call.deadline(), work).await.map_err(|_| unavailable())??;
    call.validate("autoclaw-intl")?;
    let mut current = call.session.state.lock().map_err(|_| unavailable())?;
    let lease = current.leases.get_mut(lease_id).ok_or_else(unavailable)?;
    if lease.closed() || lease.authorization_url.is_some() || lease.uri != callback || lease.expected != call.expected() {
        return Err(unavailable());
    }
    lease.authorization_url = Some(url.clone());
    lease.official_state = Some(state.clone());
    Ok(json!({ "url": url, "officialState": state }))
}
