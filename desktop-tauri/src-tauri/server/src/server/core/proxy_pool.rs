//! 命名代理池：手动地址与 Clash Verge 端点按 `{source:'pool', proxyId}` 引用。
//! 条目与最近一次测试结果存于 kv 的 `proxyPool` 保留键。
//! 旧订阅镜像只保留原始记录，不再列出、解析或参与轮询。

use std::sync::{Mutex, MutexGuard, OnceLock};

use serde_json::{json, Value};

use crate::server::core::proxies::{resolve_account_proxy, ProxyResolution};
use crate::server::db::Db;
use crate::server::logging;

pub mod usage;
mod import;

pub const KV_KEY: &str = "proxyPool";
pub const RETIRED_HINT: &str = "代理订阅与内置代理内核功能已下线，请在「网络代理」页添加 HTTP / SOCKS5 地址或同步 Clash Verge，并重新选择代理";

const CLASH_READONLY_HINT: &str = "该代理来自 Clash Verge 同步，请到 Clash Verge 中修改名称或端口；本页可启停和测试";

const MAX_NAME_LENGTH: usize = 60;
const MAX_LABEL_LENGTH: usize = 100;

static DB: OnceLock<Option<Db>> = OnceLock::new();
static MUTATIONS: Mutex<()> = Mutex::new(());

fn mutation_lock() -> MutexGuard<'static, ()> {
    MUTATIONS.lock().unwrap_or_else(|error| error.into_inner())
}

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Result<&'static Db, String> {
    DB.get()
        .and_then(Option::as_ref)
        .ok_or_else(|| "代理池数据库不可用".to_string())
}

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

fn is_manual(item: &Value) -> bool {
    matches!(item.get("source").and_then(Value::as_str), None | Some("" | "manual" | "custom"))
}

fn is_clash(item: &Value) -> bool {
    item.get("source").and_then(Value::as_str) == Some("clash")
}

fn is_supported(item: &Value) -> bool {
    is_manual(item) || is_clash(item)
}

pub(crate) fn raw_items() -> Vec<Value> {
    read_items().into_iter().filter(is_supported).collect()
}

fn write_items(items: &[Value]) -> Result<(), String> {
    let _usage_guard = crate::server::core::usage_query::config::lock();
    let db = database()?;
    let text = serde_json::to_string(&json!({ "items": items }))
        .map_err(|error| format!("编码代理池失败: {error}"))?;
    let written = db
        .with(|conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![KV_KEY, text],
            )
            .map(|_| ())
            .map_err(|error| format!("保存代理池失败: {error}"))
        })
        .ok_or_else(|| "代理池数据库不可用".to_string())?;
    written?;
    crate::server::core::cloud_sync::auto_sync::notify_change();
    Ok(())
}

fn clean_string(value: Option<&Value>, max: usize) -> String {
    let Some(Value::String(text)) = value else {
        return String::new();
    };
    text.trim().chars().take(max).collect()
}

fn valid_port(value: Option<&Value>) -> Option<u16> {
    let number = match value {
        Some(Value::Number(number)) => number.as_f64()?,
        Some(Value::String(text)) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !number.is_finite() || number.fract() != 0.0 || !(1.0..=65535.0).contains(&number) {
        return None;
    }
    Some(number as u16)
}

fn text_of(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn normalize_item(input: &Value, existing: Option<&Value>) -> Result<Value, String> {
    let Some(object) = input.as_object() else {
        return Err("代理条目必须是对象".to_string());
    };
    if is_clash(input) {
        return Err("Clash Verge 出口由「同步 Clash Verge」自动导入，不能手工新建".to_string());
    }
    if !is_manual(input) {
        return Err(RETIRED_HINT.to_string());
    }
    let name = clean_string(object.get("name"), MAX_NAME_LENGTH);
    if name.is_empty() {
        return Err("请填写代理名称".to_string());
    }
    let protocol = clean_string(object.get("protocol"), 10).to_lowercase();
    let protocol = if protocol.is_empty() { "http".to_string() } else { protocol };
    if protocol != "http" && protocol != "socks5" {
        return Err("代理协议只支持 http 或 socks5".to_string());
    }
    let host = crate::server::core::proxies::normalize_host(object.get("host").and_then(Value::as_str).unwrap_or(""))?;
    let non_direct = crate::server::core::proxies::normalize_non_direct(
        object.get("nonDirect").or_else(|| existing.and_then(|item| item.get("nonDirect"))),
    )?;
    let Some(port) = valid_port(object.get("port")) else {
        return Err("代理端口必须是 1-65535 的整数".to_string());
    };
    let now = logging::now_ms();
    let id = existing.map(|item| text_of(item, "id")).unwrap_or_default();
    let id = if id.is_empty() { new_id() } else { id };
    let created_at = existing
        .and_then(|item| item.get("createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(now);
    // 测试结果保留为历史事实，时刻由界面显示，不参与实际选路。
    let last_test = existing
        .and_then(|item| item.get("lastTest"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "id": id,
        "name": name,
        "source": "manual",
        "enabled": !matches!(object.get("enabled"), Some(Value::Bool(false))),
        "protocol": protocol,
        "host": host,
        "port": port,
        "username": crate::server::core::proxies::normalize_credential(object.get("username"))?,
        "password": crate::server::core::proxies::normalize_credential(object.get("password"))?,
        "nonDirect": non_direct,
        "createdAt": created_at,
        "updatedAt": now,
        "lastTest": last_test,
    }))
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(logging::now_ms() as u64);
    hasher.write_u64(std::process::id() as u64);
    let suffix = (hasher.finish() & 0xffff_ffff) as u32;
    format!("px_{}_{suffix:08x}", logging::now_ms())
}

pub fn is_enabled(item: &Value) -> bool {
    is_supported(item)
        && !matches!(item.get("enabled"), Some(Value::Bool(false)))
        && !matches!(item.get("localDisabled"), Some(Value::Bool(true)))
}

pub fn resolve_item(item: &Value) -> Option<ProxyResolution> {
    if is_clash(item) {
        return resolve_account_proxy(Some(&json!({
            "source": "clash",
            "listenerUid": text_of(item, "listenerUid"),
        })));
    }
    if !is_manual(item) {
        return Some(ProxyResolution::Failed(RETIRED_HINT.to_string()));
    }
    resolve_account_proxy(Some(&json!({
        "source": "custom",
        "protocol": text_of(item, "protocol"),
        "host": text_of(item, "host"),
        "port": item.get("port").cloned().unwrap_or(Value::Null),
        "username": text_of(item, "username"),
        "password": text_of(item, "password"),
        "nonDirect": item.get("nonDirect").cloned().unwrap_or(Value::Bool(false)),
    })))
}

pub fn resolve_reference(proxy_id: &str) -> Result<crate::server::core::proxies::ResolvedProxy, String> {
    let item = find_raw(proxy_id)?;
    if !is_supported(&item) {
        return Err(RETIRED_HINT.to_string());
    }
    if !is_enabled(&item) {
        return Err(format!(
            "代理「{}」已禁用（在「网络代理」页启用它）",
            display_name(&item, proxy_id)
        ));
    }
    resolve_entry(&item, proxy_id)
}

pub fn resolve_for_test(proxy_id: &str) -> Result<crate::server::core::proxies::ResolvedProxy, String> {
    resolve_entry(&find_raw(proxy_id)?, proxy_id)
}

fn display_name(item: &Value, fallback: &str) -> String {
    let name = text_of(item, "name");
    if name.is_empty() { fallback.to_string() } else { name }
}

fn resolve_entry(
    item: &Value,
    proxy_id: &str,
) -> Result<crate::server::core::proxies::ResolvedProxy, String> {
    let name = display_name(item, proxy_id);
    match resolve_item(item) {
        Some(ProxyResolution::Resolved(mut proxy)) => {
            proxy.label = name;
            Ok(proxy)
        }
        Some(ProxyResolution::Failed(reason)) => Err(format!("代理「{name}」不可用: {reason}")),
        None => Err(format!("代理「{name}」配置不完整")),
    }
}

fn describe_item(item: &Value) -> Value {
    let (resolved, resolve_error) = match resolve_item(item) {
        Some(ProxyResolution::Resolved(proxy)) => (
            json!({ "protocol": proxy.protocol, "host": proxy.host, "port": proxy.port_json(), "label": proxy.label }),
            Value::Null,
        ),
        Some(ProxyResolution::Failed(reason)) => (Value::Null, Value::String(reason)),
        None => (Value::Null, Value::String("配置不完整".to_string())),
    };
    json!({
        "id": text_of(item, "id"),
        "name": text_of(item, "name"),
        "source": if is_clash(item) { "clash" } else { "manual" },
        "enabled": is_enabled(item),
        "sourceEnabled": !matches!(item.get("enabled"), Some(Value::Bool(false))),
        "localDisabled": matches!(item.get("localDisabled"), Some(Value::Bool(true))),
        "listenerUid": text_of(item, "listenerUid"),
        "protocol": text_of(item, "protocol"),
        "host": text_of(item, "host"),
        "port": item.get("port").cloned().unwrap_or(Value::Null),
        "username": text_of(item, "username"),
        "password": text_of(item, "password"),
        "nonDirect": item.get("nonDirect").and_then(Value::as_bool).unwrap_or(false),
        "createdAt": item.get("createdAt").cloned().unwrap_or(Value::from(0)),
        "updatedAt": item.get("updatedAt").cloned().unwrap_or(Value::from(0)),
        "lastTest": item.get("lastTest").cloned().unwrap_or(Value::Null),
        "resolved": resolved,
        "resolveError": resolve_error,
    })
}

pub fn list() -> Vec<Value> {
    raw_items().iter().map(describe_item).collect()
}

fn find_raw(id: &str) -> Result<Value, String> {
    read_items().into_iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| format!("代理「{id}」不存在或已被删除"))
}

pub fn create_batch(input: &Value) -> Result<Value, String> {
    import::create(input)
}

pub fn create(input: &Value) -> Result<Vec<Value>, String> {
    let _guard = mutation_lock();
    let item = normalize_item(input, None)?;
    let mut items = read_items();
    items.insert(0, item.clone());
    write_items(&items)?;
    logging::log("[Proxies]", &format!("✅ 新增代理「{}」", text_of(&item, "name")));
    Ok(list())
}

pub fn update(input: &Value) -> Result<Vec<Value>, String> {
    let _guard = mutation_lock();
    let id = clean_string(input.get("id"), MAX_LABEL_LENGTH);
    if id.is_empty() {
        return Err("缺少代理 id".to_string());
    }
    let mut items = read_items();
    let index = writable_index(&items, &id)?;
    let next = normalize_item(input, Some(&items[index]))?;
    items[index] = next.clone();
    write_items(&items)?;
    logging::log("[Proxies]", &format!("✅ 更新代理「{}」", text_of(&next, "name")));
    Ok(list())
}

fn writable_index(items: &[Value], id: &str) -> Result<usize, String> {
    let index = items.iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| format!("代理「{id}」不存在"))?;
    if !is_supported(&items[index]) {
        return Err(RETIRED_HINT.to_string());
    }
    if is_clash(&items[index]) {
        return Err(CLASH_READONLY_HINT.to_string());
    }
    Ok(index)
}

fn supported_index(items: &[Value], id: &str) -> Result<usize, String> {
    let index = items.iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| format!("代理「{id}」不存在"))?;
    if !is_supported(&items[index]) {
        return Err(RETIRED_HINT.to_string());
    }
    Ok(index)
}

pub struct EnabledChange {
    pub name: String,
    pub enabled: bool,
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(Vec<Value>, EnabledChange), String> {
    let _guard = mutation_lock();
    let mut items = read_items();
    let index = supported_index(&items, id)?;
    let name = display_name(&items[index], id);
    let clash = is_clash(&items[index]);
    if clash && enabled && matches!(items[index].get("enabled"), Some(Value::Bool(false))) {
        return Err("这条在 Clash Verge 里是停用的，请到 Clash Verge 里打开它".to_string());
    }
    let Some(object) = items[index].as_object_mut() else {
        return Err("代理条目形状异常".to_string());
    };
    if clash {
        if enabled {
            object.remove("localDisabled");
        } else {
            object.insert("localDisabled".to_string(), Value::Bool(true));
        }
    } else {
        object.insert("enabled".to_string(), Value::Bool(enabled));
    }
    object.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
    write_items(&items)?;
    logging::log("[Proxies]", &format!("{}代理「{name}」", if enabled { "✅ 启用" } else { "⏸️ 禁用" }));
    Ok((list(), EnabledChange { name, enabled }))
}

pub fn remove(id: &str) -> Result<(Vec<Value>, String), String> {
    let _guard = mutation_lock();
    let mut items = read_items();
    let index = writable_index(&items, id)?;
    let removed = items.remove(index);
    write_items(&items)?;
    let name = text_of(&removed, "name");
    logging::log("[Proxies]", &format!("🗑️ 删除代理「{name}」"));
    Ok((list(), name))
}

pub struct ClashSyncReport {
    pub changes: usize,
    pub error: Option<String>,
}

pub fn sync_clash(force: bool) -> ClashSyncReport {
    let snapshot = crate::server::core::clash::clash_snapshot_with(force);
    if !snapshot.available {
        return ClashSyncReport {
            changes: 0,
            error: Some(snapshot.error.unwrap_or_else(||
                crate::server::core::clash::CLASH_UNAVAILABLE.to_string())),
        };
    }
    let mut targets = Vec::new();
    if snapshot.mixed_port.is_some() {
        targets.push((
            crate::server::core::clash::CLASH_MIXED_UID.to_string(),
            "Clash 混合端口（按规则分流）".to_string(),
            true,
        ));
    }
    for listener in snapshot.listeners {
        targets.push((listener.uid, listener.name, listener.enabled));
    }
    let _guard = mutation_lock();
    let mut items = read_items();
    let mut changes = 0;
    let now = logging::now_ms();
    for (uid, name, enabled) in &targets {
        if let Some(index) = items.iter().position(|item| is_clash(item)
            && item.get("listenerUid").and_then(Value::as_str) == Some(uid.as_str())) {
            let current_enabled = !matches!(items[index].get("enabled"), Some(Value::Bool(false)));
            if text_of(&items[index], "name") != *name || current_enabled != *enabled {
                if let Some(object) = items[index].as_object_mut() {
                    object.insert("name".to_string(), json!(name));
                    object.insert("enabled".to_string(), json!(enabled));
                    object.insert("updatedAt".to_string(), json!(now));
                }
                changes += 1;
            }
        } else {
            items.insert(0, json!({
                "id": new_id(),
                "name": name,
                "source": "clash",
                "listenerUid": uid,
                "enabled": enabled,
                "createdAt": now,
                "updatedAt": now,
                "lastTest": Value::Null,
            }));
            changes += 1;
        }
    }
    // 只对成功读取的 Clash 集合做镜像对账，其他来源的历史记录不受影响。
    let before = items.len();
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| {
        if !is_clash(item) { return true; }
        let Some(uid) = item.get("listenerUid").and_then(Value::as_str) else { return false; };
        targets.iter().any(|(target, _, _)| target == uid) && seen.insert(uid.to_string())
    });
    changes += before - items.len();
    if changes > 0 {
        if let Err(error) = write_items(&items) {
            return ClashSyncReport { changes: 0, error: Some(error) };
        }
        logging::log("[Proxies]", &format!("🔄 已同步 Clash Verge 出口（{changes} 项变更）"));
    }
    ClashSyncReport { changes, error: None }
}

pub struct TestOutcome {
    pub success: bool,
    pub ip: String,
    pub duration_ms: i64,
    pub error: Option<String>,
}

pub fn record_test(id: &str, result: &TestOutcome) -> Result<Vec<Value>, String> {
    let _guard = mutation_lock();
    let mut items = read_items();
    let index = supported_index(&items, id)?;
    let Some(object) = items[index].as_object_mut() else {
        return Err("代理条目形状异常".to_string());
    };
    object.insert("lastTest".to_string(), json!({
        "success": result.success,
        "ip": result.ip,
        "durationMs": result.duration_ms,
        "at": logging::now_ms(),
        "error": result.error,
    }));
    write_items(&items)?;
    Ok(list())
}
