//! Custom literals, date variants and bounded prefix/regex rules (Magpie MIT; see licenses/magpie-MIT).
use super::detectors::{month, month_name};
use chrono::NaiveDate;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

const SEP: &str = r"(?:[ \t]*[-/.、／－][ \t]*|[ \t])";
const ORD: &str = r"(?:st|nd|rd|th)?";
static DATE_FORMATS: LazyLock<[Regex; 5]> = LazyLock::new(|| {
    [
    format!(r"^([0-9]{{4}}){SEP}([0-9]{{1,2}}){SEP}([0-9]{{1,2}})\.?$"),
    r"^([0-9]{4})[ \t]*[年년][ \t]*([0-9]{1,2})[ \t]*[月월][ \t]*([0-9]{1,2})[ \t]*[日号號일]?$".to_owned(),
    format!(r"^([0-9]{{1,2}}){SEP}([0-9]{{1,2}}){SEP}([0-9]{{4}})$"),
    format!(r"^([A-Za-z]+)\.?[ \t]+([0-9]{{1,2}}){ORD},?[ \t]+([0-9]{{4}})$"),
    format!(r"^([0-9]{{1,2}}){ORD}(?:[ \t]+|-)([A-Za-z]+)\.?,?(?:[ \t]+|-)([0-9]{{4}})$"),
].map(|pattern|Regex::new(&pattern).expect("constant date expression"))
});

fn date_word(s: &str) -> Option<(i32, u32, u32)> {
    for (i, re) in DATE_FORMATS.iter().enumerate() {
        let Some(m) = re.captures(s) else { continue };
        let (a, b, c) = (&m[1], &m[2], &m[3]);
        let number = |s: &str| s.parse::<u32>().unwrap_or(0);
        let (y, m, d) = match i {
            0 | 1 => (number(a), number(b), number(c)),
            2 => {
                let (p, q, y) = (number(a), number(b), number(c));
                if p == q || (q > 12 && p <= 12) {
                    (y, p, q)
                } else if p > 12 && q <= 12 {
                    (y, q, p)
                } else {
                    return None;
                }
            }
            3 => (number(c), month(a)?, number(b)),
            _ => (number(c), month(b)?, number(a)),
        };
        if !(1000..=2999).contains(&y) {
            return None;
        }
        return NaiveDate::from_ymd_opt(y as i32, m, d).map(|_| (y as i32, m, d));
    }
    None
}

fn date_variants(w: &str) -> Option<String> {
    let (y, m, d) = date_word(w)?;
    let name = month_name(m);
    let short = if m == 9 { "Sept|Sep" } else { &name[..3] };
    let named = format!(r"(?i:{name}|(?:{short})\.?)");
    let (month, day) = (
        if m < 10 {
            format!("0?{m}")
        } else {
            m.to_string()
        },
        if d < 10 {
            format!("0?{d}")
        } else {
            d.to_string()
        },
    );
    let mut forms = vec![
        format!("{y}{SEP}{month}{SEP}{day}"),
        format!(r"{y}[ \t]*年[ \t]*{month}[ \t]*月[ \t]*{day}(?:[ \t]*[日号號])?"),
        format!(r"{y}[ \t]*년[ \t]*{month}[ \t]*월[ \t]*{day}(?:[ \t]*일)?"),
        format!("{y}{m:02}{d:02}"),
        format!(r"{named}[ \t]+{day}{ORD},?[ \t]+{y}"),
        format!(r"{day}{ORD}(?:[ \t]+|-){named},?(?:[ \t]+|-){y}"),
    ];
    if d > 12 || d == m {
        forms.push(format!("{day}{SEP}{month}{SEP}{y}"));
        if d != m {
            forms.push(format!("{month}{SEP}{day}{SEP}{y}"));
        }
    }
    forms.push(regex::escape(w));
    Some(forms.join("|"))
}

fn date_boundary(s: &str, a: usize, b: usize) -> bool {
    let t = s.as_bytes();
    let digit = |i: usize| t.get(i).is_some_and(u8::is_ascii_digit);
    !(a > 0 && (digit(a - 1) || a > 1 && t[a - 1] == b'.' && digit(a - 2)))
        && !s[..a]
            .chars()
            .next_back()
            .is_some_and(|p| p.is_ascii_alphabetic() && t[a].is_ascii_alphabetic())
        && !(digit(b) || t.get(b) == Some(&b'.') && digit(b + 1))
}
fn whole_word(s: &str, a: usize, b: usize) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let v = &s[a..b];
    let first = v.chars().next().unwrap_or(' ');
    let last = v.chars().next_back().unwrap_or(' ');
    !(is_word(first) && s[..a].chars().next_back().is_some_and(is_word))
        && !(is_word(last) && s[b..].chars().next().is_some_and(is_word))
        && !(first.is_ascii_digit()
            && a >= 2
            && s.as_bytes()[a - 1] == b'.'
            && s.as_bytes()[a - 2].is_ascii_digit())
        && !(last.is_ascii_digit()
            && s.as_bytes().get(b) == Some(&b'.')
            && s.as_bytes().get(b + 1).is_some_and(u8::is_ascii_digit))
}

#[derive(Clone)]
pub(super) struct Term {
    pattern: Regex,
    date: bool,
}
impl Term {
    pub(super) fn compile(word: &str, fold: bool) -> Result<Self, regex::Error> {
        let date = date_variants(word);
        let expression = date.clone().unwrap_or_else(|| regex::escape(word));
        Ok(Self {
            pattern: RegexBuilder::new(&expression)
                .case_insensitive(fold)
                .size_limit(512 * 1024)
                .build()?,
            date: date.is_some(),
        })
    }
    pub(super) fn spans<'a>(
        &'a self,
        s: &'a str,
        whole: bool,
    ) -> impl Iterator<Item = (usize, usize)> + 'a {
        self.pattern
            .find_iter(s)
            .filter(move |m| {
                if self.date {
                    date_boundary(s, m.start(), m.end())
                        && (!whole || whole_word(s, m.start(), m.end()))
                } else {
                    !whole || whole_word(s, m.start(), m.end())
                }
            })
            .map(|m| (m.start(), m.end()))
    }
}

#[derive(Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustomRule {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) regex: String,
}
impl CustomRule {
    pub(super) fn compile(&mut self) -> Result<Option<Regex>, &'static str> {
        self.prefix = self.prefix.trim().to_owned();
        self.regex = self.regex.trim().to_owned();
        self.kind = self
            .kind
            .trim()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .take(24)
            .collect::<String>()
            .trim_matches('_')
            .to_owned();
        if self.prefix.is_empty() && self.regex.is_empty() {
            return Ok(None);
        }
        if self.prefix.is_empty() == self.regex.is_empty()
            || (!self.prefix.is_empty() && !(3..=64).contains(&self.prefix.len()))
            || self.regex.len() > 300
        {
            return Err("自定义规则仅填前缀或正则；前缀 3–64 字节、正则最多 300 字节");
        }
        let pattern = if self.prefix.is_empty() {
            self.regex.clone()
        } else {
            format!("{}[A-Za-z0-9_-]{{8,}}", regex::escape(&self.prefix))
        };
        let re = RegexBuilder::new(&pattern)
            .size_limit(512 * 1024)
            .build()
            .map_err(|_| "自定义脱敏正则无效或过于复杂")?;
        if re.is_match("") {
            return Err("自定义脱敏正则不得匹配空字符串");
        }
        Ok(Some(re))
    }
}
