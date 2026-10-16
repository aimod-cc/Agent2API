//! ZCode 的**端点映射**：跟随上游 `GET /api/v1/agent/configs` 下发的
//! `proxyEndpoint.mapping` 表，把推理请求的 URL 改写到上游指定的地址。
//!
//! ── 这是什么（issue #232 / #134 的另一半背景）────────────────
//! 官方客户端 3.7+ 有一套 `ProviderEndpointRoutingService`：客户端周期性取
//! `GET {zcode}/api/v1/agent/configs`，按响应里 `data.proxyEndpoint.mapping`
//! 的 `from → to` 表（规范化 URL 精确匹配）改写 provider 请求的地址。
//! 参考实现 `Acankao/zcode-api` 的 `src/proxy/endpoint-routing.ts` 复刻了它，
//! 并记录：2026-08-19 起服务端把 coding-plan 的 Anthropic 端点映射到
//! `zcode.z.ai/api/v1/ultra[-zai]/...` —— 也就是说上游正在把推理流量从
//! 开放平台（`open.bigmodel.cn` / `api.z.ai`）往 `zcode.z.ai` 搬，官方文档里
//! 「编程套餐账号授权连接时端点由 ZCode 自动路由」说的就是这张表。本模块
//! 让网关跟随同一张表，上游迁移时不需要等发版。
//!
//! ── fail-open：任何一步失败都**原样放行** ────────────────────
//! 没拉到快照、URL 解析失败、表里没这条 —— 一律返回原 URL。改写是
//! 「上游说了才做」的优化，永远不能变成「网关自造的故障」：参考实现同口径
//! （握手失败、解析失败都保持原 URL，失败后还有冷却期）。`apply` 因此是
//! 纯同步查表：热路径上没有网络、没有重试。
//!
//! ── 表的形状与匹配规则（与参考实现逐条对齐）─────────────────
//!   · key = 规范化的请求 URL：`https://host:port/path`（scheme/host 小写、
//!     显式端口缺省 443、path 去尾斜杠）—— 参考实现的 `routingKey`；
//!   · 只认 `https`（参考实现的 `parseMappingUrl` 拒绝其它协议）；
//!   · 改写 = 换 origin+path，**保留原请求的 query**（参考实现
//!     `rewritten.search = parsed.search`：to 上的 query 丢弃）；
//!   · 表上限 256 条（防上游被玩坏时撑爆内存，与参考实现同值）；坏条目
//!     跳过、不拖垮整表 —— 参考实现是整次 fetch 失败，这里更宽松一点，
//!     方向仍是「上游说了多少、我们认多少」。
//!
//! ── 逃生口 ──────────────────────────────────────────────────
//! `ZCODE_DISABLE_ENDPOINT_MAPPING` / `ZCODE_INTL_DISABLE_ENDPOINT_MAPPING`
//! （国内 / 国际各一，见 `Region::env_override`）设了值即停用：同步循环不再
//! 拉取、并清掉已有快照。上游哪天下发了一张会把我们打挂的表，用户不必等
//! 修复版。禁用判定在**同步侧**做（低频），`apply` 热路径零开销。
//!
//! ── 凭证与出口 ──────────────────────────────────────────────
//! 参考实现在取配置时带 `x-api-key`（编码套餐 API Key，客户端 `sourceHeaders`
//! 的口径），本模块照带 —— 代表账号由 `config_sync` 挑（有 API Key 的启用账号）。
//! 出网代理跟账号走（与余额 / 领取同一条理由：`zcode.z.ai` 对国内用户常常
//! 需要代理）。同步间隔复用 [`super::config_sync`] 的 5 分钟循环。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，取值一律走 Option 链。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;

use crate::server::core::auth_http::send_raw;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::region::Region;

/// `GET /api/v1/agent/configs`（参考实现 `CONFIG_PATH`，与验证码配置的
/// `client/configs` 是**两个**接口：这份要凭证、那份匿名）
const AGENT_CONFIGS_PATH: &str = "/api/v1/agent/configs";

/// 单次请求超时。参考实现给 3 秒（它是请求路径上的懒刷新，卡不得）；我们是
/// 后台循环，5 秒给国内网络留点余量，失败也只是保持现值。
const REQUEST_TIMEOUT_MS: u64 = 5_000;

/// 表上限（与参考实现 `MAX_MAPPING_ENTRIES` 同值，见模块头）
const MAX_ENTRIES: usize = 256;

/// 按地区存的快照：`None`（缺键）= 从没拉到过。值用 `Arc` 让 `apply` 的读
/// （持锁）只做一次 clone-free 查表。
static SNAPSHOTS: OnceLock<RwLock<HashMap<Region, Arc<HashMap<String, String>>>>> = OnceLock::new();

fn snapshots() -> &'static RwLock<HashMap<Region, Arc<HashMap<String, String>>>> {
    SNAPSHOTS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 环境变量停用开关（见模块头「逃生口」）。只在同步循环里调。
pub(super) fn disabled(region: Region) -> bool {
    region.env_override("DISABLE_ENDPOINT_MAPPING").is_some()
}

/// 清掉某地区的快照（停用开关被打开时由同步循环调）。
pub(super) fn clear(region: Region) {
    let removed = {
        let mut guard = match snapshots().write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(&region).is_some()
    };
    if removed {
        logging::log(
            "[ZCode]",
            &format!("{} 的推理端点映射已停用（环境变量开关）", region.label()),
        );
    }
}

/// 对一条**推理请求 URL** 做映射改写（fail-open，见模块头）。
///
/// 两条通道都过这里：编码套餐的开放平台 URL 是映射的主要对象，套餐池
/// （`zcode.z.ai`）的 URL 若也被上游映射（表是服务端控制的、可能增长），
/// 跟随同一张表与官方客户端的语义一致。
pub fn apply(region: Region, url: &str) -> String {
    let Some(key) = routing_key(url) else {
        return url.to_string();
    };
    let guard = match snapshots().read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(target) = guard.get(&region).and_then(|table| table.get(&key)) else {
        return url.to_string();
    };
    rewrite_with_query(target, url)
}

/// 拉取一份映射并落快照。返回 `(表条数, 是否有变化)`。
///
/// 上游没下发 `proxyEndpoint.mapping`（或结构认不出）按**空表**收：那是
/// 「上游现在没有映射」，把旧快照清掉才与上游口径一致；网络失败则**不动
/// 现值**（fail-open 的另一半：拉不到不等于没有）。
pub(super) async fn refresh(
    region: Region,
    credential: Option<&str>,
    proxy: Option<&ResolvedProxy>,
) -> Result<(usize, bool), GatewayError> {
    let url = format!(
        "{}{AGENT_CONFIGS_PATH}?app_version={}&platform={}",
        region.zcode_origin(),
        super::claim::urlencode(&super::claim::app_version()),
        super::claim::urlencode(super::claim::platform()),
    );
    // 头：客户端身份族 + 可选 `x-api-key`（编码套餐 API Key，参考实现口径）
    let mut headers = super::adapter::identity_headers(None);
    if let Some(key) = credential.map(str::trim).filter(|value| !value.is_empty()) {
        headers.push(("x-api-key".to_string(), key.to_string()));
    }
    let response = send_raw("GET", &url, None, &headers, proxy, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, "拉取端点映射超时")
            } else {
                GatewayError::with_status(502, format!("拉取端点映射失败: {error}"))
            }
        })?;
    if !(200..300).contains(&response.status) {
        return Err(GatewayError::with_status(
            502,
            format!("拉取端点映射失败: HTTP {}", response.status),
        ));
    }
    let payload = response.payload.unwrap_or(Value::Null);
    let table = parse_mapping(&payload);
    let count = table.len();
    let mut guard = match snapshots().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let changed = match guard.get(&region) {
        Some(existing) => existing.as_ref() != &table,
        None => true,
    };
    if changed {
        guard.insert(region, Arc::new(table));
    }
    Ok((count, changed))
}

/// 从整份响应里解析映射表。上游没给这一段 = 空表（语义见 [`refresh`]）。
fn parse_mapping(payload: &Value) -> HashMap<String, String> {
    let mut table = HashMap::new();
    let Some(entries) = payload
        .get("data")
        .and_then(|value| value.get("proxyEndpoint"))
        .and_then(|value| value.get("mapping"))
        .and_then(Value::as_array)
    else {
        return table;
    };
    for entry in entries {
        if table.len() >= MAX_ENTRIES {
            break;
        }
        let from = entry.get("from").and_then(Value::as_str).unwrap_or("");
        let to = entry.get("to").and_then(Value::as_str).unwrap_or("");
        // 坏条目跳过（见模块头：不拖垮整表）；from 还要排 query/fragment ——
        // 映射按「origin+path」匹配，带 query 的 from 永远命中不了
        let (Some(key), Some(target)) = (routing_key(from), origin_and_path(to)) else {
            continue;
        };
        table.insert(key, target);
    }
    table
}

/// 规范化 URL 为映射表的 key（参考实现 `routingKey` 的移植，见模块头）。
fn routing_key(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.trim().to_lowercase();
    if host.is_empty() {
        return None;
    }
    // https 的显式端口与缺省等价：`port()` 对默认端口返回 None，统一成 443
    let port = parsed.port().unwrap_or(443);
    Some(format!("https://{}:{port}{}", host, normalize_path(parsed.path())))
}

/// 提取 to 目标的 `scheme://host[:port]/path`（query 与 fragment 丢弃，
/// 见 [`apply`] 的改写语义）。https 之外的协议一律拒绝。
fn origin_and_path(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.trim().to_lowercase();
    if host.is_empty() {
        return None;
    }
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Some(format!("https://{authority}{}", normalize_path(parsed.path())))
}

/// path 去尾斜杠（根 `/` 保留，与参考实现 `normalizePath` 同口径）
fn normalize_path(path: &str) -> String {
    let mut path = path.to_string();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    path
}

/// 改写后保留原请求的 query（见 [`apply`]）。
fn rewrite_with_query(target: &str, original: &str) -> String {
    let query = url::Url::parse(original)
        .ok()
        .and_then(|parsed| parsed.query().map(str::to_string));
    match query {
        Some(query) if !query.is_empty() => {
            let separator = if target.contains('?') { "&" } else { "?" };
            format!("{target}{separator}{query}")
        }
        _ => target.to_string(),
    }
}
