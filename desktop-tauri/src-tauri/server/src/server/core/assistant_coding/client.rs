use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use reqwest::{Client, Method, Response};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use url::Url;
use zeroize::Zeroizing;

use super::{config::{self, CodingConfig}, CodingError};
use crate::server::db::Db;

struct Token {
    value: Zeroizing<String>,
    expires: Instant,
}

pub struct HubClient {
    pub config: CodingConfig,
    http: Client,
    token: Mutex<Option<Token>>,
}

pub async fn connect(db: &Db) -> Result<Arc<HubClient>, CodingError> {
    static CACHE: OnceLock<Mutex<Option<Arc<HubClient>>>> = OnceLock::new();
    let config = config::load(db)?;
    if !config.enabled { return Err(CodingError::new(409, "编程会话尚未启用，请先设置 HAPI 连接")); }
    config::validate_url(&config.hub_url)?;
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some(client) = cache.as_ref() {
        if client.config.hub_url == config.hub_url && client.config.access_token == config.access_token {
            return Ok(client.clone());
        }
    }
    let http = Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8)).user_agent("Agent2API-HAPI/1")
        .build().map_err(|_| CodingError::new(500, "创建编程会话连接失败"))?;
    let client = Arc::new(HubClient { config, http, token: Mutex::new(None) });
    client.health().await?;
    *cache = Some(client.clone());
    Ok(client)
}

impl HubClient {
    fn url(&self, path: &[&str], query: &[(String, String)]) -> Result<Url, CodingError> {
        let mut url = Url::parse(&self.config.hub_url).map_err(|_| CodingError::new(400, "HAPI 地址无效"))?;
        url.path_segments_mut().map_err(|_| CodingError::new(400, "HAPI 地址无效"))?
            .pop_if_empty().extend(path.iter().copied());
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query.iter().map(|(k, v)| (k, v))); }
        Ok(url)
    }

    async fn bearer(&self, rejected: Option<&str>) -> Result<Zeroizing<String>, CodingError> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.expires > Instant::now() && rejected != Some(token.value.as_str()) {
                return Ok(Zeroizing::new(token.value.to_string()));
            }
        }
        let access = self.config.token()?;
        let response = self.http.post(self.url(&["api", "auth"], &[])?)
            .header("Accept-Encoding", "identity").json(&json!({ "accessToken": access.as_str() }))
            .timeout(Duration::from_secs(12)).send().await.map_err(|_| CodingError::new(502, "无法连接 HAPI 后台，请检查地址及运行状态"))?;
        let payload = read_json(response, false).await.map_err(|error| {
            if error.status == 401 { CodingError::new(502, "HAPI 连接密钥已失效，请在连接设置里重新保存") } else { error }
        })?;
        let value = payload.get("token").and_then(Value::as_str).filter(|v| !v.is_empty())
            .ok_or_else(|| CodingError::new(502, "HAPI 未返回有效的会话凭据"))?;
        let result = Zeroizing::new(value.to_string());
        *cached = Some(Token { value: Zeroizing::new(value.to_string()), expires: Instant::now() + Duration::from_secs(3 * 3600) });
        Ok(result)
    }

    pub async fn health(&self) -> Result<Value, CodingError> {
        let response = self.http.get(self.url(&["health"], &[])?)
            .header("Accept-Encoding", "identity").timeout(Duration::from_secs(10)).send().await
            .map_err(|_| CodingError::new(502, "无法连接 HAPI 后台，请确认 HAPI 已启动"))?;
        let value = read_json(response, false).await?;
        if value.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err(CodingError::new(409, "HAPI 客户端协议版本不兼容，请使用支持客户端协议 1 的版本"));
        }
        Ok(value)
    }

    pub async fn request(&self, method: Method, path: &[&str], query: &[(String, String)], body: Option<&Value>) -> Result<Value, CodingError> {
        let response = self.send(method.clone(), path, query, body, false).await?;
        let value = read_json(response, method != Method::GET).await?;
        if path == ["api", "sessions"] && value.get("sessions").is_none() {
            return Err(CodingError::new(502, "HAPI 会话列表响应不完整"));
        }
        Ok(value)
    }

    pub async fn events(&self, query: &[(String, String)]) -> Result<Response, CodingError> {
        self.send(Method::GET, &["api", "events"], query, None, true).await
    }

    async fn send(&self, method: Method, path: &[&str], query: &[(String, String)], body: Option<&Value>, stream: bool) -> Result<Response, CodingError> {
        let mut token = self.bearer(None).await?;
        for attempt in 0..2 {
            let mut request = self.http.request(method.clone(), self.url(path, query)?)
                .bearer_auth(token.as_str()).header("Accept-Encoding", "identity");
            if let Some(body) = body { request = request.json(body); }
            if !stream { request = request.timeout(Duration::from_secs(55)); }
            let response = tokio::time::timeout(Duration::from_secs(60), request.send()).await
                .map_err(|_| transport_error(method != Method::GET))?
                .map_err(|_| transport_error(method != Method::GET))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                token = self.bearer(Some(token.as_str())).await?;
                continue;
            }
            if response.status().is_success() { return Ok(response); }
            return read_json(response, method != Method::GET).await
                .and_then(|_| Err(CodingError::new(502, "HAPI 返回了异常响应")));
        }
        Err(CodingError::new(502, "HAPI 连接密钥已失效，请重新保存"))
    }
}

fn transport_error(write: bool) -> CodingError {
    if write { CodingError::uncertain("连接中断，无法确认操作是否已执行；请刷新会话确认，不要重复提交") }
    else { CodingError::new(502, "HAPI 连接中断，请检查后台和网络") }
}

async fn read_json(mut response: Response, write: bool) -> Result<Value, CodingError> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| transport_error(write))? {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            return Err(CodingError::new(502, "HAPI 返回的数据过大，请缩小消息范围"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| CodingError::new(502, "HAPI 返回的不是有效 JSON，请检查后台地址"))?;
    if !status.is_success() || value.get("success") == Some(&Value::Bool(false)) || value.get("type").and_then(Value::as_str) == Some("error") {
        let message = value.get("error").and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str)).unwrap_or("请求失败");
        let status = if status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED { 502 } else { status.as_u16() };
        return Err(CodingError::new(status, format!("HAPI：{}", message.chars().take(600).collect::<String>())));
    }
    Ok(value)
}
