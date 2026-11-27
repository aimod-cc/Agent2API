//! 隐私文本检测与遮罩编排：
//! - 58+ 条规则表（含加密钱包与国际个人信息）+ 严格格式与上下文防误判；
//! - 支持 JSON 敏感字段整值保护（不看长度与格式）；
//! - 合并重叠遮罩区间防止漏出半截密钥，同时保留已有占位符不二次遮罩。

mod builtins;
mod guard_data;
mod guards;
mod ids;
mod wallet;

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Match, Regex};

pub(in crate::server::core::privacy) use guards::{month, month_name};
use ids::{IdFilter, IdRule};

use super::rules::{CompiledPrivacySettings, PrivacyAction};
use super::vault::PrivacyVault;

const MAX_MATCHES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivacyHitRecord {
    pub kind: String,
    pub count: usize,
    pub action: PrivacyAction,
}

#[derive(Clone, Debug)]
pub enum MaskTextResult {
    Unchanged,
    Handled {
        text: Option<String>,
        hits: Vec<PrivacyHitRecord>,
        blocked_message: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    Secrets,
    Email,
    PhoneCn,
    IdCardCn,
    BankCard,
    Ssn,
    Passport,
    Iban,
    Birthday,
    Mac,
    Serial,
    HomePath,
    Userhost,
    PublicIp,
    Bucket,
}

impl Category {
    fn enabled(self, compiled: &CompiledPrivacySettings) -> bool {
        match self {
            Self::Secrets => compiled.settings.secrets,
            Self::Email => compiled.category_enabled("email"),
            Self::PhoneCn => compiled.category_enabled("phone_cn"),
            Self::IdCardCn => compiled.category_enabled("id_card_cn"),
            Self::BankCard => compiled.category_enabled("bank_card"),
            Self::Ssn => compiled.category_enabled("ssn"),
            Self::Passport => compiled.category_enabled("passport"),
            Self::Iban => compiled.category_enabled("iban"),
            Self::Birthday => compiled.category_enabled("birthday"),
            Self::Mac => compiled.category_enabled("mac"),
            Self::Serial => compiled.category_enabled("serial"),
            Self::HomePath => compiled.category_enabled("home_path"),
            Self::Userhost => compiled.category_enabled("userhost"),
            Self::PublicIp => compiled.category_enabled("public_ip"),
            Self::Bucket => compiled.category_enabled("bucket"),
        }
    }

    fn is_secret_or_ip(self) -> bool {
        matches!(self, Self::Secrets | Self::PublicIp)
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
    AssignedSecret,
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
    kind: &'static str,
    priority: u8,
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
        kind: "SECRET",
        priority: 75,
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

static BUILTIN_RULES: LazyLock<Vec<(Spec, Regex)>> = LazyLock::new(|| {
    builtins::BUILTINS
        .iter()
        .map(|&spec| {
            let re = Regex::new(spec.pattern).expect("内置脱敏正则必须有效");
            (spec, re)
        })
        .collect()
});

#[derive(Clone, Debug)]
struct DetectedSpan {
    start: usize,
    end: usize,
    kind: String,
    priority: u8,
    action: PrivacyAction,
    is_literal_term: bool,
}

/// 跳过编码附件（如图片/音视频/二进制 base64 data URL），避免把海量编码当正文扫描。
fn is_encoded_attachment(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with("data:image/")
        || trimmed.starts_with("data:audio/")
        || trimmed.starts_with("data:video/")
    {
        return true;
    }
    trimmed
        .strip_prefix("data:")
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

pub(super) fn is_secret_field(key: &str) -> bool {
    guards::secret_field(key)
}

pub fn mask_text(
    text: &str,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
) -> MaskTextResult {
    mask_text_with_field(text, None, compiled, vault)
}

/// 对文本执行脱敏检测与处理；当 `secret_field_name` 有值时表示该字符串位于 JSON 敏感字段下，
/// 只要值本身不是变量引用或时间戳，就整值视为敏感凭据遮罩（不受长度与字符组合限制）。
pub(super) fn mask_text_with_field(
    text: &str,
    secret_field_name: Option<&str>,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
) -> MaskTextResult {
    if text.is_empty() || is_encoded_attachment(text) {
        return MaskTextResult::Unchanged;
    }

    let existing_placeholders = collect_existing_placeholders(text);
    let mut spans = Vec::new();

    detect_words(text, compiled, &mut spans);

    let whole_secret = secret_field_name
        .filter(|_| compiled.settings.secrets)
        .filter(|key| !ids::tool_lookup(key, text))
        .filter(|_| guards::secret_field_value(text));

    if let Some(field_key) = whole_secret {
        if !overlaps_fully(0, text.len(), &existing_placeholders) {
            let kind = guards::kind_for_field_name(Some(field_key), "SECRET");
            push_span(&mut spans, 0, text.len(), kind, 86, compiled, None, false);
        }
    } else {
        detect_builtins(text, compiled, &mut spans);
        detect_custom_rules(text, compiled, &mut spans);
    }

    spans.retain(|span| {
        if span.start >= span.end || span.end > text.len() {
            return false;
        }
        if !text.is_char_boundary(span.start) || !text.is_char_boundary(span.end) {
            return false;
        }
        if contained_in_any(span.start, span.end, &existing_placeholders) {
            return false;
        }
        // 自定义保密词优先级高于白名单；内置规则与自定义正则受白名单放行。
        if !span.is_literal_term && compiled.is_allowed(&text[span.start..span.end]) {
            return false;
        }
        true
    });

    if spans.is_empty() {
        return MaskTextResult::Unchanged;
    }

    // 优先检查拦截动作：只要命中任一 Block 规则立即拒绝，避免泄漏明文。
    if let Some(blocked) = spans.iter().find(|s| s.action == PrivacyAction::Block) {
        let preview = safe_preview(&text[blocked.start..blocked.end]);
        let kind_label = super::rules::kind_label_zh(&blocked.kind);
        let msg = format!(
            "隐私脱敏已拦截此请求：检测到{kind_label}（{preview}），按当前安全策略禁止发送。"
        );
        let hits = summarize_hits(&spans);
        return MaskTextResult::Handled {
            text: None,
            hits,
            blocked_message: Some(msg),
        };
    }

    // 将重叠的遮罩区间合并为连续区间，防止短保密词与长密钥重叠时漏出半截原文；
    // 仅提醒（Warn）区间保留统计但不改写文本。
    let mut mask_spans: Vec<DetectedSpan> = spans
        .iter()
        .filter(|s| s.action == PrivacyAction::Mask)
        .cloned()
        .collect();
    let warn_spans: Vec<DetectedSpan> = dedup_non_overlapping(
        spans
            .into_iter()
            .filter(|s| s.action == PrivacyAction::Warn)
            .collect(),
    );

    let merged_masks = merge_overlapping_masks(&mut mask_spans);

    let mut all_for_stats = merged_masks.clone();
    all_for_stats.extend(warn_spans);
    let hits = summarize_hits(&all_for_stats);

    if merged_masks.is_empty() {
        return MaskTextResult::Handled {
            text: None,
            hits,
            blocked_message: None,
        };
    }

    // 生成替换文本：若合并后的区间横跨了已有占位符，则把已有占位符原样保留、仅遮罩其两侧新明文。
    let mut out = String::with_capacity(text.len() + 32);
    let mut cursor = 0usize;
    for span in &merged_masks {
        out.push_str(&text[cursor..span.start]);
        let first = existing_placeholders.partition_point(|&(_, stop)| stop <= span.start);
        let mut begin = span.start;
        for &(a, b) in &existing_placeholders[first..] {
            if a >= span.end {
                break;
            }
            if begin < a {
                emit_masked_piece(&mut out, &span.kind, &text[begin..a], compiled, vault);
            }
            out.push_str(&text[begin.max(a)..span.end.min(b)]);
            begin = span.end.min(b);
        }
        if begin < span.end {
            emit_masked_piece(&mut out, &span.kind, &text[begin..span.end], compiled, vault);
        }
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);

    MaskTextResult::Handled {
        text: Some(out),
        hits,
        blocked_message: None,
    }
}

fn emit_masked_piece(
    out: &mut String,
    kind: &str,
    raw: &str,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
) {
    if compiled.settings.reversible {
        out.push_str(&vault.mint(kind, raw));
    } else {
        out.push('[');
        out.push_str(kind);
        out.push_str("_REDACTED]");
    }
}

fn merge_overlapping_masks(spans: &mut Vec<DetectedSpan>) -> Vec<DetectedSpan> {
    if spans.is_empty() {
        return Vec::new();
    }
    spans.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
    });
    let mut merged: Vec<DetectedSpan> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        match merged.last_mut() {
            Some(prev) if span.start < prev.end => {
                if span.end > prev.end {
                    prev.end = span.end;
                }
                if span.priority > prev.priority {
                    prev.kind = span.kind;
                    prev.priority = span.priority;
                }
            }
            _ => merged.push(span),
        }
    }
    merged
}

fn dedup_non_overlapping(mut spans: Vec<DetectedSpan>) -> Vec<DetectedSpan> {
    spans.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
    });
    let mut chosen: Vec<DetectedSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        if chosen.last().is_none_or(|prev| span.start >= prev.end) {
            chosen.push(span);
        }
    }
    chosen
}

fn summarize_hits(spans: &[DetectedSpan]) -> Vec<PrivacyHitRecord> {
    let mut hits_map: HashMap<(String, PrivacyAction), usize> = HashMap::new();
    for span in spans {
        *hits_map.entry((span.kind.clone(), span.action)).or_insert(0) += 1;
    }
    let mut hits: Vec<PrivacyHitRecord> = hits_map
        .into_iter()
        .map(|((kind, action), count)| PrivacyHitRecord {
            kind,
            count,
            action,
        })
        .collect();
    hits.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    hits
}

fn safe_preview(raw: &str) -> String {
    let char_len = raw.chars().count();
    if char_len < 8 {
        return format!("***，共 {char_len} 字符");
    }
    let head: String = raw.chars().take(3).collect();
    format!("{head}…，共 {char_len} 字符")
}

fn collect_existing_placeholders(text: &str) -> Vec<(usize, usize)> {
    if !text.contains("{{") {
        return Vec::new();
    }
    let mut ranges: Vec<(usize, usize)> = super::vault::placeholder_extract_regex()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();
    ranges.sort_unstable();
    ranges
}

fn contained_in_any(start: usize, end: usize, ranges: &[(usize, usize)]) -> bool {
    let idx = ranges.partition_point(|&(_, stop)| stop <= start);
    ranges
        .get(idx)
        .is_some_and(|&(s, e)| s <= start && end <= e)
}

fn overlaps_fully(start: usize, end: usize, ranges: &[(usize, usize)]) -> bool {
    contained_in_any(start, end, ranges)
}

fn captures(
    spec: Spec,
    caps: &Captures<'_>,
    mut visit: impl FnMut(Match<'_>, Guard),
) {
    match spec.capture {
        Capture::Groups(group_guards) => {
            for (index, guard) in group_guards.iter().copied().enumerate() {
                if let Some(hit) = caps.get(index + 1) {
                    visit(hit, guard);
                }
            }
        }
        Capture::Any => {
            if let Some(hit) = caps.iter().skip(1).flatten().next() {
                visit(hit, spec.check);
            }
        }
        Capture::First => {
            if let Some(hit) = caps.get(if caps.len() > 1 { 1 } else { 0 }) {
                visit(hit, spec.check);
            }
        }
    }
}

fn detect_builtins(
    text: &str,
    compiled: &CompiledPrivacySettings,
    spans: &mut Vec<DetectedSpan>,
) {
    if !(compiled.settings.secrets || compiled.settings.personal) || text.len() < 3 {
        return;
    }
    let number_arrays = guards::numeric_arrays(text);
    let lower: OnceCell<String> = OnceCell::new();

    for (spec, regex) in BUILTIN_RULES.iter() {
        if spans.len() >= MAX_MATCHES {
            break;
        }
        if !spec.category.enabled(compiled)
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
        let mut from = 0usize;
        while from < text.len() && spans.len() < MAX_MATCHES {
            let Some(caps) = regex.captures_at(text, from) else {
                break;
            };
            let Some(full) = caps.get(0) else { break };
            let full_end = full.end();
            if full_end == full.start() {
                break;
            }
            let mut resume_at = full_end;
            from = full_end;
            if spec.field {
                if let Some(hit) = caps.get(1).filter(|m| {
                    m.start() > full.start() && guards::next_field(text, m.start(), m.end())
                }) {
                    from = hit.start();
                    continue;
                }
            }
            captures(*spec, &caps, |hit, check| {
                if spans.len() >= MAX_MATCHES {
                    return;
                }
                let (a, b) = (hit.start(), hit.end());
                let repaired: Vec<(usize, usize)> = if matches!(spec.repair, Guard::SeedPhraseAt) {
                    wallet::phrases(text, a, b)
                } else if matches!(check, Guard::Luhn) {
                    guards::card_prefix(text, a, b).into_iter().collect()
                } else {
                    guards::repair(spec.repair, text, a, b)
                        .into_iter()
                        .collect()
                };
                for (a, b) in repaired {
                    if spans.len() >= MAX_MATCHES {
                        break;
                    }
                    if (matches!(spec.repair, Guard::IbanAt) || matches!(check, Guard::Luhn))
                        && b < resume_at
                        && b > full.start()
                    {
                        resume_at = b;
                    }
                    if a >= b || guards::adjacent(text, a, b, spec.bound) {
                        continue;
                    }
                    let value = &text[a..b];
                    if !guards::accept(check, value)
                        || !guards::context(spec.context, text, a, b)
                        || id_filter.keep(spec.id_rule, text, a, b)
                        || (!spec.category.is_secret_or_ip()
                            && guards::part_of_number_or_id(text, a, b, &number_arrays))
                    {
                        continue;
                    }
                    let kind = if spec.field && spec.kind == "SECRET" {
                        guards::kind_for_field_name(guards::field_name(text, a), spec.kind)
                    } else {
                        spec.kind
                    };
                    push_span(spans, a, b, kind, spec.priority, compiled, None, false);
                }
            });
            if resume_at < full_end && resume_at > full.start() {
                from = resume_at;
            }
        }
    }
}

fn detect_custom_rules(
    text: &str,
    compiled: &CompiledPrivacySettings,
    spans: &mut Vec<DetectedSpan>,
) {
    for rule in &compiled.custom_rules {
        for caps in rule.regex.captures_iter(text) {
            if spans.len() >= MAX_MATCHES {
                return;
            }
            let m = caps.get(1).or_else(|| caps.get(0));
            if let Some(m) = m {
                if m.start() < m.end() {
                    push_span(
                        spans,
                        m.start(),
                        m.end(),
                        &rule.kind,
                        80,
                        compiled,
                        rule.action,
                        false,
                    );
                }
            }
        }
    }
}

fn detect_words(
    text: &str,
    compiled: &CompiledPrivacySettings,
    spans: &mut Vec<DetectedSpan>,
) {
    for term in &compiled.compiled_terms {
        for (start, end) in term.spans(text, compiled.settings.words_whole_word) {
            if spans.len() >= MAX_MATCHES {
                return;
            }
            push_span(spans, start, end, "TERM", 78, compiled, None, true);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_span(
    spans: &mut Vec<DetectedSpan>,
    start: usize,
    end: usize,
    kind: &str,
    priority: u8,
    compiled: &CompiledPrivacySettings,
    rule_action: Option<PrivacyAction>,
    is_literal_term: bool,
) {
    spans.push(DetectedSpan {
        start,
        end,
        kind: kind.to_string(),
        priority,
        action: compiled.action_for(kind, rule_action),
        is_literal_term,
    });
}
