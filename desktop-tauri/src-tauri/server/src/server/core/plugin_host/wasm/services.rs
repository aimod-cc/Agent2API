use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

use super::context::Context;
use super::super::script;

fn text<'a>(value: &'a Value, key: &str) -> &'a str { value[key].as_str().unwrap_or("") }

impl Context {
    pub(super) fn permission(&self, name: &str) -> Result<(), proto::WireError> {
        if self.shared.options.allows(name) { Ok(()) }
        else { Err(proto::WireError::with_code("插件未获此能力授权", "forbidden")) }
    }

    pub(super) fn account(&self) -> Result<&str, proto::WireError> {
        if self.account.is_empty() { return Err(super::error("当前调用没有账号")); }
        let provider = &self.shared.options.provider;
        if !self.shared.callbacks.provider_allowed(provider)
            || self.shared.callbacks.account_provider(&self.account).as_ref() != Some(provider)
        {
            return Err(proto::WireError::with_code("账号不属于当前插件", "forbidden"));
        }
        Ok(&self.account)
    }

    fn resource(&self, params: &Value, kind: &str) -> Result<String, proto::WireError> {
        let id = text(params, "id");
        if self.resources.get(id).copied() != Some(kind) {
            return Err(proto::WireError::with_code("连接不属于当前调用", "forbidden"));
        }
        Ok(id.to_string())
    }

    pub async fn invoke(&mut self, method: &str, mut params: Value) -> Result<Value, proto::WireError> {
        self.calls += 1;
        if self.calls > 100_000 { return Err(proto::WireError::with_code("插件调用次数超出上限", "plugin_resource_limit")); }
        if method != "clock.now" && !params.is_object() { return Err(super::error("插件能力参数必须是 JSON 对象")); }
        let provider = self.shared.options.provider.clone();
        match method {
            "clock.now" => { self.permission("clock.v1")?; Ok(json!(crate::server::logging::now_ms())) }
            "random.bytes" => {
                self.permission("random.v1")?;
                let count = params["length"].as_u64().filter(|n| *n > 0 && *n <= 256)
                    .ok_or_else(|| super::error("随机字节长度无效"))? as usize;
                let mut bytes = vec![0; count];
                getrandom::getrandom(&mut bytes).map_err(|_| super::error("系统随机数不可用"))?;
                Ok(json!(base64::engine::general_purpose::STANDARD.encode(bytes)))
            }
            "account.get" => {
                self.permission("accounts.cas.v1")?;
                if self.account.is_empty() {
                    return Err(proto::WireError::with_code("当前操作未被主程序授予账号上下文", "forbidden"));
                }
                let record = self.shared.callbacks.store_get(provider, self.account.clone()).await?;
                self.account = record["id"].as_str().filter(|id| !id.is_empty())
                    .ok_or_else(|| super::error("账号记录缺少标识"))?.to_string();
                self.account()?;
                self.identity = record["identity"].as_str().unwrap_or(&self.account).to_string();
                self.user_agent = Some(record["public"]["userAgent"].as_str().unwrap_or("").trim().to_string());
                if !self.proxy_selected {
                    self.proxy = match record["proxy"]["state"].as_str() {
                        Some("failed") => return Err(super::error("账号代理不可用")),
                        Some("resolved") => record["proxy"]["proxy"].clone(),
                        Some("static") => script::resolve_proxy(&json!({ "config": record["proxy"]["config"] }))?,
                        _ => Value::Null,
                    };
                    self.proxy_selected = true;
                }
                let mut record = record;
                if let Some(map) = record.as_object_mut() { map.remove("proxy"); }
                Ok(record)
            }
            "account.update" => {
                self.permission("accounts.cas.v1")?;
                if params.to_string().len() > 512 * 1024 { return Err(super::error("插件账号写入超过大小限制")); }
                let account = self.account()?.to_string();
                if !params["secret"].is_object() || !params["expect"].is_object() || !params["public"].is_object() {
                    return Err(super::error("账号写入必须包含凭据原值、新值和公开视图"));
                }
                super::public_view::validate(&params["public"], &params["secret"])?;
                self.shared.callbacks.store_update_with_public(account, params["expect"].clone(),
                    params["secret"].clone(), params["public"].clone()).await
            }
            "ledger.get" | "ledger.put" => {
                self.permission("accounts.cas.v1")?;
                if method == "ledger.put" && params["value"].to_string().len() > 64 * 1024 {
                    return Err(super::error("插件台账超过大小限制"));
                }
                let account = self.account()?.to_string();
                let kind = text(&params, "kind").to_string();
                if kind.len() > 80 || kind.is_empty() { return Err(super::error("台账名称无效")); }
                if method == "ledger.get" { self.shared.callbacks.ledger_get(account, kind).await }
                else { self.shared.callbacks.ledger_put(account, kind, params["value"].clone()).await?; Ok(Value::Null) }
            }
            "settings.get" => {
                self.permission("settings.v1")?;
                let key = text(&params, "key");
                if key.len() > 128 || key.is_empty() { return Err(super::error("插件设置名称无效")); }
                self.shared.callbacks.settings_get(provider, key.to_string()).await
            }
            "catalog.get" => {
                self.permission("catalog.v1")?;
                if self.shared.options.allows("catalog.accounts.v1") {
                    return super::catalog::cached(&provider, self.account()?);
                }
                self.shared.callbacks.settings_get(provider, super::super::rpc_cache::SETTING_MODEL_CATALOG.into()).await
            }
            "lock.acquire" => {
                self.permission("locks.v1")?;
                let account = self.account()?.to_string();
                if !self.guards.is_empty() { return Err(super::error("同一次调用不能嵌套申请账号锁")); }
                let scope = text(&params, "scope");
                let key = if scope == "conversation" {
                    let session = match params.get("conversation") {
                        Some(value) => value.as_str().filter(|key| !key.is_empty() && key.len() <= 256 && !key.contains('\0'))
                            .map(|key| format!("plugin:{key}")).ok_or_else(|| super::error("会话锁标识无效"))?,
                        None => self.session.clone(),
                    };
                    if session.is_empty() { return Err(super::error("当前请求缺少会话标识")); }
                    format!("{provider}:{account}:conversation:{session}")
                } else if scope.is_empty() || scope == "account" { format!("{provider}:{account}") }
                else { return Err(super::error("锁范围无效")); };
                let guard = self.shared.account_lock(&key).lock_owned().await;
                self.guards.insert("account".into(), guard);
                Ok(json!("account"))
            }
            "lock.release" => { self.guards.remove(text(&params, "id")); Ok(Value::Null) }
            "session.put" | "session.get" | "session.remove" => {
                self.permission("sessions.v1")?;
                let now = crate::server::logging::now_ms();
                let mut sessions = self.shared.sessions.lock().map_err(|_| super::error("插件临时会话不可用"))?;
                sessions.retain(|_, (until, _)| *until > now);
                let id = text(&params, "id");
                if id.is_empty() || id.len() > 256 { return Err(super::error("插件临时会话标识无效")); }
                if method == "session.remove" { sessions.remove(id); return Ok(Value::Null); }
                if method == "session.get" { return Ok(sessions.get(id).map(|(_, data)| data.clone()).unwrap_or(Value::Null)); }
                if (sessions.len() >= 64 && !sessions.contains_key(id)) || params["value"].to_string().len() > 64 * 1024 {
                    return Err(super::error("插件临时会话超过限额"));
                }
                let ttl = params["ttl_ms"].as_i64().unwrap_or(300_000).clamp(1000, 900_000);
                sessions.insert(id.to_string(), (now + ttl, params["value"].clone()));
                Ok(Value::Null)
            }
            "native.call" => Box::pin(self.native(&params)).await,
            "state.get" | "state.compare_exchange" => self.state(method, &params).await,
            "resource.create" | "resource.append" | "resource.seal" | "resource.read" | "resource.remove" => {
                self.permission("resources.v1")?;
                let id = text(&params, "id");
                match method {
                    "resource.create" => self.blobs.create(text(&params, "mime")),
                    "resource.append" => self.blobs.append(id, text(&params, "data")),
                    "resource.seal" => self.blobs.seal(id),
                    "resource.read" => self.blobs.read(id, params["offset"].as_u64().unwrap_or(0) as usize,
                        params["length"].as_u64().unwrap_or(64 * 1024) as usize),
                    _ => { self.blobs.remove(id); Ok(Value::Null) }
                }
            }
            "clock.sleep" => {
                self.permission("clock.v1")?;
                let millis = params["milliseconds"].as_u64().filter(|ms| *ms <= 60_000)
                    .ok_or_else(|| super::error("等待时长必须在一分钟以内"))?;
                tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
                Ok(Value::Null)
            }
            "http.open" | "ws.open" => {
                let websocket = method == "ws.open";
                self.permission(if websocket { "websocket.v1" } else { "http.v1" })?;
                if self.resources.len() >= 8 { return Err(super::error("单次插件调用连接数超限")); }
                self.shared.options.check_url(text(&params, "url"), websocket)?;
                if self.user_agent.is_none() && !self.account.is_empty() {
                    let account = self.account()?.to_string();
                    let record = self.shared.callbacks.store_get(provider.clone(), account).await?;
                    self.user_agent = Some(record["public"]["userAgent"].as_str().unwrap_or("").trim().to_string());
                }
                let user_agent = self.user_agent.as_deref().unwrap_or("");
                if !user_agent.is_empty() {
                    if params.get("headers").is_none_or(Value::is_null) { params["headers"] = json!({}); }
                    let headers = params["headers"].as_object_mut().ok_or_else(|| super::error("插件请求头格式无效"))?;
                    headers.retain(|name, _| !name.eq_ignore_ascii_case("user-agent"));
                    headers.insert("User-Agent".to_string(), json!(user_agent));
                }
                if params["body"].as_str().is_some_and(|body| body.len() > super::MAX_MESSAGE) {
                    return Err(super::error("插件请求体过大"));
                }
                let id = format!("wasm-{}", crate::server::core::protocol::random_id("net"));
                params["id"] = json!(id);
                params["proxy"] = self.proxy.clone();
                params["provider"] = json!(provider);
                params["trace"] = self.trace.clone();
                params["max_chunk"] = json!(64 * 1024);
                let body = if websocket { None } else { super::http::body(self, &params)? };
                self.resources.insert(id.clone(), if websocket { "ws" } else { "http" });
                let result = if websocket { script::open_ws(&params).await } else { script::open_body(&params, body).await };
                if result.is_err() { self.resources.remove(&id); }
                result
            }
            "http.read" => {
                let id = self.resource(&params, "http")?;
                let result = script::read_http(&params).await;
                if result.as_ref().is_ok_and(|value| value["done"] == true) || result.is_err() {
                    self.resources.remove(&id);
                }
                result
            }
            "http.close" => {
                let id = self.resource(&params, "http")?;
                script::close_http(&params); self.resources.remove(&id); Ok(Value::Null)
            }
            "ws.read" | "ws.send" | "ws.close" => {
                let id = self.resource(&params, "ws")?;
                match method {
                    "ws.read" => script::read_ws(&params).await,
                    "ws.send" => script::send_ws(&params).await,
                    _ => { script::abort_ws(&params); self.resources.remove(&id); Ok(Value::Null) }
                }
            }
            "stream.head" => {
                self.permission("chat.stream.v1")?;
                let sink = self.sink.as_ref().ok_or_else(|| super::error("当前不是流式调用"))?;
                if self.head_sent { return Err(super::error("插件重复发送响应头")); }
                let status = params["status"].as_u64().and_then(|s| u16::try_from(s).ok())
                    .filter(|s| (100..600).contains(s)).ok_or_else(|| super::error("插件响应状态无效"))?;
                let headers: Vec<(String, String)> = serde_json::from_value(params["headers"].clone())
                    .map_err(|_| super::error("插件响应头格式无效"))?;
                if headers.len() > 64 || params.to_string().len() > 32 * 1024 { return Err(super::error("插件响应头过大")); }
                sink.events.send(super::super::StreamEvent::Head(proto::StreamHead {
                    status, headers: headers.into_iter().collect(), upstream: proto::UpstreamKind::Chat,
                })).map_err(|_| super::error("下游请求已结束"))?;
                self.head_sent = true;
                Ok(Value::Null)
            }
            "stream.chunk" => {
                self.permission("chat.stream.v1")?;
                if !self.head_sent { return Err(super::error("插件尚未发送响应头")); }
                let sink = self.sink.as_ref().ok_or_else(|| super::error("当前不是流式调用"))?;
                let data = text(&params, "data");
                if data.len() > 128 * 1024 { return Err(super::error("插件流式分片过大")); }
                let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| super::error("插件流式编码无效"))?;
                for chunk in bytes.chunks(64 * 1024) {
                    let credit = sink.credit.clone().acquire_many_owned((chunk.len() + proto::FRAME_OVERHEAD_BYTES) as u32).await
                        .map_err(|_| super::error("插件流已关闭"))?;
                    sink.events.send(super::super::StreamEvent::Chunk(chunk.to_vec())).map_err(|_| super::error("下游请求已结束"))?;
                    credit.forget();
                }
                Ok(Value::Null)
            }
            _ => Err(proto::WireError::with_code("插件请求了未支持的主程序能力", proto::WireError::NO_METHOD)),
        }
    }
}
