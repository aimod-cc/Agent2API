//! 统计归类与请求行为识别分离；自定义规则只改变日志和报表。

pub mod catalog;
mod evidence;
mod matcher;

pub use catalog::{attribution_client, ClientRule, ClientSettings, ProviderRule};
pub use evidence::ClientEvidence;
pub use matcher::{identify, rule_matches};

pub const CLIENT_OTHER: &str = "other";

/// Codex 的严格行为归类值；报表显示规则与来源声明不参与协议兼容判断。
pub const CLIENT_CODEX: &str = "Codex";

/// 行为判断保留原来的严格识别，不读取显示规则或调用方声明。
pub fn classify(ua: &str) -> String {
    let ua = ua.trim();
    if ua.is_empty() { return String::new(); }
    if ua.starts_with("Codex Desktop/") || ua.starts_with("codex_vscode/") || ua.starts_with("codex-tui/") {
        return CLIENT_CODEX.into();
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
    // 记账归属与归属规则存的是客户端 id（如 zcode），历史行可能是 value（如
    // ZCode），两种形态都要换算成显示名
    if let Some(client) = catalog::CLIENTS.iter().find(|client| client.id == value || client.value == value) {
        return client.name.into();
    }
    if !recorded.is_empty() { return recorded.into(); }
    value.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_accepts_client_id_and_value() {
        // 归属改写后的行存 id，识别依据里记录的是显示名，界面取到的都是名称
        assert_eq!(display_name("zcode", "Codex"), "ZCode");
        assert_eq!(display_name("Codex", ""), "Codex");
        assert_eq!(display_name("custom:not-registered", "记录名"), "记录名");
    }
}
