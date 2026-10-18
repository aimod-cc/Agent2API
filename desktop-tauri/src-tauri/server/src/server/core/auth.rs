//! 登录态、token 刷新与鉴权头（对照 src/workbuddy-auth.mjs 的会话部分）。
//!
//! 与 Node 版的分工差异：Node 的 `createWorkBuddyAuth` 把「登录态存储 + 上游管理
//! 请求 + 无头登录 + token 刷新 + 头构造」全放在一个模块里；Rust 侧拆三块：
//!   - `auth_http.rs`：传输层（请求发送 / 响应解包 / 鉴权错误类型）
//!   - 本模块：会话读取（队首账号派生）、getStatus、buildAuthHeaders、
//!     token 刷新与临期主动刷新、清会话
//!   - `login.rs`：无头登录流程与登录任务表
//! 拆分的理由是刷新与登录对「取消 / 并发防重」的要求不同，混在一起会让
//! 两者的锁与任务表互相牵扯。
//!
//! ── 三类前缀（务必区分）──────────────────────────────────
//!   1. 登录/账号：`{endpoint}/v2{prefixPath}/...`（国内版 = /v2/plugin/...）
//!   2. 计费/签到：`{endpoint}/v2/...`（不带 prefixPath）
//!   3. LLM 对话：`{endpoint}/v2/chat/completions`（不带 prefixPath）
//!
//! ── 出网（切片 3 已接入）────────────────────────────────
//! Node 版的请求一律走 `proxyFetch`（按账号挂 undici dispatcher），出口缓存
//! 在 `core::egress`；本模块的 token 刷新按**账号自己的** outlet 走
//! （`session.proxy` 解析出的出口），否则会出现「能转发但刷不了 token」。
//! 登录流程（login.rs）没有账号上下文，因此固定直连 —— 与 Node 版一致。

use serde_json::{json, Value};

use crate::server::core::account_store::state::json_number;
use crate::server::core::account_store::AccountStore;
use crate::server::core::endpoints::{resolve_edition, Context, ANONYMOUS_HEADERS};
use crate::server::core::providers::refresh_flight;
use crate::server::core::proxies::{session_proxy, ResolvedProxy};
use crate::server::logging;

// 传输层与错误类型对本模块的调用方再导出一次：login.rs 与账号路由原来从
// `core::auth` 取这些名字，保留这条路径可以避免它们跟着拆分而改动。
// 登录轮询码 SERVER_CODE_RETRY_FETCH_TOKEN 也在其中 —— login.rs 用它判「token 未就绪」。
// `send_request` 保留导出：登录流程（login.rs）与切片 4 的转发都会用它走直连档。
#[allow(unused_imports)]
pub use crate::server::core::auth_http::{
    send_public_request, send_request, send_request_via, unwrap_public_response,
    unwrap_response, WorkBuddyAuthError, REFRESH_WINDOW_MS, SERVER_CODE_RETRY_FETCH_TOKEN,
};

// ─── 会话与凭证 ─────────────────────────────────────────────

/// 鉴权服务句柄：账号存储 + 默认上下文 + 刷新防重表。
///
/// 防重表记录「正在刷新的账号 id」：Node 版的刷新是「读账号 →
/// 发请求 → 写回」，两个并发请求会各自发一次上游刷新、后写的覆盖先写的；
/// 更糟的是部分上游会对同一 refreshToken 的并发使用做失效处理。
/// 因此这里用一张占位表把并发收敛成一次真实刷新（后到者拿到同一个结果）。
///
/// ── 防重表为什么是**进程级**（Agent2API 改造 W2b-T3）─────────
/// 改造前刷新只从 `ServerState` 那一份 AuthService 发起（克隆共享同一个 Arc
/// 表，等价于进程级）。W2b 之后 provider 适配器的 `ensure_access_token`
/// 与 `refresh_access_token` 也会触发刷新，而适配器契约只拿得到 `&AccountStore`
/// （见 `core::providers::adapter`），于是它必须能自己构造一个 AuthService
/// （`for_store`）。若防重表仍挂在实例上，适配器构造出的那份就与
/// `/api/accounts/refresh` 那份**各有一张表** —— 用户点「刷新」的同一瞬间
/// 恰好有转发在刷新同一账号时，两次真实刷新会并发打出去，
/// 正是这张表要防的事。挪成静态表后，所有实例天然共享同一份占位状态，
/// 行为与改造前（单实例）逐字一致。
#[derive(Clone)]
pub struct AuthService {
    store: AccountStore,
    context: Context,
}

/// 进程级刷新单飞表（见 `AuthService` 的说明）。
///
/// ── 为什么从「占位表」换成 `refresh_flight`（2026-10，issue 同款事故）────
/// 旧实现是一张 `HashMap<String, ()>` 占位表：插入 → `await` 真实刷新 →
/// 无条件 `remove`。它挡住了并发，但有两个致命缺口，两个都真实发生过：
///
///   1. **future 被 drop 时占位永久残留**。`refresh_account` 是 async，
///      调用方（转发链路）的 future 随时可能被取消（客户端断开、超时）。
///      取消点在 `refresh_account_inner(...).await` 那一行之后、`remove`
///      之前，于是这个账号**再也刷不了**，只能重启进程 —— 表现就是
///      「点刷新一直提示正在刷新中」。
///   2. **后到者拿到的是错误而不是结果**。旧实现直接回 409，适配器那边
///      只好「重读 store 碰运气」：刷新还没落盘时读到的是**旧 token**，
///      拿它去重试只会再吃一次 401。
///
/// `refresh_flight::Table` 把这两条都关掉了（RAII 守卫 + 后到者复用结果），
/// 而 Raccoon / AutoClaw / Cline / CodeArts / Accio / Antigravity 六家早就
/// 在用它 —— WorkBuddy 是唯一漏网的一家，因为它是最早写的、刷新链路
/// 独立在 `core::auth` 而不是各家的 `refresh.rs`。
///
/// 值是 `RefreshedAuth`（不是 `()`）：后到者要复用 leader 的结果，
/// 否则只能回 409 或读旧 token。
fn refresh_flight_table() -> &'static refresh_flight::Table<RefreshedAuth> {
    static TABLE: std::sync::OnceLock<refresh_flight::Table<RefreshedAuth>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(refresh_flight::Table::new)
}

/// 单飞键：账号 id + refreshToken 指纹。
///
/// 带上 RT 指纹的理由与另外六家一致：同一账号换号（重新 OAuth 登录）之后
/// 的刷新不该复用旧凭证那一轮的结果。指纹是截断 SHA-256，不进日志。
fn refresh_flight_key(id: &str, refresh_token: &str) -> String {
    format!("workbuddy:{}:{}", id, refresh_flight::fingerprint(refresh_token))
}

/// 上游「refresh token 已作废」的业务码（`12153:refresh token is invalid`）。
///
/// ── 为什么要单独识别它（2026-10-17 上游批量注销会话）────────────────
/// 这个码的含义通常是**确定性失败**：上游已经把这条会话连同 refresh token 一起
/// 注销（issue #215 里 3~7 个账号同时中招），网关侧再刷多少次都不会好。
/// 旧实现把它当普通失败：定时凭证维护每轮都重试、每次都失败，日志刷屏，
/// 而真正该做的事（提示用户重新登录）没人做。
///
/// 但它**也会被临时触发**（网络抖动 / 上游闪断 / 刷新竞态），所以不能见一次就
/// 判死 —— 见 [`REFRESH_TOKEN_INVALID_THRESHOLD`]。
pub const CODE_REFRESH_TOKEN_INVALID: i64 = 12153;

/// 连续多少次 12153 才判定「会话真的死了」（需重新登录）。
///
/// ── 这个数字从哪来（参考项目 workbuddy2api-panel 的实测）────────────
/// 他们的旧实现是「一次 12153 即禁用」，事后排查发现 **13 个被禁用的账号全部
/// refresh 成功** —— 都是误判的受害者。12153 会被网络抖动、上游闪断、refresh
/// 竞态临时触发，一次失败就永久杀号会误杀健康账号。
///
/// 取 3 是他们的结论：容忍偶发抖动，又不会让真正失效的 session 在池里反复被
/// 选中。中间任何一次刷新成功都会把计数清零（`clear_refresh_token_invalid`），
/// 所以「隔几天抖一次」不会慢慢攒到阈值。
pub const REFRESH_TOKEN_INVALID_THRESHOLD: i64 = 3;


/// 刷新结果（供路由层与登录流程共用）
#[derive(Clone, Debug)]
pub struct RefreshedAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<f64>,
    pub refresh_expires_at: Option<f64>,
}

impl AuthService {
    pub fn new(store: AccountStore, context: Context) -> Self {
        Self { store, context }
    }

    /// 用「与默认上下文同源」的上下文构造（Agent2API W2b-T3）。
    ///
    /// provider 适配器与模型目录刷新只拿得到 `&AccountStore`，而 `AuthService`
    /// 还需要一个 `Context`。这里用的就是 `ServerState::bootstrap` 里那份
    /// `endpoints::default_context()` —— 同一进程内两条构造路径的默认上下文
    /// 因此完全一致（token 刷新本身走的是**账号自己的** endpoint，
    /// 这个 context 只用于「账号记录里没有端点时的兜底」）。
    pub fn for_store(store: AccountStore) -> Self {
        Self {
            store,
            context: crate::server::core::endpoints::default_context(),
        }
    }

    /// 默认上下文（未登录态展示与 /api/endpoints 用）
    pub fn default_context(&self) -> &Context {
        &self.context
    }

    /// 账号数据所在的**文件**（Node 版 `auth.authFile` 的对等字段）。
    ///
    /// 取值来自 `AccountStore::file_string()`，改造后是 **SQLite 库文件**
    /// （`{config_dir}/agent2api.db`）—— 账号记录在它的 `accounts` 表里，
    /// 不再是早期那个 `accounts.json`。面板上的「凭证文件」一栏显示的就是它，
    /// 用户据此知道该备份什么（WAL 模式下还有 `-wal` / `-shm` 两个附属文件）；
    /// 库没打开时会回落到那个库的**约定路径**，而不是空串。
    ///
    /// Windows 下把分隔符统一成 `\`：Node 的 `path.join` 在 Windows 上就是这么输出的，
    /// 而这个路径会直接显示给用户（面板上的「凭证文件」）并给壳侧打开目录用，
    /// 反斜杠形态才与 Node 版一致。
    pub fn auth_file(&self) -> String {
        normalize_display_path(&self.store.file_string())
    }

    // ── 环境变量凭证（优先级最高）────────────────────────────
    //
    // Node 版：`WORKBUDDY_TOKEN` 存在时直接用它组会话，完全不看账号库。
    // 这是给脚本/CI 用的旁路入口，保持一致。

    fn env_session() -> Option<Value> {
        let token = std::env::var("WORKBUDDY_TOKEN").ok()?.trim().to_string();
        if token.is_empty() {
            return None;
        }
        let refresh = std::env::var("WORKBUDDY_REFRESH_TOKEN")
            .ok()
            .unwrap_or_default();
        let uid = std::env::var("WORKBUDDY_USER_ID").ok().unwrap_or_default();
        Some(json!({
            "auth": with_expires_at(json!({
                "accessToken": token,
                "refreshToken": refresh,
                "tokenType": "Bearer",
            })),
            "account": { "uid": uid },
        }))
    }

    /// 当前可用凭证；临期自动刷新并回写。
    ///
    /// 优先级：环境变量 WORKBUDDY_TOKEN > 账号存储当前账号。
    /// 返回 None 表示没有任何可用凭证（未登录）。
    ///
    /// 多提供商（W2b-T3）后本函数是 `scope = None` 的薄封装：语义与改造前
    /// 逐字一致（全局派生的当前账号，供 /api/session、计费、模型目录使用）。
    /// **转发链路不用它** —— 转发必须只看目标 provider 的账号，
    /// 走 `get_current_session_for`。
    pub async fn get_current_session(&self) -> Result<Option<Value>, WorkBuddyAuthError> {
        self.session_with_refresh(None).await
    }

    /// 指定 provider 的当前可用凭证（临期自动刷新并回写）。
    ///
    /// 「当前账号」在该 provider 的账号组内派生（`current_entry_for_provider`），
    /// 而不是全局队首 —— 否则「只有 raccoon 账号」的机器上，
    /// workbuddy 的默认登录态会借到 raccoon 账号的凭证（发出去必然 401）。
    ///
    /// 环境变量旁路（`WORKBUDDY_TOKEN`）是 **workbuddy 专属**：只有 scope 是
    /// workbuddy 时才认它（see `env_session`）。
    pub async fn get_current_session_for(
        &self,
        provider: &str,
    ) -> Result<Option<Value>, WorkBuddyAuthError> {
        self.session_with_refresh(Some(provider)).await
    }

    /// 会话读取 + 临期刷新的共用内核；`scope` 为 None 表示全局当前账号。
    async fn session_with_refresh(
        &self,
        scope: Option<&str>,
    ) -> Result<Option<Value>, WorkBuddyAuthError> {
        // 环境变量凭证只属于 workbuddy（WORKBUDDY_TOKEN）
        let env_applies = scope
            .map(|provider| provider == crate::server::core::providers::DEFAULT_PROVIDER_ID)
            .unwrap_or(true);
        if env_applies {
            if let Some(session) = Self::env_session() {
                return Ok(Some(session));
            }
        }
        let entry = match scope {
            Some(provider) => self.store.current_entry_for_provider(provider),
            None => self.store.get_current_entry(),
        };
        let Some(entry) = entry else {
            return Ok(None);
        };
        let expires_at = entry
            .session
            .get("auth")
            .and_then(|auth| auth.get("expiresAt"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let refresh_token = entry
            .session
            .get("auth")
            .and_then(|auth| auth.get("refreshToken"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if is_token_expiring(expires_at) && !refresh_token.is_empty() {
            logging::verbose(
                "[Auth]",
                &format!("token 临期，自动刷新（账号 {}）…", entry.id),
            );
            // 续期走该账号自己的端点与出口，否则会出现「能转发但刷不了 token」
            let refreshed = self.refresh_account(&entry.id).await?;
            let mut session = entry.session;
            if let Some(auth) = session.get_mut("auth") {
                if let Some(object) = auth.as_object_mut() {
                    object.insert(
                        "accessToken".to_string(),
                        Value::String(refreshed.access_token),
                    );
                    if let Some(token) = refreshed.refresh_token {
                        object.insert("refreshToken".to_string(), Value::String(token));
                    }
                    if let Some(value) = refreshed.expires_at {
                        object.insert("expiresAt".to_string(), json_number(value));
                    }
                    if let Some(value) = refreshed.refresh_expires_at {
                        object.insert("refreshExpiresAt".to_string(), json_number(value));
                    }
                }
            }
            return Ok(Some(session));
        }
        Ok(Some(entry.session))
    }

    /// 会话状态只读本地凭证；续期由维护任务和实际转发按需触发。
    pub fn get_status(&self) -> Value {
        let session = Self::env_session()
            .or_else(|| self.store.get_current_entry().map(|entry| entry.session));
        let Some(session) = session else {
            return self.unconfigured_status();
        };
        let expires_at = session
            .get("auth")
            .and_then(|auth| auth.get("expiresAt"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let refresh_token = session
            .get("auth")
            .and_then(|auth| auth.get("refreshToken"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let account = session.get("account").cloned().unwrap_or(Value::Null);
        // 这几个字段 Node 用的是 `|| null` —— 空串也会变成 null（而不仅缺失时）。
        // 界面据此判断「是不是企业账号」（enterpriseId 为 null 就不显示企业标记），
        // 给空串会让所有个人账号都带上一个空的企业标记。
        let nullable_text = |key: &str| -> Value {
            account
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null)
        };
        json!({
            "loggedIn": true,
            "endpoint": session
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or(&self.context.base_url),
            "prefixPath": session
                .get("prefixPath")
                .and_then(Value::as_str)
                .unwrap_or(&self.context.prefix),
            "edition": session
                .get("edition")
                .and_then(Value::as_str)
                .unwrap_or(&self.context.edition),
            "platform": session
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or(&self.context.platform),
            "accountUid": nullable_text("uid"),
            "nickname": nullable_text("nickname"),
            "accountType": account
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("personal"),
            "enterpriseId": nullable_text("enterpriseId"),
            "currentAccountId": self.store.current_account_id(),
            "tokenExpiresAt": if expires_at > 0.0 {
                crate::server::core::account_store::state::json_number(expires_at)
            } else {
                Value::Null
            },
            "canRefresh": !refresh_token.is_empty(),
            "authFile": self.auth_file(),
        })
    }

    /// 未登录态（`/health`、`/api/session` 的 login 字段用它）
    pub fn unconfigured_status(&self) -> Value {
        json!({
            "loggedIn": false,
            "endpoint": self.context.base_url,
            "prefixPath": self.context.prefix,
            "edition": self.context.edition,
            "platform": self.context.platform,
            "authFile": self.auth_file(),
            "reason": "尚未登录或凭证缺失",
        })
    }

    /// 上游配置摘要（对照 workbuddy-upstream-client.mjs 的 `getConfigSummary`）。
    ///
    /// **纯本地**：只查凭证是否存在，不发任何网络请求 —— 所以本切片就能给真值。
    /// 它不做 token 临期刷新（`get_status` 同样只读，见那里的说明），
    /// 否则 /health 会被上游网络状况拖慢 —— 它是壳侧的就绪探针，探一次就出网
    /// 会让「网关是否就绪」取决于上游网络。
    pub fn get_config_summary(&self) -> Value {
        let Some(entry) = self.store.get_current_entry() else {
            return json!({
                "configured": false,
                "baseUrl": self.context.base_url,
                "unavailableReason": UNCONFIGURED_REASON,
            });
        };
        let auth = entry.session.get("auth").cloned().unwrap_or(Value::Null);
        let expires_at = auth.get("expiresAt").and_then(Value::as_f64).unwrap_or(0.0);
        let refresh_token = auth
            .get("refreshToken")
            .and_then(Value::as_str)
            .unwrap_or("");
        let account_uid = entry
            .session
            .get("account")
            .and_then(|account| account.get("uid"))
            .cloned()
            .unwrap_or(Value::Null);
        json!({
            "configured": true,
            "baseUrl": entry
                .session
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or(&self.context.base_url),
            "authApiBase": self.context.auth_api_base(),
            "chatApiUrl": self.context.chat_api_url(),
            "authSource": if refresh_token.is_empty() {
                "环境变量/静态 token"
            } else {
                "auth.json（可自动刷新）"
            },
            "currentAccountId": account_uid,
            "tokenExpiresAt": if expires_at > 0.0 { json_number(expires_at) } else { Value::Null },
            "canRefresh": !refresh_token.is_empty(),
        })
    }

    /// 转发 LLM 请求所需的完整鉴权头（对齐桌面端 buildAuthHeaders）。
    ///
    /// 逐个复刻，**包括条件头**：X-Enterprise-Id / X-Tenant-Id 只在企业账号给、
    /// X-Domain 只在 auth.domain 存在时给、X-Department-Info 只在有
    /// departmentFullName 时给。少给一个头不会被本地测试发现，但上游会拒绝。
    pub fn build_auth_headers(session: &Value) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> = Vec::new();
        let auth = session.get("auth").cloned().unwrap_or(Value::Null);
        let account = session.get("account").cloned().unwrap_or(Value::Null);

        if let Some(uid) = account.get("uid").and_then(Value::as_str) {
            if !uid.is_empty() {
                headers.push(("X-User-Id".to_string(), uid.to_string()));
            }
        }
        if let Some(token) = auth.get("accessToken").and_then(Value::as_str) {
            if !token.is_empty() {
                headers.push(("Authorization".to_string(), format!("Bearer {token}")));
            }
        }
        if let Some(enterprise_id) = account.get("enterpriseId").and_then(Value::as_str) {
            if !enterprise_id.is_empty() {
                headers.push(("X-Enterprise-Id".to_string(), enterprise_id.to_string()));
                headers.push(("X-Tenant-Id".to_string(), enterprise_id.to_string()));
            }
        }
        if let Some(domain) = auth.get("domain").and_then(Value::as_str) {
            if !domain.is_empty() {
                headers.push(("X-Domain".to_string(), domain.to_string()));
            }
        }
        if let Some(department) = account.get("departmentFullName").and_then(Value::as_str) {
            if !department.is_empty() {
                headers.push(("X-Department-Info".to_string(), department.to_string()));
            }
        }
        headers
    }

    /// 企业扩展头（对照 `enterpriseHeaders`）：总是带 X-Domain
    /// （取 auth.domain，缺失时回退端点 authority）。
    fn enterprise_headers(auth: &Value, context: &Context) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(enterprise_id) = auth
            .get("account")
            .and_then(|account| account.get("enterpriseId"))
            .and_then(Value::as_str)
        {
            if !enterprise_id.is_empty() {
                headers.push(("X-Enterprise-Id".to_string(), enterprise_id.to_string()));
                headers.push(("X-Tenant-Id".to_string(), enterprise_id.to_string()));
            }
        }
        let domain = auth
            .get("domain")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| context.authority());
        if !domain.is_empty() {
            headers.push(("X-Domain".to_string(), domain));
        }
        headers
    }

    // ─── token 刷新 ──────────────────────────────────────────

    /// 单次 token 续期（对照 `refreshSession`）：不读盘、不写回，纯请求。
    ///
    /// ctx 决定向哪个端点续期 —— 多账号（国内版/国际版）必须用该账号自己的
    /// endpoint，否则会拿 A 站点的 refreshToken 打 B 站点。
    ///
    /// `proxy` 是该账号的出网代理（Node 版 `refreshSession(auth, ctx, proxy)`），
    /// 传 None 表示直连 —— 续期与转发共用同一出口，否则会出现
    /// 「能转发但刷不了 token」。
    pub async fn refresh_session(
        &self,
        auth: &Value,
        context: &Context,
        proxy: Option<&ResolvedProxy>,
    ) -> Result<RefreshedAuth, WorkBuddyAuthError> {
        let refresh_token = auth
            .get("refreshToken")
            .and_then(Value::as_str)
            .unwrap_or("");
        if refresh_token.is_empty() {
            return Err(WorkBuddyAuthError::new(
                "缺少 refreshToken，无法续期，请重新登录",
            ));
        }
        let mut headers = Self::enterprise_headers(auth, context);
        headers.push(("X-Refresh-Token".to_string(), refresh_token.to_string()));
        headers.push(("X-Auth-Refresh-Source".to_string(), "plugin".to_string()));
        let url = context.auth_url("/auth/token/refresh");
        let response =
            send_request_via("POST", &url, Some(&json!({})), &headers, proxy, None).await?;
        let data = unwrap_response(&response, "auth/token/refresh")?;
        let access_token = data
            .get("accessToken")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if access_token.is_empty() {
            return Err(WorkBuddyAuthError::new("token 刷新响应缺少 accessToken"));
        }
        Ok(refresh_result_from(&data))
    }

    /// 按账号 id 刷新并回写（对照 account-routes.mjs 的 `refreshById`）。
    ///
    /// ── 并发语义（2026-10 重做，见 `refresh_flight_table` 的说明）──────
    /// 同一账号同时只允许一次真实刷新；**后到者等这一轮的结果并复用它**，
    /// 而不是像旧实现那样直接回 409 让上层去「重读 store 碰运气」。
    ///
    /// 这条改动直接对应上游 issue #215 的事故链：refresh token 是**一次性**
    /// 的（换发即消费），两个并发刷新同时带着同一个 RT 去换，第二个必然拿到
    /// `12153:refresh token is invalid`，而它一旦把这个失败当成「这次刷新
    /// 失败」，整条 RT 链就死了 —— 之后无论怎么重试都救不回来，只能重新登录。
    /// 收敛成一次真实请求之后，这个竞态从源头消失。
    ///
    /// 空 `refreshToken` 的账号不进单飞表（没有可刷的东西，直接报错更快）。
    pub async fn refresh_account(&self, id: &str) -> Result<RefreshedAuth, WorkBuddyAuthError> {
        // ── 已判定会话死亡的账号直接早退，不再打上游 ────────────────
        // 连续 N 次 12153 之后 `refresh_account_inner` 会落 `needsRelogin`；
        // 从此再刷新是纯粹的无效请求（上游答案不会变），只会刷日志、拖慢
        // 转发链路。凭证维护已经跳过这类账号，但转发链路的 401 重试仍会走到
        // 这里，所以这道闸必须放在入口。
        //
        // 解除方式只有一个：用户重新登录（换一条 refresh token 链）。
        if self.store.needs_relogin(id) {
            return Err(WorkBuddyAuthError::with_code(
                "该账号的登录态已被上游注销，请重新登录该账号",
                Some(401),
                Some(CODE_REFRESH_TOKEN_INVALID),
            ));
        }
        // 先读一次 RT 只为构造单飞键：没有 RT 的账号走 `refresh_account_inner`
        // 的早退分支（那里给出「没有 refreshToken，无法刷新」的准确文案）。
        let key = match self.store.get_credentials_by_id(id) {
            Some(creds) if !creds.refresh_token.is_empty() => {
                refresh_flight_key(id, &creds.refresh_token)
            }
            // 没有凭证 / 没有 RT：不需要单飞，直接交给 inner 报错
            _ => return self.refresh_account_inner(id).await,
        };
        let outcome = match refresh_flight_table().join(&key) {
            refresh_flight::Join::Leader(guard) => {
                let result = self.refresh_account_inner(id).await;
                // 单飞表存的是 `GatewayError`（六家共用的原语）：先把本家的
                // 错误转成它再落表，返回时再转回来 —— 两条路径（leader / waiter）
                // 因此拿到的是同一份结果，与「后到者复用 leader 结果」的语义一致。
                guard.finish(result.clone().map_err(|error| error.to_gateway_error()));
                result
            }
            refresh_flight::Join::Waiter(waiter) => waiter.wait().await.map_err(|error| {
                WorkBuddyAuthError::with_code(
                    error.message,
                    Some(error.status_code),
                    error.upstream_code,
                )
            }),
        };
        outcome
    }

    async fn refresh_account_inner(&self, id: &str) -> Result<RefreshedAuth, WorkBuddyAuthError> {
        let creds = self
            .store
            .get_credentials_by_id(id)
            .ok_or_else(|| WorkBuddyAuthError::with_status(404, "账号不存在"))?;
        if creds.refresh_token.is_empty() {
            return Err(WorkBuddyAuthError::with_status(
                400,
                format!("账号 {} 没有 refreshToken，无法刷新", creds.name),
            ));
        }
        // 按账号自己的端点续期（国内版/国际版不同站点），出口也按账号的代理走
        let context = Context::make(
            Some(&creds.endpoint),
            Some(&creds.prefix_path),
            Some(&creds.platform),
            Some(&creds.edition),
        );
        // 用账号的完整会话形态作为续期入参：enterpriseId / domain 会变成
        // X-Enterprise-Id / X-Domain 条件头，企业账号缺了它们会被上游拒绝
        let session = self
            .store
            .get_session_by_id(id)
            .map(|entry| entry.session)
            .unwrap_or(Value::Null);
        let auth = session.get("auth").cloned().unwrap_or(Value::Null);
        // 续期也走该账号自己的出口（账号没配代理 → 直连）
        let proxy = session_proxy(&session);
        let next = match self.refresh_session(&auth, &context, proxy.as_ref()).await {
            Ok(next) => next,
            Err(error) => {
                // ── RT 已被上游作废（12153）：连续多次才判定会话死亡 ──────
                //
                // issue #215 里那批「账号被踢出」的结局态：上游把会话连同
                // refresh token 一起注销，网关侧再刷多少次都不会好。旧实现把它当
                // 普通失败，于是凭证维护每轮重试、每轮失败，日志刷屏，而用户
                // 始终不知道「该去重新登录」。
                //
                // ── 为什么不能一次就判死（参考项目踩过的坑）───────────────
                // workbuddy2api-panel 的注释里记着：旧实现「一次 12153 即 Disable」，
                // 事后排查发现 **13 个被禁用的号全部 refresh 成功** —— 全是误判。
                // 12153 会被网络抖动、上游闪断、refresh 竞态临时触发，一次失败就
                // 永久杀号会误杀健康账号。因此这里改成「连续 N 次才判定」，
                // 中间任何一次刷新成功都把计数清零。
                if error.upstream_code == Some(CODE_REFRESH_TOKEN_INVALID)
                    || error.message.contains("refresh token is invalid")
                {
                    let streak = self.store.note_refresh_token_invalid(id);
                    if streak < REFRESH_TOKEN_INVALID_THRESHOLD {
                        logging::verbose(
                            "[Accounts]",
                            &format!(
                                "账号 {} 刷新收到 12153（连续第 {}/{} 次，暂不判定为失效）",
                                creds.name, streak, REFRESH_TOKEN_INVALID_THRESHOLD
                            ),
                        );
                        return Err(error);
                    }
                    self.mark_needs_relogin(
                        id,
                        &creds.name,
                        "refresh token 已失效（上游注销了会话）",
                    );
                    return Err(WorkBuddyAuthError::with_code(
                        format!(
                            "账号 {} 的 refresh token 已被上游作废（连续 {} 次），无法自动续期，请重新登录该账号",
                            creds.name, streak
                        ),
                        Some(401),
                        Some(CODE_REFRESH_TOKEN_INVALID),
                    ));
                }
                return Err(error);
            }
        };
        if next.access_token.is_empty() {
            return Err(WorkBuddyAuthError::new("token 刷新响应缺少 accessToken"));
        }
        self.store.update_account_tokens(
            id,
            Some(&next.access_token),
            next.refresh_token.as_deref(),
            next.expires_at,
            next.refresh_expires_at,
        );
        // 刷新成功 = 账号被证明还活着：清掉「需要重新登录」标记与 12153 连续
        // 计数（用户可能刚重新登录过，也可能上游自己恢复了会话）
        self.clear_needs_relogin(id);
        self.store.clear_refresh_token_invalid(id);
        logging::log(
            "[Accounts]",
            &format!(
                "✅ 账号 token 已刷新: {}（{}…）",
                creds.name,
                crate::server::core::account_store::store_util::truncate_text(&creds.uid, 8)
            ),
        );
        Ok(next)
    }

    /// 标记账号「需要重新登录」（落 `needsRelogin` + 原因，界面据此显示）。
    ///
    /// 写盘失败只记日志：这是个提示性标记，丢了不该让刷新流程报错。
    fn mark_needs_relogin(&self, id: &str, name: &str, reason: &str) {
        let applied = self.store.set_needs_relogin(id, Some(reason));
        logging::log(
            "[Accounts]",
            &format!(
                "⚠️ 账号 {name} 需要重新登录：{reason}{}",
                if applied { "" } else { "（标记未能落盘）" }
            ),
        );
    }

    /// 清掉「需要重新登录」标记（刷新成功 / 重新登录之后调用）。
    fn clear_needs_relogin(&self, id: &str) {
        self.store.set_needs_relogin(id, None);
    }

    /// 手动刷新：当前账号（对照 `refreshStoredSession` 的 store 分支）。
    pub async fn refresh_stored_session(&self) -> Result<Value, WorkBuddyAuthError> {
        let Some(entry) = self.store.get_current_entry() else {
            return Err(WorkBuddyAuthError::new("当前没有可刷新的登录态"));
        };
        let refresh_token = entry
            .session
            .get("auth")
            .and_then(|auth| auth.get("refreshToken"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if refresh_token.is_empty() {
            return Err(WorkBuddyAuthError::new("当前没有可刷新的登录态"));
        }
        let refreshed = self.refresh_account(&entry.id).await?;
        let mut session = entry.session;
        if let Some(object) = session.get_mut("auth").and_then(Value::as_object_mut) {
            object.insert("accessToken".to_string(), Value::String(refreshed.access_token));
            if let Some(token) = refreshed.refresh_token {
                object.insert("refreshToken".to_string(), Value::String(token));
            }
            if let Some(value) = refreshed.expires_at {
                object.insert("expiresAt".to_string(), json_number(value));
            }
            if let Some(value) = refreshed.refresh_expires_at {
                object.insert("refreshExpiresAt".to_string(), json_number(value));
            }
        }
        Ok(session)
    }

    // ─── 账号/登录账号接口（登录流程与转发共用）───────────────

    /// 拉取账号列表（对照 `fetchAccounts`）
    pub async fn fetch_accounts(
        &self,
        auth: &Value,
        context: &Context,
        proxy: Option<&ResolvedProxy>,
    ) -> Result<Vec<Value>, WorkBuddyAuthError> {
        let mut headers = Self::enterprise_headers(auth, context);
        let token = auth
            .get("accessToken")
            .and_then(Value::as_str)
            .unwrap_or("");
        headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        let url = context.auth_url("/accounts");
        let response = send_request_via("GET", &url, None, &headers, proxy, None).await?;
        let data = unwrap_response(&response, "accounts")?;
        Ok(data
            .get("accounts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// 登录后账号详情（对照 `fetchLoginAccount`，④ GET /v2{prefix}/login/account）
    pub async fn fetch_login_account(
        &self,
        auth: &Value,
        state: &str,
        context: &Context,
        proxy: Option<&ResolvedProxy>,
    ) -> Result<Value, WorkBuddyAuthError> {
        let mut headers = Self::enterprise_headers(auth, context);
        let token = auth
            .get("accessToken")
            .and_then(Value::as_str)
            .unwrap_or("");
        headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        headers.push(("X-No-User-Id".to_string(), "true".to_string()));
        headers.push(("X-No-Enterprise-Id".to_string(), "true".to_string()));
        headers.push(("X-No-Department-Info".to_string(), "true".to_string()));
        let url = context.auth_url(&format!("/login/account?state={}", urlencoding(state)));
        let response = send_request_via("GET", &url, None, &headers, proxy, None).await?;
        unwrap_response(&response, "login/account")
    }

    /// 清空登录态。
    ///
    /// store 模式下「清空」= 删掉当前账号（Node 版 `clearSession` 同样如此）：
    /// 当前账号由优先级派生，删掉它之后自动变成下一个可用账号。
    /// 没有任何可用账号时（无操作可做）返回 false，让调用方能如实写日志。
    pub fn clear_session(&self) -> bool {
        let Some(entry) = self.store.get_current_entry() else {
            logging::verbose("[Auth]", "当前没有可用账号，无需清理登录态");
            return false;
        };
        match self.store.remove_account(&entry.id) {
            Ok(()) => {
                logging::log(
                    "[Auth]",
                    &format!("已清除当前登录态（账号 {} 已从列表移除）", entry.id),
                );
                true
            }
            Err(error) => {
                logging::log("[Auth]", &format!("清理登录态失败: {error}"));
                false
            }
        }
    }
}

/// 把路径分隔符统一成 Windows 形态（`C:/a/b` → `C:\a\b`）。
///
/// `PathBuf::to_string_lossy` 在 Windows 上给的是 `C:\a\b`，但 config_dir 可能是
/// 从环境变量（AGENT2API_PROXY_HOME，旧名 WORKBUDDY_PROXY_HOME 仍可读）拼出来的
/// 正斜杠形式。这个串会显示给用户、也会被壳侧用来打开目录，统一成 Node 的
/// `path.join` 输出形态最不容易出错。
fn normalize_display_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}

/// 未登录原因文案（getConfigSummary 用）。
/// Node 版这条是「尚未登录（node server.mjs --login）」；壳内 Rust 版没有
/// 那个命令行入口（登录走 UI 的 /api/session/login/*），用它会指向一个不存在
/// 的命令，因此取与启动横幅一致的措辞。
pub const UNCONFIGURED_REASON: &str = "暂无可用登录态";

/// 在 auth 对象上补 expiresAt / refreshExpiresAt / lastRefreshTime。
/// Node 版 `withExpiresAt`：已有值优先，否则拿 expiresIn（秒）换算。
pub fn with_expires_at(mut auth: Value) -> Value {
    let now = logging::now_ms() as f64;
    let Some(object) = auth.as_object_mut() else {
        return auth;
    };
    let expires_in = object
        .get("expiresIn")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let refresh_expires_in = object
        .get("refreshExpiresIn")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let expires_at = object
        .get("expiresAt")
        .and_then(Value::as_f64)
        .filter(|value| *value != 0.0)
        .unwrap_or_else(|| if expires_in != 0.0 { now + expires_in * 1000.0 } else { 0.0 });
    let refresh_expires_at = object
        .get("refreshExpiresAt")
        .and_then(Value::as_f64)
        .filter(|value| *value != 0.0)
        .unwrap_or_else(|| {
            if refresh_expires_in != 0.0 {
                now + refresh_expires_in * 1000.0
            } else {
                0.0
            }
        });
    let last_refresh = object
        .get("lastRefreshTime")
        .and_then(Value::as_f64)
        .filter(|value| *value != 0.0)
        .unwrap_or(now);
    // 时间戳写成整数形态（json_number）：Node 的 `JSON.stringify` 对
    // `Date.now() + n*1000` 这种整数值不会输出 `.0`，浮点会让 accounts.json
    // 与本机展示的时间戳出现「看起来一样但文本不同」的差异
    object.insert("expiresAt".to_string(), json_number(expires_at));
    object.insert("refreshExpiresAt".to_string(), json_number(refresh_expires_at));
    object.insert("lastRefreshTime".to_string(), json_number(last_refresh));
    auth
}

/// token 是否临期（过期前 5 分钟内视为需要刷新）
pub fn is_token_expiring(expires_at: f64) -> bool {
    if expires_at <= 0.0 {
        return false;
    }
    expires_at - REFRESH_WINDOW_MS <= logging::now_ms() as f64
}

/// 把刷新响应包成 RefreshedAuth（字段缺失时给 None，让调用方保留原值）
fn refresh_result_from(data: &Value) -> RefreshedAuth {
    let enriched = with_expires_at(data.clone());
    RefreshedAuth {
        access_token: enriched
            .get("accessToken")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        refresh_token: enriched
            .get("refreshToken")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        expires_at: enriched
            .get("expiresAt")
            .and_then(Value::as_f64)
            .filter(|value| *value > 0.0),
        refresh_expires_at: enriched
            .get("refreshExpiresAt")
            .and_then(Value::as_f64)
            .filter(|value| *value > 0.0),
    }
}

/// 查询串值的百分号编码（对照 JS 的 `encodeURIComponent`）。
///
/// 自己实现而不是引 url crate 的 form 编码：`encodeURIComponent` 不转义
/// `!'()*` 之外的可打印 ASCII，而 form 编码会把空格写成 '+'。state 是
/// 服务端下发的随机串，两者都能用，但保持与 Node 完全一致的编码更稳妥。
pub fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 匿名请求头（登录流程用）
pub fn anonymous_headers() -> Vec<(String, String)> {
    ANONYMOUS_HEADERS
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// 版本解析的薄封装（登录流程按 edition 决定端点）
pub fn context_for_edition(edition: Option<&str>, endpoint: Option<&str>) -> Context {
    let info = resolve_edition(edition);
    Context::make(
        endpoint.or(Some(info.endpoint)),
        None,
        Some(info.platform),
        Some(info.id),
    )
}
