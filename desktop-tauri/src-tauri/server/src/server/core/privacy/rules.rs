//! 规则与配置模型：含 14 类个人信息开关、双模式占位符配置、自定义规则与日期多格式展开。
//! 日期变体展开参考 yetone/magpie（MIT，见 licenses/magpie-MIT）。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::detectors::{month, month_name};

pub const MAX_WORDS: usize = 200;
pub const MAX_WORD_CHARS: usize = 256;
pub const MAX_ALLOWLIST: usize = 200;
pub const MAX_CUSTOM_RULES: usize = 50;
pub const MAX_NAME_CHARS: usize = 128;
pub const MAX_KIND_CHARS: usize = 24;
pub const MAX_PREFIX_CHARS: usize = 64;
pub const MIN_PREFIX_CHARS: usize = 3;
pub const MAX_REGEX_CHARS: usize = 500;
pub const MAX_PROVIDERS: usize = 200;
pub const MAX_TEST_TEXT_BYTES: usize = 32 * 1024;

const PREFIX_TAIL_PATTERN: &str = r"[A-Za-z0-9_\-]{8,}";

/// 个人信息子类目录：(id, 展示名, 默认开关)。
/// 保留原有 6 个 id 不变以兼容历史配置；新增 8 类按风险高低分层设定默认值。
pub const PERSONAL_CATEGORIES: &[(&str, &str, bool)] = &[
    ("email", "邮箱地址", true),
    ("phone_cn", "中国大陆手机号", true),
    ("id_card_cn", "中国身份证号", true),
    ("bank_card", "银行卡号", true),
    ("ssn", "美国社保号 (SSN)", true),
    ("passport", "护照号", true),
    ("iban", "国际银行账号 (IBAN)", true),
    ("birthday", "出生日期", true),
    ("mac", "MAC 地址", true),
    ("serial", "设备序列号", true),
    ("public_ip", "公网 IP 地址 (v4/v6)", false),
    ("home_path", "本机主目录用户名", false),
    ("userhost", "登录名与主机名", false),
    ("bucket", "云存储桶名称", false),
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyAction {
    #[default]
    Mask,
    Warn,
    Block,
}

impl PrivacyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mask => "mask",
            Self::Warn => "warn",
            Self::Block => "block",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyProviderMode {
    #[default]
    Exclude,
    Include,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CustomRuleMatchType {
    #[default]
    Prefix,
    Regex,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrivacyCustomRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_custom_kind")]
    pub kind: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub match_type: CustomRuleMatchType,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<PrivacyAction>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PrivacySettings {
    pub enabled: bool,
    pub reversible: bool,
    /// 开启后每次请求独立生成隐藏类型的随机占位符（`{{A2A_<32hex>}}`），请求结束即销毁映射；
    /// 关闭时使用确定性占位符（`{{KIND_xxxxxxxx}}`）以保护上游提示词缓存与多轮还原。
    #[serde(default)]
    pub ephemeral_placeholders: bool,
    pub action: PrivacyAction,
    pub secrets: bool,
    pub personal: bool,
    pub categories: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kind_actions: BTreeMap<String, PrivacyAction>,
    pub words: Vec<String>,
    pub words_ignore_case: bool,
    pub words_whole_word: bool,
    pub custom_rules: Vec<PrivacyCustomRule>,
    pub allowlist: Vec<String>,
    pub provider_mode: PrivacyProviderMode,
    pub providers: Vec<String>,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            reversible: true,
            ephemeral_placeholders: false,
            action: PrivacyAction::Mask,
            secrets: true,
            personal: false,
            categories: default_categories(),
            kind_actions: BTreeMap::new(),
            words: Vec::new(),
            words_ignore_case: false,
            words_whole_word: false,
            custom_rules: Vec::new(),
            allowlist: Vec::new(),
            provider_mode: PrivacyProviderMode::Exclude,
            providers: Vec::new(),
        }
    }
}

pub fn default_categories() -> BTreeMap<String, bool> {
    PERSONAL_CATEGORIES
        .iter()
        .map(|(id, _, default_on)| ((*id).to_string(), *default_on))
        .collect()
}

fn default_custom_kind() -> String {
    "CUSTOM".to_string()
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Debug)]
pub(super) struct CompiledCustomRule {
    pub kind: String,
    pub regex: Regex,
    pub action: Option<PrivacyAction>,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledTerm {
    pattern: Regex,
    date: bool,
}

impl CompiledTerm {
    fn compile(word: &str, ignore_case: bool) -> Result<Self, String> {
        let date = date_variants(word);
        let expression = date.clone().unwrap_or_else(|| regex::escape(word));
        let pattern = RegexBuilder::new(&expression)
            .case_insensitive(ignore_case)
            .size_limit(512 * 1024)
            .build()
            .map_err(|err| format!("保密词规则编译失败：{err}"))?;
        Ok(Self {
            pattern,
            date: date.is_some(),
        })
    }

    pub(super) fn spans<'a>(
        &'a self,
        text: &'a str,
        whole_word: bool,
    ) -> impl Iterator<Item = (usize, usize)> + 'a {
        self.pattern
            .find_iter(text)
            .filter(move |m| {
                if self.date {
                    date_boundary(text, m.start(), m.end())
                        && (!whole_word || term_whole_word(text, m.start(), m.end()))
                } else {
                    !whole_word || term_whole_word(text, m.start(), m.end())
                }
            })
            .map(|m| (m.start(), m.end()))
    }
}

#[derive(Clone, Debug)]
pub struct CompiledPrivacySettings {
    pub settings: PrivacySettings,
    pub(super) custom_rules: Vec<CompiledCustomRule>,
    pub(super) allowlist_lower: HashSet<String>,
    pub(super) compiled_terms: Vec<CompiledTerm>,
}

impl Default for CompiledPrivacySettings {
    fn default() -> Self {
        Self::compile(PrivacySettings::default()).expect("默认隐私脱敏配置必须有效")
    }
}

impl CompiledPrivacySettings {
    pub fn compile(mut settings: PrivacySettings) -> Result<Self, String> {
        let mut categories = default_categories();
        for (key, value) in &settings.categories {
            if categories.contains_key(key) {
                categories.insert(key.clone(), *value);
            }
        }
        settings.categories = categories;

        let mut normalized_kind_actions = BTreeMap::new();
        for (kind, action) in &settings.kind_actions {
            let norm = normalize_kind(kind);
            normalized_kind_actions.insert(norm, *action);
        }
        settings.kind_actions = normalized_kind_actions;

        settings.words = normalize_term_list(settings.words, MAX_WORDS, MAX_WORD_CHARS, "保密词")?;
        settings.allowlist =
            normalize_term_list(settings.allowlist, MAX_ALLOWLIST, MAX_WORD_CHARS, "白名单")?;

        if settings.providers.len() > MAX_PROVIDERS {
            return Err(format!("适用提供商最多 {MAX_PROVIDERS} 个"));
        }
        let mut providers: Vec<String> = settings
            .providers
            .into_iter()
            .map(|id| id.trim().to_ascii_lowercase())
            .filter(|id| !id.is_empty())
            .collect();
        providers.sort();
        providers.dedup();
        settings.providers = providers;

        if settings.custom_rules.len() > MAX_CUSTOM_RULES {
            return Err(format!("自定义识别规则最多 {MAX_CUSTOM_RULES} 条"));
        }
        let mut rule_ids = BTreeSet::new();
        let mut custom_rules = Vec::new();
        for (index, rule) in settings.custom_rules.iter_mut().enumerate() {
            let prefix = format!("第 {} 条自定义规则", index + 1);
            rule.id = rule.id.trim().to_string();
            if rule.id.is_empty()
                || rule.id.len() > 64
                || !rule
                    .id
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
            {
                return Err(format!(
                    "{prefix}的 id 必须是 1–64 位字母、数字、连字符或下划线"
                ));
            }
            if !rule_ids.insert(rule.id.clone()) {
                return Err(format!("自定义识别规则 id 重复：{}", rule.id));
            }
            rule.name = rule.name.trim().to_string();
            if rule.name.is_empty() || rule.name.chars().count() > MAX_NAME_CHARS {
                return Err(format!("{prefix}的名称必须是 1–{MAX_NAME_CHARS} 个字符"));
            }
            rule.kind = normalize_kind(&rule.kind);
            rule.pattern = rule.pattern.trim().to_string();
            let regex = match rule.match_type {
                CustomRuleMatchType::Prefix => {
                    let len = rule.pattern.chars().count();
                    if !(MIN_PREFIX_CHARS..=MAX_PREFIX_CHARS).contains(&len) {
                        return Err(format!(
                            "{prefix}的前缀长度必须在 {MIN_PREFIX_CHARS}–{MAX_PREFIX_CHARS} 个字符之间"
                        ));
                    }
                    let expr = format!("{}{}", regex::escape(&rule.pattern), PREFIX_TAIL_PATTERN);
                    RegexBuilder::new(&expr)
                        .size_limit(512 * 1024)
                        .build()
                        .map_err(|err| format!("{prefix}的前缀编译失败：{err}"))?
                }
                CustomRuleMatchType::Regex => {
                    let len = rule.pattern.chars().count();
                    if len == 0 || len > MAX_REGEX_CHARS {
                        return Err(format!(
                            "{prefix}的正则表达式长度必须在 1–{MAX_REGEX_CHARS} 个字符之间"
                        ));
                    }
                    let compiled = RegexBuilder::new(&rule.pattern)
                        .size_limit(1024 * 1024)
                        .build()
                        .map_err(|err| format!("{prefix}的正则表达式无效：{err}"))?;
                    if compiled.is_match("") {
                        return Err(format!("{prefix}的正则表达式不能匹配空文本"));
                    }
                    compiled
                }
            };
            if rule.enabled {
                custom_rules.push(CompiledCustomRule {
                    kind: rule.kind.clone(),
                    regex,
                    action: rule.action,
                });
            }
        }

        let allowlist_lower = settings
            .allowlist
            .iter()
            .map(|item| item.to_lowercase())
            .collect();
        let mut sorted_words = settings.words.clone();
        sorted_words.sort_by(|left, right| right.chars().count().cmp(&left.chars().count()));
        let mut compiled_terms = Vec::with_capacity(sorted_words.len());
        for word in &sorted_words {
            if !word.is_empty() {
                compiled_terms.push(CompiledTerm::compile(word, settings.words_ignore_case)?);
            }
        }

        Ok(Self {
            settings,
            custom_rules,
            allowlist_lower,
            compiled_terms,
        })
    }

    pub fn applies_to(&self, provider_id: &str) -> bool {
        if !self.settings.enabled {
            return false;
        }
        match self.settings.provider_mode {
            PrivacyProviderMode::Exclude => !self
                .settings
                .providers
                .iter()
                .any(|id| id.eq_ignore_ascii_case(provider_id)),
            PrivacyProviderMode::Include => {
                self.settings.providers.is_empty()
                    || self
                        .settings
                        .providers
                        .iter()
                        .any(|id| id.eq_ignore_ascii_case(provider_id))
            }
        }
    }

    pub fn category_enabled(&self, id: &str) -> bool {
        self.settings.personal && self.settings.categories.get(id).copied().unwrap_or(false)
    }

    pub fn action_for(&self, kind: &str, rule_action: Option<PrivacyAction>) -> PrivacyAction {
        rule_action
            .or_else(|| self.settings.kind_actions.get(kind).copied())
            .unwrap_or(self.settings.action)
    }

    pub fn is_allowed(&self, value: &str) -> bool {
        self.allowlist_lower.contains(&value.to_lowercase())
    }
}

fn normalize_term_list(
    raw: Vec<String>,
    max_items: usize,
    max_chars: usize,
    label: &str,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in raw {
        let trimmed = item.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > max_chars {
            return Err(format!("{label}单条最多 {max_chars} 个字符"));
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            out.push(trimmed);
        }
    }
    if out.len() > max_items {
        return Err(format!("{label}最多 {max_items} 条"));
    }
    Ok(out)
}

pub fn normalize_kind(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else if matches!(ch, '_' | '-' | ' ' | '.') {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        return "CUSTOM".to_string();
    }
    if !out.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        out = format!("CUSTOM_{out}");
    }
    if out.len() > MAX_KIND_CHARS {
        out.truncate(MAX_KIND_CHARS);
        while out.ends_with('_') {
            out.pop();
        }
    }
    if out.is_empty() {
        "CUSTOM".to_string()
    } else {
        out
    }
}

pub fn category_catalog() -> Vec<Value> {
    PERSONAL_CATEGORIES
        .iter()
        .map(|(id, label, default_on)| {
            json!({
                "id": id,
                "label": label,
                "defaultOn": default_on,
            })
        })
        .collect()
}

pub fn kind_label_zh(kind: &str) -> &str {
    match kind {
        "API_KEY" => "API 密钥",
        "PRIVATE_KEY" => "私钥内容",
        "JWT" => "JWT 令牌",
        "PASSWORD" => "密码凭证",
        "SECRET" => "机密凭据",
        "WALLET_KEY" => "钱包私钥",
        "SEED_PHRASE" => "钱包助记词",
        "EMAIL" => "邮箱地址",
        "PHONE_CN" => "中国大陆手机号",
        "ID_CARD_CN" => "中国身份证号",
        "BANK_CARD" => "银行卡号",
        "SSN" => "美国社保号",
        "PASSPORT" => "护照号",
        "IBAN" => "国际银行账号",
        "BIRTHDAY" => "出生日期",
        "MAC" => "MAC 地址",
        "SERIAL" => "设备序列号",
        "PUBLIC_IP" => "公网 IP 地址",
        "HOME_PATH" => "主目录用户名",
        "USERHOST" => "登录名/主机名",
        "BUCKET" => "云存储桶",
        "TERM" => "自定义保密词",
        "CUSTOM" => "自定义规则",
        other => other,
    }
}

// ─── 日期词变体展开（Magpie MIT）────────────────────────────────

const DATE_SEP: &str = r"(?:[ \t]*[-/.、／－][ \t]*|[ \t])";
const DATE_ORD: &str = r"(?:st|nd|rd|th)?";

static DATE_FORMATS: LazyLock<[Regex; 5]> = LazyLock::new(|| {
    [
        format!(r"^([0-9]{{4}}){DATE_SEP}([0-9]{{1,2}}){DATE_SEP}([0-9]{{1,2}})\.?$"),
        r"^([0-9]{4})[ \t]*[年년][ \t]*([0-9]{1,2})[ \t]*[月월][ \t]*([0-9]{1,2})[ \t]*[日号號일]?$"
            .to_owned(),
        format!(r"^([0-9]{{1,2}}){DATE_SEP}([0-9]{{1,2}}){DATE_SEP}([0-9]{{4}})$"),
        format!(r"^([A-Za-z]+)\.?[ \t]+([0-9]{{1,2}}){DATE_ORD},?[ \t]+([0-9]{{4}})$"),
        format!(
            r"^([0-9]{{1,2}}){DATE_ORD}(?:[ \t]+|-)([A-Za-z]+)\.?,?(?:[ \t]+|-)([0-9]{{4}})$"
        ),
    ]
    .map(|pattern| Regex::new(&pattern).expect("日期识别正则必须有效"))
});

fn parse_date_word(s: &str) -> Option<(i32, u32, u32)> {
    for (i, re) in DATE_FORMATS.iter().enumerate() {
        let Some(m) = re.captures(s) else { continue };
        let (a, b, c) = (&m[1], &m[2], &m[3]);
        let number = |part: &str| part.parse::<u32>().unwrap_or(0);
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

fn date_variants(word: &str) -> Option<String> {
    let (y, m, d) = parse_date_word(word)?;
    let name = month_name(m);
    let short = if m == 9 { "Sept|Sep" } else { &name[..3] };
    let named = format!(r"(?i:{name}|(?:{short})\.?)");
    let month_str = if m < 10 {
        format!("0?{m}")
    } else {
        m.to_string()
    };
    let day_str = if d < 10 {
        format!("0?{d}")
    } else {
        d.to_string()
    };
    let mut forms = vec![
        format!("{y}{DATE_SEP}{month_str}{DATE_SEP}{day_str}"),
        format!(r"{y}[ \t]*年[ \t]*{month_str}[ \t]*月[ \t]*{day_str}(?:[ \t]*[日号號])?"),
        format!(r"{y}[ \t]*년[ \t]*{month_str}[ \t]*월[ \t]*{day_str}(?:[ \t]*일)?"),
        format!("{y}{m:02}{d:02}"),
        format!(r"{named}[ \t]+{day_str}{DATE_ORD},?[ \t]+{y}"),
        format!(r"{day_str}{DATE_ORD}(?:[ \t]+|-){named},?(?:[ \t]+|-){y}"),
    ];
    if d > 12 || d == m {
        forms.push(format!("{day_str}{DATE_SEP}{month_str}{DATE_SEP}{y}"));
        if d != m {
            forms.push(format!("{month_str}{DATE_SEP}{day_str}{DATE_SEP}{y}"));
        }
    }
    forms.push(regex::escape(word));
    Some(forms.join("|"))
}

fn date_boundary(s: &str, a: usize, b: usize) -> bool {
    let t = s.as_bytes();
    let digit = |i: usize| t.get(i).is_some_and(u8::is_ascii_digit);
    !(a > 0 && (digit(a - 1) || a > 1 && t[a - 1] == b'.' && digit(a - 2)))
        && !s[..a]
            .chars()
            .next_back()
            .is_some_and(|p| p.is_ascii_alphabetic() && t.get(a).is_some_and(u8::is_ascii_alphabetic))
        && !(digit(b) || t.get(b) == Some(&b'.') && digit(b + 1))
}

fn term_whole_word(s: &str, a: usize, b: usize) -> bool {
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
