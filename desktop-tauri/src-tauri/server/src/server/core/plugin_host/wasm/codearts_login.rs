mod protocol;
mod listener;

use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}, time::Duration};
use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use super::context::{Context,Shared};
use crate::server::core::egress::managed::FrozenHttpClient;

struct Flow {
    flow_id: String,
    shared: std::sync::Weak<Shared>,
    cancel: CancellationToken,
    until: tokio::time::Instant,
    protocol: protocol::Login,
    incoming: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Value>>,
    sender: tokio::sync::mpsc::Sender<Value>,
    state: tokio::sync::Mutex<Progress>,
    client: FrozenHttpClient,
}
struct Progress { result: Option<Value>, submitted: bool, ticket_secret: Option<String>, attempts: usize, redirected_at: Option<tokio::time::Instant> }
fn flows() -> &'static Mutex<HashMap<String,Arc<Flow>>> { static ITEMS: OnceLock<Mutex<HashMap<String,Arc<Flow>>>> = OnceLock::new(); ITEMS.get_or_init(Mutex::default) }

pub(super) async fn invoke(context: &Context,method: &str,params: &Value) -> Result<Value,proto::WireError> {
    context.permission("login.codearts.v1")?;
    if context.shared.options.provider != "codearts" { return Err(invalid()); }
    if method == "codearts.login.start" {
        if params["method"].as_str() != Some("browser")
            || params["payload"].get("methodIndex").is_some_and(|index| index.as_u64() != Some(1)) {
            return Err(invalid());
        }
        return start(context).await;
    }
    if !matches!(method, "codearts.login.poll" | "codearts.login.callback" | "codearts.login.cancel") { return Err(invalid()); }
    let id = params["flow"].as_str().filter(|id| id.starts_with("codearts_login_") && id.len() <= 128).ok_or_else(invalid)?;
    let flow = flows().lock().map_err(|_| invalid())?.get(id).cloned().ok_or_else(invalid)?;
    if flow.shared.upgrade().is_none_or(|shared| !Arc::ptr_eq(&shared,&context.shared)) { return Err(invalid()); }
    if method == "codearts.login.cancel" {
        flow.cancel.cancel();
        flows().lock().map_err(|_| invalid())?.remove(id);
        return Ok(Value::Null);
    }
    if flow.until <= tokio::time::Instant::now() || flow.cancel.is_cancelled() { return Err(invalid()); }
    if method == "codearts.login.callback" {
        let url = params["url"].as_str().or_else(|| params["code"].as_str()).ok_or_else(invalid)?;
        let value = listener::query(url,&flow.protocol.callback)?;
        let next = value["redirect"].as_str().map(str::to_string);
        let rejected = value["error"].is_string();
        flow.sender.try_send(value).map_err(|_| invalid())?;
        if rejected { return poll(&flow).await; }
        return Ok(json!({"state":"pending","nextUrl":next}));
    }
    poll(&flow).await
}

async fn start(context: &Context) -> Result<Value,proto::WireError> {
    use base64::Engine;
    let mut bytes = [0u8;24]; getrandom::getrandom(&mut bytes).map_err(|_| invalid())?;
    let id = format!("codearts_login_{}",base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.map_err(|_| invalid())?;
    let port = listener.local_addr().map_err(|_| invalid())?.port();
    let protocol = protocol::Login::new(port)?;
    let selected = crate::server::core::proxies::ResolvedProxy::from_json(&context.proxy).map_err(|_| invalid())?;
    let client = FrozenHttpClient::from_selected(selected).await.map_err(|_| invalid())?;
    let cancel = context.shared.cancel.child_token();
    let (sender,incoming) = tokio::sync::mpsc::channel(8);
    let flow = Arc::new(Flow {flow_id:id.clone(),shared:Arc::downgrade(&context.shared),cancel:cancel.clone(),until:tokio::time::Instant::now()+Duration::from_secs(300),
        protocol,incoming:tokio::sync::Mutex::new(incoming),sender,state:tokio::sync::Mutex::new(Progress{result:None,submitted:false,ticket_secret:None,attempts:0,redirected_at:None}),client});
    let url = flow.protocol.authorization();
    {
        let mut items = flows().lock().map_err(|_| invalid())?;
        items.retain(|_,flow| flow.until > tokio::time::Instant::now() && !flow.cancel.is_cancelled());
        if items.len() >= 8 { return Err(invalid()); }
        items.insert(id.clone(),flow.clone());
    }
    if let Ok(v6) = tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST,port)).await {
        listener::spawn(v6,flow.clone());
    }
    listener::spawn(listener,flow.clone());
    let flow_id = id.clone();
    let expired = flow.clone();
    crate::spawn_task(async move {
        tokio::select! { _ = tokio::time::sleep_until(expired.until) => {}, _ = expired.cancel.cancelled() => {} }
        expired.cancel.cancel();
        if let Ok(mut items) = flows().lock() { items.remove(&flow_id); }
    });
    Ok(json!({"flow":id,"url":url,"poll_interval_ms":1500,"external_browser":false,"allowed_hosts":["huaweicloud.com"],
        "instructions":"请完成华为云官方登录，完成后自动添加账号。远程面板可粘贴浏览器最后的本机回调地址；临时兜底登录无法自动续期。"}))
}

async fn poll(flow: &Arc<Flow>) -> Result<Value,proto::WireError> {
    let mut state = flow.state.lock().await;
    if let Some(value) = &state.result { return Ok(value.clone()); }
    if state.submitted { return Err(invalid()); }
    let mut incoming = flow.incoming.lock().await;
    let mut code = None;
    while let Ok(value) = incoming.try_recv() {
        if value["error"].is_string() {
            return Ok(failed(flow, &mut state, "华为云未完成本次授权，请重新发起登录；没有交换授权码或保存账号"));
        }
        if let Some(secret) = value["secret"].as_str().filter(|secret| !secret.is_empty()) { if state.ticket_secret.is_none() { state.ticket_secret = Some(secret.into()); } }
        if value["redirect"].is_string() && state.redirected_at.is_none() { state.redirected_at = Some(tokio::time::Instant::now()); }
        if let Some(value) = value["code"].as_str().filter(|code| !code.is_empty()) { code = Some(value.to_string()); break; }
    }
    drop(incoming);
    let material = if let Some(code) = code {
        state.submitted = true;
        match flow.protocol.exchange(&flow.client,&code).await {
            Ok(material) => Some(material),
            Err(_) => return Ok(failed(flow, &mut state, "CodeArts 本次授权结果未确认，请重新登录；不会再次交换旧授权码")),
        }
    } else if let Some(secret) = state.ticket_secret.clone() {
        if state.redirected_at.is_some_and(|at| at.elapsed() < Duration::from_secs(45)) { return Ok(json!({"state":"pending"})); }
        if state.attempts >= 100 { return Ok(failed(flow, &mut state, "CodeArts 临时登录未完成，请重新发起登录")); }
        state.attempts += 1;
        match flow.protocol.ticket(&flow.client,&secret).await {
            Ok(material) => material,
            Err(_) => return Ok(failed(flow, &mut state, "CodeArts 临时登录结果未确认，请重新发起登录")),
        }
    } else { None };
    let Some(material) = material else { return Ok(json!({"state":"pending"})); };
    if flow.cancel.is_cancelled() || flow.until <= tokio::time::Instant::now() { return Err(invalid()); }
    state.submitted = true;
    let account = material.account();
    let result = json!({"state":"done","account":account});
    state.result = Some(result.clone());
    Ok(result)
}

fn failed(flow: &Flow, state: &mut Progress, message: &'static str) -> Value {
    state.submitted = true;
    state.ticket_secret = None;
    let result = json!({"state":"failed","message":message});
    state.result = Some(result.clone());
    // 已失败的一次调用保留失败回执，不复用授权码或临时ticket材料。
    flow.cancel.cancel();
    result
}

fn invalid() -> proto::WireError { proto::WireError::with_code("CodeArts 登录流程失效或未完成，请重新发起；没有复用旧授权码或公开凭据","codearts_login_unavailable") }
