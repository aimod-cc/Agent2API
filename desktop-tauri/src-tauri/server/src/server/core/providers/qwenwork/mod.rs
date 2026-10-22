//! 千问办公（QwenWork 中国版）适配器：账号管理 **+ 推理转发**。
//!
//! ── 一句话说清这家的形态 ─────────────────────────────────────
//! 它是 Qoder 的换皮（认证链逐字复用），但**推理链路比 Qoder 简单得多**：
//! 上游就是一个标准 OpenAI 兼容网关，因此本家走**通用无状态转发**
//! （`is_stateful` 保持默认 false），不像 Qoder 那样自己发请求。
//!
//! ```text
//!   ① 本机登录态（auth-v2.dat，DPAPI + AES-256-GCM）
//!         └─ 解出 JWT —— 只用于打 gateway.qwenwork.cn
//!   ② POST {gateway}/api/desktop-sessions   （Bearer JWT）
//!         └─ 换出 sk-mm-v1.* MaaS 密钥 + base_url + ttl_ms(2h)
//!   ③ POST {base_url}/v1/chat/completions   （Bearer MaaS 密钥）
//!         └─ 标准 OpenAI chat-completions（含 SSE、reasoning_content）
//! ```
//!
//! ── 与 Qoder 那家的关键差异（同步上游时别照抄）─────────────────
//! | | Qoder | 千问办公 |
//! |---|---|---|
//! | 推理鉴权 | COSY 签名（AES+RSA+MD5，`cosy.rs` 360 行） | 普通 `Bearer sk-mm-v1.*` |
//! | 请求体 | 先编码再签名（`Encode=1`） | 原样 JSON |
//! | 响应 | SSE 外面还包一层 `{statusCodeValue, body}` 信封 | 标准 OpenAI SSE |
//! | 转发方式 | `is_stateful = true`，自己发 | 通用无状态转发 |
//! | 模型目录 | `/algo/api/v2/model/list`（要签名） | `/api/desktop/chat-modes`（普通 Bearer） |
//!
//! 因此本模块**没有** `cosy.rs` / `protocol.rs` / `stream.rs` —— 那些在 Qoder
//! 里存在的理由是「上游不标准」，而千问办公的推理上游是标准的。
//!
//! ── 两级令牌为什么不混用（实测，2026-10-19）───────────────────
//!   - JWT 打 `maas.xiaoluozi.cn` → `401 unrecognized bearer scheme`
//!   - MaaS key 打 `gateway.qwenwork.cn/api/*` → 401
//! 所以 [`ProviderAdapter::build_chat_request`] 用 MaaS key，
//! `list_models` / `query_usage` 用 JWT —— 这是上游要求，不是不一致。
//!
//! ── 模型名（`flash` / `pro` / `qwen3.8-max-preview`）────────────
//! 上游模型 id 是三个短名，不是常见的 `qwen3.8-max`：
//! ```text
//!   flash                「标准」   price_factor 0.1  1M 上下文  is_vl
//!   pro                  「高级」   price_factor 1.0  1M 上下文
//!   qwen3.8-max-preview  「千问最强」price_factor 1.8  1M 上下文  is_vl
//! ```
//! 三者 `format` 全是 `openai/chat-completions`、`harness` 全是 `oc`。
//! **不要**把它们改写成 `qwen3.8-max` 之类 —— 上游只认 `key` 字段的原值。
//!
//! ── 额度在哪看 ─────────────────────────────────────────────
//! `GET {gateway}/api/v2/quota/usage`（普通 Bearer JWT）回
//! `total_usage_percentage` / `is_quota_exceeded` / `user_quota` / `add_on_quota`。
//! 实测免费版这三个数值字段都是 null（用百分比表达），见 [`usage`]。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本模块在对话链路上，绝不 unwrap/expect/panic。

pub mod auth;
pub mod checkin;
pub mod crypt;
pub mod endpoints;
pub mod models;
pub mod oauth;
pub mod usage;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use super::ProviderKind;
use super::refresh_flight;

/// 控制面请求（打 `gateway.qwenwork.cn` 的那一路）的**总超时**。
///
/// ── 为什么每一处都要显式设 ────────────────────────────────────
/// `egress` 刻意**不设**总超时：它只给 connect / read 后备，而 read 后备默认
/// 是 600 秒。适配器的契约（`adapter.rs` 里那段「超时由实现自己设」）要求
/// 各家按自己的上游形态定，源项目给的是 15~20 秒。本家的控制面全是轻量
/// 查询（签发密钥 / 查额度 / 拉目录 / 签到），20 秒足够，挂住则早失败早提示
/// —— 否则一次卡住的余额查询会让界面转圈十分钟。
///
/// 与 `codearts/balance.rs`（20s）、`cline/models.rs`（30s）同档。
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(20);

/// 千问办公适配器（无状态单例）。
pub struct QwenworkAdapter;

/// 全局唯一实例（`adapter_for` 返回它的引用）。
pub static QWENWORK_ADAPTER: QwenworkAdapter = QwenworkAdapter;

/// MaaS 密钥的进程内缓存：`user_id → MaasKey`。
///
/// ── 为什么要缓存 ───────────────────────────────────────────
/// 签发一次 MaaS 密钥要打一次 `POST /api/desktop-sessions`（HTTPS 往返 +
/// 服务端会话创建）。一次对话可能连发几十条请求，每条都签一次既慢又招风控，
/// 而密钥本身有 2 小时有效期 —— 天然适合按用户缓存。
///
/// ── 为什么用 `Mutex<HashMap>` 而不是 `OnceLock<MaasKey>` ─────
/// 多账号是常态（用户可能同时挂多个千问办公账号做轮询），而且不同账号的
/// 密钥互相独立。`OnceLock` 只能存一个，会退化成「换账号就重新签发」。
///
/// 这里**刻意**不引第三方缓存 crate：键只有 user_id（几十个量级）、
/// 过期靠 `MaasKey::is_usable` 现场判断，一把 std 的 Mutex 足够。
static MAAS_CACHE: OnceLock<Mutex<HashMap<String, auth::MaasKey>>> = OnceLock::new();

fn maas_cache() -> &'static Mutex<HashMap<String, auth::MaasKey>> {
    MAAS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// MaaS 签发的单飞表（进程级；表里**只有在飞的签发**，本轮结束即移除，
/// 见 `refresh_flight` 的模块头）。
///
/// 与上面的密钥缓存分工明确：缓存管「已经签好的」（跨请求长期复用），
/// 本表管「正在签的」（同一账号同一枚 JWT 的并发只打一次上游）。
fn maas_flight() -> &'static refresh_flight::Table<auth::MaasKey> {
    static TABLE: OnceLock<refresh_flight::Table<auth::MaasKey>> = OnceLock::new();
    TABLE.get_or_init(refresh_flight::Table::new)
}

impl QwenworkAdapter {
    /// 取（必要时签发）某个账号的 MaaS 密钥。
    ///
    /// 单飞：同一账号（同一枚 JWT）的并发请求只会签发一次 —— 后到的请求等
    /// 这一轮的结果，而不是各自去打一次上游。单飞原语、取消处理与锁纪律都在
    /// `refresh_flight`（Raccoon / AutoClaw 共用的那个），这里只负责 key 的
    /// 构成。签发失败不写缓存（下次重试）。
    async fn maas_key_for(&self, jwt: &str, user_id: &str) -> Result<auth::MaasKey, GatewayError> {
        let now = auth::now_ms();
        {
            // 临界区只做一次 HashMap 读，不跨 await
            let cache = maas_cache()
                .lock()
                .map_err(|_| GatewayError::with_status(500, "千问办公密钥缓存锁已中毒"))?;
            if let Some(key) = cache.get(user_id) {
                if key.is_usable(now) {
                    return Ok(key.clone());
                }
            }
        }
        // key = `userId:JWT 指纹`：换账号 / 重新登录（JWT 变）不该并进同一格，
        // 否则新登录态的请求会拿到旧登录态签出来的密钥（与 AutoClaw 的
        // `refresh_key` 同一构成理由；指纹让完整 JWT 不进 key、不打印）
        let flight_key = format!("{user_id}:{}", refresh_flight::fingerprint(jwt));
        let guard = match maas_flight().join(&flight_key) {
            // 已加入同一轮：结果（含缓存写回）由 leader 负责，这里只等它
            refresh_flight::Join::Waiter(waiter) => return waiter.wait().await,
            refresh_flight::Join::Leader(guard) => guard,
        };
        // 双检：抢到这一格的这会儿，上一轮 leader 可能已经落好一份可用密钥
        // （它的表项刚移除，否则这里会是 Waiter）—— 再打一次上游就白花了。
        // 顺手把这份结果交给可能已加入本格的等待者（提前 return 会让守卫走
        // Drop 的取消路径，等待者拿到 409 而不是可用密钥）
        let cached = {
            let cache = maas_cache()
                .lock()
                .map_err(|_| GatewayError::with_status(500, "千问办公密钥缓存锁已中毒"))?;
            cache
                .get(user_id)
                .filter(|key| key.is_usable(auth::now_ms()))
                .cloned()
        };
        if let Some(key) = cached {
            guard.finish(Ok(key.clone()));
            return Ok(key);
        }
        let result = self.mint(jwt).await;
        if let Ok(key) = &result {
            if let Ok(mut cache) = maas_cache().lock() {
                cache.insert(user_id.to_string(), key.clone());
            }
        }
        guard.finish(result.clone());
        result
    }

    /// 打一次签发接口。
    async fn mint(&self, jwt: &str) -> Result<auth::MaasKey, GatewayError> {
        let url = endpoints::gateway_url(endpoints::MINT_PATH);
        // 走系统代理：这是**控制面**请求（打 gateway 域名），与推理面同一出口
        // 选择，避免「推理能通、签发不通」的分裂（用户配了代理就全局生效）。
        let client = crate::server::core::egress::client_for_system_proxy();
        let response = client
            .post(&url)
            .timeout(CONTROL_TIMEOUT)
            .header("Authorization", format!("Bearer {jwt}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", user_agent())
            .json(&json!({}))
            .send()
            .await
            .map_err(|err| {
                GatewayError::with_status(502, format!("千问办公签发接口不可达：{err}"))
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            logging::log_event(
                "warn",
                "qwenwork",
                "MaaS 密钥签发失败",
                Some(json!({ "status": status, "body": truncate(&text, 300) })),
            );
            let hint = match status {
                401 | 403 => "登录态被网关拒绝，请在「账号」页重新从本机导入（先在千问办公客户端登录一次）",
                404 => "签发接口不存在：上游可能已改路径，请检查 endpoints.rs 的 MINT_PATH",
                503 => "上游未开启桌面端签发能力（服务端 SESSION_MGMT_DESKTOP_MAAS_ENABLED 关闭）",
                _ => "签发 MaaS 密钥失败",
            };
            return Err(GatewayError::with_status(
                if status == 401 || status == 403 { 401 } else { 502 },
                format!("{hint}（HTTP {status}）"),
            ));
        }
        let body: Value = serde_json::from_str(&text).map_err(|_| {
            GatewayError::with_status(502, "千问办公签发接口返回的不是合法 JSON")
        })?;
        let key = auth::MaasKey::from_mint_response(&body)?;
        logging::log_event(
            "info",
            "qwenwork",
            "MaaS 密钥签发成功",
            Some(json!({
                "base_url": key.base_url,
                "ttl_ms": if key.expires_at_ms > 0 { key.expires_at_ms - auth::now_ms() } else { 0 },
            })),
        );
        Ok(key)
    }

    /// 清掉某账号（或全部）的缓存密钥。401 后强制刷新时用。
    pub fn invalidate_maas_cache(user_id: Option<&str>) {
        if let Ok(mut cache) = maas_cache().lock() {
            match user_id {
                Some(id) => {
                    cache.remove(id);
                }
                None => cache.clear(),
            }
        }
    }

    /// 拿 JWT 问一次 `{gateway}/api/v1/userinfo`，做「添加账号」时的**真实校验**。
    ///
    /// ── 为什么要真打一次而不是只看 JWT 的 payload ────────────────
    /// JWT 解出来只能说明「格式对、未过期」，说明不了上游**还认不认**它
    /// （改密码 / 登出 / 被风控都会让服务端先失效，而 JWT 的 exp 还没到）。
    /// 与 OrcaTerm / KukuAI / 智谱清言同一口径：添加时验证，要的是「现在真的能用」。
    ///
    /// 顺带把服务端返回的昵称 / 套餐名补进凭据 —— 界面上的展示名与
    /// 「个人免费版 / 标准版」全从这里来，不从 JWT 里猜。
    ///
    /// ── 失败为什么回 401 而不是 502 ────────────────────────────
    /// 走到这里的调用方是「添加账号」，失败原因是**用户给的那枚令牌不可用**
    /// （不是我们的网出了问题），401 才会让界面提示「重新登录」而不是「稍后重试」。
    pub async fn verify_and_enrich(
        jwt: &str,
        mut credentials: auth::Credentials,
    ) -> Result<auth::Credentials, GatewayError> {
        let url = endpoints::gateway_url(endpoints::USERINFO_PATH);
        let client = crate::server::core::egress::client_for_system_proxy();
        let response = client
            .get(&url)
            .timeout(CONTROL_TIMEOUT)
            .header("Authorization", format!("Bearer {jwt}"))
            .header("Accept", "application/json")
            .header("User-Agent", user_agent())
            .send()
            .await
            .map_err(|err| {
                GatewayError::with_status(502, format!("千问办公用户信息接口不可达：{err}"))
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            logging::log_event(
                "warn",
                "qwenwork",
                "登录态校验失败",
                Some(json!({ "status": status, "body": truncate(&text, 300) })),
            );
            return Err(GatewayError::with_status(
                if status == 401 || status == 403 { 401 } else { 502 },
                match status {
                    401 | 403 => {
                        "千问办公登录态无效或已过期，请在客户端重新登录后再导入".to_string()
                    }
                    _ => format!("千问办公用户信息接口返回 HTTP {status}"),
                },
            ));
        }
        let Ok(body) = serde_json::from_str::<Value>(&text) else {
            // 校验已经通过（200），只是响应体解析不了 —— 不该因此拒绝导入，
            // 用 JWT 里解出来的那份信息照常落库。
            return Ok(credentials);
        };
        // 上游可能把字段包在 `data` / `user` 里，逐层找
        let node = body
            .get("data")
            .filter(|v| v.is_object())
            .or_else(|| body.get("user").filter(|v| v.is_object()))
            .unwrap_or(&body);
        let name = auth::first_string(node, &["nickname", "name", "display_name", "displayName"]);
        if !name.is_empty() {
            credentials.name = name.chars().take(100).collect();
        }
        let username = auth::first_string(node, &["username", "email", "phone", "login"]);
        if !username.is_empty() {
            credentials.username = username.chars().take(100).collect();
        }
        let plan = auth::first_string(node, &["plan_name", "planName", "plan", "tier", "user_type"]);
        if !plan.is_empty() {
            credentials.plan_name = plan.chars().take(100).collect();
        }
        // 服务端给的 id 更权威（JWT 的 sub 可能只是登录会话 id）
        let id = auth::first_string(node, &["id", "user_id", "userId", "uid"]);
        if !id.is_empty() && credentials.user_id.is_empty() {
            credentials.user_id = id.chars().take(64).collect();
        }
        Ok(credentials)
    }
}

/// 从会话里取 JWT 与 user_id。
///
/// `account_id` 为空 → 取该 provider 的当前账号；非空 → 按 id 直取
/// （取不到报错，**不**回落队首 —— 那会把「点名的账号坏了」变成
/// 「静默用了别人的额度」）。
fn session_credentials(
    store: &AccountStore,
    account_id: &str,
) -> Result<(auth::Credentials, Value), GatewayError> {
    let missing = || {
        GatewayError::with_status(401, "没有可用的千问办公账号，请先在「账号」页添加")
    };
    let session = if account_id.trim().is_empty() {
        store.current_entry_for_provider("qwenwork").map(|e| e.session)
    } else {
        store.get_session_by_id(account_id).map(|e| e.session)
    };
    let session = session.ok_or_else(missing)?;
    if session_is_disabled(&session) {
        return Err(GatewayError::with_status(401, "千问办公账号已停用"));
    }
    let creds = credential_from_session(&session)?;
    Ok((creds, session))
}

/// 会话里是否有 `accessToken`（各家的通用字段名）。
fn session_token(session: &Value) -> String {
    session
        .get("auth")
        .and_then(|a| a.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 会话是否被停用（各家的通用字段名）。
fn session_is_disabled(session: &Value) -> bool {
    session
        .get("account")
        .and_then(|a| a.get("disabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 从会话里取 `user_id`（`build_chat_request` 的缓存键）。
///
/// 三个来源按可靠性排序：`auth.userId`（`session_from_record` 直接落的）→
/// `uid`（账号记录的身份字段，顶层被摊进会话）→ `account.uid`。
/// 与 [`credential_from_session`] 读的是同一批字段，故意不合并成一处：
/// 那个函数返回完整凭据（要拼 payload、做校验），这里只要一个键，
/// 合并会让「取一个 id」也背上构造凭据的全部分支。
fn session_user_id(session: &Value) -> String {
    [
        session.pointer("/auth/userId").and_then(Value::as_str),
        session.get("uid").and_then(Value::as_str),
        session.pointer("/account/uid").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .unwrap_or("")
    .to_string()
}

/// 从会话里还原出 [`auth::Credentials`]。
///
/// `AccountStore` 把账号载荷摊在 `auth` 与 `account` 两处，这里只关心
/// 认证所需的那几个字段。`auth.accessToken` 存的就是 **JWT**（与其它家的
/// 「访问令牌」同义，导入时由 [`auth::Credentials::to_payload`] 写入）——
/// MaaS 密钥**不落账号库**，只在进程内缓存（见 `maas_cache`），
/// 因为它是 2 小时短命的派生凭据，落盘反而要处理「文件里的密钥已过期」
/// 这种没有价值的中间态。
fn credential_from_session(session: &Value) -> Result<auth::Credentials, GatewayError> {
    let token = session_token(session);
    if token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "千问办公账号缺少登录令牌，请重新从本机导入",
        ));
    }
    let auth = session.get("auth").cloned().unwrap_or(Value::Null);
    let user_id = auth
        .get("userId")
        .and_then(Value::as_str)
        .or_else(|| auth.pointer("/user/id").and_then(Value::as_str))
        .or_else(|| session.pointer("/account/uid").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    if user_id.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "千问办公账号缺少用户 id，请重新从本机导入",
        ));
    }
    // 用 to_payload 的形状喂回去，让字段解析只有一处定义
    let mut payload = Map::new();
    payload.insert("token".into(), Value::String(token));
    if let Some(rt) = auth.get("refreshToken").and_then(Value::as_str) {
        payload.insert("refreshToken".into(), Value::String(rt.to_string()));
    }
    if let Some(at) = auth.get("expiresAt").and_then(Value::as_i64) {
        payload.insert("expiresAt".into(), Value::Number(at.into()));
    }
    let mut user = Map::new();
    user.insert("id".into(), Value::String(user_id));
    if let Some(name) = auth.get("displayName").and_then(Value::as_str) {
        user.insert("name".into(), Value::String(name.to_string()));
    }
    if let Some(label) = session.pointer("/account/label").and_then(Value::as_str) {
        user.entry("username").or_insert(Value::String(label.to_string()));
    }
    payload.insert("user".into(), Value::Object(user));
    auth::Credentials::from_payload(&Value::Object(payload))
}

impl ProviderAdapter for QwenworkAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Qwenwork
    }

    /// 模型目录来自远程（`GET {gateway}/api/desktop/chat-modes`）。
    ///
    /// 拉取失败时回落到 [`models::fallback_manifest`] 的三条静态记录 ——
    /// 「清单为空」会让入口校验把用户真能用的模型全挡在门外
    /// （`api::pipeline::resolve_model` 以广告视图为准）。
    fn list_models(&self) -> Vec<Value> {
        models::manifest()
    }

    /// 构造上游推理请求：`POST {maas_base}/v1/chat/completions`，Bearer **MaaS 密钥**。
    ///
    /// ── 为什么不能直接读 `auth.accessToken`（这是个真踩过的坑）────────
    /// 编排层（`upstream::provider_loop`）的时序是：
    /// ```text
    ///   ① session = session_for(...)          ← 会话里的 accessToken 是 JWT
    ///   ② adapter.ensure_access_token(...)    ← 只取返回值判成功，**丢弃 token**
    ///   ③ session = session_for(...)          ← 重取会话，accessToken 仍是 JWT
    ///   ④ adapter.build_chat_request(&session, ...)
    /// ```
    /// 第 ② 步的返回值**不会被写回会话**（那是为「回写 store」设计的，
    /// 而本家的 MaaS 密钥刻意不落库）。因此这里若读 `auth.accessToken`，
    /// 拿到的永远是 **JWT** —— 打推理端点就是实测过的那句
    /// `401 unauthorized: unrecognized bearer scheme`。
    ///
    /// ── 正确的取法：读进程内缓存 ────────────────────────────
    /// `ensure_access_token` 已经把（可用的）密钥放进 [`MAAS_CACHE`]，
    /// 键是 `user_id`；会话里带 `uid`（见 `credential_from_session`），
    /// 因此这里**不需要 await** 就能同步取到。缓存未命中意味着第 ② 步
    /// 的签发失败或密钥刚过期 —— 回 401 让编排层走
    /// `refresh_access_token` 重签（那正是本家的自愈路径）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let user_id = session_user_id(account);
        let maas = maas_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&user_id).map(|key| key.clone()));
        let Some(key) = maas.filter(|key| key.is_usable(auth::now_ms())) else {
            return Err(GatewayError::with_status(
                401,
                if user_id.is_empty() {
                    "千问办公账号缺少用户 id，请重新从本机导入"
                } else {
                    "千问办公推理密钥未就绪（签发失败或已过期），请重试"
                },
            ));
        };
        let base = if key.base_url.trim().is_empty() {
            endpoints::DEFAULT_MAAS_BASE
        } else {
            key.base_url.as_str()
        };
        let url = endpoints::chat_completions_url(base);
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
            ("Authorization".to_string(), format!("Bearer {}", key.access_key)),
            ("User-Agent".to_string(), user_agent()),
        ];
        let mut outgoing = body.clone();
        // 流式请求补 `stream_options.include_usage`：上游默认不在流末回用量帧，
        // 不补的话请求日志的输入/输出两列恒为 0（与 zcode 同一处置）。
        ensure_include_usage(&mut outgoing);
        Ok(ChatRequestPlan::chat(url, headers, outgoing))
    }

    /// 上游错误分类。
    ///
    /// ── 本家的错误体形状（实测）─────────────────────────────
    /// MaaS 网关沿用 OpenAI 的 `{"error":{"message","type","code","param"}}`，
    /// 而超时/额度类有时是扁平 `{"message": "...", "code": ...}`。两种都读。
    ///
    /// ── 四档去哪个桶（编排层只认这四档）────────────────────────
    ///   - `401` / `403` → [`UpstreamErrorClass::TokenExpired`]：MaaS 密钥过期
    ///     （2 小时 TTL）或登录态被吊销。编排层会先调 `refresh_access_token`
    ///     重签一次再同账号重试 —— 这正是本家最常见的自愈场景。
    ///   - `429` → [`UpstreamErrorClass::QuotaLimited`]：每日额度用尽。
    ///     上游不给结构化恢复时间（响应里没有 `reset_at`），所以给 `None`
    ///     让编排层走兜底冷却时长。
    ///   - 文案里带额度关键词的（`quota` / `limit exceeded` / `额度`）也算限额 ——
    ///     MaaS 在部分场景用 400/403 表达「额度不足」，只看状态码会漏判。
    ///   - 其余交给共用的内容拦截判定（[`super::super::content_block::classify_or_fatal`]），
    ///     与其它各家同一口径。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let message = upstream_message(error_body);
        let lowered = message.to_lowercase();
        let upstream_code = error_body
            .get("code")
            .and_then(Value::as_i64)
            .or_else(|| error_body.pointer("/error/code").and_then(Value::as_i64));
        let visible = format!("上游返回 {status}: {message}");
        match status {
            401 | 403 => UpstreamErrorClass::TokenExpired { message: visible },
            429 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message: visible,
                upstream_code,
                status,
            },
            _ => {
                if lowered.contains("quota")
                    || lowered.contains("limit exceeded")
                    || lowered.contains("额度")
                    || lowered.contains("insufficient")
                {
                    UpstreamErrorClass::QuotaLimited {
                        reset_at: None,
                        message: visible,
                        upstream_code,
                        status,
                    }
                } else {
                    super::content_block::classify_or_fatal(
                        status,
                        error_body,
                        visible,
                        upstream_code,
                    )
                }
            }
        }
    }

    /// 取可用的 MaaS 密钥（必要时签发）。
    ///
    /// ── 这个方法打破了「先取 token 再构造请求」的常规时序 ─────────
    /// 常规 provider 里这个方法只做「把会话里的 token 读出来」。本家不同：
    /// 会话里存的是 **JWT**（只能打 gateway），而推理要 **MaaS 密钥**。
    /// 因此这里多做一步「用 JWT 签发 MaaS 密钥」，把结果放进进程内缓存，
    /// 并**返回 MaaS 密钥**作为「可用于推理的 token」。
    ///
    /// ── 为什么缓存键是 `user_id` 而不是 `account_id` ────────────
    /// 同一个用户可能被导入两次（不同备注名），而 MaaS 密钥是**用户级**的
    /// （签发时只认 JWT）。按 `user_id` 缓存能让两条账号记录共用一个密钥，
    /// 避免重复签发。反过来按 `account_id` 缓存会在用户重命名账号后失效。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let (creds, _session) = session_credentials(store, account_id)?;
            let now = auth::now_ms();
            // 缓存命中（密钥仍可用）直接返回，省一次签发往返
            {
                let cache = maas_cache()
                    .lock()
                    .map_err(|_| GatewayError::with_status(500, "千问办公密钥缓存锁已中毒"))?;
                if let Some(key) = cache.get(&creds.user_id) {
                    if key.is_usable(now) {
                        return Ok(key.access_key.clone());
                    }
                }
            }
            // 冷路径：先确认 JWT 本身没过期，再签发
            if creds.is_expired(now) {
                return Err(GatewayError::with_status(
                    401,
                    "千问办公登录态已过期，请在客户端重新登录后再从本机导入",
                ));
            }
            let key = self.maas_key_for(&creds.token, &creds.user_id).await?;
            Ok(key.access_key)
        })
    }

    /// 401 后的强制刷新：扔掉缓存密钥，用同一个 JWT 重签一次。
    ///
    /// 与 default 的差别只在「先失效缓存」—— 不失效的话 `ensure_access_token`
    /// 会把那个刚被上游拒掉的密钥原样返回，让「刷新后重试一次」退化成
    /// 一次无意义的重复请求。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let (creds, _session) = session_credentials(store, account_id)?;
            Self::invalidate_maas_cache(Some(&creds.user_id));
            let key = self.mint(&creds.token).await?;
            if let Ok(mut cache) = maas_cache().lock() {
                cache.insert(creds.user_id.clone(), key.clone());
            }
            Ok(key.access_key)
        })
    }

    /// 模型目录有远程来源（`chat-modes`），支持手动刷新。
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 账号级额度池的冷却分组。
    ///
    /// ── 为什么本家要返回整组 ──────────────────────────────────
    /// 额度是**账号级**的一个池子（`usage` 读的是 `data.quota.remaining`，
    /// 各模型按 `price_factor` 从同一个池子里扣，见 `qwenwork::usage`），
    /// 池子耗尽时换哪个模型名都会撞同样的 insufficient quota —— 只记本次
    /// 这一个名，顺延到同账号的下一个模型会再白撞一次。与 CodeArts 的福利池
    /// 同一处境（契约见 `ProviderAdapter::quota_cooldown_models`）。
    ///
    /// 清单取进程内缓存（`models::manifest`，空则内置兜底三件套）：
    /// 没有远程目录可读时那三个名字就是这家已知的全部模型，分组仍然成立。
    fn quota_cooldown_models(&self, _account_id: &str, wire_model: &str) -> Vec<String> {
        let mut names: Vec<String> = models::manifest()
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        // 契约要求分组里含本次真名（请求名解析不出映射时按它记账，见
        // `benefit_cooldown_group` 同款收尾）—— 缓存与线上不同步时兜住
        if !names.iter().any(|name| name == wire_model) {
            names.push(wire_model.to_string());
        }
        names
    }

    /// 拉一次 `chat-modes` 并更新进程内清单。
    ///
    /// ── 为什么 `force` 在这家用不上 ────────────────────────────
    /// 全仓 `refresh_models` 只有一个调用点（`catalog_refresh::refresh`），
    /// 而那条路**先过** `task_state::claim` 的排期闸门（间隔、在途占位、
    /// 失败冷却都在那里判，见那个模块的头部）才走到适配器：能被调到就是
    /// 「这次真的该刷」—— `force = true`（用户点按钮）与 `force = false`
    /// （定时任务到期）到了这里是同一个动作：真打一次上游。
    /// 因此不像 accio / raccoon 那样在适配器里再设一层内存 TTL：那一层对
    /// 经闸门进来的调用是重复挡截，还会把「点了没反应」这种手动路径的故障
    /// 掺进来（`catalog_refresh` 模块头对小浣熊的说明）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        _force: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>,
    > {
        Box::pin(async move {
            let jwt = match session_credentials(store, account_id) {
                Ok((creds, _)) => creds.token,
                Err(err) => return ModelRefreshOutcome::failed(err.message.clone()),
            };
            match models::fetch_and_store(&jwt).await {
                Ok(count) => ModelRefreshOutcome::refreshed(count),
                Err(err) => ModelRefreshOutcome::failed(err.message),
            }
        })
    }

    /// 有余额概念（`GET /api/v2/quota/usage`）。
    fn supports_usage(&self) -> bool {
        true
    }

    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { usage::query(store, account_id).await })
    }
}

/// 流式请求补 `stream_options.include_usage = true`（不覆盖客户端已有取值）。
fn ensure_include_usage(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if object.get("stream").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let options = object
        .entry("stream_options".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(options) = options.as_object_mut() {
        options
            .entry("include_usage".to_string())
            .or_insert_with(|| json!(true));
    }
}

/// 上游按客户端形态识别请求，带上官方 UA 更像真实客户端（免费，且能避免
/// 将来按形态灰度时缺头）。
///
/// 版本号取客户端 `app.getVersion()` 的口径（`qoderwork/1.3.1`）——
/// 硬编码主版本是刻意的：客户端升级时这个值会变，但上游并不校验它，
/// 写死一个已知可用的值比读不到时发空串安全。
fn user_agent() -> String {
    "qoderwork/1.3.1".to_string()
}

/// 日志里截断长文本（避免把整个响应体写进日志）。
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

/// 从上游错误体里取一段可读文案（三种形状都认）。
///
/// 上游 `maas` 走 OpenAI 形状（`error.message`），但超时/路由中间件会回扁平
/// `{"message"}`；`msg` 是少数中文网关的写法。取不到时给一句中性文案 ——
/// 让 `classify_error` 的分类逻辑不必为「没有文案」开分支。
fn upstream_message(error_body: &Value) -> String {
    error_body
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error_body.get("msg").and_then(Value::as_str))
        .or_else(|| error_body.pointer("/error/message").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("上游错误（无详情）")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_usage_added_only_for_streaming() {
        let mut body = json!({"stream": true, "model": "flash"});
        ensure_include_usage(&mut body);
        assert_eq!(body.pointer("/stream_options/include_usage"), Some(&json!(true)));

        let mut non_stream = json!({"model": "flash"});
        ensure_include_usage(&mut non_stream);
        assert!(non_stream.get("stream_options").is_none());
    }

    #[test]
    fn include_usage_does_not_override_client_choice() {
        let mut body = json!({"stream": true, "stream_options": {"include_usage": false}});
        ensure_include_usage(&mut body);
        assert_eq!(
            body.pointer("/stream_options/include_usage"),
            Some(&json!(false))
        );
    }

    #[test]
    fn classify_maps_status_codes() {
        let adapter = QwenworkAdapter;
        let empty = json!({});
        // 四档都是**结构体变体**（它们各自带 message / reset_at 等字段），
        // 所以必须用 `{ .. }` 而不是裸名字匹配
        assert!(matches!(
            adapter.classify_error(401, &empty),
            UpstreamErrorClass::TokenExpired { .. }
        ));
        assert!(matches!(
            adapter.classify_error(429, &empty),
            UpstreamErrorClass::QuotaLimited { .. }
        ));
        // 文案里带额度关键词时，即便状态码是 400 也算限额（见 classify_error 的说明）
        let quota_text = json!({ "error": { "message": "quota exceeded for today" } });
        assert!(matches!(
            adapter.classify_error(400, &quota_text),
            UpstreamErrorClass::QuotaLimited { .. }
        ));
    }
}
