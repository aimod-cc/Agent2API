//! 真实 loopback 监听租约；只认批准 profile，禁止 JSON 自定主机/端口/路径/officialState。
use std::collections::{BTreeMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Weak};
use std::time::Duration;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{Call, Session, State, WeakTask, unavailable};
use crate::server::{core::plugin_host::wasm::{manifest::Options, managed_policy::CallbackKind}, errors::GatewayError};

mod post;

pub(super) struct Lease {
    pub(super) profile: &'static str,
    pub(super) expected: String,
    pub(super) uri: String,
    pub(super) step: Option<String>,
    pub(super) official_state: Option<String>,
    pub(super) authorization_url: Option<String>,
    pub(super) passport: Option<super::catpaw::Passport>,
    pub(super) kiro: Option<super::kiro::Binding>,
    pub(super) consumed: bool,
    pub(super) queued: bool,
    pub(super) cancel: CancellationToken,
}

impl Lease {
    pub(super) fn closed(&self) -> bool { self.consumed || self.cancel.is_cancelled() }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(passport) = &self.passport { passport.poll_cancel.cancel(); }
        self.official_state.take();
    }
}

fn invalid() -> GatewayError { GatewayError::bad_request("完整回调地址与当前登录步骤不一致，未提交授权码") }

fn exact_fields(value: &Value, keys: &[&str]) -> Result<(), GatewayError> {
    if value.as_object().is_none_or(|object| object.len() != keys.len()
        || object.keys().any(|key| !keys.contains(&key.as_str()))) { return Err(invalid()); }
    Ok(())
}

pub(super) async fn open(call: &Call, params: &Value, options: &Options) -> Result<Value, GatewayError> {
    exact_fields(params, &["profile", "expected"])?;
    if params["expected"].as_str() != Some(call.expected()) { return Err(unavailable()); }
    let declaration = options.manifest.managed_login.as_ref().ok_or_else(unavailable)?;
    let profile = declaration.profile(call.provider(), call.method_index(), params["profile"].as_str().unwrap_or(""))
        .ok_or_else(|| GatewayError::with_status(403, "当前提供商、登录方式或回调地址尚未获准"))?;
    if profile.callback == CallbackKind::TraeCode {
        return Err(GatewayError::with_status(403, "Trae 固定PKCE准备必须经本次context.prepare，不能借通用loopback重绑"));
    }
    if matches!(profile.callback, CallbackKind::KiroPortal | CallbackKind::KiroAws) {
        return super::kiro::open(call, profile).await;
    }
    if profile.callback == CallbackKind::CatpawPassport {
        if ["http.v1", "http.timeout.v1"].iter().any(|capability| !options.allows(capability)) {
            return Err(GatewayError::with_status(403, "CatPaw 托管登录缺少有界网络能力声明"));
        }
        options.check_url("https://catx.nocode.cn/api/gateway/passport/login-config", false)
            .map_err(|_| GatewayError::with_status(403, "CatPaw 登录服务来源未声明"))?;
    }
    {
        let state = call.session.state.lock().map_err(|_| unavailable())?;
        if !state.leases.is_empty() { return Err(GatewayError::with_status(409, "当前登录已申请回调监听")); }
    }
    let mut bound = None;
    for port in profile.ports {
        let Ok(v4) = TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).await else { continue; };
        let actual = v4.local_addr().map_err(|_| unavailable())?.port();
        if actual == 0 { return Err(unavailable()); }
        if profile.callback == CallbackKind::CatpawPassport {
            // CatPaw 授权地址固定127.0.0.1，不绑定或猜测localhost的IPv6端口。
            bound = Some((actual, v4, None));
            break;
        }
        // localhost 可能解析为 ::1；若该族已被别的程序占用则整组跳过，不能让回调落给它。
        let v6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, actual)).await {
            Ok(listener) => Some(listener),
            Err(error) if matches!(error.kind(), std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported) => None,
            Err(_) => continue,
        };
        bound = Some((actual, v4, v6));
        break;
    }
    let (port, v4, v6) = bound.ok_or_else(|| GatewayError::with_status(409,
        if profile.callback == CallbackKind::CatpawPassport { "无法申请本次 CatPaw 登录的随机本机回调端口，未发起授权" }
        else { "官方登录所需的本地回调端口不可用，请先关闭占用端口的官方客户端后重新发起" }))?;
    call.validate(call.provider())?;
    let id = super::nonce()?;
    let uri = format!("http://{}:{port}{}", profile.host, profile.path);
    let cancel = call.session.cancel.child_token();
    let opened = if profile.callback == CallbackKind::CatpawPassport {
        Some(super::catpaw::prepare_open(call, &uri).await?)
    } else { None };
    let response = match &opened {
        Some(opened) => opened.response(&id, &uri, call.session.expires_at),
        None => json!({ "leaseId": id, "callbackUrl": uri, "expiresAt": call.session.expires_at }),
    };
    call.validate(call.provider())?;
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if !call.session.valid() || !state.leases.is_empty()
            || state.active.as_deref() != Some(call.id())
            || state.step.as_ref().is_none_or(|step| step.id != call.expected()) { return Err(unavailable()); }
        let passport = opened.map(|opened| opened.install(&mut state));
        state.leases.insert(id.clone(), Lease { profile: profile.id, expected: call.expected().into(), uri: uri.clone(),
            step: None, official_state: None, authorization_url: None, passport, kiro: None, consumed: false, queued: false, cancel: cancel.clone() });
    }
    let slots = Arc::new(Semaphore::new(4));
    if profile.callback == CallbackKind::CatpawPassport {
        post::spawn(v4, &call.task, &call.session, &id, &uri, &cancel, &slots);
    } else {
        spawn(v4, &call.task, &call.session, &id, &uri, &cancel, &slots);
        if let Some(v6) = v6 { spawn(v6, &call.task, &call.session, &id, &uri, &cancel, &slots); }
    }
    Ok(response)
}

pub(super) fn close(call: &Call, params: &Value) -> Result<Value, GatewayError> {
    exact_fields(params, &["leaseId"])?;
    let id = params["leaseId"].as_str().filter(|id| !id.is_empty()).ok_or_else(invalid)?;
    if super::trae::applies(call.provider(), call.method_index()) {
        return super::trae::close(call, id);
    }
    if super::kiro::applies(call.provider(), call.method_index()) {
        return super::kiro::close(call, id);
    }
    let mut state = call.session.state.lock().map_err(|_| unavailable())?;
    let lease = state.leases.remove(id);
    let closed = lease.is_some();
    if state.step.as_ref().is_some_and(|step| step.lease.as_deref() == Some(id)) {
        if let Some(step) = &mut state.step { step.status = "processing"; step.browser = None; step.lease = None; }
    }
    drop(state);
    drop(lease);
    if closed && super::catpaw::applies(call.provider(), call.method_index()) { call.session.close(); }
    Ok(json!({ "closed": closed }))
}

fn parse(raw: &str, limit: usize) -> Result<Url, GatewayError> {
    if raw.is_empty() || raw.len() > limit || raw.contains('\\') || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(invalid());
    }
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() { return Err(invalid()); }
    Ok(url)
}

fn query(url: &Url) -> Result<BTreeMap<String, String>, GatewayError> {
    let raw = url.query().ok_or_else(invalid)?;
    let bytes = raw.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            if offset + 2 >= bytes.len() || !bytes[offset + 1].is_ascii_hexdigit() || !bytes[offset + 2].is_ascii_hexdigit() {
                return Err(invalid());
            }
            offset += 3;
        } else { offset += 1; }
    }
    let mut params = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if params.len() >= 32 || key.is_empty() || key.len() > 128 || value.len() > 8192
            || key.chars().any(char::is_control) || value.chars().any(char::is_control) || value.contains('\u{fffd}')
            || params.insert(key.into_owned(), value.into_owned()).is_some() { return Err(invalid()); }
    }
    Ok(params)
}

fn required<'a>(params: &'a BTreeMap<String, String>, key: &str, limit: usize) -> Result<&'a str, GatewayError> {
    params.get(key).map(String::as_str).filter(|value| !value.is_empty() && value.len() <= limit
        && !value.chars().any(|ch| ch.is_control() || ch.is_whitespace())).ok_or_else(invalid)
}

pub(super) fn authorization(raw: &str, callback: &str, origins: &[&str]) -> Result<String, GatewayError> {
    let url = parse(raw, 16 * 1024)?;
    if url.scheme() != "https" || !origins.contains(&url.origin().ascii_serialization().as_str()) {
        return Err(GatewayError::with_status(403, "授权页面不在主程序批准的精确来源中"));
    }
    let params = query(&url)?;
    if params.get("redirect_uri").map(String::as_str) != Some(callback)
        || params.get("response_type").is_some_and(|value| value != "code") { return Err(invalid()); }
    Ok(required(&params, "state", 4096)?.to_string())
}

fn check(lease: &Lease, step: &str, raw: &str) -> Result<(), GatewayError> {
    if lease.passport.is_some() { return Err(invalid()); }
    if lease.closed() || lease.step.as_deref() != Some(step) { return Err(unavailable()); }
    let url = parse(raw, 8192)?;
    if raw.split_once('?').map(|(base, _)| base) != Some(lease.uri.as_str()) { return Err(invalid()); }
    let params = query(&url)?;
    if Some(required(&params, "state", 4096)?) != lease.official_state.as_deref() { return Err(invalid()); }
    if params.contains_key("error") {
        if params.contains_key("code") { return Err(invalid()); }
        required(&params, "error", 256)?;
    } else { required(&params, "code", 4096)?; }
    Ok(())
}

pub(super) fn claim(state: &mut State, id: &str, step: &str, raw: &str, manual: bool) -> Result<Option<super::kiro::Receipt>, GatewayError> {
    if state.leases.get(id).is_some_and(|lease|
        lease.profile == crate::server::core::plugin_host::wasm::managed_policy::TRAE_PROFILE) {
        return Err(GatewayError::with_status(403, "Trae 无state授权码只接受真实IPv4监听对象的私有收据"));
    }
    if state.leases.get(id).is_some_and(|lease| lease.kiro.is_some()) {
        return super::kiro::claim(state, id, step, raw, manual).map(Some);
 }
    let current = state.step.as_ref().ok_or_else(unavailable)?;
    if current.id != step || current.lease.as_deref() != Some(id) || current.status != "ready" || current.browser.is_none() {
        return Err(unavailable());
    }
    let lease = state.leases.get_mut(id).ok_or_else(unavailable)?;
    // 当前 profile 都要求 state；未来没有 state 的 PKCE flow 不能沿用手填入口。
    if manual && (lease.official_state.is_none() || lease.queued) || !manual && !lease.queued { return Err(invalid()); }
    check(lease, step, raw)?;
    lease.consumed = true;
    lease.queued = false;
    lease.cancel.cancel();
    lease.official_state.take();
    state.callback_step = Some(step.to_string());
    Ok(None)
}

fn spawn(listener: TcpListener, task: &WeakTask, session: &Arc<Session>, id: &str, uri: &str,
    cancel: &CancellationToken, slots: &Arc<Semaphore>) {
    let task = task.clone();
    let session = Arc::downgrade(session);
    let id = id.to_string();
    let uri = uri.to_string();
    let cancel = cancel.clone();
    let slots = slots.clone();
    crate::spawn_task(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                result = listener.accept() => result,
            };
            let Ok((stream, peer)) = accepted else { return; };
            if !peer.ip().is_loopback() { continue; }
            let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
            let task = task.clone();
            let session = session.clone();
            let id = id.clone();
            let uri = uri.clone();
            let cancel = cancel.clone();
            crate::spawn_task(async move {
                let _permit = permit;
                serve(stream, task, session, &id, &uri, cancel).await;
            });
        }
    });
}

async fn request(stream: &mut TcpStream, uri: &str) -> Result<String, GatewayError> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0; 1024];
    loop {
        let size = stream.read(&mut chunk).await.map_err(|_| invalid())?;
        if size == 0 { return Err(invalid()); }
        buffer.extend_from_slice(&chunk[..size]);
        if buffer.len() > 12 * 1024 { return Err(invalid()); }
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") { break; }
    }
    let end = buffer.windows(4).position(|window| window == b"\r\n\r\n").ok_or_else(invalid)?;
    if end + 4 != buffer.len() { return Err(invalid()); }
    let raw = std::str::from_utf8(&buffer[..end]).map_err(|_| invalid())?;
    let mut lines = raw.split("\r\n");
    let first = lines.next().ok_or_else(invalid)?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3 || parts[0] != "GET" || parts[2] != "HTTP/1.1" || parts[1].len() > 8192 { return Err(invalid()); }
    let base = Url::parse(uri).map_err(|_| invalid())?;
    let (path, _) = parts[1].split_once('?').ok_or_else(invalid)?;
    if path != base.path() { return Err(invalid()); }
    let authority = format!("{}:{}", base.host_str().ok_or_else(invalid)?, base.port().ok_or_else(invalid)?);
    let mut host = None;
    let mut names = HashSet::new();
    for line in lines {
        if names.len() >= 48 || line.starts_with([' ', '\t']) { return Err(invalid()); }
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') { return Err(invalid()); }
        let name = name.to_ascii_lowercase();
        if !names.insert(name.clone()) { return Err(invalid()); }
        let value = value.trim();
        if value.bytes().any(|byte| byte.is_ascii_control() && byte != b'\t') { return Err(invalid()); }
        match name.as_str() {
            "host" => host = Some(value.to_string()),
            "content-length" if value != "0" => return Err(invalid()),
            "transfer-encoding" | "expect" | "upgrade" => return Err(invalid()),
            _ => {}
        }
    }
    if host.as_deref() != Some(authority.as_str()) { return Err(invalid()); }
    Ok(format!("http://{authority}{}", parts[1]))
}

async fn serve(mut stream: TcpStream, task: WeakTask, session: Weak<Session>, id: &str, uri: &str, cancel: CancellationToken) {
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(5), request(&mut stream, uri)) => result,
    };
    let raw = match result { Ok(Ok(raw)) => raw, _ => { respond(&mut stream, false).await; return; } };
    let Some(session) = session.upgrade() else { return; };
    let accepted = {
        let mut state = session.state.lock().unwrap_or_else(|error| error.into_inner());
        let valid = session.valid() && state.runtime.as_ref().is_some_and(|runtime| !runtime.stopped.is_cancelled() && (runtime.alive)())
            && state.leases.get(id).zip(state.step.as_ref())
                .is_some_and(|(lease, step)| !lease.queued && step.status == "ready" && check(lease, &step.id, &raw).is_ok());
        if valid {
            if let Some(lease) = state.leases.get_mut(id) { lease.queued = true; }
        }
        valid
    };
    if !accepted { respond(&mut stream, false).await; return; }
    let Some(task) = task.upgrade() else { return; };
    // 不把完整回调放进全局队列或日志。操作锁下再次校验租约并一次消费。
    // 同一租约最多排一个事件；等串行锁时仍占连接许可，避免无限堆积回调任务。
    let id = id.to_string();
    let completion = crate::spawn_task(async move { super::receive_callback(task, &id, &raw).await });
    respond(&mut stream, true).await;
    let _ = completion.await;
}

async fn respond(stream: &mut TcpStream, accepted: bool) {
    let message = if accepted { "回调已交给登录任务处理，请返回面板查看最终结果。" }
        else { "未接受此回调，请返回面板检查当前登录步骤。" };
    // OPTIONS/POST/外站探测不回显请求材料，也不开放跨域读取。
    let status = if accepted { "200 OK" } else { "400 Bad Request" };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{message}", message.len());
    let _ = tokio::time::timeout(Duration::from_secs(1), stream.write_all(response.as_bytes())).await;
    let _ = stream.shutdown().await;
}
