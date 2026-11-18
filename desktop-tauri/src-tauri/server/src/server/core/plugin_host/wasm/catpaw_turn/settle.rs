//! 同步永久消费本次结算预约；JSON只作插件主张，不能签发宿主完成证据。
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::sync::oneshot;

use super::{error, json_text, registry::{Record, Registry}, settlement};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

pub(super) struct Command {
    pub(super) sequence: u64,
    pub(super) claims_json: String,
    pub(super) delivery: oneshot::Sender<Result<Value, proto::WireError>>,
}

pub(in crate::server::core::plugin_host::wasm) struct SettlementWait {
    receiver: oneshot::Receiver<Result<Value, proto::WireError>>,
    record: Arc<Record>,
    completed: bool,
}

impl SettlementWait {
    pub(in crate::server::core::plugin_host::wasm) async fn wait(mut self) -> Result<Value, proto::WireError> {
        let ended = self.record.ended_token();
        let call = self.record.source.ended_token();
        let result = tokio::select! {
            biased;
            result = &mut self.receiver => result.map_err(|_| settlement::unconfirmed()),
            _ = ended.cancelled() => Err(error::ended()),
            _ = call.cancelled() => Err(error::ended()),
            _ = tokio::time::sleep_until(self.record.source.deadline()) => Err(error::ended()),
        };
        let value = match result {
            Ok(Ok(value)) => value,
            Ok(Err(failure)) | Err(failure) => return Err(failure),
        };
        // 本次结果先交付，不等原worker退场。上层丢失不会重发完成或撤回已受理事实。
        self.record.source.with_submission_gate(|_| self.record.accept_settlement_delivery(&value))?;
        self.completed = true;
        Ok(value)
    }
}
impl Drop for SettlementWait {
    fn drop(&mut self) {
        if !self.completed { self.record.lose_settlement(); }
    }
}

pub(super) fn settle(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<SettlementWait, proto::WireError> {
    json_text::fields(&params, &["handle", "receipt", "session_revision", "read_sequence", "parsed_json"], &["handle", "receipt", "session_revision", "read_sequence", "parsed_json"])?;
    json_text::shape(&params, 4 * 1024 * 1024)?;
    let handle = params["handle"].as_str().filter(|s| json_text::identifier(s, 256)).ok_or_else(error::invalid)?;
    let receipt = params["receipt"].as_str().filter(|s| json_text::identifier(s, 256)).ok_or_else(error::invalid)?;
    let revision = params["session_revision"].as_u64().ok_or_else(error::invalid)?;
    let sequence = params["read_sequence"].as_u64().ok_or_else(error::invalid)?;
    let parsed = params["parsed_json"].as_str().filter(|s| !s.is_empty() && s.len() <= 2 * 1024 * 1024).ok_or_else(error::invalid)?;
    let record = registry.get(source, handle)?;
    let claims_json = parsed.to_string();
    let (delivery, receiver) = oneshot::channel();
    // 无await即消费预约；重复键/内容主张随后由唯一worker严检，坏主张不归还机会。
    source.with_submission_gate(|_| record.reserve_settlement(receipt, revision, Command { sequence, claims_json, delivery }))?;
    Ok(SettlementWait { receiver, record, completed: false })
}
