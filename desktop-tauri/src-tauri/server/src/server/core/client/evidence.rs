//! 只采集客户端身份相关请求头，不保存认证、Cookie 或请求正文。

use std::collections::BTreeMap;

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::catalog::{IDENTITY_HEADERS, MAX_HEADER_BYTES, MAX_UA_BYTES};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientEvidence {
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub user_agent_truncated: bool,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub truncated_headers: Vec<String>,
    #[serde(default)]
    pub invalid_headers: Vec<String>,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub rule_id: String,
    #[serde(default)]
    pub matched_header: String,
    #[serde(default)]
    pub matched_pattern: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_name: String,
    #[serde(default)]
    pub candidate: String,
    #[serde(default)]
    pub declaration_status: String,
}

pub fn bounded(text: &str, max: usize) -> (String, bool) {
    let mut out = String::with_capacity(text.len().min(max));
    for ch in text.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        if out.len() + ch.len_utf8() > max { return (out, true); }
        out.push(ch);
    }
    (out, false)
}

impl ClientEvidence {
    pub fn capture(headers: &HeaderMap) -> Self {
        let mut evidence = Self::default();
        for name in std::iter::once("user-agent").chain(IDENTITY_HEADERS.iter().copied()) {
            let mut values = headers.get_all(name).iter();
            let Some(value) = values.next() else { continue; };
            let text = match value.to_str() {
                Ok(text) if values.next().is_none() && !text.chars().any(|ch| ch.is_control() && ch != '\t') => text,
                _ => { evidence.invalid_headers.push(name.into()); continue; }
            };
            let (text, truncated) = bounded(text, if name == "user-agent" { MAX_UA_BYTES } else { MAX_HEADER_BYTES });
            if name == "user-agent" {
                evidence.user_agent = text;
                evidence.user_agent_truncated = truncated;
            } else {
                evidence.headers.insert(name.into(), text);
                if truncated { evidence.truncated_headers.push(name.into()); }
            }
        }
        evidence
    }

    pub fn value(&self, header: &str) -> &str {
        if header == "user-agent" { &self.user_agent }
        else { self.headers.get(header).map(String::as_str).unwrap_or("") }
    }

    pub fn truncated(&self, header: &str) -> bool {
        if header == "user-agent" { self.user_agent_truncated }
        else { self.truncated_headers.iter().any(|name| name == header) }
    }

    pub fn normalized(mut self) -> Self {
        let (ua, truncated) = bounded(&self.user_agent, MAX_UA_BYTES);
        self.user_agent = ua;
        self.user_agent_truncated |= truncated;
        self.headers.retain(|name, _| IDENTITY_HEADERS.contains(&name.as_str()));
        for (name, text) in &mut self.headers {
            let (value, truncated) = bounded(text, MAX_HEADER_BYTES);
            *text = value;
            if truncated && !self.truncated_headers.contains(name) { self.truncated_headers.push(name.clone()); }
        }
        self.truncated_headers.retain(|name| IDENTITY_HEADERS.contains(&name.as_str()));
        self.invalid_headers.retain(|name| name == "user-agent" || IDENTITY_HEADERS.contains(&name.as_str()));
        self
    }

    pub fn encoded(&self) -> String {
        if self.method.is_empty() { return "{}".into(); }
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}
