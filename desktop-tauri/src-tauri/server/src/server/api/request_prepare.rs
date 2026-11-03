use std::sync::Arc;

use serde_json::Value;

use crate::server::core::blocking;
use crate::server::core::key_scope::KeyScope;
use crate::server::errors::GatewayError;
use crate::server::request_stats::{RequestSource, RequestStats};
use crate::server::ServerState;

pub(super) async fn resolve_model(
    state: &ServerState,
    payload: &mut Value,
    scope: Option<&KeyScope>,
) -> Result<String, GatewayError> {
    let state = state.clone();
    let scope = scope.cloned();
    let mut body = std::mem::take(payload);
    let (body, result) = blocking::run(move || {
        let result = super::pipeline::resolve_model(&state, &mut body, scope.as_ref());
        (body, result)
    }).await?;
    *payload = body;
    result
}

pub(super) async fn record_started(
    stats: Arc<RequestStats>,
    id: &str,
    ts: i64,
    model: &str,
    client_model: &str,
    reasoning: &str,
    source: &RequestSource,
    is_test: bool,
) {
    // 进行中行写入与调用方创建断线守卫之间不能留下可取消的等待窗口。
    blocking::in_place(|| {
        stats.record_started(id, ts, model, client_model, reasoning, source, is_test);
    });
}
