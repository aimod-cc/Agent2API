//! 出网代理池：集中管理一组**命名代理**（手动填的 / 引用 Clash Verge 的 /
//! 订阅节点镜像），账号的 proxy 字段可以按 id 引用它们（`{source:'pool', proxyId}`）。
//!
//! ── 为什么要有它（对照 OmniProxy 的 proxies 表）──────────────
//! 改造前代理只能**逐个账号**配：同一个出口用在三个账号上就要填三遍，
//! 改一次端口要改三处；也看不到「这个出口到底通不通、出口 IP 是多少」——
//! 出口测试只有账号弹窗里那一处入口。代理池把「出口」抽成独立实体：
//! 配一次、测一次、多个账号引用，账号记录里只留一个 id。
//!
//! ── 存储：kv 的 `proxyPool` 键（不是独立表）────────────────────
//! 条目数量级是几个到几十个、无查询需求（永远是整份读、整份写），
//! 与 `scheduledTasks` / `desktopSettings` 同一类零散状态 —— 按项目约定
//! 落 `kv` 的固定键（登记在 `db::schema::RESERVED_KV_KEYS`，否则
//! `config::save_raw` 写一次配置就会把它删掉）。不新增表也就**不动 schema
//! 版本**：`db::schema` 的模块头把「表只增不改」的成本讲得很清楚，这里没有
//! 需要索引/约束的字段，不值得为它推一个版本。
//!
//! ── 三种来源（对标 OmniProxy 的 proxies 表）────────────────────
//!   manual  手填 `{protocol, host, port, username, password}`；
//!   clash   **从 Clash Verge 同步进来**的镜像记录 `{listenerUid}`：名称 / 端口 /
//!           启用状态全部跟随 Clash（端口实时读取，见 `core::clash` 的「严格镜像」
//!           说明），因此名称与端口**本地不可改不可删** —— 想改去 Clash Verge 改，
//!           想删去 Clash Verge 删（下一次同步会把它从池里摘掉）。这条只读语义照
//!           OmniProxy 的 `imported_clash_listeners` 分支实现（它的 PUT/DELETE
//!           对同步来的记录一律 400「请在 Clash Verge 中修改」）；
//!   subscription  **订阅节点镜像**（内核拉到订阅后同步进来，见 `core::sub_kernel`）：
//!           条目存 `{providerId, node, port}`，地址是内核起的本地监听器
//!           （http://127.0.0.1:端口），端口注册表持久在内核目录里、重启不漂移。
//!           与 clash 同为**只读镜像** —— 名称 / 端口跟随订阅与内核，本地不可改
//!           不可删（要改去「订阅管理」操作，下一次同步会镜像回来）。
//!
//! ── `localDisabled`：镜像条目的本地开关（启用列不再只读）────────
//! 名称与端口必须跟随来源侧，但**「这条本端用不用」是用户自己的事**：某条节点
//! 常年不通、或只是暂时不想用它，用户要能在「网络代理」页把它关掉 —— 不必去
//! Clash Verge 关监听器、也不必改订阅里的节点。所以镜像条目多一个可选字段：
//!   · `localDisabled: true` = 本页禁用。它**只在本端生效**：有效启用为
//!     「来源侧启用 ∧ 非本地禁用」（`is_enabled`），引用解析、监听器分配、
//!     展示都按它算；
//!   · 为什么不直接改写 `enabled`：那个字段是来源侧的镜像值，下一次同步就会
//!     被写回来（`sync_clash` 按 uid 更新名称与启用），开关会自己弹回去；
//!   · 禁用一条**订阅节点**会连带收掉它的本地监听端口（关闭流程，见
//!     `api::proxies::pool_enable` 与 `reconcile_listeners`）：内核不再为被
//!     禁用的节点开监听器 —— 禁用的语义就是「这个端口不再听着」，只留一个
//!     标记会让端口继续开着、地址看着能用而实际没人用它。
//!     manual / clash 没有本机端口（clash 的端口在 Clash Verge 手里），
//!     禁用只影响本端的取用。
//!
//! ── 端口跟随引用（双向对账，`api::proxies::reconcile_listeners`）────
//! 专用端口只为账号转发而存在（测试走统一测试口，见下节），于是「引用 ⇄
//! 端口」两个方向都自动收敛：新增引用补监听器，**失去引用（删账号 / 改走
//! 别的代理）回收端口**。账号每次写操作后对账一次，缺口非空就投一次内核
//! 重建（重建按最新引用全量重算，一次同时补缺与回收）。「无引用却还挂着
//! 端口」因此只是两次重建之间的瞬时态，不是需要用户手动清理的状态。
//!
//! ── 测试走统一测试口（不占节点端口）────────────────────────
//! 内核里有一个固定的「测试口」监听器，背后是 select 策略组（成员 = 全部
//! 节点）：「测试」= 把组切到目标节点 → 从测试口发请求（见
//! `sub_kernel::runtime::select_test_node`），**不为被测节点开专用端口**。
//! 于是「先测一个节点、测通了再决定给哪个账号用」这条最自然的用法零成本 ——
//! 曾经为此存在的 `pinned`（测试前把条目钉住、请内核补建、等端口写回）
//! 整套机制随之退役。专用端口只服务真正在用的节点（账号引用，lazy listeners）；
//! 未引用的节点条目端口为 null（「未分配端口」），这是正常状态而不是待补建的缺口。

//! ── 同步（`sync_clash`，对标 OmniProxy 的 syncClashVergeListeners）──
//! 把 Clash 当前的出口集合**整体镜像**进池：新出口插入、已有出口按 uid 更新
//! （名称 / 启用）、Clash 侧已消失的摘掉。手动条目不受影响。触发点两处：
//! `GET /api/proxies/pool` 进来时自动同步一次（OmniProxy 的 `router.get('/proxies')`
//! 也是这么做的 —— 「Clash 里加了出口，打开页面就能看到」不必再点一次按钮），
//! 以及页面上的「同步 Clash Verge」按钮（手动强制一次）。
//! **镜像是全量的**：用户在池里看到的就是 Clash 当前的出口集合，不存在
//! 「半套」状态；这也意味着在 Clash 侧删掉一个出口，引用它的账号会解析失败并
//! 回退直连（与「账号直接引用 Clash 监听器」时的行为完全一致）。
//!
//! 与 OmniProxy 的一处**有意差异**：同步集合包含 Clash 的**混合端口**
//! （`__mixed__`，即「按规则分流」那个入口）。OmniProxy 只导手动监听器，而
//! workbuddy 的账号代理下拉从一开始就把混合端口列为第一项（见 `clash.rs` 的
//! `clash_proxy_options`）—— 不导入它，「按规则分流」这条最常用的出口在池里
//! 就没有任何可引用的条目，等于功能倒退。
//!
//! ── 解析复用账号代理那一条链，不另写一份 ──────────────────────
//! 条目 → 构造出与账号 `proxy` 字段同形的 config → 交给
//! `core::proxies::resolve_account_proxy`。于是「Clash 监听器被删了」
//! 「端口非法」这些失败形态与账号侧逐字一致，前端显示与排障口径只有一套。
//! 反向的依赖（proxies.rs 解析 pool 引用时回来查池）见 `core::proxies` 的
//! `pool` 分支 —— 条目自身只可能是 clash / manual（归一里挡掉了嵌套引用），
//! 不存在递归。
//!
//! ── 测试结果为什么也进条目 ────────────────────────────────────
//! `lastTest`（成功与否 / 出口 IP / 耗时 / 时刻）在测试后落库：重启、切页、
//! 换设备打开都能看到「上次测出来是什么」。它只是**展示用的历史事实**，
//! 不参与选路 —— 转发不会因为上次测试失败就跳过某个出口（判断出口可用性
//! 的唯一权威是实际请求的结果，测试结果只回答「上次手测时通不通」）。

use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::server::core::proxies::{resolve_account_proxy, ProxyResolution};
use crate::server::db::Db;
use crate::server::logging;

/// `kv` 表里的键名（保留键，见 `db::schema::RESERVED_KV_KEYS`）
pub const KV_KEY: &str = "proxyPool";

/// 名称 / 主机的长度上限（与账号代理的 MAX_* 同量级；名称要出现在下拉与表格里）
const MAX_NAME_LENGTH: usize = 60;
const MAX_HOST_LENGTH: usize = 255;
const MAX_USER_LENGTH: usize = 200;
const MAX_LABEL_LENGTH: usize = 100;

/// 进程级库句柄（照 `core::task_state` 的形态：bootstrap 时 install 一次）
static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 接线（`ServerState::bootstrap` 调用）。重复调用无副作用。
pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Result<&'static Db, String> {
    DB.get()
        .and_then(Option::as_ref)
        .ok_or_else(|| "代理池数据库不可用".to_string())
}

// ─── 存取 ───────────────────────────────────────────────────

/// 读全部条目（原始形态，`items` 数组；库不可用/无记录/解析失败都给空表）。
///
/// 解析失败按空表处理而不是报错：库里的这行只可能被外部工具改坏，
/// 用户界面上的表现是「代理池空了」——比整个页面报错更可恢复（重新加一条即可）。
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

/// 全部条目的**原始形态**（`core::proxy_rotate` 的读入口）。
///
/// 轮询档要按池的实时状态算候选（新增 / 删除 / 禁用都要立刻反映到下一次取用），
/// 而那份计算只需要条目本身；公开形态（`describe_item`）要解析出口、成本高一个
/// 量级，不适合逐请求调用。给原始形态而不是包一层轮询专用访问器，是为了让
/// 「哪些条目能轮询」这条判据只留在 `proxy_rotate` 一处。
pub(crate) fn raw_items() -> Vec<Value> {
    read_items()
}

/// 整份写回（增删改都走它；条目数量小，整份重写的开销可忽略）
fn write_items(items: &[Value]) -> Result<(), String> {
    let db = database()?;
    let text = serde_json::to_string(&json!({ "items": items }))
        .map_err(|error| format!("编码代理池失败: {error}"))?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KV_KEY, text],
        )
        .map(|_| ())
        .map_err(|error| format!("保存代理池失败: {error}"))
    })
    .ok_or_else(|| "代理池数据库不可用".to_string())?
}

// ─── 正规化与校验 ───────────────────────────────────────────

fn clean_string(value: Option<&Value>, max: usize) -> String {
    let Some(Value::String(text)) = value else {
        return String::new();
    };
    text.trim().chars().take(max).collect()
}

/// 1..65535 的整数（数字或数字字符串，口径与 `core::proxies::valid_port` 一致）
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

/// 把 API 输入（新建 / 编辑的整份表单）归一成一条**手动**池条目。
///
/// `existing` 是编辑时的旧条目（新建传 None）：`id` / `createdAt` / `lastTest`
/// 从旧条目沿用，其余字段一律按本次输入覆盖（与账号「页面表单整份提交」同语义，
/// 前端编辑时会回显密码，不存在「空 = 保留」的隐式行为）。
///
/// **只产出 manual 条目**：clash 来源的条目由同步产生（见 `sync_clash`），
/// subscription 来源的条目由内核的节点同步产生（见 `sync_subscription_nodes`），
/// 从 API 进来的这两种形状一律拒绝 —— 否则会凭空造出一条「不属于任何同步
/// 集合的镜像记录」，而它下次同步就会被摘掉，等于用户白填一次。
fn normalize_item(input: &Value, existing: Option<&Value>) -> Result<Value, String> {
    let Some(object) = input.as_object() else {
        return Err("代理条目必须是对象".to_string());
    };
    let source_input = clean_string(object.get("source"), 20);
    if source_input == "clash" {
        return Err("Clash Verge 出口由「同步 Clash Verge」自动导入，不能手工新建".to_string());
    }
    if source_input == "subscription" {
        return Err("订阅节点由「订阅」功能自动导入，不能手工新建".to_string());
    }
    let name = clean_string(object.get("name"), MAX_NAME_LENGTH);
    if name.is_empty() {
        return Err("请填写代理名称".to_string());
    }

    let now = logging::now_ms();
    let id = existing
        .map(|item| text_of(item, "id"))
        .unwrap_or_default();
    let id = if id.is_empty() { new_id() } else { id };
    let created_at = existing
        .and_then(|item| item.get("createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(now);
    // 测试结果是「历史事实」：编辑配置后旧结果已经不对应新出口了，但**不删** ——
    // 前端在结果旁标注它测于何时，用户自己能判断要不要重测（悄悄删掉会让
    // 「上次测过、当时是通的」这条信息无端消失）。改地址时保留、改协议时保留，
    // 一律保留，唯一的重置是删除条目本身。
    let last_test = existing
        .and_then(|item| item.get("lastTest"))
        .cloned()
        .unwrap_or(Value::Null);

    let protocol = {
        let cleaned = clean_string(object.get("protocol"), 10).to_lowercase();
        if cleaned.is_empty() { "http".to_string() } else { cleaned }
    };
    if protocol != "http" && protocol != "socks5" {
        return Err("代理协议只支持 http 或 socks5".to_string());
    }
    let host = clean_string(object.get("host"), MAX_HOST_LENGTH);
    if host.is_empty() {
        return Err("请填写代理主机地址".to_string());
    }
    let Some(port) = valid_port(object.get("port")) else {
        return Err("代理端口必须是 1-65535 的整数".to_string());
    };

    let mut normalized = Map::new();
    normalized.insert("id".to_string(), Value::String(id));
    normalized.insert("name".to_string(), Value::String(name));
    normalized.insert("source".to_string(), Value::String("manual".to_string()));
    normalized.insert(
        "enabled".to_string(),
        Value::Bool(!matches!(object.get("enabled"), Some(Value::Bool(false)))),
    );
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
    normalized.insert("createdAt".to_string(), Value::from(created_at));
    normalized.insert("updatedAt".to_string(), Value::from(now));
    normalized.insert("lastTest".to_string(), last_test);
    Ok(Value::Object(normalized))
}

/// 新条目的 id：`px_<毫秒>_<8 位随机>`。随机段与 `upstream::request` 的
/// 请求 id 同一手法（RandomState 的随机种子），不引第三方依赖。
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(logging::now_ms() as u64);
    hasher.write_u64(std::process::id() as u64);
    let suffix = (hasher.finish() & 0xffff_ffff) as u32;
    format!("px_{}_{suffix:08x}", logging::now_ms())
}

// ─── 条目状态判定（本地禁用位 / 有效启用）────────────────────

/// 本地禁用位（`localDisabled: true`；缺键 / false 一律按未禁用）。
///
/// 只对**镜像条目**（clash / subscription）有意义 —— manual 条目的 `enabled`
/// 就是用户自己的开关，不需要第二个字段（见模块头 localDisabled 一节）。
pub fn is_locally_disabled(item: &Value) -> bool {
    matches!(item.get("localDisabled"), Some(Value::Bool(true)))
}

/// 条目的**有效**启用状态：来源侧启用 ∧ 非本地禁用。
///
/// 「这条代理现在能不能用」全项目都按它算（引用解析、监听器分配、列表展示）——
/// 少一处就会冒出「界面显示已禁用、转发却还在用它」这类谁也解释不清的状态。
/// manual 条目没有本地禁用位，退化成 `enabled` 本身。
pub fn is_enabled(item: &Value) -> bool {
    !is_locally_disabled(item) && !matches!(item.get("enabled"), Some(Value::Bool(false)))
}

// ─── 解析（给 core::proxies 的 pool 分支与 describe 共用）──────

/// 把条目解析成可用的出口（复用账号代理那条链，见模块头）。
///
/// 返回 None 只发生在条目形状残缺到连 config 都拼不出时（手工改坏的库）；
/// 正常条目一定拿到 `Resolved` / `Failed` 二选一。
///
/// **subscription 分支不经过账号代理那条链**：节点地址是内核起的本地监听器
/// （http://127.0.0.1:端口），端口存在条目自身里（内核的端口注册表在同步时
/// 写进来的），直接构造 Resolved —— 不查内核状态（内核停了时条目照常显示，
/// 转发层的「连接被拒」错误自然会冒出来）；节点从订阅里消失（同步摘掉端口）
/// 才是 Failed。
pub fn resolve_item(item: &Value) -> Option<ProxyResolution> {
    let source = text_of(item, "source");
    let config = if source == "clash" {
        json!({ "source": "clash", "listenerUid": text_of(item, "listenerUid") })
    } else if source == "subscription" {
        let Some(port) = valid_port(item.get("port")) else {
            // 两种「没有端口」要分开说：节点还在但还没有账号引用它（lazy
            // listeners 只给引用中的节点开监听器；刚保存引用的话，内核可能
            // 还在重建），与节点已被订阅移除 —— 处置动作完全不同。
            //
            // 「没有端口」是未引用节点的**正常状态**而不是待补建的缺口：
            // 测试走统一测试口（见模块头），不再需要端口；地址只在「真在用」
            // 时存在。
            let node_alive = crate::server::core::subscriptions::ids()
                .contains(&text_of(item, "providerId"));
            return Some(ProxyResolution::Failed(if node_alive {
                "该节点还没有监听端口（还没有账号引用它 —— 在账号里选它并保存后，内核会自动开端口）".to_string()
            } else {
                "订阅节点已不存在（订阅更新后节点被移除，等下一次同步或重新订阅）".to_string()
            }));
        };
        return Some(ProxyResolution::Resolved(crate::server::core::proxies::ResolvedProxy {
            source: "pool".to_string(),
            protocol: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port: Some(port),
            username: String::new(),
            password: String::new(),
            label: String::new(),
        }));
    } else {
        json!({
            "source": "custom",
            "protocol": text_of(item, "protocol"),
            "host": text_of(item, "host"),
            "port": item.get("port").cloned().unwrap_or(Value::Null),
            "username": text_of(item, "username"),
            "password": text_of(item, "password"),
        })
    };
    resolve_account_proxy(Some(&config))
}

/// 池引用的解析（`core::proxies` 的 pool 分支调它）。
///
/// 三态（`Err` 的两种分开报：用户看到「已禁用」才会想到去代理页启用它，
/// 而「不存在」提示的是「被谁删了」）：
///   `Ok(proxy)` 出口可用；
///   `Err(原因)` 条目不存在 / 被禁用 / 解析失败 —— 调用方原样报出去
///               （账号列表的「代理异常」气泡、转发时回退直连的日志）。
pub fn resolve_reference(proxy_id: &str) -> Result<crate::server::core::proxies::ResolvedProxy, String> {
    let Some(item) = find_raw(proxy_id) else {
        return Err(format!("代理「{proxy_id}」不存在或已被删除"));
    };
    if !is_enabled(&item) {
        return Err(format!(
            "代理「{}」已禁用（在「网络代理」页启用它）",
            display_name(&item, proxy_id)
        ));
    }
    resolve_entry(&item, proxy_id)
}

/// 出口测试专用解析：与 `resolve_reference` 只差**不看禁用位** ——
/// 「先测通、再启用」是常见操作顺序，禁用的条目也要能测（只有引用解析
/// 那条链必须挡住禁用项，理由见 `resolve_reference`）。
pub fn resolve_for_test(proxy_id: &str) -> Result<crate::server::core::proxies::ResolvedProxy, String> {
    let Some(item) = find_raw(proxy_id) else {
        return Err(format!("代理「{proxy_id}」不存在或已被删除"));
    };
    resolve_entry(&item, proxy_id)
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
            // label 换成用户自己起的名字（账号代理列 / 出口测试结果都读它）：
            // 「香港节点」比底层的 `http://127.0.0.1:7890` 更好认；
            // 实际地址仍在 host / port 字段里（测试响应会带出去）
            proxy.label = name;
            Ok(proxy)
        }
        Some(ProxyResolution::Failed(reason)) => Err(format!("代理「{name}」不可用: {reason}")),
        None => Err(format!("代理「{name}」配置不完整")),
    }
}

// ─── 公开形态 ───────────────────────────────────────────────

fn describe_item(item: &Value) -> Value {
    let source = text_of(item, "source");
    let (resolved, resolve_error) = match resolve_item(item) {
        Some(ProxyResolution::Resolved(proxy)) => (
            json!({
                "protocol": proxy.protocol,
                "host": proxy.host,
                "port": proxy.port_json(),
                "label": proxy.label,
            }),
            Value::Null,
        ),
        Some(ProxyResolution::Failed(reason)) => (Value::Null, Value::String(reason)),
        None => (Value::Null, Value::String("配置不完整".to_string())),
    };
    json!({
        "id": text_of(item, "id"),
        "name": text_of(item, "name"),
        "source": source,
        // 有效启用（来源侧启用 ∧ 非本地禁用）：前端拿它当开关的值
        "enabled": is_enabled(item),
        // 本地禁用位（只对镜像条目有意义）：前端据此区分「来源侧关着」与
        // 「本页关着」两种 off —— 前者点不动（改它要去 Clash Verge），后者
        // 是本页的开关，点一下就恢复
        "localDisabled": is_locally_disabled(item),
        // manual 的字段原样带出（编辑弹窗要回显，含密码 —— 与账号代理表单同一口径：
        // 那是本机自己的配置，账号记录里的代理密码同样是明文存的）
        "protocol": text_of(item, "protocol"),
        "host": text_of(item, "host"),
        "port": item.get("port").cloned().unwrap_or(Value::Null),
        "username": text_of(item, "username"),
        "password": text_of(item, "password"),
        // clash 的引用 uid（编辑弹窗选中它 + 前端标注「来自 Clash Verge」）
        "listenerUid": text_of(item, "listenerUid"),
        // subscription 的引用信息（前端标注节点属于哪条订阅 + 同步时定位条目）
        "providerId": text_of(item, "providerId"),
        "node": text_of(item, "node"),
        "createdAt": item.get("createdAt").cloned().unwrap_or(Value::from(0)),
        "updatedAt": item.get("updatedAt").cloned().unwrap_or(Value::from(0)),
        "lastTest": item.get("lastTest").cloned().unwrap_or(Value::Null),
        "resolved": resolved,
        "resolveError": resolve_error,
    })
}

/// 最新全量列表（公开形态）。写操作也返回它 —— 与模型管理页的写接口同约定：
/// 前端就地替换，不必再拉一次。
pub fn list() -> Vec<Value> {
    read_items().iter().map(describe_item).collect()
}

fn find_raw(id: &str) -> Option<Value> {
    if id.is_empty() {
        return None;
    }
    read_items()
        .into_iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
}

// ─── 写操作 ─────────────────────────────────────────────────

pub fn create(input: &Value) -> Result<Vec<Value>, String> {
    let item = normalize_item(input, None)?;
    let mut items = read_items();
    // 新条目排在最前（与账号页「新加的看得见」一致；代理数量少，没有再排序的价值）
    items.insert(0, item.clone());
    write_items(&items)?;
    logging::log(
        "[Proxies]",
        &format!("✅ 新增代理「{}」", text_of(&item, "name")),
    );
    Ok(list())
}

/// 该条目是否来自 Clash 同步（只读镜像）。写操作一律先过它。
fn is_clash_item(item: &Value) -> bool {
    text_of(item, "source") == "clash"
}

/// 该条目是否为订阅节点镜像（只读镜像，见模块头 subscription 一节）
fn is_subscription_item(item: &Value) -> bool {
    text_of(item, "source") == "subscription"
}

/// 只读条目（Clash 同步 / 订阅镜像）的判定 —— 写操作与前端启停都按它挡。
fn is_readonly(item: &Value) -> bool {
    is_clash_item(item) || is_subscription_item(item)
}

/// 只读条目被写操作命中时的统一提示（照 OmniProxy 的文案）
const CLASH_READONLY_HINT: &str =
    "该代理来自 Clash Verge 同步（名称 / 端口 / 启用都跟随 Clash），请到 Clash Verge 中修改";

/// 订阅节点的只读提示（与 Clash 那条同一形态；改去「订阅管理」）
const SUBSCRIPTION_READONLY_HINT: &str =
    "该节点来自订阅（名称 / 端口 / 启用跟随订阅与内核），请在「订阅管理」修改";

fn readonly_hint(item: &Value) -> String {
    if is_subscription_item(item) {
        SUBSCRIPTION_READONLY_HINT.to_string()
    } else {
        CLASH_READONLY_HINT.to_string()
    }
}

pub fn update(input: &Value) -> Result<Vec<Value>, String> {
    let id = clean_string(input.get("id"), MAX_LABEL_LENGTH);
    if id.is_empty() {
        return Err("缺少代理 id".to_string());
    }
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id.as_str()))
    else {
        return Err(format!("代理「{id}」不存在"));
    };
    if is_readonly(&items[index]) {
        return Err(readonly_hint(&items[index]));
    }
    let next = normalize_item(input, Some(&items[index]))?;
    items[index] = next.clone();
    write_items(&items)?;
    logging::log(
        "[Proxies]",
        &format!("✅ 更新代理「{}」", text_of(&next, "name")),
    );
    Ok(list())
}

/// 启用 / 禁用的结果（`api::proxies::pool_enable` 读它决定要不要联动内核）
pub struct EnabledChange {
    /// 条目的展示名（提示文案用）
    pub name: String,
    /// manual | clash | subscription（只有 subscription 需要内核联动）
    pub source: String,
    /// 变更前端口是否在监听（订阅节点的关闭流程判据；其它来源恒 false）
    pub had_port: bool,
    /// 变更后的有效启用状态
    pub enabled: bool,
}

/// 行内启用 / 禁用一条池条目（三种来源都放行，写法各不相同）。
///
///   · manual：直接改写它自己的 `enabled` —— 与过去经 `update` 整份覆盖的效果
///     相同（禁用后引用它的账号回退直连），但少一份「表单少带一个键就丢字段」
///     的风险：这里只碰 `enabled` 与 `updatedAt`；
///   · clash / subscription：改写**本地禁用位** `localDisabled`（理由见模块头）
///     —— `enabled` 是来源侧的镜像值，留给同步写。
///
/// 返回值带上「改动前端口是否开着」与来源：订阅节点禁用时调用方要接着走关闭
/// 流程（内核不给它开监听器，端口要真的收掉），启用时走补建流程。
pub fn set_enabled(id: &str, enabled: bool) -> Result<(Vec<Value>, EnabledChange), String> {
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Err(format!("代理「{id}」不存在"));
    };
    // 先读完需要的信息再拿可变借用（同一元素不能同时借两次）
    let source = text_of(&items[index], "source");
    let name = display_name(&items[index], id);
    // 关闭流程的判据：这条现在是不是真开着**本机**监听端口（只有订阅节点有）
    let had_port = is_subscription_item(&items[index]) && valid_port(items[index].get("port")).is_some();
    let now = logging::now_ms();
    let Some(object) = items[index].as_object_mut() else {
        return Err("代理条目形状异常".to_string());
    };
    if source == "manual" {
        object.insert("enabled".to_string(), Value::Bool(enabled));
    } else if enabled {
        // 开 = 撤掉本地禁用位（键直接删掉：缺键本来就按未禁用算，记录形态也干净）
        object.remove("localDisabled");
    } else {
        object.insert("localDisabled".to_string(), Value::Bool(true));
    }
    object.insert("updatedAt".to_string(), Value::from(now));
    write_items(&items)?;
    logging::log(
        "[Proxies]",
        &format!("{}代理「{name}」", if enabled { "✅ 启用" } else { "⏸️ 禁用" }),
    );
    Ok((list(), EnabledChange { name, source, had_port, enabled }))
}

/// 删除条目；返回（最新列表, 被删条目的名称 —— 调用方拼日志/提示用）。
/// Clash 同步来的条目不可删（照 OmniProxy）：去 Clash 里删出口，下次同步自然摘掉。
pub fn remove(id: &str) -> Result<(Vec<Value>, String), String> {
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Err(format!("代理「{id}」不存在"));
    };
    if is_readonly(&items[index]) {
        return Err(readonly_hint(&items[index]));
    }
    let removed = items.remove(index);
    write_items(&items)?;
    let name = text_of(&removed, "name");
    logging::log("[Proxies]", &format!("🗑️ 删除代理「{name}」"));
    Ok((list(), name))
}

// ─── Clash Verge 同步（全量镜像）─────────────────────────────

/// 同步结果：变更条数 + 失败原因（「Clash 没装」不是错误路径，见 `sync_clash`）
pub struct ClashSyncReport {
    pub changes: usize,
    pub error: Option<String>,
}

/// 一个待镜像的 Clash 出口（合并「混合端口 + 各监听器」两种来源）
struct SyncTarget {
    uid: String,
    name: String,
    enabled: bool,
}

/// 把 Clash 当前出口集合整体镜像进池（详见模块头「同步」一节）。
///
/// 返回值语义：
///   · `error: Some(..)` 只表示**这一次没能同步**（找不到 Clash 配置 / verge.yaml
///     读不出来）—— 池原样保留、不算失败（绝大多数机器上没装 Clash，那是最常见
///     的正常状态，页面上只在 Clash 那一栏给一句说明）；
///   · 同步成功但**什么都没变**时 `changes = 0`，界面提示「已是最新」而不是
///     假装做了事。
///
/// 手动条目从不被碰；镜像集合的判据是 `listenerUid`（条目里存的 Clash 出口 uid）。
/// 手工改库造出的重复 uid（同一条 Clash 出口两条池条目）在收尾的 retain 里去重，
/// 只留第一行 —— 结果自然收敛到一 uid 一条。
pub fn sync_clash() -> ClashSyncReport {
    let snapshot = crate::server::core::clash::clash_snapshot();
    if !snapshot.available {
        return ClashSyncReport {
            changes: 0,
            error: Some(
                snapshot
                    .error
                    .unwrap_or_else(|| crate::server::core::clash::CLASH_UNAVAILABLE.to_string()),
            ),
        };
    }

    let mut targets: Vec<SyncTarget> = Vec::new();
    // 混合端口排在最前（与账号代理下拉里的顺序一致：它是最常用的那条）
    if snapshot.mixed_port.is_some() {
        targets.push(SyncTarget {
            uid: crate::server::core::clash::CLASH_MIXED_UID.to_string(),
            name: "Clash 混合端口（按规则分流）".to_string(),
            enabled: true,
        });
    }
    for listener in &snapshot.listeners {
        targets.push(SyncTarget {
            uid: listener.uid.clone(),
            name: listener.name.clone(),
            enabled: listener.enabled,
        });
    }

    let now = logging::now_ms();
    let mut items = read_items();
    let mut changes = 0usize;

    // ① 目标集合逐条 upsert
    for target in &targets {
        let existing = items.iter().position(|item| {
            is_clash_item(item)
                && item.get("listenerUid").and_then(Value::as_str) == Some(target.uid.as_str())
        });
        match existing {
            Some(index) => {
                // 先读完旧值再拿可变借用（同一元素不能同时借两次）
                let name_changed = text_of(&items[index], "name") != target.name;
                let current_enabled =
                    !matches!(items[index].get("enabled"), Some(Value::Bool(false)));
                if name_changed || current_enabled != target.enabled {
                    if let Some(object) = items[index].as_object_mut() {
                        object.insert("name".to_string(), Value::String(target.name.clone()));
                        object.insert("enabled".to_string(), Value::Bool(target.enabled));
                        object.insert("updatedAt".to_string(), Value::from(now));
                    }
                    changes += 1;
                }
            }
            None => {
                // 端口不落库：clash 条目的地址每次从 Clash 实时解析（见 resolve_item）
                items.insert(
                    0,
                    json!({
                        "id": new_id(),
                        "name": target.name,
                        "source": "clash",
                        "listenerUid": target.uid,
                        "enabled": target.enabled,
                        "createdAt": now,
                        "updatedAt": now,
                        "lastTest": Value::Null,
                    }),
                );
                changes += 1;
            }
        }
    }

    // ② Clash 侧已消失的镜像条目摘掉（严格镜像：池里不存在「半套」状态）
    //
    // 判据是「listenerUid 在不在本次目标集合里」：不在 = Clash 那边删了这个出口。
    // 顺手去重（同一条 Clash 出口只留第一行）—— 正常路径产不出重复（create 不收
    // clash，上一步的 upsert 按 uid 定位），防的是手工改库；没有 listenerUid 的
    // 坏行也在这里被清掉。
    let before = items.len();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    items.retain(|item| {
        if !is_clash_item(item) {
            return true;
        }
        let Some(uid) = item.get("listenerUid").and_then(Value::as_str) else {
            return false;
        };
        if !targets.iter().any(|target| target.uid == uid) {
            return false;
        }
        seen.insert(uid.to_string())
    });
    changes += before - items.len();

    if changes > 0 {
        if let Err(error) = write_items(&items) {
            return ClashSyncReport { changes: 0, error: Some(error) };
        }
        logging::log(
            "[Proxies]",
            &format!("🔄 已同步 Clash Verge 出口（{changes} 项变更）"),
        );
    }
    ClashSyncReport { changes, error: None }
}

/// 一次出口测试的结果（字段与 `egress::ConnectivityResult` 对应）。
///
/// 在这里独立一份而不是直接用 egress 的类型：`egress` 反过来经由
/// `core::proxies` 依赖本模块，为了一个「搬字段」的结构把它引进来会让
/// 依赖图绕一圈；两个类型都由 API 层对接，字段少了编译期就会发现。
pub struct TestOutcome {
    pub success: bool,
    pub ip: String,
    pub duration_ms: i64,
    pub error: Option<String>,
}

/// 记下一次连通性测试的结果（前端行内「测试」与弹窗里的测试都走它）。
pub fn record_test(id: &str, result: &TestOutcome) -> Result<Vec<Value>, String> {
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Err(format!("代理「{id}」不存在"));
    };
    let Some(object) = items[index].as_object_mut() else {
        return Err("代理条目形状异常".to_string());
    };
    let last_test = json!({
        "success": result.success,
        "ip": result.ip,
        "durationMs": result.duration_ms,
        "at": logging::now_ms(),
        // 成功时也带 null（前端按 success 分支取值，不靠键是否存在）
        "error": result.error,
    });
    object.insert("lastTest".to_string(), last_test);
    write_items(&items)?;
    Ok(list())
}

/// 删除一条订阅节点的**孤儿**条目（所属订阅已不存在）。
///
/// 只服务「手动清理」这一条出路：正常路径下订阅删除 / 禁用后条目会随
/// 同步自动摘掉（见 `sync_subscription_nodes`），孤儿只出现在数据被外部
/// 工具清掉的场景（例如旧版进程误删 `subscriptions` 存储键 —— 实测踩过）：
/// 那时条目归属的订阅已经不存在，任何同步都不会再碰它，界面上需要一条
/// 手动删掉它的路。仍然拒绝 clash 条目与手动条目 —— 它们不属于订阅，
/// 不能借这个入口绕过各自的只读 / 正常写路径。
///
/// **是否孤儿由调用方（api 层）判定**：本模块不认识订阅存储（依赖方向
/// 是 subscriptions/sub_kernel → proxy_pool），只认「source == subscription」
/// 这一个形状约束。
pub fn remove_subscription_orphan(id: &str) -> Result<(Vec<Value>, String), String> {
    let mut items = read_items();
    let Some(index) = items
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Err(format!("代理「{id}」不存在"));
    };
    if !is_subscription_item(&items[index]) {
        return Err(readonly_hint(&items[index]));
    }
    let removed = items.remove(index);
    write_items(&items)?;
    let name = text_of(&removed, "name");
    logging::log(
        "[Proxies]",
        &format!("🗑️ 删除订阅节点「{name}」（所属订阅已不存在，手动清理）"),
    );
    Ok((list(), name))
}

// ─── 订阅节点同步（全量镜像，内核拉到订阅后调用）─────────────

/// 一个待镜像的订阅节点（`core::sub_kernel` 在 provider 同步后构造）。
///
/// 定义在**本模块**而不是 sub_kernel：依赖方向是 sub_kernel → proxy_pool
/// （内核把节点写进来），反向会让两个模块成环 —— 订阅条目的端口存在条目
/// 自身里，本模块的解析（`resolve_item`）不需要回查内核。
pub struct SubscriptionNodeTarget {
    /// 订阅 id（与 `core::subscriptions` 条目的 id 一致；同步定位条目用它）
    pub provider_id: String,
    /// 订阅名（条目名的前半段）
    pub sub_name: String,
    /// 节点名（条目名的后半段；同时是内核配置里监听器绑定的出口名）
    pub node: String,
    /// 内核监听端口；**None = 未分配**（lazy listeners：内核只为被账号引用的
    /// 节点开监听器，未引用的节点仍镜像进池供浏览选择，端口为空 —— 引用后
    /// 由对账链路触发内核重启补上，见 `api::proxies::reconcile_listeners`）
    pub port: Option<u16>,
}

/// 把订阅节点集合整体镜像进池（与 `sync_clash` 同一套只读镜像语义）：
///
///   · 集合里的节点 upsert（按 `providerId + node` 定位：**已存在的沿用 id**
///     —— 账号按 id 引用池条目，重生成 id 会让引用全部悬空）；
///   · 订阅侧已消失的（或订阅被删 / 被禁用后不再出现在集合里的）镜像条目摘掉；
///   · manual 与 clash 条目从不被碰。
///
/// 返回变更条数（「已是最新」时为 0）。注意目标集合是**全部启用订阅的全部
/// 节点**：只传一条订阅的节点时，其它订阅的条目会被当作「已消失」摘掉
/// （调用方要传全量 —— sub_kernel 的同步总是构造全量）。
pub fn sync_subscription_nodes(targets: &[SubscriptionNodeTarget]) -> Result<usize, String> {
    let now = logging::now_ms();
    let mut items = read_items();
    let mut changes = 0usize;

    // ① 目标集合逐条 upsert（沿用已有条目的 id / createdAt / lastTest）
    for target in targets {
        let name = if target.sub_name.is_empty() {
            target.node.clone()
        } else {
            format!("{} · {}", target.sub_name, target.node)
        };
        let existing = items.iter_mut().position(|item| {
            is_subscription_item(item)
                && item.get("providerId").and_then(Value::as_str) == Some(target.provider_id.as_str())
                && item.get("node").and_then(Value::as_str) == Some(target.node.as_str())
        });
        match existing {
            Some(index) => {
                let current_port = items[index].get("port").and_then(Value::as_u64).map(|v| v as u16);
                let port_changed = current_port != target.port;
                let name_changed = text_of(&items[index], "name") != name;
                if port_changed || name_changed {
                    if let Some(object) = items[index].as_object_mut() {
                        object.insert("name".to_string(), Value::String(name));
                        // None = 未分配端口（lazy listeners）：写 null 而不是删键，
                        // 前端按 null 显示「未分配」
                        object.insert(
                            "port".to_string(),
                            target.port.map(|port| Value::from(port)).unwrap_or(Value::Null),
                        );
                        object.insert("updatedAt".to_string(), Value::from(now));
                    }
                    changes += 1;
                }
            }
            None => {
                items.insert(
                    0,
                    json!({
                        "id": new_id(),
                        "name": name,
                        "source": "subscription",
                        "providerId": target.provider_id,
                        "node": target.node,
                        "enabled": true,
                        // None = 未分配（lazy listeners，见 SubscriptionNodeTarget.port）
                        "port": target.port.map(|port| Value::from(port)).unwrap_or(Value::Null),
                        "createdAt": now,
                        "updatedAt": now,
                        "lastTest": Value::Null,
                    }),
                );
                changes += 1;
            }
        }
    }

    // ② 目标集合之外（且 providerId 不在集合里）的订阅镜像条目摘掉。
    //
    // 判据刻意按 **providerId** 而不是节点名：节点名随订阅内容变化（机场
    // 改名很常见），按名判定会把改名后的节点删了重加（id 变了，账号引用
    // 悬空）。providerId 不在本次集合 = 该订阅被删 / 被禁用 / 已无节点，
    // 它名下的全部条目才真正该走。
    let live_providers: std::collections::HashSet<&str> = targets
        .iter()
        .map(|target| target.provider_id.as_str())
        .collect();
    let before = items.len();
    items.retain(|item| {
        if !is_subscription_item(item) {
            return true;
        }
        let Some(provider_id) = item.get("providerId").and_then(Value::as_str) else {
            return false; // 没有引用信息的坏行（手工改库），清掉
        };
        live_providers.contains(provider_id)
    });
    changes += before - items.len();

    if changes > 0 {
        write_items(&items)?;
    }
    Ok(changes)
}

/// 把**全部订阅镜像条目**的端口清空（删内核时调用，见
/// `sub_kernel` worker 的 Remove 分支）。
///
/// ── 为什么清端口而不是把条目摘掉 ────────────────────────────
/// `sync_subscription_nodes(&[])` 会把订阅镜像整批摘掉，但重装内核后重新镜像
/// 出来的是**新 id**（`new_id()`），账号里存的旧 id 就永久悬空了 —— 用户只是
/// 删了个内核，回头再装上却发现所有引用订阅节点的账号都解析不出来，得逐个重配。
/// 端口置空的语义反而更准：节点还在（订阅还在），只是**这一轮没有监听器**，
/// 与 lazy listeners 的「未分配端口」是同一个状态；重装内核后同步按
/// `providerId + node` 命中同一条，端口原位写回，账号引用自然复活。
///
/// 保留 `lastTest`（历史读数，重启换端口后同样会留存）。返回变更条数。
pub fn unprovision_subscription_nodes() -> Result<usize, String> {
    let now = logging::now_ms();
    let mut items = read_items();
    let mut changes = 0usize;
    for item in items.iter_mut() {
        if !is_subscription_item(item) || valid_port(item.get("port")).is_none() {
            continue;
        }
        let Some(object) = item.as_object_mut() else { continue };
        object.insert("port".to_string(), Value::Null);
        object.insert("updatedAt".to_string(), Value::from(now));
        changes += 1;
    }
    if changes > 0 {
        write_items(&items)?;
    }
    Ok(changes)
}

/// 被引用的订阅节点键集合（`<订阅id>::<节点名>`，与内核端口注册表的键同一形态）。
///
/// `pool_ids` 是账号 proxy 指向的池条目 id（调用方从账号存储收集）。id 不在池里
/// 的引用（条目已被删）查不出节点、自然跳过 —— 那类引用解析时本来就会报「不存在」。
/// 内核的 `allocate_listeners` 用它决定「哪些节点开监听器」（lazy listeners）。
///
/// **本页禁用中的条目不算**（`is_enabled`）：账号虽然还引用着它，但引用解析已经
/// 按「已禁用」失败并回退直连了（见 `resolve_reference`）—— 再给它开着监听端口
/// 就是为一条没人用的节点白占端口，也与用户在启用列上按下的那一下自相矛盾。
pub fn subscription_node_keys(pool_ids: &std::collections::HashSet<&str>) -> std::collections::HashSet<String> {
    read_items()
        .iter()
        .filter(|item| is_subscription_item(item) && is_enabled(item))
        .filter(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .map(|id| pool_ids.contains(id))
                .unwrap_or(false)
        })
        .filter_map(|item| {
            let provider_id = item.get("providerId").and_then(Value::as_str)?;
            let node = item.get("node").and_then(Value::as_str)?;
            Some(format!("{provider_id}::{node}"))
        })
        .collect()
}

/// 内核该给哪些节点开监听器：**账号引用中的条目**（禁用中的不算）。
///
/// 内核侧（`runtime::listener_node_keys`）与对账判定
/// （`api::proxies::reconcile_listeners`）都从这一处取，必须同一口径
/// —— 口径不一致就会出现「判定说没有缺口、内核却不建」这类谁也解释不清的
/// 状态。测试不在此列（测试走统一测试口，不需要端口，见模块头）。
pub fn listener_node_keys(pool_ids: &std::collections::HashSet<&str>) -> std::collections::HashSet<String> {
    subscription_node_keys(pool_ids)
}

/// 测试目标（`api::proxies::pool_test` 读它）：条目的展示名与订阅节点的定位
/// 信息（manual / clash 条目的 `provider_id` / `node` 为空串 —— 它们不走内核）。
pub struct TestTarget {
    /// 条目展示名（测试结果与日志里标「测的是谁」）
    pub name: String,
    /// manual | clash | subscription
    pub source: String,
    /// 订阅 id（订阅节点定位内核里的节点用）
    pub provider_id: String,
    /// 节点名（同上）
    pub node: String,
}

/// 找一条池条目作为测试目标（不存在报错）。
pub fn test_target(id: &str) -> Result<TestTarget, String> {
    let Some(item) = find_raw(id) else {
        return Err(format!("代理「{id}」不存在或已被删除"));
    };
    Ok(TestTarget {
        name: display_name(&item, id),
        source: text_of(&item, "source"),
        provider_id: text_of(&item, "providerId"),
        node: text_of(&item, "node"),
    })
}

/// 这条订阅条目现在**需要监听器却没有端口**吗（启用后要不要补建的判据）。
///
/// 判据与 `listener_node_keys` 完全同口径（账号引用，禁用中的不算），只是限定
/// 到一条：条目自身已启用（禁用中的节点不要监听器）、是订阅节点、端口还空着，
/// 且确实被引用。`referenced` 是账号引用中的池条目 id 集合（调用方从账号存储取）。
///
/// 与内核侧同口径是硬要求：口径不一致就会出现「判定说没有缺口、内核却不建」
/// 这类谁也解释不清的状态（补建那条链的注释里也写着同一句话）。
pub fn needs_listener(id: &str, referenced: &std::collections::HashSet<&str>) -> bool {
    let Some(item) = find_raw(id) else {
        return false;
    };
    if !is_subscription_item(&item) || !is_enabled(&item) {
        return false;
    }
    if valid_port(item.get("port")).is_some() {
        return false;
    }
    // `listener_node_keys` 的唯一来源是账号引用；条目已启用，引用即算数
    referenced.contains(id)
}

/// 列出池里订阅节点的**端口状态**（`{providerId, node, port}`，`port` 为
/// null 表示未分配）。
///
/// 对账链路（`api::proxies::reconcile_listeners`）的读入口：拿它和「该开
/// 监听器的节点集合」（`listener_node_keys`）比对 —— 该开却没端口的要补建，
/// 端口还挂着却不该开的要回收。`provider_nodes`（provider 缓存）里没有的
/// 节点跳过 —— 订阅还没拉到内容时谈不上补建。
pub fn subscription_entries_with_ports() -> Vec<(String, String, Option<u16>)> {
    read_items()
        .iter()
        .filter(|item| is_subscription_item(item))
        .filter_map(|item| {
            let provider_id = item.get("providerId").and_then(Value::as_str)?.to_string();
            let node = item.get("node").and_then(Value::as_str)?.to_string();
            let port = item.get("port").and_then(Value::as_u64).map(|v| v as u16);
            Some((provider_id, node, port))
        })
        .collect()
}
