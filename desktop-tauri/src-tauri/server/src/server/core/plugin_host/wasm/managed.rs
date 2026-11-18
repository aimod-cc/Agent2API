mod trae_execution;
pub(super) mod kiro_execution;
mod kiro_network;
mod kiro_response;
mod kiro_worker;
pub(crate) use kiro_worker::Completed as KiroCompleted;
pub(crate) use trae_execution::TraeExecution;
pub(super) use kiro_worker::{close_for_host as close_kiro_for_host, release_ready as kiro_release_ready, wait_for_host as wait_kiro_for_host};

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{context::Context, manifest::Options};
use super::super::Host;
use crate::server::{core::login::managed::Call, errors::GatewayError};

pub(super) fn wire(error: GatewayError) -> proto::WireError {
    proto::WireError::with_code(error.message, format!("provider_http:{}", json!({
        "status": error.status_code, "code": error.code,
    })))
}

fn unavailable() -> GatewayError {
    GatewayError::with_status(409, "托管登录执行体已关闭、停用或换代，请重新发起")
}

pub(super) fn guard_ordinary(options: &Options, method: &str, params: &Value) -> Result<(), proto::WireError> {
    let managed_flow = params["flow"].as_str().is_some_and(|flow| flow.starts_with("managed_"))
        || params["payload"]["flow"].as_str().is_some_and(|flow| flow.starts_with("managed_"));
    let managed_start = method == "login.start" && options.manifest.managed_login.as_ref().is_some_and(|declaration|
        declaration.methods.contains(&params["payload"]["methodIndex"].as_u64().unwrap_or(0)));
    if managed_flow || managed_start || method.starts_with("login.loopback.") || method.starts_with("login.trae.") || method.starts_with("login.kiro.") || method == "login.step.publish"
        || method == "admin.action" && matches!(params["action"].as_str(),
            Some("login.step.publish" | "login.loopback.open" | "login.loopback.close" | "login.loopback.kiro.advance" | "login.input.take" | "login.trae.context.prepare" | "login.trae.exchange" | "login.kiro.exchange")) {
        return Err(proto::WireError::with_code("托管登录只接受当前主程序任务签发的调用", "forbidden"));
    }
    Ok(())
}

fn current(host: &Arc<Host>, provider: &str, generation: u64) -> bool {
    host.ready() && host.generation.load(Ordering::Acquire) == generation && !host.activity.retired()
        && !super::super::registry::is_switching(provider) && host.callbacks.provider_allowed(provider)
        && super::super::serves_provider(provider)
        && super::super::host_for_provider(provider).is_some_and(|owner| Arc::ptr_eq(&owner, host))
}

struct Active(Arc<Host>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.embedded.active.fetch_sub(1, Ordering::AcqRel);
        self.0.embedded.idle.notify_waiters();
    }
}

impl Host {
    pub(crate) fn prepare_wasm_managed(self: &Arc<Self>, call: &Call) -> Result<(), GatewayError> {
        call.validate(call.provider())?;
        let runtime = self.embedded.runtime.try_read().map_err(|_| unavailable())?.clone().ok_or_else(unavailable)?;
        let options = &runtime.shared.options;
        if !options.allows("login.managed.v1") || options.provider != call.provider()
            || options.manifest.managed_login.as_ref().is_none_or(|declaration| !declaration.methods.contains(&call.method_index())) {
            return Err(GatewayError::with_status(403, "此登录方式没有托管权限"));
        }
        let generation = self.generation.load(Ordering::Acquire);
        if !current(self, call.provider(), generation) { return Err(unavailable()); }
        let weak = Arc::downgrade(self);
        let shared = Arc::downgrade(&runtime.shared);
        let provider = call.provider().to_string();
        let flow = call.flow().to_string();
        call.bind_runtime(Arc::as_ptr(&runtime.shared) as usize, generation, runtime.shared.cancel.clone(),
            move || weak.upgrade().is_some_and(|host| current(&host, &provider, generation)),
            move || { if let Some(shared) = shared.upgrade() { super::host::login_cleanup(&shared, &flow); } })
    }

    pub(crate) async fn call_wasm_managed(self: &Arc<Self>, call: &Call, timeout: Duration) -> Result<Value, proto::WireError> {
        call.validate(call.provider()).map_err(wire)?;
        let runtime = self.embedded.runtime.try_read().map_err(|_| wire(unavailable()))?.clone().ok_or_else(|| wire(unavailable()))?;
        let generation = self.generation.load(Ordering::Acquire);
        call.runtime_matches(Arc::as_ptr(&runtime.shared) as usize, generation).map_err(wire)?;
        let lease = self.activity.acquire().ok_or_else(|| wire(unavailable()))?;
        // 许可保留到宿主验收结果/落库完毕，不能在 Wasm 返回后与换代交叉。
        call.retain_execution(lease).map_err(wire)?;
        {
            let _lifecycle = self.lifecycle.lock().unwrap_or_else(|error| error.into_inner());
            if !current(self, call.provider(), generation) { return Err(wire(unavailable())); }
            self.embedded.active.fetch_add(1, Ordering::AcqRel);
        }
        let _active = Active(self.clone());
        let kiro_execution = if call.provider() == "kiro" && call.method_index() == 0 {
            Some(kiro_execution::KiroExecution::capture(self, &runtime, call).map_err(wire)?)
        } else { None };
        let params = call.params().map_err(wire)?;
        let result = tokio::select! {
            biased;
            _ = call.ended() => Err(wire(unavailable())),
            result = runtime.invoke_managed(call.clone(), params, timeout, kiro_execution) => result,
        };
        call.runtime_matches(Arc::as_ptr(&runtime.shared) as usize, generation).map_err(wire)?;
        let value = super::errors::decode(result?)?;
        // 托管步骤只从受控 publish 产生；插件返回的同名 JSON 不能覆盖公开快照。
        if value["account"].is_object() {
            super::host::validate_identity(&runtime.shared.options, &value["account"])?;
            super::public_view::validate(&value["account"]["public"], &value["account"]["secret"])?;
        }
        Ok(value)
    }
}

impl Context {
    pub(super) async fn managed_service(&mut self, method: &str, params: &Value) -> Result<Value, proto::WireError> {
        self.permission("login.managed.v1")?;
        let call = self.managed.clone().ok_or_else(|| proto::WireError::with_code("当前调用没有托管登录权限", "forbidden"))?;
        call.validate(&self.shared.options.provider).map_err(wire)?;
        if call.flow() != self.flow { return Err(wire(unavailable())); }
        match method {
            "login.context" | "login.input.take" => {
                if params.as_object().is_none_or(|fields| !fields.is_empty()) { return Err(wire(GatewayError::bad_request("托管登录上下文查询不接受参数"))); }
                if method == "login.input.take" { call.take_input().map_err(wire) }
                else {
                    let mut context = call.context().map_err(wire)?;
                    let declaration = self.shared.options.manifest.managed_login.as_ref().ok_or_else(|| wire(unavailable()))?;
                    context["authorizationOrigins"] = json!(declaration.origins(call.provider(), call.method_index()));
                    Ok(context)
                }
            }
            "login.step.publish" => call.publish(params, &self.shared.options).map_err(wire),
            "login.trae.context.prepare" => {
                self.permission("login.loopback.v1")?;
                trae_execution::validate_context(self, &call).map_err(wire)?;
                call.prepare_trae_context(params, &self.shared.options).await.map_err(wire)
            }
            "login.kiro.exchange" => {
                self.permission("login.loopback.v1")?;
                if call.provider() != "kiro" || call.method_index() != 0 {
                    return Err(proto::WireError::with_code("Kiro兑换只接受原本次托管回调", "forbidden"));
                }
                let execution = self.kiro_execution.as_ref().cloned().ok_or_else(|| wire(unavailable()))?;
                kiro_worker::exchange(&call, execution, params).await.map_err(wire)
            }
            "login.trae.exchange" => {
                self.permission("login.loopback.v1")?;
                let execution = TraeExecution::capture(self, &call).map_err(wire)?;
                call.retain_trae_execution(execution.clone()).map_err(wire)?;
                call.exchange_trae(params, &self.shared.options).await.map_err(wire)
            }
            "login.autoclaw.authorization" => {
                self.permission("login.loopback.v1")?;
                call.authorize_autoclaw(params, &self.shared.options).await.map_err(wire)
            }
            "login.loopback.open" => {
                self.permission("login.loopback.v1")?;
                call.open_loopback(params, &self.shared.options).await.map_err(wire)
            }
            "login.loopback.kiro.advance" => {
                self.permission("login.loopback.v1")?;
                if call.provider() != "kiro" || call.method_index() != 0 {
                    return Err(proto::WireError::with_code("Kiro 阶段推进只能用于原 method0 托管调用", "forbidden"));
                }
                // 仅Kiro推进核对当前Context实际Shared来源，不能借同provider另一运行实例。
                call.runtime_matches(Arc::as_ptr(&self.shared) as usize,
                    super::super::host_for_provider(call.provider()).map(|host| host.generation.load(Ordering::Acquire))
                        .ok_or_else(|| wire(unavailable()))?).map_err(wire)?;
                // 与state写者一致：原Shared登录状态锁 → Session → 原Call Material。
                // 只毁宿主持有的旧 kiro.oauth；不声称擦除已经交付WASM栈的材料副本。
                let mut state = self.shared.login_state.lock().map_err(|_| wire(unavailable()))?;
                let result = match call.advance_kiro(params, &self.shared.options) {
                    Ok(result) => result,
                    Err(error) => {
                        drop(state);
                        call.invalidate_kiro();
                        return Err(wire(error));
                    }
                };
                if let Err(error) = state.clear_kiro_oauth(call.flow()) {
                    drop(state);
                    call.invalidate_kiro();
                    return Err(error);
                }
                Ok(result)
            }
            "login.loopback.close" => {
                self.permission("login.loopback.v1")?;
                call.close_loopback(params).map_err(wire)
            }
            _ => Err(proto::WireError::with_code("未知托管登录操作", proto::WireError::NO_METHOD)),
        }
    }
}
