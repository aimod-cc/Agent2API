use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use super::PromptMode;

pub const MAX_RULES: usize = 200;
pub const MAX_CONDITIONS: usize = 32;
pub const MAX_VALUES: usize = 100;
pub const MAX_PATTERN_CHARS: usize = 256;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field { Provider, Client, Model }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator { Is, IsNot, Contains, NotContains, Glob, Regex }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Logic { #[default] All, Any }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Condition {
    pub id: String,
    pub field: Field,
    pub op: Operator,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Content {
    pub mode: PromptMode,
    pub file: String,
    /// 只保存用户正文，不把文件内容或内置正文变成覆盖。
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub logic: Logic,
    pub conditions: Vec<Condition>,
    pub mode: PromptMode,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub text: String,
}

impl Rule {
    pub fn content(&self) -> Content {
        Content { mode: self.mode, file: self.file.clone(), text: self.text.clone() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub client: String,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct CompiledRule {
    pub rule: Rule,
    pub prompt: crate::server::config::ProviderPrompt,
    pub matchers: Vec<Matcher>,
}

impl CompiledRule {
    pub fn matches(&self, provider: &str, client: &str, model: &str) -> bool {
        if !self.rule.enabled || self.matchers.is_empty() { return false; }
        let matches = |matcher: &Matcher| matcher.matches(provider, client, model);
        match self.rule.logic {
            Logic::All => self.matchers.iter().all(matches),
            Logic::Any => self.matchers.iter().any(matches),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Matcher {
    condition: Condition,
    pattern: Option<Regex>,
}

impl Matcher {
    pub fn compile(condition: &Condition) -> Result<Self, String> {
        let value = condition.values.first().map(String::as_str).unwrap_or("");
        let pattern = if condition.field == Field::Model && !value.is_empty() {
            match condition.op {
                Operator::Glob => {
                    let mut body = String::from("\\A");
                    for character in value.chars() {
                        match character {
                            '*' => body.push_str(".*"),
                            '?' => body.push('.'),
                            character => body.push_str(&regex::escape(&character.to_string())),
                        }
                    }
                    body.push_str("\\z");
                    Some(compile_regex(&body)?)
                }
                Operator::Regex => Some(compile_regex(value)?),
                _ => None,
            }
        } else { None };
        Ok(Self { condition: condition.clone(), pattern })
    }

    pub fn matches(&self, provider: &str, client: &str, model: &str) -> bool {
        let actual = match self.condition.field {
            Field::Provider => provider,
            Field::Client => client,
            Field::Model => model,
        };
        // 空条件不能扩大范围；未指定模型也不参与「不是 / 不包含」。
        if actual.is_empty() || self.condition.values.is_empty() { return false; }
        if self.condition.field != Field::Model {
            let hit = self.condition.values.iter().any(|value| actual.eq_ignore_ascii_case(value));
            return if self.condition.op == Operator::IsNot { !hit } else { hit };
        }
        let value = &self.condition.values[0];
        if value.is_empty() { return false; }
        match self.condition.op {
            Operator::Is => actual.eq_ignore_ascii_case(value),
            Operator::IsNot => !actual.eq_ignore_ascii_case(value),
            Operator::Contains => actual.to_ascii_lowercase().contains(&value.to_ascii_lowercase()),
            Operator::NotContains => !actual.to_ascii_lowercase().contains(&value.to_ascii_lowercase()),
            Operator::Glob | Operator::Regex => self.pattern.as_ref().is_some_and(|pattern| pattern.is_match(actual)),
        }
    }
}

fn compile_regex(value: &str) -> Result<Regex, String> {
    RegexBuilder::new(value).case_insensitive(true)
        .size_limit(1024 * 1024).dfa_size_limit(1024 * 1024)
        .build().map_err(|error| format!("模型匹配表达式无效：{error}"))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte|
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

pub fn validate_content(content: &Content) -> Result<(), String> {
    if content.text.len() > MAX_TEXT_BYTES { return Err("提示词正文不能超过 1 MiB".into()); }
    if content.file.len() > 4096 || content.file.chars().any(char::is_control) {
        return Err("提示词文件路径无效或过长".into());
    }
    Ok(())
}

pub fn validate_rules(rules: &[Rule]) -> Result<(), String> {
    use std::collections::HashSet;
    if rules.len() > MAX_RULES { return Err(format!("最多允许 {MAX_RULES} 条提示词规则")); }
    let mut ids = HashSet::new();
    for rule in rules {
        if !valid_id(&rule.id) || !ids.insert(&rule.id) { return Err("提示词规则标识无效或重复".into()); }
        if rule.name.chars().count() > 80 || rule.name.chars().any(char::is_control) {
            return Err("规则名称最长 80 个字符，不能包含换行等控制字符".into());
        }
        validate_content(&rule.content())?;
        if rule.conditions.len() > MAX_CONDITIONS {
            return Err(format!("每条规则最多允许 {MAX_CONDITIONS} 个条件"));
        }
        let mut conditions = HashSet::new();
        for condition in &rule.conditions {
            if !valid_id(&condition.id) || !conditions.insert(&condition.id) {
                return Err("条件标识无效或重复".into());
            }
            if condition.field != Field::Model && !matches!(condition.op, Operator::Is | Operator::IsNot) {
                return Err("提供商和客户端条件只能使用「是 / 不是」".into());
            }
            let limit = if condition.field == Field::Model { 1 } else { MAX_VALUES };
            if condition.values.len() > limit { return Err("条件值数量超过上限".into()); }
            let mut values = HashSet::new();
            for value in &condition.values {
                if value.trim() != value || value.is_empty() || value.chars().count() > MAX_PATTERN_CHARS
                    || value.chars().any(char::is_control) || !values.insert(value) {
                    return Err("条件值不能为空、重复或含控制字符，最长 256 个字符".into());
                }
                if condition.field != Field::Model && !valid_id(value) {
                    return Err("提供商或客户端标识无效".into());
                }
            }
            Matcher::compile(condition)?;
        }
    }
    Ok(())
}
