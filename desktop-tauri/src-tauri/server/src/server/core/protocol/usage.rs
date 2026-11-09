//! 各协议共用的 token 读数；Chat / Responses 的输入总量包含缓存，Anthropic 分开计数。

use serde_json::{json, Map, Value};

const PROMPT_KEYS: &[&str] = &["prompt_tokens", "input_tokens", "promptTokens", "inputTokens"];
const COMPLETION_KEYS: &[&str] = &[
    "completion_tokens", "output_tokens", "completionTokens", "outputTokens",
];
const TOTAL_KEYS: &[&str] = &["total_tokens", "totalTokens"];
const CACHE_CONTAINERS: &[&str] = &["prompt_tokens_details", "input_tokens_details"];
const CACHE_READ_KEYS: &[&str] = &[
    "cached_tokens", "cached_input_tokens", "cache_read_tokens", "cache_read_input_tokens",
    "cacheReadTokens", "cacheReadInputTokens", "cachedInputTokens", "prompt_cache_hit_tokens",
    "prompt_tokens_details.cached_tokens",
];
const CACHE_CREATION_KEYS: &[&str] = &[
    "cache_creation_input_tokens", "cache_creation_tokens", "cache_write_tokens",
    "cached_creation_tokens", "cache_write_input_tokens", "cacheCreationInputTokens",
    "cacheCreationTokens", "cacheWriteTokens", "cacheWriteInputTokens",
];

#[derive(Clone, Copy, Debug, Default)]
pub struct TokenUsage {
    pub prompt: Option<i64>,
    pub completion: Option<i64>,
    pub total: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_creation: Option<i64>,
    pub reasoning: Option<i64>,
    pub deferred_prompt: Option<i64>,
    pub deferred_completion: Option<i64>,
    pub deferred_cache_read: Option<i64>,
    pub deferred_cache_write: Option<i64>,
    pub cache_unknown: bool,
}

impl TokenUsage {
    pub fn from_chat(usage: &Value) -> Option<Self> {
        let object = usage.as_object()?;
        Some(Self {
            prompt: first_count(object, PROMPT_KEYS),
            completion: first_count(object, COMPLETION_KEYS),
            total: first_count(object, TOTAL_KEYS),
            cache_read: cache_count(object, CACHE_READ_KEYS),
            cache_creation: preferred_count([
                cache_count(object, CACHE_CREATION_KEYS),
                creation_breakdown(object),
            ]),
            reasoning: preferred_count([
                token_count(usage.pointer("/completion_tokens_details/reasoning_tokens")),
                token_count(usage.pointer("/output_tokens_details/reasoning_tokens")),
            ]),
            deferred_prompt: token_count(usage.pointer("/magpie_deferred/prompt_tokens")),
            deferred_completion: token_count(usage.pointer("/magpie_deferred/completion_tokens")),
            deferred_cache_read: token_count(usage.pointer("/magpie_deferred/cached_tokens")),
            deferred_cache_write: token_count(usage.pointer("/magpie_deferred/cache_write_tokens")),
            cache_unknown: usage.get("magpie_cache_unknown").and_then(Value::as_bool) == Some(true),
        })
    }

    /// 同一帧的别名可用正数替代占位零；跨帧显式上报的零仍须覆盖旧值。
    pub fn absorb(&mut self, next: Self) {
        if next.prompt.is_some() || next.completion.is_some() || next.total.is_some() {
            self.total = next.total;
        }
        self.prompt = next.prompt.or(self.prompt);
        self.completion = next.completion.or(self.completion);
        self.cache_read = next.cache_read.or(self.cache_read);
        self.cache_creation = next.cache_creation.or(self.cache_creation);
        self.reasoning = next.reasoning.or(self.reasoning);
        self.deferred_prompt = next.deferred_prompt.or(self.deferred_prompt);
        self.deferred_completion = next.deferred_completion.or(self.deferred_completion);
        self.deferred_cache_read = next.deferred_cache_read.or(self.deferred_cache_read);
        self.deferred_cache_write = next.deferred_cache_write.or(self.deferred_cache_write);
        self.cache_unknown |= next.cache_unknown;
    }

    pub fn total_tokens(&self) -> i64 {
        self.total.unwrap_or(0).max(
            self.prompt.unwrap_or(0).saturating_add(self.completion.unwrap_or(0)),
        )
    }

    /// 只补齐已知读数，保留上游的其他字段；不把未上报的缓存伪造成命中。
    pub fn write_chat(&self, usage: &mut Value) -> bool {
        let Some(object) = usage.as_object_mut() else { return false };
        let mut changed = false;
        changed |= set_count(object, "prompt_tokens", self.prompt);
        changed |= set_count(object, "completion_tokens", self.completion);
        if self.total.is_some() || self.prompt.is_some() || self.completion.is_some() {
            changed |= set_count(object, "total_tokens", Some(self.total_tokens()));
        }
        changed |= set_detail(object, "prompt_tokens_details", "cached_tokens", self.cache_read);
        changed |= set_count(object, "cache_creation_input_tokens", self.cache_creation);
        changed |= set_detail(object, "completion_tokens_details", "reasoning_tokens", self.reasoning);
        for (key, count) in [("prompt_tokens", self.deferred_prompt), ("completion_tokens", self.deferred_completion),
            ("cached_tokens", self.deferred_cache_read), ("cache_write_tokens", self.deferred_cache_write)] {
            changed |= set_detail(object, "magpie_deferred", key, count);
        }
        if self.cache_unknown && object.get("magpie_cache_unknown") != Some(&Value::Bool(true)) {
            object.insert("magpie_cache_unknown".into(), Value::Bool(true));
            changed = true;
        }
        changed
    }

    pub fn anthropic(&self) -> Value {
        let cache_read = self.cache_read.unwrap_or(0);
        let cache_creation = self.cache_creation.unwrap_or(0);
        json!({
            "input_tokens": self.prompt.unwrap_or(0)
                .saturating_sub(cache_read).saturating_sub(cache_creation).max(0),
            "output_tokens": self.completion.unwrap_or(0),
            "cache_read_input_tokens": cache_read,
            "cache_creation_input_tokens": cache_creation,
        })
    }

    pub fn responses(&self) -> Value {
        let mut usage = json!({
            "input_tokens": self.prompt.unwrap_or(0),
            "output_tokens": self.completion.unwrap_or(0),
            "total_tokens": self.total_tokens(),
        });
        if let Some(object) = usage.as_object_mut() {
            set_detail(object, "input_tokens_details", "cached_tokens", self.cache_read);
            set_detail(object, "output_tokens_details", "reasoning_tokens", self.reasoning);
            set_count(object, "cache_creation_input_tokens", self.cache_creation);
        }
        usage
    }
}

pub fn normalize_chat(usage: &mut Value) -> bool {
    TokenUsage::from_chat(usage).is_some_and(|tokens| tokens.write_chat(usage))
}

pub fn merge_chat(current: &mut Option<Value>, update: &Value) {
    let Some(next) = TokenUsage::from_chat(update) else { return };
    let mut tokens = current.as_ref().and_then(TokenUsage::from_chat).unwrap_or_default();
    tokens.absorb(next);
    // 只继承规范字段，不能让旧别名中的正数盖过这一帧明确上报的零。
    let mut merged = update.clone();
    tokens.write_chat(&mut merged);
    *current = Some(merged);
}

pub fn token_count(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().filter(|number| *number >= 0)
            .or_else(|| number.as_f64().and_then(finite_count)),
        Value::String(text) => text.trim().parse::<i64>().ok().filter(|number| *number >= 0)
            .or_else(|| text.trim().parse::<f64>().ok().and_then(finite_count)),
        _ => None,
    }
}

fn finite_count(number: f64) -> Option<i64> {
    (number.is_finite() && number >= 0.0).then_some(number as i64)
}

fn preferred_count(values: impl IntoIterator<Item = Option<i64>>) -> Option<i64> {
    let mut zero = None;
    for value in values.into_iter().flatten() {
        if value > 0 { return Some(value); }
        zero = Some(0);
    }
    zero
}

fn first_count(object: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    preferred_count(keys.iter().map(|key| token_count(object.get(*key))))
}

fn cache_count(object: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    preferred_count(CACHE_CONTAINERS.iter().map(|container| {
        object.get(*container).and_then(Value::as_object)
            .and_then(|details| first_count(details, keys))
    }).chain(std::iter::once(first_count(object, keys))))
}

fn creation_breakdown(object: &Map<String, Value>) -> Option<i64> {
    let details = object.get("cache_creation")?.as_object()?;
    let short = token_count(details.get("ephemeral_5m_input_tokens"));
    let long = token_count(details.get("ephemeral_1h_input_tokens"));
    (short.is_some() || long.is_some())
        .then(|| short.unwrap_or(0).saturating_add(long.unwrap_or(0)))
}

fn set_count(object: &mut Map<String, Value>, key: &str, count: Option<i64>) -> bool {
    let Some(count) = count else { return false };
    let value = Value::from(count);
    if object.get(key) == Some(&value) { return false; }
    object.insert(key.to_string(), value);
    true
}

fn set_detail(object: &mut Map<String, Value>, container: &str, key: &str, count: Option<i64>) -> bool {
    if count.is_none() { return false; }
    let details = object.entry(container.to_string()).or_insert_with(|| json!({}));
    if !details.is_object() { *details = json!({}); }
    details.as_object_mut().is_some_and(|details| set_count(details, key, count))
}
