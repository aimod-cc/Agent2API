//! 同步消费读预约；等待端丢失只关闭原记录，不重读、不重新创建任务。
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::sync::oneshot;

use super::{error, json_text, registry::{Record, Registry}};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

pub(super) const MAX_CHUNK: usize = 64 * 1024;
pub(super) const MAX_READS: u64 = 16_384;

pub(super) struct Command {
    pub(super) sequence: u64,
    pub(super) delivery: oneshot::Sender<Result<Value, proto::WireError>>,
}

pub(in crate::server::core::plugin_host::wasm) struct ReadWait {
    receiver: oneshot::Receiver<Result<Value, proto::WireError>>,
    record: Arc<Record>,
    sequence: u64,
    completed: bool,
}

impl ReadWait {
    pub(in crate::server::core::plugin_host::wasm) async fn wait(mut self) -> Result<Value, proto::WireError> {
        let ended = self.record.ended_token();
        let call = self.record.source.ended_token();
        let result = tokio::select! {
            biased;
            result = &mut self.receiver => result.map_err(|_| error::delivery()),
            _ = ended.cancelled() => Err(error::ended()),
            _ = call.cancelled() => Err(error::ended()),
            _ = tokio::time::sleep_until(self.record.source.deadline()) => Err(error::ended()),
        };
        let value = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) | Err(error) => return Err(error),
        };
        // send(Ok)不证明交付；仍需原调用门和Record同一短锁确认预约。
        let accepted = self.record.source.with_submission_gate(|_| {
            self.record.accept_read_delivery(self.sequence, &value)
        });
        if accepted.is_err() {
            return Err(error::delivery());
        }
        self.completed = true;
        Ok(value)
    }
}

impl Drop for ReadWait {
    fn drop(&mut self) {
        if !self.completed {
            self.record.lose_read(self.sequence);
        }
    }
}

pub(super) fn read(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<ReadWait, proto::WireError> {
    json_text::fields(&params, &["handle", "receipt", "sequence"], &["handle", "receipt", "sequence"])?;
    let handle = params["handle"].as_str().filter(|s| json_text::identifier(s, 256)).ok_or_else(error::invalid)?;
    let receipt = params["receipt"].as_str().filter(|s| json_text::identifier(s, 256)).ok_or_else(error::invalid)?;
    let sequence = params["sequence"].as_u64().ok_or_else(error::invalid)?;
    let record = registry.get(source, handle)?;
    let (delivery, receiver) = oneshot::channel();
    // Future尚未建立/poll即永久占用序号；失败后不得再次领取这一笔。
    source.with_submission_gate(|_| record.reserve_read(receipt, sequence, delivery))?;
    Ok(ReadWait { receiver, record, sequence, completed: false })
}
