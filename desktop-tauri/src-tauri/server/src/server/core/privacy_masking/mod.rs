//! 内存内的推理正文脱敏。只接受协议层挑出的文本；不接触鉴权、路由或持久化。

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write,
    sync::{Arc, LazyLock, Mutex},
};

use regex::Regex;
use serde::{Deserialize, Serialize};

mod custom;
mod detectors;
use detectors::Category;

pub(crate) mod stream;
pub(crate) mod wire;

const PREFIX: &str = "{{A2A_";
const SUFFIX: &str = "}}";
const TOKEN_LEN: usize = PREFIX.len() + 32 + SUFFIX.len();
const MAX_VALUES: usize = 4096;
const MAX_MATCHES: usize = 65_536;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_EXPANSION: usize = 16 * 1024 * 1024;
const MAX_MAPPED_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) secrets: bool,
    pub(crate) personal: bool,
    pub(crate) terms: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    kinds: BTreeMap<Category, bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    terms_any_case: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    terms_whole: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rules: Vec<custom::CustomRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Default)]
pub(crate) struct Rules {
    pub(crate) settings: Settings,
    terms: Vec<custom::Term>,
    custom: Vec<Regex>,
}

impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivacyRules")
            .field("enabled", &self.enabled())
            .finish_non_exhaustive()
    }
}

impl Rules {
    pub(crate) fn enabled(&self) -> bool {
        self.settings.secrets || self.settings.personal || !self.terms.is_empty()
    }

    pub(crate) fn compile(mut settings: Settings) -> Result<Self, &'static str> {
        for term in &mut settings.terms {
            *term = term.trim().to_owned();
        }
        settings.terms.retain(|s| !s.is_empty());
        settings.terms.sort();
        settings.terms.dedup();
        if settings.terms.len() > 100 || settings.terms.iter().any(|s| s.len() > 256) {
            return Err("terms 最多 100 项，每项最多 256 字节");
        }
        if !settings.kinds.keys().copied().all(Category::optional) {
            return Err("无效的个人信息分类");
        }
        if (settings.secrets || settings.personal) && !detectors::ready() {
            return Err("内置脱敏规则初始化失败");
        }
        let terms = settings
            .terms
            .iter()
            .map(|term| custom::Term::compile(term, settings.terms_any_case))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "脱敏词规则编译失败")?;
        for allowed in &mut settings.allow {
            *allowed = allowed.trim().to_lowercase();
        }
        settings.allow.retain(|v| !v.is_empty());
        settings.allow.sort();
        settings.allow.dedup();
        if settings.allow.len() > 100 || settings.allow.iter().any(|v| v.len() > 256) {
            return Err("白名单最多 100 项，每项最多 256 字节");
        }
        if settings.rules.len() > 32 {
            return Err("自定义脱敏规则最多 32 条");
        }
        let mut configured = Vec::new();
        let mut custom = Vec::new();
        for mut rule in std::mem::take(&mut settings.rules) {
            if let Some(compiled) = rule.compile()? {
                if !configured.contains(&rule) {
                    configured.push(rule);
                    custom.push(compiled);
                }
            }
        }
        settings.rules = configured;
        Ok(Self {
            settings,
            terms,
            custom,
        })
    }
}

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{A2A_[0-9a-f]{32}\}\}").unwrap());

pub(super) fn is_secret_field(name: &str) -> bool {
    detectors::is_secret_field(name)
}

// Encoded attachments can also occur as strings in tool output. Keep only
// the data-URL form, not arbitrary prose beginning with "data:".
fn encoded_attachment(text: &str) -> bool {
    text.strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .and_then(|(head, data)| head.strip_suffix(";base64").map(|media| (media, data)))
        .is_some_and(|(media, data)| {
            media.contains('/')
                && !media.bytes().any(|b| b.is_ascii_whitespace())
                && !data.is_empty()
                && data
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        })
}

pub(crate) struct Mask {
    rules: Rules,
    by_value: HashMap<String, String>,
    by_token: HashMap<String, String>,
    mapped_bytes: usize,
    json_output: Option<bool>,
}

impl Mask {
    pub(crate) fn new(rules: &Rules) -> Self {
        Self {
            rules: rules.clone(),
            by_value: HashMap::new(),
            by_token: HashMap::new(),
            mapped_bytes: 0,
            json_output: None,
        }
    }

    fn token(&mut self, value: &str) -> Result<&str, ()> {
        if value.len() > MAX_VALUE_BYTES {
            return Err(());
        }
        if !self.by_value.contains_key(value) {
            if self.by_value.len() >= MAX_VALUES
                || self.mapped_bytes + value.len() > MAX_MAPPED_BYTES
            {
                return Err(());
            }
            let token = loop {
                let mut random = [0; 16];
                getrandom::getrandom(&mut random).map_err(|_| ())?;
                let mut token = String::with_capacity(TOKEN_LEN);
                token.push_str(PREFIX);
                for byte in random {
                    write!(token, "{byte:02x}").map_err(|_| ())?;
                }
                token.push_str(SUFFIX);
                if !self.by_token.contains_key(&token) {
                    break token;
                }
            };
            self.by_token.insert(token.clone(), value.to_owned());
            self.by_value.insert(value.to_owned(), token);
            self.mapped_bytes += value.len();
        }
        self.by_value.get(value).map(String::as_str).ok_or(())
    }

    pub(super) fn apply(&mut self, text: &mut String, secret_field: bool) -> Result<(), ()> {
        if encoded_attachment(text) {
            return Ok(());
        }
        // The egress pass must not remask placeholders that this request already owns.
        let retained: Vec<_> = PLACEHOLDER
            .find_iter(text)
            .filter(|hit| self.by_token.contains_key(hit.as_str()))
            .map(|hit| (hit.start(), hit.end()))
            .collect();
        let owned = |(a, b): (usize, usize)| {
            let index = retained.partition_point(|&(_, end)| end <= a);
            retained
                .get(index)
                .is_some_and(|&(start, end)| start <= a && b <= end)
        };
        let mut ranges = Vec::new();
        let mut collect = |items: &mut dyn Iterator<Item = (usize, usize)>| {
            for pair in items {
                if pair.0 >= pair.1 || owned(pair) {
                    continue;
                }
                if ranges.len() >= MAX_MATCHES {
                    return Err(());
                }
                ranges.push(pair);
            }
            Ok(())
        };
        let settings = &self.rules.settings;
        for term in &self.rules.terms {
            collect(&mut term.spans(text, settings.terms_whole))?;
        }
        let literal_count = ranges.len();
        let mut record = |pair: (usize, usize)| {
            if pair.0 >= pair.1 || owned(pair) {
                return Ok(());
            }
            if ranges.len() >= MAX_MATCHES {
                return Err(());
            }
            ranges.push(pair);
            Ok(())
        };
        if secret_field && settings.secrets && detectors::secret_field_value(text) {
            record((0, text.len()))?;
        } else {
            detectors::scan(
                text,
                settings.secrets,
                settings.personal,
                &settings.kinds,
                &mut record,
            )?;
            if settings.secrets {
                for re in &self.rules.custom {
                    for m in re
                        .captures_iter(text)
                        .filter_map(|m| m.get(if m.len() > 1 { 1 } else { 0 }))
                    {
                        record((m.start(), m.end()))?;
                    }
                }
            }
        }
        if !settings.allow.is_empty() {
            let mut index = 0;
            ranges.retain(|&(a, b)| {
                index += 1;
                index <= literal_count
                    || settings
                        .allow
                        .binary_search(&text[a..b].to_lowercase())
                        .is_err()
            });
        }
        if ranges.is_empty() {
            return Ok(());
        }
        ranges.sort_unstable();
        // Overlaps must not expose the unmatched end of a secret (for example if a
        // custom term overlaps a key rather than lying wholly inside it).
        let mut merged = Vec::<(usize, usize)>::new();
        for (start, end) in ranges {
            match merged.last_mut() {
                Some((_, previous_end)) if start < *previous_end => {
                    *previous_end = end.max(*previous_end)
                }
                _ => merged.push((start, end)),
            }
        }

        // Subtract owned placeholders from matched ranges, but still mask any new
        // plaintext a match spans (e.g. a freshly appended credential on the same line).
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for (start, end) in merged {
            out.push_str(&text[at..start]);
            let first = retained.partition_point(|&(_, stop)| stop <= start);
            let mut begin = start;
            for &(a, b) in &retained[first..] {
                if a >= end {
                    break;
                }
                if begin < a {
                    out.push_str(self.token(&text[begin..a])?);
                }
                // Copy only the part within this merged range. The rest remains in the source.
                out.push_str(&text[begin.max(a)..end.min(b)]);
                begin = end.min(b);
            }
            if begin < end {
                out.push_str(self.token(&text[begin..end])?);
            }
            at = end;
        }
        out.push_str(&text[at..]);
        *text = out;
        Ok(())
    }

    #[cfg(test)]
    fn into_restore(self) -> Option<Restore> {
        (!self.by_token.is_empty()).then(|| RequestMask(Arc::new(Mutex::new(self))).restore())
    }
}

/// 单次请求的不透明句柄。只在鉴权后创建，不进入日志或遥测快照。
#[derive(Clone)]
pub(crate) struct RequestMask(Arc<Mutex<Mask>>);

impl RequestMask {
    pub(crate) fn new(rules: &Rules) -> Self {
        Self(Arc::new(Mutex::new(Mask::new(rules))))
    }

    pub(crate) fn apply(
        &self,
        format: wire::Format,
        body: &mut serde_json::Value,
    ) -> Result<(), crate::server::errors::GatewayError> {
        let mut mask = self.0.lock().map_err(|_| unavailable())?;
        wire::request(format, body, &mut mask).map_err(|_| unavailable())
    }

    // A stream can start an automatic continuation after the HTTP handler returns.
    // Keep the same mapping alive until both sides of the request are done.
    pub(crate) fn restore(&self) -> Restore {
        Restore {
            request: self.clone(),
            budget: MAX_EXPANSION,
        }
    }
}

pub(crate) fn unavailable() -> crate::server::errors::GatewayError {
    crate::server::errors::GatewayError::with_status(503, "隐私脱敏处理失败，未转发该请求")
        .with_code("privacy_masking")
}

pub(crate) struct Restore {
    // Never derive Debug, and never put this map into an extension visible to metrics/loggers.
    request: RequestMask,
    budget: usize,
}

impl Restore {
    pub(super) fn json_output(&self) -> bool {
        self.request
            .0
            .lock()
            .is_ok_and(|mask| mask.json_output.unwrap_or(false))
    }

    /// Only wait for a prefix of a token actually emitted for this HTTP request.
    pub(super) fn partial_len(&self, text: &str) -> usize {
        let Ok(mask) = self.request.0.lock() else {
            return 0;
        };
        if mask.by_token.is_empty() {
            return 0;
        }
        let max = text.len().min(TOKEN_LEN - 1);
        (1..=max)
            .rev()
            .find(|&n| {
                text.get(text.len() - n..).is_some_and(|tail| {
                    (n <= PREFIX.len() && PREFIX.starts_with(tail))
                        || (tail.starts_with(PREFIX)
                            && mask.by_token.keys().any(|token| token.starts_with(tail)))
                })
            })
            .unwrap_or(0)
    }

    pub(super) fn apply(&mut self, text: &mut String, json_fragment: bool) -> Result<(), ()> {
        let mask = self.request.0.lock().map_err(|_| ())?;
        let mut out = String::new();
        let mut at = 0;
        for hit in PLACEHOLDER.find_iter(text) {
            let Some(value) = mask.by_token.get(hit.as_str()) else {
                continue;
            };
            out.push_str(&text[at..hit.start()]);
            let encoded;
            let value = if json_fragment {
                // Nested tool arguments are themselves JSON strings on the wire.
                encoded = serde_json::to_string(value).map_err(|_| ())?;
                &encoded[1..encoded.len() - 1]
            } else {
                value.as_str()
            };
            self.budget = self.budget.checked_sub(value.len()).ok_or(())?;
            out.push_str(value);
            at = hit.end();
        }
        if at != 0 {
            out.push_str(&text[at..]);
            *text = out;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_mode_also_bounds_date_variants() {
        let rules = Rules::compile(Settings {
            terms: vec!["1990-05-20".into()],
            terms_whole: true,
            ..Settings::default()
        })
        .unwrap();
        let mut mask = Mask::new(&rules);
        let original = "item1990-05-20draft | 1990-05-20";
        let mut payload = serde_json::json!({"messages":[{"role":"user","content":original}]});
        wire::request(wire::Format::Chat, &mut payload, &mut mask).unwrap();
        let redacted = payload
            .pointer("/messages/0/content")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(redacted.starts_with("item1990-05-20draft | "));
        assert_eq!(PLACEHOLDER.find_iter(redacted).count(), 1);
        let mut restored = redacted.to_owned();
        mask.into_restore()
            .unwrap()
            .apply(&mut restored, false)
            .unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn structured_json_output_restores_quotes_as_json_values() {
        let secret = "A\"B";
        let rules = Rules::compile(Settings {
            terms: vec![secret.into()],
            ..Settings::default()
        })
        .unwrap();
        let mut mask = Mask::new(&rules);
        let mut request = serde_json::json!({"model":"fixture","response_format":{"type":"json_object"},"messages":[{"role":"user","content":secret}]});
        wire::request(wire::Format::Chat, &mut request, &mut mask).unwrap();
        let mut translated = serde_json::json!({"input":[]});
        wire::request(wire::Format::Responses, &mut translated, &mut mask).unwrap();
        let token = request
            .pointer("/messages/0/content")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        let mut response = serde_json::json!({"choices":[{"message":{"role":"assistant","content":format!("{{\"name\":{}}}", serde_json::to_string(token).unwrap()),"reasoning_content":format!("reasoning {token}")}}]});
        let mut restore = mask.into_restore().unwrap();
        wire::response(wire::Format::Chat, &mut response, &mut restore).unwrap();
        let text = response
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["name"], secret);
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            format!("reasoning {secret}")
        );
    }

    #[test]
    fn categories_overlap_reuse_and_cross_request_isolation() {
        let rules = Rules::compile(Settings {
            secrets: true,
            personal: true,
            terms: vec!["nightjar".into(), "lp-example".into()],
            ..Settings::default()
        })
        .unwrap();
        let email = "alice@mailprobe.internal";
        let phone = "+8613900009931";
        let password = "Example1234";
        let key = concat!("sk-", "lp-example", "ABC123def456GHI789");
        let input = format!("nightjar {email} {phone} DB_PASSWORD={password} {key} lp-example");
        let mut mask = Mask::new(&rules);
        let mut redacted = input.clone();
        mask.apply(&mut redacted, false).unwrap();
        let tokens = PLACEHOLDER
            .find_iter(&redacted)
            .map(|m| m.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tokens.len(), 6);
        for expected in ["nightjar", email, phone, password, key, "lp-example"] {
            assert!(
                !redacted.contains(expected),
                "expected range remained: {expected}"
            );
            assert!(
                input.contains(expected),
                "fixture omitted expected range: {expected}"
            );
            assert!(
                mask.by_value.contains_key(expected),
                "range had no mapping: {expected}"
            );
        }
        let mut repeated = input.to_owned();
        mask.apply(&mut repeated, false).unwrap();
        assert_eq!(repeated, redacted);
        let mut second = Mask::new(&rules);
        second.apply(&mut repeated, false).unwrap();
        let mut other = input.to_owned();
        second.apply(&mut other, false).unwrap();
        assert_ne!(redacted, other);
        let mut saved = redacted.clone();
        second
            .into_restore()
            .unwrap()
            .apply(&mut saved, false)
            .unwrap();
        assert_eq!(saved, redacted);
        mask.into_restore()
            .unwrap()
            .apply(&mut redacted, false)
            .unwrap();
        assert_eq!(redacted, input);
    }
}
