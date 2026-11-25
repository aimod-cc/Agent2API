use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::OnceLock;

use regex::Regex;

use super::rules::{CompiledPrivacySettings, PrivacyAction};
use super::vault::PrivacyVault;

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

#[derive(Clone, Debug)]
struct DetectedSpan {
    start: usize,
    end: usize,
    kind: String,
    priority: u8,
    action: PrivacyAction,
}

struct SecretPatterns {
    pem_private_key: Regex,
    known_api_keys: Regex,
    jwt: Regex,
    wallet_prefix: Regex,
    wallet_hex: Regex,
    dsn_password: Regex,
    auth_header: Regex,
    kv_secret: Regex,
    cjk_secret: Regex,
}

struct PersonalPatterns {
    email: Regex,
    phone_cn: Regex,
    id_card_cn: Regex,
    bank_card: Regex,
    ipv4: Regex,
    home_path: Regex,
}

fn secret_patterns() -> &'static SecretPatterns {
    static PATS: OnceLock<SecretPatterns> = OnceLock::new();
    PATS.get_or_init(|| SecretPatterns {
        pem_private_key: Regex::new(
            r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]{16,}?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        )
        .expect("PEM 私钥正则必须有效"),
        known_api_keys: Regex::new(
            r"(?:sk-proj-|sk-ant-api\d*-|sk-ant-|sk-or-v1-|sk-svcacct-)[A-Za-z0-9_\-]{16,}|(?:^|[^A-Za-z0-9_\-])(sk-[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{22,}|gh[pousr]_[A-Za-z0-9]{20,}|glpat-[A-Za-z0-9_\-]{20,}|AIza[0-9A-Za-z_\-]{35}|(?:AKIA|ASIA)[0-9A-Z]{16}|xox[baprs]-[0-9A-Za-z\-]{10,}|[sr]k_live_[0-9A-Za-z]{16,}|hf_[A-Za-z0-9]{20,}|gsk_[A-Za-z0-9]{20,}|xai-[A-Za-z0-9]{20,}|npm_[A-Za-z0-9]{20,}|pypi-[A-Za-z0-9_\-]{20,}|SG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}|dop_v1_[a-f0-9]{64}|cf(?:ut|at|fk|oat)_[A-Za-z0-9_\-]{20,})",
        )
        .expect("API Key 正则必须有效"),
        jwt: Regex::new(r"eyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}")
            .expect("JWT 正则必须有效"),
        wallet_prefix: Regex::new(r"(?:suiprivkey1|nsec1)[0-9a-z]{30,}")
            .expect("钱包前缀私钥正则必须有效"),
        wallet_hex: Regex::new(
            r"(?i)(?:private[_-]?key|私钥|私鑰)[^0-9a-fA-Fx\r\n]{0,16}(0x[0-9a-fA-F]{64})",
        )
        .expect("十六进制私钥正则必须有效"),
        dsn_password: Regex::new(
            r#"(?i)\b(?:postgres|postgresql|mysql|mariadb|mongodb(?:\+srv)?|redis|rediss|amqp|amqps|mssql|ftp|socks5)://[^:\s/@"']+:([^@\s/"']+)@"#,
        )
        .expect("连接串密码正则必须有效"),
        auth_header: Regex::new(
            r"(?im)^[ \t]*(?:authorization|proxy-authorization|x-api-key|cookie)[ \t]*:[ \t]*(?:(?:Bearer|Basic|Token)[ \t]+)?(\S[^\r\n]{7,})",
        )
        .expect("认证头正则必须有效"),
        kv_secret: Regex::new(
            r#"(?i)(?:^|[^A-Za-z0-9_.\-])(?:[A-Za-z0-9_.\-]*_)?(password|passwd|passphrase|secret|client_secret|api_key|apikey|access_key|private_key|auth_token|access_token|refresh_token|credential)(?:\\?["'])?[ \t]*[:=][ \t]*(?:\\?["'])?([A-Za-z0-9_\-./+=~!@#%^&*]{8,})"#,
        )
        .expect("键值凭证正则必须有效"),
        cjk_secret: Regex::new(
            r#"(?:密码|密碼|口令|密钥|密鑰|私钥|私鑰|令牌)[ \t]*(?:为|為|是|[:：=])[ \t]*["'“「]?([A-Za-z0-9_\-./+=~!@#%^&*]{6,})"#,
        )
        .expect("中文凭证正则必须有效"),
    })
}

fn personal_patterns() -> &'static PersonalPatterns {
    static PATS: OnceLock<PersonalPatterns> = OnceLock::new();
    PATS.get_or_init(|| PersonalPatterns {
        email: Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,24}")
            .expect("邮箱正则必须有效"),
        phone_cn: Regex::new(r"1[3-9]\d{9}").expect("手机号正则必须有效"),
        id_card_cn: Regex::new(r"[1-6]\d{16}[0-9Xx]").expect("身份证正则必须有效"),
        bank_card: Regex::new(r"(?:\d[ \-]?){12,18}\d").expect("银行卡正则必须有效"),
        ipv4: Regex::new(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}").expect("IPv4 正则必须有效"),
        home_path: Regex::new(
            r"(?:/Users/|/home/|[A-Za-z]:\\\\Users\\\\|[A-Za-z]:\\Users\\|[A-Za-z]:/Users/)([A-Za-z0-9._\-]{2,64})",
        )
        .expect("主目录路径正则必须有效"),
    })
}

fn is_media_data_url(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("data:image/")
        || trimmed.starts_with("data:audio/")
        || trimmed.starts_with("data:video/")
}

pub fn mask_text(
    text: &str,
    compiled: &CompiledPrivacySettings,
    vault: &PrivacyVault,
) -> MaskTextResult {
    if text.is_empty() || is_media_data_url(text) {
        return MaskTextResult::Unchanged;
    }

    let existing_placeholders = collect_existing_placeholders(text);
    let mut spans = Vec::new();

    if compiled.settings.secrets {
        detect_secrets(text, compiled, &mut spans);
    }
    if compiled.settings.personal {
        detect_personal(text, compiled, &mut spans);
    }
    detect_custom_rules(text, compiled, &mut spans);
    detect_words(text, compiled, &mut spans);

    spans.retain(|span| {
        if span.start >= span.end || span.end > text.len() {
            return false;
        }
        if !text.is_char_boundary(span.start) || !text.is_char_boundary(span.end) {
            return false;
        }
        if overlaps_any(span.start, span.end, &existing_placeholders) {
            return false;
        }
        let slice = &text[span.start..span.end];
        if compiled.is_allowed(slice) {
            return false;
        }
        true
    });

    if spans.is_empty() {
        return MaskTextResult::Unchanged;
    }

    spans.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
    });

    let mut chosen: Vec<DetectedSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        let overlaps = chosen
            .last()
            .is_some_and(|prev| span.start < prev.end);
        if !overlaps {
            chosen.push(span);
        }
    }

    let mut hits_map: HashMap<(String, PrivacyAction), usize> = HashMap::new();
    let mut blocked_info: Option<(String, String)> = None;
    for span in &chosen {
        *hits_map.entry((span.kind.clone(), span.action)).or_insert(0) += 1;
        if span.action == PrivacyAction::Block && blocked_info.is_none() {
            let preview = safe_preview(&text[span.start..span.end]);
            blocked_info = Some((span.kind.clone(), preview));
        }
    }

    let mut hits: Vec<PrivacyHitRecord> = hits_map
        .into_iter()
        .map(|((kind, action), count)| PrivacyHitRecord {
            kind,
            count,
            action,
        })
        .collect();
    hits.sort_by(|left, right| right.count.cmp(&left.count).then_with(|| left.kind.cmp(&right.kind)));

    if let Some((kind, preview)) = blocked_info {
        let kind_label = super::rules::kind_label_zh(&kind);
        let msg = format!(
            "隐私脱敏已拦截此请求：检测到{kind_label}（{preview}），按当前安全策略禁止发送。"
        );
        return MaskTextResult::Handled {
            text: None,
            hits,
            blocked_message: Some(msg),
        };
    }

    let has_mask = chosen.iter().any(|span| span.action == PrivacyAction::Mask);
    if !has_mask {
        return MaskTextResult::Handled {
            text: None,
            hits,
            blocked_message: None,
        };
    }

    let mut out = String::with_capacity(text.len() + 32);
    let mut cursor = 0usize;
    for span in &chosen {
        out.push_str(&text[cursor..span.start]);
        let raw = &text[span.start..span.end];
        if span.action == PrivacyAction::Mask {
            if compiled.settings.reversible {
                out.push_str(&vault.mint(&span.kind, raw));
            } else {
                out.push('[');
                out.push_str(&span.kind);
                out.push_str("_REDACTED]");
            }
        } else {
            out.push_str(raw);
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
    super::vault::placeholder_extract_regex()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn overlaps_any(start: usize, end: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|(s, e)| start < *e && end > *s)
}

fn detect_secrets(
    text: &str,
    compiled: &CompiledPrivacySettings,
    spans: &mut Vec<DetectedSpan>,
) {
    let pats = secret_patterns();

    for m in pats.pem_private_key.find_iter(text) {
        push_span(spans, m.start(), m.end(), "PRIVATE_KEY", 95, compiled, None);
    }

    for caps in pats.known_api_keys.captures_iter(text) {
        let m = caps.get(1).or_else(|| caps.get(0));
        if let Some(m) = m {
            if is_word_token_boundary(text, m.start(), m.end()) {
                push_span(spans, m.start(), m.end(), "API_KEY", 90, compiled, None);
            }
        }
    }

    for m in pats.jwt.find_iter(text) {
        if is_word_token_boundary(text, m.start(), m.end()) {
            push_span(spans, m.start(), m.end(), "JWT", 88, compiled, None);
        }
    }

    for m in pats.wallet_prefix.find_iter(text) {
        if is_word_token_boundary(text, m.start(), m.end()) {
            push_span(spans, m.start(), m.end(), "WALLET_KEY", 88, compiled, None);
        }
    }

    for caps in pats.wallet_hex.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            if is_word_token_boundary(text, m.start(), m.end()) {
                push_span(spans, m.start(), m.end(), "WALLET_KEY", 88, compiled, None);
            }
        }
    }

    for caps in pats.dsn_password.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            let val = m.as_str();
            if not_placeholder_var(val) {
                push_span(spans, m.start(), m.end(), "PASSWORD", 85, compiled, None);
            }
        }
    }

    for caps in pats.auth_header.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            let val = m.as_str().trim_end_matches(|c: char| c == '"' || c == '\'');
            if val.len() >= 8 && not_placeholder_var(val) {
                let end = m.start() + val.len();
                push_span(spans, m.start(), end, "SECRET", 82, compiled, None);
            }
        }
    }

    for caps in pats.kv_secret.captures_iter(text) {
        let Some(key_m) = caps.get(1) else { continue };
        let Some(val_m) = caps.get(2) else { continue };
        let val = val_m.as_str();
        if !is_secret_candidate(text, val_m.start(), val_m.end(), val) {
            continue;
        }
        let key_lower = key_m.as_str().to_ascii_lowercase();
        let kind = if key_lower.contains("pass") {
            "PASSWORD"
        } else if key_lower.contains("key") {
            "API_KEY"
        } else {
            "SECRET"
        };
        push_span(spans, val_m.start(), val_m.end(), kind, 75, compiled, None);
    }

    for caps in pats.cjk_secret.captures_iter(text) {
        let Some(val_m) = caps.get(1) else { continue };
        let val = val_m.as_str();
        if !is_cjk_secret_candidate(text, val_m.start(), val_m.end(), val) {
            continue;
        }
        let prefix = &text[caps.get(0).map_or(val_m.start(), |m| m.start())..val_m.start()];
        let kind = if prefix.contains("密码") || prefix.contains("密碼") || prefix.contains("口令") {
            "PASSWORD"
        } else if prefix.contains("私钥") || prefix.contains("私鑰") || prefix.contains("密钥") || prefix.contains("密鑰") {
            "API_KEY"
        } else {
            "SECRET"
        };
        push_span(spans, val_m.start(), val_m.end(), kind, 74, compiled, None);
    }
}

fn detect_personal(
    text: &str,
    compiled: &CompiledPrivacySettings,
    spans: &mut Vec<DetectedSpan>,
) {
    let pats = personal_patterns();

    if compiled.category_enabled("email") {
        for m in pats.email.find_iter(text) {
            if is_valid_email_match(text, m.start(), m.end()) {
                push_span(spans, m.start(), m.end(), "EMAIL", 68, compiled, None);
            }
        }
    }

    if compiled.category_enabled("id_card_cn") {
        for m in pats.id_card_cn.find_iter(text) {
            if is_word_token_boundary(text, m.start(), m.end()) && is_valid_cn_id(m.as_str()) {
                push_span(spans, m.start(), m.end(), "ID_CARD_CN", 70, compiled, None);
            }
        }
    }

    if compiled.category_enabled("bank_card") {
        for m in pats.bank_card.find_iter(text) {
            if is_valid_bank_card_match(text, m.start(), m.end()) {
                push_span(spans, m.start(), m.end(), "BANK_CARD", 66, compiled, None);
            }
        }
    }

    if compiled.category_enabled("phone_cn") {
        for m in pats.phone_cn.find_iter(text) {
            if is_valid_cn_phone_match(text, m.start(), m.end()) {
                push_span(spans, m.start(), m.end(), "PHONE_CN", 64, compiled, None);
            }
        }
    }

    if compiled.category_enabled("public_ip") {
        for m in pats.ipv4.find_iter(text) {
            if is_valid_public_ip_match(text, m.start(), m.end()) {
                push_span(spans, m.start(), m.end(), "PUBLIC_IP", 62, compiled, None);
            }
        }
    }

    if compiled.category_enabled("home_path") {
        for caps in pats.home_path.captures_iter(text) {
            let Some(m) = caps.get(1) else { continue };
            let name = m.as_str();
            if !is_generic_home_user(name) {
                push_span(spans, m.start(), m.end(), "HOME_PATH", 60, compiled, None);
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
    if compiled.sorted_words.is_empty() {
        return;
    }
    let text_lower = if compiled.settings.words_ignore_case {
        Some(text.to_ascii_lowercase())
    } else {
        None
    };

    for word in &compiled.sorted_words {
        if word.is_empty() {
            continue;
        }
        if let Some(lower) = text_lower.as_deref() {
            let needle = word.to_ascii_lowercase();
            // 仅当 ASCII 大小写折叠后长度一致时用字节偏移对齐；否则退回精确字符匹配。
            if lower.len() == text.len() && needle.len() == word.len() {
                let mut from = 0usize;
                while let Some(pos) = lower[from..].find(&needle) {
                    let start = from + pos;
                    let end = start + needle.len();
                    if text.is_char_boundary(start)
                        && text.is_char_boundary(end)
                        && (!compiled.settings.words_whole_word
                            || is_word_token_boundary(text, start, end))
                    {
                        push_span(spans, start, end, "TERM", 78, compiled, None);
                    }
                    from = start + needle.len().max(1);
                }
                continue;
            }
        }

        let mut from = 0usize;
        while let Some(pos) = text[from..].find(word.as_str()) {
            let start = from + pos;
            let end = start + word.len();
            if !compiled.settings.words_whole_word || is_word_token_boundary(text, start, end) {
                push_span(spans, start, end, "TERM", 78, compiled, None);
            }
            from = start + word.len().max(1);
        }
    }
}

fn push_span(
    spans: &mut Vec<DetectedSpan>,
    start: usize,
    end: usize,
    kind: &str,
    priority: u8,
    compiled: &CompiledPrivacySettings,
    rule_action: Option<PrivacyAction>,
) {
    spans.push(DetectedSpan {
        start,
        end,
        kind: kind.to_string(),
        priority,
        action: compiled.action_for(kind, rule_action),
    });
}

fn is_word_token_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].as_bytes().last().copied();
    let after = text[end..].as_bytes().first().copied();
    let ok_before = before.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_'));
    let ok_after = after.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_'));
    ok_before && ok_after
}

fn not_placeholder_var(val: &str) -> bool {
    let Some(first) = val.chars().next() else {
        return false;
    };
    if matches!(first, '$' | '%' | '{' | '<' | '[' | '*') {
        return false;
    }
    if val.starts_with("process.env") || val.starts_with("os.") || val.starts_with("env.") {
        return false;
    }
    !val.trim_matches(['*', 'x', 'X', '.']).is_empty()
}

fn looks_like_path_or_version(val: &str) -> bool {
    if val.starts_with('/') || val.starts_with("~/") || val.starts_with("./") {
        return true;
    }
    if val.len() >= 10 && val.as_bytes().get(4) == Some(&b'-') && val.as_bytes().get(7) == Some(&b'-') {
        return true;
    }
    val.split('.').count() >= 3 && val.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

fn is_code_call_or_ident(text: &str, end: usize, val: &str) -> bool {
    let rest = text[end..].trim_start_matches([' ', '\t']);
    if rest.starts_with('(') {
        return true;
    }
    let is_ident = !val.is_empty()
        && val
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
    // 纯标识符（含字母但无数字且长度 < 16）通常是类型名或字段引用，不是真实密钥。
    is_ident && !val.bytes().any(|b| b.is_ascii_digit()) && val.len() < 16
}

fn is_secret_candidate(text: &str, start: usize, end: usize, val: &str) -> bool {
    if !not_placeholder_var(val) || looks_like_path_or_version(val) || is_code_call_or_ident(text, end, val) {
        return false;
    }
    let has_letter = val.bytes().any(|b| b.is_ascii_alphabetic());
    let has_digit = val.bytes().any(|b| b.is_ascii_digit());
    let _ = start;
    (has_letter && has_digit) || val.len() >= 16
}

fn is_cjk_secret_candidate(text: &str, start: usize, end: usize, val: &str) -> bool {
    if !not_placeholder_var(val) || looks_like_path_or_version(val) {
        return false;
    }
    let after = text[end..].trim_start();
    const UNITS: &[&str] = &[
        "位", "比特", "字节", "字節", "个", "個", "次", "秒", "毫秒", "分钟", "分鐘", "小时",
        "小時", "天", "年", "bit", "byte", "ms", "sec", "min",
    ];
    if UNITS.iter().any(|u| after.starts_with(u)) {
        return false;
    }
    let _ = start;
    true
}

fn is_valid_email_match(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].as_bytes().last().copied();
    let after = text[end..].as_bytes().first().copied();
    if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'/') {
        return false;
    }
    if after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return false;
    }
    let email = &text[start..end];
    let Some((_, domain)) = email.split_once('@') else {
        return false;
    };
    let Some((_, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    !matches!(
        tld.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif" | "ico" | "css" | "js" | "ts"
    )
}

fn context_before(text: &str, start: usize, max_bytes: usize) -> String {
    let mut from = start.saturating_sub(max_bytes);
    while from > 0 && !text.is_char_boundary(from) {
        from -= 1;
    }
    text[from..start].to_ascii_lowercase()
}

fn is_valid_cn_phone_match(text: &str, start: usize, end: usize) -> bool {
    let after = text[end..].as_bytes().first().copied();
    if after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return false;
    }
    let before = text[..start].as_bytes();
    if let Some(&last) = before.last() {
        if last.is_ascii_alphanumeric() || last == b'_' {
            // 兼容 +86138... 连写写法
            let ends_86 = before.ends_with(b"+86")
                || (before.ends_with(b"86")
                    && before
                        .get(before.len().saturating_sub(3))
                        .is_none_or(|b| !b.is_ascii_alphanumeric()));
            if !ends_86 {
                return false;
            }
        }
    }
    let ctx = context_before(text, start, 48);
    !["order_id", "orderno", "order_no", "trace_id", "request_id", "span_id", "订单号", "订单编号", "运单号", "流水号", "时间戳", "timestamp"]
        .iter()
        .any(|cue| ctx.contains(cue))
}

fn is_valid_cn_id(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != 18 {
        return false;
    }
    let Ok(prov) = raw[0..2].parse::<u8>() else {
        return false;
    };
    if !matches!(
        prov,
        11..=15 | 21..=23 | 31..=37 | 41..=46 | 50..=54 | 61..=65
    ) {
        return false;
    }
    let Ok(year) = raw[6..10].parse::<u16>() else {
        return false;
    };
    let Ok(month) = raw[10..12].parse::<u8>() else {
        return false;
    };
    let Ok(day) = raw[12..14].parse::<u8>() else {
        return false;
    };
    if !(1900..=2099).contains(&year) || !(1..=12).contains(&month) {
        return false;
    }
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    if day == 0 || day > max_day {
        return false;
    }
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECKS: [u8; 11] = [b'1', b'0', b'X', b'9', b'8', b'7', b'6', b'5', b'4', b'3', b'2'];
    let mut sum = 0u32;
    for (idx, weight) in WEIGHTS.iter().enumerate() {
        let Some(digit) = bytes.get(idx).and_then(|b| b.checked_sub(b'0')) else {
            return false;
        };
        if digit > 9 {
            return false;
        }
        sum += u32::from(digit) * weight;
    }
    let expected = CHECKS[(sum % 11) as usize];
    bytes[17].to_ascii_uppercase() == expected
}

fn is_valid_bank_card_match(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].as_bytes().last().copied();
    let after = text[end..].as_bytes().first().copied();
    if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-') {
        return false;
    }
    if after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-') {
        return false;
    }
    let digits: Vec<u8> = text[start..end]
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    if digits.iter().all(|d| *d == digits[0]) {
        return false;
    }
    let ctx = context_before(text, start, 48);
    if ["order", "trace", "uuid", "timestamp", "hash", "sha", "commit", "订单", "流水"]
        .iter()
        .any(|cue| ctx.contains(cue))
    {
        return false;
    }
    luhn_valid(&digits)
}

fn luhn_valid(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut val = u32::from(d);
        if double {
            val *= 2;
            if val > 9 {
                val -= 9;
            }
        }
        sum += val;
        double = !double;
    }
    sum % 10 == 0
}

fn is_valid_public_ip_match(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].as_bytes().last().copied();
    let after = text[end..].as_bytes().first().copied();
    if before.is_some_and(|b| b.is_ascii_digit() || b == b'.' || b == b'v' || b == b'V') {
        return false;
    }
    if after.is_some_and(|b| b.is_ascii_digit() || b == b'.' || b == b'-') {
        return false;
    }
    let ctx = context_before(text, start, 24);
    if ctx.contains("version") || ctx.contains("ver") {
        return false;
    }
    let Ok(ip) = text[start..end].parse::<Ipv4Addr>() else {
        return false;
    };
    let [a, b, c, _d] = ip.octets();
    if a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
    {
        return false;
    }
    true
}

fn is_generic_home_user(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "shared"
            | "public"
            | "default"
            | "all users"
            | "administrator"
            | "root"
            | "user"
            | "runner"
            | "ubuntu"
            | "vscode"
            | "admin"
            | "guest"
            | "nobody"
    )
}
