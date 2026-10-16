//! 内核运行时：配置生成、进程 spawn/kill、mihomo 控制器调用、节点同步。
//!
//! ── 与 mihomo 的全部交互只有两条本地通道 ──────────────────────
//!   ① 配置文件：`{kernel_dir}/config.yaml`（本模块生成，serde_yaml 序列化
//!      —— 节点名是用户提供的数据，字符串拼接会坏 YAML，一律走结构化序列化）；
//!   ② 控制器 REST：`http://127.0.0.1:<port>`（Bearer secret，绑定 127.0.0.1
//!      —— 只本机可达；secret 随机生成、落在 kernel-state.json 里，与内核
//!      进程一一对应）。
//! 进程是独立程序（GPL「聚合」边界，见 mod.rs 模块头），不做任何进程内嵌入。
//!
//! ── 启动流程（run_restart）─────────────────────────────────
//!   ① 清理上次运行遗留的内核进程（pid 从状态文件里来，控制器以我们的
//!      secret 响应确认「是我们的」才杀 —— pid 复用会误伤无关进程）；
//!   ② 读 provider 缓存文件拿节点名 + 节点定义（缓存缺失 = 该订阅还没拉到过，
//!      先不给它生成监听器 —— 内核启动时自己会拉，拉到后第 ⑤ 步补一轮）；
//!   ③ 端口分配：控制端口、统一测试口与节点端口从状态文件里沿用，缺了才
//!      现分配（绑定探测挑空闲口）；重启后端口保持稳定，账号引用不漂移；
//!   ④ 生成配置 → 杀旧进程 → spawn 新进程 → 等控制器就绪；
//!   ⑤ 轮询控制器拿各 provider 的节点清单 → 节点集合与配置不一致（首次
//!      拉到 / 订阅更新过）时重建配置 + 重启一次；
//!   ⑥ 节点镜像进代理池（`proxy_pool::sync_subscription_nodes`）。
//!
//! 配置里订阅节点是**全量内联**进顶层 `proxies` 的（监听器的 `proxy` 只认
//! 顶层 proxy 或策略组名，provider 的节点不在顶层 —— 详见 `build_config` 的
//! 「内联」一节；这是「订阅节点全都测不通」那个 bug 的根因）。监听器有两类：
//! **每节点一个专用端口**（只给账号引用中的节点，lazy listeners）与**一个
//! 统一测试口**（背后是 select 策略组，「测试」切组即换节点，见
//! `select_test_node`）。
//!
//! ── 失败形态 ────────────────────────────────────────────────
//! spawn 失败 / 控制器起不来 / 订阅拉取失败都返回 Err（worker 记进快照的
//! last_error）；某一家订阅拉不到不影响其它家的监听器（错误文案里点名是
//! 哪一家，内核照常带着其余节点跑）。

use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpListener};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Map, Value};
use serde_yaml::Value as Yaml;

use crate::server::core::proxy_pool::SubscriptionNodeTarget as NodeTarget;
use crate::server::core::sub_kernel::{
    log_line, record_provider_sync, record_running, sync_nodes_to_pool, ProviderInput,
    KERNEL_VERSION,
};
use crate::server::logging;

/// 控制端口候选范围（避开 Clash Verge 默认的 7897 与常见 9090）
const CONTROLLER_PORT_BASE: u16 = 19090;
const CONTROLLER_PORT_END: u16 = 19099;
/// 统一测试口的候选范围（在控制端口段下面，与节点端口段的默认区间 19101+
/// 不重叠；见 `ensure_test_port` 与 `build_config` 的「统一测试口」一节）
const TEST_PORT_BASE: u16 = 19080;
const TEST_PORT_END: u16 = 19089;
/// 统一测试口背后的策略组名（成员 = 全部节点；「测试」= 把它切到目标节点再
/// 从测试口发请求，见 `select_test_node`）。名字避开节点别名前缀 `wbn_`。
const TEST_GROUP_NAME: &str = "wbtest";
const TEST_LISTENER_NAME: &str = "wbtest-in";
/// 等控制器就绪 / 等 provider 拉取的总时长
const CONTROLLER_WAIT_MS: u64 = 10_000;
const PROVIDER_WAIT_MS: u64 = 25_000;
/// 等测试口就绪的上限（内核没在跑时投一次重启，重建 + 拉 provider 的慢路径
/// 在 25 秒级，且「订阅缓存刚落地 → 轮询发现集合变了 → 再重建一次」会再叠
/// 一轮 —— 上限必须盖过它，否则「订阅刚加、首次拉取慢」的正常情况会被判成
/// 超时。与旧补建流程的 45 秒同一量级）
const TEST_READY_WAIT_MS: u64 = 45_000;
/// 状态文件名（内核目录下：{version, controllerPort, testPort, secret, ports, pid}）
const STATE_FILE: &str = "kernel-state.json";

/// 内核子进程句柄（kill_on_drop(true)：句柄 drop 时进程被杀 —— 重启与停机
/// 都是「取走句柄 → drop」，不需要额外的杀进程入口；drop 后的收尾由
/// spawn_task 里的 wait 完成，不留僵尸）。
static CHILD: std::sync::Mutex<Option<tokio::process::Child>> = std::sync::Mutex::new(None);

fn lock_child() -> std::sync::MutexGuard<'static, Option<tokio::process::Child>> {
    match CHILD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 杀掉内核子进程（停机钩子与重启共用；没有子进程时是 no-op）。
///
/// 可能从**同步上下文**调（backend::shutdown 不在 async 里）：
/// `start_kill()` 是同步的（只发信号），僵尸收尾交给运行时 wait。
pub fn kill_child() {
    let Some(mut child) = lock_child().take() else {
        return;
    };
    let _ = child.start_kill();
    crate::spawn_task(async move {
        let _ = child.wait().await;
    });
}

// ─── 状态文件（端口注册表 + secret + pid）──────────────────

fn state_file_path() -> std::path::PathBuf {
    crate::server::core::sub_kernel::kernel_dir().join(STATE_FILE)
}

fn read_state() -> Value {
    std::fs::read_to_string(state_file_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_state(state: &Value) -> Result<(), String> {
    let dir = crate::server::core::sub_kernel::kernel_dir();
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建内核目录失败: {error}"))?;
    let text =
        serde_json::to_string_pretty(state).map_err(|error| format!("编码内核状态失败: {error}"))?;
    std::fs::write(state_file_path(), text).map_err(|error| format!("写入内核状态失败: {error}"))
}

/// 从状态里读「节点键 → 端口」的映射（nodeKey = `<订阅id>::<节点名>`）
fn state_ports(state: &Value) -> Map<String, Value> {
    state
        .get("ports")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// 往状态对象里写一个键（Value 顶层没有 insert，走 as_object_mut）
fn state_insert(state: &mut Value, key: &str, value: Value) {
    if let Some(object) = state.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

/// 状态里的端口号（数字 1-65535；serde_json 没有 as_u16，走 as_u64 收窄）
fn json_u16(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .filter(|number| (1..=65535).contains(number))
        .map(|number| number as u16)
}

// ─── 端口分配 ──────────────────────────────────────────────

/// 端口是否空闲（绑定探测：bind 成功后立即 drop）
fn port_free(port: u16) -> bool {
    let Ok(address) = format!("127.0.0.1:{port}").parse::<SocketAddr>() else {
        return false;
    };
    TcpListener::bind(address).is_ok()
}

/// 从 [from, end] 里挑第一个空闲端口（`taken` 是已注册 / 已占用的集合）
fn alloc_port(taken: &HashSet<u16>, from: u16, end: u16) -> Option<u16> {
    (from..=end).find(|port| !taken.contains(port) && port_free(*port))
}

// ─── provider 缓存读取 ─────────────────────────────────────

/// 读一个 provider 缓存文件里的节点名（缺失 / 解析失败 / 没有 proxies 都给空表
/// —— 内核启动时会自己拉，拉到后第 ⑤ 步补监听器）。
fn read_provider_nodes(provider_id: &str) -> Vec<String> {
    let path = crate::server::core::sub_kernel::kernel_dir()
        .join("providers")
        .join(format!("sub_{provider_id}.yaml"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_yaml::from_str::<Yaml>(&text) else {
        return Vec::new();
    };
    let Some(items) = parsed.get("proxies").and_then(Yaml::as_sequence) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.get("name").and_then(Yaml::as_str).map(str::to_string))
        .collect()
}

/// 读各订阅缓存里的**节点定义**（`<订阅id>::<节点名>` → mihomo 的 proxy 条目）。
///
/// ── 为什么要读它 ────────────────────────────────────────────
/// 监听器的 `proxy` 字段只认**顶层** proxy 或策略组名，而 `proxy-providers` 里的
/// 节点不在顶层 —— 直接把节点名写进监听器，内核每次连接都会
/// `[Metadata] parse failed: proxy X not found` 然后中止连接（Windows 上客户端
/// 看到的是 os error 10053「软件中止了一个已建立的连接」；实测复现过）。所以
/// 被选中的节点要**内联**进配置的顶层 `proxies`（见 `build_config`），定义就从
/// 这个缓存文件里取 —— 它是 mihomo 自己拉下来的订阅原文，节点定义是完整的
/// （协议 / 传输层 / TLS 各项都在），照着搬即可。
///
/// 取不到的条目直接跳过（不是错误）：形状认不出来的节点这轮开不了监听器，
/// 池里照常显示它、端口留空，下一次重启再试。
fn read_provider_definitions(providers: &[ProviderInput]) -> HashMap<String, Value> {
    let dir = crate::server::core::sub_kernel::kernel_dir().join("providers");
    let mut defs: HashMap<String, Value> = HashMap::new();
    for provider in providers {
        let Ok(text) = std::fs::read_to_string(dir.join(format!("sub_{}.yaml", provider.id))) else {
            continue;
        };
        let Ok(parsed) = serde_yaml::from_str::<Yaml>(&text) else {
            continue;
        };
        let Some(items) = parsed.get("proxies").and_then(Yaml::as_sequence) else {
            continue;
        };
        for item in items {
            let Some(name) = item.get("name").and_then(Yaml::as_str) else {
                continue;
            };
            // YAML → JSON：配置是 json! 拼出来再交给 serde_yaml 序列化的，定义也要
            // 走同一形态；带 YAML 特有标量（时间戳等）的条目转不过去，跳过即可
            let Ok(definition) = serde_json::to_value(item) else {
                continue;
            };
            defs.insert(node_key(&provider.id, name), definition);
        }
    }
    defs
}

// ─── 控制器调用（本地 REST，Bearer secret）─────────────────

/// 控制器 GET（响应失败一律 None —— 轮询语义里「还没就绪」与「失败」同路）
async fn controller_get(port: u16, secret: &str, path: &str) -> Option<Value> {
    let response = controller_client()
        .get(format!("http://127.0.0.1:{port}{path}"))
        .header("Authorization", format!("Bearer {secret}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

/// 控制器 PUT（强制刷新 provider 用）
async fn controller_put(port: u16, secret: &str, path: &str) -> Result<(), String> {
    let response = controller_client()
        .put(format!("http://127.0.0.1:{port}{path}"))
        .header("Authorization", format!("Bearer {secret}"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| format!("控制器请求失败: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("控制器返回 {}", response.status().as_u16()))
    }
}

/// 控制器 PUT + JSON body（切换策略组的选择用，见 `select_test_node`）。
///
/// 与 `controller_put`（无 body 的 provider 刷新）分开：切组要看**状态码**——
/// 404 意味着组或组成员不存在，调用方要给「节点不在内核里」这种可行动的话，
/// 而不是笼统的「控制器返回 404」。
async fn controller_put_json(
    port: u16,
    secret: &str,
    path: &str,
    body: &Value,
) -> Result<(), String> {
    let response = controller_client()
        .put(format!("http://127.0.0.1:{port}{path}"))
        .header("Authorization", format!("Bearer {secret}"))
        .json(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| format!("控制器请求失败: {error}"))?;
    let status = response.status().as_u16();
    if response.status().is_success() {
        return Ok(());
    }
    if status == 404 {
        return Err("内核里没有这个代理或策略组（配置可能还是旧版，重启内核后重试）".to_string());
    }
    Err(format!("控制器返回 {status}"))
}

/// 控制器专用客户端：**显式 no_proxy** —— 控制器是本机回环，绝不能被环境里的
/// HTTPS_PROXY 悄悄改写（否则控制器调用会打到环境代理上并失败）。与
/// egress 的直连客户端同一手法，但那里缓存键带超时设置、代理可配，这里
/// 独立一份免得与出网缓存互相干扰。
fn controller_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 轮询控制器拿「全部 provider 的节点清单」：`{subId: [节点名]}`。
///
/// 内核启动后 provider 是异步拉的（缓存缺失时才现拉），所以给
/// `wait_ms` 的窗口轮询；有任一 provider 出现节点就提前收手
/// （各家独立，不互相等）。
async fn poll_providers(
    port: u16,
    secret: &str,
    provider_ids: &[String],
    wait_ms: u64,
) -> Map<String, Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    let mut latest: Map<String, Value> = Map::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(value) = controller_get(port, secret, "/providers/proxies").await {
            if let Some(providers) = value.get("providers").and_then(Value::as_object) {
                for provider_id in provider_ids {
                    let Some(provider) = providers.get(&format!("sub_{provider_id}")) else {
                        continue;
                    };
                    let names: Vec<Value> = provider
                        .get("proxies")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.get("name").and_then(Value::as_str).map(|name| json!(name)))
                                .collect()
                        })
                        .unwrap_or_default();
                    latest.insert(provider_id.clone(), Value::Array(names));
                }
            }
            // 至少一家拿到了节点就收手 —— 拉取失败的留给下一次 restart 再试
            let any_nodes = latest
                .values()
                .any(|value| value.as_array().map(|items| !items.is_empty()).unwrap_or(false));
            if any_nodes {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    latest
}

// ─── 配置生成 ──────────────────────────────────────────────

/// 生成 mihomo 配置（结构化 → serde_yaml 序列化，节点名不做字符串拼接）。
///
/// 刻意**不开 TUN、不接管系统代理、没有分流规则**（只有一条 MATCH,DIRECT
/// 兜底）：本内核只服务「每节点一个本地监听器」这一件事，系统代理 / 全局
/// 分流是用户自己的 Clash Verge 的事，这里越界只会打架。也不带 geo 规则
/// —— 用不到 geoip/geosite，内核不会去下载那几十 MB 的地理库。
///
/// ── 订阅节点要**内联**进顶层 `proxies`（否则监听器全是死的）────────
/// `listeners[].proxy` 只认顶层 proxy 或策略组名（见 mihomo 文档 inbound 一节：
/// 「值必须是合法的 proxy 名，否则报错」），而 `proxy-providers` 里的节点**不在
/// 顶层** —— provider 的节点只通过 `proxy-groups` 的 `use` 进入视图。早先这里
/// 直接把节点名写进监听器，实测内核每次连接都报
/// `[Metadata] parse failed: proxy 日本-优化3 not found` 然后中止连接，客户端
/// 侧就是 os error 10053（「软件中止了一个已建立的连接」）；同一节点走 Clash
/// Verge 的端口却是通的（那条路不经过本内核），所以现象是「订阅节点全测不通、
/// Clash 节点都能测通」。
///
/// 所以：`defs` 里有的节点，定义原样搬进顶层 `proxies`、只把 `name` 换成
/// `wbn_<节点键哈希>`（**内部名**，与监听器名同一字符串 —— 一对一的节点身份，
/// 顺带避开「两条订阅有同名节点」这种顶层 proxy 重名冲突），监听器 `proxy`
/// 也指这个内部名。`proxy-providers` 保留：节点清单（池镜像 / 控制器查询）与
/// 周期更新还是走它，只是不再被监听器直接引用。
///
/// ── 内联是**全量**的（不只是分到端口的那批）──────────────────
/// 统一测试口（见下）背后的策略组成员是**全部节点**——「测试」要能切到任何
/// 一条节点，所以缓存里有定义的节点全部进顶层。几个节点定义的量级对内核的
/// 配置体积与内存可以忽略（Clash Verge 的配置本来就是整份订阅全量）。
/// 按 key 排序产出：HashMap 遍历序是随机的，不排的话每次重启的配置文件
/// 内容都会漂移，没法对照排障。
///
/// ── 统一测试口（一个 select 组 + 一个 mixed 监听器）────────────
/// 参照 Clash Verge 的形态：固定端口 `test_port` 的监听器出口指向一个
/// select 策略组（成员 = 全部节点别名）。「测试」= 控制器把组切到目标节点 →
/// 从测试口发请求（见 `select_test_node`），**不再为被测节点开专用端口**；
/// 专用端口只服务真正在用的节点（账号引用，lazy listeners）。没有节点时
/// 不生成组与监听器（空策略组会让内核配置解析失败）。
fn build_config(
    providers: &[ProviderInput],
    controller_port: u16,
    secret: &str,
    test_port: u16,
    listeners: &[NodeTarget],
    defs: &HashMap<String, Value>,
) -> Result<String, String> {
    let mut proxy_providers = Map::new();
    for provider in providers {
        // 本地文件订阅（content 非空）用 file provider：path 就是统一缓存位，
        // 内容由 run_restart 每次重启前从订阅存储写过去。没有 url、也没有
        // interval —— 文件即真相，「定时拉取」对它没有意义。health-check 照常
        // （file provider 同样支持，测试口 / 延迟检测靠它）。
        let entry = if provider.content.is_empty() {
            json!({
                "type": "http",
                "url": provider.url,
                "interval": 86400,
                "path": format!("providers/sub_{}.yaml", provider.id),
                "health-check": {
                    "enable": true,
                    "url": "https://www.gstatic.com/generate_204",
                    "interval": 1800,
                },
            })
        } else {
            json!({
                "type": "file",
                "path": format!("providers/sub_{}.yaml", provider.id),
                "health-check": {
                    "enable": true,
                    "url": "https://www.gstatic.com/generate_204",
                    "interval": 1800,
                },
            })
        };
        proxy_providers.insert(format!("sub_{}", provider.id), entry);
    }
    // 全量内联：defs 里有的节点全部进顶层（测试组要能切到任何一条）。
    // HashMap 遍历序随机，按 key 排序让配置输出稳定可对照。
    let mut inline: Vec<(&String, &Value)> = defs.iter().collect();
    inline.sort_by(|a, b| a.0.cmp(b.0));
    let mut proxies: Vec<Value> = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    for (key, definition) in inline {
        let Some(object) = definition.as_object() else { continue };
        let alias = alias_of_key(key);
        let mut entry = object.clone();
        entry.insert("name".to_string(), Value::String(alias.clone()));
        aliases.push(alias);
        proxies.push(Value::Object(entry));
    }
    let mut listener_list: Vec<Value> = listeners
        .iter()
        // 未分配端口的节点（lazy listeners：还没被账号引用）不进配置 ——
        // 它们只参与代理池镜像，被引用后由补建链路补上
        .filter_map(|target| {
            let port = target.port?;
            let alias = node_alias(&target.provider_id, &target.node);
            let key = node_key(&target.provider_id, &target.node);
            // 全量内联后这里的缺项只剩「两次读缓存之间节点没了」的兜底：
            // 跳过，不留死监听器
            if !defs.contains_key(&key) {
                return None;
            }
            Some(json!({
                "name": alias,
                "type": "mixed",
                "port": port,
                "listen": "127.0.0.1",
                "proxy": alias,
            }))
        })
        .collect();
    // 统一测试口（没有节点时不生成：空策略组会让内核配置解析失败）
    let mut proxy_groups: Vec<Value> = Vec::new();
    if !aliases.is_empty() {
        proxy_groups.push(json!({
            "name": TEST_GROUP_NAME,
            "type": "select",
            "proxies": aliases,
        }));
        listener_list.push(json!({
            "name": TEST_LISTENER_NAME,
            "type": "mixed",
            "port": test_port,
            "listen": "127.0.0.1",
            "proxy": TEST_GROUP_NAME,
        }));
    }
    let config = json!({
        "mode": "rule",
        "log-level": "warning",
        "ipv6": false,
        "external-controller": format!("127.0.0.1:{controller_port}"),
        "secret": secret,
        "profile": { "store-selected": false },
        "proxies": proxies,
        "proxy-groups": proxy_groups,
        "proxy-providers": proxy_providers,
        "rules": ["MATCH,DIRECT"],
        "listeners": listener_list,
    });
    serde_yaml::to_string(&config).map_err(|error| format!("编码内核配置失败: {error}"))
}

/// 节点键（端口注册表与代理池条目共用的稳定标识）
fn node_key(provider_id: &str, node: &str) -> String {
    format!("{provider_id}::{node}")
}

/// 节点的**内部名**（监听器名与内联 proxy 名是同一个）
///
/// 节点名含任意字符，做监听器名 / 顶层 proxy 名不安全（mihomo 名里有 `,` 之类
/// 也可能踩到别处的解析）；哈希稳定（同节点同名），一一对应。两个名字用同一
/// 字符串是刻意的：日志里看到的 `wbn_xxxx using wbn_xxxx` 就是「这条监听器直连
/// 这个节点」，不用再对一层映射。
fn node_alias(provider_id: &str, node: &str) -> String {
    format!("wbn_{}", short_hash(&node_key(provider_id, node)))
}

/// 节点键 → 内部名（`alias_of_key(node_key(..)) == node_alias(..)`）。
/// 全量内联那一步手里只有 key（defs 的键），用它直接算别名。
fn alias_of_key(key: &str) -> String {
    format!("wbn_{}", short_hash(key))
}

/// 本次该给哪些节点开监听器的节点键集合（`<订阅id>::<节点名>`）。
///
/// = **账号引用中的池条目**（`proxy_pool::listener_node_keys`，禁用中的条目
/// 不算）。专用端口只服务「真正在用」的节点 —— 测试走统一测试口（见
/// `select_test_node`），不再需要为它开端口；曾经还有「被测试钉住」这半个
/// 来源，统一测试口上线后随 pinned 机制一起退役了。
///
/// 账号存储未接线 / 不可用 / 读失败时**只**给空引用集合（不整体返回空）：
/// 与 `proxy_pool` 的「解析失败按空表处理」同一取向。
fn listener_node_keys() -> HashSet<String> {
    let pool_ids = crate::server::core::sub_kernel::account_store()
        .map(|store| store.referenced_pool_ids())
        .unwrap_or_default();
    let borrowed: HashSet<&str> = pool_ids.iter().map(String::as_str).collect();
    crate::server::core::proxy_pool::listener_node_keys(&borrowed)
}

/// 按**现存订阅**裁剪端口注册表（分配之前调用）。
///
/// ── 为什么必须裁 ────────────────────────────────────────────
/// 注册表（`kernel-state.json` 的 `ports`）按 `<订阅id>::<节点名>` 记端口，而
/// 订阅删除后**重新添加会换 providerId**（id 由创建时刻生成）——旧键就再也
/// 对不上任何节点，却仍被 `alloc_port` 的 `taken` 当成已占用。删一次再拖回
/// 一条 30 节点的订阅就白耗 30 个号；实测默认区间 19101-19200 里有 90 个被
/// 三条已删订阅占着，只剩 10 个可用（报「没有可用的节点端口」只是时间问题）。
///
/// 裁剪判据是「订阅 id 还在本次要建的订阅清单里」而不是「端口空闲」：
/// 端口可能被内核自己正在用（重启瞬间），按端口判会把在用的号清掉。
/// 订阅被禁用时也不在清单里（`enabled_provider_inputs` 只给启用的），它的
/// 端口一并回收 —— 那些节点本来就不会出现在池里，账号引用早已解析失败。
fn prune_ports(state: &mut Value, providers: &[ProviderInput]) {
    let live: HashSet<&str> = providers.iter().map(|provider| provider.id.as_str()).collect();
    let kept: Map<String, Value> = state_ports(state)
        .into_iter()
        .filter(|(key, _)| {
            key.split("::")
                .next()
                .map(|provider_id| live.contains(provider_id))
                .unwrap_or(false)
        })
        .collect();
    state_insert(state, "ports", Value::Object(kept));
}

/// 短哈希（节点内部名用，只求稳定不求密码学强度）
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:08x}")
}

// ─── 内核启动 / 重启 ───────────────────────────────────────

/// 重建配置 + 启动（或重启）内核 + 同步节点（详见模块头「启动流程」）。
pub async fn run_restart(providers: &[ProviderInput]) -> Result<(), String> {
    if !crate::server::core::sub_kernel::installed() {
        return Err("代理内核未安装（先在「订阅管理」下载内核）".to_string());
    }
    let dir = crate::server::core::sub_kernel::kernel_dir();
    std::fs::create_dir_all(dir.join("providers"))
        .map_err(|error| format!("创建内核目录失败: {error}"))?;

    // ⓪ 本地文件订阅：把存储里的 YAML 原文写进 provider 缓存位。
    //    http 订阅的缓存由内核自己拉取，本地文件没人替它写 —— 所以**每次重启
    //    都写**（而不是只在导入时写一次）：内核目录可能换过（kernelDir 改到
    //    别的盘，providers 目录不会跟着搬）、缓存文件可能被外部清掉，以存储为
    //    准写一遍全对齐。必须落在读节点 / 定义（②）之前，首轮启动就能带上节点。
    for provider in providers {
        if provider.content.is_empty() {
            continue;
        }
        let path = dir
            .join("providers")
            .join(format!("sub_{}.yaml", provider.id));
        std::fs::write(&path, provider.content.as_str())
            .map_err(|error| format!("写入本地订阅文件失败（{}）: {error}", provider.name))?;
    }

    // ① 遗留进程清理（上次崩溃留下的内核还占着控制端口；pid 复用有
    //    secret 验证挡着，不会误伤）
    cleanup_stale_kernel().await;

    // ② provider 缓存里的节点名与节点定义（都可能为空 —— 首次拉取在第 ⑤ 步补；
    //    定义是内联进顶层 `proxies` 用的，见 build_config）
    let mut provider_nodes: Vec<(String, Vec<String>)> = Vec::new();
    for provider in providers {
        provider_nodes.push((provider.id.clone(), read_provider_nodes(&provider.id)));
    }
    let mut defs = read_provider_definitions(providers);

    // ③ 端口分配：状态文件里沿用，缺了才现分配
    let mut state = read_state();
    let secret = {
        let existing = state.get("secret").and_then(Value::as_str).unwrap_or("");
        if existing.is_empty() {
            let generated = random_secret();
            state_insert(&mut state, "secret", json!(generated));
            generated
        } else {
            existing.to_string()
        }
    };
    let controller_port = ensure_controller_port(&mut state, &secret).await?;
    // 统一测试口：与控制端口一样持久在状态文件里（重启不漂移，测试口是
    // 「测试」唯一的入口，漂移会让正在排障的人摸不着头脑）
    let test_port = ensure_test_port(&mut state, controller_port, &secret).await?;
    // 分配之前先按现存订阅裁掉注册表里的死键（已删 / 已禁用订阅的端口号），
    // 否则它们会被当成已占用、白白吃掉端口范围（见 prune_ports）
    prune_ports(&mut state, providers);
    let targets = allocate_listeners(
        providers,
        &state_ports(&state),
        &provider_nodes,
        &defs,
        controller_port,
        test_port,
    )?;
    write_ports(&mut state, &targets);
    state_insert(&mut state, "version", json!(KERNEL_VERSION));
    save_state(&state)?;

    // ④ 写配置 → 杀旧 → spawn → 等控制器
    restart_with_config(providers, controller_port, test_port, &secret, &targets, &defs).await?;

    // ⑤ 轮询 provider：节点集合与配置不一致时重建 + 重启一次
    let provider_ids: Vec<String> = providers.iter().map(|provider| provider.id.clone()).collect();
    let live = poll_providers(controller_port, &secret, &provider_ids, PROVIDER_WAIT_MS).await;
    let mut final_targets = targets;
    // 全量节点清单（provider 缓存优先，轮询到的新节点并入）—— 比较与镜像都按它
    let mut live_nodes: Vec<(String, Vec<String>)> = provider_ids
        .iter()
        .map(|id| (id.clone(), read_provider_nodes(id)))
        .collect();
    for (id, names) in live_nodes.iter_mut() {
        if let Some(latest) = live.get(id).and_then(Value::as_array) {
            let latest: Vec<String> = latest
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if !latest.is_empty() {
                *names = latest;
            }
        }
    }
    if nodes_changed(&final_targets, &live_nodes) {
        // 定义要**重新读**：首次拉取的缓存是这一轮才落地的（② 读时还没有），
        // 拿旧的那份会让新节点全都「没有定义」而分不到端口
        defs = read_provider_definitions(providers);
        let next_targets = allocate_listeners(
            providers,
            &state_ports(&state),
            &live_nodes,
            &defs,
            controller_port,
            test_port,
        )?;
        let mut state = read_state();
        write_ports(&mut state, &next_targets);
        save_state(&state)?;
        restart_with_config(
            providers,
            controller_port,
            test_port,
            &secret,
            &next_targets,
            &defs,
        )
        .await?;
        final_targets = next_targets;
    }

    // ⑥ 节点镜像进代理池（**全量**：未被引用的节点端口为 None，界面上照常
    //    可见可选）+ 记录各订阅的同步结果
    let port_of = |provider_id: &str, node: &str| -> Option<u16> {
        let key = node_key(provider_id, node);
        final_targets
            .iter()
            .find(|target| node_key(&target.provider_id, &target.node) == key)
            .and_then(|target| target.port)
    };
    let all_targets: Vec<NodeTarget> = live_nodes
        .iter()
        .flat_map(|(provider_id, nodes)| {
            let sub_name = providers
                .iter()
                .find(|provider| &provider.id == provider_id)
                .map(|provider| provider.name.clone())
                .unwrap_or_default();
            nodes.iter().map(move |node| NodeTarget {
                provider_id: provider_id.clone(),
                sub_name: sub_name.clone(),
                node: node.clone(),
                port: port_of(provider_id, node),
            })
        })
        .collect();
    sync_nodes_to_pool(&all_targets);
    for provider in providers {
        let count = all_targets
            .iter()
            .filter(|target| target.provider_id == provider.id)
            .count();
        record_provider_sync(&provider.id, &provider.name, count);
        if count == 0 {
            log_line(&format!(
                "⚠️ 订阅「{}」还没有节点（链接拉取失败或内容为空，可点「立即更新」重试）",
                provider.name
            ));
        }
    }
    Ok(())
}

/// 把节点的端口映射写回状态（重启后端口稳定，账号引用不漂移）。
///
/// **只写本次分配的端口，不在这里裁剪**：上次运行分配过、本次不需要（没有
/// 账号引用它）的端口留在注册表里 —— 用户取消引用又反悔（改回来）时端口
/// 原样恢复，且下次重启时它会被重新分配（`listener_node_keys` 里已经有它）。
/// 真正需要回收的是**订阅都不在了**的键：那部分由 `prune_ports` 在分配前按
/// 现存订阅裁掉（订阅重建会换 providerId，死键会一直占号）。
fn write_ports(state: &mut Value, targets: &[NodeTarget]) {
    let mut port_map = state_ports(state);
    for target in targets {
        port_map.insert(node_key(&target.provider_id, &target.node), json!(target.port));
    }
    state_insert(state, "ports", Value::Object(port_map));
}

/// 杀旧 + 写配置 + spawn + 等控制器就绪。
async fn restart_with_config(
    providers: &[ProviderInput],
    controller_port: u16,
    test_port: u16,
    secret: &str,
    targets: &[NodeTarget],
    defs: &HashMap<String, Value>,
) -> Result<(), String> {
    let dir = crate::server::core::sub_kernel::kernel_dir();
    let config =
        build_config(providers, controller_port, secret, test_port, targets, defs)?;
    std::fs::write(dir.join("config.yaml"), &config)
        .map_err(|error| format!("写入内核配置失败: {error}"))?;

    // 杀旧进程（重启语义；首次启动时是 no-op）
    kill_child();

    let binary = dir.join(crate::server::core::sub_kernel::kernel_binary_name());
    if !binary.exists() {
        return Err("代理内核未安装（先在「订阅管理」下载内核）".to_string());
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("kernel.log"))
        .map_err(|error| format!("打开内核日志失败: {error}"))?;
    let log_err = log_file
        .try_clone()
        .map_err(|error| format!("打开内核日志失败: {error}"))?;
    let mut command = tokio::process::Command::new(&binary);
    command
        .arg("-d")
        .arg(&dir)
        .arg("-f")
        .arg(dir.join("config.yaml"))
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .kill_on_drop(true);
    // ── Windows：绝不给内核分配控制台窗口 ────────────────────────
    // 桌面端是 GUI 子系统进程（没有控制台），启动控制台程序（mihomo）时系统会
    // **新开一个控制台窗口**，标题就是 `-d` 指的内核目录。那个窗口不是装饰：
    // 内核挂在它的控制台上，用户点 ✕ 关掉 = 控制台关闭事件把内核一起杀掉
    //（实测「一关闭黑框，内核就停了」）。内核的 stdout/stderr 早已写进
    // kernel.log，不需要控制台，所以直接不创建（CREATE_NO_WINDOW）。
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动内核进程失败: {error}"))?;

    // 等控制器就绪（内核启动 ~100ms，给 10s 的宽限）；起完就崩时把退出
    // 状态带出来
    let deadline = tokio::time::Instant::now() + Duration::from_millis(CONTROLLER_WAIT_MS);
    loop {
        if controller_get(controller_port, secret, "/version").await.is_some() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            return Err(format!(
                "内核控制器未就绪（端口 {controller_port}，详见 kernel.log）"
            ));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("内核进程退出（{status}，详见 kernel.log）"));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    // 持久 pid（下一次启动识别遗留进程用）+ 记录句柄（kill_on_drop 杀它）
    let pid = child.id();
    *lock_child() = Some(child);
    let mut state = read_state();
    if let Some(pid) = pid {
        state_insert(&mut state, "pid", json!(pid));
        save_state(&state)?;
    }

    record_running(Some(controller_port), targets.len(), None);
    log_line(&format!(
        "✅ 代理内核 {KERNEL_VERSION} 已启动（控制端口 {controller_port}，测试口 {test_port}，{} 个节点监听器）",
        targets.len()
    ));
    Ok(())
}

/// 停掉内核（worker 的 Stop 分支用；等待进程退出再返回）
pub async fn stop_kernel() {
    let Some(mut child) = lock_child().take() else {
        record_running(None, 0, None);
        return;
    };
    let _ = child.start_kill();
    let _ = child.wait().await;
    record_running(None, 0, None);
    log_line("🛑 已停止代理内核");
}

// ─── 刷新与遗留进程 ────────────────────────────────────────

/// 强制刷新一条订阅的内容：控制器 PUT（provider 强制拉取）→ 轮询节点 →
/// 集合变了才重建配置 + 重启（端口注册表保持稳定）。
pub async fn run_refresh(sub_id: &str, providers: &[ProviderInput]) -> Result<(), String> {
    let state = read_state();
    let Some(controller_port) = state.get("controllerPort").and_then(json_u16) else {
        return Err("内核未运行（先启动内核）".to_string());
    };
    let Some(secret) = state.get("secret").and_then(Value::as_str) else {
        return Err("内核状态缺少控制器密钥".to_string());
    };
    if !controller_alive_port(controller_port) {
        return Err("内核未运行（先启动内核）".to_string());
    }
    let provider = providers
        .iter()
        .find(|provider| provider.id == sub_id)
        .ok_or_else(|| "订阅不存在".to_string())?;
    controller_put(
        controller_port,
        secret,
        &format!("/providers/proxies/sub_{sub_id}"),
    )
    .await?;
    log_line(&format!("🔄 已强制刷新订阅「{}」", provider.name));

    // 拉取后的节点清单（给几秒的窗口让 provider 写缓存）
    let live = poll_providers(controller_port, secret, &[sub_id.to_string()], 8_000).await;
    let names: Vec<String> = live
        .get(sub_id)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let count = names.len();
    record_provider_sync(sub_id, &provider.name, count);
    if count == 0 {
        return Err(format!(
            "订阅「{}」刷新后仍然没有节点（检查链接或流量是否用尽）",
            provider.name
        ));
    }

    // 节点集合变了 → 重建配置 + 重启（新节点分新端口，消失的摘掉）
    let current: HashSet<String> = read_provider_nodes(sub_id).into_iter().collect();
    let next: HashSet<String> = names.iter().cloned().collect();
    if current != next {
        run_restart(providers).await?;
    }
    Ok(())
}

/// 启动前的遗留进程清理：状态文件里的 pid 还活着且确实是本内核
/// （控制器以我们的 secret 响应）才杀 —— pid 复用会误伤无关进程。
///
/// 桌面端正常退出走 backend::shutdown（杀内核），这里兜「上次崩溃 /
/// 被任务管理器强杀」的情况。
pub async fn cleanup_stale_kernel() {
    let state = read_state();
    let (Some(pid), Some(controller_port), Some(secret)) = (
        state.get("pid").and_then(Value::as_u64),
        state.get("controllerPort").and_then(json_u16),
        state.get("secret").and_then(Value::as_str),
    ) else {
        return;
    };
    if !controller_alive_port(controller_port) {
        return; // 内核已死，pid 记录过期，无需处理
    }
    // 控制器活着：确认是我们的（secret 对上）才按 pid 杀
    if controller_get(controller_port, secret, "/version").await.is_none() {
        return;
    }
    if kill_by_pid(pid) {
        logging::log(
            "[Subscription]",
            &format!("🧹 已清理上次运行遗留的内核进程（pid {pid}）"),
        );
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// 控制端口探活（本地 TCP connect，超时 300ms）
fn controller_alive_port(port: u16) -> bool {
    let Ok(address) = format!("127.0.0.1:{port}").parse::<SocketAddr>() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// 按 pid 杀进程（Windows taskkill / unix kill；进程已退出返回 false）
fn kill_by_pid(pid: u64) -> bool {
    #[cfg(windows)]
    {
        // taskkill 是控制台程序：不加 CREATE_NO_WINDOW 时桌面端会闪一个黑框
        // （这条路径在「清理上次运行遗留的内核进程」与端口回收时都会走）
        use std::os::windows::process::CommandExt;
        match std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        match std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }
}

// ─── 端口与监听器分配 ──────────────────────────────────────

/// 控制端口：状态文件里沿用；被别人占了（控制器不认我们的 secret）才换新口。
async fn ensure_controller_port(state: &mut Value, secret: &str) -> Result<u16, String> {
    if let Some(port) = state.get("controllerPort").and_then(json_u16) {
        if controller_alive_port(port) {
            // 有进程在听：是我们的（secret 对上）就沿用（上面已把旧进程杀掉，
            // 端口马上释放）；不是我们的（别人的服务占了）就往下走挑新口
            if controller_get(port, secret, "/version").await.is_some() {
                return Ok(port);
            }
        } else {
            return Ok(port); // 空闲，沿用
        }
    }
    let taken: HashSet<u16> = HashSet::new();
    let port = alloc_port(&taken, CONTROLLER_PORT_BASE, CONTROLLER_PORT_END)
        .ok_or_else(|| format!("没有可用的控制端口（{CONTROLLER_PORT_BASE}-{CONTROLLER_PORT_END} 全被占用）"))?;
    state_insert(state, "controllerPort", json!(port));
    Ok(port)
}

/// 统一测试口：状态文件里沿用；被**别人**占了才换新口。
///
/// 占用判据与控制端口那条不同，原因是测试口**不是控制器**——没法用 secret
/// 验明正身，只能间接推：
///   · 端口空闲 → 沿用（新内核会绑它）；
///   · 端口被占、但控制端口上有**我们的**内核在跑 → 占用者就是旧内核自己
///    （它马上要被重启收掉），沿用；
///   · 端口被占、内核又不在跑 → 是别人抢的，换新口。
/// 分配时避开控制端口与端口注册表里的节点端口（`taken`）——两类监听器
/// 都在同一个内核进程里，撞号就是启动失败。
async fn ensure_test_port(
    state: &mut Value,
    controller_port: u16,
    secret: &str,
) -> Result<u16, String> {
    let old_kernel_ours = controller_alive_port(controller_port)
        && controller_get(controller_port, secret, "/version").await.is_some();
    if let Some(port) = state.get("testPort").and_then(json_u16) {
        if port != controller_port && (port_free(port) || old_kernel_ours) {
            return Ok(port);
        }
    }
    let mut taken: HashSet<u16> = state_ports(state).values().filter_map(json_u16).collect();
    taken.insert(controller_port);
    let port = alloc_port(&taken, TEST_PORT_BASE, TEST_PORT_END).ok_or_else(|| {
        format!("没有可用的测试端口（{TEST_PORT_BASE}-{TEST_PORT_END} 全被占用）")
    })?;
    state_insert(state, "testPort", json!(port));
    Ok(port)
}

/// 给节点分配监听器端口 —— 只给**需要的**节点分配（lazy listeners）。
///
/// 订阅动辄几百个节点，而账号实际引用的通常只有几个：给全部节点开监听器
/// 会让配置体积、内核内存与启动时间都跟着节点数走（真正的性能大头其实是
/// mihomo 逐节点的健康检查与监听器结构）。因此这里只给「账号引用中的池
/// 条目」分配端口（`listener_node_keys`，禁用中的不算）；全量节点仍镜像进
/// 代理池（浏览 / 选择体验不变），条目在未分配端口时 `port` 为 null、解析报
/// 「还没有监听端口」（见 proxy_pool 的 resolve_item）。**测试不在这里**
/// —— 测试走统一测试口（见 `build_config` 与 `select_test_node`），不再为
/// 被测节点开专用端口。
///
/// 端口分配细节：
///   · 已注册的端口**沿用前先探活**——停机期间被别的进程占走的口不再沿用，
///     换新口并更新注册表（引用按节点不按端口，换口对账号无感）；
///   · 新分配从代理设置的范围（`core::proxy_settings::listener_port_range`）
///     里挑第一个空闲口；控制端口与测试口在 `reserved` 里先占住（两类监听器
///     同进程，撞号就是配置加载失败）；
///   · 分配不出（范围用尽）返回 Err —— 此时内核带着其余监听器照常跑，
///     失败文案里点名设置入口。
///
/// `sub_name` 从 providers 里按 id 查（provider_nodes 只有 id；节点条目命名
/// 要用订阅名）。
fn allocate_listeners(
    providers: &[ProviderInput],
    ports: &Map<String, Value>,
    provider_nodes: &[(String, Vec<String>)],
    defs: &HashMap<String, Value>,
    controller_port: u16,
    test_port: u16,
) -> Result<Vec<NodeTarget>, String> {
    let (listener_base, listener_end) =
        crate::server::core::proxy_settings::listener_port_range();
    let wanted = listener_node_keys();
    let mut taken: HashSet<u16> = HashSet::new();
    for port in ports.values().filter_map(json_u16) {
        taken.insert(port);
    }
    taken.insert(controller_port);
    taken.insert(test_port);
    let mut targets = Vec::new();
    for (provider_id, nodes) in provider_nodes {
        let sub_name = providers
            .iter()
            .find(|provider| &provider.id == provider_id)
            .map(|provider| provider.name.clone())
            .unwrap_or_default();
        for node in nodes {
            // 不需要的节点不开监听器（lazy listeners，见函数头）
            let key = node_key(provider_id, node);
            if !wanted.contains(&key) {
                continue;
            }
            // 拿不到节点定义就开不出能用的监听器（见 build_config 的「内联」一节）：
            // 不给它端口，池里停在「未分配端口」，下一次重启（账号引用变化触发的
            // 补建）等缓存落地后再补 —— 给一个连不上的地址比留空更坏
            if !defs.contains_key(&key) {
                continue;
            }
            let port = match ports.get(&key).and_then(json_u16) {
                // 注册表里有：沿用前探活，被占（停机期间被别的进程抢走）就当没有
                Some(port) if port_free(port) => port,
                Some(_) | None => {
                    let port = alloc_port(&taken, listener_base, listener_end)
                        .ok_or_else(|| {
                            format!("没有可用的节点端口（{listener_base}-{listener_end} 用尽）—— 节点太多时可在「网络代理」页的设置里扩大端口范围")
                        })?;
                    port
                }
            };
            taken.insert(port);
            targets.push(NodeTarget {
                provider_id: provider_id.clone(),
                sub_name: sub_name.clone(),
                node: node.clone(),
                port: Some(port),
            });
        }
    }
    Ok(targets)
}

/// 节点集合是否与当前监听器不一致（provider 轮询后判断要不要重建配置）。
///
/// 比较的是**全量节点清单**而不是已分配端口的子集：机场加了新节点时，哪怕
/// 没人引用它，也要让代理池的镜像跟上（sync 在重启后做，但重建配置的触发
/// 在这里）。
fn nodes_changed(targets: &[NodeTarget], live_nodes: &[(String, Vec<String>)]) -> bool {
    for (provider_id, names) in live_nodes {
        // 配置里的节点（targets 只含被引用的）必须在最新清单里
        for target in targets.iter().filter(|target| &target.provider_id == provider_id) {
            if !names.contains(&target.node) {
                return true; // 引用中的节点在最新清单里消失了
            }
        }
        // 清单里出现了配置里没有的节点（新节点 → 池镜像要更新）
        let known: HashSet<&str> = targets
            .iter()
            .filter(|target| &target.provider_id == provider_id)
            .map(|target| target.node.as_str())
            .collect();
        for name in names {
            if !known.contains(name.as_str()) {
                return true;
            }
        }
    }
    // 上次有监听器、这次订阅整个消失（live_nodes 没有这家）—— 也要重建
    for target in targets {
        if !live_nodes.iter().any(|(provider_id, _)| &target.provider_id == provider_id) {
            return true;
        }
    }
    false
}

/// 控制器密钥（随机 32 字节 → hex；getrandom 与 Qoder PKCE 同一依赖）
fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    let _ = getrandom::getrandom(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ─── 统一测试口（池测试的订阅分支；配置生成见 build_config）──

/// 状态文件里登记的统一测试口（None = 内核从没跑过 / 旧版状态文件）
pub fn test_port() -> Option<u16> {
    read_state().get("testPort").and_then(json_u16)
}

/// 把统一测试口的策略组切到目标节点，返回测试口端口号。
///
/// ── 为什么测试不走节点专用端口 ─────────────────────────────
/// 参照 Clash Verge 的形态：内核配置里有一个固定的「测试口」监听器，出口指向
/// 一个 select 策略组（成员 = 全部节点）。测试 = 控制器把组切到目标节点 →
/// 从测试口发请求。这样「测一条节点」不再需要为它开专用端口 + 等内核重建
/// —— 过去的 pinned + provision + 45 秒等待那一整套就是为这件事存在的，
/// 统一测试口上线后整套退役；专用端口只服务真正在用的节点（账号引用，
/// lazy listeners）。切换不持久（配置里 `store-selected: false`），下次测试
/// 会重新切。
///
/// ── 就绪判定与重试 ─────────────────────────────────────────
/// 「就绪」= 内核在跑 + 状态里有测试口 + 测试组存在**且包含目标节点**。
/// 不满足时投一次重启命令（幂等：覆盖「内核停着」「配置还是旧版（升级后
/// 首测）」「订阅缓存刚落地还没重建」三种情况）并轮询等待，上限
/// [`TEST_READY_WAIT_MS`]（盖过内核启动 + provider 首拉的慢路径）。超时报错
/// 不悬着 —— 真正的原因在内核状态与 kernel.log 里。
///
/// **手动关闭闩**优先于就绪判定：用户刚点过「关闭内核」时直接拒测（提示
/// 先点「启动内核」），不投重启 —— 否则「内核停着」会被当成第一种可修复
/// 情况，测试就把内核悄悄拉起来了。
///
/// 节点根本不在订阅缓存里（已被机场移除）时**不等**，当场给出与池解析
/// （`proxy_pool::resolve_item`）同一口径的话。
pub async fn select_test_node(provider_id: &str, node: &str) -> Result<u16, String> {
    if !crate::server::core::sub_kernel::installed() {
        return Err("代理内核未安装（先在「订阅管理」下载内核）".to_string());
    }
    if crate::server::core::sub_kernel::is_manually_halted() {
        return Err("内核已手动关闭 —— 点内核状态条的「启动内核」后再测试".to_string());
    }
    // 缓存里没有这个节点 = 定义进不了配置、组里也不可能有它 —— 等待是白等
    if !read_provider_nodes(provider_id).iter().any(|name| name == node) {
        return Err(
            "该节点已不在订阅里（订阅更新后节点被移除，等下一次同步或重新订阅）".to_string(),
        );
    }
    let alias = node_alias(provider_id, node);
    let group_path = format!("/proxies/{TEST_GROUP_NAME}");
    let mut requested = false;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(TEST_READY_WAIT_MS);
    loop {
        let state = read_state();
        let controller_port = state.get("controllerPort").and_then(json_u16);
        let secret = state
            .get("secret")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let test_port = state.get("testPort").and_then(json_u16);
        if let (Some(controller_port), Some(test_port)) = (controller_port, test_port) {
            if !secret.is_empty() && controller_alive_port(controller_port) {
                // 组成员校验放在切组之前：404 没有区分度，这里能说出是
                // 「节点还没进内核（等重建）」还是「组整个不存在」
                let member = controller_get(controller_port, &secret, &group_path)
                    .await
                    .and_then(|group| group.get("all").and_then(Value::as_array).cloned())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|name| name == alias)
                    })
                    .unwrap_or(false);
                if member {
                    controller_put_json(
                        controller_port,
                        &secret,
                        &group_path,
                        &json!({ "name": alias }),
                    )
                    .await?;
                    return Ok(test_port);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "测试口没有就绪（内核可能停着或卡在拉取订阅）；稍后重试，或看「订阅管理」的内核状态与日志"
                    .to_string(),
            );
        }
        if !requested {
            crate::server::core::sub_kernel::request_restart(
                crate::server::core::subscriptions::enabled_provider_inputs(),
            );
            requested = true;
            log_line("🔄 测试口未就绪，已请求内核重建");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
