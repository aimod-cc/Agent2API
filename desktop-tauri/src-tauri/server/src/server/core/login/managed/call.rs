mod kiro_exchange;
pub(crate) use kiro_exchange::KiroCallSource;

use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::{session, unavailable, Input, LoginTaskHandle, RuntimeBinding, Session, WeakTask};
use crate::server::{core::providers::adapter::ProviderAdapter, errors::GatewayError};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action { Start, Poll, Captcha, Callback }

struct Material {
    value: Mutex<Option<Value>>,
    taken: AtomicBool,
    catpaw_verifying: AtomicBool,
    catpaw_ready: AtomicBool,
    // 只由真实 Kiro callback claim 签发，绝不从 params/状态串构造。
    kiro_receipt: Mutex<Option<Arc<super::kiro::Receipt>>>,
    kiro_advanced: AtomicBool,
    kiro_exchange: kiro_exchange::Slot,
    // 只由Trae真实IPv4监听器的queue/claim签发，不从任何JSON code/state恢复。
    trae_receipt: Mutex<Option<super::trae::AuthCodeReceipt>>,
    trae_taken: AtomicBool,
    trae_execution: Mutex<Option<Arc<crate::server::core::plugin_host::wasm::managed::TraeExecution>>>,
    trae_execution_retained: AtomicBool,
}

/// 只由原Trae Callback Call签发的弱对象见证；没有serde/公开构造入口或执行体强引用。
/// 防止execution→Call→execution自持环，也不按flow/id从表里重新找材料。
pub(crate) struct TraeCallSource {
    session: Weak<Session>,
    material: Weak<Material>,
    id: String,
    expected: String,
    action: Action,
}

impl TraeCallSource {
    pub(in crate::server::core::login::managed) fn matches_state(&self, state: &super::State) -> bool {
        self.action == Action::Callback && state.active.as_deref() == Some(self.id.as_str())
            && state.callback_step.as_deref() == Some(self.expected.as_str())
            && state.step.as_ref().is_some_and(|step| step.id == self.expected && step.status == "processing")
            && self.session.upgrade().is_some_and(|session| session.valid()
                && super::trae::applies(&session.provider, session.method_index))
            && self.material.upgrade().is_some_and(|material| material.taken.load(Ordering::Acquire)
                && material.trae_taken.load(Ordering::Acquire) && material.trae_execution_retained.load(Ordering::Acquire))
    }
    pub(crate) fn matches(&self, call: &Call) -> bool {
        self.action == Action::Callback && call.action == self.action
            && super::trae::applies(call.provider(), call.method_index())
            && self.id == call.id && self.expected == call.expected
            && self.session.upgrade().is_some_and(|session| Arc::ptr_eq(&session, &call.session))
            && self.material.upgrade().is_some_and(|material| Arc::ptr_eq(&material, &call.material))
    }
}

/// 不实现 Deserialize，字段全部私有。UI 的 state/interaction 只是索引，不是调用权限。
#[derive(Clone)]
pub struct Call {
    pub(super) task: WeakTask,
    pub(super) session: Arc<Session>,
    owner: Weak<dyn ProviderAdapter>,
    action: Action,
    id: String,
    expected: String,
    options: Value,
    material: Arc<Material>,
    published: Arc<AtomicBool>,
    execution: Arc<Mutex<Option<Arc<dyn std::any::Any + Send + Sync>>>>,
}

impl Call {
    fn new(task: &LoginTaskHandle, action: Action, options: Value) -> Result<Self, GatewayError> {
        let session = session(task)?;
        let owner = task.adapter().ok_or_else(unavailable)?;
        let weak = WeakTask::new(task)?;
        let (expected, id) = {
            let mut state = session.state.lock().map_err(|_| unavailable())?;
            if !session.valid() { return Err(unavailable()); }
            if state.active.is_some() { return Err(GatewayError::with_status(409, "当前登录步骤正在处理")); }
            let expected = state.step.as_ref().ok_or_else(unavailable)?.id.clone();
            let id = super::nonce()?;
            state.active = Some(id.clone());
            (expected, id)
        };
        Ok(Self { task: weak, session, owner: Arc::downgrade(&owner), action, id, expected,
            options, material: Arc::new(Material { value: Mutex::new(None), taken: AtomicBool::new(false),
                catpaw_verifying: AtomicBool::new(false), catpaw_ready: AtomicBool::new(false),
                kiro_receipt: Mutex::new(None), kiro_advanced: AtomicBool::new(false),
                kiro_exchange: Default::default(),
                trae_receipt: Mutex::new(None), trae_taken: AtomicBool::new(false),
                trae_execution: Mutex::new(None), trae_execution_retained: AtomicBool::new(false) }),
            published: Arc::new(AtomicBool::new(false)), execution: Arc::new(Mutex::new(None)) })
    }

    pub(super) fn start(task: &LoginTaskHandle, options: Value) -> Result<Self, GatewayError> {
        Self::new(task, Action::Start, options)
    }

    pub(super) fn poll(task: &LoginTaskHandle) -> Result<Self, GatewayError> {
        Self::new(task, Action::Poll, Value::Null)
    }

    pub(super) fn submit(task: &LoginTaskHandle, input: &Input) -> Result<Self, GatewayError> {
        let session = session(task)?;
        if input.state != session.flow { return Err(unavailable()); }
        let action = {
            let mut state = session.state.lock().map_err(|_| unavailable())?;
            if state.active.is_some() { return Err(GatewayError::with_status(409, "当前登录步骤正在处理")); }
            let step = state.step.as_mut().ok_or_else(unavailable)?;
            step.check(input)?;
            if step.captcha.is_some() { Action::Captcha } else { Action::Callback }
        };
        let call = Self::new(task, action, Value::Null)?;
        let claimed = (|| {
            let mut state = session.state.lock().map_err(|_| unavailable())?;
            if !session.valid() || state.active.as_deref() != Some(call.id.as_str()) { return Err(unavailable()); }
            let step = state.step.as_ref().ok_or_else(unavailable)?;
            if step.id != input.interaction || step.id != call.expected { return Err(unavailable()); }
            if action == Action::Callback {
                let lease_id = step.lease.as_ref().ok_or_else(unavailable)?.clone();
                let raw = input.inputs["callbackUrl"].as_str().ok_or_else(unavailable)?;
                let receipt = super::loopback::claim(&mut state, &lease_id, &input.interaction, raw, true)?;
                if let Some(mut receipt) = receipt {
                    receipt.bind_call(&call)?;
                    *call.material.kiro_receipt.lock().map_err(|_| unavailable())? = Some(Arc::new(receipt));
                }
            }
            state.step.as_mut().ok_or_else(unavailable)?.status = "processing";
            let material = if action == Action::Captcha {
                json!({ "captcha": input.captcha, "inputs": {} })
            } else { json!({ "url": input.inputs["callbackUrl"] }) };
            *call.material.value.lock().map_err(|_| unavailable())? = Some(material);
            Ok(())
        })();
        if let Err(error) = claimed { call.finish(); return Err(error); }
        Ok(call)
    }

    pub(super) fn callback(task: &LoginTaskHandle, lease: &str, raw: &str) -> Result<Self, GatewayError> {
        let current = session(task)?;
        if super::trae::applies(&current.provider, current.method_index) {
            return Err(GatewayError::with_status(403, "Trae 无state回调只接受原监听器对象，不接受原始URL注入"));
        }
        let call = Self::new(task, Action::Callback, Value::Null)?;
        let claimed = (|| {
            let mut state = call.session.state.lock().map_err(|_| unavailable())?;
            if !call.session.valid() || state.active.as_deref() != Some(call.id.as_str()) { return Err(unavailable()); }
            let receipt = super::loopback::claim(&mut state, lease, &call.expected, raw, false)?;
            if let Some(mut receipt) = receipt {
                receipt.bind_call(&call)?;
                *call.material.kiro_receipt.lock().map_err(|_| unavailable())? = Some(Arc::new(receipt));
            }
            state.step.as_mut().ok_or_else(unavailable)?.status = "processing";
            *call.material.value.lock().map_err(|_| unavailable())? = Some(json!({ "url": raw }));
            Ok(())
        })();
        if let Err(error) = claimed { call.finish(); return Err(error); }
        Ok(call)
    }

    pub(super) fn trae_callback(task: &LoginTaskHandle, queued: super::trae::QueuedCallback)
        -> Result<Self, GatewayError> {
        let call = Self::new(task, Action::Callback, Value::Null)?;
        let result = (|| {
            let receipt = super::trae::claim(&call, queued)?;
            let mut material = call.material.trae_receipt.lock().map_err(|_| unavailable())?;
            if material.is_some() { return Err(unavailable()); }
            *material = Some(receipt);
            Ok(())
        })();
        if let Err(error) = result { call.finish(); return Err(error); }
        Ok(call)
    }

    /// 只读原用户首次选择，插件参数不能决定或补写variant；原生缺省为solo。
    pub(in crate::server::core::login::managed) fn original_trae_variant(&self) -> Result<&str, GatewayError> {
        if !super::trae::applies(self.provider(), self.method_index()) || !self.is_start() { return Err(unavailable()); }
        if self.options.get("inputs").is_some_and(|inputs| !inputs.is_object()) {
            return Err(GatewayError::bad_request("Trae 原登录选择格式无效，未猜测应用归属"));
        }
        match self.options.pointer("/inputs/variant") {
            None => Ok("solo"),
            Some(Value::String(value)) if value == "cn" || value == "solo" => Ok(value.as_str()),
            Some(_) => Err(GatewayError::bad_request("Trae 原登录选择只接受明确的 cn 或 solo")),
        }
    }

    /// 固定方法login.trae.context.prepare的Rust入口；只由真实本次Start Call调用。
    pub(crate) async fn prepare_trae_context(&self, params: &Value,
        options: &crate::server::core::plugin_host::wasm::manifest::Options) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        super::trae::prepare(self, params, options).await
    }

    /// 原Callback对象签发一次执行来源；字符串相同不授予另一个Call归属。
    pub(crate) fn trae_call_source(&self) -> Result<TraeCallSource, GatewayError> {
        self.validate("trae")?;
        if !super::trae::applies(self.provider(), self.method_index()) || !self.is_callback()
            || self.published() { return Err(unavailable()); }
        Ok(TraeCallSource { session: Arc::downgrade(&self.session), material: Arc::downgrade(&self.material),
            id: self.id.clone(), expected: self.expected.clone(), action: self.action })
    }

    /// 仅真实Context捕获的封闭typed lease；不能另配Host/Runtime/代次或Any对象。
    pub(crate) fn retain_trae_execution(&self,
        execution: Arc<crate::server::core::plugin_host::wasm::managed::TraeExecution>,
    ) -> Result<(), GatewayError> {
        execution.validate(self)?;
        if !super::trae::applies(self.provider(), self.method_index()) || !self.is_callback()
            || self.material.trae_taken.load(Ordering::Acquire) { return Err(unavailable()); }
        let mut slot = self.material.trae_execution.lock().map_err(|_| unavailable())?;
        if self.material.trae_execution_retained.swap(true, Ordering::AcqRel) || slot.is_some() {
            return Err(unavailable());
        }
        *slot = Some(execution);
        Ok(())
    }

    pub(in crate::server::core::login::managed) fn trae_execution(&self)
        -> Result<Arc<crate::server::core::plugin_host::wasm::managed::TraeExecution>, GatewayError> {
        let execution = self.material.trae_execution.lock().map_err(|_| unavailable())?
            .as_ref().cloned().ok_or_else(unavailable)?;
        execution.validate(self)?;
        Ok(execution)
    }

    /// 固定login.trae.exchange只收{expected}；授权码/上下文完全来自原Material的一次take。
    pub(crate) async fn exchange_trae(&self, params: &Value,
        options: &crate::server::core::plugin_host::wasm::manifest::Options,
    ) -> Result<Value, GatewayError> {
        super::trae::exchange(self, params, options).await
    }

    pub(crate) fn validate_trae_account(&self,
        account: &agent2api_provider_protocol::NormalizedAccount,
    ) -> Result<(), GatewayError> {
        super::trae::validate_account(self, account)
    }

    /// 仅host exchange消费，插件不能传code/收据或重建上下文。
    /// 失败也消耗本Call的一次资格，不会把Material/Session里的材料放回。
    pub(in crate::server::core::login::managed) fn take_trae_exchange(&self)
        -> Result<super::trae::ExchangeInput, GatewayError> {
        self.validate(self.provider())?;
        if !super::trae::applies(self.provider(), self.method_index()) || !self.is_callback()
            || self.published() || self.material.taken.load(Ordering::Acquire)
            || self.material.trae_taken.load(Ordering::Acquire) { return Err(unavailable()); }
        let mut state = self.session.state.lock().map_err(|_| unavailable())?;
        let mut receipt_slot = self.material.trae_receipt.lock().map_err(|_| unavailable())?;
        if !self.session.valid() || state.active.as_deref() != Some(self.id())
            || self.material.trae_taken.swap(true, Ordering::AcqRel) { return Err(unavailable()); }
        let receipt = receipt_slot.take().ok_or_else(unavailable)?;
        let result = super::trae::take(self, &mut state, &receipt);
        drop(receipt_slot);
        drop(state);
        match result {
            Ok(context) => {
                self.material.taken.store(true, Ordering::Release);
                Ok(receipt.into_exchange(context))
            }
            Err(error) => { drop(receipt); Err(error) }
        }
    }

    pub(super) fn catpaw_callback(task: &LoginTaskHandle, lease: &str, expected: &str,
        input: super::catpaw::PostToken) -> Result<Self, GatewayError> {
        let call = Self::new(task, Action::Callback, Value::Null)?;
        let claimed = super::catpaw::claim_post(&call, lease, expected, input);
        match claimed {
            Ok(material) => { *call.material.value.lock().map_err(|_| unavailable())? = Some(material); Ok(call) }
            Err(error) => { call.finish(); Err(error) }
        }
    }

    /// poll 在同一操作锁、同一Call内晋升；不再次申请步骤或调用receive_callback。
    pub(super) fn promote_catpaw(&mut self, input: Value) -> Result<(), GatewayError> {
        if self.action != Action::Poll || !super::catpaw::applies(self.provider(), self.method_index())
            || !self.can_complete()? { return Err(unavailable()); }
        self.validate(self.provider())?;
        let mut material = self.material.value.lock().map_err(|_| unavailable())?;
        if material.is_some() || self.material.taken.load(Ordering::Acquire) { return Err(unavailable()); }
        *material = Some(input);
        self.action = Action::Callback;
        Ok(())
    }

    pub(super) fn take_pending_catpaw(&self) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        if !self.is_callback() || !super::catpaw::applies(self.provider(), self.method_index())
            || self.material.catpaw_verifying.swap(true, Ordering::AcqRel) { return Err(unavailable()); }
        self.material.value.lock().map_err(|_| unavailable())?.take().ok_or_else(unavailable)
    }

    pub(super) fn install_catpaw_input(&self, input: Value) -> Result<(), GatewayError> {
        self.validate(self.provider())?;
        if !self.material.catpaw_verifying.load(Ordering::Acquire) || self.material.taken.load(Ordering::Acquire)
            || self.material.catpaw_ready.load(Ordering::Acquire) { return Err(unavailable()); }
        let mut material = self.material.value.lock().map_err(|_| unavailable())?;
        if !self.session.valid() || material.is_some() { return Err(unavailable()); }
        *material = Some(input);
        self.material.catpaw_ready.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) fn validate(&self, provider: &str) -> Result<(), GatewayError> {
        if let Some(source) = &self.session.local_source {
            if !source.is_current() { return Err(unavailable()); }
        }
        let task = self.task.upgrade().ok_or_else(unavailable)?;
        let current = session(&task)?;
        let owner = self.owner.upgrade().ok_or_else(unavailable)?;
        if !Arc::ptr_eq(&current, &self.session) || provider != self.session.provider
            || owner.provider_id() != provider
            || !task.adapter().is_some_and(|adapter| Arc::ptr_eq(&adapter, &owner)) { return Err(unavailable()); }
        let state = self.session.state.lock().map_err(|_| unavailable())?;
        if !self.session.valid() || state.active.as_deref() != Some(self.id.as_str()) { return Err(unavailable()); }
        if let Some(runtime) = &state.runtime {
            if runtime.stopped.is_cancelled() || !(runtime.alive)() { return Err(unavailable()); }
        } else if self.action != Action::Start { return Err(unavailable()); }
        Ok(())
    }

    pub(crate) fn validate_adapter<T: ProviderAdapter + ?Sized>(&self, adapter: &T) -> Result<(), GatewayError> {
        self.validate(adapter.provider_id())?;
        if self.owner.as_ptr() as *const () != adapter as *const T as *const () { return Err(unavailable()); }
        Ok(())
    }

    pub(crate) fn bind_runtime(&self, identity: usize, generation: u64, stopped: CancellationToken,
        alive: impl Fn() -> bool + Send + Sync + 'static,
        cleanup: impl Fn() + Send + Sync + 'static) -> Result<(), GatewayError> {
        self.validate(&self.session.provider)?;
        if !alive() || stopped.is_cancelled() { return Err(unavailable()); }
        let mut state = self.session.state.lock().map_err(|_| unavailable())?;
        if !self.session.valid() || state.active.as_deref() != Some(self.id.as_str()) { return Err(unavailable()); }
        if let Some(runtime) = &state.runtime {
            if runtime.identity != identity || runtime.generation != generation { return Err(unavailable()); }
        } else if self.action == Action::Start {
            state.runtime = Some(Arc::new(RuntimeBinding { identity, generation, stopped,
                alive: Arc::new(alive), cleanup: Arc::new(cleanup) }));
        } else { return Err(unavailable()); }
        Ok(())
    }

    pub(crate) fn with_trae_local_source<R>(&self,
        action: impl FnOnce() -> Result<R, GatewayError>,
    ) -> Result<R, GatewayError> {
        if !super::trae::applies(self.provider(), self.method_index()) { return Err(unavailable()); }
        let source = self.session.local_source.as_ref().ok_or_else(unavailable)?;
        source.with_current(action).ok_or_else(unavailable)?
    }

    pub(crate) fn runtime_matches(&self, identity: usize, generation: u64) -> Result<(), GatewayError> {
        self.validate(&self.session.provider)?;
        let state = self.session.state.lock().map_err(|_| unavailable())?;
        if !state.runtime.as_ref().is_some_and(|runtime| runtime.identity == identity && runtime.generation == generation) {
            return Err(unavailable());
        }
        Ok(())
    }

    pub(crate) fn flow(&self) -> &str { &self.session.flow }
    pub(crate) fn provider(&self) -> &str { &self.session.provider }
    pub(crate) fn method_index(&self) -> u64 { self.session.method_index }
    pub(crate) fn expected(&self) -> &str { &self.expected }
    pub(crate) fn id(&self) -> &str { &self.id }
    pub(crate) fn deadline(&self) -> tokio::time::Instant { self.session.deadline }
    pub(crate) fn cancellation(&self) -> CancellationToken { self.session.cancel.clone() }
    pub(super) fn is_callback(&self) -> bool { self.action == Action::Callback }
    pub(crate) fn method(&self) -> &'static str {
        match self.action { Action::Start => "login.start", Action::Poll => "login.poll",
            Action::Captcha => "login.submit", Action::Callback => "login.callback" }
    }
    pub(super) fn is_start(&self) -> bool { self.action == Action::Start }
    pub(super) fn is_captcha(&self) -> bool { self.action == Action::Captcha }
    pub(super) fn consumed_input(&self) -> bool {
        !matches!(self.action, Action::Captcha | Action::Callback) || self.material.taken.load(Ordering::Acquire)
    }
    pub(super) fn published(&self) -> bool { self.published.load(Ordering::Acquire) }

    pub(crate) fn params(&self) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        let task = self.task.upgrade().ok_or_else(unavailable)?;
        let proxy = task.context.as_ref().and_then(|context| context.proxy.as_ref());
        Ok(json!({ "provider": self.provider(), "flow": self.flow(), "account": "", "method": "browser",
            "payload": self.options, "interaction": self.expected, "proxy": proxy.map(|proxy| &proxy.resolved),
            "proxy_override": proxy.is_some() }))
    }

    pub(crate) fn context(&self) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        Ok(json!({ "provider": self.provider(), "flow": self.flow(), "methodIndex": self.method_index(),
            "interaction": self.expected, "expiresAt": self.session.expires_at,
            "remainingMs": self.session.deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis() as u64 }))
    }

    pub(crate) fn take_input(&self) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        if !matches!(self.action, Action::Captcha | Action::Callback) { return Err(GatewayError::with_status(403, "当前调用没有真人提交材料")); }
        if super::trae::applies(self.provider(), self.method_index()) {
            return Err(GatewayError::with_status(403, "Trae 授权码仅由固定宿主兑换消费，不向插件交付原始材料"));
        }
        if super::catpaw::applies(self.provider(), self.method_index()) && !self.material.catpaw_ready.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let mut material = self.material.value.lock().map_err(|_| unavailable())?;
        if !self.session.valid() {
            let expired = material.take();
            drop(material);
            drop(expired);
            return Err(unavailable());
        }
        let input = material.take().ok_or_else(|| GatewayError::with_status(409, "当前登录输入已被读取，不能重复使用"))?;
        self.material.taken.store(true, Ordering::Release);
        Ok(input)
    }

    pub(crate) async fn ended(&self) {
        tokio::select! {
            biased;
            _ = self.session.cancel.cancelled() => {},
            _ = tokio::time::sleep_until(self.session.deadline) => {},
        }
    }

    pub(crate) fn retain_execution<T: std::any::Any + Send + Sync>(&self, lease: Arc<T>) -> Result<(), GatewayError> {
        self.validate(self.provider())?;
        let mut execution = self.execution.lock().map_err(|_| unavailable())?;
        if execution.is_some() { return Err(unavailable()); }
        *execution = Some(lease);
        Ok(())
    }

    pub(super) fn finish(&self) {
        if let Ok(mut state) = self.session.state.lock() {
            super::trae::finish_call(self, &mut state);
            if state.active.as_deref() == Some(self.id.as_str()) { state.active = None; }
        }
        kiro_exchange::finish(self);
        let material = self.material.value.lock().ok().and_then(|mut material| material.take());
        let receipt = self.material.kiro_receipt.lock().ok().and_then(|mut receipt| receipt.take());
        let trae_receipt = self.material.trae_receipt.lock().ok().and_then(|mut receipt| receipt.take());
        let trae_execution = self.material.trae_execution.lock().ok().and_then(|mut execution| execution.take());
        let execution = self.execution.lock().ok().and_then(|mut execution| execution.take());
        drop(trae_receipt);
        drop(trae_execution);
        drop(receipt);
        drop(material);
        drop(execution);
    }

    pub(super) fn can_complete(&self) -> Result<bool, GatewayError> {
        if super::trae::applies(self.provider(), self.method_index()) {
            return super::trae::can_complete(self);
        }
        if super::kiro::applies(self.provider(), self.method_index()) {
            return self.kiro_can_complete();
        }
        let state = self.session.state.lock().map_err(|_| unavailable())?;
        let preparing = state.step.as_ref().is_some_and(|step| step.status == "processing"
            && state.callback_step.as_deref() == Some(step.id.as_str()));
        Ok(preparing)
    }

    pub(super) fn published_browser(&self) -> Result<bool, GatewayError> {
        let state = self.session.state.lock().map_err(|_| unavailable())?;
        Ok(self.published() && self.material.taken.load(Ordering::Acquire)
            && state.step.as_ref().is_some_and(|step| step.id != self.expected && step.browser.is_some() && step.status == "ready"))
    }

    /// 仅固定 Kiro 门户 choice：持同一会话锁时消费原Call收据一次，不接受外部收据。
    pub(super) fn with_kiro_advance<T>(&self,
        action: impl FnOnce(&mut super::State, &super::kiro::Receipt) -> Result<T, GatewayError>,
    ) -> Result<T, GatewayError> {
        self.validate(self.provider())?;
        if self.material.kiro_advanced.load(Ordering::Acquire) {
            return Err(GatewayError::with_status(409, "Kiro 本次阶段推进已使用，未返回旧回执或再次推进"));
        }
        if !super::kiro::applies(self.provider(), self.method_index()) || !self.is_callback()
            || self.published() || !self.material.taken.load(Ordering::Acquire)
        { return Err(unavailable()); }
        let mut state = self.session.state.lock().map_err(|_| unavailable())?;
        if !self.session.valid() || state.active.as_deref() != Some(self.id())
            || state.step.as_ref().is_none_or(|step| step.id != self.expected() || step.status != "processing")
            || state.callback_step.as_deref() != Some(self.expected())
            || state.runtime.as_ref().is_none_or(|runtime| runtime.stopped.is_cancelled() || !(runtime.alive)())
        { return Err(unavailable()); }
        let mut receipt = self.material.kiro_receipt.lock().map_err(|_| unavailable())?;
        let mut material = self.material.value.lock().map_err(|_| unavailable())?;
        if material.is_some() { return Err(unavailable()); }
        if receipt.as_ref().is_none_or(|receipt| !receipt.choice()) || self.material.kiro_advanced.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        // 验证失败也不复活一次资格：本Call之后只能失败退出，不缓存成功回包。
        self.material.kiro_advanced.store(true, Ordering::Release);
        let result = action(&mut state, receipt.as_ref().ok_or_else(unavailable)?);
        if result.is_ok() { receipt.take(); }
        material.take();
        result
    }

    pub(super) fn kiro_needs_next(&self) -> Result<bool, GatewayError> {
        let state = self.session.state.lock().map_err(|_| unavailable())?;
        let receipt = self.material.kiro_receipt.lock().map_err(|_| unavailable())?;
        Ok(super::kiro::applies(self.provider(), self.method_index()) && self.is_callback()
            && (self.material.kiro_advanced.load(Ordering::Acquire)
                || receipt.as_ref().is_some_and(|receipt| receipt.choice())
                || super::kiro::choice_needs_next(&state)))
    }

    pub(crate) fn publish(&self, params: &Value,
        options: &crate::server::core::plugin_host::wasm::manifest::Options) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        if super::trae::applies(self.provider(), self.method_index()) {
            super::trae::reserve_publication(self)?;
        }
        if self.published() || self.action == Action::Poll || params["expected"].as_str() != Some(self.expected.as_str())
            || super::catpaw::applies(self.provider(), self.method_index()) && self.action != Action::Start
            || matches!(self.action, Action::Captcha | Action::Callback) && !self.material.taken.load(Ordering::Acquire) {
            return Err(GatewayError::with_status(409, "登录步骤已变化或不允许重复发布"));
        }
        if super::kiro::applies(self.provider(), self.method_index()) && !self.is_start() && !self.is_callback() {
            return Err(GatewayError::with_status(403, "Kiro 托管登录不接受验证码或普通轮询发布浏览器步骤"));
        }
        if super::kiro::applies(self.provider(), self.method_index()) && self.is_callback()
            && !self.material.kiro_advanced.load(Ordering::Acquire) {
            return Err(GatewayError::with_status(403, "Kiro 只有原门户选择一次推进后才能发布 AWS 新步骤"));
        }
        let mut state = self.session.state.lock().map_err(|_| unavailable())?;
        if !self.session.valid() || state.active.as_deref() != Some(self.id.as_str())
            || state.step.as_ref().is_none_or(|step| step.id != self.expected) { return Err(unavailable()); }
        let step = super::step::build(params, &self.session, &mut state, options)?;
        let public = step.public(&self.session);
        state.callback_step = None;
        state.step = Some(step);
        self.published.store(true, Ordering::Release);
        Ok(public)
    }

    pub(crate) async fn authorize_autoclaw(&self, params: &Value,
        options: &crate::server::core::plugin_host::wasm::manifest::Options) -> Result<Value, GatewayError> {
        super::autoclaw::authorize(self, params, options).await
    }

    pub(crate) async fn open_loopback(&self, params: &Value,
        options: &crate::server::core::plugin_host::wasm::manifest::Options) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        if self.published() || !matches!(self.action, Action::Start | Action::Captcha)
            || self.action == Action::Captcha && !self.material.taken.load(Ordering::Acquire)
                && !(self.provider() == "autoclaw-intl" && self.method_index() == 0
                    && params["profile"] == "autoclaw-intl.zai.v1") {
            return Err(GatewayError::with_status(403, "当前登录步骤不能申请回调监听"));
        }
        super::loopback::open(self, params, options).await
    }

    /// 只捕获原任务已选择的完整Rust路径；缺失选择不猜直连或重读系统代理。
    pub(crate) fn kiro_selected_path(&self) -> Result<crate::server::core::proxies::chain::SelectedPath, GatewayError> {
        self.validate("kiro")?;
        if !super::kiro::applies(self.provider(), self.method_index()) || !self.is_callback() { return Err(unavailable()); }
        let task = self.task.upgrade().ok_or_else(unavailable)?;
        let path = task.context.as_ref().and_then(|context| context.proxy.as_ref())
            .ok_or_else(|| GatewayError::with_status(409,
                "Kiro本次未捕获明确的登录出口，请重新选择直连或代理；未回退默认系统出口"))?
            .selected.clone();
        self.validate("kiro")?;
        Ok(path)
    }

    /// 固定Kiro首次表单只用于选择Portal/AWS profile，不接受插件另传登录方式。
    pub(super) fn kiro_start_choice(&self) -> Result<&str, GatewayError> {
        if !super::kiro::applies(self.provider(), self.method_index()) || !self.is_start() { return Err(unavailable()); }
        let choice = match self.options.pointer("/inputs/loginMethod") {
            None => "portal",
            Some(value) => value.as_str().ok_or_else(|| GatewayError::bad_request("Kiro 首次登录方式必须是文字"))?,
        };
        if !["portal", "builderid", "enterprise"].contains(&choice) {
            return Err(GatewayError::bad_request("Kiro 首次登录方式无效"));
        }
        Ok(choice)
    }

    pub(super) fn kiro_start_region(&self) -> Result<&str, GatewayError> {
        self.kiro_start_choice()?;
        let region = match self.options.pointer("/inputs/region") {
            None => "us-east-1",
            Some(value) => value.as_str().ok_or_else(|| GatewayError::bad_request("Kiro 目录地区必须是文字"))?,
        };
        let region = region.trim();
        let region = if region.is_empty() { "us-east-1" } else { region };
        if crate::server::core::plugin_host::wasm::aws_oidc::origin(region).is_none() {
            return Err(GatewayError::bad_request("Kiro 目录地区尚未获得固定来源授权，未替换地区"));
        }
        Ok(region)
    }

    pub(crate) fn advance_kiro(&self, params: &Value,
        options: &crate::server::core::plugin_host::wasm::manifest::Options,
    ) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        if options.provider != self.provider() || !options.allows("login.managed.v1") || !options.allows("login.loopback.v1") {
            return Err(unavailable());
        }
        super::kiro::advance(self, params, options)
    }

    /// 内部推进收尾异常只能关闭原会话，不返回可能复用的成功材料。
    pub(crate) fn invalidate_kiro(&self) {
        if super::kiro::applies(self.provider(), self.method_index()) { self.session.close(); }
    }

    pub(crate) fn close_loopback(&self, params: &Value) -> Result<Value, GatewayError> {
        self.validate(self.provider())?;
        super::loopback::close(self, params)
    }
}
