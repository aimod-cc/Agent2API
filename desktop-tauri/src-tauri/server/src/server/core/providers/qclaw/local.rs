//! QClaw 本机网关发现：`~/.qclaw/openclaw.json` + `~/.qclaw/qclaw.json`，
//! 以及（尽力而为地）解密 Electron localStorage 里的云端登录态。
//!
//! ── 这一家与别家的根本不同：**凭证在本机，不在云端** ──────────────
//! 别家的账号凭证是一枚能打上游域名的令牌（JWT / API Key / Cookie），
//! 而 QClaw 的推理上游是**客户端自己拉起来的本机网关**（OpenClaw）：
//! ```text
//!   ~/.qclaw/openclaw.json   gateway.port = 9434
//!                            gateway.auth.token = <随机 hex>
//!   ~/.qclaw/qclaw.json      authGatewayBaseUrl = http://127.0.0.1:19000/proxy
//! ```
//! 聊天打 `127.0.0.1:9434/v1/chat/completions`（Bearer 那个 token），
//! 模型清单打 `127.0.0.1:19000/proxy/llm/models`。两个都是**回环地址**，
//! 因此本模块所有出网一律走 `egress::client_for(None)`（直连）——
//! 走系统代理会把 127.0.0.1 的请求发给代理服务器，那是必败的。
//!
//! ── 为什么要「现场读文件」而不是只用导入时存下的令牌 ────────────
//! `gateway.auth.token` 由 QClaw 客户端**每次启动时生成**（实测重启后
//! 会变）。账号记录里存的那份是导入那一刻的快照，QClaw 一重启就可能失效。
//! 因此 [`read_gateway`] 在每次构造请求前都重新读盘 —— 它只是个几 KB 的
//! 本地 JSON，代价相对一次 LLM 请求可以忽略，换来的是「QClaw 重启后
//! 无需重新导入账号」。
//!
//! ── 登录态解密（CloudSession）有两个消费者 ─────────────────────
//! ① **账号身份**：昵称与 uid（导入时写进记录）。解不开丝毫不影响转发，
//!    所以这一路失败只记日志、返回空身份（`read_identity` 的历史行为）；
//! ② **云端额度查询**（`points` 模块）：jprx 接口要 JWT + 渠道令牌 + guid，
//!    这一路解不开 = 余额/订阅查不了，调用方必须如实报错。
//! 两个消费者共用同一次解密（`read_cloud_session`），代价只有一次。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本模块零 unwrap/expect/panic，全部返回 `Result` 或可空值。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::server::errors::GatewayError;
use crate::server::logging;

/// 千问办公那套 Chromium 解密原语（DPAPI + AES-256-GCM + base64）。
///
/// 复用而不是新写一份：QClaw 与千问办公的登录态都是 Electron `os_crypt`
/// 的同一套布局（`v10` + nonce + cipher||tag），原语与「哪家客户端」无关
/// （见 `qwenwork::crypt` 的模块头）。
use super::super::qwenwork::crypt;

/// `openclaw.json` 没写 `gateway.port` 时的默认值（QClaw 0.3.x 实测都是它）。
pub const DEFAULT_CHAT_PORT: u16 = 9434;

/// `qclaw.json` 没写 `authGatewayBaseUrl` 时的默认模型清单端口。
pub const DEFAULT_MODELS_PORT: u16 = 19000;

/// 聊天端点上 body 的 `model` 只能是 agent 名（上游源码：
/// `Invalid \`model\`. Use \`openclaw\` or \`openclaw/<agentId>\`.`），
/// 真正的模型选择走 `x-openclaw-model` 头（见 [`MODEL_HEADER`]）。
pub const AGENT_MODEL: &str = "openclaw";

/// 指定具体模型的请求头名（OpenClaw `chatCompletions` 端点的扩展）。
pub const MODEL_HEADER: &str = "x-openclaw-model";

/// 模型头取值的前缀（`openclaw.json` 的 `agents.defaults.model.primary`
/// 就是 `qclaw/pool-glm-5.2-night` 这个形态）。
pub const MODEL_PREFIX: &str = "qclaw/";

/// 本机网关的两处端口 + 令牌。
#[derive(Clone, Debug)]
pub struct LocalGateway {
    /// 聊天网关端口（`gateway.port`）
    pub chat_port: u16,
    /// 网关令牌（`gateway.auth.token`）
    pub token: String,
    /// 模型清单端口（`authGatewayBaseUrl` 的端口）
    pub models_port: u16,
}

/// 从本机登录态解出来的云端身份（只为备注名与 uid 服务，见模块头）。
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// 云端用户 id（JWT 的 `sub`）
    pub user_id: String,
    /// 昵称
    pub nickname: String,
}

/// 打 QClaw **云端**接口（jprx 网关：积分 / 订阅 / 流水）要的全套凭证。
///
/// ── 与 [`LocalGateway`] 的分工 ─────────────────────────────────
/// 推理走本机网关（回环 + 网关令牌），额度/订阅走云端（jprx + 云端 JWT）。
/// 两套凭证互不通用：网关令牌打不了 jprx，JWT 也打不了本机网关。
///
/// ── 为什么要「现场读」而不是存进账号记录 ──────────────────────
/// 云端 JWT 同样会变（用户在客户端里重新登录 / 令牌轮换），而 leveldb 解密
/// 在本机是纯本地操作（DPAPI + 本地文件），代价可以忽略。存进记录只会多一份
/// 会过期的副本 —— 与网关令牌「记录里存快照兜底」不同，这里**不落记录**，
/// 每次用每次解（解不开时调用方按失败处理，见 `points` 模块头）。
#[derive(Clone, Debug, Default)]
pub struct CloudSession {
    /// 云端 JWT（`X-OpenClaw-Token` 头；同时是有效期的来源）
    pub jwt: String,
    /// 渠道令牌（`X-Token` 头；leveldb 的 `login` 键）
    pub login_key: String,
    /// 机器 GUID（`X-Guid` 头；`qclaw.json` 的 `sharedParams.guid`）
    pub guid: String,
    /// 云端用户 id（`X-Account` 头）
    pub user_id: String,
    /// 昵称
    pub nickname: String,
}

// ── 测试期的数据目录覆盖 ──────────────────────────────────────
//
// `HOME_OVERRIDE` 让测试把 `~/.qclaw` 指到临时目录。
//
// ── 为什么需要这层接缝（2026-10-20 实测踩到）──────────────────
// `read_gateway()` 优先读**本机真实文件** `~/.qclaw/openclaw.json`，只有
// 读不到才退回账号快照。于是「装了 QClaw 的机器」与 CI 会走**两条不同的
// 分支**：CI 上文件不存在 → 走快照（端口 9434）；开发机上文件存在 → 走真
// 文件，而端口由 QClaw **每次启动随机分配**（实测踩到 10959）。
// 结果是任何断言「端口等于快照里那个值」的测试**只在开发机上失败** ——
// 它把本机状态伪装成了代码缺陷，比没有测试更糟。
//
// 有了它，测试可以确定性地走任一分支，结果与本机装没装 QClaw 无关。
//
// 用 `thread_local!` 而不是 `std::env::set_var`：后者改的是**进程全局**
// 状态，与并行跑的其它测试互相干扰（且 Rust 2024 起是 unsafe）。
// 配套的取值/注入接口见下面的 `with_home_override` 与 `qclaw_home`。
// 注：这里用普通注释而不是 `///` —— `thread_local!` 是宏展开，rustdoc
// 不为它生成文档，写 `///` 会报 `unused doc comment`。
#[cfg(test)]
thread_local! {
    static HOME_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
}

/// 在测试作用域内把 [`qclaw_home`] 指到 `home`，离开作用域自动还原。
#[cfg(test)]
pub fn with_home_override<T>(home: PathBuf, run: impl FnOnce() -> T) -> T {
    HOME_OVERRIDE.with(|cell| {
        let previous = cell.replace(Some(home));
        let output = run();
        cell.replace(previous);
        output
    })
}

/// `~/.qclaw`（QClaw 的数据目录；Windows 上是 `%USERPROFILE%\.qclaw`）。
pub fn qclaw_home() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(overridden) = HOME_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return Some(overridden);
    }
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join(".qclaw"))
}

/// 读本机网关配置。**每次调用都重新读盘**（令牌会随 QClaw 重启而变，
/// 见模块头）。
pub fn read_gateway() -> Result<LocalGateway, GatewayError> {
    let Some(home) = qclaw_home() else {
        return Err(GatewayError::with_status(
            400,
            "当前系统不支持读取 QClaw 本机网关配置（仅 Windows）",
        ));
    };
    let config_path = home.join("openclaw.json");
    if !config_path.is_file() {
        return Err(GatewayError::with_status(
            400,
            "未找到 QClaw 网关配置（~/.qclaw/openclaw.json），请先安装并登录 QClaw 客户端",
        ));
    }
    let text = std::fs::read_to_string(&config_path).map_err(|err| {
        GatewayError::with_status(500, format!("读取 QClaw 网关配置失败：{}", err.kind()))
    })?;
    let config: Value = serde_json::from_str(&text).map_err(|_| {
        GatewayError::with_status(400, "QClaw 网关配置（openclaw.json）不是合法 JSON")
    })?;
    let gateway = config.get("gateway").ok_or_else(|| {
        GatewayError::with_status(400, "QClaw 网关配置缺少 gateway 段（客户端可能未完成初始化）")
    })?;
    let chat_port = gateway
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_CHAT_PORT);
    let token = gateway
        .pointer("/auth/token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            GatewayError::with_status(
                400,
                "QClaw 网关配置缺少 gateway.auth.token（客户端可能未完成登录）",
            )
        })?
        .to_string();
    // 端点开关：显式关掉时聊天必然 404，导入阶段就拦下来，别等用户发消息才报错
    if gateway.pointer("/http/endpoints/chatCompletions/enabled") == Some(&json!(false)) {
        return Err(GatewayError::with_status(
            400,
            "QClaw 的 OpenAI 兼容端点未开启（gateway.http.endpoints.chatCompletions.enabled = false）",
        ));
    }
    Ok(LocalGateway {
        chat_port,
        token,
        models_port: read_models_port(&home),
    })
}

/// 从 `qclaw.json` 的 `authGatewayBaseUrl` 解析模型清单端口。
///
/// 形如 `http://127.0.0.1:19000/proxy`；文件缺失 / 字段缺失 / 解析不出
/// 端口时一律回退 [`DEFAULT_MODELS_PORT`]（这是**尽力而为**的字段：
/// 缺了它只影响模型清单刷新，聊天不受影响）。
fn read_models_port(home: &Path) -> u16 {
    let path = home.join("qclaw.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return DEFAULT_MODELS_PORT;
    };
    let Ok(config) = serde_json::from_str::<Value>(&text) else {
        return DEFAULT_MODELS_PORT;
    };
    let Some(url) = config.get("authGatewayBaseUrl").and_then(Value::as_str) else {
        return DEFAULT_MODELS_PORT;
    };
    // 只认 http:// 形式（本地网关不可能是 https）；端口取最后一段 `:NNNN`
    let rest = url.trim();
    let without_scheme = rest
        .strip_prefix("http://")
        .or_else(|| rest.strip_prefix("https://"))
        .unwrap_or(rest);
    let host_port = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    host_port
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_MODELS_PORT)
}

/// 聊天 completions 的完整 URL。
pub fn chat_completions_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/chat/completions")
}

/// 模型清单的完整 URL（鉴权网关的 `/proxy/llm/models`）。
pub fn models_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/proxy/llm/models")
}

/// 探一次本机聊天网关：`GET /v1/models`（带令牌）。
///
/// ── 为什么用 `/v1/models` 而不是 `/v1/chat/completions` 做探针 ─────
/// 后者是 POST 且会真花一次配额；前者是轻量 GET，且同样要过 Bearer 校验
/// （令牌错会 401），足以回答「网关活着且令牌对」。
///
/// 失败分两档给文案：连不上（QClaw 没在跑）与令牌被拒（重启后令牌变了）。
pub async fn probe_chat_gateway(gateway: &LocalGateway) -> Result<(), GatewayError> {
    let client = crate::server::core::egress::client_for(None);
    let url = format!("http://127.0.0.1:{}/v1/models", gateway.chat_port);
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(8))
        .header("Authorization", format!("Bearer {}", gateway.token))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| {
            GatewayError::with_status(
                502,
                format!(
                    "连不上 QClaw 本机网关（127.0.0.1:{}）：请确认 QClaw 客户端正在运行（{}）",
                    gateway.chat_port,
                    crate::server::core::egress::describe_error_detail(&err)
                ),
            )
        })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        logging::log_event(
            "warn",
            "qclaw",
            "本机网关探测失败",
            Some(json!({ "status": status, "body": body.chars().take(200).collect::<String>() })),
        );
        return Err(GatewayError::with_status(
            if status == 401 || status == 403 { 401 } else { 502 },
            if status == 401 || status == 403 {
                "QClaw 网关令牌无效：客户端可能刚重启过，请重新导入账号".to_string()
            } else {
                format!("QClaw 本机网关返回 HTTP {status}")
            },
        ));
    }
    Ok(())
}

// ─── 云端登录态（jprx 凭证 + 身份，见模块头）────────────────────

/// 尽力而为地解出云端会话。任何一步失败都返回**空会话**（调用方据此报错）。
///
/// 与 [`read_identity`] 的差别：那个只服务备注名（失败无所谓），这个要真去
/// 打云端接口，所以调用方必须检查 `jwt` 是否为空。
pub fn read_cloud_session() -> CloudSession {
    match read_cloud_session_inner() {
        Ok(session) => session,
        Err(err) => {
            logging::log_event(
                "warn",
                "qclaw",
                "云端登录态解密失败（积分/订阅查询不可用）",
                Some(json!({ "error": err.message })),
            );
            CloudSession::default()
        }
    }
}

/// 尽力而为地解出云端身份。任何一步失败都返回空身份（不阻断导入）。
pub fn read_identity() -> Identity {
    let session = read_cloud_session();
    Identity {
        user_id: session.user_id,
        nickname: session.nickname,
    }
}

#[cfg(windows)]
fn read_cloud_session_inner() -> Result<CloudSession, GatewayError> {
    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else {
        return Err(GatewayError::with_status(400, "读不到 APPDATA 环境变量"));
    };
    let local_state = appdata.join("QClaw").join("Local State");
    let leveldb_dir = appdata.join("QClaw").join("Local Storage").join("leveldb");
    if !local_state.is_file() || !leveldb_dir.is_dir() {
        return Err(GatewayError::with_status(
            400,
            "未找到 QClaw 的登录态存储（%APPDATA%\\QClaw\\Local Storage\\leveldb）",
        ));
    }
    let state_text = std::fs::read_to_string(&local_state)
        .map_err(|err| GatewayError::with_status(500, format!("读取 Local State 失败：{}", err.kind())))?;
    let state: Value = serde_json::from_str(&state_text)
        .map_err(|_| GatewayError::with_status(400, "Local State 不是合法 JSON"))?;
    let encrypted_key = state
        .pointer("/os_crypt/encrypted_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| GatewayError::with_status(400, "Local State 缺少 os_crypt.encrypted_key"))?;
    let raw = crypt::base64_decode(encrypted_key)
        .ok_or_else(|| GatewayError::with_status(400, "encrypted_key 不是合法 base64"))?;
    // DPAPI 密文带 5 字节 "DPAPI" 前缀，解掉才是 32 字节 AES-256 主密钥
    let Some(blob) = raw.strip_prefix(b"DPAPI") else {
        return Err(GatewayError::with_status(
            400,
            "encrypted_key 缺少 DPAPI 前缀（Chromium os_crypt 布局已变？）",
        ));
    };
    let master = crypt::dpapi_unprotect(blob)?;

    // LevelDB 的键值散落在目录下的 .log / .ldb 里，全部摊开找
    let mut blobs: Vec<u8> = Vec::new();
    let entries = std::fs::read_dir(&leveldb_dir)
        .map_err(|err| GatewayError::with_status(500, format!("读取 leveldb 目录失败：{}", err.kind())))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            blobs.extend_from_slice(&bytes);
        }
    }
    if blobs.is_empty() {
        return Err(GatewayError::with_status(400, "leveldb 目录为空"));
    }

    // jwt_token 解出来是 JWT 文本；userInfo 是 JSON 字节（下面按字节喂
    // serde_json::from_slice，所以 find_decrypted 统一回 Vec<u8>）；
    // `login` 是渠道令牌（jprx 的 `X-Token` 头），同样只以密文形态存在。
    let jwt_raw = find_decrypted(&blobs, &master, "jwt_token");
    let user_info = find_decrypted(&blobs, &master, "userInfo");
    let login_raw = find_decrypted(&blobs, &master, "login");
    let jwt = jwt_raw
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .unwrap_or("")
        .to_string();
    let login_key = login_raw
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .unwrap_or("")
        .to_string();
    let nickname = identity_nickname(jwt_raw.as_deref().and_then(|b| std::str::from_utf8(b).ok()), user_info.as_deref());
    Ok(CloudSession {
        jwt,
        login_key,
        guid: read_guid(),
        user_id: identity_user_id(
            jwt_raw.as_deref().and_then(|b| std::str::from_utf8(b).ok()),
            user_info.as_deref(),
        ),
        nickname,
    })
}

#[cfg(not(windows))]
fn read_cloud_session_inner() -> Result<CloudSession, GatewayError> {
    Err(GatewayError::with_status(
        400,
        "QClaw 登录态解密仅支持 Windows（os_crypt 走 DPAPI）",
    ))
}

/// `qclaw.json` 的 `sharedParams.guid`（jprx 的 `X-Guid` 头）。
/// 缺失时回空串 —— 由 `points` 的调用方按失败处理。
fn read_guid() -> String {
    let Some(home) = qclaw_home() else {
        return String::new();
    };
    read_guid_from(&home)
}

/// [`read_guid`] 的可测形态：显式传入 `~/.qclaw` 目录。
fn read_guid_from(home: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(home.join("qclaw.json")) else {
        return String::new();
    };
    let Ok(config) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    config
        .pointer("/sharedParams/guid")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|guid| !guid.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 从 JWT 的 `exp` 声明解出到期时刻（毫秒时间戳）。
///
/// 账号页「有效期」列读它：云端会话 30 天一期（实测 `iat`→`exp`），
/// 过期后本机网关的云端转发会失效。解不出（非 JWT / 无 exp）返回 `None`，
/// 界面按 chatglm 同口径不渲染那一列。
pub fn jwt_expires_at(token: &str) -> Option<i64> {
    let payload = jwt_payload(token)?;
    let exp = payload.get("exp").and_then(Value::as_i64).filter(|n| *n > 0)?;
    Some(exp.saturating_mul(1000))
}

/// 从 JWT / userInfo 两份（都可能没有）里挑用户 id。
fn identity_user_id(jwt: Option<&str>, user_info: Option<&[u8]>) -> String {
    if let Some(token) = jwt {
        if let Some(payload) = jwt_payload(token) {
            let id = first_string(&payload, &["sub", "user_id", "userId", "uid"]);
            if !id.is_empty() {
                return id.chars().take(64).collect();
            }
        }
    }
    if let Some(bytes) = user_info {
        if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            let id = first_string(&value, &["user_id", "userId", "uid", "id", "sub"]);
            if !id.is_empty() {
                return id.chars().take(64).collect();
            }
        }
    }
    String::new()
}

/// 昵称：优先 userInfo（客户端展示用的那个），退回 JWT 声明。
fn identity_nickname(jwt: Option<&str>, user_info: Option<&[u8]>) -> String {
    if let Some(bytes) = user_info {
        if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            let name = first_string(
                &value,
                &["nickname", "nick", "name", "display_name", "displayName"],
            );
            if !name.is_empty() {
                return name.chars().take(100).collect();
            }
        }
    }
    if let Some(token) = jwt {
        if let Some(payload) = jwt_payload(token) {
            let name = first_string(&payload, &["nickname", "name", "display_name"]);
            if !name.is_empty() {
                return name.chars().take(100).collect();
            }
        }
    }
    String::new()
}

/// 在 LevelDB 字节里找 `key` 紧跟其后的 `v10` 密文并解开。
///
/// ── 为什么「值跟在键后面」这个假设成立 ─────────────────────
/// LevelDB 一条记录是 `key → value` 连续排放的，localStorage 后端把
/// `_chromium_...` 之外的键原样落盘。键名与值之间只隔 varint 长度 +
/// 类型字节，所以键名后几百字节内必然能扫到值的 base64（`djEw` 开头）。
///
/// ── 为什么要从长到短逐个回退 ─────────────────────────────
/// base64 字符会一路延伸到相邻记录里去，贪婪匹配会把**别人的字节**
/// 也吃进来，GCM tag 校验必败。从长到短砍，第一根能通过 tag 校验的
/// 就是真值（tag 校验失败的概率是 2^-128，不存在误收）。
fn find_decrypted(blobs: &[u8], master: &[u8], key: &str) -> Option<Vec<u8>> {
    let needle = key.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = find_subslice(blobs, needle, from) {
        from = offset + needle.len();
        let window_end = (from + 800).min(blobs.len());
        let Some(encoded) = find_v10_base64(&blobs[from..window_end]) else {
            continue;
        };
        if let Some(plain) = try_open(master, encoded) {
            return Some(plain);
        }
    }
    None
}

/// 字节串里找子串（`[u8]::windows` 那个写法在超长缓冲上太慢，手写双循环）。
fn find_subslice(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    let mut i = from;
    while i <= last {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 找 `djEw`（base64 的 `v10`）起始的一串 base64 字符。
fn find_v10_base64(window: &[u8]) -> Option<&[u8]> {
    let start = find_subslice(window, b"djEw", 0)?;
    let mut end = start;
    while end < window.len() && is_base64_char(window[end]) {
        end += 1;
    }
    // 太短的不可能是密文（v10 + nonce + tag 至少 31 字节 → 42 个 base64 字符）
    if end - start < 42 {
        return None;
    }
    Some(&window[start..end])
}

fn is_base64_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/' || byte == b'='
}

/// 从长到短砍 base64，第一根通过 GCM tag 校验的就是真值（见 [`find_decrypted`]）。
fn try_open(master: &[u8], encoded: &[u8]) -> Option<Vec<u8>> {
    let mut length = encoded.len();
    while length > 40 {
        let candidate = &encoded[..length];
        // base64 按 4 字符一组，砍齐再解
        let usable = length - (length % 4);
        if usable >= 44 {
            if let Some(raw) = crypt::base64_decode(&String::from_utf8_lossy(&candidate[..usable])) {
                if raw.len() > 31 && &raw[..3] == b"v10" {
                    if let Ok(plain) = crypt::aes256_gcm_open(master, &raw[3..15], &raw[15..]) {
                        return Some(plain);
                    }
                }
            }
        }
        length -= 1;
    }
    None
}

/// 解 JWT 的 payload 段（与 `qwenwork::auth::jwt_payload` 同一套口径）。
fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    if payload.is_empty() {
        return None;
    }
    let bytes = crypt::base64_decode(payload)?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

/// 取第一个非空字符串字段（与 `qwenwork::auth::first_string` 同口径）。
fn first_string(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_port_parses_from_auth_gateway_url() {
        let dir = std::env::temp_dir().join("qclaw-test-models-port");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(
            dir.join("qclaw.json"),
            r#"{"authGatewayBaseUrl":"http://127.0.0.1:19000/proxy"}"#,
        )
        .expect("write");
        assert_eq!(read_models_port(&dir), 19000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn models_port_falls_back_when_missing() {
        let dir = std::env::temp_dir().join("qclaw-test-models-port-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        assert_eq!(read_models_port(&dir), DEFAULT_MODELS_PORT);
        std::fs::write(dir.join("qclaw.json"), "not json").expect("write");
        assert_eq!(read_models_port(&dir), DEFAULT_MODELS_PORT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn urls_are_loopback() {
        assert_eq!(
            chat_completions_url(9434),
            "http://127.0.0.1:9434/v1/chat/completions"
        );
        assert_eq!(
            models_url(19000),
            "http://127.0.0.1:19000/proxy/llm/models"
        );
    }

    #[test]
    fn v10_base64_requires_minimum_length() {
        // 42 字符是 v10+nonce+tag 的 base64 下限，短了的不是密文
        assert!(find_v10_base64(b"djEwAAAA").is_none());
        let long = b"djEwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(find_v10_base64(long).is_some());
    }

    #[test]
    fn subslice_search_handles_overlap() {
        assert_eq!(find_subslice(b"ababab", b"bab", 0), Some(1));
        assert_eq!(find_subslice(b"ababab", b"bab", 2), Some(3));
        assert_eq!(find_subslice(b"abc", b"abcd", 0), None);
        assert_eq!(find_subslice(b"abc", b"", 0), None);
    }

    /// JWT `exp`（秒）→ 毫秒时间戳；非 JWT / 无 exp 一律 None。
    #[test]
    fn jwt_expires_at_reads_exp_claim() {
        // payload = {"exp": 1793320886}
        let token = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjE3OTMzMjA4ODZ9.sig";
        assert_eq!(jwt_expires_at(token), Some(1_793_320_886_000));
        assert_eq!(jwt_expires_at("not-a-jwt"), None);
        assert_eq!(jwt_expires_at("a.b.c"), None);
    }

    /// guid 从 `qclaw.json` 的 `sharedParams.guid` 读；缺失/坏 JSON 回空串。
    #[test]
    fn guid_reads_from_shared_params() {
        let dir = std::env::temp_dir().join("qclaw-test-guid");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(
            dir.join("qclaw.json"),
            r#"{"sharedParams":{"guid":"9c77ba16"}}"#,
        )
        .expect("write");
        assert_eq!(read_guid_from(&dir), "9c77ba16");
        std::fs::write(dir.join("qclaw.json"), "not json").expect("write");
        assert_eq!(read_guid_from(&dir), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
