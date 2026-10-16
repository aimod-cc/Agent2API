//! 订阅存储：「订阅管理」维护的机场/节点订阅链接（kv 的 `subscriptions` 键）。
//!
//! ── 存储形态（与 proxyPool 同一类）────────────────────────────
//! 条目数量级是个位数、无查询需求（永远整份读、整份写），与 `proxyPool` /
//! `scheduledTasks` 同一类零散状态 —— 落 `kv` 的固定键（登记在
//! `db::schema::RESERVED_KV_KEYS`，否则 `config::save_raw` 写一次配置就会把
//! 它删掉）。不新增表也就不动 schema 版本。
//!
//! ── 职责边界（刻意只有 KV CRUD）──────────────────────────────
//! 本模块是**纯存储**：不认识内核（`core::sub_kernel`）、不认识代理池。内核
//! 的拉起 / 节点同步由 api 层编排（handler 读完这里 → 构造 `ProviderInput`
//! 清单 → `sub_kernel::request_restart`），依赖方向是 api → subscriptions →
//! sub_kernel 的类型，反向（sub_kernel → subscriptions）不存在 —— worker
//! 命令带全量输入，内核不回读订阅。节点镜像进代理池的写侧在
//! `core::sub_kernel`（它只认识 proxy_pool），本模块不碰。
//!
//! ── 字段 ────────────────────────────────────────────────────
//! `{id, name, url, enabled, createdAt, updatedAt}`。id 形如
//! `sub_<毫秒>_<8 位随机>`（与代理池条目同一手法），它同时是 mihomo
//! proxy-provider 名字的前缀（`sub_<id>`），所以只允许小写字母数字下划线。
//!
//! ── 两种来源（kind）────────────────────────────────────────
//! `kind: "url"`（缺省，老条目没有该字段也按它算）= 订阅链接，内容由内核定时
//! 拉取；`kind: "local"` = 本地 YAML 文件导入，`content` 存 YAML 原文、
//! `fileName` 存展示用的原文件名（不参与任何路径拼接 —— 内核侧的缓存文件固定叫
//! `providers/sub_<id>.yaml`，由 runtime 每次重启前从 `content` 写出，所以换
//! 内核目录也不会丢文件）。本地订阅没有 url（存空串，字段留着让条目形状稳定），
//! 也没有「定时刷新」语义 —— 内容在导入那一刻就定了，更新 = 编辑重选文件。

use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::server::core::sub_kernel::ProviderInput;
use crate::server::db::Db;
use crate::server::logging;

/// `kv` 表里的键名（保留键，见 `db::schema::RESERVED_KV_KEYS`）
pub const KV_KEY: &str = "subscriptions";

/// 名称 / 链接的长度上限（名称出现在表格里；链接要进 mihomo 配置）
const MAX_NAME_LENGTH: usize = 60;
const MAX_URL_LENGTH: usize = 2048;

/// 本地文件名的长度上限（只做展示，不参与路径）
const MAX_FILE_NAME_LENGTH: usize = 150;
/// 本地 YAML 内容的字节数上限。订阅文件量级是几十 KB（上百节点），2MB 已经
/// 宽裕到可疑；再大基本是拿错了文件，早一点报错比让内核背一份巨配置好。
const MAX_CONTENT_BYTES: usize = 2_000_000;

/// 进程级库句柄（照 `core::proxy_pool` 的形态：bootstrap 时 install 一次）
static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 接线（`ServerState::bootstrap` 调用）。重复调用无副作用。
pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Result<&'static Db, String> {
    DB.get()
        .and_then(Option::as_ref)
        .ok_or_else(|| "订阅数据库不可用".to_string())
}

// ─── 存取 ───────────────────────────────────────────────────

/// 读全部条目（库不可用/无记录/解析失败都给空表；解析失败按空表处理，
/// 与 proxyPool 同一口径 —— 用户界面上的表现是「订阅空了」，可重新添加）
fn read_items() -> Vec<Value> {
    let Some(db) = DB.get().and_then(Option::as_ref) else {
        return Vec::new();
    };
    db.with(|conn| {
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [KV_KEY], |row| {
            row.get::<_, String>(0)
        })
        .ok()
    })
    .flatten()
    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    .and_then(|value| value.get("items").and_then(Value::as_array).cloned())
    .unwrap_or_default()
}

/// 整份写回（增删改都走它；条目数量小，整份重写的开销可忽略）
fn write_items(items: &[Value]) -> Result<(), String> {
    let db = database()?;
    let text = serde_json::to_string(&json!({ "items": items }))
        .map_err(|error| format!("编码订阅失败: {error}"))?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KV_KEY, text],
        )
        .map(|_| ())
        .map_err(|error| format!("保存订阅失败: {error}"))
    })
    .ok_or_else(|| "订阅数据库不可用".to_string())?
}

// ─── 正规化与校验 ───────────────────────────────────────────

fn clean_string(value: Option<&Value>, max: usize) -> String {
    let Some(Value::String(text)) = value else {
        return String::new();
    };
    text.trim().chars().take(max).collect()
}

fn text_of(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// 订阅链接校验：http/https 开头（mihomo 的 http provider 只认这两种）。
fn valid_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("请填写订阅链接".to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("订阅链接必须是 http:// 或 https:// 开头".to_string());
    }
    Ok(())
}

/// 校验本地 YAML 内容，通过即返回节点数。
///
/// 提前把 mihomo 要到 provider 加载时才报的错拦在导入时 —— 内核侧的报错是
/// kernel.log 里的一行英文，用户无从下手；这里的每一条都是可行动的中文。
/// 逐条对应 mihomo 的真实行为：YAML 语法错 → provider 解析失败；没有
/// `proxies` 列表 / 列表为空 → provider 里 0 个节点；节点缺 name 或重名 →
/// 整个 provider 加载失败（mihomo 对 provider 内的节点名有唯一性要求）。
fn validate_local_content(content: &str) -> Result<usize, String> {
    let parsed: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|error| format!("YAML 解析失败: {error}"))?;
    let Some(items) = parsed
        .get("proxies")
        .and_then(serde_yaml::Value::as_sequence)
    else {
        return Err(
            "文件里没有找到节点列表（需要 Clash YAML 格式：顶层有非空的 proxies 列表）"
                .to_string(),
        );
    };
    if items.is_empty() {
        return Err("文件里的 proxies 列表是空的".to_string());
    }
    let mut names = std::collections::HashSet::new();
    for item in items {
        let Some(name) = item.get("name").and_then(serde_yaml::Value::as_str) else {
            return Err("有节点缺少 name 字段（mihomo 要求每个节点都有名称）".to_string());
        };
        if !names.insert(name.to_string()) {
            return Err(format!("文件里有重复的节点名「{name}」"));
        }
    }
    Ok(items.len())
}

/// 新条目的 id：`sub_<毫秒>_<8 位随机>`。随机段与 `proxy_pool::new_id` 同一
/// 手法（RandomState 的随机种子），不引第三方依赖。
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(logging::now_ms() as u64);
    hasher.write_u64(std::process::id() as u64);
    let suffix = (hasher.finish() & 0xffff_ffff) as u32;
    format!("sub_{}_{suffix:08x}", logging::now_ms())
}

/// 把 API 输入（新增的整份表单）归一成一条订阅条目。
///
/// 新增时 `existing` 传 None；编辑时从旧条目沿用 `id` / `createdAt`。
fn normalize_item(input: &Value, existing: Option<&Value>) -> Result<Value, String> {
    let Some(object) = input.as_object() else {
        return Err("订阅条目必须是对象".to_string());
    };
    let name = clean_string(object.get("name"), MAX_NAME_LENGTH);
    if name.is_empty() {
        return Err("请填写订阅名称".to_string());
    }
    // ── 来源类型 ────────────────────────────────────────────
    // 显式带了 kind 就按它；没带（行内启停 / 老客户端只回传 name/url/enabled）
    // 沿用旧条目的 —— 本地订阅若被当成链接订阅，url 是空的，整条编辑会莫名 400。
    // 编辑时显式改回 "url" 是合法的换源（content 丢弃）；反向（url → local）
    // 必须带上新文件内容，走同一条校验。
    let is_local = match object.get("kind").and_then(Value::as_str) {
        Some("local") => true,
        Some("url") | Some("") => false,
        Some(_) => return Err("订阅来源类型只支持 url / local".to_string()),
        None => existing
            .map(|item| text_of(item, "kind") == "local")
            .unwrap_or(false),
    };
    let url = clean_string(object.get("url"), MAX_URL_LENGTH);
    let (url, content, file_name) = if is_local {
        // 新内容优先；没带（编辑没重选文件 / 行内启停）沿用旧条目的。
        // 本地订阅 url 一律置空：留着表单漏过来的旧链接会让它看起来像链接订阅
        let content = match object.get("content").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => text.to_string(),
            _ => existing
                .map(|item| text_of(item, "content"))
                .unwrap_or_default(),
        };
        if content.is_empty() {
            return Err("请先选择要导入的 YAML 文件".to_string());
        }
        if content.len() > MAX_CONTENT_BYTES {
            return Err("YAML 文件太大（上限 2MB）".to_string());
        }
        validate_local_content(&content)?;
        let mut file_name = clean_string(object.get("fileName"), MAX_FILE_NAME_LENGTH);
        if file_name.is_empty() {
            file_name = existing
                .map(|item| text_of(item, "fileName"))
                .unwrap_or_default();
        }
        (String::new(), content, file_name)
    } else {
        valid_url(&url)?;
        (url, String::new(), String::new())
    };
    let now = logging::now_ms();
    let id = existing
        .map(|item| text_of(item, "id"))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(new_id);
    let created_at = existing
        .and_then(|item| item.get("createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(now);
    let mut normalized = Map::new();
    normalized.insert("id".to_string(), Value::String(id));
    normalized.insert("name".to_string(), Value::String(name));
    if is_local {
        normalized.insert("kind".to_string(), Value::String("local".to_string()));
        normalized.insert("url".to_string(), Value::String(String::new()));
        normalized.insert("fileName".to_string(), Value::String(file_name));
        // 内容原样入库（已过 UTF-8 与 YAML 校验；base64 只在前后的传输层存在）
        normalized.insert("content".to_string(), Value::String(content));
    } else {
        normalized.insert("url".to_string(), Value::String(url));
    }
    normalized.insert(
        "enabled".to_string(),
        Value::Bool(!matches!(object.get("enabled"), Some(Value::Bool(false)))),
    );
    normalized.insert("createdAt".to_string(), Value::from(created_at));
    normalized.insert("updatedAt".to_string(), Value::from(now));
    Ok(Value::Object(normalized))
}

// ─── 公开形态 ───────────────────────────────────────────────

fn describe_item(item: &Value, node_count: usize) -> Value {
    json!({
        "id": text_of(item, "id"),
        "name": text_of(item, "name"),
        "url": text_of(item, "url"),
        // 来源类型：老条目没有该字段，一律按链接算；**content 刻意不外发** ——
        // 列表接口每次整份返回，把几百 KB 的 YAML 原文带上纯属浪费
        "kind": if text_of(item, "kind") == "local" { "local" } else { "url" },
        "fileName": text_of(item, "fileName"),
        "enabled": !matches!(item.get("enabled"), Some(Value::Bool(false))),
        "createdAt": item.get("createdAt").cloned().unwrap_or(Value::from(0)),
        "updatedAt": item.get("updatedAt").cloned().unwrap_or(Value::from(0)),
        "nodeCount": node_count,
    })
}

/// 最新全量列表；`node_count` 是「订阅 id → 节点数」的映射（api 层从内核
/// 状态里带进来 —— 本模块不认识内核，节点数由调用方注入）。
pub fn list(node_counts: &std::collections::HashMap<String, usize>) -> Vec<Value> {
    read_items()
        .iter()
        .map(|item| {
            let id = text_of(item, "id");
            describe_item(item, node_counts.get(&id).copied().unwrap_or(0))
        })
        .collect()
}

// ─── 写操作 ─────────────────────────────────────────────────

/// 新增订阅；返回最新全量列表（原始形态，条目不带节点数 —— api 层统一注入）。
pub fn create(input: &Value) -> Result<Vec<Value>, String> {
    let item = normalize_item(input, None)?;
    let mut items = read_items();
    // 同一来源只存一份：重复导入是常见误操作（同一订阅改个名再存一遍
    // 会拉两次内容、占两份 provider）。链接按 url 查重；本地文件按内容查重 ——
    // 同一份 YAML 换个名字再导一遍，效果与重复订阅链接完全一样。
    let is_local = text_of(&item, "kind") == "local";
    let url = text_of(&item, "url");
    let content = text_of(&item, "content");
    let duplicated = items.iter().any(|existing| {
        if is_local {
            !content.is_empty() && text_of(existing, "content") == content
        } else {
            text_of(existing, "url") == url
        }
    });
    if duplicated {
        return Err(if is_local {
            "这份 YAML 文件的内容已经导入过（不用重复添加）".to_string()
        } else {
            "该订阅链接已存在".to_string()
        });
    }
    items.insert(0, item.clone());
    write_items(&items)?;
    logging::log(
        "[Subscription]",
        &format!("✅ 新增订阅「{}」", text_of(&item, "name")),
    );
    Ok(items)
}

/// 编辑订阅（id 必带；整份覆盖语义，与代理池条目同口径）。
pub fn update(input: &Value) -> Result<Vec<Value>, String> {
    let id = clean_string(input.get("id"), 100);
    if id.is_empty() {
        return Err("缺少订阅 id".to_string());
    }
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id.as_str()))
    else {
        return Err(format!("订阅「{id}」不存在"));
    };
    let next = normalize_item(input, Some(&items[index]))?;
    // 改链接 / 换文件内容时查重（排除自己），判据与 create 同一条
    let next_is_local = text_of(&next, "kind") == "local";
    let next_url = text_of(&next, "url");
    let next_content = text_of(&next, "content");
    let duplicated = items.iter().enumerate().any(|(position, item)| {
        if position == index {
            return false;
        }
        if next_is_local {
            !next_content.is_empty() && text_of(item, "content") == next_content
        } else {
            text_of(item, "url") == next_url
        }
    });
    if duplicated {
        return Err(if next_is_local {
            "这份 YAML 文件的内容已经导入过（不用重复添加）".to_string()
        } else {
            "该订阅链接已存在".to_string()
        });
    }
    items[index] = next.clone();
    write_items(&items)?;
    logging::log(
        "[Subscription]",
        &format!("✅ 更新订阅「{}」", text_of(&next, "name")),
    );
    Ok(items)
}

/// 删除订阅；返回（最新列表, 被删条目的名称 —— 调用方拼日志/提示用）。
pub fn remove(id: &str) -> Result<(Vec<Value>, String), String> {
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Err(format!("订阅「{id}」不存在"));
    };
    let removed = items.remove(index);
    write_items(&items)?;
    let name = text_of(&removed, "name");
    logging::log("[Subscription]", &format!("🗑️ 删除订阅「{name}」"));
    Ok((items, name))
}

/// 是否有启用的订阅（内核要不要拉起 / api 层要不要投重启命令的判据）
pub fn has_enabled() -> bool {
    read_items()
        .iter()
        .any(|item| !matches!(item.get("enabled"), Some(Value::Bool(false))))
}

/// 条目是否存在 / 是否启用三态（`None` = 不存在）—— refresh 的报错分流用。
///
/// 「不存在」与「被禁用」是两种事：前者多半是条目被删（或数据被外部工具
/// 清掉），后者用户自己就能启用 —— 笼统报「不存在或已禁用」会把排查方向
/// 带偏（实测踩过：旧版进程误删存储键时，用户从这句文案里看不出真正原因）。
pub fn find_enabled_state(id: &str) -> Option<bool> {
    read_items()
        .iter()
        .find(|item| text_of(item, "id") == id)
        .map(|item| !matches!(item.get("enabled"), Some(Value::Bool(false))))
}

/// 现存订阅的 id 集合（代理池标注「孤儿订阅节点」用：条目的 providerId
/// 不在这个集合里 = 所属订阅已不存在，可手动清理）。
pub fn ids() -> std::collections::HashSet<String> {
    read_items()
        .iter()
        .map(|item| text_of(item, "id"))
        .collect()
}

/// 订阅来源类型（"local" / "url"）；`None` = 订阅不存在。
///
/// api 层用它分流「立即更新」：本地文件没有链接可拉（内容在导入时就已落盘），
/// 刷新无从谈起，handler 当场报一句可行动的话。
pub fn kind_of(id: &str) -> Option<&'static str> {
    read_items()
        .iter()
        .find(|item| text_of(item, "id") == id)
        .map(|item| {
            if text_of(item, "kind") == "local" {
                "local"
            } else {
                "url"
            }
        })
}

/// 启用订阅的输入清单（api 层 / bootstrap 构造 `ProviderInput` 用：
/// 内核的 proxy-providers 只收启用的条目）。
pub fn enabled_provider_inputs() -> Vec<ProviderInput> {
    read_items()
        .iter()
        .filter(|item| !matches!(item.get("enabled"), Some(Value::Bool(false))))
        .map(|item| ProviderInput {
            id: text_of(item, "id"),
            name: text_of(item, "name"),
            url: text_of(item, "url"),
            // 本地文件的 YAML 原文跟着走：runtime 每次重启前把它写进内核的
            // provider 缓存位（那是本地文件唯一的存在形式，没有内核替它拉取）
            content: text_of(item, "content"),
        })
        .collect()
}

/// 是否已安装内核且有启用的订阅（bootstrap 自动拉起的判据）
pub fn should_autostart() -> bool {
    crate::server::core::sub_kernel::installed() && has_enabled()
}
