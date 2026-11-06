use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use yaml_edit::{Mapping, Sequence};

use super::dsh_document::{self as document, PatchDocument};
use super::{dsh_installation as installation, dsh_paths, files};

pub const PROVIDERS: &str = "llm-pi-ai";
pub const DEFAULT_MODEL: &str = "agent-default-model";
pub const MCP_PACKAGE: &str = "@deepseek-ai/dsh-mcp-client";

#[derive(Clone)]
pub struct Entry {
    pub value: Value,
    pub source: PathBuf,
    pub nested: bool,
}

pub struct Composition {
    pub entries: BTreeMap<String, Entry>,
    pub inputs: Vec<files::Change>,
    pub inherited: BTreeMap<String, Entry>,
}

impl Composition {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::load_with(path, None)
    }

    pub fn preview(path: &Path, text: &str) -> Result<Self, String> {
        Self::load_with(path, Some(text))
    }

    fn load_with(path: &Path, replacement: Option<&str>) -> Result<Self, String> {
        let mut result = Self {
            entries: BTreeMap::new(),
            inputs: Vec::new(),
            inherited: BTreeMap::new(),
        };
        let parent = path.parent().ok_or("DSH 配置没有父目录")?;
        let home = dsh_paths::home_for(path);
        if parent != home {
            let manifest = parent.join("package.json");
            let bytes = result.input(&manifest)?;
            let package: Value =
                serde_json::from_slice(bytes.as_deref().ok_or("请先启动一次所选 DSH 运行方式")?)
                    .map_err(|_| "DSH profile 的 package.json 无法读取")?;
            let bundles = package
                .pointer("/dsh/profile/bundles")
                .and_then(Value::as_array)
                .ok_or("DSH profile 缺少插件组合清单")?;
            let binary = super::paths::find_binary(super::find("dsh").ok_or("DSH 客户端未登记")?);
            let local = installation::profile(parent);
            let install = if dsh_paths::profile_name(path) == "desktop" {
                installation::desktop()
            } else {
                installation::cli(binary.as_deref())
            };
            let install = if install.version.is_empty() {
                local
            } else {
                install
            };
            for bundle in bundles {
                let name = bundle.as_str().ok_or("DSH 插件组合名称不是字符串")?;
                if !valid_package(name) {
                    return Err("DSH 插件组合名称不是可安全读取的包名".into());
                }
                let mut candidates = Vec::new();
                if let Some(path) = installation::package_file(&install, name) {
                    candidates.push(path);
                }
                if let Some(patches) = installation::bundled_files(&install, name) {
                    for (source, text) in patches {
                        result.apply_layer(&source, &text)?;
                    }
                    continue;
                }
                for dir in parent.ancestors().take(4) {
                    candidates.push(dir.join("node_modules").join(name).join("package.json"));
                }
                let manifest = candidates.into_iter().find(|path| path.is_file());
                if let Some(manifest) = manifest {
                    let bytes = result.input(&manifest)?;
                    let package: Value =
                        serde_json::from_slice(bytes.as_deref().unwrap_or_default())
                            .map_err(|_| "DSH 插件包清单无法读取")?;
                    if let Some(patches) = package.pointer("/dsh/bundle/patch") {
                        let patches = if let Some(path) = patches.as_str() {
                            vec![path]
                        } else {
                            patches
                                .as_array()
                                .ok_or("DSH 插件组合 patch 声明无效")?
                                .iter()
                                .map(|value| value.as_str().ok_or("DSH 插件组合 patch 路径无效"))
                                .collect::<Result<Vec<_>, _>>()?
                        };
                        for patch in patches {
                            let relative = Path::new(patch);
                            if relative.is_absolute()
                                || relative.components().any(|part| {
                                    matches!(
                                        part,
                                        std::path::Component::ParentDir
                                            | std::path::Component::Prefix(_)
                                    )
                                })
                            {
                                return Err("DSH 插件组合 patch 超出包目录".into());
                            }
                            result.layer(
                                &manifest
                                    .parent()
                                    .ok_or("DSH 插件包路径无效")?
                                    .join(relative),
                            )?;
                        }
                    }
                } else {
                    return Err(format!(
                        "无法读取 DSH 插件组合 {name} 的配置，已停止以免覆盖继承设置"
                    ));
                }
            }
        }
        result.inherited = result.entries.clone();
        if let Some(text) = replacement {
            result.input(path)?;
            result.apply_layer(path, text)?;
        } else {
            result.layer(path)?;
        }
        let home_patch = home.join(dsh_paths::PATCH_FILE);
        if home_patch != path {
            result.layer(&home_patch)?;
        }
        Ok(result)
    }

    fn input(&mut self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        let change = files::Change::load(path.into())?;
        let bytes = change.before.clone();
        if !self.inputs.iter().any(|item| item.path == path) {
            self.inputs.push(change);
        }
        Ok(bytes)
    }

    fn layer(&mut self, path: &Path) -> Result<(), String> {
        let bytes = self.input(path)?;
        let text = files::text(bytes.as_deref())?;
        self.apply_layer(path, &text)
    }

    fn apply_layer(&mut self, path: &Path, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let rows = PatchDocument::parse(text)?.rows()?;
        for row in rows {
            let map = row.as_object().ok_or("DSH 插件配置条目不是对象")?;
            if let Some(insert) = map.get("insert") {
                let nested = map.get("id").is_some();
                if nested {
                    let id = map
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or("DSH 插入目标使用动态标识，不能安全解析")?;
                    if !self.entries.get(id).is_some_and(|entry| {
                        entry.value.get("group").and_then(Value::as_bool) == Some(true)
                    }) {
                        continue;
                    }
                }
                self.insert(
                    insert.as_array().ok_or("DSH insert 不是列表")?,
                    path,
                    nested,
                )?;
                continue;
            }
            let Some(id) = map.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(entry) = self.entries.get_mut(id) else {
                continue;
            };
            if map
                .get("name")
                .is_some_and(|name| entry.value.get("name") != Some(name))
            {
                continue;
            }
            let target = entry.value.as_object_mut().ok_or("DSH 条目无效")?;
            for (key, value) in map {
                if matches!(key.as_str(), "id" | "name") {
                    continue;
                }
                target.insert(key.clone(), value.clone());
            }
            entry.source = path.into();
        }
        Ok(())
    }

    fn insert(&mut self, rows: &[Value], path: &Path, nested: bool) -> Result<(), String> {
        for row in rows {
            let map = row.as_object().ok_or("DSH 插入条目不是对象")?;
            if let Some(id) = map.get("id").and_then(Value::as_str) {
                if self.entries.contains_key(id) {
                    return Err(format!("DSH 插件标识 {id} 重复，已停止编辑"));
                }
                self.entries.insert(
                    id.into(),
                    Entry {
                        value: row.clone(),
                        source: path.into(),
                        nested,
                    },
                );
            }
            if map.get("group").and_then(Value::as_bool) == Some(true) {
                if let Some(children) = map.get("config").and_then(Value::as_array) {
                    self.insert(children, path, true)?;
                }
            }
        }
        Ok(())
    }

    pub fn config(&self, id: &str) -> Result<Value, String> {
        let Some(entry) = self.entries.get(id) else {
            return Ok(json!({}));
        };
        if entry.value.get("isolate").is_some()
            || entry.value.get("intercept").is_some()
            || entry.value.get("inject").is_some()
            || entry.nested
            || entry
                .value
                .get("disabled")
                .is_some_and(|value| value != &Value::Bool(false) && !value.is_null())
        {
            return Err(format!("DSH 的 {id} 位于独立作用域或已禁用，不能安全接管"));
        }
        if let Some(id) = entry.value.get("id").and_then(Value::as_str) {
            let expected = match id {
                PROVIDERS => Some("@deepseek-ai/dsh-llm-pi-ai"),
                DEFAULT_MODEL => Some("@deepseek-ai/dsh-agent-default-model"),
                "credentials" => Some("@deepseek-ai/dsh-credentials-local"),
                _ => None,
            };
            if expected
                .is_some_and(|name| entry.value.get("name").and_then(Value::as_str) != Some(name))
            {
                return Err(format!("DSH 的 {id} 已替换为其他插件，不能安全接管"));
            }
        }
        let value = entry
            .value
            .get("config")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !value.is_object() || document::dynamic(&value) {
            return Err(format!("DSH 的 {id} 使用动态配置，不能按普通设置改写"));
        }
        Ok(value)
    }

    pub fn settings(&self) -> Result<Value, String> {
        Ok(
            json!({ PROVIDERS: self.config(PROVIDERS)?, DEFAULT_MODEL: self.config(DEFAULT_MODEL)? }),
        )
    }

    pub fn editable(&self, id: &str, target: &Path) -> Result<(), String> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| format!("所选 DSH 配置没有启用 {id} 插件"))?;
        if entry.source == dsh_paths::home_for(target).join(dsh_paths::PATCH_FILE)
            && entry.source != target
        {
            return Err(format!(
                "DSH 的 {id} 被全局 cordis.patch.yml 覆盖，请在 DSH 中编辑全局配置"
            ));
        }
        self.config(id)?;
        Ok(())
    }

    pub fn guards(&self, changes: &mut Vec<files::Change>) {
        for input in &self.inputs {
            if !changes.iter().any(|change| change.path == input.path) {
                changes.push(input.clone());
            }
        }
    }
}

fn valid_package(name: &str) -> bool {
    let parts = name.split('/').collect::<Vec<_>>();
    let shape = if name.starts_with('@') {
        parts.len() == 2 && parts[0].len() > 1 && !parts[1].is_empty()
    } else {
        parts.len() == 1
    };
    shape
        && !name.is_empty()
        && !name.contains("..")
        && !name.contains(['\\', ':'])
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "@/._-".contains(ch))
}

pub fn update_config(text: &str, id: &str, value: &Value) -> Result<String, String> {
    let document = PatchDocument::parse(text)?;
    let mut rows = document.rows()?;
    let mut matches = Vec::new();
    collect(&document.root()?, id, &mut matches)?;
    if let Some((_, _, map)) = matches.last() {
        let current = map
            .get("config")
            .map(|node| document::node_value(&node))
            .transpose()?
            .unwrap_or_else(|| json!({}));
        if let Some(config) = map.get_mapping("config") {
            document::sync_mapping(&config, &current, value)?;
        } else {
            map.set("config", document::json_node(value)?);
        }
        update_rows(&mut rows, id, value);
        return document.checked(&json!(rows));
    }
    document.append(&json!({ "id": id, "config": value }))
}

fn update_rows(rows: &mut [Value], id: &str, value: &Value) -> bool {
    for row in rows.iter_mut().rev() {
        if let Some(items) = row.get_mut("insert").and_then(Value::as_array_mut) {
            if update_rows(items, id, value) {
                return true;
            }
        }
        if row.get("id").and_then(Value::as_str) == Some(id) {
            row["config"] = value.clone();
            return true;
        }
    }
    false
}

pub fn collect(
    sequence: &Sequence,
    id: &str,
    output: &mut Vec<(Sequence, usize, Mapping)>,
) -> Result<(), String> {
    for (index, node) in sequence.values().enumerate() {
        let map = node.as_mapping().ok_or("DSH 插件配置条目不是对象")?;
        if let Some(insert) = map.get_sequence("insert") {
            collect(&insert, id, output)?;
        } else if document::string(map, "id").as_deref() == Some(id) {
            output.push((sequence.clone(), index, map.clone()));
        }
    }
    Ok(())
}

pub fn remove_entry(text: &str, id: &str) -> Result<String, String> {
    let document = PatchDocument::parse(text)?;
    let mut matches = Vec::new();
    collect(&document.root()?, id, &mut matches)?;
    if matches.is_empty() {
        return Err("此 MCP 来自继承的插件组合，不能从当前配置直接删除".into());
    }
    let mut expected = document.rows()?;
    remove_rows(&mut expected, id);
    for (sequence, index, map) in matches.into_iter().rev() {
        if document::has_references(&yaml_edit::YamlNode::Mapping(map)) {
            return Err("此 MCP 条目使用标签或锚点，不能安全删除".into());
        }
        sequence.remove(index);
        if sequence.is_empty() {
            sequence.clear();
        }
    }
    document.checked(&json!(expected))
}

fn remove_rows(rows: &mut Vec<Value>, id: &str) {
    rows.retain(|row| {
        row.get("insert").is_some() || row.get("id").and_then(Value::as_str) != Some(id)
    });
    for row in rows {
        if let Some(items) = row.get_mut("insert").and_then(Value::as_array_mut) {
            remove_rows(items, id);
        }
    }
}

pub fn mcp_entries(composition: &Composition) -> Result<Vec<(String, Value)>, String> {
    let mut names = HashSet::new();
    let mut entries = Vec::new();
    for (id, entry) in &composition.entries {
        if entry.value.get("name").and_then(Value::as_str) != Some(MCP_PACKAGE) {
            continue;
        }
        let config = entry
            .value
            .get("config")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let name = config
            .get("serverName")
            .and_then(Value::as_str)
            .ok_or("DSH MCP 的服务名为动态值或缺失，不能安全判定重名")?;
        if !names.insert(name.to_string()) {
            return Err("DSH 存在重名 MCP，不能安全改写".into());
        }
        entries.push((id.clone(), config));
    }
    Ok(entries)
}
