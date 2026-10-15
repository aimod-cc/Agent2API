//! 出网代理路由（对照 workbuddy-account-routes.mjs 的 `tryHandleProxies`）。
//!
//!   GET  /api/proxies            可选项（Clash Verge 实时快照）+ 各账号当前出口
//!   POST /api/proxies/test       测试出口连通性并返回出口 IP
//!   GET  /api/proxies/pool       代理池列表（进来时自动同步一次 Clash 出口）
//!   POST /api/proxies/pool       新建池条目（只收手动条目）
//!   POST /api/proxies/pool/update 编辑池条目（body 带 id；Clash 同步来的不可改）
//!   POST /api/proxies/pool/remove 删除池条目（body 带 id；Clash 同步来的不可删）
//!   POST /api/proxies/pool/test  测试池条目并把结果记进条目（订阅节点还没有
//!                                监听器时先钉住 + 补建 + 等端口就绪再测）
//!   POST /api/proxies/pool/sync-clash 手动同步 Clash Verge 出口
//!   POST /api/proxies/pool/provision 补建订阅节点监听器（lazy listeners 的
//!                                    主动入口：账号保存 / 测试前调用；body 可带
//!                                    `{ids}` 把一批条目一并补建并等就绪）
//!   GET  /api/proxies/settings   代理设置（节点端口范围 + 测试网址 + 默认值）
//!   POST /api/proxies/settings   保存代理设置（整份表单；非法形状 400）
//!
//! 前两条是账号侧的出口能力（Node 版原有）；`pool` 那六条是「网络代理」页
//! 的代理池管理（对照 OmniProxy 的 proxies CRUD + sync-clash + 测试，存储、
//! 只读语义与镜像规则见 `core::proxy_pool`）。路径用固定段 + `body` 传 id
//! 而不是 `/pool/{id}`：http.rs 的路由按固定路径注册，通配段会与「已注册路径
//! 上的其它方法 → 404 信封」那条兜底互相干扰（见 http.rs 的注册注释）。
//!
//! 从 accounts.rs 拆出（单文件行数约定）。Node 版也把这两条放在
//! `tryHandleProxies` 里，与 `tryHandle`（账号 CRUD）分开 —— 拆分点与它一致。
//!
//! 入口 `proxies_entry` 仍在 accounts.rs：路由注册在 http.rs 里按模块分组，
//! 各条 proxies 路径与账号路径挨着注册更便于对照 Node 的判定顺序。

use axum::body::Bytes;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::egress;
use crate::server::core::proxies::{normalize_account_proxy, ResolvedProxy};
use crate::server::core::proxy_pool;
use crate::server::core::proxy_settings;
use crate::server::core::sub_kernel;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

use super::accounts::proxy_error;

// ─── GET /api/proxies ───────────────────────────────────────

/// 出网代理可选项。
///
/// `clash_proxy_options()` 实时读 Clash Verge 配置（三个候选目录 + 3 秒 TTL 缓存）：
/// 未安装 Clash 时 `clash.available=false` + `error="未找到 Clash Verge 配置"` +
/// `options=[]`；装了则给出「混合端口 + 各监听器」两份可选项
/// （前端 proxy-form.js 直接读 `.options.length`，所以空态也必须是数组）。
/// accounts 里每个账号带上 priority/enabled/proxy（proxy 已由 store 描述过，
/// 含「解析失败」分支）。
pub async fn list_proxies(state: &ServerState) -> Response {
    let clash = crate::server::core::proxies::clash_proxy_options();
    let snapshot = state.store().list_accounts();
    let accounts: Vec<Value> = snapshot
        .get("accounts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|account| {
                    // name：统一展示名优先（与账号页同口径，见
                    // `store_view::display_name_of`），缺键回落记录 name
                    let name = account
                        .get("displayName")
                        .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                        .cloned()
                        .or_else(|| account.get("name").cloned())
                        .unwrap_or(Value::Null);
                    json!({
                        "id": account.get("id").cloned().unwrap_or(Value::Null),
                        "name": name,
                        "priority": account.get("priority").cloned().unwrap_or(Value::Null),
                        "enabled": account.get("enabled").cloned().unwrap_or(Value::Null),
                        "proxy": account.get("proxy").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    ok_json(json!({
        "clash": clash,
        "accounts": accounts,
        "note": "账号默认无代理（直连）；选择 Clash 项后端口由 Clash Verge 管理，这里实时读取",
    }))
}

// ─── POST /api/proxies/test ─────────────────────────────────

/// 出网连通性测试（对照 account-routes.mjs 的 tryHandleProxies test 分支）。
///
/// 两种用法：
///   `{ id }`     测该账号的出网代理（账号不存在 → 404；代理解析失败 → 400）
///   `{ proxy }`  测临时配置（含 `proxy: null` = 测直连）
///
/// 判据是「能否连上 WorkBuddy 上游」，而不是能否访问第三方站点（理由见
/// `core::egress::test_connectivity` 的注释）；出口 IP 是附加信息。
/// 响应固定 200 + `{success, status?, ip?, durationMs, error?, proxy}` ——
/// **测试失败也是 200**（`success:false` + error 文案），前端据此显示红色提示。
pub async fn test_proxy(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let mut proxy: Option<crate::server::core::proxies::ResolvedProxy> = None;
    if let Some(id) = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let Some(creds) = state.store().get_credentials_by_id(id) else {
            return management_error(404, "账号不存在");
        };
        if let Some(error) = creds.proxy_error {
            return management_error(400, error);
        }
        // 账号记录里的出口已经是解析后的形态（带 label）
        match crate::server::core::proxies::ResolvedProxy::from_json(&creds.proxy) {
            Ok(resolved) => proxy = resolved,
            Err(reason) => return management_error(400, reason),
        }
    } else if let Some(config_value) = payload.get("proxy") {
        let config = match normalize_account_proxy(config_value) {
            Ok(value) => value,
            Err(error) => return proxy_error(error),
        };
        if let Some(config) = config {
            if let Some(resolution) =
                crate::server::core::proxies::resolve_account_proxy(Some(&config))
            {
                if let Some(error) = resolution.error() {
                    return management_error(400, error);
                }
                proxy = resolution.resolved().cloned();
            }
        }
        // config 为 None（payload.proxy === null）= 测直连，proxy 保持 None
    }

    let result = egress::test_proxy(proxy.as_ref()).await;
    let outcome = if result.success {
        format!("✅ 出口 IP {}", result.ip)
    } else {
        format!("❌ {}", result.error.clone().unwrap_or_default())
    };
    logging::log(
        "[Accounts]",
        &format!(
            "代理测试{}: {outcome}",
            match &proxy {
                Some(proxy) if !proxy.label.is_empty() => format!("（{}）", proxy.label),
                Some(proxy) => format!("（{}）", proxy.host),
                None => "（直连）".to_string(),
            }
        ),
    );
    let mut data = result.to_json();
    if let Some(object) = data.as_object_mut() {
        object.insert("proxy".to_string(), egress::describe_public(proxy.as_ref()));
    }
    ok_json(data)
}

// ─── 代理池（「网络代理」页）─────────────────────────────────

/// 引用统计：`proxyId → [{id, name}]`（引用它的账号）。
///
/// 只认 `proxy.config.source == "pool"` 的记录：账号里存的 pool 引用在
/// describe 之后的形态是 `{source:'pool', ..., config:{source:'pool', proxyId}}`
/// （见 `core::proxies::describe_account_proxy`）—— 解析失败的记录 config 也在，
/// 所以「引用了一个已被删掉的代理」同样统计得到（那正是最需要提醒用户的情况）。
fn pool_references(state: &ServerState) -> std::collections::HashMap<String, Vec<Value>> {
    let mut references: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    let snapshot = state.store().list_accounts();
    let Some(accounts) = snapshot.get("accounts").and_then(Value::as_array) else {
        return references;
    };
    for account in accounts {
        let Some(config) = account.get("proxy").and_then(|proxy| proxy.get("config")) else {
            continue;
        };
        if config.get("source").and_then(Value::as_str) != Some("pool") {
            continue;
        }
        let Some(proxy_id) = config.get("proxyId").and_then(Value::as_str) else {
            continue;
        };
        references.entry(proxy_id.to_string()).or_default().push(json!({
            "id": account.get("id").cloned().unwrap_or(Value::Null),
            // 统一展示名优先（与列表接口同一取舍，见上面的说明）
            "name": account
                .get("displayName")
                .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                .cloned()
                .or_else(|| account.get("name").cloned())
                .unwrap_or(Value::Null),
            "enabled": account.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        }));
    }
    references
}

/// 池列表的响应体：`{items, clash}`。
///
/// 每个条目注入 `usedBy`（引用它的账号）—— 删除确认框要用它提示
/// 「有 N 个账号正在使用」；`clash` 是实时快照，前端据此显示「Clash 是否可用 /
/// 有几个出口」。所有写操作也返回这份（与模型管理页的写接口同约定：前端就地替换）。
///
/// 订阅条目另外注入 `orphan`（所属订阅已不存在——数据被外部工具清掉、
/// 或旧版进程误删存储键的场景）：孤儿条目在界面上允许**手动删除**清理，
/// 非孤儿仍是只读镜像（见 `core::proxy_pool::remove_subscription_orphan`）。
fn pool_payload(state: &ServerState) -> Value {
    let references = pool_references(state);
    let live_subscriptions = crate::server::core::subscriptions::ids();
    let items: Vec<Value> = proxy_pool::list()
        .into_iter()
        .map(|mut item| {
            let used = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| references.get(id))
                .cloned()
                .unwrap_or_default();
            let orphan = item.get("source").and_then(Value::as_str) == Some("subscription")
                && item
                    .get("providerId")
                    .and_then(Value::as_str)
                    .map(|provider_id| !live_subscriptions.contains(provider_id))
                    .unwrap_or(true);
            if let Some(object) = item.as_object_mut() {
                object.insert("usedBy".to_string(), Value::Array(used));
                object.insert("orphan".to_string(), Value::Bool(orphan));
            }
            item
        })
        .collect();
    json!({
        "items": items,
        "clash": crate::server::core::proxies::clash_proxy_options(),
    })
}

/// 先同步 Clash 再取列表；`with_report = true` 时把本次同步的变更数 / 失败原因
/// 一并带上（「同步 Clash Verge」按钮要报「更新了 N 项」或「Clash 不可用」，
/// 而自动同步那次不必刷提示 —— 用户没点任何东西）。
///
/// 为什么列接口也同步：OmniProxy 的 `router.get('/proxies')` 就是这么做的 ——
/// 「在 Clash 里加了出口，打开页面就能看到」，不必先点一次同步。同步是幂等的，
/// 没有变更时一个字段都不写（见 `proxy_pool::sync_clash`）。
fn synced_payload(state: &ServerState, with_report: bool) -> Value {
    let report = proxy_pool::sync_clash();
    let mut data = pool_payload(state);
    if with_report {
        if let Some(object) = data.as_object_mut() {
            object.insert("changes".to_string(), Value::from(report.changes));
            object.insert(
                "syncError".to_string(),
                report.error.map(Value::String).unwrap_or(Value::Null),
            );
        }
    }
    data
}

/// GET /api/proxies/pool（顺手同步一次 Clash 出口，见 `synced_payload`）
pub async fn pool_list(state: &ServerState) -> Response {
    ok_json(synced_payload(state, false))
}

/// POST /api/proxies/pool/sync-clash（手动强制同步；响应带本次变更数与失败原因）
pub async fn pool_sync_clash(state: &ServerState) -> Response {
    ok_json(synced_payload(state, true))
}

/// POST /api/proxies/pool（新建）
pub async fn pool_create(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    match proxy_pool::create(&payload) {
        Ok(_) => ok_json(pool_payload(state)),
        Err(message) => management_error(400, message),
    }
}

/// POST /api/proxies/pool/update（编辑；body 里带 id）
pub async fn pool_update(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    match proxy_pool::update(&payload) {
        Ok(_) => ok_json(pool_payload(state)),
        // 条目不存在 / 是 Clash 同步来的（只读）/ 输入非法，都从 core 拿一句话：
        // 统一 400，文案本身就是可读的说明（界面照原样显示）
        Err(message) => management_error(400, message),
    }
}

/// POST /api/proxies/pool/remove（删除；body 里带 id）
///
/// 只读镜像（Clash 同步 / 订阅节点）默认拒绝，**孤儿订阅节点例外**：
/// 归属订阅已不存在（providerId 不在订阅清单里）的订阅条目允许手动删除
/// —— 它们不会再被任何同步碰，删除是唯一出路。orphan 判定在这里做
/// （core 层不认识订阅存储，见 `core::proxy_pool::remove_subscription_orphan`）。
pub async fn pool_remove(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return management_error(400, "缺少代理 id");
    }
    // 状态码要分开：条目**不存在** → 404（客户端多半拿着过期的 id）；
    // 存在但从 Clash 同步来（只读）→ 400（请求合法，是对象不允许删）。
    // core 的 Err 只有一句文案，所以存在性在这里先探一次（列表量级很小）。
    let items = proxy_pool::list();
    let target = items
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id));
    let exists = target.is_some();
    let orphan = target
        .filter(|item| item.get("source").and_then(Value::as_str) == Some("subscription"))
        .and_then(|item| item.get("providerId").and_then(Value::as_str))
        .map(|provider_id| !crate::server::core::subscriptions::ids().contains(provider_id))
        .unwrap_or(false);
    let result = if orphan {
        proxy_pool::remove_subscription_orphan(id)
    } else {
        proxy_pool::remove(id)
    };
    match result {
        Ok((_, name)) => {
            let mut data = pool_payload(state);
            if let Some(object) = data.as_object_mut() {
                object.insert("removed".to_string(), Value::String(name));
            }
            ok_json(data)
        }
        Err(message) => management_error(if exists { 400 } else { 404 }, message),
    }
}

/// POST /api/proxies/pool/test（测一条池条目，结果记进条目）
///
/// **禁用的条目也测**（`resolve_for_test` 不看禁用位）：「先测通再启用」
/// 是常见顺序。测试失败也返回 200（`success:false` + error 文案），
/// 与 `/api/proxies/test` 同一约定 —— 前端据 success 分支显示红/绿。
///
/// ── 订阅节点还没有监听器时先补建 ────────────────────────────
/// 内核只给「账号引用中的节点 ∪ 测试钉住的节点」开监听器（lazy listeners）。
/// 池里的「测试」按钮打的正是「先测一个节点、测通了再决定给谁用」这个主意，
/// 而这类节点在引用之前是没有监听器的：必须先把它钉住、请求内核重建、等端口
/// 写回条目，才谈得上测。**顺序不能反** —— 老实现是先 `resolve_for_test`
/// 再补建，未分配端口时解析失败就直接 400 返回了，补建永远执行不到，而那句
/// 错误文案还建议用户「点一次测试」，成了死循环（实测踩过）。
pub async fn pool_test(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    let prep = match proxy_pool::prepare_test(id) {
        Ok(prep) => prep,
        Err(message) => return management_error(400, message),
    };
    // 补建期间的结果：不是测试结论，只说明「内核重建中」。名字带上（此时没有
    // 已解析的出口可描述），前端照常显示 ❌ + 这句话，重测就是补建后的结果
    let mut pending: Option<String> = None;
    // 内核没装（没下过 / 刚被删）：补建无从谈起 —— 这条是**结论**，重测不会变
    let mut uninstalled: Option<String> = None;
    if prep.needs_provision {
        if sub_kernel::installed() {
            provision_missing_listeners(state);
            if !wait_for_listeners(&[id.to_string()]).await {
                pending = Some("监听器正在补建（内核重建中），稍等几秒后重测一次".to_string());
            }
        } else {
            // 不装内核时走补建 + 等待只会白等 45 秒（restart 早退在「未安装」上），
            // 末了还给一句「稍后重测」—— 而重测永远不会成功，是条死路。直接说清楚
            // 该做什么（去「订阅管理」下载内核），结论照常落进条目。
            uninstalled =
                Some("代理内核未安装（在「订阅管理」里下载内核后，节点会自动分配监听端口）"
                    .to_string());
        }
    }
    let (result, public) = match (uninstalled, pending) {
        (Some(error), _) | (None, Some(error)) => (
            egress::ConnectivityResult {
                success: false,
                status: None,
                ip: String::new(),
                duration_ms: 0,
                error: Some(error),
            },
            // 补建中：退回一个只表明「这条订阅节点、本地监听器、端口待定」的形态
            // （egress 的公开描述要 ResolvedProxy，这里能拼全 —— 订阅节点的地址
            // 本来就是内核起的本地监听器；label 取条目展示名）
            egress::describe_public(Some(&ResolvedProxy {
                source: "pool".to_string(),
                protocol: "http".to_string(),
                host: "127.0.0.1".to_string(),
                port: None,
                username: String::new(),
                password: String::new(),
                label: prep.name.clone(),
            })),
        ),
        // 不需要补建（或已就绪）：照常解析 + 测
        (None, None) => match proxy_pool::resolve_for_test(id) {
            Ok(proxy) => (
                egress::test_proxy(Some(&proxy)).await,
                egress::describe_public(Some(&proxy)),
            ),
            Err(message) => return management_error(400, message),
        },
    };
    let write_error = proxy_pool::record_test(
        id,
        &proxy_pool::TestOutcome {
            success: result.success,
            ip: result.ip.clone(),
            duration_ms: result.duration_ms,
            error: result.error.clone(),
        },
    )
    .err();
    logging::log(
        "[Proxies]",
        &format!(
            "代理测试（{}）: {}",
            prep.name,
            if result.success {
                format!("✅ 出口 IP {}", result.ip)
            } else {
                format!("❌ {}", result.error.clone().unwrap_or_default())
            }
        ),
    );
    let mut data = result.to_json();
    if let Some(object) = data.as_object_mut() {
        object.insert("proxy".to_string(), public);
        // 结果落库失败不该把「测试已经跑完」这个事实吞掉：测试结论照常返回，
        // 界面额外提示一句「没能记住这次结果」即可
        if let Some(error) = write_error {
            object.insert("saveError".to_string(), Value::String(error));
        }
        object.insert(
            "items".to_string(),
            pool_payload(state).get("items").cloned().unwrap_or_else(|| json!([])),
        );
    }
    ok_json(data)
}

/// 等这些条目的监听端口写回（补建完成）；**全部**就绪才返回 true。
///
/// 上限 45 秒：内核重启本身是秒级，但重启流程里会等 provider 拉取
/// （`PROVIDER_WAIT_MS` = 25 秒），端口是在那之后的节点同步里写回条目的 ——
/// 上限必须覆盖它，否则「首次补建 + 订阅拉取慢」这两个正常情况都会被判成超时。
/// 500ms 一轮、就绪立即返回：常见路径（订阅缓存已在本地）大约一两秒。
///
/// 超时**不报错**：调用方给一句「正在补建，稍后重测」，比在这里猜原因好；
/// 真正的失败原因（内核没装、订阅拉不到）会在内核状态与运行日志里。
async fn wait_for_listeners(ids: &[String]) -> bool {
    for _ in 0..90 {
        if ids.iter().all(|id| proxy_pool::current_port(id).is_some()) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}

// ─── 代理设置（「网络代理」页的设置弹窗）─────────────────────

/// GET /api/proxies/settings —— 代理设置的公开形态（含默认值，弹窗的
/// placeholder 与「恢复默认」读它）。
pub async fn settings_get() -> Response {
    ok_json(proxy_settings::get())
}

/// POST /api/proxies/settings —— 保存代理设置。
///
/// 校验在 `core::proxy_settings::save`（Err 是一句可读的中文 → 400）。
/// **端口范围改完不会自动重启内核**：已注册的端口不回收（沿用注册表），
/// 新分配才受新范围约束，前端在保存成功后提示用户手动重启内核使新范围
/// 完整生效 —— 内核重启要重建全部监听器，静默做会让界面节点端口闪变。
pub async fn settings_save(body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    match proxy_settings::save(&payload) {
        Ok(value) => ok_json(value),
        Err(message) => management_error(400, message),
    }
}

// ─── 节点监听器补建（lazy listeners 的主动入口）──────────────

/// 给「需要监听器但还没有端口」的订阅节点补建监听器（lazy listeners 的主动入口）。
///
/// 「需要」= **账号引用 ∪ 测试钉住**（`proxy_pool::listener_node_keys`），与内核
/// 侧 `allocate_listeners` 用同一个口径 —— 口径不一致会出现「这里说没缺口、
/// 内核却不建」这类谁也解释不清的状态。lazy 的语义是「内核只为需要的节点开
/// 监听器」；引用动作发生在账号保存时，测试动作发生在池的「测试」按钮上，
/// 后者先 `prepare_test` 把条目钉住再来这里。本函数是一个**幂等的重启请求**，
/// 只在确实有缺口（需要 ∧ 无端口）时才投命令，重复调用无害。
///
/// 三类调用方：
///   · 账号保存链路（`api::accounts` 的写操作收尾）—— 引用变化后自动补；
///   · `pool_test`（测试一个没有端口的订阅节点前先补建，否则连不上）;
///   · HTTP：POST /api/proxies/pool/provision（前端手动兜底）。
///
/// 返回是否投了重启命令：true 时节点端口要等内核重建完成才有值，前端轮询
/// 列表即可。同步版（投命令是 mpsc send，无需 await）—— 账号写操作收尾在
/// 持锁路径的边缘调用它，不想为它包一层 spawn。
pub fn provision_missing_listeners(state: &ServerState) -> bool {
    let pool_ids = state.store().referenced_pool_ids();
    let borrowed: std::collections::HashSet<&str> =
        pool_ids.iter().map(String::as_str).collect();
    let wanted = proxy_pool::listener_node_keys(&borrowed);
    let missing = if wanted.is_empty() {
        Vec::new()
    } else {
        proxy_pool::subscription_entries_with_ports()
            .into_iter()
            .filter(|(provider_id, node, port)| {
                port.is_none() && wanted.contains(&format!("{provider_id}::{node}"))
            })
            .collect::<Vec<_>>()
    };
    if missing.is_empty() {
        return false;
    }
    // 有缺口才重启内核（重建配置把缺的监听器建出来）。内核未安装时投命令
    // 也无害（restart 早退在「未安装」上，见 `sub_kernel` worker 的说明）。
    sub_kernel::request_restart(crate::server::core::subscriptions::enabled_provider_inputs());
    logging::log(
        "[Proxies]",
        &format!(
            "🔄 检测到 {} 个被引用的订阅节点还没有监听器，已请求内核补建",
            missing.len()
        ),
    );
    true
}

/// POST /api/proxies/pool/provision —— 补建的主动入口（响应带 `pending`：
/// true = 投了重启命令；`ready` = 指定的那批条目端口是否都就绪）。
///
/// body 可选 `{ids: [...]}`：把这几条池条目**一并钉住**再补建，并等它们的
/// 端口落地才返回。批量测试用这个形态 —— 「哪个节点要测」只有前端知道，
/// 后端不猜；一次重启把所有要测的节点都建出来，比每条各等一轮内核重建快得多
/// （后者会把「批量测 30 条」变成 30 次重启）。不传 ids 时保持原来的语义：
/// 只补「已经需要监听器（账号引用 ∪ 已钉住）却还没有端口」的节点。
pub async fn pool_provision(state: &ServerState, body: &Bytes) -> Response {
    let ids: Vec<String> = parse_body(body)
        .ok()
        .and_then(|value| value.get("ids").and_then(Value::as_array).cloned())
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // 逐个钉住（不存在的 id / 已经不需要补建的条目直接忽略：批量里混进
    // 已就绪、甚至已被删的条目都是正常时序，不该让整批失败）
    let pinned: Vec<String> = ids
        .iter()
        .filter(|id| matches!(proxy_pool::prepare_test(id), Ok(prep) if prep.needs_provision))
        .cloned()
        .collect();
    let pending = provision_missing_listeners(state);
    // 钉住的那批等一下端口（没钉住任何条目 = 不用等，直接回报就绪）
    let ready = pinned.is_empty() || wait_for_listeners(&pinned).await;
    ok_json(json!({ "pending": pending, "missing": pending as u8, "ready": ready }))
}
