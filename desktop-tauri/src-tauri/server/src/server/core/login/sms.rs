use serde::Deserialize;
use serde_json::{json, Value};

use super::{sms_contract::SmsConfig, LoginProxySelection, LoginTaskHandle, LOGIN_TIMEOUT_MS};
use crate::server::{core::providers::adapter::ProviderAdapter, errors::GatewayError, logging, ServerState};

mod limits;
mod challenge;
mod context;

pub use context::Call;
pub(crate) use context::device;

pub(crate) struct Session {
    flow: String,
    cleanup: std::sync::Arc<context::Cleanup>,
    config: SmsConfig,
    step: String,
    phone: String,
    country_code: String,
    ticket: Value,
    challenge: Option<challenge::Challenge>,
    sent: bool,
    retry_at: i64,
    expires_at: i64,
    deadline: i64,
    verify_attempts: u8,
    cancel: tokio_util::sync::CancellationToken,
    _reservation: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for Session {
    fn drop(&mut self) { self.cancel.cancel(); self.cleanup.clear(); }
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum SendResult {
    Sent {
        ticket: Value,
        #[serde(rename = "retryAfterSeconds")]
        retry_after_seconds: u64,
        #[serde(rename = "expiresAfterSeconds")]
        expires_after_seconds: u64,
    },
    Captcha { ticket: Value, challenge: challenge::Challenge },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    action: String,
    interaction: String,
    #[serde(default)]
    inputs: Inputs,
    #[serde(default)]
    captcha: Value,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Inputs {
    #[serde(default)]
    phone: String,
    #[serde(default)]
    country_code: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
}

fn nonce() -> Result<String, GatewayError> {
    let mut bytes = [0; 24];
    getrandom::getrandom(&mut bytes).map_err(|_| GatewayError::with_status(500, "无法生成登录标识"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) async fn start(
    state: &ServerState,
    adapter: std::sync::Arc<dyn ProviderAdapter>,
    method_index: u64,
    config: SmsConfig,
    proxy: Option<LoginProxySelection>,
) -> Result<Value, GatewayError> {
    let _reservation = limits::start()?;
    let flow = format!("sms_{}", nonce()?);
    let provider = adapter.provider_id().to_string();
    let task = state.login().new_handle_for_adapter("", adapter, proxy);
    let cancel = tokio_util::sync::CancellationToken::new();
    let session = Session {
        flow: flow.clone(), cleanup: std::sync::Arc::new(context::Cleanup::default()),
        country_code: config.country_codes.first().map(|code| code.value.clone()).unwrap_or_default(),
        config, step: nonce()?, phone: String::new(), ticket: Value::Null, challenge: None,
        sent: false, retry_at: 0, expires_at: 0, deadline: logging::now_ms() + LOGIN_TIMEOUT_MS as i64,
        verify_attempts: 0, cancel: cancel.clone(), _reservation,
    };
    *task.context.as_ref().ok_or_else(unavailable)?.sms.lock().map_err(|_| unavailable())? = Some(session);
    task.update(|snapshot| {
        snapshot.state = Some(flow.clone());
        snapshot.method_index = Some(method_index);
        snapshot.sms = true;
    });
    state.login().tasks().register(&flow, task.clone());
    let cleanup = task.clone();
    crate::spawn_task(async move {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(std::time::Duration::from_millis(LOGIN_TIMEOUT_MS)) => {}
        }
        let _operation = cleanup.operation.lock().await;
        if !cleanup.snapshot().done && !cleanup.snapshot().canceled {
            super::fail_plugin_login(&cleanup, "短信登录已过期，请重新发起");
        }
        cleanup.cancel_provider_flow_locked(&flow).await;
    });
    Ok(json!({ "state": task.snapshot().state, "provider": provider, "methodIndex": method_index,
        "url": "", "authUrl": "", "browserMode": "none", "manualCallback": false,
        "callbackInput": "url", "pollIntervalMs": 0, "interactionType": "sms" }))
}

fn unavailable() -> GatewayError { GatewayError::with_status(409, "短信登录已结束，请重新发起") }

fn with_session<T>(task: &LoginTaskHandle, read: impl FnOnce(&mut Session) -> Result<T, GatewayError>) -> Result<T, GatewayError> {
    let snapshot = task.snapshot();
    let context = task.context.as_ref().ok_or_else(unavailable)?;
    if !snapshot.sms || snapshot.done || snapshot.canceled || snapshot.provider != context.provider {
        return Err(unavailable());
    }
    let mut guard = context.sms.lock().map_err(|_| unavailable())?;
    let session = guard.as_mut().ok_or_else(unavailable)?;
    if session.cancel.is_cancelled() || logging::now_ms() >= session.deadline
        || snapshot.state.as_deref() != Some(session.flow.as_str()) { return Err(unavailable()); }
    read(session)
}

pub(super) fn interrupt(task: &LoginTaskHandle) {
    if let Some(context) = &task.context {
        if let Ok(guard) = context.sms.lock() {
            if let Some(session) = guard.as_ref() { session.cancel.cancel(); }
        }
    }
}

async fn invoke(task: &LoginTaskHandle, adapter: &dyn ProviderAdapter, action: &str, params: &Value) -> Result<Value, GatewayError> {
    let call = Call::new(task)?;
    let context = task.context.as_ref().ok_or_else(unavailable)?;
    call.validate_adapter(adapter)?;
    call.validate_action(action, params)?;
    let result = adapter.sms_action(&context.store, &call, action, params).await;
    call.validate(adapter.provider_id())?;
    result
}

pub(crate) fn snapshot(task: &LoginTaskHandle) -> Result<Value, GatewayError> {
    with_session(task, |session| {
        let now = logging::now_ms();
        Ok(json!({ "id": session.step, "type": "sms", "title": "手机验证码登录",
            "sms": { "config": session.config, "phone": session.phone, "countryCode": session.country_code,
                "sent": session.sent, "expired": session.sent && (session.expires_at <= now || session.verify_attempts >= 5),
                "retryAfterSeconds": ((session.retry_at - now).max(0) + 999) / 1000,
                "expiresAfterSeconds": ((session.expires_at - now).max(0) + 999) / 1000,
                "challenge": session.challenge } }))
    })
}

pub(crate) async fn submit(_state: &ServerState, task: &LoginTaskHandle, value: Value) -> Result<Value, GatewayError> {
    let input: Input = serde_json::from_value(value).map_err(|_| GatewayError::bad_request("短信登录输入无效"))?;
    let step = input.interaction.as_str();
    if input.inputs.phone.len() > 32 || input.inputs.country_code.len() > 6 || input.inputs.code.len() > 16
        || input.inputs.name.chars().count() > 100 || input.inputs.name.contains('\0')
    {
        return Err(GatewayError::bad_request("短信登录输入超过限制"));
    }
    with_session(task, |session| {
        if session.step != step { return Err(GatewayError::with_status(409, "登录步骤已变化，请重新读取")); }
        Ok(())
    })?;
    let adapter = task.adapter().ok_or_else(unavailable)?;
    let context = task.context.as_ref().ok_or_else(unavailable)?;
    let mut params = json!({ "methodIndex": task.snapshot().method_index });
    if let Some(proxy) = &context.proxy { params["proxy"] = proxy.resolved.clone(); }
    match input.action.as_str() {
        "change_phone" => {
            if !input.inputs.phone.is_empty() || !input.inputs.country_code.is_empty() || !input.inputs.code.is_empty()
                || !input.inputs.name.is_empty() || !input.captcha.is_null()
            {
                return Err(GatewayError::bad_request("修改手机号时不接受其他登录输入"));
            }
            with_session(task, |session| {
                let step = nonce()?;
                // 保留主机 flow，显式清除这一轮全部 Wasm 登录状态；普通网络失败重发不走这里。
                session.cleanup.clear();
                session.step = step;
                session.phone.clear();
                session.ticket = Value::Null;
                session.challenge = None;
                session.sent = false;
                session.retry_at = 0;
                session.expires_at = 0;
                session.verify_attempts = 0;
                Ok(())
            })?;
        }
        "send" => {
            if !input.inputs.code.is_empty() || !input.inputs.name.is_empty() {
                return Err(GatewayError::bad_request("发送验证码时不能提交账号凭证"));
            }
            let (phone, country, cooldown) = with_session(task, |session| {
                let country = input.inputs.country_code.trim();
                let phone = session.config.phone(&input.inputs.phone, country)?;
                if (!session.phone.is_empty() && (session.phone != phone || session.country_code != country))
                    || session.retry_at > logging::now_ms()
                {
                    return Err(GatewayError::bad_request("请等待重发倒计时结束，或先修改手机号"));
                }
                if let Some(challenge) = &session.challenge { challenge.check(&input.captcha)?; }
                else if !input.captcha.is_null() { return Err(GatewayError::bad_request("当前登录步骤不接受人机验证结果")); }
                params["phone"] = json!(phone);
                params["countryCode"] = json!(country);
                params["ticket"] = session.ticket.clone();
                params["captcha"] = input.captcha.clone();
                Ok((phone, country.to_string(), session.config.resend_after_seconds))
            })?;
            let (allowed, retry_at) = limits::reserve(adapter.provider_id(), &country, &phone, cooldown)?;
            with_session(task, |session| {
                session.retry_at = retry_at;
                session.phone = phone.clone();
                session.country_code = country.clone();
                if allowed {
                    session.sent = false;
                    session.expires_at = 0;
                }
                Ok(())
            })?;
            if !allowed { return Err(GatewayError::with_status(429, "短信发送过于频繁，请等待倒计时结束")); }
            let result = match invoke(task, adapter.as_ref(), "login.sms.send", &params).await {
                Ok(result) => result,
                Err(error) => {
                    if let Some(seconds) = error.retry_after_secs.filter(|seconds| *seconds > 0) {
                        let until = logging::now_ms() + seconds.min(86_400) as i64 * 1000;
                        limits::extend(adapter.provider_id(), &country, &phone, until);
                        let _ = with_session(task, |session| { session.retry_at = session.retry_at.max(until); Ok(()) });
                    }
                    return Err(error);
                }
            };
            if task.snapshot().canceled || task.snapshot().done { return Err(unavailable()); }
            let result: SendResult = serde_json::from_value(result)
                .map_err(|_| GatewayError::with_status(502, "插件返回的短信发送结果无效"))?;
            with_session(task, |session| {
                let now = logging::now_ms();
                match result {
                    SendResult::Sent { ticket, retry_after_seconds, expires_after_seconds } => {
                        check_ticket(&ticket)?;
                        if !(1..=3600).contains(&retry_after_seconds) || !(30..=900).contains(&expires_after_seconds) {
                            return Err(GatewayError::with_status(502, "插件返回的短信有效时间无效"));
                        }
                        session.ticket = ticket;
                        session.challenge = None;
                        session.sent = true;
                        session.retry_at = now + retry_after_seconds.max(cooldown) as i64 * 1000;
                        session.expires_at = (now + expires_after_seconds.min(session.config.expires_after_seconds) as i64 * 1000)
                            .min(session.deadline);
                        limits::extend(adapter.provider_id(), &country, &phone, session.retry_at);
                    }
                    SendResult::Captcha { ticket, challenge } => {
                        check_ticket(&ticket)?;
                        challenge.validate()?;
                        session.retry_at = limits::challenge_ready(adapter.provider_id(), &country, &phone, retry_at)?;
                        session.ticket = ticket;
                        session.challenge = Some(challenge);
                        session.sent = false;
                        session.expires_at = 0;
                    }
                }
                session.phone = phone;
                session.country_code = country;
                session.step = nonce()?;
                session.verify_attempts = 0;
                Ok(())
            })?;
        }
        "verify" => {
            with_session(task, |session| {
                if !session.sent || session.expires_at <= logging::now_ms() {
                    return Err(GatewayError::bad_request("验证码已过期，请重新获取"));
                }
                if !input.inputs.phone.is_empty() && input.inputs.phone != session.phone
                    || !input.inputs.country_code.is_empty() && input.inputs.country_code != session.country_code
                    || !input.captcha.is_null()
                {
                    return Err(GatewayError::bad_request("验证码不属于当前登录步骤"));
                }
                params["phone"] = json!(session.phone);
                params["countryCode"] = json!(session.country_code);
                params["code"] = json!(session.config.code(&input.inputs.code)?);
                if session.verify_attempts >= 5 {
                    return Err(GatewayError::with_status(429, "验证码尝试次数过多，请重新获取"));
                }
                session.verify_attempts += 1;
                params["ticket"] = session.ticket.clone();
                Ok(())
            })?;
            let value = invoke(task, adapter.as_ref(), "login.sms.verify", &params).await?;
            if task.snapshot().canceled || task.snapshot().done { return Err(unavailable()); }
            with_session(task, |_| Ok(()))?;
            let mut account: agent2api_provider_protocol::NormalizedAccount = serde_json::from_value(value)
                .map_err(|_| GatewayError::with_status(502, "插件返回的登录结果无效"))?;
            if !input.inputs.name.trim().is_empty() {
                let public = account.public.as_object_mut().ok_or_else(|| GatewayError::with_status(502, "插件返回的账号信息无效"))?;
                public.insert("name".into(), json!(input.inputs.name.trim().chars().take(100).collect::<String>()));
            }
            let result = super::persist_plugin_login(task, adapter.as_ref(), &account);
            task.cancel_provider_flow_locked(task.snapshot().state.as_deref().unwrap_or("")).await;
            result?.ok_or_else(unavailable)?;
        }
        _ => return Err(GatewayError::bad_request("未知的短信登录操作")),
    }
    Ok(json!({ "accepted": true }))
}

fn check_ticket(ticket: &Value) -> Result<(), GatewayError> {
    if !ticket.is_object() || ticket.to_string().len() > 64 * 1024 {
        return Err(GatewayError::with_status(502, "插件返回的短信关联信息无效"));
    }
    Ok(())
}
