//! 首次只从原AccountStore准备RowSource；之后只复核该来源，不再次prepare。
//! 许可在阻塞闭包真正完成前一直保留；调用方取消不释放仍在运行的读取名额。

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::time::Instant;

use super::{check_call, material, source_matches_call, Material, SnapshotError};
use crate::server::core::plugin_host::wasm::conversation::TaskCallRef;
use crate::server::core::{
    account_store::{
        credential_version::{
            Error, RowFinalCallback, RowFinalError, RowFinalState, RowProvider, StorageFailure,
        },
        AccountStore,
    },
    blocking,
};
use crate::server::db::TaskTransactionError;

const MAX_READS: usize = 4;
const READ_TIMEOUT: Duration = Duration::from_secs(2);

fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_READS)))
        .clone()
}

pub(super) async fn read(
    call: &TaskCallRef,
    store: &AccountStore,
    local: Arc<Semaphore>,
) -> Result<Material, SnapshotError> {
    run(call, Reading::Initial(store.clone()), local).await
}

pub(super) async fn recheck(
    call: &TaskCallRef,
    previous: Material,
    local: Arc<Semaphore>,
) -> Result<Material, SnapshotError> {
    run(call, Reading::Existing(previous), local).await
}

// 复核分支根本不接受另一份store；旧来源只能整体移入worker，再整体交还。
enum Reading {
    Initial(AccountStore),
    Existing(Material),
}

async fn run(
    call: &TaskCallRef,
    reading: Reading,
    local: Arc<Semaphore>,
) -> Result<Material, SnapshotError> {
    check_call(call)?;
    // try_acquire不创建等待队列；本快照同时最多一次，所有快照合计最多四次阻塞读。
    let local = local.try_acquire_owned().map_err(|_| SnapshotError::Busy)?;
    let global = slots()
        .try_acquire_owned()
        .map_err(|_| SnapshotError::Busy)?;
    let deadline = call.call_deadline().min(Instant::now() + READ_TIMEOUT);
    let ended = call.ended_token();
    let worker_call = call.clone();
    let work = blocking::run(move || {
        let _local = local;
        let _global = global;
        check_call(&worker_call)?;
        if Instant::now() >= deadline {
            return Err(SnapshotError::TimedOut);
        }
        let material = match reading {
            Reading::Initial(store) => {
                // 唯一来源构造点：真实提交确认后才得到私有Db/代次/出生版本。
                // 仅可能建立宿主版本元数据；不换凭据、创建账号或更改启停。
                let source = store
                    .prepare_row_source(if worker_call.identity().provider() == "monkeycode-intl" { RowProvider::MonkeyCodeIntl } else { RowProvider::MonkeyCode }, worker_call.identity().account())
                    .map_err(source_error)?;
                check_call(&worker_call)?;
                if Instant::now() >= deadline {
                    return Err(SnapshotError::TimedOut);
                }
                material::from_source(source, &worker_call)?
            }
            Reading::Existing(previous) => {
                if !source_matches_call(&previous.source, &worker_call) {
                    return Err(SnapshotError::InvalidAccount);
                }
                // 点时核对只进入固定空动作；不消费Record，也不持Signal/Host等外层锁。
                // 失败、失效、忙或异常均终止，绝不重新prepare或补签旧调用。
                let checked = previous.source.try_final_check(|| ());
                match checked.into_state() {
                    RowFinalState::Entered {
                        result: RowFinalCallback::Returned(()),
                        storage_closed: true,
                    } => previous,
                    RowFinalState::NotEntered(error) => return Err(recheck_error(error)),
                    RowFinalState::Entered { .. } => return Err(SnapshotError::StorageUnavailable),
                }
            }
        };
        // 首读锁等待不可强行中止；迟到材料丢弃，真实读取许可随闭包结束才回收。
        check_call(&worker_call)?;
        if Instant::now() >= deadline {
            return Err(SnapshotError::TimedOut);
        }
        Ok(material)
    });
    let result = tokio::select! {
        biased;
        _ = ended.cancelled() => return Err(SnapshotError::CallEnded),
        _ = tokio::time::sleep_until(deadline) => return Err(SnapshotError::TimedOut),
        result = work => result.map_err(|_| SnapshotError::StorageUnavailable)?,
    }?;
    check_call(call)?;
    Ok(result)
}

fn source_error(error: Error) -> SnapshotError {
    match error {
        Error::InvalidInput | Error::UnsupportedProvider => SnapshotError::InvalidAccount,
        Error::InvalidRecord => SnapshotError::InvalidCredential,
        Error::LimitExceeded => SnapshotError::TooLarge,
        Error::VersionExhausted | Error::Conflict(_) => SnapshotError::AccountChanged,
        Error::Storage(StorageFailure::Busy) => SnapshotError::Busy,
        Error::Storage(_) => SnapshotError::StorageUnavailable,
    }
}

fn recheck_error(error: RowFinalError) -> SnapshotError {
    match error {
        RowFinalError::AccountBusy | RowFinalError::Transaction(TaskTransactionError::Busy) => {
            SnapshotError::Busy
        }
        RowFinalError::Read(error) => source_error(error),
        RowFinalError::AccountPoisoned
        | RowFinalError::StoreUnavailable
        | RowFinalError::Transaction(_)
        | RowFinalError::Interrupted => SnapshotError::StorageUnavailable,
    }
}
