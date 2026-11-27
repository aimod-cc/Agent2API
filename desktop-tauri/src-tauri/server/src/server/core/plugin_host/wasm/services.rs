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
        if let Some(call) = &self.verification {
            call.check_context(self)?;
            return Ok(&self.account);
        }
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

    pub async fn invoke(&mut self, method: &str, params: Value) -> Result<Value, proto::WireError> {
        self.invoke_at(method, params, tokio::time::Instant::now()).await
    }

    pub(super) async fn invoke_at(&mut self, method: &str, params: Value, started: tokio::time::Instant) -> Result<Value, proto::WireError> {
        if let Some(call) = &self.desktop_import { call.validate(&self.shared)?; }
        if let Some(call) = &self.desktop_launch {
            call.validate(self)?;
            if !call.preparing() && !matches!(method, "account.get" | "desktop.launch.context" | "clock.now") {
                return Err(proto::WireError::with_code("本机启动资格检查只能读取账号，不允许刷新或修改", "forbidden"));
            }
        }
        self.calls = self.calls.saturating_add(1);
        self.spin_calls = self.spin_calls.saturating_add(1);
        if self.spin_calls > 100_000 { return Err(proto::WireError::with_code("插件调用次数超出上限", "plugin_resource_limit")); }
        if method != "clock.now" && !params.is_object() { return Err(super::error("插件能力参数必须是 JSON 对象")); }
        // started 由 ABI 入口在解码前记录；权限、账号读取及正文准备不能另起计时。
        let opening = matches!(method, "http.open" | "http.open_account");
        let deadline = if opening { super::http::deadline(self, &params, started)? }
            else if method == "http.read" {
                self.resource(&params, "http")?;
                self.http_handles.get(text(&params, "id")).map(|handle| handle.deadline()).unwrap_or_default()
            } else { script::HttpDeadline::default() };
        let previous: Vec<String> = if opening { self.http_handles.keys().cloned().collect() } else { Vec::new() };
        let read_id = (method == "http.read").then(|| text(&params, "id").to_string());
        let result = deadline.run_until(Box::pin(self.invoke_inner(method, params, deadline))).await;
        // 传输层已在 EOF 原子结束期限；完成后的服务收尾不能把成功改判成超时。
        let eof = method == "http.read" && result.as_ref().is_ok_and(|value| value["done"] == true);
        let result = if eof { result } else { match deadline.check() { Ok(()) => result, Err(error) => Err(error) } };
        if let Ok(value) = &result {
            match method {
                "ws.read" => self.progress.websocket_read(value),
                "catpaw.turn.read" => self.progress.catpaw_read(value),
                "managed_task.read" => self.progress.task_read(value),
                _ => {}
            }
        }
        if let Some(id) = read_id.filter(|_| result.is_err()) { self.forget_http(&id); }
        if opening && result.is_err() {
            // 包括 keep_http 完成后外层期限复核失败：未交付给插件的句柄不能留下隐形终态。
            let created: Vec<String> = self.http_handles.keys().filter(|id| !previous.contains(id)).cloned().collect();
            for id in created { self.forget_http(&id); }
        }
        result
    }

    async fn invoke_inner(&mut self, method: &str, mut params: Value, deadline: script::HttpDeadline) -> Result<Value, proto::WireError> {
        if let Some(call) = &self.doubao_rewards {
            if method != "claim.doubao.tasks" || params.as_object().is_none_or(|fields| !fields.is_empty()) {
                return Err(proto::WireError::with_code("豆包任务领奖只接受本次原调用的固定入口", "forbidden"));
            }
            return super::doubao_rewards::exchange(call.clone()).await;
        }
        if method == "claim.doubao.tasks" {
            return Err(proto::WireError::with_code("当前调用没有豆包原任务领奖来源", "forbidden"));
        }
        if let Some(result) = self.verification_guard(method, &params).await? { return Ok(result); }
        if let Some(call) = &self.managed { call.validate(&self.shared.options.provider).map_err(super::managed::wire)?; }
        let provider = self.shared.options.provider.clone();
        match method {
            "desktop.launch.context" => {
                self.permission("desktop.launch.v1")?;
                if params.as_object().is_none_or(|fields| !fields.is_empty()) { return Err(super::error("本机启动上下文查询不接受参数")); }
                Ok(self.desktop_launch.as_ref().ok_or_else(|| super::error("当前没有本机启动确认来源"))?.context())
            }
            "desktop.launch.read" => {
                self.permission("desktop.launch.v1")?;
                if params.as_object().is_none_or(|fields| fields.len() != 1 || !fields.contains_key("resource")) { return Err(super::error("本机启动读取参数无效")); }
                let call = self.desktop_launch.as_ref().ok_or_else(|| super::error("当前没有本机启动确认来源"))?.clone();
                let resource = params["resource"].as_str().ok_or_else(|| super::error("客户端资源标识无效"))?.to_string();
                let result = crate::server::core::blocking::run(move || call.read(&resource)).await
                    .map_err(|_| super::error("本机启动凭证读取中断"))?.map_err(super::error)?;
                self.desktop_launch.as_ref().ok_or_else(|| super::error("本机启动来源已失效"))?.validate(self)?;
                Ok(result)
            }
            "credential.kiro.external.refresh" => super::external_refresh::invoke(self, &params).await,
            "crypto.deepseek.pow.solve" => super::deepseek_pow::solve(self, &params).await,
            "crypto.monkeycode.cap.solve" => super::monkeycode_cap::solve(self, &params).await,
            "resource.cookie-file.take" => {
                self.permission("resources.cookie-file.v1")?;
                if provider != "kuku" || params.as_object().is_none_or(|fields| !fields.is_empty()) {
                    return Err(super::error("Cookie文件解析只接受本次导入的固定材料"));
                }
                let raw = self.supplied_cookie_file.take().ok_or_else(|| super::error("本次没有用户选定的Cookie数据库，或材料已经使用"))?;
                crate::server::core::blocking::run(move || super::desktop_credentials::cookie_file::parse(raw)).await
                    .map_err(|_| super::error("Cookie文件解析已中断"))?
            }
            "desktop.credentials.read" => super::desktop_credentials::invoke(self, &params).await,
            "doubao.image.upload" => super::doubao_upload::upload(self, &params).await,
            "doubao.turn.receipt" | "doubao.turn.finish" => super::doubao_turn::invoke(self, method, &params).await,
            "codearts.session.open" | "codearts.session.close" => self.codearts_session(method, &params).await,
            "codearts.login.start" | "codearts.login.poll" | "codearts.login.callback" | "codearts.login.cancel" => super::codearts_login::invoke(self, method, &params).await,
            "catpaw.turn.prepare" | "catpaw.turn.start" | "catpaw.turn.read" | "catpaw.turn.settle" | "catpaw.turn.close" => {
                self.catpaw_turn_service(method, params).await
            }
            "managed_task.create" | "managed_task.observe" | "managed_task.read" | "managed_task.status" | "managed_task.close" => {
                self.managed_task_service(method, &params).await
            }
            "login.desktop.credentials.take" => {
                self.permission("login.desktop.chatglm.v1")?;
                self.desktop_import.as_ref().ok_or_else(||
                    proto::WireError::with_code("当前调用没有本机桌面导入材料", "forbidden"))?
                    .take(&self.shared, &params)
            }
            "login.context" | "login.input.take" | "login.step.publish" | "login.loopback.open" | "login.loopback.close"
                | "login.loopback.kiro.advance" | "login.trae.context.prepare" | "login.trae.exchange" | "login.kiro.exchange" | "login.autoclaw.authorization" => {
                self.managed_service(method, &params).await
            }
            "request.context" => {
                self.permission("request.context.v1")?;
                if params.as_object().is_none_or(|fields| !fields.is_empty()) {
                    return Err(proto::WireError::with_code("请求上下文查询不接受参数", "bad_params"));
                }
                let client_stream = self.client_stream.ok_or_else(||
                    proto::WireError::with_code("当前调用没有原始对话请求上下文", "forbidden"))?;
                Ok(json!({ "client_stream": client_stream }))
            }
            "runtime.platform" => {
                self.permission("runtime.platform.v1")?;
                if params.as_object().is_none_or(|fields| !fields.is_empty()) {
                    return Err(proto::WireError::with_code("系统类型查询不接受参数", "bad_params"));
                }
                Ok(json!(match std::env::consts::OS {
                    "windows" => "win",
                    "macos" => "mac",
                    "linux" => "linux",
                    _ => "other",
                }))
            }
            "runtime.architecture" | "runtime.client_fingerprint" => {
                self.permission(if method == "runtime.architecture" { "runtime.architecture.v1" }
                    else { "runtime.client-fingerprint.v1" })?;
                if params.as_object().is_none_or(|fields| !fields.is_empty()) {
                    return Err(proto::WireError::with_code("运行环境查询不接受参数", "bad_params"));
                }
                if method == "runtime.architecture" { Ok(super::runtime_info::architecture()) }
                else { super::runtime_info::client_fingerprint() }
            }
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
                let mut record = self.shared.callbacks.store_get(provider, self.account.clone()).await?;
                if let Some(call) = &self.desktop_launch {
                    if let Err(error) = call.validate(self).map_err(|error| error.message).and_then(|_| call.observe_account(&record)) {
                        super::desktop_launch::wipe(&mut record);
                        return Err(super::error(error));
                    }
                }
                self.account = record["id"].as_str().filter(|id| !id.is_empty())
                    .ok_or_else(|| super::error("账号记录缺少标识"))?.to_string();
                self.account()?;
                self.identity = record["identity"].as_str().unwrap_or(&self.account).to_string();
                if self.desktop_launch.is_none()
                    && self.doubao_rewards.is_none() {
                    super::legacy_secret::expose(self, &mut record);
                }
                self.observe_origin_account(&record)?;
                self.user_agent = Some(crate::server::core::user_agent::override_value(
                    &self.shared.options.provider,
                    record["public"]["userAgent"].as_str().unwrap_or(""),
                ).to_string());
                if !self.proxy_selected {
                    self.proxy = match record["proxy"]["state"].as_str() {
                        Some("failed") => return Err(super::error("账号代理不可用")),
                        Some("resolved") => record["proxy"]["proxy"].clone(),
                        Some("static") => script::resolve_proxy(&json!({ "config": record["proxy"]["config"] }))?,
                        _ => Value::Null,
                    };
                    self.system_proxy_when_unset &= record["proxy"]["state"] == "unset";
                    self.proxy_selected = true;
                }
                let mut record = record;
                if let Some(map) = record.as_object_mut() {
                    map.remove("proxy");
                    map.insert("usageQueryCall".to_string(), json!(self.usage_query));
                    map.insert("accountMaintenanceCall".to_string(), json!(self.account_maintenance));
                }
                Ok(record)
            }
            "account.update" => {
                self.permission("accounts.cas.v1")?;
                if params.to_string().len() > 512 * 1024 { return Err(super::error("插件账号写入超过大小限制")); }
                let account = self.account()?.to_string();
                let legacy_expect = params["expect"].as_str().is_some_and(|raw| raw.len() <= 256 * 1024
                    && serde_json::from_str::<Value>(raw).is_ok_and(|value| value.is_object()));
                if !params["secret"].is_object() || !params["expect"].is_object() && !legacy_expect || !params["public"].is_object() {
                    return Err(super::error("账号写入必须包含凭据原值、新值和公开视图"));
                }
                super::public_view::validate(&params["public"], &params["secret"])?;
                self.protect_origin_update(&params["expect"], &params["secret"])?;
                let result = if let Some(call) = self.desktop_launch.clone() {
                    let expect = super::desktop_launch::Secret(params["expect"].clone());
                    let secret = super::desktop_launch::Secret(params["secret"].clone());
                    let public = params["public"].clone();
                    crate::server::core::blocking::run(move || call.update(&expect, &secret, &public)).await
                        .map_err(|_| super::error("本机启动账号写回中断"))?.map_err(super::error)?
                } else {
                    let expect = super::legacy_secret::original(self, &account, &params["expect"]);
                    self.shared.callbacks.store_update_with_public(account, expect,
                        params["secret"].clone(), params["public"].clone()).await?
                };
                if result["updated"] == true {
                    self.origin_updated(&params["secret"])?;
                    super::external_refresh::settle(self, &params["expect"], &params["secret"]).await?;
                    self.legacy_secret = None;
                }
                Ok(result)
            }
            "ledger.get" | "ledger.put" => {
                self.permission("accounts.cas.v1")?;
                if method == "ledger.put" && params["value"].to_string().len() > 64 * 1024 {
                    return Err(super::error("插件台账超过大小限制"));
                }
                let account = self.account()?.to_string();
                let kind = text(&params, "kind").to_string();
                if kind.len() > 80 || kind.is_empty() { return Err(super::error("台账名称无效")); }
                if method == "ledger.get" {
                    let store = crate::server::core::account_store::plugin_accounts::shared()
                        .ok_or_else(|| super::error("插件台账账号存储尚未就绪，未返回空台账"))?;
                    crate::server::core::blocking::run(move || store.plugin_ledger_strict(&provider, &account, &kind))
                        .await.map_err(|_| super::error("插件台账读取已中断，未返回空台账"))?.map_err(super::error)
                } else { self.shared.callbacks.ledger_put(account, kind, params["value"].clone()).await?; Ok(Value::Null) }
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
                let lock_started = std::time::Instant::now();
                let guard = tokio::time::timeout_at(self.call_deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(10)),
                    self.shared.account_lock(&key).lock_owned()).await
                    .map_err(|_| {
                        crate::server::logging::log_with_level(
                            "[Plugin]",
                            &format!(
                                "⚠️ 插件账号锁排队超时 (503 plugin_busy): 提供商={provider} 范围={} 等待={}ms",
                                if scope.is_empty() { "account" } else { scope },
                                lock_started.elapsed().as_millis(),
                            ),
                            "warn",
                        );
                        proto::WireError::with_code("当前账号还有其他操作，等待账号锁超时，请稍后重试", "plugin_busy")
                    })?;
                self.guards.insert("account".into(), guard);
                Ok(json!("account"))
            }
            "lock.release" => { self.guards.remove(text(&params, "id")); Ok(Value::Null) }
            "session.put" | "session.get" | "session.remove" => {
                self.permission("sessions.v1")?;
                let now = crate::server::logging::now_ms();
                let mut sessions = self.shared.sessions.lock().map_err(|_| super::error("插件临时会话不可用"))?;
                let mut memory = self.shared.session_memory.lock().map_err(|_| super::error("插件临时会话不可用"))?;
                sessions.retain(|_, (until, _)| *until > now);
                memory.retain(|id, _| sessions.contains_key(id));
                let id = text(&params, "id");
                if id.is_empty() || id.len() > 256 { return Err(super::error("插件临时会话标识无效")); }
                if method == "session.remove" { sessions.remove(id); memory.remove(id); return Ok(Value::Null); }
                if method == "session.get" { return Ok(sessions.get(id).map(|(_, data)| data.clone()).unwrap_or(Value::Null)); }
                if params["value"].to_string().len() > 64 * 1024 {
                    return Err(super::error("插件临时会话超过限额"));
                }
                let ttl = params["ttl_ms"].as_i64().unwrap_or(300_000).clamp(1000, 900_000);
                let allocation = crate::server::core::runtime_capacity::Allocation::new(
                    crate::server::core::runtime_capacity::value_bytes(&params["value"]).saturating_add(id.len() + 128))?;
                memory.insert(id.to_string(), allocation);
                sessions.insert(id.to_string(), (now + ttl, params["value"].clone()));
                Ok(Value::Null)
            }
            "native.call" => Box::pin(self.native(&params)).await,
            "state.get" | "state.compare_exchange" => self.state(method, &params).await,
            "verification.lease.open" | "verification.lease.check" | "verification.lease.close" => self.verification_lease(method, &params).await,
            "protocol.request" | "protocol.open" | "protocol.push" | "protocol.close" => self.protocol(method, &params),
            "resource.fetch_public" => {
                self.permission("resources.fetch-public.v1")?;
                self.permission("resources.v1")?;
                if params.as_object().is_none_or(|object| object.len() != 1 || !object.contains_key("url")) {
                    return Err(proto::WireError::with_code("公共图片下载只接受 url 参数", "bad_params"));
                }
                let url = params["url"].as_str().ok_or_else(||
                    proto::WireError::with_code("公共图片下载地址必须是文本", "bad_params"))?;
                let ordinary = super::managed_task::OrdinaryPolicy::from_options(&self.shared.options)?;
                self.blobs.fetch_public(url, ordinary).await
            }
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
            "http.open_account" => self.open_account_origin(&params, deadline).await,
            "http.open" | "ws.open" => {
                let websocket = method == "ws.open";
                self.permission(if websocket { "websocket.v1" } else { "http.v1" })?;
                if self.resources.len() >= 8 { return Err(super::error("单次插件调用连接数超限")); }
                let ordinary = super::managed_task::OrdinaryPolicy::from_options(&self.shared.options)?;
                super::http::guard_ordinary(ordinary, &params)?;
                super::http::guard_token_exchange(self, &params)?;
                self.shared.options.check_url(text(&params, "url"), websocket)?;
                if self.user_agent.is_none() && !self.account.is_empty() {
                    let account = self.account()?.to_string();
                    let record = self.shared.callbacks.store_get(provider.clone(), account).await?;
                    self.user_agent = Some(crate::server::core::user_agent::override_value(
                        &self.shared.options.provider,
                        record["public"]["userAgent"].as_str().unwrap_or(""),
                    ).to_string());
                }
                if !websocket { deadline.check()?; }
                let user_agent = crate::server::core::user_agent::override_value(
                    &self.shared.options.provider, self.user_agent.as_deref().unwrap_or(""),
                );
                if !user_agent.is_empty() {
                    if params.get("headers").is_none_or(Value::is_null) { params["headers"] = json!({}); }
                    let headers = params["headers"].as_object_mut().ok_or_else(|| super::error("插件请求头格式无效"))?;
                    headers.retain(|name, _| !name.eq_ignore_ascii_case("user-agent"));
                    headers.insert("User-Agent".to_string(), json!(user_agent));
                }
                if params["body"].as_str().is_some_and(|body| body.len() > super::MAX_MESSAGE) {
                    return Err(super::error("插件请求体过大"));
                }
                if !websocket
                    && self.conversation.is_some()
                    && !self.upstream_protocol_explicit
                    && params["method"].as_str().unwrap_or("GET").eq_ignore_ascii_case("POST")
                {
                    if let Some(inferred) = infer_upstream_protocol_from_url(text(&params, "url")) {
                        self.note_upstream_protocol(inferred, false);
                    }
                }
                let id = format!("wasm-{}", crate::server::core::protocol::random_id("net"));
                // 沿用旧入口：这些字段始终由宿主覆盖；绝对 deadline 只走 Rust 类型，不读 JSON。
                params["id"] = json!(id);
                params["proxy"] = self.proxy.clone();
                params["provider"] = json!(provider);
                params["trace"] = self.trace.clone();
                params["max_chunk"] = json!(64 * 1024);
                super::http::guard_ordinary(ordinary, &params)?;
                if websocket {
                    self.resources.insert(id.clone(), "ws");
                    let result = script::open_ws_with_policy(&params, self.system_proxy_when_unset).await;
                    if result.is_err() { self.resources.remove(&id); }
                    result
                } else {
                    if let Some((result, handle)) = self.workbuddy_http(&params, deadline).await? {
                        let id = handle.id().to_string();
                        self.attach_http_rejection_observer(&id, &result);
                        return self.keep_http(result, handle, deadline);
                    }
                    if let Some((result, handle)) = self.doubao_http(&params, deadline).await? {
                        return self.keep_http(result, handle, deadline);
                    }
                    if provider == "codearts" && params["url"].as_str().is_some_and(|raw| raw.starts_with("https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/chat/completions")) {
                        let session = self.codearts_session.as_ref().ok_or_else(|| super::error("对话缺少本次 CodeArts 心跳会话"))?;
                        if params["headers"]["User-Session-Id"].as_str() != Some(session.id.as_str()) { return Err(super::error("CodeArts 对话的会话编号与心跳不符")); }
                        let client = session.client.client().map_err(|_| super::error("CodeArts 原出口已关闭"))?.clone();
                        let body = super::http::body(self, &params, deadline)?;
                        deadline.check()?;
                        self.guard_http_submission(&params)?;
                        let opened = script::open_pinned_body(&params, body, client, deadline.cap_at(self.call_deadline)).await;
                        self.finish_http_submission(&opened);
                        let (result, handle) = opened?;
                        return self.keep_http(result,handle,deadline);
                    }
                    let body = super::http::body(self, &params, deadline)?;
                    deadline.check()?;
                    self.guard_http_submission(&params)?;
                    let opened = script::open_body_with_policy(&params, body, self.system_proxy_when_unset, deadline).await;
                    self.finish_http_submission(&opened);
                    let (result, handle) = opened?;
                    self.keep_http(result, handle, deadline)
                }
            }
            "http.read" => {
                let id = self.resource(&params, "http")?;
                let result = match self.http_handles.get(&id) {
                    Some(handle) => handle.read().await,
                    None => Err(super::error("当前调用的 HTTP 连接已结束")),
                };
                self.observe_http_rejection(&id, &result);
                if let (Some(turn),Ok(value)) = (&mut self.doubao_turn,&result) {
                    if let Err(error) = super::doubao_turn::observe(turn,&id,value) { self.forget_http(&id); return Err(error); }
                }
                if let Ok(value) = &result { self.progress.http_read(&id, value); }
                if result.as_ref().is_ok_and(|value| value["done"] == true) || result.is_err() {
                    self.forget_http(&id);
                }
                result
            }
            "http.close" => {
                let id = self.resource(&params, "http")?;
                self.forget_http(&id);
                Ok(Value::Null)
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
                if self.conversation.is_some() && !self.upstream_protocol_noted {
                    self.note_upstream_protocol("proprietary", false);
                }
                if (200..400).contains(&status) {
                    self.confirm_stream_started();
                }
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
                    self.progress.output(chunk.len());
                }
                Ok(Value::Null)
            }
            _ => Err(proto::WireError::with_code("插件请求了未支持的主程序能力", proto::WireError::NO_METHOD)),
        }
    }
}

fn infer_upstream_protocol_from_url(raw: &str) -> Option<&'static str> {
    let parsed = url::Url::parse(raw).ok()?;
    let path = parsed.path().trim_end_matches('/').to_ascii_lowercase();
    if path.ends_with("/chat/completions") {
        Some("chat_completions")
    } else if path.ends_with("/responses") {
        Some("responses")
    } else if path.ends_with("/messages") {
        Some("anthropic")
    } else if path.contains(":streamgeneratecontent")
        || path.contains(":generatecontent")
        || path.ends_with("/generatecontent")
    {
        Some("gemini")
    } else if path.ends_with("/images/generations") || path.ends_with("/images/edits") {
        Some("images")
    } else {
        None
    }
}
