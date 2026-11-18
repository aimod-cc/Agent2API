use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{OwnedMutexGuard, Semaphore};
use tokio_util::sync::CancellationToken;

use super::super::{GatewayCallbacks, StreamEvent};
use super::manifest::Options;

pub struct Shared {
    pub options: Options,
    pub callbacks: Arc<dyn GatewayCallbacks>,
    pub system_proxy_when_unset: AtomicBool,
    pub cancel: CancellationToken,
    pub execution_slots: Option<Arc<Semaphore>>,
    pub verification_slots: Arc<Semaphore>,
    pub(super) managed_tasks: super::managed_task::ExecutorSlot,
    pub(super) catpaw_turns: Arc<super::catpaw_turn::Registry>,
    pub locks: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    pub sessions: Mutex<HashMap<String, (i64, Value)>>,
    pub session_memory: Mutex<HashMap<String, crate::server::core::runtime_capacity::Allocation>>,
    pub(super) login_state: Mutex<super::state::Transient>,
    pub(super) interactions: super::login::Interactions,
    pub login_proxies: Mutex<HashMap<String, (i64, Value, bool)>>,
}

impl Shared {
    pub fn new(options: Options, callbacks: Arc<dyn GatewayCallbacks>) -> Arc<Self> {
        Arc::new(Self {
            execution_slots: options.manifest.concurrency_hard_limit.map(|limit| Arc::new(Semaphore::new(limit))),
            verification_slots: Arc::new(Semaphore::new(4)), options,
            managed_tasks: Default::default(), catpaw_turns: Default::default(), callbacks,
            system_proxy_when_unset: AtomicBool::new(false),
            cancel: CancellationToken::new(), locks: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()), session_memory: Mutex::new(HashMap::new()), login_state: Mutex::new(Default::default()),
            interactions: Default::default(), login_proxies: Mutex::new(HashMap::new()),
        })
    }

    pub fn account_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(key).and_then(std::sync::Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(key.to_string(), Arc::downgrade(&lock));
                lock
            }
        }
    }
}

#[derive(Clone)]
pub struct Sink {
    pub events: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    pub credit: Arc<Semaphore>,
    pub cancel: CancellationToken,
}

pub(super) fn account_maintenance(method: &str, params: &Value) -> bool {
    match method {
        "token.ensure" | "token.refresh" | "token.expiring" | "models.list" | "models.refresh"
            | "usage.query" | "checkin.claim" | "claim" | "claim.status" | "tasks.list" | "tasks.run" => true,
        "admin.action" => params["action"].as_str().is_some_and(|action| {
            ["activity.", "benefits.", "billing.", "captcha.", "catalog.", "checkin.", "claim.",
                "growth.", "promotion.", "quota.", "subscription.", "usage.", "wallet."]
                .iter().any(|prefix| action.starts_with(prefix))
        }),
        _ => false,
    }
}

pub struct Context {
    pub shared: Arc<Shared>,
    pub(super) call_deadline: tokio::time::Instant,
    pub account: String,
    pub identity: String,
    pub(super) origin_binding: Option<super::account_origin::Binding>,
    pub(super) usage_query: bool,
    pub(super) account_maintenance: bool,
    pub(super) verification: Option<super::verification::VerificationCall>,
    pub(super) claim_owner: Option<super::verification::lease::Owner>,
    // 只来自真实RPC等待端的宿主对象；不向插件序列化，不替代账号或发送许可。
    pub(super) checkin_call: Option<super::checkin_call::CheckinCallRef>,
    pub(super) managed: Option<crate::server::core::login::managed::Call>,
    pub(super) kiro_execution: Option<Arc<super::managed::kiro_execution::KiroExecution>>,
    pub(super) desktop_import: Option<super::native::chatglm::DesktopCall>,
    pub(super) desktop_launch: Option<super::desktop_launch::binding::Call>,
    pub(super) desktop_selected: bool,
    pub(super) supplied_credentials: Option<Value>,
    pub(super) supplied_cookie_file: Option<bytes::Bytes>,
    pub(super) supplied_legacy_credentials: Option<Value>,
    pub(super) legacy_secret: Option<super::legacy_secret::Snapshot>,
    pub(super) conversation: Option<super::conversation::ConversationCall>,
    pub(super) catpaw_material: Option<Arc<super::conversation::CatPawMaterial>>,
    pub(super) doubao_turn: Option<super::doubao_turn::Turn>,
    pub(super) codearts_session: Option<super::codearts_session::Session>,
    pub(super) codearts_refresh: Option<Arc<super::codearts_refresh::Call>>,
    pub(super) cline_free_refresh: Option<Arc<super::cline_free_refresh::Call>>,
    pub(super) doubao_rewards: Option<Arc<super::doubao_rewards::Call>>,
    pub(super) task_service: super::managed_task::service::Owner,
    pub(super) catpaw_service: super::catpaw_turn::service::CallSlot,
    pub flow: String,
    pub session: String,
    pub client_stream: Option<bool>,
    pub proxy: Value,
    pub proxy_selected: bool,
    pub system_proxy_when_unset: bool,
    pub user_agent: Option<String>,
    pub trace: Value,
    pub sink: Option<Sink>,
    pub head_sent: bool,
    pub resources: HashMap<String, &'static str>,
    pub(super) http_handles: HashMap<String, super::super::script::HttpHandle>,
    pub(super) converters: HashMap<String, super::protocol::Conversion>,
    pub blobs: super::resources::Resources,
    pub(super) progress: super::progress::Progress,
    pub guards: HashMap<String, OwnedMutexGuard<()>>,
    pub calls: u32,
}

impl Context {
    pub fn new(shared: Arc<Shared>, params: &Value, sink: Option<Sink>, call_deadline: tokio::time::Instant) -> Self {
        let system_proxy_when_unset = shared.system_proxy_when_unset.load(Ordering::Acquire)
            && params["proxy_override"] != true && params["proxy"].is_null();
        Self {
            shared, call_deadline, account: params["account"].as_str().unwrap_or("").to_string(),
            identity: String::new(), origin_binding: None, usage_query: false, account_maintenance: false, verification: None, claim_owner: None, checkin_call: None, managed: None, kiro_execution: None,
            desktop_import: None, desktop_launch: None, desktop_selected: false, supplied_credentials: None, supplied_cookie_file: None, supplied_legacy_credentials: None, legacy_secret: None, conversation: None, catpaw_material: None, doubao_turn: None, codearts_session: None, codearts_refresh: None, cline_free_refresh: None, doubao_rewards: None,
            task_service: Default::default(), catpaw_service: Default::default(),
            flow: params["flow"].as_str().unwrap_or("").to_string(),
            session: params["session"].as_str().unwrap_or("").to_string(),
            client_stream: sink.as_ref().and_then(|_| params["client_stream"].as_bool()),
            proxy: params.get("proxy").cloned().unwrap_or(Value::Null),
            proxy_selected: params.get("proxy").is_some(), system_proxy_when_unset, user_agent: None,
            trace: params.get("plugin_trace").cloned().unwrap_or(Value::Null),
            sink, head_sent: false, resources: HashMap::new(), http_handles: HashMap::new(), converters: HashMap::new(),
            blobs: Default::default(), progress: Default::default(), guards: HashMap::new(), calls: 0,
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        self.task_service.cancel();
        self.catpaw_service.cancel(&self.shared.catpaw_turns);
        // 先撤销领取租约；WASM 陷阱/超时不依赖插件端析构或清理请求。
        drop(self.claim_owner.take());
        // 精确对象句柄先统一关闭；超时轻量终态也在这里回收，不按可能重用的 ID 关闭 HTTP。
        self.http_handles.clear();
        for (id, kind) in &self.resources {
            let params = serde_json::json!({ "id": id });
            match *kind {
                "ws" => super::super::script::abort_ws(&params),
                _ => {}
            }
        }
    }
}
