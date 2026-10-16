//! 订阅路由（「网络代理」页的「订阅管理」）。
//!
//!   GET  /api/subscriptions              订阅列表 + 内核状态
//!   POST /api/subscriptions/add          新增订阅 {name, url} 或本地文件
//!                                        {name, fileName, contentBase64}
//!   POST /api/subscriptions/save         编辑订阅 {id, name, url, enabled}；
//!                                        本地订阅可带 contentBase64 换文件
//!   POST /api/subscriptions/refresh      强制刷新一条订阅的内容 {id}
//!   POST /api/subscriptions/remove       删除订阅 {id}
//!   POST /api/subscriptions/decode-qr    从图片解二维码 {imageBase64} → {url}
//!   GET  /api/subscriptions/kernel       内核状态（下载进度 / 运行状态）
//!   POST /api/subscriptions/kernel/download  开始下载内核（{proxy, dir} 可选，
//!                                        后台任务，轮询看进度）
//!   POST /api/subscriptions/kernel/download/cancel  停止下载（删掉半截文件）
//!   POST /api/subscriptions/kernel/restart   启动 / 重启内核（重建配置）
//!   POST /api/subscriptions/kernel/remove    删除已下载的内核（先停进程再删文件）
//!
//! ── 分发方式 ────────────────────────────────────────────────
//! 与 proxies 同构：一条 `/api/subscriptions` 固定路径入口 + 按 (方法, 剩余
//! 段) 分发（http.rs 的路由按固定路径注册，通配段会与「已注册路径上的其它
//! 方法 → 404 信封」那条兜底互相干扰，见 http.rs 的注册注释）。
//!
//! ── 编排（core 不认识内核，api 层负责串）────────────────────
//! 写操作（add / save / remove）落库后投一条内核命令（重启重建配置 /
//! 删光订阅时停内核），**命令是异步的**：handler 立即返回最新列表 + 内核
//! 快照，节点数 / 运行状态由前端轮询（内核重启 + provider 拉取要几秒到
//! 几十秒，同步等会把本地请求挂死）。refresh 同理（provider 强制拉取的
//! 时长取决于订阅站）。
//!
//! ── decode-qr 为什么在服务端解 ──────────────────────────────
//! 二维码解码是纯逻辑（rqrr + 已有的 image 依赖），放服务端让 web 前端
//! 不必内嵌一份 JS 解码库；图片以 base64 随请求上传（本机回环，32MB 的
//! 管理面请求上限对二维码截图绰绰有余）。识别内容只接受订阅链接
//! （http/https）；单节点分享链接（vmess:// 等）明确报「暂不支持」，
//! 不静默吞掉。

use axum::body::Bytes;
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::sub_kernel;
use crate::server::core::subscriptions;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;

/// 列表响应：`{items, kernel}`。items 的节点数从内核快照注入（订阅存储
/// 不认识内核，见 `core::subscriptions::list` 的说明）。
fn build_payload() -> Value {
    let kernel = sub_kernel::status_json();
    let node_counts = kernel
        .get("providerSync")
        .and_then(Value::as_object)
        .map(|providers| {
            providers
                .iter()
                .filter_map(|(id, value)| {
                    value.get("count").and_then(Value::as_u64).map(|count| (id.clone(), count as usize))
                })
                .collect::<std::collections::HashMap<String, usize>>()
        })
        .unwrap_or_default();
    json!({
        "items": subscriptions::list(&node_counts),
        "kernel": kernel,
    })
}

/// 写操作 / 刷新后的统一响应（就地替换约定，与代理池写接口一致）
fn saved_payload() -> Response {
    ok_json(build_payload())
}

/// api 层编排骨架：订阅写操作之后内核该做什么。
///
/// 有启用的订阅 → 重启内核（重建 proxy-providers 与监听器，节点镜像进池）；
/// 一条启用订阅都不剩 → 停内核（留着只会空跑健康检查）。内核未安装时投
/// 命令也无害（restart 早退在「未安装」上，worker 串行消费）。
fn sync_kernel_after_change() {
    let enabled = subscriptions::enabled_provider_inputs();
    if enabled.is_empty() {
        sub_kernel::request_stop();
    } else if sub_kernel::is_manually_halted() {
        // 手动关闭闩：订阅变更先落库，但不悄悄把内核拉起来；变更在下次
        // 「启动内核」重建配置时生效（订阅、节点、监听器都在重建范围里）
        logging::log(
            "[Subscription]",
            "内核已手动关闭 —— 订阅变更已保存，「启动内核」后生效",
        );
    } else {
        sub_kernel::request_restart(enabled);
    }
}

// ─── 入口分发 ───────────────────────────────────────────────

/// 订阅操作全部走进程级句柄（subscriptions 的库句柄 / sub_kernel 的 worker
/// 命令队列），不经过 ServerState —— 与 `core::subscriptions` 的 install
/// 形态一致，entry 也就不需要 State 提取器。
pub async fn entry(request: axum::extract::Request) -> Response {
    let method = request.method().clone();
    let full_path = request.uri().path().to_string();
    let rest = full_path
        .strip_prefix("/api/subscriptions")
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string();
    let body = match axum::body::to_bytes(request.into_body(), crate::server::http::MAX_BODY_SIZE)
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => return management_error(413, format!("请求体读取失败或过大: {error}")),
    };
    match (method.as_str(), rest.as_str()) {
        ("GET", "") => ok_json(build_payload()),
        ("POST", "add") => add(&body).await,
        ("POST", "save") => save(&body).await,
        ("POST", "refresh") => refresh(&body),
        ("POST", "remove") => remove(&body).await,
        ("POST", "decode-qr") => decode_qr(&body),
        ("GET", "kernel") => ok_json(sub_kernel::status_json()),
        ("POST", "kernel/download") => kernel_download(&body).await,
        ("POST", "kernel/download/cancel") => kernel_download_cancel(),
        ("POST", "kernel/restart") => kernel_restart(),
        ("POST", "kernel/stop") => kernel_stop(),
        ("POST", "kernel/remove") => kernel_remove(),
        _ => management_error(
            404,
            format!("Not found: {} {full_path}", method.as_str()),
        ),
    }
}

// ─── 订阅 CRUD ─────────────────────────────────────────────

/// 把请求体里的本地文件内容（`contentBase64`，兼容 data URL 前缀）解出来，
/// 就地改写成存储形态（`kind: "local"` + 明文 `content`）；没带就原样放行。
///
/// base64 是**传输形态**（YAML 原文走 JSON 转义既臃肿又容易在壳层被改写坏，
/// 与二维码图片同一套传法），解完即剥掉 `contentBase64` —— 存储里只留明文
/// content。YAML 形状校验在 `core::subscriptions::normalize_item`（存储层拥有
/// 「什么内容能入库」的判断权，api 层只负责把传输形态翻译过来）。
fn attach_local_content(payload: &mut Value) -> Result<(), String> {
    let Some(encoded) = payload
        .get("contentBase64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Ok(());
    };
    let encoded = encoded
        .split_once("base64,")
        .map(|(_, rest)| rest)
        .unwrap_or(encoded);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "文件内容不是有效的 base64".to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "YAML 文件不是 UTF-8 编码".to_string())?;
    let Some(object) = payload.as_object_mut() else {
        return Err("订阅条目必须是对象".to_string());
    };
    object.insert("kind".to_string(), Value::String("local".to_string()));
    object.insert("content".to_string(), Value::String(text));
    object.remove("contentBase64");
    Ok(())
}

/// POST /api/subscriptions/add
async fn add(body: &Bytes) -> Response {
    let mut payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    if let Err(message) = attach_local_content(&mut payload) {
        return management_error(400, message);
    }
    match subscriptions::create(&payload) {
        Ok(_) => {
            sync_kernel_after_change();
            saved_payload()
        }
        Err(message) => management_error(400, message),
    }
}

/// POST /api/subscriptions/save
async fn save(body: &Bytes) -> Response {
    let mut payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    if let Err(message) = attach_local_content(&mut payload) {
        return management_error(400, message);
    }
    match subscriptions::update(&payload) {
        Ok(_) => {
            sync_kernel_after_change();
            saved_payload()
        }
        Err(message) => management_error(400, message),
    }
}

/// POST /api/subscriptions/refresh（强制拉取订阅内容；节点变了自动重启内核）
fn refresh(body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return management_error(400, "缺少订阅 id");
    }
    // 「不存在」与「被禁用」分流报错（见 subscriptions::find_enabled_state 的说明）
    match subscriptions::find_enabled_state(id) {
        None => {
            return management_error(
                404,
                format!("订阅「{id}」不存在（可能已被删除，刷新列表后重试）"),
            )
        }
        Some(false) => return management_error(400, "该订阅已禁用（先启用再刷新）"),
        Some(true) => {}
    }
    // 本地文件订阅没有链接可拉：内容在导入时就已写进内核缓存位（每次重启还会
    // 从存储重写一遍），「强制拉取」对它没有意义 —— 当场说明，不假装刷新成功
    if subscriptions::kind_of(id) == Some("local") {
        return management_error(
            400,
            "本地文件订阅无需刷新：要更新内容请编辑该订阅、重新选择 YAML 文件",
        );
    }
    sub_kernel::request_refresh(id.to_string(), subscriptions::enabled_provider_inputs());
    saved_payload()
}

/// POST /api/subscriptions/remove
async fn remove(body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return management_error(400, "缺少订阅 id");
    }
    match subscriptions::remove(id) {
        Ok((_, name)) => {
            sync_kernel_after_change();
            let mut data = build_payload();
            if let Some(object) = data.as_object_mut() {
                object.insert("removed".to_string(), Value::String(name));
            }
            ok_json(data)
        }
        Err(message) => management_error(400, message),
    }
}

// ─── 二维码解码 ─────────────────────────────────────────────

/// 单节点分享链接的 scheme（识别到这些 → 明确报「暂不支持」而不是笼统的
/// 「不是订阅链接」，把排查方向点明）
const NODE_SCHEMES: &[&str] = &[
    "ss://", "ssr://", "vmess://", "vless://", "trojan://", "hysteria://", "hysteria2://",
    "hy2://", "tuic://", "socks://", "socks5://",
];

/// POST /api/subscriptions/decode-qr —— 从图片解二维码。
///
/// `imageBase64` 接受裸 base64 或 data URL（`data:image/png;base64,...`）。
/// 图里可能不止一个码（长截图），逐个试，优先取 http/https 的内容。
fn decode_qr(body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let raw = payload
        .get("imageBase64")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return management_error(400, "缺少图片数据");
    }
    // data URL 前缀剥掉（`base64,` 之后才是正文）
    let encoded = raw.split_once("base64,").map(|(_, rest)| rest).unwrap_or(raw);
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(_) => return management_error(400, "图片数据不是有效的 base64"),
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(_) => return management_error(400, "图片格式无法识别（支持 PNG / JPEG / WebP）"),
    };
    let mut prepared = rqrr::PreparedImage::prepare(img.to_luma8());
    let grids = prepared.detect_grids();
    if grids.is_empty() {
        return management_error(400, "图片里没有识别到二维码");
    }
    let mut first_content: Option<String> = None;
    for grid in &grids {
        let Ok((_, content)) = grid.decode() else {
            continue;
        };
        let lower = content.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return ok_json(json!({ "url": content }));
        }
        if NODE_SCHEMES.iter().any(|scheme| lower.starts_with(scheme)) {
            let scheme = content.split("://").next().unwrap_or("").to_string();
            return management_error(
                400,
                format!("二维码里是单节点分享链接（{scheme}://），暂不支持，请使用订阅链接"),
            );
        }
        if first_content.is_none() {
            first_content = Some(content);
        }
    }
    match first_content {
        Some(content) => management_error(400, format!("二维码内容不是订阅链接：{}", content)),
        None => management_error(400, "二维码无法解码（可能是图片太模糊或码不完整）"),
    }
}

// ─── 内核操作 ───────────────────────────────────────────────

/// POST /api/subscriptions/kernel/download（后台下载，进度轮询看）
///
/// 请求体（两个字段都可省）：`{proxy: null | {source,…}, dir: "D:\\kernel" | ""}`
/// —— 对应下载弹窗里的两个选择。**先落配置再投命令**：worker 执行时读配置决定
/// 走哪条线路、装到哪儿（见 `sub_kernel::request_download` 的说明），所以这两个
/// 值同时是「本次下载的参数」与「下次的默认值」—— 弹窗下次打开显示的就是它们，
/// 不必另设一个「保存」按钮（也就不会出现「改了没点保存」的中间态）。
///
/// `dir` 空串 = 恢复默认目录；非绝对路径当场 400 —— 宁可在这里说清楚，也不要
/// 等下载时报一句「创建内核目录失败」。
async fn kernel_download(body: &Bytes) -> Response {
    if sub_kernel::installed() {
        return ok_json(sub_kernel::status_json());
    }
    // 下载中重复投命令只会让 worker 排队（它此刻正忙着下载），用户看到的是
    // 「点了没反应」；当场说清楚
    if downloading() {
        return management_error(400, "内核正在下载中，先停止或等它下完");
    }
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    // 出口：与账号代理 / 更新出口同一套归一（null / 空 = 默认链）
    let proxy = match crate::server::core::proxies::normalize_account_proxy(
        payload.get("proxy").unwrap_or(&Value::Null),
    ) {
        Ok(proxy) => proxy,
        Err(error) => return management_error(400, &error.message),
    };
    // 轮询档要在这里挡下：它是**账号级**出网配置（逐请求换出口，见
    // `core::proxy_rotate`），订阅下载是一条固定的抓取动作，没有这个语义；
    // 放行只会存进一条解析不出地址的配置（下载静默走直连）。
    if proxy
        .as_ref()
        .is_some_and(crate::server::core::proxies::is_rotate_config)
    {
        return management_error(400, "订阅下载出口不支持「轮询」档（轮询用于 OpenCode Zen 账号的出网）");
    }
    // 目录：**缺字段 = 不动已存的设置**（老客户端与别的入口不带它）；
    // 有值就必须是绝对路径（空串 = 恢复默认）
    let dir = match payload.get("dir") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Some(None)
            } else if std::path::Path::new(trimmed).is_absolute() {
                Some(Some(trimmed.to_string()))
            } else {
                return management_error(400, "存放目录必须是绝对路径（例如 D:\\proxy-kernel）");
            }
        }
        Some(_) => return management_error(400, "存放目录必须是字符串"),
    };
    // 两个键各自落盘，任一失败都算「没完全落盘」（进程内已生效，重启会回到旧值）
    let proxy_saved = if payload.get("proxy").is_some() {
        crate::server::config::set_kernel_proxy(proxy)
    } else {
        true
    };
    let dir_saved = match dir {
        Some(value) => crate::server::config::set_kernel_dir(value),
        None => true,
    };
    sub_kernel::request_download(subscriptions::enabled_provider_inputs());
    logging::log(
        "[Subscription]",
        &format!(
            "⬇️ 开始下载代理内核（存放目录 {}）",
            sub_kernel::kernel_dir().to_string_lossy()
        ),
    );
    let mut status = sub_kernel::status_json();
    if let Some(object) = status.as_object_mut() {
        object.insert("saved".to_string(), Value::Bool(proxy_saved && dir_saved));
    }
    ok_json(status)
}

/// POST /api/subscriptions/kernel/download/cancel（停止下载）。
///
/// 只有**正在下载**时才受理：`{canceled: true}` 表示停止请求已置位（真正的
/// 落地由前端轮询 `download.state` 收敛 —— 下载循环在下个分片就响应，毫秒级）；
/// 不在下载中时 `{canceled: false}`，不假装停成功了（与 remove 的「幂等但如实
/// 报」同一取向）。
fn kernel_download_cancel() -> Response {
    let canceled = sub_kernel::request_cancel_download();
    if canceled {
        logging::log("[Subscription]", "⏹️ 请求停止下载代理内核");
    }
    let mut status = sub_kernel::status_json();
    if let Some(object) = status.as_object_mut() {
        object.insert("canceled".to_string(), Value::Bool(canceled));
    }
    ok_json(status)
}

/// 快照里的下载状态是不是 downloading（下载与删除两个入口共用同一个判据）
fn downloading() -> bool {
    sub_kernel::status_json()
        .get("download")
        .and_then(|download| download.get("state"))
        .and_then(Value::as_str)
        == Some("downloading")
}

/// POST /api/subscriptions/kernel/restart（手动启动 / 重启；先落日志便于排障）
fn kernel_restart() -> Response {
    let enabled = subscriptions::enabled_provider_inputs();
    if enabled.is_empty() {
        return management_error(400, "还没有启用的订阅（先添加一条订阅再启动内核）");
    }
    sub_kernel::request_restart(enabled);
    ok_json(sub_kernel::status_json())
}

/// POST /api/subscriptions/kernel/stop（手动关闭内核；内核状态条的「关闭内核」）。
///
/// 与「一条启用订阅都不剩」那条自动停（`request_stop`，会清空订阅节点镜像）
/// 的区别：这里**只停进程** —— 订阅还在、账号引用还在，池条目与端口注册表
/// 原样保留，「启动内核」后原位恢复（端口沿用注册表，账号引用不漂移）。
///
/// 幂等：没在跑（或没装）直接返回现状 —— 这个动作的目标状态是「内核不在运行」，
/// 它已经成立。响应是**投命令前**的快照（命令异步执行）：`running` 由前端轮询
/// 收敛（与删除 / 重启同一套）。
fn kernel_stop() -> Response {
    let status = sub_kernel::status_json();
    if status.get("running") != Some(&Value::Bool(true)) {
        return ok_json(status);
    }
    sub_kernel::request_halt();
    logging::log("[Subscription]", "🛑 关闭代理内核（手动；订阅与端口注册表保留）");
    ok_json(status)
}

/// POST /api/subscriptions/kernel/remove（删除已下载的内核）。
///
/// 幂等：没装（或已经删掉）直接返回现状，不报错 —— 这个动作的目标状态是
/// 「内核不在磁盘上」，它已经成立。下载中直接拒绝：命令队列是串行的，此时投
/// 删除会排在下载后面，用户看到的是「点了没反应」，不如当场说清楚。
///
/// 响应是**投命令前**的快照（命令异步执行）：真正的落地状态由前端轮询
/// `installed` 收敛（与下载 / 重启同一套）。
fn kernel_remove() -> Response {
    let status = sub_kernel::status_json();
    let downloading = status
        .get("download")
        .and_then(|download| download.get("state"))
        .and_then(Value::as_str)
        == Some("downloading");
    if downloading {
        return management_error(400, "内核正在下载中，等下载结束后再删除");
    }
    if !sub_kernel::installed() {
        return ok_json(status);
    }
    sub_kernel::request_remove();
    logging::log("[Subscription]", "🗑️ 删除代理内核（停进程 + 清内核目录）");
    ok_json(status)
}
