use std::collections::HashSet;
use std::str::FromStr;

use serde_json::{json, Map, Value};
use yaml_edit::{AsYaml, Document, Mapping, Sequence, YamlFile, YamlNode};

pub const TAG: &str = "$agent2apiYamlTag";

pub struct PatchDocument {
    tree: YamlFile,
    bom: bool,
    crlf: bool,
}

impl PatchDocument {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim_start_matches('\u{feff}').trim().is_empty() {
            return Self::parse("[]\n");
        }
        let bom = text.starts_with('\u{feff}');
        let text = text.trim_start_matches('\u{feff}');
        let normalized = text.to_string();
        let value = parse_value(&normalized)?;
        if !value.is_array() {
            return Err("DSH 插件配置必须是 YAML 列表，已停止写入".into());
        }
        let tree = YamlFile::from_str(&normalized)
            .map_err(|_| "DSH 插件配置无法保留原有格式，已停止写入")?;
        if tree.documents().count() != 1 {
            return Err("DSH 插件配置只能包含一个 YAML 文档".into());
        }
        Ok(Self {
            tree,
            bom,
            crlf: text.contains("\r\n"),
        })
    }

    pub fn root(&self) -> Result<Sequence, String> {
        self.tree
            .documents()
            .next()
            .and_then(|document| document.as_sequence())
            .ok_or_else(|| "DSH 插件配置根不是列表".into())
    }

    pub fn rows(&self) -> Result<Vec<Value>, String> {
        parse_value(&self.render())?
            .as_array()
            .cloned()
            .ok_or_else(|| "DSH 插件配置根不是列表".into())
    }

    pub fn render(&self) -> String {
        let text = self.tree.to_string();
        if self.bom {
            format!("\u{feff}{text}")
        } else {
            text
        }
    }

    pub fn append(&self, value: &Value) -> Result<String, String> {
        let mut desired = self.rows()?;
        desired.push(value.clone());
        let source = serde_yaml::to_string(value).map_err(|_| "生成 DSH 配置失败")?;
        let mut text = self.render();
        let root = self.root()?;
        if root.is_empty() && root.is_flow_style() {
            let range = root.byte_range();
            let start = range.start as usize + usize::from(self.bom) * 3;
            let end = range.end as usize + usize::from(self.bom) * 3;
            if text.get(start..end).is_none() {
                return Err("DSH 配置位置无法解析".into());
            }
            text.replace_range(start..end, "");
        } else if root.is_flow_style() {
            let node = json_node(value)?;
            root.push(node);
            return self.checked(&json!(desired));
        }
        let eol = if self.crlf { "\r\n" } else { "\n" };
        if !text.is_empty() && !text.ends_with('\n') {
            text.push_str(eol);
        }
        if text.lines().any(|line| line.trim() == "...") {
            return Err("DSH 配置含文档结束标记，请在客户端内编辑".into());
        }
        for (index, line) in source.trim_end().lines().enumerate() {
            text.push_str(if index == 0 { "- " } else { "  " });
            text.push_str(line);
            text.push_str(eol);
        }
        if parse_value(&text)? != json!(desired) {
            return Err("DSH 配置追加结果与预期不一致，已停止写入".into());
        }
        Self::parse(&text)?;
        Ok(text)
    }

    pub fn checked(&self, expected: &Value) -> Result<String, String> {
        let mut text = self.render();
        if self.root()?.is_empty() && parse_value(&text).is_ok_and(|value| value.is_null()) {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("[]\n");
        }
        if parse_value(&text)? != *expected {
            return Err("DSH 配置编辑结果与预期不一致，已停止写入".into());
        }
        Ok(text)
    }
}

pub fn parse_value(text: &str) -> Result<Value, String> {
    let value: serde_yaml::Value = serde_yaml::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| {
            error
                .location()
                .map(|at| {
                    format!(
                        "DSH YAML 在第 {} 行、第 {} 列无法解析",
                        at.line(),
                        at.column()
                    )
                })
                .unwrap_or_else(|| "DSH YAML 无法解析".into())
        })?;
    let mut value = from_yaml(value, 0)?;
    let tree = YamlFile::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "DSH YAML 无法完整解析")?;
    if tree.documents().count() > 1 {
        return Err("DSH 配置只能包含一个 YAML 文档".into());
    }
    for root in tree.documents() {
        if let Some(sequence) = root.as_sequence() {
            preserve_special(&mut value, &YamlNode::Sequence(sequence));
        } else if let Some(mapping) = root.as_mapping() {
            preserve_special(&mut value, &YamlNode::Mapping(mapping));
        } else if let Some(tag) = root.as_tagged() {
            preserve_special(&mut value, &YamlNode::TaggedNode(tag));
        }
    }
    Ok(value)
}

fn preserve_special(value: &mut Value, node: &YamlNode) {
    match node {
        YamlNode::TaggedNode(tag) => {
            *value = json!({ TAG: tag.tag().unwrap_or_default(), "value": value.clone() });
        }
        YamlNode::Alias(alias) => {
            *value = json!({ TAG: "alias", "value": alias.name() });
        }
        YamlNode::Mapping(mapping) => {
            for (key, child) in mapping.iter() {
                if let Some(key) = key.as_scalar() {
                    if let Some(value) = value.get_mut(&key.as_string()) {
                        preserve_special(value, &child);
                    }
                }
            }
        }
        YamlNode::Sequence(sequence) => {
            if let Some(values) = value.as_array_mut() {
                for (value, child) in values.iter_mut().zip(sequence.values()) {
                    preserve_special(value, &child);
                }
            }
        }
        _ => {}
    }
}

fn from_yaml(value: serde_yaml::Value, depth: usize) -> Result<Value, String> {
    use serde_yaml::Value as Yaml;
    if depth > 64 {
        return Err("DSH YAML 嵌套超过安全上限".into());
    }
    Ok(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => json!(value),
        Yaml::Number(value) => serde_json::to_value(value).map_err(|_| "DSH YAML 数值无法转换")?,
        Yaml::String(value) => json!(value),
        Yaml::Sequence(values) => Value::Array(
            values
                .into_iter()
                .map(|value| from_yaml(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Mapping(values) => {
            let mut map = Map::new();
            for (key, value) in values {
                let key = key.as_str().ok_or("DSH YAML 配置键必须是字符串")?;
                if key == TAG {
                    return Err("DSH YAML 含保留键，已停止编辑".into());
                }
                map.insert(key.into(), from_yaml(value, depth + 1)?);
            }
            Value::Object(map)
        }
        Yaml::Tagged(value) => {
            json!({ TAG: value.tag.to_string(), "value": from_yaml(value.value, depth + 1)? })
        }
    })
}

pub fn dynamic(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(TAG) || map.contains_key("<<") || map.values().any(dynamic)
        }
        Value::Array(items) => items.iter().any(dynamic),
        _ => false,
    }
}

pub fn json_node(value: &Value) -> Result<YamlNode, String> {
    if dynamic(value) {
        return Err("不能把动态 YAML 表达式作为普通配置写入".into());
    }
    let text =
        serde_yaml::to_string(&json!({ "value": value })).map_err(|_| "生成 DSH YAML 失败")?;
    Document::from_str(&text)
        .map_err(|_| "生成 DSH YAML 节点失败")?
        .get("value")
        .ok_or_else(|| "DSH YAML 缺少配置值".into())
}

pub fn node_value(node: &YamlNode) -> Result<Value, String> {
    let wrapper = Mapping::new();
    wrapper.set("value", node);
    parse_value(&wrapper.to_string())?
        .get("value")
        .cloned()
        .ok_or_else(|| "DSH YAML 缺少配置值".into())
}

pub fn string(map: &Mapping, key: &str) -> Option<String> {
    map.get(key)?.as_scalar().map(|value| value.as_string())
}

pub fn sync_mapping(map: &Mapping, before: &Value, after: &Value) -> Result<(), String> {
    let old = before.as_object().ok_or("DSH 配置原值不是对象")?;
    let new = after.as_object().ok_or("DSH 配置目标不是对象")?;
    let mut seen = HashSet::new();
    for key in map.keys() {
        let key = key
            .as_scalar()
            .ok_or("DSH 配置包含动态键，已停止编辑")?
            .as_string();
        if !seen.insert(key) {
            return Err("DSH 配置含重复键，已停止编辑".into());
        }
    }
    for (key, value) in map.iter().collect::<Vec<_>>() {
        let Some(name) = key.as_scalar().map(|key| key.as_string()) else {
            continue;
        };
        if old.contains_key(&name) && !new.contains_key(&name) && has_references(&value) {
            return Err(format!("DSH 的 {name} 使用标签或锚点，不能安全删除"));
        }
    }
    for key in old.keys().filter(|key| !new.contains_key(*key)) {
        map.remove(key.as_str());
    }
    for (key, value) in new {
        if old.get(key) == Some(value) {
            continue;
        }
        if let Some(current) = map.get(key.as_str()) {
            if has_references(&current) {
                return Err(format!("DSH 的 {key} 使用标签或锚点，不能安全改写"));
            }
            if let (Some(child), Some(previous)) = (
                current.as_mapping(),
                old.get(key).filter(|value| value.is_object()),
            ) {
                if value.is_object() {
                    sync_mapping(child, previous, value)?;
                    continue;
                }
            }
        }
        map.set(key.as_str(), json_node(value)?);
    }
    Ok(())
}

pub fn has_references(node: &YamlNode) -> bool {
    node.as_node().is_some_and(|syntax| {
        syntax.descendants_with_tokens().any(|element| {
            element.as_token().is_some_and(|token| {
                matches!(
                    token.kind(),
                    yaml_edit::SyntaxKind::ANCHOR
                        | yaml_edit::SyntaxKind::REFERENCE
                        | yaml_edit::SyntaxKind::TAG
                )
            })
        })
    }) || node.is_alias()
        || node.is_tagged()
}

pub fn redact(text: &str) -> Result<String, String> {
    let tree = YamlFile::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "DSH 配置预览无法安全脱敏")?;
    for document in tree.documents() {
        if let Some(root) = document.as_sequence() {
            redact_sequence(&root);
        }
    }
    let output = tree.to_string();
    Ok(if text.starts_with('\u{feff}') {
        format!("\u{feff}{output}")
    } else {
        output
    })
}

fn redact_sequence(sequence: &Sequence) {
    for node in sequence.values().collect::<Vec<_>>() {
        redact_node(node);
    }
}

fn redact_node(node: YamlNode) {
    match node {
        YamlNode::Mapping(map) => {
            for (key, value) in map.iter().collect::<Vec<_>>() {
                let Some(name) = key.as_scalar().map(|key| key.as_string()) else {
                    continue;
                };
                if super::preview::secret_name(&name) || name == "env" {
                    map.set(name.as_str(), "••••••••");
                } else {
                    redact_node(value);
                }
            }
        }
        YamlNode::Sequence(sequence) => redact_sequence(&sequence),
        _ => {}
    }
}
