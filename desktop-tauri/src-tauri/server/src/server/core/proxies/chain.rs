use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::ResolvedProxy;

mod tunnel;

const MAX_BRIDGES: usize = 128;
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);
const HEADER_LIMIT: usize = 16 * 1024;

struct Bridge {
    port: u16,
    username: String,
    password: String,
    touched: Arc<Mutex<Instant>>,
    active: Arc<AtomicUsize>,
    cancel: CancellationToken,
}

impl Drop for Bridge {
    fn drop(&mut self) { self.cancel.cancel(); }
}

fn bridges() -> &'static Mutex<HashMap<String, Arc<Bridge>>> {
    static BRIDGES: OnceLock<Mutex<HashMap<String, Arc<Bridge>>>> = OnceLock::new();
    BRIDGES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(proxy: &ResolvedProxy, upstream: &ResolvedProxy) -> String {
    let mut hash = Sha256::new();
    for value in [proxy, upstream] {
        for part in [&value.protocol, &value.host, &value.username, &value.password] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        hash.update(value.port.unwrap_or(0).to_be_bytes());
    }
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate(proxy: &ResolvedProxy) -> Result<(), String> {
    super::normalize_host(&proxy.host)?;
    if proxy.port.is_none_or(|port| port == 0) { return Err("代理端口无效".to_string()); }
    if !matches!(proxy.protocol.as_str(), "http" | "socks5") {
        return Err("非直连代理只支持 HTTP 或 SOCKS5".to_string());
    }
    if proxy.protocol == "socks5" && (proxy.username.len() > 255 || proxy.password.len() > 255) {
        return Err("SOCKS5 用户名和密码不能超过 255 字节".to_string());
    }
    Ok(())
}

pub(crate) fn endpoint(proxy: &ResolvedProxy) -> Result<ResolvedProxy, String> {
    if !proxy.non_direct { return Ok(proxy.clone()); }
    validate(proxy)?;
    let upstream = super::system::upstream()?;
    validate(&upstream)?;
    if super::normalize_host(&proxy.host)? == super::normalize_host(&upstream.host)? && proxy.port == upstream.port {
        return Err("非直连代理不能与系统代理使用同一个地址和端口".to_string());
    }
    let key = fingerprint(proxy, &upstream);
    let mut entries = bridges().lock().map_err(|_| "非直连代理转发不可用")?;
    entries.retain(|_, bridge| !bridge.cancel.is_cancelled()
        && (bridge.active.load(Ordering::Relaxed) > 0 || bridge.touched.lock().is_ok_and(|time| time.elapsed() < IDLE_TTL)));
    let bridge = if let Some(bridge) = entries.get(&key) {
        bridge.clone()
    } else {
        if entries.len() >= MAX_BRIDGES {
            let oldest = entries.iter().filter(|(_, bridge)| bridge.active.load(Ordering::Relaxed) == 0)
                .filter_map(|(key, bridge)| bridge.touched.lock().ok().map(|time| (key.clone(), *time)))
                .min_by_key(|(_, time)| *time).map(|(key, _)| key);
            let Some(oldest) = oldest else { return Err("非直连代理连接过多，请稍后重试".to_string()); };
            entries.remove(&oldest);
        }
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "无法创建非直连代理转发端口")?;
        listener.set_nonblocking(true).map_err(|_| "无法初始化非直连代理转发端口")?;
        let port = listener.local_addr().map_err(|_| "无法读取非直连代理转发端口")?.port();
        let mut secret = [0_u8; 32];
        getrandom::getrandom(&mut secret).map_err(|_| "无法生成非直连代理连接凭证")?;
        let username = "agent2api".to_string();
        let password = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret);
        let cancel = CancellationToken::new();
        let touched = Arc::new(Mutex::new(Instant::now()));
        let active = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(Bridge {
            port, username: username.clone(), password: password.clone(),
            touched: touched.clone(), active: active.clone(), cancel: cancel.clone(),
        });
        let target = proxy.clone();
        crate::spawn_task(async move {
            serve(listener, target, upstream, username, password, cancel, touched, active).await;
        });
        entries.insert(key, bridge.clone());
        bridge
    };
    if let Ok(mut touched) = bridge.touched.lock() { *touched = Instant::now(); }
    Ok(ResolvedProxy {
        source: proxy.source.clone(), protocol: "http".to_string(),
        host: "127.0.0.1".to_string(), port: Some(bridge.port),
        username: bridge.username.clone(), password: bridge.password.clone(),
        label: proxy.label.clone(), non_direct: false,
    })
}

pub(crate) fn wire_json(proxy: &ResolvedProxy) -> Result<Value, String> {
    let proxy = endpoint(proxy)?;
    Ok(json!({
        "protocol": proxy.protocol, "host": proxy.host, "port": proxy.port,
        "username": proxy.username, "password": proxy.password,
    }))
}

fn limits() -> Arc<Semaphore> {
    static LIMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMITS.get_or_init(|| Arc::new(Semaphore::new(512))).clone()
}

async fn serve(
    listener: std::net::TcpListener, target: ResolvedProxy, upstream: ResolvedProxy,
    username: String, password: String, cancel: CancellationToken, touched: Arc<Mutex<Instant>>, active: Arc<AtomicUsize>,
) {
    let Ok(listener) = TcpListener::from_std(listener) else { cancel.cancel(); return; };
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}")));
    let target = Arc::new(target);
    let upstream = Arc::new(upstream);
    loop {
        let accepted = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let Ok((socket, _)) = accepted else { break; };
        if let Ok(mut time) = touched.lock() { *time = Instant::now(); }
        let Ok(permit) = limits().try_acquire_owned() else { continue; };
        let target = target.clone();
        let upstream = upstream.clone();
        let auth = auth.clone();
        let touched = touched.clone();
        let active = active.clone();
        active.fetch_add(1, Ordering::Relaxed);
        crate::spawn_task(async move {
            let _permit = permit;
            handle(socket, &target, &upstream, &auth).await;
            if let Ok(mut time) = touched.lock() { *time = Instant::now(); }
            active.fetch_sub(1, Ordering::Relaxed);
        });
    }
    cancel.cancel();
}

async fn handle(socket: TcpStream, target: &ResolvedProxy, upstream: &ResolvedProxy, auth: &str) {
    let mut client = BufReader::new(socket);
    let timeout = Duration::from_millis(crate::server::config::timeout_settings().connect_ms());
    let opening = async {
        let (host, port, forward) = request(&mut client, auth).await?;
        let mut remote = tunnel::connect(upstream, &target.host, target.port.unwrap_or(0)).await?;
        tunnel::handshake(&mut remote, target, &host, port).await?;
        if let Some(forward) = forward {
            remote.write_all(&forward).await.map_err(|_| "无法写入代理请求".to_string())?;
        } else {
            client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await
                .map_err(|_| "本机代理连接已关闭".to_string())?;
        }
        Ok::<_, String>(remote)
    };
    match tokio::time::timeout(timeout, opening).await {
        Ok(Ok(mut remote)) => { let _ = tokio::io::copy_bidirectional(&mut client, &mut remote).await; }
        result => {
            let reason = match result { Ok(Err(reason)) => reason, _ => "非直连代理连接超时".to_string() };
            let (status, code) = if reason == "本机代理认证失败" {
                ("407 Proxy Authentication Required", 407)
            } else { ("502 Bad Gateway", 502) };
            let response = format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{reason}", reason.len());
            let _ = tokio::time::timeout(Duration::from_secs(2), client.write_all(response.as_bytes())).await;
            if code != 407 { crate::server::logging::log("[Proxies]", &format!("非直连代理连接失败：{reason}")); }
        }
    }
}

pub(super) async fn read_header<S: tokio::io::AsyncRead + Unpin>(socket: &mut S) -> Result<Vec<u8>, String> {
    let mut header = Vec::new();
    while header.len() < HEADER_LIMIT {
        let byte = socket.read_u8().await.map_err(|_| "代理在握手完成前关闭连接".to_string())?;
        header.push(byte);
        if header.ends_with(b"\r\n\r\n") { return Ok(header); }
    }
    Err("代理响应头过大".to_string())
}

async fn request(client: &mut BufReader<TcpStream>, auth: &str) -> Result<(String, u16, Option<Vec<u8>>), String> {
    let header = read_header(client).await?;
    let text = std::str::from_utf8(&header).map_err(|_| "本机代理请求格式无效")?;
    let mut lines = text.split("\r\n");
    let mut start = lines.next().unwrap_or("").split_whitespace();
    let method = start.next().ok_or("本机代理请求格式无效")?;
    let target = start.next().ok_or("本机代理请求缺少地址")?;
    let version = start.next().ok_or("本机代理请求格式无效")?;
    if start.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err("本机代理请求格式无效".to_string());
    }
    let headers: Vec<_> = lines.filter(|line| !line.is_empty()).map(|line| {
        line.split_once(':').map(|(name, value)| (name, value.trim()))
            .ok_or_else(|| "本机代理请求头格式无效".to_string())
    }).collect::<Result<_, _>>()?;
    for (name, value) in &headers {
        axum::http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| "本机代理请求头格式无效")?;
        axum::http::HeaderValue::from_str(value).map_err(|_| "本机代理请求头格式无效")?;
    }
    let credentials: Vec<_> = headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization")).collect();
    if credentials.len() != 1 || credentials[0].1 != auth { return Err("本机代理认证失败".to_string()); }
    if method == "CONNECT" {
        let authority = target.parse::<axum::http::uri::Authority>().map_err(|_| "代理目标地址无效")?;
        let port = authority.port_u16().filter(|port| *port > 0).ok_or("代理目标端口无效")?;
        return Ok((super::normalize_host(authority.host())?, port, None));
    }
    let url = url::Url::parse(target).map_err(|_| "代理请求地址无效")?;
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return Err("本机代理仅接受 HTTP 请求或 CONNECT 隧道".to_string());
    }
    let host = super::normalize_host(url.host_str().ok_or("代理请求缺少主机")?)?;
    let port = url.port_or_known_default().filter(|port| *port > 0).ok_or("代理目标端口无效")?;
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    if method.is_empty() || !method.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)) {
        return Err("本机代理请求方法无效".to_string());
    }
    let mut forward = format!("{method} {path} {version}\r\n");
    let port_suffix = if port == 80 { String::new() } else { format!(":{port}") };
    let authority = if host.contains(':') { format!("[{host}]{port_suffix}") } else { format!("{host}{port_suffix}") };
    forward.push_str(&format!("Host: {authority}\r\n"));
    for (name, value) in headers {
        if ["host", "proxy-authorization", "proxy-connection", "connection"].iter().any(|skip| name.eq_ignore_ascii_case(skip)) { continue; }
        forward.push_str(&format!("{name}: {value}\r\n"));
    }
    forward.push_str("Connection: close\r\n\r\n");
    Ok((host, port, Some(forward.into_bytes())))
}
