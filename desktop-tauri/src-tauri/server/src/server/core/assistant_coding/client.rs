use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use crate::server::{db::Db, logging};
use super::{config::{self, CodingConfig, ConfigInput}, executable, process::{self, ProcessControl}, state::SessionState,
    terminal_gateway::TerminalGateway, transport::{self, WriteRequest}, string, CodingError};

static RUNTIME: OnceLock<AsyncMutex<Option<Arc<CodexClient>>>> = OnceLock::new();
static STOPPING: AtomicBool = AtomicBool::new(false);
static SERVER_ID: AtomicU64 = AtomicU64::new(0);
static LIFECYCLE: Mutex<()> = Mutex::new(());
fn runtime() -> &'static AsyncMutex<Option<Arc<CodexClient>>> { RUNTIME.get_or_init(|| AsyncMutex::new(None)) }

pub struct CodexClient {
    pub config: CodingConfig,
    pub db: Db,
    pub scope: String,
    pub generation: String,
    server_id: u64,
    pub version: String,
    pub executable: executable::Executable,
    pub endpoint: String,
    pub connection_token: String,
    pub state: SessionState,
    pub running: AtomicBool,
    pub connected: AtomicBool,
    pub connection_epoch: AtomicU64,
    pub process: Arc<ProcessControl>,
    pub writer: AsyncMutex<Option<mpsc::Sender<WriteRequest>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, CodingError>>>>,
    next_id: AtomicU64,
    pub operations: Arc<AsyncMutex<()>>,
    pub terminal_gateway: OnceLock<TerminalGateway>,
    pub catalog_cache: AsyncMutex<Option<(i64, Value)>>,
    pub usage_cache: AsyncMutex<Option<(i64, Value)>>,
}

pub fn allow_start() -> u64 {
    let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    let id = SERVER_ID.fetch_add(1, Ordering::AcqRel) + 1;
    STOPPING.store(false, Ordering::Release);
    id
}

pub fn request_shutdown() {
    let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    STOPPING.store(true, Ordering::Release);
    process::stop_all();
}

pub async fn shutdown(server_id: u64) {
    {
        let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
        if SERVER_ID.load(Ordering::Acquire) != server_id { return; }
        STOPPING.store(true, Ordering::Release);
        process::stop_all();
    }
    let mut slot = runtime().lock().await;
    if slot.as_ref().is_some_and(|c| c.server_id == server_id) {
        if let Some(client) = slot.take() { client.stop().await; }
    }
}

pub async fn save_config(db: &Db, input: ConfigInput) -> Result<CodingConfig, CodingError> {
    let next = config::prepare(input)?;
    let mut slot = runtime().lock().await;
    if STOPPING.load(Ordering::Acquire) { return Err(CodingError::new(503, "后台正在关闭，未修改本机设置")); }
    if let Some(client) = slot.as_ref() {
        if client.config != next && client.running.load(Ordering::Acquire) {
            let _operation = client.operations.lock().await;
            {
                let state = client.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                if !client.connected.load(Ordering::Acquire) || state.threads.values().any(|s| s.busy() || s.terminals > 0 || s.terminal_pending > 0) {
                    return Err(CodingError::new(409, "Codex 仍在执行、重连或连接着官方终端，请先停止任务并关闭终端再修改设置"));
                }
                if state.permissions.values().any(|p| !p.submitted) { return Err(CodingError::new(409, "Codex 仍有等待处理的审批，请先处理或结束会话")); }
            }
            client.stop().await;
            if !client.process.exited.load(Ordering::Acquire) { return Err(CodingError::new(409, "旧 Codex 进程尚未退出，未修改设置")); }
        }
    }
    config::save(db, &next)?;
    if slot.as_ref().is_some_and(|c| c.config != next) { *slot = None; }
    Ok(next)
}

pub async fn connect(db: &Db) -> Result<Arc<CodexClient>, CodingError> {
    let db = db.clone();
    tokio::spawn(async move { connect_inner(&db).await }).await
        .map_err(|_| CodingError::new(500, "启动本机 Codex 的后台操作中断"))?
}

async fn connect_inner(db: &Db) -> Result<Arc<CodexClient>, CodingError> {
    if STOPPING.load(Ordering::Acquire) { return Err(CodingError::new(503, "后台正在关闭")); }
    let server_id = SERVER_ID.load(Ordering::Acquire);
    let mut slot = runtime().lock().await;
    if STOPPING.load(Ordering::Acquire) || SERVER_ID.load(Ordering::Acquire) != server_id { return Err(CodingError::new(503, "后台正在关闭或重启")); }
    let config = config::load(db)?;
    if !config.enabled { return Err(CodingError::new(409, "本机 Codex 尚未启用，请先在本机设置中启用")); }
    if let Some(client) = slot.as_ref() {
        if client.config == config && client.server_id == server_id && client.running.load(Ordering::Acquire) { return Ok(client.clone()); }
        client.stop().await;
        if !client.process.exited.load(Ordering::Acquire) { return Err(CodingError::new(409, "旧 Codex 进程尚未退出，未启动新进程")); }
    }
    let executable = executable::resolve(&config)?;
    let version = executable.version(&config).await?;
    let connection_token = crate::server::access::random_hex(32);
    let mut command = executable.command(&config);
    command.args(["app-server", "--listen", "ws://127.0.0.1:0", "--ws-auth", "capability-token", "--ws-token-sha256"])
        .arg(format!("{:x}", Sha256::digest(connection_token.as_bytes())))
        .env("NO_COLOR", "1").env_remove("FORCE_COLOR")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| CodingError::new(500, format!("启动本机 Codex 失败：{e}")))?;
    let stderr = child.stderr.take().ok_or_else(|| CodingError::new(500, "Codex 启动管道未建立"))?;
    let process = ProcessControl::new(child)?;
    let endpoint = match tokio::time::timeout(Duration::from_secs(15), process::read_endpoint(stderr)).await {
        Ok(Ok(Ok(endpoint))) => endpoint,
        _ => { process.kill(); process.wait().await; return Err(CodingError::new(503, "Codex 未提供安全的本机共享连接，请升级官方 Codex 后重试")); }
    };
    let client = Arc::new(CodexClient {
        scope: config.scope(), config, db: db.clone(), generation: crate::server::access::random_hex(12), server_id,
        version, executable, endpoint, connection_token, state: SessionState::new(), running: AtomicBool::new(true),
        connected: AtomicBool::new(false), connection_epoch: AtomicU64::new(0), process,
        writer: AsyncMutex::new(None), pending: Mutex::new(HashMap::new()), next_id: AtomicU64::new(1), operations: Arc::new(AsyncMutex::new(())),
        terminal_gateway: OnceLock::new(), catalog_cache: AsyncMutex::new(None), usage_cache: AsyncMutex::new(None),
    });
    process::supervise(client.clone());
    if STOPPING.load(Ordering::Acquire) || SERVER_ID.load(Ordering::Acquire) != server_id {
        client.stop().await;
        return Err(CodingError::new(503, "后台正在关闭或重启"));
    }
    let (tx, rx) = oneshot::channel();
    tokio::spawn(transport::run(client.clone(), tx));
    match tokio::time::timeout(Duration::from_secs(50), rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => { client.stop().await; return Err(error); }
        _ => { client.stop().await; return Err(CodingError::new(503, "Codex 共享后台初始化失败，请检查版本及本机配置")); }
    }
    let gateway = match TerminalGateway::start(Arc::downgrade(&client)).await {
        Ok(gateway) => gateway,
        Err(error) => { client.stop().await; return Err(error); }
    };
    let _ = client.terminal_gateway.set(gateway);
    if STOPPING.load(Ordering::Acquire) || SERVER_ID.load(Ordering::Acquire) != server_id { client.stop().await; return Err(CodingError::new(503, "后台正在关闭或重启")); }
    logging::log("[Codex]", &format!("本机 Codex 共享后台已启动：{}", client.version));
    *slot = Some(client.clone());
    Ok(client)
}

impl CodexClient {
    pub fn projects(&self) -> Result<Vec<super::projects::CodingProject>, CodingError> {
        super::projects::load(&self.db, &self.scope)
    }

    pub fn directory(&self, raw: &str) -> Result<std::path::PathBuf, CodingError> {
        super::projects::directory(&self.projects()?, raw)
    }

    pub async fn check_current(&self) -> Result<(), CodingError> {
        let config = config::load(&self.db)?;
        if !config.enabled || config.revision() != self.config.revision() || !self.running.load(Ordering::Acquire)
            || STOPPING.load(Ordering::Acquire) || SERVER_ID.load(Ordering::Acquire) != self.server_id {
            return Err(CodingError::new(409, "本机 Codex 状态已变化，请刷新后再操作"));
        }
        if !self.connected.load(Ordering::Acquire) { return Err(CodingError::new(503, "正在重新连接 Codex；电脑上的任务仍会继续，请稍后再操作")); }
        Ok(())
    }

    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, CodingError> {
        if !self.running.load(Ordering::Acquire) { return Err(CodingError::new(503, "本机 Codex 已退出，请重新打开会话")); }
        let revision = self.state.inner.lock().unwrap_or_else(|e| e.into_inner()).revision;
        let id = format!("a2a-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), tx);
        if let Err(error) = self.write(json!({ "id": id, "method": method, "params": params })).await {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return Err(error);
        }
        let result = tokio::time::timeout(Duration::from_secs(45), rx).await;
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        match result {
            Ok(Ok(Ok(mut value))) => {
                if value.get("thread").is_some() { value["_agent2apiRevision"] = json!(revision); }
                Ok(value)
            }
            Ok(Ok(Err(error))) => Err(error),
            _ => Err(CodingError::uncertain("Codex 响应中断或超时，无法确认操作结果；请查看会话，不要重复提交")),
        }
    }

    pub async fn write(&self, value: Value) -> Result<(), CodingError> {
        let sender = self.writer.lock().await.clone().ok_or_else(|| CodingError::new(503, "Codex 连接正在恢复，未发送操作"))?;
        let (tx, rx) = oneshot::channel();
        let send = async {
            sender.send(WriteRequest { value, result: tx }).await.map_err(|_| CodingError::uncertain("Codex 连接中断，未自动重发"))?;
            rx.await.map_err(|_| CodingError::uncertain("Codex 连接中断，未自动重发"))?
        };
        tokio::time::timeout(Duration::from_secs(15), send).await
            .unwrap_or_else(|_| Err(CodingError::uncertain("Codex 写入超时，未自动重发")))
    }

    pub fn resolve(&self, message: Value) {
        let Some(id) = message.get("id").and_then(Value::as_str) else { return; };
        let sender = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        if let Some(sender) = sender {
            let result = if let Some(error) = message.get("error") {
                let text = string(error, "message").chars().take(1200).collect::<String>();
                Err(CodingError { status: 422, message: format!("Codex：{text}"), code: "codex_rejected" })
            } else { Ok(message.get("result").cloned().unwrap_or(Value::Null)) };
            let _ = sender.send(result);
        }
    }

    pub fn transport_lost(&self) {
        self.connected.store(false, Ordering::Release);
        self.connection_epoch.fetch_add(1, Ordering::AcqRel);
        {
            let mut state = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.permissions.clear(); state.revision += 1;
            for thread in state.threads.values_mut() {
                thread.items.clear(); thread.item_versions.clear(); thread.item_order.clear();
                thread.usage_source = thread.usage_source.as_ref().map(|_| "history".into());
                thread.metadata_checked_at = 0;
            }
        }
        for (_, sender) in self.pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = sender.send(Err(CodingError::uncertain("Codex 连接中断，未自动重发操作")));
        }
        self.state.changed_all();
        self.process.notify.notify_waiters();
    }

    pub fn disconnected(&self, message: &str) {
        if !self.running.swap(false, Ordering::AcqRel) { return; }
        if let Some(gateway) = self.terminal_gateway.get() { gateway.close(); }
        self.transport_lost();
        {
            let mut state = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.last_error = Some(message.into());
            for thread in state.threads.values_mut() {
                thread.attached = false; thread.turn = None; thread.terminals = 0;
                thread.thread["status"] = json!({ "type": "notLoaded" });
            }
        }
        self.state.changed_all();
        logging::log("[Codex]", message);
    }

    pub async fn stop(&self) {
        self.disconnected("本机 Codex 后台正在停止，历史记录仍保留");
        *self.writer.lock().await = None;
        self.process.kill();
        self.process.wait().await;
    }
}
