use std::net::IpAddr;

use base64::Engine;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::super::ResolvedProxy;

pub(super) async fn connect(proxy: &ResolvedProxy, host: &str, port: u16) -> Result<BufReader<TcpStream>, String> {
    let proxy_host = super::super::normalize_host(&proxy.host)?;
    let socket = TcpStream::connect((proxy_host.as_str(), proxy.port.unwrap_or(0))).await
        .map_err(|error| format!("无法连接系统代理：{error}"))?;
    let _ = socket.set_nodelay(true);
    let mut socket = BufReader::new(socket);
    handshake(&mut socket, proxy, host, port).await?;
    Ok(socket)
}

pub(super) async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut S, proxy: &ResolvedProxy, host: &str, port: u16,
) -> Result<(), String> {
    let host = super::super::normalize_host(host)?;
    if port == 0 { return Err("代理目标端口无效".to_string()); }
    if proxy.protocol == "socks5" { socks(socket, proxy, &host, port).await }
    else { http(socket, proxy, &host, port).await }
}

async fn http<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut S, proxy: &ResolvedProxy, host: &str, port: u16,
) -> Result<(), String> {
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let authority = format!("{host}:{port}");
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n");
    if !proxy.username.is_empty() || !proxy.password.is_empty() {
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", proxy.username, proxy.password));
        request.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
    }
    request.push_str("\r\n");
    socket.write_all(request.as_bytes()).await.map_err(|_| "无法发送 HTTP 代理握手")?;
    let response = super::read_header(socket).await?;
    let status = std::str::from_utf8(&response).ok().and_then(|text| text.lines().next())
        .and_then(|line| {
            let mut parts = line.split_whitespace();
            if !matches!(parts.next(), Some("HTTP/1.0" | "HTTP/1.1")) { return None; }
            parts.next()?.parse::<u16>().ok()
        }).ok_or("HTTP 代理响应无效")?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP 代理拒绝连接（HTTP {status}）"));
    }
    Ok(())
}

async fn socks<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut S, proxy: &ResolvedProxy, host: &str, port: u16,
) -> Result<(), String> {
    let has_auth = !proxy.username.is_empty() || !proxy.password.is_empty();
    let methods: &[u8] = if has_auth { &[5, 2, 0, 2] } else { &[5, 1, 0] };
    socket.write_all(methods).await.map_err(|_| "无法发送 SOCKS5 握手")?;
    let mut response = [0_u8; 2];
    socket.read_exact(&mut response).await.map_err(|_| "SOCKS5 代理握手失败")?;
    if response[0] != 5 { return Err("SOCKS5 代理响应版本无效".to_string()); }
    match response[1] {
        0 => {}
        2 if has_auth => {
            let username = proxy.username.as_bytes();
            let password = proxy.password.as_bytes();
            if username.is_empty() || username.len() > 255 || password.len() > 255 {
                return Err("SOCKS5 用户名不能为空，用户名和密码不能超过 255 字节".to_string());
            }
            let mut auth = vec![1, username.len() as u8];
            auth.extend_from_slice(username);
            auth.push(password.len() as u8);
            auth.extend_from_slice(password);
            socket.write_all(&auth).await.map_err(|_| "无法发送 SOCKS5 认证")?;
            socket.read_exact(&mut response).await.map_err(|_| "SOCKS5 认证失败")?;
            if response != [1, 0] { return Err("SOCKS5 代理拒绝用户名或密码".to_string()); }
        }
        _ => return Err("SOCKS5 代理不支持当前认证方式".to_string()),
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => { request.push(1); request.extend_from_slice(&address.octets()); }
        Ok(IpAddr::V6(address)) => { request.push(4); request.extend_from_slice(&address.octets()); }
        Err(_) => {
            if host.len() > 255 { return Err("SOCKS5 目标域名过长".to_string()); }
            request.extend_from_slice(&[3, host.len() as u8]);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    socket.write_all(&request).await.map_err(|_| "无法发送 SOCKS5 连接请求")?;
    let mut response = [0_u8; 4];
    socket.read_exact(&mut response).await.map_err(|_| "SOCKS5 连接响应不完整")?;
    if response[0] != 5 || response[1] != 0 || response[2] != 0 {
        return Err(format!("SOCKS5 代理拒绝连接（状态 {}）", response[1]));
    }
    let size = match response[3] {
        1 => 4,
        4 => 16,
        3 => socket.read_u8().await.map_err(|_| "SOCKS5 地址响应不完整")? as usize,
        _ => return Err("SOCKS5 地址类型无效".to_string()),
    };
    let mut rest = vec![0_u8; size + 2];
    socket.read_exact(&mut rest).await.map_err(|_| "SOCKS5 地址响应不完整")?;
    Ok(())
}
