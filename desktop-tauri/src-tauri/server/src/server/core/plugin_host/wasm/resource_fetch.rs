//! 公网图片下载只产出本次调用的资源，不借用账号、插件请求头或代理。
//! DNS 结果逐跳校验并固定到连接；所有下载错误均不回显 URL、响应正文或底层错误。

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE, LOCATION};
use url::Url;

use super::managed_task::OrdinaryPolicy;

pub(super) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const MAX_URL_BYTES: usize = 8192;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Image {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

fn invalid(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, "bad_params")
}

fn forbidden() -> proto::WireError {
    proto::WireError::with_code("公共图片地址不能指向本机、内网、保留地址或未授权端口", "forbidden")
}

fn limit(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, "plugin_resource_limit")
}

fn reference(value: &str) -> Result<(), proto::WireError> {
    if value.is_empty() || value.len() > MAX_URL_BYTES
        || value.chars().any(|ch| ch.is_control() || ch.is_whitespace()) || value.contains('\\') {
        return Err(invalid("公共图片地址无效或超过长度限制"));
    }
    // URL 解析器会消去空 userinfo，并容忍缺失或多余的斜杠；先拒绝这些歧义形式。
    if value.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("https:"))
        && !value.get(6..).is_some_and(|rest| rest.starts_with("//")) {
        return Err(invalid("公共图片地址格式无效"));
    }
    let authority = value.strip_prefix("//").or_else(|| value.split_once("://").map(|(_, rest)| rest));
    if authority.is_some_and(|rest| rest.split(['/', '?', '#']).next()
        .is_none_or(|part| part.is_empty() || part.contains('@'))) {
        return Err(invalid("公共图片地址不能包含登录凭据或空主机"));
    }
    Ok(())
}

fn checked_url(url: &Url) -> Result<(), proto::WireError> {
    reference(url.as_str())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
        || url.fragment().is_some() {
        return Err(invalid("公共图片仅支持不含登录凭据或片段的 HTTPS 地址"));
    }
    if url.port_or_known_default() != Some(443) { return Err(forbidden()); }
    let host = url.host_str().ok_or_else(|| invalid("公共图片地址缺少主机"))?
        .trim_matches(['[', ']']).trim_end_matches('.');
    if host.is_empty() || host.eq_ignore_ascii_case("localhost")
        || [".localhost", ".local", ".internal", ".home.arpa"].iter().any(|suffix| host.ends_with(*suffix))
        || host.parse::<IpAddr>().is_err() && !host.contains('.') {
        return Err(forbidden());
    }
    Ok(())
}

fn public_ip(address: IpAddr) -> bool {
    // 与已有图片下载的公网策略一致，并封闭特殊用途地址；不开放转换/隧道前缀。
    // 特殊用途依据 IANA registries / RFC 6890、7526、9637。
    match address {
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

async fn addresses(url: &Url) -> Result<(String, Vec<SocketAddr>), proto::WireError> {
    checked_url(url)?;
    let host = url.host_str().ok_or_else(|| invalid("公共图片地址缺少主机"))?
        .trim_matches(['[', ']']).to_string();
    let mut addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, 443)]
    } else {
        tokio::net::lookup_host((host.as_str(), 443)).await
            .map_err(|_| super::error("公共图片域名解析失败"))?.collect::<Vec<_>>()
    };
    // 不能先过滤或截取 DNS 答案；混入任何非公网地址时整次请求均拒绝。
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(forbidden());
    }
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.len() > 64 { return Err(super::error("公共图片域名返回的地址过多")); }
    Ok((host, addresses))
}

fn mime(headers: &reqwest::header::HeaderMap) -> Result<&'static str, proto::WireError> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let value = values.next().and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next()).map(str::trim).unwrap_or("");
    if values.next().is_some() { return Err(super::error("公共图片响应包含冲突的内容类型")); }
    if value.eq_ignore_ascii_case("image/png") { Ok("image/png") }
    else if value.eq_ignore_ascii_case("image/jpeg") { Ok("image/jpeg") }
    else if value.eq_ignore_ascii_case("image/webp") { Ok("image/webp") }
    else { Err(super::error("公共下载仅支持 PNG、JPEG 或 WebP 图片")) }
}

fn check_content(bytes: &[u8], mime: &str) -> Result<(), proto::WireError> {
    let actual = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        Ok(image::ImageFormat::WebP) if bytes.get(8..12) == Some(b"WEBP") => "image/webp",
        _ => return Err(super::error("公共下载内容不是支持的图片编码")),
    };
    if actual != mime { return Err(super::error("公共图片的实际编码与响应内容类型不一致")); }
    Ok(())
}

pub(super) async fn download(url: &str, remaining: usize, downloaded: &mut usize,
    ordinary: OrdinaryPolicy) -> Result<Image, proto::WireError> {
    if *downloaded >= MAX_TOTAL_BYTES { return Err(limit("单次调用公共图片下载累计超过 64 MiB")); }
    if remaining == 0 { return Err(limit("单次调用资源总量超过 96 MiB")); }
    reference(url)?;
    ordinary.check_public_url(url)?;
    let url = Url::parse(url).map_err(|_| invalid("公共图片地址格式无效"))?;
    // 不 spawn：宿主取消/关闭实例会直接 drop 此 future 和响应体；预算按已读取字节立即记账。
    tokio::time::timeout(DOWNLOAD_TIMEOUT, request(url, remaining, downloaded, ordinary)).await
        .map_err(|_| proto::WireError::with_code("公共图片下载超时", "timeout"))?
}

async fn request(mut url: Url, remaining: usize, downloaded: &mut usize,
    ordinary: OrdinaryPolicy) -> Result<Image, proto::WireError> {
    let _network = crate::server::core::runtime_capacity::try_acquire(
        crate::server::core::runtime_capacity::Class::Network, "public-images")?;
    for redirects in 0..=MAX_REDIRECTS {
        ordinary.check_public_url(url.as_str())?;
        let (host, addresses) = addresses(&url).await?;
        // 复用宿主现有 no_proxy + no_redirect + resolve_to_addrs 客户端；
        // TLS 仍验证 URL 的原始域名，不使用插件/账号/系统代理及任何凭据头。
        let client = crate::server::core::egress::image_download_client(None, false, &host, &addresses)
            .map_err(|_| super::error("无法创建公共图片下载连接"))?;
        let mut response = client.get(url.clone()).timeout(DOWNLOAD_TIMEOUT)
            .header("user-agent", "Agent2API-PublicImages/1.0")
            .header("accept", "image/png, image/jpeg, image/webp")
            .header("accept-encoding", "identity")
            .send().await.map_err(|_| super::error("公共图片下载连接失败"))?;
        if response.status().is_redirection() {
            if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                return Err(super::error("公共图片返回了不支持的跳转状态"));
            }
            if redirects == MAX_REDIRECTS { return Err(super::error("公共图片跳转超过 3 次限制")); }
            let mut locations = response.headers().get_all(LOCATION).iter();
            let location = locations.next().and_then(|value| value.to_str().ok())
                .ok_or_else(|| super::error("公共图片跳转缺少有效的目标地址"))?;
            if locations.next().is_some() { return Err(super::error("公共图片跳转目标不明确")); }
            reference(location)?;
            url = ordinary.redirect(&url, location)?;
            continue;
        }
        // 无 Range 请求只接受完整 200 响应，不把 206 或空响应当成完整图片。
        if response.status() != reqwest::StatusCode::OK {
            return Err(super::error(format!("公共图片下载返回 HTTP {}", response.status().as_u16())));
        }
        let mime = mime(response.headers())?;
        if response.headers().get_all(CONTENT_ENCODING).iter().any(|value|
            value.to_str().map_or(true, |value| !value.trim().eq_ignore_ascii_case("identity"))) {
            return Err(super::error("公共图片响应使用了不支持的内容压缩"));
        }
        let budget = MAX_IMAGE_BYTES.min(remaining).min(MAX_TOTAL_BYTES.saturating_sub(*downloaded));
        if response.content_length().is_some_and(|length| length > budget as u64) {
            return Err(limit("公共图片超过单图 32 MiB、累计 64 MiB 或资源总量 96 MiB 限制"));
        }
        let memory = crate::server::core::runtime_capacity::MemoryBudget::new();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| super::error("公共图片下载中断"))? {
            // 即使这片越界、之后图片无效或下载失败，也不返还已经读取的累计预算。
            *downloaded = (*downloaded).saturating_add(chunk.len());
            if *downloaded > MAX_TOTAL_BYTES || bytes.len().saturating_add(chunk.len()) > budget {
                return Err(limit("公共图片超过单图 32 MiB、累计 64 MiB 或资源总量 96 MiB 限制"));
            }
            memory.grow(chunk.len())?;
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() { return Err(super::error("公共图片内容为空")); }
        check_content(&bytes, mime)?;
        return Ok(Image { bytes, mime });
    }
    Err(super::error("公共图片跳转超过 3 次限制"))
}
