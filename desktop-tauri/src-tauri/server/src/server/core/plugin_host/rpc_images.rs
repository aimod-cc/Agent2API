use std::sync::Arc;
use std::time::Duration;
use axum::http::HeaderMap;
use serde_json::{json, Value};

use super::{headers_json, rpc_error, wire_proxy, RpcAdapter};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::images::ImageOperation;
use crate::server::core::upstream::{usage::RequestTelemetry, ForwardOutcome};
use crate::server::errors::GatewayError;

impl RpcAdapter {
    pub(super) async fn wasm_images(
        &self, account: &str, body: &Value, headers: &HeaderMap, proxy: Option<ResolvedProxy>,
        operation: ImageOperation, telemetry: &Arc<RequestTelemetry>,
    ) -> Result<ForwardOutcome, GatewayError> {
        super::super::wasm::catalog::ensure(self, account, body["model"].as_str().unwrap_or("")).await?;
        let execution = self.execution().await?;
        let host = execution.host.clone();
        let options = host.options.wasm.as_ref().ok_or_else(|| GatewayError::with_status(501, "非 Wasm 图片通道"))?;
        if !options.allows("images.v1") { return Err(GatewayError::with_status(501, "插件未声明标准图片能力")); }
        host.ensure().await.map_err(|message| GatewayError::with_status(503, message))?;
        let mut params = json!({ "provider": self.id, "account": account, "body": body,
            "headers": headers_json(headers), "proxy": proxy.map(wire_proxy).transpose()?.unwrap_or(Value::Null),
            "session": crate::server::core::upstream::plugin_session_id(headers, body) });
        if let Some(trace) = super::super::script::register_trace(telemetry) { params["plugin_trace"] = json!(trace); }
        let token = telemetry.cancel_token();
        let cancelled = async { match token { Some(token) => token.cancelled().await, None => std::future::pending().await } };
        let result = tokio::select! {
            biased;
            _ = cancelled => return Err(crate::server::core::upstream::cancellation::cancelled_error()),
            result = host.call_wasm(operation.method(), params, Duration::from_secs(3600)) => result,
        };
        let body = match result {
            Ok(body) => body,
            Err(error) => {
                let mut failure = rpc_error(self.id, error, operation.method());
                if failure.status_code >= 500 {
                    failure.code = Some("image_output_uncertain".into());
                    telemetry.note_attempt_notice("生图结果尚未确认，未自动重发，以免重复消耗额度");
                }
                return Err(failure);
            }
        };
        if let Some(usage) = body.get("usage").filter(|usage| usage.is_object()) { telemetry.report_usage(usage); }
        Ok(ForwardOutcome::Completion { body })
    }
}
