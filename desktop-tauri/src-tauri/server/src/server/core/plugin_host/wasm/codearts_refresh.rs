//! CodeArts固定换证由原调用、原Ready账号行和冻结出口共同约束；未知结果不重发。

mod call;
mod protocol;
mod worker;
pub(super) use call::Call;
pub(super) use worker::{close_for_host, release_ready, wait_for_host};

pub(super) async fn exchange(call: std::sync::Arc<Call>) -> Result<serde_json::Value, agent2api_provider_protocol::WireError> {
    worker::exchange(call).await
}

