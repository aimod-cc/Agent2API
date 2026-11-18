use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::super::LoginTaskHandle;
use crate::server::{core::providers::adapter::ProviderAdapter, errors::GatewayError, logging};

struct RuntimeCleanup {
    identity: usize,
    run: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
pub(super) struct Cleanup(Mutex<Option<RuntimeCleanup>>);

impl Cleanup {
    pub(super) fn clear(&self) {
        let action = self.0.lock().unwrap_or_else(|error| error.into_inner()).as_ref().map(|entry| entry.run.clone());
        if let Some(action) = action { action(); }
    }
}

/// 只能由主程序短信任务创建；不是可反序列化的凭据，不能从管理动作载荷构造。
#[derive(Clone)]
pub struct Call {
    task: LoginTaskHandle,
    owner: Weak<dyn ProviderAdapter>,
    flow: String,
    method_index: u64,
    step: String,
    deadline: i64,
    cancel: CancellationToken,
    cleanup: Arc<Cleanup>,
}

fn unavailable() -> GatewayError { GatewayError::with_status(409, "短信登录已结束、过期或执行体已变化，请重新发起") }

impl Call {
    pub(super) fn new(task: &LoginTaskHandle) -> Result<Self, GatewayError> {
        let snapshot = task.snapshot();
        let context = task.context.as_ref().ok_or_else(unavailable)?;
        let owner = task.adapter().ok_or_else(unavailable)?;
        let guard = context.sms.lock().map_err(|_| unavailable())?;
        let session = guard.as_ref().ok_or_else(unavailable)?;
        let call = Self {
            task: task.clone(), owner: Arc::downgrade(&owner), flow: session.flow.clone(),
            method_index: snapshot.method_index.ok_or_else(unavailable)?, step: session.step.clone(), deadline: session.deadline,
            cancel: session.cancel.clone(), cleanup: session.cleanup.clone(),
        };
        drop(guard);
        call.validate(context.provider)?;
        Ok(call)
    }

    pub(crate) fn validate(&self, provider: &str) -> Result<(), GatewayError> {
        let snapshot = self.task.snapshot();
        let context = self.task.context.as_ref().ok_or_else(unavailable)?;
        let owner = self.owner.upgrade().ok_or_else(unavailable)?;
        if !snapshot.sms || snapshot.done || snapshot.canceled || self.cancel.is_cancelled()
            || logging::now_ms() >= self.deadline || snapshot.provider != provider || context.provider != provider
            || owner.provider_id() != provider || snapshot.state.as_deref() != Some(self.flow.as_str())
            || snapshot.method_index != Some(self.method_index)
            || !self.flow.starts_with("sms_") || self.flow.len() > 256
            || !self.task.adapter().is_some_and(|current| Arc::ptr_eq(&current, &owner)) {
            return Err(unavailable());
        }
        let session = context.sms.lock().map_err(|_| unavailable())?;
        if !session.as_ref().is_some_and(|session| session.flow == self.flow && session.step == self.step
            && session.deadline == self.deadline && Arc::ptr_eq(&session.cleanup, &self.cleanup)) {
            return Err(unavailable());
        }
        Ok(())
    }

    pub(crate) fn validate_adapter<T: ProviderAdapter + ?Sized>(&self, adapter: &T) -> Result<(), GatewayError> {
        self.validate(adapter.provider_id())?;
        if self.owner.as_ptr() as *const () != adapter as *const T as *const () { return Err(unavailable()); }
        Ok(())
    }

    pub(crate) fn validate_action(&self, action: &str, payload: &Value) -> Result<(), GatewayError> {
        if !matches!(action, "login.sms.send" | "login.sms.verify") || !payload.is_object()
            || payload["methodIndex"].as_u64() != Some(self.method_index) {
            return Err(GatewayError::bad_request("短信登录调用不属于当前任务或方法"));
        }
        Ok(())
    }

    pub(crate) fn flow(&self) -> &str { &self.flow }

    pub(crate) fn proxy(&self) -> Value {
        self.task.context.as_ref().and_then(|context| context.proxy.as_ref())
            .map(|proxy| proxy.resolved.clone()).unwrap_or(Value::Null)
    }

    pub(crate) fn proxy_overridden(&self) -> bool {
        self.task.context.as_ref().is_some_and(|context| context.proxy.is_some())
    }

    pub(crate) fn bind_runtime(&self, identity: usize, run: impl Fn() + Send + Sync + 'static) -> Result<(), GatewayError> {
        let owner = self.owner.upgrade().ok_or_else(unavailable)?;
        self.validate(owner.provider_id())?;
        let mut cleanup = self.cleanup.0.lock().map_err(|_| unavailable())?;
        if let Some(current) = cleanup.as_ref() {
            if current.identity != identity { return Err(unavailable()); }
        } else { *cleanup = Some(RuntimeCleanup { identity, run: Arc::new(run) }); }
        Ok(())
    }

    pub(crate) async fn ended(&self) {
        let wait = self.deadline.saturating_sub(logging::now_ms()).max(0) as u64;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
        }
    }
}

pub(crate) fn device() -> Value {
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME"))
        .ok().map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
        .unwrap_or_else(|| "unknown".to_string());
    // 保持旧登录协议的系统族/版本口径；这里只提供主机事实，不计算厂商设备指纹。
    let (platform, os_version) = match std::env::consts::OS {
        "windows" => ("win", "10.0"),
        "macos" => ("mac", "darwin"),
        _ => ("linux", "linux"),
    };
    json!({ "name": name, "platform": platform, "os_version": os_version })
}
