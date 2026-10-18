//! 账号管理路由（对照 src/workbuddy-account-routes.mjs 逐条实现）。
//!
//!   GET    /api/accounts                账号列表（含当前账号标记、优先级、启用状态、代理、
//!                                       provider 与顶层 providers 摘要）
//!   POST   /api/accounts                手动添加账号（accessToken/refreshToken JSON）
//!   GET    /api/accounts/export         导出全部账号（含 token，换机器后导入继续用）
//!   POST   /api/accounts/import         导入账号（merge：按 uid 匹配，命中更新、未命中追加）
//!   POST   /api/accounts/current        把账号置顶（仅调整全局优先级）{ id }
//!   POST   /api/accounts/batch          批量操作 { action, ids, proxy?, maxConcurrent? }
//!                                       action: enable / disable / proxy / concurrency / remove
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
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStoreError;
use crate::server::core::auth::WorkBuddyAuthError;
use crate::server::core::billing::checkin;
use crate::server::core::proxies::ProxyConfigError;
use crate::server::core::providers::adapter::adapter_for;
use crate::server::core::providers::kuku::credentials::KukuCredentials;
use crate::server::core::providers::zcode;
use crate::server::core::providers::ProviderKind;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body, query_param};
use crate::server::logging;
use crate::server::ServerState;

/// 账号存储错误 → 管理信封（打日志 + 原状态码）。
///
/// 可见性是 `pub(super)`：`api::session` 的 AutoClaw 验证码登录要落账号
/// （`add_autoclaw_account`），失败时必须是同一个响应形状与同一条日志格式 ——
/// 让那边各写一份会让「登录失败」与「添加失败」在界面上长得不一样。
pub(super) fn store_error(error: AccountStoreError) -> Response {
    logging::log("[Accounts]", &format!("❌ {}", error.message));
    management_error(error.status_code, error.message)
}

fn auth_error(error: WorkBuddyAuthError) -> Response {
    logging::log("[Accounts]", &format!("❌ {}", error.message));
    management_error(error.http_status(), error.message)
}

pub(super) fn proxy_error(error: ProxyConfigError) -> Response {
    logging::log("[Accounts]", &format!("❌ {}", error.message));
    management_error(error.status_code, error.message)
}

/// 解析请求体：空 body 视为 `{}`；非法 JSON 按各处的原始文案报 400。
///
/// Node 版不同分支的文案不一样（「上传内容不是有效 JSON」/「请求内容不是有效 JSON」），
/// 所以文案由调用方传入，不统一成一句话。
fn parse_json_body(body: &Bytes, fallback_message: &str) -> Result<Value, Response> {
    match parse_body(body) {
        Ok(value) => Ok(value),
        Err(_) => Err(management_error(400, fallback_message)),
    }
}

/// 文件导入失败的统一包装：给文案补上「哪个文件」——前端传了 `authFileName`
/// 才显示文件名，没传就说「凭证文件」。
///
/// 为什么在接口层拼、不传进账号存储：文件名是**这次请求的用户上下文**，而
/// 落账号的入口（`add_*_account`）只认内容 —— 让它多收一个名字字符串，等于把
/// HTTP 字段的语义泄进账号存储的解析口径里（同一个入口还要服务「手动粘贴」等
/// 没有文件名的调用方）。
fn describe_file_import_error(
    error: AccountStoreError,
    file_name: Option<&str>,
) -> AccountStoreError {
    AccountStoreError::new(
        format!(
            "凭证文件{}无法导入：{}",
            file_name.map(|name| format!("「{name}」")).unwrap_or_default(),
            error.message,
        ),
        error.status_code,
    )
}

/// `authFileB64`（二进制凭证文件）的统一 400：**只识别、不实现**的几家才调它。
///
/// 接了文本文件导入的几家（小浣熊 / CatPaw / Cline）都会调它：这三家的
/// 凭证文件都是**文本 JSON**，二进制是别家才有的形态（KukuAI 的 Cookies 是
/// SQLite —— 它**实现** `authFileB64`，不调这里，见其分支与
/// `kuku_credentials_from_b64_file`）；别家分支不读文件导入三字段（历史契约不变）。
/// 为什么明确 400 而不是静默忽略：静默会让用户以为导入成功了，而账号根本没落。
fn binary_auth_file_unsupported(provider: &str) -> AccountStoreError {
    AccountStoreError::new(
        format!(
            "该提供商（{}）暂不支持二进制凭证文件（authFileB64），请改用文本内容（authFileContent）",
            crate::server::core::providers::label_of(provider)
        ),
        400,
    )
}

/// KukuAI 文件导入（`authFileB64`）：二进制 Cookies 文件的 base64 → 解析出凭证。
///
/// 为什么是 base64 + 临时文件：KukuAI 的凭证文件是 Cookies 的 SQLite **二进制**，
/// 文本字段（`authFileContent`）装不下它；而读写判定只有
/// `kuku::credentials::read_credentials_from_path` 这一份、按**路径**工作（与本机
/// `importDesktop` 共用），所以先把字节解回临时文件、再走同一条读取链 ——
/// 不另写「从内存字节解析 SQLite」，免得两边对「什么算有效的 Cookies 库」的口径
/// 分叉。
///
/// 临时文件的生命周期只覆盖这一次读取：读取完成**无论成败**立即删除（成功时
/// 凭证已读到内存，失败时内容也不该留在盘上）；base64 解码失败时文件压根没建，
/// 写入失败时顺手清掉可能留下的半截文件。
///
/// 所有失败的文案都经 `describe_file_import_error` 带上原文件名（与另外三家同款），
/// 状态码一律 400（这是「这次请求给的文件有问题」，不是服务端故障）。
fn kuku_credentials_from_b64_file(
    encoded: &str,
    auth_file_name: Option<&str>,
) -> Result<KukuCredentials, AccountStoreError> {
    let bytes = BASE64.decode(encoded).map_err(|error| {
        describe_file_import_error(
            AccountStoreError::new(
                format!("authFileB64 不是合法的 base64（{error}）；请重新选择文件后再试"),
                400,
            ),
            auth_file_name,
        )
    })?;
    let path = kuku_temp_cookies_path();
    if let Err(error) = std::fs::write(&path, &bytes) {
        // 写入失败也可能已经留下半截文件（建文件成功、写盘失败）：顺手清掉，
        // 不留本次请求的残留（清不掉也不阻断，只是垃圾）。
        let _ = std::fs::remove_file(&path);
        return Err(describe_file_import_error(
            AccountStoreError::new(
                format!(
                    "写入临时文件失败（{}）：{error}；请检查系统临时目录是否可写（磁盘空间 / 权限）后重试",
                    path.display()
                ),
                400,
            ),
            auth_file_name,
        ));
    }
    let read_result =
        crate::server::core::providers::kuku::credentials::read_credentials_from_path(&path);
    // 读后即删：失败也要删（见函数说明）—— 这里是最靠近「文件不再需要」的位置，
    // 删一次覆盖后面所有分支（含解析失败）。
    let _ = std::fs::remove_file(&path);
    read_result.map_err(|error| {
        describe_file_import_error(
            AccountStoreError::new(error.file_import_message(), 400),
            auth_file_name,
        )
    })
}

/// KukuAI 文件导入的临时 Cookies 文件路径（系统临时目录 + 随机文件名）。
///
/// 随机段与 `proxy_pool::new_id` / `upstream::request::new_request_id` 同一手法
/// （`RandomState` 的随机种子 + 毫秒 + 进程号，不引第三方依赖）：只要求并发写
/// 不撞名，不承担安全语义（文件读完即删）。
fn kuku_temp_cookies_path() -> std::path::PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(logging::now_ms() as u64);
    hasher.write_u64(std::process::id() as u64);
    let suffix = (hasher.finish() & 0xffff_ffff) as u32;
    std::env::temp_dir().join(format!(
        "kuku-cookies-{}-{}-{suffix:08x}",
        std::process::id(),
        logging::now_ms()
    ))
}

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

/// 账号写、批量改代理 / 删除账号后的**监听器对账**（lazy listeners 的触发点，
/// 见 `api::proxies::reconcile_listeners`）。
///
/// 用「改动前后引用 id 集合」的差集来判：改个备注名 / 改密钥不该付一次内核
/// 重启的等待（重启要重建全部监听器，几秒）。引用增加要补监听器、引用减少
/// 要回收端口（删账号 / 改走别的代理 / 改直连）—— 两个方向都交给对账的缺口
/// 检测收敛，这里不猜方向，只拦「没变化」这一种白重启。
fn reconcile_kernel_listeners(state: &ServerState, before: &std::collections::HashSet<String>) {
    let after = state.store().referenced_pool_ids();
    if &after == before {
        return; // 引用集合没变（纯改配置），不重启
    }
    super::proxies::reconcile_listeners(state);
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
            "GET" => return ok_json(state.store().list_accounts()),
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
        ("GET", "export") => return export_accounts(&state),
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
///   ② `CatPaw` → CatPaw 路径（W5-T-d4）：`importDesktop === true` 导入桌面端
///      实时登录态（`~/.meituan-catpaw/auth.json`）；否则手动添加（token + uid/
///      loginName，兼容粘贴原项目账号记录与桌面端 auth.json 内容）；
///   ③ `AutoClaw` → AutoClaw 路径（W4b-T-c2）：`importDesktop === true` 导入
///      桌面端实时登录态（`%APPDATA%/AutoClaw/auth.json`，DPAPI 解密）；
///      否则手动添加（token/refreshToken + deviceId，`enc:` 密文自动解密）；
///   ④ `WorkBuddy` / `WorkBuddyIntl` 或**字段缺失** → 既有 workbuddy 路径
///      （`store.add_account`，归属由这里的 provider id 给出：国际版与国内版
///      共用同一套凭证形态与落账号路径，差别只在落进哪一家）。
///
/// ── 文件导入（小浣熊 / CatPaw / Cline / KukuAI）──────────────────
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
    // 引用集合快照（成功落账号后对账，见 `reconcile_kernel_listeners`）
    let refs_before = state.store().referenced_pool_ids();
    let provider = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let store = state.store();
    // 三家（小浣熊 / CatPaw / AutoClaw）的分派形状相同：`importDesktop: true`
    // 导入桌面端实时登录态，否则按 payload 手动添加。差异只在调哪个方法，
    // 因此这里把「取 flag + 取 name」收一次，各分支只留一行调用。
    let import_desktop = payload
        .get("importDesktop")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let import_name = name.as_deref();
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
    // 接了这条路径的四家（小浣熊 / CatPaw / Cline / KukuAI）见各自分支，
    // 别家的分支不读这三个字段。
    let auth_file_content = payload
        .get("authFileContent")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let auth_file_b64 = payload
        .get("authFileB64")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let auth_file_name = payload
        .get("authFileName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
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
                "list": store.list_accounts(),
            })),
            Err(error) => store_error(error),
        };
    }
    let result = match crate::server::core::providers::kind_from_id(provider) {
        Some(crate::server::core::providers::ProviderKind::Raccoon) => {
            // ── 文件导入优先于 importDesktop（文件导入契约）────────────
            // 两条路径的语义完全不同，别混用：
            //   - `authFileContent`（文件导入）＝把文件里的**凭证解析后落库**，
            //     得到一个**可独立转发**的账号（文件可来自另一台机器或备份，
            //     导入后不依赖本机客户端）—— 与「手动粘贴」同一入口
            //     `add_raccoon_account`，校验 / 撞 id 保护 / 来源标记全部复用；
            //   - `importDesktop: true` ＝只建一条 `raccoon-desktop` 记录，
            //     记录**不落 token**，转发时实时读本机固定路径
            //     （`~/.box-agent/config/auth.json`）。
            // 两者同时给时以文件为准：用户的显式文件更强。
            // `authFileB64` 本期不实现：二进制凭证是别家（Cookies/加密库）才有
            // 的形态，明确 400 而不是静默忽略（静默会让用户以为导入成功了）。
            if let Some(content) = auth_file_content {
                // 解析失败 → 400；成功 → 与「手动粘贴」同一入口落账号
                // （400 文案统一经 describe_file_import_error 带上文件名）
                match serde_json::from_str::<Value>(content) {
                    Ok(parsed) => store
                        .add_raccoon_account(&parsed, import_name)
                        .map_err(|error| describe_file_import_error(error, auth_file_name)),
                    Err(_) => Err(describe_file_import_error(
                        AccountStoreError::new(
                            "不是合法 JSON（请选择小浣熊客户端登录后生成的 auth.json）",
                            400,
                        ),
                        auth_file_name,
                    )),
                }
            } else if auth_file_b64.is_some() {
                Err(binary_auth_file_unsupported(provider))
            } else if import_desktop {
                store.import_raccoon_desktop_account("manual")
            } else {
                store.add_raccoon_account(&payload, import_name)
            }
        }
        Some(crate::server::core::providers::ProviderKind::CatPaw) => {
            // ── 文件导入优先于 importDesktop（与上一条分支同款）────────
            // `authFileContent`（文件导入）＝把文件里的凭证解析后落库，得到
            // 一个可独立转发的账号（文件可来自任意机器/备份）—— 与「手动粘贴」
            // 同一入口 `add_catpaw_account`（它认嵌套 auth.json：
            // `auth.accessToken` + `account.uid`/`loginName`，见那边的取值候选表）；
            // `importDesktop: true` ＝只建一条 `desktop-auth` 记录，记录**不落
            // token**，转发时实时读本机固定路径（`~/.meituan-catpaw/auth.json`）。
            if let Some(content) = auth_file_content {
                match serde_json::from_str::<Value>(content) {
                    Ok(parsed) => store
                        .add_catpaw_account(&parsed, import_name)
                        .map_err(|error| describe_file_import_error(error, auth_file_name)),
                    Err(_) => Err(describe_file_import_error(
                        AccountStoreError::new(
                            "不是合法 JSON（请选择 CatPaw 客户端登录后生成的 auth.json）",
                            400,
                        ),
                        auth_file_name,
                    )),
                }
            } else if auth_file_b64.is_some() {
                Err(binary_auth_file_unsupported(provider))
            } else if import_desktop {
                store.import_catpaw_desktop_account("manual")
            } else {
                store.add_catpaw_account(&payload, import_name)
            }
        }
        // AutoClaw（W4b-T-c2）：粘贴 token / refreshToken（含 `enc:` 密文自动解密）
        // → 手动添加；`importDesktop: true` → 导入桌面端实时登录态（记录不落 token）。
        //
        // 两个地区走**同一份实现**、按地区参数化（`autoclaw::region`）：账号集合
        // 按 provider 隔离，因此这里的 kind → region 必须逐字对应，不能让国际版
        // 落进国内版的记录里（那会让两家的账号在同一分组里混着，选路也按错误的
        // 域名发请求）。`importDesktop` 两地都给：桌面端那个 auth.json 没有地区
        // 标记、两个构建共用，**地区由用户选的那一项决定**（见
        // `import_autoclaw_desktop_account` 与 `region.rs` 的完整讨论）。
        Some(kind @ (crate::server::core::providers::ProviderKind::AutoClaw
            | crate::server::core::providers::ProviderKind::AutoClawIntl)) => {
            let region = crate::server::core::providers::autoclaw::Region::from_kind(kind)
                .unwrap_or(crate::server::core::providers::autoclaw::Region::Cn);
            if import_desktop {
                store.import_autoclaw_desktop_account(region, "manual")
            } else {
                store.add_autoclaw_account(region, &payload, import_name)
            }
        }
        // CodeArts：粘贴凭证（AK/SK/STS + domain/user + 可选的 refresh token 与
        // oauth_context）。**不调上游**——凭据是登录换来的，添加时没有可交换的
        // 授权码；目录与连通性由刷新链路验。
        // 网页登录（要 DPoP + PKCE 回调）在 M5 后续切片，届时这里加一条分支。
        Some(crate::server::core::providers::ProviderKind::CodeArts) => {
            match crate::server::core::providers::codearts::credentials::Credential::from_payload(&payload) {
                Ok(credential) => store.add_codearts_account(&credential, import_name, "manual"),
                Err(reason) => Err(AccountStoreError::new(reason, 400)),
            }
        }
        // Qoder（两个地区走同一份实现、按地区参数化，与 AutoClaw 同款）：
        // 粘贴 PAT / 凭证 → 手动添加。拆家后界面上是两张卡片（`qoder` /
        // `qoder-intl`），地区由 kind 反查（provider 身份是权威，payload 里
        // 的 `mode` 只是兼容字段）。
        Some(kind @ (crate::server::core::providers::ProviderKind::Qoder
            | crate::server::core::providers::ProviderKind::QoderIntl)) => {
            let region = crate::server::core::providers::qoder::endpoints::Region::from_kind(kind)
                .unwrap_or(crate::server::core::providers::qoder::endpoints::Region::Cn);
            match crate::server::core::providers::qoder::auth::prepare_account(&payload, region).await {
                Ok(credentials) => store.add_qoder_account(region, &credentials, import_name, "manual"),
                Err(error) => Err(AccountStoreError::new(error.message, error.status_code)),
            }
        }
        // Accio（两个地区）：粘贴 accessToken / refreshToken（`mode` 选地区，
        // 缺省国际版）→ 手动添加。「网页登录」是另一条链路（适配器的
        // `supports_web_login` 走 `/api/session/login/start` → OAuth 回调 →
        // 同一个落账号入口 `add_accio_account`）。
        //
        // `importDesktop` 不提供：Accio 桌面端的登录态落在 Electron 会话与 OS
        // 加密区里（没有 auth.json 那种稳定可读的文件形态），给了入口只会稳定
        // 失败 —— 与 AutoClaw「一读一解密」不同，不要照抄那边。
        Some(kind @ (crate::server::core::providers::ProviderKind::Accio
            | crate::server::core::providers::ProviderKind::AccioCn)) => {
            let region = crate::server::core::providers::accio::endpoints::Region::from_kind(kind)
                .unwrap_or(crate::server::core::providers::accio::endpoints::Region::Global);
            if import_desktop {
                return management_error(
                    400,
                    format!(
                        "Accio {}不支持导入桌面端登录态，请用「网页登录」或粘贴凭证添加账号",
                        region.label()
                    ),
                );
            }
            match crate::server::core::providers::accio::auth::prepare_account(&payload).await {
                Ok(credentials) => store.add_accio_account(&credentials, import_name, "manual"),
                Err(error) => Err(AccountStoreError::new(error.message, error.status_code)),
            }
        }
        // Cline：粘贴 accessToken / refreshToken → 手动添加；
        // `importDesktop: true` → 导入桌面端实时登录态（记录不落 token，
        // 实时读 `~/.cline/data/settings/providers.json`）；
        // `authFileContent` → 文件导入（从 providers.json 提取凭证后**落 token**）。
        // 校验（token 非空 / 长度 / 补 `workos:` 前缀 / 从 JWT 取 expiresAt 与
        // 展示名）都在 `add_cline_account` 里，与设备授权登录共用同一入口。
        // **两个池各是一个 provider**（`cline-free` / `cline-pass`），添加时
        // 由 body 的 `provider` 决定进哪一家 —— 不需要额外的池参数。
        Some(kind @ (crate::server::core::providers::ProviderKind::ClineFree
            | crate::server::core::providers::ProviderKind::ClinePass)) => {
            let provider_id = crate::server::core::providers::kind_id(kind);
            // ── 文件导入优先于 importDesktop（文件导入契约）────────────
            // 两条路径的语义完全不同，别混用：
            //   - `authFileContent`（文件导入）＝把用户手选的 `providers.json`
            //     里的凭证**提取出来落库**，得到一个**可独立转发**的账号
            //     （文件可来自另一台机器或备份）—— 与「手动粘贴」同一入口
            //     `add_cline_account`（补 `workos:` 前缀 / 落 token / 撞 id 保护
            //     全部复用）；
            //   - `importDesktop: true` ＝只建一条 `cline-<池>-desktop` 记录，
            //     记录**不落 token**，转发时实时读本机固定路径。
            // 「文件文本 → 凭证」的提取口径与桌面导入共用同一份（见
            // `account_store::cline_accounts` 的 `cline_credentials_from_settings_text`），
            // 不在这里另写一套键名候选 —— 两处口径分叉会让「同一个文件，手选能
            // 导入、本机路径却报格式不对」这种最难排查的形态出现。
            if let Some(content) = auth_file_content {
                store
                    .add_cline_account_from_file(provider_id, content, import_name)
                    .map_err(|error| describe_file_import_error(error, auth_file_name))
            } else if auth_file_b64.is_some() {
                Err(binary_auth_file_unsupported(provider))
            } else if import_desktop {
                store.import_cline_desktop_account(provider_id, import_name)
            } else {
                store.add_cline_account(provider_id, &payload, import_name)
            }
        }
        // ZCode（两个地区）：**粘贴凭证**（`jwt` + `accessToken`）→ 手动添加；
        // 「网页登录」是另一条链路（`api::session::login_start` 的 ZCode 分支 →
        // `core::login::zcode` 的 CLI 轮询任务 → 同一个落账号入口
        // `add_zcode_account`）。
        //
        // ── 为什么两个字段都要收、且只给一个也能落 ───────────────
        // 两者服务**不同的功能**（见 `zcode::credentials` 的模块头）：
        // `accessToken` 转发用、`jwt` 领取用，互相不能替代。用户想测哪一半
        // 就填哪一半 —— 只给了 jwt 的账号能领取不能转发（转发会如实报缺），
        // 反之亦然。这正是「粘贴」这条路相对网页登录的价值：
        // 不必先过一遍 OAuth 就能单独验证领取协议。
        //
        // `importDesktop` 不提供：ZCode 客户端的凭证在它自己的加密存储里，
        // 没有 auth.json 那种稳定可读的形态 —— 给了入口只会稳定失败
        // （与 Accio 同一处境、同一处置，不要照抄 AutoClaw 那边）。
        Some(kind @ (crate::server::core::providers::ProviderKind::Zcode
            | crate::server::core::providers::ProviderKind::ZcodeIntl)) => {
            let region = crate::server::core::providers::zcode::region::Region::from_kind(kind)
                .unwrap_or(crate::server::core::providers::zcode::region::Region::Cn);
            if import_desktop {
                return management_error(
                    400,
                    format!(
                        "ZCode {}不支持导入桌面端登录态，请用「网页登录」或粘贴凭证添加账号",
                        region.label()
                    ),
                );
            }
            let text_field = |key: &str| {
                payload
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("")
                    .to_string()
            };
            let access_token = text_field("accessToken");
            let jwt = text_field("jwt");
            if access_token.is_empty() && jwt.is_empty() {
                return management_error(
                    400,
                    "请至少填写 accessToken（用于转发）或 jwt（用于领取套餐）",
                );
            }
            let credentials = crate::server::core::providers::zcode::credentials::ZcodeCredentials {
                region,
                access_token,
                jwt,
                user_id: text_field("userId"),
                // 设备标识是领取链路的活动期要求（见 `credentials.rs` 的模块头）：
                // 用户没给就新生成一个，随凭证落盘后跨请求稳定
                device_mid: crate::server::core::providers::zcode::credentials::new_device_mid()
                    .unwrap_or_default(),
            };
            store.add_zcode_account(&credentials, import_name, "manual")
        }
        // Trae：**粘贴凭据**（`accessToken` / `refreshToken` / 设备三件套）→ 手动添加。
        // 「网页登录」是另一条链路（`api::session::login_start` 的 Trae 分支 →
        // `core::login::trae` 的回调任务 → 同一个落账号入口 `add_trae_account`）。
        //
        // ── 形状判定为什么交给 `Credential::from_payload` ────────
        // 参考实现的落盘形态是嵌套的 `{type, provider, auth{…}, account{…}}`
        // （CPA 的凭据文件），而手填表单是平铺的。两个位置都读一次是必须的：
        // 只认平铺会让「从 CPA 导出粘贴过来」变成一句"缺 accessToken"，而那份
        // 数据明明是完整的。它同时**拒绝**别家的蛇形键（`access_key_id` 一类），
        // 免得一条有内容的记录被当成空凭据收下。
        //
        // `importDesktop` 不提供：Trae 桌面端的登录态在它自己的加密存储里
        // （与 Accio / ZCode 同一处境），给了入口只会稳定失败。
        Some(crate::server::core::providers::ProviderKind::Trae) => {
            if import_desktop {
                return management_error(
                    400,
                    "Trae 不支持导入桌面端登录态，请用「网页登录」或粘贴凭证添加账号",
                );
            }
            let mut credential =
                match crate::server::core::providers::trae::credentials::Credential::from_payload(&payload) {
                    Ok(credential) => credential,
                    Err(reason) => return management_error(400, reason),
                };
            // 身份是 best-effort 的读数（昵称/uid 只影响显示与去重）：
            // 粘贴的凭据一般不带 uid，这里替用户问一次上游。问不到也照样落账号
            // —— 见 `providers::trae::profile` 模块头，「登录已成功却因为读不到
            // 昵称而判失败」是本家明确要避免的那种行为。
            if credential.uid.is_empty() {
                if let Ok(identity) =
                    crate::server::core::providers::trae::profile::get_user_info(&credential, None).await
                {
                    credential.uid = identity.uid;
                    credential.nickname = identity.nickname;
                    credential.enterprise_id = identity.enterprise_id;
                }
            }
            store.add_trae_account(&credential, import_name, "manual")
        }
        // Loomy（讯飞）：**粘贴 session** → 手动添加（短信登录是另一条链路，
        // `POST /api/session/login/loomy/sms/verify` → 同一个落账号入口
        // `add_loomy_account`）。
        //
        // `importDesktop` 不提供：Loomy 的登录态在客户端自己的加密存储里，
        // 没有 auth.json 那种稳定可读的文件形态 —— 给了入口只会稳定失败
        // （与 Accio / ZCode 同一处境）。
        Some(crate::server::core::providers::ProviderKind::Loomy) => {
            if import_desktop {
                return management_error(
                    400,
                    "Loomy 不支持导入桌面端登录态，请用「手机号验证码登录」或粘贴 session 添加账号",
                );
            }
            store.add_loomy_account(&payload, import_name)
        }
        // KukuAI（百度文库库库 AI）：**粘贴 Cookie**（Cookie 头 / Cookie 编辑器
        // JSON / `{BDUSS, STOKEN}` 对象）→ 解析并向上游验证（userreport 换
        // 会话三件套 + uk）→ 落账号。
        //
        // `importDesktop: true` → 读本机客户端登录态（`%APPDATA%\baidugenflowpro\
        // Network\Cookies`，明文 SQLite 无需解密，见 `kuku::credentials` 模块头）
        // → 同样验证 → 落账号。
        //
        // `authFileB64` → **文件导入**：把用户手选的 Cookies 文件解析出凭证后
        // 落库（来源 `"file"`），文件可来自另一台机器 / 备份，导入后不依赖本机
        // 客户端 —— 与「粘贴 Cookie」落进同一入口 `add_kuku_account`。
        //
        // 验证失败（凭证无效 / 客户端占用 Cookies 文件）→ 400，不落空记录。
        Some(crate::server::core::providers::ProviderKind::Kuku) => {
            // ── 文件导入优先于 importDesktop（文件导入契约；本家吃**二进制**）──
            // KukuAI 的凭证文件是 Cookies 的 SQLite 数据库（二进制），不是 JSON
            // 文本：文件内容走 `authFileB64`（前端读文件字节后 base64），
            // `authFileContent` 对它只是「传错了字段」—— 明确 400 指路 base64，
            // 不静默忽略（会让用户以为导入成功了），也不硬当 Cookie 头解析
            // （那会把「字段选错」伪装成「缺 BDUSS」）。
            //
            // 为什么落临时文件再把路径交给解析：读写判定（rusqlite 打开 + 查
            // `cookies` 表）只有 `read_credentials_from_path` 这一份、按**路径**
            // 工作，让文件导入与 importDesktop 复用同一条读取链（见
            // `kuku_credentials_from_b64_file` 的说明）；临时文件读完即删。
            //
            // 与 importDesktop 的语义差别（别混用）：文件导入＝把用户手选文件里
            // 的**凭证解析后落库**，得到一个**可独立转发**的账号（来源标记
            // `"file"`）；`importDesktop: true` ＝只建一条记录，转发时**实时**读
            // 本机固定路径（来源标记 `"desktop"`，占用冲突由两段式处置解）。
            // 两者同时给时以文件为准：用户的显式文件更强。
            if let Some(encoded) = auth_file_b64 {
                // 两个内容字段同时出现时以 base64 为准：文本形态对 KukuAI 必然
                // 不可用，而 base64 是能走通的那条路（见上）。
                match kuku_credentials_from_b64_file(encoded, auth_file_name) {
                    // 解码 / 临时文件 / 解析失败 → 400（文案已带原文件名）
                    Err(error) => Err(error),
                    Ok(parsed) => {
                        // 与 importDesktop 同一条链：真实校验（userreport 换会话
                        // 三件套 + uk）通过才落账号；网络失败同样 400 —— 要的是
                        // 「现在真的能用」。
                        match crate::server::core::providers::kuku::credentials::enrich_identity(
                            &parsed,
                            None,
                            true,
                        )
                        .await
                        {
                            Ok(verified) => store.add_kuku_account(&verified, import_name, "file"),
                            Err(error) => {
                                Err(AccountStoreError::new(error.message, error.status_code))
                            }
                        }
                    }
                }
            } else if auth_file_content.is_some() {
                Err(describe_file_import_error(
                    AccountStoreError::new(
                        "KukuAI 的桌面凭证是二进制的 Cookies 数据库，不能用文本导入（请用 authFileB64 传文件内容）",
                        400,
                    ),
                    auth_file_name,
                ))
            } else {
                // ── 历史路径（行为逐字不变）：importDesktop → 实时读本机固定
                //    路径；否则把 payload 当粘贴的 Cookie 解析 ──
                let parsed = if import_desktop {
                    match crate::server::core::providers::kuku::credentials::read_desktop_credentials()
                    {
                        Ok(credentials) => credentials,
                        Err(reason) => return management_error(400, reason),
                    }
                } else {
                    match crate::server::core::providers::kuku::credentials::credentials_from_payload(
                        &payload,
                    ) {
                        Ok(credentials) => credentials,
                        Err(reason) => return management_error(400, reason),
                    }
                };
                let verified = match crate::server::core::providers::kuku::credentials::enrich_identity(
                    &parsed,
                    None,
                    true,
                )
                .await
                {
                    Ok(credentials) => credentials,
                    Err(error) => return management_error(error.status_code, error.message),
                };
                store.add_kuku_account(
                    &verified,
                    import_name,
                    if import_desktop { "desktop" } else { "manual" },
                )
            }
        }
        // MonkeyCode（长亭科技）：**粘贴 session**（`monkeycode_ai_session` cookie）
        // → 先向上游 `GET /api/v1/users/status` 校验并归一化，再落账号（`login.rs`
        // 里顺带 best-effort 发现 image_id）。两个站点走同一份实现、按地区参数化
        // （provider id 是权威，见 `monkeycode::region`）。
        //
        // `importDesktop` 不提供：本家的登录态就是浏览器 cookie，没有客户端
        // 「auth.json」那种可读文件（与 Accio / ZCode 同一处境）。
        Some(kind @ (crate::server::core::providers::ProviderKind::MonkeyCode
            | crate::server::core::providers::ProviderKind::MonkeyCodeIntl)) => {
            let region = crate::server::core::providers::monkeycode::Region::from_kind(kind)
                .unwrap_or(crate::server::core::providers::monkeycode::Region::Cn);
            if import_desktop {
                return management_error(
                    400,
                    format!(
                        "MonkeyCode {}不支持导入桌面端登录态，请粘贴 session（monkeycode_ai_session）添加账号",
                        region.label()
                    ),
                );
            }
            let session = payload
                .get("session")
                .or_else(|| payload.get("accessToken"))
                .or_else(|| payload.get("token"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if session.is_empty() {
                return management_error(400, "请粘贴 MonkeyCode 的 session cookie（monkeycode_ai_session）");
            }
            let provided_image = payload
                .get("imageId")
                .or_else(|| payload.get("image_id"))
                .and_then(Value::as_str);
            match crate::server::core::providers::monkeycode::login::verify_session(
                region,
                &session,
                provided_image,
            )
            .await
            {
                Ok(credentials) => {
                    store.add_monkeycode_account(region, &credentials, import_name, "manual")
                }
                Err(error) => return management_error(error.status_code, error.message),
            }
        }
        // Command Code（`api.commandcode.ai`）：**粘贴 API Key**（`user_` 开头）
        // → 本地归一化（剥引号 / `Authorization: Bearer …` 整行 / 尾部杂字段）
        // + 形态检查，随后一次 best-effort 探活（`GET /alpha/billing/credits`
        // 的 401/403 才判无效）→ 落账号。key 本身也是上游的账号身份
        // （没有 userId 可解析，账号 id 由 key 摘要派生，见
        // `commandcode_accounts.rs` 的模块头）。
        //
        // `importDesktop` 不提供：本家的凭证就是一枚 API Key，没有客户端
        // 「auth.json」那种可读登录态（与 Loomy / MonkeyCode 同一处境）。
        Some(crate::server::core::providers::ProviderKind::CommandCode) => {
            if import_desktop {
                return management_error(
                    400,
                    "Command Code 不支持导入桌面端登录态，请粘贴 API Key（user_ 开头）添加账号",
                );
            }
            let raw = payload
                .get("apiKey")
                .or_else(|| payload.get("accessToken"))
                .or_else(|| payload.get("token"))
                .or_else(|| payload.get("key"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let credentials =
                match crate::server::core::providers::commandcode::login::verify_key(raw).await {
                    Ok(credentials) => credentials,
                    Err(error) => return management_error(error.status_code, error.message),
                };
            store.add_commandcode_account(&credentials, import_name, "manual")
        }
        // Antigravity（Google 的 AI IDE）：**粘贴 Google refresh token**
        // → 本地归一化（剥引号 / `refresh_token=` 整行 / 空白；`1//` 是令牌本体，
        // 不剥）→ 打一次 Google token 端点做**真实校验**（refresh_token 没有可判
        // 的本地形态，不像 `user_` 那种有前缀可查），顺带拿到 access_token /
        // 到期时间 → best-effort 发现 `cloudaicompanionProject` → 落账号
        // （见 `providers::antigravity::login` 的模块头）。
        //
        // `importDesktop` 不提供：本家的登录态在 Antigravity IDE 自己的存储里
        // （与 Accio / ZCode / Loomy 同一处境）—— 给了入口只会稳定失败。
        Some(crate::server::core::providers::ProviderKind::Antigravity) => {
            if import_desktop {
                return management_error(
                    400,
                    "Antigravity 不支持导入桌面端登录态，请粘贴 Google refresh token（1// 开头）添加账号",
                );
            }
            let credentials = match crate::server::core::providers::antigravity::login::verify_paste(
                &payload, None,
            )
            .await
            {
                Ok(credentials) => credentials,
                Err(error) => return management_error(error.status_code, error.message),
            };
            store.add_antigravity_account(&credentials, import_name, "manual")
        }
        // OfficeAce（华为云果办 / OfficeClaw）：粘贴模型网关的 Basic 凭据
        // （`baseUrl` + `modelAppKey`/`modelAppSecret`，可选控制面临时凭据）。
        // **不调上游**（与 CodeArts 同口径）——凭据是导入/登录换来的，添加时
        // 没有可交换的授权码；目录与连通性由刷新链路验。
        Some(crate::server::core::providers::ProviderKind::OfficeAce) => {
            // 面板传来的名字是用户自己打的 ⇒ 标 custom（此后续期/重登不改它）
            store.add_officeace_account(&payload, import_name, true)
        }
        // OrcaTerm（腾讯云遨驰终端桌面客户端）：**粘贴 accessToken**（OAuth JWT）
        // → 本地归一化（剥 `Bearer ` / `ot_session=` 前缀；整段 data.bin JSON
        // 直接粘也行）→ 打一次 `GetUserInfo` 做**真实校验**（顺带补 userId 与
        // 昵称；JWT 里本来就有 userId，这一步同时确认 token 没被上游拒绝）→
        // 落账号（续期由网关接管，见 `orcaterm::credentials` 模块头）。
        //
        // `importDesktop: true` → 读桌面客户端登录态
        // （`%APPDATA%\com.orcaterm-desktop.app\data.bin`，Tauri store 明文 JSON
        // 无需解密，见 `orcaterm::credentials::read_desktop_credentials`）→
        // 同样校验 → 落账号。校验失败（token 无效 / 文件不存在）→ 400，不落空记录。
        Some(crate::server::core::providers::ProviderKind::OrcaTerm) => {
            let parsed = if import_desktop {
                match crate::server::core::providers::orcaterm::credentials::read_desktop_credentials()
                {
                    Ok(credentials) => credentials,
                    Err(reason) => return management_error(400, reason),
                }
            } else {
                match crate::server::core::providers::orcaterm::credentials::credentials_from_payload(
                    &payload,
                ) {
                    Ok(credentials) => credentials,
                    Err(reason) => return management_error(400, reason),
                }
            };
            // 真实校验 + 补身份：GetUserInfo 失败（token 无效 / 上游拒绝）→ 400。
            // 网络失败同样 400 —— 添加时验证与 kuku 同一口径（要的是「现在真的能用」）。
            let verified = match crate::server::core::providers::orcaterm::credentials::fetch_user_info(
                &parsed, None,
            )
            .await
            {
                Ok((user_id, nickname)) => {
                    let mut credentials = parsed;
                    if credentials.user_id.is_empty() {
                        credentials.user_id = user_id;
                    }
                    if credentials.nickname.is_empty() {
                        credentials.nickname = nickname;
                    }
                    credentials
                }
                Err(error) => return management_error(error.status_code, error.message),
            };
            store.add_orcaterm_account(
                &verified,
                import_name,
                if import_desktop { "desktop" } else { "manual" },
            )
        }
        // 智谱清言（`chatglm.cn`）：**三条添加路径**，其中「访客模式」是本家独有的
        // （见 `chatglm_accounts.rs` 模块头）：
        //   1. `guest: true`（或既没给令牌也不是 importDesktop）→ 后台空 body 打一次
        //      `guest/access`，拿到一对令牌 + 一个**新设备号**后落账号。
        //      不需要手机号 / 扫码 —— 本家「零配置就能用」的入口。
        //      代价是没有积分体系（余额 / 签到对他们如实跳过）。
        //   2. 粘贴令牌 → 读 `accessToken` / `token`（可从浏览器 Cookie 里取
        //      `chatglm_token`）→ 打一次 `user/info` 做**真实校验**（顺带补 uid
        //      与昵称，并判出它其实是访客还是登录账号）。
        //   3. `importDesktop: true` → 读本机客户端 Cookies（**DPAPI 加密**，
        //      仅 Windows 可解，见 `chatglm::credentials`）→ 同样校验后落账号。
        // 校验失败一律 400（不落空记录）—— 与 OrcaTerm / KukuAI 同一口径：
        // 添加时验证，要的是「现在真的能用」。
        Some(crate::server::core::providers::ProviderKind::Chatglm) => {
            let device_id = crate::server::core::providers::chatglm::credentials::resolve_device_id(
                payload
                    .get("deviceId")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            let plain_token = payload
                .get("accessToken")
                .or_else(|| payload.get("access_token"))
                .or_else(|| payload.get("token"))
                .or_else(|| payload.get("refreshToken"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let want_guest = payload
                .get("guest")
                .and_then(Value::as_bool)
                .unwrap_or(plain_token.is_none() && !import_desktop);
            let provider_label = "智谱清言";

            // ── 路径 3：导入本机客户端登录态 ─────────────────────────
            let (access, refresh) = if import_desktop {
                match crate::server::core::providers::chatglm::credentials::read_desktop_credentials()
                {
                    Ok(Some((access, refresh))) => {
                        let access = (!access.trim().is_empty()).then_some(access);
                        let refresh = (!refresh.trim().is_empty()).then_some(refresh);
                        (access, refresh)
                    }
                    Ok(None) => {
                        return management_error(
                            400,
                            "没有在本机找到智谱清言客户端的登录态（请先打开客户端登录，\
                             或改用「访客模式」/「粘贴令牌」）",
                        )
                    }
                    Err(reason) => return management_error(400, reason),
                }
            } else if want_guest {
                // ── 路径 1：访客换证（本家独有）──────────────────────
                match crate::server::core::providers::chatglm::http::guest_access(
                    &device_id, None,
                )
                .await
                {
                    Ok((access, refresh, _uid)) => (Some(access), Some(refresh)),
                    Err(error) => return management_error(error.status_code, error.message),
                }
            } else {
                // ── 路径 2：粘贴令牌 ────────────────────────────────
                let Some(token) = plain_token else {
                    return management_error(400, "请提供智谱清言的访问令牌（accessToken）");
                };
                // 用户可能整段粘了 `chatglm_token=xxx;` 形态
                let token = token
                    .trim_start_matches("chatglm_token=")
                    .trim_end_matches(';')
                    .trim();
                (Some(token.to_string()), None)
            };
            let Some(access) = access else {
                return management_error(400, "没有取到智谱清言的访问令牌");
            };
            // 真实校验：拿令牌问一次 user/info。顺带读回 uid / 昵称 / 访客标记 ——
            // 这三样决定账号 id、展示名与「有没有积分体系」。
            let verified = match crate::server::core::providers::chatglm::http::user_info(
                &access, &device_id, None,
            )
            .await
            {
                Ok(info) => info,
                Err(error) => return management_error(400, format!("{provider_label}校验失败：{}", error.message)),
            };
            let is_guest = verified
                .get("is_guest")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    crate::server::core::providers::chatglm::credentials::token_is_guest(
                        &access, want_guest,
                    )
                });
            let uid = verified
                .get("_id")
                .or_else(|| verified.get("user_id"))
                .or_else(|| verified.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let nickname = verified
                .get("nickname")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let credentials = crate::server::core::providers::chatglm::credentials::ChatglmCredentials {
                access_token: access,
                // 粘贴路径拿不到 refresh_token：留空，续期会如实报「没有 refresh_token」
                refresh_token: refresh.unwrap_or_default(),
                device_id,
                is_guest,
                uid,
                nickname,
            };
            store.add_chatglm_account(
                &credentials,
                import_name,
                if import_desktop {
                    "desktop"
                } else if want_guest {
                    "guest"
                } else {
                    "manual"
                },
            )
        }
        // WorkBuddy 系的两家（国内版 / 国际版）：同一套凭证形态与落账号路径，
        // 差别只有归属 —— provider id 自己就是归属（拆家后不再从 payload 里的
        // `edition` 反推：那是账号的属性，而落哪一家是身份问题）。
        // `None`（老客户端不带 provider 字段）按国内版，历史契约不变。
        // `importDesktop: true` → 读本机官方客户端（Tencent WorkBuddy）的登录态
        // 文件（`%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\`，国内版
        // `workbuddy-desktop*.info`、国际版 `workbuddy-desktop-ai*.info`，见
        // `account_store::workbuddy_import` 的模块头）；文件内容原样交
        // `add_account` 落库，备注名取 payload 的 `name`（可选）。老客户端不带
        // provider 字段时无从选文件组，直接 400 而不是静默按国内版。
        Some(crate::server::core::providers::ProviderKind::WorkBuddy)
        | Some(crate::server::core::providers::ProviderKind::WorkBuddyIntl)
        | None => {
            let provider = crate::server::core::providers::kind_from_id(provider)
                .map(crate::server::core::providers::kind_id);
            if import_desktop {
                let Some(provider) = provider else {
                    return management_error(
                        400,
                        "导入桌面端登录态需要 provider 字段（workbuddy / workbuddy-intl）",
                    );
                };
                store.import_workbuddy_desktop_account(provider, import_name)
            } else {
                store.add_account(&payload, import_name, provider)
            }
        }
    };
    match result {
        Ok(account) => {
            // 落号成功后对账：新增引用补监听器、失去引用回收端口（lazy listeners）
            reconcile_kernel_listeners(state, &refs_before);
            ok_json(json!({
                "account": account,
                "list": store.list_accounts(),
            }))
        }
        Err(error) => store_error(error),
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

// ─── POST /api/accounts/batch ───────────────────────────────

pub async fn batch_accounts(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(&body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    // 引用集合快照（批量改代理成功后对账，见 `reconcile_kernel_listeners`）
    let refs_before = state.store().referenced_pool_ids();
    // ids 去重 + 过滤空串，逐字照抄 Node 版
    let mut ids: Vec<String> = Vec::new();
    if let Some(items) = payload.get("ids").and_then(Value::as_array) {
        for item in items {
            if let Some(text) = item.as_str() {
                if !text.is_empty() && !ids.iter().any(|existing| existing == text) {
                    ids.push(text.to_string());
                }
            }
        }
    }
    if ids.is_empty() {
        return management_error(400, "缺少要操作的账号 id");
    }
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    let result = match action.as_str() {
        "remove" => state.store().batch_remove(&ids),
        "enable" | "disable" => {
            let enabled = action == "enable";
            state.store().batch_update(&ids, &json!({ "enabled": enabled }))
        }
        "proxy" => {
            // 允许 proxy=null（批量改为直连）；缺字段是显式报错
            let Some(target) = payload.get("proxy") else {
                return management_error(400, "批量修改代理时缺少 proxy 字段");
            };
            state.store().batch_update(&ids, &json!({ "proxy": target }))
        }
        "concurrency" => {
            // 并发上限（0 = 不限）；缺字段是显式报错，值本身由 store 归一
            // （非负整数、封顶 999，与单账号设置面板同一份校验）
            let Some(target) = payload.get("maxConcurrent") else {
                return management_error(400, "批量设置并发上限时缺少 maxConcurrent 字段");
            };
            state
                .store()
                .batch_update(&ids, &json!({ "maxConcurrent": target }))
        }
        other => {
            return management_error(
                400,
                format!(
                    "不支持的批量操作: {}",
                    if other.is_empty() { "(空)" } else { other }
                ),
            )
        }
    };

    match result {
        Ok(value) => {
            // 批量改代理 / 删除成功后对账：补监听器与回收端口都由对账的
            // 缺口检测收敛（见 `reconcile_kernel_listeners`）
            reconcile_kernel_listeners(state, &refs_before);
            // 响应带上 action，与 Node 版 `{ action, ...result, list }` 一致
            let mut merged = Map::new();
            merged.insert("action".to_string(), Value::String(action));
            if let Some(object) = value.as_object() {
                for (key, item) in object {
                    merged.insert(key.clone(), item.clone());
                }
            }
            ok_json(Value::Object(merged))
        }
        Err(error) => store_error(error),
    }
}

// ─── POST /api/accounts/refresh ─────────────────────────────

/// 刷新账号 token（**按账号所属 provider 分派**）。
///
/// workbuddy 账号走既有的 `AuthService::refresh_account`；小浣熊、AutoClaw、
/// CodeArts、**Trae / ZCode**、Accio、Cline 账号各走自家适配器的
/// `refresh_access_token`；**CatPaw / MonkeyCode / Command Code 账号没有可刷新的
/// 东西**（§9.1：`X-Passport-Token` 过期只能在桌面端重新登录，没有 refreshToken；
/// 后两家是静态 session / API Key，上游根本没有续期接口），
/// 因此这里明确报 400 并说明做法 —— 静默走 workbuddy 的刷新会拿别家的凭证
/// 去打腾讯的鉴权接口。
///
/// 漏登记的实际代价（两条都是生产抓到的）：CodeArts 漏的时候用户点「刷新 Token」
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
        None => match state.store().get_active_entry() {
            Some(entry) => entry.id,
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
    // CodeArts：一次性 refresh token + 写回，必须走适配器。
    // **这条不能省**：漏了就会落到下面的 workbuddy 兜底链路，用户得到一条与
    // 本家毫无关系的错误（沙箱端到端实测抓到的原文是
    // `auth/token/refresh 失败: client key [] not found`）。
    if state.store().codearts_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::CodeArts).await;
    }
    // Trae：一次性 refreshToken + 每次换发都轮换，必须走它自己的适配器
    // （`ExchangeToken`）。**这条也不能省**：漏了就落到下面的 workbuddy 兜底，
    // 用户点「刷新 Token」收到的是腾讯那侧的错 —— 生产实测（2026-10-02 15:47:57）
    // 原文是 `auth/token/refresh 失败: 12153:refresh token failed:10000:token format
    // error`，"token format error" 其实就是**把 Trae 的续期串发给了 workbuddy 的
    // 鉴权端点**，与账号本身好不好没有任何关系。
    if state.store().trae_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::Trae).await;
    }
    // ZCode：同一条理由（它的续期是自家 OAuth 那套，不是 workbuddy 的链）
    if state.store().zcode_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::Zcode).await;
    }
    // OfficeAce：控制面临时凭据（约 2 小时）用 refresh token + DPoP 私钥续期，
    // 必须走它自己的适配器。**这条不能省**：漏了就落到 workbuddy 兜底链路，
    // 用户点「刷新 Token」收到的是腾讯那侧的错（与 CodeArts/Trae 同一条理由）。
    if state.store().officeace_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::OfficeAce).await;
    }
    // Accio（两个地区）：走适配器的强制刷新（`POST /api/auth/refresh_token`，
    // 结果按「比较再写」回写）。两个地区各查一次 —— 账号集合按 provider 隔离，
    // 同一 id 不可能同时属于两家（撞 id 在存储层就报错了）。
    for region in crate::server::core::providers::accio::endpoints::Region::ALL {
        if state
            .store()
            .accio_account_record(&id, region.provider_id())
            .is_some()
        {
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
    // 小浣熊账号：走它自己的刷新（结果由 `credentials::refresh` 回写）
    if state.store().raccoon_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::Raccoon).await;
    }
    // AutoClaw 账号：同一套「读凭证 → 强制刷新 → 按来源回写」的适配器链路。
    // **桌面端实时登录态不主动刷新**（原项目 `account-routes.mjs` 的同款拒绝）：
    // 网关与桌面端共用同一个 refresh_token，网关侧刷新会造成轮换竞态，
    // 因此这类账号明确报 400 并说明做法（`autoclaw::credentials` 模块头详述）。
    //
    // 两个地区各查一次（账号集合按 provider 隔离）。顺序无关紧要 —— 同一 id
    // 不可能同时属于两家（撞 id 在存储层就报错了），这里按注册表顺序写，
    // 读起来与 `PROVIDERS` 一致。
    for region in [
        crate::server::core::providers::autoclaw::Region::Cn,
        crate::server::core::providers::autoclaw::Region::Intl,
    ] {
        if let Some(record) = state.store().autoclaw_account_record(region, &id) {
            if record.get("desktop").and_then(Value::as_bool).unwrap_or(false) {
                return management_error(
                    400,
                    "AutoClaw 桌面端登录态由桌面客户端维护，网关不主动刷新：\
                     请在 AutoClaw 桌面端重新登录后重试",
                );
            }
            return refresh_provider_account(state, &id, region.kind()).await;
        }
    }
    // CatPaw 账号：没有刷新机制（见本函数的说明）
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
    // Command Code：同样**没有刷新机制**（单枚静态 API Key、上游无 refresh
    // 接口，见 `providers::commandcode::credentials` 的模块头）。不显式拦下的
    // 话会落到下面的 workbuddy 兜底链路，用户会收到一条与腾讯鉴权端点相关的
    // 错误 —— 与 CatPaw / MonkeyCode 漏登记时的症状同一性质。
    if state.store().commandcode_account_record(&id).is_some() {
        return management_error(
            400,
            "Command Code 的 API Key 没有刷新机制（静态 key、上游无续期接口）：\
             请在 commandcode.ai/studio 重新获取后在本页重新粘贴",
        );
    }
    // Antigravity：**有**可刷新的 refreshToken（Google OAuth），走自家适配器的
    // 强制刷新（`grant_type=refresh_token` 打 `oauth2.googleapis.com/token`）。
    // 这条分派不能省：漏了就落到下面的 workbuddy 兜底链路，用户点「刷新 Token」
    // 收到的是腾讯鉴权端点的错 —— 与 Command Code / Trae 漏登记时同一性质
    // （`refresh_account` 的文档里把这条列为「加带 refreshToken 的家时必须一起加」）。
    if state.store().antigravity_account_record(&id).is_some() {
        return refresh_provider_account(state, &id, ProviderKind::Antigravity).await;
    }
    // WorkBuddy 系（国内版 / 国际版）的兜底：两条链路都按**账号自己的**
    // endpoint / prefixPath / platform / edition 续期
    // （`AuthService::refresh_account_inner` 从记录里读，见那里的说明），
    // 因此拆家后不必按地区分派 —— 国际版账号自然打国际站的 refresh 端点。
    match state.auth().refresh_account(&id).await {
        Ok(_) => ok_json(json!({
            "refreshedId": id,
            "list": state.store().list_accounts(),
        })),
        Err(error) => auth_error(error),
    }
}

/// 「读凭证 → 强制刷新 → 回写」这条适配器链路的手动刷新（小浣熊 / AutoClaw 共用）。
///
/// 两家的刷新协议完全不同（各自的 `credentials` / `refresh` 模块），但**管理动作
/// 的形状一致**：拿该账号的凭证、走 `refresh_access_token`（force 语义，不看临期
/// 窗口）、按来源回写。这里是唯一调用点，差异全在各家适配器里。
async fn refresh_provider_account(state: &ServerState, id: &str, kind: ProviderKind) -> Response {
    let adapter = adapter_for(kind);
    match adapter.refresh_access_token(state.store(), id).await {
        Ok(_) => ok_json(json!({
            "refreshedId": id,
            "list": state.store().list_accounts(),
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
    // 引用集合快照（改代理成功后对账，见 `reconcile_kernel_listeners`）
    let refs_before = state.store().referenced_pool_ids();
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
    // 自定义账号的凭证（`apiKey` / `noAuth`）：与上面两段同一处境 —— 只有自定义
    // 家认这两个键，所以不进通用 `apply_patch`。**账号 id 由 HTTP 路径给定，
    // 改凭证不改 id**（id 是身份：限额冷却、请求日志、导出文件都按它关联），
    // 于是这里不需要「同 key 合并」那条添加侧的逻辑。
    if patch.get("apiKey").is_some() || patch.get("noAuth").is_some() {
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
                management_error(400, "只有自定义提供商的账号有 API Key / 无需鉴权这两项")
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
        match state.store().update_custom_credentials(&id, &Value::Object(scoped)) {
            Ok(changes) => balance_changes.extend(changes),
            Err(error) => return store_error(error),
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
            reconcile_kernel_listeners(state, &refs_before);
            ok_json(json!({
                "account": account,
                "changes": changes,
                "list": state.store().list_accounts(),
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
    ok_json(json!({ "cleared": cleared, "list": state.store().list_accounts() }))
}

// ─── DELETE /api/accounts/{id} ──────────────────────────────

pub fn delete_account(state: &ServerState, id: &str) -> Response {
    if id.is_empty() {
        return management_error(400, "缺少账号 id");
    }
    // 引用快照要在删除**之前**取：被删账号撇下的引用要靠对账回收端口
    // （最后一个引用消失 → 这个节点的监听器该收了）
    let refs_before = state.store().referenced_pool_ids();
    match state.store().remove_account(id) {
        Ok(()) => {
            reconcile_kernel_listeners(state, &refs_before);
            ok_json(json!({ "list": state.store().list_accounts() }))
        }
        Err(error) => store_error(error),
    }
}
