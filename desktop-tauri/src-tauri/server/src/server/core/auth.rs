//! 会话读取、登录态状态与登出（**只剩编排**）。
//!
//! ── 这里原来有什么 ──────────────────────────────────────────
//! 对照 Node 版 `workbuddy-auth.mjs` 的完整实现：会话存储、`buildAuthHeaders`、
//! `refreshSession`（打上游 `/auth/token/refresh`）、12153 判死与
//! `needsRelogin` 标记、`fetchAccounts` / `fetchLoginAccount`、环境变量旁路
//! （`WORKBUDDY_TOKEN`）、`Context` 端点拼装。
//!
//! 那些是「怎么伪装成官方客户端打上游」的知识，按项目的风险隔离要求已整体
//! 搬到提供商插件（`agent2api-providers` 的 `workbuddy` 模块）。本模块现在只
//! 做三件事：
//!
//!   1. **读会话**：从账号库里取「当前账号」的会话形态（`get_current_session`
//!      / `get_current_session_for`），供 `/api/session`、计费编排与转发兜底用；
//!   2. **报状态**：`get_status` / `unconfigured_status` / `get_config_summary`
//!      —— 全是**纯本地**的读数，不发网络；
//!   3. **登出**：`clear_session`（删掉当前账号）。
//!
//! ── 续期去哪了 ──────────────────────────────────────────────
//! 续期是各家自己的协议（端点、头、业务码、轮换语义），现在由适配器
//! （插件）的 `refresh_access_token` 负责。`api/accounts.rs` 的刷新入口按
//! 「账号归属的 provider 是否由插件服务」分流，WorkBuddy 也走那条通用链。
//!
//! ── 环境变量旁路（`WORKBUDDY_TOKEN`）去哪了 ─────────────────
//! 它同样搬到了插件：插件在 `provider.info` 里申报
//! `anonymous_default_session` / `env_credentials`，转发链路在「一个账号都
//! 没有」时向插件要默认登录态的 token（`ensure_access_token(store, "")`）。
//! 网关不再自己读那几个变量 —— 读它就意味着网关知道某一家有这条旁路。

use serde_json::{json, Value};

use crate::server::core::account_store::state::json_number;
use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::logging;

// 传输层与错误类型对本模块的调用方再导出一次（`autoclaw` 等内置家直接用
// `send_raw`；`WorkBuddyAuthError` 是管理接口失败的最小公共形状）。
#[allow(unused_imports)]
pub use crate::server::core::auth_http::{
    send_raw, ApiResponse, WorkBuddyAuthError, REFRESH_WINDOW_MS,
};

// ─── 会话与凭证 ─────────────────────────────────────────────

/// 鉴权服务句柄：账号存储。
///
/// 名字保留 `AuthService` 是为了不改动 `ServerState` 的字段与各处构造点 ——
/// 它现在只是一个「账号库 + 会话派生」的薄封装。
#[derive(Clone)]
pub struct AuthService {
    store: AccountStore,
}

/// 刷新结果（供适配器与路由层共用）。
///
/// 本模块已不做续期，这个类型保留给仍在内置实现里的几家（Raccoon / AutoClaw /
/// CodeArts）与插件侧的会话写回路径复用。
#[derive(Clone, Debug)]
pub struct RefreshedAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<f64>,
    pub refresh_expires_at: Option<f64>,
}

impl AuthService {
    pub fn new(store: AccountStore) -> Self {
        Self { store }
    }

    /// 与 `new` 同一入口（保留旧名，改造前它是「用默认上下文构造」）。
    pub fn for_store(store: AccountStore) -> Self {
        Self::new(store)
    }

    /// 账号库句柄（克隆即共享）。
    ///
    /// 给「手上已经有会话、但要按账号 id 调适配器」的路径用 —— 目前是
    /// `BillingService`：它按会话入参，而 `admin.action` 必须带账号 id 才能
    /// 路由到该 provider 的插件进程。
    pub fn store(&self) -> AccountStore {
        self.store.clone()
    }

    /// 账号数据所在的**文件**（Node 版 `auth.authFile` 的对等字段）。
    ///
    /// 取值来自 `AccountStore::file_string()`，改造后是 **SQLite 库文件**
    /// （`{config_dir}/agent2api.db`）—— 账号记录在它的 `accounts` 表里，
    /// 不再是早期那个 `accounts.json`。面板上的「凭证文件」一栏显示的就是它，
    /// 用户据此知道该备份什么（WAL 模式下还有 `-wal` / `-shm` 两个附属文件）；
    /// 库没打开时会回落到那个库的**约定路径**，而不是空串。
    ///
    /// Windows 下把分隔符统一成 `\`：Node 的 `path.join` 在 Windows 上就是这么
    /// 输出的，而这个路径会直接显示给用户（面板上的「凭证文件」）并给壳侧打开
    /// 目录用，反斜杠形态才与 Node 版一致。
    pub fn auth_file(&self) -> String {
        normalize_display_path(&self.store.file_string())
    }

    /// 当前可用凭证；**不再自动刷新**（续期在插件里，见模块头）。
    ///
    /// 优先级：账号存储当前账号（WorkBuddy 国内版）。
    /// 返回 None 表示没有任何可用凭证（未登录）。
    ///
    /// **转发链路不用它** —— 转发必须只看目标 provider 的账号，
    /// 走 `get_current_session_for`。
    pub async fn get_current_session(&self) -> Result<Option<Value>, WorkBuddyAuthError> {
        self.session_of(None).await
    }

    /// 指定 provider 的当前可用凭证。
    ///
    /// 「当前账号」在该 provider 的账号组内派生（`current_entry_for_provider`），
    /// 而不是全局队首 —— 否则「只有 raccoon 账号」的机器上，
    /// workbuddy 的默认登录态会借到 raccoon 账号的凭证（发出去必然 401）。
    pub async fn get_current_session_for(
        &self,
        provider: &str,
    ) -> Result<Option<Value>, WorkBuddyAuthError> {
        self.session_of(Some(provider)).await
    }

    /// 会话读取的共用内核；`scope` 为 None 表示全局当前账号。
    async fn session_of(&self, scope: Option<&str>) -> Result<Option<Value>, WorkBuddyAuthError> {
        let store = self.store.clone();
        let scope = scope.map(str::to_string);
        let entry = super::blocking::run(move || match scope {
            Some(provider) => store.current_entry_for_provider(&provider),
            None => store.get_current_entry(),
        }).await.map_err(|error| WorkBuddyAuthError::new(error.message))?;
        Ok(entry.map(|entry| entry.session))
    }

    /// 会话状态只读本地凭证；续期由维护任务和实际转发按需触发。
    pub fn get_status(&self) -> Value {
        let session = self.store.get_current_entry().map(|entry| entry.session);
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
            "endpoint": session.get("endpoint").cloned().unwrap_or(Value::Null),
            "prefixPath": session.get("prefixPath").cloned().unwrap_or(Value::Null),
            "edition": session.get("edition").cloned().unwrap_or(Value::Null),
            "platform": session.get("platform").cloned().unwrap_or(Value::Null),
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
            "endpoint": Value::Null,
            "prefixPath": Value::Null,
            "edition": Value::Null,
            "platform": Value::Null,
            "authFile": self.auth_file(),
            "reason": "尚未登录或凭证缺失",
        })
    }

    /// 上游配置摘要（对照 workbuddy-upstream-client.mjs 的 `getConfigSummary`）。
    ///
    /// **纯本地**：只查凭证是否存在，不发任何网络请求。它不做 token 临期刷新，
    /// 否则 /health 会被上游网络状况拖慢 —— 它是壳侧的就绪探针，探一次就出网
    /// 会让「网关是否就绪」取决于上游网络。
    ///
    /// `authApiBase` / `chatApiUrl` 两栏已删除：它们是**上游端点**，网关不再
    /// 持有那套知识（`baseUrl` 仍报记录里存的值，那是账号自己的字段）。
    pub fn get_config_summary(&self) -> Value {
        let Some(entry) = self.store.get_current_entry() else {
            return json!({
                "configured": false,
                "baseUrl": Value::Null,
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
                .cloned()
                .unwrap_or(Value::Null),
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

    // ─── 续期（仅内置家使用）──────────────────────────────────

    /// 单飞表的进程级实例（见 `refresh_flight` 的模块头）。
    ///
    /// 仍在网关里的几家（Raccoon / AutoClaw / CodeArts）各自的 `refresh.rs`
    /// 直接用 `refresh_flight::Table`，所以这里没有包装方法 —— 保留这个
    /// 类型别名只为让上面的 `RefreshedAuth` 有明确归属。

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
///
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
pub fn refresh_result_from(data: &Value) -> RefreshedAuth {
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

/// 刷新用的出网代理类型别名（内置家的刷新签名共用它）。
pub type RefreshProxy = ResolvedProxy;
