//! 固定CatPaw round/running/turn与原回执stop/终态POST，单次发送不随Location换来源。
//! 短响应须有明确成功业务码；清理受理不代表远端资源已销毁。
use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use tokio::time::Instant;

use super::{error, json_text, registry::nonce};
use crate::server::core::egress::managed::FrozenHttpClient;
use super::super::catpaw_account::Account as RowSource;

const ORIGIN: &str = "https://ai.catpaw.meituan.com";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Catalog,
    Round,
    Running,
    Turn,
    Stop,
    Terminal,
}
impl Route {
    fn path(self) -> &'static str {
        match self {
            Self::Catalog => "/api/agent/maas/model-types",
            Self::Round => "/api/agent/conversation/round",
            Self::Running | Self::Terminal => "/api/agent/conversation/event",
            Self::Turn => "/api/agent/conversation/turn",
            Self::Stop => "/api/agent/conversation/turn/stop",
        }
    }
    fn accept(self) -> &'static str {
        if self == Self::Turn {
            "text/event-stream"
        } else {
            "application/json"
        }
    }
}

pub(super) struct RoundReceipt {
    conversation: String,
    route: Route,
    accepted_at: i64,
}
impl RoundReceipt {
    pub(super) fn matches(&self, conversation: &str) -> bool {
        self.route == Route::Round && self.conversation == conversation && self.accepted_at > 0
    }
}

pub(super) struct PreparedRequest {
    request: reqwest::Request,
    route: Route,
}

impl PreparedRequest {
    pub(super) fn into_catalog(self) -> Result<reqwest::Request, proto::WireError> {
        if self.route != Route::Catalog {
            return Err(error::invalid());
        }
        Ok(self.request)
    }
}

pub(super) fn prepare(
    client: &FrozenHttpClient,
    account: &RowSource,
    route: Route,
    body: &str,
) -> Result<PreparedRequest, proto::WireError> {
    let credentials =
        super::super::catpaw_account::credentials(account).map_err(|_| error::unavailable())?;
    let mut headers = HeaderMap::new();
    let value = |value: &str| HeaderValue::from_str(value).map_err(|_| error::unavailable());
    headers.insert("accept", HeaderValue::from_static(route.accept()));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("m-traceid", value(&nonce()?.replace('-', ""))?);
    headers.insert(
        "m-appkey",
        HeaderValue::from_static("fe_com.sankuai.catpaw.external.front"),
    );
    headers.insert("gray-set", HeaderValue::from_static("new-agent-sdk"));
    headers.insert("enableheartbeat", HeaderValue::from_static("true"));
    headers.insert("x-agent-version", HeaderValue::from_static("1.0.1"));
    headers.insert(
        "cookie",
        value(&format!("X-Passport-Token={}", credentials.token))?,
    );
    headers.insert("user-uid", value(credentials.user_uid)?);
    if !credentials.user_agent.is_empty() {
        headers.insert("user-agent", value(credentials.user_agent)?);
    }
    let request = client
        .client()
        .map_err(|_| error::unavailable())?
        .post(format!("{ORIGIN}{}", route.path()))
        .headers(headers)
        .body(body.to_string())
        .build()
        .map_err(|_| error::unavailable())?;
    Ok(PreparedRequest { request, route })
}

pub(super) async fn short(
    client: &FrozenHttpClient,
    prepared: PreparedRequest,
    route: Route,
    conversation: &str,
) -> Result<RoundReceipt, proto::WireError> {
    if matches!(route, Route::Catalog | Route::Turn) || prepared.route != route {
        return Err(error::invalid());
    }
    let PreparedRequest { request, .. } = prepared;
    let deadline = Instant::now() + if route == Route::Stop { Duration::from_secs(3) } else { CONTROL_TIMEOUT };
    let response = tokio::time::timeout_at(
        deadline,
        client
            .client()
            .map_err(|_| error::unknown())?
            .execute(request),
    )
    .await
    .map_err(|_| error::unknown())?
    .map_err(|_| error::unknown())?;
    check_response(&response, route)?;
    let mut response = response;
    let mut bytes = Vec::new();
    if response
        .content_length()
        .is_some_and(|size| size > RESPONSE_BYTES as u64)
    {
        return Err(error::unknown());
    }
    loop {
        let chunk = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .map_err(|_| error::unknown())?
            .map_err(|_| error::unknown())?;
        let Some(chunk) = chunk else { break };
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|total| total > RESPONSE_BYTES)
        {
            return Err(error::unknown());
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| error::unknown())?;
    let value = json_text::parse(text, RESPONSE_BYTES).map_err(|_| error::unknown())?;
    if let Err(error) = accept(&value, conversation) {
        crate::server::logging::verbose("[CatPaw]", &format!("{route:?} 原响应未确认：HTTP {}，业务码 {:?}",
            response.status().as_u16(), value.get("code").and_then(Value::as_i64)));
        return Err(error);
    }
    let accepted_at = crate::server::logging::now_ms();
    if accepted_at <= 0 {
        return Err(error::unknown());
    }
    Ok(RoundReceipt {
        conversation: conversation.to_string(),
        route,
        accepted_at,
    })
}

pub(super) async fn open_turn(
    client: &FrozenHttpClient,
    prepared: PreparedRequest,
    capture: Option<&Arc<crate::server::core::debug_traffic::TrafficCapture>>,
) -> Result<reqwest::Response, proto::WireError> {
    if prepared.route != Route::Turn {
        return Err(error::invalid());
    }
    let PreparedRequest { request, .. } = prepared;
    if let Some(capture) = capture {
        let headers = request.headers().iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("[binary]").to_string())).collect::<Vec<_>>();
        let body = request.body().and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok()).unwrap_or(Value::Null);
        capture.reset_request(request.url().as_str(), "catpaw", &headers, &body);
    }
    let response = tokio::time::timeout(
        CONTROL_TIMEOUT,
        client
            .client()
            .map_err(|_| error::unknown())?
            .execute(request),
    )
    .await
    .map_err(|_| error::unknown())?
    .map_err(|_| error::unknown())?;
    if let Some(capture) = capture { capture.attach_response(response.status().as_u16(), response.headers()); }
    check_response(&response, Route::Turn)?;
    Ok(response)
}

fn check_response(response: &reqwest::Response, route: Route) -> Result<(), proto::WireError> {
    if response.status().is_redirection()
        || response.headers().contains_key("location")
        || response.url().as_str() != format!("{ORIGIN}{}", route.path())
    {
        return Err(error::unknown());
    }
    if route == Route::Turn {
        let mut mime = response.headers().get_all("content-type").iter();
        let content = mime
            .next()
            .and_then(|value| value.to_str().ok())
            .ok_or_else(error::stream)?;
        if response.status() != reqwest::StatusCode::OK
            || mime.next().is_some()
            || !content
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(error::stream());
        }
    } else if !response.status().is_success() {
        return Err(error::rejected());
    }
    Ok(())
}

fn accept(value: &Value, conversation: &str) -> Result<(), proto::WireError> {
    let object = value.as_object().ok_or_else(error::unknown)?;
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(error::unknown)?;
    if !matches!(code, 0 | 200) {
        return Err(error::rejected());
    }
    accept_branch(value, conversation, 0)
}

fn accept_branch(value: &Value, conversation: &str, depth: usize) -> Result<(), proto::WireError> {
    if depth > 8 {
        return Err(error::unknown());
    }
    let Some(object) = value.as_object() else {
        // data:null/空标量本身不提供额外确认；只由上层明确成功码确认这次操作。
        return Ok(());
    };
    if object.get("error").is_some_and(|value| {
        !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())
    }) || value["success"] == false
        || value["failed"] == true
        || value["type"] == "error"
        || value["status"] == "failed"
    {
        return Err(error::rejected());
    }
    if let Some(code) = object.get("code") {
        let code = code.as_i64().ok_or_else(error::unknown)?;
        if !matches!(code, 0 | 200) {
            return Err(error::rejected());
        }
    }
    if object
        .get("success")
        .is_some_and(|flag| flag.as_bool() != Some(true))
        || object.get("failed").is_some_and(|flag| !flag.is_boolean())
    {
        return Err(error::unknown());
    }
    if let Some(id) = object.get("conversationId") {
        if id.as_str() != Some(conversation) {
            return Err(error::unknown());
        }
    }
    for name in ["data", "result"] {
        if let Some(inner) = object.get(name) {
            accept_branch(inner, conversation, depth + 1)?;
        }
    }
    Ok(())
}
