//! Deterministic Magpie detectors; no model, network, global originals or restoration state.
mod builtins;
mod guard_data;
mod guards;
mod ids;
mod wallet;

use std::{collections::BTreeMap, sync::LazyLock};

pub(super) use guards::{month, month_name};
use ids::{IdFilter, IdRule};
use regex::{Captures, Match, Regex};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Category {
    Secrets,
    Personal,
    Ssn,
    Passport,
    Iban,
    Birthday,
    Mac,
    Serial,
    Home,
    Userhost,
    Ip,
    Bucket,
}

impl Category {
    pub(super) fn optional(self) -> bool {
        !matches!(self, Self::Secrets | Self::Personal)
    }
    fn enabled(self, secrets: bool, personal: bool, overrides: &BTreeMap<Self, bool>) -> bool {
        match self {
            Self::Secrets => secrets,
            Self::Personal => personal,
            other => {
                personal
                    && overrides.get(&other).copied().unwrap_or(!matches!(
                        other,
                        Self::Home | Self::Userhost | Self::Ip | Self::Bucket
                    ))
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Capture {
    First,
    Any,
    Groups(&'static [Guard]),
}

#[derive(Clone, Copy)]
enum Guard {
    None,
    Bip32Key,
    BucketOK,
    ChineseID,
    DateOK,
    EmailAt,
    FieldAt,
    HeaderValue,
    HiddenInURL,
    HomeAt,
    HostOK,
    IbanAt,
    Ipv4At,
    Ipv6OK,
    KeyAt,
    LineHostOK,
    LineUserOK,
    Luhn,
    MacOK,
    NamedSecretAt,
    NotASlug,
    NotAVariable,
    NotLineIdent,
    NotPackagePin,
    PassportOK,
    PhoneAt,
    RealEmail,
    SecretValue,
    SeedPhraseAt,
    SerialOK,
    SolanaKey,
    SolanaKeypair,
    SsnOK,
    StellarSeed,
    UserOK,
    WalletKeyAt,
    WifKey,
}

#[derive(Clone, Copy)]
struct Spec {
    category: Category,
    pattern: &'static str,
    capture: Capture,
    id_rule: IdRule,
    bound: &'static str,
    check: Guard,
    context: Guard,
    repair: Guard,
    field: bool,
    markers: &'static [&'static str],
    fold: &'static [&'static str],
}

impl Spec {
    const DEFAULT: Self = Self {
        category: Category::Secrets,
        pattern: "",
        capture: Capture::First,
        id_rule: IdRule::Preserve,
        bound: "",
        check: Guard::None,
        context: Guard::None,
        repair: Guard::None,
        field: false,
        markers: &[],
        fold: &[],
    };
}

static RULES: LazyLock<Result<Vec<(Spec, Regex)>, regex::Error>> = LazyLock::new(|| {
    builtins::BUILTINS
        .iter()
        .map(|&spec| Regex::new(spec.pattern).map(|re| (spec, re)))
        .collect()
});

pub(super) fn ready() -> bool {
    RULES.is_ok()
}

fn captures(
    spec: Spec,
    caps: &Captures<'_>,
    mut visit: impl FnMut(Match<'_>, Guard) -> Result<(), ()>,
) -> Result<(), ()> {
    match spec.capture {
        Capture::Groups(guards) => {
            for (index, guard) in guards.iter().copied().enumerate() {
                if let Some(hit) = caps.get(index + 1) {
                    visit(hit, guard)?;
                }
            }
        }
        Capture::Any => {
            if let Some(hit) = caps.iter().skip(1).flatten().next() {
                visit(hit, spec.check)?;
            }
        }
        Capture::First => {
            if let Some(hit) = caps.get(if caps.len() > 1 { 1 } else { 0 }) {
                visit(hit, spec.check)?;
            }
        }
    }
    Ok(())
}

pub(super) fn scan(
    text: &str,
    secrets: bool,
    personal: bool,
    overrides: &BTreeMap<Category, bool>,
    mut visit: impl FnMut((usize, usize)) -> Result<(), ()>,
) -> Result<(), ()> {
    if !(secrets || personal) || text.len() < 3 {
        return Ok(());
    }
    let rules = RULES.as_ref().map_err(|_| ())?;
    let lower = std::cell::OnceCell::new();
    for (spec, regex) in rules {
        if !spec.category.enabled(secrets, personal, overrides)
            || (!spec.markers.is_empty() && !spec.markers.iter().any(|m| text.contains(m)))
            || (!spec.fold.is_empty()
                && !spec
                    .fold
                    .iter()
                    .any(|m| lower.get_or_init(|| text.to_ascii_lowercase()).contains(m)))
        {
            continue;
        }
        let mut id_filter = IdFilter::default();
        let mut from = 0;
        while let Some(caps) = regex.captures_at(text, from) {
            let Some(full) = caps.get(0) else { break };
            from = full.end();
            if from == full.start() {
                return Err(());
            }
            if spec.field {
                if let Some(hit) = caps.get(1).filter(|m| {
                    m.start() > full.start() && guards::next_field(text, m.start(), m.end())
                }) {
                    from = hit.start();
                    continue;
                }
            }
            captures(*spec, &caps, |hit, check| {
                let (a, b) = (hit.start(), hit.end());
                let Some((a, b)) = guards::repair(spec.repair, text, a, b) else {
                    return Ok(());
                };
                if a >= b || guards::adjacent(text, a, b, spec.bound) {
                    return Ok(());
                }
                let value = &text[a..b];
                if !guards::accept(check, value)
                    || !guards::context(spec.context, text, a, b)
                    || id_filter.keep(spec.id_rule, text, a, b)
                    || (spec.category != Category::Secrets
                        && spec.category != Category::Ip
                        && guards::part_of_number_or_id(text, a, b))
                {
                    return Ok(());
                }
                visit((a, b))
            })?;
        }
    }
    Ok(())
}

pub(super) fn is_secret_field(key: &str) -> bool {
    guards::secret_field(key)
}

pub(super) fn secret_field_value(value: &str) -> bool {
    guards::secret_field_value(value)
}

pub(super) fn tool_lookup(name: &str, value: &str) -> bool {
    ids::tool_lookup(name, value)
}

#[cfg(test)]
mod tests;
