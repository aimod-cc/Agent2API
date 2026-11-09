use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::context::{Shared, Sink};
use super::runtime::Runtime;
use super::errors::decode as unwrap_reply;
use super::super::{Host, StreamCall, StreamEvent};

fn validate_identity(options: &super::manifest::Options, account: &Value) -> Result<(), proto::WireError> {
    if options.allows("accounts.identity.v1") && account["identity"].as_str().is_none_or(|identity|
        identity.trim().is_empty() || identity.len() > 512 || identity.contains('\0')) {
        return Err(super::error("插件必须返回稳定、非空的账号身份"));
    }
    Ok(())
}

struct LoginCleanup { shared: Arc<Shared>, flow: String }
impl Drop for LoginCleanup {
    fn drop(&mut self) {
        if self.flow.is_empty() { return; }
        self.shared.interactions.remove(&self.flow);
        if let Ok(mut state) = self.shared.login_state.lock() {
            state.remove_namespace(&format!("{}:{}", self.shared.options.provider, self.flow));
        }
        if let Ok(mut proxies) = self.shared.login_proxies.lock() { proxies.remove(&self.flow); }
    }
}

struct Active<'a>(&'a super::Embedded);
impl Drop for Active<'_> {
    fn drop(&mut self) { self.0.active.fetch_sub(1, Ordering::AcqRel); self.0.idle.notify_waiters(); }
}

impl Host {
    pub(crate) async fn start_wasm(self: &Arc<Self>) -> Result<Vec<proto::ProviderInfo>, String> {
        let options = self.options.wasm.clone().ok_or("Wasm 启动参数缺失")?;
        let shared = Shared::new(options, self.callbacks.clone());
        if let Some(attempt) = &self.options.startup { attempt.phase(super::super::startup::Phase::Loading); }
        let runtime = Runtime::load(self.options.exe.clone(), shared).await;
        let result = async {
            let runtime = runtime?;
            if let Some(attempt) = &self.options.startup {
                attempt.waiting(super::super::startup::Phase::Loading, self.options.init_timeout.as_secs());
            }
            let reply = runtime.invoke(proto::methods::gateway::INIT, super::contract(), self.options.init_timeout, None).await.map_err(|error| {
                if error.code.as_deref() == Some("timeout") {
                    if let Some(attempt) = &self.options.startup { attempt.fail(&error.message, true); }
                }
                error.message
            })?;
            let value = unwrap_reply(reply).map_err(|error| error.message)?;
            let settings = super::super::provider_settings::parse(self, &value)?;
            let providers: Vec<proto::ProviderInfo> = serde_json::from_value(value).map_err(|_| "Wasm 握手返回无效提供商信息")?;
            if providers.len() != 1 || providers[0].id.as_str() != runtime.shared.options.provider {
                return Err("Wasm 提供商身份与安装清单不一致".to_string());
            }
            let provider = &providers[0];
            for (required, capability) in [
                (provider.capabilities.chat, "chat.stream.v1"),
                (provider.capabilities.model_refresh, "catalog.v1"),
                (provider.capabilities.web_login, "login.forms.v1"),
            ] {
                if required && !runtime.shared.options.allows(capability) {
                    return Err(format!("Wasm 提供商能力缺少授权声明：{capability}"));
                }
            }
            if provider.capabilities.anonymous_default_session || provider.capabilities.env_credentials {
                return Err("Wasm 不允许隐式本机凭证，请使用显式账号或已授权的原生辅助".into());
            }
            if !runtime.shared.options.allows("accounts.cas.v1") && provider.capabilities.chat {
                return Err("Wasm 对话必须声明受控账号访问能力".into());
            }
            *self.embedded.runtime.write().await = Some(runtime);
            Ok((providers, settings))
        }.await;
        match result {
            Ok((providers, settings)) => {
                *self.providers.lock().unwrap_or_else(|error| error.into_inner()) = providers.clone();
                *self.provider_settings.lock().unwrap_or_else(|error| error.into_inner()) = settings;
                self.generation.fetch_add(1, Ordering::AcqRel);
                self.running.store(true, Ordering::Release);
                self.ready.store(true, Ordering::Release);
                *self.crashes.lock().unwrap_or_else(|error| error.into_inner()) = 0;
                *self.retry_after.lock().unwrap_or_else(|error| error.into_inner()) = None;
                *self.last_error.lock().unwrap_or_else(|error| error.into_inner()) = None;
                if let Some(attempt) = &self.options.startup { attempt.runtime_ready(); }
                Ok(providers)
            }
            Err(message) => { self.note_failure(&message); Err(message) }
        }
    }

    pub(crate) async fn call_wasm(&self, method: &str, mut params: Value, timeout: Duration) -> Result<Value, proto::WireError> {
        let _lease = self.activity.acquire().ok_or_else(|| super::error("插件正在切换或已关闭"))?;
        let runtime = self.embedded.runtime.read().await.clone().ok_or_else(|| super::error("Wasm 插件尚未加载"))?;
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if self.closing.load(Ordering::Acquire) { return Err(super::error("Wasm 插件已关闭")); }
            self.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let _active = Active(&self.embedded);
        let secret = params.get("secret").cloned();
        let cleanup = (method == "login.cancel").then(|| LoginCleanup { shared: runtime.shared.clone(),
            flow: params["flow"].as_str().unwrap_or("").to_string() });
        let _cleanup = cleanup;
        if method == "login.start" && (runtime.shared.options.allows("login.interaction.v1")
            || runtime.shared.options.allows("login.callback.v1")) {
            let mut nonce = [0u8; 32];
            getrandom::getrandom(&mut nonce).map_err(|_| super::error("无法生成安全的登录流程标识"))?;
            params["flow"] = json!(format!("login_{}", nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>()));
            if runtime.shared.options.allows("login.callback.v1") {
                let port = super::super::bootstrap::loopback_port();
                if port == 0 { return Err(super::error("网关本机回调端口尚未就绪")); }
                params["callback_url"] = json!(format!("http://127.0.0.1:{port}/api/session/login/wasm-callback"));
            }
        }
        let flow = params["flow"].as_str().unwrap_or("").to_string();
        let mut start_cleanup = (method == "login.start" && !flow.is_empty()).then(|| LoginCleanup {
            shared: runtime.shared.clone(), flow: flow.clone(),
        });
        if method == "login.interaction" { return Ok(runtime.shared.interactions.snapshot(&flow).unwrap_or(Value::Null)); }
        if matches!(method, "login.poll" | "login.callback" | "login.submit") {
            let proxies = runtime.shared.login_proxies.lock().map_err(|_| super::error("登录会话不可用"))?;
            let (_, proxy) = proxies.get(&flow).filter(|(until, _)| *until > crate::server::logging::now_ms())
                .ok_or_else(|| super::error("登录流程已取消或过期"))?;
            params["proxy"] = proxy.clone();
        }
        if method == "login.callback" && runtime.shared.options.allows("login.callback.v1") {
            let raw = params["url"].as_str().unwrap_or("");
            let url = url::Url::parse(raw).or_else(|_| url::Url::parse(&format!("http://127.0.0.1{}", raw)))
                .map_err(|_| super::error("登录回调地址无效"))?;
            let states: Vec<_> = url.query_pairs().filter(|(key, _)| key == "state").map(|(_, value)| value.into_owned()).collect();
            if states.len() != 1 || states[0] != flow { return Err(super::error("登录回调不属于当前流程")); }
        }
        if method == "login.submit" { runtime.shared.interactions.check(&flow, &params)?; }
        let proxy = params.get("proxy").cloned().unwrap_or(Value::Null);
        let submitted = (method == "login.submit").then(|| params.clone());
        let reply = runtime.invoke(method, params, timeout, None).await?;
        let result = unwrap_reply(reply)?;
        if let Some(submitted) = submitted { runtime.shared.interactions.accepted(&flow, &submitted); }
        if method == "login.start" {
            if (runtime.shared.options.allows("login.interaction.v1") || runtime.shared.options.allows("login.callback.v1"))
                && result["flow"].as_str() != Some(flow.as_str()) {
                return Err(super::error("交互式登录必须使用主程序分配的流程标识"));
            }
            let flow = result["flow"].as_str().filter(|flow| !flow.is_empty() && flow.len() <= 256)
                .ok_or_else(|| super::error("插件登录标识无效"))?;
            if start_cleanup.is_none() { start_cleanup = Some(LoginCleanup { shared: runtime.shared.clone(), flow: flow.to_string() }); }
            let now = crate::server::logging::now_ms();
            let mut proxies = runtime.shared.login_proxies.lock().map_err(|_| super::error("登录会话不可用"))?;
            for expired in proxies.iter().filter(|(_, (until, _))| *until <= now).map(|(flow, _)| flow.clone()).collect::<Vec<_>>() {
                runtime.shared.interactions.remove(&expired);
                if let Ok(mut state) = runtime.shared.login_state.lock() {
                    state.remove_namespace(&format!("{}:{}", runtime.shared.options.provider, expired));
                }
            }
            proxies.retain(|_, (until, _)| *until > now);
            if proxies.len() >= 64 { return Err(super::error("登录会话数量超过上限")); }
            proxies.insert(flow.to_string(), (now + 900_000, proxy));
        }
        if matches!(method, "login.start" | "login.poll" | "login.callback" | "login.submit") {
            let current_flow = if method == "login.start" { result["flow"].as_str().unwrap_or("") } else { &flow };
            runtime.shared.interactions.remember(current_flow, &result, &runtime.shared.options)?;
        }
        match method {
            proto::methods::gateway::ACCOUNT_VIEW => super::public_view::validate(&result, &secret.unwrap_or(Value::Null))?,
            proto::methods::gateway::ACCOUNT_NORMALIZE => {
                validate_identity(&runtime.shared.options, &result)?;
                super::public_view::validate(&result["public"], &result["secret"] )?;
            }
            proto::methods::gateway::LOGIN_POLL | proto::methods::gateway::LOGIN_CALLBACK | "login.submit" => {
                if result["account"].is_object() {
                    validate_identity(&runtime.shared.options, &result["account"])?;
                    super::public_view::validate(&result["account"]["public"], &result["account"]["secret"])?;
                }
            }
            _ => {}
        }
        if let Some(cleanup) = &mut start_cleanup { cleanup.flow.clear(); }
        Ok(result)
    }

    pub(crate) async fn open_wasm_stream(self: &Arc<Self>, params: Value) -> Result<StreamCall, String> {
        let lease = self.activity.acquire().ok_or("插件正在切换或已关闭")?;
        self.ensure().await?;
        let runtime = self.embedded.runtime.read().await.clone().ok_or("Wasm 插件尚未加载")?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let sink = Sink {
            events, credit: Arc::new(Semaphore::new(512 * 1024)), cancel: runtime.shared.cancel.child_token(),
        };
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if self.closing.load(Ordering::Acquire) { return Err("Wasm 插件已关闭".into()); }
            self.embedded.streams.lock().map_err(|_| "Wasm 流式登记不可用")?.insert(id, sink.clone());
            self.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let host = self.clone();
        crate::spawn_task(async move {
            let _lease = lease;
            let _active = Active(&host.embedded);
            let reply = runtime.invoke(proto::methods::gateway::CONVERSATION, params,
                Duration::from_secs(3600), Some(sink.clone())).await.and_then(unwrap_reply);
            let _ = sink.events.send(StreamEvent::Done(reply));
            sink.credit.close();
            host.embedded.streams.lock().unwrap_or_else(|error| error.into_inner()).remove(&id);
        });
        Ok(StreamCall { id, events: receiver })
    }

    pub(crate) fn abort_wasm(&self, id: i64) -> bool {
        let streams = self.embedded.streams.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(sink) = streams.get(&id) { sink.cancel.cancel(); sink.credit.close(); true } else { false }
    }

    pub(crate) fn credit_wasm(&self, id: i64, count: usize) {
        let streams = self.embedded.streams.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(sink) = streams.get(&id) {
            sink.credit.add_permits(count.min(512 * 1024 - sink.credit.available_permits().min(512 * 1024)));
        }
    }

    pub(crate) async fn shutdown_wasm(&self) {
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            self.closing.store(true, Ordering::Release);
            self.ready.store(false, Ordering::Release);
        }
        if let Some(runtime) = self.embedded.runtime.read().await.as_ref() {
            runtime.shared.cancel.cancel();
            runtime.shared.slots.close();
            runtime.shared.control_slots.close();
        }
        loop {
            let idle = self.embedded.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.embedded.active.load(Ordering::Acquire) == 0 { break; }
            idle.await;
        }
        self.embedded.runtime.write().await.take();
        self.running.store(false, Ordering::Release);
    }
}
