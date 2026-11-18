use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{authorized, denied, digest, path, Declaration};
use super::super::context::Context;
use super::super::super::script;

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private() && !ip.is_loopback() && !ip.is_link_local() && !ip.is_multicast()
                && !ip.is_broadcast() && !ip.is_documentation() && a > 0 && a < 224
                && !(a == 100 && (64..=127).contains(&b)) && !(a == 198 && matches!(b, 18 | 19))
                && !(a == 192 && b == 0 && c == 0) && !(a == 192 && b == 88 && c == 99)
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() { return public_ip(IpAddr::V4(ip)); }
            let parts = ip.segments();
            parts[0] & 0xe000 == 0x2000 && parts[0] != 0x2002
                && !(parts[0] == 0x2001 && (parts[1] < 0x0200 || parts[1] == 0x0db8))
                && parts[0] != 0x3ffe && !(parts[0] == 0x3fff && parts[1] & 0xf000 == 0)
        }
    }
}

pub(in super::super) async fn addresses(url: &url::Url) -> Result<Vec<SocketAddr>, proto::WireError> {
    let hostname = url.host_str().ok_or_else(|| denied("账号来源缺少服务器域名"))?;
    let addresses = tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host((hostname, 443))).await
        .map_err(|_| proto::WireError::with_code("账号服务器域名解析超时", "timeout"))?
        .map_err(|_| denied("账号服务器域名解析失败"))?;
    let mut addresses: Vec<_> = addresses.collect();
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(denied("账号服务器域名包含本机、私网或保留地址，已停止连接"));
    }
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.len() > 64 { return Err(denied("账号服务器域名返回地址过多")); }
    Ok(addresses)
}

fn bad_params() -> proto::WireError {
    proto::WireError::with_code("账号来源请求仅接受 method、path、headers、可选正文和 timeout_ms，不接受地址、账号或代理参数", "bad_params")
}

impl Context {
    async fn origin_record(&self, declaration: &Declaration) -> Result<Value, proto::WireError> {
        let account = self.account()?;
        let mut record = self.shared.callbacks.store_get(self.shared.options.provider.clone(), account.to_string()).await
            .map_err(|_| denied("当前账号不可用，未访问其他账号来源"))?;
        let original_digest = digest(&record["secret"]).map_err(|_| denied("账号凭据快照不可用"))?;
        super::super::legacy_secret::project(&mut record);
        let binding = self.origin_binding.as_ref().ok_or_else(|| denied("当前调用没有可信账号来源快照"))?;
        if record["id"].as_str() != Some(binding.account.as_str())
            || record["provider"].as_str() != Some(self.shared.options.provider.as_str())
            || record["identity"].as_str() != Some(binding.identity.as_str())
            || record["public"]["enabled"] == false && !self.account_maintenance
            || record["secret"].get(&declaration.secret_field) != binding.source.as_ref()
            || original_digest != binding.credential_digest {
            return Err(denied("账号身份、服务器来源或凭据已变化，请重新读取账号后重试"));
        }
        Ok(record)
    }

    pub(in super::super) async fn open_account_origin(&mut self, params: &Value, deadline: script::HttpDeadline) -> Result<Value, proto::WireError> {
        deadline.check()?;
        self.permission(super::CAPABILITY)?;
        self.permission("http.v1")?;
        self.permission("accounts.cas.v1")?;
        let declaration = self.shared.options.manifest.account_origin.clone()
            .ok_or_else(|| denied("插件未声明账号服务器来源"))?;
        let object = params.as_object().ok_or_else(bad_params)?;
        if object.keys().any(|key| !matches!(key.as_str(), "method" | "path" | "headers" | "body" | "timeout_ms")) {
            return Err(bad_params());
        }
        let method = params["method"].as_str().ok_or_else(bad_params)?;
        if self.usage_query && method != "GET" {
            return Err(denied("余额查询仅允许账号服务器的只读请求"));
        }
        let request_path = params["path"].as_str().filter(|value| path(value)).ok_or_else(bad_params)?;
        if !declaration.routes.iter().any(|route| route.method == method && route.path == request_path) {
            return Err(denied("账号来源请求的方法或路径不在已授权清单中"));
        }
        let headers = params["headers"].as_object().filter(|headers| headers.len() <= 128)
            .ok_or_else(bad_params)?;
        if headers.iter().any(|(name, value)| reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err()
            || value.as_str().is_none_or(|value| reqwest::header::HeaderValue::from_str(value).is_err())) {
            return Err(bad_params());
        }
        if params.get("body").is_some_and(|body| !body.is_null() && body.as_str().is_none_or(|body| body.len() > super::super::MAX_MESSAGE)) {
            return Err(bad_params());
        }
        if self.resources.len() >= 8 { return Err(denied("单次插件调用连接数超限")); }
        authorized(self, &declaration)?;
        deadline.check()?;
        let record = deadline.run(self.origin_record(&declaration)).await?;
        let source = declaration.source(&record["secret"])?;
        let mut url = source.clone();
        url.set_path(request_path);
        let addresses = deadline.run(addresses(&url)).await?;
        let proxy = crate::server::core::proxies::ResolvedProxy::from_json(&self.proxy)
            .map_err(|_| denied("当前账号代理不可用，未改走直连"))?;
        let client = crate::server::core::egress::resolved_client(proxy.as_ref(), self.system_proxy_when_unset, url.host_str().unwrap_or(""), &addresses)
            .map_err(|_| denied("无法创建受控账号服务器连接"))?;
        deadline.check()?;
        // DNS/客户端准备后重新读取当前账号；不拿旧凭据给新身份或新服务器发送。
        deadline.run(self.origin_record(&declaration)).await?;
        authorized(self, &declaration)?;
        deadline.check()?;
        if self.shared.cancel.is_cancelled() { return Err(proto::WireError::with_code("插件调用已结束", proto::WireError::ABORTED)); }
        let id = format!("wasm-{}", crate::server::core::protocol::random_id("net"));
        let mut params = params.clone();
        if let Some(object) = params.as_object_mut() { object.remove("path"); }
        params["url"] = json!(url.as_str());
        params["id"] = json!(id);
        params["provider"] = json!(self.shared.options.provider);
        params["trace"] = self.trace.clone();
        params["max_chunk"] = json!(64 * 1024);
        let ordinary = super::super::managed_task::OrdinaryPolicy::from_options(&self.shared.options)?;
        super::super::http::guard_ordinary(ordinary, &params)?;
        super::super::http::guard_token_exchange(self, &params)?;
        let user_agent = crate::server::core::user_agent::override_value(
            &self.shared.options.provider, self.user_agent.as_deref().unwrap_or(""),
        );
        if !user_agent.is_empty() {
            let headers = params["headers"].as_object_mut().ok_or_else(bad_params)?;
            headers.retain(|key, _| !key.eq_ignore_ascii_case("user-agent"));
            headers.insert("User-Agent".into(), json!(user_agent));
        }
        deadline.check()?;
        let (result, handle) = script::open_pinned(&params, client, deadline).await?;
        self.keep_http(result, handle, deadline)
    }
}
