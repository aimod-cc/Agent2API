//! 在唯一对话投递前检查官方入口；检查不带账号凭据，也不提交对话正文。

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::time::Instant;

use crate::server::core::{
    egress::managed::FrozenHttpClient,
    plugin_host::script::HttpDeadline,
    providers::policy::{HttpDnsPolicy, NetworkPolicy},
    proxies::chain::SelectedPath,
    upstream::http_dns,
};

const BACKUP: &str = "https://wb.tencentbuddy.com";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn policy() -> NetworkPolicy {
    NetworkPolicy {
        trusted_origins: super::ORIGINS.iter().map(|origin| (*origin).into()).collect(),
        backup_origin: BACKUP.into(),
        http_dns: Some(HttpDnsPolicy {
            token: "719980335".into(),
            bootstrap_ips: vec!["119.29.29.99".into(), "119.28.28.99".into()],
        }),
        fallback_addresses: vec!["162.14.3.241".into()],
        ..Default::default()
    }
}

async fn probe(client: &FrozenHttpClient, origin: &str, deadline: HttpDeadline) -> bool {
    if deadline.check().is_err() { return false; }
    let Ok(client) = client.client() else { return false; };
    let request = client.head(format!("{origin}/")).header("Accept", "*/*");
    match tokio::time::timeout_at(Instant::now() + PROBE_TIMEOUT, request.send()).await {
        Ok(Ok(response)) => !matches!(response.status().as_u16(), 408 | 502 | 503 | 504),
        _ => false,
    }
}

pub(super) async fn prepare(path: SelectedPath, url: &str, deadline: HttpDeadline)
    -> Result<(FrozenHttpClient, String), proto::WireError>
{
    deadline.check()?;
    let parsed = url::Url::parse(url).map_err(|_| super::invalid_endpoint())?;
    let origin = parsed.origin().ascii_serialization();
    if !super::ORIGINS.contains(&origin.as_str()) || parsed.path() != super::PATH {
        return Err(super::invalid_endpoint());
    }
    let original = FrozenHttpClient::from_path(path.clone()).await.map_err(|_| super::unavailable())?;
    if probe(&original, &origin, deadline).await {
        deadline.check()?;
        return Ok((original, url.into()));
    }
    let backup = if origin != BACKUP {
        if probe(&original, BACKUP, deadline).await {
            deadline.check()?;
            return Ok((original, format!("{BACKUP}{}", super::PATH)));
        }
        BACKUP
    } else {
        origin.as_str()
    };
    if path.is_direct() {
        let hostname = url::Url::parse(backup).map_err(|_| super::invalid_endpoint())?
            .host_str().ok_or_else(super::invalid_endpoint)?.to_string();
        let policy = policy();
        let mut addresses = http_dns::resolve(&policy, &hostname).await;
        addresses.extend(policy.fallback_addresses.iter().filter_map(|ip| ip.parse::<IpAddr>().ok()));
        addresses.retain(http_dns::public_ip);
        addresses.sort_unstable();
        addresses.dedup();
        for ip in addresses.into_iter().take(4) {
            deadline.check()?;
            let addresses = [SocketAddr::new(ip, 443)];
            let client = FrozenHttpClient::from_path_with_addresses(path.clone(), &hostname, &addresses)
                .await.map_err(|_| super::unavailable())?;
            if probe(&client, backup, deadline).await {
                deadline.check()?;
                return Ok((client, format!("{backup}{}", super::PATH)));
            }
        }
    }
    deadline.check()?;
    // HEAD 不通不证明对话不通；最后仍只投递原请求一次，不把检查失败变成重放资格。
    Ok((original, url.into()))
}
