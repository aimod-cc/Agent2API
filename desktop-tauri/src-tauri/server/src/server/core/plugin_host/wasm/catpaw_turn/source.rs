//! 原 SourceRef + 真实 CatPaw RowSource 的封闭组合；没有 JSON/账号 ID 构造器。
//! 最终门只同步核对、冻结与 CAS；RowFinalCheck 必须在全部外层锁释放后拆开。
use agent2api_provider_protocol as proto;

use super::{error, registry::Record};
use crate::server::core::account_store::credential_version::RowProvider;
use super::super::catpaw_account::Account as RowSource;
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

pub(super) fn check(source: &SourceRef, account: &RowSource) -> Result<(), proto::WireError> {
    if source.provider() != super::PROVIDER
        || account.provider() != RowProvider::CatPaw
        || account.account() != source.account()
        || account.identity().is_empty()
    {
        return Err(error::unavailable());
    }
    account.check_desktop()?;
    source.check_current().map_err(|_| error::unavailable())?;
    super::super::catpaw_account::credentials(account)
        .map(|_| ())
        .map_err(|_| error::unavailable())
}

pub(super) async fn submit(
    source: &SourceRef,
    account: &RowSource,
    record: &Record,
) -> Result<(), proto::WireError> {
    final_value(source, account, || record.mark_submission()).await
}

pub(super) async fn subsequent(
    source: &SourceRef,
    account: &RowSource,
    consume: impl FnOnce() -> Result<(), proto::WireError>,
) -> Result<(), proto::WireError> {
    final_value(source, account, consume).await
}

pub(super) async fn final_value<R>(
    source: &SourceRef,
    account: &RowSource,
    consume: impl FnOnce() -> Result<R, proto::WireError>,
) -> Result<R, proto::WireError> {
    let mut consume = Some(consume);
    super::local_gate::wait(source.deadline(), || {
        check(source, account)?;
        source.with_submission_gate(|gate| Ok(account.try_final_check(|| {
            gate.freeze_replay()?;
            consume.take().ok_or_else(error::conflict)?()
        })))
    }).await
}

pub(super) async fn inspect_account(source: &SourceRef, account: &RowSource) -> Result<(), proto::WireError> {
    super::local_gate::wait(source.deadline(), || {
        check(source, account)?;
        source.with_submission_gate(|_| Ok(account.try_final_check(|| Ok(()))))
    }).await
}
