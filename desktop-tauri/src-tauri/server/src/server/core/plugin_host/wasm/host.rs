mod shutdown;

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

pub(super) fn validate_identity(options: &super::manifest::Options, account: &Value) -> Result<(), proto::WireError> {
    if options.allows("accounts.identity.v1") && account["identity"].as_str().is_none_or(|identity|
        identity.trim().is_empty() || identity.len() > 512 || identity.contains('\0')) {
        return Err(super::error("插件必须返回稳定、非空的账号身份"));
    }
    Ok(())
}

pub(super) fn login_cleanup(shared: &Shared, flow: &str) {
    if flow.is_empty() { return; }
    shared.interactions.remove(flow);
    shared.login_state.lock().unwrap_or_else(|error| error.into_inner())
        .remove_namespace(&format!("{}:{flow}", shared.options.provider));
    shared.login_proxies.lock().unwrap_or_else(|error| error.into_inner()).remove(flow);
}

fn sms_error(error: crate::server::errors::GatewayError) -> proto::WireError {
    proto::WireError::with_code(error.message, format!("provider_http:{}", json!({
        "status": error.status_code, "code": error.code, "upstreamCode": error.upstream_code,
    })))
}

struct LoginCleanup { shared: Arc<Shared>, flow: String }
impl Drop for LoginCleanup {
    fn drop(&mut self) { login_cleanup(&self.shared, &self.flow); }
}

struct Active<'a>(&'a super::Embedded);
impl Drop for Active<'_> {
    fn drop(&mut self) { self.0.active.fetch_sub(1, Ordering::AcqRel); self.0.idle.notify_waiters(); }
}

struct ConversationStream {
    host: Arc<Host>,
    id: i64,
    credit: Arc<Semaphore>,
}
impl Drop for ConversationStream {
    fn drop(&mut self) {
        self.credit.close();
        self.host.embedded.streams.lock().unwrap_or_else(|error| error.into_inner()).remove(&self.id);
        self.host.embedded.active.fetch_sub(1, Ordering::AcqRel);
        self.host.embedded.idle.notify_waiters();
    }
}

impl Host {
    pub(crate) async fn start_wasm(self: &Arc<Self>) -> Result<Vec<proto::ProviderInfo>, String> {
        let options = self.options.wasm.clone().ok_or("Wasm 启动参数缺失")?;
        let shared = Shared::new(options, self.callbacks.clone());
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if self.closing.load(Ordering::Acquire) || super::super::runtime::admission_closed() {
                return Err("Wasm 宿主关闭中，未装载新运行代次".into());
            }
            if self.embedded.runtime.try_read().map_or(true, |runtime| runtime.is_some()) {
                return Err("旧Wasm运行实例尚未完成关闭，未覆盖运行代次".into());
            }
            *self.embedded.loading.lock().unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(&shared));
        }
        if let Some(attempt) = &self.options.startup { attempt.phase(super::super::startup::Phase::Loading); }
        let runtime = Runtime::load(self.options.exe.clone(), shared.clone()).await;
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
            let error_rules = super::super::error_rules::parse(&value);
            let usage_policies = super::super::usage_policy::parse(&value);
            super::desktop_launch::validate_metadata(&runtime.shared.options, &value)?;
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
            runtime.shared.system_proxy_when_unset.store(provider.capabilities.system_proxy_when_unset, Ordering::Release);
            Ok((providers, settings, error_rules, usage_policies, runtime))
        }.await;
        match result {
            Ok((providers, settings, error_rules, usage_policies, runtime)) => {
                let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
                if self.closing.load(Ordering::Acquire) || super::super::runtime::admission_closed()
                    || runtime.shared.cancel.is_cancelled() {
                    return Err("Wasm宿主关闭中，未发布新运行代次".into());
                }
                let mut current = self.embedded.runtime.try_write().map_err(|_| "Wasm运行时发布正忙，未覆盖旧实例")?;
                if current.is_some() { return Err("旧Wasm运行实例尚未完成关闭，未覆盖运行代次".into()); }
                // Host生命周期 -> Runtime写锁 -> 安装表；只撤销属于此Host的已提交旧回执。
                // 首次安装尚无此Host的Lease时不推进revision，不能破坏自身PendingInstall。
                super::super::packages::provenance::revoke_host_runtime(self);
                *current = Some(runtime);
                // 当前Shared仍保持弱登记，关闭时不依赖可能正被借用的Runtime锁。
                *self.providers.lock().unwrap_or_else(|error| error.into_inner()) = providers.clone();
                *self.provider_settings.lock().unwrap_or_else(|error| error.into_inner()) = settings;
                *self.error_rules.lock().unwrap_or_else(|error| error.into_inner()) = error_rules;
                *self.usage_policies.lock().unwrap_or_else(|error| error.into_inner()) = usage_policies;
                self.generation.fetch_add(1, Ordering::AcqRel);
                self.running.store(true, Ordering::Release);
                self.ready.store(true, Ordering::Release);
                *self.crashes.lock().unwrap_or_else(|error| error.into_inner()) = 0;
                *self.retry_after.lock().unwrap_or_else(|error| error.into_inner()) = None;
                *self.last_error.lock().unwrap_or_else(|error| error.into_inner()) = None;
                drop(current);
                drop(_state);
                if let Some(attempt) = &self.options.startup { attempt.runtime_ready(); }
                Ok(providers)
            }
            Err(message) => { self.note_failure(&message); Err(message) }
        }
    }

    pub(in crate::server::core::plugin_host) async fn call_wasm_doubao_image(self: &Arc<Self>, params: Value,
        call: super::conversation::ConversationCall, timeout: Duration,
    ) -> Result<Value, proto::WireError> {
        let ended = call.execution_guard()?;
        let _lease = self.activity.acquire().ok_or_else(|| super::error("豆包工作插件正在切换"))?;
        let runtime = self.embedded.runtime.read().await.clone().ok_or_else(|| super::error("豆包工作插件未加载"))?;
        let rpc = self.next_id.fetch_add(1, Ordering::Relaxed);
        let call = call.bind(self, &runtime.shared, rpc)?;
        unwrap_reply(runtime.invoke_doubao_image(call, ended, params, timeout).await?)
    }

    pub(crate) async fn call_wasm(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, proto::WireError> {
        self.call_wasm_inner(method, params, timeout, None, None).await
    }

    pub(crate) async fn call_wasm_with_cancel(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Value, proto::WireError> {
        self.call_wasm_inner(method, params, timeout, None, cancel).await
    }

    pub(in crate::server::core::plugin_host) async fn call_wasm_doubao_rewards(self: &Arc<Self>,
        params: Value, timeout: Duration,
    ) -> Result<Value, proto::WireError> {
        let account = params["account"].as_str().ok_or_else(|| super::error("豆包原任务领奖缺少目标账号"))?;
        let campaign = params["claim"].as_str().ok_or_else(|| super::error("豆包原任务领奖缺少活动"))?;
        if params["provider"].as_str() != Some("doubaowork") || !self.ready() {
            return Err(super::error("豆包原任务领奖运行来源不可用"));
        }
        let runtime = self.embedded.runtime.try_read().map_err(|_| super::error("豆包原运行实例正忙"))?
            .clone().ok_or_else(|| super::error("豆包原运行实例不可用"))?;
        let call = super::doubao_rewards::Call::capture(self, &runtime, account, campaign, timeout)?;
        unwrap_reply(runtime.invoke_doubao_rewards(call, params, timeout).await?)
    }

    pub(in crate::server::core::plugin_host::wasm) async fn call_wasm_desktop_import(self: &Arc<Self>,
        prepared: super::native::chatglm::Prepared,
    ) -> Result<Value, proto::WireError> {
        let _lease = self.activity.acquire().ok_or_else(|| super::error("插件正在切换或已关闭"))?;
        let super::native::chatglm::Entered { runtime, call, params, timeout } = prepared.enter(self)?;
        let _active = Active(&self.embedded);
        let result = unwrap_reply(runtime.invoke_desktop_import(call, params, timeout).await?)?;
        validate_identity(&runtime.shared.options, &result)?;
        super::public_view::validate(&result["public"], &result["secret"])?;
        Ok(result)
    }

    /// 只接受原等待端绑定的一次签到；不从JSON注入来源，也不重新ensure/选择Runtime。
    pub(in crate::server::core::plugin_host::wasm) async fn call_wasm_checkin(
        &self, prepared: super::checkin_call::Prepared, timeout: Duration,
    ) -> Result<super::checkin_call::CallResult, proto::WireError> {
        super::checkin_call::persistence::guard(self.options.wasm.as_ref().map_or("", |options| options.provider.as_str()))?;
        let _lease = self.activity.acquire().ok_or_else(|| super::error("插件正在切换或已关闭"))?;
        let super::checkin_call::Entered { runtime, call, mut ended, params } = prepared.enter(self)?;
        let _active = Active(&self.embedded);
        let method = proto::methods::gateway::CHECKIN_CLAIM;
        // 与普通入口相同的前置保护和验证码所有者，account.get及统一解码仍由原内核完成。
        super::managed::guard_ordinary(&runtime.shared.options, method, &params)?;
        let owner = super::verification::lease::Owner::new(self, &runtime.shared, method, &params, timeout)?;
        let persistence = super::checkin_call::persistence::PersistenceSlot::for_call(&call);
        let original = call.clone();
        let reply = runtime.invoke_checkin(call, params, timeout, owner, persistence.clone()).await;
        ended.runtime_result(&reply);
        if let Some(settlement) = persistence.settle(&original).await? {
            return Ok(super::checkin_call::CallResult::Unknown(settlement));
        }
        unwrap_reply(reply?).map(super::checkin_call::CallResult::Ordinary)
    }

    pub(crate) async fn call_wasm_verification(self: &Arc<Self>, store: &crate::server::core::account_store::AccountStore,
        provider: &str, account: &str, action: &str, payload: &Value) -> Result<Value, proto::WireError> {
        use super::verification;
        let deadline = tokio::time::Instant::now() + verification::TIMEOUT;
        // 不 ensure，不等待注册表或运行时启动，也不调用 account_egress。
        if !self.ready() { return Err(verification::unavailable()); }
        let _lease = self.activity.acquire().ok_or_else(verification::unavailable)?;
        let runtime = self.embedded.runtime.try_read().map_err(|_| verification::unavailable())?
            .clone().ok_or_else(verification::unavailable)?;
        let permits = verification::slots(&runtime.shared)?;
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if super::super::runtime::admission_closed() || !self.ready() { return Err(verification::unavailable()); }
            self.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let _active = Active(&self.embedded);
        let work = async {
            let call = verification::VerificationCall::for_action(self, &runtime.shared, store,
                provider, account, action, payload, deadline, permits).await?;
            let reply = runtime.invoke_verification(call.clone()).await?;
            verification::lease::project(&call, unwrap_reply(reply)?)
        };
        tokio::select! {
            biased;
            _ = runtime.shared.cancel.cancelled() => Err(verification::unavailable()),
            result = tokio::time::timeout_at(deadline, work) => result
                .map_err(|_| proto::WireError::with_code("验证码提交超时，请重新读取当前账号状态", "timeout"))?,
        }
    }

    pub(crate) async fn call_wasm_sms(&self, call: &crate::server::core::login::sms::Call,
        action: &str, payload: Value, timeout: Duration) -> Result<Value, proto::WireError> {
        let provider = &self.options.wasm.as_ref().ok_or_else(|| super::error("不是 Wasm 短信执行体"))?.provider;
        call.validate(provider).map_err(sms_error)?;
        call.validate_action(action, &payload).map_err(sms_error)?;
        let params = json!({ "provider": provider, "account": "", "action": action, "payload": payload });
        self.call_wasm_inner("admin.action", params, timeout, Some(call), None).await
    }

    async fn call_wasm_inner(&self, method: &str, mut params: Value, timeout: Duration,
        sms: Option<&crate::server::core::login::sms::Call>,
        cancel: Option<&tokio_util::sync::CancellationToken>) -> Result<Value, proto::WireError> {
        if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
            return Err(proto::WireError::with_code(
                if method.starts_with("login.") { "登录已取消" } else { "Wasm 调用已取消" },
                proto::WireError::ABORTED,
            ));
        }
        if matches!(method, super::desktop_launch::declaration::CHECK | super::desktop_launch::declaration::PREPARE) {
            return Err(proto::WireError::with_code("本机启动必须由原桌面确认入口发起", "forbidden"));
        }
        if method == "admin.action" && super::verification::guarded_action(params["action"].as_str().unwrap_or("")) {
            return Err(proto::WireError::with_code("验证码交互必须由主程序账号验证入口发起", "forbidden"));
        }
        let _lease = self.activity.acquire().ok_or_else(|| super::error("插件正在切换或已关闭"))?;
        let runtime = self.embedded.runtime.read().await.clone().ok_or_else(|| super::error("Wasm 插件尚未加载"))?;
        if runtime.shared.options.provider == "doubaowork" && matches!(method, proto::methods::gateway::CLAIM | proto::methods::gateway::CLAIM_STATUS) {
            return Err(proto::WireError::with_code("豆包任务领奖必须由原账号固定宿主入口执行", "forbidden"));
        }
        if method == proto::methods::gateway::ACCOUNT_NORMALIZE
            && runtime.shared.options.provider == "chatglm" && params["source"]["kind"] == "desktop" {
            return Err(proto::WireError::with_code("ChatGLM 本机导入必须由当前桌面确认入口发起", "forbidden"));
        }
        super::managed::guard_ordinary(&runtime.shared.options, method, &params)?;
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if self.closing.load(Ordering::Acquire) || super::super::runtime::admission_closed() { return Err(super::error("Wasm 插件已关闭")); }
            self.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let _active = Active(&self.embedded);
        let secret = params.get("secret").cloned();
        let sms_action = method == "admin.action" && matches!(params["action"].as_str(), Some("login.sms.send" | "login.sms.verify"));
        if sms_action {
            if !runtime.shared.options.allows("login.sms.v1") {
                return Err(proto::WireError::with_code("插件未声明短信登录能力", "forbidden"));
            }
            let call = sms.ok_or_else(|| proto::WireError::with_code("短信动作必须由主程序登录任务发起", "forbidden"))?;
            call.validate(&runtime.shared.options.provider).map_err(sms_error)?;
            call.validate_action(params["action"].as_str().unwrap_or(""), &params["payload"]).map_err(sms_error)?;
            let shared = runtime.shared.clone();
            let flow = call.flow().to_string();
            let weak = Arc::downgrade(&shared);
            call.bind_runtime(Arc::as_ptr(&shared) as usize, move || {
                if let Some(shared) = weak.upgrade() { login_cleanup(&shared, &flow); }
            }).map_err(sms_error)?;
            params["provider"] = json!(runtime.shared.options.provider);
            params["account"] = json!("");
            params["flow"] = json!(call.flow());
            params["proxy"] = call.proxy();
            params["proxy_override"] = json!(call.proxy_overridden());
            if let Some(payload) = params["payload"].as_object_mut() {
                for key in ["flow", "device"] { payload.remove(key); }
            }
            if let Some(fields) = params.as_object_mut() { fields.remove("device"); }
            if runtime.shared.options.allows("login.device.v1") {
                params["device"] = crate::server::core::login::sms::device();
            }
        }
        let sms_verify = sms_action && params["action"] == "login.sms.verify";
        let cleanup = (method == "login.cancel").then(|| LoginCleanup { shared: runtime.shared.clone(),
            flow: params["flow"].as_str().unwrap_or("").to_string() });
        let _cleanup = cleanup;
        if method == "login.start" && (runtime.shared.options.allows("login.interaction.v1")
            || runtime.shared.options.allows("login.callback.v1")) {
            let mut nonce = [0u8; 32];
            getrandom::getrandom(&mut nonce).map_err(|_| super::error("无法生成安全的登录流程标识"))?;
            params["flow"] = json!(format!("login_{}", nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>()));
            if runtime.shared.options.provider == "raccoon" && params["payload"]["mode"] == "external"
                && params["payload"].get("callbackOrigin").is_none() {
                let port = super::super::bootstrap::loopback_port();
                if port == 0 { return Err(super::error("网关本机回调端口尚未就绪")); }
                params["payload"]["callbackOrigin"] = json!(format!("http://127.0.0.1:{port}"));
            }
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
            let (_, proxy, overridden) = proxies.get(&flow).filter(|(until, _, _)| *until > crate::server::logging::now_ms())
                .ok_or_else(|| super::error("登录流程已取消或过期"))?;
            params["proxy"] = proxy.clone();
            params["proxy_override"] = json!(*overridden);
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
        let proxy_overridden = params["proxy_override"] == true;
        let submitted = (method == "login.submit").then(|| params.clone());
        let claim_owner = super::verification::lease::Owner::new(self, &runtime.shared, method, &params, timeout)?;
        let reply = if let Some(call) = sms {
            let result = tokio::select! {
                biased;
                _ = call.ended() => Err(proto::WireError::with_code("短信登录已取消或过期", proto::WireError::ABORTED)),
                result = runtime.invoke(method, params, timeout, None) => result,
            };
            if let Err(error) = call.validate(&runtime.shared.options.provider) {
                login_cleanup(&runtime.shared, call.flow());
                return Err(sms_error(error));
            }
            result?
        } else if let Some(owner) = claim_owner { runtime.invoke_claim_with_cancel(method, params, timeout, owner, cancel.cloned()).await? }
        else { runtime.invoke_with_cancel(method, params, timeout, None, cancel.cloned()).await? };
        let result = unwrap_reply(reply)?;
        let sms_captcha = sms_action && result["status"] == "captcha";
        let _sms_completed = (sms_verify && !sms_captcha).then(|| LoginCleanup { shared: runtime.shared.clone(),
            flow: sms.map(|call| call.flow().to_string()).unwrap_or_default() });
        if sms_captcha && result["challenge"]["type"] == "geetest"
            && !runtime.shared.options.allows("login.captcha.geetest.v1")
        {
            return Err(proto::WireError::with_code("插件未声明人机验证能力", "forbidden"));
        }
        if sms_captcha && result["challenge"]["type"] == "alicom4"
            && (runtime.shared.options.provider != "codar"
                || !runtime.shared.options.allows("login.sms.alicom4.v1"))
        {
            return Err(proto::WireError::with_code("插件未声明人机验证能力", "forbidden"));
        }
        if sms_captcha && result["challenge"]["type"] == "external" {
            runtime.shared.options.check_url(result["challenge"]["url"].as_str().unwrap_or(""), false)?;
        }
        if sms_verify && !sms_captcha {
            validate_identity(&runtime.shared.options, &result)?;
            super::public_view::validate(&result["public"], &result["secret"])?;
        }
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
            for expired in proxies.iter().filter(|(_, (until, _, _))| *until <= now).map(|(flow, _)| flow.clone()).collect::<Vec<_>>() {
                runtime.shared.interactions.remove(&expired);
                if let Ok(mut state) = runtime.shared.login_state.lock() {
                    state.remove_namespace(&format!("{}:{}", runtime.shared.options.provider, expired));
                }
            }
            proxies.retain(|_, (until, _, _)| *until > now);
            if proxies.len() >= 64 { return Err(super::error("登录会话数量超过上限")); }
            proxies.insert(flow.to_string(), (now + 900_000, proxy, proxy_overridden));
        }
        if matches!(method, "login.start" | "login.poll" | "login.callback" | "login.submit") {
            let current_flow = if method == "login.start" { result["flow"].as_str().unwrap_or("") } else { &flow };
            runtime.shared.interactions.remember(current_flow, &result, &runtime.shared.options)?;
        }
        match method {
            proto::methods::gateway::ACCOUNT_VIEW => super::public_view::validate(&result, &secret.unwrap_or(Value::Null))?,
            proto::methods::gateway::ACCOUNT_NORMALIZE => {
                if result["desktop"] == true && !super::desktop_credentials::selected_import(&runtime.shared.options.provider) {
                    return Err(proto::WireError::with_code("本机账号只能由用户明确选择本机导入后创建", "forbidden"));
                }
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

    pub(in crate::server::core::plugin_host) async fn open_wasm_conversation(self: &Arc<Self>, params: Value,
        call: super::conversation::ConversationCall,
    ) -> Result<(StreamCall, Option<Result<super::stream_failure::Classifier, String>>), String> {
        // 在任何 await/spawn 之前建立执行终态保护，不能依赖后台首次 poll。
        let ended = call.execution_guard().map_err(|error| error.message)?;
        let lease = self.activity.acquire().ok_or("插件正在切换或已关闭")?;
        self.ensure().await?;
        let runtime = self.embedded.runtime.read().await.clone().ok_or("Wasm 插件尚未加载")?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let call = call.bind(self, &runtime.shared, id).map_err(|error| error.message)?;
        call.validate_params(&runtime.shared, &params).map_err(|error| error.message)?;
        let input = super::inbound_images::prepare(call, &runtime.shared, params).map_err(|error| error.message)?;
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let receiver = super::super::StreamEvents::wasm(receiver).map_err(|error| error.message)?;
        let sink = Sink {
            events, credit: Arc::new(Semaphore::new(512 * 1024)), cancel: runtime.shared.cancel.child_token(),
        };
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if !self.ready() || runtime.shared.cancel.is_cancelled() || super::super::runtime::admission_closed() { return Err("Wasm 插件已关闭".into()); }
            self.embedded.streams.lock().map_err(|_| "Wasm 流式登记不可用")?.insert(id, sink.clone());
            self.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let active = ConversationStream { host: self.clone(), id, credit: sink.credit.clone() };
        let classifier = (runtime.shared.options.provider == "workbuddy")
            .then(|| super::stream_failure::Classifier::capture(self, &runtime, lease.clone()));
        // Future 本体及其结束守卫先构造，再移入后台；未被 poll 就丢弃也会如实收尾。
        let invoke_sink = sink.clone();
        let invocation = async move {
            runtime.invoke_conversation(input, ended, Duration::from_secs(3600), invoke_sink).await
        };
        crate::spawn_task(async move {
            let _lease = lease;
            let _active = active;
            let reply = invocation.await.and_then(unwrap_reply);
            let _ = sink.events.send(StreamEvent::Done(reply));
        });
        Ok((StreamCall { id, events: receiver }, classifier))
    }

    pub(crate) async fn open_wasm_stream(self: &Arc<Self>, params: Value) -> Result<StreamCall, String> {
        let lease = self.activity.acquire().ok_or("插件正在切换或已关闭")?;
        self.ensure().await?;
        let runtime = self.embedded.runtime.read().await.clone().ok_or("Wasm 插件尚未加载")?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let receiver = super::super::StreamEvents::wasm(receiver).map_err(|error| error.message)?;
        let sink = Sink {
            events, credit: Arc::new(Semaphore::new(512 * 1024)), cancel: runtime.shared.cancel.child_token(),
        };
        {
            let _state = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if self.closing.load(Ordering::Acquire) || super::super::runtime::admission_closed() { return Err("Wasm 插件已关闭".into()); }
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
}
