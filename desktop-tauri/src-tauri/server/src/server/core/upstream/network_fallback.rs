//! 可选入口与地址容错只作用于可信 HTTPS origin，不改变账号选择和代理出口。

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use super::request::{self, TransportRequest, UpstreamRequestError};
use crate::server::core::{egress, providers::policy::NetworkPolicy};
use crate::server::logging;

struct Breaker {
    failures: Vec<i64>,
    until: i64,
    backoff: usize,
}

fn breakers() -> &'static Mutex<HashMap<String, Breaker>> {
    static BREAKERS: OnceLock<Mutex<HashMap<String, Breaker>>> = OnceLock::new();
    BREAKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) struct Fallback {
    policy: Option<NetworkPolicy>,
    current: TransportRequest,
    origin_switched: bool,
    addresses: Vec<IpAddr>,
    used: HashSet<(String, IpAddr)>,
    pinned: Option<IpAddr>,
    attempts: usize,
}

impl Fallback {
    pub fn new(transport: &TransportRequest, policy: Option<&NetworkPolicy>) -> Self {
        let policy = policy
            .filter(|policy| trusted(&transport.url, policy))
            .cloned();
        let mut current = transport.clone();
        let mut origin_switched = false;
        if let Some(policy) = &policy {
            let origin = origin(&current.url);
            if origin != policy.backup_origin
                && open(&origin, &current)
                && !open(&policy.backup_origin, &current)
            {
                swap(&mut current, &policy.backup_origin);
                origin_switched = true;
            }
        }
        Self {
            policy,
            current,
            origin_switched,
            addresses: Vec::new(),
            used: HashSet::new(),
            pinned: None,
            attempts: 0,
        }
    }

    pub fn transport(&self) -> &TransportRequest {
        &self.current
    }

    pub async fn send(&mut self) -> Result<reqwest::Response, UpstreamRequestError> {
        self.attempts += 1;
        if self.policy.is_none() {
            return request::send_chat_request(&self.current).await;
        }
        let host = url::Url::parse(&self.current.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        let addresses: Vec<SocketAddr> = self
            .pinned
            .into_iter()
            .map(|ip| SocketAddr::new(ip, 443))
            .collect();
        let client = egress::resolved_client(
            self.current.proxy.as_ref(),
            self.current.system_proxy_when_unset,
            &host,
            &addresses,
        )
        .map_err(|message| UpstreamRequestError {
            message,
            reason: "无法创建备用连接".into(),
            safe_to_retry: true,
            network_failure: false,
        })?;
        request::send_chat_with_client(&self.current, &client).await
    }

    pub fn success(&self) {
        if self.policy.is_some() {
            if let Ok(mut breakers) = breakers().lock() {
                breakers.remove(&breaker_key(&origin(&self.current.url), &self.current));
            }
        }
    }

    pub async fn after_transport(&mut self, error: &UpstreamRequestError) -> Option<String> {
        if self.policy.is_none() || !error.network_failure {
            return None;
        }
        note_failure(&origin(&self.current.url), &self.current);
        if self.switch_origin() {
            return Some("官方入口连接失败，沿用同一代理改发备用入口".into());
        }
        self.next_address()
            .await
            .then(|| "官方入口连接失败，保留原域名改用候选地址".into())
    }

    pub async fn after_status(
        &mut self,
        status: u16,
        body: &Value,
        allowed: bool,
    ) -> Option<String> {
        let policy = self.policy.as_ref()?;
        if !allowed
            || retry_denied(body)
            || business_error(body)
            || !policy.retry_statuses.contains(&status)
            || !matches!(status, 408 | 502 | 503 | 504)
        {
            return None;
        }
        let address =
            policy.address_statuses.contains(&status) && matches!(status, 408 | 502 | 503);
        note_failure(&origin(&self.current.url), &self.current);
        if self.switch_origin() {
            return Some(format!(
                "官方入口返回 HTTP {status}，沿用同一代理改发备用入口"
            ));
        }
        (address && self.next_address().await)
            .then(|| format!("官方入口返回 HTTP {status}，保留原域名改用候选地址"))
    }

    fn available(&self) -> bool {
        self.policy
            .as_ref()
            .is_some_and(|policy| self.attempts < policy.max_attempts.clamp(1, 4))
    }

    fn switch_origin(&mut self) -> bool {
        let Some(policy) = &self.policy else {
            return false;
        };
        if !self.available()
            || self.origin_switched
            || origin(&self.current.url) == policy.backup_origin
        {
            return false;
        }
        swap(&mut self.current, &policy.backup_origin);
        self.origin_switched = true;
        self.pinned = None;
        self.addresses.clear();
        true
    }

    async fn next_address(&mut self) -> bool {
        let Some(policy) = &self.policy else {
            return false;
        };
        // 指定 HTTP/SOCKS 代理的 DNS 由代理负责，不能绕过它去直连解析或连接。
        if !self.available() || self.current.proxy.is_some() || self.current.system_proxy_when_unset
        {
            return false;
        }
        let host = url::Url::parse(&self.current.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        if self.addresses.is_empty() {
            self.addresses = super::http_dns::resolve(policy, &host).await;
            if origin(&self.current.url) == policy.backup_origin {
                self.addresses.extend(
                    policy
                        .fallback_addresses
                        .iter()
                        .filter_map(|ip| ip.parse::<IpAddr>().ok())
                        .filter(super::http_dns::public_ip),
                );
            }
        }
        for ip in &self.addresses {
            if self.used.insert((host.clone(), *ip)) {
                self.pinned = Some(*ip);
                return true;
            }
        }
        false
    }
}

fn trusted(url: &str, policy: &NetworkPolicy) -> bool {
    if policy.trusted_origins.is_empty() || policy.trusted_origins.len() > 8 {
        return false;
    }
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && policy
            .trusted_origins
            .iter()
            .any(|origin| origin == &url.origin().ascii_serialization())
        && policy.trusted_origins.contains(&policy.backup_origin)
        && url::Url::parse(&policy.backup_origin)
            .ok()
            .is_some_and(|url| {
                url.scheme() == "https"
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.port_or_known_default() == Some(443)
            })
}

fn swap(transport: &mut TransportRequest, backup: &str) {
    let Ok(mut url) = url::Url::parse(&transport.url) else {
        return;
    };
    let Ok(backup) = url::Url::parse(backup) else {
        return;
    };
    if url.set_host(backup.host_str()).is_err() {
        return;
    }
    let _ = url.set_port(backup.port());
    transport.url = url.to_string();
    transport
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("host"));
}

fn origin(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default()
}

fn breaker_key(origin: &str, transport: &TransportRequest) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(origin.as_bytes());
    if let Some(proxy) = &transport.proxy {
        digest.update(format!("{proxy:?}").as_bytes());
    }
    digest.update([u8::from(transport.system_proxy_when_unset)]);
    format!("{:x}", digest.finalize())
}

fn open(origin: &str, transport: &TransportRequest) -> bool {
    breakers()
        .lock()
        .ok()
        .and_then(|breakers| {
            breakers
                .get(&breaker_key(origin, transport))
                .map(|entry| entry.until > logging::now_ms())
        })
        .unwrap_or(false)
}

fn note_failure(origin: &str, transport: &TransportRequest) {
    let Ok(mut breakers) = breakers().lock() else {
        return;
    };
    if breakers.len() >= 128 {
        breakers.clear();
    }
    let entry = breakers
        .entry(breaker_key(origin, transport))
        .or_insert(Breaker {
            failures: Vec::new(),
            until: 0,
            backoff: 0,
        });
    let now = logging::now_ms();
    entry.failures.retain(|at| *at > now.saturating_sub(60_000));
    entry.failures.push(now);
    if entry.until > now {
        let seconds = [30_i64, 60, 120][entry.backoff.min(2)];
        entry.backoff = entry.backoff.saturating_add(1);
        entry.until = now.saturating_add(seconds * 1000);
    } else if entry.failures.len() >= 3 {
        entry.until = now.saturating_add(15_000);
    }
}

fn retry_denied(body: &Value) -> bool {
    [
        "",
        "/data",
        "/error",
        "/error/data",
        "/error/error",
        "/error/error/data",
    ]
    .iter()
    .filter_map(|path| {
        if path.is_empty() {
            Some(body)
        } else {
            body.pointer(path)
        }
    })
    .find_map(|node| node.get("retry_policy"))
    .and_then(|policy| policy.get("retry"))
    .and_then(Value::as_bool)
        == Some(false)
}

fn business_error(body: &Value) -> bool {
    has_business_code(body, 6)
}

fn has_business_code(node: &Value, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    node.get("code").is_some_and(|code| {
        code.as_i64()
            .or_else(|| code.as_str().and_then(|text| text.parse().ok()))
            .is_some_and(|code| code >= 1000)
    }) || [
        node.get("data"),
        node.get("error"),
        node.pointer("/response/data"),
    ]
    .into_iter()
    .flatten()
    .any(|node| has_business_code(node, depth - 1))
}
