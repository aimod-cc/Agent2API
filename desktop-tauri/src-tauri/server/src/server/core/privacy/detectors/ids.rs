//! Identifier exclusions adapted from Magpie's ids.go (MIT; see licenses/magpie-MIT).
use std::sync::LazyLock;

use regex::Regex;

use super::guards::{adjacent, before, field_name, ident_words, word_byte};

#[derive(Clone, Copy)]
pub(super) enum IdRule {
    Preserve,
    Assignment,
    Credential,
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Identifier,
    Digest,
}

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$").expect("UUID shape")
});

fn hex_shape(value: &str) -> Option<Shape> {
    if value.len() == 36 && UUID.is_match(value) {
        return Some(Shape::Identifier);
    }
    if value.len() >= 32 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(Shape::Identifier);
    }
    if !(16..32).contains(&value.len()) {
        return None;
    }
    let (mut lower, mut upper, mut digits) = (0, 0, 0);
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' => digits += 1,
            b'a'..=b'f' => lower += 1,
            b'A'..=b'F' => upper += 1,
            _ => return None,
        }
    }
    let min = 3.max(value.len() / 8);
    ((lower == 0 || upper == 0) && lower + upper >= min && digits >= min).then_some(Shape::Digest)
}

fn hex_or_hyphen(byte: u8) -> bool {
    byte.is_ascii_hexdigit() || byte == b'-'
}

struct HexRun {
    left: usize,
    right: usize,
    id: Option<(usize, usize, Shape)>,
}

// Cache the last run per rule: a long run with many candidate hits is scanned once.
#[derive(Default)]
pub(super) struct IdFilter {
    run: Option<HexRun>,
}

impl IdFilter {
    fn around(&mut self, text: &str, a: usize, b: usize) -> Option<(usize, usize, Shape)> {
        if let Some(run) = self
            .run
            .as_ref()
            .filter(|run| a >= run.left && b <= run.right)
        {
            return run.id.filter(|&(first, last, _)| a >= first && b <= last);
        }
        if !text[a..b].bytes().all(hex_or_hyphen) {
            return None;
        }
        let raw = text.as_bytes();
        let left = raw[..a]
            .iter()
            .rposition(|&c| !hex_or_hyphen(c))
            .map_or(0, |n| n + 1);
        let right = raw[b..]
            .iter()
            .position(|&c| !hex_or_hyphen(c))
            .map_or(raw.len(), |n| b + n);
        let first = right - text[left..right].trim_start_matches('-').len();
        let last = first + text[first..right].trim_end_matches('-').len();
        let shape = hex_shape(&text[first..last]).and_then(|kind| {
            if kind == Shape::Digest
                && adjacent(
                    text,
                    first,
                    last,
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_",
                )
            {
                None
            } else {
                Some((first, last, kind))
            }
        });
        self.run = Some(HexRun {
            left,
            right,
            id: shape,
        });
        shape.filter(|&(first, last, _)| a >= first && b <= last)
    }

    pub(super) fn keep(&mut self, rule: IdRule, text: &str, a: usize, b: usize) -> bool {
        let field = || field_name(text, a);
        if element_token(text, a, b, field) {
            return !named_credential(text, a, field());
        }
        let Some((left, right, shape)) = self.around(text, a, b) else {
            return false;
        };
        if a != left || b != right {
            return true;
        }
        match rule {
            IdRule::Preserve => true,
            IdRule::Credential => false,
            IdRule::Assignment => {
                let name = field();
                !(shape == Shape::Digest && name.is_none()) && !named_credential(text, a, name)
            }
        }
    }
}

pub(super) fn tool_lookup(name: &str, value: &str) -> bool {
    let value = value.trim();
    (hex_shape(value).is_some() || element_token(value, 0, value.len(), || Some(name)))
        && !credential_name(name)
}

fn element_field(name: &str) -> bool {
    let words = ident_words(name);
    words.len() == 2
        && words[0].eq_ignore_ascii_case("element")
        && words[1].eq_ignore_ascii_case("token")
}

fn element_token<'a>(s: &str, a: usize, b: usize, field: impl FnOnce() -> Option<&'a str>) -> bool {
    let raw = s.as_bytes();
    if b - a < 9 || raw[a] != b's' || a > 0 && word_byte(raw[a - 1]) {
        return false;
    }
    let stem = a + 9;
    if !raw[a + 1..stem]
        .iter()
        .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    {
        return false;
    }
    let end = if raw.get(stem) == Some(&b':') {
        let n = raw[stem + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if n == 0 {
            return false;
        }
        stem + 1 + n
    } else {
        stem
    };
    (b == stem || b == end)
        && !raw.get(end).is_some_and(|&c| word_byte(c))
        && (end > stem || field().is_some_and(element_field))
}

fn named_credential(s: &str, a: usize, name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };
    if !name.eq_ignore_ascii_case("key") {
        return credential_name(name);
    }
    let lower = s[before(s, a, 24)..a].to_ascii_lowercase();
    lower.rsplit_once("key").is_some_and(|(prefix, _)| {
        let prefix = prefix.trim_end_matches([' ', '\t']);
        [
            "api", "access", "secret", "private", "signing", "license", "licence",
        ]
        .iter()
        .any(|part| {
            prefix
                .strip_suffix(part)
                .is_some_and(|rest| rest.as_bytes().last().is_none_or(|&c| !word_byte(c)))
        })
    })
}

const NON_CREDENTIAL: &[&str] = &[
    "page",
    "next",
    "cursor",
    "continuation",
    "csrf",
    "xsrf",
    "trace",
    "request",
    "idempotency",
    "sync",
    "nonce",
    "hash",
    "digest",
    "checksum",
    "sha",
    "fingerprint",
    "etag",
    "cache",
    "commit",
    "object",
    "blob",
    "file",
    "content",
    "revision",
    "rev",
    "partition",
    "shard",
    "bucket",
    "dedup",
    "dedupe",
    "lookup",
    "row",
    "sort",
];

fn credential_name(name: &str) -> bool {
    let words = ident_words(name);
    if words.is_empty()
        || name.eq_ignore_ascii_case("key")
        || element_field(name)
        || words.last().is_some_and(|last| {
            ["id", "ids", "uuid", "guid"]
                .iter()
                .any(|word| last.eq_ignore_ascii_case(word))
        })
    {
        return false;
    }
    // "file_password" is still a password. A password's hash/digest is not.
    let password = words.iter().any(|word| {
        ["password", "passwd", "passphrase", "pass", "pwd"]
            .iter()
            .any(|secret| word.eq_ignore_ascii_case(secret))
    });
    if password
        && !words.iter().any(|word| {
            ["hash", "digest", "checksum", "sha", "fingerprint", "etag"]
                .iter()
                .any(|data| word.eq_ignore_ascii_case(data))
        })
    {
        return true;
    }
    !words.iter().any(|word| {
        NON_CREDENTIAL
            .iter()
            .any(|keep| word.eq_ignore_ascii_case(keep))
    })
}
