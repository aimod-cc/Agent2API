use super::catalog::{self, ClientRule, ClientSettings, MatchType, CLIENTS, DECLARATION_HEADER};
use super::{ClientEvidence, CLIENT_OTHER};

fn product_position(ua: &str, product: &str, truncated: bool) -> Option<usize> {
    let ua = ua.to_ascii_lowercase();
    let product = product.to_ascii_lowercase();
    ua.match_indices(&product).find_map(|(start, _)| {
        let end = start + product.len();
        let before = start == 0 || ua.as_bytes()[start - 1].is_ascii_whitespace();
        let after = ua.as_bytes().get(end).is_some_and(|byte| *byte == b'/' || *byte == b'(' || byte.is_ascii_whitespace())
            || (end == ua.len() && !truncated);
        let depth = ua[..start].bytes().fold(0usize, |depth, byte| match byte {
            b'(' => depth + 1, b')' => depth.saturating_sub(1), _ => depth,
        });
        (before && after && depth == 0).then_some(start)
    })
}

pub fn rule_matches(rule: &ClientRule, evidence: &ClientEvidence) -> bool {
    if evidence.invalid_headers.contains(&rule.header) { return false; }
    let value = evidence.value(&rule.header).trim();
    if value.is_empty() { return false; }
    match rule.match_type {
        MatchType::Product => product_position(value, &rule.pattern, evidence.truncated(&rule.header)).is_some(),
        MatchType::Prefix => value.to_ascii_lowercase().starts_with(&rule.pattern.to_ascii_lowercase()),
        MatchType::Contains => value.to_ascii_lowercase().contains(&rule.pattern.to_ascii_lowercase()),
        MatchType::Exact => !evidence.truncated(&rule.header) && value.eq_ignore_ascii_case(&rule.pattern),
    }
}

fn unknown_product(evidence: &ClientEvidence) -> String {
    let ua = evidence.user_agent.trim();
    let Some((name, _)) = ua.split_once('/') else { return String::new(); };
    if name.is_empty() || name.len() > 64 || !name.bytes().all(|byte|
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')) { return String::new(); }
    let name_lower = name.to_ascii_lowercase();
    let generic = [
        "mozilla", "chrome", "safari", "electron", "applewebkit", "curl", "wget", "httpie", "postmanruntime", "insomnia",
        "node", "node-fetch", "bun", "deno", "undici", "axios", "got", "ky", "fetch", "runtime", "python", "python-httpx",
        "python-requests", "httpx", "requests", "python-urllib", "urllib3", "aiohttp", "go-http-client", "go-resty", "resty",
        "okhttp", "java", "java-http-client", "apache-httpclient", "reactor-netty", "ruby", "faraday", "rest-client",
        "guzzlehttp", "dart", "reqwest", "libcurl", "openai", "asyncopenai", "anthropic", "asyncanthropic", "ai-sdk", "google-genai-sdk",
    ];
    // 压缩构建可能把 SDK 名称改成 Rs/JS、kc/JS，不能把这些随机名字当成产品。
    if generic.contains(&name_lower.as_str()) || ua[name.len() + 1..].to_ascii_lowercase().starts_with("js ") { return String::new(); }
    name.into()
}

fn assign(evidence: &mut ClientEvidence, settings: &ClientSettings, id: &str, method: &str, header: &str, pattern: &str) -> String {
    let Some((value, name)) = catalog::target(settings, id) else { return CLIENT_OTHER.into(); };
    evidence.method = method.into();
    evidence.client_id = id.into();
    evidence.client_name = name.into();
    evidence.matched_header = header.into();
    evidence.matched_pattern = pattern.into();
    value.into()
}

pub fn identify(mut evidence: ClientEvidence, settings: &ClientSettings) -> (String, ClientEvidence) {
    evidence.method.clear();
    evidence.rule_id.clear();
    evidence.client_id.clear();
    evidence.client_name.clear();
    evidence.matched_header.clear();
    evidence.matched_pattern.clear();
    evidence.candidate.clear();
    evidence.declaration_status.clear();
    let declaration = evidence.value(DECLARATION_HEADER).trim().to_string();
    if evidence.invalid_headers.iter().any(|name| name == DECLARATION_HEADER) {
        evidence.declaration_status = "invalid".into();
    } else if !declaration.is_empty() {
        evidence.declaration_status = if !settings.accept_declarations { "disabled" }
            else if evidence.truncated(DECLARATION_HEADER) { "invalid" } else { "unregistered" }.into();
        if settings.accept_declarations && !evidence.truncated(DECLARATION_HEADER) {
            if let Some(id) = catalog::declared_client(settings, &declaration) {
                let value = assign(&mut evidence, settings, id, "declared", DECLARATION_HEADER, &declaration);
                evidence.declaration_status = "accepted".into();
                return (value, evidence);
            }
        }
    }
    for rule in &settings.rules {
        if rule.enabled && (rule.header != DECLARATION_HEADER || settings.accept_declarations) && rule_matches(rule, &evidence) {
            let value = assign(&mut evidence, settings, &rule.client_id, "custom_rule", &rule.header, &rule.pattern);
            if rule.header == DECLARATION_HEADER { evidence.declaration_status = "accepted".into(); }
            evidence.rule_id = rule.id.clone();
            return (value, evidence);
        }
    }
    let mut found: Option<(&str, &str, String)> = None;
    let mut ambiguous = false;
    let mut remember = |id: &'static str, header: &'static str, pattern: String| {
        if let Some((previous, _, _)) = &found {
            if *previous != id { ambiguous = true; }
        } else { found = Some((id, header, pattern)); }
    };
    for client in CLIENTS {
        if !evidence.invalid_headers.iter().any(|name| name == "user-agent") {
            if let Some(product) = client.products.iter().find(|product|
                product_position(evidence.user_agent.trim(), product, evidence.user_agent_truncated).is_some()) {
                remember(client.id, "user-agent", (*product).into());
            }
        }
    }
    // 仅认经过核实的产品专用值，不把通用平台/版本头当成客户端身份证明。
    for (header, value, id) in [
        ("x-title", "Cline", "cline"),
        ("x-client-type", "cline-sdk", "cline"),
        ("x-cerebras-3rd-party-integration", "cline", "cline"),
    ] {
        if !evidence.truncated(header) && !evidence.invalid_headers.iter().any(|name| name == header)
            && evidence.value(header).trim().eq_ignore_ascii_case(value) {
            remember(id, header, value.into());
        }
    }
    if ambiguous {
        evidence.method = "ambiguous".into();
        return (CLIENT_OTHER.into(), evidence);
    }
    if let Some((id, header, pattern)) = found {
        let method = if header == "user-agent" { "builtin_ua" } else { "builtin_header" };
        let value = assign(&mut evidence, settings, id, method, header, &pattern);
        return (value, evidence);
    }
    if evidence.user_agent.trim().is_empty() && evidence.headers.values().all(|value| value.trim().is_empty()) {
        evidence.method = if evidence.invalid_headers.is_empty() { "missing" } else { "invalid" }.into();
        return (if evidence.method == "missing" { String::new() } else { CLIENT_OTHER.into() }, evidence);
    }
    evidence.method = "unknown".into();
    evidence.candidate = unknown_product(&evidence);
    (CLIENT_OTHER.into(), evidence)
}
