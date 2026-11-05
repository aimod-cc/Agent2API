//! 会话相关路由（对照 Node 版 server.mjs 776-869、963-974 行逐条实现）。
//!
//!   GET    /api/session                 前端首屏状态（需鉴权）
//!   GET    /api/session/login/methods   动态登录方式与纯文本 prompts（需鉴权）
//!   POST   /api/session/login/start     发起登录（可选 methodIndex / inputs / proxy）
//!   GET    /api/session/login/wait      轮询登录结果 ?state=
//!   POST   /api/session/login/cancel    取消登录（关弹窗/用户放弃）
//!   POST   /api/session/login/callback  提交网页登录回调（壳侧捕获或远程面板粘贴）
//!   GET    /api/session/login/remote-callback   远程网页登录回调中转（网页端）
//!   POST   /api/session/refresh         刷新当前账号 token
//!   POST   /api/session/logout          清除登录态（删掉当前账号）
//!   POST   /auth/login                  同步登录（等完成才响应）
//!   POST   /auth/logout                 清除登录态
//!
//! ── 鉴权分组（务必与 Node 版一致）────────────────────────────
//! 会话与登录管理入口挂 protected；浏览器 / 壳侧回调中转由一次性 state 自理。
//! 分组以 http.rs 为准，动态登录 methods 与 start 均经过同一管理面鉴权。
//!
//! ── 字段来源（/api/session）────────────────────────────────
//!   health          ← auth.get_config_summary()（纯本地：只查凭证是否存在）
//!   session         ← auth.get_status()（含临期自动刷新）
//!   accounts        ← store.list_accounts()
//!   lastRequestModel← config.json 的 lastRequestModel
//!   routedAccountId ← 按 lastRequestModel 派生的「下一个请求会先用谁」（见其函数注释）
//!   defaultModel    ← config.json 的 defaultModel
//!   proxies         ← Clash 摘要（切片 3 起为实时读取的真实值）
//!   models          ← 模型目录（切片 4 起为真实清单，含远程刷新结果）
//!   sanitizeBlacklistFingerprints ← 出站指纹脱敏开关（与 GET /api/sanitize 同源）

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Json, OriginalUri, Query, State};
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::api::health::UNCONFIGURED_REASON;
use crate::server::config;
use crate::server::core::login::LOGIN_TIMEOUT_MS;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/session —— 首屏状态（管理面鉴权见 http.rs）
pub async fn get_session(State(state): State<ServerState>) -> Response {
    let snapshot = config::current();
    // 可见账号与路由页预演共用同一份快照，插件未就绪的账号不参与。
    let accounts = state.store().list_visible_accounts();
    let routed = super::routing_api::queue_for_accounts(
        &state, snapshot.last_request_model().map(str::to_string), &crate::server::core::routing::accounts_of(&accounts),
    )
        .get("pickedId").and_then(Value::as_str).filter(|id| !id.is_empty())
        .map(|id| Value::String(id.to_string())).unwrap_or(Value::Null);
    let summary = state.auth().get_config_summary();
    let configured = summary
        .get("configured")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ok_json(json!({
        "health": {
            "upstreamConfigured": configured,
            "upstreamBaseUrl": summary.get("baseUrl").cloned().unwrap_or(Value::Null),
            "authSource": summary.get("authSource").cloned().unwrap_or(Value::Null),
            "unavailableReason": if configured {
                Value::Null
            } else {
                summary
                    .get("unavailableReason")
                    .cloned()
                    .unwrap_or_else(|| Value::String(UNCONFIGURED_REASON.to_string()))
            },
        },
        "session": state.auth().get_status(),
        // accounts.currentAccountId = **不限模型**的队首（判据 = 启用 + 有可用凭证 +
        // 优先级序，见 core::account_store）。它回答「队列第一位是谁」，别的地方
        // （凭证来源、刷新目标）读的也一直是它，语义不变。
        // 账号页的 ★ / 「首选」要读的是**下一个请求会先用谁** —— 那是按最近一次
        // 请求的模型派生的 `routedAccountId`（见下），两者在队首正被限流时会不同。
        "accounts": accounts,
        // 最近一次实际请求用的模型：账号页的「模型」筛选默认选它
        "lastRequestModel": snapshot.last_request_model(),
        // 与路由页共用的无会话预演队首，包含自动消耗规则与运行期保护。
        "routedAccountId": routed,
        "defaultModel": snapshot.default_model(),
        "proxies": proxies_summary(),
        // 模型目录真值（对照 server.mjs 802-804 的字段形状 {id, name, isDefault, credits}）。
        // 来源是**聚合目录**（`providers::catalog::session_models`），与 `GET /v1/models`
        // 同一份合并结果：界面上的模型清单必须等于客户端能拿到的清单，否则用户会照着一个
        // 路由不到的模型名去调（改造前这里只读 workbuddy 单家，另三家在界面上不可见）。
        // 每条另带 provider / providerLabel，供网关页按家分组。
        "models": crate::server::core::providers::catalog::session_models(state.store()),
        // 启动期的历史统计整理是否还在后台跑（模型维度回填 / 来源维度回填 /
        // 保留期裁剪）。界面可据此显示一条轻提示；它不影响账号与转发。
        "maintenance": { "running": state.startup_maintenance_running() },
        // 指纹脱敏开关（与 GET /api/sanitize 同一个键、同一个值）
        config::KEY_SANITIZE_FINGERPRINTS: snapshot.sanitize_fingerprints(),
        // 插件申报的**账号行渲染参数**（`{provider: AccountFeatures}`）。
        //
        // ── 为什么挂在 /api/session ──────────────────────────────
        // 账号页的首屏数据就来自这里，而它每一列都要用这些参数（标识读哪个
        // 键、有效期读哪个键、要不要给余额入口）。放在同一次响应里，界面
        // 不必再发一次请求，也不会出现「账号已经画出来了、参数还没到」的
        // 中间态（那会让整列闪一下）。
        //
        // 没申报的家不在表里 —— 界面走自己的兜底表，旧插件照常工作。
        "accountFeatures": account_features(),
    }))
}

/// 已加载插件的公开账号能力表。
fn account_features() -> Value {
    Value::Object(crate::server::core::plugin_host::account_features())
}

fn proxies_summary() -> Value {
    let clash = crate::server::core::proxies::clash_proxy_options();
    json!({
        "clashAvailable": clash["available"],
        "clashError": clash["error"],
        "optionCount": clash.get("options").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
    })
}

// ─── GET /api/session/login/methods（protected）──────────────

// 只投影明确的元数据字段。插件的 HTML / raw / secret / inputs 等附带字段
// 不进入响应；这里所有文案都是 JSON 纯文本，消费端只能按文本节点渲染。
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginMethodsMetadata {
    methods: Vec<LoginMethodMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginMethodMetadata {
    method_index: u64,
    #[serde(rename = "type")]
    kind: LoginMethodType,
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(default)]
    prompts: Vec<LoginPromptMetadata>,
    #[serde(default)]
    local_only: bool,
    #[serde(default)]
    manual_callback: bool,
    #[serde(default)]
    callback_input: LoginCallbackInput,
    /// 插件要求这条 OAuth 流程只在应用内嵌窗口打开（不提供系统浏览器选项）。
    ///
    /// 用于登录页跑在本机回环地址上的插件（AStudio）：交给系统浏览器只会让
    /// 用户多做一个没有意义的选择。只对 oauth 方式有效。
    #[serde(default)]
    embedded_only: bool,
}

#[derive(Clone, Copy, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum LoginCallbackInput { #[default] Url, Code }

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum LoginMethodType { Oauth, Api }

#[derive(serde::Deserialize, serde::Serialize)]
struct LoginPromptMetadata {
    key: String,
    #[serde(rename = "type")]
    kind: LoginPromptType,
    label: String,
    #[serde(default)]
    placeholder: String,
    #[serde(default)]
    optional: bool,
    // 只允许非敏感的静态默认值；凭证字段永远不下发 default/value。
    #[serde(default)]
    secret: bool,
    #[serde(default, rename = "default", skip_serializing_if = "Option::is_none")]
    default_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    when: Option<LoginPromptCondition>,
    #[serde(default)]
    options: Vec<LoginPromptOption>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum LoginPromptType { Text, Select }

#[derive(serde::Deserialize, serde::Serialize)]
struct LoginPromptCondition {
    key: String,
    op: String,
    value: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct LoginPromptOption {
    value: String,
    label: String,
}

struct SelectedLogin {
    /// methods 校验与 login.start 共用同一实例，不在二者之间重新解析 owner。
    adapter: std::sync::Arc<dyn crate::server::core::providers::adapter::ProviderAdapter>,
    proxy: Option<crate::server::core::login::LoginProxySelection>,
    options: Value,
    local_only: bool,
    manual_callback: bool,
    callback_input: LoginCallbackInput,
    instructions: Option<String>,
}

/// 旧入口（没有 `methodIndex`）的 `SelectedLogin`：只为把登录代理带下去。
///
/// ── 为什么旧入口也要构造它 ──────────────────────────────────
/// `start_plugin_web_login` 只有在 `selected` 为 `Some` 时才把 `options` 发给
/// 插件（`None` 分支用的是空 `options`）。旧入口不声明 `manualCallback` /
/// `localOnly` —— 那些是动态方法的元数据，旧调用没有，保持默认值即与改造前
/// 一致；唯一的增量是 `options.proxy`。
///
/// 适配器查不到时返回 `None`，让 `start_plugin_web_login` 走它原有的
/// 「未知提供商 / 不支持网页登录」报错路径，而不是在这里吞掉错误。
fn legacy_selected(
    provider: &str,
    proxy: Option<crate::server::core::login::LoginProxySelection>,
    options: Value,
) -> Option<SelectedLogin> {
    let adapter = crate::server::core::providers::adapter::adapter_by_id(provider)?;
    Some(SelectedLogin {
        adapter,
        proxy,
        options,
        local_only: false,
        manual_callback: false,
        callback_input: LoginCallbackInput::Url,
        instructions: None,
    })
}

async fn read_login_methods(
    state: &ServerState,
    adapter: &dyn crate::server::core::providers::adapter::ProviderAdapter,
) -> Result<Option<LoginMethodsMetadata>, crate::server::errors::GatewayError> {
    use crate::server::core::login::login_text;
    use crate::server::errors::GatewayError;
    let Some(value) = adapter.login_methods_metadata(state.store()).await? else {
        return Ok(None);
    };
    let value = if value.is_array() { json!({ "methods": value }) } else { value };
    let mut metadata: LoginMethodsMetadata = serde_json::from_value(value)
        .map_err(|_| GatewayError::with_status(502, "插件返回的登录方式元数据无法解析"))?;
    let mut indices = std::collections::HashSet::new();
    for method in &mut metadata.methods {
        // JSON number 要能被网页端精确表示；索引保留插件原值，绝不按显示顺序编号。
        if method.method_index > 9_007_199_254_740_991 || !indices.insert(method.method_index) {
            return Err(GatewayError::with_status(502, "插件登录方式索引无效或重复"));
        }
        if matches!(method.callback_input, LoginCallbackInput::Code) && !method.manual_callback {
            return Err(GatewayError::with_status(502, "code 回调输入只能用于手动登录回调"));
        }
        method.label = login_text(&method.label);
        method.instructions = method.instructions.take().map(|text| login_text(&text));
        let mut keys = std::collections::HashSet::new();
        for prompt in &mut method.prompts {
            if prompt.key.is_empty() || prompt.key.len() > 64 ||
                !prompt.key.bytes().enumerate().all(|(index, ch)| ch.is_ascii_alphabetic() ||
                    (index > 0 && (ch.is_ascii_digit() || matches!(ch, b'_' | b'-')))) ||
                matches!(prompt.key.as_str(), "__proto__" | "prototype" | "constructor" | "signal") ||
                !keys.insert(prompt.key.clone()) {
                return Err(GatewayError::with_status(502, "插件登录输入字段无效或重复"));
            }
            prompt.label = login_text(&prompt.label);
            prompt.placeholder = login_text(&prompt.placeholder);
            for option in &mut prompt.options { option.label = login_text(&option.label); }
            if matches!(prompt.kind, LoginPromptType::Text) { prompt.options.clear(); }
            if matches!(prompt.kind, LoginPromptType::Select) && prompt.options.is_empty() {
                return Err(GatewayError::with_status(502, "插件登录选择字段没有可用选项"));
            }
            if prompt.secret { prompt.default_value = None; }
            if prompt.default_value.as_ref().is_some_and(|value| value.len() > 8192 || value.contains('\0') ||
                (matches!(prompt.kind, LoginPromptType::Select) && !prompt.options.iter().any(|option| &option.value == value))) {
                return Err(GatewayError::with_status(502, "插件登录字段默认值无效"));
            }
        }
        for prompt in &method.prompts {
            if let Some(condition) = &prompt.when {
                if !keys.contains(&condition.key) || !matches!(condition.op.as_str(), "eq" | "neq") ||
                    condition.value.len() > 8192 || condition.value.contains('\0') {
                    return Err(GatewayError::with_status(502, "插件登录字段条件无效"));
                }
            }
        }
        if method.local_only && matches!(method.kind, LoginMethodType::Api) {
            return Err(GatewayError::with_status(502, "本机登录不能进入复制 API 凭证路线"));
        }
        if method.embedded_only && matches!(method.kind, LoginMethodType::Api) {
            return Err(GatewayError::with_status(502, "API 凭证方式不能要求内嵌窗口"));
        }
    }
    metadata.instructions = metadata.instructions.map(|text| login_text(&text));
    Ok(Some(metadata))
}

/// 成功数据 `{methods:[...], instructions?}`。404 / 501 表示没有动态元数据，
/// 调用方回到既有表单；不携带 methodIndex 的旧 POST /start 不做这次探测。
pub async fn login_methods(
    State(state): State<ServerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let provider = params.get("provider").map(|value| value.trim()).unwrap_or("");
    if provider.is_empty() { return management_error(400, "缺少 provider"); }
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(provider) else {
        return management_error(400, format!("未知的提供商：{provider}"));
    };
    match read_login_methods(&state, adapter.as_ref()).await {
        Ok(Some(metadata)) => match serde_json::to_value(metadata) {
            Ok(value) => ok_json(value),
            Err(_) => management_error(502, "登录方式元数据无法序列化"),
        },
        Ok(None) => management_error(501, "该提供商尚未申报动态登录方式，请使用既有登录入口"),
        Err(error) => management_error(error.status_code, error.message),
    }
}

fn checked_login_inputs(
    method: &LoginMethodMetadata,
    inputs: Option<&Value>,
) -> Result<Value, crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    let mut inputs = inputs.cloned().unwrap_or_else(|| json!({}));
    let Some(object) = inputs.as_object_mut() else {
        return Err(GatewayError::bad_request("inputs 必须是字符串字段对象"));
    };
    if object.iter().any(|(key, value)| !value.is_string() || !method.prompts.iter().any(|prompt| &prompt.key == key)) {
        return Err(GatewayError::bad_request("inputs 包含未申报的字段或非字符串值"));
    }
    for prompt in &method.prompts {
        if let Some(condition) = &prompt.when {
            let equal = object.get(&condition.key).and_then(Value::as_str) == Some(condition.value.as_str());
            let enabled = if condition.op == "eq" { equal } else { !equal };
            if !enabled {
                if object.contains_key(&prompt.key) { return Err(GatewayError::bad_request("inputs 包含未启用的字段")); }
                continue;
            }
        }
        if !object.contains_key(&prompt.key) {
            if let Some(default) = &prompt.default_value { object.insert(prompt.key.clone(), Value::String(default.clone())); }
        }
        let value = object.get(&prompt.key).and_then(Value::as_str).unwrap_or("");
        if matches!(prompt.kind, LoginPromptType::Select) {
            let supplied = object.get(&prompt.key).and_then(Value::as_str);
            if supplied.is_none() || (prompt.optional && value.is_empty()) {
                if !prompt.optional { return Err(GatewayError::bad_request(format!("请选择 {}", prompt.label))); }
            } else if !prompt.options.iter().any(|option| option.value == value) {
                return Err(GatewayError::bad_request(format!("{}的选项无效", prompt.label)));
            }
        } else if !prompt.optional && value.trim().is_empty() {
            return Err(GatewayError::bad_request(format!("请填写 {}", prompt.label)));
        }
    }
    // 只校验、不裁剪凭证内容，且后续绝不把 inputs 回显给前端或日志。
    Ok(inputs)
}

fn checked_login_proxy(
    value: Option<&Value>,
) -> Result<Option<crate::server::core::login::LoginProxySelection>, crate::server::errors::GatewayError> {
    use crate::server::core::login::LoginProxySelection;
    use crate::server::core::proxies::{normalize_account_proxy, resolve_account_proxy, ResolvedProxy};
    use crate::server::errors::GatewayError;
    let Some(value) = value else { return Ok(None) };
    // null（现有表单）及显式 direct 都记为直连，不能与「没有选择」混淆。
    let direct = value.get("source").and_then(Value::as_str)
        .is_some_and(|source| source.trim().eq_ignore_ascii_case("direct"));
    let config = normalize_account_proxy(if direct { &Value::Null } else { value })
        .map_err(|_| GatewayError::bad_request("登录代理配置无效，请检查代理来源、主机与端口"))?
        .unwrap_or_else(|| json!({ "source": "direct" }));
    if config.get("source").and_then(Value::as_str) == Some("direct") {
        return Ok(Some(LoginProxySelection { config, resolved: Value::Null }));
    }
    if crate::server::core::proxies::is_rotate_config(&config) {
        return Err(GatewayError::bad_request("登录代理不支持轮询出网，请选择一条已保存的代理"));
    }
    let Some(resolution) = resolve_account_proxy(Some(&config)) else {
        return Err(GatewayError::bad_request("所选登录代理未能解析，已拒绝回退默认出口"));
    };
    if resolution.error().is_some() {
        return Err(GatewayError::bad_request("登录代理无法解析，请检查所选代理是否存在且已启用"));
    }
    let proxy = ResolvedProxy::from_json(&resolution.to_json())
        .map_err(|_| GatewayError::bad_request("登录代理出口无效，请检查主机与端口"))?;
    let wire = proxy.map(|proxy| agent2api_provider_protocol::WireProxy {
        protocol: proxy.protocol, host: proxy.host, port: proxy.port,
        username: proxy.username, password: proxy.password,
    });
    let resolved = serde_json::to_value(wire)
        .map_err(|_| GatewayError::bad_request("登录代理无法序列化"))?;
    Ok(Some(LoginProxySelection { config, resolved }))
}

fn login_start_response(
    start: &agent2api_provider_protocol::LoginStart,
    provider: &str,
    edition: &str,
    manual_callback: bool,
    callback_input: LoginCallbackInput,
) -> Value {
    json!({
        "state": start.flow, "flow": start.flow,
        "authUrl": start.url, "url": start.url,
        "edition": edition, "provider": provider,
        "instructions": crate::server::core::login::login_text(&start.instructions),
        "pollIntervalMs": start.poll_interval_ms,
        "externalBrowser": start.external_browser,
        "allowedHosts": start.allowed_hosts,
        "manualCallback": manual_callback,
        "callbackInput": callback_input,
    })
}

// ─── POST /api/session/login/start ──────────────────────────

/// 发起登录：显式 methodIndex 时校验动态 methods；旧调用保持专用入口。
/// 空 URL 可用于本机 / 自动登录，但必须有非空 flow，最终仍只看轮询 Done。
///
/// ── `provider` 维度 ───────────────────────────────────────
/// body 里的可选 `provider`（缺省 = workbuddy）决定走哪条链：
///   - **由插件接管的家**（含全部内置家装上插件后、以及第三方 provider）→
///     `start_plugin_web_login`：问插件的 `login.start` 拿完整 `LoginStart`
///     （授权地址 + flow + 流程形态），回调型等 `login.callback`、轮询型由网关
///     按 `pollIntervalMs` 问 `login.poll`；
///   - 注册表里的 provider 但**没有插件接管** → 503，指名「需要安装插件」；
///   - 注册表里没有的 id → 400「未知的提供商」（不静默回落 workbuddy：
///     那会把一次小浣熊登录发起成 workbuddy 登录）。
///
/// 网关已经没有任何一家的登录实现 —— 这一段的分流只有「插件」与「没装插件」
/// 两种结果，不再有内置换码的分支。
pub async fn login_start(State(state): State<ServerState>, body: Bytes) -> Response {
    // 请求里的 `edition` 不再参与地区判定：拆家后界面上两张卡片各发各的
    // provider id，那是权威（`edition` 只是前端回显字段，见下面 Qoder /
    // ZCode 两个分支的说明）。解析失败不是错误（Node 版 `catch { edition = null }`）。
    let payload = parse_body(&body).ok();
    let provider = payload
        .as_ref()
        .and_then(|payload| payload.get("provider").and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or("");
    // 显式索引走动态契约，先于旧 AutoClaw 特判；没有索引的旧调用完全不探测
    // auth.methods，仍保留各家的专用登录入口及支持位判定。
    if let Some(request) = payload.as_ref().filter(|value| value.get("methodIndex").is_some()) {
        let Some(index) = request.get("methodIndex").and_then(Value::as_u64) else {
            return management_error(400, "methodIndex 必须是非负整数");
        };
        let provider = if provider.is_empty() { "workbuddy" } else { provider };
        let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(provider) else {
            return management_error(400, format!("未知的提供商：{provider}"));
        };
        let metadata = match read_login_methods(&state, adapter.as_ref()).await {
            Ok(Some(metadata)) => metadata,
            Ok(None) => return management_error(501, "该提供商尚未申报动态登录方式，请使用既有登录入口"),
            Err(error) => return management_error(error.status_code, error.message),
        };
        let Some(method) = metadata.methods.iter().find(|method| method.method_index == index) else {
            return management_error(400, "methodIndex 不在该提供商的登录方式清单中");
        };
        let inputs = match checked_login_inputs(method, request.get("inputs")) {
            Ok(inputs) => inputs,
            Err(error) => return management_error(error.status_code, error.message),
        };
        let proxy = match checked_login_proxy(request.get("proxy")) {
            Ok(proxy) => proxy,
            Err(error) => return management_error(error.status_code, error.message),
        };
        let mut options = json!({ "methodIndex": index, "inputs": inputs });
        if let Some(selection) = &proxy {
            options["proxy"] = selection.resolved.clone();
        }
        let instructions = [metadata.instructions.as_deref(), method.instructions.as_deref()]
            .into_iter().flatten().filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>().join("\n");
        let selected = SelectedLogin {
            adapter,
            proxy,
            options,
            local_only: method.local_only,
            manual_callback: method.manual_callback,
            callback_input: method.callback_input,
            instructions: (!instructions.is_empty()).then_some(instructions),
        };
        return start_plugin_web_login(state, provider.to_string(), Some(selected)).await;
    }
    // AutoClaw：网页登录走**另一条入口**（`/api/session/login/oauth/start`），
    // 必须在下面所有分流之前挡掉 —— 通用插件链会回「插件未能生成授权地址」，
    // 而国际版是**支持**的，只是发起方式不同（授权地址要先过一次浏览器端风控
    // 验证码，由前端拿地址再交壳开窗口）。
    if let Some(region) = crate::server::core::providers::autoclaw::Region::from_provider_id(provider) {
        let message = if region == crate::server::core::providers::autoclaw::Region::Intl {
            "AutoClaw 国际版的网页登录需要先完成一次风控验证，请回到「添加账号」弹窗，             用「网页登录（Zai / Google）」发起"
        } else {
            // 国内版确实没有 OAuth（上游 oauth-captcha-config 返回 enabled:false），
            // 它的官方唯一登录方式是手机验证码 —— 文案要指向那条
            "AutoClaw 国内版没有网页授权登录，请改用「手机验证码登录」或「填写凭证」"
        };
        return management_error(400, message);
    }
    // 注册表是 provider 白名单的唯一事实来源（见 providers::kind_from_id 的说明）
    // 旧入口（没有 methodIndex）也可以带登录代理：内置家与旧插件的登录链
    // 都要出网（antigravity 尤其 —— 无头服务器上只有应用内代理能到 Google）。
    // 这里解析一次，随 options 下发；插件按 `payload.proxy` 收（见 antigravity
    // `login::resolve_login_proxy`），旧插件忽略这个键、行为不变。
    let legacy_proxy = match checked_login_proxy(
        payload.as_ref().and_then(|value| value.get("proxy")),
    ) {
        Ok(proxy) => proxy,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let legacy_options = match &legacy_proxy {
        Some(selection) => json!({ "proxy": selection.resolved.clone() }),
        None => json!({}),
    };
    let kind = match provider {
        "" if crate::server::core::plugin_host::serves_provider("workbuddy") => {
            return start_plugin_web_login(
                state,
                "workbuddy".to_string(),
                legacy_selected("workbuddy", legacy_proxy, legacy_options),
            )
            .await;
        }
        "" => crate::server::core::providers::ProviderKind::WorkBuddy,
        other => match crate::server::core::providers::kind_from_id(other) {
            // 已被插件接管的内置家：走插件那条路。下面那些分支是**内置实现**
            // 的登录链，插件接管的同一 id 若从这里进，会拿内置的 OAuth 流程
            // 换一个插件并不认识的会话（症状是「登录成功但账号不能用」）。
            Some(_) if crate::server::core::plugin_host::serves_provider(other) => {
                return start_plugin_web_login(
                    state,
                    other.to_string(),
                    legacy_selected(other, legacy_proxy, legacy_options),
                )
                .await;
            }
            Some(kind) => kind,
            // 插件家的 id 不在枚举里（第三方 provider 没有 `ProviderKind`）。
            // 它们走 `start_plugin_web_login`：`build_login_url_async` 发一次
            // `login.start` RPC，拿到授权地址与 state 后与内置家同一条收尾链。
            None => {
                if crate::server::core::providers::adapter::adapter_by_id(other).is_none() {
                    return management_error(400, format!("未知的提供商：{other}"));
                }
                return start_plugin_web_login(
                    state,
                    other.to_string(),
                    legacy_selected(other, legacy_proxy, legacy_options),
                )
                .await;
            }
        },
    };
    // 走到这里说明：这是注册表里的 provider，但此刻没有插件接管它 ——
    // 小浣熊 / CatPaw / Qoder / ZCode / WorkBuddy / Trae / OfficeAce / AtomCode /
    // Cline / AutoClaw 等都属这一类。历史上这里按家分流到各自的
    // `start_web_login`，那套实现已逐家搬空。
    // 内置实现已经一家不剩（`implemented_kinds()` 为空），所以剩下的每种
    // 情况都只有「没装插件」这一个解释。直接说清楚，而不是回一句「这家不支持
    // 网页登录」—— 后者会让用户去翻凭证格式，而真正该做的是装插件。
    management_error(
        503,
        format!(
            "{}的网页登录需要安装提供商插件：本版本不再内置该家的登录实现",
            crate::server::core::providers::meta(kind).label
        ),
    )
}

/// 插件家的网页登录分支。
///
/// 网关唯一一条网页登录发起链：授权地址由**插件进程**生成（`login.start`
/// RPC），任务句柄仍由网关持有 —— 回调进来时 `submit_login_callback` 按任务上
/// 记的 provider id 找插件，走 `login.callback` 收尾。
///
/// `poll_interval_ms > 0` 表示网关负责轮询；回调可能在上游，也可能在
/// provider 自己绑定的 localhost 端口。manualCallback 独立描述是否需要粘贴。
///
/// 丢掉这个位的后果是「地址出来了、界面一直挂着等一个永远不会到的跳转」——
/// 没有任何报错，是最难查的一类失败。所以**必须**用 `login_start_async`
/// 拿完整的 `LoginStart`，不能用只回 `(url, state)` 的那个。
async fn start_plugin_web_login(
    state: ServerState,
    provider: String,
    selected: Option<SelectedLogin>,
) -> Response {
    let selected = match selected {
        Some(selected) => selected,
        None => {
            let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(&provider) else {
                return management_error(400, format!("未知的提供商：{provider}"));
            };
            if !adapter.supports_web_login() {
                return management_error(400, format!(
                    "{}不支持网页登录，请改用「填写凭证」或「导入桌面端登录态」添加账号", adapter.label(),
                ));
            }
            SelectedLogin {
                adapter, proxy: None, options: json!({}), local_only: false,
                manual_callback: false, callback_input: LoginCallbackInput::Url, instructions: None,
            }
        }
    };
    let SelectedLogin { adapter, proxy, options, local_only, manual_callback, callback_input, instructions } = selected;
    let label = adapter.label();
    let method_index = options.get("methodIndex").and_then(Value::as_u64);
    let mut start = match adapter.build_login_flow_with_options(options).await {
        Ok(Some(start)) => start,
        Ok(None) => return management_error(400, format!("{label}未能生成登录流程，请重试")),
        Err(error) => return management_error(error.status_code, error.message),
    };
    if start.flow.trim().is_empty() {
        return management_error(502, "插件没有返回登录流程 id");
    }
    start.instructions = crate::server::core::login::login_text(&start.instructions);
    if start.instructions.is_empty() { start.instructions = instructions.unwrap_or_default(); }
    // 本机 / 自动登录也必须轮询等 Done；空 URL 只表示不需要打开授权页。
    if start.url.trim().is_empty() { start.url.clear(); }
    if (start.url.is_empty() || local_only) && start.poll_interval_ms == 0 {
        start.poll_interval_ms = 1_000;
    }
    // 版本标签只有 WorkBuddy 系有意义（`workbuddy` → cn / `workbuddy-intl` → intl），
    // 别家传空串。判据按 provider id 反查地区，不猜 —— 猜错会让国际版账号
    // 的登录完成摘要显示成国内版。
    let edition = crate::server::core::providers::workbuddy::Region::from_provider_id(&provider)
        .map(|region| region.id())
        .unwrap_or("");
    let handle = state.login().new_handle_for_adapter(edition, adapter.clone(), proxy);
    handle.update(|task| {
        task.state = Some(start.flow.clone());
        task.auth_url = Some(start.url.clone());
        task.instructions = Some(start.instructions.clone());
        task.method_index = method_index;
        task.polling = start.poll_interval_ms > 0;
    });
    state.login().tasks().register(&start.flow, handle.clone());
    // 轮询型：起后台任务问上游「授权完了没」，完成后落账号并写回任务状态
    if start.poll_interval_ms > 0 {
        spawn_plugin_login_poll(
            handle.clone(),
            start.flow.clone(),
            start.poll_interval_ms,
            provider.clone(),
        );
        logging::log(
            "[Login]",
            &format!(
                "发起{label}网页登录（轮询型：每 {}ms 问一次上游）",
                start.poll_interval_ms
            ),
        );
    } else {
        logging::log("[Login]", &format!("发起{label}网页登录（等待浏览器回调…）"));
        // 没有轮询 consumer 的回调型任务也要超时并清理提供商侧 session。
        let login = state.login().clone();
        let flow = start.flow.clone();
        crate::spawn_task(async move {
            tokio::time::sleep(Duration::from_millis(LOGIN_TIMEOUT_MS)).await;
            login.cancel_async(&flow).await;
        });
    }
    let mut response = login_start_response(&start, &provider, edition, manual_callback, callback_input);
    if let Some(index) = method_index { response["methodIndex"] = json!(index); }
    if method_index.is_some() { response["localOnly"] = json!(local_only); }
    // 所有返回字段均来自已校验的元数据与 LoginStart，不把 options / inputs 回显。
    ok_json(response)
}

/// 轮询型插件登录的后台任务。
///
/// ── 为什么不在请求里等 ──────────────────────────────────────
/// 轮询要跑几分钟（用户在浏览器里操作）。阻塞在 `/start` 会让那次 HTTP 请求
/// 挂到超时，而前端拿不到 state 就无法 `/wait`。
///
/// ── 失败语义 ────────────────────────────────────────────────
///   - `Failed` → 落定任务错误（前端 `/wait` 立刻拿到）；
///   - 传输层错误 → 也落定错误。与「设备授权」那套不同：这里一次问不成就
///     说明代理不可达，继续等只会把一个网络故障伪装成「用户还没操作」；
///   - `Pending` → 继续，直到 `LOGIN_TIMEOUT_MS`。
///
/// 任务被取消（`/cancel`）时循环下一拍就退出（`cancel` 会把任务从表里移除，
/// 但句柄还在，所以这里查的是句柄上的 `canceled` 标记）。
pub(crate) fn spawn_plugin_login_poll(
    handle: crate::server::core::login::LoginTaskHandle,
    flow: String,
    interval_ms: u64,
    provider: String,
) {
    crate::spawn_task(async move {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(crate::server::core::login::LOGIN_TIMEOUT_MS);
        let interval = Duration::from_millis(interval_ms.clamp(1_000, LOGIN_TIMEOUT_MS));
        loop {
            tokio::time::sleep(interval).await;
            let _operation = handle.operation.lock().await;
            let task = handle.snapshot();
            if task.canceled || task.done {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                crate::server::core::login::fail_plugin_login(
                    &handle,
                    "网页登录超时，请重新发起",
                );
                handle.cancel_provider_flow_locked(&flow).await;
                return;
            }
            let Some(adapter) = handle.adapter() else {
                crate::server::core::login::fail_plugin_login(&handle, "登录任务未绑定执行体，请重新发起");
                return;
            };
            let poll = adapter.login_poll_async(&flow).await;
            // 请求在 await 中被取消或由回调落定：迟到的 Done 不得再落库。
            let task = handle.snapshot();
            if task.canceled || task.done { return; }
            match poll {
                Ok(agent2api_provider_protocol::LoginPoll::Done { account }) => {
                    match crate::server::core::login::persist_plugin_login(&handle, adapter.as_ref(), &account) {
                        Ok(Some(_)) => logging::log("[Login]", &format!("{provider} 登录完成，账号已加入列表")),
                        Ok(None) => {}
                        Err(error) => crate::server::core::login::fail_plugin_login(&handle, &error.message),
                    }
                    handle.cancel_provider_flow_locked(&flow).await;
                    return;
                }
                Ok(agent2api_provider_protocol::LoginPoll::Pending { hint }) => {
                    crate::server::core::login::set_plugin_login_hint(&handle, hint.as_deref());
                }
                Ok(agent2api_provider_protocol::LoginPoll::Failed { message }) => {
                    crate::server::core::login::fail_plugin_login(&handle, &message);
                    handle.cancel_provider_flow_locked(&flow).await;
                    return;
                }
                Err(error) => {
                    crate::server::core::login::fail_plugin_login(&handle, &error.message);
                    handle.cancel_provider_flow_locked(&flow).await;
                    return;
                }
            }
        }
    });
}


// ─── GET /api/session/login/wait ────────────────────────────

/// 轮询登录结果（三种响应：pending / done+error / done+session）
pub async fn login_wait(
    State(state): State<ServerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let task_state = params.get("state").cloned().unwrap_or_default();
    let Some(task) = state.login().tasks().get(&task_state) else {
        return management_error(404, "登录任务不存在或已过期");
    };
    ok_json(task.snapshot().to_wait_response())
}

// ─── POST /api/session/login/cancel ─────────────────────────

pub async fn login_cancel(State(state): State<ServerState>, body: Bytes) -> Response {
    let task_state = parse_body(&body)
        .ok()
        .and_then(|payload| {
            payload
                .get("state")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    // 走 LoginService 那一层（而不是 `tasks().cancel`）：AutoClaw 那条链借来的
    // 回调端口挂在它的待办表上，取消时得一起释放（见 `LoginService::cancel`）
    let canceled = state.login().cancel_async(&task_state).await;
    ok_json(json!({ "canceled": canceled }))
}

// ─── POST /api/session/login/callback ───────────────────────

/// 提交网页登录的回调 URL（**壳侧登录窗口专用**）。
///
/// ── 什么样的家需要它 ────────────────────────────────────────
/// 回调落在一个**壳或浏览器抓得住、网关自己收不到**的地方时，由这一侧的调用方
/// 把原文转进来。两种来源：
///   - 壳侧登录窗口：`on_navigation` 认出回调 URL、拦下导航、把原文 POST 进来
///     （自定义协议如 `office-raccoon://`、`orcaterm://` 只能这样）；
///   - 网页端：用户在弹窗里手工粘贴地址栏里的整条 URL（`web_shim.rs` 的
///     `needsManualCallback`）。
///
/// ── 网关不认识任何一家的回调形状 ──────────────────────────────
/// 本函数只做「按 state 找回登录任务 → 把整条 URL 交给**服务这一家的插件**」。
/// `scheme / host / path / 查询串里哪个键是授权码` 全在插件里（`login.callback`
/// 收到的是原样 URL 与 `code` 两份），所以新增一家不需要改这里一行代码。
///
/// ── 校验与失败语义 ──────────────────────────────────────────
/// body `{state, callbackUrl}`：
///   - state 必须对应一个**进行中**的登录任务（404 表示没有这个任务，前端应重新发起）；
///   - 插件负责 state 逐字比对与一次性换码；失败要落定任务错误，
///     否则前端会一直等到 5 分钟超时。
pub async fn login_callback(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    let task_state = payload
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let callback_url = payload
        .get("callbackUrl")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if callback_url.trim().is_empty() {
        return management_error(400, "缺少 callbackUrl（登录回调地址）");
    }
    // 标准错误在旧 AutoClaw 特判前统一落定，不把敏感描述送进插件/页面。
    if let Some(error) = url::Url::parse(&callback_url).ok().and_then(|url| {
        url.query_pairs().find(|(key, value)| key == "error" && !value.trim().is_empty())
            .map(|(_, error)| error.into_owned())
    }) {
        let error = state.login().reject_login_callback(&task_state, &error).await;
        return management_error(error.status_code, error.message);
    }
    // ── AutoClaw 的网页登录是**轮询型**（回调落在插件自己的登记端口）──
    // `submit_login_callback` 那条路只服务「网关等回调」的回调型家（它会把
    // 一次性 code 交给插件的 `login.callback`）。AutoClaw 的流程活在插件的
    // 流程表里，没有 `login.callback` 语义 —— 用户粘贴的地址要**投递进那个
    // 流程**（`login.oauth-callback` 管理动作），后续仍由 `login.poll` 收尾。
    if let Some(task) = state.login().tasks().get(&task_state) {
        let snapshot = task.snapshot();
        if snapshot.canceled { return management_error(400, "登录已取消"); }
        if snapshot.done {
            return match snapshot.error {
                Some(error) => management_error(400, error),
                None => ok_json(json!({ "accountId": snapshot.session.as_ref().and_then(|session| session.get("accountUid")) })),
            };
        }
        if snapshot.method_index.is_none()
            && crate::server::core::providers::autoclaw::Region::from_provider_id(&snapshot.provider).is_some()
        {
            let Some(adapter) = task.adapter() else {
                return management_error(503, "AutoClaw 登录任务未绑定执行体，请重新发起");
            };
            let _operation = task.operation.lock().await;
            let current = task.snapshot();
            if current.canceled { return management_error(400, "登录已取消"); }
            if current.done {
                return match current.error {
                    Some(error) => management_error(400, error),
                    None => ok_json(json!({ "accountId": current.session.as_ref().and_then(|session| session.get("accountUid")) })),
                };
            }
            return match adapter
                .admin_action(
                    state.store(),
                    "",
                    "login.oauth-callback",
                    &json!({ "state": task_state, "url": callback_url }),
                )
                .await
            {
                Ok(_) => ok_json(json!({ "accepted": true })),
                Err(error) => management_error(error.status_code, error.message),
            };
        }
    }
    match state
        .login()
        .submit_login_callback(&task_state, &callback_url)
        .await
    {
        Ok(crate::server::core::login::LoginCallbackSubmission::Completed(account_id)) => {
            ok_json(json!({ "accountId": account_id }))
        }
        Ok(crate::server::core::login::LoginCallbackSubmission::Accepted) => {
            ok_json(json!({ "accepted": true, "continue": true }))
        }
        Ok(crate::server::core::login::LoginCallbackSubmission::ContinueTo(next_url)) => {
            ok_json(json!({ "continue": true, "nextUrl": next_url }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

// ─── GET /api/session/login/remote-callback ────────────────

/// **远程网页登录回调中转**（浏览器能到网关、但授权页只能把授权码带回一个
/// 普通 HTTP 地址时用）。
///
/// ── 谁在用 ──────────────────────────────────────────────────
/// 网页端（`web_shim.rs`）：内嵌窗口拿不到自定义协议，网页 shim 改走授权页的
/// `redirect` 分支，把 `authorization_code` 追加到面板自己的地址上导航回来。
/// 面板地址是浏览器与网关**都能到达**的那一个，所以由网关收下这一次跳转。
///
/// ── 为什么路径是通用的 ──────────────────────────────────────
/// 回调里带什么参数（`code` / `authorization_code`）、要还原成什么协议的形态，
/// 都是**那一家的上游知识**，已经随实现搬到插件。网关这边只做三件事：
///   ① 从 query 认出一次性授权码（只认这两个标准 OAuth 参数名）；
///   ② 把 code 与已登记的 state 交给 `submit_login_callback`；完整 URL 由 POST 入口传入；
///   ③ 回一张人看的 HTML 页（用户此刻正盯着浏览器）。
///
/// 该路由必须免鉴权：调用方是授权页，不会携带面板 API Key；安全性由本次
/// 登录任务生成的不可预测 state 承担（插件侧再逐字比对一次）。
pub async fn login_remote_callback(
    State(state): State<ServerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let task_state = params.get("state").cloned().unwrap_or_default();
    if let Some(error) = params.get("error").filter(|value| !value.trim().is_empty()) {
        let error = state.login().reject_login_callback(&task_state, error).await;
        return oauth_callback_page(error.status_code, &error.message);
    }
    let code = params
        .get("code")
        .or_else(|| params.get("authorization_code"))
        .cloned()
        .unwrap_or_default();
    drop(params);
    if task_state.trim().is_empty() || code.trim().is_empty() {
        return oauth_callback_page(
            400,
            "登录失败：回调缺少 state 或授权码，请重新发起网页登录。",
        );
    }
    // 这是既有的「只交一次性 code」中转，不是 provider 的真实 callback URL。
    // 不伪造 localhost URL：旧插件会校验真实 scheme/host/path（例如小浣熊），
    // 合成一个占位 URL 再保留它反而会破坏旧登录。完整 URL 走 POST /callback。
    match state
        .login()
        .submit_login_callback(&task_state, &code)
        .await
    {
        Ok(crate::server::core::login::LoginCallbackSubmission::Completed(_)) => {
            oauth_callback_page(200, "登录成功，已返回网关，可以关闭此页面。")
        }
        Ok(crate::server::core::login::LoginCallbackSubmission::Accepted) => {
            oauth_callback_page(200, "回调已接收，登录仍在处理中，请返回面板等待。")
        }
        Ok(crate::server::core::login::LoginCallbackSubmission::ContinueTo(_)) => {
            oauth_callback_page(400, "登录回调仍需继续，请重新发起网页登录。")
        }
        Err(error) => {
            oauth_callback_page(error.status_code, &format!("登录失败：{}", error.message))
        }
    }
}

// ─── POST /api/session/login/sms/send 与 /verify ────────────

/// 从请求体里读 AutoClaw 地区（`provider` 字段，与 `POST /api/accounts` 同名）。
///
/// 缺省（字段没带 / 空串）落**国内版**：与 provider id 的历史口径一致 ——
/// 老客户端不带这个字段，而那些用户本来就在用国内版。
///
/// ── 带值却不认识：明确拒绝（issue #93 的教训）──────────────────
/// 原来是「未知值也落国内版」，代价是**跨 provider 的误投完全无声**：另一个家
/// 的 id（如 Loomy）发到这条端点时，号码被发去 AutoClaw 的站点、账号也存成
/// AutoClaw，而调用方拿到的是**成功响应**。因此只有「没带」才按历史口径回落；
/// 带了不认识的值就报出来。
fn autoclaw_region_of(
    payload: &Value,
) -> Result<crate::server::core::providers::autoclaw::Region, Response> {
    use crate::server::core::providers::autoclaw::Region;
    let raw = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if raw.is_empty() {
        return Ok(Region::Cn);
    }
    match Region::from_provider_id(raw) {
        Some(region) => Ok(region),
        None => {
            let known = Region::ALL
                .into_iter()
                .map(Region::provider_id)
                .collect::<Vec<_>>()
                .join(" / ");
            Err(management_error(
                400,
                format!("未知的登录地区「{raw}」（只支持 {known}）"),
            ))
        }
    }
}

/// 取 AutoClaw 插件适配器（没装插件时给出可操作的文案）。
fn autoclaw_adapter(
    region: crate::server::core::providers::autoclaw::Region,
) -> Result<std::sync::Arc<dyn crate::server::core::providers::adapter::ProviderAdapter>, Response> {
    let provider = region.provider_id();
    match crate::server::core::providers::adapter::adapter_by_id(provider) {
        Some(adapter) => Ok(adapter),
        None => Err(management_error(
            503,
            format!(
                "AutoClaw {} 的登录需要安装提供商插件：本版本不再携带该家的登录实现",
                region.label()
            ),
        )),
    }
}

/// 发送短信验证码（**AutoClaw 国内版专用**，手机号验证码登录的第一步）。
///
/// ── 为什么这不是「网页登录」──────────────────────────────────
/// AutoClaw 国内版没有网页授权：桌面端没有公网 Web 应用、账号体系里没有授权码，
/// 它的唯一入口是手机号 + 验证码（国际版反过来，只有 OAuth）。因此这里既不开
/// 窗口也不起任务，就是**一次同步的上游调用**（插件侧 `login.sms-send`）。
///
/// body `{phone, provider?}` → `{deviceId}`。
///
/// ── 为什么把 deviceId 回给前端 ──────────────────────────────
/// 上游把「发的这个码」绑在发码时的 device_id 上，登录必须带同一个 ——
/// 但网关不替用户保存这个中间态（一次登录可以跨多次 HTTP 请求、也可以被用户
/// 放弃，存在服务端只会多一份要清理的状态）。回给前端让它随下一次请求带回。
pub async fn login_sms_send(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    // 兼容旧缓存前端：它把 `provider: tabbit` 发到 AutoClaw 的端点。
    if payload.get("provider").and_then(Value::as_str) == Some("tabbit") {
        return tabbit_sms_send_inner(&state, &payload).await;
    }
    let phone = payload.get("phone").and_then(Value::as_str).unwrap_or("");
    let region = match autoclaw_region_of(&payload) {
        Ok(region) => region,
        Err(response) => return response,
    };
    let adapter = match autoclaw_adapter(region) {
        Ok(adapter) => adapter,
        Err(response) => return response,
    };
    // 发码不需要账号（还没登录），`account` 传空串；插件那条动作不读账号库。
    match adapter
        .admin_action(state.store(), "", "login.sms-send", &json!({ "phone": phone }))
        .await
    {
        Ok(result) => ok_json(result),
        Err(error) => management_error(error.status_code, error.message),
    }
}

/// 用手机号 + 验证码登录并**直接落成账号**（**AutoClaw 国内版专用**）。
///
/// body `{phone, code, deviceId?, name?, provider?}` → `{account, list}` ——
/// 响应形状与 `POST /api/accounts` **逐字一致**：登录只是另一种拿到凭证的方式，
/// 落盘、命名、去重、优先级分配全部复用插件那条落库通路。
pub async fn login_sms_verify(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    // 兼容旧缓存前端：它把 `provider: tabbit` 发到 AutoClaw 的端点。
    if payload.get("provider").and_then(Value::as_str) == Some("tabbit") {
        return tabbit_sms_verify_inner(&state, &payload).await;
    }
    let phone = payload.get("phone").and_then(Value::as_str).unwrap_or("");
    let code = payload.get("code").and_then(Value::as_str).unwrap_or("");
    let device_id = payload.get("deviceId").and_then(Value::as_str);
    let name = payload.get("name").and_then(Value::as_str);
    let region = match autoclaw_region_of(&payload) {
        Ok(region) => region,
        Err(response) => return response,
    };
    let adapter = match autoclaw_adapter(region) {
        Ok(adapter) => adapter,
        Err(response) => return response,
    };
    let account = match adapter
        .admin_action(
            state.store(),
            "",
            "login.sms-verify",
            &json!({ "phone": phone, "code": code, "deviceId": device_id, "name": name }),
        )
        .await
    {
        Ok(value) => match serde_json::from_value::<agent2api_provider_protocol::NormalizedAccount>(value)
        {
            Ok(account) => account,
            Err(error) => {
                return management_error(502, format!("插件返回的登录结果无法解析：{error}"))
            }
        },
        Err(error) => return management_error(error.status_code, error.message),
    };
    match adapter.persist_login_account(&account) {
        Ok(account_id) => {
            let label = account
                .public
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("AutoClaw 账号");
            logging::log(
                "[Login]",
                &format!("✅ AutoClaw {}登录成功: {label}", region.label()),
            );
            let list = state.store().list_visible_accounts();
            let account = list
                .get("accounts")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(&account_id))
                })
                .cloned()
                .unwrap_or(account.public);
            ok_json(json!({ "account": account, "list": list }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

// ─── Loomy 手机号验证码登录（两段）─────────────────────────

/// 发送短信验证码（**Loomy 专用**，手机号验证码登录的第一步）。
///
/// ── 为什么这不是「网页登录」──────────────────────────────────
/// Loomy 的账号体系（CAccount）没有网页授权码那套 —— 官方客户端唯一的自助
/// 入口就是手机号 + 验证码（微信扫码那条需要讯飞侧登记的回调域名，网关复刻
/// 不了，见 `providers/loomy/login.rs` 的模块头）。因此这里既不开窗口也不起
/// 后台任务，就是**一次同步的上游调用**（带 HMAC-SHA1 签名头）。
///
/// body `{phone}` → `{msgid}`。
///
/// ── 为什么把 msgid 回给前端（与 AutoClaw 的 deviceId 同款）──
/// 上游把「发的这个码」绑在发码响应的 msgid 上，登录必须带同一个 ——
/// 但网关不替用户保存这个中间态（一次登录可以跨多次 HTTP 请求、也可以被
/// 放弃，存在服务端只会多一份要清理的状态）。
pub async fn login_loomy_sms_send(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    let phone = payload.get("phone").and_then(Value::as_str).unwrap_or("");
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id("loomy") else {
        return management_error(400, "Loomy 的提供商插件没有安装，无法发送验证码");
    };
    // 发码不需要账号（还没登录），`account` 传空串；`store` 也只是占位
    // （插件的这条动作不读账号库）。
    match adapter
        .admin_action(state.store(), "", "login.sms-send", &json!({ "phone": phone }))
        .await
    {
        Ok(result) => ok_json(result),
        Err(error) => management_error(error.status_code, error.message),
    }
}

/// 用手机号 + 验证码登录并**直接落成账号**（**Loomy 专用**）。
///
/// body `{phone, code, msgid, name?}` → `{account, list}` —— 响应形状与
/// `POST /api/accounts` **逐字一致**：登录只是另一种拿到凭证的方式，
/// 账号保存与去重复用插件登录的落库通路，前端可以直接交给「已添加账号」收尾逻辑。
pub async fn login_loomy_sms_verify(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    let phone = payload.get("phone").and_then(Value::as_str).unwrap_or("");
    let code = payload.get("code").and_then(Value::as_str).unwrap_or("");
    let msgid = payload.get("msgid").and_then(Value::as_str);
    let Some(adapter) =
        crate::server::core::providers::adapter::adapter_by_id("loomy")
    else {
        return management_error(400, "Loomy 的提供商插件没有安装，无法登录");
    };
    let mut account = match adapter
        .admin_action(
            state.store(),
            "",
            "login.sms-verify",
            &json!({ "phone": phone, "code": code, "msgid": msgid }),
        )
        .await
    {
        Ok(value) => match serde_json::from_value::<agent2api_provider_protocol::NormalizedAccount>(value)
        {
            Ok(account) => account,
            Err(error) => {
                return management_error(502, format!("插件返回的登录结果无法解析：{error}"))
            }
        },
        Err(error) => return management_error(error.status_code, error.message),
    };
    if let Some(name) = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(public) = account.public.as_object_mut() {
            public.insert(
                "name".to_string(),
                Value::String(name.chars().take(100).collect()),
            );
        }
    }
    match adapter.persist_login_account(&account) {
        Ok(account_id) => {
            let label = account
                .public
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Loomy 账号");
            logging::log("[Login]", &format!("✅ Loomy 登录成功: {label}"));
            let list = state.store().list_visible_accounts();
            let account = list
                .get("accounts")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(&account_id))
                })
                .cloned()
                .unwrap_or(account.public);
            ok_json(json!({ "account": account, "list": list }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

// ─── Tabbit 国内站手机号验证码登录（两段）───────────────────

/// 发送短信验证码（**Tabbit 国内站专用**）。
///
/// body `{phone, uuid?, supportsCaptcha?}` → `{uuid}`。uuid 是这次登录的会话
/// 标识：上游把刚发的码绑在它上面，登录时必须带同一个；网关不保存这个中间态，
/// 回给前端随下一次请求带回即可。
///
/// ── 第二种成功形态：`{status:"captcha", captcha:{requestCode, verifyUrl}}` ──
/// 风控判定人机可疑时上游不会发码，而是给一个滑块验证页。用户在那边手动完成
/// 后**重新发一次码**才成功（官方网页版同样如此，见插件的 `challenge_of`）。
/// 声明了 `supportsCaptcha` 的界面会拿到这个 200 + 状态，自己去开验证页并
/// 引导用户再点一次；没声明的（旧缓存前端）收到 409 与一句可读提示。
pub async fn login_tabbit_sms_send(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    tabbit_sms_send_inner(&state, &payload).await
}

/// 实际发码逻辑；也供旧版 `/api/session/login/sms/send` 按 `provider` 分派
/// （旧缓存的前端会把 `provider: tabbit` 发到 AutoClaw 的端点）。
async fn tabbit_sms_send_inner(state: &ServerState, payload: &Value) -> Response {
    let phone = payload.get("phone").and_then(Value::as_str).unwrap_or("");
    // 过完滑块重发时要沿用同一个 uuid（同一轮登录），前端会带回来。
    let uuid = payload.get("uuid").and_then(Value::as_str);
    // 新界面声明自己认识「发码前要先过滑块」这种结果；不声明的一律按错误回，
    // 免得旧缓存前端把它当成「已发送」而让用户等一条不会来的短信。
    let supports_captcha = payload
        .get("supportsCaptcha")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(adapter) =
        crate::server::core::providers::adapter::adapter_by_id("tabbit")
    else {
        return management_error(503, "Tabbit 的提供商插件没有安装，无法发送验证码");
    };
    match adapter
        .admin_action(
            state.store(),
            "",
            "login.sms-send",
            &json!({ "phone": phone, "uuid": uuid }),
        )
        .await
    {
        Ok(result) => {
            let needs_captcha =
                result.get("status").and_then(Value::as_str) == Some("captcha");
            if needs_captcha && !supports_captcha {
                return management_error(
                    409,
                    "发送验证码前需要先完成滑块验证。请刷新页面后再试，或改用「填写凭证」粘贴 Cookie",
                );
            }
            ok_json(result)
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

/// 用手机号 + 验证码登录并**直接落成账号**（**Tabbit 国内站专用**）。
///
/// body `{phone, code, uuid, name?}` → `{account, list}`，响应形状与
/// `POST /api/accounts` 一致；落盘、命名、去重复用插件的落库通路。
pub async fn login_tabbit_sms_verify(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    tabbit_sms_verify_inner(&state, &payload).await
}

/// 实际换登录态逻辑；也供旧版 `/api/session/login/sms/verify` 按 `provider`
/// 分派（旧缓存前端把 `provider: tabbit` 发到 AutoClaw 端点时仍能登录）。
async fn tabbit_sms_verify_inner(state: &ServerState, payload: &Value) -> Response {
    let phone = payload.get("phone").and_then(Value::as_str).unwrap_or("");
    let code = payload.get("code").and_then(Value::as_str).unwrap_or("");
    // 新版前端传 `uuid`；旧缓存前端（AutoClaw 档案）把同一个中间态叫
    // `deviceId`，两个都认，避免旧页面必须强刷才能登录。
    let uuid = payload
        .get("uuid")
        .or_else(|| payload.get("deviceId"))
        .and_then(Value::as_str);
    let Some(adapter) =
        crate::server::core::providers::adapter::adapter_by_id("tabbit")
    else {
        return management_error(503, "Tabbit 的提供商插件没有安装，无法登录");
    };
    let mut account = match adapter
        .admin_action(
            state.store(),
            "",
            "login.sms-verify",
            &json!({
                "phone": phone,
                "code": code,
                "uuid": uuid,
                "name": payload.get("name"),
            }),
        )
        .await
    {
        Ok(value) => {
            match serde_json::from_value::<agent2api_provider_protocol::NormalizedAccount>(value) {
                Ok(account) => account,
                Err(error) => {
                    return management_error(502, format!("插件返回的登录结果无法解析：{error}"))
                }
            }
        }
        Err(error) => return management_error(error.status_code, error.message),
    };
    if let Some(name) = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(public) = account.public.as_object_mut() {
            public.insert(
                "name".to_string(),
                Value::String(name.chars().take(100).collect()),
            );
        }
    }
    match adapter.persist_login_account(&account) {
        Ok(account_id) => {
            let label = account
                .public
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Tabbit 账号");
            logging::log("[Login]", &format!("✅ Tabbit 登录成功: {label}"));
            let list = state.store().list_visible_accounts();
            let account = list
                .get("accounts")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(&account_id))
                })
                .cloned()
                .unwrap_or(account.public);
            ok_json(json!({ "account": account, "list": list }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

// ─── KukuAI 短信登录收尾（百度通行证）──────────────────────────

/// `POST /api/session/login/kuku/complete` —— 壳侧交回登录态。
///
/// 百度通行证登录成功后的跳转**不携带授权码**（凭证在 `.baidu.com` 域的
/// Cookie 里，跨域跳转不会带过去），而 `BDUSS` 是 **HttpOnly** Cookie，
/// 登录窗口的页面脚本 `document.cookie` 读不到 —— 因此交回由**壳侧进程**
/// 完成：登录窗口跳转 `success.html` 后，壳用 WebView2 的 Cookie 存储
/// （`cookies()` 含 HttpOnly）读出 `BDUSS` / `STOKEN`，拼成 Cookie 头
/// POST 到这里（`state` + `cookie` 在请求体里，不走 URL）。
///
/// 该路由**免鉴权**：调用方是壳侧进程（登录窗口），不带 API Key；安全性由
/// 登录任务的一次性 state 承担（与远程回调中转同口径）。
pub async fn login_kuku_complete(
    State(state): State<ServerState>,
    Json(payload): Json<Value>,
) -> Response {
    let task_state = payload
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let cookie = payload
        .get("cookie")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if task_state.is_empty() || cookie.is_empty() {
        return management_error(400, "登录失败：回调缺少 state 或登录态 Cookie，请重新发起短信登录。");
    }
    // 收尾走**插件**那条通用回调链：Cookie 作为一次性 `code` 交给插件，
    // 插件自己解析（`BDUSS` / `STOKEN` / 其余百度域 Cookie）、打 `userreport`
    // 复核、产出账号 —— 网关这边不认识任何百度 Cookie 的名字。
    match state.login().submit_login_callback(task_state, cookie).await {
        Ok(crate::server::core::login::LoginCallbackSubmission::Completed(_)) => ok_json(json!({ "ok": true })),
        Ok(_) => ok_json(json!({ "accepted": true, "continue": true })),
        Err(error) => management_error(error.status_code, &format!("登录失败：{}", error.message)),
    }
}

// ─── OrcaTerm 网页登录收尾（兜底入口）──────────────────────────

/// `POST /api/session/login/orcaterm/complete` —— 直接交回一枚现成的令牌。
///
/// ── 正式链路不在这里（必读）─────────────────────────────────
/// 网页登录走**官方 OAuth**：窗口停在授权页 → 微信扫码 → 上游把浏览器带回
/// `orcaterm://oauth_callback?session_id=…` → 壳侧把回调 URL 交给
/// `POST /api/session/login/callback` → 后端 `OAuthExchangeToken` 换令牌
/// （见 `providers/orcaterm/login.rs` 模块头，那里也记录了「为什么不用控制台
/// 的 `ot_session`」：那枚 Cookie 在 SPA 内扫码时根本不会被写出来）。
///
/// 本端点留给「调用方手里已经有一枚令牌」的场景（壳侧若从别处读到、外部工具
/// 替我们拿到）：`{state, token}` POST 进来，同样做上游探活，通过才落账号。
///
/// 令牌形态兼容 `ot_session=…` 前缀（后端 `normalize_token` 会剥），
/// 因此字段既叫 `token` 也接受别名 `cookie`。
///
/// 该路由**免鉴权**：调用方是壳侧进程（登录窗口），不带 API Key；安全性由
/// 登录任务的一次性 state 承担（与 kuku/complete 同口径）。
pub async fn login_orcaterm_complete(
    State(state): State<ServerState>,
    Json(payload): Json<Value>,
) -> Response {
    let task_state = payload
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    // 字段叫 `token`：交回的是裸令牌（`ot_session=` 前缀由后端再剥一次，
    // 兼容壳侧原样传整段 Cookie 的形态）；`cookie` 作为别名一并接受。
    let token = payload
        .get("token")
        .or_else(|| payload.get("cookie"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if task_state.is_empty() || token.is_empty() {
        return management_error(
            400,
            "登录失败：回调缺少 state 或登录态令牌，请重新发起网页登录。",
        );
    }
    match state.login().finish_orcaterm_login(task_state, token).await {
        Ok(_) => ok_json(json!({ "ok": true })),
        Err(error) => management_error(error.status_code, &format!("登录失败：{}", error.message)),
    }
}

// ─── AutoClaw OAuth 网页登录（国际版，三段）──────────────────

/// 从请求体里读 OAuth 变体（`vendor` 字段：`zai` / `google`）。
///
/// 不认识的值一律 400：**不静默回落**到某一个变体 —— 那会让用户点 Google
/// 却打开 Zai 的授权页（两者是不同账号体系，登进去是个陌生账号）。
///
/// 变体清单由插件申报（`login.oauth-config` 的 `region` 段不列变体，因此这里
/// 只做**形态**校验：非空且是已知的两个字面量之一 —— 这是路由层的事实，
/// 不是上游知识）。
fn oauth_vendor_of(payload: &Value) -> Result<&'static str, Response> {
    let raw = payload
        .get("vendor")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    match raw {
        "zai" => Ok("zai"),
        "google" => Ok("google"),
        _ => Err(management_error(
            400,
            format!("未知的登录方式「{raw}」（只支持 zai / google）"),
        )),
    }
}

/// `POST /api/session/login/oauth/captcha-config` —— 取风控验证配置。
///
/// 前端据此初始化阿里云 SDK（`prefix` / `sceneId` 是 SDK 的必填参数）。
/// `?provider=` 决定问哪个地区；缺省国内版。
///
/// ── 为什么这条单独成一个端点（不塞进 start）──────────────────
/// 验证码要在**浏览器环境**里跑（SDK 是浏览器端 JS），而 SDK 初始化需要
/// `prefix` / `sceneId` —— 那两个值只能从上游拿。若把「取配置」与「取授权
/// 地址」合并成一个请求，就变成「网关先取配置、但它没法自己跑验证码」，
/// 前端仍然要再发一次请求带验证码回来。
///
/// `enabled: false`（国内版）不是错误：前端据此不渲染 OAuth 按钮。
pub async fn login_oauth_captcha_config(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    let region = match autoclaw_region_of(&payload) {
        Ok(region) => region,
        Err(response) => return response,
    };
    let adapter = match autoclaw_adapter(region) {
        Ok(adapter) => adapter,
        Err(response) => return response,
    };
    match adapter
        .admin_action(state.store(), "", "login.oauth-config", &Value::Null)
        .await
    {
        Ok(config) => ok_json(config),
        Err(error) => management_error(error.status_code, error.message),
    }
}

/// `POST /api/session/login/oauth/start` —— 带验证码参数发起 OAuth 登录。
///
/// body `{provider, vendor, captchaVerifyParam}` → `{state, authUrl, provider,
/// edition, pollIntervalMs, warning?}`（与另外几条登录链**同一个响应形状**，
/// 前端与壳侧的等待逻辑不必为新家分叉）。
///
/// ── 为什么这条不走壳侧的 `start_login` ────────────────────────
/// 壳侧那条命令的职责是「打开一个窗口去登录」，而这条的前提是「前端已经在
/// **主窗口里**跑完验证码 SDK 了」。把验证码参数从渲染层传给壳、再让壳回头调
/// 网关，等于绕一圈传一个不该由壳理解的不透明字符串。
///
/// ── 回调落在哪里（拆家后）─────────────────────────────────
/// 插件自己借 z.ai 登记的四个端口之一并**自己收下**回调（`web_login` +
/// `callback_server`）；插件报 `pollIntervalMs`，网关起轮询问 `login.poll`。
/// 网关这边**不再有** `/auth/callback-*` 路由 —— 它不再知道那四个端口的存在。
pub async fn login_oauth_start(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).unwrap_or(Value::Null);
    let region = match autoclaw_region_of(&payload) {
        Ok(region) => region,
        Err(response) => return response,
    };
    let vendor = match oauth_vendor_of(&payload) {
        Ok(vendor) => vendor,
        Err(response) => return response,
    };
    let captcha = payload
        .get("captchaVerifyParam")
        .and_then(Value::as_str)
        .unwrap_or("");
    let adapter = match autoclaw_adapter(region) {
        Ok(adapter) => adapter,
        Err(response) => return response,
    };
    if !adapter.supports_web_login() {
        return management_error(
            400,
            format!(
                "AutoClaw {}没有网页授权登录，请改用「填写凭证」或「导入桌面端登录态」",
                region.label()
            ),
        );
    }
    let started = match adapter
        .admin_action(
            state.store(),
            "",
            "login.oauth-start",
            &json!({ "vendor": vendor, "captchaVerifyParam": captcha }),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => return management_error(error.status_code, error.message),
    };
    let flow = started
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let auth_url = started
        .get("authUrl")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if flow.is_empty() || auth_url.is_empty() {
        return management_error(502, "插件没有返回授权地址或登录流程 id，请重试");
    }
    // 网关侧任务：`/wait` 按 state 查它，`pollIntervalMs > 0` 时起后台轮询。
    let handle = state.login().new_handle_for_adapter("", adapter.clone(), None);
    let interval = started
        .get("pollIntervalMs")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let start = agent2api_provider_protocol::LoginStart {
        flow: flow.clone(), url: auth_url.clone(),
        instructions: crate::server::core::login::login_text(started.get("instructions").and_then(Value::as_str).unwrap_or("")),
        poll_interval_ms: interval,
        external_browser: started.get("externalBrowser").and_then(Value::as_bool).unwrap_or(false),
        allowed_hosts: started.get("allowedHosts").and_then(Value::as_array)
            .map(|hosts| hosts.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
    };
    handle.update(|task| {
        task.state = Some(flow.clone());
        task.auth_url = Some(auth_url.clone());
        task.instructions = Some(start.instructions.clone());
        task.polling = interval > 0;
    });
    state.login().tasks().register(&flow, handle.clone());
    if interval > 0 {
        spawn_plugin_login_poll(
            handle,
            flow.clone(),
            interval,
            region.provider_id().to_string(),
        );
    } else {
        let login = state.login().clone();
        let flow = flow.clone();
        crate::spawn_task(async move {
            tokio::time::sleep(Duration::from_millis(LOGIN_TIMEOUT_MS)).await;
            login.cancel_async(&flow).await;
        });
    }
    logging::log(
        "[Login]",
        &format!(
            "发起 AutoClaw {} {}网页登录（插件侧回调，轮询 {}ms）",
            region.label(),
            vendor,
            interval
        ),
    );
    let mut body = login_start_response(&start, region.provider_id(), "intl", true, LoginCallbackInput::Url);
    if let Some(warning) = started.get("warning").and_then(Value::as_str) {
        if let Some(object) = body.as_object_mut() {
            object.insert("warning".to_string(), Value::String(warning.to_string()));
        }
    }
    ok_json(body)
}

/// OAuth 回调结果页（浏览器停在它上边，用户看到人话即可）。
///
/// 不带任何脚本与外链：这一页的内容我们完全控制，注入面越小越好。
/// 状态码可能是 410（登录上下文已丢失）这类非 200 —— 统一用 `from_u16`
/// 兜底成 200，避免一个非法状态码让响应构造失败。
fn oauth_callback_page(status: i32, message: &str) -> Response {
    use axum::response::IntoResponse;

    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let html = format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
         <title>登录回调</title></head>\
         <body style=\"font-family:system-ui,sans-serif;padding:48px;text-align:center\">\
         <p style=\"font-size:16px\">{escaped}</p></body></html>"
    );
    let status = u16::try_from(status)
        .ok()
        .and_then(|code| axum::http::StatusCode::from_u16(code).ok())
        .unwrap_or(axum::http::StatusCode::OK);
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

// ─── /auth/callback（LobsterAI 的 loopback 回调）─────────────

/// LobsterAI 网页登录的回调。**浏览器 302 到这里**（授权页完成后顶层导航回我们
/// 交给上游门户的 `redirect_uri`，查询串带 `code` / `state`）。
///
/// 路径是裸 `/auth/callback`（与别家带家名后缀的风格不一致）——理由见
/// `http.rs` 里那条路由的注释：`redirect_uri` 是我们给门户的，而门户是否对
/// 回调路径有白名单没有实测支撑，所以跟随参考实现那条已验证的形状。
///
/// 免鉴权的理由与 antigravity / lobsterai 几条 loopback 回调
/// 同一句：调用方是**用户的浏览器**，它当然没有我们的 API Key；安全性由本次
/// 登录任务生成的一次性 `state` 承担（见 `core::login::lobsterai`）。
pub async fn login_lobsterai_callback(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: axum::http::HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let task_state = params.get("state").cloned().unwrap_or_default();
    if let Some(task) = state.login().tasks().get(&task_state) {
        let snapshot = task.snapshot();
        if snapshot.provider != crate::server::core::providers::kind_id(crate::server::core::providers::ProviderKind::LobsterAi) {
            return oauth_callback_page(400, "这次登录不属于 LobsterAI");
        }
        if let Some(error) = params.get("error").filter(|value| !value.trim().is_empty()) {
            let error = state.login().reject_login_callback(&task_state, error).await;
            return oauth_callback_page(error.status_code, &error.message);
        }
        if task.adapter().is_some() {
            let callback_url = match original_callback_url(&uri, &headers) {
                Ok(url) => url,
                Err(error) => return oauth_callback_page(error.status_code, &error.message),
            };
            return login_callback_page(state.login().submit_login_callback(&task_state, &callback_url).await);
        }
    }
    if let Some(error) = params.get("error").filter(|value| !value.trim().is_empty()) {
        let error = state.login().reject_login_callback(&task_state, error).await;
        return oauth_callback_page(error.status_code, &error.message);
    }
    // 未绑定的旧强类型任务仍走专用收尾，不改变其解析与落库契约。
    let code = params.get("code").cloned().unwrap_or_default();
    match state.login().finish_lobsterai_login(&code, &task_state).await {
        Ok(_) => oauth_callback_page(200, "登录成功，已返回网关，可以关闭此页面。"),
        Err(error) => oauth_callback_page(error.status_code, &format!("登录失败：{}", error.message)),
    }
}

// ─── GET /oauth-callback（Antigravity 的 Google OAuth 登录回调）────

/// Antigravity 网页登录的回调（**浏览器 302 到这里**，查询串带 `code` / `state`）。
///
/// ── 路径为什么是 `/oauth-callback`（连字符）──────────────────
/// 它是发起授权时交给 Google 的 `redirect_uri` 的路径，参考实现
/// （`Antigravity-Manager/src-tauri/src/modules/oauth_server.rs`）逐字使用
/// `/oauth-callback`；Google 的 desktop 型 client 允许 loopback 任意端口 +
/// 这个路径。
///
/// ── 为什么免鉴权（挂 public 组）──────────────────────────────
/// 调用方是**用户的浏览器**（Google 授权页完成后顶层导航回来），它当然没有
/// 我们的 API Key。与 CatPaw / Accio 几条 loopback 回调同一取舍；
/// 安全性由本次登录任务生成的不可预测 state 承担（逐字比对见
/// `core::login::submit_login_callback` 的 Antigravity 分支）。
///
/// ── 与「手动粘贴回调 URL」共用同一收尾 ────────────────────────
/// 远程 / Docker 场景下浏览器可能到不了本机端口，用户会把地址栏整条 URL 粘
/// 回登录弹窗；那条入口同样落到 `submit_login_callback`（Accio 同款理由），
/// 本函数只负责把浏览器看到的结果渲染成人话。
pub async fn login_antigravity_callback(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: axum::http::HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let code = params.get("code").cloned().unwrap_or_default();
    let task_state = params.get("state").cloned().unwrap_or_default();
    if let Some(error) = params.get("error").filter(|value| !value.trim().is_empty()) {
        let error = state.login().reject_login_callback(&task_state, error).await;
        return oauth_callback_page(error.status_code, &error.message);
    }
    if task_state.trim().is_empty() || code.trim().is_empty() {
        return oauth_callback_page(400, "登录失败：回调缺少 state 或授权码，请重新发起网页登录。");
    }
    let callback_url = match original_callback_url(&uri, &headers) {
        Ok(url) => url,
        Err(error) => return oauth_callback_page(error.status_code, &error.message),
    };
    login_callback_page(state.login().submit_login_callback(&task_state, &callback_url).await)
}

/// HTTP 请求不携带 fragment；只补原始请求的 origin，保留 path/query 的原始编码，
/// 不从已解码的 Query 重新拼 code/state，不伪造 localhost 或丢弃其它供应商参数。
fn original_callback_url(
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
) -> Result<String, crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    if uri.scheme_str().is_some() && uri.authority().is_some() { return Ok(uri.to_string()); }
    let host = headers.get(axum::http::header::HOST).and_then(|value| value.to_str().ok())
        .ok_or_else(|| GatewayError::bad_request("登录回调缺少原始主机地址"))?;
    host.parse::<axum::http::uri::Authority>()
        .map_err(|_| GatewayError::bad_request("登录回调主机地址无效"))?;
    let scheme = uri.scheme_str().unwrap_or("http");
    Ok(format!("{scheme}://{host}{uri}"))
}

fn login_callback_page(
    result: Result<crate::server::core::login::LoginCallbackSubmission, crate::server::errors::GatewayError>,
) -> Response {
    use crate::server::core::login::LoginCallbackSubmission;
    match result {
        Ok(LoginCallbackSubmission::Completed(_)) => oauth_callback_page(200, "登录成功，已返回网关，可以关闭此页面。"),
        Ok(LoginCallbackSubmission::Accepted) => oauth_callback_page(200, "回调已接收，登录仍在处理中，请返回面板等待。"),
        Ok(LoginCallbackSubmission::ContinueTo(_)) => oauth_callback_page(400, "登录回调仍需继续，请重新发起网页登录。"),
        Err(error) => oauth_callback_page(error.status_code, &format!("登录失败：{}", error.message)),
    }
}

// ─── POST /api/session/refresh ──────────────────────────────

/// POST /api/session/refresh
///
/// 出错时走**最外层 catch → errorPayload**（OpenAI 风格 `{error:{message,type}}`），
/// 而不是管理 API 的 `{success:false,error}` 信封 —— Node 版这条就在大 try 里，
/// 刷新失败的响应形状与 /api/accounts/refresh 不同，别混用。
///
/// ── 续期怎么走（2026-10 拆家）───────────────────────────────
/// 原来这里调 `AuthService::refresh_stored_session`，那是 WorkBuddy 自己的
/// `/auth/token/refresh`。续期协议随实现搬到了提供商插件，所以本入口改成
/// **按当前账号的归属分派到适配器**（`refresh_access_token`）—— 与
/// `/api/accounts/refresh` 走同一条链路，只是不带账号 id（默认取当前账号）。
pub async fn session_refresh(State(state): State<ServerState>) -> Response {
    let Some(entry) = state.store().get_active_entry() else {
        return session_error(401, "当前没有可刷新的登录态");
    };
    let Some(provider) = state.store().account_provider_of(&entry.id) else {
        return session_error(401, "当前没有可刷新的登录态");
    };
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(&provider) else {
        return session_error(400, format!("未知的提供商：{provider}"));
    };
    match adapter.refresh_access_token(state.store(), &entry.id).await {
        Ok(_) => ok_json(json!({ "session": state.auth().get_status() })),
        Err(error) => {
            logging::log("[Auth]", &format!("❌ {}", error.message));
            use axum::response::IntoResponse;
            error.into_response()
        }
    }
}

/// 一条 OpenAI 风格的错误响应（`{error:{message,type}}`）。
fn session_error(status: i32, message: impl Into<String>) -> Response {
    use axum::response::IntoResponse;
    crate::server::errors::GatewayError::with_status(status, message).into_response()
}

// ─── POST /api/session/logout 与 POST /auth/logout ─────────

pub async fn session_logout(State(state): State<ServerState>) -> Response {
    state.auth().clear_session();
    crate::server::http::ok_empty()
}

/// POST /auth/logout —— 与 /api/session/logout 同一动作与同一响应形状
pub async fn auth_logout(State(state): State<ServerState>) -> Response {
    state.auth().clear_session();
    crate::server::http::ok_empty()
}

// ─── POST /auth/login ───────────────────────────────────────

/// 同步登录（对照 server.mjs 601-610 行的 `runLogin`）：等登录完成才响应。
///
/// 桌面端不用这条（它走 /api/session/login/* 的异步三步），保留它是为了
/// 与 Node 版的命令行/脚本入口保持契约一致。
///
/// ── 实现已经不在网关里（2026-10 拆家）────────────────────────
/// 原来这里调 `LoginService::run_login`，那是 WorkBuddy 自己的
/// `auth/state` → `auth/token` → `login/account` 轮询链。那条链已整体搬到
/// 提供商插件（`workbuddy::login`），所以本入口改成**通用的插件驱动**：
/// 问插件要授权地址 → 打印给用户 → 按插件给的间隔轮询 `login.poll` →
/// 完成时落库。与异步三步的差别只是「谁来轮询」（这里在请求内轮询）。
///
/// ── 地区怎么定 ──────────────────────────────────────────────
/// 优先读 `provider`（`workbuddy` / `workbuddy-intl`，与其它入口同一口径：
/// 身份即归属），其次读 `edition`（Node 版既有入参，脚本用户在用），
/// 都没有则国内版。
///
/// ── 只支持轮询型 ────────────────────────────────────────────
/// 回调型（`poll_interval_ms == 0`）在命令行里无法完成 —— 没有浏览器窗口
/// 替它收回调。那种情况明确报错并指向网页登录入口，而不是挂到 5 分钟超时。
pub async fn auth_login(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = parse_body(&body).ok();
    let provider = payload
        .as_ref()
        .and_then(|payload| payload.get("provider").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            // 没有 `provider` 时按 `edition` 归一（脚本用户的历史入参）
            let edition = payload
                .as_ref()
                .and_then(|payload| payload.get("edition").and_then(Value::as_str))
                .filter(|value| !value.is_empty());
            crate::server::core::providers::workbuddy::Region::from_edition_id(edition)
                .provider_id()
                .to_string()
        });
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(&provider) else {
        return management_error(400, format!("未知的提供商：{provider}"));
    };
    let label = adapter.label();
    if !adapter.supports_web_login() {
        return management_error(
            400,
            format!("{label}不支持网页登录，请改用「填写凭证」或「导入桌面端登录态」添加账号"),
        );
    }
    let Some(start) = adapter.login_start_async().await else {
        return management_error(400, format!("{label}未能生成网页登录授权地址，请重试"));
    };
    if start.poll_interval_ms == 0 {
        return management_error(
            400,
            format!(
                "{label}的网页登录要等浏览器回调，命令行入口无法完成；\
                 请用桌面端的「网页登录」，或改用「填写凭证」"
            ),
        );
    }
    logging::log("[Login]", "请在浏览器中打开以下链接并完成登录：");
    logging::console_line("[Login]", &format!("  {}", start.url));
    logging::log("[Login]", "登录完成后本网关将自动获取并保存 token…");

    let interval = Duration::from_millis(start.poll_interval_ms.max(1_000));
    let deadline = tokio::time::Instant::now() + Duration::from_millis(LOGIN_TIMEOUT_MS);
    loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() >= deadline {
            return management_error(
                504,
                format!("{label}登录轮询超时（{} 分钟），请重试", LOGIN_TIMEOUT_MS / 60_000),
            );
        }
        match adapter.login_poll_async(&start.flow).await {
            Ok(agent2api_provider_protocol::LoginPoll::Pending { .. }) => continue,
            Ok(agent2api_provider_protocol::LoginPoll::Done { account }) => {
                let public = account.public.clone();
                return match adapter.persist_login_account(&account) {
                    Ok(account_id) => {
                        logging::log(
                            "[Login]",
                            &format!("✅ {label}登录完成，账号已加入列表"),
                        );
                        crate::server::http::raw_json(json!({
                            "accountId": account_id,
                            "provider": provider,
                            "account": public,
                        }))
                    }
                    Err(error) => management_error(error.status_code, error.message),
                };
            }
            Ok(agent2api_provider_protocol::LoginPoll::Failed { message }) => {
                return management_error(400, message)
            }
            Err(error) => return management_error(error.status_code, error.message),
        }
    }
}
