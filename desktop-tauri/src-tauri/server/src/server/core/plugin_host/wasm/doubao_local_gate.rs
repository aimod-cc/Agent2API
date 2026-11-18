//! 只等待未进入消费回调的本地锁竞争；回调进入、账号变化或存储收尾不明时不重试。

use std::time::Duration;

use tokio::time::Instant;

use crate::server::core::account_store::credential_version::{
    Error, RowFinalCallback, RowFinalCheck, RowFinalError, RowFinalState, StorageFailure,
};
use crate::server::db::TaskTransactionError;

pub(super) async fn wait<R, E>(
    deadline: Instant,
    unavailable: impl Fn() -> E,
    mut attempt: impl FnMut() -> Result<RowFinalCheck<Result<R, E>>, E>,
) -> Result<R, E> {
    let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    loop {
        if Instant::now() >= deadline { return Err(unavailable()); }
        match attempt()?.into_state() {
            RowFinalState::Entered { result: RowFinalCallback::Returned(result), storage_closed: true } => return result,
            RowFinalState::NotEntered(RowFinalError::AccountBusy
                | RowFinalError::Transaction(TaskTransactionError::Busy)
                | RowFinalError::Read(Error::Storage(StorageFailure::KeyUnavailable))) => {
                tokio::time::sleep_until(deadline.min(Instant::now() + Duration::from_millis(10))).await;
            }
            RowFinalState::NotEntered(reason) => {
                let detail = match reason {
                    RowFinalError::Read(error) => format!("{error:?}"),
                    RowFinalError::AccountPoisoned => "account_lock_unavailable".into(),
                    RowFinalError::StoreUnavailable => "store_unavailable".into(),
                    RowFinalError::Transaction(_) => "transaction_unavailable".into(),
                    _ => "local_check_interrupted".into(),
                };
                crate::server::logging::verbose("[DoubaoWork]", &format!("本地账号核对未进入：{detail}"));
                return Err(unavailable());
            }
            RowFinalState::Entered { .. } => {
                crate::server::logging::verbose("[DoubaoWork]", "本地账号核对收尾未确认，未重复消费或发送请求");
                return Err(unavailable());
            }
        }
    }
}
