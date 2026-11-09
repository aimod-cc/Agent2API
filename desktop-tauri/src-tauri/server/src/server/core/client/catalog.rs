use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinClient {
    pub id: &'static str,
    pub name: &'static str,
    pub value: &'static str,
    pub products: &'static [&'static str],
    /// 该客户端一份**代表性**的 User-Agent 取值（账号级自定义 UA 的预置项）。
    ///
    /// 取值优先用真实抓包（Claude Code / Codex / ZCode / Copilot / Cursor /
    /// OpenCode / Kimi Code / DeepSeek Harness / Magpie / Hermes / Cherry
    /// Studio / WorkBuddy），拿不到实测的少数几家退化成产品标识 + `1.0`。
    /// 它只是「一键填入」的起点，用户可以在输入框里改成任意版本号 ——
    /// 上游的 UA 白名单通常只校验产品名，不校验版本。
    pub ua: &'static str,
}

/// 顺序即界面顺序：Claude Code / Codex / ZCode 是绝大多数用户要冒充的前三个，
/// 固定排在最前；其余按名字排，新增一家插到尾部即可。
pub const CLIENTS: &[BuiltinClient] = &[
    BuiltinClient { id: "claude-code", name: "Claude Code", value: "Claude Code", products: &["claude-cli", "claude-code"], ua: "claude-cli/2.1.296 (external, cli)" },
    BuiltinClient { id: "codex", name: "Codex", value: "Codex", products: &["codex desktop", "codex_vscode", "codex-tui", "codex_cli_rs", "codex-cli", "codex"], ua: "codex_cli_rs/0.160.1 (Windows 10.0.26100; x86_64)" },
    BuiltinClient { id: "zcode", name: "ZCode", value: "ZCode", products: &["zcode"], ua: "ZCode/3.10.1" },
    BuiltinClient { id: "github-copilot", name: "GitHub Copilot", value: "GitHub Copilot", products: &["githubcopilotchat", "github-copilot", "copilot"], ua: "GitHubCopilotChat/0.68.0" },
    BuiltinClient { id: "cherry-studio", name: "Cherry Studio", value: "CherryStudio", products: &["cherrystudio", "cherry-studio"], ua: "CherryStudio/1.5" },
    BuiltinClient { id: "dsh", name: "DeepSeek Harness", value: "deepseek-harness", products: &["deepseek-harness"], ua: "deepseek-harness/0.3.1" },
    BuiltinClient { id: "magpie", name: "Magpie", value: "Magpie", products: &["magpie", "magpie-router", "magpie-vision", "magpie-search", "magpie-image"], ua: "magpie/0.1.983" },
    BuiltinClient { id: "hermes", name: "Hermes Agent", value: "Hermes Agent", products: &["hermes", "hermesagent", "hermes-agent"], ua: "hermes-agent/0.12" },
    BuiltinClient { id: "hindsight", name: "Hindsight", value: "Hindsight", products: &["hindsight"], ua: "hindsight/1.0" },
    BuiltinClient { id: "cline", name: "Cline", value: "Cline", products: &["cline", "cline-cli"], ua: "Cline/1.0" },
    BuiltinClient { id: "cursor", name: "Cursor", value: "Cursor", products: &["cursor", "cursor-agent"], ua: "Cursor/3.23.12" },
    BuiltinClient { id: "opencode", name: "OpenCode", value: "OpenCode", products: &["opencode"], ua: "opencode/1.4.0" },
    BuiltinClient { id: "workbuddy", name: "WorkBuddy", value: "WorkBuddy", products: &["workbuddy", "workbuddy-ai"], ua: "WorkBuddy/5.9.2 WorkBuddy/5.9.2 CLI/2.165.0" },
    BuiltinClient { id: "codebuddy", name: "CodeBuddy", value: "CodeBuddy", products: &["codebuddy", "codebuddy-code"], ua: "CodeBuddy/1.0" },
    BuiltinClient { id: "kimi-code", name: "Kimi Code", value: "Kimi Code", products: &["kimicli", "kimi-code-cli", "kimi-cli", "kimi-code"], ua: "kimi-code-cli/2.4.1" },
    BuiltinClient { id: "zed", name: "Zed", value: "Zed", products: &["zed"], ua: "Zed/1.0" },
    BuiltinClient { id: "crush", name: "Crush", value: "Crush", products: &["crush"], ua: "crush/0.85.0" },
    BuiltinClient { id: "qwen-code", name: "Qwen Code", value: "Qwen Code", products: &["qwencode", "qwen-code"], ua: "QwenCode/0.5.0" },
    BuiltinClient { id: "factory-droid", name: "Factory Droid", value: "Factory Droid", products: &["factory-cli"], ua: "factory-cli/1.0" },
    BuiltinClient { id: "goose", name: "Goose", value: "Goose", products: &["goose"], ua: "goose/1.0" },
];

pub const DECLARATION_HEADER: &str = "x-agent2api-client";
pub const IDENTITY_HEADERS: &[&str] = &[
    DECLARATION_HEADER, "x-title", "x-client-type", "x-cerebras-3rd-party-integration",
];
pub const MAX_UA_BYTES: usize = 1024;
pub const MAX_HEADER_BYTES: usize = 256;
pub const MAX_RULES: usize = 200;
pub const MAX_CLIENTS: usize = 100;

/// 账号级自定义 User-Agent 的归一与校验。
///
/// `null` 与非字符串一律按「清除」处理（返回空串），`string` 去首尾空白后
/// 校验。三条硬约束，缺一条都是真实的故障面：
///   · **不能含控制字符**：这个值原样成为出站请求头，CR/LF 就是请求头注入；
///   · **不能超过 1 KiB**：与入站 UA 的上限同一档，上游对超长头的处理各家不一，
///     超了可能在 TLS 之后才被拒，排障时看不出原因；
///   · **必须能转成合法的头值**（无孤立代理对等）：Rust 的 HeaderValue 会拒，
///     与其在发送时 500，不如在保存时明确报错。
///
/// 空串是合法输入（= 清除自定义 UA），调用方据此把字段从记录里删掉。
pub fn normalize_user_agent_override(
    value: &serde_json::Value,
) -> Result<String, crate::server::core::account_store::AccountStoreError> {
    let raw = match value {
        serde_json::Value::Null => return Ok(String::new()),
        serde_json::Value::String(text) => text,
        _ => return Err(crate::server::core::account_store::AccountStoreError::bad_request(
            "自定义 User-Agent 必须是字符串",
        )),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() { return Ok(String::new()); }
    if trimmed.chars().any(char::is_control) {
        return Err(crate::server::core::account_store::AccountStoreError::bad_request(
            "自定义 User-Agent 不能包含换行等控制字符",
        ));
    }
    if trimmed.len() > MAX_UA_BYTES {
        return Err(crate::server::core::account_store::AccountStoreError::bad_request(
            "自定义 User-Agent 不能超过 1024 字节",
        ));
    }
    if axum::http::HeaderValue::from_str(trimmed).is_err() {
        return Err(crate::server::core::account_store::AccountStoreError::bad_request(
            "自定义 User-Agent 含有不能放进请求头的字符",
        ));
    }
    Ok(trimmed.to_string())
}

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
