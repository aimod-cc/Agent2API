//! 格式校验与上下文防误判守卫，移植自 yetone/magpie（MIT，见 licenses/magpie-MIT），
//! 并保留本项目更严格的中国身份证号（省份码 + 闰年日期 + 校验位）验证。

use std::net::IpAddr;
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate, Utc};
use regex::Regex;

use super::guard_data::*;
use super::{wallet, Guard};

pub(super) fn before(s: &str, a: usize, n: usize) -> usize {
    let mut at = a.saturating_sub(n);
    while !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

pub(super) fn adjacent(s: &str, a: usize, b: usize, bound: &str) -> bool {
    a.checked_sub(1)
        .is_some_and(|i| bound.as_bytes().contains(&s.as_bytes()[i]))
        || s.as_bytes()
            .get(b)
            .is_some_and(|c| bound.as_bytes().contains(c))
}

pub(super) fn word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn numeric(c: Option<&u8>) -> bool {
    c.is_some_and(u8::is_ascii_digit)
}

pub(super) fn repair(guard: Guard, s: &str, a: usize, mut b: usize) -> Option<(usize, usize)> {
    match guard {
        Guard::IbanAt => loop {
            if iban(&s[a..b]) {
                break Some((a, b));
            }
            b = a + s[a..b].rfind(' ')?;
        },
        Guard::HomeAt => {
            let name = s[a..b].trim_end_matches('.');
            (!name.is_empty()
                && !name.starts_with('.')
                && !NOT_NAMES.contains(&name.to_ascii_lowercase().as_str()))
            .then_some((a, a + name.len()))
        }
        Guard::None => Some((a, b)),
        _ => None,
    }
}

pub(super) fn accept(guard: Guard, v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    match guard {
        Guard::None => true,
        Guard::Bip32Key => wallet::bip32(v),
        Guard::BucketOK => {
            v.len() >= 3
                && !v.bytes().all(|b| b.is_ascii_digit())
                && !NOT_BUCKETS.contains(&v.to_ascii_lowercase().as_str())
        }
        Guard::ChineseID => chinese_id(v),
        Guard::DateOK => date_ok(v),
        Guard::HeaderValue => header(v),
        Guard::HostOK => host(v),
        Guard::Ipv6OK => {
            v.matches(':').count() >= 2
                && v.parse::<IpAddr>()
                    .is_ok_and(|a| a.is_ipv6() && public_ip(a))
        }
        Guard::LineHostOK => host(v) && !in_code(v),
        Guard::LineUserOK => {
            user(v)
                && !in_code(v)
                && !CHAT_WORDS.contains(&v.to_ascii_lowercase().as_str())
                && !IDENT_PARTS.contains(&v.to_ascii_lowercase().as_str())
        }
        Guard::Luhn => luhn(v),
        Guard::MacOK => mac(v),
        Guard::NotASlug => not_slug(v),
        Guard::NotAVariable => not_variable(v),
        Guard::PassportOK => {
            v.bytes().filter(|c| c.is_ascii_digit()).count() >= 5 && not_repeated(v)
        }
        Guard::RealEmail => email(v),
        Guard::SecretValue => secret(v),
        Guard::AssignedSecret => secret_field_value(v) && !path_value(v) && !in_code(v),
        Guard::SerialOK => {
            v.len() >= 6 && v.bytes().any(|c| c.is_ascii_digit()) && not_repeated(v)
        }
        Guard::SolanaKey => wallet::solana(v),
        Guard::SolanaKeypair => wallet::solana_array(v),
        Guard::SsnOK => ssn(v),
        Guard::StellarSeed => wallet::stellar(v),
        Guard::UserOK => user(v),
        Guard::WifKey => wallet::wif(v),
        _ => false,
    }
}

pub(super) fn context(guard: Guard, s: &str, a: usize, b: usize) -> bool {
    match guard {
        Guard::None => true,
        Guard::EmailAt => {
            !s[..a].ends_with('.')
                && !(s.as_bytes().get(b) == Some(&b'.')
                    && s.as_bytes()
                        .get(b + 1)
                        .is_some_and(|&c| word_byte(c) || c == b'-'))
                && !in_userinfo(s, a)
        }
        Guard::FieldAt => field_at(s, a, b),
        Guard::HiddenInURL => hidden_in_url(s, a, b),
        Guard::Ipv4At => ipv4(s, a, b),
        Guard::KeyAt => key_at(s, a, b),
        Guard::NamedSecretAt => {
            field_at(s, a, b) && field_name(s, a).is_none_or(|name| secret_field(name))
        }
        Guard::NotLineIdent => {
            let words = ident_words(&s[a..b]);
            s[..a].ends_with(['\'', '"'])
                || words.len() < 2
                || !words
                    .iter()
                    .any(|w| IDENT_PARTS.contains(&w.to_ascii_lowercase().as_str()))
        }
        Guard::NotPackagePin => {
            !s[b..]
                .strip_prefix('@')
                .is_some_and(|t| VERSION.is_match(t.trim_end_matches('.')))
                && !(s[..a].ends_with('@')
                    && VERSION.is_match(&s[a..b])
                    && s[a..b].parse::<IpAddr>().is_err())
        }
        Guard::PhoneAt => phone(s, a, b),
        Guard::WalletKeyAt => wallet::wallet_context(s, a),
        _ => false,
    }
}

fn not_repeated(v: &str) -> bool {
    v.as_bytes().windows(2).any(|p| p[0] != p[1])
}

fn not_variable(v: &str) -> bool {
    !v.starts_with(['$', '%', '{', '<', '[', '*'])
        && !v.trim_matches(['*', 'x', 'X', '.']).is_empty()
}

fn secret(v: &str) -> bool {
    not_variable(v)
        && !v.starts_with("process.env")
        && !v.starts_with("os.")
        && v.bytes().any(|b| b.is_ascii_digit())
        && v.bytes().any(|b| b.is_ascii_alphabetic())
}

fn not_slug(v: &str) -> bool {
    v.bytes().any(|c| c.is_ascii_uppercase())
        || v.trim_start_matches("sk-")
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-' || b == b'_')
        || v.split(['-', '_']).any(|p| p.len() >= 16)
}

/// 严格的中国大陆 18 位身份证校验：合法省级行政区码 + 合法公历日期（含闰年） + GB 11643 校验位。
fn chinese_id(v: &str) -> bool {
    let bytes = v.as_bytes();
    if bytes.len() != 18 || !bytes[..17].iter().all(u8::is_ascii_digit) {
        return false;
    }
    let Ok(prov) = v[0..2].parse::<u8>() else {
        return false;
    };
    if !matches!(
        prov,
        11..=15 | 21..=23 | 31..=37 | 41..=46 | 50..=54 | 61..=65
    ) {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        v[6..10].parse::<i32>(),
        v[10..12].parse::<u32>(),
        v[12..14].parse::<u32>(),
    ) else {
        return false;
    };
    if !(1900..=2099).contains(&year) || NaiveDate::from_ymd_opt(year, month, day).is_none() {
        return false;
    }
    let sum: usize = bytes[..17]
        .iter()
        .zip([7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2])
        .map(|(b, w)| usize::from(b - b'0') * w)
        .sum();
    bytes[17].to_ascii_uppercase() == b"10X98765432"[sum % 11]
}

pub(super) fn card_prefix(s: &str, a: usize, mut b: usize) -> Option<(usize, usize)> {
    if luhn(&s[a..b]) {
        return Some((a, b));
    }
    while b > a {
        let separator = s[a..b].rfind([' ', '-'])?;
        b = a + separator;
        if luhn(&s[a..b]) {
            return Some((a, b));
        }
    }
    None
}

fn luhn(v: &str) -> bool {
    let n = v.bytes().filter(u8::is_ascii_digit).count();
    if !(15..=19).contains(&n) {
        return false;
    }
    let digits: Vec<u8> = v
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    if digits.iter().all(|d| *d == digits[0]) {
        return false;
    }
    digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            u16::from(if i % 2 == 0 {
                d
            } else if d > 4 {
                d * 2 - 9
            } else {
                d * 2
            })
        })
        .sum::<u16>()
        % 10
        == 0
}

fn ssn(v: &str) -> bool {
    let s = v.replace([' ', '-'], "");
    s.len() == 9
        && !s.starts_with("000")
        && !s.starts_with("666")
        && !s.starts_with('9')
        && &s[3..5] != "00"
        && &s[5..] != "0000"
        && !matches!(s.as_str(), "123456789" | "078051120" | "219099999")
        && not_repeated(&s)
}

fn iban(v: &str) -> bool {
    let s = v.replace(' ', "");
    let Some(country) = s.get(..2) else {
        return false;
    };
    if !IBAN_LEN
        .iter()
        .any(|&(key, n)| key == country && n == s.len())
    {
        return false;
    }
    let n = s
        .bytes()
        .skip(4)
        .chain(s.bytes().take(4))
        .try_fold(0_u32, |n, b| match b {
            b'0'..=b'9' => Some((n * 10 + u32::from(b - b'0')) % 97),
            b'A'..=b'Z' => Some((n * 100 + u32::from(b - b'A') + 10) % 97),
            _ => None,
        });
    n == Some(1)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub(in crate::server::core::privacy) fn month(name: &str) -> Option<u32> {
    let lower = name.trim_end_matches('.').to_ascii_lowercase();
    if lower == "sept" {
        return Some(9);
    }
    MONTHS
        .iter()
        .position(|n| {
            n.eq_ignore_ascii_case(&lower)
                || lower.len() == 3 && n[..3].eq_ignore_ascii_case(&lower)
        })
        .map(|n| 1 + n as u32)
}

pub(in crate::server::core::privacy) fn month_name(number: u32) -> &'static str {
    MONTHS[number as usize - 1]
}

static DATE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+").expect("date pattern"));
static MONTH_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z]{3,}").expect("month pattern"));

fn date_ok(v: &str) -> bool {
    let ns: Vec<_> = DATE_NUMBER.find_iter(v).map(|m| m.as_str()).collect();
    let val = |x: &str| x.parse::<u32>().unwrap_or(0);
    let today = Utc::now().date_naive();
    let cutoff = today
        .with_year(today.year() - 1)
        .or_else(|| NaiveDate::from_ymd_opt(today.year() - 1, today.month(), 28));
    let valid = |y: u32, m, d| {
        (1900..=2099).contains(&y)
            && NaiveDate::from_ymd_opt(y as i32, m, d)
                .zip(cutoff)
                .is_some_and(|(past, cutoff)| past < cutoff)
    };
    match ns.as_slice() {
        [x, y] => MONTH_WORD
            .find_iter(v)
            .find_map(|m| month(m.as_str()))
            .is_some_and(|m| {
                if x.len() == 4 {
                    valid(val(x), m, val(y))
                } else {
                    valid(val(y), m, val(x))
                }
            }),
        [x, y, z] if x.len() == 4 => valid(val(x), val(y), val(z)),
        [x, y, z] => valid(val(z), val(x), val(y)) || valid(val(z), val(y), val(x)),
        _ => false,
    }
}

fn mac(v: &str) -> bool {
    let l = v.replace('-', ":").to_ascii_lowercase();
    !matches!(
        l.as_str(),
        "00:00:00:00:00:00"
            | "ff:ff:ff:ff:ff:ff"
            | "00:11:22:33:44:55"
            | "aa:bb:cc:dd:ee:ff"
            | "01:23:45:67:89:ab"
            | "12:34:56:78:9a:bc"
    ) && l.bytes().any(|b| matches!(b, b'a'..=b'f'))
}

fn user(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    l.len() >= 2 && !NOT_USERS.contains(&l.as_str()) && !NOT_NAMES.contains(&l.as_str())
}

fn in_code(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    l.split_once('.').is_some_and(|(first, rest)| {
        let last = rest.rsplit('.').next().unwrap_or(rest);
        CODE_OBJECTS.contains(&first)
            || NOT_USERS.contains(&last)
            || NOT_NAMES.contains(&last)
            || PUBLIC_HOSTS.contains(&last)
            || matches!(last, "login" | "username")
    })
}

fn placeholder_host(host: &str) -> bool {
    [
        "example.com",
        "example.org",
        "example.net",
        ".example",
        ".test",
        ".invalid",
        ".localhost",
    ]
    .iter()
    .any(|a| {
        let domain = a.trim_start_matches('.');
        host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

fn host(v: &str) -> bool {
    let l = v.trim_end_matches('.').to_ascii_lowercase();
    l.len() >= 2
        && !PUBLIC_HOSTS.contains(&l.as_str())
        && !["process.", "os.", "env."].iter().any(|p| l.starts_with(p))
        && l.parse::<IpAddr>().is_err()
        && !placeholder_host(&l)
}

fn public_ip(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    if ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unspecified()
        || RESOLVERS.contains(&ip.to_string().as_str())
    {
        return false;
    }
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !v.is_private()
                && !v.is_link_local()
                && !v.is_documentation()
                && a != 0
                && a < 240
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0 && c == 0)
                && !(a == 198 && matches!(b, 18 | 19))
        }
        IpAddr::V6(v) => {
            let [first, second, ..] = v.segments();
            (first & 0xe000) == 0x2000 && !(first == 0x2001 && second == 0xdb8)
        }
    }
}

fn ipv4(s: &str, a: usize, b: usize) -> bool {
    let before = s[before(s, a, 16)..a]
        .trim_end_matches([' ', '\t', ':', '=', '"', '\''])
        .to_ascii_lowercase();
    if ["version", "ver", "release", "build", "rev", "版本"]
        .iter()
        .any(|w| {
            before
                .strip_suffix(w)
                .is_some_and(|p| p.as_bytes().last().is_none_or(|&c| !word_byte(c)))
        })
    {
        return false;
    }
    if adjacent(
        s,
        a,
        b,
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-",
    ) || s[..a].ends_with(['.', '=', '~', '^'])
        || (s.as_bytes().get(b) == Some(&b'.') && numeric(s.as_bytes().get(b + 1)))
        || (s[..a].ends_with('/')
            && a >= 2
            && (word_byte(s.as_bytes()[a - 2]) || matches!(s.as_bytes()[a - 2], b'.' | b'-')))
    {
        return false;
    }
    s[a..b].parse().is_ok_and(public_ip)
}

static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[~^]|[<>=!~]=?|==)?[vV]?[0-9]+(?:\.[0-9]+)+(?:[-+._]?[A-Za-z0-9.-]+)?$")
        .expect("version pattern")
});
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}[Tt][0-9]{1,2}(?::[0-9]{1,2}){0,2}(?:\.[0-9]+)?(?:[Zz]|[+-][0-9]{2}:?[0-9]{2})?$")
        .expect("timestamp pattern")
});
static LINE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^L?[0-9]+(?:[-–:]L?[0-9]+)*[.,;:]?$").expect("line pattern"));
static SKILL_SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)+$").expect("slug pattern"));
static SCALE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+(?:\.[0-9]+)?x$").expect("scale pattern"));

fn email(v: &str) -> bool {
    let Some((local, raw)) = v.rsplit_once('@') else {
        return false;
    };
    if local.eq_ignore_ascii_case("git")
        || local.eq_ignore_ascii_case("noreply")
        || local.ends_with('@')
    {
        return false;
    }
    let lower = raw.to_ascii_lowercase();
    let mut host = lower.as_str();
    let Some((stem, tld)) = host.rsplit_once('.') else {
        return false;
    };
    if SCALE_EXT.contains(&tld) && SCALE.is_match(stem) {
        return false;
    }
    while let Some((stem, tld)) = host.rsplit_once('.') {
        if FILE_EXT.contains(&tld) {
            host = stem;
        } else {
            break;
        }
    }
    let Some((_, tld)) = host.rsplit_once('.') else {
        return false;
    };
    !host.starts_with("noreply")
        && !DOC_FILES.contains(&host)
        && !placeholder_host(host)
        && !host.split('.').all(|s| LINKER_WORDS.contains(&s))
        && (LOCAL_TLDS.contains(&tld) || psl::suffix(tld.as_bytes()).is_some_and(|s| s.is_known()))
}

fn in_userinfo(s: &str, a: usize) -> bool {
    let prefix = &s[..a];
    let Some((scheme, authority)) = prefix.rsplit_once("://") else {
        return false;
    };
    !authority.contains([
        ' ', '\t', '\r', '\n', '/', '?', '#', '"', '\'', '<', '>', '@', '\x60',
    ]) && scheme
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric())
}

fn hidden_in_url(s: &str, a: usize, b: usize) -> bool {
    let pw = &s[a..b];
    let Some((_, name)) = s[..a].strip_suffix(':').and_then(|p| p.rsplit_once("//")) else {
        return true;
    };
    name != pw
        && !s[b..]
            .strip_prefix('@')
            .unwrap_or("")
            .split([
                ' ', '\t', '\r', '\n', '"', '\'', '<', '>', '\x60', '\\', ')', ']', '}',
            ])
            .next()
            .unwrap_or("")
            .split(['/', ':', '.', '?', '#', '&', '=', ';', ',', '@'])
            .any(|part| part == pw)
}

fn header(v: &str) -> bool {
    if !not_variable(v) {
        return false;
    }
    let fields: Vec<_> = v.split_whitespace().collect();
    if fields.len() == 1
        || (fields.len() == 2 && fields[0].bytes().all(|x| x.is_ascii_alphabetic()))
    {
        let x = fields[fields.len() - 1];
        return x.len() >= 8 && not_variable(x) && x.bytes().any(|b| !b.is_ascii_alphabetic());
    }
    v.contains('=')
        && v.split(';')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .all(|t| {
                t.split_once('=')
                    .is_some_and(|(k, _)| !k.is_empty() && !k.contains([' ', '\t']))
            })
}

fn path_value(v: &str) -> bool {
    !v.contains(['+', '='])
        && (["~/", "./", "../"]
            .iter()
            .any(|p| v.starts_with(p) && v.len() > p.len())
            || v.strip_prefix('/')
                .and_then(|s| s.split_once('/'))
                .is_some_and(|(root, rest)| PATH_ROOTS.contains(&root) && !rest.is_empty()))
}

pub(super) fn ident_words(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for part in v.split(['_', '.', '-']).filter(|x| !x.is_empty()) {
        let mut start = 0;
        let b = part.as_bytes();
        for i in 1..b.len() {
            if b[i].is_ascii_uppercase() && b[i - 1].is_ascii_lowercase() {
                out.push(&part[start..i]);
                start = i;
            }
        }
        out.push(&part[start..]);
    }
    out
}

fn names_a_secret(v: &str) -> bool {
    v.bytes().all(word_byte_or_dot)
        && (v.as_bytes().first().is_some_and(u8::is_ascii_lowercase) || v.contains(['_', '.']))
        && {
            let words = ident_words(v);
            words.len() >= 2
                && words.iter().any(|w| {
                    SECRET_WORDS.contains(
                        &w.trim_end_matches(|c: char| c.is_ascii_digit())
                            .to_ascii_lowercase()
                            .as_str(),
                    )
                })
        }
}

fn word_byte_or_dot(b: u8) -> bool {
    word_byte(b) || b == b'.'
}

pub(super) fn field_name(s: &str, a: usize) -> Option<&str> {
    let start = s[..a].trim_end_matches([' ', '\t', '"', '\'', '\\', '“', '「']);
    if let Some(xml) = start.strip_suffix('>') {
        return xml
            .rsplit_once('<')
            .and_then(|(_, tag)| tag.split_ascii_whitespace().next())
            .filter(|x| {
                !x.is_empty()
                    && x.bytes()
                        .all(|b| word_byte_or_dot(b) || matches!(b, b':' | b'-'))
            });
    }
    let end = start
        .strip_suffix([':', '=', '：'])?
        .trim_end_matches([' ', '\t', '"', '\'', '\\']);
    let field = end
        .rsplit(|c: char| {
            !(c.is_ascii_alphanumeric()
                || matches!(c, '_' | '.' | '-')
                || ('\u{4e00}'..='\u{9fff}').contains(&c))
        })
        .next()?;
    (!field.is_empty()).then_some(field)
}

static SECRET_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|priv[_-]?key|credential|authori[sz]ation|cookie|^key$|(?:^|_)pass$")
        .expect("secret field pattern")
});

pub(super) fn secret_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("_id")
        || lower.ends_with("keyid")
        || lower.ends_with("key_id")
        || lower.ends_with("_count")
        || lower.ends_with("tokens")
        || matches!(
            lower.as_str(),
            "max_tokens"
                | "token_count"
                | "token_type"
                | "public_key"
                | "publickey"
                | "accesskeyid"
        )
    {
        return false;
    }
    SECRET_FIELD.is_match(name)
        || name.ends_with("_KEY")
            && !["public", "cache", "sort", "license", "lookup"]
                .iter()
                .any(|w| lower.contains(w))
}

pub(super) fn secret_field_value(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && not_variable(value) && !TIMESTAMP.is_match(value)
}

pub(super) fn kind_for_field_name(name: Option<&str>, default_kind: &'static str) -> &'static str {
    let Some(name) = name else {
        return default_kind;
    };
    let lower = name.to_ascii_lowercase();
    if lower.contains("pass") || lower.contains("pwd") || name.contains("密码") || name.contains("密碼") || name.contains("口令") {
        "PASSWORD"
    } else if lower.contains("key") || name.contains("密钥") || name.contains("密鑰") || name.contains("私钥") || name.contains("私鑰") {
        "API_KEY"
    } else {
        default_kind
    }
}

fn field_at(s: &str, a: usize, b: usize) -> bool {
    let v = &s[a..b];
    let prospective = field_name(s, a);
    if prospective.is_some_and(|f| {
        f.rsplit_once('.')
            .is_some_and(|(_, x)| SOURCE_EXT.contains(&x.to_ascii_lowercase().as_str()))
            || matches!(
                f.to_ascii_lowercase().as_str(),
                "access_key_id" | "accesskeyid" | "token_count" | "public_key" | "publickey"
            )
    }) {
        return false;
    }
    let after = s[b..].trim_start_matches([' ', '\t']);
    let code = v.bytes().all(word_byte_or_dot)
        && (after.starts_with('(')
            || !s[..a].ends_with(['"', '\''])
                && s[..a].trim_end().ends_with(':')
                && after.starts_with([';', ',', ')', '}', ']', '|', '?', '&', '<', '>']));
    let skill = SKILL_SLUG.is_match(v)
        && !not_slug(v)
        && s[..a]
            .strip_suffix(':')
            .and_then(|p| {
                p.rsplit(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
                    .next()
            })
            .is_some_and(|p| SKILL_SLUG.is_match(p));
    !code
        && !skill
        && !v.starts_with('=')
        && (!LINE_REF.is_match(v)
            || prospective.is_some_and(secret_field) && v.bytes().all(|byte| byte.is_ascii_digit()))
        && !VERSION.is_match(v)
        && !TIMESTAMP.is_match(v)
        && !path_value(v)
        && !names_a_secret(v)
}

const QUANTITY_UNITS: &[&str] = &[
    "位", "比特", "字节", "字節", "个", "個", "次", "秒", "毫秒", "分钟", "分鐘", "小时", "小時",
    "天", "年", "bit", "byte", "ms", "sec", "min", "ビット", "バイト", "비트", "바이트",
];

fn key_at(s: &str, a: usize, b: usize) -> bool {
    let v = &s[a..b];
    if path_value(v) || VERSION.is_match(v) || TIMESTAMP.is_match(v) {
        return false;
    }
    if v.bytes().all(|byte| byte.is_ascii_digit()) {
        let before = s[..a].trim_end_matches([' ', '\t', '"', '\'', '“', '「', '\\']);
        let after = s[b..].trim_start_matches([' ', '\t']);
        return v.len() >= 6
            && before.ends_with(['为', '為', '是', '：', ':', '=', 'は', '는'])
            && !QUANTITY_UNITS.iter().any(|unit| after.starts_with(unit));
    }
    field_at(s, a, b)
        && not_variable(v)
        && v.bytes().any(|byte| byte.is_ascii_alphabetic())
        && (v.bytes().any(|byte| byte.is_ascii_digit()) || v.len() >= 16)
}

static NEXT_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Z_][A-Z0-9_]*|[a-z_][a-z0-9_-]*|--?[A-Za-z][A-Za-z0-9_-]*)=[^=]")
        .expect("assignment pattern")
});

pub(super) fn next_field(s: &str, a: usize, b: usize) -> bool {
    s[..a].ends_with([' ', '\t']) && NEXT_FIELD.is_match(&s[a..b])
}

fn phone(s: &str, a: usize, b: usize) -> bool {
    if b - a > 11 {
        return true;
    }
    let name_is_phone = |w: &str| {
        PHONE_MARKS.iter().any(|m| w.contains(m))
            || ident_words(w)
                .iter()
                .any(|p| PHONE_WORDS.contains(&p.to_ascii_lowercase().as_str()))
    };
    if let Some(name) = field_name(s, a) {
        if name_is_phone(name) || name.eq_ignore_ascii_case("number") {
            return true;
        }
        if ident_words(name)
            .iter()
            .any(|p| ID_WORDS.contains(&p.to_ascii_lowercase().as_str()))
        {
            return false;
        }
        let clause = &s[before(s, a, 48)..a];
        let clause = clause
            .rsplit_once([',', '，', ';', '；', '\n', '\r'])
            .map_or(clause, |(_, tail)| tail);
        return clause
            .split([' ', '\t'])
            .rev()
            .skip(1)
            .take(2)
            .any(name_is_phone);
    }
    let prefix = s[before(s, a, 32)..a]
        .trim_end_matches([' ', '\t', '(', '"', '\'', '“', '「', '是', '为', '為'])
        .to_ascii_lowercase();
    if prefix.ends_with('#') {
        return false;
    }
    for w in ID_WORDS {
        if let Some(before) = prefix.strip_suffix(w) {
            if w.is_ascii() && before.as_bytes().last().is_some_and(|&b| word_byte(b)) {
                continue;
            }
            return matches!(*w, "no" | "no." | "number")
                && before
                    .split_ascii_whitespace()
                    .next_back()
                    .is_some_and(name_is_phone);
        }
    }
    true
}

pub(super) fn numeric_arrays(s: &str) -> Vec<(usize, usize)> {
    if !s.trim_start().starts_with(['[', '{'])
        || serde_json::from_str::<serde::de::IgnoredAny>(s).is_err()
    {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = None;
    let (mut quoted, mut escaped) = (false, false);
    for (i, byte) in s.bytes().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        if byte == b'"' {
            quoted = true;
            start = None;
            continue;
        }
        match byte {
            b'[' => start = Some(i),
            b']' => {
                if let Some(begin) = start.take() {
                    ranges.push((begin, i + 1));
                }
            }
            b'0'..=b'9'
            | b','
            | b'.'
            | b'-'
            | b'+'
            | b'e'
            | b'E'
            | b' '
            | b'\t'
            | b'\n'
            | b'\r' => {}
            _ => start = None,
        }
    }
    ranges
}

pub(super) fn part_of_number_or_id(
    s: &str,
    a: usize,
    b: usize,
    numeric_arrays: &[(usize, usize)],
) -> bool {
    let raw = s.as_bytes();
    if (numeric(raw.get(a)) && a >= 2 && raw[a - 1] == b'.' && numeric(raw.get(a - 2)))
        || (numeric(raw.get(b - 1)) && raw.get(b) == Some(&b'.') && numeric(raw.get(b + 1)))
    {
        return true;
    }
    if !s[a..b].bytes().all(|x| x.is_ascii_digit()) {
        return false;
    }
    let in_array = numeric_arrays
        .partition_point(|&(start, _)| start <= a)
        .checked_sub(1)
        .is_some_and(|index| numeric_arrays[index].1 >= b);
    if in_array {
        return true;
    }
    let after = s[b..].trim_start();
    let before = s[..a].trim_end();
    (after.is_empty() || after.starts_with([',', '}', ']']))
        && before
            .strip_suffix(':')
            .is_some_and(|key| key.trim_end().ends_with('"'))
}
