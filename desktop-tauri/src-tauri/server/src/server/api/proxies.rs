//! 出网代理路由（对照 workbuddy-account-routes.mjs 的 `tryHandleProxies`）。
//!
//!   GET  /api/proxies            可选项（Clash Verge 实时快照）+ 各账号当前出口
//!   POST /api/proxies/test       测试出口连通性并返回出口 IP
//!   GET  /api/proxies/pool       代理池列表（进来时自动同步一次 Clash 出口）
//!   POST /api/proxies/pool       新建池条目（只收手动条目）
//!   POST /api/proxies/pool/update 编辑池条目（body 带 id；Clash 同步来的不可改）
//!   POST /api/proxies/pool/enable 启用 / 禁用池条目（body 带 id + enabled；
//!                                 三种来源都放行：manual 写自己的 enabled，
//!                                 镜像条目写本地禁用位；订阅节点禁用时走
//!                                 「关闭监听端口」流程、启用时走补建）
//!   POST /api/proxies/pool/remove 删除池条目（body 带 id；Clash 同步来的不可删）
//!   POST /api/proxies/pool/test  测试池条目并把结果记进条目（订阅节点走内核的
//!                                统一测试口：切 select 组到目标节点再测，
//!                                不为被测节点开专用端口）
//!   POST /api/proxies/pool/sync-clash 手动同步 Clash Verge 出口
//!   GET  /api/proxies/settings   代理设置（节点端口范围 + 测试网址 + 默认值）
//!   POST /api/proxies/settings   保存代理设置（整份表单；非法形状 400）
//!
//! 前两条是账号侧的出口能力（Node 版原有）；`pool` 那几条是「网络代理」页
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
///
/// **轮询档（rotate）也算引用**：它的候选来自池里同一批条目（全部启用 / 指定
/// 几条）。不在删除确认框里列出来，用户会以为「这条没人用」而删掉一条正在被
/// 轮询的出口。两条分支回答的是同一个问题 ——「这条池条目会不会被某条账号的
/// 转发用到」—— 所以放在同一个函数里。
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
        let ids: Vec<String> = if crate::server::core::proxies::is_rotate_config(config) {
            crate::server::core::proxy_rotate::candidate_ids(config)
        } else if config.get("source").and_then(Value::as_str) == Some("pool") {
            config
                .get("proxyId")
                .and_then(Value::as_str)
                .map(|id| vec![id.to_string()])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let entry = json!({
            "id": account.get("id").cloned().unwrap_or(Value::Null),
            // 统一展示名优先（与列表接口同一取舍，见上面的说明）
            "name": account
                .get("displayName")
                .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                .cloned()
                .or_else(|| account.get("name").cloned())
                .unwrap_or(Value::Null),
            "enabled": account.get("enabled").cloned().unwrap_or(Value::Bool(true)),
        });
        for proxy_id in ids {
            references.entry(proxy_id).or_default().push(entry.clone());
        }
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

/// POST /api/proxies/pool/enable（启用 / 禁用一条池条目；body `{id, enabled}`）
///
/// 三种来源都能开关，但写的东西不一样：
///   · manual：写它自己的 `enabled`（禁用后引用它的账号回退直连）；
///   · clash / subscription（只读镜像）：写**本地禁用位** `localDisabled` ——
///     名称 / 端口 / 来源侧的启用状态继续跟随来源，本地那位只让本端不用它
///     （为什么不直接改写 `enabled`，见 `core::proxy_pool` 的模块头）。
///     所以本接口不吃 `pool/update` 的整份覆盖语义：它只碰开关这一个字段，
///     镜像条目的名称 / 端口一个字节都不动。
///
/// ── 订阅节点的监听器联动（关闭流程 / 补建流程）──────────────
/// 订阅节点的地址是本机内核起的监听端口，`enabled` 若只写一个标记，端口会继续
/// 开着 —— 而用户禁用一条节点往往正是为了**不要那个端口**。所以两个方向都要
/// 让内核跟上，判据都是「真实的缺口」而不是「用户点了什么」：
///   · 禁用 → 缺口是「端口还开着但已不该开」（禁用 / 失去引用），请求内核
///     重建配置（新配置里没有这条监听器，进程重启即端口关闭），端口由重启后
///     的节点同步写回 null；
///   · 启用 → 缺口是「启用 ∧ 被账号引用 ∧ 没有端口」，走补建链路。
/// 两个方向都走同一个对账入口（`reconcile_listeners`，与账号保存收尾同一条
/// 路），没缺口就不投重启命令 —— 重复点击 / 反复开关都不会排出一串多余的重建。
///
/// 响应里的 `listener` 说明这次要不要盯一会儿端口（manual / clash 恒 `none`）：
///   `none`    没有本机端口要动；
///   `closing` 已请求关闭（前端轮询列表，等端口变成「未分配」）；
///   `opening` 已请求补建（前端轮询列表，等端口落地）。
/// **不在请求里等**：内核重建要几秒到几十秒，等在这里会让开关十几秒不动，
/// 用户只会以为没点上（等与轮询的分工与订阅增删那条链一致，见页面的
/// `settleListener`）。
pub async fn pool_enable(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return management_error(400, "缺少代理 id");
    }
    // 缺 enabled 就报错而不是猜：这条接口只有一个语义，猜错方向等于把用户
    // 按下的开关反过来执行
    let Some(enabled) = payload.get("enabled").and_then(Value::as_bool) else {
        return management_error(400, "缺少 enabled（true / false）");
    };
    let (_, change) = match proxy_pool::set_enabled(id, enabled) {
        Ok(value) => value,
        Err(message) => return management_error(400, message),
    };
    let listener = listener_flow(state, id, &change);
    let mut data = pool_payload(state);
    if let Some(object) = data.as_object_mut() {
        object.insert("name".to_string(), Value::String(change.name));
        object.insert("enabled".to_string(), Value::Bool(change.enabled));
        object.insert("listener".to_string(), Value::String(listener.to_string()));
    }
    ok_json(data)
}

/// 开关之后的内核联动，返回响应里的 `listener`（`none` / `closing` / `opening`）。
///
/// 只对订阅节点有事可做：manual 没有本机端口，clash 的端口在 Clash Verge 手里
/// （那是别人家的进程，本服务能做的只是「本端别用它」）。
fn listener_flow(state: &ServerState, id: &str, change: &proxy_pool::EnabledChange) -> &'static str {
    if change.source != "subscription" {
        return "none";
    }
    if change.enabled {
        // 内核没装：补建无从谈起（地址列会提示「去订阅管理下载内核」），
        // 这里报 opening 只会让前端白轮询十几秒
        if !sub_kernel::installed() {
            return "none";
        }
        let pool_ids = state.store().referenced_pool_ids();
        let borrowed: std::collections::HashSet<&str> =
            pool_ids.iter().map(String::as_str).collect();
        if !proxy_pool::needs_listener(id, &borrowed) {
            return "none";
        }
        // 对账（补建方向）。返回 false 只有两种情况：这条其实没缺口（上面
        // 已挡）/ 手动关闭闩置位（对账记了日志，「启动内核」时按 wanted
        // 一并补上）—— 都不该报 opening 让前端白轮询
        if reconcile_listeners(state) {
            "opening"
        } else {
            "none"
        }
    } else {
        // 禁用：对账（回收方向）—— 缺口判据是 reconcile 的 stale 检测
        // （不在 wanted 里却还挂着端口：刚禁用的这条、先前遗留的、
        // 失去引用的都算），内核没装则无端口可收
        if !sub_kernel::installed() {
            return "none";
        }
        if reconcile_listeners(state) {
            "closing"
        } else {
            "none"
        }
    }
}

/// POST /api/proxies/pool/test（测一条池条目，结果记进条目）
///
/// **禁用的条目也测**：解析不看禁用位（`resolve_for_test`），订阅节点走统一
/// 测试口更与禁用无关 —— 「先关着、偶尔测一下看机场修好没有」是合理用法。
/// 测试失败也返回 200（`success:false` + error 文案），与 `/api/proxies/test`
/// 同一约定 —— 前端据 success 分支显示红/绿。
///
/// ── 订阅节点走统一测试口（不为它开端口）─────────────────────
/// 内核里有一个固定的「测试口」监听器，背后是 select 策略组（成员 = 全部
/// 节点）：先通过控制器把组切到目标节点，再从测试口发测试（见
/// `sub_kernel::runtime::select_test_node`）。这是 Clash Verge 的形态 ——
/// 「测试」不再需要为被测节点开专用端口 + 等内核重建（曾经的 pinned +
/// provision + 45 秒等待那一整套就是为这件事存在的，统一测试口上线后退役）。
/// 专用端口只服务真正在用的节点（账号引用，lazy listeners）。
///
/// 订阅分支全程持 [`SUB_TEST_LOCK`]：策略组的「当前指向」是内核的**全局状态**，
/// 两条测试并发时后切的会把先切的覆盖掉，先测的那条就走了别人的出口 ——
/// 前端本来同时只放行一条测试（其余按钮禁用），这把锁兜的是多入口（桌面 +
/// headless 面板）与将来并发化的可能性。
static SUB_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn pool_test(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    let target = match proxy_pool::test_target(id) {
        Ok(target) => target,
        Err(message) => return management_error(400, message),
    };
    // 出口解析 + 测试按来源分两条路：
    //   · subscription → 统一测试口（切组到目标节点，从测试口发测试）；
    //   · manual / clash → 原有解析（manual 用它自己的地址；clash 实时读
    //     Clash 配置），与内核无关。
    // Err 都是一句可读的中文 → 400（条目不存在 / 内核没装 / 节点已不在订阅里）。
    // 订阅分支**全程持锁**：从切组到拿到测试结果，组不能被另一条测试切走
    // （锁只在订阅分支内拿，manual / clash 不碰内核、不排队）。
    let (result, public) = if target.source == "subscription" {
        let _guard = SUB_TEST_LOCK.lock().await;
        let proxy = match sub_kernel::runtime::select_test_node(&target.provider_id, &target.node)
            .await
        {
            Ok(port) => ResolvedProxy {
                source: "pool".to_string(),
                protocol: "http".to_string(),
                host: "127.0.0.1".to_string(),
                port: Some(port),
                username: String::new(),
                password: String::new(),
                label: target.name.clone(),
            },
            Err(message) => return management_error(400, message),
        };
        let result = egress::test_proxy(Some(&proxy)).await;
        (result, egress::describe_public(Some(&proxy)))
    } else {
        let proxy = match proxy_pool::resolve_for_test(id) {
            Ok(proxy) => proxy,
            Err(message) => return management_error(400, message),
        };
        let result = egress::test_proxy(Some(&proxy)).await;
        (result, egress::describe_public(Some(&proxy)))
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
            target.name,
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

// ─── 节点监听器补建（lazy listeners 的触发点）───────────────

/// 监听器对账：把「该开的监听器」与「池里条目的端口状态」收敛一致
/// （lazy listeners 的**双向**触发点）。
///
/// 该开的集合 `wanted` = **账号引用 ∩ 启用中的订阅节点**
/// （`proxy_pool::listener_node_keys`，与内核侧 `allocate_listeners` 同口径
/// —— 口径不一致会出现「这里说没缺口、内核却不建」这类谁也解释不清的状态）。
/// 两个方向的缺口各检测各的：
///   · **待补建**：在 wanted 里、条目端口还是 null（新增引用 / 刚启用）；
///   · **待回收**：不在 wanted 里、条目端口还挂着值（失去引用 / 被禁用 /
///     订阅侧关掉）—— 内核重建时不再给它开监听器，端口随重建一起关。
/// 任一方向非空就投一次重启（重建按 wanted 全量重算，一次同时干两件事）；
/// 没缺口不投，重复调用无害 —— 这是调用方敢在每次账号写操作后都调它的前提。
/// 测试**不在其中** —— 测试走统一测试口，不需要端口（见 `pool_test` 与
/// `sub_kernel::runtime::select_test_node`）。
///
/// 调用方：
///   · 账号写操作收尾（`api::accounts` 的 add / update / 批量 / 删除）——
///     引用变化后补缺与回收；
///   · `pool_enable`（启用 / 禁用一条订阅节点后的联动）。
///
/// 手动关闭闩置位时只记日志不投命令：用户刚按过「关闭内核」，保存引用不能
/// 把它悄悄带起来 —— 「启动内核」重建时同样按 wanted 全量分配，缺口自然收敛。
///
/// 返回是否投了重启命令：true 时节点端口要等内核重建完成才有终值，前端轮询
/// 列表即可。同步版（投命令是 mpsc send，无需 await）—— 账号写操作收尾在
/// 持锁路径的边缘调用它，不想为它包一层 spawn。
pub fn reconcile_listeners(state: &ServerState) -> bool {
    let pool_ids = state.store().referenced_pool_ids();
    let borrowed: std::collections::HashSet<&str> =
        pool_ids.iter().map(String::as_str).collect();
    let wanted = proxy_pool::listener_node_keys(&borrowed);
    let mut missing = 0usize;
    let mut stale = 0usize;
    for (provider_id, node, port) in proxy_pool::subscription_entries_with_ports() {
        // key 形态与 `listener_node_keys` 的构造处一致（`{providerId}::{node}`）
        match (wanted.contains(&format!("{provider_id}::{node}")), port) {
            (true, None) => missing += 1,
            (false, Some(_)) => stale += 1,
            _ => {}
        }
    }
    if missing == 0 && stale == 0 {
        return false;
    }
    // 手动关闭闩：用户刚点过「关闭内核」，这里不悄悄把它带起来 —— 缺口
    // 记在这条日志里，「启动内核」重建配置时按最新引用一并收敛
    if sub_kernel::is_manually_halted() {
        logging::log(
            "[Proxies]",
            &format!(
                "内核已手动关闭 —— 监听器对账暂缓（{missing} 个待补建 / {stale} 个待回收），「启动内核」时按最新引用重算"
            ),
        );
        return false;
    }
    // 有缺口才重启内核（重建配置把缺的建出来、多的收掉）。内核未安装时投
    // 命令也无害（restart 早退在「未安装」上，见 `sub_kernel` worker 的说明）。
    sub_kernel::request_restart(crate::server::core::subscriptions::enabled_provider_inputs());
    logging::log(
        "[Proxies]",
        &format!("🔄 监听器对账：{missing} 个待补建 / {stale} 个待回收，已请求内核重建"),
    );
    true
}
