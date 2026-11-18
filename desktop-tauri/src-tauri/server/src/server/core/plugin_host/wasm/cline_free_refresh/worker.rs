//! 取消等待不丢弃已消费的Free原Pending；整个后台工作持原运行占用至同步保存收尾。

use std::{sync::{Arc, OnceLock}, time::Duration};

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::{sync::{OwnedSemaphorePermit, Semaphore}, time::Instant};
use zeroize::Zeroizing;

use super::{protocol::{self, MAX_RESPONSE, URL}, unavailable, Call};
use crate::server::core::{account_store::plugin_accounts, egress::managed::FrozenHttpClient};

struct Waiter { call: Arc<Call> }
impl Drop for Waiter {
    fn drop(&mut self) { self.call.close(); }
}
struct Owner { call: Arc<Call>, _permit: OwnedSemaphorePermit }

fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}

pub(super) async fn exchange(call: Arc<Call>) -> Result<Value, proto::WireError> {
    let waiter = Waiter { call: call.clone() };
    call.prepare()?;
    let permit = slots().try_acquire_owned().map_err(|_| unavailable())?;
    let owner = Owner { call: call.clone(), _permit: permit };
    let job = crate::spawn_task(async move { run(owner).await });
    let outcome = tokio::select! {
        biased;
        _ = call.closed() => Err(unavailable()),
        outcome = job => outcome.map_err(|_| unavailable())?,
    };
    drop(waiter);
    outcome
}

async fn run(owner: Owner) -> Result<Value, proto::WireError> {
    let call = &owner.call;
    call.current()?;
    let store = plugin_accounts::shared().ok_or_else(unavailable)?;
    let account = call.account.clone();
    let origin = tokio::task::spawn_blocking(move || store.read_free_refresh_origin(&account)).await
        .map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    call.current()?;
    let uid = origin.secret()["account"].as_str().filter(|uid| !uid.is_empty()).ok_or_else(unavailable)?.to_string();
    let mut public = origin.public().clone();
    if let Some(name) = origin.secret().get("displayName").filter(|value| value.is_string()) {
        public["displayName"] = name.clone();
    }
    let body = protocol::request(origin.secret())?;
    let path = call.take_path(&origin)?;
    let client = tokio::time::timeout_at(call.deadline.min(Instant::now() + Duration::from_secs(5)), FrozenHttpClient::from_path(path))
        .await.map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    let request = client.client().map_err(|_| unavailable())?.post(URL)
        .header("accept", "application/json").header("content-type", "application/json")
        .body(body.as_bytes().to_vec()).build().map_err(|_| unavailable())?;
    if request.method() != reqwest::Method::POST || request.url().as_str() != URL
        || request.body().and_then(reqwest::Body::as_bytes) != Some(body.as_bytes()) { return Err(unavailable()); }
    call.current()?;
    let pending = tokio::task::spawn_blocking(move || origin.consume()).await.map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    call.authorize_send(&pending)?;
    let bytes = tokio::time::timeout_at(call.deadline.min(Instant::now() + Duration::from_secs(25)), async {
        let mut response = client.client().map_err(|_| unavailable())?.execute(request).await.map_err(|_| unavailable())?;
        if !response.status().is_success() || response.url().as_str() != URL || response.headers().contains_key("location")
            || response.content_length().is_some_and(|length| length > MAX_RESPONSE as u64) { return Err(unavailable()); }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len().checked_add(chunk.len()).is_none_or(|size| size > MAX_RESPONSE) { return Err(unavailable()); }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }).await.map_err(|_| unavailable())??;
    let (secret, public) = protocol::replacement(&bytes, &uid, &public)?;
    let access = Zeroizing::new(secret.0["accessToken"].as_str().ok_or_else(unavailable)?.to_string());
    let original = call.clone();
    tokio::task::spawn_blocking(move || original.complete(&pending, &secret.0, &public)).await.map_err(|_| unavailable())??;
    call.current()?;
    Ok(Value::String(access.to_string()))
}
