//! 统计归类与请求行为识别分离；自定义规则只改变日志和报表。

pub mod catalog;
mod evidence;
mod matcher;

pub use catalog::{ClientRule, ClientSettings};
pub use evidence::ClientEvidence;
pub use matcher::{identify, rule_matches};

pub const CLIENT_OTHER: &str = "other";

/// 行为判断保留原来的严格识别，不读取显示规则或调用方声明。
pub fn classify(ua: &str) -> String {
    let ua = ua.trim();
    if ua.is_empty() { return String::new(); }
    if ua.starts_with("Codex Desktop/") || ua.starts_with("codex_vscode/") || ua.starts_with("codex-tui/") {
        return "Codex".into();
    }
    if ua.starts_with("ZCode/") { return "ZCode".into(); }
    if ua.starts_with("claude-cli/") { return "Claude Code".into(); }
    if ua.starts_with("deepseek-harness/") { return "deepseek-harness".into(); }
    if ua.starts_with("GitHubCopilotChat/") { return "GitHub Copilot".into(); }
    if ua.contains("CherryStudio/") { return "CherryStudio".into(); }
    CLIENT_OTHER.into()
}

pub fn display_name(value: &str, recorded: &str) -> String {
    if value.starts_with("custom:") {
        let snapshot = crate::server::config::client_identification::current();
        if let Ok(settings) = &snapshot.settings {
            if let Some(client) = settings.clients.iter().find(|client| client.id == value) {
                return client.name.clone();
            }
        }
    }
    if let Some(client) = catalog::CLIENTS.iter().find(|client| client.value == value) {
        return client.name.into();
    }
    if !recorded.is_empty() { return recorded.into(); }
    value.into()
}
