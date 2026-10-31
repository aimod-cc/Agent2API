//! HTTPDNS 只查询策略列出的 HTTPS 原域名；解析值只能改变连接地址，不能改变 TLS 校验。

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::server::core::providers::policy::{HttpDnsPolicy, NetworkPolicy};
use crate::server::logging;

struct Entry {
    ips: Vec<IpAddr>,
    expires_at: i64,
}

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) async fn resolve(policy: &NetworkPolicy, host: &str) -> Vec<IpAddr> {
    let trusted = policy.trusted_origins.iter().any(|origin| {
        url::Url::parse(origin)
            .ok()
            .is_some_and(|url| url.scheme() == "https" && url.host_str() == Some(host))
    });
    if !trusted || host.parse::<IpAddr>().is_ok() {
        return Vec::new();
    }
    let Some(dns) = &policy.http_dns else {
        return Vec::new();
    };
    if dns.token.len() > 128 || dns.token.is_empty() {
        return Vec::new();
    }
    let key = format!("{}:{host}", dns.token);
    if let Some(ips) = cache().lock().ok().and_then(|cache| {
        cache
            .get(&key)
            .filter(|entry| entry.expires_at > logging::now_ms())
            .map(|entry| entry.ips.clone())
    }) {
        return ips;
    }
    let ips = tokio::time::timeout(Duration::from_secs(3), lookup(dns, host))
        .await
        .unwrap_or_default();
    if let Ok(mut cache) = cache().lock() {
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(
            key,
            Entry {
                ips: ips.clone(),
                expires_at: logging::now_ms().saturating_add(if ips.is_empty() {
                    60_000
                } else {
                    300_000
                }),
            },
        );
    }
    ips
}

async fn lookup(dns: &HttpDnsPolicy, host: &str) -> Vec<IpAddr> {
    let Ok(client) = reqwest::Client::builder()
        .use_rustls_tls()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(800))
        .timeout(Duration::from_millis(1200))
        .build()
    else {
        return Vec::new();
    };
    let bootstrap: Vec<IpAddr> = dns
        .bootstrap_ips
        .iter()
        .take(4)
        .filter_map(|ip| ip.parse().ok())
        .filter(public_ip)
        .collect();
    let mut services = Vec::new();
    for ip in &bootstrap {
        if let Some(text) = get(&client, *ip, "/conf", &[("token", dns.token.as_str())]).await {
            for part in text.split('|') {
                if let Some((key, value)) = part.split_once(':') {
                    if key.trim().eq_ignore_ascii_case("ip") {
                        services = parse_ips(value);
                    }
                }
            }
            if !services.is_empty() {
                break;
            }
        }
    }
    if services.is_empty() {
        services = bootstrap;
    }
    for ip in services.into_iter().take(4) {
        if let Some(text) = get(
            &client,
            ip,
            "/d",
            &[("token", dns.token.as_str()), ("dn", host), ("ttl", "1")],
        )
        .await
        {
            let ips = parse_ips(text.split('|').next().unwrap_or(""));
            if !ips.is_empty() {
                return ips;
            }
        }
    }
    Vec::new()
}

async fn get(
    client: &reqwest::Client,
    ip: IpAddr,
    path: &str,
    query: &[(&str, &str)],
) -> Option<String> {
    let host = match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    // 解析服务本身也不关闭证书验证；失败只视为没有 DNS 候选。
    let mut response = client
        .get(format!("https://{host}{path}"))
        .query(query)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > 16_384 {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).ok()
}

fn parse_ips(text: &str) -> Vec<IpAddr> {
    let mut seen = HashSet::new();
    text.split(|ch: char| ch == ';' || ch == ',' || ch.is_whitespace())
        .filter_map(|text| text.parse::<IpAddr>().ok())
        .filter(public_ip)
        .filter(|ip| seen.insert(*ip))
        .take(4)
        .collect()
}

pub(super) fn public_ip(ip: &IpAddr) -> bool {
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
                && !(ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                && !(ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19))
        }
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|ip| public_ip(&IpAddr::V4(ip)))
            .unwrap_or_else(|| {
                !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && !ip.is_unique_local()
                    && !ip.is_unicast_link_local()
                    && ip.segments()[0] & 0xe000 == 0x2000
                    && !(ip.segments()[0] == 0x2001
                        && matches!(ip.segments()[1], 0x0000 | 0x0002 | 0x0db8))
            }),
    }
}
