use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::LogEntry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogView {
    Important,
    #[default]
    All,
    Debug,
}

impl LogView {
    pub fn parse(value: &str) -> Self {
        match value {
            "important" => Self::Important,
            "debug" => Self::Debug,
            _ => Self::All,
        }
    }

    pub fn matches(self, entry: &LogEntry) -> bool {
        match self {
            Self::Important => entry.level != "debug",
            Self::Debug => entry.level == "debug",
            Self::All => true,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogGroup {
    pub id: u64,
    pub first_ts: i64,
    pub last_ts: i64,
    pub count: usize,
    pub entry: LogEntry,
    pub entries: Vec<LogEntry>,
}

pub fn category_for_tag(tag: &str) -> Option<&'static str> {
    match tag.trim_matches(['[', ']']) {
        "Server" | "Init" | "Logs" | "HTTP" | "Storage" => Some("server"),
        "Config" | "Security" => Some("config"),
        "Login" | "Auth" => Some("auth"),
        "Accounts" | "Usage" => Some("account"),
        "Models" | "Model" => Some("model"),
        "Upstream" | "Billing" | "Activity" => Some("upstream"),
        "Desensitize" => Some("desensitize"),
        "Checkin" => Some("checkin"),
        "Maintenance" => Some("maintenance"),
        "Update" => Some("update"),
        "Plugin" => Some("plugin"),
        _ => None,
    }
}

fn split_tags(message: &str) -> (Vec<&str>, &str) {
    let mut tags = Vec::new();
    let mut text = message.trim();
    if let Some(rest) = text.strip_prefix("Plugin ") {
        tags.push("Plugin");
        text = rest.trim_start();
    }
    while text.starts_with('[') {
        let Some(end) = text.find(']') else { break };
        let tag = &text[1..end];
        if tag.is_empty() || tag.len() > 48 || tag.contains(char::is_whitespace) {
            break;
        }
        tags.push(tag);
        text = text[end + 1..].trim_start();
    }
    (tags, text)
}

fn string_field<'a>(data: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

// 旧插件把 verbose 写成了 info；只识别已知的过程消息，不降低警告和错误。
pub fn prepare(mut entry: LogEntry) -> LogEntry {
    // `entry.data` 已经在本条记录里独占，直接取走 map 即可；原先用
    // `as_object().cloned()` 会把每条日志的 data 整份深拷贝一次，日志页
    // 一次要过几万条时这是纯白花的内存与时间。
    let mut data = match entry.data.take() {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let (tags, text) = split_tags(&entry.message);
    let plugin = tags.contains(&"Plugin") || string_field(&data, "source") == Some("plugin");
    let category = tags.iter().rev().find_map(|tag| category_for_tag(tag));
    if let Some(category) = category {
        if matches!(entry.category.as_str(), "server" | "plugin") && entry.category != category {
            data.entry("originalCategory")
                .or_insert_with(|| json!(entry.category));
            entry.category = category.to_string();
        }
    }
    if plugin {
        data.entry("source").or_insert_with(|| json!("plugin"));
    }
    if let Some(tag) = tags.iter().rev().find(|tag| **tag != "Plugin") {
        data.entry("component").or_insert_with(|| json!(tag));
    }

    let http_request = text.split_once(' ').filter(|(method, url)| {
        matches!(
            *method,
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
        ) && (url.starts_with("https://") || url.starts_with("http://"))
    });
    let legacy_detail = plugin
        && (http_request.is_some()
            || text.contains("出站归一：")
            || text.starts_with("首条消息非 system，已注入系统消息："));
    if legacy_detail && entry.level == "info" {
        data.entry("originalLevel")
            .or_insert_with(|| json!(entry.level));
        entry.level = "debug".to_string();
    }
    let summary = if http_request.is_some() {
        if tags.contains(&"Billing") {
            "计费接口请求"
        } else {
            "上游接口请求"
        }
    } else if text.contains("出站归一：") {
        "出站参数归一"
    } else if text.starts_with("首条消息非 system，已注入系统消息：") {
        "补充系统消息"
    } else {
        text
    };
    data.entry("summary").or_insert_with(|| json!(summary));
    if summary != text {
        data.entry("detail").or_insert_with(|| json!(text));
    }
    if let Some((method, _)) = http_request {
        data.entry("httpMethod").or_insert_with(|| json!(method));
    }
    entry.data = Some(Value::Object(data));
    entry
}

// 未携带账号上下文的记录不能确认属于同一操作，保守地逐条保留。
fn group_key(entry: &LogEntry) -> Option<String> {
    let data = entry.data.as_ref()?.as_object()?;
    let account = string_field(data, "account")
        .or_else(|| string_field(data, "accountId"))
        .or_else(|| string_field(data, "from"));
    if entry.level != "debug" && account.is_none() {
        return None;
    }
    if string_field(data, "source") == Some("plugin") && account.is_none() {
        return None;
    }
    // 原先这里会先把 context 整份克隆成新 Map，再 `json!` 序列化；每条日志
    // 多一次深拷贝。分组键只是用来比较相等，不需要拥有数据 —— 直接用引用
    // 序列化，省掉这次克隆。
    let key = (
        entry.level.as_str(),
        entry.category.as_str(),
        entry.message.as_str(),
        ContextField(data),
    );
    serde_json::to_string(&key).ok()
}

/// `data` 里除几个随请求变化 / 仅用于展示的键之外的全部字段。
struct ContextField<'a>(&'a Map<String, Value>);

impl serde::Serialize for ContextField<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.0 {
            if matches!(
                key.as_str(),
                "callId" | "durationMs" | "accountName" | "providerLabel"
            ) {
                continue;
            }
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// 按页折叠并返回 `(分组总数, 当前页分组)`。
///
/// 与「先构造全部分组、再 skip/take」的区别在**只为当前页保留完整条目**：
/// 日志页一次要过几万条记录，而一页只有几十组；全量构造会把每条日志的
/// data / entry 都克隆一遍（每组至少两份），翻页时白花的内存和 CPU 远大于
/// 分页本身。分组总数仍按全量算（分页器需要它），但页外组只留
/// first/last/count 这几个标量。
pub fn groups_paged(
    entries: Vec<LogEntry>,
    aggregate: bool,
    offset: usize,
    limit: usize,
) -> (usize, Vec<LogGroup>) {
    const WINDOW_MS: i64 = 30_000;

    if !aggregate {
        let grouped = entries.len();
        let groups = entries
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|entry| LogGroup {
                id: entry.id,
                first_ts: entry.ts,
                last_ts: entry.ts,
                count: 1,
                entries: vec![entry.clone()],
                entry,
            })
            .collect();
        return (grouped, groups);
    }

    struct State {
        first_ts: i64,
        /// 分组内**最新**一条的 ts（本轮按 id 倒序遍历，第一条就是最新的）。
        /// 窗口判断始终拿它比，与原先 `groups` 的行为逐字一致。
        last_ts: i64,
        count: usize,
        /// 落在当前页时，指向 `page` 里的下标；页外组为 None。
        page_index: Option<usize>,
    }

    let mut states: Vec<State> = Vec::new();
    let mut recent: HashMap<String, usize> = HashMap::new();
    let mut page: Vec<LogGroup> = Vec::new();

    for entry in entries {
        let key = group_key(&entry);
        if let Some(index) = key.as_ref().and_then(|key| recent.get(key)).copied() {
            let state = &mut states[index];
            if entry.ts <= state.last_ts && state.last_ts.saturating_sub(entry.ts) <= WINDOW_MS {
                state.first_ts = state.first_ts.min(entry.ts);
                state.count += 1;
                if let Some(page_index) = state.page_index {
                    let group = &mut page[page_index];
                    group.first_ts = group.first_ts.min(entry.ts);
                    group.count += 1;
                    group.entries.push(entry);
                }
                continue;
            }
        }
        let index = states.len();
        if let Some(key) = key {
            recent.insert(key, index);
        }
        let in_page = index >= offset && index < offset + limit;
        let entry_id = entry.id;
        let entry_ts = entry.ts;
        let page_index = if in_page {
            page.push(LogGroup {
                id: entry_id,
                first_ts: entry_ts,
                last_ts: entry_ts,
                count: 1,
                entry: entry.clone(),
                entries: vec![entry],
            });
            Some(page.len() - 1)
        } else {
            None
        };
        states.push(State {
            first_ts: entry_ts,
            last_ts: entry_ts,
            count: 1,
            page_index,
        });
    }
    (states.len(), page)
}
