use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUpdateTask {
    pub id: String,
    pub version: String,
    pub phase: String,
    pub started_at: i64,
    pub updated_at: i64,
    pub error: Option<String>,
}

impl ServiceUpdateTask {
    pub fn active(&self) -> bool {
        matches!(self.phase.as_str(), "downloading" | "verifying" | "installing" | "restarting")
    }

    pub(super) fn set_phase(&mut self, phase: &str, error: Option<String>) {
        self.phase = phase.to_string();
        self.error = error;
        self.updated_at = crate::server::logging::now_ms();
    }
}
