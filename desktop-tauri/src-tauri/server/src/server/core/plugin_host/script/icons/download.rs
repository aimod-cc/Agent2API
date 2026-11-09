use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private() && !ip.is_loopback() && !ip.is_link_local() && !ip.is_multicast()
                && !ip.is_broadcast() && !ip.is_documentation() && a > 0 && a < 224
                && !(a == 100 && (64..=127).contains(&b)) && !(a == 198 && matches!(b, 18 | 19))
                && !(a == 192 && b == 0 && c == 0)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() { return public_ip(IpAddr::V4(v4)); }
            let parts = ip.segments();
            parts[0] & 0xe000 == 0x2000 && parts[0] != 0x2002
                && !(parts[0] == 0x2001 && (parts[1] < 0x0200 || parts[1] == 0x0db8))
        }
    }
}

async fn request(value: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut url = url::Url::parse(value).map_err(|_| "图标地址无效")?;
    let configured = crate::server::config::update_proxy();
    let proxy = match configured.as_ref() {
        None => None,
        Some(config) if config.is_null() || config["source"] == "direct" => None,
        Some(config) => {
            use crate::server::core::proxies::{resolve_account_proxy, ProxyResolution};
            match resolve_account_proxy(Some(config)) {
                Some(ProxyResolution::Resolved(proxy)) => Some(proxy),
                _ => return Err("图标下载出口不可用".into()),
            }
        }
    };
    for _ in 0..=4 {
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err("图标仅支持不含登录凭据的 HTTPS 地址".into());
        }
        let host = url.host_str().ok_or("图标地址缺少域名")?.trim_matches(['[', ']']);
        if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".localhost") {
            return Err("图标地址不能指向本机或内网".into());
        }
        let port = url.port_or_known_default().ok_or("图标地址端口无效")?;
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await
            .map_err(|_| "图标域名解析失败")?.collect();
        if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
            return Err("图标地址不能指向本机、内网或保留地址".into());
        }
        let client = crate::server::core::egress::image_download_client(proxy.as_ref(), false, host, &addresses)?;
        let mut response = client.get(url.clone()).timeout(Duration::from_secs(15))
            .header("user-agent", "Agent2API-PluginIcons/1.0").header("accept", "image/*, application/json;q=0.9, */*;q=0.5")
            .send().await.map_err(|_| "图标下载失败")?;
        if response.status().is_redirection() {
            let location = response.headers().get("location").and_then(|value| value.to_str().ok())
                .ok_or("图标跳转缺少目标地址")?;
            url = url.join(location).map_err(|_| "图标跳转地址无效")?;
            continue;
        }
        if !response.status().is_success() { return Err(format!("图标下载返回 HTTP {}", response.status().as_u16())); }
        if response.content_length().is_some_and(|size| size > limit as u64) { return Err("图标资源超过大小限制".into()); }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "图标下载中断")? {
            if bytes.len().saturating_add(chunk.len()) > limit { return Err("图标资源超过大小限制".into()); }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() { return Err("图标资源为空".into()); }
        return Ok(bytes);
    }
    Err("图标地址跳转次数过多".into())
}

pub(super) async fn bytes(value: &str, limit: usize) -> Result<Vec<u8>, String> {
    tokio::time::timeout(Duration::from_secs(30), request(value, limit)).await
        .map_err(|_| "图标获取超时".to_string())?
}
