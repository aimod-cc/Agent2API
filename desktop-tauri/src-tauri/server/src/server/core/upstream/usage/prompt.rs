use super::RequestTelemetry;
use crate::server::core::prompt::rules::Request;

impl RequestTelemetry {
    /// 入站模型与识别结果只采一次，自动续写和路由重试沿用原始条件。
    pub(crate) fn capture_prompt_request(&self, source: &crate::server::request_stats::RequestSource, model: &str) {
        let mut slot = self.prompt_request.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() { return; }
        let client = source.evidence.as_ref().map(|evidence| evidence.client_id.as_str())
            .filter(|client| !client.is_empty()).unwrap_or("other");
        *slot = Some(Request { client: client.into(), model: model.into() });
    }

    pub(crate) fn prompt_request(&self) -> Option<Request> {
        self.prompt_request.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}
