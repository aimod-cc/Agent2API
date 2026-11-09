use std::collections::{BTreeSet, VecDeque};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::network::{self, Failure};

pub(super) struct Evidence {
    pub entry_present: bool,
    pub provider: bool,
    pub providers: Vec<String>,
}

fn declared(value: &Value) -> bool {
    match value {
        Value::String(value) => !value.trim().is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        _ => false,
    }
}

pub(super) fn excluded(manifest: &Value) -> bool {
    if declared(&manifest["magpie"]["providers"]) || declared(&manifest["magpie"]["provider"])
        || manifest["magpie"]["kind"] == "provider" { return false; }
    declared(&manifest["pi"]) || ["agent", "middleware", "hooks", "page", "tools"].iter()
        .any(|key| declared(&manifest["magpie"][*key]))
        || manifest["magpie"]["kind"].as_str().is_some_and(|kind| !kind.is_empty() && kind != "provider")
}

#[derive(Clone)]
enum Token {
    Word(String),
    Text(String),
    Mark(u8),
}

impl Token {
    fn name(&self) -> Option<&str> {
        match self { Self::Word(value) | Self::Text(value) => Some(value), _ => None }
    }
    fn mark(&self, value: u8) -> bool { matches!(self, Self::Mark(found) if *found == value) }
}

fn tokens(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() { at += 1; continue; }
        if bytes.get(at..at + 2) == Some(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' { at += 1; }
            continue;
        }
        if bytes.get(at..at + 2) == Some(b"/*") {
            at += 2;
            while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" { at += 1; }
            at = (at + 2).min(bytes.len());
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            let start = at + 1;
            at += 1;
            while at < bytes.len() && bytes[at] != byte {
                if bytes[at] == b'\\' { at += 1; }
                at += 1;
            }
            let end = at.min(bytes.len());
            out.push(Token::Text(String::from_utf8_lossy(&bytes[start..end]).into_owned()));
            at = (at + 1).min(bytes.len());
            continue;
        }
        if byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') {
            let start = at;
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'_' | b'$')) { at += 1; }
            out.push(Token::Word(source[start..at].to_string()));
        } else {
            out.push(Token::Mark(byte));
            at += 1;
        }
    }
    out
}

fn auth_provider(source: &str) -> Option<Vec<String>> {
    let tokens = tokens(source);
    if !tokens.iter().any(|token| token.name() == Some("export") || token.name() == Some("exports")) { return None; }
    let mut found = false;
    let mut providers = BTreeSet::new();
    for at in 0..tokens.len().saturating_sub(3) {
        if tokens[at].name() != Some("auth") || !(tokens[at + 1].mark(b':') || tokens[at + 1].mark(b'='))
            || !tokens[at + 2].mark(b'{') { continue; }
        let mut depth = 1;
        let mut provider = false;
        let mut capability = false;
        for index in at + 3..tokens.len() {
            if tokens[index].mark(b'{') { depth += 1; }
            if tokens[index].mark(b'}') { depth -= 1; if depth == 0 { break; } }
            if depth != 1 { continue; }
            if tokens[index].name() == Some("provider") && tokens.get(index + 1).is_some_and(|token| token.mark(b':')) {
                provider = true;
                if let Some(Token::Text(id)) = tokens.get(index + 2) {
                    if !id.is_empty() && id.len() <= 128 && id.bytes().all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch)) {
                        providers.insert(id.clone());
                    }
                }
            }
            if matches!(tokens[index].name(), Some("methods" | "loader"))
                && tokens.get(index + 1).is_some_and(|token| token.mark(b':') || token.mark(b'(')) { capability = true; }
        }
        found |= provider && capability;
    }
    found.then(|| providers.into_iter().collect())
}

fn imports(source: &str) -> Vec<String> {
    static IMPORTS: OnceLock<Regex> = OnceLock::new();
    let expression = IMPORTS.get_or_init(|| Regex::new(
        r#"(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)["'](\.{1,2}/[^"'\r\n]+)["']"#,
    ).expect("固定入口导入表达式"));
    expression.captures_iter(source).take(32).map(|item| item[1].to_string()).collect()
}

fn relative(parent: &str, import: &str) -> Option<String> {
    let mut segments: Vec<_> = parent.rsplit_once('/').map(|(directory, _)| directory.split('/').collect()).unwrap_or_default();
    for part in import.split('/') {
        match part {
            "." | "" => {},
            ".." => { segments.pop()?; },
            value if value.contains(['\\', '?', '#', ':', '%']) => return None,
            value => segments.push(value),
        }
    }
    Some(segments.join("/"))
}

// 只读取入口及有限的相对导入，不为插件发现执行任何第三方代码。
pub(super) async fn inspect(repository: &str, reference: &str, manifest: &Value) -> Result<Evidence, Failure> {
    let entries = super::super::dependencies::entries(manifest);
    let mut pending: VecDeque<_> = entries.into_iter().map(|path| (path.trim_start_matches("./").to_string(), true, 0)).collect();
    pending.extend(["src/index.ts", "src/index.js", "src/index.mjs", "src/plugin.ts"].map(|path| (path.to_string(), false, 0)));
    let mut seen = BTreeSet::new();
    let mut evidence = Evidence { entry_present: false, provider: false, providers: Vec::new() };
    let mut downloaded = 0;
    while let Some((path, entry, depth)) = pending.pop_front() {
        if seen.len() >= 16 || downloaded >= 6 * 1024 * 1024 { break; }
        if !seen.insert(path.clone()) { continue; }
        if ![".ts", ".tsx", ".js", ".mjs", ".cjs"].iter().any(|suffix| path.ends_with(suffix)) { continue; }
        let url = super::github::raw_url(repository, reference, &path)?;
        let Some(bytes) = network::github_bytes(&url, 2 * 1024 * 1024).await? else { continue; };
        downloaded += bytes.len();
        evidence.entry_present |= entry;
        let Ok(source) = std::str::from_utf8(&bytes) else { continue; };
        if let Some(providers) = auth_provider(source) {
            evidence.provider = true;
            evidence.providers = providers;
            return Ok(evidence);
        }
        if depth >= 3 { continue; }
        for import in imports(source).into_iter().rev() {
            let Some(path) = relative(&path, &import) else { continue; };
            if path.rsplit('/').next().is_some_and(|file| file.contains('.')) {
                pending.push_front((path, false, depth + 1));
            } else {
                for suffix in [".ts", ".js", ".mjs", "/index.ts", "/index.js"].into_iter().rev() {
                    pending.push_front((format!("{path}{suffix}"), false, depth + 1));
                }
            }
        }
    }
    Ok(evidence)
}
