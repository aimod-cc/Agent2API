//! 内存内的推理正文脱敏。只接受协议层挑出的文本；不接触鉴权、路由或持久化。

use std::{collections::HashMap, fmt::Write, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

pub(crate) mod stream;
pub(crate) mod wire;

const PREFIX: &str = "{{A2A_";
const SUFFIX: &str = "}}";
const TOKEN_LEN: usize = PREFIX.len() + 32 + SUFFIX.len();
const MAX_VALUES: usize = 4096;
const MAX_MATCHES: usize = 65_536;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_EXPANSION: usize = 16 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) secrets: bool,
    pub(crate) personal: bool,
    pub(crate) terms: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Rules {
    pub(crate) settings: Settings,
    terms: Option<Regex>,
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
        self.settings.secrets || self.settings.personal || self.terms.is_some()
    }

    pub(crate) fn compile(mut settings: Settings) -> Result<Self, &'static str> {
        if settings.terms.len() > 100 || settings.terms.iter().any(|s| s.len() > 256) {
            return Err("terms 最多 100 项，每项最多 256 字节");
        }
        for term in &mut settings.terms {
            *term = term.trim().to_owned();
        }
        settings.terms.retain(|s| !s.is_empty());
        settings.terms.sort();
        settings.terms.dedup();
        let terms = if settings.terms.is_empty() {
            None
        } else {
            let mut by_length: Vec<_> = settings.terms.iter().collect();
            by_length.sort_by_key(|s| std::cmp::Reverse(s.len()));
            let pattern = by_length
                .iter()
                .map(|s| regex::escape(s))
                .collect::<Vec<_>>()
                .join("|");
            Some(Regex::new(&pattern).map_err(|_| "脱敏词规则编译失败")?)
        };
        Ok(Self { settings, terms })
    }
}

static SECRETS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
    Regex::new(r"(?s)-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----.*?-----END (?:[A-Z0-9]+ )*PRIVATE KEY-----").unwrap(),
    Regex::new(r"(?-u:\b)(?:(?:sk-|sk_live_|gh[pousr]_|github_pat_|glpat-|hf_|xox[baprs]-|gsk_|npm_)[A-Za-z0-9_-]{12,}|AIza[A-Za-z0-9_-]{30,}|(?:AKIA|ASIA)[A-Z0-9]{16}|eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)").unwrap(),
    Regex::new(r"(?i)(?-u:\b)bearer[ \t]+([a-z0-9._~+/-]{12,}={0,2})").unwrap(),
]
});
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?-u:\b)(?:[a-z0-9_.-]+[_-])?(?:api[_-]?key|(?:client|api)[_-]?secret|(?:access|refresh|auth)[_-]?token|private[_-]?key|password|passwd|secret|token|credential)["']?[ \t]*[:=][ \t]*(?:"([^"\r\n]+)"|'([^'\r\n]+)'|([^\s,;{}<>"']+))"#).unwrap()
});
static URL_SECRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"://[^:/\s@]+:([^@/\s]+)@").unwrap());
static KEY_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:[a-z0-9_.-]+[_-])?(?:api[_-]?key|(?:client|api)[_-]?secret|(?:access|refresh|auth)[_-]?token|private[_-]?key|password|passwd|secret|token|credential)$").unwrap()
});
static PERSONAL: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(r"(?i)[A-Z0-9.!#$%&'*+/=?^_{|}~-]+@[A-Z0-9-]+(?:\.[A-Z0-9-]+)*\.[A-Z]{2,63}")
            .unwrap(),
        Regex::new(r"(?:\+86[ -]?)?(?-u:\b)1[3-9][0-9]{9}(?-u:\b)").unwrap(),
    ]
});
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{A2A_[0-9a-f]{32}\}\}").unwrap());

pub(super) fn is_secret_field(name: &str) -> bool {
    KEY_NAME.is_match(name)
}

pub(crate) struct Mask<'a> {
    rules: &'a Rules,
    by_value: HashMap<String, String>,
    by_token: HashMap<String, String>,
}

impl<'a> Mask<'a> {
    pub(crate) fn new(rules: &'a Rules) -> Self {
        Self {
            rules,
            by_value: HashMap::new(),
            by_token: HashMap::new(),
        }
    }

    fn token(&mut self, value: &str) -> Result<&str, ()> {
        if value.len() > MAX_VALUE_BYTES {
            return Err(());
        }
        if !self.by_value.contains_key(value) {
            if self.by_value.len() >= MAX_VALUES {
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
        }
        self.by_value.get(value).map(String::as_str).ok_or(())
    }

    pub(super) fn apply(&mut self, text: &mut String, secret_field: bool) -> Result<(), ()> {
        let mut ranges = Vec::new();
        let mut collect = |items: &mut dyn Iterator<Item = (usize, usize)>| {
            for pair in items {
                if ranges.len() >= MAX_MATCHES {
                    return Err(());
                }
                ranges.push(pair);
            }
            Ok(())
        };
        if secret_field && self.rules.settings.secrets && !text.trim().is_empty() {
            ranges.push((0, text.len()));
        } else {
            if self.rules.settings.secrets {
                for pattern in &SECRETS[..2] {
                    collect(&mut pattern.find_iter(text).map(|m| (m.start(), m.end())))?;
                }
                for pattern in [&SECRETS[2], &*ASSIGNMENT, &*URL_SECRET] {
                    collect(&mut pattern.captures_iter(text).filter_map(|c| {
                        c.iter()
                            .skip(1)
                            .flatten()
                            .next()
                            .map(|m| (m.start(), m.end()))
                    }))?;
                }
            }
            if self.rules.settings.personal {
                for pattern in PERSONAL.iter() {
                    collect(&mut pattern.find_iter(text).map(|m| (m.start(), m.end())))?;
                }
            }
            if let Some(pattern) = &self.rules.terms {
                collect(&mut pattern.find_iter(text).map(|m| (m.start(), m.end())))?;
            }
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
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for (start, end) in merged {
            out.push_str(&text[at..start]);
            out.push_str(self.token(&text[start..end])?);
            at = end;
        }
        out.push_str(&text[at..]);
        *text = out;
        Ok(())
    }

    pub(crate) fn into_restore(self) -> Option<Restore> {
        (!self.by_token.is_empty()).then_some(Restore {
            tokens: self.by_token,
            budget: MAX_EXPANSION,
        })
    }
}

pub(crate) struct Restore {
    // Never derive Debug, and never put this map into an extension visible to metrics/loggers.
    tokens: HashMap<String, String>,
    budget: usize,
}

impl Restore {
    /// Only wait for a prefix of a token actually emitted for this HTTP request.
    pub(super) fn partial_len(&self, text: &str) -> usize {
        let max = text.len().min(TOKEN_LEN - 1);
        (1..=max)
            .rev()
            .find(|&n| {
                text.get(text.len() - n..).is_some_and(|tail| {
                    (n <= PREFIX.len() && PREFIX.starts_with(tail))
                        || (tail.starts_with(PREFIX)
                            && self.tokens.keys().any(|token| token.starts_with(tail)))
                })
            })
            .unwrap_or(0)
    }

    pub(super) fn apply(&mut self, text: &mut String, json_fragment: bool) -> Result<(), ()> {
        let mut out = String::new();
        let mut at = 0;
        for hit in PLACEHOLDER.find_iter(text) {
            let Some(value) = self.tokens.get(hit.as_str()) else {
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
    fn categories_overlap_reuse_and_cross_request_isolation() {
        let rules = Rules::compile(Settings {
            secrets: true,
            personal: true,
            terms: vec!["nightjar".into(), "lp-example".into()],
        })
        .unwrap();
        let input = "nightjar example@example.test +86 13800138000 DB_PASSWORD=example sk-example1234567890";
        let mut mask = Mask::new(&rules);
        let mut redacted = input.to_owned();
        mask.apply(&mut redacted, false).unwrap();
        assert_eq!(PLACEHOLDER.find_iter(&redacted).count(), 5);
        for term in [
            "nightjar",
            "example@example.test",
            "13800138000",
            "DB_PASSWORD=example",
            "sk-example",
        ] {
            assert!(!redacted.contains(term));
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
