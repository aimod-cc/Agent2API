//! 出网代理：账号级配置归一 / 解析 / 展示描述。
//!
//! 对照 Node 版 src/workbuddy-proxy.mjs 的「账号代理」部分（归一/解析/描述）。
//! 本模块只做**纯逻辑**：把账号里存的 proxy 配置解析成可用的连接信息。
//! Clash Verge 配置的真实读取（三个候选目录 + 两个 YAML + 3 秒 TTL 缓存）
//! 在 `core::clash`；真实出网（按出口缓存 reqwest Client）在 `core::egress`。
//! 三者拆分是因为它们的变化原因不同：本模块随**账号数据格式**变，
//! clash.rs 随**Clash 版本**变，egress.rs 随**HTTP 客户端实现**变。
//!
//! 账号记录里的 proxy 字段形态（null 表示无代理）：
//!   { source: 'clash',  listenerUid: '__mixed__' | '<Clash 节点名>' }
//!   { source: 'custom', protocol: 'http' | 'socks5', host, port, username?, password? }
//!   { source: 'pool',   proxyId: '<代理池条目 id>' }
//!   { source: 'rotate', all: true } | { source: 'rotate', all: false, proxyIds: ['<池条目 id>'] }
//!
//! `pool` 是「网络代理」页那批命名代理的**引用**（见 `core::proxy_pool`）：
//! 解析时去池里取条目、再按条目自己的 clash/custom 形态解析（端口实时读取、
//! 条目被删/被禁用的失败文案在 `proxy_pool::resolve_reference`）。入口放在
//! 这里而不是让每个调用方各查一次池：账号的展示描述、会话解析、出口测试
//! 三条链走的都是本模块，加一个分支就全部生效。
//! 注意 `proxy_pool::resolve_item` 反过来调本模块 —— 两条路径**不递归**：
//! 池条目的 source 只可能是 clash / custom（归一里挡掉了嵌套引用）。
//!
//! `rotate` 是上面的**轮询档**（见 `core::proxy_rotate`）：逐请求轮换池里的一条
//! 出口（`all: true` = 池里全部启用的条目），目前只对 OpenCode Zen 开放
//! （那个免费档按 IP 限流）。它比 `pool` 多一层「这次该用哪条」的取用动作，
//! 所以解析分成两个入口：`resolve_account_proxy_rotating`（转发等**逐次**链路，
//! 前进轮询游标）与 `resolve_account_proxy`（展示等**静态**链路，恒取第一条
//! 可用），两者的差别只在 rotate 分支上。

use serde_json::{json, Map, Value};

// Clash 常量与可选项从 core::clash 透出：账号模块与路由层一直从本模块引用
// 这些名字（clash_proxy_options / CLASH_MIXED_UID），保持这条路径能少改调用方。
// CLASH_UNAVAILABLE 的权威定义在 clash.rs，需要它的调用方从那里取 ——
// 这里不再二次导出，免得同一个常量有两条引用路径。
pub use crate::server::core::clash::{clash_proxy_options, CLASH_MIXED_UID};

const MAX_HOST_LENGTH: usize = 255;
const MAX_USER_LENGTH: usize = 200;
const MAX_LABEL_LENGTH: usize = 100;

/// 轮询档的 source 值（`core::proxy_rotate` 的配置形状）
pub const ROTATE_SOURCE: &str = "rotate";

/// 这条 proxy 配置是不是轮询档（判据只看 source；形状是否完整由归一侧保证）
pub fn is_rotate_config(config: &Value) -> bool {
    config.get("source").and_then(Value::as_str) == Some(ROTATE_SOURCE)
}

/// 代理配置错误（对应 Node 版 ProxyConfigError，状态码固定 400）。
///
/// 单独一个类型而不是塞进 GatewayError：账号路由要按它给出 400，
/// 而 GatewayError 的默认语义是 500（上游/内部错误）。
#[derive(Clone, Debug)]
pub struct ProxyConfigError {
    pub message: String,
    pub status_code: i32,
}

impl ProxyConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code: 400 }
    }
}

impl std::fmt::Display for ProxyConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

// ─── 归一与解析 ─────────────────────────────────────────────

/// 去空白并截断长度（对照 Node 版 `cleanString`：非字符串一律当空串）
fn clean_string(value: Option<&Value>, max: usize) -> String {
    let Some(Value::String(text)) = value else {
        return String::new();
    };
    let trimmed = text.trim();
    trimmed.chars().take(max).collect()
}

/// Node 的 `Number(value)` 语义下「是 1..65535 的整数」
fn valid_port(value: Option<&Value>) -> Option<u16> {
    let number = match value {
        Some(Value::Number(number)) => number.as_f64()?,
        // 与 Node 的 Number('8080') 一致：数字字符串也接受
        Some(Value::String(text)) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !number.is_finite() || number.fract() != 0.0 || !(1.0..=65535.0).contains(&number) {
        return None;
    }
    Some(number as u16)
}

/// 归一账号代理配置（来自 API 的原始输入）。
///
/// 返回 `Ok(None)`（无代理）或 `Ok(Some(标准形态))`；非法输入抛 ProxyConfigError
/// （→ HTTP 400）。`source` 缺省时按字段推断，便于手工编辑账号记录
/// （库里的 `data` 列，或导出文件的 JSON）。
pub fn normalize_account_proxy(input: &Value) -> Result<Option<Value>, ProxyConfigError> {
    if input.is_null() || input.as_str() == Some("") {
        return Ok(None);
    }
    let Some(object) = input.as_object() else {
        return Err(ProxyConfigError::new("代理配置必须是对象或 null"));
    };

    let explicit_source = clean_string(object.get("source"), 20).to_lowercase();
    let has_listener = object
        .get("listenerUid")
        .and_then(Value::as_str)
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false);
    let has_host = object
        .get("host")
        .and_then(Value::as_str)
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false);
    let source = if !explicit_source.is_empty() {
        explicit_source
    } else if has_listener {
        "clash".to_string()
    } else if has_host {
        "custom".to_string()
    } else {
        String::new()
    };
    if source.is_empty() {
        return Err(ProxyConfigError::new(
            "代理配置缺少 source（clash / custom / pool / rotate）",
        ));
    }

    // 轮询档：只存「怎么挑」（全部 or 指定哪几条），出口本身仍然全在池里。
    // 与 pool 引用同一条纪律：条目是否存在 / 是否禁用留给**解析**时报错，
    // 保存账号不依赖代理池当下的读库结果（池还没建好时也存得下这条配置）。
    if source == ROTATE_SOURCE {
        let all = match object.get("all") {
            None | Some(Value::Null) => true,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err(ProxyConfigError::new("轮询的 all 必须是布尔值")),
        };
        if all {
            // 「全部」恒不带 proxyIds：留着它会让「以后池里新增的条目要不要进轮询」
            // 变成一个没人回答的问题（两处状态取谁为准）。归一即收口成唯一形态
            return Ok(Some(json!({ "source": ROTATE_SOURCE, "all": true })));
        }
        let mut ids: Vec<String> = Vec::new();
        if let Some(items) = object.get("proxyIds") {
            let Some(items) = items.as_array() else {
                return Err(ProxyConfigError::new("轮询的 proxyIds 必须是数组"));
            };
            for item in items {
                let id = clean_string(Some(item), MAX_LABEL_LENGTH);
                if id.is_empty() || ids.iter().any(|existing| existing == &id) {
                    continue;
                }
                ids.push(id);
                if ids.len() >= crate::server::core::proxy_rotate::MAX_IDS {
                    break;
                }
            }
        }
        if ids.is_empty() {
            return Err(ProxyConfigError::new(
                "轮询至少要选择一条代理（或改选「全部」）",
            ));
        }
        return Ok(Some(json!({
            "source": ROTATE_SOURCE,
            "all": false,
            "proxyIds": ids,
        })));
    }

    if source == "clash" {
        let listener_uid = clean_string(object.get("listenerUid"), MAX_LABEL_LENGTH);
        if listener_uid.is_empty() {
            return Err(ProxyConfigError::new("缺少 Clash 监听器 uid"));
        }
        return Ok(Some(json!({ "source": "clash", "listenerUid": listener_uid })));
    }
    // 代理池引用：只存条目 id（形状校验；条目是否存在 / 是否禁用留给**解析**时
    // 报错 —— 与本模块的纯逻辑定位一致，也不让「保存账号」依赖代理池的读库）
    if source == "pool" {
        let proxy_id = clean_string(object.get("proxyId"), MAX_LABEL_LENGTH);
        if proxy_id.is_empty() {
            return Err(ProxyConfigError::new("缺少代理池条目 id"));
        }
        return Ok(Some(json!({ "source": "pool", "proxyId": proxy_id })));
    }
    if source != "custom" {
        return Err(ProxyConfigError::new(format!("不支持的代理来源: {source}")));
    }

    let protocol = {
        let cleaned = clean_string(object.get("protocol"), 10).to_lowercase();
        if cleaned.is_empty() { "http".to_string() } else { cleaned }
    };
    if protocol != "http" && protocol != "socks5" {
        return Err(ProxyConfigError::new("代理协议只支持 http 或 socks5"));
    }
    let host = clean_string(object.get("host"), MAX_HOST_LENGTH);
    if host.is_empty() {
        return Err(ProxyConfigError::new("缺少代理主机地址"));
    }
    let Some(port) = valid_port(object.get("port")) else {
        return Err(ProxyConfigError::new("代理端口必须是 1-65535 的整数"));
    };
    let label = clean_string(object.get("label"), MAX_LABEL_LENGTH);
    // label 缺失时按 `协议://主机:端口` 生成，前端直接展示这个串
    let label = if label.is_empty() {
        format!("{protocol}://{host}:{port}")
    } else {
        label
    };

    let mut normalized = Map::new();
    normalized.insert("source".to_string(), Value::String("custom".to_string()));
    normalized.insert("protocol".to_string(), Value::String(protocol));
    normalized.insert("host".to_string(), Value::String(host));
    normalized.insert("port".to_string(), Value::from(port));
    normalized.insert(
        "username".to_string(),
        Value::String(clean_string(object.get("username"), MAX_USER_LENGTH)),
    );
    normalized.insert(
        "password".to_string(),
        Value::String(clean_string(object.get("password"), MAX_USER_LENGTH)),
    );
    normalized.insert("label".to_string(), Value::String(label));
    Ok(Some(Value::Object(normalized)))
}

/// 解析结果：成功时给出可用的出口，失败时 `error` 说明原因
/// （调用方据此回退直连并记日志，对应 Node 版 `{ error }` 分支）。
///
/// `port` 是 `Option<u16>`：Node 的 custom 分支直接写 `Number(config.port)`，
/// 手工编辑出的非法端口会变成 `NaN` → JSON `null`。这里如实保留那个 null，
/// 而不是伪造一个 0 —— 前端拿到 null 才知道「这条记录本来就坏」。
#[derive(Clone, Debug)]
pub struct ResolvedProxy {
    pub source: String,
    pub protocol: String,
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub label: String,
}

impl ResolvedProxy {
    /// 端口的 JSON 形态（非法/缺失 → null，与 Node 的 NaN → null 一致）
    pub fn port_json(&self) -> Value {
        self.port.map(Value::from).unwrap_or(Value::Null)
    }

    /// 从 JSON 形态还原（会话里的 `proxy` 字段就是这个形态，
    /// 由 account_store 的 `session_from_record` 用 `to_json` 的成功分支写出）。
    ///
    /// 三态：`Ok(None)` = 没有代理（直连）；`Ok(Some(...))` = 可用出口；
    /// `Err(原因)` = **配了代理但数据坏了**（缺主机 / 端口非法）。
    /// 第三态必须与「没配代理」分开：手工编辑账号记录（库里 `data` 列的
    /// `proxy` 字段，或导出文件）写出 `port: "abc"` 时，Node 会带着 `NaN`
    /// 端口去建 ProxyAgent 并失败（出口测试显示「❌ 无法连接」）—— 若这里
    /// 静默按直连处理，用户会看到「✅ 出口可用」，那是在骗人。
    /// 出网侧的调用方遇到 Err 时按直连兜底（可用性优先），
    /// 但**出口测试**要把原因如实报出来。
    pub fn from_json(value: &Value) -> Result<Option<Self>, String> {
        if value.is_null() {
            return Ok(None);
        }
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let host = text("host");
        if host.is_empty() {
            return Err("代理配置缺少主机地址".to_string());
        }
        let Some(port) = valid_port(value.get("port")) else {
            return Err("代理端口必须是 1-65535 的整数".to_string());
        };
        let protocol = {
            let protocol = text("protocol");
            if protocol.is_empty() { "http".to_string() } else { protocol }
        };
        Ok(Some(ResolvedProxy {
            source: text("source"),
            protocol,
            host,
            port: Some(port),
            username: text("username"),
            password: text("password"),
            label: text("label"),
        }))
    }
}

/// 会话里的出口（`session.proxy`）。
///
/// 账号存储组装的会话已经带上了 `proxy`（解析成功）或 `proxyError`（解析失败，
/// 此时 `proxy` 为 null）—— 所以拿到 None 就表示「直连」：没配代理、
/// 配了但解析失败（如引用了已被删掉的 Clash 监听器）、以及配了但数据坏了，
/// 这三种情况在出网层是同一个动作。第三种会额外记一条日志提醒。
pub fn session_proxy(session: &Value) -> Option<ResolvedProxy> {
    match ResolvedProxy::from_json(session.get("proxy").unwrap_or(&Value::Null)) {
        Ok(proxy) => proxy,
        Err(reason) => {
            crate::server::logging::log(
                "[Upstream]",
                &format!("⚠️ 账号代理不可用（{reason}），本次回退直连"),
            );
            None
        }
    }
}

/// JS 真值判定（`Boolean(x)`）：null/false/0/"" 为假，其余（含数组/对象）为真。
/// 用于复刻 Node 的 `config.label || ...` 这类短路取值。
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|item| item != 0.0).unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// JS 模板串里插值的等效渲染：`a` 缺失是 `undefined`、显式 null 是 `null`、
/// 其余按 `String(a)` 的字面量（对象/数组在 JS 里是 `[object Object]` /
/// 逗号拼接，这里用 JSON 文本近似 —— 这些形态只可能来自手工改坏的账号记录，
/// 近似的目的是「不要把值吞掉」，而不是逐字复刻 JS 的 toString）。
/// 只用于复刻 Node 拼接 label / 报错文案的行为（见 `resolve_account_proxy`）。
fn js_interpolation(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Object(_)) => "[object Object]".to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// 解析失败的原因（与成功结果互斥）
#[derive(Clone, Debug)]
pub enum ProxyResolution {
    Resolved(ResolvedProxy),
    Failed(String),
}

impl ProxyResolution {
    pub fn error(&self) -> Option<&str> {
        match self {
            ProxyResolution::Resolved(_) => None,
            ProxyResolution::Failed(message) => Some(message.as_str()),
        }
    }

    pub fn resolved(&self) -> Option<&ResolvedProxy> {
        match self {
            ProxyResolution::Resolved(proxy) => Some(proxy),
            ProxyResolution::Failed(_) => None,
        }
    }

    /// 转成 Node 版 resolveAccountProxy 的返回形态（JSON，含 error 分支）。
    /// 账号公开形态走 `describe_account_proxy`（它另带 config/label 兜底），
    /// 这条与它逐字段对应，保留供排障时直接比对解析结果。
    #[allow(dead_code)]
    pub fn to_json(&self) -> Value {
        match self {
            ProxyResolution::Resolved(proxy) => json!({
                "source": proxy.source,
                "protocol": proxy.protocol,
                "host": proxy.host,
                "port": proxy.port_json(),
                "username": proxy.username,
                "password": proxy.password,
                "label": proxy.label,
            }),
            ProxyResolution::Failed(message) => json!({ "error": message }),
        }
    }
}

/// 把账号里存的代理配置解析成可用的连接信息。
///
/// 逐字段复刻 Node 版 `resolveAccountProxy`，**包括它对脏数据的宽容**：
///   - protocol 只认 `socks5`，其余（含 undefined）都当 http
///   - host 原样透出（Node 不做字符串化，非字符串会变成 undefined → 丢失）
///   - port 走 `Number()`：非数字得 NaN → JSON null（不是 0）
///   - label 缺省时按 JS 模板串拼接，所以 `undefined://bad:undefined` 这种
///     输出是**预期行为** —— 它恰好告诉用户这条记录本身就坏（前端显示为
///     「出口 undefined://…」比显示一个编造的默认值更容易排障）
///
/// clash 类型从 Clash Verge 快照实时取端口（快照由 `core::clash` 维护，
/// 含 3 秒 TTL 缓存）；取不到时返回 Failed，调用方回退直连并提示。
pub fn resolve_account_proxy(config: Option<&Value>) -> Option<ProxyResolution> {
    let config = config?;
    if config.is_null() {
        return None;
    }
    // 非对象（字符串/数字/数组）在 JS 里取 `.source` 得 undefined，
    // 于是落到「不支持的代理来源: undefined」—— 手工编辑账号记录
    // 写错形状时就是这条文案，照抄不改成更「友好」的提示
    let Some(object) = config.as_object() else {
        return Some(ProxyResolution::Failed("不支持的代理来源: undefined".to_string()));
    };
    // source 缺失时同样是 undefined（Node 是 `config.source` 直接进模板串）。
    // 这条文案会显示在账号列表的「代理异常」气泡里，所以必须逐字一致 ——
    // 给空串会让用户看到「不支持的代理来源: 」这种像是程序坏了的提示
    let source = object
        .get("source")
        .map(|value| js_interpolation(Some(value)))
        .unwrap_or_else(|| "undefined".to_string());

    // 轮询档：静态取第一条可用（**不动游标**）。逐请求的轮询走
    // `resolve_account_proxy_rotating`；这条留给展示与「一次调用不代表一次转发」
    // 的链路（见模块头）—— 它们问的是「这条配置现在能不能用」，不是「这次该走哪条」。
    if source == ROTATE_SOURCE {
        return Some(
            match crate::server::core::proxy_rotate::first_usable(config) {
                Ok(proxy) => ProxyResolution::Resolved(proxy),
                Err(reason) => ProxyResolution::Failed(reason),
            },
        );
    }

    // 代理池引用：去池里取条目、按条目自己的 clash / custom 形态解析。
    // 失败文案（不存在 / 已禁用 / Clash 监听器没了）由 proxy_pool 给出，
    // 这里只做转手 —— 账号列表的「代理异常」气泡、转发时的回退日志都读它。
    if source == "pool" {
        let proxy_id = object.get("proxyId").and_then(Value::as_str).unwrap_or("");
        if proxy_id.is_empty() {
            return Some(ProxyResolution::Failed("代理引用缺少条目 id".to_string()));
        }
        return Some(
            match crate::server::core::proxy_pool::resolve_reference(proxy_id) {
                Ok(proxy) => ProxyResolution::Resolved(proxy),
                Err(reason) => ProxyResolution::Failed(reason),
            },
        );
    }

    if source == "custom" {
        let protocol = if object.get("protocol").and_then(Value::as_str) == Some("socks5") {
            "socks5"
        } else {
            "http"
        };
        // host 原样透出：非字符串时 Node 会把数字/布尔照透给 JSON，但那种值
        // 无法用作主机名，这里统一给空串 —— 调用方（session_proxy）见到空 host
        // 就回退直连并记日志，比带着 `host: 123` 去连一个好
        let host = object.get("host").and_then(Value::as_str).unwrap_or("").to_string();
        let port = valid_port(object.get("port"));
        // label 缺省时按 JS 模板串拼接（未设置的值渲染成 undefined）；
        // 显式给了真值就用它（Node 是 `config.label || \`...\``，数字/布尔这类
        // 真值也会被原样采用，这里用 js_interpolation 取同形态的文本）
        let label = match object.get("label") {
            Some(value) if js_truthy(value) => js_interpolation(Some(value)),
            _ => format!(
                "{}://{}:{}",
                js_interpolation(object.get("protocol")),
                js_interpolation(object.get("host")),
                js_interpolation(object.get("port")),
            ),
        };
        return Some(ProxyResolution::Resolved(ResolvedProxy {
            source: "custom".to_string(),
            protocol: protocol.to_string(),
            host,
            port,
            username: object
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            password: object
                .get("password")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            label,
        }));
    }
    if source != "clash" {
        return Some(ProxyResolution::Failed(format!("不支持的代理来源: {source}")));
    }

    let snapshot = crate::server::core::clash::clash_snapshot();
    if !snapshot.available {
        let detail = snapshot
            .error
            .as_ref()
            .map(|error| format!("（{error}）"))
            .unwrap_or_default();
        return Some(ProxyResolution::Failed(format!("Clash Verge 配置不可用{detail}")));
    }
    let listener_uid = object.get("listenerUid").and_then(Value::as_str).unwrap_or("");
    if listener_uid == CLASH_MIXED_UID {
        let Some(port) = snapshot.mixed_port else {
            return Some(ProxyResolution::Failed("Clash Verge 未启用混合端口".to_string()));
        };
        return Some(ProxyResolution::Resolved(ResolvedProxy {
            source: "clash".to_string(),
            protocol: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port: Some(port),
            username: String::new(),
            password: String::new(),
            label: format!("Clash 混合端口 {port}"),
        }));
    }
    let Some(listener) = snapshot.listeners.iter().find(|item| item.uid == listener_uid) else {
        return Some(ProxyResolution::Failed(format!(
            "Clash Verge 中找不到监听器「{listener_uid}」（可能已在 Clash 中删除）"
        )));
    };
    if !listener.enabled {
        return Some(ProxyResolution::Failed(format!(
            "Clash Verge 监听器「{}」已禁用",
            listener.name
        )));
    }
    Some(ProxyResolution::Resolved(ResolvedProxy {
        source: "clash".to_string(),
        protocol: "http".to_string(),
        host: "127.0.0.1".to_string(),
        port: Some(listener.port),
        username: String::new(),
        password: String::new(),
        label: format!("{}（:{}）", listener.name, listener.port),
    }))
}

/// **逐次**解析：与 [`resolve_account_proxy`] 只差 rotate 档——它按 `rotate_key`
/// 取下一个出口（前进轮询游标），其余 source 逐字委托给静态那条。
///
/// `rotate_key` 是轮询进度的归属键（**账号 id**）：同一账号的多条请求依次落到
/// 不同出口，不同账号各转各的（见 `core::proxy_rotate` 的模块头）。
/// 只有真正代表一次出网的链路（会话组装、转发重取）才该走这条：展示、
/// 余额查询这类「一次调用不代表一次转发」的地方用静态解析，否则账号列表
/// 一刷新就把游标推走好几格（轮询的意义就是「相邻两次出网落在不同 IP」，被
/// 界面读一次就错位，等于白转）。
pub fn resolve_account_proxy_rotating(
    config: Option<&Value>,
    rotate_key: &str,
) -> Option<ProxyResolution> {
    let config = config?;
    if config.is_null() {
        return None;
    }
    if is_rotate_config(config) {
        return Some(match crate::server::core::proxy_rotate::next(config, rotate_key) {
            Ok(proxy) => ProxyResolution::Resolved(proxy),
            Err(reason) => ProxyResolution::Failed(reason),
        });
    }
    resolve_account_proxy(Some(config))
}

/// 账号代理的展示描述（公开形态，不带单独字段的密码 —— 与 Node 版一致，
/// 密码只在 config 里原样带回）。
///
/// 无代理返回 Null；解析失败时 `label` 为「解析失败」并附 `error`
/// （前端据此提示「转发时会回退直连」）。
///
/// 轮询档单独一支（见下）：它的「当前出口」是个随机进度的概念，展示要回答的是
/// 「轮询了几条、都可用吗」，不能借用某一次解析的 label 冒充「当前出口」。
pub fn describe_account_proxy(config: Option<&Value>) -> Value {
    let Some(config) = config.filter(|value| !value.is_null()) else {
        return Value::Null;
    };
    if is_rotate_config(config) {
        return describe_rotate_proxy(config);
    }
    let Some(resolution) = resolve_account_proxy(Some(config)) else {
        return Value::Null;
    };
    let source = config
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match &resolution {
        ProxyResolution::Failed(message) => json!({
            "source": source,
            "label": "解析失败",
            "error": message,
            "config": config,
        }),
        ProxyResolution::Resolved(proxy) => json!({
            // source 用**账号里配的那个**（pool / clash / custom）：pool 引用
            // 解析成功后底层是 clash / custom，但界面上要如实显示「引用了
            // 代理池的某条」—— 前端据此在代理列上标出来。手工改坏的记录
            // （source 缺失）才回落到解析结果的 source
            "source": if source.is_empty() { proxy.source.clone() } else { source.clone() },
            "protocol": proxy.protocol,
            "host": proxy.host,
            "port": proxy.port,
            "label": proxy.label,
            "error": Value::Null,
            "config": config,
        }),
    }
}

/// 轮询档的展示形态（`describe_account_proxy` 的 rotate 分支）。
///
/// 字段与其它档同形（`source` / `label` / `error` / `config`），**不**给
/// host / port：轮询没有「当前出口」这个读数（下一次请求可能就换了），给一个
/// 只会让人以为「这条代理就是那个地址」。多的 `rotate` 子对象是给界面的读数：
/// 总共几条、几条例看起来可用（判据见 `proxy_rotate::entry_ready`）。
///
/// `label` 是账号列表代理格与弹窗直接显示的文案；`error` 只在**一条候选都用
/// 不了**时给出 —— 那时转发会回退直连，账号列表按既有口径标红提示
/// （与单条引用档「代理不可用」同一形制）。
fn describe_rotate_proxy(config: &Value) -> Value {
    let summary = crate::server::core::proxy_rotate::summary(config);
    let label = if summary.all {
        format!("轮询全部代理（{} 个出口）", summary.total)
    } else {
        format!("轮询指定代理（{} 个出口）", summary.total)
    };
    json!({
        "source": ROTATE_SOURCE,
        "label": label,
        "error": summary.error,
        "config": config,
        "rotate": {
            "all": summary.all,
            "total": summary.total,
            "usable": summary.usable,
        },
    })
}
