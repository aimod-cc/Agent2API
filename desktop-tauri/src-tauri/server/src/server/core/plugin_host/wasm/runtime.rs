use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use wasmtime::{Caller, InstancePre, Linker, Memory, Module, Store, StoreLimitsBuilder};

use super::context::{Context, Shared, Sink};

struct State {
    context: Context,
    limits: super::limits::Limits,
    budget: super::fuel::Budget,
    diagnostics: Arc<super::diagnostics::Invocation>,
    input_bytes: usize,
    // 入站消息超过单次调用上限时在这里留下尺寸；不借用文案判断。
    input_oversize: Option<(usize, usize)>,
    reply: Vec<u8>,
    reply_memory: crate::server::core::runtime_capacity::MemoryBudget,
}

impl State {
    fn capture(&self, remaining: Option<u64>, reason: Option<&'static str>) {
        self.diagnostics.capture(&self.budget, remaining, &self.context, &self.limits, self.input_bytes, reason);
    }
}

struct InvocationStore(Store<State>);

impl std::ops::Deref for InvocationStore {
    type Target = Store<State>;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl std::ops::DerefMut for InvocationStore {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl Drop for InvocationStore {
    fn drop(&mut self) {
        self.0.data().capture(self.0.get_fuel().ok(), None);
    }
}

pub struct Runtime {
    pub shared: Arc<Shared>,
    module: Module,
    module_digest: String,
    pre: InstancePre<State>,
}

/// 普通领取仍是原验证码所有者；签到另外携带不可从JSON恢复的取消事实。
/// 单个入口槽避免给短信、托管登录和对话入口继续追加参数。
enum ClaimContext {
    Ordinary(super::verification::lease::Owner),
    Checkin {
        owner: Option<super::verification::lease::Owner>,
        call: super::checkin_call::CheckinCallRef,
        persistence: super::checkin_call::persistence::PersistenceSlot,
    },
}

enum InvocationContext {
    Ordinary,
    Verification(super::verification::VerificationCall),
    Claim(ClaimContext),
    Managed {
        call: crate::server::core::login::managed::Call,
        kiro: Option<Arc<super::managed::kiro_execution::KiroExecution>>,
    },
    Conversation(super::inbound_images::Prepared),
    DoubaoImage(super::conversation::ConversationCall),
    DesktopImport(super::native::chatglm::DesktopCall),
    DesktopLaunch(super::desktop_launch::binding::Call),
    DoubaoRewards(Arc<super::doubao_rewards::Call>),
}

fn memory(caller: &mut Caller<'_, State>) -> wasmtime::Result<Memory> {
    caller.get_export("memory").and_then(|export| export.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("Wasm 缺少内存导出"))
}

fn read(caller: &mut Caller<'_, State>, pointer: i32, length: i32) -> wasmtime::Result<Vec<u8>> {
    if pointer < 0 || length < 0 || length as usize > caller.data().context.message_limit() {
        return Err(wasmtime::Error::msg("Wasm 消息长度无效"));
    }
    let memory = memory(caller)?;
    let end = (pointer as usize).checked_add(length as usize).ok_or_else(|| wasmtime::Error::msg("Wasm 内存越界"))?;
    let data = memory.data(&*caller).get(pointer as usize..end)
        .ok_or_else(|| wasmtime::Error::msg("Wasm 内存越界"))?;
    Ok(data.to_vec())
}

impl Runtime {
    pub async fn load(path: std::path::PathBuf, shared: Arc<Shared>) -> Result<Arc<Self>, String> {
        let (module, module_digest) = super::compilation::load(path, &shared.cancel).await?;
        let engine = module.engine();
        for import in module.imports() {
            if import.module() != "agent2api" || !matches!(import.name(), "call" | "read") {
                return Err("Wasm 模块导入了未授权的系统接口".into());
            }
        }
        let mut linker: Linker<State> = Linker::new(engine);
        linker.func_wrap_async("agent2api", "call", |mut caller: Caller<'_, State>, (ptr, len): (i32, i32)| {
            let received = tokio::time::Instant::now();
            Box::new(async move {
                let allocation = crate::server::core::runtime_capacity::Allocation::new(
                    (len.max(0) as usize).saturating_mul(4))
                    .map_err(|error| wasmtime::Error::msg(error.message))?;
                let bytes = read(&mut caller, ptr, len)?;
                let input: Value = serde_json::from_slice(&bytes).map_err(|_| wasmtime::Error::msg("Wasm 主程序调用不是 JSON"))?;
                let method = input["method"].as_str().ok_or_else(|| wasmtime::Error::msg("Wasm 调用缺少方法"))?;
                let result = caller.data_mut().context.invoke_at(method, input["params"].clone(), received).await;
                let upstream_bytes = caller.data_mut().context.progress.take();
                if result.is_ok() && upstream_bytes > 0 && caller.data().budget.is_adaptive() {
                    caller.data_mut().context.spin_calls = 0;
                    let remaining = caller.get_fuel()?;
                    let grant = caller.data_mut().budget.replenish(remaining, upstream_bytes);
                    if grant > 0 { caller.set_fuel(remaining.saturating_add(grant))?; }
                }
                caller.data().capture(caller.get_fuel().ok(), None);
                drop(input);
                drop(bytes);
                drop(allocation);
                let result = match result {
                    Ok(result) => json!({ "result": result }),
                    Err(error) => json!({ "error": error }),
                };
                let allocation = crate::server::core::runtime_capacity::Allocation::new(
                    crate::server::core::runtime_capacity::value_bytes(&result))
                    .map_err(|error| wasmtime::Error::msg(error.message))?;
                let reply = serde_json::to_vec(&result).map_err(|_| wasmtime::Error::msg("Wasm 回应序列化失败"))?;
                if reply.len() > caller.data().context.message_limit() { return Err(wasmtime::Error::msg("Wasm 主程序回应过大")); }
                caller.data().reply_memory.resize(reply.len()).map_err(|error| wasmtime::Error::msg(error.message))?;
                let length = reply.len() as i32;
                caller.data_mut().reply = reply;
                drop(result);
                drop(allocation);
                Ok(length)
            })
        }).map_err(|error| error.to_string())?;
        linker.func_wrap("agent2api", "read", |mut caller: Caller<'_, State>, pointer: i32, capacity: i32| -> wasmtime::Result<i32> {
            let length = caller.data().reply.len();
            if pointer < 0 || capacity < 0 || (capacity as usize) < length {
                return Err(wasmtime::Error::msg("Wasm 回应缓冲区不足"));
            }
            let memory = memory(&mut caller)?;
            let reply = std::mem::take(&mut caller.data_mut().reply);
            memory.write(&mut caller, pointer as usize, &reply)?;
            drop(reply);
            caller.data().reply_memory.shrink(length);
            Ok(length as i32)
        }).map_err(|error| error.to_string())?;
        let pre = linker.instantiate_pre(&module).map_err(|error| error.to_string())?;
        Ok(Arc::new(Self { shared, module, module_digest, pre }))
    }

    pub(super) fn module_digest(&self) -> &str { &self.module_digest }

    pub async fn invoke(&self, method: &str, params: Value, timeout: Duration, sink: Option<Sink>) -> Result<Value, proto::WireError> {
        self.invoke_inner(method, params, timeout, sink, InvocationContext::Ordinary).await
    }

    pub(super) async fn invoke_doubao_image(&self, call: super::conversation::ConversationCall,
        mut ended: super::conversation::ExecutionGuard, params: Value, timeout: Duration,
    ) -> Result<Value, proto::WireError> {
        let result = self.invoke_inner("images.generate", params, timeout, None, InvocationContext::DoubaoImage(call)).await;
        ended.finish(result.is_ok());
        result
    }

    pub(super) async fn invoke_claim(&self, method: &str, params: Value, timeout: Duration,
        owner: super::verification::lease::Owner) -> Result<Value, proto::WireError> {
        self.invoke_inner(method, params, timeout, None, InvocationContext::Claim(ClaimContext::Ordinary(owner))).await
    }

    pub(super) async fn invoke_checkin(&self, call: super::checkin_call::CheckinCallRef,
        params: Value, timeout: Duration, owner: Option<super::verification::lease::Owner>,
        persistence: super::checkin_call::persistence::PersistenceSlot,
    ) -> Result<Value, proto::WireError> {
        self.invoke_inner(proto::methods::gateway::CHECKIN_CLAIM, params, timeout, None,
            InvocationContext::Claim(ClaimContext::Checkin { owner, call, persistence })).await
    }

    pub(super) async fn invoke_verification(&self, call: super::verification::VerificationCall) -> Result<Value, proto::WireError> {
        let params = call.params();
        let timeout = call.remaining();
        self.invoke_inner("admin.action", params, timeout, None, InvocationContext::Verification(call)).await
    }

    pub(super) async fn invoke_managed(&self, call: crate::server::core::login::managed::Call,
        params: Value, timeout: Duration, kiro: Option<Arc<super::managed::kiro_execution::KiroExecution>>,
    ) -> Result<Value, proto::WireError> {
        self.invoke_inner(call.method(), params, timeout, None, InvocationContext::Managed { call, kiro }).await
    }

    pub(super) async fn invoke_doubao_rewards(&self, call: Arc<super::doubao_rewards::Call>,
        params: Value, timeout: Duration,
    ) -> Result<Value, proto::WireError> {
        let method = if params["claim"].as_str().is_some_and(|claim| !claim.is_empty()) {
            proto::methods::gateway::CLAIM
        } else { proto::methods::gateway::CLAIM_STATUS };
        self.invoke_inner(method, params, timeout, None, InvocationContext::DoubaoRewards(call)).await
    }

    pub(super) async fn invoke_desktop_launch(&self, call: super::desktop_launch::binding::Call,
        method: &str, params: Value, timeout: Duration) -> Result<Value, proto::WireError> {
        self.invoke_inner(method, params, timeout, None, InvocationContext::DesktopLaunch(call)).await
    }

    pub(super) async fn invoke_desktop_import(&self, call: super::native::chatglm::DesktopCall,
        params: Value, timeout: Duration) -> Result<Value, proto::WireError> {
        self.invoke_inner(proto::methods::gateway::ACCOUNT_NORMALIZE, params, timeout, None,
            InvocationContext::DesktopImport(call)).await
    }

    pub(super) async fn invoke_conversation(&self, input: super::inbound_images::Prepared,
        mut ended: super::conversation::ExecutionGuard, timeout: Duration, sink: Sink,
    ) -> Result<Value, proto::WireError> {
        if !input.matches(&ended) {
            return Err(proto::WireError::with_code("Wasm 对话结束守卫不属于本次调用", "forbidden"));
        }
        let result = self.invoke_inner(proto::methods::gateway::CONVERSATION, Value::Null, timeout, Some(sink),
            InvocationContext::Conversation(input)).await;
        // 只记录执行返回/中断，不把插件 JSON 的业务状态当成任务成功。
        ended.finish(result.is_ok());
        result
    }

    async fn invoke_inner(&self, method: &str, params: Value, timeout: Duration, sink: Option<Sink>,
        entry: InvocationContext) -> Result<Value, proto::WireError> {
        let cancel = sink.as_ref().map(|sink| sink.cancel.clone()).unwrap_or_else(|| self.shared.cancel.child_token());
        use crate::server::core::runtime_capacity::{self as capacity, Class};
        let interactive = matches!(&entry, InvocationContext::Verification(_));
        let (account, caller, input_bytes) = match &entry {
            InvocationContext::Conversation(input) => {
                let (account, caller, bytes) = input.scheduling();
                (account.to_string(), caller.to_string(), bytes)
            }
            InvocationContext::DoubaoImage(call) => (params["account"].as_str().unwrap_or("").to_string(),
                call.capacity_caller().to_string(), capacity::value_bytes(&params)),
            _ => (params["account"].as_str().unwrap_or("").to_string(), capacity::caller(), capacity::value_bytes(&params)),
        };
        let class = if matches!(method, "images.generate" | "images.edit") { Class::Image }
            else if sink.is_some() { Class::Conversation }
            else if capacity::background() || matches!(method, "tasks.run" | "usage.query" | "checkin.claim" | "claim" | "claim.status" | "models.refresh") {
                Class::Background
            } else { Class::Interactive };
        let call_deadline = crate::server::core::usage_query::account_query_deadline(&self.shared.options.provider, &account)
            .map_or(tokio::time::Instant::now() + timeout, |deadline| deadline.min(tokio::time::Instant::now() + timeout));
        let query_reserved = !interactive && super::query_queue::reserved(&self.shared, &account, method)?;
        let diagnostics = super::diagnostics::Invocation::new(&self.shared.options.provider, method, &self.module_digest);
        let work = async {
            let wait_started = tokio::time::Instant::now();
            let queue_deadline = (tokio::time::Instant::now() + Duration::from_secs(capacity::limits().queue_wait_seconds)).min(call_deadline);
            let _hard_limit = if matches!(class, Class::Conversation | Class::Image) {
                match &self.shared.execution_slots {
                    Some(slots) => match tokio::time::timeout_at(queue_deadline, slots.acquire()).await {
                        Ok(Ok(permit)) => Some(permit),
                        Ok(Err(_)) | Err(_) => {
                            let rejection = capacity::record_slot_rejection(class, wait_started.elapsed());
                            crate::server::logging::log_with_level(
                                "[Plugin]",
                                &format!(
                                    "⚠️ 插件执行槽位已满 (503 plugin_busy): 提供商={} 方法={} {}",
                                    self.shared.options.provider, method, rejection.describe(),
                                ),
                                "warn",
                            );
                            return Err(rejection.into_wire_error());
                        }
                    },
                    None => None,
                }
            } else { None };
            let _ticket = if interactive || query_reserved { None } else {
                match capacity::acquire_detailed(class, &self.shared.options.provider, &caller, input_bytes, queue_deadline, &cancel).await {
                    Ok(ticket) => Some(ticket),
                    Err(capacity::AcquireError::Cancelled) => {
                        return Err(capacity::AcquireError::Cancelled.into_wire_error());
                    }
                    Err(capacity::AcquireError::Rejected(rejection)) => {
                        crate::server::logging::log_with_level(
                            "[Plugin]",
                            &format!(
                                "⚠️ 插件容量排队超时/拒绝 (503 plugin_busy): 提供商={} 方法={} {}",
                                self.shared.options.provider, method, rejection.describe(),
                            ),
                            "warn",
                        );
                        return Err(rejection.into_wire_error());
                    }
                }
            };
            let _input_memory = capacity::Allocation::new(input_bytes)?;
            diagnostics.phase("preparing");
            let (mut context, params) = match entry {
                InvocationContext::Conversation(input) => {
                    if method != proto::methods::gateway::CONVERSATION || !params.is_null() {
                        return Err(proto::WireError::with_code("可信对话上下文不能用于普通调用", "forbidden"));
                    }
                    let sink = sink.ok_or_else(|| proto::WireError::with_code("可信对话缺少原响应通道", "forbidden"))?;
                    input.into_context(&self.shared, sink, call_deadline).await?
                }
                entry => {
                    let mut context = Context::new(self.shared.clone(), &params, sink, call_deadline);
                    match entry {
                        InvocationContext::Ordinary => {},
                        InvocationContext::DoubaoImage(call) => {
                            if method != "images.generate" || self.shared.options.provider != "doubaowork" { return Err(super::error("豆包生图调用来源无效")); }
                            call.validate_params(&self.shared, &params)?;
                            context.conversation = Some(call);
                        }
                        InvocationContext::Claim(ClaimContext::Ordinary(owner)) => context.claim_owner = Some(owner),
                        InvocationContext::Claim(ClaimContext::Checkin { owner, call, persistence }) => {
                            call.enter_context(self, method, &params)?;
                            persistence.bind(&call, self).await?;
                            persistence.observe_original_daily(&call).await?;
                            context.claim_owner = owner;
                            context.checkin_call = Some(call);
                        }
                        InvocationContext::Managed { call, kiro } => {
                            call.validate(&self.shared.options.provider).map_err(super::managed::wire)?;
                            context.kiro_execution = kiro;
                            context.managed = Some(call);
                        }
                        InvocationContext::Verification(call) => {
                            call.validate().await?;
                            context.bind_verification(call);
                        }
                        InvocationContext::DoubaoRewards(call) => {
                            call.enter(self, method, &context.account)?;
                            context.doubao_rewards = Some(call);
                        }
                        InvocationContext::DesktopLaunch(call) => {
                            if !matches!(method, super::desktop_launch::declaration::CHECK | super::desktop_launch::declaration::PREPARE) {
                                return Err(proto::WireError::with_code("本机启动上下文不能用于其他操作", "forbidden"));
                            }
                            call.validate(&context)?;
                            context.desktop_launch = Some(call);
                        }
                        InvocationContext::DesktopImport(call) => {
                            call.enter_context(self, method, &params)?;
                            context.desktop_import = Some(call);
                        }
                        InvocationContext::Conversation(_) => {
                            return Err(proto::WireError::with_code("可信对话上下文重复装配", "forbidden"));
                        }
                    }
                    (context, params)
                }
            };
            if method != proto::methods::gateway::INIT && params["provider"].as_str() != Some(self.shared.options.provider.as_str()) {
                return Err(proto::WireError::with_code("调用不属于当前插件提供商", "forbidden"));
            }
            if matches!(method, "images.generate" | "images.edit") && !self.shared.options.allows("images.v1") {
                return Err(proto::WireError::with_code("插件未声明标准图片能力", "forbidden"));
            }
            if method == "tasks.run" && !self.shared.options.allows("tasks.v1") {
                return Err(proto::WireError::with_code("插件未声明后台任务能力", "forbidden"));
            }
            if context.flow.len() > 256 || context.session.len() > 1024 || context.session.contains('\0') {
                return Err(proto::WireError::with_code("登录或会话标识无效", "bad_params"));
            }
            context.account_maintenance = super::context::account_maintenance(method, &params)
                || context.desktop_launch.is_some();
            context.usage_query = method == proto::methods::gateway::USAGE_QUERY
                || matches!(method, proto::methods::gateway::TOKEN_ENSURE | proto::methods::gateway::TOKEN_REFRESH)
                    && crate::server::core::usage_query::is_account_query(&self.shared.options.provider, &context.account);
            context.desktop_selected = super::desktop_credentials::selected_import(&self.shared.options.provider)
                && (method == "login.start" || method == proto::methods::gateway::ACCOUNT_NORMALIZE);
            if method == proto::methods::gateway::ACCOUNT_NORMALIZE
                && super::desktop_credentials::supplied_import()
                && matches!(self.shared.options.provider.as_str(), "autoclaw" | "autoclaw-intl")
            {
                context.supplied_credentials = Some(super::desktop_credentials::supplied_tokens(&params)?);
            }
            if method == proto::methods::gateway::ACCOUNT_NORMALIZE && super::desktop_credentials::supplied_import()
                && self.shared.options.provider == "kuku" && params["source"]["kind"] == "file_base64" {
                context.permission("resources.cookie-file.v1")?;
                context.supplied_cookie_file = Some(super::desktop_credentials::cookie_file::prepare(&params)?);
            }
            if !context.account.is_empty() && context.doubao_rewards.is_none() {
                context.invoke("account.get", json!({})).await?;
            }
            if let Some(call) = &context.conversation { call.validate_context(&self.shared, &context.account)?; }
            if let Some(call) = &context.checkin_call { call.observe_context(&context)?; }
            let mut params = params;
            if context.supplied_cookie_file.is_some() { params["source"]["content"] = Value::Null; }
            if let Some(map) = params.as_object_mut() {
                for key in ["proxy", "proxy_override", "plugin_trace", "client_stream"] { map.remove(key); }
                if self.shared.options.provider == "raccoon" {
                    // 只由原真实对话的下游鉴权头派生关联摘要；不接受JSON自报，不导出Key或授予权限。
                    map.remove("raccoonCaller");
                    if method == proto::methods::gateway::CONVERSATION && context.conversation.is_some() {
                        use sha2::{Digest, Sha256};
                        let mut caller = Sha256::new();
                        caller.update(b"agent2api:raccoon-caller:v1");
                        let mut present = false;
                        if let Some(headers) = map.get("headers").and_then(Value::as_object) {
                            for name in ["authorization", "x-api-key", "api-key"] {
                                if let Some(value) = headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))
                                    .and_then(|(_, value)| value.as_str()).filter(|value| !value.is_empty()) {
                                    caller.update((name.len() as u64).to_be_bytes());
                                    caller.update(name.as_bytes());
                                    caller.update((value.len() as u64).to_be_bytes());
                                    caller.update(value.as_bytes());
                                    present = true;
                                }
                            }
                        }
                        if present { map.insert("raccoonCaller".into(), json!(format!("{:x}", caller.finalize()))); }
                    }
                }
                if self.shared.options.provider == "workbuddy-intl" {
                    // 只给原真实对话提供调用方关联摘要；清除自报值，不输出鉴权头或授予新许可。
                    map.remove("workbuddyIntlCaller");
                    if method == proto::methods::gateway::CONVERSATION && context.conversation.is_some() {
                        use sha2::{Digest, Sha256};
                        let mut caller = Sha256::new();
                        caller.update(b"agent2api:workbuddy-intl-caller:v1");
                        let mut present = false;
                        if let Some(headers) = map.get("headers").and_then(Value::as_object) {
                            for name in ["authorization", "x-api-key", "api-key"] {
                                if let Some(value) = headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))
                                    .and_then(|(_, value)| value.as_str()).filter(|value| !value.is_empty()) {
                                    caller.update((name.len() as u64).to_be_bytes());
                                    caller.update(name.as_bytes());
                                    caller.update((value.len() as u64).to_be_bytes());
                                    caller.update(value.as_bytes());
                                    present = true;
                                }
                            }
                        }
                        if present { map.insert("workbuddyIntlCaller".into(), json!(format!("{:x}", caller.finalize()))); }
                    }
                }
                if self.shared.options.provider == "antigravity" {
                    // 仅原真实对话的鉴权头派生缓存隔离摘要；清除自报值，不输出凭据或授予许可。
                    map.remove("antigravityCaller");
                    if method == proto::methods::gateway::CONVERSATION && context.conversation.is_some() {
                        use sha2::{Digest, Sha256};
                        let mut caller = Sha256::new();
                        caller.update(b"agent2api:antigravity-caller:v1");
                        let mut present = false;
                        if let Some(headers) = map.get("headers").and_then(Value::as_object) {
                            for name in ["authorization", "x-api-key", "api-key"] {
                                if let Some(value) = headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))
                                    .and_then(|(_, value)| value.as_str()).filter(|value| !value.is_empty()) {
                                    caller.update((name.len() as u64).to_be_bytes());
                                    caller.update(name.as_bytes());
                                    caller.update((value.len() as u64).to_be_bytes());
                                    caller.update(value.as_bytes());
                                    present = true;
                                }
                            }
                        }
                        if present { map.insert("antigravityCaller".into(), json!(format!("{:x}", caller.finalize()))); }
                    }
                }
                if self.shared.options.provider == "trae" {
                    // 只在原真实对话中派生调用者关联摘要；不输出鉴权头或把输入当作许可。
                    map.remove("traeCaller");
                    if method == proto::methods::gateway::CONVERSATION && context.conversation.is_some() {
                        use sha2::{Digest, Sha256};
                        let mut caller = Sha256::new();
                        caller.update(b"agent2api:trae-caller:v1");
                        let mut present = false;
                        if let Some(headers) = map.get("headers").and_then(Value::as_object) {
                            for name in ["authorization", "x-api-key", "api-key"] {
                                if let Some(value) = headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))
                                    .and_then(|(_, value)| value.as_str()).filter(|value| !value.is_empty()) {
                                    caller.update((name.len() as u64).to_be_bytes());
                                    caller.update(name.as_bytes());
                                    caller.update((value.len() as u64).to_be_bytes());
                                    caller.update(value.as_bytes());
                                    present = true;
                                }
                            }
                        }
                        if present { map.insert("traeCaller".into(), json!(format!("{:x}", caller.finalize()))); }
                    }
                }
                if let Some(payload) = map.get_mut("payload").and_then(Value::as_object_mut) { payload.remove("proxy"); }
                if let Some(payload) = map.get_mut("source").and_then(|source| source.get_mut("payload")).and_then(Value::as_object_mut) {
                    payload.remove("proxy");
                }
                if let Some(headers) = map.get_mut("headers").and_then(Value::as_object_mut) {
                    headers.retain(|name, _| !matches!(name.to_ascii_lowercase().as_str(),
                        "authorization" | "x-api-key" | "api-key" | "cookie" | "proxy-authorization"));
                }
            }
            let memory_limit_mib = self.shared.options.effective_memory_mib(method == proto::methods::gateway::CONVERSATION);
            let limits = StoreLimitsBuilder::new().memory_size(memory_limit_mib * 1024 * 1024)
                .instances(1).memories(1).tables(2).table_elements(100_000).trap_on_grow_failure(true).build();
            let memory = capacity::MemoryBudget::new();
            memory.resize(2 * capacity::MIB)?;
            let limits = super::limits::Limits { store: limits, exhausted: false, reason: None, memory, baseline: 2 * capacity::MIB,
                linear_bytes: 0, table_bytes: 0 };
            let adaptive = class == Class::Conversation && context.conversation.is_some();
            let budget = super::fuel::Budget::new(adaptive, interactive);
            let mut store = InvocationStore(Store::new(self.module.engine(), State { context, limits, budget, diagnostics: diagnostics.clone(),
                input_bytes: 0, input_oversize: None, reply: Vec::new(), reply_memory: capacity::MemoryBudget::new() }));
            store.limiter(|state| &mut state.limits);
            store.fuel_async_yield_interval(Some(super::fuel::YIELD_INTERVAL)).map_err(|_| super::error("Wasm 调度配置失败"))?;
            let result = self.execute(&mut store, method, params).await;
            if let Some((size, limit)) = store.data().input_oversize {
                // 消息在实例化前就放不下：明确告诉调用方尺寸，而不是折成笼统的资源错误。
                let hint = if method == proto::methods::gateway::CONVERSATION {
                    "；请新建会话或减少图片后重试"
                } else { "" };
                let message = format!("本次请求约 {:.1} MiB，超过插件单次调用上限 {:.0} MiB{hint}",
                    size as f64 / capacity::MIB as f64, limit as f64 / capacity::MIB as f64);
                store.data().capture(store.get_fuel().ok(), Some("input"));
                return Err(proto::WireError::with_code(message, "plugin_request_too_large"));
            }
            let reason = if store.data().limits.exhausted { store.data().limits.reason }
                else { result.as_ref().err().and_then(|error| match error.downcast_ref::<wasmtime::Trap>() {
                    Some(wasmtime::Trap::OutOfFuel) => Some(store.data().budget.exhausted()),
                    Some(wasmtime::Trap::StackOverflow) => Some("stack"),
                    Some(_) => Some("trap"),
                    None => Some("protocol"),
                }) };
            store.data().capture(store.get_fuel().ok(), reason);
            match result {
                Ok(mut value) => {
                    if let Some(call) = &store.data().context.checkin_call { call.observe_context(&store.data().context)?; }
                    if let Some(call) = store.data().context.verification.clone() { call.validate().await?; }
                    if let Some(call) = &store.data().context.managed { call.validate(&self.shared.options.provider).map_err(super::managed::wire)?; }
                    if let Some(call) = &store.data().context.desktop_import { call.validate(&self.shared)?; }
                    if let Some(call) = &store.data().context.desktop_launch {
                        if let Err(error) = call.validate(&store.data().context) {
                            super::desktop_launch::wipe(&mut value);
                            return Err(error);
                        }
                    }
                    if matches!(method, "images.generate" | "images.edit") {
                        super::resources::materialize(&mut value, &store.data().context.blobs, &self.shared.options)?;
                    }
                    Ok(value)
                }
                Err(_) if store.data().limits.exhausted => {
                    let reason = store.data().limits.reason;
                    let limit_mib = memory_limit_mib;
                    let input = store.data().input_bytes;
                    let (message, code) = match reason {
                        Some("table") => ("Wasm 插件表容量超限".to_string(), "plugin_resource_limit"),
                        Some("host_memory") => ("Wasm 插件共享内存预算超限".to_string(), "plugin_resource_limit"),
                        // 对话撞上内存上限，真正原因是正文/图片太大。给出尺寸与下一步，
                        // 既便于用户处理，也避免被通用策略当成服务故障去换号重试。
                        _ if method == proto::methods::gateway::CONVERSATION => (
                            format!("本次对话上下文或图片过大：请求约 {:.1} MiB，插件内存上限 {limit_mib} MiB；请新建会话或减少图片后重试",
                                input as f64 / capacity::MIB as f64),
                            "plugin_request_too_large",
                        ),
                        _ => ("Wasm 插件内存超限".to_string(), "plugin_resource_limit"),
                    };
                    Err(proto::WireError::with_code(message, code))
                }
                Err(error) => Err(super::errors::interrupted(&error)),
            }
        };
        let result = tokio::select! {
            biased;
            _ = self.shared.cancel.cancelled() => Err(proto::WireError::with_code("Wasm 插件已关闭", "plugin_unavailable")),
            _ = cancel.cancelled() => Err(proto::WireError::with_code("Wasm 调用已取消", proto::WireError::ABORTED)),
            result = tokio::time::timeout_at(call_deadline, work) => result
                .unwrap_or_else(|_| Err(proto::WireError::with_code("Wasm 插件调用超时", "timeout"))),
        };
        diagnostics.finish(&result);
        result
    }

    async fn execute(&self, store: &mut Store<State>, method: &str, params: Value) -> wasmtime::Result<Value> {
        let input = json!({ "method": method, "params": params });
        let bytes = serde_json::to_vec(&input)?;
        drop(input);
        store.data_mut().input_bytes = bytes.len();
        if bytes.len() > store.data().context.message_limit() {
            let limit = store.data().context.message_limit();
            store.data_mut().input_oversize = Some((bytes.len(), limit));
            return Err(wasmtime::Error::msg("Wasm 输入消息过大"));
        }
        let fuel = store.data_mut().budget.input(bytes.len());
        store.set_fuel(fuel)?;
        store.data().diagnostics.phase("instantiating");
        store.data().capture(store.get_fuel().ok(), None);
        let instance = self.pre.instantiate_async(&mut *store).await?;
        let abi = instance.get_typed_func::<(), i32>(&mut *store, "a2a_abi_version")?;
        if abi.call_async(&mut *store, ()).await? != super::manifest::ABI as i32 {
            return Err(wasmtime::Error::msg("Wasm 导出接口版本不一致"));
        }
        let allocate = instance.get_typed_func::<i32, i32>(&mut *store, "a2a_alloc")?;
        let call = instance.get_typed_func::<(i32, i32), i64>(&mut *store, "a2a_call")?;
        let memory = instance.get_memory(&mut *store, "memory").ok_or_else(|| wasmtime::Error::msg("Wasm 内存导出缺失"))?;
        store.data().diagnostics.phase("allocating");
        store.data().capture(store.get_fuel().ok(), None);
        let pointer = allocate.call_async(&mut *store, bytes.len() as i32).await?;
        if pointer < 0 { return Err(wasmtime::Error::msg("Wasm 分配地址无效")); }
        memory.write(&mut *store, pointer as usize, &bytes)?;
        drop(bytes);
        store.data().diagnostics.phase("executing");
        store.data().capture(store.get_fuel().ok(), None);
        let input_length = store.data().input_bytes as i32;
        let packed = call.call_async(&mut *store, (pointer, input_length)).await? as u64;
        store.data().diagnostics.phase("decoding");
        let pointer = (packed & 0xffff_ffff) as usize;
        let length = (packed >> 32) as usize;
        if store.data().context.sink.is_none() { store.data_mut().context.progress.output(length); }
        if length > store.data().context.message_limit() { return Err(wasmtime::Error::msg("Wasm 输出消息过大")); }
        let data = memory.data(&*store).get(pointer..pointer.checked_add(length).ok_or_else(|| wasmtime::Error::msg("Wasm 输出越界"))?)
            .ok_or_else(|| wasmtime::Error::msg("Wasm 输出越界"))?;
        serde_json::from_slice(data).map_err(Into::into)
    }
}
