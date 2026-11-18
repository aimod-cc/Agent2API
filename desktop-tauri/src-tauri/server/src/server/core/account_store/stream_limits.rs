//! 原流账号冷却的有限执行门面；最多4个真实阻塞工作，等待取消不提前归还许可。

use std::sync::{Arc, OnceLock};
use tokio::{sync::Semaphore, time::Instant};

use super::{sql::stream_limits, AccountStore};
use crate::server::config::error_policy::{CooldownScope, ErrorRule};
use crate::server::core::upstream::cancellation::CancelToken;

pub(crate) struct StreamLimitSource {
    store: AccountStore,
    stamp: stream_limits::Stamp,
    revision: u64,
}

/// Future被外层超时或消费端丢弃时，只关闭本次本地写入资格，不启动补偿任务。
struct Waiting(Arc<std::sync::atomic::AtomicBool>);
impl Drop for Waiting {
    fn drop(&mut self) { self.0.store(true, std::sync::atomic::Ordering::Release); }
}

fn slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4)))
}

impl AccountStore {
    pub(crate) async fn workbuddy_stream_limit_source(&self, account: String, model: String,
        cancel: Option<Arc<CancelToken>>,
    ) -> Result<StreamLimitSource, String> {
        let permit = slots().clone().try_acquire_owned().map_err(|_| unavailable())?;
        let store = self.clone();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        let waiting = Waiting(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let abandoned = waiting.0.clone();
        let worker_cancel = cancel.clone();
        let read = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let guard = store.guard();
            if stopped(deadline, worker_cancel.as_deref(), &abandoned) { return Err(unavailable()); }
            let db = store.database().ok_or_else(unavailable)?;
            let (stamp, revision) = store.with_conn(&guard, |conn| {
                if stopped(deadline, worker_cancel.as_deref(), &abandoned) { return Ok(Err(unavailable())); }
                let revision = db.account_revision();
                Ok(stream_limits::capture(conn, &account, &model).and_then(|stamp| {
                    if db.account_revision() != revision { return Err(unavailable()); }
                    Ok((stamp, revision))
                }))
            }).map_err(|_| unavailable())??;
            drop(guard);
            Ok(StreamLimitSource { store, stamp, revision })
        });
        let cancelled = async { match cancel { Some(token) => token.cancelled().await, None => std::future::pending().await } };
        tokio::select! {
            biased;
            _ = cancelled => Err(unavailable()),
            result = tokio::time::timeout_at(deadline, read) => result.map_err(|_| unavailable())?.map_err(|_| unavailable())?,
        }
    }
}

impl StreamLimitSource {
    pub(crate) async fn record(self, status: u16, code: Option<i64>, reset_at: i64, message: String,
        rule: ErrorRule, scope: CooldownScope, floor_ms: u64, deadline: Instant, cancel: Option<Arc<CancelToken>>,
    ) -> Result<serde_json::Value, String> {
        let permit = slots().clone().try_acquire_owned().map_err(|_| unavailable())?;
        let waiting = Waiting(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let abandoned = waiting.0.clone();
        if stopped(deadline, cancel.as_deref(), &abandoned) { return Err(unavailable()); }
        let worker_cancel = cancel.clone();
        let write = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let guard = self.store.guard();
            if stopped(deadline, worker_cancel.as_deref(), &abandoned) { return Err(unavailable()); }
            let db = self.store.database().ok_or_else(unavailable)?;
            self.store.with_conn_mut(&guard, |conn| Ok(stream_limits::apply(conn, self.stamp, status, code,
                reset_at, &message, &rule, scope, floor_ms, deadline, worker_cancel.as_deref(), &abandoned, |rowid| {
                    let changes = db.account_changes(Some(self.revision));
                    changes.revision >= self.revision && changes.rows.is_some_and(|rows| !rows.contains(&rowid))
                }))).map_err(|_| unavailable())?
        });
        let cancelled = async { match cancel { Some(token) => token.cancelled().await, None => std::future::pending().await } };
        tokio::select! {
            biased;
            _ = cancelled => Err(unavailable()),
            result = tokio::time::timeout_at(deadline, write) => result.map_err(|_| unavailable())?.map_err(|_| unavailable())?,
        }
    }
}

fn stopped(deadline: Instant, cancel: Option<&CancelToken>, abandoned: &std::sync::atomic::AtomicBool) -> bool {
    Instant::now() >= deadline || cancel.is_some_and(CancelToken::is_cancelled)
        || abandoned.load(std::sync::atomic::Ordering::Acquire)
}

fn unavailable() -> String { "WorkBuddy冷却记录未确认（来源变化、取消、超时或存储正忙），未重试".into() }
