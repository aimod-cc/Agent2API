//! close只归还本次原来源句柄；没有调用者可选的远端状态或任务ID。
//! cleanup:confirmed仅指本次停止及终态明确受理，不宣称远端资源已销毁。
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{error, json_text, registry::{Record, Registry}};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

pub(in crate::server::core::plugin_host::wasm) struct CloseWait {
    record: Arc<Record>,
}

impl CloseWait {
    pub(in crate::server::core::plugin_host::wasm) async fn wait(self) -> Result<Value, proto::WireError> {
        // 此等待没有取消已发清理的权力。调用者丢弃本future，原worker仍完整执行。
        self.record.worker_settled().await;
        self.record.close_response()
    }
}

pub(super) fn close(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<CloseWait, proto::WireError> {
    json_text::fields(&params, &["handle"], &["handle"])?;
    let handle = params["handle"].as_str().filter(|s| json_text::identifier(s, 256)).ok_or_else(error::invalid)?;
    // current可能已结束；唯有同一个SourceRef对象仍可释放原责任，不能凭ID换来源。
    let record = registry.get_for_close(source, handle)?;
    record.release_call();
    Ok(CloseWait { record })
}
