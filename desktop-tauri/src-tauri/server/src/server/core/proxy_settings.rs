//! 网络代理的全局设置：节点监听端口范围 + 出口连通性测试网址。
//!
//! ── 存储形态（与 proxyPool 同一类）────────────────────────────
//! 两三项标量、无查询需求（永远整份读、整份写），与 `proxyPool` /
//! `subscriptions` 同类零散状态 —— 落 `kv` 的固定键（登记在
//! `db::schema::RESERVED_KV_KEYS`，否则 `config::save_raw` 写一次配置就会把
//! 它删掉）。不新增表也就不动 schema 版本。
//!
//! ── 两项设置分别被谁读 ──────────────────────────────────────
//!   · `listenerPortBase` / `listenerPortEnd`：内核（`sub_kernel::runtime`）
//!     给订阅节点分配本地监听端口时用。默认 19101-19200 是**编译期常量的
//!     历史值** —— 节点多的大订阅（几百个节点）会把这个区间用尽，启动报
//!     「没有可用的节点端口」；现在范围可调，改完重启内核即生效。已注册的
//!     端口（状态文件里的注册表）即使超出新范围也沿用 —— 注册表是「账号
//!     引用不漂移」的承诺，改范围不该把既有端口作废，新分配才受范围约束。
//!   · `testUrl`：「网络代理」页出口连通性测试的目标网址（`core::egress`
//!     的 `test_proxy`）。默认打 WorkBuddy 上游（copilot.tencent.com），
//!     判据是「能否连上**上游**」；自定义上游的用户换到这里测自家的上游
//!     才与实际链路一致。空串 = 用默认。
//!
//! ── 校验（save 在写入前拦住非法形状）─────────────────────────
//!   · 端口范围：1..=65535 的整数，且 base <= end；区间收敛到「合法端口」
//!     的交集。不校验与控制端口段（19090-19099）的重叠 —— `alloc_port`
//!     本来就带绑定探测（被占用的口跳过），范围里撞了控制口会自然避开，
//!     校验只把「明显写错」挡在门外。
//!   · 测试网址：http(s) 前缀 + 长度上限；空串合法（= 恢复默认）。
//!     不校验可达性 —— 保存的时候连不上不代表用的时候连不上，真不通时
//!     测试结果里的 error 文案自会说明。
//!
//! ── 为什么读接口是同步的 ─────────────────────────────────────
//! 调用方（runtime 的端口分配 / egress 的测试入口）在同步与 async 混合的
//! 上下文里都要能用；kv 读是本地 sqlite 单查询，阻塞可忽略 —— 与
//! `proxy_pool::read_items` 同一取舍。

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::server::db::Db;

/// `kv` 表里的键名（保留键，见 `db::schema::RESERVED_KV_KEYS`）
pub const KV_KEY: &str = "proxySettings";

/// 节点监听端口范围的默认值（与历史编译期常量一致，迁移语义：没存过 = 默认）
pub const DEFAULT_LISTENER_PORT_BASE: u16 = 19101;
pub const DEFAULT_LISTENER_PORT_END: u16 = 19200;

/// 连通性测试的默认目标（与 `core::egress::DEFAULT_TEST_URL` 同值；
/// 两处共用一份默认会引入循环依赖（egress 认识本模块，本模块不认识
/// egress），所以这里复制字面量、注释互指 —— 改一处要同步另一处）
pub const DEFAULT_TEST_URL: &str = "https://copilot.tencent.com/v2/config";

/// 网址长度上限（要进内核配置与请求行的，超长多半是粘错了）
const MAX_URL_LENGTH: usize = 2048;

/// 进程级库句柄（照 `core::proxy_pool` 的形态：bootstrap 时 install 一次）
static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 接线（`ServerState::bootstrap` 调用）。重复调用无副作用。
pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

/// 读整份设置（库不可用 / 无记录 / 解析失败都给空对象 —— 全部回落默认）
fn read_raw() -> Value {
    let Some(db) = DB.get().and_then(Option::as_ref) else {
        return json!({});
    };
    db.with(|conn| {
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [KV_KEY], |row| {
            row.get::<_, String>(0)
        })
        .ok()
    })
    .flatten()
    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    .unwrap_or_else(|| json!({}))
}

/// 整份写回
fn write_raw(value: &Value) -> Result<(), String> {
    let Some(db) = DB.get().and_then(Option::as_ref) else {
        return Err("设置数据库不可用".to_string());
    };
    let text = serde_json::to_string(value).map_err(|error| format!("编码代理设置失败: {error}"))?;
    let written = db
        .with(|conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![KV_KEY, text],
            )
            .map(|_| ())
            .map_err(|error| format!("保存代理设置失败: {error}"))
        })
        .ok_or_else(|| "设置数据库不可用".to_string())?;
    written?;
    // 保留键不走 `config::update`，显式通知自动同步（见 cloud_sync::auto_sync）
    crate::server::core::cloud_sync::auto_sync::notify_change();
    Ok(())
}

/// 端口字段 → 合法的 u16（数字或数字字符串；与 proxy_pool 的 valid_port 同口径）
fn port_of(value: Option<&Value>) -> Option<u16> {
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

// ─── 读取（运行时消费的入口）────────────────────────────────

/// 节点监听端口的分配范围 `(base, end)`（含端点）。
///
/// 存量非法（被外部工具改坏 / 范围倒挂）时整体回落默认 —— 与 proxyPool 的
/// 「解析失败按空表处理」同一恢复哲学：用户界面上的表现是「回到默认范围」，
/// 而不是内核起不来。
pub fn listener_port_range() -> (u16, u16) {
    let raw = read_raw();
    let base = port_of(raw.get("listenerPortBase"));
    let end = port_of(raw.get("listenerPortEnd"));
    match (base, end) {
        (Some(base), Some(end)) if base <= end => (base, end),
        _ => (DEFAULT_LISTENER_PORT_BASE, DEFAULT_LISTENER_PORT_END),
    }
}

/// 连通性测试的目标网址；空（未配置）返回 None，调用方用默认。
pub fn test_url() -> Option<String> {
    let text = read_raw()
        .get("testUrl")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

// ─── 公开形态（API 的读写契约）──────────────────────────────

/// 设置的公开形态（GET 响应体）。`defaults` 一并给出，前端弹窗的
/// placeholder 与「恢复默认」按钮读它 —— 后端是默认值的唯一事实来源。
pub fn get() -> Value {
    let (base, end) = listener_port_range();
    let raw = read_raw();
    json!({
        "listenerPortBase": base,
        "listenerPortEnd": end,
        // 空串 = 未自定义（用默认）；原样返回而不是解析成「当前生效值」，
        // 让「用户配没配过」与「当前生效的是什么」两个信息都在
        "testUrl": raw.get("testUrl").and_then(Value::as_str).unwrap_or(""),
        "defaults": {
            "listenerPortBase": DEFAULT_LISTENER_PORT_BASE,
            "listenerPortEnd": DEFAULT_LISTENER_PORT_END,
            "testUrl": DEFAULT_TEST_URL,
        },
    })
}

/// 保存（POST 的整份表单语义：带哪个键改哪个键 —— 弹窗一次提交全部字段，
/// 实际上就是整份覆盖，但按字段判空保留让将来加字段不必动这条链）。
/// 返回保存后的公开形态；Err 是一句可读的中文（前端原样显示）。
pub fn save(input: &Value) -> Result<Value, String> {
    let mut next = read_raw();
    if let Some(base_value) = input.get("listenerPortBase") {
        let base = port_of(Some(base_value))
            .ok_or_else(|| "端口范围必须是 1-65535 的整数".to_string())?;
        let end = port_of(next.get("listenerPortEnd")).unwrap_or(DEFAULT_LISTENER_PORT_END);
        if base > end {
            return Err("起始端口不能大于结束端口".to_string());
        }
        next["listenerPortBase"] = json!(base);
    }
    if let Some(end_value) = input.get("listenerPortEnd") {
        let end = port_of(Some(end_value))
            .ok_or_else(|| "端口范围必须是 1-65535 的整数".to_string())?;
        let base = port_of(next.get("listenerPortBase")).unwrap_or(DEFAULT_LISTENER_PORT_BASE);
        if base > end {
            return Err("起始端口不能大于结束端口".to_string());
        }
        next["listenerPortEnd"] = json!(end);
    }
    if let Some(url_value) = input.get("testUrl") {
        let text = url_value.as_str().unwrap_or("").trim().to_string();
        if !text.is_empty()
            && !text.starts_with("http://")
            && !text.starts_with("https://")
        {
            return Err("测试网址要以 http:// 或 https:// 开头".to_string());
        }
        let text: String = text.chars().take(MAX_URL_LENGTH).collect();
        next["testUrl"] = json!(text);
    }
    write_raw(&next)?;
    crate::server::logging::log(
        "[Proxies]",
        &format!("✅ 已更新代理设置（端口范围 {}-{}，测试网址「{}」）",
            next.get("listenerPortBase").and_then(Value::as_u64).unwrap_or(DEFAULT_LISTENER_PORT_BASE as u64),
            next.get("listenerPortEnd").and_then(Value::as_u64).unwrap_or(DEFAULT_LISTENER_PORT_END as u64),
            next.get("testUrl").and_then(Value::as_str).filter(|text| !text.is_empty()).unwrap_or("默认"),
        ),
    );
    Ok(get())
}
