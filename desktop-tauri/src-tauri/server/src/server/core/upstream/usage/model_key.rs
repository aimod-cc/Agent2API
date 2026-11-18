use std::sync::Arc;
use crate::server::config::model_key_source::Source;

impl super::RequestTelemetry {
    pub(crate) fn new_with_model_key(source: Option<Arc<Source>>) -> Self {
        let mut telemetry = Self::new();
        telemetry.model_key = source;
        telemetry
    }

    pub(crate) fn with_model_key(source: Option<Arc<Source>>) -> Self {
        let telemetry = Self::new_with_model_key(source);
        telemetry.ensure_id(&super::super::request::new_request_id());
        telemetry
    }

    pub(crate) fn model_key_source(&self) -> Option<&Arc<Source>> {
        self.model_key.as_ref()
    }
}
