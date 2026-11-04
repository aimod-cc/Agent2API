//! 账号管理路由（对照 src/workbuddy-account-routes.mjs 逐条实现）。
//!
//!   GET    /api/accounts                账号列表（含当前账号标记、优先级、启用状态、代理、
//!                                       provider 与顶层 providers 摘要）
//!   POST   /api/accounts                手动添加账号（accessToken/refreshToken JSON）
//!   GET    /api/accounts/export         导出全部账号（含 token，换机器后导入继续用）
//!   POST   /api/accounts/export-files   导出单账号或本组的逐账号文件 { id? / provider? }
//!   POST   /api/accounts/import         导入账号（merge：按 uid 匹配，命中更新、未命中追加）
//!   POST   /api/accounts/current        把账号置顶（仅调整全局优先级）{ id }
//!   POST   /api/accounts/batch          批量操作（见 accounts_batch.rs）
//!                                       enable / disable / proxy / concurrency / usageQuery / limiters / remove
//!   POST   /api/accounts/reorder        整队重排（分组模式）{ queue: [id, …] }
//!   POST   /api/accounts/refresh        刷新指定（或当前）账号的 token { id? }
//!   POST   /api/accounts/refresh-expiring 刷新**全部**已过期/临期的账号凭证（批量）
//!   GET    /api/accounts/usage          逐账号查询积分/额度（并发，单账号失败不拖垮整批）
//!                                       `?id=` 指定单个账号（**不看启用状态**，见 usage_query）
//!   GET    /api/accounts/usage/snapshot 最近一次**定时查询**的结果快照（形状同 usage）
//!   GET    /api/accounts/connections     逐账号活跃连接数（账号页「连接数」列，2 秒轮询）
//!   POST   /api/accounts/checkin        签到（串行，跳过国际版与范围外的提供商）{ id? }
//!   PATCH  /api/accounts/{id}           修改账号属性 { name?, priority?, enabled?, proxy? }
//!   POST   /api/accounts/{id}/move      与相邻账号交换优先级 { direction: 'up' | 'down' }
//!   DELETE /api/accounts/{id}           删除账号
//!
//! ── Agent2API 改造在本文件的行为 ─────────────────────────────
//!   - 列表响应的**形状是增量的**：每个账号对象多一个 `provider`、顶层多一个
//!     `providers` 摘要（都由 `AccountStore::list_accounts` 组装，本文件不用改）；
//!   - `POST /api/accounts` 按 payload 的 provider **穷举分派**（见 `add_account`）；
//!   - `usage` 从「只服务 workbuddy」扩到**四家混查**（余额查询移植）：目标集合
//!     不再按 provider 过滤，逐账号分流到 `ProviderAdapter::query_usage`。
//!     `checkin` 同样扩到**三家**（WorkBuddy / 小浣熊 / AutoClaw，各自签到链路
//!     互不相通，见 `core::billing::checkin::checkin_for`）；其余 CRUD 语义保持不变。
//!
//! 出网代理的两条（/api/proxies、/api/proxies/test）在 `api::proxies`，
//! 但它们的入口 `proxies_entry` 留在本文件 —— 与账号入口挨着，便于对照
//! Node 版 `tryHandle` → `tryHandleProxies` 的判定顺序。
//!
//! ── 分发方式：与 Node 版同构的「一个大入口 + 按路径判定」────────
//! Node 版是 `tryHandle(req,res,path)`：先按完整路径匹配固定子路径，都不命中
//! 才把剩余段当账号 id。这些判定顺序**是可观察行为**，例如：
//!   DELETE /api/accounts/export → 走 `<id>` 分支 → 404「账号不存在」
//!   GET    /api/accounts/xxx/yyy → 谁都不命中 → 404「Not found: GET ...」
//! 若改用 axum 的静态路由 + `{id}`（matchit 静态优先），前者会变成 405 兜底，
//! 与 Node 分叉。因此这里刻意保留 Node 的判定结构：一条 `/api/accounts/{*rest}`
//! 通配入口 + 一个方法/路径分发函数，注册顺序不再重要，行为逐条对齐。
//!
//! ── usage / checkin 两条的两处易错点 ─────────────────────────
//!   ① `skipped` 的口径在两条路径上**不同**（一个是「可用账号中被禁用的数」，
//!      另一个是「可用账号总数 − 可签到数」），详见 `resolve_checkin_targets`；
//!   ② `usage` 的「没有可用凭证」分支**不带 name 键**（Node 那条早退 return
//!      就没带），别顺手补齐，详见 `query_usage_for`。

use axum::body::Bytes;
use axum::extract::State;
use axum::http::Method;
use axum::response::Response;
use serde_json::{json, Map, Value};

pub use super::accounts_batch::batch_accounts;

use crate::server::core::account_store::AccountStoreError;
use crate::server::core::billing::checkin;
use crate::server::core::proxies::ProxyConfigError;
use crate::server::core::providers::adapter::adapter_for;
use crate::server::core::providers::zcode;
use crate::server::core::providers::ProviderKind;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body, query_param};
use crate::server::logging;
use crate::server::ServerState;

/// 账号存储错误 → 管理信封（打日志 + 原状态码）。
///
/// 可见性是 `pub(super)`：`api::session` 的 Loomy 验证码登录要落账号
/// （`add_loomy_account`），失败时必须是同一个响应形状与同一条日志格式 ——
/// 让那边各写一份会让「登录失败」与「添加失败」在界面上长得不一样。
pub(super) fn store_error(error: AccountStoreError) -> Response {
    logging::log("[Accounts]", &format!("❌ {}", error.message));
    management_error(error.status_code, error.message)
}

pub(super) fn proxy_error(error: ProxyConfigError) -> Response {
    logging::log("[Accounts]", &format!("❌ {}", error.message));
    management_error(error.status_code, error.message)
}

/// 标准账号的添加入口：把面板 payload 拆成 `NormalizeInput` 交给真实 adapter。
/// 插件与本体共用 secret/public 落库及公开白名单，不在 API 中读取令牌字段。
/// `account_source` 是网关选择的来源，不从 payload 或 secret 中取。
async fn add_plugin_account(
    state: &ServerState,
    adapter: &dyn crate::server::core::providers::adapter::ProviderAdapter,
    provider: &str,
    payload: &Value,
    import_name: Option<&str>,
    account_source: &str,
) -> Response {
    use crate::server::core::proxies::{normalize_account_proxy, resolve_account_proxy, ResolvedProxy};
    // 缺省不覆盖存量出口；显式 null/direct 是需要落库的直连选择。
    let proxy_config = match payload.get("proxy") {
        Some(value) if !value.is_null() && !value.is_object() => {
            return management_error(400, "账号代理配置必须是对象或 null，未回退直连");
        }
        Some(value) => match normalize_account_proxy(value) {
            Ok(config) => Some(config.unwrap_or_else(|| json!({ "source": "direct" }))),
            Err(error) => return proxy_error(error),
        },
        None => None,
    };
    // normalize 与落账号共用同一归一配置；pool/Clash 解析失败不能静默直连。
    let selected_proxy = match proxy_config.as_ref() {
        None => None,
        Some(config) if config.get("source").and_then(Value::as_str) == Some("direct") => None,
        Some(config) => {
            if crate::server::core::proxies::is_rotate_config(config) {
                return management_error(400, "添加标准账号不支持轮询代理，请选择直连或一条代理");
            }
            let Some(resolution) = resolve_account_proxy(Some(config)) else {
                return management_error(400, "账号代理未能解析，未回退直连");
            };
            if resolution.error().is_some() {
                return management_error(400, "账号代理无法解析，请检查所选代理是否存在且已启用；未回退直连");
            }
            match ResolvedProxy::from_json(&resolution.to_json()) {
                Ok(Some(proxy)) => Some(proxy),
                _ => return management_error(400, "账号代理出口无效，请检查主机与端口；未回退直连"),
            }
        }
    };
    let proxy = selected_proxy.map(|proxy| agent2api_provider_protocol::WireProxy {
        protocol: proxy.protocol,
        host: proxy.host,
        port: proxy.port,
        username: proxy.username,
        password: proxy.password,
    });

    let source = if payload.get("importDesktop").and_then(Value::as_bool).unwrap_or(false) {
        agent2api_provider_protocol::NormalizeSource::Desktop
    } else if let Some(content) = payload
        .get("authFileContent")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        agent2api_provider_protocol::NormalizeSource::FileText {
            content: content.to_string(),
            file_name: payload
                .get("authFileName")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    } else if let Some(content) = payload
        .get("authFileB64")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        agent2api_provider_protocol::NormalizeSource::FileBase64 {
            content: content.to_string(),
            file_name: payload
                .get("authFileName")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    } else {
        // 新 runtime 的手动归一器只吃自身凭证/方法参数，不把网关代理配置混入
        // RAW 认证输入。旧插件仍收到原 payload，保留历史归一合同。
        let mut manual_payload = payload.clone();
        if crate::server::core::providers::is_scoped_runtime_provider(provider) {
            if let Some(fields) = manual_payload.as_object_mut() {
                if fields.get("methodIndex").is_some() {
                    for key in ["provider", "name", "proxy"] { fields.remove(key); }
                } else if account_source == "builtin" {
                    for key in ["name", "proxy"] { fields.remove(key); }
                }
            }
        }
        agent2api_provider_protocol::NormalizeSource::Manual { payload: manual_payload }
    };

    let input = agent2api_provider_protocol::NormalizeInput {
        provider: provider.to_string(),
        source,
        name: import_name.map(str::to_string),
        proxy,
    };
    let account = match adapter.normalize_account(&state.store, input).await {
        Ok(account) => account,
        Err(error) => {
            logging::log("[Accounts]", &format!("❌ {}", error.message));
            return management_error(error.status_code, error.message);
        }
    };

    let id = account.identity.clone();
    let name = account
        .public
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or(import_name)
        .unwrap_or(adapter.label());
    match state.store().plugin_upsert_account_with_source_and_proxy(
        &id,
        provider,
        name,
        &account.secret,
        &account.public,
        account.desktop,
        account_source,
        proxy_config.as_ref(),
    ) {
        Ok((record, proxy_saved)) => ok_json(json!({
            "account": record,
            "created": true,
            "proxySaved": proxy_saved,
            "list": state.store().list_visible_accounts(),
        })),
        Err(message) => {
            logging::log("[Accounts]", &format!("❌ {message}"));
            management_error(500, &message)
        }
    }
}

/// 解析请求体：空 body 视为 `{}`；非法 JSON 按各处的原始文案报 400。
///
/// Node 版不同分支的文案不一样（「上传内容不是有效 JSON」/「请求内容不是有效 JSON」），
/// 所以文案由调用方传入，不统一成一句话。
pub(super) fn parse_json_body(body: &Bytes, fallback_message: &str) -> Result<Value, Response> {
    match parse_body(body) {
        Ok(value) => Ok(value),
        Err(_) => Err(management_error(400, fallback_message)),
    }
}

/// `authFileContent` / `authFileB64`（凭证文件导入）现在**只服务插件家**：
/// 两个字段由 `add_plugin_account` 原样转成 `NormalizeSource::{FileText,
/// FileBase64}`，文件解析（含「这一家的文件是文本还是二进制」）是插件自己的
/// 知识。网关不再按家拼「哪个文件导不进来」的文案 —— 那套措辞会与插件实际
/// 支持的格式漂移，而用户看到的错误必须来自真正解析它的那一侧。
///
/// 百分号解码（对应 Node 版 `decodeURIComponent(path.slice(...))`）。
///
/// axum 已在提取参数时解过一次；账号 id 形如 `user-<uid>`、正常不含特殊字符，
/// 所以这一步基本是恒等变换 —— 保留它是因为 Node 版就是这么写的，
/// 手工构造的 URL 里带 `%2F` 这类编码时行为才不会分叉。
fn decode_segment(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|text| u8::from_str_radix(text, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

// ─── axum 入口 ──────────────────────────────────────────────

/// `/api/accounts`（无尾段）与 `/api/accounts/{*rest}` 共用的入口。
///
/// 用 `any(...)` 注册（接受任意方法），因为 Node 的判定里有「PATCH/DELETE 落到
/// `<id>` 分支」这种跨方法的路径匹配 —— 交给 axum 的方法路由反而会把它拆错。
pub async fn accounts_entry(State(state): State<ServerState>, request: axum::extract::Request) -> Response {
    let method = request.method().clone();
    let full_path = request.uri().path().to_string();
    // 原始查询串（未解码的原文，由用到它的分支自行 `query_param` 解码）。
    // 目前只有 `GET /api/accounts/usage?id=<id>` 用它 —— 那一支是「用户手点某一行
    // 的余额按钮」，与批量的区别见 `core::usage_query::query_all` 的说明。
    let query = request.uri().query().unwrap_or("").to_string();
    let body = match axum::body::to_bytes(request.into_body(), crate::server::http::MAX_BODY_SIZE)
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => return management_error(413, format!("请求体读取失败或过大: {error}")),
    };
    // `suffix` 为空 = 精确命中 `/api/accounts`（列表/新增）；其它情况（含 `/api/accounts/`
    // 这个只有尾斜杠的形态）都是子路径 —— Node 版用 `path === '/api/accounts'` 严格
    // 判等，所以 `/api/accounts/` 落到最后的 404 分支，这里必须保留这个区分
    let suffix = full_path.strip_prefix("/api/accounts").unwrap_or("");
    let rest = suffix.strip_prefix('/').map(str::to_string);
    dispatch(state, method, rest.as_deref(), &full_path, &query, &body).await
}

/// `/api/proxies*` 的入口（同样接受任意方法）。
///
/// 已注册路径：`/api/proxies`、`/api/proxies/test`（Node 版原有的出口能力）与
/// `/api/proxies/pool`、`/pool/update`、`/pool/enable`、`/pool/remove`、
/// `/pool/test`、`/pool/sync-clash`（「网络代理」页的代理池 CRUD + 启停 + 测试
/// + Clash 同步，见 `api::proxies` 的模块头）。池那几条的 id 走 body，不做
/// 路径段 —— 判定的写法因此还是一张 (方法, 剩余路径) 的表，与原来的两条完全同形。
pub async fn proxies_entry(State(state): State<ServerState>, request: axum::extract::Request) -> Response {
    let method = request.method().clone();
    let full_path = request.uri().path().to_string();
    let rest = full_path
        .strip_prefix("/api/proxies")
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
        ("GET", "") => super::proxies::list_proxies(&state).await,
        ("POST", "test") => super::proxies::test_proxy(&state, &body).await,
        ("GET", "pool") => super::proxies::pool_list(&state).await,
        ("POST", "pool") => super::proxies::pool_create(&state, &body).await,
        ("POST", "pool/update") => super::proxies::pool_update(&state, &body).await,
        ("POST", "pool/enable") => super::proxies::pool_enable(&state, &body).await,
        ("POST", "pool/remove") => super::proxies::pool_remove(&state, &body).await,
        ("POST", "pool/test") => super::proxies::pool_test(&state, &body).await,
        ("POST", "pool/sync-clash") => super::proxies::pool_sync_clash(&state).await,
        ("GET", "settings") => super::proxies::settings_get().await,
        ("POST", "settings") => super::proxies::settings_save(&body).await,
        // 已注册路径上的其它方法：Node 的 tryHandleProxies 落到它自己的 404 信封
        _ => management_error(
            404,
            format!("Not found: {} {full_path}", method.as_str()),
        ),
    }
}

// ─── 路径分发（与 Node 版 tryHandle 的判定顺序逐条对齐）──────

/// 账号路由的路径分发。
///
/// `rest` 为 `None` 表示精确命中 `/api/accounts`；`Some(...)` 是去掉一层前导斜杠后的
/// 剩余段（`/api/accounts/` 得到的是 `Some("")` —— Node 严格判等，它属于子路径而非列表）。
/// `full_path` 是原始完整路径（404 文案里要原样回显），`query` 是未解码的查询串
/// （只有 `usage` 那一支用它取 `id`，见 `accounts_usage::accounts_usage`）。判定顺序
/// **照抄 Node 版**：
///
///   ① 固定子路径（无尾段的一批）：GET/POST ``、GET export、POST import、
///      POST current、POST batch、POST refresh、GET usage、POST checkin
///   ② POST + 以 `/move` 结尾 → 调整顺序；以 `/rate-limits/clear` 结尾 → 清除限流标记
///   ③ PATCH / DELETE + 有剩余段 → 当成账号 id（**不做白名单校验**，
///      所以 `DELETE /api/accounts/export` 是「删一个叫 export 的账号」）
///   ④ 谁都不命中 → 404「Not found: <METHOD> <path>」（管理 API 信封）
///
/// 第 ③ 步的顺序很关键：Node 的 PATCH 分支只判 `startsWith('/api/accounts/')`，
/// 所以固定子路径在 GET/POST 之外的方法上会被当作账号 id，得到 404「账号不存在」。
pub async fn dispatch(
    state: ServerState,
    method: Method,
    rest: Option<&str>,
    full_path: &str,
    query: &str,
    body: &Bytes,
) -> Response {
    // ① 精确命中 `/api/accounts`
    if rest.is_none() {
        match method.as_str() {
            "GET" => return ok_json(state.store().list_visible_accounts()),
            "POST" => return add_account(&state, body).await,
            // Node 版对 `/api/accounts` 上的其它方法同样走 404 信封
            _ => {
                return management_error(404, format!("Not found: {} {full_path}", method.as_str()))
            }
        }
    }
    let rest = rest.unwrap_or("");

    // ② 固定子路径（Node 版把它们放在 `<id>` 通配之前的原因）
    match (method.as_str(), rest) {
        ("GET", "sorting") => return super::accounts_sort::get_sorting(&state),
        ("PUT", "sorting") => return super::accounts_sort::save_sorting(&state, body),
        ("GET", "export") => return export_accounts(&state),
        ("POST", "export-files") => return super::accounts_export::account_files(&state, body).await,
        ("POST", "import") => return import_accounts(&state, body).await,
        ("POST", "current") => return set_current(&state, body).await,
        ("POST", "batch") => return batch_accounts(&state, body).await,
        // 整队重排（账号页分组模式：拖组 / 拖行之后把新顺序整条交上来）
        ("POST", "reorder") => return reorder_accounts(&state, body).await,
        ("POST", "refresh") => return refresh_account(&state, body).await,
        ("POST", "refresh-expiring") => return refresh_expiring_accounts(&state).await,
        // 余额 / 积分查询（四家混查）实现在 `api::accounts_usage`（拆分见那里的模块头）。
        // `?id=` 是「手点某一行积分按钮」的单查形态，语义见 accounts_usage 的说明。
        ("GET", "usage") => {
            return super::accounts_usage::accounts_usage(&state, query).await
        }
        // 定时查询那一轮的结果快照（形状同 usage，多一个 `at`）
        ("GET", "usage/snapshot") => {
            return super::accounts_usage::accounts_usage_snapshot(&state).await
        }
        // 账号级活跃连接数（账号页「连接数」列；见 `core::upstream::connections`）
        ("GET", "connections") => return account_connections(&state),
        ("POST", "checkin") => return accounts_checkin(&state, body).await,
        _ => {}
    }

    // ③ POST + /move 结尾；POST + /rate-limits/clear 结尾
    if method == Method::POST {
        if let Some(id) = rest.strip_suffix("/move") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return move_account(&state, &id, body).await;
            }
        }
        if let Some(id) = rest.strip_suffix("/rate-limits/clear") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return clear_rate_limits(&state, &id, body);
            }
        }
        // ZCode「限时套餐」：探测（只读、不要验证码）与领取（要验证码）。
        // 两个后缀互不包含（`/zcode-claim/preview` 不以 `/zcode-claim` 结尾），
        // 因此这里的先后不影响命中 —— 但读的时候按「先探测后领取」排列，
        // 与界面上用户的操作顺序一致。
        if let Some(id) = rest.strip_suffix("/zcode-claim/captcha-config") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::zcode_claim::captcha_config(&state, &id).await;
            }
        }
        if let Some(id) = rest.strip_suffix("/zcode-claim/preview") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::zcode_claim::preview(&state, &id).await;
            }
        }
        // CodeArts 每日福利：先探测（只读）再领取（写）。
        // 顺序上 preview 必须排在前面 —— 两个后缀互不包含，但读的时候按用户
        // 的操作顺序排列，与上面 ZCode 那三条同一写法。
        if let Some(id) = rest.strip_suffix("/codearts-welfare/preview") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::codearts_welfare::preview(&state, &id).await;
            }
        }
        if let Some(id) = rest.strip_suffix("/codearts-welfare") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::codearts_welfare::claim(&state, &id, body).await;
            }
        }
        if let Some(id) = rest.strip_suffix("/zcode-claim") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::zcode_claim::claim_plan(&state, &id, body).await;
            }
        }
        // 新手任务一键领取（执行体按账号的提供商分派：Loomy 任务表 / 小浣熊
        // 首次登录奖励，见 `api::onboarding`；幂等，重复点不会重复加分）。
        if let Some(id) = rest.strip_suffix("/onboarding/claim") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::onboarding::claim(&state, &id).await;
            }
        }
        // WorkBuddy 成长任务（国内版）：单任务执行 / 开学季 / 猫猫旅行，见
        // `api::growth`。三条后缀互不包含（各自以 /growth/xxx 结尾），先后不影响
        // 命中；读的时候按界面上按钮的顺序排列（单任务执行 → 开学季 → 旅行巡检）。
        // 执行拆成单任务端点的原因（60 秒转发超时）见 api::growth 的模块头。
        if let Some(id) = rest.strip_suffix("/growth/task") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::growth::run_task(&state, &id, body).await;
            }
        }
        if let Some(id) = rest.strip_suffix("/growth/school") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::growth::school_run(&state, &id, body).await;
            }
        }
        if let Some(id) = rest.strip_suffix("/growth/travel") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::growth::travel_run(&state, &id).await;
            }
        }
    }

    // ③' GET + /onboarding 结尾 → 新手任务状态（只读，签到后弹窗的查询口）。
    // 与上面 POST 段同一写法：后缀互不包含，先后不影响命中。
    // `?refresh=1` 强制实查上游（手点「查询任务」）；不带则吃结算记忆（一次性
    // 福利领完就不再问上游，见 `api::onboarding` 的模块说明）。
    if method == Method::GET {
        if let Some(id) = rest.strip_suffix("/onboarding") {
            let id = decode_segment(id);
            if !id.is_empty() {
                let refresh = query_param(query, "refresh").as_deref() == Some("1");
                return super::onboarding::status(&state, &id, refresh).await;
            }
        }
        // WorkBuddy 成长任务盘点（只读，见 `api::growth` 的模块头）。
        // 与上面的 onboarding 同一纪律：查询走 GET、动状态走 POST ——
        // 页面展开只查，不给「看一眼就改状态」留机会。
        if let Some(id) = rest.strip_suffix("/growth") {
            let id = decode_segment(id);
            if !id.is_empty() {
                return super::growth::status(&state, &id).await;
            }
        }
    }

    // ④ PATCH / DELETE → 把剩余段当账号 id。
    //    id 为空（即 `/api/accounts/`）也交给 handler：它返回 400「缺少账号 id」，
    //    这与 Node 版 PATCH 分支 `if (!id) throw` 的结果一致。
    //    注意与 `/api/accounts//` 的区别：那条解出的是 "/"（非空）→ 404「账号不存在」。
    match method.as_str() {
        "PATCH" => return patch_account(&state, &decode_segment(rest), body).await,
        "DELETE" => return delete_account(&state, &decode_segment(rest)),
        _ => {}
    }

    // ⑤ 兜底 404（Node 版 `tryHandle` 结尾那句）
    management_error(404, format!("Not found: {} {full_path}", method.as_str()))
}

// ─── POST /api/accounts ─────────────────────────────────────

/// 添加账号：**按 payload 的 provider 分派到各家的添加路径**（W3-T4 起，
/// W4a 改为注册表驱动的穷举分派）。
///
/// ── 分派规则（W4a：不再手写「workbuddy/raccoon」清单）──────────
/// provider 字段先过 `providers::kind_from_id` 换算（**注册表就是白名单**，
/// 见 `providers::is_known_provider_id` 的说明），再按 kind 穷举：
///   ① `Raccoon` → 小浣熊路径：`importDesktop === true` 导入桌面端实时登录态
///      （忽略 token 字段）；否则手动添加（token / refreshToken，或粘贴
///      auth.json 内容）；
///   ② `CatPaw` → 实现已搬到插件：装上插件时上面的 `serves_provider` 分流
///      先带走（三种来源都由插件的 `account.normalize` 处理），没装插件时
///      如实报 503；
///   ③ `AutoClaw` → AutoClaw 路径（W4b-T-c2）：`importDesktop === true` 导入
///      桌面端实时登录态（`%APPDATA%/AutoClaw/auth.json`，DPAPI 解密）；
///      否则手动添加（token/refreshToken + deviceId，`enc:` 密文自动解密）；
///   ④ `WorkBuddy` / `WorkBuddyIntl` 或**字段缺失** → 既有 workbuddy 路径
///      （`store.add_account`，归属由这里的 provider id 给出：国际版与国内版
///      共用同一套凭证形态与落账号路径，差别只在落进哪一家）。
///
/// ── 文件导入（小浣熊 / Cline / KukuAI，以及插件家）─────────────────
/// payload 新增三个可选字段（契约，历史请求不受影响）：
///   - `authFileContent`：用户手选的凭证文件**文本内容**（KukuAI 不吃文本 ——
///     它的凭证是二进制 SQLite，收到文本明确 400 指路 `authFileB64`）；
///   - `authFileB64`：文件 base64（前三家的凭证是文本 JSON，只识别并 400；
///     KukuAI 的 Cookies SQLite 是二进制，真正走这条的就是它）；
///   - `authFileName`：原文件名，仅用于错误文案与日志。
/// `authFileContent` / `authFileB64` 任一存在 → 走「文件导入」路径，**优先于**
/// `importDesktop`（见 ① ② 分支内的说明）；两者都没有时行为与历史逐字一致。
/// 语义差别（两条路径别混用）：文件导入＝把文件里的**凭证解析后落库**，得到
/// 一个**可独立转发**的账号（与「手动粘贴」同一入口，文件可来自任意机器或
/// 备份）；`importDesktop` ＝只建一条「转发时实时读本机固定路径」的记录。
///
/// ── AutoClaw 曾经在这里显式 400（历史，别改回去）─────────────────
/// W4a–W4b 之间它的凭证链路还没落地，那时这里对它的 payload 报 400：若让它落进
/// ④ 的兜底，用户会得到一条**200 + 存进 workbuddy 组**的账号（凭证是别家的、
/// 分组是错的、转发永远失败），界面上看不出异常。W4b-T-c2 起它有自己的分支
/// （③），该 400 随之退役；纪律不变 —— **绝不写进没人认的组**。分派用穷举
/// match：新增 provider 时编译器会强制给出分支；注册表里没有的 id（前端比后端
/// 新、或手改的请求）仍走 ④ —— 老客户端不带 provider 字段，未知 id 不能报错。
pub async fn add_account(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(&body, "上传内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let provider = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let store = state.store();
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let import_name = name.as_deref();
    if let Some(response) = super::accounts_export::import_account_file(state, provider, &payload, import_name) {
        return response;
    }
    if crate::server::core::providers::availability::is_suspended(provider) {
        return management_error(503, "该提供商已暂停接入，暂不支持添加账号；已有数据保留");
    }
    // ── 「文件导入」三字段（均为可选；文件导入契约）─────────────
    // 用户在界面上**手选**桌面客户端的凭证文件时，前端把文件内容放进这些字段：
    //   - `authFileContent`：文件文本内容（JSON 类文件，前端已读过整个文件）；
    //   - `authFileB64`：文件 base64（二进制文件：KukuAI 的 Cookies SQLite 走它；
    //     凭证是文本 JSON 的几家只识别并 400）；
    //   - `authFileName`：原文件名，仅用于错误文案与日志（不参与解析）。
    // 任一内容字段存在 → 走文件导入路径，**优先于** `importDesktop`：前者把文件
    // 里的**凭证解析后落库**（得到一个可独立转发的账号，文件可来自任意机器或
    // 备份），后者只建一条「实时读本机固定路径」的记录；两者同时给时以文件为准
    // （用户的显式选择更强）。两个内容字段都没有时行为不变（历史契约零变化）。
    // 三个字段本身**不做解析**：插件家由 `add_plugin_account` 转成
    // `NormalizeSource::{FileText,FileBase64}`（插件自己按家解析），内置家
    // 只剩 Cline / KukuAI 等仍读它们的分支。
    // ── 自定义提供商（`custom-` 前缀，不在静态注册表里）─────────
    // 它**不进** `ProviderKind` 枚举 —— `kind_from_id` 对它返回 None 是设计
    // 而不是遗漏（见 `providers` 的模块头），所以必须在穷举 match **之前**
    // 分流：落进 `None` 分支会被当成 workbuddy 会话形态存进 workbuddy 组
    // （凭证是 apiKey、分组是错的，正是 W4a 要消灭的「静默进错组」）。
    // 判据是 `is_custom_provider_id`（前缀 + 存储里存在）：只看前缀会让
    // 手改数据塞进来的陌生 id 也被收下，而它没有协议与基址，必然是个死组。
    if crate::server::core::custom_providers::is_custom_provider_id(provider) {
        let result = store.add_custom_account(provider, &payload, import_name);
        return match result {
            Ok(upsert) => ok_json(json!({
                "account": upsert.account,
                // 幂等合并（同 key / 同家无需鉴权）时为 false：前端据此把提示
                // 说成「已更新」而不是「已添加」，见 CustomAccountUpsert 的说明
                "created": upsert.created,
                "list": store.list_visible_accounts(),
            })),
            Err(error) => store_error(error),
        };
    }
    // 真实插件与本体 runtime ID 共用 normalize / 标准落库，必须先于旧枚举拒绝路径。
    // 同一 ID 仍由插件优先执行；source 仅区分来源，不改变 provider 归属。
    if crate::server::core::providers::is_executable_provider(provider) {
        let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(provider) else {
            return management_error(503, format!("提供商执行体尚未就绪：{provider}"));
        };
        let source = if crate::server::core::plugin_host::serves_provider(provider) {
            "plugin"
        } else {
            "builtin"
        };
        return add_plugin_account(state, adapter.as_ref(), provider, &payload, import_name, source).await;
    }
    // ── 兜底分派：**每个分支都直接返回**（没有内置落库路径了）──────
    // 本版本不再内置任何一家的账号实现，所以这里的每个 arm 都是 503 指路。
    // 保留穷举 match 而不是删成一句通用 503：新增 `ProviderKind` 时编译器
    // 会强制给出分支，而「这家到底是不是真的搬完了」只有逐家看过才知道 ——
    // 一句通用兜底会让「忘了迁某一家」变成静默的错误文案。
    match crate::server::core::providers::kind_from_id(provider) {
        // Raccoon（小浣熊）：实现已整体搬到插件（JWT 凭证解析 / 桌面端实时
        // 登录态 / 文件导入 / 续期 / 目录 / 余额 / 签到 / 新手任务），这里只留
        // 拒绝路径 —— 装上插件时上面的 `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::Raccoon) => {
            return management_error(
                503,
                "小浣熊的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与桌面端读取实现",
            );
        }
        // CatPaw：实现已整体搬到插件（凭证归一化、桌面端实时登录态、余额凭证、
        // 会话协议转发）。装上插件时上面的 `serves_provider` 分流会先带走，
        // 三种来源（文件 / 桌面端 / 粘贴）都由插件的 `account.normalize` 处理。
        Some(crate::server::core::providers::ProviderKind::CatPaw) => {
            return management_error(
                503,
                "CatPaw 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与桌面端读取实现",
            );
        }
        // AutoClaw（两个地区）：实现已整体搬到插件（凭证解析 / safeStorage 解密 /
        // 桌面端实时登录态 / 续期 / 转发 / 短信与 OAuth 登录），这里只留拒绝路径
        // —— 装上插件时上面的 `serves_provider` 分流会先带走。
        Some(
            crate::server::core::providers::ProviderKind::AutoClaw
            | crate::server::core::providers::ProviderKind::AutoClawIntl,
        ) => {
            return management_error(
                503,
                "AutoClaw 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与登录实现",
            );
        }
        // CodeArts：实现已整体搬到插件（AK/SK/STS 凭据解析、DPoP + PKCE 登录、
        // 目录三源、余额、每日福利），这里只留拒绝路径 —— 装上插件时上面的
        // `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::CodeArts) => {
            return management_error(
                503,
                "CodeArts 的账号需要安装提供商插件：本版本不再内置该家的凭据解析与登录实现",
            );
        }
        // Qoder（两个地区）：实现已整体搬到插件（设备授权登录、PAT 换证、
        // 续期、目录、会话式转发、额度、签到、风控组件），这里只留拒绝路径
        // —— 装上插件时上面的 `serves_provider` 分流会先带走。
        Some(
            crate::server::core::providers::ProviderKind::Qoder
            | crate::server::core::providers::ProviderKind::QoderIntl,
        ) => {
            return management_error(
                503,
                "Qoder 的账号管理需要安装提供商插件：本版本不再内置该家的设备授权与凭据解析实现",
            );
        }
        // Accio（两个地区）：实现已整体搬到插件（含粘贴凭证归一化、网页登录、
        // 续期与转发），这里只留拒绝路径 —— 装上插件时上面的 `serves_provider`
        // 分流会先带走。
        Some(
            crate::server::core::providers::ProviderKind::Accio
            | crate::server::core::providers::ProviderKind::AccioCn,
        ) => {
            return management_error(
                503,
                "Accio 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与登录实现",
            );
        }
        // Cline：实现已整体搬到插件（设备授权登录、凭证解析、续期、目录、
        // 伪装头、余额、转发），这里只留拒绝路径 —— 装上插件时上面的
        // `serves_provider` 分流会先带走。
        Some(
            crate::server::core::providers::ProviderKind::ClineFree
            | crate::server::core::providers::ProviderKind::ClinePass,
        ) => {
            return management_error(
                503,
                "Cline 的账号管理需要安装提供商插件：本版本不再内置该家的设备授权登录与凭证解析实现",
            );
        }
        // ZCode（两个地区）：实现已整体搬到插件（CLI 轮询登录、编码套餐凭证
        // 换取、活动套餐通道、领取、余额、验证码令牌池、目录、转发），这里只留
        // 拒绝路径 —— 装上插件时上面的 `serves_provider` 分流会先带走。
        Some(
            crate::server::core::providers::ProviderKind::Zcode
            | crate::server::core::providers::ProviderKind::ZcodeIntl,
        ) => {
            return management_error(
                503,
                "ZCode 的账号管理需要安装提供商插件：本版本不再内置该家的 CLI 登录与凭证换取实现",
            );
        }
        // Trae：实现已整体搬到插件（网页登录、凭据解析、续期、目录、转发、
        // 额度、签到），这里只留拒绝路径 —— 装上插件时上面的 `serves_provider`
        // 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::Trae) => {
            return management_error(
                503,
                "Trae 的账号管理需要安装提供商插件：本版本不再内置该家的网页登录与凭据解析实现",
            );
        }
        // Loomy（讯飞）：实现已整体搬到插件（粘贴 session、短信登录、续期、
        // 转发、额度、签到、新手任务），这里只留拒绝路径 —— 装上插件时上面的
        // `serves_provider` 分流会先带走。
        //
        // 曾经这里保留了一条内置的 `add_loomy_account` 粘贴路径，后果是
        // 「没装插件也能加进一个 Loomy 账号，但转发与签到全部 503」——
        // 账号能建、功能全无，用户看不出为什么。与其他家统一成同一条 503。
        Some(crate::server::core::providers::ProviderKind::Loomy) => {
            return management_error(
                503,
                "Loomy 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与短信登录实现",
            );
        }
        // KukuAI（百度文库库库 AI）：实现已整体搬到插件（三态凭证解析——粘贴
        // Cookie / 本机客户端 Cookies 数据库 / 文件导入——加真实校验与网页登录），
        // 这里只留拒绝路径。装上插件时上面的 `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::Kuku) => {
            return management_error(
                503,
                "KukuAI 的账号管理需要安装提供商插件：本版本不再内置该家的 Cookie 解析与校验实现",
            );
        }
        // MonkeyCode（长亭科技）：实现已整体搬到插件（session 探活、image_id
        // 发现、目录、会话转发），这里只留拒绝路径。装上插件时上面的
        // `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::MonkeyCode)
        | Some(crate::server::core::providers::ProviderKind::MonkeyCodeIntl) => {
            return management_error(
                503,
                "MonkeyCode 的账号管理需要安装提供商插件：本版本不再内置该家的 session 校验实现",
            );
        }
        // Command Code：实现已整体搬到插件（API Key 归一化 / 形态检查 / 探活 /
        // 目录 / NDJSON 转发）。装上插件时上面的 `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::CommandCode) => {
            return management_error(
                503,
                "Command Code 的账号管理需要安装提供商插件：本版本不再内置该家的 API Key 校验与转发实现",
            );
        }
        // Antigravity（Google 的 AI IDE）：实现已整体搬到插件（授权码登录 +
        // refresh token 归一化 + 探活 + 转发 + 余额），这里只留拒绝路径。
        Some(crate::server::core::providers::ProviderKind::Antigravity) => {
            return management_error(
                503,
                "Antigravity 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与探活实现",
            );
        }
        // OfficeAce（华为云果办 / OfficeClaw）：实现已整体搬到插件（含自助
        // OAuth 登录与续期），这里只留拒绝路径 —— 装上插件时上面的
        // `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::OfficeAce) => {
            return management_error(
                503,
                "OfficeAce 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与登录实现",
            );
        }
        // OrcaTerm（腾讯云遨驰终端桌面客户端）：实现已整体搬到插件，这里不再
        // 保留内置分支 —— 装上插件时上面的 `serves_provider` 分流会带走，
        // 没装插件时落到这里，如实拒绝而不是走一条没有执行体的落库路径。
        Some(crate::server::core::providers::ProviderKind::OrcaTerm) => {
            return management_error(
                503,
                "OrcaTerm 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析与探活实现",
            );
        }
        // 智谱清言（`chatglm.cn`）：实现已整体搬到插件（访客换证 / 粘贴令牌 /
        // 导入本机登录态三条路径、续期、目录、转发、余额、签到、任务中心），
        // 这里只留拒绝路径 —— 装上插件时上面的 `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::Chatglm) => {
            return management_error(
                503,
                "智谱清言的账号管理需要安装提供商插件：本版本不再内置该家的访客换证、令牌校验与本机登录态读取实现",
            );
        }
        // ModelScope（魔搭）的两个站点：实现已整体搬到插件（`ms-` 令牌探活取
        // 身份、转发、目录、魔粒、签到），这里只留拒绝路径 —— 装上插件时上面的
        // `serves_provider` 分流会先带走。
        Some(
            crate::server::core::providers::ProviderKind::Modelscope
            | crate::server::core::providers::ProviderKind::ModelscopeIntl,
        ) => {
            return management_error(
                503,
                "ModelScope 的账号管理需要安装提供商插件：本版本不再内置该家的令牌探活与凭证解析实现",
            );
        }
        // AtomCode：实现已整体搬到插件（令牌对归一化、目录、转发、额度、续期、
        // 领取）。装上插件时上面的 `serves_provider` 分流会先带走。
        Some(crate::server::core::providers::ProviderKind::Atomcode) => {
            return management_error(
                503,
                "AtomCode 的账号管理需要安装提供商插件：本版本不再内置该家的凭证解析实现",
            );
        }
        // LobsterAI：**没有粘贴路径**（如实拒绝，而不是落一条用不了的账号）。
        // 它的凭证里有两个 `keyfrom` 与一个 `uuid`，只在换码那一刻由上游下发 ——
        // 用户从别处粘不出这三样，缺了它们账号续不了期。所以本家只有一个入口：
        // 网页登录落到本机 loopback 回调后由**插件**换码落账号。
        // 装上插件时上面的 `serves_provider` 分流会先带走（插件的
        // `normalize_account` 对同样的问题给同一条文案）。
        Some(crate::server::core::providers::ProviderKind::LobsterAi) => {
            return management_error(
                400,
                "LobsterAI 只能通过「网页登录」添加：它的续期需要换码时下发的 uuid 与 keyfrom，粘贴给不出这三项",
            );
        }
        // WorkBuddy 系的两家（国内版 / 国际版）：实现已整体搬到插件
        // （会话凭证解析 / 桌面端登录态读取 / 续期 / 转发），这里只留拒绝路径。
        //
        // `None`（老客户端不带 provider 字段）曾经按国内版处理：那是**旧内置
        // 链**的兼容。现在没有可落的路径了 —— 静默按国内版落一条记录等于把
        // 用户的凭证写进一个没有执行体的组，界面看不出异常但转发必然失败，
        // 所以统一回 503 并指路去装插件。
        Some(crate::server::core::providers::ProviderKind::WorkBuddy)
        | Some(crate::server::core::providers::ProviderKind::WorkBuddyIntl)
        | None => {
            management_error(
                503,
                "WorkBuddy 的账号管理需要安装提供商插件：本版本不再内置该家的会话凭证解析与桌面端登录态读取实现",
            )
        }
    }
}

// ─── GET /api/accounts/export ───────────────────────────────

pub fn export_accounts(state: &ServerState) -> Response {
    let data = crate::server::core::account_transfer::export_accounts(state.store());
    let count = data
        .get("accounts")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0);
    logging::log("[Accounts]", &format!("📤 账号已导出: {count} 个"));
    ok_json(data)
}

// ─── POST /api/accounts/import ──────────────────────────────

pub async fn import_accounts(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(&body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    // 兼容管理 API 的 `{ success, data }` 信封壳：网页端此前的「导出」把整个
    // 信封存成了文件，顶层没有 accounts。这类文件导回时在这里下钻一层 ——
    // 已发出的旧文件不作废；标准导出文件（顶层就是 accounts）不受影响。
    let payload = match envelope_data(&payload) {
        Some(data) => data,
        None => payload,
    };
    match crate::server::core::account_transfer::import_accounts(state.store(), &payload) {
        Ok(result) => ok_json(result),
        Err(error) => store_error(error),
    }
}

/// 若 `payload` 是「信封壳」（对象、顶层无 accounts、`data` 字段是含 accounts
/// 的对象）则返回内层 data 的克隆，否则 None。
fn envelope_data(payload: &Value) -> Option<Value> {
    let object = payload.as_object()?;
    if object.contains_key("accounts") {
        return None;
    }
    let data = object.get("data")?;
    data.get("accounts").is_some().then(|| data.clone())
}

// ─── POST /api/accounts/current ─────────────────────────────

pub async fn set_current(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(&body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() {
        return management_error(400, "缺少账号 id");
    }
    match state.store().promote_to_front(id) {
        Ok(result) => {
            if result.get("changed").and_then(Value::as_bool) == Some(false) {
                logging::log("[Accounts]", &format!("已在全局队列第一位，无需置顶: {id}"));
            }
            ok_json(result)
        }
        Err(error) => store_error(error),
    }
}

// ─── POST /api/accounts/refresh ─────────────────────────────

/// 刷新账号 token（**按账号所属 provider 分派**）。
///
/// workbuddy 账号走既有的 `AuthService::refresh_account`；小浣熊、AutoClaw、
/// CodeArts、ZCode、Cline 账号各走自家适配器的 `refresh_access_token`；
/// **CatPaw / MonkeyCode / Command Code 账号没有可刷新的东西**（§9.1：
/// `X-Passport-Token` 过期只能在桌面端重新登录，没有 refreshToken；后两家是
/// 静态 session / API Key，上游根本没有续期接口），因此这里明确报 400 并说明
/// 做法 —— 静默走 workbuddy 的刷新会拿别家的凭证去打腾讯的鉴权接口。
///
/// 已搬到插件的家（Trae / OfficeAce / AtomCode / LobsterAI …）由上面那条
/// 「插件接管优先」分支带走；没装插件时它们的记录仍在库里，会落到各自的显式
/// 拒绝（503）而不是 workbuddy 兜底。
///
/// 漏登记的实际代价（生产抓到）：CodeArts 漏的时候用户点「刷新 Token」
/// 得到 `client key [] not found`；Trae 漏的时候得到
/// `12153:refresh token failed:10000:token format error`（2026-10-02 15:47:57 实测）——
/// 两条都长得像"这把凭据坏了"，其实是发错了家的端点。所以**加一家带 refreshToken
/// 的提供商时，这张分派表必须一起加** —— 它在 `PROVIDERS` 与各 adapter 注册点之外，
/// 编译器不会提醒，漏了只在用户点按钮时才暴露。
///
/// 为什么不统一到一个抽象：四家的刷新协议毫无共同点，而「刷新」是**管理动作**、
/// 不是转发链路上的 provider 契约（那条契约的 `refresh_access_token` 是转发时的
/// 同账号重试语义）。分派点只有这一处。
pub async fn refresh_account(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(&body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty());
    let id = match id {
        Some(value) => value,
        // 默认刷新全局真实执行体队首，不借 WorkBuddy 专属会话入口选本体账号。
        None => match state.store().current_account_id() {
            Some(id) => id,
            None => return management_error(400, "当前没有可用账号"),
        },
    };
    logging::verbose("[Accounts]", &format!("刷新账号 token: {id}"));
    // 自定义提供商账号：凭证是用户手填的 apiKey（`custom_accounts`），没有
    // 可刷新的登录态。不拦截的话会落到下面的 workbuddy 刷新链路 ——
    // `get_credentials_by_id` 对只有 `apiKey` 的记录返回 None，用户会得到
    // 一条「账号不存在」，与列表里明明可见的那条自相矛盾。
    if state.store().custom_account_provider(&id).is_some() {
        return management_error(
            400,
            "自定义提供商账号的凭证由用户直接提供，无需刷新（更换凭证请重新添加或直接编辑）",
        );
    }
    // 标准账号按顶层 provider 选择真实 adapter，不探测 secret 或占位 kind。
    if let Some(provider) = state.store().account_provider_of(&id) {
        if crate::server::core::providers::is_executable_provider(&provider) {
            let Some(adapter) =
                crate::server::core::providers::adapter::adapter_by_id(&provider)
            else {
                return management_error(400, format!("未知的提供商：{provider}"));
            };
            return match crate::server::core::credential_maintenance::refresh_checked(
                state.store(),
                adapter.as_ref(),
                &id,
            )
            .await
            {
                Ok(_) => ok_json(json!({
                    "refreshedId": id,
                    "list": state.store().list_visible_accounts(),
                })),
                Err(error) => {
                    logging::log("[Accounts]", &format!("❌ {}", error.message));
                    management_error(i32::from(error.http_status().as_u16()), error.message)
                }
            };
        }
    }
    // Qoder（两个地区各查一次 —— 账号集合按 provider 隔离，同一 id 不可能
    // 同时属于两家，与 Accio / AutoClaw 同一条遍历）
    for region in crate::server::core::providers::qoder::endpoints::Region::ALL {
        if state
            .store()
            .qoder_account_record(region, &id)
            .is_some()
        {
            return refresh_provider_account(state, &id, region.kind()).await;
        }
    }
    // CodeArts：一次性 refresh token + 写回，必须走适配器（实现已搬到插件）。
    // **这条不能省**：漏了就会落到下面的兜底链路，用户得到一条与本家毫无关系
    // 的错误（沙箱端到端实测抓到的原文是
    // `auth/token/refresh 失败: client key [] not found`）。
    if state.store().account_provider_of(&id).as_deref()
        == Some(crate::server::core::providers::kind_id(ProviderKind::CodeArts))
    {
        return refresh_provider_account(state, &id, ProviderKind::CodeArts).await;
    }
    // Trae 的续期已随实现搬到插件：装上插件时上面的 `serves_provider` 分流会
    // 先带走；没装插件时落到下面的 workbuddy 兜底会给出与本家无关的错误，所以
    // 这里按「记录还属于本家」显式拒绝。判据是 provider 归属，不需要读凭据 ——
    // 网关不再解析这家的凭据形状（那部分随实现搬到插件了）。
    if state.store().account_provider_of(&id).as_deref()
        == Some(crate::server::core::providers::kind_id(ProviderKind::Trae))
    {
        return management_error(
            503,
            "Trae 的令牌刷新需要安装提供商插件：本版本不再内置该家的续期实现",
        );
    }
    // ZCode：实现已随插件搬走（它本来就没有续期协议 —— 令牌用坏了只能重新
    // 登录）。这里按「记录还属于本家」显式拒绝：没装插件时落到底下的
    // workbuddy 兜底会给出与本家无关的错误。
    if let Some(provider_id) = state.store().account_provider_of(&id) {
        if crate::server::core::providers::zcode::region::Region::from_provider_id(&provider_id)
            .is_some()
        {
            return management_error(
                503,
                "ZCode 的令牌刷新需要安装提供商插件：本版本不再内置该家的登录链实现",
            );
        }
    }
    // OfficeAce 的续期已随实现搬到插件：装上插件时上面的 `serves_provider`
    // 分流会先带走；没装插件时 `refresh_provider_account` 拿到的是占位适配器，
    // 会如实报「需要安装插件」。
    if state.store().officeace_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::OfficeAce).await;
    }
    // AtomCode：续期走它自己的适配器（`POST {broker}/oauth/refresh`，**会轮换**
    // refresh token，且必须单飞 + 写回）。**这条不能省**：漏了就落到 workbuddy
    // 兜底链路，用户点「刷新 Token」收到的是腾讯那侧的错（与 CodeArts/Trae/
    // OfficeAce 同一条理由），而 AtomGit 的令牌会因此悄悄过期到只能重新登录。
    if state.store().atomcode_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::Atomcode).await;
    }
    // LobsterAI：续期走它自己的适配器（`POST /api/auth/refresh`，**会轮换**，
    // 且必须把 uuid 与两个 keyfrom 原样回传）。**这条不能省**：漏了就落到
    // workbuddy 兜底链路，用户点「刷新 Token」收到的是腾讯那侧的错（与
    // CodeArts/Trae/OfficeAce 同一条理由），而龙虾的令牌会因此
    // 悄悄过期到只能重新网页登录 —— 而那一轮登录是用户手动做的。
    // 实现已搬到插件：没装插件时 `refresh_provider_account` 拿到的是占位适配器，
    // 会如实报「需要安装插件」。
    if state.store().lobsterai_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::LobsterAi).await;
    }
    // Accio（两个地区）的续期已随实现搬到插件：装上插件时上面的
    // `serves_provider` 分流会先带走；没装插件时拿到的是占位适配器，会如实报
    // 「需要安装插件」。两个地区各查一次 —— 账号集合按 provider 隔离。
    for region in crate::server::core::providers::accio::endpoints::Region::ALL {
        if state.store().account_provider_of(&id).as_deref() == Some(region.provider_id()) {
            return refresh_provider_account(state, &id, region.kind()).await;
        }
    }
    // Cline 账号：与 AutoClaw 同一取舍 —— **桌面端实时登录态不主动刷新**。
    // 网关与 Cline 客户端共用同一份 providers.json 里的 refreshToken，
    // 网关侧刷新会让客户端那边的会话作废（两边都在轮换，后写的赢）。
    // 手动账号（自己粘贴的凭证）走适配器的强制刷新链路。
    //
    // 「属于哪一家」由记录自己回答（两个池共用这条链路），因此判据是
    // `cline_account_provider` 而不是某个写死的 provider id —— 池是记录上的
    // 属性，刷新链路对它无差别。
    if let Some(provider_id) = state.store().cline_account_provider(&id) {
        if state.store().cline_is_desktop_account(&id) {
            return management_error(
                400,
                "Cline 桌面端登录态由 Cline 客户端维护，网关不主动刷新：\
                 请在 Cline 客户端重新登录后重新导入，或改用「填写凭证」添加",
            );
        }
        let kind = match crate::server::core::providers::kind_from_id(&provider_id) {
            Some(kind) => kind,
            None => return management_error(400, format!("未知的提供商：{provider_id}")),
        };
        return refresh_provider_account(state, &id, kind).await;
    }
    // CatPaw 账号：没有刷新机制（见本函数的说明）。插件接管后这条判定同样
    // 有效 —— 插件的 `refresh` 能力位也是 false，走适配器会得到同一条结论，
    // 但这里先拦下能省一次 RPC，且文案更具体。
    if state.store().catpaw_account_record(&id).is_some() {
        return management_error(
            400,
            "CatPaw 登录态没有刷新机制：请在 CatPaw 桌面端重新登录，\
             然后在本页重新导入桌面端登录态（或重新粘贴新的登录凭证）",
        );
    }
    // MonkeyCode 的两个站点：同样**没有刷新机制**（session 30 天硬限制，
    // 上游无 refresh 接口，见 `providers::monkeycode::credentials` 的模块头）。
    // 不显式拦下的话会落到下面的 workbuddy 兜底链路，用户会收到一条与腾讯
    // 鉴权端点相关的错误 —— 与 CatPaw 漏登记时的症状同一性质。
    for region in crate::server::core::providers::monkeycode::Region::ALL {
        if state
            .store()
            .monkeycode_account_record(region, &id)
            .is_some()
        {
            return management_error(
                400,
                "MonkeyCode 登录态没有刷新机制（会话 30 天、上游无续期接口）：\
                 请在浏览器重新取一次 session cookie 后在本页重新粘贴",
            );
        }
    }
    // Command Code 的「没有刷新机制」判定已随账号实现搬到插件：插件的
    // `refresh` 方法会对这类账号给出同一条结论，网关不再按 provider 预判。
    // Antigravity：**有**可刷新的 refreshToken（Google OAuth），走自家适配器的
    // 强制刷新（`grant_type=refresh_token` 打 `oauth2.googleapis.com/token`）。
    // 这条分派不能省：漏了就落到下面的 workbuddy 兜底链路，用户点「刷新 Token」
    // 收到的是腾讯鉴权端点的错 —— 与 Command Code / Trae 漏登记时同一性质
    // （`refresh_account` 的文档里把这条列为「加带 refreshToken 的家时必须一起加」）。
    if state.store().antigravity_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::Antigravity).await;
    }
    // ── 兜底：按记录归属的 provider 走适配器 ─────────────────────
    //
    // 走到这里说明上面那些内置家的探测函数都没命中（它们按各家自己的记录
    // 形态判）。最后这一档**不再假设是 WorkBuddy**：取记录上的 provider，
    // 能查到适配器就走它（插件家在上面 `serves_provider` 那档已带走；这里
    // 接住的是「插件没装 / 或某家还没单独登记」的情形，占位适配器会如实报
    // 「需要安装插件」）。查不到适配器就明确报未知，而不是拿 WorkBuddy 的
    // 续期去打腾讯的鉴权端点。
    let provider = match state.store().account_provider_of(&id) {
        Some(provider) => provider,
        None => return management_error(404, "账号不存在"),
    };
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(&provider) else {
        return management_error(
            400,
            format!(
                "{} 的令牌刷新需要安装提供商插件：本版本不再内置该家的续期实现",
                crate::server::core::providers::label_of(&provider)
            ),
        );
    };
    match crate::server::core::credential_maintenance::refresh_checked(
        state.store(),
        adapter.as_ref(),
        &id,
    )
    .await
    {
        Ok(_) => ok_json(json!({
            "refreshedId": id,
            "list": state.store().list_visible_accounts(),
        })),
        Err(error) => {
            logging::log("[Accounts]", &format!("❌ {}", error.message));
            management_error(i32::from(error.http_status().as_u16()), error.message)
        }
    }
}

/// 「读凭证 → 强制刷新 → 回写」这条适配器链路的手动刷新（小浣熊 / AutoClaw 共用）。
///
/// 两家的刷新协议完全不同（各自的 `credentials` / `refresh` 模块），但**管理动作
/// 的形状一致**：拿该账号的凭证、走 `refresh_access_token`（force 语义，不看临期
/// 窗口）、按来源回写。这里是唯一调用点，差异全在各家适配器里。
async fn refresh_provider_account(state: &ServerState, id: &str, kind: ProviderKind) -> Response {
    let adapter = adapter_for(kind);
    match crate::server::core::credential_maintenance::refresh_checked(state.store(), adapter, id)
        .await
    {
        Ok(_) => ok_json(json!({
            "refreshedId": id,
            "list": state.store().list_visible_accounts(),
        })),
        Err(error) => {
            logging::log("[Accounts]", &format!("❌ {}", error.message));
            management_error(i32::from(error.http_status().as_u16()), error.message)
        }
    }
}

// ─── POST /api/accounts/refresh-expiring ────────────────────

/// 刷新**全部**已过期 / 临期的账号凭证（批量维护动作）。
///
/// 与 `/api/accounts/refresh` 的分工：那条是**用户指定一个账号**的强制刷新
/// （不看临期窗口）；这条是**系统遍历全部账号**、只刷确实需要刷的那些。
/// 判定标准与刷新协议都在各家适配器里，本 handler 只透出结果 ——
/// 为什么这条判断不能留在壳侧、为什么失败不给非 2xx，
/// 见 `core::credential_maintenance` 的模块头与本文件那条 400 的说明。
pub async fn refresh_expiring_accounts(state: &ServerState) -> Response {
    let report =
        crate::server::core::credential_maintenance::refresh_expiring_report(state.store()).await;
    // 这一轮已经真刷过一遍了：把定时维护的排期顺延一个间隔（口径与其它手动
    // 入口一致）—— 否则「刚点完维护、到点或重启后又立刻再刷一遍」。
    crate::server::core::credential_maintenance::note_manual_run();
    ok_json(report)
}

// ─── GET /api/accounts/usage 与 POST /api/accounts/checkin ──

/// ── 两条路径的目标解析都不在本文件了 ─────────────────────
/// · 余额查询的目标集合：`core::usage_query::resolve_batch_targets`
///   （它跟着查询逻辑一起下沉 —— 每账号自动查询与手动批量共用同一份口径）；
/// · 签到的目标集合：`core::billing::checkin::resolve_checkin_targets`
///   （定时签到与手动签到必须共用同一段逻辑）。
/// 本文件只剩 `accounts_checkin` 一个转发壳。


/// POST /api/accounts/checkin
///
/// 串行签到：避免多账号同时打上游触发 11-128 风控。
/// id 为空时签全部符合条件的账号，给了 id 则只签该账号；
/// 返回 `{results, succeeded, active, total, skipped}`；`active` 是 WorkBuddy
/// 国际版完成免费模型保活的数量，不等同于普通签到领取成功数。
///
/// 执行体是 `core::billing::checkin::run_checkin` —— 与定时签到共用同一段逻辑
/// （Node 版也是 `createAutoCheckin({ runCheckin: accountRoutes.runCheckin })`）。
/// 差异只有一处：Node 的整个 handler 在 try 里，抛出的 AccountStoreError
/// 走 errorPayload 得到 **OpenAI 风格** body；Rust 侧本文件的历史实现用的是
/// 管理信封（`management_error`）。这里保持**本文件既有形状**不变，避免
/// 切片 6 顺手改掉已交付的契约 —— 两种信封都在 400/404 上，前端只看 message。
pub async fn accounts_checkin(state: &ServerState, body: &Bytes) -> Response {
    let id = match parse_body(body) {
        Ok(payload) => payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.is_empty()),
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    // 批量路径签**全部提供商**（多条目改造后配置勾选属于定时条目，见
    // `batch_providers` 的说明）；指定 id 的单签不受范围限制
    // （见 resolve_checkin_targets 的说明）
    let providers = state.auto_checkin().batch_providers();
    // reason 进签到历史台账（仅批量轮次记账，见 checkin_history 的说明）：
    // 账号页「全部签到」与签到中心的批量动作都从这条路由走，与定时触发的
    // 「到点触发 / 启动补签」在时间线上区分开。
    // source 是签到记录台账的触发来源（单签 single / 批量 manual）。
    let source = if id.is_some() { "single" } else { "manual" };
    match checkin::run_checkin(
        state.store(),
        state.billing(),
        providers.as_slice(),
        id.as_deref(),
        "手动签到",
        source,
    )
    .await
    {
        Ok(result) => ok_json(result),
        Err(error) => management_error(error.status_code, error.message),
    }
}

// ─── GET /api/accounts/connections ──────────────────────────

/// 账号级活跃连接数（账号页「连接数」列）。
///
/// 响应 `{ counts: { <accountId>: <n> } }`：`counts` **只含非零项** ——
/// 界面按「缺失即 0」处理，于是不必为几十个空闲账号各送一个 0，
/// 轮询的响应体也小得多（2 秒一次）。
///
/// 口径是「此刻正在使用这个账号的下游请求数」（一个请求在账号间轮换时计数跟着走），
/// 见 `core::upstream::connections` 的模块头。数据是**进程内内存计数**，
/// 不读盘、不出网，所以这条接口足够便宜，能扛得住 2 秒一次的轮询。
///
/// 这条走 `protected` 分组（`http::router` 里 /api/accounts 整族都挂了 checkApiKey），
/// 与同族的 usage 一致 —— 界面在配置 Key 之前也读不到它。
pub fn account_connections(state: &ServerState) -> Response {
    let counts = state
        .upstream()
        .connections()
        .snapshot()
        .into_iter()
        .map(|(id, count)| (id, Value::from(count)))
        .collect::<Map<String, Value>>();
    ok_json(json!({ "counts": Value::Object(counts) }))
}

// ─── PATCH /api/accounts/{id} ───────────────────────────────

pub async fn patch_account(state: &ServerState, id: &str, body: &Bytes) -> Response {
    if id.is_empty() {
        return management_error(400, "缺少账号 id");
    }
    let patch = match parse_json_body(&body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !patch.is_object() {
        return management_error(400, "请求内容必须是 JSON 对象");
    }
    // CatPaw 的余额查询凭证（`balanceToken`）先落盘：它是**这一家独有**的字段，
    // 通用 `update_account` 不认识它（见 `update_catpaw_balance_token` 的说明：
    // 不把它塞进四家共用的 apply_patch）。放在通用 patch 之前做，于是同一次
    // 「保存设置」里改备注名与改凭证都会生效，而通用 patch 不会因为多出来的
    // 键报错（它按字段白名单取值，未知键本就被忽略）。
    let mut balance_changes: Vec<String> = Vec::new();
    if let Some(value) = patch.get("balanceToken") {
        if state.store().catpaw_account_record(&id).is_none() {
            // 「账号不存在」与「账号存在但不是 CatPaw」要分开说：前者让用户刷新页面，
            // 后者告诉他这个字段不属于这家（静默忽略会让用户以为存进去了）。
            let known = state
                .store()
                .list_accounts()
                .get("accounts")
                .and_then(Value::as_array)
                .map(|accounts| {
                    accounts
                        .iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
                })
                .unwrap_or(false);
            return if known {
                management_error(400, "只有 CatPaw 账号有余额查询凭证（balanceToken）")
            } else {
                store_error(AccountStoreError::new("账号不存在", 404))
            };
        }
        match state.store().update_catpaw_balance_token(&id, value) {
            Ok(changes) => balance_changes = changes,
            Err(error) => return store_error(error),
        }
    }
    // ZCode 的「用哪条上游通道」（`zcodePlan`，见 `providers::zcode::plan`）：
    // 与上面 CatPaw 那一段同一处境 —— 只有这一家认这个键，所以不进通用
    // `apply_patch`，在这里显式落盘并做取值校验（认不出的值 400，而不是
    // 静默退回默认通道）。
    if let Some(value) = patch.get(zcode::PLAN_FIELD) {
        if state.store().zcode_account_record(&id).is_none() {
            let known = state
                .store()
                .list_accounts()
                .get("accounts")
                .and_then(Value::as_array)
                .map(|accounts| {
                    accounts
                        .iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
                })
                .unwrap_or(false);
            return if known {
                management_error(400, "只有 ZCode 账号可以选「使用哪个套餐」（zcodePlan）")
            } else {
                store_error(AccountStoreError::new("账号不存在", 404))
            };
        }
        match state.store().update_zcode_plan(&id, value) {
            Ok(changes) => balance_changes.extend(changes),
            Err(error) => return store_error(error),
        }
    }
    // 自定义账号的凭证（`apiKey` / `noAuth`）**与上游地址覆盖项（`baseUrl`）**：
    // 与上面两段同一处境 —— 只有自定义家认这几个键，所以不进通用 `apply_patch`。
    // **账号 id 由 HTTP 路径给定，改凭证不改 id**（id 是身份：限额冷却、请求日志、
    // 导出文件都按它关联），于是这里不需要「同 key 合并」那条添加侧的逻辑。
    //
    // `baseUrl` 是账号上的**覆盖项**（缺省时转发回落到提供商的地址）：界面用它
    // 表达「改回跟随提供商」（传 null），也是 issue #455 里那份看不见、改不掉的
    // 旧地址的清理入口。
    if patch.get("apiKey").is_some() || patch.get("noAuth").is_some() || patch.get("baseUrl").is_some() {
        // `custom_account_provider` 一次回答两件事：账号在不在、属不属于自定义家
        // （判据是记录里的 `provider` 前缀，不要求提供商本身还在配置里，
        // 见那个函数的说明）—— 与 balanceToken / zcodePlan 的两次查询同口径
        if state.store().custom_account_provider(&id).is_none() {
            let known = state
                .store()
                .list_accounts()
                .get("accounts")
                .and_then(Value::as_array)
                .map(|accounts| {
                    accounts
                        .iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
                })
                .unwrap_or(false);
            return if known {
                management_error(400, "只有自定义提供商的账号有 API Key / 无需鉴权 / 上游地址这几项")
            } else {
                store_error(AccountStoreError::new("账号不存在", 404))
            };
        }
        // 只把请求里**真的带了**的键递下去（`apiKey: null` 是有意义的取值：
        // 清除凭证），其余字段由下面的通用 `update_account` 处理 —— 两者各守
        // 各的字段集，互不干扰
        let mut scoped = Map::new();
        for key in ["apiKey", "noAuth"] {
            if let Some(value) = patch.get(key) {
                scoped.insert(key.to_string(), value.clone());
            }
        }
        if !scoped.is_empty() {
            match state.store().update_custom_credentials(&id, &Value::Object(scoped)) {
                Ok(changes) => balance_changes.extend(changes),
                Err(error) => return store_error(error),
            }
        }
        // 地址覆盖项单独一条路径：`null` / 空串 = 清除（跟随提供商），字符串 =
        // 校验后写入。与凭证互不影响，失败也不该把已改好的凭证一起报成失败 ——
        // 但两者都在同一次保存里，按「先凭证后地址」的顺序各自报错即可
        if let Some(value) = patch.get("baseUrl") {
            match state.store().update_custom_base_url(&id, value) {
                Ok(changes) => balance_changes.extend(changes),
                Err(error) => return store_error(error),
            }
        }
    }
    // 自定义账号的**余额查询配置**（`balanceQuery`）：与上面 apiKey 那一段同一
    // 处境 —— 只有「登记了余额方案的自定义家」认这个键（判据在 core 层：不是
    // 自定义家、或那一家没登记方案，一律 400，见 `update_custom_balance_query`）。
    // 三个密字段（访问令牌 / 密码 / 控制台令牌）只在这一次请求里以明文出现，
    // 落库即被整行加密信封包住（见 `account_store::sql` 的模块头）；响应侧
    // 永远只有 `has*` 标记（见 `store_view::balance_query_public`）。
    if let Some(value) = patch.get("balanceQuery") {
        if state.store().custom_account_provider(&id).is_none() {
            let known = state
                .store()
                .list_accounts()
                .get("accounts")
                .and_then(Value::as_array)
                .map(|accounts| {
                    accounts
                        .iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
                })
                .unwrap_or(false);
            return if known {
                management_error(400, "只有自定义提供商的账号有余额查询配置（balanceQuery）")
            } else {
                store_error(AccountStoreError::new("账号不存在", 404))
            };
        }
        match state.store().update_custom_balance_query(&id, value) {
            Ok(changes) => balance_changes.extend(changes),
            Err(error) => return store_error(error),
        }
    }
    match state.store().update_account(&id, &patch) {
        Ok((account, mut changes)) => {
            changes.extend(balance_changes);
            // 改代理成功后对账：新引用的补监听器、旧引用失去的回收端口
            ok_json(json!({
                "account": account,
                "changes": changes,
                "list": state.store().list_visible_accounts(),
            }))
        }
        Err(error) => store_error(error),
    }
}

// ─── POST /api/accounts/reorder ──────────────────────────────

/// 整队重排（分组模式）：body `{ queue: [账号 id, …] }`，从队首到队尾。
///
/// 账号页的拖拽把结果整条交上来 —— 拖整组 = 改组顺位，拖组内的行 = 改组内顺位，
/// 两者合成一条队列；这里按给定顺序把全局 `priority` 编成 0、1、2…。队列语义没变
/// （仍是「全局一条按优先级升序尝试的队列」，见 `account_store::priority`），分组
/// 只是它的展示口径，所以转发链路一行都不用改。
///
/// 与 `POST /api/accounts/{id}/move`（相邻交换）的分工：那条服务旧的 ↑/↓ 箭头，
/// 一次只换两行；这条服务拖拽，一次定整队。校验（必须是全部账号的一个排列）与
/// 实现见 `account_store::store_order::reorder_accounts`。
pub async fn reorder_accounts(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(items) = payload.get("queue").and_then(Value::as_array) else {
        return management_error(400, "缺少 queue 字段（账号 id 数组，从队首到队尾）");
    };
    // 非字符串项直接忽略（与 batch 的 ids 解析同一宽容口径），全空则报错
    let ids: Vec<String> = items
        .iter()
        .filter_map(Value::as_str)
        .map(|value| value.to_string())
        .collect();
    if ids.is_empty() {
        return management_error(400, "queue 不能为空");
    }
    match state.store().reorder_accounts(&ids) {
        Ok(result) => ok_json(result),
        Err(error) => store_error(error),
    }
}

// ─── POST /api/accounts/{id}/move ───────────────────────────

pub async fn move_account(state: &ServerState, id: &str, body: &Bytes) -> Response {
    if id.is_empty() {
        return management_error(400, "缺少账号 id");
    }
    let payload = match parse_json_body(&body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let direction = if payload.get("direction").and_then(Value::as_str) == Some("down") {
        "down"
    } else {
        "up"
    };
    match state.store().move_account(&id, direction) {
        Ok(result) => {
            if result.get("moved").and_then(Value::as_bool) == Some(false) {
                let reason = result
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("顺序未变");
                logging::log("[Accounts]", &format!("顺序未变（{reason}）"));
            }
            ok_json(result)
        }
        Err(error) => store_error(error),
    }
}

// ─── POST /api/accounts/{id}/rate-limits/clear ─────────────────

/// 清除账号的限流标记：body `{model?}`，给了模型只清那一个，否则全清。
///
/// 这是账号页「限流明细」面板上的动作。语义只是「让本机立刻重新尝试这个模型」：
/// 标记是本机从上游 429 推断出来的冷却期，清掉之后下一次请求若仍被上游限流，
/// 会再次被标记 —— 所以这个动作是安全的，不需要二次确认。
/// 返回 `{cleared: 清掉的模型数, list}`，前端拿 list 直接刷新。
pub fn clear_rate_limits(state: &ServerState, id: &str, body: &Bytes) -> Response {
    if id.is_empty() {
        return management_error(400, "缺少账号 id");
    }
    let payload = if body.is_empty() {
        Value::Object(Default::default())
    } else {
        match parse_json_body(body, "请求内容不是有效 JSON") {
            Ok(value) => value,
            Err(response) => return response,
        }
    };
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let cleared = match model {
        Some(model) => usize::from(state.store().clear_rate_limit(id, model)),
        None => state.store().clear_all_rate_limits(id),
    };
    if cleared > 0 {
        logging::log(
            "[Accounts]",
            &format!(
                "🧹 已清除账号 {id} 的限流标记（{}）",
                model.map(|value| value.to_string()).unwrap_or_else(|| format!("{cleared} 个模型"))
            ),
        );
    }
    ok_json(json!({ "cleared": cleared, "list": state.store().list_visible_accounts() }))
}

// ─── DELETE /api/accounts/{id} ──────────────────────────────

pub fn delete_account(state: &ServerState, id: &str) -> Response {
    if id.is_empty() {
        return management_error(400, "缺少账号 id");
    }
    // 引用快照要在删除**之前**取：被删账号撇下的引用要靠对账回收端口
    // （最后一个引用消失 → 这个节点的监听器该收了）
    match state.store().remove_account(id) {
        Ok(()) => {
            ok_json(json!({ "list": state.store().list_visible_accounts() }))
        }
        Err(error) => store_error(error),
    }
}
