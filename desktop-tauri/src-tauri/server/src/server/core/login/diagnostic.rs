use std::collections::BTreeMap;

use serde::Serialize;

use super::LoginTaskHandle;
use crate::server::errors::GatewayError;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginDiagnostic {
    pub stage: String,
    pub method: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub truncated: bool,
    pub redacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_note: Option<String>,
}

pub(crate) fn fail_with_error(handle: &LoginTaskHandle, error: &GatewayError) {
    handle.update(|task| {
        if task.canceled || task.done {
            return;
        }
        super::mark_task_error(task, &error.message);
        task.diagnostic = error.login_diagnostic.clone();
    });
}
