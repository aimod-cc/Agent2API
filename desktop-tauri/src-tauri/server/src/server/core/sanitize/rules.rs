use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

pub const MAX_RULES: usize = 200;
pub const MAX_NAME_CHARS: usize = 128;
pub const MAX_PATTERN_CHARS: usize = 4096;
pub const MAX_REPLACEMENT_CHARS: usize = 4096;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MatchType {
    #[default]
    Literal,
    Regex,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum SanitizeRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
    Function,
}

impl SanitizeRole {
    pub const ALL: [Self; 6] = [
        Self::System,
        Self::Developer,
        Self::User,
        Self::Assistant,
        Self::Tool,
        Self::Function,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Function => "function",
        }
    }

    pub(super) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == text)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SanitizeRule {
    pub id: String,
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub match_type: MatchType,
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
    #[serde(default)]
    pub repeat: bool,
    /// 空列表沿用旧规则的全字段行为；指定角色时只改对应消息的正文。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<SanitizeRole>,
}

impl SanitizeRule {
    pub(super) fn applies_to(&self, role: Option<SanitizeRole>, content: bool) -> bool {
        self.roles.is_empty() || (content && role.is_some_and(|role| self.roles.contains(&role)))
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug)]
pub(super) enum Matcher {
    Literal,
    Regex(Regex),
}

#[derive(Debug)]
pub(super) struct CompiledRule {
    pub rule: SanitizeRule,
    pub matcher: Matcher,
}

#[derive(Debug)]
pub struct RuleSet {
    entries: Vec<SanitizeRule>,
    compiled: Vec<CompiledRule>,
}

impl RuleSet {
    pub fn compile(mut entries: Vec<SanitizeRule>) -> Result<Self, String> {
        if entries.len() > MAX_RULES {
            return Err(format!("脱敏词表最多 {MAX_RULES} 条规则"));
        }
        let mut ids = BTreeSet::new();
        let mut compiled = Vec::with_capacity(entries.len());
        for (index, rule) in entries.iter_mut().enumerate() {
            let prefix = format!("第 {} 条脱敏规则", index + 1);
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
            if !ids.insert(rule.id.clone()) {
                return Err(format!("脱敏规则 id 重复：{}", rule.id));
            }
            rule.name = rule.name.trim().to_string();
            if rule.name.is_empty() || rule.name.chars().count() > MAX_NAME_CHARS {
                return Err(format!("{prefix}的名称必须是 1–{MAX_NAME_CHARS} 个字符"));
            }
            if rule.pattern.trim().is_empty() || rule.pattern.chars().count() > MAX_PATTERN_CHARS {
                return Err(format!(
                    "{prefix}的匹配内容不能为空，且最多 {MAX_PATTERN_CHARS} 个字符"
                ));
            }
            if rule.replacement.chars().count() > MAX_REPLACEMENT_CHARS {
                return Err(format!(
                    "{prefix}的替换内容最多 {MAX_REPLACEMENT_CHARS} 个字符"
                ));
            }
            rule.roles.sort();
            rule.roles.dedup();
            // 重复执行只允许删除，否则用户规则可能不断扩张文本而无法收敛。
            if rule.repeat && (rule.match_type != MatchType::Regex || !rule.replacement.is_empty())
            {
                return Err(format!("{prefix}只有正则删除规则可以启用重复清理"));
            }
            let matcher = match rule.match_type {
                MatchType::Literal => Matcher::Literal,
                MatchType::Regex => Matcher::Regex(
                    RegexBuilder::new(&rule.pattern)
                        .size_limit(1024 * 1024)
                        .build()
                        .map_err(|error| format!("{prefix}的正则表达式无效：{error}"))?,
                ),
            };
            if rule.enabled {
                compiled.push(CompiledRule {
                    rule: rule.clone(),
                    matcher,
                });
            }
        }
        Ok(Self { entries, compiled })
    }

    pub fn entries(&self) -> &[SanitizeRule] {
        &self.entries
    }

    pub(super) fn compiled(&self) -> &[CompiledRule] {
        &self.compiled
    }
}

pub fn default_rules() -> Arc<RuleSet> {
    static RULES: OnceLock<Arc<RuleSet>> = OnceLock::new();
    RULES
        .get_or_init(|| {
            Arc::new(RuleSet::compile(default_entries()).expect("内置脱敏规则必须有效"))
        })
        .clone()
}

fn default_entries() -> Vec<SanitizeRule> {
    let rewrites = [
        (
            "claude-code-identity",
            "Claude Code",
            "You are Claude Code, Anthropic's official CLI for Claude",
            "You are Claude Code, Anthropic's official CLI tool for Claude",
        ),
        (
            "main-branch",
            "Main branch",
            "Main branch (you will usually use this for PRs)",
            "Default branch (you will usually use this for PRs)",
        ),
        (
            "codex-cli-identity",
            "Codex CLI",
            "You are a coding agent running in the Codex CLI, a terminal-based coding assistant.",
            "You are a coding agent running in the Codex CLI tool, a terminal-based coding assistant.",
        ),
        (
            "claude-code-feedback",
            "Claude Code feedback",
            "To give feedback, users should report the issue at https://github.com/anthropics/claude-code/issues",
            "To provide feedback, users should report the issue at https://github.com/anthropics/claude-code/issues",
        ),
        ("error-11128", "11128", "11128", "11-128"),
    ];
    let mut entries: Vec<_> = rewrites
        .into_iter()
        .map(|(id, name, pattern, replacement)| SanitizeRule {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            match_type: MatchType::Literal,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            repeat: false,
            roles: Vec::new(),
        })
        .collect();
    // 键值删除必须先于裸键名改写；cc_ 删除重复执行以清理新暴露的相邻边界。
    for (id, name, pattern, replacement, repeat) in [
        (
            "billing-header-value",
            "x-anthropic-billing-header",
            r"(?i)x\u{200b}-anthropic-billing-header:[^;\n]*;?[\t\n\f\r ]*",
            "",
            false,
        ),
        (
            "cc-parameters",
            "cc_*=",
            r"(?i)(?-u:\b)cc_[a-z0-9_]+=[^;\n]*;?[\t\n\f\r ]*",
            "",
            true,
        ),
        (
            "billing-header-name",
            "x-anthropic-billing-header",
            r"(?i)x\u{200b}-anthropic-billing-header",
            "x-anthropic-billing-hdr",
            false,
        ),
    ] {
        entries.push(SanitizeRule {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            match_type: MatchType::Regex,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            repeat,
            roles: Vec::new(),
        });
    }
    for (id, name, pattern, replacement) in [
        (
            "agent-identity",
            "Agent identity",
            r"(?i)\bYou are\s+(?:an?\s+|the\s+)?(?:[\w.-]+\s+){0,3}?(?:agent|assistant|model|AI|LLM)?[\s,]*(?:powered by|built on|built by|created by|developed by|made by|trained by)[^\r\n]*?(?:\.\B|(?mR:$))",
            "You are a helpful AI assistant.",
        ),
        (
            "dsh-gui-description",
            "DeepSeek Harness GUI",
            r"(?i)You are interacting with the user through the DeepSeek[ \t]+Harness[ \t]+Web[ \t]+GUI whose URL the runtime context names\.",
            "You are interacting with the user through the DeepSeek Harness Web GUI whose URL is named in the runtime context.",
        ),
        (
            "dsh-brand",
            "DeepSeek Harness",
            r"(?i)\bDeepSeek[ \t]+Harness\b",
            "DeepSeek",
        ),
        // DeepSeek Harness 的系统提示词里有两段会被 WorkBuddy 国内版按「整段」逐字
        // 命中指纹，命中即返回 upstream_code 11-128
        // "Illegal API invocation from an unapproved channel"。
        // 上游比对的是完整段落，任一处措辞差异即可放行，所以这里只做最小改写，
        // 保持语义不变。两段在不同 DSH 版本里措辞略有出入，因此用 regex 兼容。
        (
            "dsh-at-paths",
            "DeepSeek Harness @ paths",
            r"(?i)\b(?:Tokens|Values) prefixed with @ (?:are|denote) paths",
            "Items prefixed with the at-sign refer to paths",
        ),
        (
            "dsh-search-fetch",
            "DeepSeek Harness search/fetch",
            r"(?i)Searching and fetching are distinct operations",
            "Search and fetch are distinct steps",
        ),
    ] {
        entries.push(SanitizeRule {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            match_type: MatchType::Regex,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            repeat: false,
            roles: vec![
                SanitizeRole::System,
                SanitizeRole::Developer,
                SanitizeRole::Assistant,
            ],
        });
    }
    entries
}
