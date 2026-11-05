use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinClient {
    pub id: &'static str,
    pub name: &'static str,
    pub value: &'static str,
    pub products: &'static [&'static str],
}

pub const CLIENTS: &[BuiltinClient] = &[
    BuiltinClient { id: "claude-code", name: "Claude Code", value: "Claude Code", products: &["claude-cli", "claude-code"] },
    BuiltinClient { id: "codex", name: "Codex", value: "Codex", products: &["codex desktop", "codex_vscode", "codex-tui", "codex_cli_rs", "codex-cli", "codex"] },
    BuiltinClient { id: "zcode", name: "ZCode", value: "ZCode", products: &["zcode"] },
    BuiltinClient { id: "github-copilot", name: "GitHub Copilot", value: "GitHub Copilot", products: &["githubcopilotchat", "github-copilot", "copilot"] },
    BuiltinClient { id: "cherry-studio", name: "Cherry Studio", value: "CherryStudio", products: &["cherrystudio", "cherry-studio"] },
    BuiltinClient { id: "dsh", name: "DeepSeek Harness", value: "deepseek-harness", products: &["deepseek-harness"] },
    BuiltinClient { id: "magpie", name: "Magpie", value: "Magpie", products: &["magpie", "magpie-router", "magpie-vision", "magpie-search", "magpie-image"] },
    BuiltinClient { id: "hermes", name: "Hermes Agent", value: "Hermes Agent", products: &["hermes", "hermesagent", "hermes-agent"] },
    BuiltinClient { id: "hindsight", name: "Hindsight", value: "Hindsight", products: &["hindsight"] },
    BuiltinClient { id: "cline", name: "Cline", value: "Cline", products: &["cline", "cline-cli"] },
    BuiltinClient { id: "cursor", name: "Cursor", value: "Cursor", products: &["cursor", "cursor-agent"] },
    BuiltinClient { id: "opencode", name: "OpenCode", value: "OpenCode", products: &["opencode"] },
    BuiltinClient { id: "workbuddy", name: "WorkBuddy", value: "WorkBuddy", products: &["workbuddy", "workbuddy-ai"] },
    BuiltinClient { id: "codebuddy", name: "CodeBuddy", value: "CodeBuddy", products: &["codebuddy", "codebuddy-code"] },
    BuiltinClient { id: "kimi-code", name: "Kimi Code", value: "Kimi Code", products: &["kimicli", "kimi-code-cli", "kimi-cli", "kimi-code"] },
    BuiltinClient { id: "zed", name: "Zed", value: "Zed", products: &["zed"] },
    BuiltinClient { id: "crush", name: "Crush", value: "Crush", products: &["crush"] },
    BuiltinClient { id: "qwen-code", name: "Qwen Code", value: "Qwen Code", products: &["qwencode", "qwen-code"] },
    BuiltinClient { id: "factory-droid", name: "Factory Droid", value: "Factory Droid", products: &["factory-cli"] },
    BuiltinClient { id: "goose", name: "Goose", value: "Goose", products: &["goose"] },
];

pub const DECLARATION_HEADER: &str = "x-agent2api-client";
pub const IDENTITY_HEADERS: &[&str] = &[
    DECLARATION_HEADER, "x-title", "x-client-type", "x-cerebras-3rd-party-integration",
];
pub const MAX_UA_BYTES: usize = 1024;
pub const MAX_HEADER_BYTES: usize = 256;
pub const MAX_RULES: usize = 200;
pub const MAX_CLIENTS: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomClient {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType { Product, Prefix, Contains, Exact }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientRule {
    pub id: String,
    pub enabled: bool,
    pub header: String,
    pub match_type: MatchType,
    pub pattern: String,
    pub client_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientSettings {
    pub accept_declarations: bool,
    pub clients: Vec<CustomClient>,
    pub rules: Vec<ClientRule>,
}

pub fn builtin(id: &str) -> Option<&'static BuiltinClient> {
    CLIENTS.iter().find(|client| client.id == id)
}

pub fn target<'a>(settings: &'a ClientSettings, id: &str) -> Option<(&'a str, &'a str)> {
    if let Some(client) = builtin(id) { return Some((client.value, client.name)); }
    settings.clients.iter().find(|client| client.id == id).map(|client| (client.id.as_str(), client.name.as_str()))
}

pub fn declared_client<'a>(settings: &'a ClientSettings, name: &str) -> Option<&'a str> {
    let name = name.trim();
    if let Some(client) = CLIENTS.iter().find(|client|
        client.id.eq_ignore_ascii_case(name) || client.name.eq_ignore_ascii_case(name)
            || client.value.eq_ignore_ascii_case(name)
            || client.products.iter().any(|product| product.eq_ignore_ascii_case(name))) {
        return Some(client.id);
    }
    settings.clients.iter().find(|client| client.id.eq_ignore_ascii_case(name)
        || client.name.eq_ignore_ascii_case(name)).map(|client| client.id.as_str())
}

pub fn validate(settings: &ClientSettings) -> Result<(), String> {
    use std::collections::HashSet;
    if settings.rules.len() > MAX_RULES || settings.clients.len() > MAX_CLIENTS {
        return Err(format!("客户端识别最多允许 {MAX_RULES} 条规则和 {MAX_CLIENTS} 个自定义客户端"));
    }
    let valid_text = |text: &str, max: usize| !text.is_empty() && text.trim() == text
        && text.chars().count() <= max && !text.chars().any(char::is_control);
    let valid_id = |id: &str| !id.is_empty() && id.len() <= 80
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'));
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let empty = ClientSettings::default();
    for client in &settings.clients {
        if !client.id.starts_with("custom:") || client.id.len() < 12 || !valid_id(&client.id)
            || !ids.insert(client.id.as_str()) {
            return Err("自定义客户端标识无效或重复".into());
        }
        if !valid_text(&client.name, 80) || !names.insert(client.name.to_lowercase())
            || declared_client(&empty, &client.name).is_some()
            || matches!(client.name.to_ascii_lowercase().as_str(), "other" | "unknown" | "assistant" | "model_test" | "assistant_images" | "assistant_title" | "checkin_keepalive") {
            return Err("客户端名称不能为空、重复或占用内置名称，最长 80 个字符".into());
        }
    }
    ids.clear();
    for rule in &settings.rules {
        if !valid_id(&rule.id) || !ids.insert(rule.id.as_str()) {
            return Err("识别规则标识无效或重复".into());
        }
        if rule.header != "user-agent" && !IDENTITY_HEADERS.contains(&rule.header.as_str()) {
            return Err("识别规则只能读取软件标识请求头，不能读取认证等敏感请求头".into());
        }
        if !valid_text(&rule.pattern, MAX_HEADER_BYTES) {
            return Err("匹配内容不能为空或含控制字符，最长 256 个字符".into());
        }
        if rule.match_type == MatchType::Product && (rule.header != "user-agent"
            || !rule.pattern.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b' '))) {
            return Err("按产品名匹配只适用于 User-Agent，产品名不能包含版本号分隔符".into());
        }
        if target(settings, &rule.client_id).is_none() {
            return Err("识别规则指向的客户端不存在".into());
        }
    }
    Ok(())
}
