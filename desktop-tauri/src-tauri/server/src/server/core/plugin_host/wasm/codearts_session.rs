use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}, time::Duration};
use agent2api_provider_protocol as proto;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{sync::{Notify, Semaphore, OwnedSemaphorePermit}, time::Instant};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::context::Context;
use crate::server::core::egress::managed::FrozenHttpClient;

const ORIGIN: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";
const HEARTBEAT: &str = "/snap-manager/v1/chat-session/heartbeat";

pub(super) struct Session {
    pub(super) id: String,
    pub(super) client: FrozenHttpClient,
    stop: CancellationToken,
    ended: Arc<Notify>,
    completed: Arc<std::sync::atomic::AtomicBool>,
}

impl Session {
    pub(super) async fn close(&self) {
        self.stop.cancel();
        loop {
            let wait = self.ended.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            if self.completed.load(std::sync::atomic::Ordering::Acquire) { break; }
            wait.await;
        }
    }
}
impl Drop for Session { fn drop(&mut self) { self.stop.cancel(); } }

struct Heartbeat {
    shared: std::sync::Weak<super::context::Shared>,
    account: String,
    original_secret: Value,
    access: Zeroizing<String>,
    secret: Zeroizing<String>,
    security: Zeroizing<String>,
    agent: String,
    id: String,
    client: FrozenHttpClient,
    _slot: OwnedSemaphorePermit,
}

fn slots(identity: &str) -> Result<Arc<Semaphore>, proto::WireError> {
    static ITEMS: OnceLock<Mutex<HashMap<String, std::sync::Weak<Semaphore>>>> = OnceLock::new();
    let mut items = ITEMS.get_or_init(Mutex::default).lock().map_err(|_| invalid())?;
    items.retain(|_, value| value.strong_count() > 0);
    if let Some(existing) = items.get(identity).and_then(std::sync::Weak::upgrade) { return Ok(existing); }
    if items.len() >= 256 { return Err(invalid()); }
    let value = Arc::new(Semaphore::new(3));
    items.insert(identity.into(),Arc::downgrade(&value));
    Ok(value)
}

impl Context {
    pub(super) async fn codearts_session(&mut self, method: &str, params: &Value) -> Result<Value, proto::WireError> {
        self.permission("http.codearts-session.v1")?;
        if self.shared.options.provider != "codearts" || self.conversation.is_none()
            || !params.as_object().is_some_and(|fields| fields.is_empty()) { return Err(invalid()); }
        if method == "codearts.session.close" {
            if let Some(session) = self.codearts_session.take() { session.close().await; }
            return Ok(Value::Null);
        }
        if self.codearts_session.is_some() { return Err(invalid()); }
        let call = self.conversation.as_ref().ok_or_else(invalid)?;
        let path = call.selected_path(&self.shared,&self.account)?;
        let deadline = self.call_deadline.min(Instant::now() + Duration::from_secs(30));
        let client = tokio::time::timeout_at(deadline,FrozenHttpClient::from_path(path)).await
            .map_err(|_| invalid())?.map_err(|_| invalid())?;
        let record = self.shared.callbacks.store_get("codearts".into(),self.account.clone()).await?;
        if record["provider"] != "codearts" || record["public"]["enabled"] == false { return Err(invalid()); }
        let secret = &record["secret"];
        let field = |name: &str| -> Result<Zeroizing<String>,proto::WireError> {
            secret[name].as_str().filter(|value| !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control))
                .map(|value| Zeroizing::new(value.to_string())).ok_or_else(invalid)
        };
        let component = |name: &str| -> Result<Option<&str>, proto::WireError> {
            match secret.get(name).filter(|value| !value.is_null()) {
                None => Ok(None),
                Some(Value::String(value)) if value.len() <= 1024 && !value.chars().any(char::is_control) =>
                    Ok((!value.trim().is_empty()).then_some(value.trim())),
                _ => Err(invalid()),
            }
        };
        let domain = component("domain_id")?;
        let user = component("user_id")?;
        let identity = match (domain, user) {
            (Some(domain), Some(user)) => format!("{domain}:{user}"),
            _ if record["id"].as_str() == Some(self.account.as_str()) => format!("account:{}", self.account),
            _ => return Err(invalid()),
        };
        let slot = slots(&identity)?.try_acquire_owned().map_err(|_| proto::WireError::with_code(
            "CodeArts 当前账号已有三路对话，请稍后重试",
            format!("provider_http:{}", json!({"status":429,"code":"codearts_concurrency_limited"})),
        ))?;
        let mut random = [0u8;16];
        getrandom::getrandom(&mut random).map_err(|_| invalid())?;
        let id = format!("{:032x}",u128::from_be_bytes(random));
        let heartbeat = Heartbeat { shared:Arc::downgrade(&self.shared), account:self.account.clone(), original_secret:secret.clone(),
            access:field("access_key_id")?,secret:field("secret_access_key")?,security:field("security_token")?,
            id:id.clone(),agent:crate::server::core::user_agent::override_value(
                "codearts", record["public"]["userAgent"].as_str().unwrap_or(""),
            ).to_string(),client:client.clone(),_slot:slot };
        let stop = CancellationToken::new();
        let ended = Arc::new(Notify::new());
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let owner = Session {id:id.clone(),client,stop:stop.clone(),ended:ended.clone(),completed:completed.clone()};
        self.codearts_session = Some(owner);
        let call_ended = call.ended_token();
        let runtime = self.shared.cancel.clone();
        let call_deadline = self.call_deadline;
        let (sent,received) = tokio::sync::oneshot::channel();
        crate::spawn_task(async move {
            let first = heartbeat.send("busy").await;
            let accepted = first.is_ok();
            let _ = sent.send(first);
            if accepted {
                loop {
                    tokio::select! { biased;
                        _ = stop.cancelled() => break,
                        _ = call_ended.cancelled() => break,
                        _ = runtime.cancelled() => break,
                        _ = tokio::time::sleep_until(call_deadline) => break,
                        _ = tokio::time::sleep(Duration::from_secs(30)) => { if heartbeat.send("busy").await.is_err() { break; } }
                    }
                }
            }
            if heartbeat.send("idle").await.is_err() { crate::server::logging::log("[CodeArts]","本次会话注销未获上游确认，未重试或注销其他会话"); }
            drop(heartbeat);
            completed.store(true,std::sync::atomic::Ordering::Release);
            ended.notify_waiters();
        });
        received.await.map_err(|_| invalid())??;
        Ok(json!({"id":id}))
    }
}

impl Heartbeat {
    async fn send(&self,status: &str) -> Result<(),proto::WireError> {
        let shared = self.shared.upgrade().ok_or_else(invalid)?;
        let record = shared.callbacks.store_get("codearts".into(), self.account.clone()).await?;
        if record["secret"] != self.original_secret || record["public"]["enabled"] == false { return Err(invalid()); }
        let date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let mut random = [0u8;16]; getrandom::getrandom(&mut random).map_err(|_| invalid())?;
        let trace = format!("{:032x}",u128::from_be_bytes(random));
        let mut headers = vec![("content-type","application/json".to_string()),("accept","application/json".to_string()),
            ("x-language","en-us".to_string()),("x-client-type","kernel".to_string()),("user-session-id",self.id.clone()),
            ("x-snap-traceid",trace),("x-security-token",self.security.to_string()),("x-sdk-date",date.clone()),
            ("host","snap-access.cn-north-4.myhuaweicloud.com".to_string())];
        headers.sort_by_key(|(name,_)| *name);
        let names = headers.iter().map(|(name,_)| *name).collect::<Vec<_>>().join(";");
        let canonical = headers.iter().map(|(name,value)| format!("{name}:{}\n",value.trim())).collect::<String>();
        let input = format!("PUT\n{HEARTBEAT}/\nstatus={status}\n{canonical}\n{names}\n{:x}",Sha256::digest(b"{}"));
        let to_sign = format!("SDK-HMAC-SHA256\n{date}\n{:x}",Sha256::digest(input));
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes()).map_err(|_| invalid())?;
        mac.update(to_sign.as_bytes());
        let auth = format!("SDK-HMAC-SHA256 Access={}, SignedHeaders={names}, Signature={:x}",self.access.as_str(),mac.finalize().into_bytes());
        let client = self.client.client().map_err(|_| invalid())?;
        let mut request = client.put(format!("{ORIGIN}{HEARTBEAT}?status={status}")).body("{}").header("authorization",auth);
        for (name,value) in headers { request = request.header(name,value); }
        if !self.agent.is_empty() { request = request.header("user-agent",&self.agent); }
        let result = tokio::time::timeout(Duration::from_secs(20),async {
            let response = request.send().await.map_err(|_| invalid())?;
            let status = response.status().as_u16();
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
                if bytes.len() + chunk.len() > 64 * 1024 { return Err(invalid()); } bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if status != 200 || value["status"] != "ok" { return Err(invalid()); }
            Ok(())
        }).await.map_err(|_| invalid())?;
        result
    }
}
fn invalid() -> proto::WireError { proto::WireError::with_code("CodeArts 本次会话准备或心跳未完成，未使用其他账号或旧会话","codearts_session_unavailable") }
