use std::collections::BTreeMap;
use agent2api_provider_protocol as proto;
use serde_json::{json,Value};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
use super::{Flow,invalid};

pub(super) fn query(raw: &str,callback: &str) -> Result<Value,proto::WireError> {
    if raw.len() > 16 * 1024 || raw.contains('\\') || raw.chars().any(char::is_control) { return Err(invalid()); }
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    let expected = url::Url::parse(callback).map_err(|_| invalid())?;
    if url.scheme() != "http" || !matches!(url.host_str(),Some("127.0.0.1" | "localhost" | "[::1]"))
        || url.port() != expected.port() || url.path() != "/oauth/callback" || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() { return Err(invalid()); }
    let mut values = BTreeMap::new();
    for (name,value) in url.query_pairs() {
        if values.len() >= 32 || name.len() > 128 || value.len() > 8192
            || name.chars().any(char::is_control) || value.chars().any(char::is_control)
            || values.insert(name.to_string(),value.to_string()).is_some() { return Err(invalid()); }
    }
    let material = ["code", "secret"].iter().any(|name| values.get(*name).is_some_and(|value| !value.is_empty()));
    let rejected = ["error", "error_code"].iter().any(|name| values.get(*name).is_some_and(|value| !value.is_empty()))
        || values.get("error_description").is_some_and(|value| !value.is_empty());
    if !material && !rejected { return Err(invalid()); }
    if rejected {
        return Ok(json!({"error":"authorization_rejected"}));
    }
    if let Some(redirect) = values.get("redirect") { if redirect.contains(['\r','\n']) || !trusted(redirect) { return Err(invalid()); } }
    Ok(json!(values))
}
fn trusted(raw: &str) -> bool {
    url::Url::parse(raw).ok().is_some_and(|url| url.scheme() == "https" && url.port_or_known_default() == Some(443)
        && url.username().is_empty() && url.password().is_none() && url.host_str().is_some_and(|host| host == "huaweicloud.com" || host.ends_with(".huaweicloud.com")))
}

pub(super) fn spawn(listener: tokio::net::TcpListener,flow: std::sync::Arc<Flow>) {
    crate::spawn_task(async move {
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        loop {
            tokio::select! { biased;
                _ = flow.cancel.cancelled() => break,
                _ = tokio::time::sleep_until(flow.until) => break,
                next = listener.accept() => {
                    let Ok((stream,peer)) = next else { break; };
                    if !peer.ip().is_loopback() { continue; }
                    let Ok(slot) = slots.clone().try_acquire_owned() else { continue; };
                    let flow = flow.clone();
                    crate::spawn_task(async move { let _slot = slot; let _ = tokio::time::timeout(std::time::Duration::from_secs(5),serve(stream,&flow)).await; });
                }
            }
        }
    });
}
async fn serve(mut stream: tokio::net::TcpStream,flow: &Flow) -> Result<(),proto::WireError> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8;1024];
    loop {
        let size = stream.read(&mut chunk).await.map_err(|_| invalid())?;
        if size == 0 { return Err(invalid()); }
        bytes.extend_from_slice(&chunk[..size]);
        if bytes.len() > 16 * 1024 { return Err(invalid()); }
        if bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") { break; }
    }
    let raw = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let line = raw.lines().next().ok_or_else(invalid)?;
    let mut parts = line.split_whitespace();
    if parts.next() != Some("GET") { return Err(invalid()); }
    let path = parts.next().ok_or_else(invalid)?;
    if !path.starts_with("/oauth/callback?") { stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.map_err(|_| invalid())?; return Ok(()); }
    let base = flow.protocol.callback.split("/oauth/callback").next().ok_or_else(invalid)?;
    let values = query(&format!("{base}{path}"),&flow.protocol.callback)?;
    let redirect = values["redirect"].as_str().filter(|redirect| trusted(redirect)).map(str::to_string);
    flow.sender.try_send(values).map_err(|_| invalid())?;
    let response = if let Some(redirect) = redirect { format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {redirect}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n") }
        else { crate::server::core::login::callback_page::raw_receipt_response(true, "codearts", "") };
    stream.write_all(response.as_bytes()).await.map_err(|_| invalid())
}
