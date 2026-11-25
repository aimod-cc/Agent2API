//! 定时任务类插件的协议数据结构、校验与单文件双向解析/序列化。

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::cron_schedule::{validate_cron, validate_timezone, DEFAULT_TIMEZONE};

pub const HEADER_MARKER: &str = "@agent2api-task-plugin";
pub const TASK_SCRIPT_TARGET: &str = "task-script";
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;
pub const MAX_FIELDS: usize = 64;
pub const MAX_ACTIONS: usize = 16;
pub const MAX_STATUS_FIELDS: usize = 16;
pub const MAX_DOMAINS: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldCondition {
    pub key: String,
    pub equals: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFieldDef {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub options: Vec<FieldOption>,
    #[serde(default)]
    pub when: Option<FieldCondition>,
}

fn default_group() -> String {
    "basic".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskActionDef {
    pub id: String,
    pub label: String,
    #[serde(default = "default_variant")]
    pub variant: String,
    #[serde(default = "default_placement")]
    pub placement: String,
    #[serde(default)]
    pub attach_to_field: Option<String>,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub cooldown_seconds: Option<u32>,
}

fn default_variant() -> String {
    "outline".to_string()
}

fn default_placement() -> String {
    "footer".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusFieldDef {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlugin {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub signed: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub icon: String,
    #[serde(default = "default_cron")]
    pub default_cron: String,
    #[serde(default = "default_tz")]
    pub default_timezone: String,
    #[serde(default = "default_timeout")]
    pub default_timeout: i64,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub declares_notify: bool,
    #[serde(default = "default_true")]
    pub notify_allowed: bool,
    #[serde(default)]
    pub fields: Vec<TaskFieldDef>,
    #[serde(default)]
    pub actions: Vec<TaskActionDef>,
    #[serde(default)]
    pub status_fields: Vec<TaskStatusFieldDef>,
    #[serde(default)]
    pub script_body: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_version() -> String {
    "0.1.0".to_string()
}
fn default_source() -> String {
    "local".to_string()
}
fn default_true() -> bool {
    true
}
fn default_cron() -> String {
    "0 9 * * *".to_string()
}
fn default_tz() -> String {
    DEFAULT_TIMEZONE.to_string()
}
fn default_timeout() -> i64 {
    30
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJobView {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub cron_expression: String,
    pub timezone: String,
    pub timeout_seconds: i64,
    pub proxy_id: String,
    pub enabled: bool,
    pub values: BTreeMap<String, Value>,
    pub saved_secrets: BTreeMap<String, bool>,
    pub status_summary: BTreeMap<String, String>,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
    pub last_duration_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJobRun {
    pub id: i64,
    pub job_id: String,
    pub trigger_type: String,
    pub trigger_label: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub output: String,
    pub error: Option<String>,
}

pub fn valid_slug(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

pub fn valid_identifier(key: &str) -> bool {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if matches!(key, "__proto__" | "prototype" | "constructor" | "signal") {
        return false;
    }
    (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

pub fn normalize_domain(raw: &str) -> Result<String, String> {
    let mut text = raw.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Err("域名不能为空".to_string());
    }
    if text == "*" {
        return Ok("*".to_string());
    }
    if let Some(rest) = text.strip_prefix("https://").or_else(|| text.strip_prefix("http://")) {
        text = rest.to_string();
    }
    if let Some((host, _)) = text.split_once('/') {
        text = host.to_string();
    }
    if let Some((host, _)) = text.split_once(':') {
        text = host.to_string();
    }
    let wildcard = text.starts_with("*.");
    let check_host = if wildcard { &text[2..] } else { text.as_str() };
    if check_host.is_empty()
        || !check_host.contains('.')
        || check_host.len() > 253
        || check_host == "localhost"
        || check_host.ends_with(".local")
        || !check_host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return Err(format!("无效域名白名单项：{raw}"));
    }
    Ok(text)
}

pub fn validate_and_normalize_plugin(mut plugin: TaskPlugin) -> Result<TaskPlugin, String> {
    plugin.id = plugin.id.trim().to_ascii_lowercase();
    if !valid_slug(&plugin.id) {
        return Err("插件标识必须以小写字母开头，仅含小写字母、数字和短横线（1–64 字符）".to_string());
    }
    plugin.name = plugin.name.trim().chars().take(100).collect();
    if plugin.name.is_empty() {
        return Err("插件名称不能为空".to_string());
    }
    plugin.description = plugin.description.trim().chars().take(2000).collect();
    plugin.version = plugin.version.trim().chars().take(40).collect();
    if plugin.version.is_empty() {
        plugin.version = "0.1.0".to_string();
    }
    if !matches!(plugin.source.as_str(), "official" | "local" | "third-party") {
        plugin.source = "local".to_string();
    }
    plugin.default_cron = validate_cron(&plugin.default_cron)?;
    plugin.default_timezone = validate_timezone(&plugin.default_timezone)?;
    if !(1..=300).contains(&plugin.default_timeout) {
        return Err("默认超时时间必须在 1–300 秒之间".to_string());
    }
    if plugin.script_body.len() > MAX_SCRIPT_BYTES {
        return Err("脚本代码超过 256 KB 上限".to_string());
    }
    if plugin.domains.len() > MAX_DOMAINS {
        return Err(format!("域名白名单最多声明 {MAX_DOMAINS} 项"));
    }
    let mut dedup_domains = Vec::new();
    for raw in &plugin.domains {
        let normalized = normalize_domain(raw)?;
        if !dedup_domains.contains(&normalized) {
            dedup_domains.push(normalized);
        }
    }
    plugin.domains = dedup_domains;

    if plugin.fields.len() > MAX_FIELDS {
        return Err(format!("表单输入项最多声明 {MAX_FIELDS} 个"));
    }
    let mut field_keys = HashSet::new();
    for field in &mut plugin.fields {
        field.key = field.key.trim().to_string();
        if !valid_identifier(&field.key) {
            return Err(format!("输入项标识无效：{}", field.key));
        }
        if !field_keys.insert(field.key.clone()) {
            return Err(format!("存在重复的输入项标识：{}", field.key));
        }
        field.label = field.label.trim().chars().take(80).collect();
        if field.label.is_empty() {
            field.label = field.key.clone();
        }
        if !matches!(
            field.field_type.as_str(),
            "text" | "password" | "textarea" | "number" | "switch" | "select" | "segmented" | "multiselect"
        ) {
            return Err(format!("不支持的表单控件类型：{}", field.field_type));
        }
        if field.field_type == "password" {
            field.secret = true;
        }
        if field.secret {
            field.default_value = None;
        }
        if field.group != "advanced" {
            field.group = "basic".to_string();
        }
        if matches!(field.field_type.as_str(), "select" | "segmented" | "multiselect") {
            if field.options.is_empty() {
                return Err(format!("选择控件「{}」至少需要一个选项", field.label));
            }
            let mut seen_opts = HashSet::new();
            for opt in &mut field.options {
                opt.value = opt.value.trim().to_string();
                if opt.value.is_empty() || !seen_opts.insert(opt.value.clone()) {
                    return Err(format!("选择控件「{}」存在空值或重复选项", field.label));
                }
                if opt.label.trim().is_empty() {
                    opt.label = opt.value.clone();
                }
            }
        }
    }
    for field in &plugin.fields {
        if let Some(when) = &field.when {
            if !field_keys.contains(&when.key) {
                return Err(format!("输入项「{}」的联动条件引用了不存在的字段：{}", field.label, when.key));
            }
        }
    }

    if plugin.actions.len() > MAX_ACTIONS {
        return Err(format!("自定义按钮最多声明 {MAX_ACTIONS} 个"));
    }
    let mut action_ids = HashSet::new();
    for action in &mut plugin.actions {
        action.id = action.id.trim().to_string();
        if !valid_identifier(&action.id) {
            return Err(format!("按钮动作标识无效：{}", action.id));
        }
        if !action_ids.insert(action.id.clone()) {
            return Err(format!("存在重复的按钮动作标识：{}", action.id));
        }
        action.label = action.label.trim().chars().take(60).collect();
        if action.label.is_empty() {
            action.label = action.id.clone();
        }
        if !matches!(action.variant.as_str(), "default" | "outline" | "destructive") {
            action.variant = "outline".to_string();
        }
        if action.placement != "inline" {
            action.placement = "footer".to_string();
            action.attach_to_field = None;
        } else if let Some(ref key) = action.attach_to_field {
            if !field_keys.contains(key) {
                action.attach_to_field = None;
                action.placement = "footer".to_string();
            }
        }
        if let Some(cd) = action.cooldown_seconds {
            if cd > 3600 {
                action.cooldown_seconds = Some(3600);
            }
        }
    }

    if plugin.status_fields.len() > MAX_STATUS_FIELDS {
        return Err(format!("状态指标最多声明 {MAX_STATUS_FIELDS} 项"));
    }
    let mut status_keys = HashSet::new();
    for sf in &mut plugin.status_fields {
        sf.key = sf.key.trim().to_string();
        if !valid_identifier(&sf.key) || !status_keys.insert(sf.key.clone()) {
            return Err(format!("状态指标标识无效或重复：{}", sf.key));
        }
        sf.label = sf.label.trim().chars().take(40).collect();
        if sf.label.is_empty() {
            sf.label = sf.key.clone();
        }
    }

    Ok(plugin)
}

/// 将插件序列化为干净的分享单文件（绝对不包含账号密码或任何任务实例数据）。
pub fn serialize_single_file(plugin: &TaskPlugin) -> String {
    let header = json!({
        "id": plugin.id,
        "name": plugin.name,
        "version": plugin.version,
        "description": plugin.description,
        "icon": plugin.icon,
        "defaultCron": plugin.default_cron,
        "defaultTimezone": plugin.default_timezone,
        "defaultTimeout": plugin.default_timeout,
        "permissions": {
            "domains": plugin.domains,
            "notify": plugin.declares_notify,
        },
        "fields": plugin.fields,
        "actions": plugin.actions,
        "statusFields": plugin.status_fields,
    });
    let pretty = serde_json::to_string_pretty(&header).unwrap_or_else(|_| "{}".to_string());
    format!("/* {HEADER_MARKER}\n{pretty}\n*/\n\n{}", plugin.script_body.trim_start())
}

/// 解析单文件脚本（头部声明块 + 下方脚本逻辑），用于双向同步编辑器与单文件导入。
pub fn parse_single_file(raw: &str, fallback_source: &str) -> Result<TaskPlugin, String> {
    if raw.len() > MAX_SCRIPT_BYTES + 64 * 1024 {
        return Err("插件文件内容过大".to_string());
    }
    let start = raw
        .find("/*")
        .ok_or_else(|| "缺少头部声明块：请以 /* @agent2api-task-plugin ... */ 开头".to_string())?;
    let after_open = &raw[start + 2..];
    let end_rel = after_open
        .find("*/")
        .ok_or_else(|| "头部声明块未正确闭合（缺少 */）".to_string())?;
    let block = after_open[..end_rel].trim();
    let json_text = block
        .strip_prefix(HEADER_MARKER)
        .ok_or_else(|| format!("头部声明块必须以 {HEADER_MARKER} 开头"))?
        .trim();
    let meta: Value = serde_json::from_str(json_text)
        .map_err(|error| format!("头部声明 JSON 格式有误：{error}"))?;
    let body = after_open[end_rel + 2..].trim_start().to_string();

    let fields: Vec<TaskFieldDef> = match meta.get("fields") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|error| format!("表单字段声明有误：{error}"))?,
        None => Vec::new(),
    };
    let actions: Vec<TaskActionDef> = match meta.get("actions") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|error| format!("自定义按钮声明有误：{error}"))?,
        None => Vec::new(),
    };
    let status_fields: Vec<TaskStatusFieldDef> = match meta.get("statusFields") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|error| format!("状态指标声明有误：{error}"))?,
        None => Vec::new(),
    };
    let domains: Vec<String> = meta
        .pointer("/permissions/domains")
        .or_else(|| meta.get("domains"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let declares_notify = meta
        .pointer("/permissions/notify")
        .or_else(|| meta.get("declaresNotify"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let plugin = TaskPlugin {
        id: meta["id"].as_str().unwrap_or("").to_string(),
        name: meta["name"].as_str().unwrap_or("").to_string(),
        description: meta["description"].as_str().unwrap_or("").to_string(),
        version: meta["version"].as_str().unwrap_or("0.1.0").to_string(),
        source: fallback_source.to_string(),
        signed: false,
        enabled: true,
        icon: meta["icon"].as_str().unwrap_or("").to_string(),
        default_cron: meta["defaultCron"].as_str().unwrap_or("0 9 * * *").to_string(),
        default_timezone: meta["defaultTimezone"]
            .as_str()
            .unwrap_or(DEFAULT_TIMEZONE)
            .to_string(),
        default_timeout: meta["defaultTimeout"].as_i64().unwrap_or(30),
        domains,
        declares_notify,
        notify_allowed: true,
        fields,
        actions,
        status_fields,
        script_body: body,
        created_at: 0,
        updated_at: 0,
    };
    validate_and_normalize_plugin(plugin)
}
