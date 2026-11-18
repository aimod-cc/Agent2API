//! 原worker一次性准备：出口、凭据、请求、随机句柄均在同步最终门之外构造。
//! 最终回调只消费唯一机会；异常返回后先销毁全部材料，退出保护才可结清。

use std::sync::Arc;

use tokio::{sync::oneshot, time::sleep_until};

use super::super::{
    egress_preparation::{EgressPreparation, PreparedEgress},
    input::PreparedInput,
    lifecycle::Record,
    network::{PreparedCreate, Session},
    registry::{self, CreatePermit, FinalizeFailure},
    status::NotSentReason,
};

pub(super) async fn prepare(
    record: &Arc<Record>,
    input: PreparedInput,
    ready: oneshot::Receiver<()>,
) -> Result<(Session, PreparedCreate, CreatePermit, [u8; 32]), FinalizeFailure> {
    let egress = prepare_egress(record, ready)
        .await
        .map_err(FinalizeFailure::NotSent)?;
    if !Arc::ptr_eq(input.binding(), record.binding()) {
        return Err(FinalizeFailure::NotSent(NotSentReason::InvalidBinding));
    }
    let (finalization, body, account) = input.into_parts();
    let mut session = Session::new(record.binding().clone(), egress, account)
        .map_err(|_| FinalizeFailure::NotSent(NotSentReason::RequestUnavailable))?;
    let request = session
        .prepare_create(body)
        .map_err(|_| FinalizeFailure::NotSent(NotSentReason::RequestUnavailable))?;
    let mut nonce = [0u8; 32];
    getrandom::getrandom(&mut nonce)
        .map_err(|_| FinalizeFailure::NotSent(NotSentReason::RandomUnavailable))?;
    let permit = registry::finalize(record, finalization, &session, &request)?;
    Ok((session, request, permit, nonce))
}

async fn prepare_egress(
    record: &Arc<Record>,
    ready: oneshot::Receiver<()>,
) -> Result<PreparedEgress, NotSentReason> {
    tokio::select! {
        biased;
        _ = record.ended() => return Err(NotSentReason::CallEnded),
        _ = sleep_until(record.prepare_deadline()) => return Err(NotSentReason::PreparationExpired),
        ready = ready => ready.map_err(|_| NotSentReason::RuntimeUnavailable)?,
    }
    if let Some(reason) = record.preparing_error() {
        return Err(reason);
    }
    let egress = EgressPreparation::begin(record)
        .map_err(|reason| record.preparing_error().unwrap_or(reason))?;
    egress
        .freeze()
        .await
        .map_err(|reason| record.preparing_error().unwrap_or(reason))
}
