//! 固定本机桌面导入端点：transport 回环 + 唯一本机壳会话；不认 JSON 的 desktop 标记。
//! 所有回包只含确认范围、公开摘要及保存状态，绝不透传客户端凭据或插件原始回包。

use std::net::SocketAddr;

use axum::{body::Bytes, extract::{ConnectInfo, Query, State}, http::HeaderMap, response::Response};
use serde::Deserialize;
use serde_json::Value;

use crate::server::{access::local_session::{self, Source}, core::plugin_host::wasm::native,
    errors::management_error, http::ok_json, ServerState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm {
    intent: String,
    confirmed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    intent: String,
}

fn source(headers: &HeaderMap, peer: SocketAddr) -> Result<Source, native::ImportError> {
    if !peer.ip().is_loopback() { return Err(native::ImportError::forbidden()); }
    // 不使用Forwarded/X-Forwarded-For，不接受普通panel cookie或API key；单个真实header才可继续。
    let mut tokens = headers.get_all("x-panel-token").iter();
    let token = tokens.next().and_then(|token| token.to_str().ok()).ok_or_else(native::ImportError::forbidden)?;
    if tokens.next().is_some() || token.len()!=64 || !token.bytes().all(|byte|byte.is_ascii_hexdigit()) {
        return Err(native::ImportError::forbidden());
    }
    local_session::capture(token).ok_or_else(native::ImportError::forbidden)
}
fn response(result: Result<Value, native::ImportError>) -> Response {
    match result { Ok(value)=>ok_json(value), Err(error)=>management_error(error.status(),error.message()) }
}
fn parse<T: for<'de> Deserialize<'de>>(bytes: &Bytes) -> Result<T, native::ImportError> {
    if bytes.is_empty() || bytes.len()>4096 { return Err(native::ImportError::invalid_request()); }
    serde_json::from_slice(bytes).map_err(|_|native::ImportError::invalid_request())
}

pub(crate) async fn prepare(ConnectInfo(peer):ConnectInfo<SocketAddr>, headers:HeaderMap, body:Bytes)->Response {
    let manager=match source(&headers,peer){Ok(source)=>source,Err(error)=>return response(Err(error))};
    let input:Value=match parse(&body){Ok(value)=>value,Err(error)=>return response(Err(error))};
    if input.as_object().is_none_or(|fields|!fields.is_empty()) {
        return response(Err(native::ImportError::invalid_request()));
    }
    // 仅安装来源/内存确认资料；prepare不调用读取器或账号库。
    response(native::desktop_import_prepare(manager))
}

pub(crate) async fn confirm(State(state):State<ServerState>, ConnectInfo(peer):ConnectInfo<SocketAddr>, headers:HeaderMap, body:Bytes)->Response {
    let manager=match source(&headers,peer){Ok(source)=>source,Err(error)=>return response(Err(error))};
    let input:Confirm=match parse(&body){Ok(value)=>value,Err(error)=>return response(Err(error))};
    if !input.confirmed {return response(Err(native::ImportError::invalid_request()));}
    // 请求返回不持有可重放凭据；唯一后台操作与status使用原本机会话绑定。
    response(native::desktop_import_confirm(manager,&input.intent,state.store().clone()))
}

pub(crate) async fn cancel(ConnectInfo(peer):ConnectInfo<SocketAddr>, headers:HeaderMap, body:Bytes)->Response {
    let manager=match source(&headers,peer){Ok(source)=>source,Err(error)=>return response(Err(error))};
    let input:Intent=match parse(&body){Ok(value)=>value,Err(error)=>return response(Err(error))};
    response(native::desktop_import_cancel(manager,&input.intent))
}

pub(crate) async fn status(ConnectInfo(peer):ConnectInfo<SocketAddr>, headers:HeaderMap, Query(input):Query<Intent>)->Response {
    let manager=match source(&headers,peer){Ok(source)=>source,Err(error)=>return response(Err(error))};
    response(native::desktop_import_status(manager,&input.intent))
}
