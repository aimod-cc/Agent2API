//! 原worker借原Reader核无工具完成或仅预约工具证据；工具预约不完成本轮、不移交责任。
//! 完成动作、真实受理、本地提交、交付独立；任何已占完成动作的未知禁止另一终态。
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{json_text, owner::Owner, registry::Record, settle::Command};

pub(super) enum ResultKind { Completed, AwaitingTools, Unconfirmed, Rejected }

pub(super) async fn execute(owner: &mut Owner, record: &Arc<Record>, command: Command) -> ResultKind {
    if !record.settlement_reserved(command.sequence) {
        let _ = command.delivery.send(Err(unconfirmed()));
        return if record.tool_reservation_consumed() { ResultKind::Unconfirmed } else { ResultKind::Rejected };
    }
    let checked = json_text::parse(&command.claims_json, 2 * 1024 * 1024).and_then(|claims| {
        json_text::fields(&claims, &["upstream_message", "observed_conversation_id", "done_event_name", "usage"], &["upstream_message", "observed_conversation_id", "done_event_name", "usage"])?;
        if owner.reserve_tool_evidence(record, command.sequence, &claims)? {
            return Ok(true);
        }
        owner.check_settlement(&record.prepared.constraints, &claims)?;
        Ok(false)
    });
    match checked {
        Ok(true) => {
            let result = owner.commit_handoff(record, command.sequence).await;
            match result {
                Ok(value) => {
                    if command.delivery.send(Ok(value)).is_err() {
                        record.lose_settlement();
                        return ResultKind::Unconfirmed;
                    }
                    return ResultKind::AwaitingTools;
                }
                Err(error) => {
                    record.note_failure(error.clone());
                    let _ = command.delivery.send(Err(error));
                    return ResultKind::Unconfirmed;
                }
            }
        }
        Err(failure) => {
            record.note_failure(failure.clone());
            let _ = command.delivery.send(Err(failure));
            // Record已消费但原证据移动失败/返回丢失也不走清理发送或恢复预约。
            return if record.tool_reservation_consumed() { ResultKind::Unconfirmed } else { ResultKind::Rejected };
        }
        Ok(false) => {}
    }
    let result = owner.complete(record, command.sequence).await;
    match result {
        Ok(value) => {
            if let Err(failure) = owner.bind_output(record, command.sequence) {
                record.note_failure(failure.clone());
                let _ = command.delivery.send(Err(failure));
                return ResultKind::Unconfirmed;
            }
            let _ = command.delivery.send(Ok(value));
            ResultKind::Completed
        }
        Err(failure) => {
            record.note_failure(failure.clone());
            let _ = command.delivery.send(Err(failure));
            if record.completion_reserved() { ResultKind::Unconfirmed } else { ResultKind::Rejected }
        }
    }
}

pub(super) fn compare(protocol: &super::protocol::Protocol, constraints: &Value, claims: &Value) -> Result<(), proto::WireError> {
    // complete来自原流的DONE/finished/收尾，调用者还须独立核原Response真实EOF。
    compare_claims(protocol, claims)?;
    protocol.check_no_tools(constraints)
}

/// 工具预约与无工具结算共用逐值原主张核对；成功本身不证明任何交付或许可。
pub(super) fn compare_claims(protocol: &super::protocol::Protocol, claims: &Value) -> Result<(), proto::WireError> {
    json_text::fields(claims, &["upstream_message", "observed_conversation_id", "done_event_name", "usage"], &["upstream_message", "observed_conversation_id", "done_event_name", "usage"])?;
    if !protocol.complete() { return Err(invalid_evidence()); }
    let message = protocol.message().ok_or_else(invalid_evidence)?;
    if &claims["upstream_message"] != message
        || !optional_text(&claims["observed_conversation_id"], protocol.observed_conversation())
        || !optional_text(&claims["done_event_name"], protocol.done_name()) {
        return Err(invalid_evidence());
    }
    let usage = protocol.normalized_usage()?;
    match (usage.as_ref(), &claims["usage"]) {
        (None, Value::Null) => Ok(()),
        (Some(original), claimed) if original == claimed => Ok(()),
        _ => Err(invalid_evidence()),
    }
}

fn optional_text(value: &Value, original: Option<&str>) -> bool {
    match original { Some(original) => value.as_str() == Some(original), None => value.is_null() }
}

pub(super) fn invalid_evidence() -> proto::WireError {
    proto::WireError::with_code("CatPaw 缺少本次宿主完整无工具终态证据，或插件材料与原流不一致，未完成本轮", "catpaw_turn_settlement_evidence_invalid")
}
pub(super) fn unconfirmed() -> proto::WireError {
    proto::WireError::with_code("CatPaw 本次完成受理、同源状态提交或交付未确认，未自动重发或补另一终态", "catpaw_turn_settlement_unconfirmed")
}
