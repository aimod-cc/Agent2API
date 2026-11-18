//! 图片按 Magpie 的 base64 内容块传入 CLI/MCP；URL 下载沿用本项目
//! vision_compat 的公开地址检查、固定解析与禁跟转策略，不允许读网关本机文件。
use crate::server::core::{egress, proxies::ResolvedProxy};
use crate::server::errors::GatewayError;
use base64::Engine;
use serde_json::{json, Value};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

const MAX_IMAGE: usize = 10 * 1024 * 1024;
#[derive(Clone, Debug)]
pub(super) struct Image {
    pub mime: String,
    pub data: String,
}
impl Image {
    pub fn anthropic(&self) -> Value {
        json!({"type":"image","source":{"type":"base64","media_type":self.mime,"data":self.data}})
    }
}
fn invalid(message: &str) -> GatewayError {
    GatewayError::bad_request(message).with_code("claude_image_invalid")
}

pub(super) async fn read(part: &Value, proxy: Option<&ResolvedProxy>) -> Result<Image, GatewayError> {
    if let Some(source) = part.get("source").filter(|s| s.get("type").and_then(Value::as_str) == Some("base64")) {
        return inline(
            source.get("media_type").and_then(Value::as_str).unwrap_or(""),
            source.get("data").and_then(Value::as_str).unwrap_or(""),
        );
    }
    let reference = part
        .pointer("/image_url/url")
        .and_then(Value::as_str)
        .or_else(|| part.get("image_url").and_then(Value::as_str))
        .or_else(|| part.pointer("/source/url").and_then(Value::as_str))
        .or_else(|| part.get("url").and_then(Value::as_str))
        .ok_or_else(|| invalid("图片没有可读取内容，未删除图片继续回答"))?;
    if reference.starts_with("data:") {
        let (header, data) = reference.split_once(',').ok_or_else(|| invalid("图片 data URI 不完整"))?;
        let mime = header
            .strip_prefix("data:")
            .and_then(|s| s.strip_suffix(";base64"))
            .ok_or_else(|| invalid("图片必须使用 base64 data URI"))?;
        return inline(mime, data);
    }
    if proxy.is_some() {
        return Err(invalid("当前代理不支持固定图片目标地址；请由客户端把图片转为 base64，未回退直连下载"));
    }
    tokio::time::timeout(Duration::from_secs(35), download(reference))
        .await
        .map_err(|_| invalid("图片下载超时，未发送不完整的消息"))?
}

fn inline(mime: &str, data: &str) -> Result<Image, GatewayError> {
    if data.len() > MAX_IMAGE * 4 / 3 + 4 {
        return Err(invalid("Claude 单张图片不能超过 10MiB"));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| invalid("图片 base64 编码无效"))?;
    let detected = match image::guess_format(&bytes).ok() {
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::WebP) => "image/webp",
        Some(image::ImageFormat::Gif) => "image/gif",
        _ => return Err(invalid("Claude 图片只接受 PNG、JPEG、WebP、GIF")),
    };
    if bytes.is_empty() || bytes.len() > MAX_IMAGE || (!mime.is_empty() && mime != detected) {
        return Err(invalid("图片内容为空、过大或与声明的格式不符"));
    }
    Ok(Image { mime: detected.into(), data: data.into() })
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && ip.octets()[0] != 0
                && ip.octets()[0] < 240
                && ip.octets()[0..3] != [192, 0, 0]
                && !(ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                && !(ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19))
        }
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map(|v| public_ip(v.into())).unwrap_or_else(|| {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && ip.segments()[0] & 0xe000 == 0x2000
                && ip.segments()[0] != 0x2002
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] < 0x0200)
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        }),
    }
}

async fn download(reference: &str) -> Result<Image, GatewayError> {
    let mut url = url::Url::parse(reference).map_err(|_| invalid("图片地址无效"))?;
    for _ in 0..=3 {
        if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
            return Err(invalid("图片地址必须是不含认证信息的 HTTP/HTTPS 地址"));
        }
        let host = url.host_str().ok_or_else(|| invalid("图片地址缺少主机"))?.trim_matches(['[', ']']).to_string();
        let port = url.port_or_known_default().ok_or_else(|| invalid("图片端口无效"))?;
        let addresses: Vec<SocketAddr> =
            tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| invalid("图片域名无法解析"))?.collect();
        if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
            return Err(invalid("图片不能指向本机、内网或保留地址"));
        }
        // 与 vision_compat 一致：只允许检查过的固定公开 IP，禁止自动跟转。
        // 代理账号须由客户端提供内联图片，不能绕过账号代理悄悄直连。
        let client = egress::image_download_client(None, false, &host, &addresses)
            .map_err(|_| invalid("无法创建图片读取连接"))?;
        let mut response = client
            .get(url.clone())
            .timeout(Duration::from_secs(25))
            .send()
            .await
            .map_err(|_| invalid("图片下载失败"))?;
        if response.status().is_redirection() {
            let target = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| invalid("图片跳转缺少地址"))?;
            url = url.join(target).map_err(|_| invalid("图片跳转地址无效"))?;
            continue;
        }
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_IMAGE as u64) {
            return Err(invalid("图片下载失败或超过大小限制"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| invalid("图片下载中断"))? {
            if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE {
                return Err(invalid("图片超过大小限制"));
            }
            bytes.extend_from_slice(&chunk);
        }
        return inline("", &base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    Err(invalid("图片跳转次数过多"))
}
