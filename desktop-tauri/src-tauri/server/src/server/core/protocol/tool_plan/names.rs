//! Chat 工具名只能包含 ASCII 字母、数字、下划线和短横线，最长 64 字节。
//! 声明、历史与回程共用一份映射；历史别名有歧义时不能任选一个工具。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::string_field;

const MAX_NAME_BYTES: usize = 64;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ToolName {
    pub namespace: String,
    pub name: String,
}

impl ToolName {
    pub fn new(namespace: &str, name: String) -> Self {
        Self {
            namespace: namespace.to_string(),
            name,
        }
    }

    fn from_call(item: &Value) -> Self {
        let nested = item.get("function");
        let name = string_field(item, "name");
        let namespace = string_field(item, "namespace");
        Self {
            name: if name.is_empty() {
                nested
                    .map(|value| string_field(value, "name"))
                    .unwrap_or_default()
            } else {
                name
            },
            namespace: if namespace.is_empty() {
                nested
                    .map(|value| string_field(value, "namespace"))
                    .unwrap_or_default()
            } else {
                namespace
            },
        }
    }

    fn qualified(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}__{}", self.namespace, self.name)
        }
    }

    fn preferred(&self) -> String {
        let qualified = self.qualified();
        if valid_name(&qualified) {
            qualified
        } else {
            self.hashed(0)
        }
    }

    fn hashed(&self, attempt: u64) -> String {
        let mut digest = Sha256::new();
        digest.update((self.namespace.len() as u64).to_be_bytes());
        digest.update(self.namespace.as_bytes());
        digest.update((self.name.len() as u64).to_be_bytes());
        digest.update(self.name.as_bytes());
        digest.update(attempt.to_be_bytes());
        let hash: String = digest
            .finalize()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let prefix: String = self
            .qualified()
            .chars()
            .map(|ch| if valid_char(ch) { ch } else { '_' })
            .take(MAX_NAME_BYTES - 18)
            .collect();
        format!("{prefix}__{hash}")
    }
}

#[derive(Default)]
pub(super) struct ToolNames {
    declared: BTreeSet<ToolName>,
    forward: BTreeMap<ToolName, String>,
    reverse: BTreeMap<String, ToolName>,
    aliases: BTreeMap<ToolName, Option<ToolName>>,
    bare_aliases: BTreeMap<ToolName, Option<ToolName>>,
}

impl ToolNames {
    pub fn new(declared: BTreeSet<ToolName>, request: &Value) -> Self {
        let mut names = Self {
            declared: declared.clone(),
            ..Self::default()
        };
        names.allocate(&declared);
        for identity in &declared {
            names.add_aliases(identity);
        }
        let mut history = BTreeSet::new();
        if let Some(input) = request.get("input") {
            match input {
                Value::Array(items) => {
                    for item in items {
                        names.collect_history(item, &mut history);
                    }
                }
                Value::Object(_) => names.collect_history(input, &mut history),
                _ => {}
            }
        }
        names.allocate(&history);
        names
    }

    fn allocate(&mut self, identities: &BTreeSet<ToolName>) {
        let reserved: BTreeSet<String> = identities.iter().map(ToolName::preferred).collect();
        // 真实的顶层合法名称优先，不能被另一个工具生成的名称占用。
        for plain in [true, false] {
            for identity in identities {
                let unchanged = identity.namespace.is_empty() && valid_name(&identity.name);
                if unchanged != plain || self.forward.contains_key(identity) {
                    continue;
                }
                let mut wire = identity.preferred();
                let mut attempt = 0;
                if self.reverse.contains_key(&wire) {
                    loop {
                        wire = identity.hashed(attempt);
                        if !reserved.contains(&wire) && !self.reverse.contains_key(&wire) {
                            break;
                        }
                        attempt += 1;
                    }
                }
                self.reverse.insert(wire.clone(), identity.clone());
                self.forward.insert(identity.clone(), wire);
            }
        }
    }

    fn add_alias(&mut self, alias: ToolName, identity: &ToolName) {
        insert_alias(&mut self.aliases, alias, identity);
    }

    fn add_aliases(&mut self, identity: &ToolName) {
        let wire = self.wire_name(identity);
        self.add_alias(ToolName::new("", wire.clone()), identity);
        if identity.namespace.is_empty() {
            self.add_alias(ToolName::new("functions", identity.name.clone()), identity);
            self.add_alias(
                ToolName::new("", format!("functions.{}", identity.name)),
                identity,
            );
            return;
        }
        insert_alias(
            &mut self.bare_aliases,
            ToolName::new("", identity.name.clone()),
            identity,
        );
        for alias in [
            wire,
            identity.qualified(),
            format!("{}{}", identity.namespace, identity.name),
            format!("{}.{}", identity.namespace, identity.name),
        ] {
            self.add_alias(ToolName::new("", alias.clone()), identity);
            self.add_alias(ToolName::new(&identity.namespace, alias), identity);
        }
    }

    fn alias(&self, identity: &ToolName) -> Option<&Option<ToolName>> {
        self.aliases
            .get(identity)
            .or_else(|| self.bare_aliases.get(identity))
    }

    fn declared_identity(&self, identity: &ToolName) -> Option<&ToolName> {
        self.declared
            .get(identity)
            .or_else(|| self.alias(identity).and_then(Option::as_ref))
    }

    fn collect_history(&self, item: &Value, history: &mut BTreeSet<ToolName>) {
        let kind = string_field(item, "type").to_ascii_lowercase();
        if !matches!(kind.as_str(), "function_call" | "custom_tool_call") {
            return;
        }
        let identity = ToolName::from_call(item);
        if !identity.name.is_empty() && self.declared_identity(&identity).is_none() {
            history.insert(identity);
        }
    }

    pub fn wire_name(&self, identity: &ToolName) -> String {
        self.forward
            .get(identity)
            .cloned()
            .unwrap_or_else(|| identity.preferred())
    }

    pub fn call_name(&self, item: &Value) -> Result<String, String> {
        let identity = ToolName::from_call(item);
        if identity.name.is_empty() {
            return Ok(String::new());
        }
        if let Some(resolved) = self.declared_identity(&identity) {
            return Ok(self.wire_name(resolved));
        }
        if self.alias(&identity).is_some_and(Option::is_none) {
            return Err(format!(
                "工具名称「{}」对应多个声明，请保留原始工具名和 namespace 字段",
                identity.name,
            ));
        }
        Ok(self.wire_name(&identity))
    }

    pub fn restore(&self, mut item: Value, wire: &str) -> Value {
        let Some(identity) = self.reverse.get(wire) else {
            return item;
        };
        if let Some(fields) = item.as_object_mut() {
            fields.insert("name".to_string(), Value::String(identity.name.clone()));
            if identity.namespace.is_empty() {
                fields.remove("namespace");
            } else {
                fields.insert(
                    "namespace".to_string(),
                    Value::String(identity.namespace.clone()),
                );
            }
        }
        item
    }
}

fn insert_alias(
    aliases: &mut BTreeMap<ToolName, Option<ToolName>>,
    alias: ToolName,
    identity: &ToolName,
) {
    aliases
        .entry(alias)
        .and_modify(|current| {
            if current.as_ref() != Some(identity) {
                *current = None;
            }
        })
        .or_insert_with(|| Some(identity.clone()));
}

fn valid_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && name.chars().all(valid_char)
}
