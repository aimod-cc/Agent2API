use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{OwnedMutexGuard, Semaphore};
use tokio_util::sync::CancellationToken;

use super::super::{GatewayCallbacks, StreamEvent};
use super::manifest::Options;

pub struct Shared {
    pub options: Options,
    pub callbacks: Arc<dyn GatewayCallbacks>,
    pub cancel: CancellationToken,
    pub slots: Arc<Semaphore>,
    pub control_slots: Arc<Semaphore>,
    pub locks: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    pub sessions: Mutex<HashMap<String, (i64, Value)>>,
    pub(super) login_state: Mutex<super::state::Transient>,
    pub(super) interactions: super::login::Interactions,
    pub login_proxies: Mutex<HashMap<String, (i64, Value)>>,
}

impl Shared {
    pub fn new(options: Options, callbacks: Arc<dyn GatewayCallbacks>) -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(options.manifest.concurrency)),
            control_slots: Arc::new(Semaphore::new(2)), options, callbacks,
            cancel: CancellationToken::new(), locks: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()), login_state: Mutex::new(Default::default()),
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

pub struct Context {
    pub shared: Arc<Shared>,
    pub account: String,
    pub identity: String,
    pub flow: String,
    pub session: String,
    pub proxy: Value,
    pub proxy_selected: bool,
    pub user_agent: Option<String>,
    pub trace: Value,
    pub sink: Option<Sink>,
    pub head_sent: bool,
    pub resources: HashMap<String, &'static str>,
    pub blobs: super::resources::Resources,
    pub guards: HashMap<String, OwnedMutexGuard<()>>,
    pub calls: u32,
}

impl Context {
    pub fn new(shared: Arc<Shared>, params: &Value, sink: Option<Sink>) -> Self {
        Self {
            shared, account: params["account"].as_str().unwrap_or("").to_string(),
            identity: String::new(), flow: params["flow"].as_str().unwrap_or("").to_string(),
            session: params["session"].as_str().unwrap_or("").to_string(),
            proxy: params.get("proxy").cloned().unwrap_or(Value::Null),
            proxy_selected: params.get("proxy").is_some(), user_agent: None,
            trace: params.get("plugin_trace").cloned().unwrap_or(Value::Null),
            sink, head_sent: false, resources: HashMap::new(), blobs: Default::default(), guards: HashMap::new(), calls: 0,
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        for (id, kind) in &self.resources {
            let params = serde_json::json!({ "id": id });
            match *kind {
                "http" => super::super::script::close_http(&params),
                "ws" => super::super::script::abort_ws(&params),
                _ => {}
            }
        }
    }
}
