//! 只等待未进入消费回调的本地锁竞争；已经进入、账号变化或存储收尾未知均不重试。

use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::time::Instant;

use super::error;
use crate::server::core::account_store::credential_version::{
    RowFinalCallback, RowFinalCheck, RowFinalError, RowFinalState, StorageFailure,
};
use crate::server::db::TaskTransactionError;

const WAIT_BUDGET: Duration = Duration::from_secs(2);
const WAIT_STEP: Duration = Duration::from_millis(10);

pub(super) async fn wait<R>(
    deadline: Instant,
    mut attempt: impl FnMut() -> Result<RowFinalCheck<Result<R, proto::WireError>>, proto::WireError>,
) -> Result<R, proto::WireError> {
    let deadline = deadline.min(Instant::now() + WAIT_BUDGET);
    loop {
        if Instant::now() >= deadline { return Err(error::unavailable()); }
        match attempt()?.into_state() {
            RowFinalState::Entered {
                result: RowFinalCallback::Returned(result), storage_closed: true,
            } => {
                if let Err(error) = &result {
                    crate::server::logging::verbose("[CatPaw]", &format!("原行消费回调失败：{}",
                        error.code.as_deref().unwrap_or("host_error")));
                }
                return result;
            }
            RowFinalState::Entered { .. } => return Err(error::unknown()),
            RowFinalState::NotEntered(RowFinalError::AccountBusy
                | RowFinalError::Transaction(TaskTransactionError::Busy)
                | RowFinalError::Read(crate::server::core::account_store::credential_version::Error::Storage(
                    StorageFailure::KeyUnavailable))) => {
                tokio::time::sleep_until(deadline.min(Instant::now() + WAIT_STEP)).await;
            }
            RowFinalState::NotEntered(reason) => {
                let detail = match reason {
                    RowFinalError::Read(error) => format!("{error:?}"),
                    RowFinalError::AccountPoisoned => "account_lock_unavailable".into(),
                    RowFinalError::StoreUnavailable => "store_unavailable".into(),
                    RowFinalError::Transaction(_) => "transaction_unavailable".into(),
                    _ => "local_gate_unavailable".into(),
                };
                crate::server::logging::verbose("[CatPaw]", &format!("原行最终门未进入：{detail}"));
                return Err(error::unavailable());
            }
        }
    }
}

pub(super) async fn account_value<R>(
    account: &super::super::catpaw_account::Account,
    consume: impl FnOnce() -> Result<R, proto::WireError>,
) -> Result<R, proto::WireError> {
    let mut consume = Some(consume);
    wait(Instant::now() + WAIT_BUDGET, || Ok(account.try_final_check(|| {
        consume.take().ok_or_else(error::cleanup)?()
    }))).await
}
